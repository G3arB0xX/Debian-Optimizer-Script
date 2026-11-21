#!/bin/bash
# =========================================================
# Unprivileged Host Parity Tests: Pure Rust CLI vs Bash
# Covers: version, config, template, f2b-sync multicall, CI non-TTY
# =========================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v cargo >/dev/null 2>&1; then
    if [[ "${CI:-}" == "true" ]]; then
        printf '[FAIL] cargo not installed in CI environment\n' >&2
        exit 1
    fi
    printf 'SKIP: cargo not installed; skipping Rust parity suite\n'
    exit 0
fi

TMP_ROOT="$(mktemp -d)"
cleanup() {
    rm -rf "$TMP_ROOT"
}
trap cleanup EXIT

PASS_COUNT=0
FAIL_COUNT=0

assert_eq() {
    local expected="$1"
    local actual="$2"
    local msg="$3"
    if [[ "$expected" == "$actual" ]]; then
        PASS_COUNT=$((PASS_COUNT + 1))
        printf '[PASS] %s\n' "$msg"
    else
        FAIL_COUNT=$((FAIL_COUNT + 1))
        printf '[FAIL] %s (expected=%q actual=%q)\n' "$msg" "$expected" "$actual" >&2
    fi
}

assert_contains() {
    local file="$1"
    local pattern="$2"
    local msg="$3"
    if grep -qF -- "$pattern" "$file"; then
        PASS_COUNT=$((PASS_COUNT + 1))
        printf '[PASS] %s\n' "$msg"
    else
        FAIL_COUNT=$((FAIL_COUNT + 1))
        printf '[FAIL] %s (missing %q in %s)\n' "$msg" "$pattern" "$file" >&2
    fi
}

assert_not_contains() {
    local file="$1"
    local pattern="$2"
    local msg="$3"
    if ! grep -qF -- "$pattern" "$file"; then
        PASS_COUNT=$((PASS_COUNT + 1))
        printf '[PASS] %s\n' "$msg"
    else
        FAIL_COUNT=$((FAIL_COUNT + 1))
        printf '[FAIL] %s (unexpected %q in %s)\n' "$msg" "$pattern" "$file" >&2
    fi
}

assert_files_identical() {
    local file_a="$1"
    local file_b="$2"
    local msg="$3"
    if cmp -s "$file_a" "$file_b"; then
        PASS_COUNT=$((PASS_COUNT + 1))
        printf '[PASS] %s\n' "$msg"
    else
        FAIL_COUNT=$((FAIL_COUNT + 1))
        printf '[FAIL] %s (files differ: %s vs %s)\n' "$msg" "$file_a" "$file_b" >&2
        diff -u "$file_a" "$file_b" >&2 || true
    fi
}

# Build debug debopty binary (offline automatically when local cache resolves)
CARGO_OFFLINE_ARGS=()
if [[ "${DEBOPTY_CARGO_OFFLINE:-0}" == "1" ]] || cargo metadata --manifest-path "${REPO_ROOT}/rust/Cargo.toml" --offline --format-version 1 >/dev/null 2>&1; then
    CARGO_OFFLINE_ARGS=(--offline)
fi

cargo build --manifest-path "${REPO_ROOT}/rust/Cargo.toml" "${CARGO_OFFLINE_ARGS[@]}" --bin debopty >/dev/null
TARGET_DIR="${CARGO_TARGET_DIR:-${REPO_ROOT}/rust/target}"
DEBOPTY_BIN="${TARGET_DIR}/debug/debopty"
if [[ ! -x "$DEBOPTY_BIN" ]]; then
    printf '[FAIL] compiled binary missing at %s\n' "$DEBOPTY_BIN" >&2
    exit 1
fi

# =========================================================
# Parity Test 1: version (Rust debopty version vs scripts/version.sh)
# =========================================================
# shellcheck source=../scripts/version.sh
source "${REPO_ROOT}/scripts/version.sh"

IFS='|' read -r repo_ver repo_build <<< "$(debopti_parse_version_file "${REPO_ROOT}/VERSION")"
bash_repo_display="$(debopti_format_display "$repo_ver" "$repo_build")"
rust_repo_display="$("$DEBOPTY_BIN" version)"
assert_eq "$bash_repo_display" "$rust_repo_display" "Parity 1a: debopty version matches scripts/version.sh on repo VERSION"

