use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use debopty_core::agent_workspace::{
    AUDIT_LOG_REL, AgentWorkspaceOptions, AuditEntry, CHECKPOINTS_DIR_REL, CREDENTIALS_DIR_REL,
    DEFAULT_WORKSPACE_DIR, MEMORY_FILE_REL, append_audit_entry, append_memory_note,
    create_checkpoint, init_workspace, list_checkpoints, read_audit_log, rollback_checkpoint,
    verify_workspace, workspace_status,
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
    /// 初始化或非破坏性升级宿主机 Agent 预设工作环境
    Init {
        /// Agent 工作目录覆盖 (默认读取 DEBOPTY_AGENT_WORKSPACE 或 /var/lib/debopty/agent)
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖 (用于隔离测试或自定义 rootfs)
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 强制覆盖受管文档模板 (仍保留 MEMORY.md 与 audit.jsonl)
        #[arg(long)]
        force: bool,
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
    /// 回滚至指定原子检查点并追加审计记录
    Rollback {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 目标检查点 ID
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
    /// 查看或追加宿主机长期维护记忆 (MEMORY.md，写入前自动脱敏)
    Memory {
        /// Agent 工作目录覆盖
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
        /// 根目录覆盖
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 追加到 MEMORY.md 的笔记内容
        #[arg(long, value_name = "TEXT")]
        append: Option<String>,
        /// 目标章节标题 (默认: 重要变更与排障记录)
        #[arg(long, default_value = "重要变更与排障记录", value_name = "STR")]
        section: String,
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
        AgentAction::Init { dir, root, force } => {
            let opts =
                build_workspace_options(dir.as_deref(), root.as_deref(), template_dir, *force);
            let ws_dir = opts.effective_workspace_dir();
            if agent.dry_run {
                let plan = DryRunPlan {
                    command: "agent init".to_string(),
                    target: ws_dir.display().to_string(),
                    mutates_system: true,
                    requires_root: false,
                    requires_yes: false,
                    planned_actions: vec![
                        format!(
                            "Initialize or upgrade VPS agent workspace at {} (force={})",
                            ws_dir.display(),
                            force
                        ),
                        "Provision dual-audience AGENTS.md, README.md, .agents/ constraints, memory, logs (0700), checkpoints (0700), and credentials (0700)".to_string(),
                    ],
                    affected_paths: vec![
                        ws_dir.join("AGENTS.md").display().to_string(),
                        ws_dir.join("README.md").display().to_string(),
                        ws_dir.join(MEMORY_FILE_REL).display().to_string(),
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
                "[agent init] workspace={} changed={} created={} updated={} preserved={} unchanged={}\n",
                report.workspace_dir,
                report.changed,
                report.created_files.len(),
                report.updated_files.len(),
                report.preserved_files.len(),
                report.unchanged_files.len()
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
                "[agent status] workspace={} initialized={} healthy={} managed={}/{} memory_lines={} audit_entries={} checkpoints={} cred_ok={}\n",
                report.workspace_dir,
                report.initialized,
                report.healthy,
                report.managed_files_present,
                report.managed_files_expected,
                report.memory_lines,
                report.audit_entries,
                report.checkpoint_count,
                report.credential_audit.ok
            );
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
        } => {
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
                write_stdout_text("(MEMORY.md not initialized)\n")?;
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
