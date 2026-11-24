---
name: agents-index
description: L0 agent router for .agents/, mapping the constraints index and layered rule files 00 through 80.
zh_summary: 治理层 `.agents/` 的 L0 Agent 路由索引，指向约束总入口、分层规范目录（`00` 至 `80`，含 v2 VPS 工作环境、CoALA 记忆、Harness 通用化与无损迁移规范）与仓库根总入口。
tags: [agents, governance, index, l0-router, progressive-disclosure]
index: [AGENTS.md, .agents/CONSTRAINTS.md, .agents/constraints/INDEX.md, .agents/constraints/README.md, .agents/constraints/00_general.md, .agents/constraints/10_bash_and_templates.md, .agents/constraints/20_security_and_systemd.md, .agents/constraints/30_release_and_vcs.md, .agents/constraints/40_testing_and_ci.md, .agents/constraints/50_context_and_docs.md, .agents/constraints/60_rust_and_reconcile.md, .agents/constraints/70_agent_workspace_and_credentials.md, .agents/constraints/80_swarm_orchestration.md]
---

<index>
LAYER:
  name: .agents
  root_agent_entry: AGENTS.md

CORE_FILES:
  constraints_entry: .agents/CONSTRAINTS.md
  human_constraints_readme: .agents/constraints/README.md

SUB_INDEXES:
  constraints: .agents/constraints/INDEX.md

LAYERED_DOCS:
  00_general: .agents/constraints/00_general.md
  10_bash_and_templates: .agents/constraints/10_bash_and_templates.md
  20_security_and_systemd: .agents/constraints/20_security_and_systemd.md
  30_release_and_vcs: .agents/constraints/30_release_and_vcs.md
  40_testing_and_ci: .agents/constraints/40_testing_and_ci.md
  50_context_and_docs: .agents/constraints/50_context_and_docs.md
  60_rust_and_reconcile: .agents/constraints/60_rust_and_reconcile.md
  70_agent_workspace_and_credentials: .agents/constraints/70_agent_workspace_and_credentials.md
  80_swarm_orchestration: .agents/constraints/80_swarm_orchestration.md
</index>
