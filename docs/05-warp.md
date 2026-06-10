# Cloudflare WARP 与 Usque (MASQUE) 部署

本文档介绍如何手动安装与配置 Cloudflare WARP 客户端以及 Usque (MASQUE 协议) 客户端。

**前提条件**：root 权限，Debian 10+

---

## 1. Cloudflare WARP 客户端配置

WARP 提供基于 WireGuard 的加密隧道，可将系统网络或指定流量通过 Cloudflare 网络转发。以下步骤将其配置为本地 Socks5 代理（默认监听 `127.0.0.1:40000`）。

### 1.1 添加 Cloudflare APT 源

```bash
apt-get install -y lsb-release gnupg curl

# 下载并导入 GPG 公钥
curl -fsSL https://pkg.cloudflareclient.com/pubkey.gpg | \
    gpg --dearmor -o /usr/share/keyrings/cloudflare-warp-archive-keyring.gpg

# 添加软件源
echo "deb [signed-by=/usr/share/keyrings/cloudflare-warp-archive-keyring.gpg] \
    https://pkg.cloudflareclient.com/ $(lsb_release -cs) main" | \
    tee /etc/apt/sources.list.d/cloudflare-client.list > /dev/null
```

### 1.2 安装与启动

```bash
apt-get update -y
apt-get install -y cloudflare-warp

# 启动守护进程
systemctl enable --now warp-svc

# 等待守护进程初始化（约 5 秒）
sleep 5

# 注册新设备（首次安装时执行）
warp-cli --accept-tos registration new
```

> **注意**：在国内服务器上注册可能会因为网络原因超时失败，建议通过可用代理环境或在海外服务器执行。

### 1.3 配置为 Socks5 代理模式

```bash
# 强制使用更轻量的 WireGuard 协议
warp-cli --accept-tos tunnel protocol set WireGuard

# 关闭内置的 DNS 遥测日志，保护隐私并降低磁盘 IO
warp-cli --accept-tos dns families off

# 设置为本地 Socks5 代理模式，指定端口为 40000
warp-cli --accept-tos mode proxy
warp-cli --accept-tos proxy port 40000

# 建立连接
warp-cli --accept-tos connect
```

验证连接：

```bash
# 查看连接状态
warp-cli --accept-tos status

# 测试代理可用性并查看出口 IP
curl -x socks5://127.0.0.1:40000 https://cloudflare.com/cdn-cgi/trace 2>/dev/null | grep ip
```

### 1.4 设置内存限制与安全沙盒

WARP 客户端进程有时会占用较多内存，推荐设置 Systemd 限制限制其开销：

```bash
mkdir -p /etc/systemd/system/warp-svc.service.d/
```

写入 `/etc/systemd/system/warp-svc.service.d/security.conf`：

```ini
[Service]
# 内存软上限，超过后内核积极回收但不会强制杀死进程
MemoryHigh=80M
# 内存硬上限，超过后进程将被 OOM Killer 终止
MemoryMax=120M
# 只记录 error 级别及以上的日志
LogLevelMax=error

# 安全沙盒
ProtectSystem=full
ProtectHome=true
PrivateTmp=true
NoNewPrivileges=true
```

```bash
systemctl daemon-reload
systemctl restart warp-svc
```

### 1.5 卸载 WARP

```bash
# 停止连接并注销客户端
warp-cli --accept-tos disconnect
warp-cli --accept-tos registration delete

apt-get purge -y cloudflare-warp
rm -f /etc/apt/sources.list.d/cloudflare-client.list
rm -rf /etc/systemd/system/warp-svc.service.d
systemctl daemon-reload
```

---

## 2. Usque (MASQUE 协议) 客户端配置

Usque 是一个基于 MASQUE 协议的高速代理客户端，可在本地建立 Socks5 监听服务（默认监听 `127.0.0.1:40001`）。

### 2.1 下载与解压

```bash
apt-get install -y jq unzip

# 自动获取最新版本及当前系统架构对应的二进制文件包
ARCH=$(uname -m)
[ "$ARCH" = "x86_64" ] && ARCH="amd64" || ARCH="arm64"

LATEST_USQUE=$(curl -fsSL "https://api.github.com/repos/Diniboy1123/usque/releases/latest" \
    | jq -r ".assets[] | select(.name | contains(\"linux_${ARCH}.zip\")) | .browser_download_url")

# 大陆环境加速下载：
# LATEST_USQUE="https://ghfast.top/${LATEST_USQUE}"

curl -fsSL "$LATEST_USQUE" -o /tmp/usque.zip

mkdir -p /opt/usque
unzip -qo /tmp/usque.zip -d /opt/usque/
chmod +x /opt/usque/usque
rm -f /tmp/usque.zip
```

### 2.2 创建非特权系统用户

为了安全起见，创建专用的非登录账户来运行该程序：

