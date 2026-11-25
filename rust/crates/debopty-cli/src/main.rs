mod agent;
mod commands;

use std::env;
use std::ffi::OsStr;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use clap::{ArgAction, Parser, Subcommand};
use debopty_core::CoreError;
use debopty_core::config::DEFAULT_CONFIG_PATH;
use debopty_core::diag::{self, DiagConfig, LogLevel};
use debopty_tui::TuiError;
use thiserror::Error;

use crate::agent::{AgentCtx, SchemaArgs, handle_schema};
use crate::commands::agent_ws::{AgentAction, handle_agent_ws};
use crate::commands::apps::{
    AppAction, SelfInstallArgs, SelfUninstallArgs, SelfUpdateArgs, UiPreviewArgs, handle_app,
    handle_self_install, handle_self_uninstall, handle_self_update, handle_ui_preview,
};
use crate::commands::core::{
    BridgeArgs, ConfigAction, F2bSyncArgs, F2bSyncMulticallCli, StatusArgs, TemplateAction,
    VersionArgs, handle_bridge, handle_config, handle_status, handle_template, handle_version,
    run_f2b_sync_cmd,
};
use crate::commands::network::{NetworkAction, handle_network};
use crate::commands::reconcile::{ReconcileArgs, handle_reconcile};
use crate::commands::security::{SecurityAction, handle_security};
use crate::commands::system::{SystemAction, handle_system};
use crate::commands::tui::{handle_tui_exec_outcome, run_default_entrypoint};

#[derive(Debug, Error)]
pub(crate) enum CliError {
    #[error("{0}")]
    Core(#[from] debopty_core::CoreError),
    #[error("{0}")]
    Bridge(#[from] debopty_bridge::BridgeError),
    #[error("{0}")]
    Tui(#[from] TuiError),
    #[error("序列化 JSON 失败: {0}")]
    Json(#[from] serde_json::Error),
    #[error("I/O 错误: {0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Usage(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    ConfirmationRequired(String),
}

impl CliError {
    pub(crate) fn exit_code(&self) -> u8 {
        match self {
            Self::Core(CoreError::AgentPolicy { code, .. }) => *code,
            Self::Usage(_)
            | Self::Core(
                CoreError::ValidationFailed(_)
                | CoreError::InvalidVersion(_)
                | CoreError::InvalidBump(_)
                | CoreError::InvalidConfig(_),
            ) => 2,
            Self::Io(_) | Self::Core(CoreError::Io(_)) => 3,
            Self::NotFound(_) | Self::Core(CoreError::TemplateNotFound(_)) => 4,
            Self::Core(
                CoreError::Network(_)
                | CoreError::HttpStatus { .. }
                | CoreError::CommandFailed { .. }
                | CoreError::CommandTimeout { .. },
            ) => 5,
            Self::ConfirmationRequired(_) => 6,
            Self::Core(_) | Self::Bridge(_) | Self::Tui(_) | Self::Json(_) => 1,
        }
    }

    pub(crate) fn error_code(&self) -> &'static str {
        match self {
            Self::Core(err) => err.error_code(),
            Self::Io(_) => "E1001",
            Self::Usage(_) => "E1002",
            Self::NotFound(_) => "E1004",
            Self::ConfirmationRequired(_) => "E1005",
            Self::Bridge(_) => "E6001",
            Self::Tui(_) => "E6002",
            Self::Json(_) => "E6003",
        }
    }

    pub(crate) fn category(&self) -> &'static str {
        match self {
            Self::Core(CoreError::AgentPolicy { kind, .. }) => kind,
            Self::Usage(_)
            | Self::Core(
                CoreError::ValidationFailed(_)
                | CoreError::InvalidVersion(_)
                | CoreError::InvalidBump(_)
                | CoreError::InvalidConfig(_),
            ) => "usage",
            Self::Io(_) | Self::Core(CoreError::Io(_)) => "io",
            Self::NotFound(_) | Self::Core(CoreError::TemplateNotFound(_)) => "not_found",
            Self::Core(CoreError::Network(_) | CoreError::HttpStatus { .. }) => "network",
            Self::Core(CoreError::CommandFailed { .. } | CoreError::CommandTimeout { .. }) => {
                "external"
            }
            Self::ConfirmationRequired(_) => "confirmation_required",
            Self::Core(_) | Self::Bridge(_) | Self::Tui(_) | Self::Json(_) => "runtime",
        }
    }

    pub(crate) fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Core(
                CoreError::Network(_)
                    | CoreError::HttpStatus { .. }
                    | CoreError::CommandTimeout { .. }
            )
        )
    }

