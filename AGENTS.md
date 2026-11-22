---
name: agents-guide
description: Root entrypoint and operational contract for all agents working on debopty (Debian Optimizer Script).
zh_summary: debopty 仓库所有 Agent 的统一总入口：规定项目定位、渐进式阅读顺序、目录路由、常用校验与发布命令、双受众文档契约及不可逾越的工程红线。
tags: [entrypoint, agents, governance, bash, debian, release, red-lines]
index: [.agents/CONSTRAINTS.md, .agents/INDEX.md, .agents/constraints/INDEX.md, .agents/constraints/README.md, .agents/constraints/00_general.md, .agents/constraints/70_agent_workspace_and_credentials.md, rust/INDEX.md, templates/INDEX.md, archive/legacy-bash/INDEX.md, tools/INDEX.md, tests/INDEX.md, docs/INDEX.md, docs/README.md, Makefile]
---

<guide>
PURPOSE:
  repo: G3arB0xX/Debian-Optimizer-Script | debopty (Debian Optimizer Script) | Pure Rust static binary TUI optimizer & automated service deployer for Debian 10–13+ (overridable via DEBOPTY_REPO)
  identity: cli /usr/local/bin/debopty -> /opt/debopty/bin/debopty (multicall /usr/local/bin/debopty-f2b-sync) | install_dir /opt/debopty | legacy shortcut debopti auto-migrated | rules override improvisation (00 §1–§4)
  compat_state: CONFIG_FILE=/etc/debopti/debopti.conf | NFT_CONF_DIR=/etc/nftables.d/debopti | VERSION keys DEBOPTI_SEMVER (primary) + DEBOPTI_CALVER & DEBOPTI_BUILD (compat)
  architecture: entrypoints (install.sh -> /opt/debopty/bin/debopty) -> rust/crates/* (21/21 NativeRust, 0 BashBridge) -> templates/ | archive/legacy-bash/ retired for parity/fallback | docs/ mirrors modules 100% | .agents/ & tools/ & tests/ drive CI & releases

READ_ORDER:
  step_1: AGENTS.md (this file)
  step_2: .agents/CONSTRAINTS.md -> .agents/constraints/README.md -> read 00_general.md + 50_context_and_docs.md + all task-required layers in full
  step_3: domain L0 routers -> .agents/INDEX.md | rust/INDEX.md | templates/INDEX.md | archive/legacy-bash/INDEX.md | tools/INDEX.md | tests/INDEX.md | docs/INDEX.md
  progressive_disclosure: 50_context_and_docs.md -> L0 router (README.md/INDEX.md) -> L1 YAML frontmatter/rg -> L2 bounded leaf shard

ARCHITECTURE:
  entrypoints: install.sh (zero-curl/wget Rust-binary-default bootstrap) | /opt/debopty/bin/debopty (unified CLI + TUI + debopty-f2b-sync multicall)
  rust_engine: rust/INDEX.md -> rust/crates/{debopty-core,debopty-bridge,debopty-tui,debopty-cli} (21/21 NativeRust core/network/system/security/reconcile/11-apps/self_manage + U13 ratatui-ui & ui-preview)
  legacy_archive: archive/legacy-bash/INDEX.md -> archived deb_optimizer.sh & scripts/{common,version,network,system,security,tui}.sh + scripts/apps/*.sh (excluded from /opt/debopty & release tarballs)
  templates: templates/INDEX.md -> templates/{common,system,security,apps}/ (single source of truth embedded via build.rs & rendered via render_template)
  docs_and_ci: docs/INDEX.md | docs/README.md | tools/INDEX.md | tests/INDEX.md | .github/workflows/{check,release}.yml | .github/release.yml
  agents_governance: AGENTS.md | CLAUDE.md | GEMINI.md | .agents/INDEX.md | .agents/CONSTRAINTS.md | .agents/constraints/INDEX.md

COMMANDS:
  env: PYTHONDONTWRITEBYTECODE=1
  make_check: bash tools/check.sh (parallel full gate: syntax | shellcheck | agent_fmt | hygiene | unit_tests | release | rust --locked + sequential rust-parity-test)
  make_check_slice: make check SLICE=`<path>` | bash tools/check.sh --slice `<path>` (fast scoped gate for changed paths; DEBOPTY_SLICE)
  make_test: unprivileged host unit tests (tests/version-test.sh | tests/release-test.sh | tests/install-bootstrap-test.sh | tests/tmux-test.sh | tests/fail2ban-sync-test.sh | tests/agent-fmt-test.sh | tests/rust-parity-test.sh)
  rust_build: make rust-build | make rust-test | make rust-test-ratatui | make rust-musl | make rust-musl-aarch64 | make rust-musl-all (--locked; rust/.cargo/config.toml musl cross-linking via cwd=rust/)
  make_lint: bash -n | shellcheck (if installed) | python3 -B tools/verify_repo_hygiene.py (incl. working-tree secret-literal & U29 credential hygiene scan)
  make_hygiene_history: make hygiene-history | python3 -B tools/verify_repo_hygiene.py --scan-git-history HEAD (full Git history secret scan)
  make_check_docs: python3 -B tools/verify_agent_fmt.py (agent-only & human-facing format + bidirectional links; supports --root `<workspace>`)
  make_release_plan: bash tools/release.sh plan [--json] (preview next SemVer & changelog)
  make_release_verify: bash tools/release.sh verify [--check-commits] (verify VERSION, VERSION_ID, CHANGELOG.md)
  make_release_check: make release-check (tools/check.sh --strict-shellcheck --strict-rust && tools/release.sh verify)
  release_bump: bash tools/release.sh bump --bump auto|major|minor|patch | --version X.Y.Z | --prerelease `<id>` | --dry-run
  make_logs: inspect check logs in DEBOPTY_LOG_DIR (default /tmp/debopty_logs)
  podman_smoke: bash tests/podman-module-test.sh (dev container smoke; PODMAN_TEST_MODE=full for reinstall/uninstall)
  sanitized_push: bash tools/github-push.sh (ONLY sanctioned path to push sanitized main & SemVer tags + local tag sync; release.yml is sole Release publisher)

AUDIENCE_CONTRACT:
  human_facing: README.md | README_CN.md | docs/*.md | tests/README.md | .agents/constraints/{README.md,[0-9][0-9]_*.md} | .agents/memory/*.md | MEMORY.md -> concise Simplified Chinese | 0 agent XML tags | valid relative links | bidirectional README.md <-> INDEX.md where both exist (00 §5, 50 §3, 70 §4)
  agent_only: AGENTS.md | CLAUDE.md | GEMINI.md | .agents/CONSTRAINTS.md | **/INDEX.md -> line-1 YAML frontmatter (name | description | zh_summary | tags | index) + 1 blank line + single outer XML boundary + ALL-CAPS SECTION: + 2-space unquoted KV & pipe enums | 0 tables/headings/bullets/nested XML (00 §5, 70 §4)
  user_interaction: all PM replies, proposals, task lists & walkthroughs MUST use natural, concise Simplified Chinese (00 §1)

