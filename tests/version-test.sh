#!/usr/bin/env bash
# CalVer + Build ID helpers (no containers)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../scripts/version.sh
source "${ROOT}/scripts/version.sh"

FAILS=0
_fail() {
    echo "FAIL: $*" >&2
    FAILS=$((FAILS + 1))
}
_pass() { echo "OK: $*"; }

_assert_eq() {
    local got=$1 expected=$2 msg=$3
    if [[ "$got" != "$expected" ]]; then
        _fail "$msg (got='$got' expected='$expected')"
    else
        _pass "$msg"
    fi
}

_assert_rc() {
    local rc=$1 expected=$2 msg=$3
    if [[ "$rc" -ne "$expected" ]]; then
        _fail "$msg (rc=$rc expected=$expected)"
    else
        _pass "$msg"
    fi
}

_run_rc() {
    set +e
    "$@"
    _RC=$?
    set -e
}

_field() {
    printf '%s' "$1" | cut -d'|' -f"$2"
}

TMP="$(mktemp -d /tmp/debopti-version-test.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

# --- parse valid VERSION file ---
cat > "${TMP}/VERSION" <<'EOF'
DEBOPTI_VERSION="26.08.15"
DEBOPTI_BUILD="uxvkolop"
EOF
if parsed="$(debopti_parse_version_file "${TMP}/VERSION")"; then
    _assert_eq "$(_field "$parsed" 1)" "26.08.15" "parse version"
    _assert_eq "$(_field "$parsed" 2)" "uxvkolop" "parse build"
else
    _fail "parse valid VERSION should succeed"
fi

# --- parse invalid files ---
echo 'DEBOPTI_VERSION="not-a-date"' > "${TMP}/bad1"
if debopti_parse_version_file "${TMP}/bad1" >/dev/null 2>&1; then
    _fail "invalid CalVer should fail"
else
    _pass "invalid CalVer rejected"
fi

echo 'DEBOPTI_VERSION="26.08.15"' > "${TMP}/bad2"
if debopti_parse_version_file "${TMP}/bad2" >/dev/null 2>&1; then
    _fail "missing build should fail"
else
    _pass "missing build rejected"
fi

echo 'VERSION_ID="legacyid"' > "${TMP}/legacy"
if parsed="$(debopti_parse_version_text "$(cat "${TMP}/legacy")")"; then
    _assert_eq "$(_field "$parsed" 1)" "" "legacy version empty"
    _assert_eq "$(_field "$parsed" 2)" "legacyid" "legacy build from VERSION_ID"
else
    _fail "legacy VERSION_ID text should parse"
fi

cat > "${TMP}/legacy_calver" <<'EOF'
DEBOPTI_VERSION="2026.08.16.1"
DEBOPTI_BUILD="oldbuild1"
EOF
if parsed="$(debopti_parse_version_file "${TMP}/legacy_calver")"; then
    _assert_eq "$(_field "$parsed" 1)" "26.08.16" "legacy four-digit CalVer normalizes to YY.MM.DD"
else
    _fail "legacy four-digit CalVer should parse"
fi

# --- CALVER key: new parser reads it; 2026.08.16.1 parser must still succeed ---
_legacy_v202608161_parse() {
    local text=$1
    local DEBOPTI_VERSION_REGEX='^[0-9]{4}\.[0-9]{2}\.[0-9]{2}\.[0-9]+$'
    local DEBOPTI_BUILD_REGEX='^[a-z0-9]+$'
    local version="" build=""
    version="$(_debopti_kv "$text" "DEBOPTI_VERSION" || true)"
    build="$(_debopti_kv "$text" "DEBOPTI_BUILD" || true)"
    if [[ -z "$build" ]]; then
        build="$(_debopti_kv "$text" "VERSION_ID" || true)"
    fi
    if [[ -n "$version" && ! "$version" =~ $DEBOPTI_VERSION_REGEX ]]; then
        return 1
    fi
    if [[ -z "$build" || ! "$build" =~ $DEBOPTI_BUILD_REGEX ]]; then
        return 1
    fi
    printf '%s|%s\n' "$version" "$build"
}

cat > "${TMP}/github_broken" <<'EOF'
DEBOPTI_VERSION="26.08.16"
DEBOPTI_BUILD="luutwynsyqkq"
EOF
if _legacy_v202608161_parse "$(cat "${TMP}/github_broken")" >/dev/null 2>&1; then
    _fail "old updater must reject DEBOPTI_VERSION=YY.MM.DD (reproduces VPS abort)"