FIX_SEMVER="${TMP_ROOT}/VERSION.semver"
cat > "$FIX_SEMVER" << 'EOF'
DEBOPTI_SEMVER="2.5.0-rc.1"
DEBOPTI_CALVER="26.11.21"
DEBOPTI_BUILD="a1b2c3d4"
EOF
IFS='|' read -r fix_ver fix_build <<< "$(debopti_parse_version_file "$FIX_SEMVER")"
assert_eq "$(debopti_format_display "$fix_ver" "$fix_build")" "$("$DEBOPTY_BIN" version --file "$FIX_SEMVER")" \
    "Parity 1b: debopty version --file matches SemVer+CalVer+Build fixture"

FIX_CALVER="${TMP_ROOT}/VERSION.calver"
cat > "$FIX_CALVER" << 'EOF'
DEBOPTI_CALVER="2026.08.16.1"
DEBOPTI_BUILD="uxvkolop"
EOF
IFS='|' read -r cal_ver cal_build <<< "$(debopti_parse_version_file "$FIX_CALVER")"
assert_eq "$(debopti_format_display "$cal_ver" "$cal_build")" "$("$DEBOPTY_BIN" version --file "$FIX_CALVER")" \
    "Parity 1c: debopty version --file normalizes legacy CalVer identically"

FIX_LEGACY="${TMP_ROOT}/common.legacy.sh"
printf 'VERSION_ID="ff00aa11"\n' > "$FIX_LEGACY"
IFS='|' read -r leg_ver leg_build <<< "$(debopti_parse_version_file "$FIX_LEGACY")"
assert_eq "$(debopti_format_display "$leg_ver" "$leg_build")" "$("$DEBOPTY_BIN" version --file "$FIX_LEGACY")" \
    "Parity 1d: debopty version --file matches legacy VERSION_ID fallback"

# =========================================================
# Parity Test 2: config (Rust config set/get vs Bash source)
# =========================================================
CFG_FILE="${TMP_ROOT}/etc/debopti/debopti.conf"
"$DEBOPTY_BIN" --config "$CFG_FILE" config set IS_CN_REGION "true"
"$DEBOPTY_BIN" --config "$CFG_FILE" config set BASE_OPTIMIZED "true"
"$DEBOPTY_BIN" --config "$CFG_FILE" config set INSTALLED "true"
"$DEBOPTY_BIN" --config "$CFG_FILE" config set SCRIPT_VERSION "1.0.7"
"$DEBOPTY_BIN" --config "$CFG_FILE" config set SCRIPT_BUILD "231e05eb"
"$DEBOPTY_BIN" --config "$CFG_FILE" config set SSH_HARDENED "true"
"$DEBOPTY_BIN" --config "$CFG_FILE" config set EDITOR_CMD "micro"
"$DEBOPTY_BIN" --config "$CFG_FILE" config set PODMAN_GC_ENABLED "true"
"$DEBOPTY_BIN" --config "$CFG_FILE" config set YAZI_MEDIA_PREVIEW "false"
"$DEBOPTY_BIN" --config "$CFG_FILE" config set XRAY_GEO_AUTO_UPDATE "true"
"$DEBOPTY_BIN" --config "$CFG_FILE" config set CUSTOM_NOTE "hello world"

assert_contains "$CFG_FILE" "# debopty (Debian Optimizer Script) 全局持久化配置文件" \
    "Parity 2a: config set preserves embedded template comments"
assert_eq "hello world" "$("$DEBOPTY_BIN" --config "$CFG_FILE" config get CUSTOM_NOTE)" \
    "Parity 2b: config get returns written custom key"

bash_sourced="$(bash -c 'set -euo pipefail; source "$1"; printf "%s|%s|%s|%s|%s|%s|%s|%s|%s|%s|%s" \
    "$IS_CN_REGION" "$BASE_OPTIMIZED" "$INSTALLED" "$SCRIPT_VERSION" "$SCRIPT_BUILD" \
    "$SSH_HARDENED" "$EDITOR_CMD" "$PODMAN_GC_ENABLED" "$YAZI_MEDIA_PREVIEW" \
    "$XRAY_GEO_AUTO_UPDATE" "$CUSTOM_NOTE"' -- "$CFG_FILE")"