    pub(crate) fn remediation_hint(&self) -> &'static str {
        match self {
            Self::Core(err) => err.remediation_hint(),
            Self::Io(_) => "请检查目标文件路径、磁盘空间与当前用户读写权限。",
            Self::Usage(_) => "请检查命令行参数组合与取值格式，使用 --help 查看合法用法。",
            Self::NotFound(_) => "目标配置项或资源不存在，请核对名称或先执行初始化。",
            Self::ConfirmationRequired(_) => {
                "非交互模式执行高危变更需显式确认：请追加 `--yes` (`-y`) 确认执行，或先使用 `--dry-run` 预览变更计划。"
            }
            Self::Bridge(_) => {
                "请确认 /opt/debopty 或 --script-dir 指向包含完整 scripts/ 目录的仓库路径。"
            }
            Self::Tui(_) => "终端交互界面初始化异常，请尝试使用 `--ui console` 或检查 TERM 设置。",
            Self::Json(_) => "JSON 序列化/解析失败，请检查输入数据结构并附加 `-vv` 排查。",
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "debopty",
    about = "debopty — Debian 10-13+ 系统优化与自动化服务部署工具"
)]
pub(crate) struct Cli {
    /// 执行引擎选择: auto | rust | bash (默认读取 DEBOPTY_ENGINE 或 auto)
    #[arg(long, global = true, value_name = "MODE")]
    pub(crate) engine: Option<String>,

    /// 交互式界面后端: console | ratatui | auto (默认: console，支持 DEBOPTY_UI 环境变量)
    #[arg(long, global = true, value_name = "BACKEND")]
    pub(crate) ui: Option<String>,

    /// 持久化配置文件路径 (默认 /etc/debopti/debopti.conf 或 CONFIG_FILE)
    #[arg(long, global = true, value_name = "PATH")]
    pub(crate) config: Option<PathBuf>,

    /// Bash 脚本根目录覆盖 (默认自动发现 DEBOPTY_SCRIPT_DIR / /opt/debopty / 当前仓库)
    #[arg(long, global = true, value_name = "PATH")]
    pub(crate) script_dir: Option<PathBuf>,

    /// 模板目录覆盖 (优先于编译内嵌模板)
    #[arg(long, global = true, value_name = "PATH")]
    pub(crate) template_dir: Option<PathBuf>,

    /// 诊断日志级别: error | warn | info | debug | trace (默认 warn，支持 DEBOPTY_LOG / RUST_LOG)
    #[arg(long, global = true, value_name = "LEVEL")]
    pub(crate) log_level: Option<String>,

    /// 诊断日志与错误格式: text | json (默认 text，支持 DEBOPTY_LOG_FORMAT，仅输出到 stderr)
    #[arg(long, global = true, value_name = "FORMAT")]
    pub(crate) log_format: Option<String>,

    /// 提高诊断详细度 (-v info, -vv debug, -vvv trace)
    #[arg(short = 'v', long = "verbose", action = ArgAction::Count, global = true)]
    pub(crate) verbose: u8,

    /// 静默模式（仅在 stderr 输出 error 级诊断）
    #[arg(short = 'q', long = "quiet", global = true)]
    pub(crate) quiet: bool,

    /// 以结构化 JSON 格式输出命令结果与错误到 stdout
    #[arg(long, global = true)]
    pub(crate) json: bool,

