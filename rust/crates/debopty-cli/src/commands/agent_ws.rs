use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use clap::Subcommand;
use debopty_core::agent_workspace::{
    AUDIT_LOG_REL, AgentWorkspaceOptions, AuditEntry, CHECKPOINTS_DIR_REL, CREDENTIALS_DIR_REL,
    DEFAULT_WORKSPACE_DIR, EXIT_AUDIT_NONSTANDARD, EXIT_SECRET_DETECTED, LEGACY_SSOT_REL,
    MEMORY_DIR_REL, MEMORY_FILE_REL, MEMORY_INDEX_CACHE_REL, MemoryTopicInput, MemoryTracking,
    OVERRIDES_DIR_REL, OverrideAction, REPO_BLUEPRINT_REL, SwitchMode, TopicStatus, TopicType,
    append_audit_entry, append_memory_note, audit_workspace, consolidate_memory, create_checkpoint,
    init_workspace, lint_memory, list_checkpoints, load_adapter_registry, manage_override,
    migrate_workspace, promote_memory, read_audit_log, rebuild_memory_index, rollback_checkpoint,
    search_memory, switch_harnesses, verify_workspace, workspace_status, write_memory_topic,
};
use debopty_core::credentials::{
    CredentialKind, audit_credentials, list_credentials, remove_credential, render_systemd_dropin,
    store_credential, validate_credential_name,
};
use debopty_core::diag;
use debopty_core::sys::RealCommandRunner;
use debopty_core::template::TemplateEngine;
use serde::Serialize;

use crate::agent::{AgentCtx, DryRunPlan, emit_dry_run_plan, emit_envelope, emit_envelope_status};
use crate::{CliError, non_empty_env};

