---
name: docs-index
description: L0 agent router for docs/, mapping all 11 manual operation tutorials and the Rust architecture design doc.
zh_summary: 文档目录 `docs/` 的 L0 Agent 路由索引，登记全部 11 篇与自动化脚本严格对齐的中文配置教程及 Rust 重构架构设计文档。
tags: [docs, manuals, tutorials, architecture, index, l0-router]
index: [AGENTS.md, docs/README.md, rust/INDEX.md, archive/legacy-bash/INDEX.md, docs/01-system-tuning.md, docs/02-security-hardening.md, docs/03-podman.md, docs/04-xray.md, docs/05-warp.md, docs/06-devops-tools.md, docs/07-port-forwarding.md, docs/08-web-services.md, docs/09-terminal-toolchain.md, docs/10-easytier.md, docs/11-freship.md, docs/12-rust-architecture.md]
---

<index>
DIR:
  path: docs
  human_readme: docs/README.md
  root_agent_entry: AGENTS.md

MANUAL_TUTORIALS:
  01_system_tuning: docs/01-system-tuning.md (BBR | sysctl | zram | logrotate | system cleanup)
  02_security_hardening: docs/02-security-hardening.md (SSH hardening | port randomization | nftables | Fail2ban)
  03_podman: docs/03-podman.md (Podman rootless container setup)
  04_xray: docs/04-xray.md (Xray Core deployment & ruleset management)
  05_warp: docs/05-warp.md (Cloudflare WARP & Usque MASQUE proxy clients)
  06_devops_tools: docs/06-devops-tools.md (Fish | Micro | Yazi | Lego installation & config)
  07_port_forwarding: docs/07-port-forwarding.md (Realm port forwarding deployment)
  08_web_services: docs/08-web-services.md (custom Caddy | DERPer stealth node | Ferron web server)
  09_terminal_toolchain: docs/09-terminal-toolchain.md (Fish + Yazi + Micro daily workflow & shortcuts)
  10_easytier: docs/10-easytier.md (Easytier mesh VPN install, firewall & PATH)
  11_freship: docs/11-freship.md (FreshIP IP maintenance, simulate mode, logs & hot-data sync)

ARCHITECTURE_DOCS:
  12_rust_architecture: docs/12-rust-architecture.md (Pure Rust stack | Strangler-Fig coexistence | rust workspace crates | musl build | 3-phase roadmap)

CROSS_LINKS:
  rust_router: rust/INDEX.md
  legacy_archive_router: archive/legacy-bash/INDEX.md
  templates_router: templates/INDEX.md
  doc_sync_rules: governance/constraints/50_context_and_docs.md
</index>
