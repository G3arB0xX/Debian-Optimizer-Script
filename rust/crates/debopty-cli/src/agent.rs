use std::env;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Args;
use debopty_core::diag;
use debopty_core::version;
use debopty_tui::is_non_interactive;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::{Cli, CliError, env_truthy};

pub const AGENT_SCHEMA_VERSION: &str = "1";

#[derive(Debug, Clone, Default)]
pub struct AgentOutputOpts {
    pub fields: Option<Vec<String>>,
    pub max_lines: Option<usize>,
    pub max_bytes: Option<usize>,
    pub kv_output: bool,
}

static AGENT_OUTPUT_OPTS: OnceLock<RwLock<AgentOutputOpts>> = OnceLock::new();
static FOLD_SEQ: AtomicU64 = AtomicU64::new(1);

fn output_opts_cell() -> &'static RwLock<AgentOutputOpts> {
    AGENT_OUTPUT_OPTS.get_or_init(|| RwLock::new(AgentOutputOpts::default()))
}

fn current_output_opts() -> AgentOutputOpts {
    output_opts_cell()
        .read()
        .map(|g| g.clone())
        .unwrap_or_default()
}

pub fn init_output_opts(cli: &Cli) {
    let raw_fields = cli
        .fields
        .clone()
        .or_else(|| env::var("DEBOPTY_FIELDS").ok())
        .unwrap_or_default();
    let parsed_fields: Vec<String> = raw_fields
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let max_lines = cli.max_lines.or_else(|| {
        env::var("DEBOPTY_MAX_LINES")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    let max_bytes = cli.max_bytes.or_else(|| {
        env::var("DEBOPTY_MAX_BYTES")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    let output_mode = cli
        .output
        .clone()
        .or_else(|| env::var("DEBOPTY_OUTPUT").ok())
        .unwrap_or_default();
    let kv_output = output_mode.trim().eq_ignore_ascii_case("kv");

    if let Ok(mut guard) = output_opts_cell().write() {
        *guard = AgentOutputOpts {
            fields: if parsed_fields.is_empty() {
                None
            } else {
                Some(parsed_fields)
            },
            max_lines,
            max_bytes,
            kv_output,
        };
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AgentCtx {
    pub json: bool,
    pub envelope: bool,
    pub yes: bool,
    pub dry_run: bool,
    pub non_interactive: bool,
    pub explicit_non_interactive: bool,
}

impl AgentCtx {
    pub fn from_cli(cli: &Cli) -> Self {
        let env_agent = env_truthy("DEBOPTY_AGENT");
        let env_ni = env_truthy("DEBOPTY_NON_INTERACTIVE") || env_truthy("NON_INTERACTIVE");
        let kv_mode = cli
            .output
            .as_deref()
            .or_else(|| {
                env::var("DEBOPTY_OUTPUT")
                    .ok()
                    .filter(|v| v.trim().eq_ignore_ascii_case("kv"))
                    .map(|_| "kv")
            })
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("kv"));
        let envelope = cli.envelope || env_agent;
        let json = cli.json || envelope || kv_mode;
        let explicit_non_interactive = cli.non_interactive || env_ni || env_agent;
        let non_interactive = explicit_non_interactive || is_non_interactive();
        Self {
            json,
            envelope,
            yes: cli.yes,
            dry_run: cli.dry_run,
            non_interactive,
            explicit_non_interactive,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AgentEnvelope<T: Serialize> {
    pub schema_version: &'static str,
    pub ok: bool,
    pub command: String,
    pub dry_run: bool,
    pub exit_code: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AgentErrorBody>,
}

#[derive(Debug, Serialize)]
pub struct AgentErrorBody {
    pub code: &'static str,
    pub category: &'static str,
    pub message: String,
    pub remediation_hint: &'static str,
    pub retryable: bool,
}

#[derive(Debug, Serialize)]
pub struct DryRunPlan {
    pub command: String,
    pub target: String,
    pub mutates_system: bool,
    pub requires_root: bool,
    pub requires_yes: bool,
    pub planned_actions: Vec<String>,
    pub affected_paths: Vec<String>,
}

#[derive(Debug, Args)]
pub struct SchemaArgs {
    /// 按命令前缀过滤清单 (例如 security | system | app | reconcile)
    #[arg(long, value_name = "PREFIX")]
    pub command: Option<String>,
}

#[derive(Debug, Serialize)]
struct GlobalFlagSpec {
    flag: &'static str,
    env: Option<&'static str>,
    summary: &'static str,
}

#[derive(Debug, Serialize)]
struct EnvelopeContractSpec {
    schema_version: &'static str,
    fields: &'static [&'static str],
    error_fields: &'static [&'static str],
    activation: &'static str,
}

#[derive(Debug, Serialize)]
struct ExitCodeSpec {
    code: u8,
    category: &'static str,
    meaning: &'static str,
}

#[derive(Debug, Serialize)]
struct CommandSpec {
    name: &'static str,
    summary: &'static str,
    mutates: bool,
    supports_dry_run: bool,
    requires_root: bool,
    supports_yes: bool,
    key_args: &'static [&'static str],
    example: &'static str,
}

#[derive(Debug, Serialize)]
struct SchemaManifest {
    schema_version: &'static str,
    tool: &'static str,
    version: String,
    global_flags: Vec<GlobalFlagSpec>,
    envelope_contract: EnvelopeContractSpec,
    exit_codes: Vec<ExitCodeSpec>,
    commands: Vec<CommandSpec>,
}

fn write_stdout_line(text: &str) -> Result<(), CliError> {
    let mut stdout = io::stdout().lock();
    match stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.write_all(b"\n"))
        .and_then(|()| stdout.flush())
    {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(err) => Err(CliError::Io(err)),
    }
}

fn project_object(map: &Map<String, Value>, fields: &[String]) -> Value {
    let mut out = Map::new();
    for key in fields {
        if let Some(v) = map.get(key) {
            out.insert(key.clone(), v.clone());
        } else if let Some((head, tail)) = key.split_once('.')
            && let Some(Value::Object(sub)) = map.get(head)
            && let Some(v) = sub.get(tail)
        {
            out.insert(key.clone(), v.clone());
        }
    }
    Value::Object(out)
}

fn project_json_value(val: &mut Value, fields: &[String]) {
    if fields.is_empty() {
        return;
    }
    match val {
        Value::Object(map) => {
            let direct_hit = fields.iter().any(|f| {
                map.contains_key(f) || f.split_once('.').is_some_and(|(h, _)| map.contains_key(h))
            });
            if direct_hit {
                *val = project_object(map, fields);
            } else {
                let mut cloned = map.clone();
                for list_key in [
                    "commands", "items", "entries", "hits", "findings", "domains", "accounts",
                    "results",
                ] {
                    if let Some(Value::Array(arr)) = cloned.get_mut(list_key) {
                        for item in arr.iter_mut() {
                            if let Value::Object(item_map) = item {
                                *item = project_object(item_map, fields);
                            }
                        }
                    }
                }
                *val = Value::Object(cloned);
            }
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                if let Value::Object(item_map) = item {
                    *item = project_object(item_map, fields);
                }
            }
        }
        _ => {}
    }
}

fn format_kv_scalar(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.replace(['\r', '\n'], " "),
        Value::Array(a) => format!("[len={}]", a.len()),
        Value::Object(o) => format!("{{keys={}}}", o.len()),
    }
}

fn flatten_kv(prefix: &str, val: &Value, out: &mut Vec<String>) {
    match val {
        Value::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                match v {
                    Value::Object(_) => flatten_kv(&key, v, out),
                    Value::Array(arr) if arr.len() <= 5 && arr.iter().all(|x| !x.is_object()) => {
                        let joined: Vec<String> = arr.iter().map(format_kv_scalar).collect();
                        out.push(format!("{key}={}", joined.join(",")));
                    }
                    _ => out.push(format!("{key}={}", format_kv_scalar(v))),
                }
            }
        }
        _ => out.push(format!(
            "{}={}",
            if prefix.is_empty() { "value" } else { prefix },
            format_kv_scalar(val)
        )),
    }
}

