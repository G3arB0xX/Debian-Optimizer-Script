# 60 Rust 架构、TUI 健壮性与稳态接管

[上一层：50 上下文与文档治理](50_context_and_docs.md) | [约束索引](README.md) | [下一层：70 Agent 工作环境与凭据安全](70_agent_workspace_and_credentials.md)

本层规定 `debopty` Pure Rust 重构架构、Strangler-Fig 共存与归档、TUI 双后端健壮性、烂摊子系统稳态接管、用户自定义保留、高危操作防护及统一诊断日志契约。

## §1 Pure Rust 架构、Strangler-Fig 共存与平滑归档（`U0`/`U1`/`U2`/`U3`）

1. **单二进制四 crate 分层**：Rust 工作区严格划分为 `debopty-core`（核心领域、模板、接管与诊断）、`debopty-bridge`（受控 Bash 桥接）、`debopty-tui`（终端界面）与 `debopty-cli`（CLI 入口与路由），最终编译为单一静态二进制。
2. **零 OpenSSL 与轻量网络栈**：网络请求统一使用 `ureq + rustls`（`ring + webpki-roots`）纯 Rust 静态链接方案，严禁引入 OpenSSL、`aws-lc-rs` 或 Tokio 运行时以控制静态产物体积与构建可移植性。
3. **多架构 `musl` 与自举降级**：发布矩阵覆盖 `x86_64` 与 `aarch64` 静态 `musl` 二进制；`install.sh` 采用“二进制优先（含架构检测与 SHA-256 多工具降级校验）、失败平滑降级纯 Bash”的双轨自举策略。
4. **Strangler-Fig 共存与防递归协议**：
   - 通过 `DEBOPTY_ENGINE` 与 `DEBOPTY_FROM_RUST=1` 控制引擎路由并阻断 Rust 与 Bash 之间的递归回环调用。
   - `BashBridge` 调起旧版脚本时必须先传 `--` 隔离参数以防止前导 `-` 标志注入，并强制钉死 `SCRIPT_DIR`、`INSTALL_DIR`、`CONFIG_FILE`、`IN_TUI`、`DEBOPTY_FROM_RUST` 与 `DEBOPTY_ENGINE` 等关键环境变量，严禁外部 `extra_env` 覆盖。
5. **100% 替代归档与安装纯净**：待 NativeRust 达到 100% 功能等价后全面取代并归档旧版 Bash 实现；开发期 `rust/` 源码与构建产物严禁打入 `/opt/debopty` 运行目录或 Bash 发布归档。

## §2 TUI 双后端、终端恢复与全尺寸健壮性（`U3`/`U4`/`U5`/`U11`）

1. **双后端编译与门禁**：以 `console` 作为默认 TUI 后端、`ratatui-ui` 作为可选 Cargo feature（CLI 支持 `--ui console|ratatui|auto`），本地检查与 CI 必须同时验证默认特性与 `--features ratatui-ui`。
2. **RAII 终端守卫**：`TerminalGuard` 独立追踪 raw mode 与 alternate screen 状态标志，在 `Drop` 与挂起态安全的 panic hook 中按状态幂等还原终端，杜绝异常退出破坏用户终端。
3. **双 TTY 检测与优雅降级**：交互能力判定必须同时校验 `stdin` 与 `stdout` 均为 TTY；遇到 `TERM=dumb`、非交互管道或 `BrokenPipe` 时立即干净退出并恢复终端状态。
4. **标准键盘导航与防误触**：坚持标准方向键、数字快捷键与 `Enter`/`Esc` 导航，禁用 Vim 独占键绑定及易误触发破坏性确认的 `Ctrl`/`Alt` 组合键（严禁 `Ctrl+Y` 等误确认）。
5. **极小视口与宽字符零 Panic**：覆盖 `0x0`、`1x1`、`Resize(0,0)` 等极端终端尺寸与 CJK/FE0F 组合图标宽字符截断渲染，严格保证 `visual_width <= max_width` 且零 Panic。

## §3 烂摊子系统接管与稳态收敛（`U7`/`U11`）

