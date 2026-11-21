use std::collections::BTreeMap;
use std::env;
use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

use clap::{Args, Parser, Subcommand};
use debopty_bridge::{
    BashAction, BashBridge, EngineMode, ModuleImpl, ModuleStatusEntry, module_registry,
};
use debopty_core::config::{ConfigStore, DEFAULT_CONFIG_PATH};
use debopty_core::security::f2b_sync::{F2bSyncOptions, F2bSyncReport, run_f2b_sync};
use debopty_core::status::{StatusSnapshot, probe_status};
use debopty_core::sys::RealCommandRunner;
use debopty_core::template::TemplateEngine;
use debopty_core::version::{self, VersionIdentity};
use debopty_tui::{
    AppState, ConsoleRenderer, TuiError, TuiOutcome, is_non_interactive, run_tui_loop_with_state,
};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
enum CliError {
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
struct Cli {
    /// 执行引擎选择: auto | rust | bash (默认读取 DEBOPTY_ENGINE 或 auto)
    #[arg(long, global = true, value_name = "MODE")]
    engine: Option<String>,

    /// 持久化配置文件路径 (默认 /etc/debopti/debopti.conf 或 CONFIG_FILE)
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Bash 脚本根目录覆盖 (默认自动发现 DEBOPTY_SCRIPT_DIR / /opt/debopty / 当前仓库)
    #[arg(long, global = true, value_name = "PATH")]
    script_dir: Option<PathBuf>,

    /// 模板目录覆盖 (优先于编译内嵌模板)
    #[arg(long, global = true, value_name = "PATH")]
    template_dir: Option<PathBuf>,

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
    /// 通过兼容桥接层显式调用未迁移的 Bash 模块或查看注册表
    Bridge(BridgeArgs),
}

#[derive(Debug, Args)]
struct VersionArgs {
    /// 以 JSON 格式输出完整版本元数据
    #[arg(long)]
    json: bool,

