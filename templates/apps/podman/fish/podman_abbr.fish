# Podman CLI 缩写（Fish）
# abbr 在输入后按空格自动展开，明确当前使用 Podman 而非 Docker 守护进程

if status is-interactive
    abbr -a docker podman
    abbr -a dc podman-compose
end
