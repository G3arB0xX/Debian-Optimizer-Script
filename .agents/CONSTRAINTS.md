---
name: constraints-index
description: Index of layered constraint documents in .agents/constraints/00..80.
zh_summary: debopty 约束总索引入口，指引 Agent 先读 `00_general.md`，再按任务类型加载 `.agents/constraints/` 下 `00` 至 `80` 号分层规范文件。
tags: [constraints, index, agents]
index: [.agents/constraints/README.md, .agents/constraints/INDEX.md, .agents/constraints/00_general.md, .agents/constraints/60_rust_and_reconcile.md, .agents/constraints/70_agent_workspace_and_credentials.md, .agents/constraints/80_swarm_orchestration.md, AGENTS.md]
---

<constraints_index>
POINTER:
  dir: .agents/constraints/
  read_order: read 00_general.md first | then task-required layers listed in .agents/constraints/README.md

LAYERS:
  00_general.md: role & PM profile | zero placeholders | plan mode | code continuity | concise Chinese style | debopty project identity & Debian 10–13+ scope | dual-audience doc contract
  10_bash_and_templates.md: set -euo pipefail | defensive quoting | ensure_runtime_home | templates/ + render_template rule (zero inline Here-Docs) | idempotency & atomic undo | zero-curl/wget bootstrap | Windows/VS Code/Micro terminal shortcut standard (Fish/Yazi/Micro/tmux) | core architecture & app module invariants
  20_security_and_systemd.md: SSH hardening & rollback | nftables /etc/nftables.d/debopti modular firewall | Fail2ban best practices (backend=systemd + python3-systemd + nftables-allports + [recidive] + all public open ports & auto sync) | systemd sandboxing & least privilege | rootless Podman apps user & debopti-certs group | credential storage baseline (70 §3)
  30_release_and_vcs.md: jj + git workflow | prod purity (zero dev terms) | SemVer 2.0.0 + backward-compatible CalVer/Build (DEBOPTI_SEMVER/DEBOPTI_CALVER/DEBOPTI_BUILD) | Conventional Commits 1.0.0 & Keep a Changelog 1.1.0 | tools/release.sh autonomous SOP | single Release publisher (.github/workflows/release.yml) & tag-only github-push.sh (git-filter-repo + --scan-git-history HEAD + _sync_local_tag + 0 literal secrets)
  40_testing_and_ci.md: make check / tools/check.sh (--locked Rust + sequential rust-parity-test + tail-120 failure logs) & make hygiene-history / release-check | pinned RUST_TOOLCHAIN + Swatinem/rust-cache + PR commit lint (check.yml + release.yml + rust/.cargo/config.toml musl) | unprivileged host tests | adversarial multi-env regression | Podman smoke tests & nested container limits
  50_context_and_docs.md: L0->L1->L2 progressive disclosure | sharding threshold (>15 files or >300–500L) | incremental docs/ manual tutorial sync | date ban in .agents constraints | platform neutrality
  60_rust_and_reconcile.md: pure Rust 4-crate & ureq+rustls static musl + Strangler-Fig coexistence & archival | dual-backend TUI (console + ratatui-ui), RAII TerminalGuard & 0x0/FE0F safety | audit-by-default dirty-system reconcile, scoped nft flush & atomic restore | non-destructive user-custom preservation | atomic mode inheritance & non-TTY fail-closed | E1xxx..E6xxx diag codes, stdout/stderr separation & secret redaction
  70_agent_workspace_and_credentials.md: U27 Agent-First CLI envelope, dry-run/schema & exit codes 21..26 | U28 v2 VPS agent workspace (/var/lib/debopty/agent), CoALA 3-tier memory, fail-closed Write Gate, BM25 index, --memory-tracking local|tracked & 3-way SHA-256 upgrade | U29 0700/0600 credential store, systemd injection & redaction | U30 dual-audience docs & L0->L1->L2 | harness-agnostic IR (14 adapters, .agents/overrides/) & lossless audit/migrate/rollback
  80_swarm_orchestration.md: U32 multi-agent engineering invariants | contract-first disjoint write slices | readonly JIT audit & path-scoped atomic commits | Cargo CARGO_TARGET_DIR/CARGO_HOME isolation | runtime test secret assembly & git-history hygiene gate | multi-round red-vs-blue & chaos sim | external /tmp scratch outside repo
</constraints_index>
