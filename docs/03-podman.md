# Podman Rootless 容器环境

本文档介绍如何在 Debian 上部署 Podman rootless 容器环境，与 `debopti` 脚本自动化步骤完全对齐。

**前提条件**：root 权限，Debian 10+，systemd 与 logind 可用

**架构概要**：
- 专用 `apps` 用户以 rootless 方式运行所有容器
- 主用户（sudoer）通过 `apppod` / `appctl` / `applog` / `appshell` 代管
- 无 dockerd 守护进程；日志统一写入 journald

---

## 1. 安装前：移除 Docker（若存在）

Podman 与 Docker 不能共存。安装前需卸载 Docker：

```bash
systemctl stop docker docker.socket containerd 2>/dev/null
apt-get purge -y docker-ce docker-ce-cli containerd.io \
    docker-buildx-plugin docker-compose-plugin docker.io docker-doc
apt-get autoremove -y
rm -rf /etc/docker /var/run/docker.sock
```

---

## 2. 安装 Podman 组件

```bash
apt-get update
apt-get install -y podman podman-docker systemd-container \
    podman-compose buildah skopeo uidmap
```

可选（旧版 Debian 可能无对应包）：

```bash
apt-get install -y fuse-overlayfs passt
```

验证：

```bash
podman --version
podman-compose version
```

---

## 3. 创建 apps 专用用户

```bash
# 若已安装 Fish 则使用 Fish，否则 Bash
APPS_SHELL=/bin/bash
command -v fish >/dev/null && APPS_SHELL=$(command -v fish)

useradd -m -s "$APPS_SHELL" -d /home/apps apps
passwd -l apps
usermod --add-subuids 100000-165535 --add-subgids 100000-165535 apps
loginctl enable-linger apps

mkdir -p /home/apps/srv /home/apps/srv/storage
mkdir -p /home/apps/.config/containers/registries.conf.d
mkdir -p /home/apps/.config/containers/systemd
chown -R apps:apps /home/apps
```

将主用户（将 `myuser` 替换为实际用户名）加入 `apps` 组并设置 ACL：

```bash
usermod -aG apps myuser
setfacl -R -m u:myuser:rwx /home/apps/srv /home/apps/.config
setfacl -R -d -m u:myuser:rwx /home/apps/srv /home/apps/.config
setfacl -m u:myuser:rx /home/apps
```

---

## 4. 引擎配置 containers.conf

写入 `/home/apps/.config/containers/containers.conf`：

```ini
# 日志写入 journald，避免 json-file 占满磁盘
[containers]
log_driver = "journald"
log_size_max = 52428800

[engine]
cgroup_manager = "systemd"
events_logger = "journald"
# 以下行仅 Podman 5.x+ 支持，旧版请删除或注释
database_backend = "sqlite"
enable_port_reservation = false
active_service = true
```

```bash
chown apps:apps /home/apps/.config/containers/containers.conf
```

---

## 5. 存储配置 storage.conf

写入 `/home/apps/.config/containers/storage.conf`：

```ini
[storage]
driver = "overlay"
graphroot = "/home/apps/srv/storage"
runroot = "/run/user/$(id -u apps)/containers"

[storage.options.overlay]
mount_program = "/usr/bin/fuse-overlayfs"
```

> `fuse-overlayfs` 未安装时，删除 `[storage.options.overlay]` 段。

---

## 6. 镜像搜索域与加速

短名拉取（`/home/apps/.config/containers/registries.conf.d/000-unqualified-search.conf`）：

```toml
unqualified-search-registries = ["docker.io", "quay.io", "gcr.io"]
```

**中国大陆环境**（`001-mirrors.conf`）：

```toml
[[registry]]
location = "docker.io"

[[registry.mirror]]
location = "docker.m.daocloud.io"

[[registry.mirror]]
location = "mirror.baidubce.com"

[[registry.mirror]]
location = "docker.nju.edu.cn"
```

---

## 7. 系统 sysctl

写入 `/etc/sysctl.d/99-debopti-podman.conf`（完整内容参见 `templates/apps/podman/sysctl/99-debopti-podman.conf`）：

```ini
# rootless 容器绑定 80/443 等低端口
net.ipv4.ip_unprivileged_port_start = 80
```

```bash
sysctl --system
```

---

## 8. sudoers 免密代管

将 `myuser` 替换为实际主用户名，写入 `/etc/sudoers.d/debopti-podman`（完整内容参见 `templates/apps/podman/sudoers.d/debopti-podman`）：

```
# debopti Podman 模块 — 主用户代管 apps 容器环境
myuser ALL=(apps) NOPASSWD: SETENV: /usr/bin/podman *
myuser ALL=(apps) NOPASSWD: SETENV: /usr/bin/systemctl --user *
myuser ALL=(apps) NOPASSWD: SETENV: /usr/bin/journalctl *
myuser ALL=(root) NOPASSWD: /usr/bin/machinectl shell apps@
```

