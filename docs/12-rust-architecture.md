# Rust 重构架构与双引擎共存设计

[文档目录](README.md) | [INDEX.md](INDEX.md) | [仓库总入口](../AGENTS.md)

本文档记录 `debopty` 从纯 Bash 脚本向**纯 Rust 单体静态二进制**渐进演进的架构设计、技术选型依据、双引擎共存契约、工作区结构以及三阶段迁移路线，供运维与开发参考。

**适用系统**：Debian 10 / 11 / 12 / 13（及后续稳定版本）

---

## 1. 背景与技术栈选型

### 1.1 现有纯 Bash 基线审计

当前纯 Bash 实现包含 19 个脚本文件（约 7,499 行代码，含 `deb_optimizer.sh`、`install.sh`、6 个核心模块与 11 个应用模块）以及 `templates/` 下的 87 个运行时模板。在长期维护与低配 VPS 场景下，主要存在以下工程瓶颈：

- **运行开销**：`scripts/tui.sh` 每次刷新菜单都会顺序派生大量外部子进程（`command -v`、`awk`、`sed`、`wc`、`sysctl`、`dpkg` 等），且中文宽度通过字节数近似估算。
- **鲁棒性与安全边界**：存在 `eval echo "~$user"` 等路径解析隐患、非原子 `sed -i` 配置改写、`debopty-f2b-sync.sh` 中对 `nftables`/`ss` 输出的复杂 `awk` 解析，以及个别应用模块中的内联 heredoc 历史债务。
- **宿主依赖面广**：跨 Debian 10（glibc 2.28）至 Debian 13+ 运行时依赖 `bash`、`awk`、`sed`、`grep`、`python3`、`jq` 等外部工具链。

### 1.2 方案对比：`Go 核心 + Rust 胶水 + Rust TUI` vs `纯 Rust`

项目确立采用**纯 Rust 单体静态二进制**，放弃 `Go + Rust` 混合栈。各维度客观对比如下：

| 评估维度 | Go 核心 + Rust 胶水 + Rust TUI | 纯 Rust 单体静态二进制（选定方案） | 结论 |
|---|---|---|---|
| **运行效率** | Go 与 Rust 间存在 C-ABI / IPC 序列化开销、双运行时启动及 GC 停顿 | 原生零开销调用，状态探测并发执行，TUI 刷新亚 10ms 级 | **纯 Rust** |
| **内存与磁盘占用** | 双运行时堆叠，产物体积约 15–30 MB，常驻内存约 15–30 MB RSS | `musl` 静态裁剪产物约 2–5 MB，空载内存约 2–4 MB RSS | **纯 Rust** |
| **兼容性与安全性** | `cgo` 与 `musl` 静态链接支持薄弱；Go 运行时抢占 `SIGURG`/`SIGCHLD`/`SIGINT`，易破坏原始模式 TUI 与 `passwd`/`apt`/`ssh` 子进程交互 | 纯 Rust `musl` 静态 ELF 零 libc 依赖，跨 Debian 10–13+ 一致运行；RAII 终端恢复、`Command::args` 防注入、`tempfile`+`rename` 原子写 | **纯 Rust** |
| **工程与工具链** | 双编译器、双包管理器、双测试流水线及复杂的交叉编译链路 | 单一 Rust 2024 Cargo 工作区，统一 `fmt`/`clippy`/`test` 与六边形 trait 边界 | **纯 Rust** |
| **向后兼容** | 需同时维护 FFI 封装层与 Bash 兼容层 | 直接原生对齐 `debopti.conf`、`VERSION`、`templates/` 与 `debopty-f2b-sync`，配合薄桥接层过渡 | **纯 Rust** |

---

## 2. Strangler-Fig 双引擎共存架构

为保证已部署主机与自动化测试在重构期间零回归，项目采用绞杀者模式（Strangler-Fig）推进迁移，Phase 1–2 默认入口仍为 Bash，`DEBOPTY_ENGINE=rust` 按需切换至原生 Rust 引擎。

### 2.1 不可破坏的向后兼容边界

