# Easytier 虚拟组网部署

本文档介绍如何手动安装、配置和管理 Easytier 跨平台组网客户端，与 `debopti` 自动化行为对齐。

**前提条件**：root 权限，Debian 10+，`unzip` 与 `curl` 可用

---

## 1. 安装 Easytier

官方提供安装脚本，默认安装至 `/opt/easytier`：

```bash
curl -fsSL https://raw.githubusercontent.com/EasyTier/EasyTier/main/script/install.sh -o /tmp/easytier-install.sh
bash /tmp/easytier-install.sh install
```

中国大陆环境可使用 GitHub 加速：

```bash
bash /tmp/easytier-install.sh install --gh-proxy https://ghfast.top/
```

安装后验证：

```bash
ls /opt/easytier/easytier-core /opt/easytier/easytier-cli
```

---

## 2. 更新 Easytier

`debopti` 以 `/opt/easytier/easytier-core` 是否存在判定已安装，并调用官方 **`update`**（非 `install`）：

```bash
bash /tmp/easytier-install.sh update
```

中国大陆环境：

```bash
bash /tmp/easytier-install.sh update --gh-proxy https://ghfast.top/
```

更新行为说明：

- 官方 `update` 仅对**更新前正在运行**的 `easytier@*` 实例短暂 stop 后重启，**不修改** `enabled` / `disabled` 状态。
- 本脚本在更新时**不** `enable` 或 `disable` 任何 `easytier@` 服务；仅幂等刷新 PATH、Systemd 安全 override 与防火墙规则。

---

## 3. 关闭开机自启

官方安装脚本会默认 `enable` 并启动 `easytier@default`。本脚本在**首次安装**后会关闭自启：

```bash
systemctl stop easytier easytier@default
systemctl stop "easytier@*"
systemctl disable easytier
systemctl disable easytier@default
systemctl disable "easytier@*"
```

按需手动启动：

```bash
systemctl start easytier@default
systemctl status easytier@default
```

开启开机自启：

```bash
systemctl enable easytier@default
```

多配置文件时，实例名为 `easytier@<配置名>`（配置文件位于 `/opt/easytier/config/<配置名>.conf`）。

---

## 4. CLI 加入 PATH

本脚本将 `/opt/easytier` 写入全局 PATH，使 `easytier-cli` 与 `easytier-core` 在 Bash 与 Fish 中可直接调用。

Bash（`/etc/profile.d/debopti-easytier.sh`，模板见 [templates/apps/easytier/profile.d/debopti-easytier.sh](../templates/apps/easytier/profile.d/debopti-easytier.sh)）：

```bash
if [[ ":${PATH}:" != *":/opt/easytier:"* ]]; then
    export PATH="/opt/easytier:${PATH}"
fi
```

Fish（SOT 用户 `~/.config/fish/conf.d/debopti_path.fish`）：

```fish
fish_add_path /opt/easytier
```

新登录 shell 后验证：

```bash
command -v easytier-cli easytier-core
```

---

## 5. 防火墙

P2P 打洞默认使用 TCP/UDP **11010–11015**，需在防火墙放行。脚本通过 nftables 模块 `Easytier_P2P` 自动添加；手动环境请自行放行对应端口。

---

## 6. 卸载与还原

```bash
bash /tmp/easytier-install.sh uninstall
rm -f /etc/profile.d/debopti-easytier.sh
# Fish：从 debopti_path.fish 移除 /opt/easytier 行
rm -rf /opt/easytier /etc/systemd/system/easytier*
rm -f /usr/sbin/easytier-core /usr/sbin/easytier-cli
systemctl daemon-reload
```
