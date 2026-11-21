# 40 测试、本地门禁与 CI 规范

[上一层：30 发布与版本管理](30_release_and_vcs.md) | [约束索引](README.md) | [下一层：50 上下文与文档治理](50_context_and_docs.md)

本层规定本地一键门禁、GitHub Actions CI、宿主无特权单元测试以及 Podman 模块容器测试的执行标准。

## §1 本地门禁与 GitHub Actions CI

1. **统一入口（`Makefile`）**：
   - `make check`：调用 `bash tools/check.sh` 并发执行全套静态校验（含 `bash -n` 与 `tools/*.py` 字节码校验）、`shellcheck`、宿主单元测试、双特性 Rust 编译测试（`default` 与 `--features ratatui-ui`，见 [60 §2](60_rust_and_reconcile.md)）、双受众格式检查、仓库卫生检查与 `tools/release.sh verify` 发布一致性门禁，并输出结构化 JSONL 日志（含 `release_rc`）。
   - `make lint`：执行全仓 `bash -n` 语法校验、`shellcheck`（本地未安装时告警跳过，GitHub Actions CI 中为强制阻断项）、`python3 tools/verify_agent_fmt.py`（双受众格式门禁）与 `python3 tools/verify_repo_hygiene.py`（仓库卫生与平台中立性门禁）。
   - `make test`：运行全部宿主无特权单元测试。
   - `make release-plan` / `make release-verify`：预览下一 SemVer 版本与 Changelog，或校验 `VERSION`、`VERSION_ID` 与 `CHANGELOG.md` 一致性。
   - `make logs`：查看最近一次门禁运行日志。
2. **GitHub Actions CI/CD**：
   - `.github/workflows/check.yml` 在 `push`、`pull_request` 与手动触发时自动运行 `shellcheck` 与 `make check`，任何失败均阻断合并。
   - `.github/workflows/release.yml` 在推送 SemVer tag（`vX.Y.Z*`）或手动触发时复跑门禁与 `tools/release.sh verify`，校验 tag 与 `VERSION` 一致性，生成洁净归档 `debopty-vX.Y.Z.tar.gz` 与 `SHA256SUMS`，并幂等创建或更新 GitHub Release。
3. **提交前自验**：每次完成代码或治理文档修改后，必须先通过 `bash -n` 及对应门禁校验，确认零错误后再交付或推送。

## §2 宿主无特权单元测试约定

1. **统一归口**：全部开发期测试脚本（单元测试、模块冒烟、交互验证）必须统一放入 `tests/` 目录并按模块分子目录管理（如 `tests/podman-test-base/`、`tests/yazi/`）。严禁在仓库根目录或已废弃的 `scratch/` 等路径散落测试文件。
2. **无特权可运行**：所有宿主单元测试（`tests/version-test.sh`、`tests/release-test.sh`、`tests/install-bootstrap-test.sh`、`tests/tmux-test.sh`、`tests/fail2ban-sync-test.sh`）必须能在无 `root` 权限、无外网访问的受限沙盒中稳定通过，不污染宿主机系统文件。
3. **对抗性审查、多环境边界与真实恶劣案例回归（`U5`/`U6`/`U11`）**：核心与 Rust 测试套件须长期覆盖多环境与混沌回归矩阵，包括 Debian 10–13+、容器/LXC/无 systemd 环境、断网与双栈网络降级、烂摊子宿主机接管 + 用户自定义保留 + 二次执行幂等收敛、真实故障案例库（`RW-SSH-01`、`RW-FW-01`、`RW-MEM-01`、`RW-TUI-01`）、`stdout`/`stderr` 流分离与 `E1xxx..E6xxx` 错误码契约，以及 `install.sh` 架构检测与 SHA-256 降级校验；相关设计规范见 [60 §2–§6](60_rust_and_reconcile.md)。

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