1. **配置状态文件（`/etc/debopti/debopti.conf`）**：保持可被 Bash `source` 解析的 `KEY="value"` 格式。Rust 通过临时文件与 `rename` 原子改写，完整保留原有注释、空行、未知自定义键及全部 10 个架构键（`IS_CN_REGION`、`BASE_OPTIMIZED`、`INSTALLED`、`SCRIPT_VERSION`、`SCRIPT_BUILD`、`SSH_HARDENED`、`EDITOR_CMD`、`PODMAN_GC_ENABLED`、`YAZI_MEDIA_PREVIEW`、`XRAY_GEO_AUTO_UPDATE`）。
2. **版本标识（`VERSION` 与 `scripts/common.sh`）**：严格维护 SemVer 2.0.0（`DEBOPTI_SEMVER`）、CalVer（`DEBOPTI_CALVER`）与 Build（`DEBOPTI_BUILD`）三键格式，绝不向磁盘写入已废弃的 `DEBOPTI_VERSION=` 键。
3. **防火墙与 Fail2ban（`/etc/nftables.d/debopti`）**：保持 `/etc/nftables.d/50-debopti.nft`、`/etc/nftables.d/55-debopty-f2b-guard.nft`、`/etc/fail2ban/jail.d/99-debopty*.local` 与 `/usr/local/bin/debopty-f2b-sync` 的路径、语法及 `--print-ports`、`--force-reload` 参数兼容。
4. **模板单一事实来源（`templates/`）**：87 个运行时模板继续集中维护于 `templates/`。`debopty-core` 在编译期通过 `build.rs` 嵌入全部运行时模板（严格排除 `INDEX.md`），同时支持通过 `DEBOPTY_TEMPLATE_DIR` 或 `$INSTALL_DIR/templates` 运行时覆盖。
5. **安装路径与快捷命令**：`/opt/debopty`、`/usr/local/bin/debopty`、`/etc/profile.d/debopty.sh`、`/etc/fish/conf.d/debopty.fish` 以及旧版 `debopti` 自动迁移语义保持不变；`install.sh` 支持二进制优先下载并校验 `SHA256SUMS`（安装至 `/opt/debopty/bin/debopty` 并软链 `/usr/local/bin/debopty-rs`），下载或校验失败时自动回退纯 Bash 引擎。

### 2.2 双引擎路由与 Phase-3 全量原生切换

- **默认入口切换（Pure Rust 默认）**：`install.sh` 与 `/usr/local/bin/debopty` 默认安装并执行静态链接的原生 Rust 二进制 `/opt/debopty/bin/debopty`（含多调用 `/usr/local/bin/debopty-f2b-sync` 与兼容软链 `/usr/local/bin/debopty-rs`）。
- **Phase-3 全量原生覆盖（21/21 NativeRust + 0 BashBridge）**：`ModuleRegistry` 中全部 21 个模块（10 个核心/安全/巡检/TUI 模块 + 11 个应用模块：`devops`、`easytier`、`ferron`、`freship`、`golang`、`podman`、`realm`、`rust`、`tailscale`、`warp`、`xray`）及 `self_manage` 均已由原生 Rust 实现；TUI 运行时调度零 `BashBridge` 回退。旧版纯 Bash 脚本已归档至 [archive/legacy-bash/](../archive/legacy-bash/README.md)，仅在用户显式传入 `--engine bash` 或 `debopty bridge` 时按需发现。
- **多调用入口（Multicall `argv[0]`）**：当通过符号链接 `debopty-f2b-sync` 调用同一二进制时，自动直达 `f2b-sync` 引擎，实现 `/usr/local/bin/debopty-f2b-sync` 的单文件无缝替换。
- **TUI 后端选择（`--ui` / `DEBOPTY_UI`）与 `ui-preview`**：默认保持轻量 `console` 后端；构建时启用 `--features ratatui-ui` 后，可通过 `--ui ratatui` 或 `--ui auto` 启用圆角分栏仪表盘、`UiPalette`（TrueColor + ANSI-16 + `NO_COLOR`/`TERM=dumb` 降级）、Braille 遥测微图、`/` 模糊筛选、暗化遮罩确认弹窗与 30fps 轻量帧动效，并支持 `debopty ui-preview --demo --screen all` 非交互渲染全部展示帧。
- **生产纯净性**：生产安装目录（`/opt/debopty`）及发布归档自动剥离 `archive/legacy-bash/`、`rust/`、`governance/`、`tests/`、`tools/` 及全部 `INDEX.md`。

