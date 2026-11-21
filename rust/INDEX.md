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
  debopty_core: rust/crates/debopty-core/ (version | config | embedded templates | sys | status | native f2b-sync)
  debopty_bridge: rust/crates/debopty-bridge/ (Strangler-Fig BashBridge | static action dispatch | DEBOPTY_FROM_RUST guard | module registry)
  debopty_tui: rust/crates/debopty-tui/ (1:1 menu tree | unicode-width CJK box layout | pluggable TuiRenderer | non-TTY auto-exit)
  debopty_cli: rust/crates/debopty-cli/ (debopty binary | debopty-f2b-sync multicall | subcommands | assert_cmd integration tests)

BUILD_AND_TEST:
  debug_build: make rust-build (cargo build --manifest-path rust/Cargo.toml)
  workspace_test: make rust-test (cargo test --manifest-path rust/Cargo.toml)
  static_musl: make rust-musl (cargo build --manifest-path rust/Cargo.toml --release --target x86_64-unknown-linux-musl)
  parity_suite: bash tests/rust-parity-test.sh (version | config | template | f2b-sync multicall | CI non-TTY parity vs Bash)
  scoped_gate: bash tools/check.sh --slice rust (fmt | clippy -D warnings | cargo test | parity test)

INVARIANTS:
  prod_strip: rust/ excluded from Bash /opt/debopty installs & Bash release tarballs
  template_ssot: debopty-core/build.rs embeds templates/** excluding INDEX.md
  coexistence: DEBOPTY_ENGINE=rust opt-in dispatch with DEBOPTY_FROM_RUST=1 anti-recursion guard
  offline_mode: set DEBOPTY_CARGO_OFFLINE=1 or pass --offline to build from local crate cache

CROSS_LINKS:
  templates_router: templates/INDEX.md
  scripts_router: scripts/INDEX.md
  tests_router: tests/INDEX.md
  tools_router: tools/INDEX.md
</index>
