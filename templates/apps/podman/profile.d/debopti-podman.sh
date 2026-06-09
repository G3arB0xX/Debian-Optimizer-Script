# debopti Podman 运维别名 — Bash
# 路径: /etc/profile.d/debopti-podman.sh
# 主用户通过 apppod/appctl/applog 代管 apps 用户的 rootless 容器

# docker 兼容：podman-docker 已提供 /usr/bin/docker 软链，此处双重保障
alias docker=podman 2>/dev/null || true
alias dc='podman-compose' 2>/dev/null || true

# 若本机存在用户级 Podman API socket，供 docker-compose 等工具连接
if [[ -n "${XDG_RUNTIME_DIR:-}" && -S "${XDG_RUNTIME_DIR}/podman/podman.sock" ]]; then
    export DOCKER_HOST="unix://${XDG_RUNTIME_DIR}/podman/podman.sock"
fi

# 以 apps 用户执行 podman（避免工作目录 chdir 冲突）
apppod() {
    local old_pwd="$PWD"
    cd /tmp || return 1
    sudo -u apps podman "$@"
    local rc=$?
    cd "$old_pwd" || true
    return "$rc"
}

# 管理 apps 用户的 systemd 容器服务（Quadlet 生成）
appctl() {
    sudo -u apps XDG_RUNTIME_DIR=/run/user/$(id -u apps) systemctl --user "$@"
}

# 查看 apps 用户容器相关日志（journald）
applog() {
    sudo -u apps XDG_RUNTIME_DIR=/run/user/$(id -u apps) journalctl --user "$@"
}

# 进入 apps 用户交互 shell（调试）
appshell() {
    sudo machinectl shell apps@
}
