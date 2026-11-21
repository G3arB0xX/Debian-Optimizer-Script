#!/bin/bash
# =========================================================
# Unprivileged Host Unit Tests for Fail2ban Public-Port Sync
# =========================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SYNC_SCRIPT="${REPO_ROOT}/templates/security/debopty-f2b-sync.sh"
FILTER_TMPL="${REPO_ROOT}/templates/security/fail2ban_filter_public_ports.conf"
SERVICE_TMPL="${REPO_ROOT}/templates/security/debopty-f2b-port-sync.service"
PATH_TMPL="${REPO_ROOT}/templates/security/debopty-f2b-port-sync.path"
TIMER_TMPL="${REPO_ROOT}/templates/security/debopty-f2b-port-sync.timer"
NFT_OVERRIDE_TMPL="${REPO_ROOT}/templates/security/nftables-fail2ban-override.conf"

TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "$TMP_ROOT"' EXIT

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

# Prepare isolated root filesystem & source security.sh with lightweight stubs
export DEBOPTY_F2B_ROOT="$TMP_ROOT"
export DEBOPTY_F2B_NO_SYS=1
export DEBOPTY_F2B_BACKEND="systemd"
export DEBOPTY_F2B_TEMPLATE_DIR="${REPO_ROOT}/templates/security"
export DEBOPTY_F2B_SYNC_BIN="$SYNC_SCRIPT"
export SCRIPT_DIR="$REPO_ROOT"
export NFT_MAIN_CONF="${TMP_ROOT}/etc/nftables.conf"
export NFT_BASE_D="${TMP_ROOT}/etc/nftables.d"
export NFT_CONF_DIR="${NFT_BASE_D}/debopti"

mkdir -p "$NFT_CONF_DIR" "${TMP_ROOT}/etc/fail2ban/jail.d"

# Minimal stubs for common.sh helpers used by scripts/security.sh
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

# 1. Initial nftables.conf with SSH port 2222 -> sync extracts SSH port & populates guard
render_template "templates/security/nftables.conf" "$NFT_MAIN_CONF" "SSH_PORT=2222"
echo "include \"${NFT_CONF_DIR}/*.nft\"" > "${NFT_BASE_D}/50-debopti.nft"
bash "$SYNC_SCRIPT"

JAIL_PORTS="${TMP_ROOT}/etc/fail2ban/jail.d/99-debopty-ports.local"
GUARD_NFT="${NFT_BASE_D}/55-debopty-f2b-guard.nft"

assert_contains "$JAIL_PORTS" "port = 2222" "Initial sync sets [sshd] port = 2222"
assert_contains "$JAIL_PORTS" "enabled = true" "Initial sync enables [debopty-public-ports]"
assert_contains "$JAIL_PORTS" "backend = systemd" "Initial sync uses systemd backend"
assert_contains "$JAIL_PORTS" "journalmatch = _TRANSPORT=kernel" "Initial sync sets kernel journalmatch"
assert_contains "$GUARD_NFT" "tcp dport 2222 update @abuse_v4" "Guard nft protects initial TCP SSH port"
assert_not_contains "$GUARD_NFT" "udp dport" "Guard nft omits UDP rule when no UDP ports open"

# 2. update_ssh_firewall changes SSH port to 50022 -> sync updates both files
update_ssh_firewall 50022
assert_contains "$JAIL_PORTS" "port = 50022" "update_ssh_firewall syncs new SSH port 50022"
assert_contains "$GUARD_NFT" "tcp dport 50022 update @abuse_v4" "Guard nft updates SSH port to 50022"
assert_not_contains "$GUARD_NFT" "2222" "Old SSH port 2222 removed from guard nft"

# 3. add_fw_rule for single (41641/udp), list (80,443/tcp/udp), range (11010-11015/tcp/udp)
add_fw_rule "41641" "udp" "Tailscale_P2P"
add_fw_rule "80,443" "tcp/udp" "Caddy_Web"
add_fw_rule "11010-11015" "tcp/udp" "Easytier_P2P"

