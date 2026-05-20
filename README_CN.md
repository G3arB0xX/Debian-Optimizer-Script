# 🚀 Debian Optimizer Script

[English](README.md) | 简体中文

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)
[![Platform: Debian](https://img.shields.io/badge/Platform-Debian%2010%2B-orange.svg)](https://www.debian.org/)

**Debian Optimizer Script** 是一个专为 Debian 系统打造的、高可用、模块化且经过安全加固的交互式 TUI 管理面板。它将内核参数调优、现代模块化防火墙、DevOps 工具链与安全的应用服务部署无缝结合，为您构建企业级的系统运维环境。

---

## 🌟 核心特性

### ⚡ 基础系统性能优化 (Base Performance Optimization)
*   **内核与网络调优**：自动启用 BBR 拥塞控制算法，通过独立 `.d` 配置文件持久化优化网络协议栈参数，提升高并发连接吞吐率。
*   **内存与日志控制**：支持动态配置 ZRAM 虚拟内存（zstd 高比压缩）或经典 Swap 交换文件；重写 Logrotate 规则，对系统日志实施日切和压缩轮转，杜绝磁盘空间爆满隐患。
*   **系统极致瘦身 (Extreme Slimming)**：削减多余 TTY 终端（保留 2 个），彻底移除冗余的 syslog (rsyslog) 服务，限制 Journald 最大日志体积，并可交互式停用 ModemManager/Avahi 等臃肿后台服务。
*   **路由转发控制**：提供独立的 IPv4 与 IPv6 路由转发状态切换，支持在纯建站模式（关闭转发）与代理/组网/容器中转模式（开启转发）间快速切换。

### 🛡️ 极致安全加固 (Extreme Security Hardening)
*   **SSH 深度硬化**：强制使用 Ed25519 公钥密钥登录并支持本地生成与自动注入；一键随机重映射至 40000+ 高位端口；深度适配 Systemd Socket 激活机制与传统 sshd 模式；内置 SSH 连通性预检与自动秒级回滚机制，防止配置错误导致远程联络中断。
*   **nftables 模块化防火墙**：弃用传统的 UFW，引入基于 `/etc/nftables.d` 的现代防火墙框架。实现模块规则的原子化隔离，支持端口去重与幂等下发接口，提供极致的包过滤性能。

### 📦 端口转发与 Web 服务 (Port Forwarding & Web Services)
*   **Realm 转发服务**：基于 Rust 开发的高性能、低开销多协议端口转发工具，配置专属 Systemd 守护及非 root 权限隔离运行。
*   **Ferron Web 服务器**：极速、轻量级的 Rust Web 服务器，支持现代化 KDL 语法配置，无缝集成自动 TLS 与 systemd 守护。
*   **定制化 Caddy**：采用 `xcaddy` 现场编译构建，集成 Layer 4 代理、Cloudflare DNS 解析以及 naiveproxy 插件，部署于受沙盒保护的独立系统用户环境。

### 🛰️ 虚拟组网与代理生态 (Networking & VPN Ecosystem)
*   **Xray Core**：官方部署脚本的模块化高可用封装，提供独立的节点管理菜单，支持一键切换 Loyalsoldier 增强版规则集，并具备 Cron 自动更新任务的状态感知与挂载能力。
*   **WARP & Usque 代理**：提供 Cloudflare WARP 客户端的极简 Socks5 模式切换、Usque (MASQUE 协议) 客户端自动注册，以及 **Xray 专属的 WireGuard 出站配置生成器**（支持节点优选扫描、精确 MTU 探测与防封锁指纹）。
*   **Tailscale & EasyTier**：一键部署虚拟组网客户端，并自动在 nftables 防火墙中放行 P2P 打洞所需的动态端口。
*   **Tailscale DERP 隐身节点**：实时克隆指定版本源码本地编译，通过 `sed` 动态注入防拨测补丁（阻断非法域名借用与 /generate_204 探针检测），自动配置双栈证书，并生成适配控制台的 ACL 配置 JSON。

### 🛠️ 终极运维工具链与 IP 养护
*   **Fish Shell 生态**：自动安装配置 Fish、`fisher` 插件管理器及 `fzf.fish`、`tide` 等常用生产力插件，支持交互式一键切换默认 Shell。
*   **Micro 编辑器与 Acme.sh**：预设鼠标支持、语法高亮与自动缩进的现代化微型编辑器；全自动 Acme.sh 证书管理，默认切换至兼容性最佳的 Let's Encrypt 证书服务。
*   **FreshIP (IP 养护)**：面向高频业务的 IP 信用养护与信誉度净化工具。支持单/双栈独立并发养护、全球节点动态切换、热重载配置，并集成 `curl-impersonate` 级别的 TLS 指纹伪装。

### ⚙️ 底层架构优势与自动化 CI
*   **全局网络自举与 DNS 自愈**：自动检测宿主机地理位置，境内优先启用高可用 APT 源镜像与多路 GitHub 镜像加速池；基于 `getent` 跨版本智能探测 DNS 可用性，网络阻断时自动写入临时 DNS 救活。
*   **CI 无人值守模式**：全局支持 `CI=true` 环境变量，在此环境下自动绕过所有 TUI 交互确认，完美接入自动化流水线与云端一键初始化。

---

## 📥 快速开始

### 🚀 一键自举命令
请在 root 权限或具有提权权限的用户下运行以下指令。自举脚本将智能检测环境，遇到无 `sudo` 或 `curl` 的环境将自动降级引导。

**通用网络环境 (GitHub 原生拉取)**
*   *使用 curl 运行*
    ```bash
    bash -c "$(curl -fsSL https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
    ```
*   *使用 wget 运行*
    ```bash
    bash -c "$(wget -qO- https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
    ```

**中国大陆网络环境 (多节点加速镜像)**
*   *使用 curl 运行*
    ```bash
    bash -c "$(curl -fsSL https://ghfast.top/https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
    ```
*   *使用 wget 运行*
    ```bash
    bash -c "$(wget -qO- https://ghfast.top/https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
    ```

> 💡 **提示**：若系统未安装 `sudo` 且当前为非 root 普通用户，请使用：`su - -c "bash <(curl -fsSL https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"` 切换至 root 直接执行（或替换为 wget 指令）。
>
> 安装完成后，您可以在系统任意路径输入 `debopti` 直接唤起多功能管理面板。

---

## 📂 项目文件结构

```text
/opt/debopti/
├── deb_optimizer.sh    # 主程序入口
├── scripts/            # 核心逻辑模块
│   ├── common.sh       # 通用工具函数库与全局状态
│   ├── network.sh      # 网络与模块化防火墙优化
│   ├── system.sh       # 内核调优与系统极简瘦身
│   ├── security.sh     # SSH 与账号安全加固
│   ├── tui.sh          # 交互式菜单渲染引擎
│   └── apps/           # 各类应用服务自动化部署模块
└── /etc/debopti/       # 配置文件及持久化状态存储
```

---

## 🛠️ 持久化配置说明

脚本将全局运行状态和检查点存储在 `/etc/debopti/debopti.conf` 中，可编辑该文件定制行为：

```bash
# 服务器所属地区标识 (true/false)
IS_CN_REGION="true"

# 系统基础性能优化是否已完成
BASE_OPTIMIZED="true"

# TUI 默认文本编辑器
EDITOR_CMD="micro"
```

---

## ⚠️ 注意事项与安全警示

1. **默认防火墙环境**：本脚本的默认防火墙为 **nftables**。强烈建议您在此环境下直接使用 `nftables` 规则进行包过滤与端口控制，**极不建议使用 ufw、firewalld 等三方防火墙管理工具代为管理**，以防规则冲突造成端口异常暴露或网络失联。
2. **系统支持**：本脚本仅支持 Debian 10 及以上版本（涵盖最新的 Debian 12/13 及未来版本）。
3. **连通性校验**：涉及内核参数优化与 SSH 端口随机重映射时，请务必配合脚本内置的连通性预检与自动秒级回滚流程，避免因本地网络或外部安全组策略导致失联。

---

## 🤝 贡献与支持

*   如果您发现了任何 Bug 或有新的模块需求，欢迎提交 [Issues](https://github.com/G3arB0xX/Debian-Optimizer-Script/issues)。
*   欢迎贡献代码！请遵循 `scripts/` 下的模块化规范和防御性 shell 编写纪律，然后发起 Pull Request。
