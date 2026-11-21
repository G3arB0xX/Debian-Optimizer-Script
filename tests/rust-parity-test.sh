#!/bin/bash
# =========================================================
# Unprivileged Host Parity Tests: Pure Rust CLI vs Bash
# Covers: version, config, template, f2b-sync multicall, CI non-TTY
# =========================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LEGACY_ROOT="${REPO_ROOT}/archive/legacy-bash"
[[ -d "${LEGACY_ROOT}/scripts" ]] || LEGACY_ROOT="${REPO_ROOT}"
LEGACY_SCRIPTS="${LEGACY_ROOT}/scripts"

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
# Parity Test 1: version (Rust debopty version vs archived scripts/version.sh)
# =========================================================
# shellcheck source=../archive/legacy-bash/scripts/version.sh
source "${LEGACY_SCRIPTS}/version.sh"

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
        LEGACY_SCRIPTS="$LEGACY_SCRIPTS" \
        INSTALL_DIR="/opt/debopty" \
        BIN_LINK="/usr/local/bin/debopty" \
        IS_CN_REGION="false" \
        DEBOPTI_VERSION="1.0.7" \
        DEBOPTI_BUILD="231e05eb" \
        VERSION_ID="231e05eb" \
        bash -c 'set -euo pipefail; source "${LEGACY_SCRIPTS}/common.sh"; render_template "$@"' \
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

# shellcheck source=../archive/legacy-bash/scripts/security.sh
source "${LEGACY_SCRIPTS}/security.sh"

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

# =========================================================
# Parity Test 6: system tmux-setup / tmux-uninstall vs scripts/system.sh
# =========================================================
TMUX_PARITY_DIR="${TMP_ROOT}/tmux_parity"
mkdir -p "$TMUX_PARITY_DIR"

for ver in "2.8" "3.5a"; do
    bash_conf="${TMUX_PARITY_DIR}/bash-${ver}.conf"
    rust_conf="${TMUX_PARITY_DIR}/rust-${ver}.conf"

    # shellcheck disable=SC2016
    env SCRIPT_DIR="$REPO_ROOT" \
        LEGACY_SCRIPTS="$LEGACY_SCRIPTS" \
        DEBOPTY_TMUX_SKIP_APT=1 \
        DEBOPTY_TMUX_CONF="$bash_conf" \
        DEBOPTY_TMUX_VERSION_OVERRIDE="tmux ${ver}" \
        bash -c 'set -euo pipefail; source "${LEGACY_SCRIPTS}/common.sh"; source "${LEGACY_SCRIPTS}/system.sh"; setup_tmux >/dev/null 2>&1'

    "$DEBOPTY_BIN" system tmux-setup \
        --conf "$rust_conf" \
        --skip-apt \
        --version-override "tmux ${ver}" >/dev/null

    assert_files_identical "$bash_conf" "$rust_conf" \
        "Parity 6a (${ver}): debopty system tmux-setup matches scripts/system.sh byte-for-byte"
done

RUST_CUSTOM_TMUX="${TMUX_PARITY_DIR}/custom.conf"
RUST_CUSTOM_BAK="${RUST_CUSTOM_TMUX}.debopty.bak"
printf '%s\n' "# user custom tmux" "set -g prefix C-a" > "$RUST_CUSTOM_TMUX"

"$DEBOPTY_BIN" system tmux-setup --conf "$RUST_CUSTOM_TMUX" --skip-apt --version-override "tmux 3.5a" >/dev/null
assert_contains "$RUST_CUSTOM_BAK" "set -g prefix C-a" "Parity 6b: Rust tmux-setup backs up unmanaged custom config"
assert_contains "$RUST_CUSTOM_TMUX" "# managed-by-debopty" "Parity 6c: Rust tmux-setup writes managed config"

"$DEBOPTY_BIN" system tmux-setup --conf "$RUST_CUSTOM_TMUX" --skip-apt --version-override "tmux 3.5a" >/dev/null
assert_contains "$RUST_CUSTOM_BAK" "set -g prefix C-a" "Parity 6d: Rust tmux-setup idempotency preserves original backup"

