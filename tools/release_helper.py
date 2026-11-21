#!/usr/bin/env python3
"""Agent-managed release engine for debopty.

Implements Semantic Versioning 2.0.0, Conventional Commits 1.0.0, and
Keep a Changelog 1.1.0 with backward compatibility for legacy CalVer/Build
updaters. Uses Python 3 standard library only.
"""
import argparse
import datetime
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile

DEFAULT_REPO = "G3arB0xX/Debian-Optimizer-Script"

RE_SEMVER = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)"
    r"(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?"
    r"(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$"
)
RE_CALVER = re.compile(r"^\d{2}\.\d{2}\.\d{2}$")
RE_CALVER_LEGACY = re.compile(r"^(\d{4})\.(\d{2})\.(\d{2})(?:\.\d+)?$")
RE_BUILD = re.compile(r"^[a-z0-9]+$")
RE_DATE = re.compile(r"^(\d{4})-(\d{2})-(\d{2})$")
RE_CONVENTIONAL = re.compile(
    r"^(?P<type>feat|fix|perf|refactor|docs|test|build|ci|chore|style|revert)"
    r"(?:\((?P<scope>[a-zA-Z0-9_./-]+)\))?"
    r"(?P<breaking>!)?:\s+(?P<desc>.+)$"
)
RE_BREAKING_FOOTER = re.compile(r"^BREAKING[ -]CHANGE:\s*(.+)$", re.MULTILINE)
RE_EXEMPT_COMMIT = re.compile(r'^(?:Merge\b|Revert\s+")')
RE_KAC_HEADING = re.compile(r"^## \[([^\]]+)\](?:\s*-\s*(\d{4}-\d{2}-\d{2}))?\s*$")
RE_REF_LINK = re.compile(r"^\[([^\]]+)\]:\s+(https?://\S+)\s*$")
RE_VERSION_ID = re.compile(r'^VERSION_ID=(["\']?)([^"\'\n]+)\1\s*$', re.MULTILINE)

KAC_SECTIONS = ("Added", "Changed", "Deprecated", "Removed", "Fixed", "Security")
SECURITY_SCOPES = frozenset({"security", "sec", "fail2ban", "ssh", "nftables"})

KAC_HEADER = (
    "# Changelog\n\n"
    "All notable changes to this project will be documented in this file.\n\n"
    "The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),\n"
    "and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).\n"
)


def is_semver(value: str) -> bool:
    return bool(value and RE_SEMVER.match(value))


def parse_semver(value: str) -> tuple[int, int, int, tuple[str, ...]]:
    m = RE_SEMVER.match(value)
    if not m:
        raise ValueError(f"invalid SemVer 2.0.0: '{value}'")
    maj, mn, pat = int(m.group(1)), int(m.group(2)), int(m.group(3))
    pre = tuple(m.group(4).split(".")) if m.group(4) else ()
    return maj, mn, pat, pre


def semver_compare(v1: str, v2: str) -> int:
    maj1, min1, pat1, pre1 = parse_semver(v1)
    maj2, min2, pat2, pre2 = parse_semver(v2)
    for a, b in ((maj1, maj2), (min1, min2), (pat1, pat2)):
        if a != b:
            return -1 if a < b else 1
    if not pre1 and not pre2:
        return 0
    if not pre1:
        return 1
    if not pre2:
        return -1
    for id1, id2 in zip(pre1, pre2):
        if id1 == id2:
            continue
        num1, num2 = id1.isdigit(), id2.isdigit()
        if num1 and num2:
            return -1 if int(id1) < int(id2) else 1
        if num1 and not num2:
            return -1
        if not num1 and num2:
            return 1
        return -1 if id1 < id2 else 1
    if len(pre1) != len(pre2):
        return -1 if len(pre1) < len(pre2) else 1
    return 0


def next_semver_value(current: str, bump: str, prerelease: str = "") -> str:
    if prerelease and not is_semver(f"1.0.0-{prerelease}"):
        raise ValueError(f"invalid prerelease identifier: '{prerelease}'")
    if not current:
        return f"1.0.0-{prerelease}" if prerelease else "1.0.0"
    maj, mn, pat, _ = parse_semver(current)
    if bump == "major":
        maj += 1
        mn = 0
        pat = 0
    elif bump == "minor":
        mn += 1
        pat = 0
    elif bump == "patch":
        pat += 1
    elif bump == "none":
        return current
    else:
        raise ValueError(f"unsupported bump type: '{bump}'")
    base = f"{maj}.{mn}.{pat}"
    return f"{base}-{prerelease}" if prerelease else base