fn write_folded_payload(body: &str, ext: &str) -> Result<PathBuf, CliError> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = FOLD_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = env::temp_dir().join(format!(
        "debopty-out-{}-{ts}-{seq}.{ext}",
        std::process::id()
    ));
    let mut f = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?;
    f.write_all(body.as_bytes())?;
    f.flush()?;
    Ok(path)
}

fn maybe_fold_output(
    command: &str,
    dry_run: bool,
    ok: bool,
    exit_code: u8,
    rendered: String,
    is_json: bool,
    opts: &AgentOutputOpts,
) -> Result<String, CliError> {
    let total_bytes = rendered.len();
    let total_lines = rendered.lines().count().max(1);
    let over_lines = opts.max_lines.is_some_and(|lim| total_lines > lim);
    let over_bytes = opts.max_bytes.is_some_and(|lim| total_bytes > lim);
    if !over_lines && !over_bytes {
        return Ok(rendered);
    }

    let ext = if is_json { "json" } else { "txt" };
    let full_path = write_folded_payload(&rendered, ext)?;
    let preview: String = rendered.chars().take(180).collect();

    if is_json {
        let folded_env = json!({
            "schema_version": AGENT_SCHEMA_VERSION,
            "ok": ok,
            "command": command,
            "dry_run": dry_run,
            "exit_code": exit_code,
            "data": {
                "folded": true,
                "full_output_path": full_path.display().to_string(),
                "total_bytes": total_bytes,
                "total_lines": total_lines,
                "preview": preview
            }
        });
        Ok(serde_json::to_string(&folded_env)?)
    } else {
        let max_keep = opts.max_lines.unwrap_or(10).min(10);
        let mut head: Vec<&str> = rendered.lines().take(max_keep).collect();
        let footer = format!(
            "folded=true full_output_path={} total_bytes={} total_lines={}",
            full_path.display(),
            total_bytes,
            total_lines
        );
        head.push(&footer);
        Ok(head.join("\n"))
    }
}

pub fn emit_envelope_status<T: Serialize>(
    command: &str,
    dry_run: bool,
    ok: bool,
    exit_code: u8,
    data: &T,
) -> Result<i32, CliError> {
    let opts = current_output_opts();
    let mut data_val = serde_json::to_value(data)?;
    diag::redact_json_value(&mut data_val);
    if let Some(ref fields) = opts.fields {
        project_json_value(&mut data_val, fields);
    }

    if opts.kv_output {
        let mut lines = vec![
            format!("ok={ok}"),
            format!("command={command}"),
            format!("exit_code={exit_code}"),
        ];
        flatten_kv("", &data_val, &mut lines);
        let rendered = maybe_fold_output(
            command,
            dry_run,
            ok,
            exit_code,
            lines.join("\n"),
            false,
            &opts,
        )?;
        write_stdout_line(&rendered)?;
        return Ok(i32::from(exit_code));
    }

    let env_payload = AgentEnvelope {
        schema_version: AGENT_SCHEMA_VERSION,
        ok,
        command: command.to_string(),
        dry_run,
        exit_code,
        data: Some(data_val),
        error: None,
    };
    let rendered = serde_json::to_string(&env_payload)?;
    let bounded = maybe_fold_output(command, dry_run, ok, exit_code, rendered, true, &opts)?;
    write_stdout_line(&bounded)?;
    Ok(i32::from(exit_code))
}

