# 系统调优

本文档介绍如何手动完成 Debian 系统的基础性能调优，内容与自动优化脚本 `scripts/system.sh` 严格对齐。

**前提条件**：root 权限，Debian 10+

---

## 1. 更新系统与安装基础工具

先同步软件包列表并升级系统，然后安装后续步骤需要的工具。特别针对 Debian 13 (Trixie) 及之后版本对 `dnsutils` 包更名为 `bind9-dnsutils` 做出了兼容性处理，并引入了 `whois` 与 `jq`：

```bash
apt-get update -y && apt-get upgrade -y

# 动态获取 Debian 主版本号，处理更名兼容性
debian_ver=$(cut -d. -f1 /etc/debian_version 2>/dev/null || echo "0")
dns_pkg="dnsutils"
if [[ "$debian_ver" -ge 13 ]]; then
    dns_pkg="bind9-dnsutils"
fi

# 安装基础运维工具包
apt-get install -y curl wget gnupg lsb-release procps unzip tar openssl git logrotate whois $dns_pkg net-tools jq sudo

# 移除冗余孤立包
apt-get autoremove -y
```

**Debian 10 (Buster) 已 EOL 说明**：Debian 10 的官方软件源已停止更新，需切换至存档源：

```bash
cp /etc/apt/sources.list /etc/apt/sources.list.bak
```

编辑 `/etc/apt/sources.list`，填入以下内容（完整内容参见 [templates/system/sources.list.buster.global](../templates/system/sources.list.buster.global)；中国大陆镜像版参见 [templates/system/sources.list.buster.cn](../templates/system/sources.list.buster.cn)）：

```
# Debian 10 Buster 存档源
deb https://archive.debian.org/debian buster main contrib non-free
deb https://archive.debian.org/debian-security buster/updates main contrib non-free
```

```bash
apt-get update -y
```

**中国大陆用户**：将 `deb.debian.org` 替换为清华大学 TUNA 镜像站（Debian 11+）：

```bash
sed -i 's/deb.debian.org/mirrors.tuna.tsinghua.edu.cn/g' /etc/apt/sources.list
sed -i 's/security.debian.org/mirrors.tuna.tsinghua.edu.cn/g' /etc/apt/sources.list
# 同时切换为安全 HTTPS 协议
sed -i 's|http://|https://|g' /etc/apt/sources.list
apt-get update -y
```

---

## 2. 硬件自适应调优与内核参数（TCP/网络协议栈）

`debopty` 会自动探测宿主机 CPU 核数、物理内存、根分区磁盘容量/可用空间、容器环境与 ZRAM 模块，动态计算调优档位与配额，无需在模板中写死固定数字。

查看当前宿主机的探测结果与生效档位（只读，Bash 与 Fish 通用）：

```sh
debopty system hardware
# 或以 JSON 输出供脚本/Agent 消费
debopty system hardware --json
```

### 2.1 内存与磁盘自适应档位矩阵

| 参数 | Micro (≤1G) | Small (1–2G) | Medium (2–8G) | Large (>8G) | `debopti.conf` 覆盖键 |
|---|---|---|---|---|---|
| `fs.file-max` | `131072` | `262144` | `524288` | `1048576` | `TUNING_FILE_MAX` |
| `nofile` (`limits` / `systemd`) | `65535` | `131072` | `524288` | `1048576` | `TUNING_NOFILE` |
| `tcp_max_tw_buckets` | `16384` | `32768` | `50000` | `131072` | `TUNING_TCP_MAX_TW_BUCKETS` |
| `rmem_max` / `wmem_max` | `4M` | `8M` | `16M` | `32M` | `TUNING_NET_BUF_MAX` |
| `somaxconn` / `backlog` | `4096` | `8192` | `16384` | `32768` | `TUNING_BACKLOG` |
| ZRAM 算法 / 占比 | `lz4`(≤512M 1C) / `zstd` (`60%`) | `zstd` (`50%`) | `zstd` (`40%`) | `zstd` (`25%`) | `ZRAM_ALGO` / `ZRAM_PERCENT` |
| 自动 Swapfile | `1024M` (且 ≤ 可用盘 20%) | `2048M` (且 ≤ 可用盘 20%) | ≤4G `2048M` / >4G `0` | `0` (仅 ZRAM) | `SWAP_SIZE_MB` |
| 编译并发 (`GOMAXPROCS` / `CARGO_BUILD_JOBS`) | `1` | `min(核数, 2)` | `min(核数, 4)` | `核数` | `BUILD_JOBS` |

