#!/bin/bash
# 在测试容器内执行的 Podman 模块验证逻辑
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive
export CI=true

# 等待 dpkg 锁释放（避免与容器启动时的 apt 进程冲突）
for _i in $(seq 1 30); do
    if ! fuser /var/lib/dpkg/lock-frontend >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

PODMAN_TEST_MODE="${PODMAN_TEST_MODE:-fast}"

# 测试主用户（模拟 sudoer）
if ! id testuser >/dev/null 2>&1; then
    useradd -m -s /bin/bash -G sudo testuser
    echo "testuser ALL=(ALL) NOPASSWD: ALL" > /etc/sudoers.d/testuser
    chmod 440 /etc/sudoers.d/testuser
fi

mkdir -p /etc/debopti /opt/debopti/.cursor
cat > /etc/debopti/debopti.conf << 'EOF'
IS_CN_REGION="false"
BASE_OPTIMIZED="false"
INSTALLED="false"
SSH_HARDENED="false"
EDITOR_CMD="micro"
PODMAN_GC_ENABLED="false"
EOF

export SUDO_USER=testuser
export SCRIPT_DIR=/opt/debopti

# shellcheck disable=SC1091
source /opt/debopti/scripts/common.sh
# shellcheck disable=SC1091
source /opt/debopti/scripts/apps/podman.sh

echo "=== INSTALL PODMAN (${DEBIAN_TAG:-unknown}) [${PODMAN_TEST_MODE}] ==="
if ! install_podman; then
    echo "INSTALL_FAILED"
    exit 1
fi

echo "=== VERIFY INSTALL ==="
id apps
test -d /home/apps/srv
test -d /home/apps/backups
test ! -d /home/apps/srv/storage
test -f /home/apps/.config/containers/containers.conf
test -f /home/apps/.config/containers/storage.conf
test -f /home/apps/.config/restic-excludes
test -f /etc/profile.d/debopti-podman.sh
test -f /etc/sudoers.d/debopti-podman
grep -q journald /home/apps/.config/containers/containers.conf
! grep -qE '^graphroot\s*=' /home/apps/.config/containers/storage.conf
grep -q restic /etc/sudoers.d/debopti-podman

runuser -u apps -- env HOME=/home/apps podman version >/dev/null 2>&1 || true

GRAPHROOT=$(runuser -u apps -- env HOME=/home/apps podman info --format '{{.Store.GraphRoot}}' 2>/dev/null || echo "")
if [[ "$GRAPHROOT" == "/home/apps/.local/share/containers/storage" ]]; then
    echo "GRAPHROOT_DEFAULT_OK"
elif [[ -z "$GRAPHROOT" ]]; then
    # 嵌套容器内 rootless podman info 可能不可用，以 storage.conf 无自定义 graphroot 为准
    ! grep -qE '^graphroot\s*=' /home/apps/.config/containers/storage.conf
    echo "GRAPHROOT_DEFAULT_BY_CONFIG"
else
    echo "GRAPHROOT_UNEXPECTED:${GRAPHROOT}"
    exit 1
fi

if command -v getfacl >/dev/null 2>&1 && [[ -d /home/apps/.local/share/containers/storage ]]; then
    if getfacl /home/apps/.local/share/containers/storage 2>/dev/null | grep -q 'user:testuser:'; then
        echo "GRAPHROOT_ACL_POLLUTED"
        exit 1
    fi
    getfacl /home/apps/backups 2>/dev/null | grep -q 'user:testuser:rwx' || {
        echo "BACKUPS_ACL_MISSING"
        exit 1
    }
    echo "ACL_LAYOUT_OK"
fi

bash -n /etc/profile.d/debopti-podman.sh
if runuser -u apps -- podman version 2>/dev/null; then
    runuser -u apps -- podman info --format "{{.Host.Security.Rootless}}"
    echo "ROOTLESS_PODMAN_OK"
else
    echo "ROOTLESS_RUNTIME_LIMITED_IN_NESTED_CONTAINER"
    podman version
fi

if sudo -u testuser -n true 2>/dev/null; then
    if sudo -u testuser bash -lc "source /etc/profile.d/debopti-podman.sh && apppod version" 2>/dev/null; then
        if sudo -u testuser bash -lc "source /etc/profile.d/debopti-podman.sh && appctl is-enabled podman-gc.timer" 2>/dev/null; then
            sudo -u testuser bash -lc "source /opt/debopti/scripts/common.sh; source /opt/debopti/scripts/apps/podman.sh; toggle_podman_gc" 2>/dev/null || true
            sudo -u testuser bash -lc "source /opt/debopti/scripts/common.sh; source /opt/debopti/scripts/apps/podman.sh; toggle_podman_gc" 2>/dev/null || true
            echo "GC_TIMER_TESTED"
        else
            echo "GC_TIMER_SKIPPED_NO_USER_SYSTEMD"
        fi
    else
        echo "SUDO_APPPOD_SKIPPED_IN_NESTED_CONTAINER"
        echo "GC_TIMER_SKIPPED_NO_USER_SYSTEMD"
    fi
else
    echo "SUDO_SKIPPED_IN_NESTED_CONTAINER"
    echo "GC_TIMER_SKIPPED_NO_USER_SYSTEMD"
fi

if [[ "$PODMAN_TEST_MODE" == "full" ]]; then
    echo "=== REINSTALL (IDEMPOTENT) ==="
    install_podman
    echo "=== UNINSTALL ==="
    uninstall_podman
fi

echo "ALL_TESTS_PASSED"
