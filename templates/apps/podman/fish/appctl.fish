# 管理 apps 用户的 systemd 用户服务（Quadlet 容器）

function appctl
    set -l apps_uid (id -u apps)
    sudo -u apps XDG_RUNTIME_DIR=/run/user/$apps_uid systemctl --user $argv
end
