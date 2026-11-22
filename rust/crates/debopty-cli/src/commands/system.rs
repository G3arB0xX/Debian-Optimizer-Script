use std::path::{Path, PathBuf};

use clap::Subcommand;
use debopty_core::network::UreqHttpClient;
use debopty_core::sys::RealCommandRunner;
use debopty_core::system::{
    SystemContext, run_base_optimization, setup_limits, setup_logrotate,
    setup_low_memory_optimization, setup_sysctl, setup_timezone, setup_tmux, toggle_ip_forwarding,
    uninstall_tmux,
};
use debopty_tui::is_non_interactive;

use crate::CliError;
use crate::agent::{AgentCtx, DryRunPlan, emit_dry_run_plan, emit_envelope};

#[derive(Debug, Subcommand)]
pub enum SystemAction {
    /// 部署版本自适应的全局 Tmux 配置 (带自动备份与幂等性)
    TmuxSetup {
        /// Tmux 配置文件路径覆盖 (默认 /etc/tmux.conf 或 DEBOPTY_TMUX_CONF)
        #[arg(long, value_name = "PATH")]
        conf: Option<PathBuf>,
        /// 跳过 apt 安装检查
        #[arg(long)]
        skip_apt: bool,
        /// 覆盖检测到的 tmux 版本字符串 (例如 "tmux 3.5a")
        #[arg(long, value_name = "VER")]
        version_override: Option<String>,
    },
    /// 卸载或还原由 debopty 管理的 Tmux 配置
    TmuxUninstall {
        /// Tmux 配置文件路径覆盖
        #[arg(long, value_name = "PATH")]
        conf: Option<PathBuf>,
        /// 跳过 apt 卸载步骤
        #[arg(long)]
        skip_apt: bool,
    },
    /// 切换 IPv4/IPv6 内核路由转发开关
    IpForwardToggle {
        /// 根目录前缀 (用于无特权隔离测试)
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 仅写入配置文件，跳过 sysctl/systemctl 等系统调用
        #[arg(long)]
        no_sys: bool,
    },
    /// 渲染并应用 BBR/TCP 内核参数调优
    Sysctl {
        /// 根目录前缀
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 跳过 sysctl --system 调用
        #[arg(long)]
        no_sys: bool,
    },
    /// 配置文件句柄上限 (limits.d 与 systemd system/user.conf)
    Limits {
        /// 根目录前缀
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
    },
    /// 配置 /etc/logrotate.conf 日志轮转策略
    Logrotate {
        /// 根目录前缀
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
    },
    /// 设置时区为 Asia/Shanghai 并启用 chrony
    Timezone {
        /// 跳过 timedatectl/apt/systemctl 系统调用
        #[arg(long)]
        no_sys: bool,
    },
    /// 低内存 VPS 优化 (TTY/rsyslog/journald/冗余服务屏蔽)
    LowMem {
        /// 根目录前缀
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 跳过 dpkg/apt/systemctl 系统调用
        #[arg(long)]
        no_sys: bool,
        /// 同时停用并屏蔽冗余后台服务
        #[arg(long)]
        mask_services: bool,
    },
    /// 执行完整一键系统级基础优化编排
    BaseOptimize {
        /// 根目录前缀
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 仅执行文件系统操作，跳过特权系统命令
        #[arg(long)]
        no_sys: bool,
    },
}

fn build_system_context(
    root: Option<&Path>,
    no_sys: bool,
    explicit_config_path: Option<&Path>,
    template_dir: Option<&Path>,
) -> SystemContext {
    let mut ctx = match root {
        Some(r) => SystemContext::with_root(r),
        None => SystemContext::default(),
    };
    ctx.no_sys = no_sys;
    ctx.non_interactive = is_non_interactive();
    ctx.template_dir = template_dir.map(Path::to_path_buf);
    if let Some(cfg) = explicit_config_path {
        ctx.config_path = cfg.to_path_buf();
    }
    ctx
}

fn root_join(root: Option<&Path>, rel: &str) -> String {
    root.unwrap_or_else(|| Path::new("/"))
        .join(rel.trim_start_matches('/'))
        .display()
        .to_string()
}