def normalize_calver(value: str) -> str:
    if not value:
        return ""
    m = RE_CALVER_LEGACY.match(value)
    if m:
        return f"{m.group(1)[2:]}.{m.group(2)}.{m.group(3)}"
    return value


def unquote(val: str) -> str:
    val = val.strip()
    if len(val) >= 2 and val[0] == val[-1] and val[0] in ("'", '"'):
        return val[1:-1]
    return val


def read_version_file(root: str) -> dict[str, object]:
    path = os.path.join(root, "VERSION")
    if not os.path.isfile(path):
        return {
            "exists": False,
            "semver": "",
            "calver": "",
            "build": "",
            "has_legacy_version_key": False,
            "raw_semver": "",
            "raw_calver": "",
        }
    text = open(path, encoding="utf-8").read()
    kv: dict[str, str] = {}
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        kv.setdefault(k.strip(), unquote(v))
    raw_semver = kv.get("DEBOPTI_SEMVER", "")
    raw_calver = kv.get("DEBOPTI_CALVER", "")
    calver = normalize_calver(raw_calver)
    build = kv.get("DEBOPTI_BUILD", "")
    return {
        "exists": True,
        "semver": raw_semver,
        "calver": calver,
        "build": build,
        "has_legacy_version_key": "DEBOPTI_VERSION" in kv,
        "raw_semver": raw_semver,
        "raw_calver": raw_calver,
    }


def read_common_version_id(root: str) -> tuple[bool, str]:
    path = os.path.join(root, "scripts", "common.sh")
    if not os.path.isfile(path):
        return False, ""
    text = open(path, encoding="utf-8").read()
    m = RE_VERSION_ID.search(text)
    if not m:
        return True, ""
    return True, m.group(2)


def run_git(root: str, args: list[str]) -> tuple[int, str]:
    try:
        proc = subprocess.run(
            ["git", "-C", root, *args],
            capture_output=True,
            text=True,
            check=False,
        )
        return proc.returncode, proc.stdout
    except OSError:
        return 1, ""


def find_latest_semver_tag(root: str) -> str:
    rc, out = run_git(root, ["tag", "--merged", "HEAD"])
    if rc != 0:
        rc, out = run_git(root, ["tag"])
    if rc != 0:
        return ""
    candidates: list[tuple[str, str]] = []
    for raw in out.splitlines():
        tag = raw.strip()
        if not tag.startswith("v"):
            continue
        if re.match(r"^v20\d{2}\.", tag):
            continue
        ver = tag[1:]
        if is_semver(ver):
            candidates.append((ver, tag))
    if not candidates:
        return ""
    best_ver, best_tag = candidates[0]
    for ver, tag in candidates[1:]:
        if semver_compare(ver, best_ver) > 0:
            best_ver, best_tag = ver, tag
    return best_tag


def resolve_since_ref(root: str, explicit_since: str = "") -> str:
    if explicit_since:
        return explicit_since
    tag = find_latest_semver_tag(root)
    if tag:
        return tag
    rc, ver_sha = run_git(root, ["log", "-n", "1", "--format=%H", "--", "VERSION"])
    ver_sha = ver_sha.strip()
    if rc == 0 and ver_sha:
        rc_head, head_sha = run_git(root, ["rev-parse", "HEAD"])
        if rc_head == 0 and head_sha.strip() and ver_sha != head_sha.strip():
            return ver_sha
    rc_legacy, legacy_tag = run_git(root, ["describe", "--tags", "--abbrev=0"])
    if rc_legacy == 0 and legacy_tag.strip():
        return legacy_tag.strip()
    return ""


def collect_commits(root: str, since_ref: str) -> list[dict[str, str]]:
    rev_spec = f"{since_ref}..HEAD" if since_ref else "HEAD"
    rc, out = run_git(root, ["log", "--format=%H%x1f%s%x1f%b%x1e", rev_spec])
    if rc != 0:
        return []
    commits: list[dict[str, str]] = []
    for chunk in out.split("\x1e"):
        chunk = chunk.strip()
        if not chunk:
            continue
        parts = chunk.split("\x1f", 2)
        if len(parts) < 2:
            continue
        commits.append(
            {
                "sha": parts[0].strip(),
                "subject": parts[1].strip(),
                "body": parts[2].strip() if len(parts) > 2 else "",
            }
        )
    return commits


