---
name: governance-index
description: L0 agent router for governance/, mapping the constraints index and layered rule files 00 through 50.
zh_summary: 治理层 `governance/` 的 L0 Agent 路由索引，指向约束总入口、分层规范目录与仓库根总入口。
tags: [governance, index, l0-router, progressive-disclosure]
index: [AGENTS.md, governance/CONSTRAINTS.md, governance/constraints/INDEX.md, governance/constraints/README.md, governance/constraints/00_general.md, governance/constraints/10_bash_and_templates.md, governance/constraints/20_security_and_systemd.md, governance/constraints/30_release_and_vcs.md, governance/constraints/40_testing_and_ci.md, governance/constraints/50_context_and_docs.md]
---

<index>
LAYER:
  name: governance
  root_agent_entry: AGENTS.md

CORE_FILES:
  constraints_entry: governance/CONSTRAINTS.md
  human_constraints_readme: governance/constraints/README.md

SUB_INDEXES:
  constraints: governance/constraints/INDEX.md

LAYERED_DOCS:
  00_general: governance/constraints/00_general.md
  10_bash_and_templates: governance/constraints/10_bash_and_templates.md
  20_security_and_systemd: governance/constraints/20_security_and_systemd.md
  30_release_and_vcs: governance/constraints/30_release_and_vcs.md
  40_testing_and_ci: governance/constraints/40_testing_and_ci.md
  50_context_and_docs: governance/constraints/50_context_and_docs.md
</index>
