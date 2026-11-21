---
name: legacy-bash-archive-index
description: L0 agent router for archive/legacy-bash/, preserving retired Bash entrypoint and module scripts for parity tests and explicit --engine bash fallback.
zh_summary: 旧版纯 Bash 脚本归档目录 `archive/legacy-bash/` 的 L0 Agent 路由索引，登记已退役的入口脚本与 17 个核心及应用脚本，供回归对照测试与显式兼容回退参考。
tags: [archive, legacy, bash, parity, index, l0-router]
index: [AGENTS.md, archive/legacy-bash/README.md, docs/12-rust-architecture.md, templates/INDEX.md, archive/legacy-bash/deb_optimizer.sh, archive/legacy-bash/scripts/common.sh, archive/legacy-bash/scripts/version.sh, archive/legacy-bash/scripts/network.sh, archive/legacy-bash/scripts/system.sh, archive/legacy-bash/scripts/security.sh, archive/legacy-bash/scripts/tui.sh, archive/legacy-bash/scripts/apps/devops.sh, archive/legacy-bash/scripts/apps/easytier.sh, archive/legacy-bash/scripts/apps/ferron.sh, archive/legacy-bash/scripts/apps/freship.sh, archive/legacy-bash/scripts/apps/golang.sh, archive/legacy-bash/scripts/apps/podman.sh, archive/legacy-bash/scripts/apps/realm.sh, archive/legacy-bash/scripts/apps/rust.sh, archive/legacy-bash/scripts/apps/tailscale.sh, archive/legacy-bash/scripts/apps/warp.sh, archive/legacy-bash/scripts/apps/xray.sh]
---

<index>
DIR:
  path: archive/legacy-bash
  human_readme: archive/legacy-bash/README.md
  root_agent_entry: AGENTS.md
  status: retired_archived (21/21 NativeRust default cutover complete | excluded from /opt/debopty & release tarballs)

ARCHIVED_ENTRYPOINT:
  deb_optimizer: archive/legacy-bash/deb_optimizer.sh (retired Bash launcher; discoverable via debopty --engine bash)

ARCHIVED_CORE_MODULES:
  common: archive/legacy-bash/scripts/common.sh (VERSION_ID compat target | render_template | config state)
  version: archive/legacy-bash/scripts/version.sh (SemVer + CalVer/Build reference parser)
  network: archive/legacy-bash/scripts/network.sh (legacy network & mirror probing)
  system: archive/legacy-bash/scripts/system.sh (legacy BBR | sysctl | zram | tmux)
  security: archive/legacy-bash/scripts/security.sh (legacy SSH | nftables | Fail2ban)
  tui: archive/legacy-bash/scripts/tui.sh (legacy interactive menu dispatcher)

ARCHIVED_APP_MODULES:
  devops: archive/legacy-bash/scripts/apps/devops.sh (Fish | Micro | Yazi | Lego)
  easytier: archive/legacy-bash/scripts/apps/easytier.sh (Easytier mesh VPN)
  ferron: archive/legacy-bash/scripts/apps/ferron.sh (Ferron web server)
  freship: archive/legacy-bash/scripts/apps/freship.sh (FreshIP IP maintenance)
  golang: archive/legacy-bash/scripts/apps/golang.sh (Go | Caddy | DERPer)
  podman: archive/legacy-bash/scripts/apps/podman.sh (rootless Podman)
  realm: archive/legacy-bash/scripts/apps/realm.sh (Realm port forwarding)
  rust: archive/legacy-bash/scripts/apps/rust.sh (Rust toolchain)
  tailscale: archive/legacy-bash/scripts/apps/tailscale.sh (Tailscale VPN)
  warp: archive/legacy-bash/scripts/apps/warp.sh (Cloudflare WARP & Usque)
  xray: archive/legacy-bash/scripts/apps/xray.sh (Xray Core & ruleset)

CROSS_LINKS:
  rust_router: rust/INDEX.md
  templates_router: templates/INDEX.md
  docs_router: docs/INDEX.md
  architecture_doc: docs/12-rust-architecture.md
</index>