"$DEBOPTY_BIN" system tmux-uninstall --conf "$RUST_CUSTOM_TMUX" --skip-apt >/dev/null
if [[ ! -e "$RUST_CUSTOM_BAK" ]]; then
    PASS_COUNT=$((PASS_COUNT + 1))
    printf '[PASS] Parity 6e: Rust tmux-uninstall removes backup file after restore\n'
else
    FAIL_COUNT=$((FAIL_COUNT + 1))
    printf '[FAIL] Parity 6e: Rust tmux-uninstall left backup file behind\n' >&2
fi
assert_contains "$RUST_CUSTOM_TMUX" "set -g prefix C-a" "Parity 6f: Rust tmux-uninstall restores original custom config"

# =========================================================
# Parity Test 7: security fw-add / fw-remove / fw-update-ssh / ndp-patch vs scripts/security.sh
# =========================================================
SEC_BASH_ROOT="${TMP_ROOT}/sec_bash_root"
SEC_RUST_ROOT="${TMP_ROOT}/sec_rust_root"
mkdir -p "${SEC_BASH_ROOT}/etc/nftables.d/debopti" "${SEC_BASH_ROOT}/etc/fail2ban/jail.d"
mkdir -p "${SEC_RUST_ROOT}/etc/nftables.d/debopti" "${SEC_RUST_ROOT}/etc/fail2ban/jail.d"

# Seed identical initial nftables.conf (with rootfs-local NFT_BASE_D)
export DEBOPTY_F2B_ROOT="$SEC_BASH_ROOT"
export DEBOPTY_F2B_NO_SYS=1
export DEBOPTY_F2B_BACKEND="systemd"
export DEBOPTY_F2B_TEMPLATE_DIR="${REPO_ROOT}/templates/security"
export DEBOPTY_F2B_SYNC_BIN="${REPO_ROOT}/templates/security/debopty-f2b-sync.sh"
export NFT_MAIN_CONF="${SEC_BASH_ROOT}/etc/nftables.conf"
export NFT_BASE_D="${SEC_BASH_ROOT}/etc/nftables.d"
export NFT_CONF_DIR="${NFT_BASE_D}/debopti"

render_template "templates/security/nftables.conf" "$NFT_MAIN_CONF" "SSH_PORT=2222"
echo "include \"${NFT_CONF_DIR}/*.nft\"" > "${NFT_BASE_D}/50-debopti.nft"

update_ssh_firewall 50022
add_fw_rule "41641" "udp" "Tailscale_P2P"
add_fw_rule "80,443" "tcp/udp" "Caddy_Web"
add_fw_rule "11010-11015" "tcp/udp" "Easytier_P2P"

"$DEBOPTY_BIN" template render "templates/security/nftables.conf" \
    --out "${SEC_RUST_ROOT}/etc/nftables.conf" \
    --var "SSH_PORT=2222" \
    --var "NFT_BASE_D=${SEC_RUST_ROOT}/etc/nftables.d"
echo "include \"${SEC_RUST_ROOT}/etc/nftables.d/debopti/*.nft\"" > "${SEC_RUST_ROOT}/etc/nftables.d/50-debopti.nft"

env -u DEBOPTY_F2B_ROOT DEBOPTY_F2B_BACKEND="systemd" \
    "$DEBOPTY_BIN" security fw-update-ssh 50022 --root "$SEC_RUST_ROOT" --no-sys >/dev/null
env -u DEBOPTY_F2B_ROOT DEBOPTY_F2B_BACKEND="systemd" \
    "$DEBOPTY_BIN" security fw-add "41641" --proto "udp" --comment "Tailscale_P2P" --root "$SEC_RUST_ROOT" --no-sys >/dev/null