```bash
if ! id -u usque >/dev/null 2>&1; then
    useradd -r -s /usr/sbin/nologin usque
fi
chown -R usque:usque /opt/usque
```

### 2.3 部署 Systemd 服务与安全限制

写入 `/etc/systemd/system/usque.service`：

```ini
[Unit]
Description=Usque MASQUE Socks5 Service
After=network.target

[Service]
Type=simple
User=usque
Group=usque
WorkingDirectory=/opt/usque
# socks: Socks5 代理模式; -b: 仅监听本地; -p: 监听端口
ExecStart=/opt/usque/usque socks -b 127.0.0.1 -p 40001
Restart=on-failure
RestartSec=5

# 安全沙盒
ProtectSystem=full
ProtectHome=true
PrivateTmp=true
NoNewPrivileges=true

[Install]
WantedBy=multi-user.target
```

> `ExecStart` 中的 `-p 40001` 可按需修改端口；连接远端服务端时需追加 `--server-url` 等参数。

```bash
systemctl daemon-reload
systemctl enable --now usque
```

验证服务是否正常启动：

```bash
systemctl status usque
```

### 2.4 卸载 Usque

```bash
systemctl stop usque
systemctl disable usque
rm -f /etc/systemd/system/usque.service
rm -rf /opt/usque
id -u usque >/dev/null 2>&1 && userdel usque
systemctl daemon-reload
```

---

## 3. 手动生成 Xray-WireGuard (WARP) 出站配置

如果不想在 VPS 运行多余的 `warp-svc` 守护进程，可以直接将 Cloudflare WARP 账户的 WireGuard 密钥及参数提取出来，直接作为 Xray Core 的 `wireguard` 协议出站代理。为了让 Xray 伪装成官方 WARP 客户端，**必须计算得到 `reserved` 字段**。

### 3.1 提取 WireGuard 参数与设备 ID

借助 `wgcf` 工具，手动在 Cloudflare 网络注册新账户并提取其配置文件：

```bash
# 1. 下载并安装 wgcf
ARCH=$(uname -m)
[[ "$ARCH" == "x86_64" ]] && ARCH="amd64" || ARCH="arm64"
curl -fsSL -o /usr/local/bin/wgcf "https://github.com/ViRb3/wgcf/releases/download/v2.2.22/wgcf_2.2.22_linux_${ARCH}"
chmod +x /usr/local/bin/wgcf

# 2. 注册新账户与生成 Profile 配置文件
# (在空目录中执行)
mkdir -p /tmp/warp_gen && cd /tmp/warp_gen
wgcf register --accept-tos
wgcf generate
```

生成的两个文件中包含了核心字段：
- 在 `wgcf-account.toml` 中，查找 `private_key` (私钥) 和 `device_id` (设备识别 ID)。
- 在 `wgcf-profile.conf` 中，查找 `Address` (IPv4 客户端内网地址)。

### 3.2 黑科技：计算 Reserved 字节掩码

Cloudflare WARP 网关会通过客户端连接中的 3 字节 `reserved` 掩码来识别其是否为官方客户端，非官方值会被限制速度或直接断连。计算此掩码的算法是将 `device_id`（即 client_id）通过 Base64 解码，然后获取解码后字节流的前 3 个字节。

我们可以在终端通过 Bash 转换公式手动算出来：

```bash
# 从注册文件提取设备识别码并运行解码计算
device_id=$(grep "device_id" wgcf-account.toml | awk -F"'" '{print $2}')
hex_id=$(echo -n "$device_id" | base64 -d | od -An -v -tx1 | tr -d ' \n')

# 转换获取十进制前三字节
r1=$((16#${hex_id:0:2}))
r2=$((16#${hex_id:2:2}))
r3=$((16#${hex_id:4:2}))

echo "计算出的 reserved 字段值为: [$r1, $r2, $r3]"
```

### 3.3 组装 Xray Outbound 规则 (WireGuard)

将提取的 `private_key`、`address` 以及算好的 `[r1, r2, r3]` 替换到下方 JSON 中。您只需将其贴入 Xray 的 `outbounds` 规则列表下：

```json
{
  "tag": "warp",
  "protocol": "wireguard",
  "settings": {
    "secretKey": "你的_private_key_内容",
    "address": ["你的_address_IPv4_如_172.16.0.2/32"],
    "peers": [{
        "publicKey": "bmXOC+F1FxEMF9dyiK2H5/1SUtzH0JuVo51h2wPfgyo=", // 官方 WARP 公钥，固定值
        "endpoint": "162.159.192.1:2408"                        // 官方网关地址，固定值
    }],
    "reserved": [r1的值, r2的值, r3的值]
  }
}
```

### 3.4 清理工作空间

```bash
rm -f /usr/local/bin/wgcf
cd /tmp && rm -rf /tmp/warp_gen
```
