# 70 Agent 预设工作环境、CLI 契约与凭据安全

[上一层：60 Rust 架构与稳态接管](60_rust_and_reconcile.md) | [约束索引](README.md)

本层规定 `debopty` 的 Agent-First CLI 契约（`U27`）、VPS Agent 预设工作环境、CoALA 弹性记忆与无损升级（`U28`）、凭据安全存储与全链路防泄露规范（`U29`）、VPS 工作环境内的双受众文档与渐进式披露约束（`U30`），以及 Harness 通用化与旧记忆/旧约束自动无损迁移协议。

## §1 Agent-First CLI 契约（`U27`）

1. **稳定 JSON 信封与流分离**：所有面向自动化与 Agent 调用的命令在 `--json` 模式下统一输出结构化信封（含 `ok`、`schema_version`、`command`、`data`、`error` / `hint`），严格遵循 `60 §6` 的 `stdout`/`stderr` 流分离，诊断日志仅写入 `stderr`。
2. **只读自省与预演**：支持 `--dry-run` 预演变更计划（`DryRunPlan`）与 `schema` 自省清单（`SchemaManifest`），在真正落盘前向调用方完整披露拟触达路径、前置检查与预期变更。
3. **非交互 Fail-Closed**：高危或不可逆操作在 `--non-interactive` 或无 TTY 环境下默认阻断（见 `60 §5`），仅在显式提供完整授权参数时执行。
4. **确定性策略退出码**：Agent 工作环境策略门禁统一映射为确定性退出码：`0`（成功）、`1`（一般错误/校验失败）、`21`（Write Gate 模式/来源/冲突拒绝）、`22`（记忆体积或代码块膨胀拒绝）、`23`（Write Gate 检测到明文密钥泄露）、`24`（只读审计发现非标准遗留/漂移/厂商锁定）、`25`（Harness 切换脏垫片或迁移/回滚冲突）、`26`（审计或迁移阶段检测到明文密钥或禁止凭据文件）。

## §2 VPS Agent 预设工作环境架构、CoALA 弹性记忆与无损升级（`U28`）

1. **标准工作目录布局（`schema_version: "2"`，兼容 `v1/v2`）**：默认初始化于 `/var/lib/debopty/agent`（支持 `--dir` 与 `DEBOPTY_AGENT_WORKSPACE` 覆盖），由 `templates/agent/**` 嵌入模板统一渲染：
   - 根目录（`0755`）：`AGENTS.md`（`0644`，Agent 专用 L0 规范化中间表示根入口，含 `<!-- managed-by-debopty:start/end -->` 托管块并保留块外手写内容）、`README.md`（`0644`，人类可读简体中文说明）、`.gitignore`（`0644`，由 `# managed-by-debopty:start/end` 托管块按记忆追踪策略维护，保留块外自定义规则）。
   - `docs/agent/` 治理与决策中心（`0755`）：`INDEX.md`（`0644`，L0 路由）、`README.md`（`0644`，与 `INDEX.md` 双向互链）、`CONSTRAINTS.md`（`0644`，约束总索引，零修改日期）、`decisions.md`（`0644`，架构与迁移决策台账 ADR，唯一允许记录 `YYYY-MM-DD` 具体日期的治理文档），以及 `docs/agent/constraints/`（`0755`）下的 `INDEX.md`、`README.md` 与内置分层规范 `00_vps_general_and_docs.md`、`10_system_change_and_style.md`、`20_security_and_credentials.md`、`30_atomic_ops_and_audit.md`、`40_memory_and_retention.md`、`50_harness_decoupling_and_migration.md`（均 `0644`）；仅在迁移或 `--harvest` 触发时生成或更新 `docs/agent/constraints/90_project_legacy_ssot.md`（`0644`，按 SHA-256 去重原文保留遗留规则）。
   - `.agents/` 边界收口（`0755`）：`INDEX.md`（`0644`）、`README.md`（`0644`，与 `INDEX.md` 双向互链）、`MEMORY.md`（`0600`，CoALA Tier-2 记忆目录 TOC，`<=150` 行且 `<=12KB`，由 `<!-- managed-by-debopty:start/end -->` 维护托管目录并保留用户手写种子段）、`workspace.json`（`0600`，`schema_version: "2"`，兼容反序列化 `v1`，记录 `wire_epoch`、`memory_tracking`、`active_harnesses`、`managed_hashes` 与 `migration_state`）。
   - `.agents/memory/`（`0700`）：`INDEX.md`（`0644`，与 `README.md` 双向互链，且在主题数 `>15` 时承载溢出全量目录）、`README.md`（`0644`）、`host-profile.md`（`0600`，托管块 `<!-- managed-by-debopty:start/end -->` 幂等刷新，保留块外手写笔记）、CoALA 主题分片 `<topic>.md` 或 `<domain>/<topic>.md`（`0600`，每篇 `<=200` 行且 `<=16KB`），以及 `.agents/memory/archive/`（`0700`，归档过期、被替代或已晋升分片，`0600`）。
   - `.agents/schemas/`（`0755`）：`INDEX.md`、`README.md` 及 5 份常驻 JSON 契约（`dot_agent_spec.json`、`memory_topic.schema.json`、`workspace_manifest.schema.json`、`repo_blueprint.json`、`harness_adapters.json`，均 `0644`）；迁移提取到无密 MCP 配置时生成 `mcp_servers.json`（`0644`）。
   - `.agents/overrides/`（`0755`）：固定插件式 Harness 覆盖槽位（`INDEX.md`、`README.md`、`_template.md` 及按需创建的 `<harness>.{md,json}`，均 `0644`），未启用时零上下文开销。
   - `.agents/skills/`（`0755`）与 `.agents/rules/`（`0755`）：可复用技能与通用规则目录，迁移时无损保留。
   - 运行态私有目录（均 `0700`）：`.agents/logs/`（含 `audit.jsonl` `0600`，追加式脱敏 NDJSON 审计日志）、`.agents/checkpoints/`（按 `<checkpoint_id>/` 隔离，含 `0600` 的 `manifest.json`/`checkpoint.json` 与保权快照）、`.agents/credentials/`（U29 托管凭据，见 `§3`）、`.agents/cache/`（含 `memory_index.json` `0600`，零 LLM 的 `mtime+size+sha256` 中英文 BM25 倒排索引）、`.agents/tmp/`（临时工作区）。