1. **非洁净宿主预设**：系统接管引擎默认假设宿主机存在非标准、相互冲突或残损的历史配置，而非洁净初始环境。
2. **稳态收敛六步闭环**：`debopty reconcile` 必须严格执行：
   - **只读审计默认（Audit-by-default）**：默认审计模式零写入，仅输出漂移与冲突清单。
   - **写前原子快照**：任何修改或隔离迁移前必须生成运行目录并落盘完整 `BackupManifest` JSON 清单。
   - **语法预检门禁**：应用 SSH 与防火墙配置前强制通过 `sshd -t` 与 `nft -c -f` 预检。
   - **原子收敛与即时回滚**：支持一键 `--restore <RUN_DIR>` 按清单逐字节还原。
   - **二次幂等稳态**：连续两次执行 `apply` 后第二次变更数必须为 `0`，随后 `audit` 漂移数为 `0`。
3. **容器防火墙作用域刷新**：宿主机存在 Podman `netavark`、Docker `nat` 等外部/容器 nftables 表时，严禁执行全局 `flush ruleset`，必须仅刷新 debopty 自身管理的表级作用域。
4. **受限环境优雅降级**：在容器、LXC 或无 systemd 环境中，对 `zram`、`sysctl` 只读权限受限及活跃 `dpkg` 锁须降级告警并跳过不可达变更，严禁强行破坏宿主状态。
5. **模板单一事实来源**：所有接管产出的系统与服务配置必须通过 `templates/`（`TemplateEngine`）渲染或受控行级规范器生成，严禁在 Rust 或 Bash 源码中内联多行配置 Here-Doc（对齐 `10 §2`）。

## §4 非破坏性尊重用户自定义内容（`U8`/`U11`）

1. **托管边界标识**：以 `# managed-by-debopty` 标记或成对托管块界定自动化管理范围，边界之外一律视为用户资产。
2. **自定义资产保留清单**：接管过程严禁无故删改以下用户手动配置：
   - 已有 SSH 公钥及用户自定义 `Match` 块；
   - Fail2ban `ignoreip` 与自定义 jail 配置；
   - 容器网络表与用户自定义 `.nft` 规则文件；
   - 当前有效且非托管的自定义 DNS 及 `search`/`options` 选项；
   - 与优化项无关的 `sysctl`、`limits`、`fstab` 与 `logrotate` 条目；
   - 第三方 APT `.list` 与 `.sources` 源配置；
   - `debopti.conf` 中的自定义键值与用户注释。
3. **冲突最小化消解**：仅当用户旧配置与稳态基线发生明确冲突时，先执行完整备份再以最小侵入方式收敛，并在审计结果中记录 `PreservedUserCustom` 状态。

## §5 原子权限保护与高危操作 Fail-Closed 红线（`U5`）

1. **原子写权限继承**：原子写操作默认继承目标文件的既有权限位（新建普通系统配置默认 `0644`），严禁因临时文件库默认 `0600` 导致 `/etc/ssh/sshd_config`、`/etc/resolv.conf` 等共享系统文件降权不可读；私钥与 `authorized_keys` 等凭据须显式限定为 `0600`。
2. **高危操作 Fail-Closed**：SSH 加固等不可逆高危操作在非 TTY 环境下默认阻断（Fail-Closed，零写入），仅在显式传入 `--non-interactive --pubkey` 等完整授权参数时方可执行，严禁使用默认密码或临时密钥静默覆盖认证配置。

## §6 统一错误码、流分离与细粒度结构化日志规范（`U10`）

1. **统一错误码与修复指引**：全仓错误体系（`CoreError`/`CliError`）统一映射至稳定错误码区间：`E1xxx`（通用/I/O/参数）、`E2xxx`（版本/配置/模板）、`E3xxx`（网络/HTTP）、`E4xxx`（系统/子进程）、`E5xxx`（安全/接管）、`E6xxx`（桥接/TUI/序列化）；每条报错必须携带明确上下文与可执行简体中文修复提示（`hint`）。
2. **严格 `stdout`/`stderr` 流分离**：`stdout` 专用于纯业务数据与机器可解析 JSON 输出，`stderr` 专用于诊断日志与错误报告，严禁任何日志级别文本混入 `stdout`。
3. **五级日志、双格式与脱敏**：零外部依赖诊断模块 `debopty-core::diag` 统一提供 `error|warn|info|debug|trace` 五级过滤与 `text|json`（含 RFC 3339 时间戳的 NDJSON）双格式输出，并强制对密码、密钥与 Token 执行脱敏处理。
