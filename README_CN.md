# Debian Optimizer Script

[English](README.md) | 简体中文

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)
[![Platform: Debian](https://img.shields.io/badge/Platform-Debian%2010%2B-orange.svg)](https://www.debian.org/)

一个用于 Debian 系统的 TUI 管理脚本，涵盖内核调优、nftables 防火墙、SSH 安全加固与常用服务的自动化部署。支持 Debian 10+，开箱即用。

---

## 快速开始

以 root 或具有 `sudo` 权限的用户运行。脚本会自动检测 `curl` / `wget` 可用性与所在地区。

**通用网络**
```bash
# curl
bash -c "$(curl -fsSL https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
# wget
bash -c "$(wget -qO- https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
```

**中国大陆（加速镜像）**
```bash
# curl
bash -c "$(curl -fsSL https://ghfast.top/https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
# wget
bash -c "$(wget -qO- https://ghfast.top/https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
```

> **无 sudo / 非 root 用户**：请先切换到 root 再执行：
> ```bash
> su - -c "bash <(curl -fsSL https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
> # 或使用 wget：
> su - -c "bash <(wget -qO- https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
> ```

安装完成后，在终端任意路径输入 `debopti` 即可唤起管理面板。

---

## 项目结构

```text
/opt/debopti/
├── deb_optimizer.sh    # 主入口
├── scripts/
│   ├── common.sh       # 公共工具函数与状态管理
│   ├── network.sh      # 网络与 nftables 防火墙
│   ├── system.sh       # 内核调优与系统精简
│   ├── security.sh     # SSH 与账号加固
│   ├── tui.sh          # TUI 渲染引擎
│   └── apps/           # 各应用模块
└── /etc/debopti/       # 持久化配置与状态存储
```

---

## 功能模块

### 系统基础优化

*   **内核网络调优**：启用 BBR，调整网络协议栈参数，通过独立 `.d` 配置文件写入，不修改主配置。
*   **内存与日志**：交互式配置 ZRAM 或 Swap；重写 Logrotate 规则，按天压缩轮转系统日志。
*   **系统精简**：削减多余 TTY，移除 rsyslog，限制 Journald 日志上限，可交互式停用 ModemManager、Avahi 等闲置服务。
*   **IP 路由转发**：独立切换 IPv4 / IPv6 路由转发状态，建站模式与代理/组网模式按需切换。

### 安全加固

*   **SSH 加固**：强制 Ed25519 公钥登录，本地生成密钥对并自动注入；端口随机重映射至 40000+，适配 Systemd Socket 与传统 sshd；内置连通性预检，失败时自动回滚。
*   **nftables 防火墙**：基于 `/etc/nftables.d` 模块化架构管理规则，原子化隔离，幂等更新。

> ⚠️ **注意**：本脚本默认使用 nftables 管理防火墙，**不建议同时使用 ufw、firewalld 等工具**，以免规则冲突导致端口状态异常或网络中断。

### 端口转发与 Web 服务

*   **Realm**：Rust 实现的轻量级多协议端口转发工具，以非 root 用户运行 Systemd 服务。
*   **Ferron**：Rust Web 服务器，支持自动 TLS 和 KDL 语法配置文件。
*   **定制 Caddy**：通过 `xcaddy` 本地编译，集成 L4 代理、Cloudflare DNS 插件与 naiveproxy 模块，以独立系统用户运行。

### 代理与组网

*   **Xray Core**：通过官方脚本部署，内置节点管理菜单，支持一键切换 Loyalsoldier 增强规则集，Cron 自动更新任务可独立开关并持久化状态。
*   **WARP & Usque**：部署 Cloudflare WARP 并配置 Socks5 模式，Usque 客户端自动注册。内置 Xray WireGuard 出站配置生成器，支持节点扫描、MTU 探测与混淆参数计算。
*   **Tailscale & EasyTier**：组网客户端一键部署，自动在 nftables 中放行 P2P 端口。
*   **Tailscale DERP 节点**：从指定版本源码本地编译，注入防拨测补丁，自动申请双栈 TLS 证书，输出适配控制台的 ACL 配置 JSON。
*   **Docker**：通过官方源或阿里云镜像安装 Docker Engine 与 Compose，注入生产级 `daemon.json`。

### 运维工具

*   **Fish Shell**：安装 Fish 与 `fisher`，预装 `fzf.fish`、`tide`，支持一键切换默认 Shell。
*   **Micro 编辑器**：预配置鼠标支持、语法高亮与自动缩进。
*   **Yazi 文件管理器**：基于 Rust 开发的极速终端文件管理器，预配置 Windows/Micro 友好快捷键，支持 shell 退出路径同步。
*   **Acme.sh**：自动化证书管理，默认切换至 Let's Encrypt CA。

### IP 养护

基于 [IP-Sentinel](https://github.com/hotyue/IP-Sentinel) 项目资源实现的 IP 信誉养护工具，在此致谢原作者。

*   单/双栈并发运行，全球节点轮换，热重载配置。
*   使用 `curl-impersonate` 模拟真实 TLS 指纹。
*   通过 Systemd 模板化定时器调度，按需启停。

### 架构特性

*   **网络自适应**：启动时检测服务器所在地区，境内自动切换 APT 镜像与 GitHub 下载加速节点。
*   **DNS 自愈**：使用 `getent` 检测 DNS 可用性，解析故障时自动写入临时公共 DNS 并备份原配置。
*   **幂等设计**：所有配置操作可重复执行，不产生配置堆叠或文件冲突。
*   **CI 支持**：设置 `CI=true` 后，脚本全程跳过交互确认，适合无人值守自动化部署。
*   **原子化卸载**：一键清除软链接、Shell 环境注入与安装目录，无残留。

---

## 持久化配置

运行状态记录在 `/etc/debopti/debopti.conf`，可手动修改：

```bash
IS_CN_REGION="true"    # 是否为中国大陆地区
BASE_OPTIMIZED="true"  # 基础优化是否已完成
EDITOR_CMD="micro"     # 默认文本编辑器
```

---

## 注意事项

1. 仅支持 **Debian 10 及以上**版本，包括 Debian 12 / 13。
2. 修改内核参数与 SSH 端口时，请按脚本提示完成连通性验证，脚本会在失败时自动回滚。
3. 默认防火墙为 nftables，**不建议同时使用 ufw 或 firewalld**。

---

## 贡献与支持

*   发现 Bug 或有需求，请提交 [Issue](https://github.com/G3arB0xX/Debian-Optimizer-Script/issues)。
*   贡献代码请遵循 `scripts/` 下的模块化规范，发起 Pull Request。