pub fn emit_envelope<T: Serialize>(
    command: &str,
    dry_run: bool,
    data: &T,
) -> Result<i32, CliError> {
    emit_envelope_status(command, dry_run, true, 0, data)
}

pub fn emit_legacy_or_envelope<T: Serialize>(
    ctx: &AgentCtx,
    command: &str,
    data: &T,
) -> Result<i32, CliError> {
    if ctx.envelope {
        emit_envelope(command, ctx.dry_run, data)
    } else {
        let opts = current_output_opts();
        let mut val = serde_json::to_value(data)?;
        diag::redact_json_value(&mut val);
        if let Some(ref fields) = opts.fields {
            project_json_value(&mut val, fields);
        }
        if opts.kv_output {
            let mut lines = Vec::new();
            flatten_kv("", &val, &mut lines);
            let rendered = maybe_fold_output(
                command,
                ctx.dry_run,
                true,
                0,
                lines.join("\n"),
                false,
                &opts,
            )?;
            write_stdout_line(&rendered)?;
            return Ok(0);
        }
        let rendered = serde_json::to_string(&val)?;
        let bounded = maybe_fold_output(command, ctx.dry_run, true, 0, rendered, true, &opts)?;
        write_stdout_line(&bounded)?;
        Ok(0)
    }
}

pub fn emit_dry_run_plan(ctx: &AgentCtx, plan: &DryRunPlan) -> Result<i32, CliError> {
    if ctx.json {
        emit_envelope(&plan.command, true, plan)
    } else {
        let mut out = format!("[dry-run] {} -> {}\n", plan.command, plan.target);
        for action in &plan.planned_actions {
            out.push_str(&format!("  plan: {action}\n"));
        }
        for path in &plan.affected_paths {
            out.push_str(&format!("  path: {path}\n"));
        }
        let mut stdout = io::stdout().lock();
        let _ = stdout
            .write_all(out.as_bytes())
            .and_then(|()| stdout.flush());
        Ok(0)
    }
}

pub fn emit_error_envelope(command: &str, dry_run: bool, err: &CliError) {
    let env_payload: AgentEnvelope<serde_json::Value> = AgentEnvelope {
        schema_version: AGENT_SCHEMA_VERSION,
        ok: false,
        command: command.to_string(),
        dry_run,
        exit_code: err.exit_code(),
        data: None,
        error: Some(AgentErrorBody {
            code: err.error_code(),
            category: err.category(),
            message: diag::redact_sensitive(&err.to_string()),
            remediation_hint: err.remediation_hint(),
            retryable: err.retryable(),
        }),
    };
    if let Ok(rendered) = serde_json::to_string(&env_payload) {
        let _ = write_stdout_line(&rendered);
    }
}

pub fn detect_json_mode_from_args() -> bool {
    if env_truthy("DEBOPTY_AGENT") {
        return true;
    }
    for arg in env::args_os().skip(1) {
        let s = arg.to_string_lossy();
        if s == "--" {
            break;
        }
        if s == "--json" || s == "--envelope" {
            return true;
        }
    }
    false
}

pub fn detect_dry_run_from_args() -> bool {
    for arg in env::args_os().skip(1) {
        let s = arg.to_string_lossy();
        if s == "--" {
            break;
        }
        if s == "--dry-run" {
            return true;
        }
    }
    false
}

pub fn detect_command_from_args() -> String {
    let mut skip_next = false;
    let value_flags = [
        "--engine",
        "--ui",
        "--config",
        "--script-dir",
        "--template-dir",
        "--log-level",
        "--log-format",
        "--fields",
        "--max-lines",
        "--max-bytes",
        "--output",
    ];
    for arg in env::args_os().skip(1) {
        let s = arg.to_string_lossy();
        if s == "--" {
            break;
        }
        if skip_next {
            skip_next = false;
            continue;
        }
        if value_flags.contains(&s.as_ref()) {
            skip_next = true;
            continue;
        }
        if !s.starts_with('-') {
            return s.into_owned();
        }
    }
    "debopty".to_string()
}