磁盘留存档位（按根分区总容量自动划分）：

| 参数 | Small (≤15G) | Standard (16–40G) | Medium (41–100G) | Large (>100G) | `debopti.conf` 覆盖键 |
|---|---|---|---|---|---|
| `SystemMaxUse` | `100M` | `200M` | `500M` | `1G` | `JOURNAL_SYSTEM_MAX_USE` |
| `MaxRetentionSec` | `14day` | `30day` | `30day` | `60day` | `JOURNAL_MAX_RETENTION_SEC` |
| 用户冷缓存留存 (`tmpfiles.d`) | `14d` | `30d` | `30d` | `30d` | `CACHE_RETENTION_AGE` |
| `debopty-apt-clean.timer` | `daily` | `weekly` | `weekly` | `weekly` | `APT_CLEAN_CALENDAR` |
| `logrotate` 保留份数 | `5` | `7` | `7` | `14` | `LOGROTATE_ROTATE_COUNT` |

### 2.2 内核参数模板

通过 `/etc/sysctl.d/` 目录写入独立配置文件 `99-debopti-optimize.conf`（模板参见 [templates/system/99-debopti-optimize.conf](../templates/system/99-debopti-optimize.conf)，占位符由 `AdaptiveTuningProfile` 渲染），重启或 `sysctl --system` 后生效：

```sh
sudo debopty system sysctl
sysctl net.ipv4.tcp_congestion_control
# 输出应为: net.ipv4.tcp_congestion_control = bbr
```

> **注意**：部分 LXC 容器环境不允许修改内核参数，`sysctl --system` 可能报错，属正常现象。

---

## 3. 文件句柄限制（nofile）

文件描述符上限按内存档位自动分配（Micro `65535` → Large `1048576`，可通过 `/etc/debopti/debopti.conf` 中的 `TUNING_NOFILE` 覆盖），模板参见 [templates/system/99-debopti-nofile.conf](../templates/system/99-debopti-nofile.conf)。

通过 CLI 一键同步 `/etc/security/limits.d/99-debopti-nofile.conf` 与 Systemd `DefaultLimitNOFILE`：

```sh
sudo debopty system limits
```

修改后需**重新登录终端**使 PAM 限制生效，验证命令（Bash 与 Fish 通用）：

```sh
ulimit -n
```

---

## 4. IP 路由转发

**建站模式（不需要转发）**：默认关闭，无需操作。

**代理 / 组网 / 容器模式（需要转发）**：

通过写入 `99-debopti-forwarding.conf`，开启内核的 IPv4 与 IPv6 路由转发和放行功能。

写入 `/etc/sysctl.d/99-debopti-forwarding.conf`（完整内容参见 [templates/system/99-debopti-forwarding.conf](../templates/system/99-debopti-forwarding.conf)）：

```ini
# 开启 IPv4/IPv6 路由转发（代理/组网/容器模式）
net.ipv4.ip_forward = 1
net.ipv4.conf.all.forwarding = 1
net.ipv4.conf.default.forwarding = 1
net.ipv6.conf.all.forwarding = 1
net.ipv6.conf.default.forwarding = 1
```

```bash
sysctl --system
```

关闭转发：

```bash
rm -f /etc/sysctl.d/99-debopti-forwarding.conf
sysctl -w net.ipv4.ip_forward=0
# 针对 IPv4/IPv6 各网卡的转发设为 0
sysctl -w net.ipv4.conf.all.forwarding=0
sysctl -w net.ipv4.conf.default.forwarding=0
sysctl -w net.ipv6.conf.all.forwarding=0
sysctl -w net.ipv6.conf.default.forwarding=0
```

---

## 5. 内存管理

### 5.1 ZRAM（压缩内存，适合 ≤2GB 内存的 VPS）

ZRAM 通过 CPU 压缩来扩展可用内存，通常比传统磁盘 Swap 响应更快。脚本设定的优先级为 100，确保优先使用压缩内存。

```bash
apt-get install -y zram-tools
```