assert_contains "$JAIL_PORTS" "port = 80,443,11010:11015,41641,50022" "Fail2ban port list includes single, list, and colon-range"
assert_contains "$GUARD_NFT" "tcp dport { 80, 443, 11010 - 11015, 50022 } update @abuse_v4" "Guard nft TCP set formats range with ' - '"
assert_contains "$GUARD_NFT" "tcp dport { 80, 443, 11010 - 11015, 50022 } update @abuse_v6" "Guard nft TCP IPv6 set present"
assert_contains "$GUARD_NFT" "udp dport { 80, 443, 11010 - 11015, 41641 } update @abuse_v4" "Guard nft UDP set includes 41641 and range"
assert_contains "$GUARD_NFT" "udp dport { 80, 443, 11010 - 11015, 41641 } update @abuse_v6" "Guard nft UDP IPv6 set present"

# 4. remove_fw_rule cleanly removes ports from both files
remove_fw_rule "Easytier_P2P"
remove_fw_rule "Tailscale_P2P"
assert_not_contains "$JAIL_PORTS" "11010:11015" "remove_fw_rule removes Easytier range from jail"
assert_not_contains "$JAIL_PORTS" "41641" "remove_fw_rule removes Tailscale UDP port from jail"
assert_not_contains "$GUARD_NFT" "11010 - 11015" "remove_fw_rule removes Easytier range from guard nft"
assert_not_contains "$GUARD_NFT" "41641" "remove_fw_rule removes Tailscale UDP port from guard nft"
assert_contains "$JAIL_PORTS" "port = 80,443,50022" "Remaining ports preserved after remove_fw_rule"

# 5. Idempotency check: repeated debopty-f2b-sync must NOT touch mtimes (prevents systemd.path loops)
touch -d "2025-01-01 00:00:00" "$JAIL_PORTS" "$GUARD_NFT"
mtime_jail_before="$(stat -c %Y "$JAIL_PORTS")"
mtime_guard_before="$(stat -c %Y "$GUARD_NFT")"
bash "$SYNC_SCRIPT"
mtime_jail_after="$(stat -c %Y "$JAIL_PORTS")"
mtime_guard_after="$(stat -c %Y "$GUARD_NFT")"
assert_eq "$mtime_jail_before" "$mtime_jail_after" "Idempotent sync leaves 99-debopty-ports.local mtime unchanged"
assert_eq "$mtime_guard_before" "$mtime_guard_after" "Idempotent sync leaves 55-debopty-f2b-guard.nft mtime unchanged"

