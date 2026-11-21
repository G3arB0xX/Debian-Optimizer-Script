#!/usr/bin/env bash
# Unprivileged offline unit tests for tools/release.sh, tools/release_helper.py,
# and tools/github-push.sh SemVer release integration.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RELEASE_CLI="${ROOT}/tools/release.sh"

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

_json_get() {
    local json_text=$1 key=$2
    printf '%s' "$json_text" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('$key',''))"
}

TMP="$(mktemp -d /tmp/debopty-release-test.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

_git() {
    local repo=$1
    shift
    git -C "$repo" -c commit.gpgsign=false -c user.name="Test Bot" -c user.email="bot@example.com" "$@"
}

_init_sandbox_repo() {
    local repo=$1
    mkdir -p "${repo}/scripts" "${repo}/tools"
    cp "${ROOT}/scripts/version.sh" "${repo}/scripts/version.sh"
    cp "${ROOT}/tools/release.sh" "${repo}/tools/release.sh"
    cp "${ROOT}/tools/release_helper.py" "${repo}/tools/release_helper.py"
    cp "${ROOT}/tools/github-push.sh" "${repo}/tools/github-push.sh"

    cat > "${repo}/VERSION" <<'EOF'
DEBOPTI_CALVER="26.08.17"
DEBOPTI_BUILD="initbuild1"
EOF
    cat > "${repo}/scripts/common.sh" <<'EOF'
#!/usr/bin/env bash
VERSION_ID="initbuild1"
EOF
    cat > "${repo}/CHANGELOG.md" <<'EOF'
# Changelog

## 26.08.17 (initbuild1)

- Build: initbuild1
- legacy CalVer entry
EOF

    _git "$repo" init -q -b main
    _git "$repo" add .
    _git "$repo" commit -q -m "chore: initial CalVer baseline"
}

REPO="${TMP}/repo"
_init_sandbox_repo "$REPO"

# --- 1. CalVer-only baseline passes verify & plan recommends bootstrap 1.0.0 ---
_run_rc bash "$RELEASE_CLI" --repo-root "$REPO" verify
_assert_rc "$_RC" 0 "CalVer-only baseline passes verify"

plan_json="$(bash "$RELEASE_CLI" --repo-root "$REPO" plan --json --date 2026-11-21)"
_assert_eq "$(_json_get "$plan_json" "recommended_bump")" "bootstrap" "CalVer baseline recommends bootstrap"
_assert_eq "$(_json_get "$plan_json" "next_semver")" "1.0.0" "bootstrap next_semver is 1.0.0"
_assert_eq "$(_json_get "$plan_json" "next_calver")" "26.11.21" "bootstrap next_calver is 26.11.21"
_assert_eq "$(_json_get "$plan_json" "tag")" "v1.0.0" "bootstrap tag is v1.0.0"

# --- 2. bump --dry-run makes zero file edits ---
before_ver="$(cat "${REPO}/VERSION")"
before_common="$(cat "${REPO}/scripts/common.sh")"
before_cl="$(cat "${REPO}/CHANGELOG.md")"
_run_rc bash "$RELEASE_CLI" --repo-root "$REPO" bump --dry-run --date 2026-11-21 >/dev/null
_assert_rc "$_RC" 0 "bump --dry-run exits 0"
_assert_eq "$(cat "${REPO}/VERSION")" "$before_ver" "bump --dry-run does not modify VERSION"
_assert_eq "$(cat "${REPO}/scripts/common.sh")" "$before_common" "bump --dry-run does not modify common.sh"
_assert_eq "$(cat "${REPO}/CHANGELOG.md")" "$before_cl" "bump --dry-run does not modify CHANGELOG.md"

# --- 3. Real bootstrap bump to 1.0.0 with curated [Unreleased] + commits ---
echo "a" > "${REPO}/a.txt"
_git "$REPO" add a.txt
_git "$REPO" commit -q -m "feat(tui): add interactive release status"
echo "b" > "${REPO}/b.txt"
_git "$REPO" add b.txt
_git "$REPO" commit -q -m "fix(fail2ban): sync public ports on reload"

