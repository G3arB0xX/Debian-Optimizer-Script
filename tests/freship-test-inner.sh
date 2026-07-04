#!/bin/bash
# FreshIP 模块容器内验证
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive
export CI=true
export FRESHIP_TEST_FAST="${FRESHIP_TEST_FAST:-1}"

for _i in $(seq 1 30); do
    if ! fuser /var/lib/dpkg/lock-frontend >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

FRESHIP_TEST_MODE="${FRESHIP_TEST_MODE:-fast}"

_fail() {
    echo "ASSERT_FAIL: $*"
    exit 1
}

_assert_file() {
    [[ -f "$1" ]] || _fail "missing file: $1"
}

_assert_exec() {
    [[ -x "$1" ]] || _fail "not executable: $1"
}

_setup_env() {
    apt-get update -yq >/dev/null 2>&1
    apt-get install -yq --no-install-recommends \
        ca-certificates curl jq sudo systemd >/dev/null 2>&1 || true

    mkdir -p /etc/debopti
    cat > /etc/debopti/debopti.conf << 'EOF'
IS_CN_REGION="false"
BASE_OPTIMIZED="false"
INSTALLED="false"
SSH_HARDENED="false"
EOF

    export SCRIPT_DIR=/opt/debopti
    # shellcheck disable=SC1091
    source /opt/debopti/scripts/common.sh
    load_project_config
    # shellcheck disable=SC1091
    source /opt/debopti/scripts/network.sh
    # shellcheck disable=SC1091
    source /opt/debopti/scripts/apps/freship.sh
    bash -n /opt/debopti/scripts/apps/freship.sh
}

echo "=== SMOKE (${DEBIAN_TAG:-unknown}) ==="
bash /opt/debopti/tests/freship/smoke.sh

echo "=== SETUP (${DEBIAN_TAG:-unknown}) [${FRESHIP_TEST_MODE}] ==="
_setup_env

echo "=== INSTALL (non-interactive: country=1 city=1 mode=ipv4) ==="
if ! printf '1\n1\n1\n' | install_freship; then
    echo "INSTALL_FAILED"
    exit 1
fi

echo "=== VERIFY INSTALL ARTIFACTS ==="
_assert_file /etc/freship/freship.conf
_assert_file /opt/freship/data/map.json
_assert_file /opt/freship/data/user_agents.txt
_assert_exec /opt/freship/core/freship_runner.sh
_assert_exec /opt/freship/core/freship_probe.sh
_assert_exec /opt/freship/core/freship_mod_google.sh
_assert_exec /opt/freship/core/freship_mod_trust.sh
_assert_exec /opt/freship/core/freship_updater.sh
_assert_exec /opt/freship/core/freship_lib.sh

# shellcheck source=/dev/null
source /etc/freship/freship.conf
[[ -n "${REGION_PATH:-}" ]] || _fail "REGION_PATH empty"
[[ -n "${TARGET_CC:-}" ]] || _fail "TARGET_CC empty"
[[ -n "${KW_FILE:-}" ]] || _fail "KW_FILE empty"
_assert_file "/opt/freship/data/regions/${REGION_PATH}.json"

grep -q "^REGION_PATH=" /etc/freship/freship.conf || _fail "conf missing REGION_PATH"
grep -q "^TARGET_CC=" /etc/freship/freship.conf || _fail "conf missing TARGET_CC"

id freship >/dev/null 2>&1 || _fail "user freship missing"
[[ "$(stat -c '%U' /etc/freship/freship.conf)" == "freship" ]] || _fail "conf owner not freship"

echo "=== TEST: hot data updater ==="
if ! runuser -u freship -- bash /opt/freship/core/freship_updater.sh; then
    _fail "freship_updater.sh failed"
fi

echo "=== TEST: probe (network-dependent result) ==="
export INSTANCE_MODE=v4
if ! runuser -u freship -- bash /opt/freship/core/freship_probe.sh; then
    echo "PROBE_RC=$? (drift/cn/fail acceptable in container)"
fi
_assert_file /etc/freship/state/v4.state
grep -q '^LAST_SCORE=' /etc/freship/state/v4.state || _fail "state missing LAST_SCORE"
grep -q '^RUN_MODE=' /etc/freship/state/v4.state || _fail "state missing RUN_MODE"

echo "=== TEST: runner one-shot (FRESHIP_TEST_FAST=1) ==="
if ! runuser -u freship -- env FRESHIP_TEST_FAST=1 CI=true \
    bash /opt/freship/core/freship_runner.sh v4; then
    echo "RUNNER_RC=$?"
fi
grep -q '\[FreshIP\]' /opt/freship/logs/freship.log 2>/dev/null || _fail "runner did not write log"

echo "=== TEST: mod_google dry start (cookie lock + coords) ==="
if ! runuser -u freship -- env FRESHIP_TEST_FAST=1 CI=true \
    timeout 90 bash /opt/freship/core/freship_mod_google.sh; then
    rc=$?
    if [[ "$rc" -eq 124 ]]; then
        echo "mod_google timeout (long session) — partial run OK"
    elif [[ "$rc" -ne 0 ]]; then
        _fail "mod_google exited $rc"
    fi
fi
[[ -d /opt/freship/data/cookies ]] || _fail "cookies dir missing"

echo "=== TEST: mod_trust (fast) ==="
if ! runuser -u freship -- env FRESHIP_TEST_FAST=1 CI=true \
    timeout 120 bash /opt/freship/core/freship_mod_trust.sh; then
    rc=$?
    [[ "$rc" -eq 124 ]] || _fail "mod_trust failed rc=$rc"
fi

if [[ "$FRESHIP_TEST_MODE" == "full" ]]; then
    echo "=== UNINSTALL ==="
    uninstall_freship
    [[ ! -d /opt/freship ]] || _fail "opt dir still exists"
    [[ ! -f /etc/freship/freship.conf ]] || _fail "conf still exists"
fi

echo "ALL_TESTS_PASSED"
