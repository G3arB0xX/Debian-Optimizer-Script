#!/usr/bin/env python3
"""Verify repository hygiene, template integrity, bytecode cleanliness, and vendor neutrality (C3).

Uses Python stdlib only.
"""
import argparse
import glob
import os
import re
import sys

DEFAULT_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))

SKIP_DIRS = {".git", ".jj", ".cache", "__pycache__", "target"}
PYCACHE_SKIP_DIRS = {".git", ".jj", ".cache", "target"}
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
    "archive/legacy-bash/deb_optimizer.sh",
    "install.sh",
    ".github/workflows/check.yml",
    ".github/workflows/release.yml",
)


def _iter_prod_scripts(root: str) -> list[str]:
    out: list[str] = []
    for rel in ("deb_optimizer.sh", "install.sh", "archive/legacy-bash/deb_optimizer.sh"):
        if os.path.isfile(os.path.join(root, rel)):
            out.append(rel)
    for rel_dir in ("scripts", os.path.join("archive", "legacy-bash", "scripts")):
        scripts_dir = os.path.join(root, rel_dir)
        if os.path.isdir(scripts_dir):
            for dirpath, dirnames, filenames in os.walk(scripts_dir):
                dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
                for fn in filenames:
                    if fn.endswith(".sh"):
                        out.append(os.path.relpath(os.path.join(dirpath, fn), root))
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


def check_no_pycache(root: str) -> list[str]:
    errors: list[str] = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in PYCACHE_SKIP_DIRS]
        if "__pycache__" in dirnames:
            rel = os.path.relpath(os.path.join(dirpath, "__pycache__"), root)
            errors.append(f"{rel}: __pycache__ directory forbidden in repository")
            dirnames.remove("__pycache__")
        for fn in filenames:
            if fn.endswith((".pyc", ".pyo")):
                rel = os.path.relpath(os.path.join(dirpath, fn), root)
                errors.append(f"{rel}: compiled Python bytecode forbidden in repository")
    return errors


def check_cursor_debug_logs(root: str) -> list[str]:
    errors: list[str] = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        if os.path.basename(dirpath) == ".cursor":
            for fn in filenames:
                if RE_CURSOR_LOG.match(fn):
                    rel = os.path.relpath(os.path.join(dirpath, fn), root)
                    errors.append(f"{rel}: stale debug log file must be deleted")
    return errors


def check_prod_scripts(prod_scripts: list[str], root: str) -> list[str]:
    errors: list[str] = []
    for rel in prod_scripts:
        abs_path = os.path.join(root, rel)
        raw_lines = open(abs_path, encoding="utf-8", errors="replace").read().splitlines()

        for lineno, logical in _logical_lines(raw_lines):
            if RE_CURSOR_REF.search(logical):
                is_cleanup = bool(re.search(r"\brm\b", logical)) and rel in (
                    "deb_optimizer.sh",
                    "archive/legacy-bash/deb_optimizer.sh",
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
                if not os.path.isfile(os.path.join(root, tpl_rel)):
                    errors.append(
                        f"{rel}:{lineno}: missing template target '{tpl_rel}'"
                    )

    return errors


def check_vibeinstrct_typo(root: str) -> list[str]:
    errors: list[str] = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        rel_dir = os.path.relpath(dirpath, root)
        if rel_dir == "tests" or rel_dir.startswith("tests" + os.sep):
            continue
        for fn in filenames:
            if not fn.endswith((".sh", ".md", ".py", ".yml", ".yaml")):
                continue
            rel = os.path.relpath(os.path.join(dirpath, fn), root)
            if rel == "tools/verify_repo_hygiene.py":
                continue
            abs_path = os.path.join(root, rel)
            lines = open(abs_path, encoding="utf-8", errors="replace").read().splitlines()
            for lineno, line in enumerate(lines, 1):
                if "VIBEINSTRCT.md" not in line:
                    continue
                if rel == "tools/github-push.sh":
                    continue
                if rel == "install.sh" and re.search(r"\brm\b", line):
                    continue
                errors.append(f"{rel}:{lineno}: legacy typo 'VIBEINSTRCT.md' found")
    return errors


def check_shim_mentions(root: str) -> list[str]:
    errors: list[str] = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for fn in filenames:
            if not fn.endswith((".sh", ".md", ".py", ".yml", ".yaml")):
                continue
            rel = os.path.relpath(os.path.join(dirpath, fn), root)
            if rel == "tools/verify_repo_hygiene.py":
                continue
            if rel in SHIM_POINTER_CONTEXTS or rel.startswith("tools/"):
                continue
            if rel in ("governance/CONSTRAINTS.md", "tools/github-push.sh"):
                continue
            abs_path = os.path.join(root, rel)
            lines = open(abs_path, encoding="utf-8", errors="replace").read().splitlines()
            for lineno, line in enumerate(lines, 1):
                if RE_SHIM_MENTION.search(line):
                    errors.append(
                        f"{rel}:{lineno}: CLAUDE.md/GEMINI.md mention outside shim pointer contexts"
                    )
    return errors


def check_vendor_neutrality(root: str) -> list[str]:
    errors: list[str] = []
    has_tests = os.path.isdir(os.path.join(root, "tests"))

    targets: set[str] = set()
    for rel in ("AGENTS.md", "Makefile"):
        if os.path.isfile(os.path.join(root, rel)):
            targets.add(rel)
    for p in glob.glob(os.path.join(root, "tools/*.sh")):
        if os.path.isfile(p):
            targets.add(os.path.relpath(p, root))
    for p in glob.glob(os.path.join(root, "tools/*.py")):
        rel_p = os.path.relpath(p, root)
        if os.path.isfile(p) and rel_p != "tools/verify_repo_hygiene.py":
            targets.add(rel_p)

    if has_tests and os.path.isdir(os.path.join(root, "governance")):
        for pat in ("governance/CONSTRAINTS.md", "governance/constraints/*.md"):
            for p in glob.glob(os.path.join(root, pat)):
                if os.path.isfile(p):
                    targets.add(os.path.relpath(p, root))

    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        if "INDEX.md" in filenames:
            targets.add(os.path.relpath(os.path.join(dirpath, "INDEX.md"), root))

    for rel in sorted(targets):
        abs_path = os.path.join(root, rel)
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


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Verify repository hygiene, template integrity, and vendor neutrality."
    )
    parser.add_argument("--root", default=DEFAULT_ROOT, help="Repository root directory")
    ns = parser.parse_args(argv)
    root = os.path.abspath(ns.root)

    prod_scripts = _iter_prod_scripts(root)
    errors: list[str] = []
    errors.extend(check_no_pycache(root))
    errors.extend(check_cursor_debug_logs(root))
    errors.extend(check_prod_scripts(prod_scripts, root))
    errors.extend(check_vibeinstrct_typo(root))
    errors.extend(check_shim_mentions(root))
    errors.extend(check_vendor_neutrality(root))

    for err in errors:
        print(f"ERROR: {err}")

    print(
        f"verify_repo_hygiene: checked {len(prod_scripts)} prod scripts; "
        f"{len(errors)} error(s)"
    )
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