_run_rc bash "$RELEASE_CLI" --repo-root "$REPO" bump --date 2026-11-21 --build rel100ab >/dev/null
_assert_rc "$_RC" 0 "bootstrap bump 1.0.0 succeeds"

grep -q '^DEBOPTI_SEMVER="1.0.0"$' "${REPO}/VERSION" || _fail "VERSION has DEBOPTI_SEMVER=1.0.0"
grep -q '^DEBOPTI_CALVER="26.11.21"$' "${REPO}/VERSION" || _fail "VERSION has DEBOPTI_CALVER=26.11.21"
grep -q '^DEBOPTI_BUILD="rel100ab"$' "${REPO}/VERSION" || _fail "VERSION has DEBOPTI_BUILD=rel100ab"
if grep -q '^DEBOPTI_VERSION=' "${REPO}/VERSION"; then
    _fail "VERSION must never contain DEBOPTI_VERSION="
else
    _pass "VERSION omits DEBOPTI_VERSION="
fi
grep -q '^VERSION_ID="rel100ab"$' "${REPO}/scripts/common.sh" || _fail "common.sh VERSION_ID synced"
grep -q '^## \[Unreleased\]$' "${REPO}/CHANGELOG.md" || _fail "CHANGELOG has ## [Unreleased]"
grep -q '^## \[1.0.0\] - 2026-11-21$' "${REPO}/CHANGELOG.md" || _fail "CHANGELOG has ## [1.0.0] - 2026-11-21"
grep -q '^### Added$' "${REPO}/CHANGELOG.md" || _fail "CHANGELOG has ### Added"
grep -q '^### Security$' "${REPO}/CHANGELOG.md" || _fail "CHANGELOG has ### Security (fail2ban scope)"
grep -q '^## 26.08.17 (initbuild1)$' "${REPO}/CHANGELOG.md" || _fail "CHANGELOG preserves legacy CalVer entry"
grep -q '^\[1.0.0\]: https://github.com/G3arB0xX/Debian-Optimizer-Script/releases/tag/v1.0.0$' "${REPO}/CHANGELOG.md" \
    || _fail "CHANGELOG has first SemVer tag link"
_pass "bootstrap bump updated VERSION, common.sh, CHANGELOG.md"

_run_rc bash "$RELEASE_CLI" --repo-root "$REPO" verify >/dev/null
_assert_rc "$_RC" 0 "verify passes post-bump"

_git "$REPO" add VERSION scripts/common.sh CHANGELOG.md
_git "$REPO" commit -q -m "chore(release): v1.0.0"
_git "$REPO" tag -a "v1.0.0" -m "Release v1.0.0"

# --- 4. notes extraction & --check-commits ---
notes_out="$(bash "$RELEASE_CLI" --repo-root "$REPO" notes --version 1.0.0)"
echo "$notes_out" | grep -q 'add interactive release status' || _fail "notes contains Added bullet"
echo "$notes_out" | grep -q 'Full Changelog' || _fail "notes appends Full Changelog link"
_pass "notes extracts clean release body"

_run_rc bash "$RELEASE_CLI" --repo-root "$REPO" verify --check-commits "HEAD~2..HEAD" >/dev/null
_assert_rc "$_RC" 0 "verify --check-commits accepts valid Conventional Commits"

# --- 5. Conventional Commits bump inference: docs -> none, fix -> patch, feat -> minor, feat! -> major ---
echo "doc" > "${REPO}/doc.txt"
_git "$REPO" add doc.txt
_git "$REPO" commit -q -m "docs: update usage notes"
plan_none="$(bash "$RELEASE_CLI" --repo-root "$REPO" plan --json)"
_assert_eq "$(_json_get "$plan_none" "recommended_bump")" "none" "docs-only recommends none"
_run_rc bash "$RELEASE_CLI" --repo-root "$REPO" bump >/dev/null 2>&1
_assert_rc "$_RC" 1 "bump refuses when recommended_bump is none"