else
    _pass "old updater rejects DEBOPTI_VERSION=YY.MM.DD"
fi

# --- next CalVer is today's YY.MM.DD (no same-day serial) ---
_assert_eq "$(debopti_next_calver "" "26.08.15")" "26.08.15" "empty existing -> today"
_assert_eq "$(debopti_next_calver "26.08.15" "26.08.15")" "26.08.15" "same day keeps date"
_assert_eq "$(debopti_next_calver "26.08.15" "26.08.16")" "26.08.16" "new day is today"

# --- update decision is Build equality ---
_run_rc debopti_needs_update "aaa" "bbb"
_assert_rc "$_RC" 0 "different builds need update"
_run_rc debopti_needs_update "aaa" "aaa"
_assert_rc "$_RC" 1 "same build skips update"
_run_rc debopti_needs_update "aaa" ""
_assert_rc "$_RC" 1 "empty remote skips update"
_run_rc debopti_needs_update "" "bbb"
_assert_rc "$_RC" 0 "empty local + remote build needs update"

# --- display / tag ---
_assert_eq "$(debopti_format_display "26.08.15" "uxvkolop")" "26.08.15 (uxvkolop)" "display format"
_assert_eq "$(debopti_format_display "2026.08.16.1" "xyz")" "26.08.16 (xyz)" "display normalizes legacy CalVer"
_assert_eq "$(debopti_format_prompt "26.08.15" "uxvkolop")" "26.08.15 (build uxvkolop)" "prompt format"
_assert_eq "$(debopti_tag_name "26.08.15" "uxvkolop")" "v26.08.15-uxvkolop" "tag includes build"

# --- write + load: on-disk key is CALVER so old updaters can parse ---
debopti_write_version_file "${TMP}/out/VERSION" "26.08.16" "abc123xy"
written="$(cat "${TMP}/out/VERSION")"
echo "$written" | grep -q '^DEBOPTI_CALVER="26.08.16"' || _fail "written file uses DEBOPTI_CALVER"
if echo "$written" | grep -q '^DEBOPTI_VERSION='; then
    _fail "written file must omit DEBOPTI_VERSION (old updater would abort)"
else
    _pass "written file omits DEBOPTI_VERSION"
fi
if parsed="$(debopti_parse_version_file "${TMP}/out/VERSION")"; then
    _assert_eq "$(_field "$parsed" 1)" "26.08.16" "written version"
    _assert_eq "$(_field "$parsed" 2)" "abc123xy" "written build"
else
    _fail "new parser should read DEBOPTI_CALVER"
fi
if parsed="$(_legacy_v202608161_parse "$written")"; then
    _assert_eq "$(_field "$parsed" 1)" "" "old updater sees empty version"
    _assert_eq "$(_field "$parsed" 2)" "abc123xy" "old updater reads Build"
else
    _fail "old updater must parse CALVER-only VERSION (VPS update path)"
fi
_run_rc debopti_version_file_canonical "${TMP}/out/VERSION"
_assert_rc "$_RC" 0 "written VERSION is canonical"
_run_rc debopti_version_file_canonical "${TMP}/github_broken"
_assert_rc "$_RC" 1 "DEBOPTI_VERSION=YY.MM.DD is not canonical"
unset DEBOPTI_VERSION DEBOPTI_BUILD VERSION_ID
debopti_load_version "${TMP}/out"
_assert_eq "${DEBOPTI_VERSION}" "26.08.16" "load version"
_assert_eq "${DEBOPTI_BUILD}" "abc123xy" "load build"
_assert_eq "${VERSION_ID}" "abc123xy" "VERSION_ID alias"

# --- extract tree verification ---
mkdir -p "${TMP}/tree-ok" "${TMP}/tree-bad"
debopti_write_version_file "${TMP}/tree-ok/VERSION" "26.08.15" "matchid"
debopti_write_version_file "${TMP}/tree-bad/VERSION" "26.08.15" "otherid"
_run_rc debopti_verify_tree "${TMP}/tree-ok" "26.08.15" "matchid"
_assert_rc "$_RC" 0 "matching tree verifies"
_run_rc debopti_verify_tree "${TMP}/tree-ok" "" "matchid"
_assert_rc "$_RC" 0 "empty expected version verifies by Build only"
_run_rc debopti_verify_tree "${TMP}/tree-ok" "2026.08.15.1" "matchid"
_assert_rc "$_RC" 0 "legacy expected version matches normalized tree"
_run_rc debopti_verify_tree "${TMP}/tree-bad" "26.08.15" "matchid"
_assert_rc "$_RC" 1 "mismatched extract metadata rejected"
_run_rc debopti_verify_tree "${TMP}/missing" "26.08.15" "matchid"
_assert_rc "$_RC" 1 "missing VERSION in tree rejected"