assert_eq "true|true|true|1.0.7|231e05eb|true|micro|true|false|true|hello world" "$bash_sourced" \
    "Parity 2c: Bash set -euo pipefail source parses all 10+1 keys identically"

# =========================================================
# Parity Test 3: template (byte-identical vs Bash render_template)
# =========================================================
TPL_DIR="${TMP_ROOT}/templates_parity"
mkdir -p "$TPL_DIR"

render_via_bash() {
    local rel="$1"
    local out="$2"
    shift 2
    # shellcheck disable=SC2016
    env SCRIPT_DIR="$REPO_ROOT" \
        INSTALL_DIR="/opt/debopty" \
        BIN_LINK="/usr/local/bin/debopty" \
        IS_CN_REGION="false" \
        DEBOPTI_VERSION="1.0.7" \
        DEBOPTI_BUILD="231e05eb" \
        VERSION_ID="231e05eb" \
        bash -c 'set -euo pipefail; source "${SCRIPT_DIR}/scripts/common.sh"; render_template "$@"' \
        -- "$rel" "$out" "$@"
}

render_via_bash "templates/common/debopty.sh" "${TPL_DIR}/bash_debopty.sh" "BIN_LINK=/usr/local/bin/debopty"
"$DEBOPTY_BIN" template render "templates/common/debopty.sh" \
    --out "${TPL_DIR}/rust_debopty.sh" --var "BIN_LINK=/usr/local/bin/debopty"
assert_files_identical "${TPL_DIR}/bash_debopty.sh" "${TPL_DIR}/rust_debopty.sh" \
    "Parity 3a: templates/common/debopty.sh renders byte-identically"

render_via_bash "templates/common/debopty.fish" "${TPL_DIR}/bash_debopty.fish" "BIN_LINK=/usr/local/bin/debopty"
"$DEBOPTY_BIN" template render "templates/common/debopty.fish" \
    --out "${TPL_DIR}/rust_debopty.fish" --var "BIN_LINK=/usr/local/bin/debopty"
assert_files_identical "${TPL_DIR}/bash_debopty.fish" "${TPL_DIR}/rust_debopty.fish" \
    "Parity 3b: templates/common/debopty.fish renders byte-identically"

render_via_bash "templates/security/nftables.conf" "${TPL_DIR}/bash_nftables.conf" \
    "SSH_PORT=50022" "NFT_BASE_D=/etc/nftables.d"
"$DEBOPTY_BIN" template render "templates/security/nftables.conf" \
    --out "${TPL_DIR}/rust_nftables.conf" \
    --var "SSH_PORT=50022" --var "NFT_BASE_D=/etc/nftables.d"
assert_files_identical "${TPL_DIR}/bash_nftables.conf" "${TPL_DIR}/rust_nftables.conf" \
    "Parity 3c: templates/security/nftables.conf renders byte-identically"

render_via_bash "templates/system/tmux.conf" "${TPL_DIR}/bash_tmux.conf" \
    "TMUX_DEFAULT_TERM=tmux-256color" "TMUX_MODERN_FEATURES=set -g extended-keys on"
"$DEBOPTY_BIN" template render "templates/system/tmux.conf" \
    --out "${TPL_DIR}/rust_tmux.conf" \
    --var "TMUX_DEFAULT_TERM=tmux-256color" --var "TMUX_MODERN_FEATURES=set -g extended-keys on"
assert_files_identical "${TPL_DIR}/bash_tmux.conf" "${TPL_DIR}/rust_tmux.conf" \
    "Parity 3d: templates/system/tmux.conf renders byte-identically"

TPL_LIST="$("$DEBOPTY_BIN" template list)"
TPL_COUNT="$(printf '%s\n' "$TPL_LIST" | wc -l | tr -d ' ')"
if (( TPL_COUNT >= 85 )); then
    PASS_COUNT=$((PASS_COUNT + 1))
    printf '[PASS] Parity 3e: embedded template count >= 85 (got %s)\n' "$TPL_COUNT"
