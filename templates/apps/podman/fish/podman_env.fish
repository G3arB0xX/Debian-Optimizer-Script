# Podman API socket 环境变量（Fish）
# 仅当用户级 socket 存在时设置，供 docker-compose 等工具使用

if test -S "$XDG_RUNTIME_DIR/podman/podman.sock"
    set -gx DOCKER_HOST "unix://$XDG_RUNTIME_DIR/podman/podman.sock"
end