    /// 强制以 AgentEnvelope v1 信封格式输出所有 JSON 结果 (隐含 --json，支持 DEBOPTY_AGENT=1)
    #[arg(long, global = true)]
    pub(crate) envelope: bool,

    /// 跳过交互确认，显式授权执行高危变更
    #[arg(short = 'y', long, global = true)]
    pub(crate) yes: bool,

    /// 仅预演变更计划与受影响路径，不写入任何磁盘或系统变更
    #[arg(long, global = true)]
    pub(crate) dry_run: bool,

    /// 显式启用非交互模式 (跳过 TUI 与终端提示，支持 DEBOPTY_NON_INTERACTIVE=1)
    #[arg(long, global = true)]
    pub(crate) non_interactive: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// 输出面向自动化与 Agent 的 CLI 能力清单、信封契约与退出码表
    Schema(SchemaArgs),
    /// 管理宿主机 Agent 预设工作环境、原子检查点、审计日志、维护记忆与 0700/0600 凭据库
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },
    /// 显示或解析 debopty 版本身份 (SemVer + CalVer/Build)
    Version(VersionArgs),
    /// 探测系统优化状态、工具安装状态与模块迁移注册表
    Status(StatusArgs),
    /// 读写 Bash 兼容的持久化配置文件
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// 列出或渲染单真理源模板文件
    Template {
        #[command(subcommand)]
        action: TemplateAction,
    },
    /// 同步 nftables 公网端口与 Fail2ban 规则 (与 debopty-f2b-sync 等价)
    F2bSync(F2bSyncArgs),
    /// 原生网络管理 (DNS 校准、区域检测、镜像下载与 Git 克隆)
    Network {
        #[command(subcommand)]
        action: NetworkAction,
    },
    /// 原生系统优化 (Tmux、内核转发、Sysctl、Limits、Logrotate、低内存与基础优化)
    System {
        #[command(subcommand)]
        action: SystemAction,
    },
    /// 原生安全防御 (nftables 规则管理、NDP 修补、Fail2ban 与 SSH 加固)
    Security {
        #[command(subcommand)]
        action: SecurityAction,
    },
    /// 9 域脏系统冲突巡检、无损稳态收敛与原子备份回滚
    Reconcile(ReconcileArgs),
    /// 原生应用服务部署与运维管理 (11 应用全原生 Rust)
    App {
        #[command(subcommand)]
        action: AppAction,
    },
    /// 将当前二进制安装至 /opt/debopty 并配置全局快捷命令与多调用软链接
    SelfInstall(SelfInstallArgs),
    /// 从 GitHub Release 校验 SHA-256 并原子升级 debopty 二进制
    SelfUpdate(SelfUpdateArgs),
    /// 卸载 debopty 二进制、软链接与 Shell 快捷方式 (默认保留 /etc/debopti)
    SelfUninstall(SelfUninstallArgs),
    /// 非交互预览 TUI 仪表盘与 U4 视觉展示帧
    UiPreview(UiPreviewArgs),
    /// 通过兼容桥接层显式调用已归档 Bash 模块或查看注册表
    Bridge(BridgeArgs),
    /// 内部提权执行入口（由非 root TUI 通过 sudo 调起，隐藏不对外暴露）
    #[command(hide = true, name = "_tui-exec-outcome")]
    TuiExecOutcome {
        #[arg(long, value_name = "PATH")]
        file: PathBuf,
    },
}

fn is_f2b_sync_multicall() -> bool {
    env::args_os()
        .next()
        .and_then(|arg0| {
            Path::new(&arg0)
                .file_name()
                .map(|name| name == OsStr::new("debopty-f2b-sync"))
        })
        .unwrap_or(false)
}

pub(crate) fn non_empty_env(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.is_empty())
}

pub(crate) fn env_truthy(key: &str) -> bool {
    non_empty_env(key).is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

fn resolve_explicit_config_path(cli_config: Option<&Path>) -> Option<PathBuf> {
    cli_config
        .map(Path::to_path_buf)
        .or_else(|| non_empty_env("CONFIG_FILE").map(PathBuf::from))
}

fn resolve_config_path(cli_config: Option<&Path>) -> PathBuf {
    resolve_explicit_config_path(cli_config).unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH))
}

