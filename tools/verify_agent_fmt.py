#!/usr/bin/env python3
"""Verify dual-audience and agent-only Markdown format contracts (C2 / C-V2-1).

Uses Python stdlib only so it runs without third-party dependencies.
"""
import argparse
import glob
import os
import re
import sys
from urllib.parse import unquote

DEFAULT_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))

REQUIRED_STR_KEYS = ("name", "description", "zh_summary")
REQUIRED_LIST_KEYS = ("tags", "index")
ALLOWED_XML_TAGS = (
    "guide",
    "constraints_index",
    "index",
    "workflow",
    "skill",
    "card",
    "task",
    "protocol",
)

RE_CJK = re.compile(r"[\u4e00-\u9fff]")
RE_INLINE_CODE = re.compile(r"`[^`]*`")
RE_OPEN_OUTER = re.compile(r"<([a-zA-Z_][a-zA-Z0-9_-]*)(?:\s+[^>]*)?>")
RE_INLINE_XML = re.compile(r"</?([a-zA-Z_][a-zA-Z0-9_-]*)(?:\s+[^>]*)?>")
RE_TABLE_SEP = re.compile(r"^\s*\|[\s:-]+\|")
RE_MD_HEADING = re.compile(r"^\s*#{1,6}\s+")
RE_MD_BULLET = re.compile(r"^\s*[-*+]\s+")
RE_ZH_SUMMARY_TAG = re.compile(r"</?zh_summary\b", re.IGNORECASE)
RE_SECTION_HDR = re.compile(r"^[A-Z][A-Z0-9_]*:\s*$")
RE_KV_LINE = re.compile(r"^  +[A-Za-z0-9_.-]+:(?:\s+.*)?$")
RE_XML_BOUNDARY = re.compile(
    r"</?(?:guide|constraints_index|index|workflow|skill|card|task|protocol|zh_summary)\b",
    re.IGNORECASE,
)
RE_BANNED_SEP = re.compile(r" · |\]·\[|｜")
RE_MD_LINK = re.compile(r"\[[^\]]*\]\(([^)]+)\)")
RE_DATE_ISO = re.compile(r"\b20[0-9]{2}-[0-9]{2}-[0-9]{2}\b")
SKIP_DIRNAMES = {".git", ".jj", ".cache", ".cursor", "__pycache__", "target"}


def _strip_quotes(val: str) -> str:
    val = val.strip()
    if len(val) >= 2 and val[0] == val[-1] and val[0] in ("'", '"'):
        return val[1:-1].strip()
    return val


def _strip_inline_code(line: str) -> str:
    return RE_INLINE_CODE.sub("", line)


def _parse_frontmatter(lines: list[str]) -> tuple[dict[str, object] | None, int, str | None]:
    """Parse minimal YAML frontmatter without external dependencies."""
    if not lines or lines[0].strip() != "---":
        return None, -1, "line 1 must be '---'"

    end_idx = -1
    for i in range(1, len(lines)):
        if lines[i].strip() == "---":
            end_idx = i
            break
    if end_idx < 0:
        return None, -1, "unclosed YAML frontmatter (missing closing '---')"

    data: dict[str, object] = {}
    current_list_key: str | None = None
    for raw in lines[1:end_idx]:
        stripped = raw.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if stripped.startswith("- ") and current_list_key is not None:
            item = _strip_quotes(stripped[2:])
            if item:
                lst = data.setdefault(current_list_key, [])
                if isinstance(lst, list):
                    lst.append(item)
            continue
        current_list_key = None
        if ":" not in raw:
            continue
        key, val = raw.split(":", 1)
        key = key.strip()
        val = val.strip()
        if not val:
            data[key] = []
            current_list_key = key
        elif val.startswith("[") and val.endswith("]"):
            inner = val[1:-1].strip()
            items = [_strip_quotes(x) for x in inner.split(",") if _strip_quotes(x)]
            data[key] = items
        else:
            data[key] = _strip_quotes(val)

    return data, end_idx, None


def _resolve_index_item(item: str, file_dir: str, root: str) -> str:
    if item.startswith(("./", "../")):
        return os.path.normpath(os.path.join(file_dir, item))
    return os.path.normpath(os.path.join(root, item))