---

## 3. Rust 工作区结构（`rust/`）

工作区采用 Rust 2024 edition，内部保持严格无环分层依赖：

```text
rust/
├── Cargo.toml                 # 工作区根配置、Wave-2 纯 Rust 依赖与 musl 体积优化 Profile
├── Cargo.lock                 # 离线锁文件
├── INDEX.md                   # Rust 工作区 L0 Agent 路由索引
└── crates/
    ├── debopty-core/          # 领域核心（零 UI 依赖，支持非特权单元测试）
    │   ├── build.rs           # 编译期嵌入 templates/**（排除 INDEX.md）与 VERSION
    │   └── src/
    │       ├── error.rs       # 基于 thiserror 的类型化 CoreError（含 Network/HttpStatus/System/Security）
    │       ├── version.rs     # SemVer 2.0.0 与 CalVer/Build 兼容解析写入
    │       ├── config.rs      # /etc/debopti/debopti.conf 原子读写存储
    │       ├── template.rs    # {{KEY}} 模板引擎（内置嵌入 + 外部覆盖 + 原子落盘）
    │       ├── sys.rs         # CommandRunner trait（含 env/stdin）、/etc/passwd、Debian 探测与原子配置助手
    │       ├── status.rs      # 并行状态探测（工具链、rustc、ip_forward、podman_gc）
    │       ├── network.rs     # 纯 Rust ureq+rustls 网络层（DNS 自愈、CN 检测、镜像池下载、Git 克隆）
    │       ├── system.rs      # 原生系统调优（APT/Buster EOL、sysctl/BBR、limits、ZRAM/Swap、low-mem、tmux、转发）
    │       └── security/
    │           ├── mod.rs     # SecurityContext 与子模块导出
    │           ├── f2b_sync.rs # 原生 debopty-f2b-sync（nft/sshd/ss 解析与守卫规则生成）
    │           ├── firewall.rs # nftables 模块化规则增删、SSH 端口更新与 IPv6 NDP 修补
    │           ├── fail2ban.rs # Fail2ban 最佳实践部署、legacy jail.local 清理与安全基线初始化
    │           └── ssh.rs     # SSH 零信任加固、随机高位端口、ssh.socket 适配与原子回滚
    ├── debopty-bridge/        # Strangler-Fig Bash 桥接适配器
    │   └── src/
    │       ├── actions.rs     # 类型化 BashAction 与 AppId 枚举（零原始字符串拼接）
    │       ├── registry.rs    # ModuleRegistry（9/20 NativeRust + 11/20 BashBridge）
    │       └── runner.rs      # 受控 bash -euo pipefail 执行器（TTY 移交与防循环）
    ├── debopty-tui/           # 与终端渲染后端解耦的交互式 TUI
    │   └── src/
    │       ├── state.rs       # Elm 风格 AppState、MenuId、MenuItem 与 StatusSnapshot
    │       ├── menu.rs        # 与 scripts/tui.sh 1:1 对齐的菜单树（12 主项 + 6 子菜单）
    │       ├── renderer.rs    # TuiRenderer trait（可插拔渲染后端抽象）与 UiBackendKind
    │       ├── console_ui.rs  # 默认轻量终端渲染器（方向键/数字键、CJK 宽度对齐、CI 自动退出）
    │       ├── terminal_guard.rs # [ratatui-ui] RAII TerminalGuard 与 panic 终端恢复钩子
    │       └── ratatui_ui.rs  # [ratatui-ui] 分栏仪表盘、状态遥测面板与确认弹窗
    └── debopty-cli/           # 统一入口二进制 debopty（含 multicall debopty-f2b-sync）
        └── src/
            ├── main.rs        # argv[0] 多调用分发、--ui 后端选择与退出码处理
            └── commands/      # core | network | system | security | tui
```

### 3.1 依赖与裁剪策略

