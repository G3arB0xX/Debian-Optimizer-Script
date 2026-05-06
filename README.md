# 🚀 Debian Optimizer Script (V2.0)

English | [简体中文](README_CN.md)

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)
[![Platform: Debian](https://img.shields.io/badge/Platform-Debian%2010%2B-orange.svg)](https://www.debian.org/)

**Debian Optimizer Script** is a versatile all-in-one management panel designed for Debian systems. It offers deep system kernel and network performance tuning, along with automated deployment of modern DevOps tools and network proxy protocols. It aims to provide a production-grade configuration experience for everything from low-spec VPS to high-performance servers.

---

## 🌟 Key Features

- **⚡ Extreme Performance Tuning**: One-click BBR activation, kernel parameter optimization, ZRAM/Swap configuration, and redundant service cleanup for enhanced responsiveness.
- **🛡️ Zero-Trust Security**: Forced Ed25519 key login, automated random high-port remapping, and precise nftables firewall control to block high-risk entries by default.
- **📦 Modern Application Ecosystem**:
  - **Relay Stack**: Realm (Rust), Ferron, Caddy (L4/Naive).
  - **Network Stack**: Xray Core, WARP (Socks5/WireGuard), Usque (Masque).
  - **Networking Stack**: Tailscale, Easytier, DERP Stealth Nodes.
  - **DevOps Stack**: Fish Shell (with plugins), Micro Editor, Acme.sh.
- **🔄 Script Lifecycle Management**: Built-in one-click installation, smooth updates, and deep uninstallation. Persistent configuration stored in `/etc/debopti/`.
- **🌐 Global Network Awareness**: Automatically detects Mainland China environments to switch APT mirrors, GitHub acceleration, and Go/Rust proxies for lightning-fast deployments.

---

## 📥 Quick Start

### One-Click Installation

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

**Author**: [G3arB0xX](https://github.com/G3arB0xX)  
**License**: GPL-3.0