#[derive(Debug, Subcommand)]
pub enum AgentAction {
    /// 初始化或非破坏性升级宿主机 Agent 预设工作环境 (SSOT v2，Harness 通用化)
    Init {
        /// Agent 工作目录覆盖 (默认读取 DEBOPTY_AGENT_WORKSPACE 或 /var/lib/debopty/agent)
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖 (用于隔离测试或自定义 rootfs)
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 强制覆盖受管文档模板 (仍保留 .agents/MEMORY.md 与 audit.jsonl)
        #[arg(long)]
        force: bool,
        /// 激活的 Harness 适配器或预设 (如 none | compat | all | antigravity,claude-code)
        #[arg(long, value_name = "SPEC")]
        harnesses: Option<String>,
        /// 记忆追踪策略: local (默认，加入 .gitignore) | tracked (受版本控制)
        #[arg(long, value_name = "MODE")]
        memory_tracking: Option<String>,
        /// 初始化同时无损迁移旧版记忆、约束、技能与 MCP 配置
        #[arg(long)]
        migrate_legacy: bool,
        /// 迁移完成后清理已归档的旧版文件 (需与 --migrate-legacy 配合)
        #[arg(long)]
        cleanup_legacy: bool,
        /// 自定义 Harness 适配器 JSON 文件路径
        #[arg(long, value_name = "PATH")]
        adapter_file: Option<PathBuf>,
    },
    /// 查看宿主机 Agent 工作环境状态、记忆行数、检查点与凭据健康度
    Status {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
    },
    /// 只读审计工作区 SSOT 合规性、旧版漂移、厂商耦合与明文密钥泄漏 (退出码 0/24/26)
    Audit {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
    },
    /// 无损迁移旧版记忆、约束、技能与 MCP 配置至 SSOT v2 (自动创建回滚检查点)
    Migrate {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 目标激活 Harness 适配器或预设
        #[arg(long, value_name = "SPEC")]
        harnesses: Option<String>,
        /// 记忆追踪策略: local | tracked
        #[arg(long, value_name = "MODE")]
        memory_tracking: Option<String>,
        /// 迁移完成后清理已无损转存的旧版文件
        #[arg(long)]
        cleanup_legacy: bool,
        /// 自定义 Harness 适配器 JSON 文件路径
        #[arg(long, value_name = "PATH")]
        adapter_file: Option<PathBuf>,
    },
    /// 切换、追加或移除激活的 Harness 投影垫片 (支持脏垫片 --harvest 无损收割)
    Switch {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 全量切换至指定 Harness 列表/预设
        #[arg(long, value_name = "SPEC")]
        to: Option<String>,
        /// 追加激活指定 Harness 列表/预设
        #[arg(long, value_name = "SPEC")]
        add: Option<String>,
        /// 移除指定 Harness 列表/预设的投影垫片
        #[arg(long, value_name = "SPEC")]
        remove: Option<String>,
        /// 移除脏垫片前自动无损收割原文至 90_project_legacy_ssot.md
        #[arg(long)]
        harvest: bool,
        /// 强制覆盖/移除脏垫片而不收割
        #[arg(long)]
        force: bool,
        /// 自定义 Harness 适配器 JSON 文件路径
        #[arg(long, value_name = "PATH")]
        adapter_file: Option<PathBuf>,
    },
    /// 管理 .agents/overrides/ 下的按 Harness 定制覆盖文件
    Override {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 为指定 Harness 初始化覆盖文件
        #[arg(long, value_name = "HARNESS")]
        init: Option<String>,
        /// 列出当前工作区的所有覆盖文件
        #[arg(long)]
        list: bool,
        /// 移除指定 Harness 的覆盖文件
        #[arg(long, value_name = "HARNESS")]
        remove: Option<String>,
        /// 使用 JSON 格式初始化覆盖文件 (默认 Markdown)
        #[arg(long)]
        json_format: bool,
    },
    /// 列出所有内置 (14 个) 与自定义 Harness 适配器及预设
    HarnessList {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 自定义 Harness 适配器 JSON 文件路径
        #[arg(long, value_name = "PATH")]
        adapter_file: Option<PathBuf>,
    },
    /// 创建或列出原子配置检查点 (复用 reconcile::backup 机制)
    Checkpoint {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 检查点标签 (用于标识本次变更域)
        #[arg(long, default_value = "manual", value_name = "LABEL")]
        label: String,
        /// 检查点原因说明
        #[arg(long, default_value = "", value_name = "TEXT")]
        reason: String,
        /// 纳入原子快照的目标路径 (可重复指定)
        #[arg(long = "path", value_name = "PATH")]
        paths: Vec<String>,
        /// 列出已有检查点而非创建新检查点
        #[arg(long)]
        list: bool,
    },
    /// 回滚至指定原子检查点 (支持检查点 ID 或 latest) 并追加审计记录
    Rollback {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 目标检查点 ID，或传 `latest` 回滚最近一次检查点
        #[arg(value_name = "CHECKPOINT_ID|latest")]
        checkpoint_id: String,
    },
    /// 查看或追加脱敏后的 NDJSON 操作审计日志
    AuditLog {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 返回最近 N 条审计记录 (默认 50)
        #[arg(long, default_value_t = 50, value_name = "N")]
        limit: usize,
        /// 追加一条新的审计记录
        #[arg(long)]
        append: bool,
        /// 追加模式下的动作名称 (如 ssh-harden / config-edit)
        #[arg(long, value_name = "STR")]
        action: Option<String>,
        /// 追加模式下的摘要说明 (写入前自动脱敏)
        #[arg(long, value_name = "STR")]
        summary: Option<String>,
        /// 追加模式下的状态标识 (默认 ok)
        #[arg(long, default_value = "ok", value_name = "STR")]
        status: String,
        /// 关联的检查点 ID (可选)
        #[arg(long, value_name = "STR")]
        checkpoint_id: Option<String>,
    },
    /// 管理宿主机长期维护记忆 (.agents/MEMORY.md 与 CoALA 分片记忆)
    Memory {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 追加到 .agents/MEMORY.md 的笔记内容 (兼容模式)
        #[arg(long, value_name = "TEXT")]
        append: Option<String>,
        /// 目标章节标题 (默认: 重要变更与排障记录)
        #[arg(long, default_value = "重要变更与排障记录", value_name = "STR")]
        section: String,
        /// CoALA 结构化记忆子命令
        #[command(subcommand)]
        subcommand: Option<Box<AgentMemoryAction>>,
    },
    /// 校验工作环境权限、凭据卫生与 U30 双受众文档格式契约
    Verify {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
    },
    /// 管理 0700/0600 凭据存储与 Systemd 安全注入配置
    Cred {
        #[command(subcommand)]
        action: AgentCredAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum AgentMemoryAction {
    /// 通过 Write Gate 写入结构化 CoALA 记忆分片 (违规退出码 21/22/23)
    Write {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 主题唯一 ID (slug 格式)
        #[arg(long, value_name = "ID")]
        id: String,
        /// 主题标题
        #[arg(long, value_name = "TITLE")]
        title: String,
        /// CoALA 记忆类型: episodic | semantic | procedural | profile
        #[arg(
            long = "type",
            alias = "topic-type",
            default_value = "semantic",
            value_name = "TYPE"
        )]
        topic_type: String,
        /// 作用域 (如 host | repo | service:nginx)
        #[arg(long, default_value = "host", value_name = "SCOPE")]
        scope: String,
        /// 标签列表 (可重复或逗号分隔)
        #[arg(
            long = "tag",
            alias = "tags",
            value_delimiter = ',',
            value_name = "TAG"
        )]
        tags: Vec<String>,
        /// 来源溯源路径或引用 (可重复或逗号分隔)
        #[arg(
            long = "source",
            alias = "sources",
            value_delimiter = ',',
            value_name = "SRC"
        )]
        sources: Vec<String>,
        /// 置信度 (0.0..=1.0，默认 0.85)
        #[arg(long, default_value_t = 0.85, value_name = "FLOAT")]
        confidence: f64,
        /// 过期天数 TTL (可选)
        #[arg(long, value_name = "DAYS")]
        ttl_days: Option<u32>,
        /// 取代的旧主题 ID (可选)
        #[arg(long, value_name = "ID")]
        supersedes: Option<String>,
        /// 允许原位更新同 ID 主题
        #[arg(long)]
        allow_update: bool,
        /// 正文内容 (与 --body-stdin 二选一)
        #[arg(long, value_name = "TEXT")]
        body: Option<String>,
        /// 从标准输入 (stdin) 读取正文内容
        #[arg(long)]
        body_stdin: bool,
    },
    /// 重建 .agents/cache/memory_index.json 与 .agents/MEMORY.md 目录索引
    Index {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
    },
    /// 基于倒排索引检索记忆分片并返回受限片段
    Search {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 检索关键词
        #[arg(value_name = "QUERY")]
        query: String,
        /// 最大返回条数 (默认 10)
        #[arg(long, default_value_t = 10, value_name = "N")]
        limit: usize,
    },
    /// 规划或执行记忆整合 (归档过期/被取代分片并识别可晋升候选)
    Consolidate {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 仅生成整合计划 (默认行为)
        #[arg(long)]
        plan: bool,
        /// 应用整合计划并创建检查点
        #[arg(long)]
        apply: bool,
    },
    /// 将成熟记忆分片晋升至规范文档并归档原分片
    Promote {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 目标记忆主题 ID
        #[arg(long, value_name = "ID")]
        id: String,
        /// 晋升目标规范文档相对路径 (如 .agents/constraints/10_network.md)
        #[arg(long, value_name = "PATH")]
        target: String,
    },
    /// 校验记忆防膨胀上限、Frontmatter 完整性与索引新鲜度
    Lint {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum AgentCredAction {
    /// 安全写入凭据 (严禁命令行明文参数，仅支持 --value-stdin 或 --from-file)
    Set {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 凭据逻辑名称 (仅允许字母数字与 . _ -)
        #[arg(long, value_name = "NAME")]
        name: String,
        /// 凭据类型: raw | env | systemd-encrypted
        #[arg(long, default_value = "raw", value_name = "KIND")]
        kind: String,
        /// 从标准输入 (stdin) 读取凭据内容
        #[arg(long)]
        value_stdin: bool,
        /// 从受保护本地文件读取凭据内容
        #[arg(long, value_name = "PATH")]
        from_file: Option<PathBuf>,
    },
    /// 列出已存储凭据元数据 (永不回显明文内容)
    List {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
    },
    /// 删除指定名称的托管凭据
    Remove {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 凭据逻辑名称
        #[arg(long, value_name = "NAME")]
        name: String,
    },
    /// 审计凭据目录权限、systemd-creds 可用性与明文泄漏风险
    Audit {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
    },
    /// 生成或写入加固的 Systemd 凭据 drop-in 片段 (LoadCredential / EnvironmentFile)
    SystemdSnippet {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 目标 systemd 单元名称 (如 myapp 或 myapp.service)
        #[arg(long, value_name = "UNIT")]
        unit: String,
        /// 凭据逻辑名称
        #[arg(long, value_name = "NAME")]
        name: String,
        /// 凭据类型: raw | env | systemd-encrypted
        #[arg(long, default_value = "raw", value_name = "KIND")]
        kind: String,
        /// 直接写入 drop-in 文件至 /etc/systemd/system/<unit>.d/90-debopty-credentials.conf
        #[arg(long)]
        write_dropin: bool,
    },
}

#[derive(Debug, Serialize)]
struct MemoryReadPayload {
    path: String,
    exists: bool,
    lines: usize,
    content: String,
}

#[derive(Debug, Serialize)]
struct MemoryAppendPayload {
    path: String,
    section: String,
    appended: bool,
}

#[derive(Debug, Serialize)]
struct CredRemovePayload {
    name: String,
    removed: bool,
}

#[derive(Debug, Serialize)]
struct SystemdSnippetPayload {
    unit: String,
    directive_kind: String,
    dropin_path: String,
    written: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    written_path: Option<String>,
    content: String,
}

fn resolve_workspace_dir(dir: Option<&Path>) -> PathBuf {
    dir.map(Path::to_path_buf)
        .or_else(|| non_empty_env("DEBOPTY_AGENT_WORKSPACE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR))
}

fn build_workspace_options(
    dir: Option<&Path>,
    root: Option<&Path>,
    template_dir: Option<&Path>,
    force: bool,
) -> AgentWorkspaceOptions {
    let root_prefix = root.unwrap_or_else(|| Path::new("/"));
    let ws_dir = resolve_workspace_dir(dir);
    let mut opts = AgentWorkspaceOptions::new(root_prefix, ws_dir);
    opts.template_dir = template_dir.map(Path::to_path_buf);
    opts.force_overwrite_managed = force;
    opts
}

fn parse_memory_tracking(raw: Option<&str>) -> Result<Option<MemoryTracking>, CliError> {
    raw.map(|s| MemoryTracking::from_str(s.trim()).map_err(|e| CliError::Usage(e.to_string())))
        .transpose()
}

fn parse_topic_type(raw: &str) -> Result<TopicType, CliError> {
    TopicType::from_str(raw.trim()).map_err(|e| CliError::Usage(e.to_string()))
}

fn parse_credential_kind(raw: &str) -> Result<CredentialKind, CliError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "raw" | "raw-file" => Ok(CredentialKind::RawFile),
        "env" | "env-file" => Ok(CredentialKind::EnvFile),
        "systemd-encrypted" | "encrypted" => Ok(CredentialKind::SystemdEncrypted),
        other => Err(CliError::Usage(format!(
            "未知凭据类型 '{other}'，合法值: raw | env | systemd-encrypted"
        ))),
    }
}

fn cred_filename(valid_name: &str, kind: CredentialKind) -> String {
    match kind {
        CredentialKind::RawFile => valid_name.to_string(),
        CredentialKind::EnvFile => {
            if valid_name.ends_with(".env") {
                valid_name.to_string()
            } else {
                format!("{valid_name}.env")
            }
        }
        CredentialKind::SystemdEncrypted => {
            if valid_name.ends_with(".cred") {
                valid_name.to_string()
            } else {
                format!("{valid_name}.cred")
            }
        }
    }
}

fn resolve_dropin_target(root_prefix: &Path, dropin_path: &str) -> PathBuf {
    if root_prefix.as_os_str().is_empty() || root_prefix == Path::new("/") {
        PathBuf::from(dropin_path)
    } else {
        root_prefix.join(dropin_path.trim_start_matches('/'))
    }
}

fn write_stdout_text(text: &str) -> Result<(), CliError> {
    let mut stdout = io::stdout().lock();
    match stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(err) => Err(CliError::Io(err)),
    }
}

