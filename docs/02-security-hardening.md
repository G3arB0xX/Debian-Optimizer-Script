# 安全加固

本文档介绍如何手动完成 SSH 加固与 nftables 防火墙配置。

**前提条件**：root 权限，Debian 10+

> ⚠️ **操作 SSH 配置前请保留一个已连接的终端会话**。如果新配置出错导致无法登录，可以通过现有会话回滚。

---

## 1. 创建普通用户（sudo 权限）

直接使用 root 登录是常见的安全隐患。建议创建一个普通用户，通过 `sudo` 执行特权操作。

```bash
# 将 myuser 替换为你想要的用户名
USERNAME="myuser"

# 创建用户（-m 创建家目录，-s 设置默认 shell）
useradd -m -s /bin/bash "$USERNAME"

# 设置密码
passwd "$USERNAME"

# 加入 sudo 组
usermod -aG sudo "$USERNAME"

# 验证
id "$USERNAME"
# 应包含 groups=... 27(sudo) ...
```

---

## 2. 配置 SSH 密钥登录

### 2.1 在本地机器生成密钥对

在你**本地电脑**（不是服务器）的终端执行：

```bash
# 生成 Ed25519 密钥对（更安全、更小）
ssh-keygen -t ed25519 -C "my-server-key"

# 按提示选择保存路径和密码短语（密码短语可为空）
# 默认保存到 ~/.ssh/id_ed25519 和 ~/.ssh/id_ed25519.pub
```

查看公钥内容（后续需要复制到服务器）：

```bash
cat ~/.ssh/id_ed25519.pub
# 输出类似: ssh-ed25519 AAAA... my-server-key
```

### 2.2 将公钥注入服务器

将上一步的公钥内容复制，然后在服务器上执行（将 `myuser` 替换为实际用户名）：

```bash
USERNAME="myuser"
USER_HOME=$(eval echo "~$USERNAME")
SSH_DIR="$USER_HOME/.ssh"

mkdir -p "$SSH_DIR"
chmod 700 "$SSH_DIR"

# 将公钥粘贴到这里（替换引号内的内容）
echo "ssh-ed25519 AAAA...你的公钥内容..." > "$SSH_DIR/authorized_keys"

chmod 600 "$SSH_DIR/authorized_keys"
chown -R "$USERNAME:$USERNAME" "$SSH_DIR"
```

或者从本地机器直接一键推送（更方便）：

```bash
# 在本地机器执行，22 改为服务器当前 SSH 端口
ssh-copy-id -i ~/.ssh/id_ed25519.pub -p 22 myuser@your-server-ip
```

---

## 3. 修改 SSH 端口

将 SSH 端口迁移到高位端口（40000 以上），可以过滤掉大量针对 22 端口的自动扫描攻击。

### 3.1 确定新端口

```bash
# 随机生成一个 40000–65535 之间的端口
NEW_PORT=$((RANDOM % 25535 + 40000))
echo "新端口: $NEW_PORT"
```

记录这个端口号，后续步骤会用到。

### 3.2 检查系统使用的是哪种 SSH 模式

```bash
# 如果输出 active，说明使用的是 Systemd Socket 激活模式（Debian 12+ 默认）
systemctl is-active ssh.socket

# 如果输出 active，说明使用的是传统守护进程模式
systemctl is-active sshd
```

### 3.3 修改端口（Systemd Socket 模式）

适用于 `ssh.socket` 处于 active 状态的情况（Debian 12+）：

```bash
mkdir -p /etc/systemd/system/ssh.socket.d/
```

将 `50022` 替换为你的端口，写入 `/etc/systemd/system/ssh.socket.d/override.conf`（完整内容参见 [templates/security/ssh.socket.override.conf](../templates/security/ssh.socket.override.conf)）：

> Systemd Socket Override 机制：第一行 `ListenStream=`（空值）清空父单元默认的 22 端口监听；第二行指定新端口。若不写第一行，新端口会与 22 并存。

```ini
[Socket]
# 清空父单元默认的 22 端口监听
ListenStream=
# 替换为你的 SSH 端口
ListenStream=50022
```

```bash
systemctl daemon-reload
systemctl restart ssh.socket
```

### 3.4 修改端口（传统 sshd 模式）

