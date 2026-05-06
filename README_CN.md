# 🚀 Debian Optimizer Script

[English](README.md) | 简体中文

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)
[![Platform: Debian](https://img.shields.io/badge/Platform-Debian%2010%2B-orange.svg)](https://www.debian.org/)

**Debian Optimizer Script** 是一个用于 Debian 系统的系统调优与服务管理脚本。支持系统内核调优、网络性能优化、DevOps 工具集成以及多种服务自动化部署。

---

## 🌟 核心特性

- **⚡ 系统性能优化**：开启 BBR、调优内核参数、配置 ZRAM/Swap、清理冗余服务。
- **🛡️ 安全加固**：配置 Ed25519 密钥登录、更改 SSH 端口、nftables 防火墙管理。
- **📦 应用自动化部署**：
  - **🔗 中转服务**：Realm, Ferron, Caddy。
  - **🛰️ 代理服务**：Xray Core, WARP, Usque。
  - **🌐 组网服务**：Tailscale, Easytier, DERP 节点。
  - **🛠️ 运维工具**：Fish Shell, Micro Editor, Acme.sh。
- **🔄 脚本管理**：支持一键安装、更新与卸载，配置持久化。
- **🌍 环境识别**：自动识别网络环境并切换镜像源。

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

## 🇨🇳 网络优化 (中国大陆)

脚本内置了针对中国大陆环境的下载加速逻辑：
1. **🔍 自动检测**：识别归属地并使用镜像站。
2. **🚀 APT 源加速**：可选切换至国内镜像源。
3. **💎 GitHub 下载加速**：内置多组代理镜像池。
4. **⚙️ 环境变量**：配置 Go/Rust 代理。

### 脚本维护

- **🆙 更新脚本**：进入面板选择 `12. 脚本维护` -> `1. 检查并同步最新版本`。
- **🗑️ 卸载脚本**：进入面板选择 `12. 脚本维护` -> `2. 彻底卸载脚本及资产`。

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