```bash
chmod 440 /etc/sudoers.d/debopti-podman
visudo -cf /etc/sudoers.d/debopti-podman
```

---

## 9. Bash 运维别名

写入 `/etc/profile.d/debopti-podman.sh`（完整内容参见 `templates/apps/podman/profile.d/debopti-podman.sh`）：

```bash
# debopti Podman 运维别名 — 主用户代管 apps 用户的 rootless 容器
alias docker=podman 2>/dev/null || true
alias dc='podman-compose' 2>/dev/null || true

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
```

重新登录或执行 `source /etc/profile.d/debopti-podman.sh` 生效。

---

## 10. Fish 运维别名

在 SOT 用户 `~/.config/fish/conf.d/` 下创建 `debopti_podman_abbr.fish`：

```fish
if status is-interactive
    abbr -a docker podman
    abbr -a dc podman-compose
end
```

创建 `debopti_apppod.fish`、`debopti_appctl.fish`、`debopti_applog.fish`、`debopti_appshell.fish`（内容与脚本模板一致，参见 `templates/apps/podman/fish/`）。

---

## 11. 启用 systemd 用户服务

```bash
APPS_UID=$(id -u apps)
sudo -u apps XDG_RUNTIME_DIR=/run/user/$APPS_UID systemctl --user daemon-reload
sudo -u apps XDG_RUNTIME_DIR=/run/user/$APPS_UID \
    systemctl --user enable --now podman-auto-update.timer
```

---

## 12. 镜像清理定时器（默认开启）

清理脚本 `/home/apps/.local/bin/podman-gc.sh`（完整内容参见 `templates/apps/podman/systemd-user/podman-gc.sh`）：

```bash
#!/bin/bash
set -euo pipefail
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
# 清理 7 天前未使用的镜像与容器
exec /usr/bin/podman system prune -f --filter until=168h
```

```bash
chmod 755 /home/apps/.local/bin/podman-gc.sh
chown apps:apps /home/apps/.local/bin/podman-gc.sh
```

单元文件置于 `/home/apps/.config/systemd/user/podman-gc.service` 与 `podman-gc.timer`（参见 `templates/apps/podman/systemd-user/`）。

```bash
sudo -u apps XDG_RUNTIME_DIR=/run/user/$APPS_UID \
    systemctl --user enable --now podman-gc.timer
```

通过 TUI 可随时开关；手动切换：

```bash
appctl enable --now podman-gc.timer   # 开启
appctl disable --now podman-gc.timer  # 关闭
```

---

## 13. Quadlet 声明式服务

示例文件 `/home/apps/.config/containers/systemd/hello.container`：

```ini
[Unit]
Description=Nginx hello example
After=network-online.target

[Container]
Image=docker.io/library/nginx:alpine
PublishPort=8080:80
Volume=/home/apps/srv/nginx/html:/usr/share/nginx/html:Z
AutoUpdate=registry

[Service]
Restart=always
TimeoutStartSec=300

[Install]
WantedBy=default.target
```

部署：

```bash
appctl daemon-reload
appctl enable --now hello.service
applog -u hello.service -f
```

---

## 14. 常用运维命令

| 命令 | 说明 |
|------|------|
| `apppod ps` | 查看容器列表 |
| `apppod pull nginx` | 拉取镜像 |
| `apppod compose up -d` | 启动 compose 项目 |
| `appctl status hello.service` | 查看 Quadlet 服务状态 |
| `appctl restart hello.service` | 重启服务 |
| `applog -u hello.service -f` | 跟踪服务日志 |
| `appshell` | 进入 apps 用户 shell |
| `docker ps` | podman 别名，兼容 Docker 习惯 |

---

## 15. IP 转发

部分容器网络需要系统 IP 转发，参见 [07-port-forwarding.md](07-port-forwarding.md) 或通过 `debopti` TUI 选项 2 开启。

---

## 16. 卸载

```bash
# 停止用户服务
APPS_UID=$(id -u apps)
sudo -u apps XDG_RUNTIME_DIR=/run/user/$APPS_UID \
    systemctl --user disable --now podman-gc.timer podman-auto-update.timer
loginctl disable-linger apps

# 移除配置
rm -f /etc/profile.d/debopti-podman.sh
rm -f /etc/sudoers.d/debopti-podman
rm -f /etc/sysctl.d/99-debopti-podman.conf

# 卸载软件包
apt-get purge -y podman podman-docker podman-compose buildah skopeo uidmap
apt-get autoremove -y

# 可选：删除数据与用户
# rm -rf /home/apps/srv
# userdel -r apps
```