fn init_diagnostics(cli: &Cli) -> Result<(), CliError> {
    let env_level = non_empty_env("DEBOPTY_LOG").or_else(|| non_empty_env("RUST_LOG"));
    let env_format = non_empty_env("DEBOPTY_LOG_FORMAT");
    let no_color = non_empty_env("NO_COLOR").is_some();

    let level = diag::resolve_level(
        cli.log_level.as_deref(),
        cli.verbose,
        cli.quiet,
        env_level.as_deref(),
    )
    .map_err(|e| CliError::Usage(e.to_string()))?;
    let format = diag::resolve_format(cli.log_format.as_deref(), env_format.as_deref())
        .map_err(|e| CliError::Usage(e.to_string()))?;

    diag::init_global(DiagConfig {
        level,
        format,
        no_color,
    });
    Ok(())
}

fn command_label(command: Option<&Commands>) -> &'static str {
    match command {
        None => "tui",
        Some(Commands::Schema(_)) => "schema",
        Some(Commands::Agent { .. }) => "agent",
        Some(Commands::Version(_)) => "version",
        Some(Commands::Status(_)) => "status",
        Some(Commands::Config { .. }) => "config",
        Some(Commands::Template { .. }) => "template",
        Some(Commands::F2bSync(_)) => "f2b-sync",
        Some(Commands::Network { .. }) => "network",
        Some(Commands::System { .. }) => "system",
        Some(Commands::Security { .. }) => "security",
        Some(Commands::Reconcile(_)) => "reconcile",
        Some(Commands::App { .. }) => "app",
        Some(Commands::SelfInstall(_)) => "self-install",
        Some(Commands::SelfUpdate(_)) => "self-update",
        Some(Commands::SelfUninstall(_)) => "self-uninstall",
        Some(Commands::UiPreview(_)) => "ui-preview",
        Some(Commands::Bridge(_)) => "bridge",
        Some(Commands::TuiExecOutcome { .. }) => "_tui-exec-outcome",
    }
}

fn command_requires_root(cmd: &Commands) -> bool {
    match cmd {
        Commands::Schema(_)
        | Commands::Version(_)
        | Commands::Status(_)
        | Commands::Template { .. }
        | Commands::UiPreview(_)
        | Commands::TuiExecOutcome { .. }
        | Commands::Agent { .. } => false,
        Commands::Config { action } => matches!(action, ConfigAction::Set { .. }),
        Commands::F2bSync(args) => !args.print_ports,
        Commands::Network { action } => matches!(action, NetworkAction::DnsCheck { .. }),
        Commands::System { action } => !matches!(action, SystemAction::Hardware { .. }),
        Commands::Security { .. } => true,
        Commands::Reconcile(args) => args.apply || args.restore.is_some(),
        Commands::App { action } => !matches!(
            action,
            AppAction::XrayStatus { .. }
                | AppAction::FreshipStatus { .. }
                | AppAction::FreshipLogs { .. }
                | AppAction::LegoList { .. }
        ),
        Commands::SelfInstall(_) | Commands::SelfUpdate(_) | Commands::SelfUninstall(_) => true,
        Commands::Bridge(args) => !args.list,
    }
}

