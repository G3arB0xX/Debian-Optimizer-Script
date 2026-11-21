#!/usr/bin/env bash
# tmux version gating, backup/idempotency, uninstall restore, and config syntax test
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LEGACY_SCRIPTS="${ROOT}/scripts"
[[ -d "$LEGACY_SCRIPTS" ]] || LEGACY_SCRIPTS="${ROOT}/archive/legacy-bash/scripts"
export SCRIPT_DIR="$ROOT"
export DEBOPTY_TMUX_SKIP_APT=1

# shellcheck source=../archive/legacy-bash/scripts/common.sh
source "${LEGACY_SCRIPTS}/common.sh"
# shellcheck source=../archive/legacy-bash/scripts/system.sh
source "${LEGACY_SCRIPTS}/system.sh"

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

_assert_contains() {
    local file=$1 needle=$2 msg=$3
    if grep -Fq "$needle" "$file"; then
        _pass "$msg"
    else
        _fail "$msg (missing '$needle' in $file)"
    fi
}

_assert_not_contains() {
    local file=$1 needle=$2 msg=$3
    if grep -Fq "$needle" "$file"; then
        _fail "$msg (unexpected '$needle' in $file)"
    else
        _pass "$msg"
    fi
}

TMP="$(mktemp -d /tmp/debopty-tmux-test.XXXXXX)"
chmod 700 "$TMP"
trap 'rm -rf "$TMP"' EXIT

# --- 1. Version parsing & comparison ---
DEBOPTY_TMUX_VERSION_OVERRIDE="tmux 3.1c"
_assert_eq "$(_tmux_parse_version)" "3.1" "parse tmux 3.1c"
DEBOPTY_TMUX_VERSION_OVERRIDE="tmux 2.8"
_assert_eq "$(_tmux_parse_version)" "2.8" "parse tmux 2.8"
DEBOPTY_TMUX_VERSION_OVERRIDE="3.5a"
_assert_eq "$(_tmux_parse_version)" "3.5" "parse tmux 3.5a"

# --- 2. Version-gated config rendering across 2.8, 3.1c, 3.3a, 3.5a ---
for ver in "2.8" "3.1c"; do
    conf="${TMP}/tmux-${ver}.conf"
    DEBOPTY_TMUX_CONF="$conf" DEBOPTY_TMUX_VERSION_OVERRIDE="tmux ${ver}" setup_tmux >/dev/null 2>&1
    _assert_contains "$conf" "# managed-by-debopty" "${ver}: managed marker present"
    _assert_contains "$conf" "set -g mouse on" "${ver}: mouse enabled"
    _assert_contains "$conf" "set -g history-limit 50000" "${ver}: history-limit 50000"
    _assert_contains "$conf" "bind -n M-Left select-pane -L" "${ver}: Alt+Left pane nav"
    _assert_not_contains "$conf" "terminal-features" "${ver}: omits terminal-features"
    _assert_not_contains "$conf" "allow-passthrough" "${ver}: omits allow-passthrough"
done

for ver in "3.3a" "3.5a"; do
    conf="${TMP}/tmux-${ver}.conf"
    DEBOPTY_TMUX_CONF="$conf" DEBOPTY_TMUX_VERSION_OVERRIDE="tmux ${ver}" setup_tmux >/dev/null 2>&1
    _assert_contains "$conf" "# managed-by-debopty" "${ver}: managed marker present"
    _assert_contains "$conf" 'set -as terminal-features ",*:RGB"' "${ver}: includes terminal-features"
    _assert_contains "$conf" "set -g allow-passthrough on" "${ver}: includes allow-passthrough"
done

# --- 3. Backup, idempotency, and uninstall restore ---
custom_conf="${TMP}/custom.conf"
backup_conf="${custom_conf}.debopty.bak"
printf '%s\n' "# user custom tmux" "set -g prefix C-a" > "$custom_conf"

DEBOPTY_TMUX_CONF="$custom_conf" DEBOPTY_TMUX_VERSION_OVERRIDE="tmux 3.5a" setup_tmux >/dev/null 2>&1
if [[ -f "$backup_conf" ]]; then
    _pass "pre-existing custom config backed up"
else
    _fail "pre-existing custom config was not backed up"
fi
_assert_contains "$backup_conf" "set -g prefix C-a" "backup retains original custom content"
_assert_contains "$custom_conf" "# managed-by-debopty" "target replaced with managed config"

# Second run must be idempotent and NOT overwrite .debopty.bak
DEBOPTY_TMUX_CONF="$custom_conf" DEBOPTY_TMUX_VERSION_OVERRIDE="tmux 3.5a" setup_tmux >/dev/null 2>&1
_assert_contains "$backup_conf" "set -g prefix C-a" "idempotent setup preserves original backup"

# Uninstall restores backup and cleans .debopty.bak
DEBOPTY_TMUX_CONF="$custom_conf" uninstall_tmux >/dev/null 2>&1
if [[ ! -e "$backup_conf" ]]; then
    _pass "uninstall removes backup file after restore"
else
    _fail "uninstall left backup file behind"
fi
_assert_contains "$custom_conf" "set -g prefix C-a" "uninstall restores original custom config"

# Uninstall on unmanaged file without backup must leave it untouched
DEBOPTY_TMUX_CONF="$custom_conf" uninstall_tmux >/dev/null 2>&1
_assert_contains "$custom_conf" "set -g prefix C-a" "uninstall does not delete unmanaged config"

# Uninstall on managed file without backup removes it cleanly
managed_only="${TMP}/managed-only.conf"
DEBOPTY_TMUX_CONF="$managed_only" DEBOPTY_TMUX_VERSION_OVERRIDE="tmux 3.3a" setup_tmux >/dev/null 2>&1
DEBOPTY_TMUX_CONF="$managed_only" uninstall_tmux >/dev/null 2>&1
if [[ ! -e "$managed_only" ]]; then
    _pass "uninstall removes managed config when no backup existed"
else
    _fail "uninstall left managed config behind"
fi

# --- 4. Real tmux syntax validation if tmux binary exists ---
_run_tmux_parse_check() {
    local sock_name=$1
    local conf_file=$2
    local out
    if out=$(umask 077 && env -u TMUX TMUX_TMPDIR="$TMP" tmux -L "$sock_name" -f "$conf_file" start-server \; kill-server 2>&1); then
        return 0
    else
        echo "tmux parse output ($conf_file): $out" >&2
        return 1
    fi
}

if command -v tmux >/dev/null 2>&1; then
    host_conf="${TMP}/host.conf"
    DEBOPTY_TMUX_CONF="$host_conf" DEBOPTY_TMUX_VERSION_OVERRIDE="" setup_tmux >/dev/null 2>&1
    if _run_tmux_parse_check "debopty-test-$$" "$host_conf"; then
        _pass "host tmux parses rendered host config with 0 errors"
    else
        _fail "host tmux failed to parse rendered host config"
    fi
    if _run_tmux_parse_check "debopty-test-legacy-$$" "${TMP}/tmux-2.8.conf"; then
        _pass "host tmux parses rendered 2.8 config with 0 errors"
    else
        _fail "host tmux failed to parse rendered 2.8 config"
    fi
else
    _pass "tmux binary not present on host; skipped live parser check"
fi

if [[ "$FAILS" -gt 0 ]]; then
    echo "FAILED: $FAILS test(s) failed" >&2
    exit 1
fi
echo "ALL TMUX TESTS PASSED"