def parse_conventional_commit(commit: dict[str, str]) -> dict[str, object] | None:
    subject = commit["subject"]
    m = RE_CONVENTIONAL.match(subject)
    if not m:
        return None
    ctype = m.group("type")
    scope = m.group("scope") or ""
    breaking = bool(m.group("breaking"))
    desc = m.group("desc").strip()
    breaking_note = ""
    fm = RE_BREAKING_FOOTER.search(commit.get("body", ""))
    if fm:
        breaking = True
        breaking_note = fm.group(1).strip()
    return {
        "sha": commit["sha"],
        "type": ctype,
        "scope": scope,
        "breaking": breaking,
        "desc": desc,
        "breaking_note": breaking_note,
    }


def append_unique(items: list[str], value: str) -> None:
    if value and value not in items:
        items.append(value)


def format_bullet(scope: str, desc: str, breaking: bool = False) -> str:
    core = f"**{scope}**: {desc}" if scope else desc
    return f"**BREAKING**: {core}" if breaking else core


def compute_plan(
    root: str,
    since_ref: str = "",
    bump: str = "auto",
    explicit_version: str = "",
    prerelease: str = "",
    date_str: str = "",
    explicit_build: str = "",
    force: bool = False,
    include_all: bool = False,
) -> dict[str, object]:
    ver_info = read_version_file(root)
    current_semver = str(ver_info["semver"])
    current_calver = str(ver_info["calver"])
    current_build = str(ver_info["build"])

    if current_semver and not is_semver(current_semver):
        raise ValueError(f"VERSION contains invalid DEBOPTI_SEMVER: '{current_semver}'")

    if date_str:
        dm = RE_DATE.match(date_str)
        if not dm:
            raise ValueError(f"invalid --date '{date_str}' (expected YYYY-MM-DD)")
        datetime.date(int(dm.group(1)), int(dm.group(2)), int(dm.group(3)))
        release_date = date_str
    else:
        release_date = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d")
    next_calver = f"{release_date[2:4]}.{release_date[5:7]}.{release_date[8:10]}"

    resolved_since = resolve_since_ref(root, since_ref)
    commits = collect_commits(root, resolved_since)

    sections: dict[str, list[str]] = {sec: [] for sec in KAC_SECTIONS}
    breaking_changes: list[str] = []
    has_feat = False
    has_patch = False

    for commit in commits:
        parsed = parse_conventional_commit(commit)
        if not parsed:
            continue
        ctype = str(parsed["type"])
        scope = str(parsed["scope"])
        is_breaking = bool(parsed["breaking"])
        desc = str(parsed["desc"])
        breaking_note = str(parsed["breaking_note"])

        if is_breaking:
            btext = format_bullet(scope, breaking_note or desc, breaking=True)
            append_unique(breaking_changes, btext)
            append_unique(sections["Changed"], btext)
            continue

        if ctype == "feat":
            has_feat = True
        elif ctype in ("fix", "perf", "revert"):
            has_patch = True

        bullet = format_bullet(scope, desc)
        if ctype in ("feat", "fix") and scope.lower() in SECURITY_SCOPES:
            append_unique(sections["Security"], bullet)
        elif ctype == "feat":
            append_unique(sections["Added"], bullet)
        elif ctype in ("perf", "refactor"):
            append_unique(sections["Changed"], bullet)
        elif ctype == "fix":
            append_unique(sections["Fixed"], bullet)
        elif ctype == "revert":
            append_unique(sections["Removed"], bullet)
        elif include_all:
            append_unique(sections["Changed"], bullet)

    if explicit_version:
        target_ver = explicit_version.lstrip("v")
        if not is_semver(target_ver):
            raise ValueError(f"invalid --version '{explicit_version}' (expected SemVer 2.0.0)")
        if current_semver and not force and semver_compare(target_ver, current_semver) <= 0:
            raise ValueError(
                f"target version {target_ver} must be greater than current {current_semver} (use --force to override)"
            )
        recommended_bump = "explicit"
        next_semver = target_ver
    elif not current_semver:
        recommended_bump = "bootstrap"
        next_semver = next_semver_value("", "major", prerelease)
    else:
        if bump == "auto":
            cur_maj = parse_semver(current_semver)[0]
            if breaking_changes:
                recommended_bump = "major" if cur_maj >= 1 else "minor"
            elif has_feat:
                recommended_bump = "minor"
            elif has_patch:
                recommended_bump = "patch"
            else:
                recommended_bump = "none"
        else:
            recommended_bump = bump

        if recommended_bump == "none":
            next_semver = current_semver
        else:
            next_semver = next_semver_value(current_semver, recommended_bump, prerelease)
            if not force and semver_compare(next_semver, current_semver) <= 0:
                raise ValueError(
                    f"computed version {next_semver} is not greater than current {current_semver} (use --force to override)"
                )

    rc_head, head_sha = run_git(root, ["rev-parse", "HEAD"])
    head_sha = head_sha.strip() if rc_head == 0 else ""

    if explicit_build:
        if not RE_BUILD.match(explicit_build):
            raise ValueError(f"invalid --build '{explicit_build}' (expected lowercase alphanumeric)")
        next_build = explicit_build
    else:
        seed = f"{next_semver}|{next_calver}|{head_sha or 'init'}"
        next_build = hashlib.sha256(seed.encode("utf-8")).hexdigest()[:8]

    non_empty_sections = {k: v for k, v in sections.items() if v}
    preview = render_release_section(next_semver, release_date, non_empty_sections)
    is_prerelease = "-" in next_semver.split("+", 1)[0]

    return {
        "current_semver": current_semver,
        "current_calver": current_calver,
        "current_build": current_build,
        "recommended_bump": recommended_bump,
        "next_semver": next_semver,
        "next_calver": next_calver,
        "next_build": next_build,
        "release_date": release_date,
        "tag": f"v{next_semver}",
        "is_prerelease": is_prerelease,
        "since_ref": resolved_since,
        "commit_count": len(commits),
        "breaking_changes": breaking_changes,
        "sections": non_empty_sections,
        "changelog_preview": preview,
    }


