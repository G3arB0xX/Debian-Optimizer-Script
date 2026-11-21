#!/usr/bin/env python3
"""Verify dual-audience and agent-only Markdown format contracts (C2).

Uses Python stdlib only so it runs without third-party dependencies.
"""
import glob
import os
import re
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))

REQUIRED_FM_KEYS = ("name", "description", "zh_summary", "tags", "index")
ALLOWED_XML_TAGS = ("guide", "constraints_index", "index", "workflow", "skill")

RE_TABLE_SEP = re.compile(r"^\s*\|[-:| ]+\|\s*$")
RE_MD_HEADING = re.compile(r"^\s*#{1,6}\s+")
RE_MD_BULLET = re.compile(r"^\s*[-*+]\s+")
RE_NESTED_XML = re.compile(r"^\s*</?([A-Za-z_][A-Za-z0-9_-]*)>\s*$")
RE_ZH_SUMMARY_TAG = re.compile(r"</?zh_summary>", re.IGNORECASE)
RE_SECTION_HDR = re.compile(r"^[A-Za-z0-9_]+:$")
RE_KV_LINE = re.compile(r"^  +[A-Za-z0-9_.-]+:(?:\s+.*)?$")
RE_XML_BOUNDARY = re.compile(
    r"</?(?:guide|constraints_index|index|workflow|skill)>"
)
RE_MD_LINK = re.compile(r"\[[^\]]*\]\(([^)]+)\)")
RE_DATE_ISO = re.compile(r"\b20[0-9]{2}-[0-9]{2}-[0-9]{2}\b")
RE_INLINE_CODE = re.compile(r"`[^`]*`")


def _strip_quotes(val: str) -> str:
    val = val.strip()
    if len(val) >= 2 and val[0] == val[-1] and val[0] in ("'", '"'):
        return val[1:-1].strip()
    return val


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


def _is_non_empty(val: object) -> bool:
    if isinstance(val, str):
        return bool(val.strip())
    if isinstance(val, list):
        return any(bool(str(x).strip()) for x in val)
    return bool(val)


def check_agent_file(rel_path: str) -> list[str]:
    """Validate an agent-only Markdown file against C2."""
    errors: list[str] = []
    abs_path = os.path.join(ROOT, rel_path)
    lines = open(abs_path, encoding="utf-8").read().splitlines()

    fm, end_idx, fm_err = _parse_frontmatter(lines)
    if fm_err or fm is None:
        errors.append(f"{rel_path}:1: {fm_err}")
        return errors

    for key in REQUIRED_FM_KEYS:
        if key not in fm or not _is_non_empty(fm[key]):
            errors.append(f"{rel_path}:1: frontmatter missing or empty required key '{key}'")

    if end_idx + 1 >= len(lines) or lines[end_idx + 1].strip() != "":
        errors.append(f"{rel_path}:{end_idx + 2}: expected exactly 1 blank line after closing '---'")
        return errors

    if end_idx + 2 >= len(lines):
        errors.append(f"{rel_path}:{end_idx + 3}: missing outer XML boundary tag")
        return errors

    open_line = lines[end_idx + 2].strip()
    matched_tag = None
    for tag in ALLOWED_XML_TAGS:
        if open_line == f"<{tag}>":
            matched_tag = tag
            break
    if matched_tag is None:
        allowed = ", ".join(f"<{t}>" for t in ALLOWED_XML_TAGS)
        errors.append(
            f"{rel_path}:{end_idx + 3}: expected outer XML tag ({allowed}), got '{open_line}'"
        )
        return errors

    last_non_empty = len(lines) - 1
    while last_non_empty >= 0 and lines[last_non_empty].strip() == "":
        last_non_empty -= 1

    expected_close = f"</{matched_tag}>"
    if last_non_empty <= end_idx + 2 or lines[last_non_empty].strip() != expected_close:
        errors.append(
            f"{rel_path}:{last_non_empty + 1}: last non-empty line must be '{expected_close}'"
        )
        return errors

    for idx in range(end_idx + 3, last_non_empty):
        lineno = idx + 1
        line = lines[idx]
        if not line.strip():
            continue
        if RE_ZH_SUMMARY_TAG.search(line):
            errors.append(f"{rel_path}:{lineno}: <zh_summary> tag forbidden inside XML body")
        if RE_TABLE_SEP.match(line):
            errors.append(f"{rel_path}:{lineno}: Markdown table forbidden inside XML body")
        if RE_MD_HEADING.match(line):
            errors.append(f"{rel_path}:{lineno}: Markdown heading forbidden inside XML body")
        if RE_MD_BULLET.match(line):
            errors.append(f"{rel_path}:{lineno}: Markdown list bullet forbidden inside XML body")
        if RE_NESTED_XML.match(line):
            errors.append(f"{rel_path}:{lineno}: nested XML tag forbidden inside XML body")
        if not (RE_SECTION_HDR.match(line) or RE_KV_LINE.match(line)):
            errors.append(
                f"{rel_path}:{lineno}: expected SECTION: or 2-space indented KEY: value line"
            )

    return errors


