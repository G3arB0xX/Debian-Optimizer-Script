use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Args, Subcommand};
use debopty_core::apps::devops::{
    cli_deps,
    fish::{install_fish, uninstall_fish},
    lego::{
        LegoDomainSpec, LegoFlag, add_lego_domain, install_lego, list_lego_domains,
        remove_lego_domain, run_lego_once, toggle_lego_flag, uninstall_lego,
    },
    micro::{install_micro, uninstall_micro},
    yazi::{YaziInstallOptions, install_yazi, uninstall_yazi},
};
use debopty_core::apps::easytier::{install_easytier, uninstall_easytier};
use debopty_core::apps::ferron::{install_ferron, uninstall_ferron};
use debopty_core::apps::freship::{
    format_freship_journal_logs, install_freship, probe_freship_status, sync_freship_data,
    toggle_freship_timers, uninstall_freship,
};
use debopty_core::apps::golang::{
    install_caddy, install_derper, install_go, uninstall_caddy, uninstall_derper, uninstall_go,
};
use debopty_core::apps::podman::{
    PodmanUninstallOptions, install_podman, toggle_podman_gc, uninstall_podman,
};
use debopty_core::apps::realm::{install_realm, uninstall_realm};
use debopty_core::apps::rust::{install_rust, uninstall_rust};
use debopty_core::apps::self_manage::{
    SelfInstallOptions, SelfUninstallOptions, SelfUpdateOptions, ensure_default_tools,
    self_install_with_http, self_uninstall, self_update,
};
use debopty_core::apps::shared::{AppContext, AppReport, sync_devops_sot_links};
use debopty_core::apps::tailscale::{install_tailscale, uninstall_tailscale};
use debopty_core::apps::warp::{
    generate_warp_xray, install_usque, install_warp, uninstall_usque, uninstall_warp,
};
use debopty_core::apps::xray::{
    install_xray, probe_xray_status, toggle_xray_cron, toggle_xray_ruleset, uninstall_xray,
};
use debopty_core::config::ConfigStore;
use debopty_core::network::UreqHttpClient;
use debopty_core::status::{StatusSnapshot, probe_status};
use debopty_core::sys::{CommandRunner, RealCommandRunner};
use debopty_core::system::{SystemContext, setup_tmux, uninstall_tmux};
use debopty_core::version;
use debopty_tui::MenuId;

use crate::CliError;
use crate::agent::{AgentCtx, DryRunPlan, emit_dry_run_plan, emit_legacy_or_envelope};

#[derive(Debug, Subcommand)]
pub enum AppAction {
    /// 安装或升级指定应用模块
    Install {
        /// 应用标识 (fish|micro|yazi|tmux|lego|podman|rust|realm|ferron|go|caddy|derper|xray|warp|usque|easytier|tailscale|freship)
        name: String,
        /// 根目录覆盖 (默认 /)
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 跳过宿主机 systemctl / apt 调用 (离线或测试环境)
        #[arg(long)]
        no_sys: bool,
        /// 强制启用中国大陆镜像加速模式
        #[arg(long)]
        cn: bool,
        /// Yazi 媒体预览开关 (仅 yazi 生效)
        #[arg(long)]
        media_preview: Option<bool>,
    },
    /// 卸载指定应用模块
    Uninstall {
        /// 应用标识
        name: String,
        /// 根目录覆盖 (默认 /)
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 跳过宿主机 systemctl / apt 调用
        #[arg(long)]
        no_sys: bool,
        /// 强制启用中国大陆镜像模式
        #[arg(long)]
        cn: bool,
        /// 仅 Podman: 一并删除 /home/apps/srv 数据目录 (默认保留)
        #[arg(long)]
        delete_data: bool,
        /// 仅 Podman: 一并删除 apps 系统用户 (默认保留)
        #[arg(long)]
        delete_user: bool,
    },
    /// 切换 Podman 定时镜像清理定时器
    PodmanGcToggle {
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        #[arg(long)]
        no_sys: bool,
    },
    /// 切换 Xray 路由规则集 (official <-> loyalsoldier)
    XrayRulesetToggle {
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        #[arg(long)]
        no_sys: bool,
        #[arg(long)]
        cn: bool,
    },
    /// 切换 Xray GeoIP/GeoSite 每周一 03:30 自动更新 Cron
    XrayCronToggle {
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        #[arg(long)]
        no_sys: bool,
    },
    /// 查询 Xray Core 安装、规则集与自动更新状态
    XrayStatus {
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 跳过宿主机 systemctl / crontab 查询
        #[arg(long)]
        no_sys: bool,
    },
    /// 生成 Cloudflare WARP Xray WireGuard Outbound 配置
    WarpXrayGen {
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        #[arg(long)]
        no_sys: bool,
        #[arg(long)]
        cn: bool,
    },
    /// 查询 FreshIP 安装、定时器与探测状态
    FreshipStatus {
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 跳过宿主机 systemctl 定时器查询
        #[arg(long)]
        no_sys: bool,
    },
    /// 启用或暂停 FreshIP 定时任务
    FreshipToggle {
        /// 明确指定开启或关闭 (省略则自动翻转)
        #[arg(long)]
        enable: Option<bool>,
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        #[arg(long)]
        no_sys: bool,
    },
    /// 触发 FreshIP 热点数据同步
    FreshipSync {
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        #[arg(long)]
        no_sys: bool,
    },
    /// 查看 FreshIP 格式化运行日志
    FreshipLogs {
        /// 日志行数上限
        #[arg(long, default_value_t = 50)]
        lines: usize,
    },
    /// 列出已托管的 Lego SSL 域名与证书有效期
    LegoList {
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
    },
    /// 添加 Lego DNS-01 域名证书托管配置
    LegoAdd {
        /// 主域名 (如 example.com)
        domain: String,
        /// ACME 通知邮箱
        #[arg(long)]
        email: String,
        /// Cloudflare API Token (建议改用 --cf-token-file 或 DEBOPTY_CF_TOKEN 环境变量避免进程表暴露)
        #[arg(long)]
        cf_token: Option<String>,
        /// 从受保护文件读取 DNS API Token (优先于环境变量)
        #[arg(long, value_name = "PATH")]
        cf_token_file: Option<PathBuf>,
        /// 额外 SAN 域名 (可重复指定)
        #[arg(long = "san")]
        san_domains: Vec<String>,
        /// DNS 提供商标识 (默认 cloudflare)
        #[arg(long, default_value = "cloudflare")]
        provider: String,
        /// 添加后立即尝试签发证书
        #[arg(long)]
        issue: bool,
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        #[arg(long)]
        no_sys: bool,
    },
    /// 切换 Lego 域名开关 (auto-renew | dns-skip-propagation | ferron-push)
    LegoToggle {
        /// 主域名
        domain: String,
        /// 开关项 (auto-renew | dns-skip-propagation | ferron-push)
        flag: String,
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        #[arg(long)]
        no_sys: bool,
    },
    /// 立即签发或续期指定 Lego 域名证书
    LegoRenew {
        /// 主域名
        domain: String,
        /// 执行模式 (issue | renew | auto | run，默认 renew；run 等价于 auto)
        #[arg(long, default_value = "renew")]
        mode: String,
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        #[arg(long)]
        no_sys: bool,
    },
    /// 移除 Lego 域名托管配置 (默认保留已签发证书文件)
    LegoRemove {
        /// 主域名
        domain: String,
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
    },
    /// 同步 DevOps SOT (Fish / Micro / Yazi) 多用户混合软链接
    DevopsSotSync {
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        #[arg(long)]
        no_sys: bool,
    },
}

