# VPS Agent 预设工作环境与凭据安全管理

[返回文档目录](README.md) | [INDEX.md](INDEX.md) | [仓库总入口](../AGENTS.md)

本文档说明 `debopty` 为远程管理 Agent 预置的 VPS 标准化工作环境（`U28`、`schema_version: "2"`）、CoALA 三层弹性记忆与 Write Gate、Harness 通用化与覆盖槽位、旧记忆与旧约束自动无损迁移协议、凭据安全存储（`U29`），以及双受众文档与离线校验方法（`U30`）。

**适用系统**：Debian 10 / 11 / 12 / 13（及后续稳定版本）

---

## 1. 预设工作环境目录结构（v2 标准）

默认初始化路径为 `/var/lib/debopty/agent`（可通过 `--dir` 或环境变量 `DEBOPTY_AGENT_WORKSPACE` 覆盖）。安装或升级 `debopty` 时会自动尽力初始化该目录（设置 `DEBOPTY_SKIP_AGENT_WORKSPACE=1` 可跳过）。

```text
/var/lib/debopty/agent/                              (0755)
├── AGENTS.md                                        (0644, Agent 专用 L0 规范化中间表示根入口)
├── README.md                                        (0644, 人类可读简体中文说明)
├── .gitignore                                       (0644, 托管块按 --memory-tracking 维护)
├── [可选 Harness 垫片]                              (0644, 仅 --harnesses != none 时生成)
└── .agents/                                         (0755)
    ├── INDEX.md                                     (0644, .agents/ L0 路由，与 README.md 互链)
    ├── README.md                                    (0644, .agents/ 人类可读概览)
    ├── CONSTRAINTS.md                               (0644, 约束总索引，零修改日期)
    ├── MEMORY.md                                    (0600, CoALA Tier-2 记忆目录 TOC <=150L/12KB)
    ├── workspace.json                               (0600, schema v2 元数据与托管文件 SHA-256)
    ├── constraints/                                 (0755)
    │   ├── INDEX.md                                 (0644, 与 README.md 双向互链)
    │   ├── README.md                                (0644, 人类可读约束索引)
    │   ├── 00_vps_general_and_docs.md               (0644, 双受众文档、渐进式披露、日期禁令与平台中立)
    │   ├── 10_system_change_and_style.md            (0644, debopty 优先、Bash/模板/drop-in 风格)
    │   ├── 20_security_and_credentials.md           (0644, U29 凭据权限、Systemd 注入与预检)
    │   ├── 30_atomic_ops_and_audit.md               (0644, 五步闭环、检查点、回滚与审计日志)
    │   ├── 40_memory_and_retention.md               (0644, CoALA 弹性记忆、Write Gate、整固与追踪策略)
    │   ├── 50_harness_decoupling_and_migration.md   (0644, Harness 通用化、覆盖槽位与无损迁移协议)
    │   └── 90_project_legacy_ssot.md                (0644, 迁移或 --harvest 时按需生成，SHA-256 去重原文)
    ├── memory/                                      (0700)
    │   ├── INDEX.md                                 (0644, 与 README.md 互链；>15 主题时承载溢出目录)
    │   ├── README.md                                (0644, 主机记忆目录说明)
    │   ├── host-profile.md                          (0600, 托管块幂等刷新，保留自定义笔记)
    │   ├── <topic>.md / <domain>/<topic>.md         (0600, CoALA 主题分片 <=200L/16KB，10 键元数据)
    │   └── archive/                                 (0700, 过期/被替代/已晋升分片归档 0600)
    ├── schemas/                                     (0755)
    │   ├── INDEX.md                                 (0644, 与 README.md 双向互链)
    │   ├── README.md                                (0644, Schema 契约说明)
    │   ├── dot_agent_spec.json                      (0644, .agents/ 规范布局与权限契约)
    │   ├── memory_topic.schema.json                 (0644, CoALA 主题分片 JSON Schema)
    │   ├── workspace_manifest.schema.json           (0644, workspace.json v1/v2 兼容 Schema)
    │   ├── repo_blueprint.json                      (0644, 蓝图版本、记忆追踪与引用计数投影表)
    │   ├── harness_adapters.json                    (0644, 14 套内置适配器、预设与 custom_adapters)
    │   └── mcp_servers.json                         (0644, 迁移检测到无密 MCP 配置时按需生成)
    ├── overrides/                                   (0755, 插件式 Harness 覆盖槽位；未启用零开销)
    │   ├── INDEX.md                                 (0644, 与 README.md 双向互链)
    │   ├── README.md                                (0644, 覆盖槽位使用说明)
    │   ├── _template.md                             (0644, 覆盖文件标准模板)
    │   └── [<harness>.md | <harness>.json]          (0644, 通过 agent override --init 按需创建)
    ├── skills/                                      (0755, 可复用技能目录，迁移无损保留)
    ├── rules/                                       (0755, 通用规则目录，迁移无损保留)
    ├── logs/                                        (0700)
    │   └── audit.jsonl                              (0600, 追加式脱敏 NDJSON 审计日志)
    ├── checkpoints/                                 (0700)
    │   └── <checkpoint_id>/                         (0700, manifest.json/checkpoint.json 0600 + 快照)
    ├── credentials/                                 (0700)
    │   └── <name>[.env|.cred]                       (0600, U29 托管凭据；严禁在日志或列表回显明文)
    ├── cache/                                       (0700)
    │   └── memory_index.json                        (0600, 零 LLM mtime+size+sha256 中英文 BM25 索引)
    └── tmp/                                         (0700, 临时运行草稿目录)
```

