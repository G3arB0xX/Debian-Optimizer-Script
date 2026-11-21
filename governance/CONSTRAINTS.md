---
name: constraints-index
description: Index of layered constraint documents in governance/constraints/00..60.
zh_summary: debopty 约束总索引入口，指引 Agent 先读 `00_general.md`，再按任务类型加载 `governance/constraints/` 下 `00` 至 `60` 号分层规范文件。
tags: [constraints, index, governance]
index: [governance/constraints/README.md, governance/constraints/INDEX.md, governance/constraints/00_general.md, governance/constraints/60_rust_and_reconcile.md, AGENTS.md]
---

<constraints_index>
POINTER:
  dir: governance/constraints/
  read_order: read 00_general.md first | then task-required layers listed in governance/constraints/README.md

LAYERS:
  00_general.md: role & PM profile | zero placeholders | plan mode | code continuity | concise Chinese style | debopty project identity & Debian 10–13+ scope | dual-audience doc contract
  10_bash_and_templates.md: set -euo pipefail | defensive quoting | ensure_runtime_home | templates/ + render_template rule (zero inline Here-Docs) | idempotency & atomic undo | zero-curl/wget bootstrap | Windows/VS Code/Micro terminal shortcut standard (Fish/Yazi/Micro/tmux) | core architecture & app module invariants
  20_security_and_systemd.md: SSH hardening & rollback | nftables /etc/nftables.d/debopti modular firewall | Fail2ban best practices (backend=systemd + python3-systemd + nftables-allports + [recidive] + all public open ports & auto sync) | systemd sandboxing & least privilege | rootless Podman apps user & debopti-certs group
  30_release_and_vcs.md: jj + git workflow | prod purity (zero dev terms) | SemVer 2.0.0 + backward-compatible CalVer/Build (DEBOPTI_SEMVER/DEBOPTI_CALVER/DEBOPTI_BUILD) | Conventional Commits 1.0.0 & Keep a Changelog 1.1.0 | agent autonomous release SOP via tools/release.sh | sanitized GitHub push protocol Stages A–E via tools/github-push.sh & .github/workflows/release.yml
  40_testing_and_ci.md: make check / tools/check.sh & make release-verify local & GitHub Actions CI gate (check.yml + release.yml) | unprivileged host unit tests (tests/*-test.sh incl. release-test.sh & agent-fmt-test.sh) | adversarial multi-env & real-world chaos regression | Podman container smoke tests & nested container limitations
  50_context_and_docs.md: L0->L1->L2 progressive disclosure | sharding threshold (>15 files or >300–500L) | incremental docs/ manual tutorial sync | date ban in governance | platform neutrality
  60_rust_and_reconcile.md: pure Rust 4-crate & ureq+rustls static musl + Strangler-Fig coexistence & archival | dual-backend TUI (console + ratatui-ui), RAII TerminalGuard & 0x0/FE0F safety | audit-by-default dirty-system reconcile, scoped nft flush & atomic restore | non-destructive user-custom preservation | atomic mode inheritance & non-TTY fail-closed | E1xxx..E6xxx diag codes, stdout/stderr separation & secret redaction
</constraints_index>