env -u DEBOPTY_F2B_ROOT DEBOPTY_F2B_BACKEND="systemd" \
    "$DEBOPTY_BIN" security fw-add "80,443" --proto "tcp/udp" --comment "Caddy_Web" --root "$SEC_RUST_ROOT" --no-sys >/dev/null
env -u DEBOPTY_F2B_ROOT DEBOPTY_F2B_BACKEND="systemd" \
    "$DEBOPTY_BIN" security fw-add "11010-11015" --proto "tcp/udp" --comment "Easytier_P2P" --root "$SEC_RUST_ROOT" --no-sys >/dev/null

for rule_name in "Tailscale_P2P.nft" "Caddy_Web.nft" "Easytier_P2P.nft"; do
    assert_files_identical \
        "${SEC_BASH_ROOT}/etc/nftables.d/debopti/${rule_name}" \
        "${SEC_RUST_ROOT}/etc/nftables.d/debopti/${rule_name}" \
        "Parity 7a (${rule_name}): Rust security fw-add matches Bash add_fw_rule byte-for-byte"
done

assert_files_identical \
    "${SEC_BASH_ROOT}/etc/fail2ban/jail.d/99-debopty-ports.local" \
    "${SEC_RUST_ROOT}/etc/fail2ban/jail.d/99-debopty-ports.local" \
    "Parity 7b: Rust security fw-add/fw-update-ssh Fail2ban ports file matches Bash"
assert_files_identical \
    "${SEC_BASH_ROOT}/etc/nftables.d/55-debopty-f2b-guard.nft" \
    "${SEC_RUST_ROOT}/etc/nftables.d/55-debopty-f2b-guard.nft" \
    "Parity 7c: Rust security fw-add/fw-update-ssh nftables guard file matches Bash"

# fw-remove parity
remove_fw_rule "Easytier_P2P"
env -u DEBOPTY_F2B_ROOT DEBOPTY_F2B_BACKEND="systemd" \
    "$DEBOPTY_BIN" security fw-remove "Easytier_P2P" --root "$SEC_RUST_ROOT" --no-sys >/dev/null

assert_files_identical \
    "${SEC_BASH_ROOT}/etc/fail2ban/jail.d/99-debopty-ports.local" \
    "${SEC_RUST_ROOT}/etc/fail2ban/jail.d/99-debopty-ports.local" \
    "Parity 7d: Rust security fw-remove matches Bash remove_fw_rule"

# ndp-patch parity on identical legacy nftables.conf
NDP_BASH_ROOT="${TMP_ROOT}/ndp_bash"
NDP_RUST_ROOT="${TMP_ROOT}/ndp_rust"
mkdir -p "${NDP_BASH_ROOT}/etc" "${NDP_RUST_ROOT}/etc"
cat > "${NDP_BASH_ROOT}/etc/nftables.conf" << 'EOF'
table inet filter {
    chain input {
        ip6 nexthdr icmpv6 icmpv6 type echo-request limit rate 5/second accept
        tcp dport 22 accept comment "SSH_Access_Port"
    }
}
EOF
cp "${NDP_BASH_ROOT}/etc/nftables.conf" "${NDP_RUST_ROOT}/etc/nftables.conf"

export NFT_MAIN_CONF="${NDP_BASH_ROOT}/etc/nftables.conf"
patch_nftables_ndp
env -u DEBOPTY_F2B_ROOT "$DEBOPTY_BIN" security ndp-patch --root "$NDP_RUST_ROOT" --no-sys >/dev/null

assert_files_identical \
    "${NDP_BASH_ROOT}/etc/nftables.conf" \
    "${NDP_RUST_ROOT}/etc/nftables.conf" \
    "Parity 7e: Rust security ndp-patch matches Bash patch_nftables_ndp byte-for-byte"