pub fn handle_agent_ws(
    action: &AgentAction,
    template_dir: Option<&Path>,
    agent: &AgentCtx,
) -> Result<i32, CliError> {
    let runner = RealCommandRunner;

    match action {
        AgentAction::Init {
            dir,
            root,
            force,
            harnesses,
            memory_tracking,
            migrate_legacy,
            cleanup_legacy,
            adapter_file,
        } => {
            let mut opts =
                build_workspace_options(dir.as_deref(), root.as_deref(), template_dir, *force);
            opts.harnesses = harnesses.clone();
            opts.memory_tracking = parse_memory_tracking(memory_tracking.as_deref())?;
            opts.migrate_legacy = *migrate_legacy;
            opts.cleanup_legacy = *cleanup_legacy;
            opts.adapter_file = adapter_file.clone();

            let ws_dir = opts.effective_workspace_dir();
            if agent.dry_run {
                let mut actions = vec![
                    format!(
                        "Initialize or upgrade VPS agent workspace at {} (force={} harnesses={} memory_tracking={})",
                        ws_dir.display(),
                        force,
                        opts.harnesses.as_deref().unwrap_or("none"),
                        opts.memory_tracking.unwrap_or_default().as_str()
                    ),
                    "Provision dual-audience AGENTS.md, README.md, .agents/ constraints, schemas, CoALA memory, logs (0700), checkpoints (0700), and credentials (0700)".to_string(),
                ];
                if *migrate_legacy {
                    actions.push(format!(
                        "Run non-destructive legacy migration (cleanup_legacy={cleanup_legacy})"
                    ));
                }
                let plan = DryRunPlan {
                    command: "agent init".to_string(),
                    target: ws_dir.display().to_string(),
                    mutates_system: true,
                    requires_root: false,
                    requires_yes: false,
                    planned_actions: actions,
                    affected_paths: vec![
                        ws_dir.join("AGENTS.md").display().to_string(),
                        ws_dir.join("README.md").display().to_string(),
                        ws_dir.join(MEMORY_FILE_REL).display().to_string(),
                        ws_dir.join(REPO_BLUEPRINT_REL).display().to_string(),
                        ws_dir.join(".agents/workspace.json").display().to_string(),
                        ws_dir.join(AUDIT_LOG_REL).display().to_string(),
                        ws_dir.join(CHECKPOINTS_DIR_REL).display().to_string(),
                        ws_dir.join(CREDENTIALS_DIR_REL).display().to_string(),
                    ],
                };
                return emit_dry_run_plan(agent, &plan);
            }

            let report = init_workspace(&opts)?;
            if agent.json {
                return emit_envelope("agent init", false, &report);
            }
            let out = format!(
                "[agent init] workspace={} changed={} created={} updated={} preserved={} unchanged={} harnesses={} memory_tracking={} migrated={}\n  notice: {}\n",
                report.workspace_dir,
                report.changed,
                report.created_files.len(),
                report.updated_files.len(),
                report.preserved_files.len(),
                report.unchanged_files.len(),
                if report.active_harnesses.is_empty() {
                    "none".to_string()
                } else {
                    report.active_harnesses.join(",")
                },
                report.memory_tracking.as_str(),
                report.migrated_legacy_count,
                report.memory_tracking_notice,
            );
            write_stdout_text(&out)?;
            Ok(0)
        }

        AgentAction::Status { dir, root } => {
            let opts =
                build_workspace_options(dir.as_deref(), root.as_deref(), template_dir, false);
            let report = workspace_status(&opts, Some(&runner))?;
            if agent.json {
                return emit_envelope("agent status", false, &report);
            }
            let out = format!(
                "[agent status] workspace={} initialized={} healthy={} managed={}/{} memory_lines={} topics={} harnesses={} tracking={} legacy={} audit_entries={} checkpoints={} cred_ok={}\n",
                report.workspace_dir,
                report.initialized,
                report.healthy,
                report.managed_files_present,
                report.managed_files_expected,
                report.memory_lines,
                report.memory_topic_count,
                if report.active_harnesses.is_empty() {
                    "none".to_string()
                } else {
                    report.active_harnesses.join(",")
                },
                report.memory_tracking.as_str(),
                report.legacy_detected,
                report.audit_entries,
                report.checkpoint_count,
                report.credential_audit.ok
            );
            write_stdout_text(&out)?;
            Ok(0)
        }

        AgentAction::Audit { dir, root } => {
            let opts =
                build_workspace_options(dir.as_deref(), root.as_deref(), template_dir, false);
            let report = audit_workspace(&opts, Some(&runner))?;
            let exit_code: u8 = if report.secret_detected {
                EXIT_SECRET_DETECTED
            } else if !report.standard {
                EXIT_AUDIT_NONSTANDARD
            } else {
                0
            };
            let ok = exit_code == 0;
            if agent.json {
                return emit_envelope_status("agent audit", false, ok, exit_code, &report);
            }
            let mut out = format!(
                "[agent audit] workspace={} standard={} secret_detected={} findings={}\n",
                report.workspace_dir,
                report.standard,
                report.secret_detected,
                report.findings.len()
            );
            for f in &report.findings {
                out.push_str(&format!(
                    "  [{}] {}: {} (hint: {})\n",
                    f.severity, f.path, f.message, f.remediation
                ));
            }
            write_stdout_text(&out)?;
            Ok(i32::from(exit_code))
        }

        AgentAction::Migrate {
            dir,
            root,
            harnesses,
            memory_tracking,
            cleanup_legacy,
            adapter_file,
        } => {
            let mut opts =
                build_workspace_options(dir.as_deref(), root.as_deref(), template_dir, false);
            opts.harnesses = harnesses.clone();
            opts.memory_tracking = parse_memory_tracking(memory_tracking.as_deref())?;
            opts.migrate_legacy = true;
            opts.cleanup_legacy = *cleanup_legacy;
            opts.adapter_file = adapter_file.clone();

            let ws_dir = opts.effective_workspace_dir();
            if agent.dry_run {
                let plan = DryRunPlan {
                    command: "agent migrate".to_string(),
                    target: ws_dir.display().to_string(),
                    mutates_system: true,
                    requires_root: false,
                    requires_yes: false,
                    planned_actions: vec![
                        format!(
                            "Create pre-migration checkpoint and migrate legacy memories/constraints/skills/MCP in {} (cleanup_legacy={})",
                            ws_dir.display(),
                            cleanup_legacy
                        ),
                        format!(
                            "Apply SSOT v2 layout (harnesses={} memory_tracking={})",
                            opts.harnesses.as_deref().unwrap_or("none"),
                            opts.memory_tracking.unwrap_or_default().as_str()
                        ),
                    ],
                    affected_paths: vec![
                        ws_dir.join(MEMORY_FILE_REL).display().to_string(),
                        ws_dir.join(LEGACY_SSOT_REL).display().to_string(),
                        ws_dir.join(REPO_BLUEPRINT_REL).display().to_string(),
                        ws_dir.join(CHECKPOINTS_DIR_REL).display().to_string(),
                    ],
                };
                return emit_dry_run_plan(agent, &plan);
            }

            let report = migrate_workspace(&opts, Some(&runner))?;
            if agent.json {
                return emit_envelope("agent migrate", false, &report);
            }
            let out = format!(
                "[agent migrate] workspace={} changed={} checkpoint={} memories={} constraints={} skills={} mcp={} cleaned={} skipped={}\n  notice: {}\n",
                report.workspace_dir,
                report.changed,
                report.checkpoint_id.as_deref().unwrap_or("none"),
                report.migrated_memories.len(),
                report.migrated_constraints.len(),
                report.migrated_skills.len(),
                report.migrated_mcp_servers,
                report.cleaned_legacy_files.len(),
                report.already_migrated_skipped,
                report.memory_tracking_notice,
            );
            write_stdout_text(&out)?;
            Ok(0)
        }

        AgentAction::Switch {
            dir,
            root,
            to,
            add,
            remove,
            harvest,
            force,
            adapter_file,
        } => {
            let selected = [
                to.as_deref().map(|s| (SwitchMode::To, s)),
                add.as_deref().map(|s| (SwitchMode::Add, s)),
                remove.as_deref().map(|s| (SwitchMode::Remove, s)),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            if selected.len() != 1 {
                return Err(CliError::Usage(
                    "`debopty agent switch` 必须且仅能指定 `--to <SPEC>`、`--add <SPEC>` 或 `--remove <SPEC>` 之一。"
                        .to_string(),
                ));
            }
            let (mode, spec) = selected[0];
            let mut opts =
                build_workspace_options(dir.as_deref(), root.as_deref(), template_dir, false);
            opts.adapter_file = adapter_file.clone();
            let ws_dir = opts.effective_workspace_dir();

            if agent.dry_run {
                let mode_str = match mode {
                    SwitchMode::To => "to",
                    SwitchMode::Add => "add",
                    SwitchMode::Remove => "remove",
                };
                let plan = DryRunPlan {
                    command: "agent switch".to_string(),
                    target: format!("{mode_str}:{spec}"),
                    mutates_system: true,
                    requires_root: false,
                    requires_yes: false,
                    planned_actions: vec![format!(
                        "Switch harness projections mode={mode_str} spec='{spec}' harvest={harvest} force={force}"
                    )],
                    affected_paths: vec![
                        ws_dir.join(REPO_BLUEPRINT_REL).display().to_string(),
                        ws_dir.join(LEGACY_SSOT_REL).display().to_string(),
                    ],
                };
                return emit_dry_run_plan(agent, &plan);
            }

            let report = switch_harnesses(&opts, mode, spec, *harvest, *force)?;
            if agent.json {
                return emit_envelope("agent switch", false, &report);
            }
            let out = format!(
                "[agent switch] changed={} active={} written={} removed={} harvested={} checkpoint={}\n",
                report.changed,
                if report.active_harnesses.is_empty() {
                    "none".to_string()
                } else {
                    report.active_harnesses.join(",")
                },
                report.written_projections.len(),
                report.removed_projections.len(),
                report.harvested_projections.len(),
                report.checkpoint_id.as_deref().unwrap_or("none"),
            );
            write_stdout_text(&out)?;
            Ok(0)
        }

        AgentAction::Override {
            dir,
            root,
            init,
            list,
            remove,
            json_format,
        } => {
            let mode_count =
                usize::from(init.is_some()) + usize::from(*list) + usize::from(remove.is_some());
            if mode_count > 1 {
                return Err(CliError::Usage(
                    "`debopty agent override` 中 `--init`、`--list` 与 `--remove` 互斥，请仅指定其一。"
                        .to_string(),
                ));
            }
            let opts =
                build_workspace_options(dir.as_deref(), root.as_deref(), template_dir, false);
            let ws_dir = opts.effective_workspace_dir();

            let ov_action = if let Some(harness) = init {
                OverrideAction::Init {
                    harness: harness.clone(),
                    json: *json_format,
                }
            } else if let Some(harness) = remove {
                OverrideAction::Remove {
                    harness: harness.clone(),
                }
            } else {
                OverrideAction::List
            };

            if agent.dry_run && !matches!(ov_action, OverrideAction::List) {
                let (act_desc, target) = match &ov_action {
                    OverrideAction::Init { harness, json } => (
                        format!("Initialize harness override file for '{harness}' (json={json})"),
                        harness.clone(),
                    ),
                    OverrideAction::Remove { harness } => (
                        format!("Remove harness override file for '{harness}'"),
                        harness.clone(),
                    ),
                    OverrideAction::List => unreachable!(),
                };
                let plan = DryRunPlan {
                    command: "agent override".to_string(),
                    target,
                    mutates_system: true,
                    requires_root: false,
                    requires_yes: false,
                    planned_actions: vec![act_desc],
                    affected_paths: vec![ws_dir.join(OVERRIDES_DIR_REL).display().to_string()],
                };
                return emit_dry_run_plan(agent, &plan);
            }

            let report = manage_override(&opts, ov_action)?;
            if agent.json {
                return emit_envelope("agent override", false, &report);
            }
            let mut out = format!(
                "[agent override] action={} changed={} count={}",
                report.action,
                report.changed,
                report.overrides.len()
            );
            if let Some(ref p) = report.target_path {
                out.push_str(&format!(" target={p}"));
            }
            out.push('\n');
            for item in &report.overrides {
                out.push_str(&format!("  {item}\n"));
            }
            write_stdout_text(&out)?;
            Ok(0)
        }

        AgentAction::HarnessList {
            dir,
            root,
            adapter_file,
        } => {
            let mut opts =
                build_workspace_options(dir.as_deref(), root.as_deref(), template_dir, false);
            opts.adapter_file = adapter_file.clone();
            let registry = load_adapter_registry(&opts)?;
            if agent.json {
                return emit_envelope("agent harness-list", false, &registry);
            }
            let mut out = format!(
                "[agent harness-list] wire_epoch={} presets={} adapters={} custom={}\n",
                registry.wire_epoch,
                registry.presets.len(),
                registry.adapters.len(),
                registry.custom_adapters.len()
            );
            for (preset, members) in &registry.presets {
                out.push_str(&format!("  preset {preset}: {}\n", members.join(",")));
            }
            for (id, spec) in &registry.adapters {
                let paths = if spec.projection_paths.is_empty() {
                    "(canonical AGENTS.md)".to_string()
                } else {
                    spec.projection_paths.join(", ")
                };
                out.push_str(&format!("  adapter {id} [{}]: {paths}\n", spec.format));
            }
            for (id, spec) in &registry.custom_adapters {
                let paths = if spec.projection_paths.is_empty() {
                    "(canonical AGENTS.md)".to_string()
                } else {
                    spec.projection_paths.join(", ")
                };
                out.push_str(&format!("  custom {id} [{}]: {paths}\n", spec.format));
            }
            write_stdout_text(&out)?;
            Ok(0)
        }

        AgentAction::Checkpoint {
            dir,
            root,
            label,
            reason,
            paths,
            list,
        } => {
            let opts =
                build_workspace_options(dir.as_deref(), root.as_deref(), template_dir, false);
            let ws_dir = opts.effective_workspace_dir();
            if *list {
                let items = list_checkpoints(&opts)?;
                if agent.json {
                    return emit_envelope("agent checkpoint", false, &items);
                }
                if items.is_empty() {
                    write_stdout_text("(no checkpoints)\n")?;
                } else {
                    let mut out = String::new();
                    for cp in &items {
                        out.push_str(&format!(
                            "{}  {}  label={}  entries={}  reason={}\n",
                            cp.checkpoint_id, cp.created_at, cp.label, cp.entry_count, cp.reason
                        ));
                    }
                    write_stdout_text(&out)?;
                }
                return Ok(0);
            }

            if agent.dry_run {
                let mut affected = paths.clone();
                affected.push(ws_dir.join(CHECKPOINTS_DIR_REL).display().to_string());
                affected.push(ws_dir.join(AUDIT_LOG_REL).display().to_string());
                let plan = DryRunPlan {
                    command: "agent checkpoint".to_string(),
                    target: label.clone(),
                    mutates_system: true,
                    requires_root: false,
                    requires_yes: false,
                    planned_actions: vec![format!(
                        "Create atomic checkpoint '{}' covering {} path(s)",
                        label,
                        paths.len()
                    )],
                    affected_paths: affected,
                };
                return emit_dry_run_plan(agent, &plan);
            }

            let meta = create_checkpoint(&opts, label, reason, paths, "agent-cli")?;
            if agent.json {
                return emit_envelope("agent checkpoint", false, &meta);
            }
            let out = format!(
                "[agent checkpoint] id={} label={} entries={} backup_dir={}\n",
                meta.checkpoint_id, meta.label, meta.entry_count, meta.backup_dir
            );
            write_stdout_text(&out)?;
            Ok(0)
        }

        AgentAction::Rollback {
            dir,
            root,
            checkpoint_id,
        } => {
            let opts =
                build_workspace_options(dir.as_deref(), root.as_deref(), template_dir, false);
            let ws_dir = opts.effective_workspace_dir();
            if agent.dry_run {
                let plan = DryRunPlan {
                    command: "agent rollback".to_string(),
                    target: checkpoint_id.clone(),
                    mutates_system: true,
                    requires_root: false,
                    requires_yes: true,
                    planned_actions: vec![format!(
                        "Restore files atomically from checkpoint '{checkpoint_id}' and append audit entry"
                    )],
                    affected_paths: vec![
                        ws_dir
                            .join(CHECKPOINTS_DIR_REL)
                            .join(checkpoint_id)
                            .display()
                            .to_string(),
                        ws_dir.join(AUDIT_LOG_REL).display().to_string(),
                    ],
                };
                return emit_dry_run_plan(agent, &plan);
            }

            if agent.explicit_non_interactive && !agent.yes {
                return Err(CliError::ConfirmationRequired(format!(
                    "非交互模式执行 `debopty agent rollback {checkpoint_id}` 将还原受管文件状态，请追加 `--yes` (`-y`) 确认，或使用 `--dry-run` 预览。"
                )));
            }

            let report = rollback_checkpoint(&opts, checkpoint_id, "agent-cli")?;
            if agent.json {
                return emit_envelope("agent rollback", false, &report);
            }
            let out = format!(
                "[agent rollback] id={} restored_entries={} audit_recorded={}\n",
                report.checkpoint_id, report.restored_entries, report.audit_recorded
            );
            write_stdout_text(&out)?;
            Ok(0)
        }

        AgentAction::AuditLog {
            dir,
            root,
            limit,
            append,
            action: audit_action,
            summary,
            status,
            checkpoint_id,
        } => {
            let opts =
                build_workspace_options(dir.as_deref(), root.as_deref(), template_dir, false);
            let ws_dir = opts.effective_workspace_dir();

            if *append {
                let act = audit_action
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        CliError::Usage(
                            "`debopty agent audit-log --append` 必须同时提供 `--action <STR>`"
                                .to_string(),
                        )
                    })?;
                let sum = summary
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        CliError::Usage(
                            "`debopty agent audit-log --append` 必须同时提供 `--summary <STR>`"
                                .to_string(),
                        )
                    })?;

                if agent.dry_run {
                    let plan = DryRunPlan {
                        command: "agent audit-log".to_string(),
                        target: act.to_string(),
                        mutates_system: true,
                        requires_root: false,
                        requires_yes: false,
                        planned_actions: vec![format!(
                            "Append redacted audit entry action='{act}' status='{status}'"
                        )],
                        affected_paths: vec![ws_dir.join(AUDIT_LOG_REL).display().to_string()],
                    };
                    return emit_dry_run_plan(agent, &plan);
                }

                let entry = AuditEntry {
                    ts: diag::utc_rfc3339_now(),
                    action: act.to_string(),
                    actor: "agent-cli".to_string(),
                    status: status.trim().to_string(),
                    checkpoint_id: checkpoint_id.clone(),
                    summary: diag::redact_sensitive(sum),
                    paths: Vec::new(),
                    details: BTreeMap::new(),
                };
                append_audit_entry(&ws_dir, &entry)?;
                if agent.json {
                    return emit_envelope("agent audit-log", false, &entry);
                }
                let out = format!(
                    "[agent audit-log] appended ts={} action={} status={}\n",
                    entry.ts, entry.action, entry.status
                );
                write_stdout_text(&out)?;
                return Ok(0);
            }

            let entries = read_audit_log(&ws_dir, *limit)?;
            if agent.json {
                return emit_envelope("agent audit-log", false, &entries);
            }
            if entries.is_empty() {
                write_stdout_text("(no audit entries)\n")?;
            } else {
                let mut out = String::new();
                for e in &entries {
                    out.push_str(&format!(
                        "{}  [{}]  {}  {}\n",
                        e.ts,
                        e.status,
                        e.action,
                        diag::redact_sensitive(&e.summary)
                    ));
                }
                write_stdout_text(&out)?;
            }
            Ok(0)
        }

        AgentAction::Memory {
            dir,
            root,
            append,
            section,
            subcommand,
        } => {
            if let Some(sub) = subcommand {
                return handle_agent_memory(
                    sub.as_ref(),
                    dir.as_deref(),
                    root.as_deref(),
                    template_dir,
                    agent,
                );
            }
            let opts =
                build_workspace_options(dir.as_deref(), root.as_deref(), template_dir, false);
            let ws_dir = opts.effective_workspace_dir();
            let mem_path = ws_dir.join(MEMORY_FILE_REL);

            if let Some(note) = append {
                if agent.dry_run {
                    let plan = DryRunPlan {
                        command: "agent memory".to_string(),
                        target: section.clone(),
                        mutates_system: true,
                        requires_root: false,
                        requires_yes: false,
                        planned_actions: vec![format!(
                            "Append redacted memory note under section '{section}'"
                        )],
                        affected_paths: vec![mem_path.display().to_string()],
                    };
                    return emit_dry_run_plan(agent, &plan);
                }

                let appended = append_memory_note(&ws_dir, section, note)?;
                let payload = MemoryAppendPayload {
                    path: mem_path.display().to_string(),
                    section: section.clone(),
                    appended,
                };
                if agent.json {
                    return emit_envelope("agent memory", false, &payload);
                }
                let out = format!(
                    "[agent memory] appended={} section={} path={}\n",
                    payload.appended, payload.section, payload.path
                );
                write_stdout_text(&out)?;
                return Ok(0);
            }

            let (exists, content) = if mem_path.is_file() {
                (
                    true,
                    diag::redact_sensitive(&fs::read_to_string(&mem_path)?),
                )
            } else {
                (false, String::new())
            };
            let payload = MemoryReadPayload {
                path: mem_path.display().to_string(),
                exists,
                lines: content.lines().count(),
                content,
            };
            if agent.json {
                return emit_envelope("agent memory", false, &payload);
            }
            if !payload.exists {
                write_stdout_text("(.agents/MEMORY.md not initialized)\n")?;
            } else {
                write_stdout_text(&payload.content)?;
            }
            Ok(0)
        }

        AgentAction::Verify { dir, root } => {
            let opts =
                build_workspace_options(dir.as_deref(), root.as_deref(), template_dir, false);
            let report = verify_workspace(&opts, Some(&runner))?;
            let exit_code: u8 = if report.passed { 0 } else { 1 };
            if agent.json {
                return emit_envelope_status(
                    "agent verify",
                    false,
                    report.passed,
                    exit_code,
                    &report,
                );
            }
            let mut out = format!(
                "[agent verify] workspace={} passed={} checks={} errors={} warnings={}\n",
                report.workspace_dir,
                report.passed,
                report.checks_run,
                report.errors.len(),
                report.warnings.len()
            );
            for err in &report.errors {
                out.push_str(&format!("  error: {err}\n"));
            }
            for warn in &report.warnings {
                out.push_str(&format!("  warn: {warn}\n"));
            }
            write_stdout_text(&out)?;
            Ok(i32::from(exit_code))
        }

        AgentAction::Cred { action: cred_act } => handle_agent_cred(cred_act, template_dir, agent),
    }
}