写入 `/etc/default/zramswap`（完整内容参见 [templates/system/zramswap](../templates/system/zramswap)）：

```ini
# 压缩算法：zstd 压缩率最高，lz4 速度最快
ALGO=zstd
# 使用物理内存的百分比作为 ZRAM 大小
PERCENT=50
# 交换区优先度，设为 100 以获得更高计算优先度
PRIORITY=100
```

```bash
systemctl restart zramswap
```

验证：

```bash
zramctl
# 或
swapon --show
```

### 5.2 Swap 交换文件

适合磁盘有余量、内存长期处于压力边缘的场景。以下命令创建与物理内存 2 倍大小的 Swap 文件：

```bash
MEM_MB=$(( $(grep MemTotal /proc/meminfo | awk '{print $2}') / 1024 ))
SWAP_SIZE=$(( MEM_MB * 2 ))  # 创建 2 倍内存大小的 Swap

# 创建文件（fallocate 更快，部分文件系统不支持时回退到 dd）
fallocate -l ${SWAP_SIZE}M /swapfile 2>/dev/null || \
    dd if=/dev/zero of=/swapfile bs=1M count=${SWAP_SIZE} status=progress

chmod 600 /swapfile
mkswap /swapfile
swapon /swapfile

# 写入 fstab，确保重启后自动挂载
grep -q '/swapfile' /etc/fstab || echo '/swapfile none swap sw 0 0' >> /etc/fstab
```

> **LXC 容器**：部分 LXC 容器不支持挂载 Swap，`swapon` 会报错，属正常现象，跳过即可。

---

## 6. 声明式日志留存、冷缓存清理与 APT 定时维护

`debopty` 将日志配额、用户冷缓存清理与 APT 包缓存清理统一纳入声明式留存模块（安装、`debopty system retention`、基础优化与 `reconcile` 共享同一套硬件自适应配置）。

一键应用或校验声明式留存策略（Bash 与 Fish 通用）：

```sh
sudo debopty system retention
```

### 6.1 Logrotate 与 Journald 声明式 Drop-in

- `/etc/logrotate.conf`：统一收敛全局策略为 `daily`、`compress` 与自适应 `rotate <N>`（Small 磁盘 5 份 / Standard 7 份 / Large 14 份），完整保留 `{ ... }` 块与 `/etc/logrotate.d/*`。
- `/etc/systemd/journald.conf.d/99-debopty-retention.conf`（模板参见 [templates/system/99-debopty-retention.conf](../templates/system/99-debopty-retention.conf)）：通过 drop-in 设置 `SystemMaxUse`、`RuntimeMaxUse` 与 `MaxRetentionSec`，不直接改写 `/etc/systemd/journald.conf`，并平滑执行 `systemctl reload-or-restart systemd-journald`。

### 6.2 用户级冷缓存留存（`user-tmpfiles.d`）

写入 `~/.config/user-tmpfiles.d/debopty-cache-retention.conf`（模板参见 [templates/system/debopty-cache-retention.conf](../templates/system/debopty-cache-retention.conf)），使用 `e %C/<dir> - - - <age>` 规则按访问时间自动清理 `restic`、`rclone`、`fish`、`yazi`、`pip`、`go-build` 冷缓存，仅清理已存在目录、不新建目录、不触碰 `~/.config` 或凭据文件，并尽力启用用户级 `systemd-tmpfiles-clean.timer`。

### 6.3 声明式 APT 包缓存清理

- `/etc/apt/apt.conf.d/99-debopty-apt-clean`（[模板](../templates/system/99-debopty-apt-clean)）：设置 `APT::Keep-Downloaded-Packages "false";`。
- `/etc/systemd/system/debopty-apt-clean.service` 与 `debopty-apt-clean.timer`（[service 模板](../templates/system/debopty-apt-clean.service)、[timer 模板](../templates/system/debopty-apt-clean.timer)）：在沙箱加固（`ProtectSystem=full`、`ProtectHome=true`、`PrivateTmp=true`、`NoNewPrivileges=true`）下定期执行 `apt-get -q clean`，防止小容量 VPS 的 `/var/cache/apt/archives` 持续膨胀。

---

## 7. 系统精简（低内存 VPS 专项）

适合 512MB 或 1GB 内存的 VPS，通过停用闲置服务与卸载 rsyslog 来释放内存。

