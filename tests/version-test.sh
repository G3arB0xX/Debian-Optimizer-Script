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

if [[ "$FAILS" -ne 0 ]]; then
    echo "version-test: ${FAILS} failure(s)" >&2
    exit 1
fi
echo "version-test: all passed"
