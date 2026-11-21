---
name: scripts-index
description: L0 agent router for scripts/, mapping core runtime modules and service application modules.
zh_summary: 脚本目录 `scripts/` 的 L0 Agent 路由索引，登记 6 个核心基础模块与 11 个应用服务部署模块。
tags: [scripts, bash, modules, index, l0-router]
index: [AGENTS.md, templates/INDEX.md, scripts/common.sh, scripts/version.sh, scripts/network.sh, scripts/system.sh, scripts/security.sh, scripts/tui.sh, scripts/apps/devops.sh, scripts/apps/easytier.sh, scripts/apps/ferron.sh, scripts/apps/freship.sh, scripts/apps/golang.sh, scripts/apps/podman.sh, scripts/apps/realm.sh, scripts/apps/rust.sh, scripts/apps/tailscale.sh, scripts/apps/warp.sh, scripts/apps/xray.sh]
---

<index>
DIR:
  path: scripts
  root_agent_entry: AGENTS.md

CORE_MODULES:
  common: scripts/common.sh (logging | render_template | config state | runtime home | helpers)
  version: scripts/version.sh (SemVer + CalVer/Build parsing & comparison)
  network: scripts/network.sh (network probing | CN/global mirror detection | downloads)
  system: scripts/system.sh (BBR | sysctl | zram | logrotate | tmux | system cleanup)
  security: scripts/security.sh (SSH hardening | nftables modular rules | Fail2ban + port sync)
  tui: scripts/tui.sh (interactive menu dispatcher & TUI chrome)

APP_MODULES:
  devops: scripts/apps/devops.sh (Fish | Micro | Yazi | Lego)
  easytier: scripts/apps/easytier.sh (Easytier mesh VPN)
  ferron: scripts/apps/ferron.sh (Ferron web server)
  freship: scripts/apps/freship.sh (FreshIP IP maintenance & probe)
  golang: scripts/apps/golang.sh (Go toolchain installer)
  podman: scripts/apps/podman.sh (rootless Podman + apps user + debopti-certs)
  realm: scripts/apps/realm.sh (Realm port forwarding)
  rust: scripts/apps/rust.sh (Rust toolchain installer)
  tailscale: scripts/apps/tailscale.sh (Tailscale + Caddy/DERPer stealth node)
  warp: scripts/apps/warp.sh (Cloudflare WARP & Usque MASQUE client)
  xray: scripts/apps/xray.sh (Xray Core & ruleset management)

CROSS_LINKS:
  templates_router: templates/INDEX.md
  docs_router: docs/INDEX.md
  bash_rules: governance/constraints/10_bash_and_templates.md
  security_rules: governance/constraints/20_security_and_systemd.md
</index>