# --- changelog prepend ---
debopti_changelog_prepend "${TMP}/CHANGELOG.md" "26.08.15" "uxvkolop" "feat: first"
debopti_changelog_prepend "${TMP}/CHANGELOG.md" "26.08.15" "newbuild1" "fix: second"
head1="$(sed -n '1,8p' "${TMP}/CHANGELOG.md")"
echo "$head1" | grep -q 'newbuild1' || _fail "changelog newest build on top"
echo "$head1" | grep -q 'fix: second' || _fail "changelog newest notes on top"
grep -q 'uxvkolop' "${TMP}/CHANGELOG.md" || _fail "changelog keeps previous entry"
_pass "changelog prepend order"

# --- common.sh VERSION_ID alias sync ---
cat > "${TMP}/common.sh" <<'EOF'
VERSION_ID="oldbuild"
EOF
debopti_sync_common_alias "${TMP}/common.sh" "newbuild"
_assert_eq "$(grep '^VERSION_ID=' "${TMP}/common.sh" | cut -d'"' -f2)" "newbuild" "sync VERSION_ID alias"

# --- legacy tree verify via common.sh ---
mkdir -p "${TMP}/tree-legacy/scripts"
echo 'VERSION_ID="matchid"' > "${TMP}/tree-legacy/scripts/common.sh"
_run_rc debopti_verify_tree "${TMP}/tree-legacy" "" "matchid"
_assert_rc "$_RC" 0 "legacy tree verifies by VERSION_ID"

# =========================================================
# SemVer 2.0.0 + backward-compatible CalVer/Build tests
# =========================================================

# --- debopti_is_semver ---
_run_rc debopti_is_semver "1.0.0"
_assert_rc "$_RC" 0 "semver 1.0.0 valid"
_run_rc debopti_is_semver "0.1.0-rc.1+build.42"
_assert_rc "$_RC" 0 "semver with prerelease and build metadata valid"
_run_rc debopti_is_semver "01.0.0"
_assert_rc "$_RC" 1 "semver leading zero in major rejected"
_run_rc debopti_is_semver "1.0.0-01"
_assert_rc "$_RC" 1 "semver leading zero in numeric prerelease rejected"
_run_rc debopti_is_semver "26.08.15"
_assert_rc "$_RC" 1 "CalVer 26.08.15 rejected as semver"

# --- debopti_semver_compare (SemVer 2.0.0 §11) ---
_assert_eq "$(debopti_semver_compare "1.0.0" "1.0.0")" "0" "semver equal"
_assert_eq "$(debopti_semver_compare "1.0.0+build.1" "1.0.0+build.2")" "0" "semver build metadata ignored"
_assert_eq "$(debopti_semver_compare "1.0.0" "2.0.0")" "-1" "semver major lt"
_assert_eq "$(debopti_semver_compare "1.2.0" "1.1.9")" "1" "semver minor gt"
_assert_eq "$(debopti_semver_compare "1.0.1" "1.0.2")" "-1" "semver patch lt"
_assert_eq "$(debopti_semver_compare "1.0.0-alpha" "1.0.0")" "-1" "prerelease < release"
_assert_eq "$(debopti_semver_compare "1.0.0" "1.0.0-rc.1")" "1" "release > prerelease"
_assert_eq "$(debopti_semver_compare "1.0.0-alpha.1" "1.0.0-alpha.beta")" "-1" "numeric prerelease < lexical"
_assert_eq "$(debopti_semver_compare "1.0.0-alpha.2" "1.0.0-alpha.10")" "-1" "numeric prerelease compared numerically"
_assert_eq "$(debopti_semver_compare "1.0.0-alpha" "1.0.0-alpha.1")" "-1" "shorter prerelease < longer"

# --- debopti_next_semver ---
_assert_eq "$(debopti_next_semver "" "minor")" "1.0.0" "empty current bootstraps to 1.0.0"
_assert_eq "$(debopti_next_semver "" "minor" "rc.1")" "1.0.0-rc.1" "empty current bootstraps with prerelease"
_assert_eq "$(debopti_next_semver "1.2.3" "patch")" "1.2.4" "patch bump"
_assert_eq "$(debopti_next_semver "1.2.3" "minor")" "1.3.0" "minor bump"
_assert_eq "$(debopti_next_semver "1.2.3" "major")" "2.0.0" "major bump"
_assert_eq "$(debopti_next_semver "1.2.3-rc.1" "minor")" "1.3.0" "minor bump strips prerelease"
_assert_eq "$(debopti_next_semver "1.2.3" "minor" "rc.1")" "1.3.0-rc.1" "minor bump with prerelease"
_assert_eq "$(debopti_next_semver "1.2.3-rc.1" "prerelease")" "1.2.3-rc.2" "prerelease increments numeric suffix"