fn handle_agent_memory(
    action: &AgentMemoryAction,
    parent_dir: Option<&Path>,
    parent_root: Option<&Path>,
    template_dir: Option<&Path>,
    agent: &AgentCtx,
) -> Result<i32, CliError> {
    match action {
        AgentMemoryAction::Write {
            dir,
            root,
            id,
            title,
            topic_type,
            scope,
            tags,
            sources,
            confidence,
            ttl_days,
            supersedes,
            allow_update,
            body,
            body_stdin,
        } => {
            if body.is_some() && *body_stdin {
                return Err(CliError::Usage(
                    "`--body` 与 `--body-stdin` 不能同时指定，请二选一。".to_string(),
                ));
            }
            let parsed_type = parse_topic_type(topic_type)?;
            let eff_dir = dir.as_deref().or(parent_dir);
            let eff_root = root.as_deref().or(parent_root);
            let opts = build_workspace_options(eff_dir, eff_root, template_dir, false);
            let ws_dir = opts.effective_workspace_dir();

            if agent.dry_run {
                let plan = DryRunPlan {
                    command: "agent memory write".to_string(),
                    target: id.clone(),
                    mutates_system: true,
                    requires_root: false,
                    requires_yes: false,
                    planned_actions: vec![format!(
                        "Validate Write Gate and write CoALA topic '{}' (type={} scope={})",
                        id,
                        parsed_type.as_str(),
                        scope
                    )],
                    affected_paths: vec![
                        ws_dir
                            .join(MEMORY_DIR_REL)
                            .join(format!("{id}.md"))
                            .display()
                            .to_string(),
                        ws_dir.join(MEMORY_FILE_REL).display().to_string(),
                        ws_dir.join(MEMORY_INDEX_CACHE_REL).display().to_string(),
                    ],
                };
                return emit_dry_run_plan(agent, &plan);
            }

            let body_text = if *body_stdin {
                let mut buf = String::new();
                io::stdin().read_to_string(&mut buf)?;
                buf
            } else {
                body.clone().unwrap_or_default()
            };

            let input = MemoryTopicInput {
                id: id.clone(),
                title: title.clone(),
                topic_type: parsed_type,
                status: TopicStatus::Active,
                scope: scope.clone(),
                tags: tags.clone(),
                sources: sources.clone(),
                confidence: *confidence,
                body: body_text,
                ttl_days: *ttl_days,
                expires_at: None,
                supersedes: supersedes.clone(),
                allow_update: *allow_update,
                actor: Some("agent-cli".to_string()),
            };
            let report = write_memory_topic(&opts, &input)?;
            if agent.json {
                return emit_envelope("agent memory write", false, &report);
            }
            let out = format!(
                "[agent memory write] id={} verdict={:?} changed={} rel_path={} toc_updated={} index_updated={}\n",
                report.topic_id,
                report.verdict,
                report.changed,
                report.rel_path,
                report.toc_updated,
                report.index_updated,
            );
            write_stdout_text(&out)?;
            Ok(0)
        }

        AgentMemoryAction::Index { dir, root } => {
            let eff_dir = dir.as_deref().or(parent_dir);
            let eff_root = root.as_deref().or(parent_root);
            let opts = build_workspace_options(eff_dir, eff_root, template_dir, false);
            let ws_dir = opts.effective_workspace_dir();

            if agent.dry_run {
                let plan = DryRunPlan {
                    command: "agent memory index".to_string(),
                    target: ws_dir.display().to_string(),
                    mutates_system: true,
                    requires_root: false,
                    requires_yes: false,
                    planned_actions: vec![
                        "Rebuild .agents/cache/memory_index.json and .agents/MEMORY.md TOC"
                            .to_string(),
                    ],
                    affected_paths: vec![
                        ws_dir.join(MEMORY_INDEX_CACHE_REL).display().to_string(),
                        ws_dir.join(MEMORY_FILE_REL).display().to_string(),
                    ],
                };
                return emit_dry_run_plan(agent, &plan);
            }

            let report = rebuild_memory_index(&opts)?;
            if agent.json {
                return emit_envelope("agent memory index", false, &report);
            }
            let out = format!(
                "[agent memory index] total={} active={} archived={} rebuilt={} reused={} toc_lines={} toc_bytes={}\n",
                report.total_topics,
                report.active_topics,
                report.archived_topics,
                report.rebuilt_entries,
                report.reused_entries,
                report.toc_lines,
                report.toc_bytes,
            );
            write_stdout_text(&out)?;
            Ok(0)
        }

        AgentMemoryAction::Search {
            dir,
            root,
            query,
            limit,
        } => {
            let eff_dir = dir.as_deref().or(parent_dir);
            let eff_root = root.as_deref().or(parent_root);
            let opts = build_workspace_options(eff_dir, eff_root, template_dir, false);
            let report = search_memory(&opts, query, *limit)?;
            if agent.json {
                return emit_envelope("agent memory search", false, &report);
            }
            if report.hits.is_empty() {
                write_stdout_text("(no matching memory topics)\n")?;
            } else {
                let mut out = format!(
                    "[agent memory search] query='{}' hits={}\n",
                    report.query, report.total_hits
                );
                for hit in &report.hits {
                    out.push_str(&format!(
                        "  [score={:.2}] {} ({}) {}\n",
                        hit.score, hit.topic_id, hit.rel_path, hit.title
                    ));
                }
                write_stdout_text(&out)?;
            }
            Ok(0)
        }

        AgentMemoryAction::Consolidate {
            dir,
            root,
            plan,
            apply,
        } => {
            if *plan && *apply {
                return Err(CliError::Usage(
                    "`--plan` 与 `--apply` 互斥，请仅指定其一。".to_string(),
                ));
            }
            let eff_dir = dir.as_deref().or(parent_dir);
            let eff_root = root.as_deref().or(parent_root);
            let opts = build_workspace_options(eff_dir, eff_root, template_dir, false);
            let ws_dir = opts.effective_workspace_dir();
            let should_apply = *apply && !*plan;

            if should_apply && agent.dry_run {
                let dry_plan = DryRunPlan {
                    command: "agent memory consolidate".to_string(),
                    target: ws_dir.display().to_string(),
                    mutates_system: true,
                    requires_root: false,
                    requires_yes: false,
                    planned_actions: vec![
                        "Archive expired/superseded topic shards and rebuild memory index"
                            .to_string(),
                    ],
                    affected_paths: vec![
                        ws_dir.join(MEMORY_DIR_REL).display().to_string(),
                        ws_dir.join(MEMORY_FILE_REL).display().to_string(),
                    ],
                };
                return emit_dry_run_plan(agent, &dry_plan);
            }

            let report = consolidate_memory(&opts, should_apply)?;
            if agent.json {
                return emit_envelope("agent memory consolidate", false, &report);
            }
            let mut out = format!(
                "[agent memory consolidate] applied={} items={} archived={} promotion_candidates={}\n",
                report.applied,
                report.items.len(),
                report.archived_count,
                report.promotion_candidates,
            );
            for item in &report.items {
                out.push_str(&format!(
                    "  [{}] {} ({}): {}\n",
                    item.action, item.topic_id, item.rel_path, item.reason
                ));
            }
            write_stdout_text(&out)?;
            Ok(0)
        }

        AgentMemoryAction::Promote {
            dir,
            root,
            id,
            target,
        } => {
            let eff_dir = dir.as_deref().or(parent_dir);
            let eff_root = root.as_deref().or(parent_root);
            let opts = build_workspace_options(eff_dir, eff_root, template_dir, false);
            let ws_dir = opts.effective_workspace_dir();

            if agent.dry_run {
                let plan = DryRunPlan {
                    command: "agent memory promote".to_string(),
                    target: format!("{id} -> {target}"),
                    mutates_system: true,
                    requires_root: false,
                    requires_yes: false,
                    planned_actions: vec![format!(
                        "Promote memory topic '{id}' into '{target}' and archive source shard"
                    )],
                    affected_paths: vec![
                        ws_dir.join(target).display().to_string(),
                        ws_dir.join(MEMORY_DIR_REL).display().to_string(),
                        ws_dir.join(MEMORY_FILE_REL).display().to_string(),
                    ],
                };
                return emit_dry_run_plan(agent, &plan);
            }

            let report = promote_memory(&opts, id, target)?;
            if agent.json {
                return emit_envelope("agent memory promote", false, &report);
            }
            let out = format!(
                "[agent memory promote] id={} target={} archive={} promoted={}\n",
                report.topic_id, report.target_doc, report.archive_path, report.promoted,
            );
            write_stdout_text(&out)?;
            Ok(0)
        }

        AgentMemoryAction::Lint { dir, root } => {
            let eff_dir = dir.as_deref().or(parent_dir);
            let eff_root = root.as_deref().or(parent_root);
            let opts = build_workspace_options(eff_dir, eff_root, template_dir, false);
            let report = lint_memory(&opts)?;
            let exit_code: u8 = if report.passed { 0 } else { 1 };
            if agent.json {
                return emit_envelope_status(
                    "agent memory lint",
                    false,
                    report.passed,
                    exit_code,
                    &report,
                );
            }
            let mut out = format!(
                "[agent memory lint] passed={} toc_lines={} toc_bytes={} topics={} active={} index_fresh={} errors={} warnings={}\n",
                report.passed,
                report.toc_lines,
                report.toc_bytes,
                report.topic_count,
                report.active_topic_count,
                report.index_fresh,
                report.errors.len(),
                report.warnings.len(),
            );
            for err in &report.errors {
                out.push_str(&format!("  error: {err}\n"));
            }
            for warn in &report.warnings {
                out.push_str(&format!("  warn: {warn}\n"));
            }
            write_stdout_text(&out)?;
            Ok(i32::from(exit_code))
        }
    }
}