fn build_global_flags() -> Vec<GlobalFlagSpec> {
    vec![
        GlobalFlagSpec {
            flag: "--json",
            env: Some("DEBOPTY_AGENT"),
            summary: "Emit machine-readable JSON output to stdout (errors also wrapped in AgentEnvelope).",
        },
        GlobalFlagSpec {
            flag: "--envelope",
            env: Some("DEBOPTY_AGENT"),
            summary: "Force AgentEnvelope v1 wrapper on all JSON outputs (implies --json).",
        },
        GlobalFlagSpec {
            flag: "-y, --yes",
            env: None,
            summary: "Auto-confirm destructive or mutating operations in non-interactive mode.",
        },
        GlobalFlagSpec {
            flag: "--dry-run",
            env: None,
            summary: "Preview planned mutations and affected paths without modifying the system.",
        },
        GlobalFlagSpec {
            flag: "--non-interactive",
            env: Some("DEBOPTY_NON_INTERACTIVE"),
            summary: "Disable interactive TTY prompts and TUI; fail closed when confirmation is missing.",
        },
        GlobalFlagSpec {
            flag: "--config <PATH>",
            env: Some("CONFIG_FILE"),
            summary: "Override persistent configuration path (default /etc/debopti/debopti.conf).",
        },
        GlobalFlagSpec {
            flag: "--template-dir <PATH>",
            env: None,
            summary: "Override embedded template directory.",
        },
        GlobalFlagSpec {
            flag: "--log-level <LEVEL>",
            env: Some("DEBOPTY_LOG"),
            summary: "Diagnostic log level on stderr: error|warn|info|debug|trace.",
        },
        GlobalFlagSpec {
            flag: "--log-format <FORMAT>",
            env: Some("DEBOPTY_LOG_FORMAT"),
            summary: "Diagnostic log format on stderr: text|json.",
        },
        GlobalFlagSpec {
            flag: "--fields <F1,F2,...>",
            env: Some("DEBOPTY_FIELDS"),
            summary: "Project comma-separated fields on --json or --output kv payloads.",
        },
        GlobalFlagSpec {
            flag: "--max-lines <N>",
            env: Some("DEBOPTY_MAX_LINES"),
            summary: "Fold stdout beyond N lines into a temp file and emit summary + path.",
        },
        GlobalFlagSpec {
            flag: "--max-bytes <N>",
            env: Some("DEBOPTY_MAX_BYTES"),
            summary: "Fold stdout beyond N bytes into a temp file and emit summary + path.",
        },
        GlobalFlagSpec {
            flag: "--output <MODE>",
            env: Some("DEBOPTY_OUTPUT"),
            summary: "Structured output mode: auto|json|kv (compact single-line JSON or k=v lines).",
        },
    ]
}

fn build_exit_codes() -> Vec<ExitCodeSpec> {
    vec![
        ExitCodeSpec {
            code: 0,
            category: "ok",
            meaning: "Command completed successfully.",
        },
        ExitCodeSpec {
            code: 1,
            category: "runtime",
            meaning: "General runtime, system, security, reconcile, or app failure.",
        },
        ExitCodeSpec {
            code: 2,
            category: "usage",
            meaning: "Invalid CLI usage, arguments, or input validation error.",
        },
        ExitCodeSpec {
            code: 3,
            category: "io",
            meaning: "Filesystem or I/O read/write permission error.",
        },
        ExitCodeSpec {
            code: 4,
            category: "not_found",
            meaning: "Requested configuration key, template, or remote release not found.",
        },
        ExitCodeSpec {
            code: 5,
            category: "network",
            meaning: "Network request, HTTP status, external command failure, or timeout.",
        },
        ExitCodeSpec {
            code: 6,
            category: "confirmation_required",
            meaning: "Non-interactive mutating command requires explicit --yes (-y) confirmation.",
        },
        ExitCodeSpec {
            code: 21,
            category: "write_gate_reject",
            meaning: "Memory topic write rejected by Write Gate (frontmatter schema, provenance, or duplicate conflict).",
        },
        ExitCodeSpec {
            code: 22,
            category: "memory_bloat",
            meaning: "Memory TOC, topic shard, or raw code fence exceeds anti-bloat limits.",
        },
        ExitCodeSpec {
            code: 23,
            category: "memory_secret_leak",
            meaning: "Plaintext secret or credential detected in memory write payload.",
        },
        ExitCodeSpec {
            code: 24,
            category: "audit_nonstandard",
            meaning: "Workspace audit detected legacy files, managed shim drift, or vendor lock-in.",
        },
        ExitCodeSpec {
            code: 25,
            category: "migration_conflict",
            meaning: "Dirty managed projection or migration conflict requires --harvest or --force.",
        },
        ExitCodeSpec {
            code: 26,
            category: "secret_detected",
            meaning: "Plaintext secret or sensitive file detected outside .agents/credentials/.",
        },
    ]
}

