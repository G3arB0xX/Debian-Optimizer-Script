# 70 Agent 预设工作环境、CLI 契约与凭据安全

[上一层：60 Rust 架构与稳态接管](60_rust_and_reconcile.md) | [约束索引](README.md)

本层规定 `debopty` 的 Agent-First CLI 契约（`U27`）、VPS Agent 预设工作环境与无损升级（`U28`）、凭据安全存储与全链路防泄露规范（`U29`），以及 VPS 工作环境内的双受众文档、渐进式披露与代码风格约束（`U30`）。

## §1 Agent-First CLI 契约（`U27`）

1. **稳定 JSON 信封与流分离**：所有面向自动化与 Agent 调用的命令在 `--json` 模式下统一输出结构化信封（含 `ok`、`schema_version`、`command`、`data`、`error` / `hint`），严格遵循 `60 §6` 的 `stdout`/`stderr` 流分离，诊断日志仅写入 `stderr`。
2. **只读自省与预演**：支持 `--dry-run` 预演变更计划（`DryRunPlan`）与 `schema` 自省清单（`SchemaManifest`），在真正落盘前向调用方完整披露拟触达路径、前置检查与预期变更。
3. **非交互 Fail-Closed**：高危或不可逆操作在 `--non-interactive` 或无 TTY 环境下默认阻断（见 `60 §5`），仅在显式提供完整授权参数时执行。

## §2 VPS Agent 预设工作环境架构与无损升级（`U28`）

1. **标准工作目录布局**：默认初始化于 `/var/lib/debopty/agent`（支持 `--dir` 与 `DEBOPTY_AGENT_WORKSPACE` 覆盖），由 `templates/agent/*.tmpl` 嵌入模板统一渲染：
   - 根目录：`AGENTS.md`（`0644`，Agent 专用 L0 入口）、`README.md`（`0644`，人类可读中文说明）、`MEMORY.md`（`0600`，长期运维记忆，仅初次播种，升级严禁覆盖）。
   - `.agents/`：`INDEX.md`（`0644`）、`CONSTRAINTS.md`（`0644`）、`workspace.json`（`0600`，记录 schema 版本与托管文件 SHA-256 哈希）。
   - `.agents/constraints/`：`INDEX.md`、`README.md` 与 `00_vps_general_and_docs.md`、`10_system_change_and_style.md`、`20_security_and_credentials.md`、`30_atomic_ops_and_audit.md`（均 `0644`）。
   - `.agents/memory/`：`INDEX.md`、`README.md` 与 `host-profile.md`（`0644`，主机画像托管块 `<!-- managed-by-debopty:start/end -->` 幂等刷新，保留用户追加笔记）。
   - `.agents/logs/`（`0700`）：`audit.jsonl`（`0600`，追加写入且经脱敏的 NDJSON 审计日志，升级严禁截断）。
   - `.agents/checkpoints/`（`0700`）：按 `<checkpoint_id>` 隔离存放变更前快照（`manifest.json`、`checkpoint.json` 均 `0600`，快照文件保留源权限），支持原子回退。
   - `.agents/credentials/`（`0700`）：托管凭据存储目录（见 `§3`）。
2. **三方哈希无损升级**：重复执行 `debopty agent init` 或 `debopty` 升级时，托管文档基于 `workspace.json` 中的 SHA-256 哈希执行三方合并判定：未改动的托管文件自动升级，用户手动定制的文件默认保留（除非显式 `--force`），`MEMORY.md`、`.agents/logs/audit.jsonl`、检查点与凭据始终无损保留。
3. **变更五步闭环**：远程管理 Agent 修改 VPS 系统时必须遵循「`debopty` 优先 -> 创建检查点（`checkpoint`） -> 语法与连通性预检（`sshd -t` / `nft -c -f`） -> 原子落盘与校验 -> 写入脱敏审计日志与 `MEMORY.md`」闭环；校验失败立即按检查点回退。
4. **生产安装隔离**：仓库开发期 `.agents/` 治理目录在 `install.sh`、`self-install` 与发布归档中必须剥离（见 `30 §1`）；运行时 VPS Agent 工作环境独立部署于 `/var/lib/debopty/agent`，安装器与 `self-install` 在非跳过模式下自动尽力初始化（支持 `DEBOPTY_SKIP_AGENT_WORKSPACE=1` 关闭）。

