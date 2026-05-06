# 🚀 Debian Optimizer Script (V2.0)

[English](README.md) | 简体中文

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)
[![Platform: Debian](https://img.shields.io/badge/Platform-Debian%2010%2B-orange.svg)](https://www.debian.org/)

**Debian Optimizer Script** 是一款专为 Debian 系统设计的全能型运维加速与服务管理面板。它不仅能深度优化系统内核与网络性能，还集成了一系列现代化 DevOps 工具与网络代理协议的自动化部署，旨在为低配 VPS 到高性能服务器提供一键式的生产级配置体验。

---

## 🌟 核心特性

- **⚡ 极致性能优化**：一键开启 BBR、调优内核参数、配置 ZRAM/Swap、清理冗余服务，显著提升网络并发与系统响应。
- **🛡️ 零信任安全加固**：强制 Ed25519 密钥登录、自动更换随机高位端口、nftables 防火墙精准管控，默认阻断一切高危入口。
- **📦 现代化应用生态**：
  - **中转栈**：Realm (Rust), Ferron, Caddy (L4/Naive)。
  - **网络栈**：Xray Core, WARP (Socks5/WireGuard), Usque (Masque)。
  - **组网栈**：Tailscale, Easytier, DERP 隐身节点。
  - **运维栈**：Fish Shell (带插件), Micro Editor, Acme.sh。
- **🔄 脚本生命周期管理**：内置一键安装、平滑更新与深度卸载功能，配置持久化于 `/etc/debopti/`。
- **🌐 全局网络感知**：智能识别中国大陆环境，自动切换 APT 源、GitHub 镜像加速及 Go/Rust 代理，确保国内环境秒级完成部署。

---

## 📥 快速开始

### 🚀 一键安装命令

在 root 权限下执行以下指令，脚本将自动完成环境预检、镜像选择、项目同步及全局命令绑定：

**通用环境 (GitHub 原生)**
```bash
bash -c "$(curl -fsSL https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
```

**中国大陆环境 (ghfast.top 镜像加速)**
```bash
bash -c "$(curl -fsSL https://ghfast.top/https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
```

> **提示**：安装完成后，您可以在系统任何路径直接输入 `debopti` 唤起管理面板。

---

## 🇨🇳 中国大陆环境专项优化

为了确保国内 VPS 用户的顺滑体验，本项目内置了以下优化：
1. **智能自举**：安装脚本自动检测归属地，优先通过 `ghfast.top` 拉取代码。
2. **APT 镜像加速**：自动切换至清华大学 TUNA 镜像源。
3. **GitHub 全局加速**：脚本内所有组件下载均内置了混合模式镜像池轮询（ghp.ci, ghfast 等）。
4. **运行环境代理**：自动配置 Go/Rust 代理环境变量。

### 脚本维护

- **更新脚本**：进入面板选择 `12. 脚本维护` -> `1. 检查并同步最新版本`。
- **卸载脚本**：进入面板选择 `12. 脚本维护` -> `2. 彻底卸载脚本及资产`。

---

## 📂 项目结构

```text
/opt/debopti/
├── deb_optimizer.sh    # 主程序入口
├── scripts/            # 核心逻辑模块
│   ├── common.sh       # 通用工具函数库
│   ├── network.sh      # 网络与防火墙优化
│   ├── system.sh       # 内核与系统级调优
│   ├── security.sh     # SSH 与账号安全加固
│   ├── tui.sh          # 交互式菜单引擎
│   └── apps/           # 各类应用自动化部署模块
└── /etc/debopti/       # 持久化配置文件目录
```

---

## 🛠️ 配置说明

所有全局状态记录在 `/etc/debopti/debopti.conf` 中，您可以手动修改该文件来调整脚本行为：

```bash
# 是否位于中国大陆 (true/false)
IS_CN_REGION="true"

# 基础优化完成标记
BASE_OPTIMIZED="true"

# 默认编辑器
EDITOR_CMD="micro"
```

---

## ⚠️ 注意事项

1. 本脚本仅支持 **Debian 10 及以上** 版本（包括最新的 Debian 12/13）。
2. 执行过程中涉及内核参数修改与 SSH 端口变更，请务必根据脚本提示验证连通性。
3. 请勿在生产环境盲目执行所有优化项，建议根据服务器实际用途（如建站或中转）按需配置。

---

## 🤝 贡献与支持

- 如果您发现了 Bug 或有功能需求，欢迎提交 [Issues](https://github.com/G3arB0xX/Debian-Optimizer-Script/issues)。
- 代码贡献请参考 `scripts/` 下的模块化风格提交 Pull Request。

**Author**: [G3arB0xX](https://github.com/G3arB0xX)  
**License**: GPL-3.0