pub fn handle_system(
    action: &SystemAction,
    explicit_config_path: Option<&Path>,
    template_dir: Option<&Path>,
    agent: &AgentCtx,
) -> Result<i32, CliError> {
    let runner = RealCommandRunner;

    match action {
        SystemAction::TmuxSetup {
            conf,
            skip_apt,
            version_override,
        } => {
            let target_conf = conf
                .clone()
                .unwrap_or_else(|| PathBuf::from("/etc/tmux.conf"));
            if agent.dry_run {
                let plan = DryRunPlan {
                    command: "system tmux-setup".to_string(),
                    target: target_conf.display().to_string(),
                    mutates_system: true,
                    requires_root: true,
                    requires_yes: false,
                    planned_actions: vec![
                        "Backup existing unmanaged tmux.conf if present".to_string(),
                        format!(
                            "Render version-adaptive tmux config to {}",
                            target_conf.display()
                        ),
                    ],
                    affected_paths: vec![target_conf.display().to_string()],
                };
                return emit_dry_run_plan(agent, &plan);
            }
            let mut ctx = build_system_context(None, false, explicit_config_path, template_dir);
            ctx.tmux_conf_override = conf.clone();
            ctx.tmux_skip_apt = *skip_apt;
            ctx.tmux_version_override = version_override.clone();
            let path = setup_tmux(&ctx, &runner)?;
            if agent.json {
                return emit_envelope(
                    "system tmux-setup",
                    false,
                    &serde_json::json!({ "conf": path, "changed": true }),
                );
            }
            println!("Tmux 配置已写入: {}", path.display());
            Ok(0)
        }
        SystemAction::TmuxUninstall { conf, skip_apt } => {
            let target_conf = conf
                .clone()
                .unwrap_or_else(|| PathBuf::from("/etc/tmux.conf"));
            if agent.dry_run {
                let plan = DryRunPlan {
                    command: "system tmux-uninstall".to_string(),
                    target: target_conf.display().to_string(),
                    mutates_system: true,
                    requires_root: true,
                    requires_yes: false,
                    planned_actions: vec![format!(
                        "Restore backup or remove debopty-managed tmux config at {}",
                        target_conf.display()
                    )],
                    affected_paths: vec![target_conf.display().to_string()],
                };
                return emit_dry_run_plan(agent, &plan);
            }
            let mut ctx = build_system_context(None, false, explicit_config_path, template_dir);
            ctx.tmux_conf_override = conf.clone();
            ctx.tmux_skip_apt = *skip_apt;
            uninstall_tmux(&ctx, &runner)?;
            if agent.json {
                return emit_envelope(
                    "system tmux-uninstall",
                    false,
                    &serde_json::json!({ "conf": target_conf, "restored": true }),
                );
            }
            println!("Tmux 卸载/还原完成。");
            Ok(0)
        }
        SystemAction::IpForwardToggle { root, no_sys } => {
            if agent.dry_run {
                let path = root_join(root.as_deref(), "etc/sysctl.d/99-debopti-forwarding.conf");
                let plan = DryRunPlan {
                    command: "system ip-forward-toggle".to_string(),
                    target: "ip_forward".to_string(),
                    mutates_system: true,
                    requires_root: true,
                    requires_yes: false,
                    planned_actions: vec![
                        "Toggle IPv4/IPv6 kernel forwarding sysctl drop-in".to_string(),
                    ],
                    affected_paths: vec![path],
                };
                return emit_dry_run_plan(agent, &plan);
            }
            let ctx =
                build_system_context(root.as_deref(), *no_sys, explicit_config_path, template_dir);
            let enabled = toggle_ip_forwarding(&ctx, &runner)?;
            if agent.json {
                return emit_envelope(
                    "system ip-forward-toggle",
                    false,
                    &serde_json::json!({ "enabled": enabled }),
                );
            }
            if enabled {
                println!("IP 转发状态: 已开启");
            } else {
                println!("IP 转发状态: 已关闭");
            }
            Ok(0)
        }
        SystemAction::Sysctl { root, no_sys } => {
            if agent.dry_run {
                let path = root_join(root.as_deref(), "etc/sysctl.d/99-debopti-optimize.conf");
                let plan = DryRunPlan {
                    command: "system sysctl".to_string(),
                    target: "sysctl".to_string(),
                    mutates_system: true,
                    requires_root: true,
                    requires_yes: false,
                    planned_actions: vec![
                        "Render BBR/TCP sysctl optimization template".to_string(),
                        "Apply sysctl --system when --no-sys is not set".to_string(),
                    ],
                    affected_paths: vec![path],
                };
                return emit_dry_run_plan(agent, &plan);
            }
            let ctx =
                build_system_context(root.as_deref(), *no_sys, explicit_config_path, template_dir);
            setup_sysctl(&ctx, &runner)?;
            if agent.json {
                return emit_envelope(
                    "system sysctl",
                    false,
                    &serde_json::json!({ "applied": true }),
                );
            }
            println!("Sysctl 参数优化已应用。");
            Ok(0)
        }
        SystemAction::Limits { root } => {
            if agent.dry_run {
                let plan = DryRunPlan {
                    command: "system limits".to_string(),
                    target: "nofile".to_string(),
                    mutates_system: true,
                    requires_root: true,
                    requires_yes: false,
                    planned_actions: vec![
                        "Write limits.d nofile drop-in and systemd DefaultLimitNOFILE".to_string(),
                    ],
                    affected_paths: vec![
                        root_join(
                            root.as_deref(),
                            "etc/security/limits.d/99-debopti-nofile.conf",
                        ),
                        root_join(root.as_deref(), "etc/systemd/system.conf"),
                        root_join(root.as_deref(), "etc/systemd/user.conf"),
                    ],
                };
                return emit_dry_run_plan(agent, &plan);
            }
            let ctx =
                build_system_context(root.as_deref(), true, explicit_config_path, template_dir);
            setup_limits(&ctx)?;
            if agent.json {
                return emit_envelope(
                    "system limits",
                    false,
                    &serde_json::json!({ "applied": true }),
                );
            }
            println!("文件句柄上限配置已应用。");
            Ok(0)
        }
        SystemAction::Logrotate { root } => {
            if agent.dry_run {
                let plan = DryRunPlan {
                    command: "system logrotate".to_string(),
                    target: "logrotate".to_string(),
                    mutates_system: true,
                    requires_root: true,
                    requires_yes: false,
                    planned_actions: vec![
                        "Update /etc/logrotate.conf to daily/rotate 7".to_string(),
                    ],
                    affected_paths: vec![root_join(root.as_deref(), "etc/logrotate.conf")],
                };
                return emit_dry_run_plan(agent, &plan);
            }
            let ctx =
                build_system_context(root.as_deref(), true, explicit_config_path, template_dir);
            setup_logrotate(&ctx)?;
            if agent.json {
                return emit_envelope(
                    "system logrotate",
                    false,
                    &serde_json::json!({ "applied": true }),
                );
            }
            println!("Logrotate 策略已更新。");
            Ok(0)
        }
        SystemAction::Timezone { no_sys } => {
            if agent.dry_run {
                let plan = DryRunPlan {
                    command: "system timezone".to_string(),
                    target: "Asia/Shanghai".to_string(),
                    mutates_system: true,
                    requires_root: true,
                    requires_yes: false,
                    planned_actions: vec![
                        "Set system timezone to Asia/Shanghai".to_string(),
                        "Ensure chrony time sync service is enabled".to_string(),
                    ],
                    affected_paths: vec!["/etc/timezone".to_string(), "/etc/localtime".to_string()],
                };
                return emit_dry_run_plan(agent, &plan);
            }
            let ctx = build_system_context(None, *no_sys, explicit_config_path, template_dir);
            setup_timezone(&ctx, &runner)?;
            if agent.json {
                return emit_envelope(
                    "system timezone",
                    false,
                    &serde_json::json!({ "timezone": "Asia/Shanghai", "applied": true }),
                );
            }
            println!("时区与时间同步配置已完成。");
            Ok(0)
        }
        SystemAction::LowMem {
            root,
            no_sys,
            mask_services,
        } => {
            if agent.dry_run {
                let plan = DryRunPlan {
                    command: "system low-mem".to_string(),
                    target: "low-mem".to_string(),
                    mutates_system: true,
                    requires_root: true,
                    requires_yes: false,
                    planned_actions: vec![
                        "Configure logind NAutoVTs and journald SystemMaxUse=200M".to_string(),
                        format!("mask_services={mask_services}"),
                    ],
                    affected_paths: vec![
                        root_join(root.as_deref(), "etc/systemd/logind.conf"),
                        root_join(root.as_deref(), "etc/systemd/journald.conf"),
                    ],
                };
                return emit_dry_run_plan(agent, &plan);
            }
            let ctx =
                build_system_context(root.as_deref(), *no_sys, explicit_config_path, template_dir);
            let masked = setup_low_memory_optimization(&ctx, *mask_services, &runner)?;
            if agent.json {
                return emit_envelope(
                    "system low-mem",
                    false,
                    &serde_json::json!({ "applied": true, "masked_services": masked }),
                );
            }
            if masked.is_empty() {
                println!("低内存优化已应用。");
            } else {
                println!("低内存优化已应用，已屏蔽服务: {}", masked.join(", "));
            }
            Ok(0)
        }
        SystemAction::BaseOptimize { root, no_sys } => {
            if agent.dry_run {
                let plan = DryRunPlan {
                    command: "system base-optimize".to_string(),
                    target: root
                        .as_deref()
                        .unwrap_or_else(|| Path::new("/"))
                        .display()
                        .to_string(),
                    mutates_system: true,
                    requires_root: true,
                    requires_yes: false,
                    planned_actions: vec![
                        "Run sysctl, limits, logrotate, timezone, low-mem, and security setup"
                            .to_string(),
                    ],
                    affected_paths: vec![
                        root_join(root.as_deref(), "etc/sysctl.d/99-debopti-optimize.conf"),
                        root_join(
                            root.as_deref(),
                            "etc/security/limits.d/99-debopti-nofile.conf",
                        ),
                        root_join(root.as_deref(), "etc/logrotate.conf"),
                        root_join(root.as_deref(), "etc/debopti/debopti.conf"),
                    ],
                };
                return emit_dry_run_plan(agent, &plan);
            }
            let ctx =
                build_system_context(root.as_deref(), *no_sys, explicit_config_path, template_dir);
            let http = UreqHttpClient::new();
            let report = run_base_optimization(&ctx, &http, &runner)?;
            if agent.json {
                return emit_envelope(
                    "system base-optimize",
                    false,
                    &serde_json::json!({
                        "is_cn_region": report.is_cn_region,
                        "base_optimized": report.base_optimized,
                    }),
                );
            }
            println!(
                "基础优化已完成 (CN={}, BASE_OPTIMIZED={})",
                report.is_cn_region, report.base_optimized
            );
            Ok(0)
        }
    }
}