2. **CoALA 三层弹性记忆与 Fail-Closed Write Gate**：
   - **三层架构**：Tier-3 工作态草稿置于外部临时目录，永不入库；Tier-2 情景/运维记忆位于 `.agents/MEMORY.md` 与 `.agents/memory/<topic>.md`；Tier-1 制度化规则沉淀于 `AGENTS.md` 与 `.agents/constraints/*.md`。
   - **弹性分片与检索引擎**：`0` 主题时仅维护 `.agents/MEMORY.md`；`1..15` 主题平铺于 `.agents/memory/<topic>.md`；超过 `15` 主题时自动按域子分片（`<domain>/<topic>.md`）并将完整目录溢出至 `.agents/memory/INDEX.md`，保持 `MEMORY.md <=150` 行 / `12KB`；搜索统一走 `.agents/cache/memory_index.json` 的中英文 BM25 索引，单条摘要折叠 `<=20` 行并附 `path:Lstart-Lend` 定位指针。
   - **10 键元数据与 Write Gate**：每篇主题分片须包含 `memory_topic.schema.json` 规定的 10 个必填键（`id`、`title`、`type`、`status`、`scope`、`created_at`、`updated_at`、`tags`、`sources`、`confidence`）及可选维护键（`ttl_days`、`expires_at`、`supersedes`、`superseded_by`、`promoted_to`、`managed_by`）。写入门禁严格 Fail-Closed：检测到明文密钥返回 `rc=23`；模式/来源/重复冲突返回 `rc=21`；超过 `200` 行、`16KB` 或单个围栏代码块 `>80` 行返回 `rc=22`。
   - **休眠整固与晋升**：支持 `consolidate --plan/--apply`（按默认 `ttl_days=90`、`expires_at`、`superseded_by` 归档至 `.agents/memory/archive/`，识别重复与超限拆分、发现晋升候选并重建 TOC 与索引）及 `promote`（将高置信度长期规则写入 Tier-1 规范文档并留下 `status: promoted` 归档墓碑）。既有 `agent memory --append` 保持向后兼容并强制脱敏。
   - **记忆追踪策略（`--memory-tracking local|tracked`）**：默认 `local`，`.gitignore` 托管块自动忽略 `.agents/MEMORY.md`、`.agents/memory/` 及全部运行态目录；显式 `--memory-tracking tracked` 允许共享 `.agents/MEMORY.md` 与 `.agents/memory/<topic>.md`，但始终忽略 `host-profile.md`、`workspace.json` 及运行态目录，并在每次 `init`/`migrate` 时输出追踪策略提示。
