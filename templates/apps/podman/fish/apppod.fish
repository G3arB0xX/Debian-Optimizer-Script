# 以 apps 用户执行 podman 命令

function apppod
    set -l old_pwd $PWD
    builtin cd /tmp
    sudo -u apps podman $argv
    set -l rc $status
    builtin cd $old_pwd
    return $rc
end
