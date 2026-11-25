#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BACKUP_SCRIPT="${REPO_ROOT}/templates/apps/podman/restic-backup.sh"

# 1. Syntax check on template script
bash -n "$BACKUP_SCRIPT"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

MOCK_RESTIC="${TMP_DIR}/mock-restic"
cat > "$MOCK_RESTIC" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "${MOCK_RESTIC_LOG:?}"
subcmd="${1:-}"
if [[ "$subcmd" == "backup" ]]; then
    exit "${MOCK_RESTIC_BACKUP_RC:-0}"
fi
exit 0
EOF
chmod 0755 "$MOCK_RESTIC"

EXCLUDES="${TMP_DIR}/restic-excludes"
cp "${REPO_ROOT}/templates/apps/podman/restic-excludes.txt" "$EXCLUDES"

assert_contains() {
    local file="$1"
    local needle="$2"
    local msg="$3"
    if ! grep -Fq -- "$needle" "$file"; then
        printf 'FAIL: %s (missing "%s" in %s)\n' "$msg" "$needle" "$file" >&2
        cat "$file" >&2 || true
        exit 1
    fi
}

assert_not_contains() {
    local file="$1"
    local needle="$2"
    local msg="$3"
    if grep -Fq -- "$needle" "$file"; then
        printf 'FAIL: %s (unexpected "%s" in %s)\n' "$msg" "$needle" "$file" >&2
        cat "$file" >&2 || true
        exit 1
    fi
}

# Case 1: rc=0 -> runs backup AND forget --prune, exits 0
LOG1="${TMP_DIR}/case1.log"
ERR1="${TMP_DIR}/case1.err"
: > "$LOG1"
rc=0
RESTIC_BIN="$MOCK_RESTIC" \
RESTIC_BACKUP_TARGET="$TMP_DIR" \
RESTIC_EXCLUDE_FILE="$EXCLUDES" \
MOCK_RESTIC_LOG="$LOG1" \
MOCK_RESTIC_BACKUP_RC=0 \
bash "$BACKUP_SCRIPT" --tag unit-test 2>"$ERR1" || rc=$?
[[ "$rc" -eq 0 ]] || { printf 'FAIL: case 1 expected rc=0, got %s\n' "$rc" >&2; exit 1; }
assert_contains "$LOG1" "backup ${TMP_DIR} --exclude-file=${EXCLUDES} --tag unit-test" "case 1 backup invocation"
assert_contains "$LOG1" "forget --prune --keep-daily 7 --keep-weekly 4 --keep-monthly 6" "case 1 forget --prune invocation"

# Case 2: rc=3 -> logs warning mentioning exit code 3, no ERR trap abort, STILL runs forget --prune, exits 0
LOG2="${TMP_DIR}/case2.log"
ERR2="${TMP_DIR}/case2.err"
: > "$LOG2"
rc=0
RESTIC_BIN="$MOCK_RESTIC" \
RESTIC_BACKUP_TARGET="$TMP_DIR" \
RESTIC_EXCLUDE_FILE="$EXCLUDES" \
MOCK_RESTIC_LOG="$LOG2" \
MOCK_RESTIC_BACKUP_RC=3 \
bash "$BACKUP_SCRIPT" 2>"$ERR2" || rc=$?
[[ "$rc" -eq 0 ]] || { printf 'FAIL: case 2 expected rc=0, got %s\n' "$rc" >&2; exit 1; }
assert_contains "$ERR2" "code 3" "case 2 warning mentions exit code 3"
assert_not_contains "$ERR2" "aborted at line" "case 2 must not trigger ERR trap abort"
assert_contains "$LOG2" "backup ${TMP_DIR} --exclude-file=${EXCLUDES}" "case 2 backup invocation"
assert_contains "$LOG2" "forget --prune --keep-daily 7 --keep-weekly 4 --keep-monthly 6" "case 2 must still run forget --prune"

# Case 3: rc=1 -> fatal error, aborts BEFORE forget --prune, exits 1
LOG3="${TMP_DIR}/case3.log"
ERR3="${TMP_DIR}/case3.err"
: > "$LOG3"
rc=0
RESTIC_BIN="$MOCK_RESTIC" \
RESTIC_BACKUP_TARGET="$TMP_DIR" \
RESTIC_EXCLUDE_FILE="$EXCLUDES" \
MOCK_RESTIC_LOG="$LOG3" \
MOCK_RESTIC_BACKUP_RC=1 \
bash "$BACKUP_SCRIPT" 2>"$ERR3" || rc=$?
[[ "$rc" -eq 1 ]] || { printf 'FAIL: case 3 expected rc=1, got %s\n' "$rc" >&2; exit 1; }
assert_contains "$LOG3" "backup ${TMP_DIR} --exclude-file=${EXCLUDES}" "case 3 backup invocation"
assert_not_contains "$LOG3" "forget --prune" "case 3 must abort before forget --prune"

# Case 4: RESTIC_SKIP_PRUNE=1 -> skips forget --prune even when rc=0
LOG4="${TMP_DIR}/case4.log"
ERR4="${TMP_DIR}/case4.err"
: > "$LOG4"
rc=0
RESTIC_BIN="$MOCK_RESTIC" \
RESTIC_BACKUP_TARGET="$TMP_DIR" \
RESTIC_EXCLUDE_FILE="$EXCLUDES" \
RESTIC_SKIP_PRUNE=1 \
MOCK_RESTIC_LOG="$LOG4" \
MOCK_RESTIC_BACKUP_RC=0 \
bash "$BACKUP_SCRIPT" 2>"$ERR4" || rc=$?
[[ "$rc" -eq 0 ]] || { printf 'FAIL: case 4 expected rc=0, got %s\n' "$rc" >&2; exit 1; }
assert_contains "$LOG4" "backup ${TMP_DIR} --exclude-file=${EXCLUDES}" "case 4 backup invocation"
assert_not_contains "$LOG4" "forget --prune" "case 4 skips forget --prune"

printf 'PASS: restic-backup-test.sh (4/4 cases passed)\n'
