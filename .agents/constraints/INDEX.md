---
name: constraints-router
description: L0 agent router for .agents/constraints/, listing layered rules 00 through 80 in Simplified Chinese.
zh_summary: 分层约束目录 `.agents/constraints/` 的 L0 Agent 路由索引，登记 `00` 至 `80` 全部人读中文约束文件及其按任务加载映射。
tags: [constraints, index, l0-router, rules]
index: [.agents/INDEX.md, .agents/CONSTRAINTS.md, .agents/constraints/README.md, .agents/constraints/00_general.md, .agents/constraints/10_bash_and_templates.md, .agents/constraints/20_security_and_systemd.md, .agents/constraints/30_release_and_vcs.md, .agents/constraints/40_testing_and_ci.md, .agents/constraints/50_context_and_docs.md, .agents/constraints/60_rust_and_reconcile.md, .agents/constraints/70_agent_workspace_and_credentials.md, .agents/constraints/80_swarm_orchestration.md]
---

<index>
DIR:
  path: .agents/constraints
  human_readme: .agents/constraints/README.md
  parent_index: .agents/INDEX.md

ALWAYS_READ:
  general: .agents/constraints/00_general.md
  context_and_docs: .agents/constraints/50_context_and_docs.md

LAYERED_RULES:
  bash_and_templates: .agents/constraints/10_bash_and_templates.md
  security_and_systemd: .agents/constraints/20_security_and_systemd.md
  release_and_vcs: .agents/constraints/30_release_and_vcs.md
  testing_and_ci: .agents/constraints/40_testing_and_ci.md
  rust_and_reconcile: .agents/constraints/60_rust_and_reconcile.md
  agent_workspace_and_credentials: .agents/constraints/70_agent_workspace_and_credentials.md
  swarm_orchestration: .agents/constraints/80_swarm_orchestration.md

TASK_ROUTING:
  scripts_and_templates: 00 + 10 + 40 + 50 (add 20 for ports/units/perms)
  security_and_firewall: 00 + 10 + 20 + 40 + 50 (add 70 for credentials)
  rust_and_reconcile: 00 + 10 + 20 + 40 + 50 + 60
  agent_workspace_and_credentials: 00 + 20 + 50 + 60 + 70 (v2 blueprint | CoALA memory | 14 adapters | lossless migrate | U29 creds)
  swarm_and_multi_agent: 00 + 30 + 40 + 50 + 80 (add 60 for Rust builds)
  release_and_push: 00 + 30 + 40
  tests_and_ci: 00 + 40 + 50 (add 30 for release filters)
  docs_and_agents: 00 + 50 (add affected domain layer)

CROSS_LINKS:
  root_guide: AGENTS.md
  docs_manual: docs/README.md
  makefile: Makefile
  check_gate: tools/check.sh
  release_cli: tools/release.sh
</index>