fn command_has_custom_root(cmd: &Commands) -> bool {
    match cmd {
        Commands::System { action } => match action {
            SystemAction::TmuxSetup { conf, .. } | SystemAction::TmuxUninstall { conf, .. } => {
                conf.is_some()
            }
            SystemAction::IpForwardToggle { root, .. }
            | SystemAction::Sysctl { root, .. }
            | SystemAction::Limits { root }
            | SystemAction::Logrotate { root }
            | SystemAction::LowMem { root, .. }
            | SystemAction::Hardware { root }
            | SystemAction::Retention { root, .. }
            | SystemAction::BaseOptimize { root, .. } => root.is_some(),
            SystemAction::Timezone { no_sys } => *no_sys,
        },
        Commands::Security { action } => match action {
            SecurityAction::FwAdd { root, .. }
            | SecurityAction::FwRemove { root, .. }
            | SecurityAction::FwUpdateSsh { root, .. }
            | SecurityAction::NdpPatch { root, .. }
            | SecurityAction::F2bSetup { root, .. }
            | SecurityAction::SshHarden { root, .. }
            | SecurityAction::Setup { root, .. } => root.is_some(),
        },
        Commands::Reconcile(args) => args.root.is_some(),
        Commands::App { action } => match action {
            AppAction::Install { root, .. }
            | AppAction::Uninstall { root, .. }
            | AppAction::PodmanGcToggle { root, .. }
            | AppAction::XrayRulesetToggle { root, .. }
            | AppAction::XrayCronToggle { root, .. }
            | AppAction::XrayStatus { root, .. }
            | AppAction::WarpXrayGen { root, .. }
            | AppAction::FreshipStatus { root, .. }
            | AppAction::FreshipToggle { root, .. }
            | AppAction::FreshipSync { root, .. }
            | AppAction::LegoList { root }
            | AppAction::LegoAdd { root, .. }
            | AppAction::LegoToggle { root, .. }
            | AppAction::LegoRenew { root, .. }
            | AppAction::LegoRemove { root, .. }
            | AppAction::DevopsSotSync { root, .. } => root.is_some(),
            AppAction::FreshipLogs { .. } => false,
        },
        Commands::SelfInstall(args) => args.root.is_some(),
        Commands::SelfUpdate(args) => args.root.is_some(),
        Commands::SelfUninstall(args) => args.root.is_some(),
        Commands::Network {
            action: NetworkAction::DnsCheck { resolv, .. },
        } => resolv.is_some(),
        Commands::F2bSync(_) => non_empty_env("DEBOPTY_F2B_ROOT").is_some(),
        _ => false,
    }
}

fn should_prompt_cli_elevation(
    cmd: &Commands,
    agent: &AgentCtx,
    is_root: bool,
    is_tty: bool,
    custom_root_set: bool,
    retry_guard: bool,
) -> bool {
    !is_root
        && is_tty
        && !agent.non_interactive
        && !agent.json
        && !agent.dry_run
        && !custom_root_set
        && !retry_guard
        && command_requires_root(cmd)
        && env::var("DEBOPTY_NO_AUTO_SUDO").ok().as_deref() != Some("1")
}

fn try_prompt_cli_sudo_reexec() -> Option<ExitCode> {
    let mut stderr = io::stderr().lock();
    if write!(
        stderr,
        "此操作需要管理员权限，是否通过 sudo 提权执行？ [Y/n]: "
    )
    .and_then(|()| stderr.flush())
    .is_err()
    {
        return None;
    }
    drop(stderr);

    let mut line = String::new();
    match io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => return None,
        Ok(_) => {}
    }
    let ans = line.trim();
    if !(ans.is_empty() || ans.eq_ignore_ascii_case("y") || ans.eq_ignore_ascii_case("yes")) {
        return None;
    }

    let exe = match env::current_exe() {
        Ok(p) => p,
        Err(err) => {
            eprintln!("定位 debopty 可执行文件失败: {err}");
            return Some(ExitCode::from(1));
        }
    };
    let status = Command::new("sudo")
        .arg("--")
        .arg(&exe)
        .args(env::args_os().skip(1))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();

    match status {
        Ok(st) => Some(ExitCode::from(
            st.code().and_then(|c| u8::try_from(c).ok()).unwrap_or(1),
        )),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            eprintln!("未找到 sudo 命令，请使用 root 账户重新运行 debopty。");
            Some(ExitCode::from(1))
        }
        Err(err) => {
            eprintln!("调用 sudo 提权失败: {err}");
            Some(ExitCode::from(1))
        }
    }
}