# --- debopti_semver_tag & debopti_tag_name disambiguation ---
_assert_eq "$(debopti_semver_tag "1.2.3")" "v1.2.3" "semver_tag adds v"
_assert_eq "$(debopti_semver_tag "v1.2.3-rc.1")" "v1.2.3-rc.1" "semver_tag normalizes v prefix"
_assert_eq "$(debopti_tag_name "1.2.3" "abcd1234")" "v1.2.3" "tag_name emits clean SemVer tag without build"
_assert_eq "$(debopti_tag_name "1.2.3-rc.1" "abcd1234")" "v1.2.3-rc.1" "tag_name emits clean SemVer prerelease tag"
_assert_eq "$(debopti_tag_name "26.11.21" "abcd1234")" "v26.11.21-abcd1234" "tag_name preserves Oct-Dec CalVer + build"
_assert_eq "$(debopti_tag_name "26.12.31" "abcd1234")" "v26.12.31-abcd1234" "tag_name preserves Dec CalVer + build"

# --- dual-key SemVer + CalVer write, parse, canonical & legacy compatibility ---
_legacy_calver260817_parse() {
    local text=$1
    local DEBOPTI_VERSION_REGEX='^[0-9]{2}\.[0-9]{2}\.[0-9]{2}$'
    local DEBOPTI_BUILD_REGEX='^[a-z0-9]+$'
    local calver="" version="" build=""
    calver="$(_debopti_kv "$text" "DEBOPTI_CALVER" || true)"
    version="$(_debopti_kv "$text" "DEBOPTI_VERSION" || true)"
    build="$(_debopti_kv "$text" "DEBOPTI_BUILD" || true)"
    [[ -n "$calver" ]] && version="$calver"
    [[ -n "$version" ]] && version="$(debopti_normalize_calver "$version")"
    if [[ -n "$version" && ! "$version" =~ $DEBOPTI_VERSION_REGEX ]]; then
        return 1
    fi
    if [[ -z "$build" || ! "$build" =~ $DEBOPTI_BUILD_REGEX ]]; then
        return 1
    fi
    printf '%s|%s\n' "$version" "$build"
}

debopti_write_version_file "${TMP}/semver_out/VERSION" "26.11.21" "deadbeef" "1.2.0"
sv_written="$(cat "${TMP}/semver_out/VERSION")"
echo "$sv_written" | grep -q '^DEBOPTI_SEMVER="1.2.0"' || _fail "written SemVer VERSION has DEBOPTI_SEMVER"
echo "$sv_written" | grep -q '^DEBOPTI_CALVER="26.11.21"' || _fail "written SemVer VERSION has DEBOPTI_CALVER"
echo "$sv_written" | grep -q '^DEBOPTI_BUILD="deadbeef"' || _fail "written SemVer VERSION has DEBOPTI_BUILD"
if echo "$sv_written" | grep -q '^DEBOPTI_VERSION='; then
    _fail "written SemVer VERSION must omit DEBOPTI_VERSION"
else
    _pass "written SemVer VERSION omits DEBOPTI_VERSION"
fi

_run_rc debopti_version_file_canonical "${TMP}/semver_out/VERSION"
_assert_rc "$_RC" 0 "dual-key SemVer VERSION is canonical"

if full="$(debopti_parse_version_file_full "${TMP}/semver_out/VERSION")"; then
    _assert_eq "$(_field "$full" 1)" "1.2.0" "full parse semver"
    _assert_eq "$(_field "$full" 2)" "26.11.21" "full parse calver"
    _assert_eq "$(_field "$full" 3)" "deadbeef" "full parse build"
else
    _fail "debopti_parse_version_file_full should succeed"
fi

if parsed="$(debopti_parse_version_file "${TMP}/semver_out/VERSION")"; then
    _assert_eq "$(_field "$parsed" 1)" "1.2.0" "2-field parse prefers DEBOPTI_SEMVER"
    _assert_eq "$(_field "$parsed" 2)" "deadbeef" "2-field parse build"
else
    _fail "debopti_parse_version_file should succeed on dual-key VERSION"
fi

