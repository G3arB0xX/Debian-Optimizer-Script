---
name: tools-index
description: L0 agent router for tools/, mapping CI gates, format/hygiene linters, release automation, sanitized push scripts, and environment variables.
zh_summary: 工具目录 `tools/` 的 L0 Agent 路由索引，登记质量门禁、文档与仓库卫生校验器、自主发布工具、脱敏推送脚本及环境变量清单。
tags: [tools, ci, lint, release, index, l0-router]
index: [AGENTS.md, Makefile, tools/check.sh, tools/verify_agent_fmt.py, tools/verify_repo_hygiene.py, tools/release.sh, tools/release_helper.py, tools/github-push.sh, tools/github-ssh-cred-setup.sh]
---

<index>
DIR:
  path: tools
  root_agent_entry: AGENTS.md
  makefile: Makefile

GATES_AND_LINTERS:
  check: tools/check.sh (parallel full gate w/ sequential rust-parity-test after run_rust | --slice `<path>` | --check-commits [RANGE] | tail-120 failure logs & JSONL summary)
  agent_fmt: tools/verify_agent_fmt.py (agent-only XML/KV & human-facing format + bidirectional README/INDEX & relative link checks; --root `<dir>`)
  repo_hygiene: tools/verify_repo_hygiene.py (0-pycache | vendor neutrality incl. **/INDEX.md | working-tree secret-literal scan & --scan-git-history [REF] | --root `<dir>`)

RELEASE_AND_PUSH:
  release_cli: tools/release.sh (plan | bump | verify | SemVer 2.0.0 + Keep a Changelog 1.1.0)
  release_helper: tools/release_helper.py (Python stdlib Conventional Commits parser & VERSION/CHANGELOG updater)
  github_push: tools/github-push.sh (ONLY sanctioned sanitized push path Stages A–E | git-filter-repo history scrub + --scan-git-history HEAD gate | tag-only push & _sync_local_tag; release.yml is sole Release publisher)
  ssh_cred_setup: tools/github-ssh-cred-setup.sh (non-interactive GitHub SSH credential bootstrap)

ENV_VARS:
  DEBOPTY_JOBS: parallel worker count for tools/check.sh (default auto)
  DEBOPTY_SLICE: scoped path filter for tools/check.sh / make check SLICE=`<path>`
  DEBOPTY_LOG_DIR: check log directory (default /tmp/debopty_logs)
  DEBOPTY_REPO: override default GitHub repo G3arB0xX/Debian-Optimizer-Script
  DEBOPTY_GITHUB_REMOTE: override target remote name for tools/github-push.sh
  PYTHONDONTWRITEBYTECODE: 1 (prevent __pycache__ & *.pyc creation)

CROSS_LINKS:
  tests_router: tests/INDEX.md
  release_rules: governance/constraints/30_release_and_vcs.md
  testing_rules: governance/constraints/40_testing_and_ci.md
</index>