#[derive(Debug, Args)]
pub struct SelfInstallArgs {
    /// 根目录覆盖 (用于隔离测试或自定义 rootfs)
    #[arg(long, value_name = "PATH")]
    pub root: Option<PathBuf>,
    /// 源二进制路径覆盖 (默认当前可执行文件)
    #[arg(long, value_name = "PATH")]
    pub source_bin: Option<PathBuf>,
    /// 安装目录覆盖 (默认 /opt/debopty)
    #[arg(long, value_name = "PATH")]
    pub install_dir: Option<PathBuf>,
    /// 跳过系统探测命令
    #[arg(long)]
    pub no_sys: bool,
    /// 跳过自动初始化宿主机 Agent 工作环境 (也可设置 DEBOPTY_SKIP_AGENT_WORKSPACE=1)
    #[arg(long)]
    pub skip_agent_workspace: bool,
    /// 自定义宿主机 Agent 工作环境目录 (默认 /var/lib/debopty/agent)
    #[arg(long, value_name = "PATH")]
    pub agent_workspace_dir: Option<PathBuf>,
    /// 跳过默认安装 sendme 与 tmux 工具 (也可设置 DEBOPTY_SKIP_DEFAULT_TOOLS=1)
    #[arg(long)]
    pub skip_default_tools: bool,
    /// 跳过声明式日志与冷缓存留存配置 (也可设置 DEBOPTY_SKIP_RETENTION=1)
    #[arg(long)]
    pub skip_retention: bool,
    /// 仅执行默认配套工具 (sendme + tmux) 尽力安装（供 install.sh 内部调用）
    #[arg(long, hide = true)]
    pub ensure_default_tools_only: bool,
}

#[derive(Debug, Args)]
pub struct SelfUpdateArgs {
    /// 根目录覆盖
    #[arg(long, value_name = "PATH")]
    pub root: Option<PathBuf>,
    /// GitHub 仓库覆盖 (owner/repo)
    #[arg(long)]
    pub repo: Option<String>,
    /// 发布包基地址覆盖 (支持离线/本地镜像测试)
    #[arg(long)]
    pub release_base_url: Option<String>,
    /// 指定目标 Release 标签 (如 v2.0.0)
    #[arg(long)]
    pub tag: Option<String>,
    /// 即使版本一致也强制重新下载安装
    #[arg(long)]
    pub force: bool,
    /// 强制启用中国大陆镜像模式
    #[arg(long)]
    pub cn: bool,
    /// 跳过宿主机系统命令
    #[arg(long)]
    pub no_sys: bool,
}

#[derive(Debug, Args)]
pub struct SelfUninstallArgs {
    /// 根目录覆盖
    #[arg(long, value_name = "PATH")]
    pub root: Option<PathBuf>,
    /// 连同 /etc/debopti 持久化配置目录一并删除 (默认保留)
    #[arg(long)]
    pub purge_config: bool,
    /// 跳过宿主机系统命令
    #[arg(long)]
    pub no_sys: bool,
}

#[derive(Debug, Args)]
pub struct UiPreviewArgs {
    /// 预览页面: main | inspector | palette | help | syssec | netproxy | reconcile | devops | podman | rust | go | warp | maintenance | confirm | task-run | all
    #[arg(long, default_value = "all")]
    pub screen: String,
    /// 渲染宽度 (列数)
    #[arg(long, default_value_t = 108)]
    pub width: u16,
    /// 渲染高度 (行数)
    #[arg(long, default_value_t = 30)]
    pub height: u16,
    /// 使用内置演示遥测数据 (不探测本机状态)
    #[arg(long)]
    pub demo: bool,
    /// 输出 24-bit TrueColor ANSI SGR 转义序列 (默认纯文本)
    #[arg(long)]
    pub ansi: bool,
    /// 动画帧计数 (用于遥测曲线相位预览)
    #[arg(long, default_value_t = 0)]
    pub tick: u64,
    /// 切屏非线性过渡剩余帧数 (0-8)
    #[arg(long, default_value_t = 0)]
    pub transition: u8,
    /// 切屏非线性过渡时间进度 (毫秒，0-220)
    #[arg(long, value_name = "MS")]
    pub transition_ms: Option<u64>,
    /// 指定默认高亮菜单项索引
    #[arg(long, default_value_t = 0, value_name = "IDX")]
    pub selected: usize,
    /// 预填内联 `/` 模糊筛选关键词
    #[arg(long, value_name = "QUERY")]
    pub filter: Option<String>,
    /// 指定当前焦点面板: menu | telemetry
    #[arg(long, default_value = "menu", value_name = "PANE")]
    pub pane: String,
    /// 显示浮动自动消失 Toast 通知横幅
    #[arg(long, value_name = "MSG")]
    pub toast: Option<String>,
    /// 弹窗改用纯色扁平无边框样式 (U26)
    #[arg(long)]
    pub flat_modals: bool,
}