if parsed="$(_legacy_v202608161_parse "$sv_written")"; then
    _assert_eq "$(_field "$parsed" 1)" "" "v2026.08.16.1 updater ignores DEBOPTI_SEMVER/CALVER"
    _assert_eq "$(_field "$parsed" 2)" "deadbeef" "v2026.08.16.1 updater reads Build"
else
    _fail "v2026.08.16.1 updater must parse dual-key SemVer VERSION"
fi

if parsed="$(_legacy_calver260817_parse "$sv_written")"; then
    _assert_eq "$(_field "$parsed" 1)" "26.11.21" "CalVer-only 26.08.17 updater reads DEBOPTI_CALVER"
    _assert_eq "$(_field "$parsed" 2)" "deadbeef" "CalVer-only 26.08.17 updater reads Build"
else
    _fail "CalVer-only 26.08.17 updater must parse dual-key SemVer VERSION"
fi

unset DEBOPTI_SEMVER DEBOPTI_CALVER DEBOPTI_VERSION DEBOPTI_BUILD VERSION_ID
debopti_load_version "${TMP}/semver_out"
_assert_eq "${DEBOPTI_SEMVER}" "1.2.0" "load_version sets DEBOPTI_SEMVER"
_assert_eq "${DEBOPTI_CALVER}" "26.11.21" "load_version sets DEBOPTI_CALVER"
_assert_eq "${DEBOPTI_VERSION}" "1.2.0" "load_version sets DEBOPTI_VERSION to semver"
_assert_eq "${DEBOPTI_BUILD}" "deadbeef" "load_version sets DEBOPTI_BUILD"

# --- dual-key debopti_verify_tree ---
_run_rc debopti_verify_tree "${TMP}/semver_out" "1.2.0" "deadbeef"
_assert_rc "$_RC" 0 "verify_tree accepts matching SemVer"
_run_rc debopti_verify_tree "${TMP}/semver_out" "26.11.21" "deadbeef"
_assert_rc "$_RC" 0 "verify_tree accepts matching CalVer"
_run_rc debopti_verify_tree "${TMP}/semver_out" "9.9.9" "deadbeef"
_assert_rc "$_RC" 1 "verify_tree rejects mismatched version"
_run_rc debopti_verify_tree "${TMP}/semver_out" "1.2.0" "wrongbld"
_assert_rc "$_RC" 1 "verify_tree rejects mismatched build"

# --- invalid DEBOPTI_SEMVER rejected ---
cat > "${TMP}/bad_semver" <<'EOF'
DEBOPTI_SEMVER="01.2.3"
DEBOPTI_CALVER="26.11.21"
DEBOPTI_BUILD="deadbeef"
EOF
_run_rc debopti_version_file_canonical "${TMP}/bad_semver"
_assert_rc "$_RC" 1 "invalid DEBOPTI_SEMVER rejected by canonical check"

# --- debopti_changelog_extract ---
cat > "${TMP}/KAC_CHANGELOG.md" <<'EOF'
# Changelog

## [Unreleased]

## [1.2.0] - 2026-11-21

### Added

- Add SemVer 2.0.0 support.

### Fixed

- Fix tag disambiguation.

## [1.1.0] - 2026-10-01

### Fixed

- Earlier fix.

[Unreleased]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.2.0...HEAD
[1.2.0]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.1.0...v1.2.0
EOF
extracted="$(debopti_changelog_extract "${TMP}/KAC_CHANGELOG.md" "1.2.0")"
echo "$extracted" | grep -q 'Add SemVer 2.0.0 support.' || _fail "changelog_extract includes Added item"
echo "$extracted" | grep -q 'Fix tag disambiguation.' || _fail "changelog_extract includes Fixed item"
if echo "$extracted" | grep -q 'Earlier fix.'; then
    _fail "changelog_extract must stop before next heading"
else
    _pass "changelog_extract stops before next heading"
fi
extracted_prev="$(debopti_changelog_extract "${TMP}/KAC_CHANGELOG.md" "1.1.0")"
if echo "$extracted_prev" | grep -q 'compare/v1.2.0'; then
    _fail "changelog_extract must stop before ref links"
else
    _pass "changelog_extract stops before ref links"
fi
if debopti_changelog_extract "${TMP}/KAC_CHANGELOG.md" "9.9.9" >/dev/null 2>&1; then
    _fail "changelog_extract should fail for missing version"
else
    _pass "changelog_extract rejects missing version"
fi

if [[ "$FAILS" -ne 0 ]]; then
    echo "version-test: ${FAILS} failure(s)" >&2
    exit 1
fi
echo "version-test: all passed"