fn handle_agent_cred(
    action: &AgentCredAction,
    template_dir: Option<&Path>,
    agent: &AgentCtx,
) -> Result<i32, CliError> {
    let runner = RealCommandRunner;

    match action {
        AgentCredAction::Set {
            dir,
            name,
            kind,
            value_stdin,
            from_file,
        } => {
            let valid_name = validate_credential_name(name)?;
            let cred_kind = parse_credential_kind(kind)?;
            if !*value_stdin && from_file.is_none() {
                return Err(CliError::Usage(
                    "严禁通过命令行明文传入凭据：必须显式指定 `--value-stdin` 或 `--from-file <PATH>`。"
                        .to_string(),
                ));
            }
            if *value_stdin && from_file.is_some() {
                return Err(CliError::Usage(
                    "`--value-stdin` 与 `--from-file` 不能同时指定，请二选一。".to_string(),
                ));
            }

            let ws_dir = resolve_workspace_dir(dir.as_deref());
            let cred_dir = ws_dir.join(CREDENTIALS_DIR_REL);
            let target_file = cred_dir.join(cred_filename(&valid_name, cred_kind));

            if agent.dry_run {
                let plan = DryRunPlan {
                    command: "agent cred set".to_string(),
                    target: valid_name.clone(),
                    mutates_system: true,
                    requires_root: false,
                    requires_yes: false,
                    planned_actions: vec![format!(
                        "Store credential '{}' (kind={}) with 0700 dir / 0600 file mode",
                        valid_name, kind
                    )],
                    affected_paths: vec![
                        cred_dir.display().to_string(),
                        target_file.display().to_string(),
                    ],
                };
                return emit_dry_run_plan(agent, &plan);
            }

            let secret_bytes = if *value_stdin {
                let mut buf = Vec::new();
                io::stdin().read_to_end(&mut buf)?;
                buf
            } else if let Some(path) = from_file {
                fs::read(path)?
            } else {
                unreachable!("validated above")
            };

            if secret_bytes.is_empty() {
                return Err(CliError::Usage("凭据内容为空，已拒绝写入。".to_string()));
            }

            let meta = store_credential(
                &cred_dir,
                &valid_name,
                &secret_bytes,
                cred_kind,
                Some(&runner),
            )?;
            if agent.json {
                return emit_envelope("agent cred set", false, &meta);
            }
            let out = format!(
                "[agent cred set] name={} path={} mode={:04o} size={} encrypted={}\n",
                meta.name, meta.path, meta.mode, meta.size_bytes, meta.encrypted
            );
            write_stdout_text(&out)?;
            Ok(0)
        }

        AgentCredAction::List { dir } => {
            let ws_dir = resolve_workspace_dir(dir.as_deref());
            let cred_dir = ws_dir.join(CREDENTIALS_DIR_REL);
            let items = list_credentials(&cred_dir)?;
            if agent.json {
                return emit_envelope("agent cred list", false, &items);
            }
            if items.is_empty() {
                write_stdout_text("(no managed credentials)\n")?;
            } else {
                let mut out = String::new();
                for c in &items {
                    out.push_str(&format!(
                        "{}  mode={:04o}  size={}  encrypted={}  perms_ok={}  path={}\n",
                        c.name, c.mode, c.size_bytes, c.encrypted, c.perms_ok, c.path
                    ));
                }
                write_stdout_text(&out)?;
            }
            Ok(0)
        }

        AgentCredAction::Remove { dir, name } => {
            let valid_name = validate_credential_name(name)?;
            let ws_dir = resolve_workspace_dir(dir.as_deref());
            let cred_dir = ws_dir.join(CREDENTIALS_DIR_REL);

            if agent.dry_run {
                let plan = DryRunPlan {
                    command: "agent cred remove".to_string(),
                    target: valid_name.clone(),
                    mutates_system: true,
                    requires_root: false,
                    requires_yes: true,
                    planned_actions: vec![format!(
                        "Remove managed credential '{}' from {}",
                        valid_name,
                        cred_dir.display()
                    )],
                    affected_paths: vec![cred_dir.join(&valid_name).display().to_string()],
                };
                return emit_dry_run_plan(agent, &plan);
            }

            if agent.explicit_non_interactive && !agent.yes {
                return Err(CliError::ConfirmationRequired(format!(
                    "非交互模式执行 `debopty agent cred remove --name {valid_name}` 将删除受管凭据，请追加 `--yes` (`-y`) 确认，或使用 `--dry-run` 预览。"
                )));
            }

            let removed = remove_credential(&cred_dir, &valid_name)?;
            let payload = CredRemovePayload {
                name: valid_name,
                removed,
            };
            if agent.json {
                return emit_envelope("agent cred remove", false, &payload);
            }
            let out = format!(
                "[agent cred remove] name={} removed={}\n",
                payload.name, payload.removed
            );
            write_stdout_text(&out)?;
            Ok(0)
        }

        AgentCredAction::Audit { dir, root } => {
            let opts =
                build_workspace_options(dir.as_deref(), root.as_deref(), template_dir, false);
            let ws_dir = opts.effective_workspace_dir();
            let cred_dir = ws_dir.join(CREDENTIALS_DIR_REL);
            let mut extra_scan = vec![ws_dir.join(MEMORY_FILE_REL), ws_dir.join(AUDIT_LOG_REL)];
            let systemd_dir = opts.root_prefix.join("etc/systemd/system");
            if systemd_dir.is_dir() {
                extra_scan.push(systemd_dir);
            }
            let report = audit_credentials(&cred_dir, &extra_scan, Some(&runner));
            if agent.json {
                return emit_envelope("agent cred audit", false, &report);
            }
            let mut out = format!(
                "[agent cred audit] dir={} ok={} credentials={} findings={} systemd_creds={}\n",
                report.cred_dir,
                report.ok,
                report.credentials.len(),
                report.findings.len(),
                report.systemd_creds_available
            );
            for f in &report.findings {
                out.push_str(&format!(
                    "  [{}] {} ({}): {}\n",
                    f.severity, f.code, f.path, f.message
                ));
            }
            write_stdout_text(&out)?;
            Ok(0)
        }

        AgentCredAction::SystemdSnippet {
            dir,
            root,
            unit,
            name,
            kind,
            write_dropin,
        } => {
            let valid_name = validate_credential_name(name)?;
            let cred_kind = parse_credential_kind(kind)?;
            let opts =
                build_workspace_options(dir.as_deref(), root.as_deref(), template_dir, false);
            let ws_dir = opts.effective_workspace_dir();
            let cred_dir = ws_dir.join(CREDENTIALS_DIR_REL);
            let cred_path = cred_dir.join(cred_filename(&valid_name, cred_kind));
            let tpl = TemplateEngine::new(template_dir.map(Path::to_path_buf));

            let snippet = render_systemd_dropin(&tpl, unit, &valid_name, &cred_path, cred_kind)?;
            let target_dropin = resolve_dropin_target(&opts.root_prefix, &snippet.dropin_path);

            if *write_dropin && agent.dry_run {
                let plan = DryRunPlan {
                    command: "agent cred systemd-snippet".to_string(),
                    target: snippet.unit.clone(),
                    mutates_system: true,
                    requires_root: true,
                    requires_yes: false,
                    planned_actions: vec![format!(
                        "Write hardened systemd credential drop-in ({}) to {}",
                        snippet.directive_kind,
                        target_dropin.display()
                    )],
                    affected_paths: vec![target_dropin.display().to_string()],
                };
                return emit_dry_run_plan(agent, &plan);
            }

            let written_path = if *write_dropin {
                if let Some(parent) = target_dropin.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&target_dropin, snippet.content.as_bytes())?;
                fs::set_permissions(&target_dropin, fs::Permissions::from_mode(0o644))?;
                Some(target_dropin.display().to_string())
            } else {
                None
            };

            let payload = SystemdSnippetPayload {
                unit: snippet.unit,
                directive_kind: snippet.directive_kind,
                dropin_path: snippet.dropin_path,
                written: written_path.is_some(),
                written_path,
                content: snippet.content,
            };

            if agent.json {
                return emit_envelope("agent cred systemd-snippet", false, &payload);
            }
            write_stdout_text(&payload.content)?;
            Ok(0)
        }
    }
}
