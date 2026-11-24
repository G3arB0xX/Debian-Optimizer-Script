# 开发流、测试门禁与自主发版指南

[返回文档目录](README.md) | [INDEX.md](INDEX.md) | [仓库总入口](../AGENTS.md)

本文档说明 `debopty` 仓库的可复用日常开发流（Dev Loop）、静态交叉编译与容器验证、自主发版与单一发布源标准操作流程（Release Loop），以及 CI 稳定性契约与常见故障排查方法。

---

## 1. 日常开发流（Dev Loop）

### 1.1 `jj` + `git` 提交与分支规范

1. **双态协同**：本地开发使用 `jj`（与 `git` 双向同步），所有 CI 门禁、Changelog 生成与发版均以 Git `main` 分支为准。
2. **Conventional Commits 1.0.0**：提交消息严格采用 `<type>[(<scope>)][!]: <summary>` 格式（例如 `feat(security): ...`、`fix(cli): ...`、`docs(workflow): ...`）。
   - **严禁在 `scope` 内使用逗号**（例如 `feat(a,b): ...` 会导致 `tools/release.sh` 解析失败并漏录 Changelog）。
   - 合法 `type` 包括 `feat`、`fix`、`perf`、`refactor`、`revert`、`docs`、`test`、`ci`、`build`、`chore`、`style`。
3. **同步 `main` 书签**：准备提交门禁或推送前，确认有效变更已描述并导出至 Git `main`：
   ```bash
   jj bookmark set main -r <rev> --allow-backwards
   jj git export
   git checkout main
   ```

### 1.2 测试数据与文档安全红线

1. **零明文秘密字面量**：严禁在源码、单元测试、模板、配置示例或文档中硬编码任何可能触发 GitHub Secret Scanning 的真实或高仿字面量，包括：`PEM 私钥头`、`tskey-auth-*`、`ghp_*`、`AKIA*`、`JWT` 载荷片段及含明文用户名密码的凭据 URL。
2. **运行期动态拼接**：脱敏与拦截测试（如日志红线脱敏、URL 凭据擦除测试）必须在**运行期动态组装构造**（Rust 通过运行期拼接或切片组合，Bash 测试通过拆分变量拼接），确保源码工作树与编译产物 `.rodata` 段均无静态秘密模式残留。
3. **双层扫描门禁**：
   - 日常 `make check` / `make lint` 会自动运行 `python3 -B tools/verify_repo_hygiene.py`，对全仓文本文件执行静态秘密字面量扫描。
   - 运行 `make hygiene-history`（即 `python3 -B tools/verify_repo_hygiene.py --scan-git-history HEAD`）可扫描当前分支全部历史 Git blob，提前拦截历史提交残留。

### 1.3 增量门禁与提交前全量门禁

1. **增量快速门禁**：局部改动时执行作用域门禁，加速反馈：
   ```bash
   make check SLICE=<path>
   # 或直接调用脚本（支持逗号分隔多路径）：
   bash tools/check.sh --slice rust,tests/rust-parity-test.sh
   ```
2. **提交前全量门禁**：合入或推送前必须通过全量门禁：
   ```bash
   make check
   # CI 严格等价命令：
   bash tools/check.sh --strict-shellcheck --strict-rust
   ```
   全量门禁并行执行以下检查并汇总结构化 JSONL 摘要：
   - **静态语法与规范**：`bash -n`、`py_compile`、`shellcheck`；
   - **双受众文档与仓库卫生**：`verify_agent_fmt.py`（YAML frontmatter、XML 边界、双向链接、日期禁令）与 `verify_repo_hygiene.py`（字节码洁净、平台中立性、工作树秘密字面量扫描）；
   - **宿主无特权单元测试**：`tests/*-test.sh`（除 `rust-parity-test.sh` 外的轻量单元测试）；
   - **发布一致性校验**：`bash tools/release.sh verify`（支持附加 `--check-commits` 校验提交消息）；
   - **Rust 双特性校验与串行契约测试**：依次执行 `cargo fmt --check`、`clippy --locked --features ratatui-ui --all-targets -- -D warnings`、`cargo test --locked --features ratatui-ui`、`clippy --locked --all-targets -- -D warnings` 与 `cargo test --locked`（默认特性放最后以保持默认调试二进制）；待 `run_rust` 彻底退出后，再**串行执行** `tests/rust-parity-test.sh`，彻底杜绝共享 `rust/target` 目录造成的二进制特性覆盖与 `ETXTBSY` 竞态。