适用于 `sshd` 处于 active 状态的情况（Debian 11 及以下）：

```bash
NEW_PORT=50022  # 替换为你的端口

sed -i "s/^#\?Port [0-9]*/Port $NEW_PORT/" /etc/ssh/sshd_config

systemctl restart ssh
```

### 3.5 验证新端口是否生效

**在新终端窗口测试连接（不要关闭当前窗口！）**：

```bash
# 在本地机器执行
ssh -p 50022 myuser@your-server-ip
```

确认可以成功登录后，再进行下一步。如果无法登录，在旧终端窗口中回滚端口修改（改回 22）。

---

## 4. 锁定 SSH 访问（禁止密码登录和 root 登录）

**仅在确认密钥登录和新端口都正常工作后再执行此步骤。**

```bash
# 修改 sshd_config
sed -i 's/^#\?PermitRootLogin .*/PermitRootLogin no/' /etc/ssh/sshd_config
sed -i 's/^#\?PasswordAuthentication .*/PasswordAuthentication no/' /etc/ssh/sshd_config
sed -i 's/^#\?PubkeyAuthentication .*/PubkeyAuthentication yes/' /etc/ssh/sshd_config

# 若行不存在则追加
grep -q '^PermitRootLogin' /etc/ssh/sshd_config || echo 'PermitRootLogin no' >> /etc/ssh/sshd_config
grep -q '^PasswordAuthentication' /etc/ssh/sshd_config || echo 'PasswordAuthentication no' >> /etc/ssh/sshd_config

# 重启 SSH
systemctl restart ssh

# 锁定 root 账户密码（禁止通过密码切换到 root）
passwd -l root
```

---

## 5. nftables 防火墙配置

nftables 是现代 Linux 系统的标准防火墙，是 iptables 的替代品。

> ⚠️ **不要同时使用 nftables 和 ufw/firewalld**，它们都会管理底层 iptables/nftables 规则，同时使用会产生冲突。

### 5.1 安装并启用 nftables

```bash
# 如果已安装 ufw，先彻底移除
if command -v ufw > /dev/null 2>&1; then
    ufw disable
    apt-get purge -y ufw
fi

apt-get install -y nftables fail2ban
systemctl enable --now nftables
```

### 5.2 配置基础防火墙规则

以下是一套适用于大多数 VPS 场景的基础规则模板，放行 SSH、HTTP、HTTPS，阻断其他入站连接。

将 `50022` 替换为你的实际 SSH 端口，写入 `/etc/nftables.conf`（基础结构参见 [templates/security/nftables.conf](../templates/security/nftables.conf)，手动部署时需额外添加 HTTP/HTTPS 放行规则）：

```nft
#!/usr/sbin/nft -f
# 清空所有现有规则，保证每次加载时从零开始
flush ruleset

table inet filter {
    chain input {
        # 默认丢弃所有入站流量，只放行明确允许的
        type filter hook input priority 0; policy drop;
        # 允许环回接口数据流
        iifname "lo" accept
        # 允许已建立和关联的报文
        ct state established,related accept
        # 丢弃无效状态报文
        ct state invalid drop
        # 允许 ICMP/ICMPv6 (Ping) 并限速
        ip protocol icmp icmp type echo-request limit rate 5/second accept
        ip6 nexthdr icmpv6 icmpv6 type echo-request limit rate 5/second accept
        # 放行 IPv6 邻居发现协议 (NDP)
        ip6 nexthdr icmpv6 icmpv6 type { nd-router-solicit, nd-router-advert, nd-neighbor-solicit, nd-neighbor-advert } accept
        # 放行 SSH 端口（替换为实际端口）
        tcp dport 50022 accept comment "SSH_Access_Port"
        # 放行 HTTP / HTTPS
        tcp dport { 80, 443 } accept
        udp dport 443 accept
    }
    chain forward {
        type filter hook forward priority 0; policy accept;
    }
    chain output {
        type filter hook output priority 0; policy accept;
    }
}

# 引入动态规则目录
include "/etc/nftables.d/*.nft"
```

```bash
nft -f /etc/nftables.conf
nft list ruleset
```

### 5.3 模块化规则管理（推荐与对齐）

