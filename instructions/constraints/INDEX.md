---
name: debopty-constraints-router
description: L0 agent router for instructions/constraints/, listing layered rules 00 through 50 in Simplified Chinese.
zh_summary: 分层约束目录 `instructions/constraints/` 的 L0 Agent 路由索引，登记 `00` 至 `50` 全部中文约束文件及其关联索引。
tags: [constraints, index, l0-router, rules]
index: [instructions/CONSTRAINTS.md, instructions/constraints/README.md, instructions/constraints/00_general.md, instructions/constraints/10_bash_and_templates.md, instructions/constraints/20_security_and_systemd.md, instructions/constraints/30_release_and_vcs.md, instructions/constraints/40_testing_and_ci.md, instructions/constraints/50_context_and_docs.md, instructions/decisions.md]
---

<index>
DIR:
  path: instructions/constraints
  human_readme: instructions/constraints/README.md
  parent_index: instructions/CONSTRAINTS.md

ALWAYS_READ:
  general: instructions/constraints/00_general.md
  context_and_docs: instructions/constraints/50_context_and_docs.md

LAYERED_RULES:
  bash_and_templates: instructions/constraints/10_bash_and_templates.md
  security_and_systemd: instructions/constraints/20_security_and_systemd.md
  release_and_vcs: instructions/constraints/30_release_and_vcs.md
  testing_and_ci: instructions/constraints/40_testing_and_ci.md

CROSS_LINKS:
  root_guide: AGENTS.md
  decisions: instructions/decisions.md
  docs_manual: docs/README.md
</index>