# 6. Zero open ports removes guard nft file and disables public-ports jail
rm -f "$NFT_MAIN_CONF" "${NFT_CONF_DIR}"/*.nft
bash "$SYNC_SCRIPT"
assert_contains "$JAIL_PORTS" "enabled = false" "Zero open ports disables [debopty-public-ports]"
if [[ ! -f "$GUARD_NFT" ]]; then
    PASS_COUNT=$((PASS_COUNT + 1))
    printf '[PASS] %s\n' "Zero open ports removes 55-debopty-f2b-guard.nft"
else
    FAIL_COUNT=$((FAIL_COUNT + 1))
    printf '[FAIL] %s\n' "Zero open ports did not remove 55-debopty-f2b-guard.nft" >&2
fi

# 7. Legacy jail.local cleanup safety: deletes old 12-line signature, preserves custom file
cat > "${TMP_ROOT}/etc/fail2ban/jail.local" << 'EOF'
[DEFAULT]
banaction = nftables-multiport
banaction_allports = nftables-allports

[sshd]
enabled = true
port = 22
filter = sshd
logpath = /var/log/auth.log
maxretry = 3
bantime = 86400
findtime = 600
EOF
cleanup_legacy_fail2ban_jail_local
if [[ ! -f "${TMP_ROOT}/etc/fail2ban/jail.local" ]]; then
    PASS_COUNT=$((PASS_COUNT + 1))
    printf '[PASS] %s\n' "cleanup_legacy_fail2ban_jail_local removes old debopti 12-line signature"
else
    FAIL_COUNT=$((FAIL_COUNT + 1))
    printf '[FAIL] %s\n' "cleanup_legacy_fail2ban_jail_local failed to remove legacy signature" >&2
fi

printf '[DEFAULT]\nignoreip = 10.0.0.1\n' > "${TMP_ROOT}/etc/fail2ban/jail.local"
cleanup_legacy_fail2ban_jail_local
if [[ -f "${TMP_ROOT}/etc/fail2ban/jail.local" ]]; then
    PASS_COUNT=$((PASS_COUNT + 1))
    printf '[PASS] %s\n' "cleanup_legacy_fail2ban_jail_local preserves user-customized jail.local"
else
    FAIL_COUNT=$((FAIL_COUNT + 1))
    printf '[FAIL] %s\n' "cleanup_legacy_fail2ban_jail_local clobbered user-customized jail.local" >&2
fi

# 8. Filter regex check against IPv4 and IPv6 kernel nftables log lines via python3
if python3 -c '
import re, sys

with open(sys.argv[1], "r", encoding="utf-8") as fh:
    failregex = None
    for line in fh:
        if line.strip().startswith("failregex"):
            failregex = line.split("=", 1)[1].strip()
            break

assert failregex is not None, "failregex not found"
host_pat = r"(?P<host>(?:\d{1,3}\.){3}\d{1,3}|[0-9a-fA-F:]+)"
compiled = re.compile(failregex.replace("<HOST>", host_pat))

v4_line = "Nov 21 12:00:01 vps kernel: [12345.678] [DEBOPTY-F2B-PORT-ABUSE] IN=eth0 OUT= SRC=203.0.113.42 DST=198.51.100.10 PROTO=TCP SPT=54321 DPT=443 SYN"
v6_line = "Nov 21 12:00:02 vps kernel: [12346.789] [DEBOPTY-F2B-PORT-ABUSE] IN=eth0 OUT= SRC=2001:db8::1 DST=2001:db8::2 PROTO=UDP SPT=55555 DPT=41641"
benign = "Nov 21 12:00:03 vps kernel: [12347.000] IN=eth0 OUT= SRC=203.0.113.42 DST=198.51.100.10 PROTO=TCP"

m4 = compiled.match(v4_line)
assert m4 and m4.group("host") == "203.0.113.42", f"IPv4 mismatch: {m4}"
m6 = compiled.match(v6_line)
assert m6 and m6.group("host") == "2001:db8::1", f"IPv6 mismatch: {m6}"
assert compiled.match(benign) is None, "Benign line should not match"
' "$FILTER_TMPL"; then
    PASS_COUNT=$((PASS_COUNT + 1))
    printf '[PASS] %s\n' "Filter regex matches IPv4 & IPv6 kernel logs and ignores benign lines"
else
    FAIL_COUNT=$((FAIL_COUNT + 1))
    printf '[FAIL] %s\n' "Filter regex verification failed" >&2
fi

# 9. Systemd unit sandboxing & hook assertions
assert_contains "$SERVICE_TMPL" "ProtectSystem=full" "Service template sets ProtectSystem=full"
assert_contains "$SERVICE_TMPL" "PrivateTmp=true" "Service template sets PrivateTmp=true"
assert_contains "$SERVICE_TMPL" "NoNewPrivileges=true" "Service template sets NoNewPrivileges=true"
assert_contains "$SERVICE_TMPL" "ReadWritePaths=/etc/fail2ban/jail.d /etc/nftables.d /run/fail2ban" "Service template sets ReadWritePaths="
assert_contains "$PATH_TMPL" "PathChanged=/etc/nftables.conf" "Path template watches /etc/nftables.conf"
assert_contains "$TIMER_TMPL" "OnUnitActiveSec=10min" "Timer template runs periodic 10min sync"
assert_contains "$NFT_OVERRIDE_TMPL" "ExecStartPost=-/usr/local/bin/debopty-f2b-sync --force-reload" "nftables override hooks ExecStartPost"
assert_contains "$NFT_OVERRIDE_TMPL" "ExecReload=-/usr/local/bin/debopty-f2b-sync --force-reload" "nftables override hooks ExecReload"

printf '\nSummary: %d passed, %d failed\n' "$PASS_COUNT" "$FAIL_COUNT"
(( FAIL_COUNT == 0 ))