### 7.1 削减多余 TTY

系统默认开启 6 个虚拟终端（tty1–tty6），普通 VPS 不需要这么多，限制为 2 个：

```bash
sed -i 's/^#\?NAutoVTs=.*/NAutoVTs=2/' /etc/systemd/logind.conf
sed -i 's/^#\?ReserveVT=.*/ReserveVT=2/' /etc/systemd/logind.conf

grep -q '^NAutoVTs=' /etc/systemd/logind.conf || echo 'NAutoVTs=2' >> /etc/systemd/logind.conf
grep -q '^ReserveVT=' /etc/systemd/logind.conf || echo 'ReserveVT=2' >> /etc/systemd/logind.conf

systemctl restart systemd-logind
```

### 7.2 移除 rsyslog

Debian 默认同时安装 `rsyslog` 和 `systemd-journald`，两者功能重叠，移除 rsyslog 可释放约 3–5MB 内存，完全由 journald 接管：

```bash
apt-get purge -y rsyslog
apt-get autoremove -y
```

### 7.3 停用与屏蔽闲置服务

以下服务在纯 VPS（非图形界面、非物理硬件绑定）环境中通常是不需要的：

```bash
# ModemManager：调制解调器管理（VPS 上没有 SIM 卡）
# Avahi：mDNS/zeroconf 服务发现（局域网功能，VPS 不需要）
# Bluetooth：蓝牙服务（VPS 上没有蓝牙硬件）
# Cups: 打印服务（VPS 不需要打印）
# Pnmos: 网络发现优化组件

for svc in ModemManager avahi-daemon bluetooth cups pnmos; do
    if systemctl list-unit-files | grep -q "^${svc}.service"; then
        systemctl stop "$svc" 2>/dev/null || true
        systemctl mask "$svc"
        echo "已屏蔽服务: $svc"
    fi
done
```

> `mask` 比 `disable` 更彻底，会阻止服务被其他依赖程序意外拉起。

---

## 8. 时区与时间同步

```bash
# 设置时区为上海（UTC+8）
timedatectl set-timezone Asia/Shanghai

# 安装并启用 chrony（比 ntp 更现代的高能时间同步工具）
apt-get install -y chrony
systemctl enable --now chrony

# 验证
timedatectl
chronyc tracking
```

---

## 9. 内核更换

Cloud 内核针对虚拟化环境做了裁剪，移除了物理机驱动，启动更快，内存占用更低。适合 KVM / Xen 类型的 VPS。

```bash
# 查看当前内核
uname -r

# 安装 Cloud 内核（x86_64）
apt-get install -y linux-image-cloud-amd64 linux-headers-cloud-amd64

# 更新 GRUB
update-grub

# 重启后验证
reboot
# 重启后执行：
uname -r  # 应包含 "cloud"
```

安装 Cloud 内核后，可清理旧内核释放 `/boot` 空间：

```bash
# 查看所有已安装内核
dpkg -l | grep linux-image

# 删除不需要的旧物理机内核（将 X.X.X-X 替换为实际版本号，通常是不带 cloud 的版本）
apt-get purge -y linux-image-X.X.X-X-amd64
apt-get autoremove -y
```

---

## 10. tmux 终端复用与标准配置

系统初始化默认安装 `tmux` 并部署全局标准配置 `/etc/tmux.conf`（无 TPM 外部插件依赖，离线可用，兼容 Debian 10–13），用于保障 SSH 断线重连后的会话持久化、TrueColor 真彩色渲染、Yazi 图片预览与 OSC52 剪贴板直通。

### 10.1 安装与备份既有配置

```bash
apt-get install -y tmux

# 若已存在手动维护的 /etc/tmux.conf（非 debopty 托管），先原子备份
if [[ -f /etc/tmux.conf ]] && ! grep -q '# managed-by-debopty' /etc/tmux.conf; then
    [[ ! -e /etc/tmux.conf.debopty.bak ]] && cp -p /etc/tmux.conf /etc/tmux.conf.debopty.bak
fi
```

### 10.2 按系统能力生成 `/etc/tmux.conf`

先检测系统是否具备 `tmux-256color` terminfo 以及当前 `tmux` 版本（完整模板参见 [templates/system/tmux.conf](../templates/system/tmux.conf)）：