def _extract_relative_links(abs_path: str, root: str) -> set[str]:
    """Extract normalized target paths for all relative Markdown links outside code."""
    file_dir = os.path.dirname(abs_path)
    lines = open(abs_path, encoding="utf-8").read().splitlines()
    resolved_links: set[str] = set()
    in_fence = False
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("```") or stripped.startswith("~~~"):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        cleaned = _strip_inline_code(line)
        for match in RE_MD_LINK.finditer(cleaned):
            raw_target = match.group(1).strip()
            if not raw_target:
                continue
            target = unquote(raw_target.split()[0].strip("<>"))
            if target.startswith(("http://", "https://", "mailto:", "file://", "#")):
                continue
            path_part = target.split("#", 1)[0].split("?", 1)[0]
            if not path_part:
                continue
            if path_part.startswith("/"):
                resolved = os.path.normpath(os.path.join(root, path_part.lstrip("/")))
            else:
                resolved = os.path.normpath(os.path.join(file_dir, path_part))
            resolved_links.add(resolved)
    return resolved_links


def check_agent_file(rel_path: str, root: str) -> tuple[list[str], dict[str, object] | None]:
    """Validate an agent-only Markdown file against C-V2-1."""
    errors: list[str] = []
    abs_path = os.path.join(root, rel_path)
    file_dir = os.path.dirname(abs_path)
    lines = open(abs_path, encoding="utf-8").read().splitlines()

    fm, end_idx, fm_err = _parse_frontmatter(lines)
    if fm_err or fm is None:
        errors.append(f"{rel_path}:1: {fm_err}")
        return errors, None

    for key in REQUIRED_STR_KEYS:
        val = fm.get(key)
        if not isinstance(val, str) or not val.strip():
            errors.append(f"{rel_path}:1: frontmatter missing non-empty string '{key}'")

    zh_val = fm.get("zh_summary")
    if isinstance(zh_val, str) and zh_val.strip() and not RE_CJK.search(zh_val):
        errors.append(f"{rel_path}:1: frontmatter 'zh_summary' must contain CJK characters")

    for key in REQUIRED_LIST_KEYS:
        val = fm.get(key)
        if not isinstance(val, list) or len(val) == 0 or not all(
            isinstance(x, str) and x.strip() for x in val
        ):
            errors.append(f"{rel_path}:1: frontmatter '{key}' must be a non-empty list")

    idx_val = fm.get("index")
    if isinstance(idx_val, list):
        for item in idx_val:
            if isinstance(item, str) and item.strip() and "*" not in item:
                resolved = _resolve_index_item(item, file_dir, root)
                if not os.path.exists(resolved):
                    errors.append(
                        f"{rel_path}:1: frontmatter index path missing on disk: '{item}'"
                    )

    idx = end_idx + 1
    if idx >= len(lines) or lines[idx].strip() != "":
        errors.append(f"{rel_path}:{idx + 1}: expected blank line after closing '---'")
    while idx < len(lines) and lines[idx].strip() == "":
        idx += 1

    outer_count = 0
    in_outer: str | None = None
    first_line_in_outer = False

    for i in range(idx, len(lines)):
        lineno = i + 1
        line = lines[i]
        stripped = line.strip()
        cleaned = _strip_inline_code(line)

        if RE_ZH_SUMMARY_TAG.search(cleaned):
            errors.append(f"{rel_path}:{lineno}: <zh_summary> tag forbidden in body")

        if RE_TABLE_SEP.match(line) or stripped.startswith("---:"):
            errors.append(f"{rel_path}:{lineno}: Markdown table forbidden inside XML body")

        if in_outer is None:
            if stripped == "":
                continue
            m = RE_OPEN_OUTER.fullmatch(stripped)
            if m and m.group(1) in ALLOWED_XML_TAGS:
                in_outer = m.group(1)
                outer_count += 1
                first_line_in_outer = True
            else:
                errors.append(
                    f"{rel_path}:{lineno}: unexpected content outside outer XML tag: '{stripped[:40]}'"
                )
        else:
            if stripped == f"</{in_outer}>":
                in_outer = None
                first_line_in_outer = False
                continue

            if stripped == "":
                first_line_in_outer = False
                continue

            for m in RE_INLINE_XML.finditer(cleaned):
                errors.append(
                    f"{rel_path}:{lineno}: nested XML tag <{m.group(1)}> inside <{in_outer}>"
                )

            if RE_MD_HEADING.match(line):
                errors.append(f"{rel_path}:{lineno}: Markdown heading forbidden inside XML body")
            if RE_MD_BULLET.match(line):
                errors.append(f"{rel_path}:{lineno}: Markdown list bullet forbidden inside XML body")

            if RE_SECTION_HDR.match(line):
                if not first_line_in_outer and i > 0 and lines[i - 1].strip() != "":
                    errors.append(
                        f"{rel_path}:{lineno}: missing blank line before section '{stripped}'"
                    )
            elif not RE_KV_LINE.match(line):
                errors.append(
                    f"{rel_path}:{lineno}: expected ALL-CAPS SECTION: or 2-space indented KEY: value line"
                )

            first_line_in_outer = False

    if in_outer is not None:
        errors.append(f"{rel_path}: unclosed outer tag <{in_outer}>")
    if outer_count < 1:
        errors.append(f"{rel_path}: expected >= 1 outer XML block")

    return errors, fm