fn dispatch_cli(cli: &Cli, agent_ctx: &AgentCtx, cmd_name: &str) -> Result<i32, CliError> {
    init_diagnostics(cli)?;

    let explicit_config_path = resolve_explicit_config_path(cli.config.as_deref());
    let config_path = resolve_config_path(cli.config.as_deref());

    diag::log_event(
        LogLevel::Debug,
        "debopty::cli",
        None,
        "dispatching CLI command",
        None,
        &[
            ("command", cmd_name),
            ("config", &config_path.to_string_lossy()),
        ],
    );

    match &cli.command {
        None => run_default_entrypoint(cli, &config_path, agent_ctx),
        Some(Commands::Schema(args)) => handle_schema(args, agent_ctx),
        Some(Commands::Agent { action }) => {
            handle_agent_ws(action, cli.template_dir.as_deref(), agent_ctx)
        }
        Some(Commands::Version(args)) => handle_version(args, agent_ctx),
        Some(Commands::Status(args)) => handle_status(args, &config_path, agent_ctx),
        Some(Commands::Config { action }) => handle_config(action, &config_path, agent_ctx),
        Some(Commands::Template { action }) => {
            handle_template(action, cli.template_dir.as_deref(), agent_ctx)
        }
        Some(Commands::F2bSync(args)) => run_f2b_sync_cmd(
            args.print_ports,
            args.force_reload,
            agent_ctx,
            cli.template_dir.as_deref(),
        ),
        Some(Commands::Network { action }) => handle_network(action, &config_path, agent_ctx),
        Some(Commands::System { action }) => handle_system(
            action,
            explicit_config_path.as_deref(),
            cli.template_dir.as_deref(),
            agent_ctx,
        ),
        Some(Commands::Security { action }) => handle_security(
            action,
            explicit_config_path.as_deref(),
            cli.template_dir.as_deref(),
            agent_ctx,
        ),
        Some(Commands::Reconcile(args)) => handle_reconcile(
            args,
            explicit_config_path.as_deref(),
            cli.template_dir.as_deref(),
            agent_ctx,
        ),
        Some(Commands::App { action }) => handle_app(
            action,
            explicit_config_path.as_deref(),
            cli.template_dir.as_deref(),
            cli.quiet,
            agent_ctx,
        ),
        Some(Commands::SelfInstall(args)) => handle_self_install(
            args,
            explicit_config_path.as_deref(),
            cli.template_dir.as_deref(),
            agent_ctx,
        ),
        Some(Commands::SelfUpdate(args)) => handle_self_update(
            args,
            explicit_config_path.as_deref(),
            cli.template_dir.as_deref(),
            agent_ctx,
        ),
        Some(Commands::SelfUninstall(args)) => handle_self_uninstall(
            args,
            explicit_config_path.as_deref(),
            cli.template_dir.as_deref(),
            agent_ctx,
        ),
        Some(Commands::UiPreview(args)) => handle_ui_preview(args, &config_path),
        Some(Commands::Bridge(args)) => {
            handle_bridge(args, cli.script_dir.as_deref(), &config_path, agent_ctx)
        }
        Some(Commands::TuiExecOutcome { file }) => {
            handle_tui_exec_outcome(file, &config_path, cli.template_dir.as_deref())
        }
    }
}

fn emit_cli_error_stderr(err: &CliError) {
    let fmt = diag::current_config().format;
    let formatted = diag::format_cli_error(
        err.error_code(),
        &err.to_string(),
        err.remediation_hint(),
        fmt,
    );
    eprintln!("{formatted}");
}