def render_release_section(
    version: str, release_date: str, sections: dict[str, list[str]]
) -> str:
    lines = [f"## [{version}] - {release_date}"]
    has_any = False
    for sec in KAC_SECTIONS:
        items = sections.get(sec) or []
        if not items:
            continue
        has_any = True
        lines.append("")
        lines.append(f"### {sec}")
        lines.append("")
        for item in items:
            lines.append(f"- {item}")
    if not has_any:
        lines.append("")
        lines.append("### Changed")
        lines.append("")
        lines.append("- Maintenance release.")
    return "\n".join(lines) + "\n"


def parse_unreleased_bullets(body_lines: list[str]) -> dict[str, list[str]]:
    sections: dict[str, list[str]] = {sec: [] for sec in KAC_SECTIONS}
    current_sec = "Changed"
    for line in body_lines:
        stripped = line.strip()
        if stripped.startswith("### "):
            sec_name = stripped[4:].strip()
            if sec_name in sections:
                current_sec = sec_name
            continue
        if stripped.startswith("- ") or stripped.startswith("* "):
            item = stripped[2:].strip()
            append_unique(sections[current_sec], item)
    return sections


def merge_sections(
    curated: dict[str, list[str]], generated: dict[str, list[str]]
) -> dict[str, list[str]]:
    merged: dict[str, list[str]] = {}
    for sec in KAC_SECTIONS:
        items: list[str] = []
        for item in curated.get(sec, []):
            append_unique(items, item)
        for item in generated.get(sec, []):
            append_unique(items, item)
        if items:
            merged[sec] = items
    return merged