def check_human_file(rel_path: str) -> list[str]:
    """Validate a human-facing Markdown file against C2."""
    errors: list[str] = []
    abs_path = os.path.join(ROOT, rel_path)
    file_dir = os.path.dirname(abs_path)
    lines = open(abs_path, encoding="utf-8").read().splitlines()

    in_fence = False
    for idx, line in enumerate(lines, 1):
        stripped = line.strip()
        if RE_XML_BOUNDARY.search(line):
            errors.append(f"{rel_path}:{idx}: agent XML boundary tag forbidden in human-facing doc")

        if stripped.startswith("```") or stripped.startswith("~~~"):
            in_fence = not in_fence
            continue
        if in_fence:
            continue

        cleaned = RE_INLINE_CODE.sub("", line)
        for match in RE_MD_LINK.finditer(cleaned):
            raw_target = match.group(1).strip()
            if not raw_target:
                continue
            target = raw_target.split()[0].strip("<>")
            if target.startswith(("http://", "https://", "mailto:", "#")):
                continue
            path_part = target.split("#", 1)[0].split("?", 1)[0]
            if not path_part.endswith(".md"):
                continue
            if path_part.startswith("/"):
                resolved = os.path.normpath(os.path.join(ROOT, path_part.lstrip("/")))
            else:
                resolved = os.path.normpath(os.path.join(file_dir, path_part))
            if not os.path.isfile(resolved):
                errors.append(f"{rel_path}:{idx}: broken relative .md link '{target}'")

    return errors


def check_date_ban(rel_path: str) -> list[str]:
    """Verify no YYYY-MM-DD modification dates appear in constraint files."""
    errors: list[str] = []
    abs_path = os.path.join(ROOT, rel_path)
    lines = open(abs_path, encoding="utf-8").read().splitlines()
    for idx, line in enumerate(lines, 1):
        m = RE_DATE_ISO.search(line)
        if m:
            errors.append(f"{rel_path}:{idx}: date '{m.group(0)}' forbidden (belongs in decisions.md)")
    return errors


def collect_targets() -> tuple[list[str], list[str], list[str]]:
    agent_files: set[str] = set()
    for rel in ("AGENTS.md", "CLAUDE.md", "GEMINI.md", "instructions/CONSTRAINTS.md"):
        if os.path.isfile(os.path.join(ROOT, rel)):
            agent_files.add(rel)

    for dirpath, dirnames, filenames in os.walk(ROOT):
        dirnames[:] = [d for d in dirnames if d not in (".git", ".jj", ".cache", ".cursor")]
        if "INDEX.md" in filenames:
            rel = os.path.relpath(os.path.join(dirpath, "INDEX.md"), ROOT)
            agent_files.add(rel)

    human_files: set[str] = set()
    for pat in (
        "README.md",
        "README_CN.md",
        "docs/*.md",
        "instructions/constraints/README.md",
        "instructions/constraints/[0-9][0-9]_*.md",
        "instructions/decisions.md",
    ):
        for p in glob.glob(os.path.join(ROOT, pat)):
            if os.path.isfile(p):
                human_files.add(os.path.relpath(p, ROOT))

    date_ban_files: set[str] = set()
    for pat in ("instructions/CONSTRAINTS.md", "instructions/constraints/*.md"):
        for p in glob.glob(os.path.join(ROOT, pat)):
            if os.path.isfile(p):
                date_ban_files.add(os.path.relpath(p, ROOT))

    return sorted(agent_files), sorted(human_files), sorted(date_ban_files)


def main() -> int:
    agent_files, human_files, date_ban_files = collect_targets()
    errors: list[str] = []

    for rel in agent_files:
        errors.extend(check_agent_file(rel))
    for rel in human_files:
        errors.extend(check_human_file(rel))
    for rel in date_ban_files:
        errors.extend(check_date_ban(rel))

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
