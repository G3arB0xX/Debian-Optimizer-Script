mod commands;

use std::env;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use debopty_core::config::DEFAULT_CONFIG_PATH;
use debopty_tui::TuiError;
use thiserror::Error;

use crate::commands::core::{
    BridgeArgs, ConfigAction, F2bSyncArgs, F2bSyncMulticallCli, StatusArgs, TemplateAction,
    VersionArgs, handle_bridge, handle_config, handle_status, handle_template, handle_version,
    run_f2b_sync_cmd,
};
use crate::commands::network::{NetworkAction, handle_network};
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
    /// 通过兼容桥接层显式调用未迁移的 Bash 模块或查看注册表
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

fn dispatch() -> Result<i32, CliError> {
    if is_f2b_sync_multicall() {
        let args = F2bSyncMulticallCli::parse();
        return run_f2b_sync_cmd(args.print_ports, args.force_reload, false, None);
    }

    let cli = Cli::parse();
    let explicit_config_path = resolve_explicit_config_path(cli.config.as_deref());
    let config_path = resolve_config_path(cli.config.as_deref());

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
        Some(Commands::Bridge(args)) => {
            handle_bridge(args, cli.script_dir.as_deref(), &config_path)
        }
    }
}

fn main() -> ExitCode {
    match dispatch() {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(err) => {
            eprintln!("debopty: {err}");
            ExitCode::from(err.exit_code())
        }
    }
}
