#!/usr/bin/env python3
"""Verify repository hygiene, template integrity, bytecode cleanliness, and vendor neutrality (C3).

Uses Python stdlib only.
"""
import argparse
import glob
import os
import re
import stat
import subprocess
import sys
import threading

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

MAX_SECRET_SCAN_BYTES = 2 * 1024 * 1024

URL_AUTH_ALLOW_SUBSTRS = ("[REDACTED]", "${", "<", "user:pass", "USER:PASS")
SECRET_PLACEHOLDER_SUBSTRS = ("{{", "${", "<", "[REDACTED]")

FORBIDDEN_CRED_BASENAMES = frozenset(
    {
        ".env",
        "id_rsa",
        "id_ed25519",
        "id_ecdsa",
        "wgcf-account.toml",
        "credentials.json",
        "secrets.json",
        "secrets.yaml",
        "secrets.yml",
    }
)
FORBIDDEN_CRED_SUFFIXES = (".cred", ".pem", ".key", ".p12", ".pfx", ".jks")
RE_SYSTEMD_SECTION = re.compile(r"^\s*\[([^\]]+)\]\s*$")
RE_SYSTEMD_SET_CRED = re.compile(r"^\s*SetCredential\s*=\s*([^:\s]+):(.*)$")
RE_SYSTEMD_ENV_LINE = re.compile(r"^\s*Environment\s*=\s*(.+)$")
RE_SYSTEMD_ENV_SECRET = re.compile(
    r"(?:^|[\s\"'])([A-Z0-9_]*(?:PASSWORD|TOKEN|SECRET|API_KEY))=([^\s\"']*)"
)

