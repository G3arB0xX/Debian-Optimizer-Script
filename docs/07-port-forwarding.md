# Realm 端口转发服务部署

本文档介绍如何手动安装与配置 Realm 端口转发服务。

**前提条件**：root 权限，Debian 10+

---

## 1. 下载与安装

Realm 是一个使用 Rust 编写的高性能、轻量级端口转发工具。我们将其安装至 `/opt/realm` 并使用非 root 用户安全运行。

### 1.1 确定系统架构并下载

```bash
# 检测系统架构
ARCH=$(uname -m)
if [ "$ARCH" = "x86_64" ]; then
    REALM_ARCH="x86_64-unknown-linux-gnu"
elif [ "$ARCH" = "aarch64" ]; then
    REALM_ARCH="aarch64-unknown-linux-gnu"
else
    echo "不支持的架构: $ARCH"
    exit 1
fi

# 获取最新版本号
LATEST_VERSION=$(curl -fsSL "https://api.github.com/repos/zhboner/realm/releases/latest" \
    | grep '"tag_name"' | head -1 | cut -d'"' -f4)
if [ -z "$LATEST_VERSION" ]; then
    LATEST_VERSION="v2.7.0" # 稳定版兜底
fi
echo "目标版本: $LATEST_VERSION"

# 下载压缩包
DOWNLOAD_URL="https://github.com/zhboner/realm/releases/download/${LATEST_VERSION}/realm-${REALM_ARCH}.tar.gz"
# 中国大陆环境可使用加速镜像：
# DOWNLOAD_URL="https://ghfast.top/https://github.com/zhboner/realm/releases/download/${LATEST_VERSION}/realm-${REALM_ARCH}.tar.gz"

curl -fsSL "$DOWNLOAD_URL" -o /tmp/realm.tar.gz

# 创建安装目录并解压
mkdir -p /opt/realm
tar -xzf /tmp/realm.tar.gz -C /opt/realm
chmod +x /opt/realm/realm
rm -f /tmp/realm.tar.gz
```

---

## 2. 安全用户与规则配置文件

为保障系统安全，严禁以 root 权限运行网络转发服务。我们创建一个专用的系统用户：

```bash
# 创建无登录权限的系统用户
if ! id -u realm >/dev/null 2>&1; then
    useradd -r -s /usr/sbin/nologin realm
fi

# 创建配置文件目录
mkdir -p /etc/realm
```

### 2.1 规则配置文件

写入 `/etc/realm/config.toml`（完整内容参见 `templates/apps/realm/config.toml`；手动部署可按下方案例扩展）：

```toml
# Realm 配置文件 (TOML 格式)
# 完整文档: https://github.com/zhboner/realm

[network]
no_delay = true    # 开启 TCP_NODELAY，降低小包转发延迟
use_udp = true     # 同时转发 UDP 流量

# 每个 [[endpoints]] 块代表一条独立转发规则
[[endpoints]]
listen = "0.0.0.0:5000"     # 本机监听端口
remote = "1.2.3.4:5000"     # 目标转发地址（替换为实际 IP）

[[endpoints]]
listen = "0.0.0.0:6000"
remote = "1.2.3.4:6000"
```

```bash
chown -R realm:realm /etc/realm /opt/realm
```

---

## 3. 配置 Systemd 服务与安全沙盒

写入 `/etc/systemd/system/realm.service`（完整内容参见 `templates/apps/realm/realm.service`）：

```ini
[Unit]
Description=Realm Relay Service
After=network.target

[Service]
Type=simple
User=realm
Group=realm
WorkingDirectory=/etc/realm
ExecStart=/opt/realm/realm -c /etc/realm/config.toml
Restart=always
RestartSec=5
LimitNOFILE=1048576
# 非 root 绑定低端口
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE

# 安全沙盒限制
ProtectSystem=full
ProtectHome=true
PrivateTmp=true
NoNewPrivileges=true


[Install]
WantedBy=multi-user.target
```

```bash
systemctl daemon-reload
systemctl enable --now realm
```

验证服务状态：

```bash
systemctl status realm
```

---

## 4. 防火墙规则放行

别忘了在 nftables 防火墙中放行监听的端口。参考 [02-security-hardening.md](02-security-hardening.md#53-模块化规则管理推荐) 动态添加模块化规则：

写入 `/etc/nftables.d/debopti/Realm_Ports.nft`（规则结构参见 `templates/security/rule_template.nft`）：

```nft
table inet filter {
    chain input {
        # 放行 Realm 监听的 TCP/UDP 端口
        tcp dport { 5000, 6000 } accept comment "Realm_Ports"
        udp dport { 5000, 6000 } accept comment "Realm_Ports"
    }
}
```

```bash
systemctl reload nftables
```

---

## 5. 卸载服务

```bash
# 停止并禁用服务
systemctl stop realm
systemctl disable realm

# 清理 Systemd 单元文件与配置文件
rm -f /etc/systemd/system/realm.service
systemctl daemon-reload

# 清理程序与配置目录
rm -rf /opt/realm
rm -rf /etc/realm

# 可选：删除专用用户
id -u realm >/dev/null 2>&1 && userdel realm
```
