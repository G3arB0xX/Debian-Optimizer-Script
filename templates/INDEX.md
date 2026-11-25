---
name: templates-index
description: L0 agent router for templates/, mapping common, system, security, and per-app configuration templates.
zh_summary: 模板目录 `templates/` 的 L0 Agent 路由索引，登记基础、系统、安全及各应用服务的单一事实源配置模板。
tags: [templates, config, render_template, index, l0-router]
index: [AGENTS.md, rust/INDEX.md, archive/legacy-bash/INDEX.md, templates/agent, templates/common, templates/system, templates/security, templates/apps/derper, templates/apps/devops, templates/apps/easytier, templates/apps/ferron, templates/apps/freship, templates/apps/golang, templates/apps/lego, templates/apps/podman, templates/apps/realm, templates/apps/rust, templates/apps/tailscale, templates/apps/warp, templates/apps/xray]
---

<index>
DIR:
  path: templates
  root_agent_entry: AGENTS.md

RULES:
  single_source: 10 §2 -> 0 inline config Here-Docs in scripts | all system/app configs live under templates/ and deploy via render_template

TEMPLATE_GROUPS:
  agent: templates/agent/ (VPS agent workspace v2 canonical IR: AGENTS/README/.agents/MEMORY/gitignore | docs/agent/ control hub & decisions.md | constraints 00..50+90 | CoALA memory | JSON schemas & 14-adapter registry | overrides plugin bay | systemd credential drop-in)
  common: templates/common/ (debopti.conf | debopty.sh | debopty.fish)
  system: templates/system/ (sysctl forwarding/nofile/optimize | buster sources.list | tmux.conf | zramswap | 99-debopty-retention.conf | debopty-cache-retention.conf | debopty-apt-clean.service | debopty-apt-clean.timer | 99-debopty-apt-clean)
  security: templates/security/ (nftables.conf | rule_template.nft | Fail2ban jail/ports/filter/guard & systemd sync units | ssh.socket override)

APP_TEMPLATES:
  derper: templates/apps/derper/
  devops: templates/apps/devops/
  easytier: templates/apps/easytier/
  ferron: templates/apps/ferron/
  freship: templates/apps/freship/
  golang: templates/apps/golang/
  lego: templates/apps/lego/
  podman: templates/apps/podman/ (containers.conf | restic-excludes.txt | restic-backup.sh | sudoers.d/debopti-podman)
  realm: templates/apps/realm/
  rust: templates/apps/rust/
  tailscale: templates/apps/tailscale/
  warp: templates/apps/warp/
  xray: templates/apps/xray/

CROSS_LINKS:
  rust_router: rust/INDEX.md
  legacy_archive_router: archive/legacy-bash/INDEX.md
  docs_router: docs/INDEX.md
  template_rules: .agents/constraints/10_bash_and_templates.md
</index>