fn build_app_context(
    root: Option<&Path>,
    explicit_config: Option<&Path>,
    template_dir: Option<&Path>,
    no_sys: bool,
    cn: bool,
    non_interactive: bool,
) -> AppContext {
    let mut ctx = match root {
        Some(r) => AppContext::with_root(r),
        None => AppContext::default(),
    };
    if let Some(cfg) = explicit_config {
        ctx.config_path = cfg.to_path_buf();
    }
    ctx.template_dir = template_dir.map(Path::to_path_buf);
    ctx.no_sys = no_sys;
    ctx.is_cn_region = cn;
    ctx.non_interactive = non_interactive;
    ctx
}

fn write_stdout_or_break(text: &str) -> Result<bool, CliError> {
    let mut stdout = io::stdout().lock();
    match stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(err) => Err(CliError::Io(err)),
    }
}

fn emit_report(report: &AppReport, agent: &AgentCtx, command: &str) -> Result<i32, CliError> {
    if agent.json {
        return emit_legacy_or_envelope(agent, command, report);
    }
    let mut out = String::new();
    out.push_str(&format!("[{}] {}\n", report.app, report.summary));
    for (k, v) in &report.details {
        out.push_str(&format!("  {k}: {v}\n"));
    }
    for w in &report.warnings {
        out.push_str(&format!("  warning: {w}\n"));
    }
    for p in &report.preserved_user_custom {
        out.push_str(&format!("  preserved: {p}\n"));
    }
    let _ = write_stdout_or_break(&out)?;
    Ok(0)
}

fn resolve_lego_cf_token(
    cli_token: Option<&str>,
    token_file: Option<&Path>,
) -> Result<String, CliError> {
    if let Some(tok) = cli_token.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(tok.to_string());
    }
    if let Some(path) = token_file {
        let raw = fs::read_to_string(path).map_err(|e| {
            CliError::Usage(format!(
                "无法读取 --cf-token-file '{}': {e}",
                path.display()
            ))
        })?;
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
        return Err(CliError::Usage(format!(
            "--cf-token-file '{}' 内容为空",
            path.display()
        )));
    }
    for env_key in ["DEBOPTY_CF_TOKEN", "CF_DNS_API_TOKEN"] {
        if let Ok(val) = std::env::var(env_key) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }
    Err(CliError::Usage(
        "缺少 DNS API Token：请提供 --cf-token-file <PATH>、环境变量 DEBOPTY_CF_TOKEN / CF_DNS_API_TOKEN 或 --cf-token <TOKEN>"
            .to_string(),
    ))
}

fn normalize_lego_renew_mode(raw: &str) -> Result<&'static str, CliError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "issue" => Ok("issue"),
        "renew" => Ok("renew"),
        "auto" | "run" => Ok("auto"),
        other => Err(CliError::Usage(format!(
            "未知 Lego 执行模式 '{other}'，合法值: issue | renew | auto | run"
        ))),
    }
}

fn parse_lego_flag(raw: &str) -> Result<LegoFlag, CliError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "auto-renew" | "auto_renew" => Ok(LegoFlag::AutoRenew),
        "dns-skip-propagation" | "dns_skip_propagation" => Ok(LegoFlag::DnsSkipPropagation),
        "ferron-push" | "ferron_push" => Ok(LegoFlag::FerronPush),
        other => Err(CliError::Usage(format!(
            "未知 Lego 开关 '{other}'，合法值: auto-renew | dns-skip-propagation | ferron-push"
        ))),
    }
}

fn is_known_app_slug(slug: &str, allow_cli_deps: bool) -> bool {
    matches!(
        slug,
        "fish"
            | "micro"
            | "yazi"
            | "lego"
            | "tmux"
            | "podman"
            | "rust"
            | "rust-sdk"
            | "rustc"
            | "realm"
            | "ferron"
            | "go"
            | "go-sdk"
            | "golang"
            | "caddy"
            | "derper"
            | "derp"
            | "xray"
            | "warp"
            | "warp-cli"
            | "usque"
            | "easytier"
            | "easytier-core"
            | "tailscale"
            | "freship"
    ) || (allow_cli_deps && slug == "cli-deps")
}