else
    FAIL_COUNT=$((FAIL_COUNT + 1))
    printf '[FAIL] Parity 3e: embedded template count too low (got %s)\n' "$TPL_COUNT" >&2
fi
if ! printf '%s\n' "$TPL_LIST" | grep -q 'INDEX\.md'; then
    PASS_COUNT=$((PASS_COUNT + 1))
    printf '[PASS] Parity 3f: embedded template list contains zero INDEX.md files\n'
else
    FAIL_COUNT=$((FAIL_COUNT + 1))
    printf '[FAIL] Parity 3f: INDEX.md leaked into embedded templates\n' >&2
fi

# =========================================================
# Parity Test 4: f2b-sync multicall symlink vs Bash fail2ban-sync
# =========================================================
F2B_ROOT="${TMP_ROOT}/f2b_rootfs"
MULTICALL_BIN="${TMP_ROOT}/bin/debopty-f2b-sync"
mkdir -p "${TMP_ROOT}/bin" "${F2B_ROOT}/etc/nftables.d/debopti" "${F2B_ROOT}/etc/fail2ban/jail.d"
ln -s "$DEBOPTY_BIN" "$MULTICALL_BIN"

export DEBOPTY_F2B_ROOT="$F2B_ROOT"
export DEBOPTY_F2B_NO_SYS=1
export DEBOPTY_F2B_BACKEND="systemd"
export DEBOPTY_F2B_TEMPLATE_DIR="${REPO_ROOT}/templates/security"
export DEBOPTY_F2B_SYNC_BIN="$MULTICALL_BIN"
export SCRIPT_DIR="$REPO_ROOT"
export NFT_MAIN_CONF="${F2B_ROOT}/etc/nftables.conf"
export NFT_BASE_D="${F2B_ROOT}/etc/nftables.d"
export NFT_CONF_DIR="${NFT_BASE_D}/debopti"

