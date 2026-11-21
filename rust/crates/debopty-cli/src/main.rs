mod commands;

use std::env;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{ArgAction, Parser, Subcommand};
use debopty_core::config::DEFAULT_CONFIG_PATH;
use debopty_core::diag::{self, DiagConfig, LogLevel};
use debopty_tui::TuiError;
use thiserror::Error;

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
use crate::commands::tui::run_default_entrypoint;

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
}

impl CliError {
    fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(_) => 2,
            _ => 1,
        }
    }

    fn error_code(&self) -> &'static str {
        match self {
            Self::Core(err) => err.error_code(),
            Self::Io(_) => "E1001",
            Self::Usage(_) => "E1002",
            Self::NotFound(_) => "E1004",
            Self::Bridge(_) => "E6001",
            Self::Tui(_) => "E6002",
            Self::Json(_) => "E6003",
        }
    }

    fn remediation_hint(&self) -> &'static str {
        match self {
            Self::Core(err) => err.remediation_hint(),
            Self::Io(_) => "请检查目标文件路径、磁盘空间与当前用户读写权限。",
            Self::Usage(_) => "请检查命令行参数组合与取值格式，使用 --help 查看合法用法。",
            Self::NotFound(_) => "目标配置项或资源不存在，请核对名称或先执行初始化。",
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
    about = "debopty — Debian 10-13+ 系统优化与自动化服务部署工具 (Rust 引擎)"
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

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
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

fn dispatch() -> Result<i32, CliError> {
    if is_f2b_sync_multicall() {
        let args = F2bSyncMulticallCli::parse();
        return run_f2b_sync_cmd(args.print_ports, args.force_reload, false, None);
    }

    let cli = Cli::parse();
    init_diagnostics(&cli)?;

    let explicit_config_path = resolve_explicit_config_path(cli.config.as_deref());
    let config_path = resolve_config_path(cli.config.as_deref());

    let cmd_name = match &cli.command {
        None => "tui",
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
    };
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
        None => run_default_entrypoint(&cli, &config_path),
        Some(Commands::Version(args)) => handle_version(args),
        Some(Commands::Status(args)) => handle_status(args, &config_path),
        Some(Commands::Config { action }) => handle_config(action, &config_path),
        Some(Commands::Template { action }) => handle_template(action, cli.template_dir.as_deref()),
        Some(Commands::F2bSync(args)) => run_f2b_sync_cmd(
            args.print_ports,
            args.force_reload,
            args.json,
            cli.template_dir.as_deref(),
        ),
        Some(Commands::Network { action }) => handle_network(action, &config_path),
        Some(Commands::System { action }) => handle_system(
            action,
            explicit_config_path.as_deref(),
            cli.template_dir.as_deref(),
        ),
        Some(Commands::Security { action }) => handle_security(
            action,
            explicit_config_path.as_deref(),
            cli.template_dir.as_deref(),
        ),
        Some(Commands::Reconcile(args)) => handle_reconcile(
            args,
            explicit_config_path.as_deref(),
            cli.template_dir.as_deref(),
        ),
        Some(Commands::App { action }) => handle_app(
            action,
            explicit_config_path.as_deref(),
            cli.template_dir.as_deref(),
        ),
        Some(Commands::SelfInstall(args)) => handle_self_install(
            args,
            explicit_config_path.as_deref(),
            cli.template_dir.as_deref(),
        ),
        Some(Commands::SelfUpdate(args)) => handle_self_update(
            args,
            explicit_config_path.as_deref(),
            cli.template_dir.as_deref(),
        ),
        Some(Commands::SelfUninstall(args)) => handle_self_uninstall(
            args,
            explicit_config_path.as_deref(),
            cli.template_dir.as_deref(),
        ),
        Some(Commands::UiPreview(args)) => handle_ui_preview(args, &config_path),
        Some(Commands::Bridge(args)) => {
            handle_bridge(args, cli.script_dir.as_deref(), &config_path)
        }
    }
}

fn main() -> ExitCode {
    match dispatch() {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(err) => {
            let fmt = diag::current_config().format;
            let formatted = diag::format_cli_error(
                err.error_code(),
                &err.to_string(),
                err.remediation_hint(),
                fmt,
            );
            eprintln!("{formatted}");
            ExitCode::from(err.exit_code())
        }
    }
}
