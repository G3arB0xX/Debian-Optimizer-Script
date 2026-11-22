use std::env;
use std::io::{self, Write};

use clap::Args;
use debopty_core::diag;
use debopty_core::version;
use debopty_tui::is_non_interactive;
use serde::Serialize;

use crate::{Cli, CliError, env_truthy};

pub const AGENT_SCHEMA_VERSION: &str = "1";

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
        let envelope = cli.envelope || env_agent;
        let json = cli.json || envelope;
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

pub fn emit_envelope<T: Serialize>(
    command: &str,
    dry_run: bool,
    data: &T,
) -> Result<i32, CliError> {
    let env_payload = AgentEnvelope {
        schema_version: AGENT_SCHEMA_VERSION,
        ok: true,
        command: command.to_string(),
        dry_run,
        exit_code: 0,
        data: Some(data),
        error: None,
    };
    let rendered = serde_json::to_string_pretty(&env_payload)?;
    write_stdout_line(&rendered)?;
    Ok(0)
}

pub fn emit_legacy_or_envelope<T: Serialize>(
    ctx: &AgentCtx,
    command: &str,
    data: &T,
) -> Result<i32, CliError> {
    if ctx.envelope {
        emit_envelope(command, ctx.dry_run, data)
    } else {
        let rendered = serde_json::to_string_pretty(data)?;
        write_stdout_line(&rendered)?;
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
    if let Ok(rendered) = serde_json::to_string_pretty(&env_payload) {
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
            summary: "Install debopty binary into /opt/debopty and wire global symlinks.",
            mutates: true,
            supports_dry_run: true,
            requires_root: true,
            supports_yes: false,
            key_args: &["--root <PATH>", "--install-dir <PATH>", "--no-sys"],
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