### 1.1 无损升级与 `--memory-tracking` 策略

- **三方哈希无损升级**：`workspace.json`（`schema_version: "2"`，兼容 `v1`）记录各托管文档的 SHA-256 哈希。重复执行 `debopty agent init` 时，未修改的托管文档自动更新，人工定制文件默认保留（显式传入 `--force` 方可覆盖）；若检测到 `v1` 根目录 `MEMORY.md` 且尚无 `.agents/MEMORY.md`，自动触发无损检查点迁移。
- **托管块保留用户文本**：`.agents/MEMORY.md`、`.agents/memory/host-profile.md`、`.agents/constraints/90_project_legacy_ssot.md` 使用 `<!-- managed-by-debopty:start/end -->`，`.gitignore` 使用 `# managed-by-debopty:start/end`，块外用户内容始终完整保留。
- **记忆追踪策略（`--memory-tracking local|tracked`）**：
  - `local`（默认）：`.gitignore` 托管块自动忽略 `.agents/MEMORY.md`、`.agents/memory/` 及全部运行态目录（`workspace.json`、`logs/`、`checkpoints/`、`credentials/`、`cache/`、`tmp/`），仅在本地留存运维记忆。
  - `tracked`（显式开启）：取消忽略 `.agents/MEMORY.md` 与 `.agents/memory/<topic>.md` 以便通过 Git 跨节点共享，但始终忽略 `host-profile.md`、`workspace.json` 与运行态目录，并强制走零密钥 Write Gate。每次 `init`/`migrate` 均会输出 `memory_tracking_notice` 提示。

---

## 2. CoALA 三层弹性记忆与 `debopty agent memory`

- **三层分工**：Tier-3 工作态临时文件置于外部临时目录；Tier-2 情景记忆存放于 `.agents/MEMORY.md`（`<=150` 行 / `12KB`）与 `.agents/memory/<topic>.md`（`<=200` 行 / `16KB`，单个围栏代码块 `<=80` 行）；Tier-1 长期规则沉淀于 `.agents/constraints/*.md`。
- **弹性分片与零 LLM 检索**：`>15` 个活跃主题时自动启用域子目录分片（`.agents/memory/<domain>/<topic>.md`）并将完整目录溢出至 `.agents/memory/INDEX.md`。`.agents/cache/memory_index.json` 提供中英文 BM25 倒排检索，单条命中折叠 `<=20` 行并附 `path:Lstart-Lend`。
- **Fail-Closed Write Gate**：所有写入须满足 `.agents/schemas/memory_topic.schema.json` 的 10 键元数据；检测到密钥泄露返回 `rc=23`，模式/来源/重复冲突返回 `rc=21`，超限膨胀返回 `rc=22`。