- **纯 Rust 离线依赖白名单**：
  - 基础与 CLI：`clap`（derive）、`serde`（derive）、`serde_json`、`thiserror`、`regex`、`console`、`unicode-width`、`walkdir`、`tempfile`、`wait-timeout`、`libc`，测试依赖 `assert_cmd`、`predicates`。
  - 网络层（Wave-2）：`ureq`（`3.5.2`，`default-features = false`，`features = ["rustls", "gzip"]`）+ `rustls`（`0.23.46`，`ring 0.17.14` + `webpki-roots 1.0.12`，纯 Rust TLS 1.2/1.3，严禁启用 `aws-lc-rs`/OpenSSL/Tokio）。
  - 可选增强 TUI（Wave-2）：`ratatui`（`0.29.0`，`default-features = false`，`features = ["crossterm"]`）+ `crossterm`（`0.28.1`），通过 `ratatui-ui` Cargo feature 按需开启，默认编译零膨胀。
- **发布构建 Profile**（`rust/Cargo.toml`）：
  - `opt-level = "z"`、`lto = true"`、`codegen-units = 1`、`strip = true`、`panic = "abort"`。
  - 通过 `x86_64-unknown-linux-musl` 与 `aarch64-unknown-linux-musl` 目标产出零动态库依赖的静态 ELF。

---

## 4. 本地构建、离线测试与差异验证

以下全部命令均支持在无网络环境执行（前置已安装 Rust 工具链与对应 `musl` target；`aarch64` 交叉编译需安装 `clang` 并配合 `rust-lld`）：

```bash
# 运行 Rust 工作区默认单元测试与集成测试（console 默认后端）
cargo test --manifest-path rust/Cargo.toml --offline
make rust-test

# 运行启用 ratatui-ui 特性的全量测试
cargo test --manifest-path rust/Cargo.toml --offline --features ratatui-ui
make rust-test-ratatui

# 运行静态代码检查（默认特性 + ratatui-ui 特性）
cargo clippy --manifest-path rust/Cargo.toml --offline --all-targets -- -D warnings
cargo clippy --manifest-path rust/Cargo.toml --offline --features ratatui-ui --all-targets -- -D warnings

# 编译静态链接的 x86_64 / aarch64 musl 发布二进制（含 ratatui-ui）
make rust-musl CARGO_FLAGS="--offline --features ratatui-ui"
make rust-musl-aarch64 CARGO_FLAGS="--offline --features ratatui-ui"
make rust-musl-all CARGO_FLAGS="--offline --features ratatui-ui"

# 验证产物为静态 ELF（预期包含 statically linked）
file rust/target/x86_64-unknown-linux-musl/release/debopty
file rust/target/aarch64-unknown-linux-musl/release/debopty

# 运行 Rust 切片门禁与 Bash/Rust 行为差异测试
bash tools/check.sh --slice rust
bash tests/rust-parity-test.sh
```

全量仓库门禁 `bash tools/check.sh` 会并行调度语法检查、ShellCheck、Agent/文档格式检查、仓库卫生检查、单元测试、发布一致性检查及 Rust 检查（含默认与 `ratatui-ui` 双特性 `clippy` 与 `test`）共 7 个任务。

---

## 5. 三阶段演进路线与 Wave-2 落地状态

### 5.1 分阶段迁移计划

| 阶段 | 范围 | 核心交付与兼容策略 |
|---|---|---|
| **Phase 1（Wave 1，已完成）** | 共存基座与核心引擎 | 完成 `debopty-core`（版本、配置、模板、系统探测、`f2b-sync`）、`debopty-bridge`、`debopty-tui`、`debopty-cli` 与差异测试；Bash 保持默认，`DEBOPTY_ENGINE=rust` 按需启用。 |
| **Phase 2（Wave 2，已完成）** | 系统、网络、安全与多架构发布 | 原生迁移 `scripts/network.sh`（`ureq` + `rustls`/`ring`、双栈 DNS 自愈、9 节点 CN 镜像池）、`scripts/system.sh`（APT/Buster EOL、`sysctl`/BBR、`ZRAM`/Swap、`tmux`）与 `scripts/security.sh`（SSH 加固回滚、`nftables`、`Fail2ban`），原生覆盖率达 **9/20**；`debopty-tui` 新增可选 `--features ratatui-ui` 与 RAII `TerminalGuard`；`.github/workflows/release.yml` 已上线 `amd64`/`arm64` 静态 `musl` 矩阵与 `SHA256SUMS`；`install.sh` 支持二进制优先校验安装与 Bash 兜底回退。 |
| **Phase 3（已完成 / `v2.0.0` 就绪）** | 11 应用全原生迁移、默认切换与旧版归档 | 全部 11 个应用模块（`realm`/`ferron`/`rust`/`easytier`/`tailscale`/`golang`/`warp`/`xray`/`freship`/`podman`/`devops`）与 `self_manage` 迁移至 `debopty-core::apps`，彻底清零内联 heredoc；`ModuleRegistry` 达到 **21/21 NativeRust（0 BashBridge）**；`/usr/local/bin/debopty` 与 `install.sh` 默认切换为静态 Rust 二进制，旧版 Bash 脚本归档至 `archive/legacy-bash/`；`ratatui-ui` 升级圆角徽章、TrueColor/降级色板、Braille 遥测微图、`/` 筛选、暗化弹窗与 `debopty ui-preview` 展示能力。 |