对于需要频繁增删规则的场景（如安装各类服务），优化脚本在 `/etc/nftables.d/` 目录下创建了 `debopti` 子文件夹，实现规则的隔离与原子加载：

```bash
# 1. 创建模块化规则目录结构
mkdir -p /etc/nftables.d/debopti

# 2. 确保主配置文件末尾包含引入根目录指令
grep -q 'include "/etc/nftables.d/*.nft"' /etc/nftables.conf || \
    echo 'include "/etc/nftables.d/*.nft"' >> /etc/nftables.conf

# 3. 创建模块引入入口文件，指向 debopti 子文件夹
echo 'include "/etc/nftables.d/debopti/*.nft"' > /etc/nftables.d/50-debopti.nft
```

添加单个端口规则（示例：以 `Custom_Web_8080` 为注释放行 `8080` 端口）。

写入 `/etc/nftables.d/debopti/Custom_Web_8080.nft`（规则结构参见 [templates/security/rule_template.nft](../templates/security/rule_template.nft)）：

```nft
table inet filter {
    chain input {
        # 放行 8080 端口（文件名与 comment 中的空格会转为下划线）
        tcp dport 8080 accept comment "Custom_Web_8080"
    }
}
```

```bash
nft -f /etc/nftables.conf
```

删除规则：直接删除对应的 `.nft` 规则文件，然后重载防火墙服务（系统会自动触发 `debopty-f2b-sync` 同步 Fail2ban 保护端口并重建 `f2b-table`）：

```bash
rm -f /etc/nftables.d/debopti/Custom_Web_8080.nft
systemctl reload nftables
```

> 若手动执行 `nft -f /etc/nftables.conf`（其中包含 `flush ruleset`），请紧接着执行 `/usr/local/bin/debopty-f2b-sync --force-reload` 以同步公网开放端口并恢复 Fail2ban 内核封禁表。

### 5.4 常用 nftables 操作命令

```bash
# 查看当前规则
nft list ruleset

# 重新加载规则（通过 systemd 联动 hook 自动同步 Fail2ban）
systemctl reload nftables

# 完全重载（先清空再加载，随后强制恢复 Fail2ban 规则表）
nft -f /etc/nftables.conf && /usr/local/bin/debopty-f2b-sync --force-reload

# 查看规则计数器（用于调试流量统计）
nft list ruleset | grep -i counter

# 临时放行某个端口（重启或重载后失效）
nft add rule inet filter input tcp dport 9000 accept

# 立即拦截某个 IP（临时，需要写入规则文件才能持久）
nft add rule inet filter input ip saddr 1.2.3.4 drop
```

### 5.5 配置 Fail2ban 最佳实践与全公网开放端口自动防护

Fail2ban 采用 `backend = systemd` + `nftables-allports` 内核全端口封禁方案，满足以下工程规范：
- **兼容 Debian 10–13 与无 `rsyslog` 轻量化环境**：安装 `python3-systemd` 直接读取 `journald` 日志，即使低内存优化卸载了 `rsyslog` 依然 100% 正常工作（若无 `python3-systemd` 则自动回退至 `/var/log/auth.log`）。
- **最佳实践三层防线**：
  1. `[sshd]`：启用 `mode = aggressive`，单次短窗口高敏检测（3 次失败封禁）；Fail2ban >= 0.11 自动启用指数递增封禁（`1h` 起步，倍率 `2`，上限 `7d` + 随机抖动 `5m`），Debian 10（0.10.x）自动回退为静态 `24h` 封禁避免语法报错。
  2. `[recidive]`：监控 `/var/log/fail2ban.log`，1 天内反复触发封禁 3 次的恶意 IP 直接全端口封禁 `7d`。
  3. `[debopty-public-ports]`：自动发现所有公网开放的 TCP/UDP 端口，在内核层通过独立 `table inet debopty_f2b_guard` 对任意单源 IP 的高频建连滥用打标记录（`[DEBOPTY-F2B-PORT-ABUSE]`），由 Fail2ban 监听内核日志自动触发全端口封禁。
