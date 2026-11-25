# Podman Rootless 容器环境

本文档介绍如何在 Debian 上部署 Podman rootless 容器环境，与 `debopti` 脚本自动化步骤完全对齐。

**前提条件**：root 权限，Debian 10+，systemd 与 logind 可用

**架构概要**：
- 专用 `apps` 用户以 rootless 方式运行所有容器
- **仅 SOT 用户与 `root`** 可使用 `apppod` / `appctl` / `applog` / `appshell` 代管；其他用户仅同步 `docker`/`dc` 缩写与环境变量（Fish：`debopti_podman_env` / `debopti_podman_abbr`）
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

mkdir -p /home/apps/srv /home/apps/backups
mkdir -p /home/apps/.config/containers/registries.conf.d
mkdir -p /home/apps/.config/containers/systemd
chown -R apps:apps /home/apps
```

**目录约定**：

| 路径 | 用途 |
|------|------|
| `/home/apps/srv/` | 容器 Volume 绑定的业务数据 |
| `/home/apps/backups/` | `pg_dump` 等逻辑备份（如 `backups/postgres/`） |
| `/home/apps/.local/share/containers/storage` | Podman 默认 graphroot（引擎内部，**勿设 ACL**） |

将主用户（将 `myuser` 替换为实际用户名）加入 `apps` 组并设置 **分层 ACL**（勿对 `/home/apps` 或 graphroot 递归授权）：

```bash
usermod -aG apps myuser
setfacl -m u:myuser:--x /home/apps
setfacl -R -m u:myuser:rwx /home/apps/srv /home/apps/backups /home/apps/.config
setfacl -R -d -m u:myuser:rwx /home/apps/srv /home/apps/backups /home/apps/.config
```

---

## 4. 引擎配置 containers.conf

写入 `/home/apps/.config/containers/containers.conf`（`log_size_max` 由 `debopty` 根据宿主机内存自动缩放：Micro ≤1G 为 `10485760`、Small ≤2G 为 `20971520`、Medium ≤8G 为 `52428800`、Large >8G 为 `104857600`，也可在 `/etc/debopti/debopti.conf` 中通过 `PODMAN_LOG_SIZE_MAX` 显式覆盖）：

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
```

```bash
chown apps:apps /home/apps/.config/containers/containers.conf
```

---

## 5. 存储配置 storage.conf

Podman **graphroot / runroot 使用默认路径**，模块不自定义 `graphroot`：

- graphroot: `/home/apps/.local/share/containers/storage`
- runroot: `/run/user/<apps-uid>/containers`

可选写入 `/home/apps/.config/containers/storage.conf`（仅 fuse-overlayfs 调优，完整模板参见 [templates/apps/podman/storage.conf](../templates/apps/podman/storage.conf)）：

```ini
# graphroot / runroot 使用 Podman 默认，不在此覆盖

[storage.options.overlay]
mount_program = "/usr/bin/fuse-overlayfs"
```

> `fuse-overlayfs` 未安装时，可不部署 `storage.conf` 或仅保留注释。

验证 graphroot：

```bash
sudo -u apps podman info --format '{{.Store.GraphRoot}}'
# 应输出: /home/apps/.local/share/containers/storage
```

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

写入 `/etc/sysctl.d/99-debopti-podman.conf`（完整内容参见 [templates/apps/podman/sysctl/99-debopti-podman.conf](../templates/apps/podman/sysctl/99-debopti-podman.conf)）：

```ini
# rootless 容器绑定 80/443 等低端口
net.ipv4.ip_unprivileged_port_start = 80
```

```bash
sysctl --system
```

---

## 8. sudoers 免密代管

将 `myuser` 替换为实际主用户名，写入 `/etc/sudoers.d/debopti-podman`（完整内容参见 [templates/apps/podman/sudoers.d/debopti-podman](../templates/apps/podman/sudoers.d/debopti-podman)）：

