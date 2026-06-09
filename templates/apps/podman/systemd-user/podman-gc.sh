#!/bin/bash
# Podman 定期清理 — 删除 7 天前停止的容器与悬空镜像
# 由 podman-gc.timer 触发，也可手动: apppod system prune -f --filter until=168h

set -euo pipefail
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
exec /usr/bin/podman system prune -f --filter until=168h
