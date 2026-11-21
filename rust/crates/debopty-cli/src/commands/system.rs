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

pub fn handle_system(
    action: &SystemAction,
    explicit_config_path: Option<&Path>,
    template_dir: Option<&Path>,
) -> Result<i32, CliError> {
    let runner = RealCommandRunner;

    match action {
        SystemAction::TmuxSetup {
            conf,
            skip_apt,
            version_override,
        } => {
            let mut ctx = build_system_context(None, false, explicit_config_path, template_dir);
            ctx.tmux_conf_override = conf.clone();
            ctx.tmux_skip_apt = *skip_apt;
            ctx.tmux_version_override = version_override.clone();
            let path = setup_tmux(&ctx, &runner)?;
            println!("Tmux 配置已写入: {}", path.display());
            Ok(0)
        }
        SystemAction::TmuxUninstall { conf, skip_apt } => {
            let mut ctx = build_system_context(None, false, explicit_config_path, template_dir);
            ctx.tmux_conf_override = conf.clone();
            ctx.tmux_skip_apt = *skip_apt;
            uninstall_tmux(&ctx, &runner)?;
            println!("Tmux 卸载/还原完成。");
            Ok(0)
        }
        SystemAction::IpForwardToggle { root, no_sys } => {
            let ctx =
                build_system_context(root.as_deref(), *no_sys, explicit_config_path, template_dir);
            let enabled = toggle_ip_forwarding(&ctx, &runner)?;
            if enabled {
                println!("IP 转发状态: 已开启");
            } else {
                println!("IP 转发状态: 已关闭");
            }
            Ok(0)
        }
        SystemAction::Sysctl { root, no_sys } => {
            let ctx =
                build_system_context(root.as_deref(), *no_sys, explicit_config_path, template_dir);
            setup_sysctl(&ctx, &runner)?;
            println!("Sysctl 参数优化已应用。");
            Ok(0)
        }
        SystemAction::Limits { root } => {
            let ctx =
                build_system_context(root.as_deref(), true, explicit_config_path, template_dir);
            setup_limits(&ctx)?;
            println!("文件句柄上限配置已应用。");
            Ok(0)
        }
        SystemAction::Logrotate { root } => {
            let ctx =
                build_system_context(root.as_deref(), true, explicit_config_path, template_dir);
            setup_logrotate(&ctx)?;
            println!("Logrotate 策略已更新。");
            Ok(0)
        }
        SystemAction::Timezone { no_sys } => {
            let ctx = build_system_context(None, *no_sys, explicit_config_path, template_dir);
            setup_timezone(&ctx, &runner)?;
            println!("时区与时间同步配置已完成。");
            Ok(0)
        }
        SystemAction::LowMem {
            root,
            no_sys,
            mask_services,
        } => {
            let ctx =
                build_system_context(root.as_deref(), *no_sys, explicit_config_path, template_dir);
            let masked = setup_low_memory_optimization(&ctx, *mask_services, &runner)?;
            if masked.is_empty() {
                println!("低内存优化已应用。");
            } else {
                println!("低内存优化已应用，已屏蔽服务: {}", masked.join(", "));
            }
            Ok(0)
        }
        SystemAction::BaseOptimize { root, no_sys } => {
            let ctx =
                build_system_context(root.as_deref(), *no_sys, explicit_config_path, template_dir);
            let http = UreqHttpClient::new();
            let report = run_base_optimization(&ctx, &http, &runner)?;
            println!(
                "基础优化已完成 (CN={}, BASE_OPTIMIZED={})",
                report.is_cn_region, report.base_optimized
            );
            Ok(0)
        }
    }
}
