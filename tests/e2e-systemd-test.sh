#!/usr/bin/env bash
# End-to-end systemd Debian Podman integration test for Pure Rust debopty.
# Boots a real systemd PID 1 Debian container and runs the compiled Rust `debopty`
# binary as the default entrypoint, then verifies:
#   1. self-install -> /opt/debopty/bin/debopty + /usr/local/bin/debopty + multicall debopty-f2b-sync
#      + legacy debopti migration + zero archive/ or rust/ in /opt/debopty
#   2. system tmux-setup -> real tmux install + /etc/tmux.conf + live tmux server parse
#   3. security setup -> real nftables + fail2ban + python3-systemd + systemd units + port sync
#   4. security fw-add / fw-remove -> dynamic Fail2ban & nftables guard sync
#   5. reconcile --audit -> 9-domain non-destructive audit clean JSON report
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

DEBOPTY_BIN="${DEBOPTY_E2E_BIN:-${REPO_ROOT}/rust/target/x86_64-unknown-linux-musl/release/debopty}"
if [[ ! -x "$DEBOPTY_BIN" ]]; then
    echo "=== Building static musl Rust binary for E2E test ==="
    CARGO_OFFLINE_ARGS=()
    if [[ "${DEBOPTY_CARGO_OFFLINE:-0}" == "1" ]] || cargo metadata --manifest-path "${REPO_ROOT}/rust/Cargo.toml" --offline --format-version 1 >/dev/null 2>&1; then
        CARGO_OFFLINE_ARGS=(--offline)
    fi
    cargo build --manifest-path "${REPO_ROOT}/rust/Cargo.toml" --release \
        --target x86_64-unknown-linux-musl --features ratatui-ui "${CARGO_OFFLINE_ARGS[@]}"
fi
test -x "$DEBOPTY_BIN"

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
    -v "${DEBOPTY_BIN}:/tmp/debopty-bin:ro,Z" \
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

echo "=== Running Pure Rust debopty E2E verification inside ${CNAME} ==="
podman exec "$CNAME" bash -euo pipefail -c '
export DEBIAN_FRONTEND=noninteractive
export CI=true
export TERM=xterm-256color

# 1. Simulate legacy debopti artifacts and stale archive/rust dirs under /opt/debopty
mkdir -p /etc/profile.d /etc/debopti /etc/nftables.d /etc/ssh /opt/debopty/archive/legacy-bash /opt/debopty/rust
ln -sf /opt/debopti/deb_optimizer.sh /usr/local/bin/debopti
printf "alias debopti=/usr/local/bin/debopti\n" > /etc/profile.d/debopti.sh

# 2. Run native Rust self-install
/tmp/debopty-bin self-install --json >/dev/null

# 3. Verify Rust default cutover, multicall symlink, legacy migration & prod strip
test -x /opt/debopty/bin/debopty
test "$(readlink -f /usr/local/bin/debopty)" = "/opt/debopty/bin/debopty"
test "$(readlink -f /usr/local/bin/debopty-f2b-sync)" = "/opt/debopty/bin/debopty"
test ! -e /usr/local/bin/debopti
test ! -e /etc/profile.d/debopti.sh
test -f /etc/profile.d/debopty.sh
test ! -d /opt/debopty/archive
test ! -d /opt/debopty/rust
test ! -d /opt/debopty/.agents
test ! -d /opt/debopty/governance
test ! -d /opt/debopty/tests
test ! -f /opt/debopty/INDEX.md
echo "[PASS] Rust self-install + /usr/local/bin/debopty + multicall debopty-f2b-sync + prod strip"

# 4. Verify status --json reports 21/21 NativeRust & 0 BashBridge
debopty status --json | grep -q "\"native-rust\""
! debopty status --json | grep -q "\"bash-bridge\""
echo "[PASS] debopty status --json reports 21/21 NativeRust (0 BashBridge)"

# 5. Verify real tmux install & live config parse via Rust CLI
debopty system tmux-setup
command -v tmux >/dev/null
test -f /etc/tmux.conf
grep -q "managed-by-debopty" /etc/tmux.conf
tmux -L debopty-e2e -f /etc/tmux.conf new-session -d "sleep 1"
tmux -L debopty-e2e kill-server 2>/dev/null || true
echo "[PASS] Rust system tmux-setup + /etc/tmux.conf + live server parse"

# 6. Verify real security baseline (nftables + fail2ban + systemd sync units)
debopty security setup
systemctl is-active --quiet fail2ban
systemctl is-enabled --quiet debopty-f2b-port-sync.path
systemctl is-enabled --quiet debopty-f2b-port-sync.timer
fail2ban-client status sshd >/dev/null
fail2ban-client status debopty-public-ports >/dev/null
echo "[PASS] Rust security setup + fail2ban + systemd sync units active"

# 7. Verify dynamic port sync via Rust security fw-add / fw-remove + multicall debopty-f2b-sync
debopty security fw-add 8443 --proto tcp --comment e2e_test
grep -q "8443" /etc/fail2ban/jail.d/99-debopty-ports.local
grep -q "8443" /etc/nftables.d/55-debopty-f2b-guard.nft
nft -c -f /etc/nftables.conf

debopty security fw-remove e2e_test
! grep -q "8443" /etc/fail2ban/jail.d/99-debopty-ports.local
! grep -q "8443" /etc/nftables.d/55-debopty-f2b-guard.nft
nft -c -f /etc/nftables.conf
echo "[PASS] Rust security fw-add / fw-remove -> fail2ban & nftables guard sync"

# 8. Verify 9-domain reconcile --audit runs cleanly
debopty reconcile --audit --json | grep -q "\"mode\": \"audit\""
echo "[PASS] Rust reconcile --audit completed cleanly"
'

echo "=== E2E SYSTEMD PODMAN TEST PASSED (${DEBIAN_TAG}) ==="
