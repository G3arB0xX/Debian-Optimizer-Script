# Xray Core 部署与规则集管理

本文档介绍如何手动安装、配置和管理 Xray Core。

**前提条件**：root 权限，Debian 10+

---

## 1. 安装 Xray Core

Xray 官方提供了安装脚本，会自动获取最新版本并配置 Systemd 服务。

### 通用环境

```bash
bash -c "$(curl -fsSL https://raw.githubusercontent.com/XTLS/Xray-install/main/install-release.sh)" @ install
```

### 中国大陆环境（使用加速镜像）

```bash
# 先下载脚本
curl -fsSL https://ghfast.top/https://raw.githubusercontent.com/XTLS/Xray-install/main/install-release.sh -o /tmp/xray-install.sh
# 执行安装
bash /tmp/xray-install.sh install
```

安装完成后验证：

```bash
xray version
# 查看安装的文件
ls /usr/local/bin/xray
ls /usr/local/etc/xray/
ls /usr/local/share/xray/
```

---

## 2. 服务管理

本脚本安装的 Xray **默认不开启开机自启**，需要手动控制：

```bash
# 启动
systemctl start xray

# 停止
systemctl stop xray

# 查看状态
systemctl status xray

# 开启开机自启
systemctl enable xray

# 关闭开机自启
systemctl disable xray

# 重新加载配置（修改 config.json 后执行）
systemctl reload xray
# 或强制重启
systemctl restart xray
```

查看运行日志：

```bash
journalctl -u xray -f        # 实时跟踪日志
journalctl -u xray -n 100    # 查看最近 100 行
```

---

## 3. 配置文件

Xray 的主配置文件位于 `/usr/local/etc/xray/config.json`。

以下是一个最简单的入站配置示例（VLESS + Reality，供参考）：

```bash
# 用你喜欢的编辑器打开配置文件
nano /usr/local/etc/xray/config.json
```

修改配置后，重载生效：

```bash
systemctl reload xray
```

---

## 4. 规则集管理

Xray 使用 `geosite.dat` 和 `geoip.dat` 进行路由分流，规则集存放在 `/usr/local/share/xray/`。

### 4.1 官方默认规则集

官方规则集来自 v2fly 社区，随 Xray 安装脚本一起安装，内容较为基础。

手动更新官方规则集：

```bash
ASSET_DIR="/usr/local/share/xray"

# 更新 geosite.dat
curl -fsSL https://github.com/v2fly/domain-list-community/releases/latest/download/dlc.dat \
    -o "$ASSET_DIR/geosite.dat"

# 更新 geoip.dat
curl -fsSL https://github.com/v2fly/geoip/releases/latest/download/geoip.dat \
    -o "$ASSET_DIR/geoip.dat"

# 中国大陆用镜像加速
# curl -fsSL https://ghfast.top/https://github.com/v2fly/domain-list-community/releases/latest/download/dlc.dat \
#     -o "$ASSET_DIR/geosite.dat"
# curl -fsSL https://ghfast.top/https://github.com/v2fly/geoip/releases/latest/download/geoip.dat \
#     -o "$ASSET_DIR/geoip.dat"

systemctl reload xray
```

### 4.2 切换到 Loyalsoldier 增强规则集