# =========================================================
# Parity Test 8: Phase-3 registry counts (21 native / 0 bridge) & --ui console
# =========================================================
STATUS_JSON="${TMP_ROOT}/status.json"
"$DEBOPTY_BIN" --config "${TMP_ROOT}/status_debopti.conf" status --json > "$STATUS_JSON"
read -r reg_native reg_bridge reg_total has_console <<< "$(python3 -B -c '
import json, sys
data = json.load(open(sys.argv[1]))
mods = data["modules"]
native = sum(1 for m in mods if m["implementation"] == "native-rust")
bridge = sum(1 for m in mods if m["implementation"] == "bash-bridge")
has_console = "console" in data.get("ui_backends", [])
print(native, bridge, len(mods), int(has_console))
' "$STATUS_JSON")"
assert_eq "21" "$reg_native" "Parity 8a: status --json reports 21 native-rust modules"
assert_eq "0" "$reg_bridge" "Parity 8b: status --json reports 0 bash-bridge modules"
assert_eq "21" "$reg_total" "Parity 8c: status --json reports 21 total modules"
assert_eq "1" "$has_console" "Parity 8d: status --json ui_backends includes console"

ui_console_rc=0
CI=true DEBOPTY_ENGINE=rust "$DEBOPTY_BIN" --ui console </dev/null >/dev/null || ui_console_rc=$?
assert_eq "0" "$ui_console_rc" "Parity 8e: CI=true debopty --ui console exits 0"

ui_ratatui_rc=0
CI=true DEBOPTY_ENGINE=rust "$DEBOPTY_BIN" --ui ratatui </dev/null >/dev/null 2>&1 || ui_ratatui_rc=$?
assert_eq "2" "$ui_ratatui_rc" "Parity 8f: default build debopty --ui ratatui exits code 2"

# =========================================================
# Parity Test 9: U5-3 CLI Hardening (P0-1 non-TTY ssh-harden lockout & P1-4 dns-check CN config)
# =========================================================
P9_SSH_ROOT="${TMP_ROOT}/p9_ssh_root"
mkdir -p "$P9_SSH_ROOT"

p9_ssh_rc=0
"$DEBOPTY_BIN" security ssh-harden --root "$P9_SSH_ROOT" --no-sys </dev/null >/dev/null 2>"${TMP_ROOT}/p9_ssh.err" || p9_ssh_rc=$?
if (( p9_ssh_rc != 0 )); then
    PASS_COUNT=$((PASS_COUNT + 1))
    printf '[PASS] Parity 9a: non-TTY debopty security ssh-harden fails closed (rc=%d)\n' "$p9_ssh_rc"
else
    FAIL_COUNT=$((FAIL_COUNT + 1))
    printf '[FAIL] Parity 9a: non-TTY debopty security ssh-harden unexpectedly exited 0\n' >&2
fi

if [[ ! -e "${P9_SSH_ROOT}/opt/debopty/test_key" && ! -e "${P9_SSH_ROOT}/etc/ssh/sshd_config" ]]; then
    PASS_COUNT=$((PASS_COUNT + 1))
    printf '[PASS] Parity 9b: non-TTY ssh-harden refusal leaves zero test_key or sshd_config mutations\n'
else
    FAIL_COUNT=$((FAIL_COUNT + 1))
    printf '[FAIL] Parity 9b: non-TTY ssh-harden mutated files under %s\n' "$P9_SSH_ROOT" >&2
fi

p9_ni_nopub_rc=0
"$DEBOPTY_BIN" security ssh-harden --non-interactive --root "$P9_SSH_ROOT" --no-sys </dev/null >/dev/null 2>&1 || p9_ni_nopub_rc=$?
if (( p9_ni_nopub_rc != 0 )) && [[ ! -e "${P9_SSH_ROOT}/opt/debopty/test_key" ]]; then
    PASS_COUNT=$((PASS_COUNT + 1))
    printf '[PASS] Parity 9c: --non-interactive ssh-harden without --pubkey refuses ephemeral key generation\n'
else
    FAIL_COUNT=$((FAIL_COUNT + 1))
    printf '[FAIL] Parity 9c: --non-interactive ssh-harden without --pubkey did not fail safely (rc=%d)\n' "$p9_ni_nopub_rc" >&2
fi