GIT_AND_RED_LINES:
  commit_format: 30 §3 -> Conventional Commits 1.0.0 `<type>[(<scope>)][!]: <summary>` (no commas in scope) | Keep a Changelog 1.1.0
  release_gate: 30 §2–§5 -> autonomous SemVer 2.0.0 via tools/release.sh | publish ONLY via tools/github-push.sh (tag-only + local tag sync; NEVER jj git push or git push origin) | .github/workflows/release.yml is sole atomic Release publisher | NEVER hand-edit VERSION, VERSION_ID, CHANGELOG.md
  secret_hygiene: 30 §6, 70 §3 -> 0 literal PEM/token/JWT/URL-cred secrets or forbidden credential files in source, tests or docs (runtime assembly only) | verify_repo_hygiene.py WT + --scan-git-history HEAD + --check-workspace-perms gate
  bash_strict: 10 §1 -> Pure Bash with set -euo pipefail | double-quote all variables | timeout & error capture on external calls | 0 placeholders or TODO stubs
  template_only: 10 §2 -> 0 inline config Here-Docs in scripts | all system/app configs live in templates/ and deploy via render_template
  security_systemd: 20 §3–§6, 70 §3 -> ProtectSystem=full | PrivateTmp=true | NoNewPrivileges=true | non-root AmbientCapabilities=CAP_NET_BIND_SERVICE | Fail2ban backend=systemd + python3-systemd + auto port sync | 0700/0600 credential store & LoadCredential/EnvironmentFile
  prod_purity: 30 §1, 70 §2 -> 0 jj | jujutsu | Change ID | dev terminology in prod UI, logs or runtime messages | strip .agents/ (and legacy governance/) & **/INDEX.md & dev metadata on /opt/debopty install & release tarballs | runtime VPS agent workspace lives separately at /var/lib/debopty/agent
  never_commit: __pycache__/ | *.pyc | *.pyo | .cache/ | private secrets | scratch/swarm state
  date_ban: 50 §4, 70 §4 -> 0 modification dates (YYYY-MM-DD) in .agents/CONSTRAINTS.md or .agents/constraints/*.md
  platform_neutrality: 50 §5 -> keep generic code & docs free of vendor-specific IDE/debug tokens | keep scratch & swarm state strictly outside repo
</guide>