    /// 从指定 VERSION 文件解析 (默认使用二进制内嵌版本)
    #[arg(long, value_name = "PATH")]
    file: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct StatusArgs {
    /// 以 JSON 格式输出状态快照与模块注册表
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum ConfigAction {
    /// 读取单个配置键
    Get {
        /// 配置键名
        key: String,
    },
    /// 原子写入或更新单个配置键 (保留注释与顺序)
    Set {
        /// 配置键名
        key: String,
        /// 配置值
        value: String,
    },
    /// 列出当前全部配置项
    List {
        /// 以 JSON 对象输出
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum TemplateAction {
    /// 列出全部内嵌模板路径
    List {
        /// 以 JSON 数组输出
        #[arg(long)]
        json: bool,
    },
    /// 渲染指定模板到标准输出或目标文件
    Render {
        /// 模板相对路径 (例如 templates/common/debopty.sh)
        rel_path: String,
        /// 输出目标文件路径 (省略或传 '-' 输出到标准输出)
        #[arg(long, value_name = "PATH")]
        out: Option<PathBuf>,
        /// 模板变量覆盖 (格式 KEY=VAL，可重复指定)
        #[arg(long = "var", value_name = "KEY=VAL")]
        vars: Vec<String>,
    },
}

#[derive(Debug, Args)]
struct F2bSyncArgs {
    /// 仅探测并打印端口摘要，不修改系统文件
    #[arg(long)]
    print_ports: bool,

    /// 即使配置未变化也强制重载 nftables / fail2ban
    #[arg(long)]
    force_reload: bool,

    /// 以 JSON 格式输出同步报告
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
#[command(
    name = "debopty-f2b-sync",
    about = "Debopty Fail2ban 公网开放端口自动同步工具"
)]
struct F2bSyncMulticallCli {
    /// 仅探测并打印端口摘要，不修改系统文件
    #[arg(long)]
    print_ports: bool,

    /// 即使配置未变化也强制重载 nftables / fail2ban
    #[arg(long)]
    force_reload: bool,
}

#[derive(Debug, Args)]
struct BridgeArgs {
    /// 要执行的桥接动作名称 (使用 --list 查看完整列表)
    action: Option<String>,

    /// 列出全部模块迁移状态与可用桥接动作
    #[arg(long)]
    list: bool,

    /// 以 JSON 格式输出注册表 (配合 --list 使用)
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Serialize)]
struct VersionOutput {
    semver: Option<String>,
    calver: Option<String>,
    build: String,
    primary: Option<String>,
    display: String,
    prompt: String,
    tag: Option<String>,
}

#[derive(Debug, Serialize)]
struct StatusOutput {
    status: StatusSnapshot,
    modules: Vec<ModuleStatusEntry>,
}

#[derive(Debug, Serialize)]
struct BridgeListOutput {
    modules: Vec<ModuleStatusEntry>,
    actions: Vec<String>,
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

fn non_empty_env(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.is_empty())
}

fn resolve_config_path(cli_config: Option<&Path>) -> PathBuf {
    cli_config
        .map(Path::to_path_buf)
        .or_else(|| non_empty_env("CONFIG_FILE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH))
}

fn resolve_engine_mode(cli_engine: Option<&str>) -> Result<EngineMode, CliError> {
    let raw = cli_engine
        .map(str::to_string)
        .or_else(|| non_empty_env("DEBOPTY_ENGINE"));
    match raw {
        Some(val) => EngineMode::from_str(&val).map_err(|e| CliError::Usage(e.to_string())),
        None => Ok(EngineMode::Auto),
    }
}

fn build_f2b_options(
    print_ports: bool,
    force_reload: bool,
    cli_template_dir: Option<&Path>,
) -> F2bSyncOptions {
    let root_prefix = non_empty_env("DEBOPTY_F2B_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    let no_sys = non_empty_env("DEBOPTY_F2B_NO_SYS")
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let backend_override = non_empty_env("DEBOPTY_F2B_BACKEND");
    let template_dir = non_empty_env("DEBOPTY_F2B_TEMPLATE_DIR")
        .map(PathBuf::from)
        .or_else(|| cli_template_dir.map(Path::to_path_buf));
    let ss_file = non_empty_env("DEBOPTY_F2B_SS_FILE").map(PathBuf::from);

    F2bSyncOptions {
        root_prefix,
        no_sys,
        force_reload,
        print_ports,
        backend_override,
        template_dir,
        ss_file,
    }
}

fn print_f2b_ports(report: &F2bSyncReport) {
    println!("SSH_PORT={}", report.ssh_port);
    println!("TCP_PORTS_NFT={}", report.tcp_ports_nft);
    println!("UDP_PORTS_NFT={}", report.udp_ports_nft);
    println!("ALL_PUBLIC_PORTS_F2B={}", report.all_public_ports_f2b);
}

fn run_f2b_sync_cmd(
    print_ports: bool,
    force_reload: bool,
    json: bool,
    cli_template_dir: Option<&Path>,
) -> Result<i32, CliError> {
    let opts = build_f2b_options(print_ports, force_reload, cli_template_dir);
    let templates = TemplateEngine::new(cli_template_dir.map(Path::to_path_buf));
    let runner = RealCommandRunner;
    let report = run_f2b_sync(&opts, &templates, &runner)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if print_ports {
        print_f2b_ports(&report);
    }
    Ok(0)
}

fn handle_version(args: &VersionArgs) -> Result<i32, CliError> {
    let id: VersionIdentity = match &args.file {
        Some(path) => version::parse_version_file(path)?,
        None => version::compiled_identity(),
    };

    if args.json {
        let primary = id.primary().map(str::to_string);
        let tag = primary
            .as_deref()
            .map(|p| version::tag_name(p, Some(&id.build)));
        let payload = VersionOutput {
            semver: id.semver.clone(),
            calver: id.calver.clone(),
            build: id.build.clone(),
            primary,
            display: version::format_display(&id),
            prompt: version::format_prompt(&id),
            tag,
        };
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!("{}", version::format_display(&id));
    }
    Ok(0)
}

fn handle_status(args: &StatusArgs, config_path: &Path) -> Result<i32, CliError> {
    let cfg = ConfigStore::new(config_path).load()?;
    let id = version::compiled_identity();
    let runner = RealCommandRunner;
    let status = probe_status(&cfg, &id, &runner);
    let modules = module_registry();

    if args.json {
        let payload = StatusOutput { status, modules };
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        let installed_tools: Vec<&str> = status
            .tools
            .iter()
            .filter_map(|(k, &v)| v.then_some(k.as_str()))
            .collect();
        let native_count = modules
            .iter()
            .filter(|m| m.implementation == ModuleImpl::NativeRust)
            .count();
        let bridge_count = modules.len().saturating_sub(native_count);

        println!("版本: {}", status.version_display);
        println!(
            "网络区域: {}",
            if status.is_cn_region {
                "中国大陆 (镜像加速)"
            } else {
                "海外直连"
            }
        );
        println!(
            "基础优化: {}",
            if status.base_optimized {
                "已执行"
            } else {
                "未执行"
            }
        );
        println!(
            "内核转发: {}",
            if status.ip_forward {
                "已开启"
            } else {
                "已关闭"
            }
        );
        println!(
            "Podman 定时清理: {}",
            if status.podman_gc_enabled {
                "已开启"
            } else {
                "已关闭"
            }
        );
        println!(
            "已检测工具 ({}/{}): {}",
            installed_tools.len(),
            status.tools.len(),
            if installed_tools.is_empty() {
                "无".to_string()
            } else {
                installed_tools.join(", ")
            }
        );
        println!(
            "模块实现: 原生 Rust {} 个 / 兼容桥接 {} 个 (共 {} 个)",
            native_count,
            bridge_count,
            modules.len()
        );
    }
    Ok(0)
}

fn handle_config(action: &ConfigAction, config_path: &Path) -> Result<i32, CliError> {
    let store = ConfigStore::new(config_path);
    match action {
        ConfigAction::Get { key } => match store.get(key)? {
            Some(val) => {
                println!("{val}");
                Ok(0)
            }
            None => Err(CliError::NotFound(format!("未找到配置项: {key}"))),
        },
        ConfigAction::Set { key, value } => {
            store.set(key, value)?;
            Ok(0)
        }
        ConfigAction::List { json } => {
            let map = store.list()?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&map)?);
            } else {
                for (k, v) in map {
                    println!("{k}={v}");
                }
            }
            Ok(0)
        }
    }
}

fn parse_var_pairs(pairs: &[String]) -> Result<BTreeMap<String, String>, CliError> {
    let mut vars = BTreeMap::new();
    for pair in pairs {
        let Some((k, v)) = pair.split_once('=') else {
            return Err(CliError::Usage(format!(
                "无效的 --var 参数 '{pair}'，必须使用 KEY=VAL 格式"
            )));
        };
        if k.is_empty() {
            return Err(CliError::Usage(format!(
                "无效的 --var 参数 '{pair}'，变量名不能为空"
            )));
        }
        vars.insert(k.to_string(), v.to_string());
    }
    Ok(vars)
}

fn handle_template(action: &TemplateAction, template_dir: Option<&Path>) -> Result<i32, CliError> {
    let engine = TemplateEngine::new(template_dir.map(Path::to_path_buf));
    match action {
        TemplateAction::List { json } => {
            let list = engine.list();
            if *json {
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else {
                for item in list {
                    println!("{item}");
                }
            }
            Ok(0)
        }
        TemplateAction::Render {
            rel_path,
            out,
            vars,
        } => {
            let kv = parse_var_pairs(vars)?;
            match out.as_deref() {
                None => {
                    let rendered = engine.render_to_string(rel_path, &kv)?;
                    let mut stdout = io::stdout().lock();
                    stdout.write_all(rendered.as_bytes())?;
                    stdout.flush()?;
                }
                Some(p) if p == Path::new("-") => {
                    let rendered = engine.render_to_string(rel_path, &kv)?;
                    let mut stdout = io::stdout().lock();
                    stdout.write_all(rendered.as_bytes())?;
                    stdout.flush()?;
                }
                Some(target) => {
                    engine.render_to_file(rel_path, target, &kv, None)?;
                }
            }
            Ok(0)
        }
    }
}

fn handle_bridge(
    args: &BridgeArgs,
    script_dir: Option<&Path>,
    config_path: &Path,
) -> Result<i32, CliError> {
    if args.list {
        let modules = module_registry();
        let actions: Vec<String> = BashAction::all_actions()
            .into_iter()
            .map(BashAction::cli_name)
            .collect();
        if args.json {
            let payload = BridgeListOutput { modules, actions };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        } else {
            println!("模块注册表:");
            for m in &modules {
                println!(
                    "  {:<18} {:<12} {}",
                    m.id,
                    m.implementation.as_str(),
                    m.summary
                );
            }
            println!("\n可用桥接动作 ({}):", actions.len());
            for act in &actions {
                println!("  {act}");
            }
        }
        return Ok(0);
    }

    let Some(raw_action) = args.action.as_deref() else {
        return Err(CliError::Usage(
            "请指定桥接动作名称，或使用 `debopty bridge --list` 查看可用列表".to_string(),
        ));
    };

    let action = BashAction::from_str(raw_action).map_err(|e| CliError::Usage(e.to_string()))?;
    let bridge = BashBridge::discover(script_dir, Some(config_path))?;
    let code = bridge.execute(&action, &[])?;
    Ok(code)
}

fn run_default_entrypoint(cli: &Cli, config_path: &Path) -> Result<i32, CliError> {
    let engine_mode = resolve_engine_mode(cli.engine.as_deref())?;
    if engine_mode == EngineMode::Bash {
        let bridge = BashBridge::discover(cli.script_dir.as_deref(), Some(config_path))?;
        return Ok(bridge.execute(&BashAction::LegacyFullEntry, &[])?);
    }

    if is_non_interactive() {
        println!("检测到非交互终端或 CI 环境，已跳过 TUI 启动；可使用 --help 查看子命令。");
        return Ok(0);
    }

    let store = ConfigStore::new(config_path);
    let cfg = store.load()?;
    let id = version::compiled_identity();
    let runner = RealCommandRunner;
    let initial_status = probe_status(&cfg, &id, &runner);

    let mut state = AppState::new(initial_status);
    let mut renderer = ConsoleRenderer::new();
    let template_dir = cli.template_dir.clone();
    let script_dir = cli.script_dir.clone();

    run_tui_loop_with_state(&mut renderer, &mut state, &mut |outcome, st| {
        match outcome {
            TuiOutcome::Exit => {}
            TuiOutcome::Execute(action) => {
                match BashBridge::discover(script_dir.as_deref(), Some(config_path)) {
                    Ok(bridge) => match bridge.execute(&action, &[]) {
                        Ok(code) => {
                            if code != 0 {
                                st.set_notice(format!("操作 {action} 退出码: {code}"));
                            }
                        }
                        Err(err) => {
                            st.set_notice(format!("执行 {action} 失败: {err}"));
                        }
                    },
                    Err(err) => {
                        st.set_notice(format!("未找到 Bash 脚本目录: {err}"));
                    }
                }
                if action.needs_pause_after() {
                    print!("\n执行完毕，按 Enter 返回菜单...");
                    let _ = io::stdout().flush();
                    let mut line = String::new();
                    let _ = io::stdin().read_line(&mut line);
                }
            }
            TuiOutcome::RunNativeF2bSync => {
                let opts = build_f2b_options(false, false, template_dir.as_deref());
                let templates = TemplateEngine::new(template_dir.clone());
                match run_f2b_sync(&opts, &templates, &runner) {
                    Ok(report) => {
                        st.set_notice(format!(
                            "端口同步完成 (SSH={}, 公网={})",
                            report.ssh_port, report.all_public_ports_f2b
                        ));
                    }
                    Err(err) => {
                        st.set_notice(format!("Fail2ban 端口同步失败: {err}"));
                    }
                }
            }
        }

        if let Ok(reloaded_cfg) = store.load() {
            let refreshed = probe_status(&reloaded_cfg, &id, &runner);
            st.refresh_status(refreshed);
        }
        Ok(())
    })?;

    Ok(0)
}

fn dispatch() -> Result<i32, CliError> {
    if is_f2b_sync_multicall() {
        let args = F2bSyncMulticallCli::parse();
        return run_f2b_sync_cmd(args.print_ports, args.force_reload, false, None);
    }

    let cli = Cli::parse();
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