fn main() -> ExitCode {
    if is_f2b_sync_multicall() {
        let args = F2bSyncMulticallCli::parse();
        let agent_ctx = AgentCtx::default();
        return match run_f2b_sync_cmd(args.print_ports, args.force_reload, &agent_ctx, None) {
            Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
            Err(err) => {
                emit_cli_error_stderr(&err);
                ExitCode::from(err.exit_code())
            }
        };
    }

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(clap_err) => {
            if !clap_err.use_stderr() {
                let _ = clap_err.print();
                return ExitCode::SUCCESS;
            }
            if agent::detect_json_mode_from_args() {
                let cmd = agent::detect_command_from_args();
                let dry_run = agent::detect_dry_run_from_args();
                let err = CliError::Usage(clap_err.to_string().trim().to_string());
                agent::emit_error_envelope(&cmd, dry_run, &err);
                return ExitCode::from(err.exit_code());
            }
            clap_err.exit();
        }
    };

    let agent_ctx = AgentCtx::from_cli(&cli);
    let cmd_name = command_label(cli.command.as_ref());

    if let Some(cmd) = cli.command.as_ref() {
        let is_root = unsafe { libc::geteuid() == 0 };
        let is_tty = io::stdin().is_terminal() && io::stderr().is_terminal();
        let custom_root = command_has_custom_root(cmd) || cli.config.is_some();
        let retry_guard = env_truthy("DEBOPTY_ELEVATED_RETRY");
        if should_prompt_cli_elevation(cmd, &agent_ctx, is_root, is_tty, custom_root, retry_guard)
            && let Some(code) = try_prompt_cli_sudo_reexec()
        {
            return code;
        }
    }

    match dispatch_cli(&cli, &agent_ctx, cmd_name) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(err) => {
            if agent_ctx.json {
                agent::emit_error_envelope(cmd_name, agent_ctx.dry_run, &err);
            } else {
                emit_cli_error_stderr(&err);
            }
            ExitCode::from(err.exit_code())
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::{Cli, should_prompt_cli_elevation};
    use crate::agent::AgentCtx;

    #[test]
    fn cli_command_tree_has_zero_duplicate_global_flags() {
        Cli::command().debug_assert();
    }

    #[test]
    fn should_prompt_cli_elevation_matrix_is_strictly_fail_closed() {
        let priv_cli = Cli::try_parse_from(["debopty", "system", "sysctl"]).expect("parse");
        let priv_cmd = priv_cli.command.as_ref().expect("subcommand");
        let ro_cli = Cli::try_parse_from(["debopty", "system", "hardware"]).expect("parse");
        let ro_cmd = ro_cli.command.as_ref().expect("subcommand");

        let interactive = AgentCtx {
            json: false,
            envelope: false,
            yes: false,
            dry_run: false,
            non_interactive: false,
            explicit_non_interactive: false,
        };

        // True ONLY on non-root interactive TTY privileged command without custom root or retry guard
        assert!(should_prompt_cli_elevation(
            priv_cmd,
            &interactive,
            false,
            true,
            false,
            false
        ));

        // False on root, non-TTY, custom root, retry guard, or read-only command
        assert!(!should_prompt_cli_elevation(
            priv_cmd,
            &interactive,
            true,
            true,
            false,
            false
        ));
        assert!(!should_prompt_cli_elevation(
            priv_cmd,
            &interactive,
            false,
            false,
            false,
            false
        ));
        assert!(!should_prompt_cli_elevation(
            priv_cmd,
            &interactive,
            false,
            true,
            true,
            false
        ));
        assert!(!should_prompt_cli_elevation(
            priv_cmd,
            &interactive,
            false,
            true,
            false,
            true
        ));
        assert!(!should_prompt_cli_elevation(
            ro_cmd,
            &interactive,
            false,
            true,
            false,
            false
        ));

        // False on --json, --non-interactive, --dry-run
        let json_ctx = AgentCtx {
            json: true,
            ..interactive
        };
        let ni_ctx = AgentCtx {
            non_interactive: true,
            ..interactive
        };
        let dry_ctx = AgentCtx {
            dry_run: true,
            ..interactive
        };
        assert!(!should_prompt_cli_elevation(
            priv_cmd, &json_ctx, false, true, false, false
        ));
        assert!(!should_prompt_cli_elevation(
            priv_cmd, &ni_ctx, false, true, false, false
        ));
        assert!(!should_prompt_cli_elevation(
            priv_cmd, &dry_ctx, false, true, false, false
        ));
    }
}