pub fn handle_app(
    action: &AppAction,
    explicit_config: Option<&Path>,
    template_dir: Option<&Path>,
    cli_quiet: bool,
    agent: &AgentCtx,
) -> Result<i32, CliError> {
    let runner = RealCommandRunner;
    let http = UreqHttpClient::new();

    match action {
        AppAction::Install {
            name,
            root,
            no_sys,
            cn,
            media_preview,
        } => {
            let slug = name.trim().to_ascii_lowercase();
            if !is_known_app_slug(&slug, true) {
                return Err(CliError::Usage(format!(
                    "未知应用模块 '{slug}'；支持: fish|micro|yazi|tmux|lego|podman|rust|realm|ferron|go|caddy|derper|xray|warp|usque|easytier|tailscale|freship"
                )));
            }
            let ctx = build_app_context(
                root.as_deref(),
                explicit_config,
                template_dir,
                *no_sys,
                *cn,
                agent.non_interactive,
            );
            if agent.dry_run {
                let plan = DryRunPlan {
                    command: "app install".to_string(),
                    target: slug.clone(),
                    mutates_system: true,
                    requires_root: true,
                    requires_yes: false,
                    planned_actions: vec![format!(
                        "Install or upgrade managed application '{slug}' under {}",
                        ctx.root_prefix.display()
                    )],
                    affected_paths: vec![ctx.root_prefix.display().to_string()],
                };
                return emit_dry_run_plan(agent, &plan);
            }
            let report = match slug.as_str() {
                "fish" => install_fish(&ctx, &http, &runner)?,
                "micro" => install_micro(&ctx, &http, &runner)?,
                "yazi" => install_yazi(
                    &ctx,
                    &YaziInstallOptions {
                        media_preview: *media_preview,
                    },
                    &http,
                    &runner,
                )?,
                "lego" => install_lego(&ctx, &http, &runner)?,
                "cli-deps" => {
                    let warnings = cli_deps::install_devops_shared_cli_deps(&ctx, &http, &runner)?;
                    let mut rep = AppReport::new(
                        "devops",
                        "install-cli-deps",
                        true,
                        "DevOps 共享 CLI 依赖已同步",
                    );
                    rep.warnings = warnings;
                    rep
                }
                "tmux" => {
                    let sys_ctx = SystemContext {
                        root_prefix: ctx.root_prefix.clone(),
                        is_cn_region: ctx.effective_is_cn(),
                        non_interactive: ctx.non_interactive,
                        no_sys: ctx.no_sys,
                        config_path: ctx.config_path.clone(),
                        template_dir: ctx.template_dir.clone(),
                        tmux_conf_override: None,
                        tmux_skip_apt: ctx.no_sys,
                        tmux_version_override: None,
                    };
                    let path = setup_tmux(&sys_ctx, &runner)?;
                    AppReport::new(
                        "tmux",
                        "install",
                        true,
                        format!("Tmux 配置已部署: {}", path.display()),
                    )
                    .with_detail("conf", path.to_string_lossy().into_owned())
                }
                "podman" => install_podman(&ctx, &runner)?,
                "rust" | "rust-sdk" | "rustc" => install_rust(&ctx, &http, &runner)?,
                "realm" => install_realm(&ctx, &http, &runner)?,
                "ferron" => install_ferron(&ctx, &http, &runner)?,
                "go" | "go-sdk" | "golang" => install_go(&ctx, &http, &runner)?,
                "caddy" => install_caddy(&ctx, &http, &runner)?,
                "derper" | "derp" => install_derper(&ctx, &http, &runner)?,
                "xray" => install_xray(&ctx, &http, &runner)?,
                "warp" | "warp-cli" => install_warp(&ctx, &http, &runner)?,
                "usque" => install_usque(&ctx, &http, &runner)?,
                "easytier" | "easytier-core" => install_easytier(&ctx, &http, &runner)?,
                "tailscale" => install_tailscale(&ctx, &http, &runner)?,
                "freship" => install_freship(&ctx, None, &http, &runner)?,
                _ => unreachable!("validated above"),
            };
            emit_report(&report, agent, "app install")
        }
        AppAction::Uninstall {
            name,
            root,
            no_sys,
            cn,
            delete_data,
            delete_user,
        } => {
            let slug = name.trim().to_ascii_lowercase();
            if !is_known_app_slug(&slug, false) {
                return Err(CliError::Usage(format!(
                    "未知应用模块 '{slug}'；无法执行卸载"
                )));
            }
            let ctx = build_app_context(
                root.as_deref(),
                explicit_config,
                template_dir,
                *no_sys,
                *cn,
                true,
            );
            if agent.dry_run {
                let plan = DryRunPlan {
                    command: "app uninstall".to_string(),
                    target: slug.clone(),
                    mutates_system: true,
                    requires_root: true,
                    requires_yes: false,
                    planned_actions: vec![format!(
                        "Uninstall managed application '{slug}' under {}",
                        ctx.root_prefix.display()
                    )],
                    affected_paths: vec![ctx.root_prefix.display().to_string()],
                };
                return emit_dry_run_plan(agent, &plan);
            }
            let report = match slug.as_str() {
                "fish" => uninstall_fish(&ctx, &runner)?,
                "micro" => uninstall_micro(&ctx, &runner)?,
                "yazi" => uninstall_yazi(&ctx, &runner)?,
                "lego" => uninstall_lego(&ctx, &runner)?,
                "tmux" => {
                    let sys_ctx = SystemContext {
                        root_prefix: ctx.root_prefix.clone(),
                        is_cn_region: ctx.effective_is_cn(),
                        non_interactive: true,
                        no_sys: ctx.no_sys,
                        config_path: ctx.config_path.clone(),
                        template_dir: ctx.template_dir.clone(),
                        tmux_conf_override: None,
                        tmux_skip_apt: ctx.no_sys,
                        tmux_version_override: None,
                    };
                    uninstall_tmux(&sys_ctx, &runner)?;
                    AppReport::new("tmux", "uninstall", true, "Tmux 已卸载并还原原有配置")
                }
                "podman" => uninstall_podman(
                    &ctx,
                    &PodmanUninstallOptions {
                        delete_data: *delete_data,
                        delete_user: *delete_user,
                    },
                    &runner,
                )?,
                "rust" | "rust-sdk" | "rustc" => uninstall_rust(&ctx, &runner)?,
                "realm" => uninstall_realm(&ctx, &runner)?,
                "ferron" => uninstall_ferron(&ctx, &runner)?,
                "go" | "go-sdk" | "golang" => uninstall_go(&ctx, &runner)?,
                "caddy" => uninstall_caddy(&ctx, &runner)?,
                "derper" | "derp" => uninstall_derper(&ctx, &runner)?,
                "xray" => uninstall_xray(&ctx, &http, &runner)?,
                "warp" | "warp-cli" => uninstall_warp(&ctx, &runner)?,
                "usque" => uninstall_usque(&ctx, &runner)?,
                "easytier" | "easytier-core" => uninstall_easytier(&ctx, &http, &runner)?,
                "tailscale" => uninstall_tailscale(&ctx, &runner)?,
                "freship" => uninstall_freship(&ctx, &runner)?,
                _ => unreachable!("validated above"),
            };
            emit_report(&report, agent, "app uninstall")
        }
        AppAction::PodmanGcToggle { root, no_sys } => {
            let ctx = build_app_context(
                root.as_deref(),
                explicit_config,
                template_dir,
                *no_sys,
                false,
                true,
            );
            let report = toggle_podman_gc(&ctx, &runner)?;
            emit_report(&report, agent, "app podman-gc-toggle")
        }
        AppAction::XrayRulesetToggle { root, no_sys, cn } => {
            let ctx = build_app_context(
                root.as_deref(),
                explicit_config,
                template_dir,
                *no_sys,
                *cn,
                true,
            );
            let report = toggle_xray_ruleset(&ctx, &http, &runner)?;
            emit_report(&report, agent, "app xray-ruleset-toggle")
        }
        AppAction::XrayCronToggle { root, no_sys } => {
            let ctx = build_app_context(
                root.as_deref(),
                explicit_config,
                template_dir,
                *no_sys,
                false,
                true,
            );
            let report = toggle_xray_cron(&ctx, &runner)?;
            emit_report(&report, agent, "app xray-cron-toggle")
        }
        AppAction::XrayStatus { root, no_sys } => {
            let ctx = build_app_context(
                root.as_deref(),
                explicit_config,
                template_dir,
                *no_sys,
                false,
                true,
            );
            let st = probe_xray_status(&ctx, &runner);
            if agent.json {
                return emit_legacy_or_envelope(agent, "app xray-status", &st);
            }
            let line = format!(
                "Xray: installed={} active={} ruleset={} geo_auto_update={} cron={}\n",
                st.installed,
                st.active,
                st.ruleset.as_str(),
                st.geo_auto_update,
                st.cron_present
            );
            let _ = write_stdout_or_break(&line)?;
            Ok(0)
        }
        AppAction::WarpXrayGen { root, no_sys, cn } => {
            let ctx = build_app_context(
                root.as_deref(),
                explicit_config,
                template_dir,
                *no_sys,
                *cn,
                true,
            );
            let out = generate_warp_xray(&ctx, &http, &runner)?;
            if agent.json {
                return emit_legacy_or_envelope(agent, "app warp-xray-gen", &out);
            }
            let line = format!("{}\n", out.json);
            let _ = write_stdout_or_break(&line)?;
            Ok(0)
        }
        AppAction::FreshipStatus { root, no_sys } => {
            let ctx = build_app_context(
                root.as_deref(),
                explicit_config,
                template_dir,
                *no_sys,
                false,
                true,
            );
            let st = probe_freship_status(&ctx, &runner);
            if agent.json {
                return emit_legacy_or_envelope(agent, "app freship-status", &st);
            }
            let line = format!(
                "FreshIP: installed={} region={}/{} mode={} v4_timer={} v6_timer={}\n",
                st.installed,
                st.region_code,
                st.region_name,
                st.work_mode.map(|m| m.as_str()).unwrap_or("unknown"),
                st.v4_timer_active,
                st.v6_timer_active
            );
            let _ = write_stdout_or_break(&line)?;
            Ok(0)
        }
        AppAction::FreshipToggle {
            enable,
            root,
            no_sys,
        } => {
            let ctx = build_app_context(
                root.as_deref(),
                explicit_config,
                template_dir,
                *no_sys,
                false,
                true,
            );
            let report = toggle_freship_timers(&ctx, *enable, &runner)?;
            emit_report(&report, agent, "app freship-toggle")
        }
        AppAction::FreshipSync { root, no_sys } => {
            let ctx = build_app_context(
                root.as_deref(),
                explicit_config,
                template_dir,
                *no_sys,
                false,
                true,
            );
            let report = sync_freship_data(&ctx, &runner)?;
            emit_report(&report, agent, "app freship-sync")
        }
        AppAction::FreshipLogs { lines } => {
            let n_str = lines.to_string();
            let raw = runner
                .run(
                    "journalctl",
                    &[
                        "-u",
                        "freship-core@*",
                        "-u",
                        "freship-updater.service",
                        "-n",
                        &n_str,
                        "--no-pager",
                        "-o",
                        "short-iso",
                    ],
                    Duration::from_secs(10),
                )
                .map(|o| o.stdout)
                .unwrap_or_default();
            let formatted = format_freship_journal_logs(&raw);
            if agent.json {
                return emit_legacy_or_envelope(agent, "app freship-logs", &formatted);
            } else if formatted.is_empty() {
                println!("暂无 FreshIP 运行日志。");
            } else {
                for line in formatted {
                    println!("{line}");
                }
            }
            Ok(0)
        }
        AppAction::LegoList { root } => {
            let ctx = build_app_context(
                root.as_deref(),
                explicit_config,
                template_dir,
                true,
                false,
                true,
            );
            let list = list_lego_domains(&ctx, &runner)?;
            if agent.json {
                return emit_legacy_or_envelope(agent, "app lego-list", &list);
            } else if list.is_empty() {
                println!("尚未配置任何 Lego 域名。");
            } else {
                for dom in list {
                    let days = dom
                        .days_left
                        .map(|d| format!("{d}d"))
                        .unwrap_or_else(|| "none".to_string());
                    println!(
                        "{:<28} provider={} auto_renew={} ferron_push={} cert={} ({})",
                        dom.primary_domain,
                        dom.provider,
                        dom.auto_renew,
                        dom.ferron_push,
                        dom.cert_exists,
                        days
                    );
                }
            }
            Ok(0)
        }
        AppAction::LegoAdd {
            domain,
            email,
            cf_token,
            cf_token_file,
            san_domains,
            provider,
            issue,
            root,
            no_sys,
        } => {
            let resolved_token =
                resolve_lego_cf_token(cf_token.as_deref(), cf_token_file.as_deref())?;
            let ctx = build_app_context(
                root.as_deref(),
                explicit_config,
                template_dir,
                *no_sys,
                false,
                true,
            );
            let spec = LegoDomainSpec {
                primary_domain: domain.clone(),
                san_domains: san_domains.clone(),
                email: email.clone(),
                cf_token: resolved_token,
                provider: provider.clone(),
                issue_immediately: *issue,
            };
            let report = add_lego_domain(&ctx, &spec, &runner)?;
            emit_report(&report, agent, "app lego-add")
        }
        AppAction::LegoToggle {
            domain,
            flag,
            root,
            no_sys,
        } => {
            let parsed_flag = parse_lego_flag(flag)?;
            let ctx = build_app_context(
                root.as_deref(),
                explicit_config,
                template_dir,
                *no_sys,
                false,
                true,
            );
            let report = toggle_lego_flag(&ctx, domain, parsed_flag, &runner)?;
            emit_report(&report, agent, "app lego-toggle")
        }
        AppAction::LegoRenew {
            domain,
            mode,
            root,
            no_sys,
        } => {
            let normalized_mode = normalize_lego_renew_mode(mode)?;
            let ctx = build_app_context(
                root.as_deref(),
                explicit_config,
                template_dir,
                *no_sys,
                false,
                true,
            );
            let report = run_lego_once(&ctx, domain, normalized_mode, &runner)?;
            emit_report(&report, agent, "app lego-renew")
        }
        AppAction::LegoRemove { domain, root } => {
            let ctx = build_app_context(
                root.as_deref(),
                explicit_config,
                template_dir,
                true,
                false,
                true,
            );
            let report = remove_lego_domain(&ctx, domain)?;
            emit_report(&report, agent, "app lego-remove")
        }
        AppAction::DevopsSotSync { root, no_sys } => {
            let ctx = build_app_context(
                root.as_deref(),
                explicit_config,
                template_dir,
                *no_sys,
                false,
                true,
            );
            let synced = sync_devops_sot_links(&ctx, cli_quiet, &runner)?;
            let report = AppReport::new(
                "devops",
                "sot-sync",
                synced,
                if synced {
                    "DevOps SOT 软链接同步完成"
                } else {
                    "DevOps SOT 无需变更"
                },
            );
            emit_report(&report, agent, "app devops-sot-sync")
        }
    }
}