def check_human_file(rel_path: str, root: str) -> list[str]:
    """Validate a human-facing Markdown file against C-V2-1."""
    errors: list[str] = []
    abs_path = os.path.join(root, rel_path)
    file_dir = os.path.dirname(abs_path)
    rel_posix = rel_path.replace(os.sep, "/")
    check_sep = (
        rel_posix.startswith(".agents/constraints/")
        or rel_posix.startswith(".agents/memory/")
        or rel_posix.startswith(".agents/overrides/")
        or rel_posix.startswith(".agents/schemas/")
        or rel_posix.startswith("docs/")
    )
    lines = open(abs_path, encoding="utf-8").read().splitlines()

    in_fence = False
    for idx, line in enumerate(lines, 1):
        stripped = line.strip()
        if stripped.startswith("```") or stripped.startswith("~~~"):
            in_fence = not in_fence
            continue
        if in_fence:
            continue

        cleaned = _strip_inline_code(line)
        if RE_XML_BOUNDARY.search(cleaned):
            errors.append(f"{rel_path}:{idx}: agent XML boundary tag forbidden in human-facing doc")

        if check_sep and RE_BANNED_SEP.search(cleaned):
            errors.append(f"{rel_path}:{idx}: banned separator ( · / ]·[ / ｜) forbidden")

        for match in RE_MD_LINK.finditer(cleaned):
            raw_target = match.group(1).strip()
            if not raw_target:
                continue
            target = unquote(raw_target.split()[0].strip("<>"))
            if target.startswith(("http://", "https://", "mailto:", "file://", "#")):
                continue
            path_part = target.split("#", 1)[0].split("?", 1)[0]
            if not path_part.endswith(".md"):
                continue
            if path_part.startswith("/"):
                resolved = os.path.normpath(os.path.join(root, path_part.lstrip("/")))
            else:
                resolved = os.path.normpath(os.path.join(file_dir, path_part))
            if not os.path.isfile(resolved):
                errors.append(f"{rel_path}:{idx}: broken relative .md link '{target}'")

    return errors


def check_bidirectional_readme_index(
    agent_files: list[str], fm_cache: dict[str, dict[str, object] | None], root: str
) -> list[str]:
    """Verify D/README.md <-> D/INDEX.md bidirectional links wherever both exist."""
    errors: list[str] = []
    for rel_idx in agent_files:
        if os.path.basename(rel_idx) != "INDEX.md":
            continue
        idx_abs = os.path.normpath(os.path.join(root, rel_idx))
        idx_dir = os.path.dirname(idx_abs)
        readme_abs = os.path.normpath(os.path.join(idx_dir, "README.md"))
        if not os.path.isfile(readme_abs):
            continue

        readme_rel = os.path.relpath(readme_abs, root)
        readme_links = _extract_relative_links(readme_abs, root)
        if idx_abs not in readme_links:
            errors.append(f"{readme_rel}: missing bidirectional link to sibling '{rel_idx}'")

        fm = fm_cache.get(rel_idx)
        idx_targets: set[str] = set()
        if isinstance(fm, dict) and isinstance(fm.get("index"), list):
            for item in fm["index"]:
                if isinstance(item, str) and item.strip() and "*" not in item:
                    idx_targets.add(_resolve_index_item(item, idx_dir, root))
        if readme_abs not in idx_targets:
            errors.append(
                f"{rel_idx}: frontmatter 'index' missing bidirectional reference to '{readme_rel}'"
            )
    return errors