```bash
# 1. 向后兼容的快捷追加（自动脱敏写入 .agents/MEMORY.md）
debopty agent memory --section "变更记录" --append "启用 nftables 端口 8443"

# 2. 通过 Write Gate 写入标准 CoALA 主题分片
debopty agent memory write --id firewall-ports --title "防火墙放行策略" \
  --topic-type procedural --scope security --tags firewall,nftables \
  --sources cli:agent-memory --confidence 0.9 --body "仅放行必需公网端口并同步 Fail2ban。"

# 3. 重建 BM25 索引、检索与记忆体检
debopty agent memory index --json
debopty agent memory search --query "防火墙 端口" --limit 5 --json
debopty agent memory lint --json

# 4. 休眠整固（预演/执行归档过期与被替代分片）与晋升至 Tier-1 规范
debopty agent memory consolidate --plan --json
debopty agent memory consolidate --apply --json
debopty agent memory promote --id firewall-ports --target 20_security_and_credentials.md --json
```

---

## 3. Harness 通用化、切换与覆盖槽位

- **零厂商锁定**：默认 `debopty agent init --harnesses none` 仅生成规范化中间表示（`AGENTS.md` + `.agents/`），写入 0 个厂商垫片。
- **14 套内置适配器与自定义扩展**：全部适配器定义与预设（`none`、`compat`、`open`、`auto`、`all` 或 `<csv>`）集中在 `.agents/schemas/harness_adapters.json` 中（支持 `custom_adapters` 与 `--adapter-file <path>`），可通过 `debopty agent harness-list` 查看完整列表。
- **引用计数切换与脏垫片保护**：`repo_blueprint.json` 维护每个托管垫片的 SHA-256 与引用计数。切换或移除时若检测到用户手动编辑过托管垫片，默认返回 `rc=25` 阻断；加 `--harvest` 可先将改动原文按 SHA-256 去重采集进 `.agents/constraints/90_project_legacy_ssot.md` 再安全切换。
- **独立覆盖槽位**：使用 `debopty agent override` 在 `.agents/overrides/` 下维护特定 Harness 的专属参数，与通用 SSOT 彻底隔离。

```bash
# 查看 14 套内置适配器、预设与自定义适配器
debopty agent harness-list --json

# 按需启用、切换或安全移除 Harness 投影垫片
debopty agent init --harnesses none --memory-tracking local --non-interactive
debopty agent switch --add compat --json
debopty agent switch --remove compat --harvest --json

# 管理插件式覆盖槽位
debopty agent override --init custom-harness --json
debopty agent override --list --json
debopty agent override --remove custom-harness --json
```

---

## 4. 旧记忆与旧约束自动无损迁移（`audit` / `migrate` / `rollback`）

```bash
# 1. 只读审计：检测遗留记忆/约束/垫片/MCP/漂移/密钥（0=标准, 24=非标准, 26=含密钥）
debopty agent audit --json

# 2. 自动无损迁移（先密钥检查 -> 创建 0700/0600 检查点 -> 分块迁移记忆 -> SHA-256 去重合并规则）
#    默认保留原遗留文件；仅在显式加 --cleanup-legacy 时清理已迁移原文件
debopty agent migrate --harnesses none --memory-tracking local --json

# 3. 校验迁移后状态
debopty agent verify --json

# 4. 任意时刻按最新或指定检查点精确原子回滚
debopty agent checkpoint --list --json
debopty agent rollback latest --yes --non-interactive
```

迁移核心不变量：
- 发现明文密钥或非凭据区禁止凭据文件时立即 `rc=26` 阻断，绝不将密钥迁入记忆或 SSOT。
- 遗留记忆自动迁入 `.agents/memory/legacy-<slug>.md`（超 `180` 行 / `14KB` 自动拆分为 `-part<N>.md`，附 SHA-256 溯源）。
- 遗留约束与非规范垫片按 `<!-- legacy-source-sha256:<hex> -->` 原文去重写入 `.agents/constraints/90_project_legacy_ssot.md`，重复运行零重复。
- 遗留技能迁入 `.agents/skills/`，无密 MCP 配置提取至 `.agents/schemas/mcp_servers.json`。

---

## 5. 确定性退出码一览

