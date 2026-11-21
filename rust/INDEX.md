---
name: rust-index
description: L0 agent router for rust/, mapping the Pure Rust workspace crates, coexistence bridge, TUI, and CLI binary.
zh_summary: Rust 工作区目录 `rust/` 的 L0 Agent 路由索引，登记 `debopty-core`、`debopty-bridge`、`debopty-tui` 与 `debopty-cli` 四个 crate 及构建校验入口。
tags: [rust, workspace, cli, tui, bridge, core, index, l0-router]
index: [AGENTS.md, docs/12-rust-architecture.md, rust/Cargo.toml, rust/crates/debopty-core/Cargo.toml, rust/crates/debopty-bridge/Cargo.toml, rust/crates/debopty-tui/Cargo.toml, rust/crates/debopty-cli/Cargo.toml, tests/rust-parity-test.sh]
---

<index>
DIR:
  path: rust
  root_agent_entry: AGENTS.md
  architecture_doc: docs/12-rust-architecture.md
  manifest: rust/Cargo.toml

WORKSPACE_CRATES:
  debopty_core: rust/crates/debopty-core/ (version | config | embedded templates | sys | status | network ureq+rustls | system | security firewall+fail2ban+ssh+f2b-sync)
  debopty_bridge: rust/crates/debopty-bridge/ (Strangler-Fig BashBridge | 9/20 NativeRust + 11/20 BashBridge registry | DEBOPTY_FROM_RUST guard)
  debopty_tui: rust/crates/debopty-tui/ (1:1 menu tree | console default renderer | opt-in ratatui-ui feature + TerminalGuard | unicode-width CJK layout)
  debopty_cli: rust/crates/debopty-cli/ (debopty binary | debopty-f2b-sync multicall | --ui console|ratatui|auto | network/system/security/core subcommands)

BUILD_AND_TEST:
  debug_build: make rust-build (cargo build --manifest-path rust/Cargo.toml)
  workspace_test: make rust-test (default console backend) | make rust-test-ratatui (--features ratatui-ui)
  static_musl: make rust-musl (x86_64-unknown-linux-musl)
  static_musl_aarch64: make rust-musl-aarch64 (aarch64-unknown-linux-musl via clang + rust-lld)
  static_musl_all: make rust-musl-all (x86_64 + aarch64 static musl binaries)
  ratatui_feature: --features ratatui-ui (opt-in on debopty-tui & debopty-cli)
  parity_suite: bash tests/rust-parity-test.sh (Wave-1 & Wave-2 Bash/Rust behavioral parity)
  scoped_gate: bash tools/check.sh --slice rust (fmt | dual-feature clippy -D warnings | dual-feature cargo test | parity test)

INVARIANTS:
  prod_strip: rust/ excluded from Bash /opt/debopty installs & Bash release tarballs
  template_ssot: debopty-core/build.rs embeds templates/** excluding INDEX.md
  coexistence: DEBOPTY_ENGINE=rust opt-in dispatch with DEBOPTY_FROM_RUST=1 anti-recursion guard
  ui_default: console UI remains default; ratatui-ui is opt-in Cargo feature selected via --ui or DEBOPTY_UI
  tls_pure_rust: ureq + rustls (ring + webpki-roots), zero aws-lc-rs or openssl
  offline_mode: set DEBOPTY_CARGO_OFFLINE=1 or pass --offline to build from local crate cache

CROSS_LINKS:
  templates_router: templates/INDEX.md
  scripts_router: scripts/INDEX.md
  tests_router: tests/INDEX.md
  tools_router: tools/INDEX.md
</index>
