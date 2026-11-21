# Legacy Bash 归档目录 (`archive/legacy-bash/`)

[INDEX.md](INDEX.md) | [Rust 架构与迁移文档](../../docs/12-rust-architecture.md) | [仓库总入口](../../AGENTS.md)

本目录归档 `debopty` 完成 Phase-3 全量纯 Rust 重构（21/21 原生模块、0 BashBridge 运行时回退）后退役的旧版纯 Bash 脚本实现（`deb_optimizer.sh` 与 `scripts/`）。

## 归档说明

- **默认入口切换**：自本阶段起，`/usr/local/bin/debopty`（含多调用 `/usr/local/bin/debopty-f2b-sync`）与引导安装脚本 `install.sh` 均默认安装并执行静态链接的纯 Rust 二进制 `/opt/debopty/bin/debopty`。
- **历史兼容与对照用途**：本目录仅保留在源码仓库中，用于版本兼容对照测试（`tests/version-test.sh`、`tests/rust-parity-test.sh` 等）以及用户显式传入 `--engine bash` 或 `debopty bridge` 时的应急回溯。
- **生产环境自动剔除**：生产安装目录 `/opt/debopty` 与 GitHub Release 源码归档包会自动排除 `archive/legacy-bash/`。
- **兼容状态保留**：`templates/`、`/etc/debopti/debopti.conf` 与 `/etc/nftables.d/debopti` 均保持原位且 100% 向后兼容。