# Minimal stubs for scripts/security.sh (mirrors tests/fail2ban-sync-test.sh)
info() { :; }
success() { :; }
warn() { :; }
err() { printf 'ERR: %s\n' "$1" >&2; }
render_template() {
    local tmpl="$1"
    local target="$2"
    shift 2
    [[ "$tmpl" != /* ]] && tmpl="${SCRIPT_DIR}/${tmpl}"
    local content
    content="$(cat "$tmpl")"
    content="${content//\{\{NFT_BASE_D\}\}/$NFT_BASE_D}"
    local pair key val
    for pair in "$@"; do
        key="${pair%%=*}"
        val="${pair#*=}"
        content="${content//\{\{${key}\}\}/$val}"
    done
    mkdir -p "$(dirname "$target")"
    printf '%s\n' "$content" > "$target"
}

# shellcheck source=../scripts/security.sh
source "${REPO_ROOT}/scripts/security.sh"

JAIL_PORTS="${F2B_ROOT}/etc/fail2ban/jail.d/99-debopty-ports.local"
GUARD_NFT="${NFT_BASE_D}/55-debopty-f2b-guard.nft"

# 4a. Initial SSH=2222
render_template "templates/security/nftables.conf" "$NFT_MAIN_CONF" "SSH_PORT=2222"
echo "include \"${NFT_CONF_DIR}/*.nft\"" > "${NFT_BASE_D}/50-debopti.nft"
"$MULTICALL_BIN"
assert_contains "$JAIL_PORTS" "port = 2222" "Parity 4a: multicall sets [sshd] port = 2222"
assert_contains "$GUARD_NFT" "tcp dport 2222 update @abuse_v4" "Parity 4b: multicall protects initial TCP SSH port"
assert_not_contains "$GUARD_NFT" "udp dport" "Parity 4c: multicall omits UDP rule when no UDP ports open"

# 4b. SSH port update + single/list/range rules via security.sh calling DEBOPTY_F2B_SYNC_BIN
update_ssh_firewall 50022
add_fw_rule "41641" "udp" "Tailscale_P2P"
add_fw_rule "80,443" "tcp/udp" "Caddy_Web"
add_fw_rule "11010-11015" "tcp/udp" "Easytier_P2P"

assert_contains "$JAIL_PORTS" "port = 80,443,11010:11015,41641,50022" "Parity 4d: multicall formats single, list, and colon-range"
assert_contains "$GUARD_NFT" "tcp dport { 80, 443, 11010 - 11015, 50022 } update @abuse_v4" "Parity 4e: multicall TCP guard set matches"
assert_contains "$GUARD_NFT" "udp dport { 80, 443, 11010 - 11015, 41641 } update @abuse_v6" "Parity 4f: multicall UDP IPv6 guard set matches"

# Exact --print-ports comparison against Bash debopty-f2b-sync.sh
bash_ports="$(bash "${REPO_ROOT}/templates/security/debopty-f2b-sync.sh" --print-ports)"
rust_ports="$("$MULTICALL_BIN" --print-ports)"
assert_eq "$bash_ports" "$rust_ports" "Parity 4g: multicall --print-ports matches Bash script verbatim"

# 4c. remove_fw_rule + idempotency mtime check
remove_fw_rule "Easytier_P2P"
remove_fw_rule "Tailscale_P2P"
assert_contains "$JAIL_PORTS" "port = 80,443,50022" "Parity 4h: remove_fw_rule retains remaining ports"

touch -d "2025-01-01 00:00:00" "$JAIL_PORTS" "$GUARD_NFT"
mtime_jail_before="$(stat -c %Y "$JAIL_PORTS")"
mtime_guard_before="$(stat -c %Y "$GUARD_NFT")"
"$MULTICALL_BIN"
assert_eq "$mtime_jail_before" "$(stat -c %Y "$JAIL_PORTS")" "Parity 4i: idempotent multicall leaves jail mtime unchanged"
assert_eq "$mtime_guard_before" "$(stat -c %Y "$GUARD_NFT")" "Parity 4j: idempotent multicall leaves guard mtime unchanged"

# 4d. Zero open ports + ss fallback excluding loopback/link-local
rm -f "$NFT_MAIN_CONF" "${NFT_CONF_DIR}"/*.nft
"$MULTICALL_BIN"
assert_contains "$JAIL_PORTS" "enabled = false" "Parity 4k: zero open ports disables public-ports jail"
if [[ ! -f "$GUARD_NFT" ]]; then
    PASS_COUNT=$((PASS_COUNT + 1))
    printf '[PASS] Parity 4l: zero open ports removes 55-debopty-f2b-guard.nft\n'
else
    FAIL_COUNT=$((FAIL_COUNT + 1))
    printf '[FAIL] Parity 4l: zero open ports did not remove 55-debopty-f2b-guard.nft\n' >&2
fi

SS_FIXTURE="${TMP_ROOT}/ss.txt"
cat > "$SS_FIXTURE" << 'EOF'
tcp LISTEN 0 128 0.0.0.0:8443 0.0.0.0:*
tcp LISTEN 0 128 127.0.0.1:9000 0.0.0.0:*
tcp LISTEN 0 128 [::1]:9001 [::]:*
udp UNCONN 0 0 [fe80::1%eth0]:5353 [::]:*
udp UNCONN 0 0 [::]:51820 [::]:*
EOF
bash_ss="$(DEBOPTY_F2B_SS_FILE="$SS_FIXTURE" bash "${REPO_ROOT}/templates/security/debopty-f2b-sync.sh" --print-ports)"
rust_ss="$(DEBOPTY_F2B_SS_FILE="$SS_FIXTURE" "$MULTICALL_BIN" --print-ports)"
assert_eq "$bash_ss" "$rust_ss" "Parity 4m: ss fallback excludes loopback/link-local identically to Bash"

# =========================================================
# Parity Test 5: CI non-TTY immediate clean exit
# =========================================================
ci_rc=0
CI=true DEBOPTY_ENGINE=rust "$DEBOPTY_BIN" </dev/null >/dev/null || ci_rc=$?
assert_eq "0" "$ci_rc" "Parity 5: CI=true debopty exits 0 immediately without blocking"

printf '\nSummary: %d passed, %d failed\n' "$PASS_COUNT" "$FAIL_COUNT"
(( FAIL_COUNT == 0 ))