---

## 2. 本地静态交叉编译与容器验证

### 2.1 集中式 musl 链接配置

- Workspace MSRV 在 `rust/Cargo.toml` 声明为 `rust-version = "1.88"`（适配 `edition = "2024"` 与 `let-chains`），依赖版本由 `rust/Cargo.lock` 严格锁定。
- `rust/.cargo/config.toml` 集中定义了 `x86_64-unknown-linux-musl` 与 `aarch64-unknown-linux-musl` 的 `rust-lld` 自包含静态链接参数，以及 `aarch64` 目标的 `clang` 交叉编译环境变量。
- **工作目录约定**：Cargo 仅在当前工作目录位于 `rust/` 时自动加载 `rust/.cargo/config.toml`，因此跨架构静态构建请统一通过 `Makefile` 或显式 `cd rust && cargo build ...` 调用。

### 2.2 常用构建与容器集成命令

```bash
# 仅编译/测试 Rust 工作区（默认特性 / ratatui-ui）
make rust-build
make rust-test
make rust-test-ratatui

# 静态 musl 发布二进制构建（自动切换至 rust/ 并启用 --locked 与 ratatui-ui）
make rust-musl
make rust-musl-aarch64
make rust-musl-all

# 核验静态 ELF 产物
file rust/target/x86_64-unknown-linux-musl/release/debopty
file rust/target/aarch64-unknown-linux-musl/release/debopty

# Podman 容器冒烟与 systemd 端到端测试
bash tests/podman-module-test.sh
make e2e-podman
```

离线环境测试可设置 `DEBOPTY_CARGO_OFFLINE=1`（或向支持脚本传 `--offline`），使用本地 Cargo 缓存进行校验。

---

## 3. 自主发版与单一发布源 SOP（Release Loop）

本项目确立 **`.github/workflows/release.yml` 为唯一的 GitHub Release 原子发布者**：本地推送脚本只负责历史净化与推送 `main` 分支和 `vX.Y.Z` annotated tag，不再在本地调用 `gh release create` 或生成残缺 `SHA256SUMS`，从根本上消除双发布源竞态。

标准发版按以下 5 步执行：

1. **步骤 1 — 演练规划（Plan）**：
   ```bash
   make release-plan
   # 或输出 JSON 详情：
   bash tools/release.sh plan --json
   ```
   只读预览下一 SemVer 版本、兼容 `DEBOPTI_CALVER` / `DEBOPTI_BUILD`、目标 tag 与待归档 Changelog 摘要。

2. **步骤 2 — 发布前门禁核验（Verify）**：
   ```bash
   make release-check
   ```
   运行严格门禁 `bash tools/check.sh --strict-shellcheck --strict-rust` 与 `bash tools/release.sh verify`，确保代码、测试、`VERSION`、`VERSION_ID` 与 `CHANGELOG.md` 完全一致。

3. **步骤 3 — 原子升版（Bump）**：
   ```bash
   bash tools/release.sh bump --dry-run
   bash tools/release.sh bump --bump auto
   ```
   可按需指定 `--bump major|minor|patch`、`--version X.Y.Z` 或 `--prerelease rc.1`。**严禁**手工编辑 `VERSION`、`VERSION_ID` 或 `CHANGELOG.md` 的发布元数据。

4. **步骤 4 — 脱敏净化推送与本地 Tag 同步（Push）**：
   ```bash
   bash tools/github-push.sh --dry-run
   bash tools/github-push.sh
   ```
   脚本在独立临时沙盒中自动执行以下操作：
   - 运行推送前校验（`release.sh verify` 与 `verify_repo_hygiene.py`）；
   - 使用 `git-filter-repo` 按规则重写历史测试秘密字面量（若未安装 `git-filter-repo` 则 fail-closed 中止）；
   - 在推送网络前于沙盒内强制运行 `verify_repo_hygiene.py --scan-git-history HEAD`，任何历史残留即刻阻断推送；
   - 仅向远端推送脱敏后的 `main` 与不可变 annotated tag `vX.Y.Z`（禁止覆盖既有远端 tag）；
   - 远端推送成功后，通过 `_sync_local_tag` 将 `vX.Y.Z` 非破坏性地同步至本地仓库 `main`，防止本地 tag 漂移导致后续 `release.sh plan` 范围异常。