```bash
# 1. 检测 terminfo（Debian 10 等旧环境若无 tmux-256color 则回退至 screen-256color）
default_term="screen-256color"
infocmp tmux-256color >/dev/null 2>&1 && default_term="tmux-256color"

# 2. 查看 tmux 版本（>= 3.2 支持 terminal-features，>= 3.3 支持 allow-passthrough）
tmux -V
```

写入 `/etc/tmux.conf`（其中 `default-terminal` 按上述检测结果填入；若 `tmux < 3.2` 则省略 `terminal-features`，若 `tmux < 3.3` 则省略 `allow-passthrough`）：

```tmux
# managed-by-debopty
# /etc/tmux.conf - debopty 标准生产配置 (兼容 Debian 10-13，零外部插件依赖)

# ----------------- 终端与 TrueColor / 直通 -----------------
set -g default-terminal "tmux-256color"
set -as terminal-overrides ",xterm-256color:RGB,xterm-kitty:RGB,alacritty:RGB"
set -as terminal-features ",*:RGB"
set -g allow-passthrough on

# ----------------- 响应速度与历史缓冲 -----------------
set -sg escape-time 10
set -g focus-events on
set -g history-limit 50000
set -g display-time 2500
set -g repeat-time 400

# ----------------- 鼠标与剪贴板 -----------------
set -g mouse on
set -g set-clipboard on

# ----------------- 索引与多端体验 -----------------
set -g base-index 1
setw -g pane-base-index 1
set -g renumber-windows on
setw -g automatic-rename on
set -g set-titles on
setw -g aggressive-resize on
setw -g monitor-activity on
set -g visual-activity off

# ----------------- 标准快捷键 (保留 Ctrl+b 前缀) -----------------
# 继承当前工作目录的新建窗口与分屏
bind | split-window -h -c "#{pane_current_path}"
bind - split-window -v -c "#{pane_current_path}"
bind '"' split-window -v -c "#{pane_current_path}"
bind % split-window -h -c "#{pane_current_path}"
bind c new-window -c "#{pane_current_path}"

# 无前缀 Alt+方向键切换窗格 (不与 micro 快捷键冲突)
bind -n M-Left select-pane -L
bind -n M-Right select-pane -R
bind -n M-Up select-pane -U
bind -n M-Down select-pane -D

# 重载全局配置
bind r source-file /etc/tmux.conf \; display-message "tmux config reloaded"

# ----------------- 纯净状态栏 (ASCII/UTF-8 安全) -----------------
set -g status on
set -g status-interval 5
set -g status-position bottom
set -g status-justify left
set -g status-style "bg=colour235,fg=colour250"
set -g status-left-length 32
set -g status-right-length 48
set -g status-left "#[fg=colour39,bold][#S] #[default]"
set -g status-right "#[fg=colour244]%Y-%m-%d %H:%M | #[fg=colour39]#H#[default]"
setw -g window-status-format " #I:#W "
setw -g window-status-current-format "#[bg=colour39,fg=colour232,bold] #I:#W "
set -g pane-border-style "fg=colour238"
set -g pane-active-border-style "fg=colour39"
set -g message-style "bg=colour236,fg=colour220,bold"
```

```bash
chmod 644 /etc/tmux.conf

# 若当前已有运行中的 tmux 服务，热重载配置
tmux list-sessions >/dev/null 2>&1 && tmux source-file /etc/tmux.conf
```

### 10.3 常用快捷键与恢复/卸载

- **分屏与新建窗口**（自动沿用当前路径）：`Ctrl+b |` 或 `Ctrl+b %` 左右分屏，`Ctrl+b -` 或 `Ctrl+b "` 上下分屏，`Ctrl+b c` 新建窗口。
- **窗格切换与重载**：`Alt+方向键` 直接切换窗格（无前缀，不冲突 `micro`），`Ctrl+b r` 重载 `/etc/tmux.conf`。
- **安全回滚配置**：若存在备份 `/etc/tmux.conf.debopty.bak`，执行 `mv -f /etc/tmux.conf.debopty.bak /etc/tmux.conf` 即可无损恢复原配置；否则仅在确认包含 `# managed-by-debopty` 时删除 `/etc/tmux.conf`。