def split_changelog_parts(text: str) -> tuple[list[str], list[str], list[tuple[str, str]]]:
    """Return (unreleased_body_lines, older_sections_lines, ref_links)."""
    lines = text.splitlines()
    ref_links: list[tuple[str, str]] = []
    body_end = len(lines)

    idx = len(lines) - 1
    while idx >= 0:
        s = lines[idx].strip()
        if not s:
            idx -= 1
            continue
        m = RE_REF_LINK.match(s)
        if m:
            ref_links.insert(0, (m.group(1), m.group(2)))
            body_end = idx
            idx -= 1
        else:
            break

    content_lines = lines[:body_end]

    # Locate first ## heading
    first_h2 = -1
    for i, ln in enumerate(content_lines):
        if ln.startswith("## "):
            first_h2 = i
            break

    if first_h2 == -1:
        return [], [], ref_links

    m_unrel = RE_KAC_HEADING.match(content_lines[first_h2])
    if m_unrel and m_unrel.group(1).lower() == "unreleased":
        next_h2 = len(content_lines)
        for i in range(first_h2 + 1, len(content_lines)):
            if content_lines[i].startswith("## "):
                next_h2 = i
                break
        unreleased_body = content_lines[first_h2 + 1 : next_h2]
        older_lines = content_lines[next_h2:]
        return unreleased_body, older_lines, ref_links

    return [], content_lines[first_h2:], ref_links


def strip_version_section(lines: list[str], version: str) -> list[str]:
    out: list[str] = []
    skipping = False
    for ln in lines:
        if ln.startswith("## "):
            m = RE_KAC_HEADING.match(ln)
            if m and m.group(1) == version:
                skipping = True
                continue
            skipping = False
        if not skipping:
            out.append(ln)
    return out


def build_updated_changelog(
    existing_text: str,
    next_semver: str,
    prev_semver: str,
    release_date: str,
    generated_sections: dict[str, list[str]],
    repo: str,
    force: bool = False,
) -> str:
    unreleased_body, older_lines, existing_links = split_changelog_parts(existing_text)

    # Check duplicate version heading
    for ln in older_lines:
        m = RE_KAC_HEADING.match(ln)
        if m and m.group(1) == next_semver:
            if not force:
                raise ValueError(
                    f"CHANGELOG.md already contains section for [{next_semver}] (use --force to replace)"
                )
            older_lines = strip_version_section(older_lines, next_semver)
            break

    curated = parse_unreleased_bullets(unreleased_body)
    merged = merge_sections(curated, generated_sections)
    new_section = render_release_section(next_semver, release_date, merged)

    # Trim leading/trailing blank lines on older_lines
    while older_lines and not older_lines[0].strip():
        older_lines.pop(0)
    while older_lines and not older_lines[-1].strip():
        older_lines.pop()

    # Build reference links
    link_map: dict[str, str] = {}
    link_order: list[str] = []
    for k, v in existing_links:
        if k.lower() == "unreleased" or k == next_semver:
            continue
        link_map[k] = v
        link_order.append(k)

    unrel_url = f"https://github.com/{repo}/compare/v{next_semver}...HEAD"
    if prev_semver and is_semver(prev_semver):
        ver_url = f"https://github.com/{repo}/compare/v{prev_semver}...v{next_semver}"
    else:
        ver_url = f"https://github.com/{repo}/releases/tag/v{next_semver}"

    final_links = [
        ("Unreleased", unrel_url),
        (next_semver, ver_url),
        *((k, link_map[k]) for k in link_order),
    ]

    parts = [KAC_HEADER.rstrip("\n"), "", "## [Unreleased]", "", new_section.rstrip("\n")]
    if older_lines:
        parts.extend(["", "\n".join(older_lines)])
    parts.append("")
    parts.extend(f"[{k}]: {v}" for k, v in final_links)
    return "\n".join(parts).rstrip("\n") + "\n"


