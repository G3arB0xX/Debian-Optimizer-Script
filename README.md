# 🚀 Debian Optimizer Script

English | [简体中文](README_CN.md)

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)
[![Platform: Debian](https://img.shields.io/badge/Platform-Debian%2010%2B-orange.svg)](https://www.debian.org/)

**Debian Optimizer Script** is a system tuning and service management script for Debian. It provides kernel optimization, network tuning, DevOps tool integration, and automated service deployment.

---

## 🌟 Key Features

- **⚡ System Performance Tuning**: BBR activation, kernel parameter optimization, ZRAM/Swap configuration, and service cleanup.
- **🛡️ Security Hardening**: Ed25519 key login, SSH port remapping, and nftables firewall management.
- **📦 Automated App Deployment**:
  - **🔗 Relay Services**: Realm, Ferron, Caddy.
  - **🛰️ Proxy Services**: Xray Core, WARP, Usque.
  - **🌐 Networking**: Tailscale, Easytier, DERP nodes.
  - **🛠️ DevOps Tools**: Fish Shell, Micro Editor, Acme.sh.
- **🔄 Script Management**: One-click installation, updates, and uninstallation with persistent configuration.
- **🌍 Network Awareness**: Automatically detects network regions and switches to local mirrors.

---

## 📥 Quick Start

### 🚀 One-Click Installation

Run the following command as root. The script will automatically handle environment pre-checks, mirror selection, project synchronization, and global command binding:

**General Environment (GitHub Native)**
```bash
bash -c "$(curl -fsSL https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
```

**Mainland China (ghfast.top Mirror)**
```bash
bash -c "$(curl -fsSL https://ghfast.top/https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
```

> **Tip**: After installation, you can simply type `debopti` anywhere in the terminal to call the management panel.

---

## 📂 Project Structure

```text
/opt/debopti/
├── deb_optimizer.sh    # Main Entry Point
├── scripts/            # Core Logic Modules
│   ├── common.sh       # Common Utils & Configuration
│   ├── network.sh      # Network & Firewall Optimization
│   ├── system.sh       # Kernel & System Tuning
│   ├── security.sh     # SSH & Account Hardening
│   ├── tui.sh          # Interactive TUI Engine
│   └── apps/           # Automated App Deployment Modules
└── /etc/debopti/       # Persistent Config Directory
```

---

## 🛠️ Configuration

Global states are recorded in `/etc/debopti/debopti.conf`. You can manually modify this file to adjust script behavior:

```bash
# Whether the server is in Mainland China (true/false)
IS_CN_REGION="true"

# Base optimization completion flag
BASE_OPTIMIZED="true"

# Default Text Editor
EDITOR_CMD="micro"
```

---

## 🤝 Contribution & Support

- If you find a bug or have a feature request, please submit an [Issue](https://github.com/G3arB0xX/Debian-Optimizer-Script/issues).
- For code contributions, please follow the modular style in `scripts/` and submit a Pull Request.