echo "fix1" > "${REPO}/fix1.txt"
_git "$REPO" add fix1.txt
_git "$REPO" commit -q -m "fix(network): handle empty interface"
plan_patch="$(bash "$RELEASE_CLI" --repo-root "$REPO" plan --json)"
_assert_eq "$(_json_get "$plan_patch" "recommended_bump")" "patch" "fix recommends patch"
_assert_eq "$(_json_get "$plan_patch" "next_semver")" "1.0.1" "patch computes 1.0.1"

echo "feat1" > "${REPO}/feat1.txt"
_git "$REPO" add feat1.txt
_git "$REPO" commit -q -m "feat(tmux): add mouse toggle"
plan_minor="$(bash "$RELEASE_CLI" --repo-root "$REPO" plan --json)"
_assert_eq "$(_json_get "$plan_minor" "recommended_bump")" "minor" "feat recommends minor"
_assert_eq "$(_json_get "$plan_minor" "next_semver")" "1.1.0" "minor computes 1.1.0"

plan_pre="$(bash "$RELEASE_CLI" --repo-root "$REPO" plan --json --prerelease rc.1)"
_assert_eq "$(_json_get "$plan_pre" "next_semver")" "1.1.0-rc.1" "--prerelease rc.1 produces 1.1.0-rc.1"
_assert_eq "$(_json_get "$plan_pre" "is_prerelease")" "True" "is_prerelease is True for rc.1"

# Promote curated [Unreleased] item + bump to 1.1.0
python3 -c '
p = "'"${REPO}/CHANGELOG.md"'"
txt = open(p, encoding="utf-8").read()
txt = txt.replace("## [Unreleased]\n", "## [Unreleased]\n\n### Changed\n\n- Curated unreleased note.\n", 1)
open(p, "w", encoding="utf-8").write(txt)
'
_run_rc bash "$RELEASE_CLI" --repo-root "$REPO" bump --date 2026-11-22 >/dev/null
_assert_rc "$_RC" 0 "minor bump to 1.1.0 succeeds"
grep -q 'Curated unreleased note.' "${REPO}/CHANGELOG.md" || _fail "curated [Unreleased] promoted into 1.1.0"
grep -q '^\[1.1.0\]: https://github.com/G3arB0xX/Debian-Optimizer-Script/compare/v1.0.0...v1.1.0$' "${REPO}/CHANGELOG.md" \
    || _fail "1.1.0 compare link points from v1.0.0 to v1.1.0"
_pass "1.1.0 bump promoted [Unreleased] and wrote compare link"

_git "$REPO" add VERSION scripts/common.sh CHANGELOG.md
_git "$REPO" commit -q -m "chore(release): v1.1.0"
_git "$REPO" tag -a "v1.1.0" -m "Release v1.1.0"

# Breaking change -> major (on >=1.0.0)
echo "brk" > "${REPO}/brk.txt"
_git "$REPO" add brk.txt
_git "$REPO" commit -q -m "feat(cli)!: drop legacy flags"
plan_major="$(bash "$RELEASE_CLI" --repo-root "$REPO" plan --json)"
_assert_eq "$(_json_get "$plan_major" "recommended_bump")" "major" "feat! recommends major on 1.x"
_assert_eq "$(_json_get "$plan_major" "next_semver")" "2.0.0" "feat! computes 2.0.0"

# Breaking change via footer on 0.x -> minor
REPO0="${TMP}/repo0"
_init_sandbox_repo "$REPO0"
_run_rc bash "$RELEASE_CLI" --repo-root "$REPO0" bump --version 0.2.0 --date 2026-11-21 >/dev/null
_assert_rc "$_RC" 0 "set 0.2.0 baseline"
_git "$REPO0" add VERSION scripts/common.sh CHANGELOG.md
_git "$REPO0" commit -q -m "chore(release): v0.2.0"
_git "$REPO0" tag -a "v0.2.0" -m "Release v0.2.0"
echo "brk0" > "${REPO0}/brk0.txt"
_git "$REPO0" add brk0.txt
_git "$REPO0" commit -q -m "refactor(core): new config schema" -m "BREAKING CHANGE: drops old env vars"
plan_zero="$(bash "$RELEASE_CLI" --repo-root "$REPO0" plan --json)"
_assert_eq "$(_json_get "$plan_zero" "recommended_bump")" "minor" "BREAKING CHANGE recommends minor on 0.x"
_assert_eq "$(_json_get "$plan_zero" "next_semver")" "0.3.0" "BREAKING CHANGE on 0.2.0 computes 0.3.0"

