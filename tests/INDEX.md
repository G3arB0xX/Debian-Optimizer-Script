---
name: tests-index
description: L0 agent router for tests/, mapping offline host unit tests, container smoke suites, and interactive TUI test harnesses.
zh_summary: 测试目录 `tests/` 的 L0 Agent 路由索引，登记宿主无特权单元测试、容器集成冒烟测试及交互验证子目录。
tags: [tests, ci, unit-tests, smoke-tests, index, l0-router]
index: [AGENTS.md, tests/README.md, tools/INDEX.md, rust/INDEX.md, tests/version-test.sh, tests/release-test.sh, tests/install-bootstrap-test.sh, tests/tmux-test.sh, tests/fail2ban-sync-test.sh, tests/agent-fmt-test.sh, tests/rust-parity-test.sh, tests/e2e-systemd-test.sh, tests/podman-module-test.sh, tests/devops-module-test.sh, tests/freship-module-test.sh, tests/micro-clipboard-test.sh, tests/yazi, tests/podman-test-base]
---

<index>
DIR:
  path: tests
  human_readme: tests/README.md
  root_agent_entry: AGENTS.md

OFFLINE_UNIT_TESTS:
  version: tests/version-test.sh (SemVer + CalVer/Build parsing & metadata validation)
  release: tests/release-test.sh (tools/release.sh plan/bump/verify offline fixture tests)
  install_bootstrap: tests/install-bootstrap-test.sh (install.sh & deb_optimizer.sh bootstrap + prod-strip checks)
  tmux: tests/tmux-test.sh (tmux template & shortcut invariants)
  fail2ban_sync: tests/fail2ban-sync-test.sh (Fail2ban public port auto-sync logic)
  agent_fmt: tests/agent-fmt-test.sh (verify_agent_fmt.py & verify_repo_hygiene.py regression suite)
  rust_parity: tests/rust-parity-test.sh (Pure Rust CLI & debopty-f2b-sync multicall parity vs Bash)

CONTAINER_SMOKE_TESTS:
  e2e_systemd: tests/e2e-systemd-test.sh (real systemd PID 1 Debian container E2E: self_install | tmux | nftables + fail2ban + port sync)
  podman: tests/podman-module-test.sh (rootless Podman install/reinstall/uninstall; PODMAN_TEST_MODE=fast|full)
  devops: tests/devops-module-test.sh (Fish | Micro | Yazi | Lego shared deps; DEVOPS_TEST_MODE=fast|full)
  freship: tests/freship-module-test.sh (FreshIP container integration + tests/freship/smoke.sh)
  micro_clipboard: tests/micro-clipboard-test.sh (Micro Ctrl+C/V & OSC 52 host clipboard sync)

SUBDIRS:
  yazi: tests/yazi/ (tmux-based Yazi keymap, config & Micro editor linkage suite)
  podman_test_base: tests/podman-test-base/ (prebuilt Debian systemd test base Dockerfile & builder)

CROSS_LINKS:
  tools_router: tools/INDEX.md
  testing_rules: governance/constraints/40_testing_and_ci.md
</index>