P9_CFG_CN="${TMP_ROOT}/p9_cn.conf"
P9_CFG_GLOBAL="${TMP_ROOT}/p9_global.conf"
P9_RESOLV="${TMP_ROOT}/p9_resolv.conf"
printf 'IS_CN_REGION="true"\n' > "$P9_CFG_CN"
printf 'IS_CN_REGION="false"\n' > "$P9_CFG_GLOBAL"
printf 'nameserver 9.9.9.9\n' > "$P9_RESOLV"

p9_cn_flag="$("$DEBOPTY_BIN" --config "$P9_CFG_CN" network dns-check --mode calibrate --resolv "$P9_RESOLV" --json | python3 -B -c 'import json,sys; print(str(json.load(sys.stdin)["is_cn_region"]).lower())')"
assert_eq "true" "$p9_cn_flag" "Parity 9d: network dns-check honors saved IS_CN_REGION=true from config"

p9_global_flag="$("$DEBOPTY_BIN" --config "$P9_CFG_GLOBAL" network dns-check --mode calibrate --resolv "$P9_RESOLV" --json | python3 -B -c 'import json,sys; print(str(json.load(sys.stdin)["is_cn_region"]).lower())')"
assert_eq "false" "$p9_global_flag" "Parity 9e: network dns-check honors saved IS_CN_REGION=false from config"

p9_override_flag="$("$DEBOPTY_BIN" --config "$P9_CFG_GLOBAL" network dns-check --mode calibrate --resolv "$P9_RESOLV" --cn --json | python3 -B -c 'import json,sys; print(str(json.load(sys.stdin)["is_cn_region"]).lower())')"
assert_eq "true" "$p9_override_flag" "Parity 9f: network dns-check --cn overrides IS_CN_REGION=false"

# =========================================================
# Parity Test 10: Phase-3 native `debopty app`, `self-install`, and `ui-preview`
# =========================================================
P10_ROOT="${TMP_ROOT}/p10_root"
mkdir -p "$P10_ROOT"

p10_xray_installed="$("$DEBOPTY_BIN" app xray-status --root "$P10_ROOT" --json | python3 -B -c 'import json,sys; print(str(json.load(sys.stdin)["installed"]).lower())')"
assert_eq "false" "$p10_xray_installed" "Parity 10a: debopty app xray-status --json reports valid offline state"

p10_freship_installed="$("$DEBOPTY_BIN" app freship-status --root "$P10_ROOT" --json | python3 -B -c 'import json,sys; print(str(json.load(sys.stdin)["installed"]).lower())')"
assert_eq "false" "$p10_freship_installed" "Parity 10b: debopty app freship-status --json reports valid offline state"

"$DEBOPTY_BIN" self-install --root "$P10_ROOT" --no-sys --json >/dev/null
if [[ -x "${P10_ROOT}/opt/debopty/bin/debopty" && -L "${P10_ROOT}/usr/local/bin/debopty" && -L "${P10_ROOT}/usr/local/bin/debopty-f2b-sync" ]]; then
    PASS_COUNT=$((PASS_COUNT + 1))
    printf '[PASS] Parity 10c: debopty self-install --root installs binary + primary/multicall symlinks\n'
else
    FAIL_COUNT=$((FAIL_COUNT + 1))
    printf '[FAIL] Parity 10c: debopty self-install --root missing binary or symlinks\n' >&2
fi

P10_PREVIEW="${TMP_ROOT}/ui_preview.txt"
"$DEBOPTY_BIN" ui-preview --demo --screen all > "$P10_PREVIEW"
assert_contains "$P10_PREVIEW" "=== [main-wide] ===" "Parity 10d: ui-preview renders main-wide showcase"
assert_contains "$P10_PREVIEW" "=== [podman-confirm] ===" "Parity 10e: ui-preview renders podman-confirm showcase"

printf '\nSummary: %d passed, %d failed\n' "$PASS_COUNT" "$FAIL_COUNT"
(( FAIL_COUNT == 0 ))
