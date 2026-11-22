# VPS Agent 预设工作环境与凭据安全管理

[返回文档目录](README.md) | [INDEX.md](INDEX.md) | [仓库总入口](../AGENTS.md)

本文档说明 `debopty` 为远程管理 Agent 预置的 VPS 标准化工作环境（`U28`）、凭据安全存储与防泄露机制（`U29`），以及工作环境内的双受众文档与渐进式披露校验方法（`U30`）。

**适用系统**：Debian 10 / 11 / 12 / 13（及后续稳定版本）

---

## 1. 预设工作环境目录结构

默认初始化路径为 `/var/lib/debopty/agent`（可通过 `--dir` 或环境变量 `DEBOPTY_AGENT_WORKSPACE` 覆盖）。安装或升级 `debopty` 时会自动尽力初始化该目录（设置 `DEBOPTY_SKIP_AGENT_WORKSPACE=1` 可跳过）。

```text
/var/lib/debopty/agent/                   (0755)
├── AGENTS.md                             (0644, Agent 专用 L0 根入口)
├── README.md                             (0644, 人类可读简体中文说明)
├── MEMORY.md                             (0600, 长期运维记忆；仅初次播种，升级严禁覆盖)
└── .agents/                              (0755)
    ├── INDEX.md                          (0644, .agents/ L0 路由)
    ├── CONSTRAINTS.md                    (0644, 约束总索引，零修改日期)
    ├── workspace.json                    (0600, 元数据与托管文件 SHA-256 哈希表)
    ├── constraints/                      (0755)
    │   ├── INDEX.md                      (0644, 与 README.md 双向互链)
    │   ├── README.md                     (0644, 人类可读约束索引)
    │   ├── 00_vps_general_and_docs.md    (0644, 双受众文档、渐进式披露与日期禁令)
    │   ├── 10_system_change_and_style.md (0644, debopty 优先、Bash/模板/drop-in 风格)
    │   ├── 20_security_and_credentials.md(0644, 凭据存储权限、Systemd 注入与预检)
    │   └── 30_atomic_ops_and_audit.md    (0644, 检查点、回滚、审计日志与记忆维护)
    ├── memory/                           (0755)
    │   ├── INDEX.md                      (0644, 与 README.md 双向互链)
    │   ├── README.md                     (0644, 主机记忆目录说明)
    │   └── host-profile.md               (0644, 托管块幂等刷新，保留自定义笔记)
    ├── logs/                             (0700)
    │   └── audit.jsonl                   (0600, 追加式脱敏 NDJSON 审计日志)
    ├── checkpoints/                      (0700)
    │   └── <checkpoint_id>/              (0700, manifest.json/checkpoint.json 0600 + 快照文件)
    └── credentials/                      (0700)
        └── <name>[.env|.cred]            (0600, 托管凭据；严禁在日志或列表回显明文)
```

### 1.1 无损升级与三方哈希策略

- **托管文档**：`workspace.json` 记录各托管文档的 SHA-256 哈希。再次运行 `debopty agent init` 时，未修改的托管文档自动更新，人工定制过的文件默认保留（显式传入 `--force` 方可覆盖）。
- **状态与用户资产**：`MEMORY.md`、`.agents/logs/audit.jsonl`、`.agents/checkpoints/` 与 `.agents/credentials/` 在任何升级场景下均无损保留，绝不覆盖或截断。
- **主机画像**：`.agents/memory/host-profile.md` 仅刷新 `<!-- managed-by-debopty:start -->` 与 `<!-- managed-by-debopty:end -->` 之间的托管块，块外手写笔记完整保留。

---

## 2. `debopty agent` 常用命令

所有命令支持 `--json` 输出结构化信封（`stdout` 仅输出数据，`stderr` 仅输出诊断日志）：

```bash
# 1. 初始化或无损升级工作环境
debopty agent init --dir /var/lib/debopty/agent --non-interactive

# 2. 查看工作环境健康状态、记忆行数、检查点数量与凭据审计摘要
debopty agent status --json

# 3. 修改系统配置前创建原子检查点
debopty agent checkpoint --label pre-ssh-change --reason "调整 SSH 端口" \
  --path /etc/ssh/sshd_config --path /etc/nftables.conf

# 4. 列出检查点并按需原子回退（非交互模式须显式加 --yes）
debopty agent checkpoint --list --json
debopty agent rollback <checkpoint_id>

# 5. 查看或追加脱敏审计日志与运维记忆
debopty agent audit-log --limit 20 --json
debopty agent audit-log --append --action sys-check --summary "巡检通过" --status ok
debopty agent memory --section "变更记录" --append "启用 nftables 端口 8443"

# 6. 校验工作环境完整性、权限与安全基线
debopty agent verify --json

# 7. 凭据托管（仅允许 --value-stdin 或 --from-file，严禁命令行明文传值）
printf '%s' "$SECRET_INPUT" | debopty agent cred set --name api_token --kind systemd-encrypted --value-stdin
debopty agent cred set --name app_env --kind env --from-file /run/secrets/app.env
debopty agent cred list --json
debopty agent cred audit --json
debopty agent cred systemd-snippet --unit my-app.service --name api_token --kind systemd-encrypted --write-dropin
debopty agent cred remove --name api_token
```

---

## 3. 凭据安全存储与 Systemd 注入（`U29`）

### 3.1 目录与权限规范

- 凭据目录（`.agents/credentials/`、`/etc/debopti/credentials/`）强制 `0700`。
- 凭据文件（`*.cred`、`*.env` 及原始密钥文件）强制 `0600`。
- 凭据列表、状态报告、审计日志、JSON 响应与 `MEMORY.md` 严禁输出原始密钥正文。

### 3.2 Systemd 服务凭据注入

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

## 4. 双受众文档、渐进式披露与自动校验（`U30`）

VPS 工作环境完整继承仓库的文档与代码规范（详见 [.agents/constraints/70_agent_workspace_and_credentials.md](../.agents/constraints/70_agent_workspace_and_credentials.md)）：

- **双受众分工**：`AGENTS.md`、`.agents/CONSTRAINTS.md` 与全部 `**/INDEX.md` 采用 Agent 专用格式（首行 YAML frontmatter + 单外边界 XML + `SECTION:` / 2 空格 `KEY: value`）；`README.md`、`MEMORY.md`、`.agents/constraints/*.md` 与 `.agents/memory/*.md` 采用人类可读简体中文，零 Agent XML 标签，同目录 `README.md` 与 `INDEX.md` 双向互链。
- **渐进式披露与日期禁令**：遵循 `L0 -> L1 -> L2` 读取顺序；`.agents/CONSTRAINTS.md` 与 `.agents/constraints/*.md` 严禁出现 `YYYY-MM-DD` 日期字面量。
- **离线校验命令**（仓库侧脚本可直接对任意 VPS 工作环境目录执行校验）：

```bash
# 校验双受众文档格式、双向链接与日期禁令
python3 -B tools/verify_agent_fmt.py --root /var/lib/debopty/agent

# 校验 0700/0600 权限、非凭据区零密钥泄露与凭据文件隔离
python3 -B tools/verify_repo_hygiene.py --root /var/lib/debopty/agent --check-workspace-perms
```