5. **步骤 5 — GitHub Actions 原子构建与发布（Publish）**：
   远端收到 `vX.Y.Z` tag 后，`.github/workflows/release.yml` 自动执行：
   - `prepare`：复跑严格门禁、`release.sh verify`、`--scan-git-history HEAD`，生成洁净生产资产包 `debopty-vX.Y.Z.tar.gz` 并校验包内无 `.agents/`、`governance/`、`rust/`、`archive/`、`**/INDEX.md` 等开发文件；
   - `build-rust`：使用固定 `RUST_TOOLCHAIN` 与 `--locked` 并发构建 `x86_64` / `aarch64` 静态 musl 二进制（`--features ratatui-ui`）；
   - `publish`：汇总资产并生成统一 `SHA256SUMS`，通过显式 `GH_REPO` / `--repo` 上下文幂等创建或更新 GitHub Release（预发布自动标记 `--prerelease`）。

> **不可变原则**：已发布的远端 `vX.Y.Z` tag 严禁删除或强制覆盖；若发版后发现问题，一律递增 `patch`（`vX.Y.(Z+1)`）重新发版。

---

## 4. CI 稳定性契约与常见故障排查

### 4.1 CI 稳定性契约

- **固定工具链与依赖锁定**：`.github/workflows/check.yml` 与 `release.yml` 顶部统一固定 `RUST_TOOLCHAIN`（例如 `"1.95.0"`），所有 Cargo 步骤强制 `--locked`，并接入 `Swatinem/rust-cache@v2` 缓存依赖与编译产物，避免上游镜像升级带来的 `clippy` 漂移或冷编译超时。
- **PR / 提交规范拦截**：`check.yml` 在 `pull_request`（全提交范围）与 `push`（`HEAD~1..HEAD`）自动运行 `bash tools/release.sh verify --check-commits ...`。
- **完整失败日志回传**：`tools/check.sh` 在任意子步骤失败时输出末尾 120 行诊断日志；`check.yml` 在失败时自动打印 `/tmp/debopty_logs/*.log` 并写入 `$GITHUB_STEP_SUMMARY`。本地排障可直接运行 `make logs` 查看日志目录。

### 4.2 常见故障排查清单

| 现象 | 常见原因 | 处理方式 |
|---|---|---|
| GitHub 报错 `Action needed: Secrets detected` | 源码、测试夹具或历史提交中包含明文 `PEM 私钥头`、`tskey-auth-*`、`ghp_*`、`AKIA*`、`JWT` 或含凭据 URL | 1) 将源码/测试中静态字面量改为运行期动态拼接；2) 运行 `make check` 确认工作树 0 命中；3) 确认 `tools/github-push.sh` 替换规则覆盖对应历史字面量后执行 `bash tools/github-push.sh --force` 重写净化远端历史并通过 `--scan-git-history HEAD` 门禁；4) 推送后在 GitHub Security 界面复核告警状态并关闭已彻底清除的历史告警 |
| 本地/CI `rust-parity-test.sh` 报 `--ui ratatui` 退出码或 `ETXTBSY` 错误 | 手动并行执行了修改同一 `rust/target` 的 Cargo 构建与测试 | 使用 `make check` / `tools/check.sh` 统一调度（已内置在 `run_rust` 完成后串行运行 `rust-parity-test.sh`） |
| `release.yml` 交叉编译链接失败 | `x86_64` 缺少 `musl-tools` 或 `aarch64` 缺少 `clang` / 未从 `rust/` 目录执行 | 本地构建统一使用 `make rust-musl-all`；CI 中保持 `musl-tools`、`clang` 安装及 `cd rust && cargo build --locked` |
| `release.yml` `publish` 阶段 `gh release` 报 `not a git repository` | `publish` 作业未检出仓库且未传仓库上下文 | 确认 `publish` 作业保留 `GH_REPO: ${{ github.repository }}` 环境变量及 `--repo` 参数 |
| `github-push.sh` 报未安装 `git-filter-repo` | 缺少历史净化依赖（脚本 fail-closed 拒绝降级） | 执行 `pip install --user git-filter-repo` 并确保 `~/.local/bin` 在 `PATH` 中 |
