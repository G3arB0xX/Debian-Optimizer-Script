#!/usr/bin/env bash
# End-to-end systemd Debian Podman integration test for debopty.
# Boots a real systemd PID 1 Debian container and runs deb_optimizer.sh (CI=true),
# then verifies:
#   1. self_install -> /opt/debopty + /usr/local/bin/debopty + legacy debopti migration + prod strip
#   2. setup_tmux -> real tmux install + /etc/tmux.conf + live tmux server parse
#   3. setup_fail2ban -> real nftables + fail2ban + python3-systemd + systemd units + port sync
#
# Usage:
#   bash tests/e2e-systemd-test.sh
#   DEBIAN_TAG=bullseye bash tests/e2e-systemd-test.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEBIAN_TAG="${DEBIAN_TAG:-bookworm}"
IMAGE_PREFIX="${PODMAN_TEST_BASE_IMAGE:-localhost/debopti-podman-test-base}"
IMAGE="${IMAGE_PREFIX}:${DEBIAN_TAG}"
APT_CACHE="${REPO_ROOT}/.cache/apt/${DEBIAN_TAG}/archives"
CNAME="debopty-e2e-systemd-${DEBIAN_TAG}-$$"

if ! command -v podman >/dev/null 2>&1; then
    echo "ERROR: podman is required for e2e-systemd-test.sh" >&2
    exit 1
fi

if ! podman image exists "$IMAGE" 2>/dev/null; then
    if [[ -x "${REPO_ROOT}/tests/podman-test-base/build-base.sh" ]]; then
        DEBIAN_TEST_TAGS="${DEBIAN_TAG}" bash "${REPO_ROOT}/tests/podman-test-base/build-base.sh"
    else
        IMAGE="docker.io/library/debian:${DEBIAN_TAG}"
    fi
fi

mkdir -p "$APT_CACHE"

cleanup() {
    podman rm -f "$CNAME" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "=== Booting systemd container ${CNAME} (${IMAGE}) ==="
podman run -d --name "$CNAME" \
    --systemd=always \
    --privileged \
    --userns=host \
    --cgroupns=host \
    --security-opt seccomp=unconfined \
    --cap-add=ALL \
    -v "${REPO_ROOT}:/tmp/debopty-src:ro,Z" \
    -v "${APT_CACHE}:/var/cache/apt/archives:Z" \
    -e DEBIAN_FRONTEND=noninteractive \
    -e CI=true \
    "$IMAGE" \
    /sbin/init >/dev/null

# Wait for systemd D-Bus & basic.target
for _ in $(seq 1 30); do
    if podman exec "$CNAME" systemctl is-system-running --wait >/dev/null 2>&1 || \
       podman exec "$CNAME" systemctl list-units >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

echo "=== Running E2E deb_optimizer.sh & service verification inside ${CNAME} ==="
podman exec "$CNAME" bash -euo pipefail -c '
export DEBIAN_FRONTEND=noninteractive
export CI=true
export TERM=xterm-256color

# 1. Stage writable repo copy and simulate legacy debopti artifacts
rm -rf /tmp/src
cp -a /tmp/debopty-src /tmp/src
mkdir -p /etc/profile.d /etc/debopti /etc/nftables.d /etc/ssh
ln -sf /opt/debopti/deb_optimizer.sh /usr/local/bin/debopti
printf "alias debopti=/usr/local/bin/debopti\n" > /etc/profile.d/debopti.sh

# 2. Run real deb_optimizer.sh end-to-end (self_install + base optimization + tmux + nftables + fail2ban)
bash /tmp/src/deb_optimizer.sh

# 3. Verify debopty install, legacy migration & prod strip
test -d /opt/debopty
test -x /usr/local/bin/debopty
test ! -e /usr/local/bin/debopti
test ! -e /etc/profile.d/debopti.sh
test ! -d /opt/debopty/governance
test ! -d /opt/debopty/tests
test ! -f /opt/debopty/INDEX.md
test ! -f /opt/debopty/scripts/INDEX.md
echo "[PASS] self_install + legacy debopti migration + prod strip"

# 4. Verify real tmux install & live config parse
command -v tmux >/dev/null
test -f /etc/tmux.conf
grep -q "managed-by-debopty" /etc/tmux.conf
tmux -L debopty-e2e -f /etc/tmux.conf new-session -d "sleep 1"
tmux -L debopty-e2e kill-server 2>/dev/null || true
echo "[PASS] real tmux install + /etc/tmux.conf + live server parse"

# 5. Verify real fail2ban + systemd units + initial public-ports jail
systemctl is-active --quiet fail2ban
systemctl is-enabled --quiet debopty-f2b-port-sync.path
systemctl is-enabled --quiet debopty-f2b-port-sync.timer
fail2ban-client status sshd >/dev/null
fail2ban-client status debopty-public-ports >/dev/null
echo "[PASS] real fail2ban + systemd units + initial public-ports jail active"

# 6. Verify dynamic port sync via add_fw_rule / remove_fw_rule
export SCRIPT_DIR=/opt/debopty
source /opt/debopty/scripts/common.sh
source /opt/debopty/scripts/security.sh

add_fw_rule 8443 tcp "e2e_test"
grep -q "8443" /etc/fail2ban/jail.d/99-debopty-ports.local
grep -q "8443" /etc/nftables.d/55-debopty-f2b-guard.nft
nft -c -f /etc/nftables.conf

remove_fw_rule "e2e_test"
! grep -q "8443" /etc/fail2ban/jail.d/99-debopty-ports.local
! grep -q "8443" /etc/nftables.d/55-debopty-f2b-guard.nft
nft -c -f /etc/nftables.conf
echo "[PASS] dynamic add_fw_rule / remove_fw_rule -> fail2ban & nftables guard sync"
'

echo "=== E2E SYSTEMD PODMAN TEST PASSED (${DEBIAN_TAG}) ==="