```
# debopti Podman 模块 — 主用户代管 apps 容器环境
myuser ALL=(apps) NOPASSWD: SETENV: /usr/bin/podman *
myuser ALL=(apps) NOPASSWD: SETENV: /usr/bin/systemctl --user *
myuser ALL=(apps) NOPASSWD: SETENV: /usr/bin/journalctl *
myuser ALL=(apps) NOPASSWD: SETENV: /usr/bin/restic *
myuser ALL=(apps) NOPASSWD: SETENV: /home/apps/.local/bin/restic-backup.sh
myuser ALL=(apps) NOPASSWD: SETENV: /home/apps/.local/bin/restic-backup.sh *
myuser ALL=(apps) NOPASSWD: SETENV: /usr/bin/bash -c *
myuser ALL=(apps) NOPASSWD: SETENV: /usr/bin/mkdir *
myuser ALL=(root) NOPASSWD: /usr/bin/machinectl shell apps@
```

```bash
chmod 440 /etc/sudoers.d/debopti-podman
visudo -cf /etc/sudoers.d/debopti-podman
```

---

## 9. Bash 运维别名

写入 `/etc/profile.d/debopti-podman.sh`（完整内容参见 [templates/apps/podman/profile.d/debopti-podman.sh](../templates/apps/podman/profile.d/debopti-podman.sh)）：

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

`debopti_podman_env.fish` / `debopti_podman_abbr.fish` 经 SOT 同步至所有用户；`debopti_apppod.fish` 等四个代管函数**仅**部署给 SOT 用户与 `root`（`root` ≠ SOT 时通过 `sync_devops_sot_links` 软链，其他用户会自动移除同名片段）。

在 SOT 用户 `~/.config/fish/conf.d/` 下创建 `debopti_podman_abbr.fish`：

```fish
if status is-interactive
    abbr -a docker podman
    abbr -a dc podman-compose
end
```

创建 `debopti_apppod.fish`、`debopti_appctl.fish`、`debopti_applog.fish`、`debopti_appshell.fish`（内容与脚本模板一致，参见 [templates/apps/podman/fish/](../templates/apps/podman/fish/)）。

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

清理脚本 `/home/apps/.local/bin/podman-gc.sh`（完整内容参见 [templates/apps/podman/systemd-user/podman-gc.sh](../templates/apps/podman/systemd-user/podman-gc.sh)）：

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

单元文件置于 `/home/apps/.config/systemd/user/podman-gc.service` 与 `podman-gc.timer`（参见 [templates/apps/podman/systemd-user/](../templates/apps/podman/systemd-user/)）。

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

### 13.1 在容器中挂载 TLS 证书（Lego / Ferron）

Lego 申请/续期后，证书会推送到 `/etc/ferron/certs/`（目录 `750 root:debopti-certs`，证书 `644`、私钥 `640`）。**仅属主或 `debopti-certs` 组成员**能读取私钥。

#### 主机上的服务用户

将运行用户加入 `debopti-certs` 组，并重启该服务（或重新登录）：

```bash
sudo usermod -aG debopti-certs <服务用户>
sudo systemctl restart <服务名>
```

**Podman `apps` 用户**：安装 Podman 模块时会**自动**创建 `debopti-certs` 组（若尚未存在）并把 `apps` 加入；若刚手动改过组成员，可结束 linger 会话以刷新组信息：

```bash
sudo loginctl terminate-user apps
appctl daemon-reload
appctl restart <你的服务>.service
```

验证 `apps` 在主机上可读证书：

```bash
sudo -u apps test -r /etc/ferron/certs/example.com.key && echo OK
```

#### Rootless 容器内使用

主机加组**不等于**容器内自动可读，还需：

1. **挂载**证书目录（建议只读 `:ro`，SELinux 环境加 `:Z`）
2. **`GroupAdd=keep-groups`**，把主机 `apps` 的附属组（含 `debopti-certs`）传入容器进程

Quadlet 示例（安装 Podman 后位于 `/home/apps/.config/containers/systemd/tls-ferron-certs.container.example`）：

```ini
[Container]
Image=docker.io/library/nginx:alpine
Volume=/etc/ferron/certs:/etc/ferron/certs:ro,Z
GroupAdd=keep-groups
```

等价的 `podman run`：

```bash
apppod run -d --name my-tls \
  --group-add keep-groups \
  -v /etc/ferron/certs:/etc/ferron/certs:ro,Z \
  docker.io/library/nginx:alpine
```

