# Debian Optimizer Script

English | [简体中文](README_CN.md)

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)
[![Platform: Debian](https://img.shields.io/badge/Platform-Debian%2010%2B-orange.svg)](https://www.debian.org/)

A TUI script for Debian system initialization, kernel tuning, security hardening, and automated service deployment. Supports Debian 10+.

---

## Quick Start

Run as root or a user with `sudo`. The script auto-detects `curl`/`wget` availability and your network region.

**General**
```bash
# curl
bash -c "$(curl -fsSL https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
# wget
bash -c "$(wget -qO- https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
```

**Mainland China (Mirror)**
```bash
# curl
bash -c "$(curl -fsSL https://ghfast.top/https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
# wget
bash -c "$(wget -qO- https://ghfast.top/https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
```

> **No sudo / non-root**: Switch to root first:
> ```bash
> su - -c "bash <(curl -fsSL https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
> # or with wget:
> su - -c "bash <(wget -qO- https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
> ```

> **Containers / non-login shells**: Docker, LXC, and similar environments may not set `HOME` when started via `bash -c`. The script auto-fills it at startup (root defaults to `/root`); no manual `export HOME` is required.

After installation, run `debopti` from anywhere to open the management panel.

---

## Documentation

Manual setup guides and usage tutorials live in [`docs/`](docs/README.md). Each file mirrors what the `debopti` script automates, so you can reproduce or customize any module by hand.

| Doc | Topic |
|---|---|
| [docs/README.md](docs/README.md) | Index of all manual guides |
| [docs/06-devops-tools.md](docs/06-devops-tools.md) | Fish, Micro, Yazi, Lego — install & config |
| [docs/09-terminal-toolchain.md](docs/09-terminal-toolchain.md) | Fish + Yazi + Micro — usage & workflows |

---

## Project Structure

```text
/opt/debopti/
├── deb_optimizer.sh    # Main entry point
├── scripts/
│   ├── common.sh       # Shared utilities and state management
│   ├── network.sh      # Network and nftables firewall
│   ├── system.sh       # Kernel tuning and system slimming
│   ├── security.sh     # SSH and account hardening
│   ├── tui.sh          # TUI rendering engine
│   └── apps/           # Per-application deployment modules
└── /etc/debopti/       # Persistent config and state
```

---

## Features

### Base System Optimization

*   **Kernel & Network Tuning**: Enables BBR and tunes network stack parameters via isolated `.d` config files.
*   **Memory & Logs**: Interactive ZRAM or Swap configuration; daily-rotated and compressed log retention via Logrotate.
*   **System Slimming**: Reduces spare TTYs, removes rsyslog, caps Journald size, and optionally disables idle services like ModemManager and Avahi.
*   **IP Forwarding**: Independent toggle for IPv4/IPv6 forwarding — web hosting mode or proxy/mesh/container mode.

### Security Hardening

*   **SSH Hardening**: Enforces Ed25519 public key auth with auto-generated keypair injection; remaps port to a random 40000+ value; supports Systemd Socket and classic sshd; built-in connectivity pre-check with auto-rollback on failure.
*   **nftables Firewall**: Manages rules under a `/etc/nftables.d` modular structure — atomic rule isolation, idempotent updates.

> ⚠️ **Note**: This script uses **nftables** as the default firewall. **Do not use `ufw`, `firewalld`, or similar tools alongside it** — conflicting rule backends can cause unexpected port exposure or network interruption.

### Port Forwarding & Web Services

*   **Realm**: Lightweight Rust-based multi-protocol port forwarder, runs as a non-root Systemd service.
*   **Ferron**: Rust web server with automatic TLS and KDL config syntax.
*   **Custom Caddy**: Compiled on-device via `xcaddy` with L4 proxy, Cloudflare DNS, and naiveproxy modules; runs under a dedicated system user.

### Proxy & Mesh Networking

*   **Xray Core**: Deployed via the upstream script with a built-in node management menu. Supports one-click switching between the official and Loyalsoldier ruleset; Cron auto-update tasks are independently toggleable with persistent state.
*   **WARP & Usque**: Deploys Cloudflare WARP in Socks5 mode and registers a Usque client. Includes an Xray WireGuard egress config generator with endpoint scanning, MTU detection, and obfuscation parameter calculation.
*   **Tailscale & EasyTier**: One-click mesh client deployment with automatic nftables P2P port rules.
*   **Tailscale DERP Node**: Compiled from source at a pinned version with anti-probing patches injected; auto-provisions dual-stack TLS certs and outputs ACL JSON for the Tailscale console.
*   **Podman**: Rootless container runtime under a dedicated `apps` user, with journald logging, registry mirrors, Quadlet, and `podman-compose`; managed via `apppod`/`appctl`/`applog`.

### DevOps Tools

*   **Fish Shell**: Installs Fish with `fisher`, deploys `fzf.fish` and `tide`; one-click default shell switch.
*   **Micro Editor**: Pre-configured with mouse support, syntax highlighting, and auto-indent.
*   **Yazi File Manager**: Extremely fast asynchronous terminal file manager with Windows/Micro-friendly bindings, plugin support, and shell CWD synchronization.
*   **Acme.sh**: Certificate management tool defaulting to Let's Encrypt CA.

### IP Maintenance

Based on resources from [IP-Sentinel](https://github.com/hotyue/IP-Sentinel) — credit to the original author.

*   Single/dual-stack concurrent operation with global node rotation and hot-reload config.
*   Uses `curl-impersonate` for TLS fingerprint spoofing.
*   Scheduled via Systemd templated timers, start/stop on demand.

### Architecture

*   **Region-aware bootstrapping**: Detects geolocation at startup; switches APT mirrors and GitHub download proxies for mainland China automatically.
*   **DNS self-healing**: Tests DNS resolution via `getent`; writes temporary public DNS entries on failure and restores the original afterward.
*   **Idempotent operations**: All configuration steps are safe to re-run — no duplicate entries or file corruption.
*   **CI support**: Set `CI=true` to skip all interactive prompts for headless automated deployment.
*   **Clean uninstall**: Removes symlinks, shell environment injections, and the install directory with no residuals.

---

## Configuration

State is persisted to `/etc/debopti/debopti.conf`:

```bash
IS_CN_REGION="true"    # Mainland China region flag
BASE_OPTIMIZED="true"  # Base optimization complete flag
EDITOR_CMD="micro"     # Default text editor
```

---

## Notes

1. Requires **Debian 10 or later**, including Debian 12 / 13.
2. SSH port changes and kernel tuning include a guided verification step with auto-rollback on failure.
3. Default firewall is nftables — **do not use `ufw` or `firewalld` at the same time**.

---

## Contributing

*   Bug reports and feature requests: [Issues](https://github.com/G3arB0xX/Debian-Optimizer-Script/issues).
*   Code contributions: follow the modular conventions in `scripts/` and open a Pull Request.