[Loyalsoldier/v2ray-rules-dat](https://github.com/Loyalsoldier/v2ray-rules-dat) 是社区维护的增强版规则集，对中国大陆域名和 IP 的覆盖更完整，适合有分流需求的场景。

```bash
ASSET_DIR="/usr/local/share/xray"

# 备份官方规则集（避免覆盖后无法还原）
cp "$ASSET_DIR/geosite.dat" "$ASSET_DIR/geosite.dat.official" 2>/dev/null || true
cp "$ASSET_DIR/geoip.dat" "$ASSET_DIR/geoip.dat.official" 2>/dev/null || true

# 下载 Loyalsoldier 规则集
curl -fsSL https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geosite.dat \
    -o "$ASSET_DIR/geosite.dat.loyalsoldier"
curl -fsSL https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geoip.dat \
    -o "$ASSET_DIR/geoip.dat.loyalsoldier"

# 中国大陆用镜像加速
# curl -fsSL https://ghfast.top/https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geosite.dat \
#     -o "$ASSET_DIR/geosite.dat.loyalsoldier"
# curl -fsSL https://ghfast.top/https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geoip.dat \
#     -o "$ASSET_DIR/geoip.dat.loyalsoldier"

# 切换为 Loyalsoldier 规则集（软链接方式，方便切换）
ln -sf geosite.dat.loyalsoldier "$ASSET_DIR/geosite.dat"
ln -sf geoip.dat.loyalsoldier "$ASSET_DIR/geoip.dat"

systemctl reload xray
echo "已切换到 Loyalsoldier 规则集"
```

### 4.3 切换回官方规则集

```bash
ASSET_DIR="/usr/local/share/xray"

ln -sf geosite.dat.official "$ASSET_DIR/geosite.dat"
ln -sf geoip.dat.official "$ASSET_DIR/geoip.dat"

systemctl reload xray
echo "已切换回官方默认规则集"
```

### 4.4 自动更新规则集（定时任务）

设置 Cron 定时任务，每周一凌晨 3:30 自动更新 Loyalsoldier 规则集：

写入 `/usr/local/bin/xray-rule-update.sh`（完整内容参见 [templates/apps/xray/xray-rule-update.sh](../templates/apps/xray/xray-rule-update.sh)；手动部署可简化为下方版本，中国大陆请将 URL 替换为 ghfast 镜像）：

```bash
#!/bin/bash
ASSET_DIR="/usr/local/share/xray"
GEOSITE_URL="https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geosite.dat"
GEOIP_URL="https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geoip.dat"

# 中国大陆请将上面两行的 URL 替换为 ghfast 镜像：
# GEOSITE_URL="https://ghfast.top/https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geosite.dat"
# GEOIP_URL="https://ghfast.top/https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geoip.dat"

curl -fsSL "$GEOSITE_URL" -o "$ASSET_DIR/geosite.dat.loyalsoldier.new" && \
    mv -f "$ASSET_DIR/geosite.dat.loyalsoldier.new" "$ASSET_DIR/geosite.dat.loyalsoldier"

curl -fsSL "$GEOIP_URL" -o "$ASSET_DIR/geoip.dat.loyalsoldier.new" && \
    mv -f "$ASSET_DIR/geoip.dat.loyalsoldier.new" "$ASSET_DIR/geoip.dat.loyalsoldier"

systemctl is-active --quiet xray && systemctl reload xray
```

```bash
chmod +x /usr/local/bin/xray-rule-update.sh
(crontab -l 2>/dev/null; echo "30 3 * * 1 /usr/local/bin/xray-rule-update.sh > /dev/null 2>&1") | crontab -
crontab -l
```

删除定时任务：

```bash
crontab -l | grep -v xray-rule-update | crontab -
```

---

## 5. Systemd 安全加固（可选）

为 Xray 服务添加沙盒限制，减少服务被攻击后对系统的影响：

```bash
mkdir -p /etc/systemd/system/xray.service.d/
```

写入 `/etc/systemd/system/xray.service.d/security.conf`（完整内容参见 [templates/apps/xray/xray.service.override.conf](../templates/apps/xray/xray.service.override.conf)）：

```ini
[Service]
ProtectSystem=full
ProtectHome=true
PrivateTmp=true
NoNewPrivileges=true
# 限制 Capabilities，即便以 root 运行也只能执行必要操作
CapabilityBoundingSet=CAP_NET_ADMIN CAP_NET_BIND_SERVICE CAP_NET_RAW
```

```bash
systemctl daemon-reload
systemctl restart xray
```

---

## 6. 卸载 Xray

```bash
# 使用官方脚本卸载
bash /tmp/xray-install.sh remove

# 如果脚本不在了，重新下载
curl -fsSL https://raw.githubusercontent.com/XTLS/Xray-install/main/install-release.sh -o /tmp/xray-install.sh
bash /tmp/xray-install.sh remove

# 清理剩余文件
rm -rf /usr/local/etc/xray /usr/local/share/xray
rm -rf /etc/systemd/system/xray.service.d
rm -f /usr/local/bin/xray-rule-update.sh

# 删除定时任务
crontab -l | grep -v xray-rule-update | crontab -

systemctl daemon-reload
```
