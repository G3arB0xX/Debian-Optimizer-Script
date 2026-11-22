# 40 测试、本地门禁与 CI 规范

[上一层：30 发布与版本管理](30_release_and_vcs.md) | [约束索引](README.md) | [下一层：50 上下文与文档治理](50_context_and_docs.md)

本层规定本地一键门禁、GitHub Actions CI、宿主无特权单元测试以及 Podman 模块容器测试的执行标准。

## §1 本地门禁与 GitHub Actions CI

1. **统一入口（`Makefile`）**：
   - `make check`：调用 `bash tools/check.sh` 并发执行全套静态校验（含 `bash -n` 与 `tools/*.py` 字节码校验）、`shellcheck`、轻量宿主单元测试、双特性 Rust 编译测试（`default` 与 `--features ratatui-ui`，强制 `--locked`，见 [60 §2](60_rust_and_reconcile.md)）、双受众格式检查、仓库卫生与工作树秘密字面量扫描（`verify_repo_hygiene.py`）及 `tools/release.sh verify` 发布一致性门禁（支持可选 `--check-commits`），并输出结构化 JSONL 日志（含 `release_rc`）。为消除共享 `rust/target` 导致的二进制特性覆盖与 `ETXTBSY` 竞态，`tests/rust-parity-test.sh` 必须在 `run_rust` 完成后**串行执行**；多代理或并行作业并发执行 `cargo` 时须通过仓库外部 `CARGO_TARGET_DIR`（必要时配合 `CARGO_HOME`）隔离构建目录（见 `60 §1`、`80 §3`）；任意子步骤失败时输出完整末尾 120 行诊断日志。
   - `make lint`：执行全仓 `bash -n` 语法校验、`shellcheck`（本地未安装时告警跳过，GitHub Actions CI 中为强制阻断项）、`python3 -B tools/verify_agent_fmt.py`（双受众格式门禁）与 `python3 -B tools/verify_repo_hygiene.py`（仓库卫生、平台中立性与工作树秘密字面量门禁）。
   - `make hygiene-history`：执行 `python3 -B tools/verify_repo_hygiene.py --scan-git-history HEAD`，扫描当前 Git 分支全量历史 blob 中的秘密字面量残留。
   - `make test`：运行全部宿主无特权单元测试。
   - `make rust-musl` / `make rust-musl-aarch64` / `make rust-musl-all`：在 `rust/` 目录上下文执行 `cargo build --release --locked`，自动加载 `rust/.cargo/config.toml` 中的 `rust-lld` 静态 musl 交叉链接配置。
   - `make release-plan` / `make release-verify` / `make release-check`：预览下一 SemVer 版本与 Changelog、校验 `VERSION`/`VERSION_ID`/`CHANGELOG.md` 一致性，或一键执行严格全门禁加发布一致性校验。
   - `make logs`：查看最近一次门禁运行日志。
2. **GitHub Actions CI/CD**：
   - `.github/workflows/check.yml` 在 `push`、`pull_request` 与手动触发时使用固定 `RUST_TOOLCHAIN`（配合 `rust/Cargo.toml` 中 `rust-version = "1.88"` 与 `--locked` 构建）、`Swatinem/rust-cache@v2` 缓存及 `fetch-depth` 历史检出，执行 PR/HEAD Conventional Commits 校验（`release.sh verify --check-commits`）与 `bash tools/check.sh --strict-shellcheck --strict-rust`，失败时自动输出 `/tmp/debopty_logs/` 日志并写入 `$GITHUB_STEP_SUMMARY`。
   - `.github/workflows/release.yml` 在推送 SemVer tag（`vX.Y.Z*`）或手动触发时使用固定 `RUST_TOOLCHAIN` 与 `Swatinem/rust-cache@v2` 复跑严格门禁、`tools/release.sh verify` 与 `--scan-git-history HEAD`，生成洁净生产资产包 `debopty-vX.Y.Z.tar.gz`（并校验排除清单）；在 `build-rust` 中分别为 `x86_64`（安装 `musl-tools`）与 `aarch64`（安装 `clang` 配合 `rust/.cargo/config.toml`）执行 `cd rust && cargo build --release --locked`；在 `publish` 作业中作为唯一原子 Release 发布者显式传入 `GH_REPO` / `--repo` 完成创建与上传统一 `SHA256SUMS`。
3. **提交前自验**：每次完成代码或治理文档修改后，必须先通过 `bash -n` 及对应门禁校验，确认零错误后再交付或推送。

## §2 宿主无特权单元测试约定