3. **三方哈希无损升级与变更五步闭环**：重复执行 `debopty agent init` 时，托管文档基于 `workspace.json` 的 SHA-256 哈希执行三方判定，未改动文件自动升级，人工定制文件默认保留（除非显式 `--force`）；`.agents/MEMORY.md`、主题记忆分片、`.agents/logs/audit.jsonl`、检查点与凭据始终无损保留。远程管理 Agent 修改 VPS 系统时须遵循「`debopty` 优先 -> 创建检查点 -> 语法与连通性预检（`sshd -t` / `nft -c -f`） -> 原子落盘与校验 -> 写入脱敏审计日志与记忆」闭环，失败立即按检查点回退。
4. **生产安装隔离**：仓库开发期 `.agents/` 治理目录在 `install.sh`、`self-install` 与发布归档中必须剥离（见 `30 §1`）；运行时 VPS Agent 工作环境独立部署于 `/var/lib/debopty/agent`，安装器与 `self-install` 在非跳过模式下自动尽力初始化（支持 `DEBOPTY_SKIP_AGENT_WORKSPACE=1` 关闭）。

## §3 凭据妥善保存与全链路防泄露规范（`U29`）

1. **权限与隔离底线**：凭据目录（`.agents/credentials/`、`/etc/debopti/credentials/`）必须为 `0700`，所有凭据文件（`.cred`、`.env` 及原始密钥文件）必须为 `0600`；禁止将凭据散落于仓库根目录、`/tmp`、脚本硬编码或普通世界可读路径。
2. **Systemd 凭据注入**：优先使用 `systemd-creds` 加密并经由 `LoadCredentialEncrypted=<name>:<path>` 或 `LoadCredential=<name>:<path>` 注入服务；在无 `systemd-creds` 环境回退至 `0600` 权限的 `EnvironmentFile=<path>`。严禁在 `.service` 单元或 drop-in 中硬编码 `SetCredential=<name>:<literal>` 或含明文密钥的 `Environment=` 指令（呼应 `20 §6`）。
3. **全链路脱敏与零回显**：
   - `debopty` 凭据列表、状态查询、审计日志（`audit.jsonl`）、JSON 响应与 `.agents/MEMORY.md` 及全部记忆分片均严禁回显原始密钥、密码、Token、私钥或凭据正文。
   - 统一通过 `debopty-core::diag` 的脱敏器对文本与 JSON 递归脱敏。
4. **自动化卫生门禁**：`tools/verify_repo_hygiene.py` 强制拦截仓库中误提交的敏感凭据文件（`.env`、`.env.*`、`*.cred`、`*.pem`、`*.key`、`*.p12`、`*.pfx`、`*.jks`、`id_rsa`、`id_ed25519`、`id_ecdsa`、`wgcf-account.toml`、`credentials.json`、`secrets.json`、`secrets.yaml`、`secrets.yml`）、密钥字面量模式以及 Systemd 模板明文凭据；同时提供 `--check-workspace-perms` 校验已初始化 VPS 工作环境的 `0700`/`0600` 权限（涵盖 `credentials/`、`checkpoints/`、`logs/`、`memory/`、`memory/archive/`、`cache/`、`tmp/` 与 `MEMORY.md`、`workspace.json`、`host-profile.md`、主题分片、`audit.jsonl`、`memory_index.json`）及非凭据区零泄露。

## §4 VPS 工作环境的双受众文档、渐进式披露与代码风格约束（`U30`）

1. **双受众格式一致**：VPS Agent 工作环境完整继承仓库 `00 §5` 双受众契约：
   - Agent 专用文档（`AGENTS.md`、`docs/agent/INDEX.md`、`docs/agent/CONSTRAINTS.md`、`docs/agent/constraints/INDEX.md`、`.agents/INDEX.md`、`.agents/memory/INDEX.md`、`.agents/schemas/INDEX.md`、`.agents/overrides/INDEX.md`）必须具备首行 YAML frontmatter、单外边界 XML 标签、全大写 `SECTION:` 与 2 空格缩进 `KEY: value`；
   - 人类可读文档（`README.md`、`docs/agent/README.md`、`docs/agent/decisions.md`、`docs/agent/constraints/{README,[0-9][0-9]_*}.md`、`.agents/README.md`、`.agents/MEMORY.md`、`.agents/memory/**/*.md`、`.agents/schemas/README.md`、`.agents/overrides/{README,_template,*}.md`）必须使用简洁简体中文、零 Agent XML 标签、零禁用分隔符且保持同目录 `README.md <-> INDEX.md` 双向链接。
