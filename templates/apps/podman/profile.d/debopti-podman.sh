# debopti Podman 运维别名 — Bash
# 路径: /etc/profile.d/debopti-podman.sh
# docker/dc 缩写全员可用；apppod/appctl/applog/appshell 仅 root 与 SOT 用户

# docker 兼容：podman-docker 已提供 /usr/bin/docker 软链，此处双重保障
alias docker=podman 2>/dev/null || true
alias dc='podman-compose' 2>/dev/null || true

# 若本机存在用户级 Podman API socket，供 docker-compose 等工具连接
if [[ -n "${XDG_RUNTIME_DIR:-}" && -S "${XDG_RUNTIME_DIR}/podman/podman.sock" ]]; then
    export DOCKER_HOST="unix://${XDG_RUNTIME_DIR}/podman/podman.sock"
fi

# 代管 apps 用户容器：仅 root 与真理源用户（{{SOT_USER}}）
_debopti_podman_maintainer_ok() {
    local me
    me=$(id -un 2>/dev/null || echo "")
    [[ "$me" == "root" || "$me" == "{{SOT_USER}}" ]]
}

if _debopti_podman_maintainer_ok; then
    apppod() {
        local old_pwd="$PWD"
        cd /tmp || return 1
        sudo -u apps podman "$@"
        local rc=$?
        cd "$old_pwd" || true
        return "$rc"
    }

    appctl() {
        sudo -u apps XDG_RUNTIME_DIR=/run/user/$(id -u apps) systemctl --user "$@"
    }

    applog() {
        sudo -u apps XDG_RUNTIME_DIR=/run/user/$(id -u apps) journalctl --user "$@"
    }

    appshell() {
        sudo machinectl shell apps@
    }
fi