1. **统一归口**：全部开发期测试脚本（单元测试、模块冒烟、交互验证）必须统一放入 `tests/` 目录并按模块分子目录管理（如 `tests/podman-test-base/`、`tests/yazi/`）。严禁在仓库根目录或已废弃的 `scratch/` 等路径散落测试文件。
2. **无特权可运行**：所有宿主单元测试（`tests/version-test.sh`、`tests/release-test.sh`、`tests/install-bootstrap-test.sh`、`tests/tmux-test.sh`、`tests/fail2ban-sync-test.sh`）必须能在无 `root` 权限、无外网访问的受限沙盒中稳定通过，不污染宿主机系统文件。
3. **多轮红蓝对抗审查、混沌仿真与多环境边界回归（`U5`/`U6`/`U11`/`U32`）**：核心与 Rust 测试套件须长期覆盖多环境与混沌回归矩阵，包括 Debian 10–13+、容器/LXC/无 systemd 环境、断网与双栈网络降级、烂摊子宿主机接管 + 用户自定义保留 + 二次执行幂等收敛、真实故障案例库（`RW-SSH-01`、`RW-FW-01`、`RW-MEM-01`、`RW-TUI-01`）、`stdout`/`stderr` 流分离与 `E1xxx..E6xxx` 错误码契约，以及 `install.sh` 架构检测与 SHA-256 降级校验；高风险模块须通过多轮红蓝对抗审查与异常注入混沌仿真直至零高危缺陷（见 [60 §2–§6](60_rust_and_reconcile.md)、[80 §4](80_swarm_orchestration.md)）。

## §3 Podman 模块容器测试（Dev）

在开发机上使用 **Podman + Debian 官方镜像** 对 Podman 等系统级模块执行冒烟与集成测试，无需真实 VPS：

1. **核心文件**：
   - `tests/podman-module-test.sh`：测试调度入口（计时统计与多版本并行）。
   - `tests/podman-test-inner.sh`：容器内安装、断言与可选重装/卸载验证。
   - `tests/podman-test-base/Dockerfile` 与 `build-base.sh`：构建 `localhost/debopti-podman-test-base:<tag>` 预装镜像（含 `systemd`/`sudo`/`acl`/`curl`）。
   - `.cache/apt/<tag>/archives/`：APT 包缓存（已加入 `.gitignore`），加速重复测试。
2. **常用命令**：
   ```bash
   # 首次构建基础镜像
   bash tests/podman-test-base/build-base.sh
   # 日常冒烟（默认 fast + bookworm）
   bash tests/podman-module-test.sh
   # 完整集成（含幂等重装与卸载）
   PODMAN_TEST_MODE=full bash tests/podman-module-test.sh
   # 多版本并行 / 跳过镜像重建
   PODMAN_TEST_PARALLEL=1 DEBIAN_TEST_TAGS="bookworm bullseye" bash tests/podman-module-test.sh
   PODMAN_TEST_SKIP_BUILD=1 bash tests/podman-module-test.sh
   ```
3. **环境变量**：
   - `PODMAN_TEST_MODE`：默认 `fast`（安装+验证），设为 `full` 追加重装与卸载测试。
   - `DEBIAN_TEST_TAGS`：默认 `bookworm`（`fast`）或 `bookworm bullseye`（`full`）。
   - `PODMAN_TEST_PARALLEL` / `PODMAN_TEST_SKIP_BUILD` / `PODMAN_TEST_FORCE_REBUILD` / `PODMAN_TEST_KEEP`：默认均为 `0`，设为 `1` 启用对应能力。
4. **回归要求**：修改 `scripts/apps/podman.sh` 或其模板后，至少执行一次 `bash tests/podman-module-test.sh` 确认无回归。

## §4 嵌套容器限制与实机补测

在嵌套 Podman 容器内，以下三项能力受内核权限边界限制无法完整验证，测试脚本自动降级并输出跳过标记，不代表真实 Debian 主机故障：

- rootless `newuidmap`（输出 `ROOTLESS_RUNTIME_LIMITED_IN_NESTED_CONTAINER`）
- `systemctl --user` 与 GC 定时器（输出 `GC_TIMER_SKIPPED_NO_USER_SYSTEMD`）
- 主用户 `sudo apppod`（输出 `SUDO_APPPOD_SKIPPED_IN_NESTED_CONTAINER`）

涉及 Podman 或 Systemd 用户级服务的重大变更，发布前应在真实 Debian 主机补测 `apppod ps`、`appctl status` 与 `applog -u <service>`。
