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

## 2. 内核参数调优（TCP/网络协议栈）

通过 `/etc/sysctl.d/` 目录写入独立配置文件 `99-debopti-optimize.conf`，重启或 `sysctl --system` 后生效，不直接改写系统全局配置文件。

写入 `/etc/sysctl.d/99-debopti-optimize.conf`（完整内容参见 [templates/system/99-debopti-optimize.conf](../templates/system/99-debopti-optimize.conf)）：

```ini
# 解除文件句柄限制 (系统级)
fs.file-max = 1048576

# 核心：开启 BBR 拥塞控制算法 (极大提升高延迟下的传输速度)
net.core.default_qdisc = fq
net.ipv4.tcp_congestion_control = bbr

# TCP 连接回收与复用优化 (针对反代场景)
net.ipv4.tcp_tw_reuse = 1
net.ipv4.tcp_fin_timeout = 15
net.ipv4.tcp_max_tw_buckets = 50000

# 缓冲区扩容：提升单线程吞吐能力
net.core.rmem_max = 16777216
net.core.wmem_max = 16777216
net.ipv4.tcp_rmem = 4096 87380 16777216
net.ipv4.tcp_wmem = 4096 65536 16777216

# 加强防 SYN 洪水攻击能力
net.core.somaxconn = 32768
net.core.netdev_max_backlog = 32768
net.ipv4.tcp_max_syn_backlog = 32768

# 开启 TCP Fast Open 减少握手往返
net.ipv4.tcp_fastopen = 3
```

```bash
sysctl --system
```

验证 BBR 是否生效：

```bash
sysctl net.ipv4.tcp_congestion_control
# 输出应为: net.ipv4.tcp_congestion_control = bbr
```

> **注意**：部分 LXC 容器环境不允许修改内核参数，`sysctl --system` 可能报错，属正常现象。

---

## 3. 文件句柄限制（nofile）

默认的文件描述符上限较低，高并发场景下容易触碰限制。以下配置将系统和 Systemd 服务的上限提升至 1048576（100万级）。

写入 `/etc/security/limits.d/99-debopti-nofile.conf`（完整内容参见 [templates/system/99-debopti-nofile.conf](../templates/system/99-debopti-nofile.conf)）：

```
# 系统级 PAM 文件句柄限制
* soft nofile 1048576
* hard nofile 1048576
root soft nofile 1048576
root hard nofile 1048576
```

```bash
# Systemd 全局服务限制（删除旧的句柄参数，并追加最新值）
sed -i '/^#\?DefaultLimitNOFILE/d' /etc/systemd/system.conf
echo 'DefaultLimitNOFILE=1048576' >> /etc/systemd/system.conf

sed -i '/^#\?DefaultLimitNOFILE/d' /etc/systemd/user.conf
echo 'DefaultLimitNOFILE=1048576' >> /etc/systemd/user.conf
```

修改后需**重新登录终端**使 PAM 限制生效，Systemd 全局服务不需要重启系统即可为新起的服务生效。

验证：

```bash
ulimit -n
# 输出应为: 1048576
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

## 6. 日志轮转与 Journald 配额

为了精简日志大小并提高磁盘 IO 效率，优化脚本直接改写 `/etc/logrotate.conf` 全局配置来控制日志寿命，而非追加子目录配置：

```bash
# 对 /etc/logrotate.conf 配置文件执行全局纠偏
# 1. 强制设为按天轮转 (daily)
sed -i 's/^weekly/daily/g' /etc/logrotate.conf
# 2. 强制保留 7 份历史日志副本
sed -i 's/^rotate [0-9]*/rotate 7/g' /etc/logrotate.conf
# 3. 强制开启全局压缩
sed -i 's/^#compress/compress/g' /etc/logrotate.conf
```

同时限制 Systemd Journal 日志的占用空间：

```bash
sed -i 's/^#\?SystemMaxUse=.*/SystemMaxUse=200M/' /etc/systemd/journald.conf
sed -i 's/^#\?RuntimeMaxUse=.*/RuntimeMaxUse=10M/' /etc/systemd/journald.conf

# 若对应行不存在，则追加
grep -q '^SystemMaxUse=' /etc/systemd/journald.conf || echo 'SystemMaxUse=200M' >> /etc/systemd/journald.conf
grep -q '^RuntimeMaxUse=' /etc/systemd/journald.conf || echo 'RuntimeMaxUse=10M' >> /etc/systemd/journald.conf

systemctl restart systemd-journald
```

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