### 5.2 Wave-2 架构定型记录

1. **TUI 渲染后端演进**：保持轻量 `console` 为默认后端；同时通过 `--features ratatui-ui` 提供分栏仪表盘与实时遥测面板（`--ui console|ratatui|auto`），并由 `TerminalGuard` 在移交 `apt-get`/`passwd`/`ssh` 等子进程前自动恢复终端状态。
2. **网络 HTTP 客户端选型**：采用纯 Rust `ureq 3.5.2` + `rustls 0.23.46` + `ring 0.17.14` + `webpki-roots`（同步流式写盘，零 Tokio 运行时膨胀，且与 `musl` 静态跨平台编译完全兼容）。
3. **多架构发布矩阵**：`.github/workflows/release.yml` 拆分为 `prepare`、`build-rust`（`x86_64-unknown-linux-musl` + `aarch64-unknown-linux-musl`）与 `publish` 三阶段，产出 Bash 源码归档、`amd64`/`arm64` 静态二进制压缩包及统一 `SHA256SUMS`。

---

## 6. 全量切换与旧版 Bash 归档规范

当 Rust 二进制在功能上完全涵盖旧版 Bash 实现后，按以下三项硬性门禁执行默认切换与旧版归档：

1. **100% 原生对等门禁（NativeRust Parity Gate）**：
   - **零桥接回退**：`debopty-bridge` 的 `ModuleRegistry` 中所有核心模块（`common`、`version`、`network`、`system`、`security`、`tui`）与全部 11 个应用模块（`devops`、`easytier`、`ferron`、`freship`、`golang`、`podman`、`realm`、`rust`、`tailscale`、`warp`、`xray`）均须标记为原生 Rust 实现，不存在任何 `BashBridge` 运行时回退。
   - **端到端对等验证**：本地差异测试（`tests/rust-parity-test.sh`）与容器化端到端测试（`tests/podman-module-test.sh`）在纯 Rust 模式下全量通过，覆盖安装、配置读写、防火墙同步与各应用启停语义。
2. **默认入口切换（Default Cutover）**：
   - 将 `/usr/local/bin/debopty`（含 multicall `/usr/local/bin/debopty-f2b-sync`）及引导安装器 `install.sh` 的默认执行目标切换为原生 Rust 静态二进制，不再以 `deb_optimizer.sh` 作为默认入口。
   - 保持现有 CLI 参数、交互菜单行为与环境变量语义不变，确保已部署主机无缝升级。
3. **旧版 Bash 归档与兼容性保留（Legacy Archival）**：
   - **归档路径与历史标签**：将原 Bash 入口及模块脚本（`deb_optimizer.sh`、`scripts/` 等）统一归档至 `archive/legacy-bash/` 目录，并在仓库中创建旧版归档 Git 标签用于历史溯源与回滚参考。
   - **生产产物剔除**：生产安装目录（`/opt/debopty`）及后续发布包（release tarball）自动排除 `archive/legacy-bash/`，仅保留原生二进制及必要运行时资产。
   - **运行时状态与配置零破坏**：归档与切换过程完整保留 `templates/` 模板目录、`/etc/debopti/debopti.conf` 配置文件以及 `/etc/nftables.d/debopti` 防火墙规则目录（含现有 `50-debopti.nft`、`55-debopty-f2b-guard.nft` 与 Fail2ban 配置），确保历史主机状态平滑继承。
