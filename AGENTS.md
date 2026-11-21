---
name: debopty-agents-guide
description: Root entrypoint and operational contract for all agents working on debopty (Debian Optimizer Script).
zh_summary: debopty 仓库所有 Agent 的统一总入口：定义项目定位、渐进式阅读顺序、常用校验与发布命令、代码架构、双受众文档契约与不可逾越的工程红线。
tags: [entrypoint, governance, bash, debian, red-lines]
index: [instructions/CONSTRAINTS.md, instructions/constraints/00_general.md, docs/README.md, Makefile]
---

<guide>
PURPOSE:
  project: debopty (`Debian Optimizer Script`) | Pure Bash TUI optimizer & automated service deployer for Debian 10–13+
  cli_paths: command `/usr/local/bin/debopty` | install dir `/opt/debopty` | legacy shortcut `debopti` auto-migrated
  compat_state: `CONFIG_FILE=/etc/debopti/debopti.conf` | `NFT_CONF_DIR=/etc/nftables.d/debopti` | `VERSION` keys `DEBOPTI_SEMVER` (primary) + backward-compatible `DEBOPTI_CALVER` & `DEBOPTI_BUILD` | default repo `G3arB0xX/Debian-Optimizer-Script` (overridable via `DEBOPTY_REPO`)

READ_ORDER:
  step_1: AGENTS.md (this file)
  step_2: instructions/CONSTRAINTS.md -> instructions/constraints/README.md -> read `00_general.md` + all task-required layers in full
  step_3: instructions/decisions.md for historical rulings & context
  progressive_disclosure: `50_context_and_docs.md` | L0 router (`README.md`/`INDEX.md`) -> L1 YAML/search -> L2 bounded leaf shard

COMMANDS:
  verify_all: make check (`bash tools/check.sh`)
  unit_tests: make test (`tests/version-test.sh` | `tests/release-test.sh` | `tests/install-bootstrap-test.sh` | `tests/tmux-test.sh` | `tests/fail2ban-sync-test.sh`)
  static_lint: make lint (`bash -n` | `shellcheck` | `tools/verify_agent_fmt.py` | `tools/verify_repo_hygiene.py`)
  release_plan: make release-plan (`bash tools/release.sh plan [--json]`)
  release_bump: bash tools/release.sh bump (`--bump auto|major|minor|patch` | `--version X.Y.Z` | `--prerelease <id>` | `--dry-run`)
  release_verify: make release-verify (`bash tools/release.sh verify [--check-commits]`)
  view_logs: make logs
  podman_smoke: bash tests/podman-module-test.sh (dev container smoke test; `PODMAN_TEST_MODE=full` for reinstall/uninstall)
  sanitized_push: bash instructions/github-push.sh (ONLY sanctioned path to publish to GitHub `main` & SemVer tags)

ARCHITECTURE:
  entrypoints: `deb_optimizer.sh` (main launcher & self-install) | `install.sh` (zero-curl/wget bootstrap installer)
  core_modules: `scripts/common.sh` | `scripts/version.sh` | `scripts/network.sh` | `scripts/system.sh` | `scripts/security.sh` | `scripts/tui.sh`
  app_modules: `scripts/apps/*.sh` (podman | xray | warp | devops | realm | ferron | golang | rust | easytier | tailscale | freship)
  templates: `templates/` (single source of truth for all system/app configs rendered via `render_template`)
  docs_and_ci: `docs/` (manual tutorials 100% aligned with scripts) | `tools/` | `tests/` | `.github/workflows/{check,release}.yml` | `.github/release.yml`
  governance: `AGENTS.md` | `CLAUDE.md` | `GEMINI.md` | `instructions/CONSTRAINTS.md` | `instructions/constraints/` | `instructions/decisions.md`

AUDIENCE_CONTRACT:
  agent_only: `AGENTS.md` | `CLAUDE.md` | `GEMINI.md` | `instructions/CONSTRAINTS.md` | `**/INDEX.md` -> line-1 YAML frontmatter (`name` | `description` | `zh_summary` | `tags` | `index`) + 1 blank line + single outer XML boundary + 2-space unquoted KV + pipe enums | 0 Markdown tables | 0 nested XML
  human_facing: `README*.md` | `docs/*.md` | `instructions/constraints/README.md` | `instructions/constraints/[0-9][0-9]_*.md` | `instructions/decisions.md` -> concise Simplified Chinese | 0 agent XML tags | valid relative links
  user_interaction: all PM replies, plans, task lists & walkthroughs MUST be written in Simplified Chinese

RED_LINES:
  bash_strict: Pure Bash with `set -euo pipefail` | double-quote all variables | timeout & error capture on external calls | zero placeholders or TODO stubs
  template_only: ZERO inline config Here-Docs in scripts | all system/app configs MUST live in `templates/` and deploy via `render_template`
  prod_purity: ZERO `jj` | `jujutsu` | Change ID | dev terminology in prod UI, logs or runtime messages | strip dev metadata when installing to `/opt/debopty`
  security_systemd: all managed units require `ProtectSystem=full` | `PrivateTmp=true` | `NoNewPrivileges=true` | non-root services with `AmbientCapabilities=CAP_NET_BIND_SERVICE` | Fail2ban uses `backend=systemd` + `python3-systemd` (survives `rsyslog` purge) + auto port sync
  release_gate: agents autonomously manage versioning & releases via `tools/release.sh` (SemVer 2.0.0 + Conventional Commits 1.0.0 + Keep a Changelog 1.1.0) | publish ONLY via `instructions/github-push.sh` (NEVER `jj git push` or `git push origin`) | NEVER manually hand-edit `VERSION` | `VERSION_ID` | `CHANGELOG.md`
  date_ban: ZERO modification dates (`YYYY-MM-DD`) in `instructions/CONSTRAINTS.md` or `instructions/constraints/*.md` | record dates ONLY in `instructions/decisions.md`
  platform_neutrality: keep generic code & docs free of vendor-specific IDE/debug tokens | keep scratch & swarm state strictly outside repo
</guide>
