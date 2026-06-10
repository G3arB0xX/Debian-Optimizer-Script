# Web 服务部署（自编译 Caddy 与 Ferron）

本文档介绍如何手动部署 Go 编译环境、自编译带有高级插件的 Caddy Web 服务器，以及部署官方标准版 Ferron Web 服务器。

**前提条件**：root 权限，Debian 10+

---

## 1. Go 语言编译环境部署

编译 Caddy 及其扩展插件需要 Go 语言开发环境。

### 1.1 下载并安装 Go

```bash
# 获取最新版本号
GO_DOMAIN="go.dev"
# 中国大陆可使用：GO_DOMAIN="golang.google.cn"
LATEST_GO=$(curl -s "https://${GO_DOMAIN}/VERSION?m=text" | head -n 1)
if [ -z "$LATEST_GO" ]; then
    LATEST_GO="go1.22.1"
fi
echo "Go 目标版本: $LATEST_GO"

# 下载 Go 归档包
DOWNLOAD_URL="https://${GO_DOMAIN}/dl/${LATEST_GO}.linux-amd64.tar.gz"
curl -fsSL "$DOWNLOAD_URL" -o /tmp/go.tar.gz

# 清理并安装至 /usr/local/go
rm -rf /usr/local/go
tar -C /usr/local -xzf /tmp/go.tar.gz
rm -f /tmp/go.tar.gz
```

写入 `/etc/profile.d/golang.sh`：

```bash
# Go 编译环境 PATH
export PATH=$PATH:/usr/local/go/bin
```

```bash
chmod +x /etc/profile.d/golang.sh
source /etc/profile.d/golang.sh

# 验证安装
go version
```

---

## 2. 自编译 Caddy (集成 layer4 / naiveproxy / cloudflare 插件)

通过 `xcaddy` 在本地编译 Caddy，集成以下生产用核心插件：
- `caddy-l4`：支持第四层 (TCP/UDP) 转发与协议分流。
- `cloudflare`：支持 Cloudflare DNS API 验证（用于 ACME 自动申请通配符证书）。
- `forwardproxy (naive)`：支持 NaiveProxy 代理协议。

### 2.1 编译 Caddy

推荐使用非 root 用户身份进行编译，以避免权限滥用风险。

```bash
# 1. 安装 xcaddy 编译工具
go install github.com/caddyserver/xcaddy/cmd/xcaddy@latest

# 2. 将编译工具复制到全局可执行目录
cp ~/go/bin/xcaddy /usr/local/bin/

# 3. 创建临时编译目录并执行 xcaddy 构建
BUILD_DIR="/tmp/caddy_build"
mkdir -p "$BUILD_DIR" && cd "$BUILD_DIR"

# 国内环境可注入 Go 代理以加速编译依赖拉取：
# export GOPROXY=https://goproxy.cn,direct

xcaddy build \
    --with github.com/mholt/caddy-l4 \
    --with github.com/caddy-dns/cloudflare \
    --with github.com/caddyserver/forwardproxy@caddy2=github.com/klzgrad/forwardproxy@naive

# 4. 将编译好的二进制移动至系统目录
mv ./caddy /usr/bin/caddy
chmod +x /usr/bin/caddy
cd /tmp && rm -rf "$BUILD_DIR"
```

### 2.2 配置特权权限（Cap_Net_Bind_Service）

为了安全起见，我们不应以 root 用户运行 Web 服务。使用 Linux Capabilities 机制，使得 Caddy 在非特权用户下依然能够绑定 `80` 和 `443` 端口。

```bash
setcap cap_net_bind_service=+ep /usr/bin/caddy
```

### 2.3 创建运行用户与目录结构

```bash
# 创建无登录权限的系统用户与同名组
if ! id -u caddy >/dev/null 2>&1; then
    groupadd --system caddy
    useradd -r -g caddy -s /usr/sbin/nologin caddy
fi

# 创建配置与数据存储目录
mkdir -p /etc/caddy /etc/ssl/caddy /usr/share/caddy /var/lib/caddy
chown -R caddy:caddy /etc/caddy /etc/ssl/caddy /usr/share/caddy /var/lib/caddy

# 写入默认演示配置
echo "<h1>Caddy Standard Landing Page</h1>" > /usr/share/caddy/index.html
```

写入 `/etc/caddy/Caddyfile`：

```
# 默认 HTTP 演示站点
:80 {
    root * /usr/share/caddy
    file_server
}
```

### 2.4 配置 Systemd 服务与安全沙盒

写入 `/etc/systemd/system/caddy.service`：

