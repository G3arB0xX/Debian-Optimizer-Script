# 🚀 Debian Optimizer Script

English | [简体中文](README_CN.md)

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)
[![Platform: Debian](https://img.shields.io/badge/Platform-Debian%2010%2B-orange.svg)](https://www.debian.org/)

**Debian Optimizer Script** is a high-performance, modular, and security-hardened TUI application tailored for Debian systems. It integrates kernel parameter tuning, modular network firewalls, DevOps tooling, and secure service deployments into a robust management console.

---

## 🌟 Key Features

### ⚡ Base Performance Optimization
*   **Kernel & Network Tuning**: Automatic BBR acceleration and optimized network buffer parameters deployed via non-intrusive `.d` configuration files.
*   **Memory & Logs Management**: Dynamic ZRAM (zstd compression) or Swap configuration, coupled with daily-rotated and compressed log retention to prevent disk depletion.
*   **Extreme VPS Slimming**: Cuts unnecessary TTY channels, removes legacy syslog daemons (rsyslog), caps Journald size, and interactively disables high-overhead background processes for low-resource environments.
*   **Route Forwarding Control**: Independent IPv4/IPv6 IP forwarding toggles, supporting on-the-fly switching between pure web hosting mode and routing/mesh-network/container modes.

### 🛡️ Deep Security Hardening
*   **SSH Hardening**: Enforces Ed25519 public key authentication with automatic keypair generation, remaps daemon ports to random high-range ports (40000+), supports Systemd Socket isolation, and embeds an active connection validation & auto-rollback mechanism to prevent lockout.
*   **nftables Firewall Framework**: Replaces legacy UFW/iptables with a modern, atomic `/etc/nftables.d` modular firewall structure, preventing duplicate SSH entries and providing clean, high-performance rule control.

### 📦 Port Forwarding & Web Services
*   **Realm**: A lightweight, multi-protocol Rust-based port forwarder with Systemd process protection and non-root execution.
*   **Ferron**: A fast Rust web server boasting native TLS configuration and contemporary KDL syntax integration.
*   **Customized Caddy**: Real-time compilations powered by `xcaddy` containing L4 proxy, Cloudflare DNS, and naiveproxy modules, structured under sandboxed system users.

### 🛰️ Mesh Networking & VPN Ecosystem
*   **Xray Core**: Seamless integration of upstream scripts with customizable rulesets (Official vs Loyalsoldier) and automated Cron update task state awareness.
*   **WARP & Usque (MASQUE)**: Configures WireGuard egress generation for Xray (including Endpoint scanner, MTU discovery, and signature camouflage) alongside Usque client registrations.
*   **Tailscale & EasyTier**: Streamlined deployment of secure overlay networks with automatic P2P firewall hole punching.
*   **Tailscale DERP Stealth Node**: Compiles tailscale DERP from source, dynamically injects anti-probing patches (blocking unauthorized DNS routing and generic /generate_204 requests), provisions dual-stack TLS, and produces the required ACL configuration JSON.

### 🛠️ DevOps Tooling & IP Grooming
*   **Fish Shell**: Installs Fish with `fisher` plugin manager, deploying productivity extensions like `fzf.fish` and `tide` with a one-click default shell switch.
*   **Micro Editor & Acme.sh**: Configures the intuitive terminal editor and implements domain certificate management switching to Let's Encrypt CA for maximum reliability.
*   **FreshIP (IP Care)**: Automated single/dual-stack IP grooming and credit purification, using TLS fingerprint camouflage (`curl-impersonate`) and Systemd templated timers.

### ⚙️ Core Architecture & CI
*   **Smart Network Bootstrapping**: Multi-node network region discovery (with priority for Mainland China acceleration proxies and local APT mirrors).
*   **DNS Self-Healing**: libc-based DNS health verification with automated fallback to prevent network timeouts during clean installations.
*   **Automated CI Pipeline**: Globally respects the `CI=true` environment variable, suppressing interactive prompts to enable headless unattended deployment scripts.

---

## 📥 Quick Start

### 🚀 One-Click Bootstrap
Run the corresponding script as root. If the system lacks `sudo` or `curl`, the script automatically detects and falls back to alternatives or prompts you with a clean escalation method.

**General Environment (GitHub Native)**
*   *Using curl:*
    ```bash
    bash -c "$(curl -fsSL https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
    ```
*   *Using wget:*
    ```bash
    bash -c "$(wget -qO- https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
    ```

**Mainland China (Accelerated Mirror)**
*   *Using curl:*
    ```bash
    bash -c "$(curl -fsSL https://ghfast.top/https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
    ```
*   *Using wget:*
    ```bash
    bash -c "$(wget -qO- https://ghfast.top/https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
    ```

> 💡 **Tip**: If `sudo` is not present and you are not root, switch to root using: `su - -c "bash <(curl -fsSL https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"` (or replace curl with wget).
>
> After installation, invoke the menu console from anywhere by simply entering `debopti`.

---

## 📂 Project Structure

```text
/opt/debopti/
├── deb_optimizer.sh    # Main Entry Point
├── scripts/            # Core Logic Modules
│   ├── common.sh       # Common Utilities & Configuration
│   ├── network.sh      # Network & Firewall Optimization
│   ├── system.sh       # Kernel & System Tuning
│   ├── security.sh     # SSH & Account Hardening
│   ├── tui.sh          # TUI Interactive Engine
│   └── apps/           # Automated Service Deployment Modules
└── /etc/debopti/       # Persistent State & Configuration
```

---

## 🛠️ State Configuration

All global configurations and execution checkpoints are persisted under `/etc/debopti/debopti.conf`:

```bash
# Geolocation region flag
IS_CN_REGION="true"

# Base system optimization execution state
BASE_OPTIMIZED="true"

# Fallback editor command
EDITOR_CMD="micro"
```

---

## ⚠️ Important Notes & Warnings

1. **Default Firewall Environment**: This script configures **nftables** as the primary system firewall framework. It is highly recommended to manage all firewall policies using native `nftables` rule packages. **We strongly advise against using `ufw`, `firewalld`, or other high-level frontends**, as their rule generation engines may conflict with the modular `nftables` rules deployed by this script.
2. **System Compatibility**: Officially supports Debian 10 and newer (fully compatible with Debian 12/13).
3. **Connectivity Validation**: Underlying network buffers and SSH remappings involve structural modifications. Always complete the script-guided SSH verification to ensure safety from server lockouts.

---

## 🤝 Contribution & Support

*   Submit structural bugs or feature suggestions through the project [Issues](https://github.com/G3arB0xX/Debian-Optimizer-Script/issues).
*   For codebase contributions, strictly follow the decoupled modular format in `scripts/` and submit a Pull Request.
