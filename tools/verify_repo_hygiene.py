#!/usr/bin/env python3
"""Verify repository hygiene, template integrity, and vendor neutrality (C3).

Uses Python stdlib only.
"""
import glob
import os
import re
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))

SKIP_DIRS = {".git", ".jj", ".cache", "__pycache__"}
RE_CURSOR_LOG = re.compile(r"^debug-.*\.log$")
RE_CURSOR_REF = re.compile(r"\.cursor\b")
RE_JUJUTSU = re.compile(r"\bjujutsu\b", re.IGNORECASE)
RE_RENDER_TPL = re.compile(r'render_template\s+["\'](templates/[^"\']+)["\']')
RE_SHIM_MENTION = re.compile(r"\b(?:CLAUDE|GEMINI)\.md\b")
RE_VENDOR_BRANDS = re.compile(
    r"(?i)(?:\.cursor\b|\bCursor\b|\bclaude(?:-code)?\b|\bgemini\b|\bantigravity\b|\bwindsurf\b|\bcline\b|\bcopilot\b)"
)

SHIM_POINTER_CONTEXTS = (
    "AGENTS.md",
    "CLAUDE.md",
    "GEMINI.md",
    "Makefile",
    "deb_optimizer.sh",
    "install.sh",
    ".github/workflows/check.yml",
    ".github/workflows/release.yml",
)


def _iter_prod_scripts() -> list[str]:
    out: list[str] = []
    for rel in ("deb_optimizer.sh", "install.sh"):
        if os.path.isfile(os.path.join(ROOT, rel)):
            out.append(rel)
    scripts_dir = os.path.join(ROOT, "scripts")
    if os.path.isdir(scripts_dir):
        for dirpath, dirnames, filenames in os.walk(scripts_dir):
            dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
            for fn in filenames:
                if fn.endswith(".sh"):
                    out.append(os.path.relpath(os.path.join(dirpath, fn), ROOT))
    return sorted(out)


def _logical_lines(lines: list[str]) -> list[tuple[int, str]]:
    """Join backslash-continued shell lines while keeping starting 1-based line numbers."""
    result: list[tuple[int, str]] = []
    buf: list[str] = []
    start_lineno = 1
    for idx, line in enumerate(lines, 1):
        if not buf:
            start_lineno = idx
        if line.rstrip().endswith("\\"):
            buf.append(line.rstrip()[:-1])
        else:
            buf.append(line)
            result.append((start_lineno, " ".join(buf)))
            buf = []
    if buf:
        result.append((start_lineno, " ".join(buf)))
    return result


def check_cursor_debug_logs() -> list[str]:
    errors: list[str] = []
    for dirpath, dirnames, filenames in os.walk(ROOT):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        if os.path.basename(dirpath) == ".cursor":
            for fn in filenames:
                if RE_CURSOR_LOG.match(fn):
                    rel = os.path.relpath(os.path.join(dirpath, fn), ROOT)
                    errors.append(f"{rel}: stale debug log file must be deleted")
    return errors


def check_prod_scripts(prod_scripts: list[str]) -> list[str]:
    errors: list[str] = []
    for rel in prod_scripts:
        abs_path = os.path.join(ROOT, rel)
        raw_lines = open(abs_path, encoding="utf-8", errors="replace").read().splitlines()

        for lineno, logical in _logical_lines(raw_lines):
            if RE_CURSOR_REF.search(logical):
                is_cleanup = bool(re.search(r"\brm\b", logical)) and rel in (
                    "deb_optimizer.sh",
                    "install.sh",
                )
                if not is_cleanup:
                    errors.append(
                        f"{rel}:{lineno}: '.cursor' reference forbidden in production scripts"
                    )

        for lineno, line in enumerate(raw_lines, 1):
            if RE_JUJUTSU.search(line):
                errors.append(f"{rel}:{lineno}: 'jujutsu' term forbidden in production scripts")

            for match in RE_RENDER_TPL.finditer(line):
                tpl_rel = match.group(1)
                if "$" in tpl_rel:
                    continue
                if not os.path.isfile(os.path.join(ROOT, tpl_rel)):
                    errors.append(
                        f"{rel}:{lineno}: missing template target '{tpl_rel}'"
                    )

    return errors