| 退出码 | 含义 | 典型触发场景 |
|---|---|---|
| `0` | 成功 | 命令执行或审计/校验完全通过 |
| `1` | 一般失败 | 参数错误、IO 失败或 `verify` 发现结构/权限异常 |
| `21` | `EXIT_WRITE_GATE_REJECT` | CoALA 主题分片缺少必填元数据、非法枚举/来源或重复冲突 |
| `22` | `EXIT_MEMORY_BLOAT` | 超过 TOC `150L/12KB`、分片 `200L/16KB` 或围栏代码块 `80L` 上限 |
| `23` | `EXIT_MEMORY_SECRET_LEAK` | Write Gate 检测到明文密钥字面量 |
| `24` | `EXIT_AUDIT_NONSTANDARD` | `agent audit` 发现遗留记忆/约束、投影漂移或厂商锁定 |
| `25` | `EXIT_MIGRATION_CONFLICT` | 脏垫片未加 `--harvest`/`--force`，或迁移/回滚冲突 |
| `26` | `EXIT_SECRET_DETECTED` | `agent audit` 或 `agent migrate` 检测到明文密钥或禁止凭据文件 |

---

## 6. 凭据安全存储与 Systemd 注入（`U29`）

### 6.1 目录与权限规范

- 凭据目录（`.agents/credentials/`、`/etc/debopti/credentials/`）强制 `0700`。
- 凭据文件（`*.cred`、`*.env` 及原始密钥文件）强制 `0600`。
- 凭据列表、状态报告、审计日志、JSON 响应与 `.agents/MEMORY.md` / 记忆分片严禁输出原始密钥正文。

### 6.2 凭据管理命令与 Systemd 注入

仅允许 `--value-stdin` 或 `--from-file` 录入凭据，严禁命令行参数明文传值：

```bash
printf '%s' "$SECRET_INPUT" | debopty agent cred set --name api_token --kind systemd-encrypted --value-stdin
debopty agent cred set --name app_env --kind env --from-file /run/secrets/app.env
debopty agent cred list --json
debopty agent cred audit --json
debopty agent cred systemd-snippet --unit my-app.service --name api_token --kind systemd-encrypted --write-dropin
debopty agent cred remove --name api_token
```

优先采用 `systemd-creds` 加密后通过 `LoadCredentialEncrypted` 注入；当宿主机不支持 `systemd-creds` 时，回退至 `0600` 权限的 `LoadCredential` 或 `EnvironmentFile`。严禁在单元文件或 drop-in 中直接明文写入 `SetCredential=<name>:<literal>` 或 `Environment=.*(PASSWORD|TOKEN|SECRET|API_KEY)=<literal>`。

```ini
# /etc/systemd/system/<unit>.service.d/20-debopty-credential.conf
[Service]
LoadCredentialEncrypted=api_token:/var/lib/debopty/agent/.agents/credentials/api_token.cred
```

或回退方案：

```ini
# /etc/systemd/system/<unit>.service.d/20-debopty-credential.conf
[Service]
EnvironmentFile=/var/lib/debopty/agent/.agents/credentials/api_token.env
```

---

## 7. 双受众文档、渐进式披露与离线校验（`U30`）

VPS 工作环境完整继承仓库的文档与代码规范（详见 [.agents/constraints/70_agent_workspace_and_credentials.md](../.agents/constraints/70_agent_workspace_and_credentials.md)）：

- **双受众分工**：`AGENTS.md`、`.agents/CONSTRAINTS.md` 与全部 `**/INDEX.md` 采用 Agent 专用格式（首行 YAML frontmatter + 单外边界 XML + `SECTION:` / 2 空格 `KEY: value`）；`README.md`、`.agents/README.md`、`.agents/MEMORY.md`、`.agents/constraints/*.md`、`.agents/memory/**/*.md`、`.agents/schemas/README.md` 与 `.agents/overrides/*.md` 采用人类可读简体中文，零 Agent XML 标签，同目录 `README.md` 与 `INDEX.md` 双向互链。
- **渐进式披露与日期禁令**：遵循 `L0 -> L1 -> L2` 读取顺序；`.agents/CONSTRAINTS.md` 与规范化 `.agents/constraints/00..50_*.md` 严禁出现 `YYYY-MM-DD` 日期字面量（`90_project_legacy_ssot.md` 内围栏代码块中的遗留原文片段豁免）。
- **离线校验命令**（仓库侧脚本可直接对任意已初始化、已切换 Harness 或已迁移的 VPS 工作环境目录执行校验）：

```bash
# 校验双受众文档格式、双向链接与日期禁令
python3 -B tools/verify_agent_fmt.py --root /var/lib/debopty/agent

# 校验 0700/0600 权限、非凭据区零密钥泄露、凭据文件隔离与平台中立性
python3 -B tools/verify_repo_hygiene.py --root /var/lib/debopty/agent --check-workspace-perms
```