SECRET_PATTERNS: tuple[tuple[str, re.Pattern[str]], ...] = (
    (
        "pem_private_key",
        re.compile(r"-----" + r"BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----"),
    ),
    (
        "tailscale_key",
        re.compile(r"\btskey" + r"-(?:auth|api|client)-[A-Za-z0-9_-]{6,}"),
    ),
    (
        "github_token",
        re.compile(
            r"\b(?:gh[pousr]_" + r"[A-Za-z0-9]{20,}|github_pat_" + r"[A-Za-z0-9_]{20,})"
        ),
    ),
    (
        "gitlab_token",
        re.compile(r"\bglpat" + r"-[A-Za-z0-9_-]{20,}"),
    ),
    (
        "aws_access_key",
        re.compile(r"\b(?:AKIA|ASIA)" + r"[0-9A-Z]{16}\b"),
    ),
    (
        "slack_token",
        re.compile(r"\bxox[baprs]" + r"-[A-Za-z0-9-]{10,}"),
    ),
    (
        "slack_webhook",
        re.compile(
            r"https://hooks\.slack\.com/" + r"services/T[A-Z0-9]+/B[A-Z0-9]+/[A-Za-z0-9]+"
        ),
    ),
    (
        "discord_webhook",
        re.compile(
            r"https://(?:discord|discordapp)\.com/api/" + r"webhooks/[0-9]+/[A-Za-z0-9_-]+"
        ),
    ),
    (
        "jwt_literal",
        re.compile(r"\beyJ" + r"[A-Za-z0-9_-]{10,}(?:\.[A-Za-z0-9_-]{3,})?"),
    ),
    (
        "provider_sk_key",
        re.compile(
            r"\b(?:sk-(?!ssh-|ecdsa-)(?:proj-|live-|test-|ant-)?[A-Za-z0-9_-]{16,}|"
            r"sk_(?:live|test)_[A-Za-z0-9]{12,})"
        ),
    ),
    (
        "url_basic_auth",
        re.compile(r"https?://[^/\s:@\"'`<>]+:[^/\s:@\"'`<>]+@[^\s\"'`<>]+"),
    ),
    (
        "cloudflare_api_token",
        re.compile(
            r"CLOUDFLARE"
            + r"_(?:DNS_)?API_TOKEN\s*=\s*[\"']?(?!\{\{|\$\{|<|\[REDACTED\])[A-Za-z0-9_-]{32,}[\"']?"
        ),
    ),
    (
        "age_secret_key",
        re.compile(r"\bAGE-SECRET" + r"-KEY-1[QPZRY9X8GF2TVDW0S3JN54KHCE6MUA7L]{58}\b"),
    ),
    (
        "telegram_bot_token",
        re.compile(r"\b[0-9]{8,10}:" + r"AA[A-Za-z0-9_-]{33}\b"),
    ),
    (
        "stripe_live_key",
        re.compile(r"\b(?:rk|pk)" + r"_live_[A-Za-z0-9]{20,}\b"),
    ),
    (
        "bearer_token_literal",
        re.compile(
            r"Authorization:\s*Bear" + r"er\s+(?!\[REDACTED\]|\$\{|\{\{|<)[A-Za-z0-9._~+/-]{16,}",
            re.IGNORECASE,
        ),
    ),
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
            if rel in (".agents/CONSTRAINTS.md", "tools/github-push.sh"):
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

    if os.path.isdir(os.path.join(root, ".agents")):
        for pat in (".agents/CONSTRAINTS.md", ".agents/constraints/*.md"):
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


def _is_placeholder_value(val: str) -> bool:
    stripped = val.strip().strip("\"'")
    if not stripped:
        return True
    return any(tok in stripped for tok in SECRET_PLACEHOLDER_SUBSTRS)


def check_forbidden_credential_files(root: str, workspace_mode: bool = False) -> list[str]:
    """Reject forbidden credential filenames/extensions outside sanctioned workspace store."""
    errors: list[str] = []
    for dirpath, dirnames, filenames in os.walk(root):
        rel_dir = os.path.relpath(dirpath, root).replace(os.sep, "/")
        next_dirs: list[str] = []
        for d in sorted(dirnames):
            if d in SKIP_DIRS:
                continue
            child_rel = d if rel_dir == "." else f"{rel_dir}/{d}"
            if workspace_mode and child_rel == ".agents/credentials":
                continue
            next_dirs.append(d)
        dirnames[:] = next_dirs

        for fn in sorted(filenames):
            rel = fn if rel_dir == "." else f"{rel_dir}/{fn}"
            if workspace_mode and rel.startswith(".agents/credentials/"):
                continue
            if (
                fn in FORBIDDEN_CRED_BASENAMES
                or fn.startswith(".env.")
                or fn.endswith(FORBIDDEN_CRED_SUFFIXES)
            ):
                errors.append(f"{rel}: forbidden credential file in repository")
    return errors


def check_systemd_credential_hygiene(root: str) -> list[str]:
    """Reject inline plaintext secrets in systemd [Service] template directives."""
    errors: list[str] = []
    tpl_root = os.path.join(root, "templates")
    if not os.path.isdir(tpl_root):
        return errors

    for dirpath, dirnames, filenames in os.walk(tpl_root):
        dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
        for fn in sorted(filenames):
            if not (
                fn.endswith(".service")
                or fn.endswith(".service.override.conf")
                or fn.endswith(".conf")
            ):
                continue
            abs_path = os.path.join(dirpath, fn)
            rel = os.path.relpath(abs_path, root).replace(os.sep, "/")
            try:
                lines = open(abs_path, encoding="utf-8", errors="replace").read().splitlines()
            except OSError:
                continue

            in_service = False
            for lineno, line in enumerate(lines, 1):
                stripped = line.strip()
                if not stripped or stripped.startswith(("#", ";")):
                    continue
                sec_m = RE_SYSTEMD_SECTION.match(line)
                if sec_m:
                    in_service = sec_m.group(1).strip() == "Service"
                    continue
                if not in_service:
                    continue

                cred_m = RE_SYSTEMD_SET_CRED.match(line)
                if cred_m and not _is_placeholder_value(cred_m.group(2)):
                    errors.append(
                        f"{rel}:{lineno}: plaintext SetCredential literal forbidden in systemd [Service]"
                    )
                    continue

                env_m = RE_SYSTEMD_ENV_LINE.match(line)
                if env_m:
                    for sm in RE_SYSTEMD_ENV_SECRET.finditer(env_m.group(1)):
                        if not _is_placeholder_value(sm.group(2)):
                            errors.append(
                                f"{rel}:{lineno}: plaintext secret in systemd Environment={sm.group(1)}"
                            )
                            break
    return errors


def check_workspace_perms(root: str) -> list[str]:
    """Verify strict 0700/0600 permissions on provisioned VPS agent workspace paths."""
    errors: list[str] = []
    dir_checks = (
        (".agents/credentials", 0o700),
        (".agents/checkpoints", 0o700),
        (".agents/logs", 0o700),
    )
    file_checks = (
        (".agents/logs/audit.jsonl", 0o600),
        ("MEMORY.md", 0o600),
    )

    for rel, expected in dir_checks:
        abs_path = os.path.join(root, rel)
        if os.path.isdir(abs_path):
            mode = stat.S_IMODE(os.stat(abs_path).st_mode)
            if mode != expected:
                errors.append(
                    f"{rel}: expected directory mode {oct(expected)}, got {oct(mode)}"
                )

    for rel, expected in file_checks:
        abs_path = os.path.join(root, rel)
        if os.path.isfile(abs_path):
            mode = stat.S_IMODE(os.stat(abs_path).st_mode)
            if mode != expected:
                errors.append(
                    f"{rel}: expected file mode {oct(expected)}, got {oct(mode)}"
                )

    cred_dir = os.path.join(root, ".agents", "credentials")
    if os.path.isdir(cred_dir):
        for dirpath, dirnames, filenames in os.walk(cred_dir):
            dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
            for fn in sorted(filenames):
                abs_path = os.path.join(dirpath, fn)
                if os.path.islink(abs_path) or not os.path.isfile(abs_path):
                    continue
                mode = stat.S_IMODE(os.stat(abs_path).st_mode)
                if mode != 0o600:
                    rel = os.path.relpath(abs_path, root).replace(os.sep, "/")
                    errors.append(
                        f"{rel}: expected credential file mode 0o600, got {oct(mode)}"
                    )
    return errors


def scan_text_for_secrets(text: str, label: str) -> list[str]:
    """Scan UTF-8 text for synthetic or real secret literals without echoing secret values."""
    if "\x00" in text:
        return []
    errors: list[str] = []
    for lineno, line in enumerate(text.splitlines(), 1):
        for rule_name, pattern in SECRET_PATTERNS:
            for match in pattern.finditer(line):
                if rule_name == "url_basic_auth" and any(
                    tok in match.group(0) for tok in URL_AUTH_ALLOW_SUBSTRS
                ):
                    continue
                if rule_name == "cloudflare_api_token" and any(
                    tok in match.group(0) for tok in SECRET_PLACEHOLDER_SUBSTRS
                ):
                    continue
                errors.append(
                    f"{label}:{lineno}: potential secret literal ({rule_name})"
                )
                break
    return errors


def check_secret_literals(root: str, workspace_mode: bool = False) -> tuple[int, list[str]]:
    scanned = 0
    errors: list[str] = []
    for dirpath, dirnames, filenames in os.walk(root):
        rel_dir = os.path.relpath(dirpath, root).replace(os.sep, "/")
        next_dirs: list[str] = []
        for d in sorted(dirnames):
            if d in SKIP_DIRS:
                continue
            child_rel = d if rel_dir == "." else f"{rel_dir}/{d}"
            if workspace_mode and child_rel == ".agents/credentials":
                continue
            next_dirs.append(d)
        dirnames[:] = next_dirs

        for fn in sorted(filenames):
            abs_path = os.path.join(dirpath, fn)
            if os.path.islink(abs_path):
                continue
            rel = os.path.relpath(abs_path, root).replace(os.sep, "/")
            if rel == "tools/verify_repo_hygiene.py":
                continue
            if workspace_mode and rel.startswith(".agents/credentials/"):
                continue
            try:
                if os.path.getsize(abs_path) > MAX_SECRET_SCAN_BYTES:
                    continue
                with open(abs_path, "rb") as fh:
                    raw = fh.read()
            except OSError:
                continue
            if b"\x00" in raw:
                continue
            scanned += 1
            text = raw.decode("utf-8", errors="replace")
            errors.extend(scan_text_for_secrets(text, rel))
    return scanned, errors


def _read_exact(stream, count: int) -> bytes:
    chunks: list[bytes] = []
    remaining = count
    while remaining > 0:
        chunk = stream.read(remaining)
        if not chunk:
            break
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def _skip_exact(stream, count: int) -> None:
    remaining = count
    while remaining > 0:
        chunk = stream.read(min(remaining, 65536))
        if not chunk:
            break
        remaining -= len(chunk)


def check_git_history_secrets(root: str, ref: str = "HEAD") -> tuple[int, list[str]]:
    rev_res = subprocess.run(
        ["git", "-C", root, "rev-list", "--objects", ref],
        capture_output=True,
        text=True,
        check=False,
    )
    if rev_res.returncode != 0:
        detail = (rev_res.stderr or rev_res.stdout or "unknown error").strip()
        return 0, [f"git rev-list --objects {ref} failed: {detail}"]

    oid_to_path: dict[str, str] = {}
    for line in rev_res.stdout.splitlines():
        parts = line.split(" ", 1)
        if len(parts) != 2:
            continue
        sha, path = parts[0], parts[1]
        if path == "tools/verify_repo_hygiene.py":
            continue
        if sha not in oid_to_path:
            oid_to_path[sha] = path

    if not oid_to_path:
        return 0, []

    oids = list(oid_to_path.keys())
    proc = subprocess.Popen(
        ["git", "-C", root, "cat-file", "--batch"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    assert proc.stdin is not None and proc.stdout is not None and proc.stderr is not None

    def _feed_stdin() -> None:
        try:
            assert proc.stdin is not None
            payload = ("\n".join(oids) + "\n").encode("ascii")
            proc.stdin.write(payload)
        except BrokenPipeError:
            pass
        finally:
            if proc.stdin is not None:
                try:
                    proc.stdin.close()
                except OSError:
                    pass

    feeder = threading.Thread(target=_feed_stdin, daemon=True)
    feeder.start()

    scanned = 0
    errors: list[str] = []
    while True:
        header = proc.stdout.readline()
        if not header:
            break
        parts = header.decode("ascii", errors="replace").strip().split()
        if len(parts) < 2:
            continue
        sha = parts[0]
        obj_type = parts[1]
        if obj_type == "missing" or len(parts) < 3:
            continue
        size = int(parts[2])
        if obj_type != "blob" or size > MAX_SECRET_SCAN_BYTES:
            _skip_exact(proc.stdout, size + 1)
            continue
        raw = _read_exact(proc.stdout, size)
        _skip_exact(proc.stdout, 1)
        if b"\x00" in raw:
            continue
        scanned += 1
        path = oid_to_path.get(sha, "<unknown>")
        txt = raw.decode("utf-8", errors="replace")
        errors.extend(scan_text_for_secrets(txt, f"git-blob:{sha[:10]}:{path}"))

    stderr_out = proc.stderr.read().decode("utf-8", errors="replace").strip()
    feeder.join()
    rc = proc.wait()
    if rc != 0:
        errors.append(f"git cat-file --batch failed (rc={rc}): {stderr_out}")
    return scanned, errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Verify repository hygiene, template integrity, secret hygiene, and vendor neutrality."
    )
    parser.add_argument("--root", default=DEFAULT_ROOT, help="Repository root directory")
    parser.add_argument(
        "--scan-git-history",
        nargs="?",
        const="HEAD",
        default=None,
        metavar="REF",
        help="Also scan all reachable Git history blobs from REF (default: HEAD)",
    )
    parser.add_argument(
        "--check-workspace-perms",
        action="store_true",
        default=False,
        help="Audit provisioned VPS agent workspace permissions (0700/0600) and allow .agents/credentials/",
    )
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
    errors.extend(
        check_forbidden_credential_files(root, workspace_mode=ns.check_workspace_perms)
    )
    errors.extend(check_systemd_credential_hygiene(root))
    if ns.check_workspace_perms:
        errors.extend(check_workspace_perms(root))

    scanned_files, secret_errors = check_secret_literals(
        root, workspace_mode=ns.check_workspace_perms
    )
    errors.extend(secret_errors)

    scanned_blobs: int | None = None
    if ns.scan_git_history is not None:
        scanned_blobs, hist_errors = check_git_history_secrets(root, ns.scan_git_history)
        errors.extend(hist_errors)

    for err in errors:
        print(f"ERROR: {err}")

    if scanned_blobs is not None:
        print(
            f"verify_repo_hygiene: checked {len(prod_scripts)} prod scripts, "
            f"{scanned_files} text files, {scanned_blobs} git blobs ({ns.scan_git_history}); "
            f"{len(errors)} error(s)"
        )
    else:
        print(
            f"verify_repo_hygiene: checked {len(prod_scripts)} prod scripts, "
            f"{scanned_files} text files; {len(errors)} error(s)"
        )
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