def check_vibeinstrct_typo() -> list[str]:
    errors: list[str] = []
    for dirpath, dirnames, filenames in os.walk(ROOT):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        rel_dir = os.path.relpath(dirpath, ROOT)
        if rel_dir == "tests" or rel_dir.startswith("tests" + os.sep):
            continue
        for fn in filenames:
            if not fn.endswith((".sh", ".md", ".py", ".yml", ".yaml")):
                continue
            rel = os.path.relpath(os.path.join(dirpath, fn), ROOT)
            if rel == "tools/verify_repo_hygiene.py":
                continue
            abs_path = os.path.join(ROOT, rel)
            lines = open(abs_path, encoding="utf-8", errors="replace").read().splitlines()
            for lineno, line in enumerate(lines, 1):
                if "VIBEINSTRCT.md" not in line:
                    continue
                if rel == "instructions/github-push.sh":
                    continue
                if rel == "install.sh" and re.search(r"\brm\b", line):
                    continue
                errors.append(f"{rel}:{lineno}: legacy typo 'VIBEINSTRCT.md' found")
    return errors


def check_shim_mentions() -> list[str]:
    errors: list[str] = []
    for dirpath, dirnames, filenames in os.walk(ROOT):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for fn in filenames:
            if not fn.endswith((".sh", ".md", ".py", ".yml", ".yaml")):
                continue
            rel = os.path.relpath(os.path.join(dirpath, fn), ROOT)
            if rel == "tools/verify_repo_hygiene.py":
                continue
            if rel in SHIM_POINTER_CONTEXTS or rel.startswith("tools/"):
                continue
            if rel in ("instructions/CONSTRAINTS.md", "instructions/decisions.md", "instructions/github-push.sh"):
                continue
            abs_path = os.path.join(ROOT, rel)
            lines = open(abs_path, encoding="utf-8", errors="replace").read().splitlines()
            for lineno, line in enumerate(lines, 1):
                if RE_SHIM_MENTION.search(line):
                    errors.append(
                        f"{rel}:{lineno}: CLAUDE.md/GEMINI.md mention outside shim pointer contexts"
                    )
    return errors


def check_vendor_neutrality() -> list[str]:
    errors: list[str] = []
    has_tests = os.path.isdir(os.path.join(ROOT, "tests"))

    targets: set[str] = set()
    for rel in ("AGENTS.md", "Makefile"):
        if os.path.isfile(os.path.join(ROOT, rel)):
            targets.add(rel)
    for p in glob.glob(os.path.join(ROOT, "tools/*.sh")):
        if os.path.isfile(p):
            targets.add(os.path.relpath(p, ROOT))
    for p in glob.glob(os.path.join(ROOT, "tools/*.py")):
        rel_p = os.path.relpath(p, ROOT)
        if os.path.isfile(p) and rel_p != "tools/verify_repo_hygiene.py":
            targets.add(rel_p)

    if has_tests and os.path.isdir(os.path.join(ROOT, "instructions")):
        for pat in ("instructions/CONSTRAINTS.md", "instructions/constraints/*.md"):
            for p in glob.glob(os.path.join(ROOT, pat)):
                if os.path.isfile(p):
                    targets.add(os.path.relpath(p, ROOT))

    for rel in sorted(targets):
        abs_path = os.path.join(ROOT, rel)
        lines = open(abs_path, encoding="utf-8", errors="replace").read().splitlines()
        for lineno, line in enumerate(lines, 1):
            sanitized = RE_SHIM_MENTION.sub("", line)
            if ".cursor" in sanitized and any(
                k in sanitized.lower() for k in ("cleanup", "清理", "rm -rf", ".gitignore", "dirnames")
            ):
                sanitized = sanitized.replace(".cursor", "")
            m = RE_VENDOR_BRANDS.search(sanitized)
            if m:
                errors.append(
                    f"{rel}:{lineno}: stray vendor/platform reference '{m.group(0)}'"
                )
    return errors


def main() -> int:
    prod_scripts = _iter_prod_scripts()
    errors: list[str] = []
    errors.extend(check_cursor_debug_logs())
    errors.extend(check_prod_scripts(prod_scripts))
    errors.extend(check_vibeinstrct_typo())
    errors.extend(check_shim_mentions())
    errors.extend(check_vendor_neutrality())

    for err in errors:
        print(f"ERROR: {err}")

    print(
        f"verify_repo_hygiene: checked {len(prod_scripts)} prod scripts; "
        f"{len(errors)} error(s)"
    )
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