```ini
[Unit]
Description=Caddy Web Server
After=network.target network-online.target
Requires=network-online.target

[Service]
# Type=notify: Caddy 支持 sd_notify，可准确判断服务就绪
Type=notify
User=caddy
Group=caddy
ExecStart=/usr/bin/caddy run --environ --config /etc/caddy/Caddyfile
ExecReload=/usr/bin/caddy reload --config /etc/caddy/Caddyfile --force
Restart=on-failure
LimitNOFILE=1048576
# 非 root 绑定 80/443 端口
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
# 安全沙盒
ProtectSystem=full
ProtectHome=true
PrivateTmp=true
NoNewPrivileges=true
ReadWritePaths=/var/lib/caddy /etc/ssl/caddy

[Install]
WantedBy=multi-user.target
```

```bash
systemctl daemon-reload
systemctl enable --now caddy
```

### 2.5 放行防火墙规则

参考 [02-security-hardening.md](02-security-hardening.md) 的 nftables 配置，放行 80 与 443 端口：

写入 `/etc/nftables.d/debopti/Caddy_Web.nft`（规则结构参见 `templates/security/rule_template.nft`）：

```nft
table inet filter {
    chain input {
        # 放行 Caddy HTTP/HTTPS（含 QUIC/HTTP3 的 UDP 443）
        tcp dport { 80, 443 } accept comment "Caddy_Web"
        udp dport { 80, 443 } accept comment "Caddy_Web"
    }
}
```

```bash
systemctl reload nftables
```

---

## 3. Ferron Web 服务器部署

Ferron 是一款使用 Rust 编写的高性能 Web 服务器，原生支持自动 TLS 证书和现代化的 KDL 配置语法。

### 3.1 添加官方 APT 仓库

```bash
apt-get install -y curl gnupg2 ca-certificates lsb-release debian-archive-keyring

# 下载 Ferron 官方 PGP 签名密钥并转换为 gpg 格式
KEYRING="/usr/share/keyrings/ferron-keyring.gpg"
curl -fsSL "https://deb.ferron.sh/signing.pgp" | gpg --dearmor -o "$KEYRING" --yes

# 注入官方软件源
codename=$(lsb_release -cs)
echo "deb [signed-by=$KEYRING] https://deb.ferron.sh $codename main" | \
    tee /etc/apt/sources.list.d/ferron.list > /dev/null

# 更新源并安装
apt-get update -y
apt-get install -y ferron
```

### 3.2 标准化配置文件与初始化

Ferron 默认会读取 `/etc/ferron.kdl`，为保证项目目录整洁，我们将其标准化迁移至 `/etc/ferron/config.kdl`。

```bash
mkdir -p /etc/ferron /var/www/ferron
```

写入 `/etc/ferron/config.kdl`（完整内容参见 `templates/apps/ferron/config.kdl`）：

```kdl
# 全局配置块（* 表示对所有 server 块生效）
* {
    # io_uring 在部分低版本内核或虚拟化环境中可能不可用
    io_uring #false
    log_stdout
    error_log_stderr
}

server {
    address "0.0.0.0:80"
    root "/var/www/ferron"
}
```

```bash
echo "<h1>Ferron Default Web Page</h1>" > /var/www/ferron/index.html
chown -R ferron:ferron /var/www/ferron /etc/ferron
```

### 3.3 部署 Systemd Override 加固与路径纠偏

由于我们将默认配置文件移动到了 `/etc/ferron/config.kdl`，需要通过 Systemd Override 来纠偏启动命令参数，并增加沙盒限制：

```bash
mkdir -p /etc/systemd/system/ferron.service.d/
```

将 `/usr/bin/ferron` 替换为实际二进制路径，写入 `/etc/systemd/system/ferron.service.d/override.conf`（完整内容参见 `templates/apps/ferron/ferron.service.override.conf`）：

> Systemd Override 机制：第一行 `ExecStart=`（空值）清除父单元默认启动命令，第二行指定新路径。若不写第一行，新旧命令会并存而报错。

```ini
[Service]
# 清除父单元默认 ExecStart，再指定新配置路径
ExecStart=
ExecStart=/usr/bin/ferron -c /etc/ferron/config.kdl
# 安全沙盒
ProtectSystem=full
ProtectHome=true
PrivateTmp=true
NoNewPrivileges=true
```

```bash
systemctl daemon-reload
systemctl restart ferron
```

---

## 4. 卸载服务

### 4.1 卸载 Caddy

```bash
systemctl stop caddy
systemctl disable caddy
rm -f /etc/systemd/system/caddy.service /usr/bin/caddy
rm -rf /etc/caddy /usr/share/caddy /etc/ssl/caddy /var/lib/caddy
rm -f /etc/nftables.d/debopti/Caddy_Web.nft
systemctl reload nftables
id -u caddy >/dev/null 2>&1 && userdel caddy
```

### 4.2 卸载 Ferron

```bash
systemctl stop ferron
systemctl disable ferron
apt-get purge -y ferron
rm -f /etc/apt/sources.list.d/ferron.list /usr/share/keyrings/ferron-keyring.gpg
rm -rf /etc/ferron /var/www/ferron /etc/systemd/system/ferron.service.d
systemctl daemon-reload
```