2. **渐进式披露、日期禁令与平台中立**：VPS 工作环境遵循 `50 §1`–`§5` 的 `L0 -> L1 -> L2` 渐进式读取顺序、目录分片阈值、平台中立性（通用 `.md` 规范与索引文件严禁出现特定厂商或特定 Harness 品牌字样）与规范化约束文件零 `YYYY-MM-DD` 日期禁令（时间戳仅允许写入 `docs/agent/decisions.md`、记忆文档与 `host-profile.md`；`90_project_legacy_ssot.md` 内围栏代码块中的遗留原文片段豁免日期校验）。
3. **脚本与配置风格继承**：管理 Agent 在 VPS 上编写或维护脚本与系统配置时，必须继承 `10 §1`–`§3` 与 `60 §4` 的代码风格规范：Bash 强制 `set -euo pipefail` 与双引号防御展开、配置优先使用 `.d` drop-in 与模板渲染、零占位符、严格保留既有用户自定义配置。
4. **统一机器校验**：`python3 -B tools/verify_agent_fmt.py --root <workspace_dir>` 与 `python3 -B tools/verify_repo_hygiene.py --root <workspace_dir> --check-workspace-perms` 须同时适用于源码仓库根目录与任意已初始化、已切换 Harness 或已迁移的 VPS Agent 工作目录。

## §5 Harness 通用化与旧记忆/旧约束自动无损迁移

1. **规范化中间表示与零厂商锁定**：工作环境以 `AGENTS.md`、`docs/agent/` 与 `.agents/` 为唯一事实源（SSOT）；默认 `--harnesses none` 写入零厂商垫片。14 套内置 Harness 适配器定义与预设（`none`、`compat`、`open`、`auto`、`all` 及 `<csv>` 自定义组合）完全数据驱动地沉淀在 `.agents/schemas/harness_adapters.json` 中，并支持 `custom_adapters` 与 `--adapter-file` 扩展，严禁污染通用 `.md` 规范文档。
2. **引用计数投影切换与脏垫片保护**：`debopty agent switch --to|--add|--remove` 在 `.agents/schemas/repo_blueprint.json` 中维护按路径引用计数与 SHA-256 记录的 `managed_projections`。切换或移除前校验磁盘文件 SHA-256：若检测到用户手工修改过托管垫片，默认返回 `rc=25` 阻断；传入 `--harvest` 时先将该垫片改动原文按 SHA-256 去重采集进 `docs/agent/constraints/90_project_legacy_ssot.md` 再执行切换，切换前自动创建检查点并清理空父目录。
3. **插件式覆盖槽位（`.agents/overrides/`）**：通过 `debopty agent override --init|--list|--remove` 为特定 Harness 维护 `.agents/overrides/<harness>.{md,json}` 独立覆盖声明，将不可避免的 Harness 专有参数与通用 SSOT 彻底解耦。
4. **只读审计（`agent audit`）与自动无损迁移（`agent migrate`）**：
   - `debopty agent audit` 为严格只读操作：发现明文密钥或非凭据区禁止凭据文件立即 Fail-Closed 返回 `rc=26`；发现遗留记忆（根目录 `MEMORY.md`、`.agent/`、`memory-bank/` 等）、非规范约束/垫片、遗留技能、MCP 配置、投影漂移或厂商锁定时返回 `rc=24` 并输出结构化清单。
   - `debopty agent migrate`（以及 `agent init` 在检测到 `v1` 根目录 `MEMORY.md` 且尚无 `.agents/MEMORY.md` 时自动触发的无损升级）严格遵循「先密钥门禁（`rc=26`） -> 创建 `0700`/`0600` 变更前检查点 -> 迁移执行 -> 重建 TOC/索引/清单 -> 脱敏审计」流程：
     - 旧记忆自动迁移至 `.agents/memory/legacy-<slug>.md`（`0600`，超 `180` 行或 `14KB` 自动分块为 `-part<N>.md`，附源路径与 SHA-256 溯源，重复迁移零重复）。
     - 旧约束与非规范垫片按 `<!-- legacy-source-sha256:<hex> -->` 去重后原文无损合入 `docs/agent/constraints/90_project_legacy_ssot.md` 托管块，并在 `docs/agent/decisions.md` 记录迁移决策。
     - 旧技能迁入 `.agents/skills/`（不覆盖已有同名技能）；旧 MCP 配置仅提取无密服务定义至 `.agents/schemas/mcp_servers.json`（含明文字面量直接 `rc=26` 阻断）。
     - 托管块之外的全部用户手写文本完整保留；除非显式传入 `--cleanup-legacy`，否则绝不删除原始遗留文件。
5. **原子回滚（`rollback <id|latest>`）**：任意迁移或切换均可通过 `debopty agent rollback <checkpoint_id|latest>` 按检查点快照精确还原迁移前的完整目录与文件状态。
