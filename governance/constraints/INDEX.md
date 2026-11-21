---
name: constraints-router
description: L0 agent router for governance/constraints/, listing layered rules 00 through 50 in Simplified Chinese.
zh_summary: 分层约束目录 `governance/constraints/` 的 L0 Agent 路由索引，登记 `00` 至 `50` 全部人读中文约束文件及其按任务加载映射。
tags: [constraints, index, l0-router, rules]
index: [governance/INDEX.md, governance/CONSTRAINTS.md, governance/constraints/README.md, governance/constraints/00_general.md, governance/constraints/10_bash_and_templates.md, governance/constraints/20_security_and_systemd.md, governance/constraints/30_release_and_vcs.md, governance/constraints/40_testing_and_ci.md, governance/constraints/50_context_and_docs.md]
---

<index>
DIR:
  path: governance/constraints
  human_readme: governance/constraints/README.md
  parent_index: governance/INDEX.md

ALWAYS_READ:
  general: governance/constraints/00_general.md
  context_and_docs: governance/constraints/50_context_and_docs.md

LAYERED_RULES:
  bash_and_templates: governance/constraints/10_bash_and_templates.md
  security_and_systemd: governance/constraints/20_security_and_systemd.md
  release_and_vcs: governance/constraints/30_release_and_vcs.md
  testing_and_ci: governance/constraints/40_testing_and_ci.md

TASK_ROUTING:
  scripts_and_templates: 00 + 10 + 40 + 50 (add 20 for ports/units/perms)
  security_and_firewall: 00 + 10 + 20 + 40 + 50
  release_and_push: 00 + 30 + 40
  tests_and_ci: 00 + 40 + 50 (add 30 for release filters)
  docs_and_governance: 00 + 50 (add affected domain layer)

CROSS_LINKS:
  root_guide: AGENTS.md
  docs_manual: docs/README.md
  makefile: Makefile
  check_gate: tools/check.sh
  release_cli: tools/release.sh
</index>