fn build_command_catalog() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            name: "schema",
            summary: "Emit machine-readable CLI capability schema, global flags, and exit codes.",
            mutates: false,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &["--command <PREFIX>"],
            example: "debopty schema --json",
        },
        CommandSpec {
            name: "agent init",
            summary: "Provision or non-destructively upgrade the VPS agent workspace (SSOT v2, harness-agnostic).",
            mutates: true,
            supports_dry_run: true,
            requires_root: false,
            supports_yes: false,
            key_args: &[
                "--dir <PATH>",
                "--root <PATH>",
                "--force",
                "--harnesses <SPEC>",
                "--memory-tracking <local|tracked>",
                "--migrate-legacy",
                "--cleanup-legacy",
                "--adapter-file <PATH>",
            ],
            example: "debopty agent init --harnesses none --memory-tracking local --json",
        },
        CommandSpec {
            name: "agent status",
            summary: "Inspect VPS agent workspace health, managed docs, memory, audit entries, checkpoints, and credentials.",
            mutates: false,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &["--dir <PATH>", "--root <PATH>"],
            example: "debopty agent status --json",
        },
        CommandSpec {
            name: "agent audit",
            summary: "Read-only audit of workspace SSOT compliance, legacy drift, vendor lock-in, leftover IDE caches, and secret leaks (exit 0/24/26).",
            mutates: false,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &[
                "--dir <PATH>",
                "--root <PATH>",
                "--check-ide-caches",
                "--no-ide-cache-check",
                "--home-dir <PATH>",
            ],
            example: "debopty agent audit --json",
        },
        CommandSpec {
            name: "agent migrate",
            summary: "Non-destructively migrate legacy memories, constraints, skills, and MCP configs into SSOT v2 with checkpointing.",
            mutates: true,
            supports_dry_run: true,
            requires_root: false,
            supports_yes: false,
            key_args: &[
                "--dir <PATH>",
                "--root <PATH>",
                "--harnesses <SPEC>",
                "--memory-tracking <local|tracked>",
                "--cleanup-legacy",
                "--adapter-file <PATH>",
            ],
            example: "debopty agent migrate --json",
        },
        CommandSpec {
            name: "agent switch",
            summary: "Switch, add, or remove active harness projection shims with dirty-shim harvest protection.",
            mutates: true,
            supports_dry_run: true,
            requires_root: false,
            supports_yes: false,
            key_args: &[
                "--dir <PATH>",
                "--root <PATH>",
                "--to <SPEC>",
                "--add <SPEC>",
                "--remove <SPEC>",
                "--harvest",
                "--force",
                "--adapter-file <PATH>",
            ],
            example: "debopty agent switch --add compat --json",
        },
        CommandSpec {
            name: "agent override",
            summary: "Manage per-harness custom override files under .agents/overrides/.",
            mutates: true,
            supports_dry_run: true,
            requires_root: false,
            supports_yes: false,
            key_args: &[
                "--dir <PATH>",
                "--root <PATH>",
                "--init <HARNESS>",
                "--list",
                "--remove <HARNESS>",
                "--json-format",
            ],
            example: "debopty agent override --list --json",
        },
        CommandSpec {
            name: "agent harness-list",
            summary: "List all built-in and custom harness adapters and presets.",
            mutates: false,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &["--dir <PATH>", "--root <PATH>", "--adapter-file <PATH>"],
            example: "debopty agent harness-list --json",
        },
        CommandSpec {
            name: "agent checkpoint",
            summary: "Create or list atomic pre-change configuration checkpoints reusing reconcile::backup.",
            mutates: true,
            supports_dry_run: true,
            requires_root: false,
            supports_yes: false,
            key_args: &[
                "--dir <PATH>",
                "--root <PATH>",
                "--label <LABEL>",
                "--reason <TEXT>",
                "--path <PATH>",
                "--list",
            ],
            example: "debopty agent checkpoint --label ssh --path /etc/ssh/sshd_config --json",
        },
        CommandSpec {
            name: "agent rollback",
            summary: "Atomically restore files from a checkpoint (ID or 'latest') and record a redacted audit entry.",
            mutates: true,
            supports_dry_run: true,
            requires_root: false,
            supports_yes: true,
            key_args: &["<CHECKPOINT_ID|latest>", "--dir <PATH>", "--root <PATH>"],
            example: "debopty agent rollback latest --yes --json",
        },
        CommandSpec {
            name: "agent audit-log",
            summary: "Read or append redacted NDJSON operational audit log entries.",
            mutates: true,
            supports_dry_run: true,
            requires_root: false,
            supports_yes: false,
            key_args: &[
                "--dir <PATH>",
                "--limit <N>",
                "--append",
                "--action <STR>",
                "--summary <STR>",
                "--status <STR>",
            ],
            example: "debopty agent audit-log --limit 20 --json",
        },
        CommandSpec {
            name: "agent memory",
            summary: "Read or append redacted long-term host maintenance notes in .agents/MEMORY.md.",
            mutates: true,
            supports_dry_run: true,
            requires_root: false,
            supports_yes: false,
            key_args: &["--dir <PATH>", "--append <TEXT>", "--section <STR>"],
            example: "debopty agent memory --append 'Verified nftables rules' --json",
        },
        CommandSpec {
            name: "agent memory write",
            summary: "Write a CoALA-structured memory topic shard enforced by Write Gate (exit 0/21/22/23).",
            mutates: true,
            supports_dry_run: true,
            requires_root: false,
            supports_yes: false,
            key_args: &[
                "--dir <PATH>",
                "--id <ID>",
                "--title <TITLE>",
                "--type <episodic|semantic|procedural|profile>",
                "--scope <SCOPE>",
                "--tag <TAG>",
                "--source <SRC>",
                "--confidence <FLOAT>",
                "--ttl-days <DAYS>",
                "--supersedes <ID>",
                "--body <TEXT>",
                "--body-stdin",
            ],
            example: "debopty agent memory write --id net-bbr --title 'BBR Tuning' --type semantic --scope host --source /etc/sysctl.d/99-debopti.conf --body 'Verified BBR enabled.' --json",
        },
        CommandSpec {
            name: "agent memory index",
            summary: "Rebuild .agents/cache/memory_index.json and .agents/MEMORY.md TOC from topic shards.",
            mutates: true,
            supports_dry_run: true,
            requires_root: false,
            supports_yes: false,
            key_args: &["--dir <PATH>", "--root <PATH>"],
            example: "debopty agent memory index --json",
        },
        CommandSpec {
            name: "agent memory search",
            summary: "Search CoALA memory shards via ranked BM25-like index returning bounded snippets.",
            mutates: false,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &["<QUERY>", "--dir <PATH>", "--root <PATH>", "--limit <N>"],
            example: "debopty agent memory search nftables --limit 5 --json",
        },
        CommandSpec {
            name: "agent memory consolidate",
            summary: "Plan or apply memory consolidation (archive expired/superseded shards and identify promotion candidates).",
            mutates: true,
            supports_dry_run: true,
            requires_root: false,
            supports_yes: false,
            key_args: &["--dir <PATH>", "--root <PATH>", "--plan", "--apply"],
            example: "debopty agent memory consolidate --plan --json",
        },
        CommandSpec {
            name: "agent memory promote",
            summary: "Promote a mature memory topic shard into a canonical constraint/doc and archive the shard.",
            mutates: true,
            supports_dry_run: true,
            requires_root: false,
            supports_yes: false,
            key_args: &[
                "--dir <PATH>",
                "--root <PATH>",
                "--id <ID>",
                "--target <PATH>",
            ],
            example: "debopty agent memory promote --id net-bbr --target .agents/constraints/10_network.md --json",
        },
        CommandSpec {
            name: "agent memory lint",
            summary: "Validate memory TOC/shard anti-bloat budgets, frontmatter integrity, and index freshness.",
            mutates: false,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &["--dir <PATH>", "--root <PATH>"],
            example: "debopty agent memory lint --json",
        },
        CommandSpec {
            name: "agent verify",
            summary: "Verify agent workspace permissions (0700/0600), credential hygiene, and dual-audience document contract.",
            mutates: false,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &["--dir <PATH>", "--root <PATH>"],
            example: "debopty agent verify --json",
        },
        CommandSpec {
            name: "agent cred set",
            summary: "Store a managed credential with 0700/0600 perms via --value-stdin or --from-file (never CLI plaintext).",
            mutates: true,
            supports_dry_run: true,
            requires_root: false,
            supports_yes: false,
            key_args: &[
                "--dir <PATH>",
                "--name <NAME>",
                "--kind raw|env|systemd-encrypted",
                "--value-stdin",
                "--from-file <PATH>",
            ],
            example: "debopty agent cred set --name cf_dns --kind raw --value-stdin --json",
        },
        CommandSpec {
            name: "agent cred list",
            summary: "List managed credential metadata (permissions, size, encryption status; never echoes secrets).",
            mutates: false,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &["--dir <PATH>"],
            example: "debopty agent cred list --json",
        },
        CommandSpec {
            name: "agent cred remove",
            summary: "Remove a managed credential from the 0700 credential store.",
            mutates: true,
            supports_dry_run: true,
            requires_root: false,
            supports_yes: true,
            key_args: &["--dir <PATH>", "--name <NAME>"],
            example: "debopty agent cred remove --name cf_dns --yes --json",
        },
        CommandSpec {
            name: "agent cred audit",
            summary: "Audit credential directory permissions, systemd-creds availability, and plaintext leak risks.",
            mutates: false,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &["--dir <PATH>", "--root <PATH>"],
            example: "debopty agent cred audit --json",
        },
        CommandSpec {
            name: "agent cred systemd-snippet",
            summary: "Render or write hardened systemd LoadCredential/EnvironmentFile drop-in config.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &[
                "--dir <PATH>",
                "--root <PATH>",
                "--unit <UNIT>",
                "--name <NAME>",
                "--kind raw|env|systemd-encrypted",
                "--write-dropin",
            ],
            example: "debopty agent cred systemd-snippet --unit myapp --name cf_dns --kind raw --json",
        },
        CommandSpec {
            name: "version",
            summary: "Show or parse debopty SemVer/CalVer/Build identity.",
            mutates: false,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &["--file <PATH>"],
            example: "debopty version --json",
        },
        CommandSpec {
            name: "status",
            summary: "Probe host hardware/kernel telemetry, optimization flags, tools, and module registry.",
            mutates: false,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &["--config <PATH>"],
            example: "debopty status --envelope",
        },
        CommandSpec {
            name: "config get",
            summary: "Read a single key from /etc/debopti/debopti.conf.",
            mutates: false,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &["<KEY>"],
            example: "debopty config get IS_CN_REGION --json",
        },
        CommandSpec {
            name: "config set",
            summary: "Atomically set a key in debopti.conf while preserving comments and order.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["<KEY>", "<VALUE>"],
            example: "debopty config set IS_CN_REGION true --dry-run --json",
        },
        CommandSpec {
            name: "config list",
            summary: "List all key-value entries in debopti.conf.",
            mutates: false,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &[],
            example: "debopty config list --json",
        },
        CommandSpec {
            name: "template list",
            summary: "List all embedded single-source-of-truth template paths.",
            mutates: false,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &[],
            example: "debopty template list --json",
        },
        CommandSpec {
            name: "template render",
            summary: "Render a template to stdout or an output file with --var KEY=VAL overrides.",
            mutates: true,
            supports_dry_run: true,
            requires_root: false,
            supports_yes: false,
            key_args: &["<REL_PATH>", "--out <PATH>", "--var KEY=VAL"],
            example: "debopty template render templates/common/debopty.sh --var BIN_LINK=/usr/local/bin/debopty --json",
        },
        CommandSpec {
            name: "f2b-sync",
            summary: "Sync nftables public ports into Fail2ban jail and nft guard rules.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["--print-ports", "--force-reload"],
            example: "debopty f2b-sync --print-ports --json",
        },
        CommandSpec {
            name: "network dns-check",
            summary: "Run two-stage DNS health check and calibration.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["--mode initial|calibrate", "--resolv <PATH>", "--cn"],
            example: "debopty network dns-check --mode calibrate --dry-run --json",
        },
        CommandSpec {
            name: "network netcheck",
            summary: "Detect CN vs global network region and cache result in debopti.conf.",
            mutates: true,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &["--force"],
            example: "debopty network netcheck --json",
        },
        CommandSpec {
            name: "network latest-release",
            summary: "Query latest GitHub release tag with CN mirror pool fallback.",
            mutates: false,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &["<OWNER/REPO>", "--cn"],
            example: "debopty network latest-release G3arB0xX/Debian-Optimizer-Script --json",
        },
        CommandSpec {
            name: "network download",
            summary: "Download a URL with mirror fallback to local path.",
            mutates: true,
            supports_dry_run: true,
            requires_root: false,
            supports_yes: false,
            key_args: &["<URL>", "--out <PATH>", "--cn"],
            example: "debopty network download https://example.com/file --out /tmp/file --dry-run --json",
        },
        CommandSpec {
            name: "network git-clone",
            summary: "Clone a Git repository with mirror fallback and protected-directory guard.",
            mutates: true,
            supports_dry_run: true,
            requires_root: false,
            supports_yes: false,
            key_args: &["<URL>", "<DEST>", "--cn"],
            example: "debopty network git-clone https://github.com/org/repo.git /tmp/repo --dry-run --json",
        },
        CommandSpec {
            name: "system tmux-setup",
            summary: "Deploy version-adaptive global Tmux config with backup.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["--conf <PATH>", "--skip-apt"],
            example: "debopty system tmux-setup --dry-run --json",
        },
        CommandSpec {
            name: "system tmux-uninstall",
            summary: "Restore previous Tmux config or uninstall managed Tmux config.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["--conf <PATH>", "--skip-apt"],
            example: "debopty system tmux-uninstall --dry-run --json",
        },
        CommandSpec {
            name: "system ip-forward-toggle",
            summary: "Toggle IPv4/IPv6 kernel forwarding configuration.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["--root <PATH>", "--no-sys"],
            example: "debopty system ip-forward-toggle --dry-run --json",
        },
        CommandSpec {
            name: "system sysctl",
            summary: "Render and apply BBR/TCP sysctl kernel tuning.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["--root <PATH>", "--no-sys"],
            example: "debopty system sysctl --dry-run --json",
        },
        CommandSpec {
            name: "system limits",
            summary: "Configure nofile limits in limits.d and systemd system/user configs.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["--root <PATH>"],
            example: "debopty system limits --dry-run --json",
        },
        CommandSpec {
            name: "system logrotate",
            summary: "Configure /etc/logrotate.conf rotation policy.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["--root <PATH>"],
            example: "debopty system logrotate --dry-run --json",
        },
        CommandSpec {
            name: "system timezone",
            summary: "Set timezone to Asia/Shanghai and enable chrony.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["--no-sys"],
            example: "debopty system timezone --dry-run --json",
        },
        CommandSpec {
            name: "system low-mem",
            summary: "Apply low-memory VPS tuning (TTY/journald/logind/service masking).",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["--root <PATH>", "--no-sys", "--mask-services"],
            example: "debopty system low-mem --dry-run --json",
        },
        CommandSpec {
            name: "system hardware",
            summary: "Probe host CPU/RAM/disk hardware specs and resolved adaptive tuning profile (read-only).",
            mutates: false,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &["--root <PATH>"],
            example: "debopty system hardware --json",
        },
        CommandSpec {
            name: "system retention",
            summary: "Deploy declarative journald log limits, user-tmpfiles.d cold-cache cleanup, and APT clean timer.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &[
                "--root <PATH>",
                "--no-sys",
                "--user-home <PATH>",
                "--skip-user-tmpfiles",
                "--skip-apt-clean",
            ],
            example: "debopty system retention --dry-run --json",
        },
        CommandSpec {
            name: "system base-optimize",
            summary: "Run full base system optimization pipeline.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["--root <PATH>", "--no-sys"],
            example: "debopty system base-optimize --dry-run --json",
        },
        CommandSpec {
            name: "security fw-add",
            summary: "Add modular nftables firewall rule and sync Fail2ban.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &[
                "<PORT>",
                "--proto tcp|udp|tcp/udp",
                "--comment <NAME>",
                "--root <PATH>",
            ],
            example: "debopty security fw-add 443 --proto tcp --comment HTTPS --dry-run --json",
        },
        CommandSpec {
            name: "security fw-remove",
            summary: "Remove modular nftables firewall rule by comment name and sync Fail2ban.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["<COMMENT>", "--root <PATH>"],
            example: "debopty security fw-remove HTTPS --dry-run --json",
        },
        CommandSpec {
            name: "security fw-update-ssh",
            summary: "Update SSH port in nftables.conf and sync Fail2ban.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["<PORT>", "--root <PATH>"],
            example: "debopty security fw-update-ssh 50022 --dry-run --json",
        },
        CommandSpec {
            name: "security ndp-patch",
            summary: "Idempotently patch IPv6 NDP rules in nftables.conf.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["--root <PATH>"],
            example: "debopty security ndp-patch --dry-run --json",
        },
        CommandSpec {
            name: "security f2b-setup",
            summary: "Deploy Fail2ban systemd backend config and port-sync timer.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["--ssh-port <PORT>", "--root <PATH>"],
            example: "debopty security f2b-setup --dry-run --json",
        },
        CommandSpec {
            name: "security ssh-harden",
            summary: "Apply zero-trust SSH hardening (Ed25519 key, high port, root lockdown).",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &[
                "--non-interactive",
                "--pubkey <ED25519_KEY>",
                "--username <USER>",
            ],
            example: "debopty security ssh-harden --dry-run --json",
        },
        CommandSpec {
            name: "security setup",
            summary: "Initialize full security engine (UFW cleanup + nftables + Fail2ban).",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["--root <PATH>", "--no-sys"],
            example: "debopty security setup --dry-run --json",
        },
        CommandSpec {
            name: "reconcile",
            summary: "9-domain drift audit, atomic convergence (--apply), and backup rollback (--restore).",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: true,
            key_args: &[
                "--audit",
                "--apply",
                "--restore <DIR>",
                "--domain <DOMAIN>",
                "--root <PATH>",
            ],
            example: "debopty reconcile --apply --dry-run --json",
        },
        CommandSpec {
            name: "app install",
            summary: "Install or upgrade a native Rust managed application module.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["<NAME>", "--root <PATH>", "--no-sys", "--cn"],
            example: "debopty app install podman --dry-run --json",
        },
        CommandSpec {
            name: "app uninstall",
            summary: "Uninstall a managed application module.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["<NAME>", "--root <PATH>", "--no-sys"],
            example: "debopty app uninstall podman --dry-run --json",
        },
        CommandSpec {
            name: "app xray-status",
            summary: "Query Xray Core installation, ruleset, and cron status.",
            mutates: false,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &["--root <PATH>", "--no-sys"],
            example: "debopty app xray-status --json",
        },
        CommandSpec {
            name: "app freship-status",
            summary: "Query FreshIP installation, region, and timer status.",
            mutates: false,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &["--root <PATH>", "--no-sys"],
            example: "debopty app freship-status --json",
        },
        CommandSpec {
            name: "app lego-list",
            summary: "List managed Lego SSL domains and certificate expiry.",
            mutates: false,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &["--root <PATH>"],
            example: "debopty app lego-list --json",
        },
        CommandSpec {
            name: "self-install",
            summary: "Install debopty binary into /opt/debopty, wire global symlinks, retention policies, and provision VPS agent workspace.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &[
                "--root <PATH>",
                "--install-dir <PATH>",
                "--no-sys",
                "--skip-agent-workspace",
                "--agent-workspace-dir <PATH>",
                "--skip-retention",
            ],
            example: "debopty self-install --dry-run --json",
        },
        CommandSpec {
            name: "self-update",
            summary: "Download, SHA-256 verify, and atomically upgrade debopty from GitHub Releases.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["--tag <TAG>", "--force", "--cn"],
            example: "debopty self-update --dry-run --json",
        },
        CommandSpec {
            name: "self-uninstall",
            summary: "Remove debopty binary and symlinks (preserves /etc/debopti unless --purge-config).",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: true,
            key_args: &["--purge-config", "--root <PATH>", "--no-sys"],
            example: "debopty self-uninstall --purge-config --dry-run --json",
        },
        CommandSpec {
            name: "ui-preview",
            summary: "Render deterministic non-interactive TUI frames for inspection.",
            mutates: false,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &[
                "--screen <SCREEN>",
                "--demo",
                "--ansi",
                "--width <W>",
                "--height <H>",
            ],
            example: "debopty ui-preview --demo --screen inspector",
        },
        CommandSpec {
            name: "bridge",
            summary: "Inspect module registry or invoke archived legacy Bash actions explicitly.",
            mutates: true,
            supports_dry_run: false,
            requires_root: false,
            supports_yes: false,
            key_args: &["--list", "<ACTION>"],
            example: "debopty bridge --list --json",
        },
    ]
}