def check_root_bilingual_links(root: str) -> list[str]:
    """Verify root README.md <-> README_CN.md link to each other when both exist."""
    errors: list[str] = []
    en_abs = os.path.normpath(os.path.join(root, "README.md"))
    cn_abs = os.path.normpath(os.path.join(root, "README_CN.md"))
    if os.path.isfile(en_abs) and os.path.isfile(cn_abs):
        if cn_abs not in _extract_relative_links(en_abs, root):
            errors.append("README.md: missing bilingual link to 'README_CN.md'")
        if en_abs not in _extract_relative_links(cn_abs, root):
            errors.append("README_CN.md: missing bilingual link to 'README.md'")
    return errors


def check_date_ban(rel_path: str, root: str) -> list[str]:
    """Verify no YYYY-MM-DD modification dates appear in constraint files."""
    errors: list[str] = []
    abs_path = os.path.join(root, rel_path)
    rel_posix = rel_path.replace(os.sep, "/")
    skip_fences = rel_posix == ".agents/constraints/90_project_legacy_ssot.md"
    lines = open(abs_path, encoding="utf-8").read().splitlines()
    in_fence = False
    for idx, line in enumerate(lines, 1):
        if skip_fences:
            stripped = line.strip()
            if stripped.startswith("```") or stripped.startswith("~~~"):
                in_fence = not in_fence
                continue
            if in_fence:
                continue
        m = RE_DATE_ISO.search(line)
        if m:
            errors.append(f"{rel_path}:{idx}: date '{m.group(0)}' forbidden")
    return errors


def collect_targets(root: str) -> tuple[list[str], list[str], list[str]]:
    agent_files: set[str] = set()
    for rel in ("AGENTS.md", "CLAUDE.md", "GEMINI.md", ".agents/CONSTRAINTS.md"):
        if os.path.isfile(os.path.join(root, rel)):
            agent_files.add(rel)

    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRNAMES]
        if "INDEX.md" in filenames:
            rel = os.path.relpath(os.path.join(dirpath, "INDEX.md"), root)
            agent_files.add(rel)

    human_files: set[str] = set()
    for pat in (
        "README.md",
        "README_CN.md",
        "MEMORY.md",
        "docs/*.md",
        "tests/README.md",
        ".agents/README.md",
        ".agents/MEMORY.md",
        ".agents/constraints/README.md",
        ".agents/constraints/[0-9][0-9]_*.md",
        ".agents/memory/README.md",
        ".agents/memory/*.md",
        ".agents/memory/*/*.md",
        ".agents/schemas/README.md",
        ".agents/overrides/README.md",
        ".agents/overrides/_template.md",
        ".agents/overrides/*.md",
    ):
        for p in glob.glob(os.path.join(root, pat)):
            if os.path.isfile(p) and os.path.basename(p) != "INDEX.md":
                human_files.add(os.path.relpath(p, root))

    date_ban_files: set[str] = set()
    for pat in (".agents/CONSTRAINTS.md", ".agents/constraints/*.md"):
        for p in glob.glob(os.path.join(root, pat)):
            if os.path.isfile(p):
                date_ban_files.add(os.path.relpath(p, root))

    return sorted(agent_files), sorted(human_files), sorted(date_ban_files)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Verify dual-audience and agent-only Markdown format contracts."
    )
    parser.add_argument("--root", default=DEFAULT_ROOT, help="Repository root directory")
    ns = parser.parse_args(argv)
    root = os.path.abspath(ns.root)

    agent_files, human_files, date_ban_files = collect_targets(root)
    errors: list[str] = []
    fm_cache: dict[str, dict[str, object] | None] = {}

    for rel in agent_files:
        file_errs, fm = check_agent_file(rel, root)
        errors.extend(file_errs)
        fm_cache[rel] = fm
    for rel in human_files:
        errors.extend(check_human_file(rel, root))
    errors.extend(check_bidirectional_readme_index(agent_files, fm_cache, root))
    errors.extend(check_root_bilingual_links(root))
    for rel in date_ban_files:
        errors.extend(check_date_ban(rel, root))

    for err in errors:
        print(f"ERROR: {err}")

    print(
        f"verify_agent_fmt: checked {len(agent_files)} agent, "
        f"{len(human_files)} human, {len(date_ban_files)} date-ban files; "
        f"{len(errors)} error(s)"
    )
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