def atomic_write(path: str, content: str) -> None:
    parent = os.path.dirname(os.path.abspath(path))
    os.makedirs(parent, exist_ok=True)
    fd, tmp_path = tempfile.mkstemp(prefix=".release-tmp-", dir=parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(content)
        if os.path.exists(path):
            st = os.stat(path)
            os.chmod(tmp_path, st.st_mode & 0o7777)
        os.replace(tmp_path, path)
    except BaseException:
        if os.path.exists(tmp_path):
            os.unlink(tmp_path)
        raise


def render_version_file(semver: str, calver: str, build: str) -> str:
    return (
        f'DEBOPTI_SEMVER="{semver}"\n'
        f'DEBOPTI_CALVER="{calver}"\n'
        f'DEBOPTI_BUILD="{build}"\n'
    )


def update_common_sh_content(text: str, next_build: str) -> str:
    if not RE_VERSION_ID.search(text):
        raise ValueError("scripts/common.sh is missing VERSION_ID= assignment")
    return RE_VERSION_ID.sub(f'VERSION_ID="{next_build}"', text, count=1)


def extract_notes(root: str, version: str) -> str:
    version = version.lstrip("v")
    if not version:
        ver_info = read_version_file(root)
        version = str(ver_info["semver"])
    if not version:
        raise ValueError("no version specified and VERSION has no DEBOPTI_SEMVER")

    cl_path = os.path.join(root, "CHANGELOG.md")
    if not os.path.isfile(cl_path):
        raise ValueError(f"CHANGELOG.md not found in {root}")

    lines = open(cl_path, encoding="utf-8").read().splitlines()
    found = False
    body: list[str] = []
    ref_url = ""

    for ln in lines:
        rm = RE_REF_LINK.match(ln.strip())
        if rm and rm.group(1) == version:
            ref_url = rm.group(2)

    for ln in lines:
        if not found:
            hm = RE_KAC_HEADING.match(ln)
            if hm and hm.group(1) == version:
                found = True
            continue
        if ln.startswith("## ") or RE_REF_LINK.match(ln.strip()):
            break
        body.append(ln)

    if not found:
        raise ValueError(f"section '## [{version}]' not found in CHANGELOG.md")

    while body and not body[0].strip():
        body.pop(0)
    while body and not body[-1].strip():
        body.pop()

    text = "\n".join(body).strip()
    if ref_url:
        text = f"{text}\n\n**Full Changelog**: {ref_url}" if text else f"**Full Changelog**: {ref_url}"
    return text + "\n"


def cmd_plan(args: argparse.Namespace, root: str) -> int:
    plan = compute_plan(
        root=root,
        since_ref=args.since or "",
        bump=args.bump,
        explicit_version=args.version or "",
        prerelease=args.prerelease or "",
        date_str=args.date or "",
        explicit_build=args.build or "",
        force=args.force,
        include_all=args.include_all,
    )
    if args.json:
        print(json.dumps(plan, indent=2, ensure_ascii=False))
        return 0

    print(f"Current SemVer : {plan['current_semver'] or '(none - CalVer baseline)'}")
    print(f"Current CalVer : {plan['current_calver'] or '(none)'} (build {plan['current_build'] or 'none'})")
    print(f"Since Ref      : {plan['since_ref'] or '(initial)'} ({plan['commit_count']} commits)")
    print(f"Bump           : {plan['recommended_bump']}")
    print(f"Next SemVer    : {plan['next_semver']} (tag {plan['tag']}, prerelease={plan['is_prerelease']})")
    print(f"Next CalVer    : {plan['next_calver']} (build {plan['next_build']})")
    print("")
    print(str(plan["changelog_preview"]).rstrip("\n"))
    return 0


def cmd_bump(args: argparse.Namespace, root: str) -> int:
    plan = compute_plan(
        root=root,
        since_ref=args.since or "",
        bump=args.bump,
        explicit_version=args.version or "",
        prerelease=args.prerelease or "",
        date_str=args.date or "",
        explicit_build=args.build or "",
        force=args.force,
        include_all=args.include_all,
    )
    if plan["recommended_bump"] == "none" and args.bump == "auto" and not args.version:
        print(
            "ERROR: no releasable Conventional Commits found since last release "
            "(pass --bump patch|minor|major or --version X.Y.Z to force a bump)",
            file=sys.stderr,
        )
        return 1

    next_semver = str(plan["next_semver"])
    next_calver = str(plan["next_calver"])
    next_build = str(plan["next_build"])
    release_date = str(plan["release_date"])
    prev_semver = str(plan["current_semver"])
    repo = os.environ.get("DEBOPTY_REPO", DEFAULT_REPO)

    version_content = render_version_file(next_semver, next_calver, next_build)

    common_path = os.path.join(root, "scripts", "common.sh")
    common_updated: str | None = None
    if os.path.isfile(common_path):
        common_updated = update_common_sh_content(
            open(common_path, encoding="utf-8").read(), next_build
        )

    cl_path = os.path.join(root, "CHANGELOG.md")
    existing_cl = open(cl_path, encoding="utf-8").read() if os.path.isfile(cl_path) else ""
    new_cl = build_updated_changelog(
        existing_text=existing_cl,
        next_semver=next_semver,
        prev_semver=prev_semver,
        release_date=release_date,
        generated_sections=plan["sections"],  # type: ignore[arg-type]
        repo=repo,
        force=args.force,
    )

    if args.dry_run:
        print(f"[dry-run] Would update {os.path.join(root, 'VERSION')}:")
        print(version_content.rstrip("\n"))
        if common_updated is not None:
            print(f"[dry-run] Would sync {common_path}: VERSION_ID=\"{next_build}\"")
        print(f"[dry-run] Would update {cl_path} with section ## [{next_semver}] - {release_date}")
        return 0

    atomic_write(os.path.join(root, "VERSION"), version_content)
    if common_updated is not None:
        atomic_write(common_path, common_updated)
    atomic_write(cl_path, new_cl)

    print(f"Bumped release to {next_semver} (CalVer {next_calver}, build {next_build}, tag v{next_semver})")
    return 0


def cmd_notes(args: argparse.Namespace, root: str) -> int:
    notes = extract_notes(root, args.version or "")
    if args.output:
        atomic_write(args.output, notes)
    else:
        sys.stdout.write(notes)
    return 0


def cmd_verify(args: argparse.Namespace, root: str) -> int:
    errors: list[str] = []
    ver_info = read_version_file(root)

    if not ver_info["exists"]:
        errors.append("VERSION file is missing")
    else:
        if ver_info["has_legacy_version_key"]:
            errors.append("VERSION must not contain forbidden legacy key 'DEBOPTI_VERSION='")
        calver = str(ver_info["calver"])
        build = str(ver_info["build"])
        semver = str(ver_info["semver"])
        if not calver or not RE_CALVER.match(calver):
            errors.append(f"VERSION has missing or invalid DEBOPTI_CALVER: '{ver_info['raw_calver']}'")
        if not build or not RE_BUILD.match(build):
            errors.append(f"VERSION has missing or invalid DEBOPTI_BUILD: '{build}'")
        if semver and not is_semver(semver):
            errors.append(f"VERSION has invalid DEBOPTI_SEMVER: '{semver}'")

        common_exists, common_vid = read_common_version_id(root)
        if not common_exists:
            errors.append("scripts/common.sh is missing")
        elif common_vid != build:
            errors.append(
                f"scripts/common.sh VERSION_ID ('{common_vid}') != VERSION DEBOPTI_BUILD ('{build}')"
            )

        if semver and is_semver(semver):
            cl_path = os.path.join(root, "CHANGELOG.md")
            if not os.path.isfile(cl_path):
                errors.append("CHANGELOG.md is missing (required when DEBOPTI_SEMVER is set)")
            else:
                cl_lines = open(cl_path, encoding="utf-8").read().splitlines()
                if not any(ln.strip() == "# Changelog" for ln in cl_lines):
                    errors.append("CHANGELOG.md is missing '# Changelog' title")
                seen_versions: dict[str, int] = {}
                has_unreleased = False
                has_target_dated = False
                for ln in cl_lines:
                    hm = RE_KAC_HEADING.match(ln)
                    if not hm:
                        continue
                    vname = hm.group(1)
                    vdate = hm.group(2)
                    if vname == "Unreleased":
                        has_unreleased = True
                        continue
                    seen_versions[vname] = seen_versions.get(vname, 0) + 1
                    if vname == semver and vdate:
                        has_target_dated = True
                if not has_unreleased:
                    errors.append("CHANGELOG.md is missing '## [Unreleased]' section")
                if not has_target_dated:
                    errors.append(
                        f"CHANGELOG.md is missing '## [{semver}] - YYYY-MM-DD' section"
                    )
                for vname, count in seen_versions.items():
                    if count > 1:
                        errors.append(
                            f"CHANGELOG.md has duplicate heading for version '[{vname}]' ({count} times)"
                        )

    if args.check_commits is not None:
        rev_range = args.check_commits.strip()
        if not rev_range:
            since = resolve_since_ref(root)
            rev_range = f"{since}..HEAD" if since else "HEAD"
        rc, out = run_git(root, ["log", "--format=%H%x1f%s", rev_range])
        if rc != 0:
            errors.append(f"failed to inspect git commit range '{rev_range}'")
        else:
            for ln in out.splitlines():
                if "\x1f" not in ln:
                    continue
                sha, subj = ln.split("\x1f", 1)
                subj = subj.strip()
                if RE_EXEMPT_COMMIT.match(subj):
                    continue
                if not RE_CONVENTIONAL.match(subj):
                    errors.append(
                        f"non-conventional commit {sha[:8]}: '{subj}'"
                    )

    if errors:
        for err in errors:
            print(f"ERROR: {err}", file=sys.stderr)
        print(f"release verify: FAIL ({len(errors)} error(s))", file=sys.stderr)
        return 1

    semver = str(ver_info["semver"])
    calver = str(ver_info["calver"])
    build = str(ver_info["build"])
    if semver:
        print(f"release verify: PASS (SemVer {semver}, CalVer {calver}, build {build})")
    else:
        print(f"release verify: PASS (CalVer-only baseline {calver}, build {build})")
    return 0


def build_parser() -> argparse.ArgumentParser:
    default_root = os.environ.get(
        "DEBOPTY_RELEASE_ROOT",
        os.path.abspath(os.path.join(os.path.dirname(__file__), "..")),
    )
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--repo-root", default=argparse.SUPPRESS, help="Repository root directory")

    parser = argparse.ArgumentParser(
        prog="tools/release.sh",
        description="debopty agent-managed release engine",
        parents=[common],
    )
    parser.set_defaults(default_root=default_root)
    sub = parser.add_subparsers(dest="command", required=True)

    p_plan = sub.add_parser("plan", parents=[common], help="Preview next release plan (read-only)")
    p_plan.add_argument("--since", default="", help="Git ref to scan commits from")
    p_plan.add_argument("--bump", choices=("auto", "major", "minor", "patch"), default="auto")
    p_plan.add_argument("--version", default="", help="Explicit target SemVer 2.0.0")
    p_plan.add_argument("--prerelease", default="", help="Prerelease identifier (e.g. rc.1)")
    p_plan.add_argument("--date", default="", help="Release date override (YYYY-MM-DD)")
    p_plan.add_argument("--build", default="", help="Build ID override (lowercase alnum)")
    p_plan.add_argument("--json", action="store_true", help="Output strict JSON")
    p_plan.add_argument("--force", action="store_true", help="Allow non-monotonic version")
    p_plan.add_argument("--include-all", action="store_true", help="Include docs/chore/ci commits")

    p_bump = sub.add_parser("bump", parents=[common], help="Atomically bump VERSION, common.sh, CHANGELOG.md")
    p_bump.add_argument("--since", default="", help="Git ref to scan commits from")
    p_bump.add_argument("--bump", choices=("auto", "major", "minor", "patch"), default="auto")
    p_bump.add_argument("--version", default="", help="Explicit target SemVer 2.0.0")
    p_bump.add_argument("--prerelease", default="", help="Prerelease identifier (e.g. rc.1)")
    p_bump.add_argument("--date", default="", help="Release date override (YYYY-MM-DD)")
    p_bump.add_argument("--build", default="", help="Build ID override (lowercase alnum)")
    p_bump.add_argument("--dry-run", action="store_true", help="Preview without writing files")
    p_bump.add_argument("--force", action="store_true", help="Allow non-monotonic/overwrite section")
    p_bump.add_argument("--include-all", action="store_true", help="Include docs/chore/ci commits")

    p_notes = sub.add_parser("notes", parents=[common], help="Extract release notes for a version")
    p_notes.add_argument("--version", default="", help="Target version (defaults to VERSION DEBOPTI_SEMVER)")
    p_notes.add_argument("--output", default="", help="Write notes atomically to path")

    p_verify = sub.add_parser("verify", parents=[common], help="Verify release metadata consistency")
    p_verify.add_argument(
        "--check-commits",
        nargs="?",
        const="",
        default=None,
        metavar="RANGE",
        help="Validate commit subjects in RANGE against Conventional Commits 1.0.0",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    root = os.path.abspath(getattr(args, "repo_root", None) or args.default_root)
    try:
        if args.command == "plan":
            return cmd_plan(args, root)
        if args.command == "bump":
            return cmd_bump(args, root)
        if args.command == "notes":
            return cmd_notes(args, root)
        if args.command == "verify":
            return cmd_verify(args, root)
    except ValueError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1
    return 1


if __name__ == "__main__":
    sys.exit(main())
