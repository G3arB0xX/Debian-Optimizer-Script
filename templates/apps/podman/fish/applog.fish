# 查看 apps 用户容器日志（journald --user）

function applog
    set -l apps_uid (id -u apps)
    sudo -u apps XDG_RUNTIME_DIR=/run/user/$apps_uid journalctl --user $argv
end