pub fn handle_schema(args: &SchemaArgs, ctx: &AgentCtx) -> Result<i32, CliError> {
    let mut commands = build_command_catalog();
    if let Some(prefix) = args
        .command
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let needle = prefix.to_ascii_lowercase();
        commands.retain(|c| c.name.to_ascii_lowercase().starts_with(&needle));
    }

    let manifest = SchemaManifest {
        schema_version: AGENT_SCHEMA_VERSION,
        tool: "debopty",
        version: version::format_display(&version::compiled_identity()),
        global_flags: build_global_flags(),
        envelope_contract: EnvelopeContractSpec {
            schema_version: AGENT_SCHEMA_VERSION,
            fields: &[
                "schema_version",
                "ok",
                "command",
                "dry_run",
                "exit_code",
                "data",
                "error",
            ],
            error_fields: &[
                "code",
                "category",
                "message",
                "remediation_hint",
                "retryable",
            ],
            activation: "Pass --envelope or set DEBOPTY_AGENT=1 for envelope on all JSON commands; schema, --dry-run, errors with --json, system/security/config/template/network --json always emit AgentEnvelope.",
        },
        exit_codes: build_exit_codes(),
        commands,
    };

    if ctx.json {
        emit_envelope("schema", false, &manifest)
    } else {
        let mut out = format!(
            "debopty CLI Schema v{} ({})\nCommands ({}):\n",
            manifest.schema_version,
            manifest.version,
            manifest.commands.len()
        );
        for cmd in &manifest.commands {
            out.push_str(&format!(
                "  {:<26} [mutates={} dry_run={}] {}\n",
                cmd.name, cmd.mutates, cmd.supports_dry_run, cmd.summary
            ));
        }
        let mut stdout = io::stdout().lock();
        let _ = stdout
            .write_all(out.as_bytes())
            .and_then(|()| stdout.flush());
        Ok(0)
    }
}