pub fn handle_self_install(
    args: &SelfInstallArgs,
    explicit_config: Option<&Path>,
    template_dir: Option<&Path>,
    agent: &AgentCtx,
) -> Result<i32, CliError> {
    let ctx = build_app_context(
        args.root.as_deref(),
        explicit_config,
        template_dir,
        args.no_sys,
        false,
        true,
    );
    if agent.dry_run {
        let install_dir = args
            .install_dir
            .clone()
            .unwrap_or_else(|| ctx.root_prefix.join("opt/debopty"));
        let plan = DryRunPlan {
            command: "self-install".to_string(),
            target: install_dir.display().to_string(),
            mutates_system: true,
            requires_root: true,
            requires_yes: false,
            planned_actions: vec![
                format!("Install debopty binary into {}", install_dir.display()),
                "Wire /usr/local/bin/debopty, debopty-f2b-sync, and /etc/profile.d/debopty.sh"
                    .to_string(),
            ],
            affected_paths: vec![
                install_dir.join("bin/debopty").display().to_string(),
                ctx.root_prefix
                    .join("usr/local/bin/debopty")
                    .display()
                    .to_string(),
                ctx.root_prefix
                    .join("etc/profile.d/debopty.sh")
                    .display()
                    .to_string(),
            ],
        };
        return emit_dry_run_plan(agent, &plan);
    }
    let http = UreqHttpClient::new();
    let runner = RealCommandRunner;
    if args.ensure_default_tools_only {
        let report = ensure_default_tools(&ctx, &http, &runner);
        return emit_report(&report, agent, "self-install");
    }
    let opts = SelfInstallOptions {
        source_bin: args.source_bin.clone(),
        install_dir: args.install_dir.clone(),
        skip_agent_workspace: args.skip_agent_workspace,
        agent_workspace_dir: args.agent_workspace_dir.clone(),
        skip_default_tools: args.skip_default_tools,
        skip_retention: args.skip_retention,
        ..SelfInstallOptions::default()
    };
    let report = self_install_with_http(&ctx, &opts, &http, &runner)?;
    emit_report(&report, agent, "self-install")
}

