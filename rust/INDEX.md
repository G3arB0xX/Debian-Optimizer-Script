---
name: rust-index
description: L0 agent router for rust/, mapping the Pure Rust workspace crates, coexistence bridge, TUI, and CLI binary.
zh_summary: Rust 工作区目录 `rust/` 的 L0 Agent 路由索引，登记 `debopty-core`、`debopty-bridge`、`debopty-tui` 与 `debopty-cli` 四个 crate 及构建校验入口。
tags: [rust, workspace, cli, tui, bridge, core, index, l0-router]
index: [AGENTS.md, docs/12-rust-architecture.md, rust/Cargo.toml, rust/.cargo/config.toml, rust/crates/debopty-core/Cargo.toml, rust/crates/debopty-bridge/Cargo.toml, rust/crates/debopty-tui/Cargo.toml, rust/crates/debopty-cli/Cargo.toml, tests/rust-parity-test.sh]
---

<index>
DIR:
  path: rust
  root_agent_entry: AGENTS.md
  architecture_doc: docs/12-rust-architecture.md
  manifest: rust/Cargo.toml
  cargo_config: rust/.cargo/config.toml (centralized x86_64/aarch64 static musl rust-lld & clang config; loaded when cwd=rust/)

WORKSPACE_CRATES:
  debopty_core: rust/crates/debopty-core/ (version | config | embedded templates | sys | diag | status | telemetry live /proc+specs+module_details+braille | network ureq+rustls | system | security firewall+fail2ban+ssh+f2b-sync | reconcile 9-domain | apps 11-native + self_manage)
  debopty_bridge: rust/crates/debopty-bridge/ (21/21 NativeRust + 0 BashBridge registry | archival archive/legacy-bash discovery for explicit --engine bash | DEBOPTY_FROM_RUST guard)
  debopty_tui: rust/crates/debopty-tui/ (1:1 menu tree | console default renderer | motion ease_out_cubic SSH-aware policy | opt-in ratatui-ui w/ translucent-safe flat slate UiPalette, real telemetry Braille curves, module inspector, mouse hit-testing, Ctrl+Q/micro/yazi keys, U31 collapsible help modal, render_showcase_frame/gallery | TerminalGuard w/ mouse cleanup)
  debopty_cli: rust/crates/debopty-cli/ (debopty binary | debopty-f2b-sync multicall | U27 agent-first schema/AgentEnvelope/--dry-run/--yes/--non-interactive/exit-codes 0..6 | 0-bridge TUI dispatch | --ui console|ratatui|auto | app/self-install/self-update/self-uninstall/ui-preview/reconcile/network/system/security/core/schema subcommands)

BUILD_AND_TEST:
  debug_build: make rust-build (cargo build --manifest-path rust/Cargo.toml --locked)
  workspace_test: make rust-test (default console backend, --locked) | make rust-test-ratatui (--locked --features ratatui-ui)
  static_musl: make rust-musl (cd rust && cargo build --release --locked --target x86_64-unknown-linux-musl)
  static_musl_aarch64: make rust-musl-aarch64 (cd rust && cargo build --release --locked --target aarch64-unknown-linux-musl via rust/.cargo/config.toml)
  static_musl_all: make rust-musl-all (x86_64 + aarch64 static musl binaries via rust/.cargo/config.toml)
  ratatui_feature: --features ratatui-ui (opt-in on debopty-tui & debopty-cli; enabled on release musl binaries)
  ui_preview: debopty ui-preview --demo --screen all (deterministic non-TTY U4 visual showcase)
  parity_suite: bash tests/rust-parity-test.sh (Phase-3 21/21 NativeRust & archived Bash behavioral parity; runs sequentially after run_rust in check.sh)
  scoped_gate: bash tools/check.sh --slice rust (fmt | dual-feature clippy -D warnings | dual-feature cargo test --locked | sequential parity test)

INVARIANTS:
  msrv_and_lock: rust-version = "1.88" in rust/Cargo.toml (edition 2024 + let-chains) | --locked enforced in CI & Makefile | zero literal test secrets (runtime assembly only)
  prod_strip: rust/ & archive/legacy-bash/ excluded from /opt/debopty installs & release tarballs
  template_ssot: debopty-core/build.rs embeds templates/** excluding INDEX.md
  default_cutover: install.sh & /usr/local/bin/debopty default to /opt/debopty/bin/debopty; --engine bash discovers archive/legacy-bash/
  ui_default: console UI remains default; ratatui-ui is opt-in Cargo feature selected via --ui or DEBOPTY_UI
  tls_pure_rust: ureq + rustls (ring + webpki-roots), zero aws-lc-rs or openssl
  offline_mode: set DEBOPTY_CARGO_OFFLINE=1 or pass --offline to build from local crate cache

CROSS_LINKS:
  templates_router: templates/INDEX.md
  legacy_archive_router: archive/legacy-bash/INDEX.md
  tests_router: tests/INDEX.md
  tools_router: tools/INDEX.md
</index>
