---
name: agents-guide
description: Root entrypoint and operational contract for all agents working on debopty (Debian Optimizer Script).
zh_summary: debopty 仓库所有 Agent 的统一总入口：规定项目定位、渐进式阅读顺序、目录路由、常用校验与发布命令、双受众文档契约及不可逾越的工程红线。
tags: [entrypoint, governance, bash, debian, release, red-lines]
index: [governance/CONSTRAINTS.md, governance/INDEX.md, governance/constraints/INDEX.md, governance/constraints/README.md, governance/constraints/00_general.md, scripts/INDEX.md, templates/INDEX.md, tools/INDEX.md, tests/INDEX.md, docs/INDEX.md, docs/README.md, Makefile]
---

<guide>
PURPOSE:
  repo: G3arB0xX/Debian-Optimizer-Script | debopty (Debian Optimizer Script) | Pure Bash TUI optimizer & automated service deployer for Debian 10–13+ (overridable via DEBOPTY_REPO)
  identity: cli /usr/local/bin/debopty | install_dir /opt/debopty | legacy shortcut debopti auto-migrated | rules override improvisation (00 §1–§4)
  compat_state: CONFIG_FILE=/etc/debopti/debopti.conf | NFT_CONF_DIR=/etc/nftables.d/debopti | VERSION keys DEBOPTI_SEMVER (primary) + DEBOPTI_CALVER & DEBOPTI_BUILD (compat)
  architecture: entrypoints (deb_optimizer.sh | install.sh) -> scripts/ -> templates/ | docs/ mirrors scripts 100% | governance/ & tools/ & tests/ drive CI & releases

READ_ORDER:
  step_1: AGENTS.md (this file)
  step_2: governance/CONSTRAINTS.md -> governance/constraints/README.md -> read 00_general.md + 50_context_and_docs.md + all task-required layers in full
  step_3: domain L0 routers -> governance/INDEX.md | scripts/INDEX.md | templates/INDEX.md | tools/INDEX.md | tests/INDEX.md | docs/INDEX.md
  progressive_disclosure: 50_context_and_docs.md -> L0 router (README.md/INDEX.md) -> L1 YAML frontmatter/rg -> L2 bounded leaf shard

ARCHITECTURE:
  entrypoints: deb_optimizer.sh (main launcher & self-install) | install.sh (zero-curl/wget bootstrap installer)
  core_modules: scripts/INDEX.md -> scripts/{common,version,network,system,security,tui}.sh
  app_modules: scripts/apps/{devops,easytier,ferron,freship,golang,podman,realm,rust,tailscale,warp,xray}.sh
  templates: templates/INDEX.md -> templates/{common,system,security,apps}/ (single source of truth rendered via render_template)
  docs_and_ci: docs/INDEX.md | docs/README.md | tools/INDEX.md | tests/INDEX.md | .github/workflows/{check,release}.yml | .github/release.yml
  governance: AGENTS.md | CLAUDE.md | GEMINI.md | governance/INDEX.md | governance/CONSTRAINTS.md | governance/constraints/INDEX.md

COMMANDS:
  env: PYTHONDONTWRITEBYTECODE=1
  make_check: bash tools/check.sh (parallel full gate: syntax | shellcheck | agent_fmt | hygiene | unit_tests | release)
  make_check_slice: make check SLICE=`<path>` | bash tools/check.sh --slice `<path>` (fast scoped gate for changed paths; DEBOPTY_SLICE)
  make_test: unprivileged host unit tests (tests/version-test.sh | tests/release-test.sh | tests/install-bootstrap-test.sh | tests/tmux-test.sh | tests/fail2ban-sync-test.sh | tests/agent-fmt-test.sh)
  make_lint: bash -n | shellcheck (if installed) | python3 -B tools/verify_repo_hygiene.py
  make_check_docs: python3 -B tools/verify_agent_fmt.py (agent-only & human-facing format + bidirectional links)
  make_release_plan: bash tools/release.sh plan [--json] (preview next SemVer & changelog)
  make_release_verify: bash tools/release.sh verify [--check-commits] (verify VERSION, VERSION_ID, CHANGELOG.md)
  release_bump: bash tools/release.sh bump --bump auto|major|minor|patch | --version X.Y.Z | --prerelease `<id>` | --dry-run
  make_logs: inspect check logs in DEBOPTY_LOG_DIR (default /tmp/debopty_logs)
  podman_smoke: bash tests/podman-module-test.sh (dev container smoke; PODMAN_TEST_MODE=full for reinstall/uninstall)
  sanitized_push: bash tools/github-push.sh (ONLY sanctioned path to publish GitHub main & SemVer tags)

AUDIENCE_CONTRACT:
  human_facing: README.md | README_CN.md | docs/*.md | tests/README.md | governance/constraints/{README.md,[0-9][0-9]_*.md} -> concise Simplified Chinese | 0 agent XML tags | valid relative links | bidirectional README.md <-> INDEX.md where both exist (00 §5, 50 §3)
  agent_only: AGENTS.md | CLAUDE.md | GEMINI.md | governance/CONSTRAINTS.md | **/INDEX.md -> line-1 YAML frontmatter (name | description | zh_summary | tags | index) + 1 blank line + single outer XML boundary + ALL-CAPS SECTION: + 2-space unquoted KV & pipe enums | 0 tables/headings/bullets/nested XML (00 §5)
  user_interaction: all PM replies, proposals, task lists & walkthroughs MUST use natural, concise Simplified Chinese (00 §1)

GIT_AND_RED_LINES:
  commit_format: 30 §3 -> Conventional Commits 1.0.0 `<type>[(<scope>)][!]: <summary>` (no commas in scope) | Keep a Changelog 1.1.0
  release_gate: 30 §2–§5 -> autonomous SemVer 2.0.0 via tools/release.sh | publish ONLY via tools/github-push.sh (NEVER jj git push or git push origin) | NEVER hand-edit VERSION, VERSION_ID, CHANGELOG.md
  bash_strict: 10 §1 -> Pure Bash with set -euo pipefail | double-quote all variables | timeout & error capture on external calls | 0 placeholders or TODO stubs
  template_only: 10 §2 -> 0 inline config Here-Docs in scripts | all system/app configs live in templates/ and deploy via render_template
  security_systemd: 20 §3–§5 -> ProtectSystem=full | PrivateTmp=true | NoNewPrivileges=true | non-root AmbientCapabilities=CAP_NET_BIND_SERVICE | Fail2ban backend=systemd + python3-systemd + auto port sync
  prod_purity: 30 §1 -> 0 jj | jujutsu | Change ID | dev terminology in prod UI, logs or runtime messages | strip governance/ & **/INDEX.md & dev metadata on /opt/debopty install & release tarballs
  never_commit: __pycache__/ | *.pyc | *.pyo | .cache/ | private secrets | scratch/swarm state
  date_ban: 50 §4 -> 0 modification dates (YYYY-MM-DD) in governance/CONSTRAINTS.md or governance/constraints/*.md
  platform_neutrality: 50 §5 -> keep generic code & docs free of vendor-specific IDE/debug tokens | keep scratch & swarm state strictly outside repo
</guide>