# --- 6. verify rejects drift & bad commits ---
REPO_BAD="${TMP}/repo_bad"
cp -a "$REPO" "$REPO_BAD"
sed -i 's/^VERSION_ID=.*/VERSION_ID="drifted99"/' "${REPO_BAD}/scripts/common.sh"
_run_rc bash "$RELEASE_CLI" --repo-root "$REPO_BAD" verify >/dev/null 2>&1
_assert_rc "$_RC" 1 "verify rejects VERSION_ID drift"

cp -a "$REPO" "${TMP}/repo_bad_key"
echo 'DEBOPTI_VERSION="26.11.22"' >> "${TMP}/repo_bad_key/VERSION"
_run_rc bash "$RELEASE_CLI" --repo-root "${TMP}/repo_bad_key" verify >/dev/null 2>&1
_assert_rc "$_RC" 1 "verify rejects forbidden DEBOPTI_VERSION="

cp -a "$REPO" "${TMP}/repo_bad_sv"
sed -i 's/^DEBOPTI_SEMVER=.*/DEBOPTI_SEMVER="01.1.0"/' "${TMP}/repo_bad_sv/VERSION"
_run_rc bash "$RELEASE_CLI" --repo-root "${TMP}/repo_bad_sv" verify >/dev/null 2>&1
_assert_rc "$_RC" 1 "verify rejects invalid DEBOPTI_SEMVER"

cp -a "$REPO" "${TMP}/repo_bad_cl"
sed -i 's/^## \[1.1.0\]/## [9.9.9]/' "${TMP}/repo_bad_cl/CHANGELOG.md"
_run_rc bash "$RELEASE_CLI" --repo-root "${TMP}/repo_bad_cl" verify >/dev/null 2>&1
_assert_rc "$_RC" 1 "verify rejects missing CHANGELOG section for DEBOPTI_SEMVER"

echo "badmsg" > "${REPO_BAD}/badmsg.txt"
_git "$REPO_BAD" add badmsg.txt
_git "$REPO_BAD" commit -q -m "not a conventional commit message"
_run_rc bash "$RELEASE_CLI" --repo-root "$REPO_BAD" verify --check-commits "HEAD~1..HEAD" >/dev/null 2>&1
_assert_rc "$_RC" 1 "verify --check-commits rejects non-conventional commit"

# --- 7. tools/github-push.sh --dry-run preserves prepared SemVer identity ---
ver_before_push="$(cat "${REPO}/VERSION")"
cl_before_push="$(cat "${REPO}/CHANGELOG.md")"
push_out="$(DEBOPTY_WORKSPACE_DIR="$REPO" bash "${REPO}/tools/github-push.sh" --dry-run --skip-check 2>&1)"
echo "$push_out" | grep -q 'SemVer   : 1.1.0' || _fail "github-push --dry-run preserves prepared SemVer 1.1.0"
echo "$push_out" | grep -q 'Tag      : v1.1.0' || _fail "github-push --dry-run resolves clean tag v1.1.0"
_assert_eq "$(cat "${REPO}/VERSION")" "$ver_before_push" "github-push --dry-run does not clobber VERSION"
_assert_eq "$(cat "${REPO}/CHANGELOG.md")" "$cl_before_push" "github-push --dry-run does not clobber CHANGELOG.md"
_pass "github-push --dry-run preserves prepared SemVer identity"

if [[ "$FAILS" -ne 0 ]]; then
    echo "release-test: ${FAILS} failure(s)" >&2
    exit 1
fi
echo "release-test: all passed"