pub fn handle_self_update(
    args: &SelfUpdateArgs,
    explicit_config: Option<&Path>,
    template_dir: Option<&Path>,
    agent: &AgentCtx,
) -> Result<i32, CliError> {
    let ctx = build_app_context(
        args.root.as_deref(),
        explicit_config,
        template_dir,
        args.no_sys,
        args.cn,
        true,
    );
    if agent.dry_run {
        let plan = DryRunPlan {
            command: "self-update".to_string(),
            target: args.tag.clone().unwrap_or_else(|| "latest".to_string()),
            mutates_system: true,
            requires_root: true,
            requires_yes: false,
            planned_actions: vec![
                "Query GitHub Releases, download binary + SHA256SUMS, verify digest".to_string(),
                "Atomically replace /opt/debopty/bin/debopty".to_string(),
            ],
            affected_paths: vec![
                ctx.root_prefix
                    .join("opt/debopty/bin/debopty")
                    .display()
                    .to_string(),
            ],
        };
        return emit_dry_run_plan(agent, &plan);
    }
    let opts = SelfUpdateOptions {
        repo: args.repo.clone(),
        release_base_url: args.release_base_url.clone(),
        tag: args.tag.clone(),
        force: args.force,
        install_dir: None,
    };
    let http = UreqHttpClient::new();
    let runner = RealCommandRunner;
    let report = self_update(&ctx, &opts, &http, &runner)?;
    emit_report(&report, agent, "self-update")
}

pub fn handle_self_uninstall(
    args: &SelfUninstallArgs,
    explicit_config: Option<&Path>,
    template_dir: Option<&Path>,
    agent: &AgentCtx,
) -> Result<i32, CliError> {
    let ctx = build_app_context(
        args.root.as_deref(),
        explicit_config,
        template_dir,
        args.no_sys,
        false,
        true,
    );
    if agent.dry_run {
        let mut affected = vec![
            ctx.root_prefix.join("opt/debopty").display().to_string(),
            ctx.root_prefix
                .join("usr/local/bin/debopty")
                .display()
                .to_string(),
            ctx.root_prefix
                .join("etc/profile.d/debopty.sh")
                .display()
                .to_string(),
        ];
        if args.purge_config {
            affected.push(ctx.root_prefix.join("etc/debopti").display().to_string());
        }
        let plan = DryRunPlan {
            command: "self-uninstall".to_string(),
            target: ctx.root_prefix.display().to_string(),
            mutates_system: true,
            requires_root: true,
            requires_yes: args.purge_config,
            planned_actions: vec![
                "Remove /opt/debopty binary, global symlinks, and profile alias".to_string(),
                if args.purge_config {
                    "Purge /etc/debopti persistent configuration directory".to_string()
                } else {
                    "Preserve /etc/debopti persistent configuration directory".to_string()
                },
            ],
            affected_paths: affected,
        };
        return emit_dry_run_plan(agent, &plan);
    }
    if args.purge_config && agent.explicit_non_interactive && !agent.yes {
        return Err(CliError::ConfirmationRequired(
            "非交互模式执行 `debopty self-uninstall --purge-config` 将删除持久化配置目录，请追加 `--yes` (`-y`) 确认，或使用 `--dry-run` 预览。"
                .to_string(),
        ));
    }
    let opts = SelfUninstallOptions {
        keep_config: !args.purge_config,
        install_dir: None,
    };
    let runner = RealCommandRunner;
    let report = self_uninstall(&ctx, &opts, &runner)?;
    emit_report(&report, agent, "self-uninstall")
}

