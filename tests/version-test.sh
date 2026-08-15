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
DEBOPTI_VERSION="2026.08.15.1"
DEBOPTI_BUILD="uxvkolop"
EOF
if parsed="$(debopti_parse_version_file "${TMP}/VERSION")"; then
    _assert_eq "$(_field "$parsed" 1)" "2026.08.15.1" "parse version"
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

echo 'DEBOPTI_VERSION="2026.08.15.1"' > "${TMP}/bad2"
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

# --- next CalVer ---
_assert_eq "$(debopti_next_calver "" "2026.08.15")" "2026.08.15.1" "empty existing -> today.1"
_assert_eq "$(debopti_next_calver "2026.08.15.1" "2026.08.15")" "2026.08.15.2" "same day increments N"
_assert_eq "$(debopti_next_calver "2026.08.15.9" "2026.08.16")" "2026.08.16.1" "new day resets N"
_assert_eq "$(debopti_next_calver "2026.08.14.3" "2026.08.15")" "2026.08.15.1" "previous day -> today.1"

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
_assert_eq "$(debopti_format_display "2026.08.15.1" "uxvkolop")" "2026.08.15.1 (uxvkolop)" "display format"
_assert_eq "$(debopti_format_prompt "2026.08.15.1" "uxvkolop")" "2026.08.15.1 (build uxvkolop)" "prompt format"
_assert_eq "$(debopti_tag_name "2026.08.15.1")" "v2026.08.15.1" "tag name"

# --- write + load ---
debopti_write_version_file "${TMP}/out/VERSION" "2026.08.15.3" "abc123xy"
_assert_eq "$(_field "$(debopti_parse_version_file "${TMP}/out/VERSION")" 1)" "2026.08.15.3" "written version"
unset DEBOPTI_VERSION DEBOPTI_BUILD VERSION_ID
debopti_load_version "${TMP}/out"
_assert_eq "${DEBOPTI_VERSION}" "2026.08.15.3" "load version"
_assert_eq "${DEBOPTI_BUILD}" "abc123xy" "load build"
_assert_eq "${VERSION_ID}" "abc123xy" "VERSION_ID alias"

# --- extract tree verification ---
mkdir -p "${TMP}/tree-ok" "${TMP}/tree-bad"
debopti_write_version_file "${TMP}/tree-ok/VERSION" "2026.08.15.1" "matchid"
debopti_write_version_file "${TMP}/tree-bad/VERSION" "2026.08.15.1" "otherid"
_run_rc debopti_verify_tree "${TMP}/tree-ok" "2026.08.15.1" "matchid"
_assert_rc "$_RC" 0 "matching tree verifies"
_run_rc debopti_verify_tree "${TMP}/tree-bad" "2026.08.15.1" "matchid"
_assert_rc "$_RC" 1 "mismatched extract metadata rejected"
_run_rc debopti_verify_tree "${TMP}/missing" "2026.08.15.1" "matchid"
_assert_rc "$_RC" 1 "missing VERSION in tree rejected"

# --- changelog prepend ---
debopti_changelog_prepend "${TMP}/CHANGELOG.md" "2026.08.15.1" "uxvkolop" "feat: first"
debopti_changelog_prepend "${TMP}/CHANGELOG.md" "2026.08.15.2" "newbuild1" "fix: second"
head1="$(sed -n '1,8p' "${TMP}/CHANGELOG.md")"
echo "$head1" | grep -q '2026.08.15.2' || _fail "changelog newest version on top"
echo "$head1" | grep -q 'fix: second' || _fail "changelog newest notes on top"
grep -q '2026.08.15.1' "${TMP}/CHANGELOG.md" || _fail "changelog keeps previous entry"
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

if [[ "$FAILS" -ne 0 ]]; then
    echo "version-test: ${FAILS} failure(s)" >&2
    exit 1
fi
echo "version-test: all passed"