## §3 凭据妥善保存与全链路防泄露规范（`U29`）

1. **权限与隔离底线**：凭据目录（`.agents/credentials/`、`/etc/debopti/credentials/`）必须为 `0700`，所有凭据文件（`.cred`、`.env` 及原始密钥文件）必须为 `0600`；禁止将凭据散落于仓库根目录、`/tmp`、脚本硬编码或普通世界可读路径。
2. **Systemd 凭据注入**：优先使用 `systemd-creds` 加密并经由 `LoadCredentialEncrypted=<name>:<path>` 或 `LoadCredential=<name>:<path>` 注入服务；在无 `systemd-creds` 环境回退至 `0600` 权限的 `EnvironmentFile=<path>`。严禁在 `.service` 单元或 drop-in 中硬编码 `SetCredential=<name>:<literal>` 或含明文密钥的 `Environment=` 指令（呼应 `20 §6`）。
3. **全链路脱敏与零回显**：
   - `debopty` 凭据列表、状态查询、审计日志（`audit.jsonl`）、JSON 响应与 `MEMORY.md` 均严禁回显原始密钥、密码、Token、私钥或凭据正文。
   - 统一通过 `debopty-core::diag` 的脱敏器对文本与 JSON 递归脱敏。
4. **自动化卫生门禁**：`tools/verify_repo_hygiene.py` 强制拦截仓库中误提交的敏感凭据文件（`.env`、`.env.*`、`*.cred`、`*.pem`、`*.key`、`*.p12`、`*.pfx`、`*.jks`、`id_rsa`、`id_ed25519`、`id_ecdsa`、`wgcf-account.toml`、`credentials.json`、`secrets.json`、`secrets.yaml`、`secrets.yml`）、新增密钥字面量模式（Cloudflare API Token、age 私钥、Telegram Bot Token、Stripe Live Key、`Authorization: Bearer` 字面量）以及 Systemd 模板明文凭据；同时提供 `--check-workspace-perms` 校验已初始化 VPS 工作环境的目录/文件权限与非凭据区零泄露。

## §4 VPS 工作环境的双受众文档、渐进式披露与代码风格约束（`U30`）

1. **双受众格式一致**：VPS Agent 工作环境完整继承仓库 `00 §5` 双受众契约：
   - Agent 专用文档（`AGENTS.md`、`.agents/INDEX.md`、`.agents/CONSTRAINTS.md`、`.agents/constraints/INDEX.md`、`.agents/memory/INDEX.md`）必须具备首行 YAML frontmatter、单外边界 XML 标签、全大写 `SECTION:` 与 2 空格缩进 `KEY: value`；
   - 人类可读文档（`README.md`、`MEMORY.md`、`.agents/constraints/README.md`、`.agents/constraints/[0-9][0-9]_*.md`、`.agents/memory/*.md`）必须使用简洁简体中文、零 Agent XML 标签、零禁用分隔符且保持同目录 `README.md <-> INDEX.md` 双向链接。
2. **渐进式披露与日期禁令**：VPS 工作环境遵循 `50 §1`–`§5` 的 `L0 -> L1 -> L2` 渐进式读取顺序、目录分片阈值、平台中立性与约束文件零 `YYYY-MM-DD` 日期禁令（时间戳仅允许写入 `MEMORY.md` 与 `host-profile.md`）。
3. **脚本与配置风格继承**：管理 Agent 在 VPS 上编写或维护脚本与系统配置时，必须继承 `10 §1`–`§3` 与 `60 §4` 的代码风格规范：Bash 强制 `set -euo pipefail` 与双引号防御展开、配置优先使用 `.d` drop-in 与模板渲染、零占位符、严格保留既有用户自定义配置。
4. **统一机器校验**：`python3 -B tools/verify_agent_fmt.py --root <workspace_dir>` 与 `python3 -B tools/verify_repo_hygiene.py --root <workspace_dir> --check-workspace-perms` 须同时适用于源码仓库根目录与任意已初始化的 VPS Agent 工作目录。