enum PreviewTarget {
    Gallery,
    Screen {
        menu: MenuId,
        confirm: bool,
        #[cfg_attr(not(feature = "ratatui-ui"), allow(dead_code))]
        palette: bool,
        #[cfg_attr(not(feature = "ratatui-ui"), allow(dead_code))]
        help: bool,
        #[cfg_attr(not(feature = "ratatui-ui"), allow(dead_code))]
        tab: debopty_tui::WorkspaceTab,
        #[cfg_attr(not(feature = "ratatui-ui"), allow(dead_code))]
        default_selected: usize,
        #[cfg_attr(not(feature = "ratatui-ui"), allow(dead_code))]
        task_run: bool,
    },
}

fn parse_preview_screen(raw: &str) -> Result<PreviewTarget, CliError> {
    use debopty_tui::WorkspaceTab;
    match raw.trim().to_ascii_lowercase().as_str() {
        "all" => Ok(PreviewTarget::Gallery),
        "main" => Ok(PreviewTarget::Screen {
            menu: MenuId::Main,
            confirm: false,
            palette: false,
            help: false,
            tab: WorkspaceTab::Overview,
            default_selected: 0,
            task_run: false,
        }),
        "inspector" => Ok(PreviewTarget::Screen {
            menu: MenuId::Main,
            confirm: false,
            palette: false,
            help: false,
            tab: WorkspaceTab::Overview,
            default_selected: 6,
            task_run: false,
        }),
        "palette" => Ok(PreviewTarget::Screen {
            menu: MenuId::Main,
            confirm: false,
            palette: true,
            help: false,
            tab: WorkspaceTab::Overview,
            default_selected: 0,
            task_run: false,
        }),
        "help" => Ok(PreviewTarget::Screen {
            menu: MenuId::Main,
            confirm: false,
            palette: false,
            help: true,
            tab: WorkspaceTab::Overview,
            default_selected: 0,
            task_run: false,
        }),
        "syssec" => Ok(PreviewTarget::Screen {
            menu: MenuId::Main,
            confirm: false,
            palette: false,
            help: false,
            tab: WorkspaceTab::SysSec,
            default_selected: 0,
            task_run: false,
        }),
        "netproxy" => Ok(PreviewTarget::Screen {
            menu: MenuId::Main,
            confirm: false,
            palette: false,
            help: false,
            tab: WorkspaceTab::NetProxy,
            default_selected: 0,
            task_run: false,
        }),
        "reconcile" => Ok(PreviewTarget::Screen {
            menu: MenuId::Main,
            confirm: false,
            palette: false,
            help: false,
            tab: WorkspaceTab::Reconcile,
            default_selected: 0,
            task_run: false,
        }),
        "devops" => Ok(PreviewTarget::Screen {
            menu: MenuId::Devops,
            confirm: false,
            palette: false,
            help: false,
            tab: WorkspaceTab::Overview,
            default_selected: 0,
            task_run: false,
        }),
        "podman" => Ok(PreviewTarget::Screen {
            menu: MenuId::Podman,
            confirm: false,
            palette: false,
            help: false,
            tab: WorkspaceTab::Overview,
            default_selected: 0,
            task_run: false,
        }),
        "rust" => Ok(PreviewTarget::Screen {
            menu: MenuId::RustStack,
            confirm: false,
            palette: false,
            help: false,
            tab: WorkspaceTab::Overview,
            default_selected: 0,
            task_run: false,
        }),
        "go" => Ok(PreviewTarget::Screen {
            menu: MenuId::GoStack,
            confirm: false,
            palette: false,
            help: false,
            tab: WorkspaceTab::Overview,
            default_selected: 0,
            task_run: false,
        }),
        "warp" => Ok(PreviewTarget::Screen {
            menu: MenuId::WarpStack,
            confirm: false,
            palette: false,
            help: false,
            tab: WorkspaceTab::Overview,
            default_selected: 0,
            task_run: false,
        }),
        "maintenance" => Ok(PreviewTarget::Screen {
            menu: MenuId::Maintenance,
            confirm: false,
            palette: false,
            help: false,
            tab: WorkspaceTab::Overview,
            default_selected: 0,
            task_run: false,
        }),
        "confirm" => Ok(PreviewTarget::Screen {
            menu: MenuId::Podman,
            confirm: true,
            palette: false,
            help: false,
            tab: WorkspaceTab::Overview,
            default_selected: 0,
            task_run: false,
        }),
        "task-run" | "live-log" => Ok(PreviewTarget::Screen {
            menu: MenuId::Main,
            confirm: false,
            palette: false,
            help: false,
            tab: WorkspaceTab::Overview,
            default_selected: 0,
            task_run: true,
        }),
        other => Err(CliError::Usage(format!(
            "未知 --screen '{other}'；合法值: main|inspector|palette|help|syssec|netproxy|reconcile|devops|podman|rust|go|warp|maintenance|confirm|task-run|all"
        ))),
    }
}

