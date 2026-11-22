use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use clap::{Args, Parser, Subcommand};
use debopty_bridge::{BashAction, BashBridge, ModuleImpl, ModuleStatusEntry, module_registry};
use debopty_core::config::ConfigStore;
use debopty_core::security::f2b_sync::{F2bSyncOptions, F2bSyncReport, run_f2b_sync};
use debopty_core::status::{StatusSnapshot, probe_status};
use debopty_core::sys::RealCommandRunner;
use debopty_core::telemetry::{
    SystemSpecs, format_count_compact, format_kib_compact, probe_system_specs, shorten_cpu_model,
};
use debopty_core::template::TemplateEngine;
use debopty_core::version::{self, VersionIdentity};
use debopty_tui::has_ratatui_ui;
use serde::Serialize;

use crate::agent::{
    AgentCtx, DryRunPlan, emit_dry_run_plan, emit_envelope, emit_legacy_or_envelope,
};
use crate::{CliError, env_truthy, non_empty_env};

#[derive(Debug, Args)]
pub struct VersionArgs {
    /// 从指定 VERSION 文件解析 (默认使用二进制内嵌版本)
    #[arg(long, value_name = "PATH")]
    pub file: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct StatusArgs {}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
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
    List,
}

#[derive(Debug, Subcommand)]
pub enum TemplateAction {
    /// 列出全部内嵌模板路径
    List,
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
pub struct F2bSyncArgs {
    /// 仅探测并打印端口摘要，不修改系统文件
    #[arg(long)]
    pub print_ports: bool,

    /// 即使配置未变化也强制重载 nftables / fail2ban
    #[arg(long)]
    pub force_reload: bool,
}

#[derive(Debug, Parser)]
#[command(
    name = "debopty-f2b-sync",
    about = "Debopty Fail2ban 公网开放端口自动同步工具"
)]
pub struct F2bSyncMulticallCli {
    /// 仅探测并打印端口摘要，不修改系统文件
    #[arg(long)]
    pub print_ports: bool,

    /// 即使配置未变化也强制重载 nftables / fail2ban
    #[arg(long)]
    pub force_reload: bool,
}

#[derive(Debug, Args)]
pub struct BridgeArgs {
    /// 要执行的桥接动作名称 (使用 --list 查看完整列表)
    pub action: Option<String>,

    /// 列出全部模块迁移状态与可用桥接动作
    #[arg(long)]
    pub list: bool,
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
    telemetry: SystemSpecs,
    modules: Vec<ModuleStatusEntry>,
    ui_backends: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
struct BridgeListOutput {
    modules: Vec<ModuleStatusEntry>,
    actions: Vec<String>,
}

pub fn available_ui_backends() -> Vec<&'static str> {
    if has_ratatui_ui() {
        vec!["console", "ratatui"]
    } else {
        vec!["console"]
    }
}