容器内应用配置指向挂载路径，例如 Nginx/Caddy 的 `ssl_certificate` / `tls` 使用 `/etc/ferron/certs/example.com.crt` 与 `.key`。

证书权限与 Lego 推送逻辑详见 [06-devops-tools.md §4](06-devops-tools.md#4-lego-自动化证书管理)。

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

## 16. ACL 禁忌与 Restic 备份

### 16.1 为何 graphroot 禁止 ACL

对 `/home/apps` 或 graphroot（`.local/share/containers/storage`）递归 `setfacl`，会使 ACL 写入 overlay 层目录，导致 **fuse-overlayfs 无法挂载**、`merged` 为空，容器启动时 crun 报「文件不存在」。

**禁止**：

```bash
# 错误示例 — 会导致容器无法拉起
setfacl -R -m u:myuser:rwx /home/apps
setfacl -R -m u:myuser:rwx /home/apps/.local/share/containers/storage
```

**已污染时恢复**：

```bash
setfacl -R -b /home/apps/.local/share/containers/storage
setfacl -k /home/apps/.local/share/containers/storage
# 或重新执行 debopti Podman 模块安装（幂等 scrub）
```

### 16.2 Restic 推荐流程与动态 0700 目录健壮性

以 **apps 身份**备份，主用户触发、无需对家目录递归 ACL。以下示例均采用 `env` 显式注入环境变量，**同时兼容 `bash` 与默认登录 Shell 为 `fish` 的环境**（避免依赖 Bash 专属的 `VAR=val cmd` 前缀语法）：

```bash
# PG 逻辑备份（示例，兼容 bash / fish）
sudo -u apps mkdir -p /home/apps/backups/postgres
sudo -u apps /usr/bin/bash -c 'podman exec -u postgres shared-pgsql pg_dumpall --clean --if-exists | gzip -c > /home/apps/backups/postgres/dump.sql.gz'

# 推荐：通过弹性包装脚本备份并自动执行 forget --prune（兼容 bash / fish）
sudo -u apps env \
  RESTIC_REPOSITORY=/mnt/backup/restic-repo \
  RESTIC_PASSWORD_FILE=/home/apps/.config/restic-password \
  /home/apps/.local/bin/restic-backup.sh
```

**动态 `0700` 目录与 Restic 退出码 `3` 说明**：

- 容器内部分动态运行时目录（例如 Gitea `sessions`、`indexers/issues.bleve`、`tmp`、`cache`）常以容器内子 UID 创建为 `0700`，宿主侧 `restic` 读取时会返回 **退出码 `3`（快照已生成，但部分文件不可读）**。
- 模块部署的排除模板 `/home/apps/.config/restic-excludes`（参见 [templates/apps/podman/restic-excludes.txt](../templates/apps/podman/restic-excludes.txt)）已默认排除上述低成本可重建目录。
- 模块同时部署 `/home/apps/.local/bin/restic-backup.sh`（参见 [templates/apps/podman/restic-backup.sh](../templates/apps/podman/restic-backup.sh)），在 `set -Eeuo pipefail` 与 `trap ERR` 保护下：
  - `backup` 返回 `0`：正常记录并继续执行 `restic forget --prune`。
  - `backup` 返回 `3`：捕获退出码而不触发 `ERR` 中断，记录 `[WARN]` 告警后**继续完成** `restic forget --prune`。
  - `backup` 返回其他非零退出码（如 `1` 仓库/认证失败）：记录 `[ERROR]` 并在 `forget --prune` 之前直接中止，避免误删历史快照。
  - 可通过环境变量调整行为：`RESTIC_BACKUP_TARGET`、`RESTIC_EXCLUDE_FILE`、`RESTIC_KEEP_DAILY`（默认 `7`）、`RESTIC_KEEP_WEEKLY`（默认 `4`）、`RESTIC_KEEP_MONTHLY`（默认 `6`）、`RESTIC_SKIP_PRUNE=1`（跳过清理）。

---

## 17. 卸载

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

# 可选：删除业务数据与用户
# rm -rf /home/apps/srv /home/apps/backups
# userdel -r apps
```