#[cfg(not(feature = "ratatui-ui"))]
fn render_console_preview(
    menu: MenuId,
    status: &StatusSnapshot,
    width: u16,
    show_confirm: bool,
) -> String {
    use debopty_tui::layout::{
        DEFAULT_BOX_INNER_WIDTH, render_header_box, render_menu_item_plain, render_section_divider,
        render_separator,
    };
    use debopty_tui::menu::{MenuEntry, build_menu};

    let inner_w = (width as usize)
        .saturating_sub(2)
        .clamp(36, DEFAULT_BOX_INNER_WIDTH.max(80));
    let screen = build_menu(menu, status);
    let mut lines = render_header_box(&screen.title, Some(&screen.subtitle), inner_w);
    let mut item_idx = 0usize;
    for entry in &screen.entries {
        match entry {
            MenuEntry::Section(label) => lines.push(render_section_divider(label, inner_w)),
            MenuEntry::Separator => lines.push(render_separator(inner_w)),
            MenuEntry::Item(item) => {
                lines.push(render_menu_item_plain(item, item_idx == 0, inner_w));
                item_idx += 1;
            }
        }
    }
    if show_confirm {
        lines.push(render_separator(inner_w));
        lines.push(" ? 移除 Podman 环境？".to_string());
    }
    lines.join("\n") + "\n"
}

fn demo_status_snapshot() -> StatusSnapshot {
    #[cfg(feature = "ratatui-ui")]
    {
        debopty_tui::sample_showcase_status()
    }
    #[cfg(not(feature = "ratatui-ui"))]
    {
        use debopty_core::status::TRACKED_TOOLS;
        use std::collections::BTreeMap;
        let mut tools = BTreeMap::new();
        for &tool in TRACKED_TOOLS {
            tools.insert(tool.to_string(), false);
        }
        for enabled in [
            "fish",
            "micro",
            "yazi",
            "tmux",
            "podman",
            "xray",
            "ferron",
            "tailscale",
        ] {
            if tools.contains_key(enabled) {
                tools.insert(enabled.to_string(), true);
            }
        }
        StatusSnapshot {
            version_display: "v2.0.0".to_string(),
            is_cn_region: true,
            base_optimized: true,
            ip_forward: true,
            podman_gc_enabled: true,
            tools,
        }
    }
}

fn parse_preview_pane(raw: &str) -> Result<debopty_tui::ActivePane, CliError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "menu" => Ok(debopty_tui::ActivePane::Menu),
        "telemetry" | "status" => Ok(debopty_tui::ActivePane::Telemetry),
        other => Err(CliError::Usage(format!(
            "未知 --pane '{other}'；合法值: menu | telemetry"
        ))),
    }
}

pub fn handle_ui_preview(args: &UiPreviewArgs, config_path: &Path) -> Result<i32, CliError> {
    let parsed = parse_preview_screen(&args.screen)?;
    let pane = parse_preview_pane(&args.pane)?;
    let status = if args.demo {
        demo_status_snapshot()
    } else {
        let cfg = ConfigStore::new(config_path).load()?;
        let id = version::compiled_identity();
        let runner = RealCommandRunner;
        probe_status(&cfg, &id, &runner)
    };

    #[cfg(feature = "ratatui-ui")]
    {
        let telemetry = if args.demo {
            None
        } else {
            let specs = debopty_core::telemetry::probe_system_specs(Path::new("/"));
            let mut collector = debopty_core::telemetry::TelemetryCollector::new("/");
            let mut history = debopty_core::telemetry::TelemetryHistory::default();
            history.push_sample(collector.sample_now());
            Some((specs, history))
        };
        let base_opts = debopty_tui::ShowcaseOptions {
            ansi: args.ansi,
            tick: args.tick,
            transition: args.transition,
            transition_ms: args.transition_ms,
            selected: args.selected,
            filter: args.filter.clone(),
            palette: None,
            tab: debopty_tui::WorkspaceTab::Overview,
            pane,
            toast: args.toast.clone(),
            show_confirm: false,
            show_help: false,
            animations: args.tick > 0 || args.transition > 0 || args.transition_ms.is_some(),
            flat_modals: args.flat_modals,
            telemetry,
            task_run: None,
        };
        match parsed {
            PreviewTarget::Gallery => {
                let gallery = debopty_tui::render_showcase_gallery_with_options(
                    &status,
                    args.width,
                    args.height,
                    &base_opts,
                );
                for (name, frame) in gallery {
                    if !write_stdout_or_break(&format!("=== [{name}] ===\n"))?
                        || !write_stdout_or_break(&frame)?
                    {
                        return Ok(0);
                    }
                }
            }
            PreviewTarget::Screen {
                menu,
                confirm,
                palette,
                help,
                tab,
                default_selected,
                task_run,
            } => {
                let mut opts = base_opts;
                opts.show_confirm = confirm;
                opts.show_help = help;
                opts.tab = tab;
                if task_run {
                    opts.task_run = Some(debopty_tui::sample_showcase_task_run());
                }
                if opts.selected == 0 && default_selected > 0 {
                    opts.selected = default_selected;
                }
                if palette {
                    opts.palette = Some(opts.filter.take().unwrap_or_default());
                }
                let frame = debopty_tui::render_showcase_frame_with_options(
                    menu,
                    &status,
                    args.width,
                    args.height,
                    &opts,
                );
                let _ = write_stdout_or_break(&frame)?;
            }
        }
    }
    #[cfg(not(feature = "ratatui-ui"))]
    {
        let _ = pane;
        match parsed {
            PreviewTarget::Gallery => {
                for (name, menu, confirm, w) in [
                    ("main-wide", MenuId::Main, false, args.width),
                    ("main-narrow", MenuId::Main, false, args.width.min(64)),
                    ("devops", MenuId::Devops, false, args.width),
                    ("podman-confirm", MenuId::Podman, true, args.width),
                    ("maintenance", MenuId::Maintenance, false, args.width),
                ] {
                    if !write_stdout_or_break(&format!("=== [{name}] ===\n"))?
                        || !write_stdout_or_break(&render_console_preview(
                            menu, &status, w, confirm,
                        ))?
                    {
                        return Ok(0);
                    }
                }
            }
            PreviewTarget::Screen {
                menu,
                confirm,
                palette: _,
                help: _,
                tab: _,
                default_selected: _,
                task_run: _,
            } => {
                let _ = write_stdout_or_break(&render_console_preview(
                    menu, &status, args.width, confirm,
                ))?;
            }
        }
    }
    Ok(0)
}