- **未来端口变更全自动同步**：通过 `debopty-f2b-sync` 配合 `systemd.path`（监听 `/etc/nftables.conf`、`/etc/nftables.d`、`/etc/nftables.d/debopti`、`/etc/ssh/sshd_config`、`/etc/systemd/system/ssh.socket.d`）、`systemd.timer`（每 10 分钟兜底巡检）以及 `nftables.service` Drop-in Hook，确保任何新增、修改或移除端口后自动更新防护列表并防止 `flush ruleset` 清空 Fail2ban 封禁表。

#### 5.5.1 安装依赖

```bash
apt-get update
apt-get install -y fail2ban python3-systemd
```

#### 5.5.2 部署核心 Jail 与公网端口过滤器

将 `50022` 替换为你的实际 SSH 端口，写入 `/etc/fail2ban/jail.d/99-debopty.local`（模板参见 [templates/security/fail2ban_jail.local](../templates/security/fail2ban_jail.local)）：

```ini
[DEFAULT]
ignoreip = 127.0.0.1/8 ::1
backend = systemd
banaction = nftables-allports
banaction_allports = nftables-allports
bantime = 1h
findtime = 10m
maxretry = 5
bantime.increment = true
bantime.factor = 2
bantime.maxtime = 7d
bantime.rndtime = 5m

[sshd]
enabled = true
mode = aggressive
port = 50022
filter = sshd
maxretry = 3
findtime = 10m
bantime = 1h
backend = systemd

[recidive]
enabled = true
logpath = /var/log/fail2ban.log
backend = auto
banaction = nftables-allports
findtime = 1d
bantime = 7d
maxretry = 3
```

写入公网端口滥用日志过滤器 `/etc/fail2ban/filter.d/debopty-public-ports.conf`（模板参见 [templates/security/fail2ban_filter_public_ports.conf](../templates/security/fail2ban_filter_public_ports.conf)）：

```ini
[Definition]
failregex = ^.*\[DEBOPTY-F2B-PORT-ABUSE\].*\bSRC=<HOST>\b.*$
ignoreregex =
```

#### 5.5.3 部署公网开放端口自动同步与 Systemd 监听

安装独立同步脚本与运行时模板（不依赖脚本安装目录）：

```bash
install -D -m 0755 templates/security/debopty-f2b-sync.sh /usr/local/bin/debopty-f2b-sync
install -D -m 0644 templates/security/fail2ban_ports.local /usr/local/share/debopty/templates/security/fail2ban_ports.local
install -D -m 0644 templates/security/f2b_public_ports_guard.nft /usr/local/share/debopty/templates/security/f2b_public_ports_guard.nft
```

部署 Systemd 监听单元与 `nftables.service` 重载 Hook：
- `/etc/systemd/system/debopty-f2b-port-sync.service`（参见 [templates/security/debopty-f2b-port-sync.service](../templates/security/debopty-f2b-port-sync.service)）
- `/etc/systemd/system/debopty-f2b-port-sync.path`（参见 [templates/security/debopty-f2b-port-sync.path](../templates/security/debopty-f2b-port-sync.path)）
- `/etc/systemd/system/debopty-f2b-port-sync.timer`（参见 [templates/security/debopty-f2b-port-sync.timer](../templates/security/debopty-f2b-port-sync.timer)）
- `/etc/systemd/system/nftables.service.d/10-debopty-fail2ban.conf`（参见 [templates/security/nftables-fail2ban-override.conf](../templates/security/nftables-fail2ban-override.conf)）

启用服务并执行首次同步：

```bash
systemctl daemon-reload
systemctl enable --now fail2ban
systemctl enable --now debopty-f2b-port-sync.path debopty-f2b-port-sync.timer
/usr/local/bin/debopty-f2b-sync --force-reload
```

#### 5.5.4 验证与日常运维命令

```bash
# 查看已激活的 Fail2ban Jail 列表（应包含 sshd、recidive、debopty-public-ports）
fail2ban-client status

# 查看 SSH 与全公网端口防护状态
fail2ban-client status sshd
fail2ban-client status debopty-public-ports

# 查看自动生成的动态端口配置与内核限速表
cat /etc/fail2ban/jail.d/99-debopty-ports.local
nft list table inet debopty_f2b_guard

# 查看自动同步监听器与定时巡检状态
systemctl status debopty-f2b-port-sync.path debopty-f2b-port-sync.timer
```