pub fn build_f2b_options(
    print_ports: bool,
    force_reload: bool,
    cli_template_dir: Option<&Path>,
) -> F2bSyncOptions {
    let root_prefix = non_empty_env("DEBOPTY_F2B_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    let no_sys = env_truthy("DEBOPTY_F2B_NO_SYS");
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

pub fn run_f2b_sync_cmd(
    print_ports: bool,
    force_reload: bool,
    agent: &AgentCtx,
    cli_template_dir: Option<&Path>,
) -> Result<i32, CliError> {
    let opts = build_f2b_options(print_ports, force_reload, cli_template_dir);
    if agent.dry_run && !print_ports {
        let plan = DryRunPlan {
            command: "f2b-sync".to_string(),
            target: opts.root_prefix.display().to_string(),
            mutates_system: true,
            requires_root: true,
            requires_yes: false,
            planned_actions: vec![
                "Scan nftables.conf and nftables.d/debopti for public TCP/UDP ports".to_string(),
                "Render Fail2ban port jail and nftables guard rules".to_string(),
                "Reload nftables and fail2ban when changed".to_string(),
            ],
            affected_paths: vec![
                opts.root_prefix
                    .join("etc/fail2ban/jail.d/99-debopty-ports.local")
                    .display()
                    .to_string(),
                opts.root_prefix
                    .join("etc/nftables.d/55-debopty-f2b-guard.nft")
                    .display()
                    .to_string(),
            ],
        };
        return emit_dry_run_plan(agent, &plan);
    }

    let templates = TemplateEngine::new(cli_template_dir.map(Path::to_path_buf));
    let runner = RealCommandRunner;
    let report = run_f2b_sync(&opts, &templates, &runner)?;

    if agent.json {
        return emit_legacy_or_envelope(agent, "f2b-sync", &report);
    } else if print_ports {
        print_f2b_ports(&report);
    }
    Ok(0)
}

pub fn handle_version(args: &VersionArgs, agent: &AgentCtx) -> Result<i32, CliError> {
    let id: VersionIdentity = match &args.file {
        Some(path) => version::parse_version_file(path)?,
        None => version::compiled_identity(),
    };

    if agent.json {
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
        return emit_legacy_or_envelope(agent, "version", &payload);
    }
    println!("{}", version::format_display(&id));
    Ok(0)
}

pub fn handle_status(
    _args: &StatusArgs,
    config_path: &Path,
    agent: &AgentCtx,
) -> Result<i32, CliError> {
    let cfg = ConfigStore::new(config_path).load()?;
    let id = version::compiled_identity();
    let runner = RealCommandRunner;
    let status = probe_status(&cfg, &id, &runner);
    let telemetry = probe_system_specs(Path::new("/"));
    let modules = module_registry();
    let ui_backends = available_ui_backends();

    if agent.json {
        let payload = StatusOutput {
            status,
            telemetry,
            modules,
            ui_backends,
        };
        return emit_legacy_or_envelope(agent, "status", &payload);
    }

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
        "主机: {} ({} · {})",
        telemetry.hostname, telemetry.os_name, telemetry.arch
    );
    println!(
        "硬件: {} ({}C) · RAM {}",
        shorten_cpu_model(&telemetry.cpu_model),
        telemetry.cpu_cores,
        format_kib_compact(telemetry.mem_total_kib)
    );
    println!(
        "内核参数: TCP={} · qdisc={} · file-max={}",
        telemetry.tcp_cc,
        telemetry.default_qdisc,
        format_count_compact(telemetry.file_max)
    );
    println!(
        "SSH / 防护: SSH :{} · nftables={} · fail2ban={} ({})",
        telemetry.ssh_port,
        if telemetry.nftables_configured {
            "已配置"
        } else {
            "未配置"
        },
        if telemetry.fail2ban_configured {
            "已配置"
        } else {
            "未配置"
        },
        telemetry.fail2ban_backend
    );
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
    Ok(0)
}

pub fn handle_config(
    action: &ConfigAction,
    config_path: &Path,
    agent: &AgentCtx,
) -> Result<i32, CliError> {
    let store = ConfigStore::new(config_path);
    match action {
        ConfigAction::Get { key } => match store.get(key)? {
            Some(val) => {
                if agent.json {
                    emit_envelope(
                        "config get",
                        agent.dry_run,
                        &serde_json::json!({
                            "key": key,
                            "value": val,
                            "config_path": config_path,
                        }),
                    )
                } else {
                    println!("{val}");
                    Ok(0)
                }
            }
            None => Err(CliError::NotFound(format!("未找到配置项: {key}"))),
        },
        ConfigAction::Set { key, value } => {
            if agent.dry_run {
                let plan = DryRunPlan {
                    command: "config set".to_string(),
                    target: key.clone(),
                    mutates_system: true,
                    requires_root: false,
                    requires_yes: false,
                    planned_actions: vec![format!(
                        "Atomically set {key} in {}",
                        config_path.display()
                    )],
                    affected_paths: vec![config_path.display().to_string()],
                };
                return emit_dry_run_plan(agent, &plan);
            }
            store.set(key, value)?;
            if agent.json {
                emit_envelope(
                    "config set",
                    false,
                    &serde_json::json!({
                        "key": key,
                        "value": value,
                        "config_path": config_path,
                        "changed": true,
                    }),
                )
            } else {
                Ok(0)
            }
        }
        ConfigAction::List => {
            let map = store.list()?;
            if agent.json {
                emit_legacy_or_envelope(agent, "config list", &map)
            } else {
                for (k, v) in map {
                    println!("{k}={v}");
                }
                Ok(0)
            }
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

pub fn handle_template(
    action: &TemplateAction,
    template_dir: Option<&Path>,
    agent: &AgentCtx,
) -> Result<i32, CliError> {
    let engine = TemplateEngine::new(template_dir.map(Path::to_path_buf));
    match action {
        TemplateAction::List => {
            let list = engine.list();
            if agent.json {
                emit_legacy_or_envelope(agent, "template list", &list)
            } else {
                for item in list {
                    println!("{item}");
                }
                Ok(0)
            }
        }
        TemplateAction::Render {
            rel_path,
            out,
            vars,
        } => {
            let kv = parse_var_pairs(vars)?;
            let rendered = engine.render_to_string(rel_path, &kv)?;
            let target_file = out.as_deref().filter(|p| *p != Path::new("-"));

            if agent.dry_run {
                let (mutates_system, affected_paths) = match target_file {
                    Some(target) => (true, vec![target.display().to_string()]),
                    None => (false, Vec::new()),
                };
                let plan = DryRunPlan {
                    command: "template render".to_string(),
                    target: rel_path.clone(),
                    mutates_system,
                    requires_root: false,
                    requires_yes: false,
                    planned_actions: vec![match target_file {
                        Some(t) => format!(
                            "Render {rel_path} ({} bytes) to {}",
                            rendered.len(),
                            t.display()
                        ),
                        None => format!("Render {rel_path} ({} bytes) to stdout", rendered.len()),
                    }],
                    affected_paths,
                };
                return emit_dry_run_plan(agent, &plan);
            }

            match target_file {
                None => {
                    if agent.json {
                        return emit_envelope(
                            "template render",
                            false,
                            &serde_json::json!({
                                "template": rel_path,
                                "out": "-",
                                "bytes": rendered.len(),
                                "content": rendered,
                            }),
                        );
                    }
                    let mut stdout = io::stdout().lock();
                    stdout.write_all(rendered.as_bytes())?;
                    stdout.flush()?;
                }
                Some(target) => {
                    engine.render_to_file(rel_path, target, &kv, None)?;
                    if agent.json {
                        return emit_envelope(
                            "template render",
                            false,
                            &serde_json::json!({
                                "template": rel_path,
                                "out": target,
                                "bytes": rendered.len(),
                            }),
                        );
                    }
                }
            }
            Ok(0)
        }
    }
}

pub fn handle_bridge(
    args: &BridgeArgs,
    script_dir: Option<&Path>,
    config_path: &Path,
    agent: &AgentCtx,
) -> Result<i32, CliError> {
    if args.list {
        let modules = module_registry();
        let actions: Vec<String> = BashAction::all_actions()
            .into_iter()
            .map(BashAction::cli_name)
            .collect();
        if agent.json {
            let payload = BridgeListOutput { modules, actions };
            return emit_legacy_or_envelope(agent, "bridge", &payload);
        }
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
