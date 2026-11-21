use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use debopty_bridge::{AppId, BashAction, BashBridge, EngineMode};
use debopty_core::apps::devops::{
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
    format_freship_journal_logs, install_freship, probe_freship_status, reconfigure_freship,
    sync_freship_data, toggle_freship_timers, uninstall_freship,
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
    SelfUninstallOptions, SelfUpdateOptions, self_uninstall, self_update,
};
use debopty_core::apps::shared::{AppContext, AppReport};
use debopty_core::apps::tailscale::{install_tailscale, uninstall_tailscale};
use debopty_core::apps::warp::{
    generate_warp_xray, install_usque, install_warp, uninstall_usque, uninstall_warp,
};
use debopty_core::apps::xray::{
    install_xray, probe_xray_status, toggle_xray_cron, toggle_xray_ruleset, uninstall_xray,
};
use debopty_core::config::ConfigStore;
use debopty_core::network::UreqHttpClient;
use debopty_core::reconcile::{ReconcileContext, ReconcileMode, audit_system, reconcile_system};
use debopty_core::security::f2b_sync::run_f2b_sync;
use debopty_core::status::probe_status;
use debopty_core::sys::{CommandRunner, RealCommandRunner};
use debopty_core::system::{
    SystemContext, run_base_optimization, setup_tmux, toggle_ip_forwarding, uninstall_tmux,
};
use debopty_core::template::TemplateEngine;
use debopty_core::version;
use debopty_tui::{
    AppState, ConsoleRenderer, TuiOutcome, TuiRenderer, UiBackendKind, is_non_interactive,
    run_tui_loop_with_state,
};

use crate::commands::core::build_f2b_options;
use crate::commands::reconcile::print_reconcile_human;
use crate::{Cli, CliError, non_empty_env};

fn resolve_engine_mode(cli_engine: Option<&str>) -> Result<EngineMode, CliError> {
    let raw = cli_engine
        .map(str::to_string)
        .or_else(|| non_empty_env("DEBOPTY_ENGINE"));
    match raw {
        Some(val) => EngineMode::from_str(&val).map_err(|e| CliError::Usage(e.to_string())),
        None => Ok(EngineMode::Auto),
    }
}

fn resolve_ui_backend(cli_ui: Option<&str>) -> Result<UiBackendKind, CliError> {
    let raw = cli_ui
        .map(str::to_string)
        .or_else(|| non_empty_env("DEBOPTY_UI"));
    match raw {
        Some(val) => UiBackendKind::from_str(&val).map_err(CliError::Usage),
        None => Ok(UiBackendKind::Console),
    }
}

#[cfg(any(feature = "ratatui-ui", test))]
fn is_dumb_term(term_env: Option<&str>) -> bool {
    term_env
        .map(str::trim)
        .is_some_and(|t| t.eq_ignore_ascii_case("dumb"))
}

#[cfg(feature = "ratatui-ui")]
fn detect_terminal_size() -> Option<(u16, u16)> {
    if let Some((cols, rows)) = debopty_tui::detect_terminal_size()
        && cols > 0
        && rows > 0
    {
        return Some((cols, rows));
    }

    let cols = non_empty_env("COLUMNS").and_then(|v| v.parse::<u16>().ok())?;
    let rows = non_empty_env("LINES").and_then(|v| v.parse::<u16>().ok())?;
    Some((cols, rows))
}

#[cfg(feature = "ratatui-ui")]
fn terminal_supports_ratatui_with(term_env: Option<&str>, size: Option<(u16, u16)>) -> bool {
    if is_dumb_term(term_env) {
        return false;
    }
    size.is_some_and(|(w, h)| w >= 80 && h >= 24)
}

#[cfg(feature = "ratatui-ui")]
fn terminal_size_supports_ratatui() -> bool {
    let term = std::env::var("TERM").ok();
    terminal_supports_ratatui_with(term.as_deref(), detect_terminal_size())
}

fn write_stdout_best_effort(bytes: &[u8]) {
    let mut stdout = io::stdout();
    if stdout.write_all(bytes).is_ok() {
        let _ = stdout.flush();
    }
}

fn prompt_line(prompt: &str) -> String {
    write_stdout_best_effort(prompt.as_bytes());
    let mut line = String::new();
    let _ = io::stdin().read_line(&mut line);
    line.trim().to_string()
}

fn build_renderer(
    ui_kind: UiBackendKind,
    _non_interactive: bool,
) -> Result<Box<dyn TuiRenderer>, CliError> {
    match ui_kind {
        UiBackendKind::Console => Ok(Box::new(ConsoleRenderer::new())),
        UiBackendKind::Ratatui => {
            #[cfg(feature = "ratatui-ui")]
            {
                Ok(Box::new(debopty_tui::RatatuiRenderer::new()))
            }
            #[cfg(not(feature = "ratatui-ui"))]
            {
                Err(CliError::Usage(
                    "当前二进制未启用 `ratatui-ui` 特性；请改用 `--ui console`，或使用 `--features ratatui-ui` 重新编译 debopty。"
                        .to_string(),
                ))
            }
        }
        UiBackendKind::Auto => {
            #[cfg(feature = "ratatui-ui")]
            {
                if !_non_interactive && terminal_size_supports_ratatui() {
                    return Ok(Box::new(debopty_tui::RatatuiRenderer::new()));
                }
            }
            Ok(Box::new(ConsoleRenderer::new()))
        }
    }
}

fn build_tui_system_context(
    config_path: &Path,
    template_dir: Option<&Path>,
    is_cn_region: bool,
) -> SystemContext {
    SystemContext {
        root_prefix: PathBuf::from("/"),
        is_cn_region,
        non_interactive: is_non_interactive(),
        no_sys: false,
        config_path: config_path.to_path_buf(),
        template_dir: template_dir.map(Path::to_path_buf),
        tmux_conf_override: None,
        tmux_skip_apt: false,
        tmux_version_override: None,
    }
}

fn build_tui_app_context(
    config_path: &Path,
    template_dir: Option<&Path>,
    is_cn_region: bool,
) -> AppContext {
    AppContext {
        config_path: config_path.to_path_buf(),
        template_dir: template_dir.map(Path::to_path_buf),
        is_cn_region,
        non_interactive: is_non_interactive(),
        ..AppContext::default()
    }
}

fn apply_app_result(st: &mut AppState, label: &str, res: debopty_core::Result<AppReport>) {
    match res {
        Ok(report) => {
            for w in &report.warnings {
                write_stdout_best_effort(format!("  [warn] {w}\n").as_bytes());
            }
            for p in &report.preserved_user_custom {
                write_stdout_best_effort(format!("  [preserved] {p}\n").as_bytes());
            }
            st.set_notice(report.summary);
        }
        Err(err) => {
            st.set_notice(format!("{label}失败 [{}]: {err}", err.error_code()));
        }
    }
}

fn handle_native_xray_submenu(
    ctx: &AppContext,
    http: &UreqHttpClient,
    runner: &RealCommandRunner,
    st: &mut AppState,
) {
    loop {
        let status = probe_xray_status(ctx, runner);
        println!("\n=== Xray Core 节点管理 (Pure Rust) ===");
        println!(
            "  状态: installed={} active={} ruleset={} auto_update={}",
            status.installed,
            status.active,
            status.ruleset.as_str(),
            status.geo_auto_update
        );
        println!("  1. 安装 / 升级 Xray Core (--beta)");
        println!("  2. 切换路由规则集 (official <-> loyalsoldier)");
        println!("  3. 切换 GeoIP/GeoSite 每日自动更新 Cron");
        println!("  4. 卸载 Xray Core");
        println!("  0. 返回主菜单");
        match prompt_line("请选择 [0-4]: ").as_str() {
            "1" => apply_app_result(st, "安装 Xray", install_xray(ctx, http, runner)),
            "2" => apply_app_result(
                st,
                "切换 Xray 规则集",
                toggle_xray_ruleset(ctx, http, runner),
            ),
            "3" => apply_app_result(st, "切换 Xray Cron", toggle_xray_cron(ctx, runner)),
            "4" => {
                if prompt_line("确认卸载 Xray Core？[y/N]: ").eq_ignore_ascii_case("y") {
                    apply_app_result(st, "卸载 Xray", uninstall_xray(ctx, http, runner));
                }
            }
            "" | "0" => break,
            other => println!("无效选项: {other}"),
        }
        if let Some(msg) = st.notice() {
            println!(">> {msg}");
        }
    }
}

fn handle_native_freship_submenu(
    ctx: &AppContext,
    http: &UreqHttpClient,
    runner: &RealCommandRunner,
    st: &mut AppState,
) {
    loop {
        let status = probe_freship_status(ctx, runner);
        println!("\n=== FreshIP IP 养护管理 (Pure Rust) ===");
        println!(
            "  状态: installed={} region={}/{} v4_timer={} v6_timer={}",
            status.installed,
            status.region_code,
            status.region_name,
            status.v4_timer_active,
            status.v6_timer_active
        );
        println!("  1. 重新配置区域与双栈模式");
        println!("  2. 暂停 / 恢复 定时养护任务");
        println!("  3. 立即同步热点数据");
        println!("  4. 查看近期运行日志");
        println!("  5. 卸载 FreshIP");
        println!("  0. 返回主菜单");
        match prompt_line("请选择 [0-5]: ").as_str() {
            "1" => apply_app_result(
                st,
                "重配 FreshIP",
                reconfigure_freship(ctx, None, http, runner),
            ),
            "2" => apply_app_result(
                st,
                "切换 FreshIP 定时器",
                toggle_freship_timers(ctx, None, runner),
            ),
            "3" => apply_app_result(st, "同步 FreshIP 数据", sync_freship_data(ctx, runner)),
            "4" => {
                let raw = runner
                    .run(
                        "journalctl",
                        &[
                            "-u",
                            "freship-core@*",
                            "-u",
                            "freship-updater.service",
                            "-n",
                            "30",
                            "--no-pager",
                            "-o",
                            "short-iso",
                        ],
                        Duration::from_secs(10),
                    )
                    .map(|o| o.stdout)
                    .unwrap_or_default();
                let lines = format_freship_journal_logs(&raw);
                if lines.is_empty() {
                    println!("暂无 FreshIP 日志。");
                } else {
                    for line in lines {
                        println!("{line}");
                    }
                }
            }
            "5" => {
                if prompt_line("确认卸载 FreshIP？[y/N]: ").eq_ignore_ascii_case("y") {
                    apply_app_result(st, "卸载 FreshIP", uninstall_freship(ctx, runner));
                    break;
                }
            }
            "" | "0" => break,
            other => println!("无效选项: {other}"),
        }
        if let Some(msg) = st.notice() {
            println!(">> {msg}");
        }
    }
}

fn handle_native_lego_submenu(
    ctx: &AppContext,
    http: &UreqHttpClient,
    runner: &RealCommandRunner,
    st: &mut AppState,
) {
    loop {
        let domains = list_lego_domains(ctx, runner).unwrap_or_default();
        println!("\n=== Lego SSL 自动化证书管理 (Pure Rust) ===");
        if domains.is_empty() {
            println!("  (暂无已配置域名)");
        } else {
            for d in &domains {
                println!(
                    "  - {} [provider={}, auto_renew={}, ferron_push={}, cert={}]",
                    d.primary_domain, d.provider, d.auto_renew, d.ferron_push, d.cert_exists
                );
            }
        }
        println!("  1. 安装 / 升级 Lego");
        println!("  2. 添加域名托管");
        println!("  3. 切换域名自动续期");
        println!("  4. 立即续期指定域名");
        println!("  5. 移除域名托管 (保留证书)");
        println!("  6. 卸载 Lego");
        println!("  0. 返回上级菜单");
        match prompt_line("请选择 [0-6]: ").as_str() {
            "1" => apply_app_result(st, "安装 Lego", install_lego(ctx, http, runner)),
            "2" => {
                let primary_domain = prompt_line("主域名 (如 example.com): ");
                let email = prompt_line("ACME 邮箱: ");
                let cf_token = prompt_line("Cloudflare API Token: ");
                if !primary_domain.is_empty() && !email.is_empty() && !cf_token.is_empty() {
                    let spec = LegoDomainSpec {
                        primary_domain,
                        san_domains: Vec::new(),
                        email,
                        cf_token,
                        provider: "cloudflare".to_string(),
                        issue_immediately: false,
                    };
                    apply_app_result(st, "添加 Lego 域名", add_lego_domain(ctx, &spec, runner));
                }
            }
            "3" => {
                let domain = prompt_line("输入要切换自动续期的主域名: ");
                if !domain.is_empty() {
                    apply_app_result(
                        st,
                        "切换自动续期",
                        toggle_lego_flag(ctx, &domain, LegoFlag::AutoRenew, runner),
                    );
                }
            }
            "4" => {
                let domain = prompt_line("输入要立即续期的主域名: ");
                if !domain.is_empty() {
                    apply_app_result(st, "续期证书", run_lego_once(ctx, &domain, "renew", runner));
                }
            }
            "5" => {
                let domain = prompt_line("输入要移除托管的主域名: ");
                if !domain.is_empty() {
                    apply_app_result(st, "移除域名托管", remove_lego_domain(ctx, &domain));
                }
            }
            "6" => {
                if prompt_line("确认卸载 Lego？[y/N]: ").eq_ignore_ascii_case("y") {
                    apply_app_result(st, "卸载 Lego", uninstall_lego(ctx, runner));
                }
            }
            "" | "0" => break,
            other => println!("无效选项: {other}"),
        }
        if let Some(msg) = st.notice() {
            println!(">> {msg}");
        }
    }
}

/// Dispatches 100% of TUI actions natively via `debopty-core` (0 `BashBridge` calls).
fn execute_tui_action(
    action: BashAction,
    st: &mut AppState,
    config_path: &Path,
    template_dir: Option<&Path>,
    runner: &RealCommandRunner,
) {
    let http = UreqHttpClient::new();
    let app_ctx = build_tui_app_context(config_path, template_dir, st.status.is_cn_region);

    match action {
        BashAction::ToggleIpForwarding => {
            let ctx = build_tui_system_context(config_path, template_dir, st.status.is_cn_region);
            match toggle_ip_forwarding(&ctx, runner) {
                Ok(enabled) => st.set_notice(if enabled {
                    "内核 IPv4/IPv6 路由转发已开启。".to_string()
                } else {
                    "内核 IPv4/IPv6 路由转发已关闭。".to_string()
                }),
                Err(err) => st.set_notice(format!("切换路由转发失败: {err}")),
            }
        }
        BashAction::InstallApp(AppId::Tmux) => {
            let ctx = build_tui_system_context(config_path, template_dir, st.status.is_cn_region);
            match setup_tmux(&ctx, runner) {
                Ok(path) => st.set_notice(format!("Tmux 配置已部署: {}", path.display())),
                Err(err) => st.set_notice(format!("部署 Tmux 失败: {err}")),
            }
        }
        BashAction::UninstallApp(AppId::Tmux) => {
            let ctx = build_tui_system_context(config_path, template_dir, st.status.is_cn_region);
            match uninstall_tmux(&ctx, runner) {
                Ok(()) => st.set_notice("Tmux 已卸载并还原原有配置。".to_string()),
                Err(err) => st.set_notice(format!("卸载 Tmux 失败: {err}")),
            }
        }
        BashAction::RunBaseOptimization => {
            let ctx = build_tui_system_context(config_path, template_dir, st.status.is_cn_region);
            match run_base_optimization(&ctx, &http, runner) {
                Ok(report) => st.set_notice(format!(
                    "一键系统级基础优化已完成 ({})。",
                    if report.is_cn_region {
                        "中国大陆"
                    } else {
                        "海外直连"
                    }
                )),
                Err(err) => st.set_notice(format!("执行基础优化失败: {err}")),
            }
        }
        BashAction::InstallApp(app) => {
            let res = match app {
                AppId::Fish => install_fish(&app_ctx, &http, runner),
                AppId::Micro => install_micro(&app_ctx, &http, runner),
                AppId::Yazi => {
                    install_yazi(&app_ctx, &YaziInstallOptions::default(), &http, runner)
                }
                AppId::Podman => install_podman(&app_ctx, runner),
                AppId::RustSdk => install_rust(&app_ctx, &http, runner),
                AppId::Realm => install_realm(&app_ctx, &http, runner),
                AppId::Ferron => install_ferron(&app_ctx, &http, runner),
                AppId::GoSdk => install_go(&app_ctx, &http, runner),
                AppId::Caddy => install_caddy(&app_ctx, &http, runner),
                AppId::Derper => install_derper(&app_ctx, &http, runner),
                AppId::Xray => install_xray(&app_ctx, &http, runner),
                AppId::Warp => install_warp(&app_ctx, &http, runner),
                AppId::Usque => install_usque(&app_ctx, &http, runner),
                AppId::Easytier => install_easytier(&app_ctx, &http, runner),
                AppId::Tailscale => install_tailscale(&app_ctx, &http, runner),
                AppId::Freship => install_freship(&app_ctx, None, &http, runner),
                AppId::Tmux => unreachable!("handled above"),
            };
            apply_app_result(st, &format!("安装 {}", app.display_name()), res);
        }
        BashAction::UninstallApp(app) => {
            let res = match app {
                AppId::Fish => uninstall_fish(&app_ctx, runner),
                AppId::Micro => uninstall_micro(&app_ctx, runner),
                AppId::Yazi => uninstall_yazi(&app_ctx, runner),
                AppId::Podman => {
                    uninstall_podman(&app_ctx, &PodmanUninstallOptions::default(), runner)
                }
                AppId::RustSdk => uninstall_rust(&app_ctx, runner),
                AppId::Realm => uninstall_realm(&app_ctx, runner),
                AppId::Ferron => uninstall_ferron(&app_ctx, runner),
                AppId::GoSdk => uninstall_go(&app_ctx, runner),
                AppId::Caddy => uninstall_caddy(&app_ctx, runner),
                AppId::Derper => uninstall_derper(&app_ctx, runner),
                AppId::Xray => uninstall_xray(&app_ctx, &http, runner),
                AppId::Warp => uninstall_warp(&app_ctx, runner),
                AppId::Usque => uninstall_usque(&app_ctx, runner),
                AppId::Easytier => uninstall_easytier(&app_ctx, &http, runner),
                AppId::Tailscale => uninstall_tailscale(&app_ctx, runner),
                AppId::Freship => uninstall_freship(&app_ctx, runner),
                AppId::Tmux => unreachable!("handled above"),
            };
            apply_app_result(st, &format!("卸载 {}", app.display_name()), res);
        }
        BashAction::TogglePodmanGc => {
            apply_app_result(st, "切换 Podman GC", toggle_podman_gc(&app_ctx, runner));
        }
        BashAction::GenerateWarpXray => match generate_warp_xray(&app_ctx, &http, runner) {
            Ok(out) => {
                println!(
                    "\n=== Cloudflare WARP Xray WireGuard Outbound ===\n{}",
                    out.json
                );
                st.set_notice("已生成 WARP Xray WireGuard Outbound JSON 配置。");
            }
            Err(err) => {
                st.set_notice(format!(
                    "生成 WARP Xray 配置失败 [{}]: {err}",
                    err.error_code()
                ));
            }
        },
        BashAction::ManageXray => handle_native_xray_submenu(&app_ctx, &http, runner, st),
        BashAction::ManageFreship => handle_native_freship_submenu(&app_ctx, &http, runner, st),
        BashAction::ManageLego => handle_native_lego_submenu(&app_ctx, &http, runner, st),
        BashAction::ScriptUpdate => {
            apply_app_result(
                st,
                "在线升级 debopty",
                self_update(&app_ctx, &SelfUpdateOptions::default(), &http, runner),
            );
        }
        BashAction::ScriptUninstall => {
            let keep = !prompt_line("是否连同 /etc/debopti 配置文件一并删除？[y/N]: ")
                .eq_ignore_ascii_case("y");
            apply_app_result(
                st,
                "卸载 debopty",
                self_uninstall(
                    &app_ctx,
                    &SelfUninstallOptions {
                        keep_config: keep,
                        install_dir: None,
                    },
                    runner,
                ),
            );
        }
        BashAction::LegacyFullEntry => {
            st.set_notice("当前已运行于 100% Pure Rust 原生引擎 (21/21)。");
        }
    }
}

pub fn run_default_entrypoint(cli: &Cli, config_path: &Path) -> Result<i32, CliError> {
    let engine_mode = resolve_engine_mode(cli.engine.as_deref())?;
    if engine_mode == EngineMode::Bash {
        let bridge = BashBridge::discover(cli.script_dir.as_deref(), Some(config_path))?;
        return Ok(bridge.execute(&BashAction::LegacyFullEntry, &[])?);
    }

    let ui_kind = resolve_ui_backend(cli.ui.as_deref())?;
    let non_interactive = is_non_interactive();
    let mut renderer = build_renderer(ui_kind, non_interactive)?;

    if non_interactive {
        println!("检测到非交互终端或 CI 环境，已跳过 TUI 启动；可使用 --help 查看子命令。");
        return Ok(0);
    }

    let store = ConfigStore::new(config_path);
    let cfg = store.load()?;
    let id = version::compiled_identity();
    let runner = RealCommandRunner;
    let initial_status = probe_status(&cfg, &id, &runner);

    let mut state = AppState::new(initial_status);
    #[cfg(feature = "ratatui-ui")]
    {
        let no_color = non_empty_env("NO_COLOR").is_some();
        let term = non_empty_env("TERM");
        let ci = non_empty_env("CI");
        state.set_animations_enabled(debopty_tui::should_enable_animations(
            no_color,
            term.as_deref(),
            ci.as_deref(),
        ));
    }
    let template_dir = cli.template_dir.clone();

    run_tui_loop_with_state(renderer.as_mut(), &mut state, &mut |outcome, st| {
        match outcome {
            TuiOutcome::Exit => {}
            TuiOutcome::Refresh => {
                // Reload ConfigStore & probe_status below without suspending or pausing.
            }
            TuiOutcome::Execute(action) => {
                execute_tui_action(action, st, config_path, template_dir.as_deref(), &runner);
                if action.needs_pause_after() {
                    if let Some(notice) = st.notice() {
                        write_stdout_best_effort(format!("\n{notice}\n").as_bytes());
                    }
                    write_stdout_best_effort("\n执行完毕，按 Enter 返回菜单...".as_bytes());
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
            TuiOutcome::RunNativeReconcile => {
                let mut ctx = ReconcileContext::with_root("/");
                ctx.config_path = config_path.to_path_buf();
                ctx.template_dir = template_dir.clone();
                ctx.is_cn_region = st.status.is_cn_region;
                ctx.f2b_backend_override = non_empty_env("DEBOPTY_F2B_BACKEND");
                match audit_system(&ctx, &runner) {
                    Ok(audit) => {
                        print_reconcile_human(&audit);
                        if audit.drift_count == 0 {
                            st.set_notice(format!(
                                "巡检完成：系统已处于稳态（保留 {} 项用户自定义配置）。",
                                audit.preserved_count
                            ));
                        } else {
                            write_stdout_best_effort(
                                "\n是否立即应用以上稳态收敛并自动创建备份？[y/N]: ".as_bytes(),
                            );
                            let mut ans = String::new();
                            let _ = io::stdin().read_line(&mut ans);
                            if ans.trim().eq_ignore_ascii_case("y") {
                                match reconcile_system(&ctx, ReconcileMode::Apply, &runner) {
                                    Ok(applied) => {
                                        print_reconcile_human(&applied);
                                        let bak_hint = applied
                                            .backup_dir
                                            .as_ref()
                                            .map(|p| format!("，备份目录: {}", p.display()))
                                            .unwrap_or_default();
                                        st.set_notice(format!(
                                            "稳态收敛完成：已修复 {} 项，保留 {} 项用户配置{bak_hint}",
                                            applied.changes_made, applied.preserved_count
                                        ));
                                    }
                                    Err(err) => {
                                        st.set_notice(format!(
                                            "稳态收敛失败 [{}]: {err}",
                                            err.error_code()
                                        ));
                                    }
                                }
                            } else {
                                st.set_notice("已取消系统稳态接管。".to_string());
                            }
                        }
                    }
                    Err(err) => {
                        st.set_notice(format!("系统冲突巡检失败 [{}]: {err}", err.error_code()));
                    }
                }
                write_stdout_best_effort("\n执行完毕，按 Enter 返回菜单...".as_bytes());
                let mut pause = String::new();
                let _ = io::stdin().read_line(&mut pause);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dumb_term_detection_is_case_insensitive() {
        assert!(is_dumb_term(Some("dumb")));
        assert!(is_dumb_term(Some("DUMB")));
        assert!(is_dumb_term(Some(" Dumb ")));
        assert!(!is_dumb_term(Some("xterm-256color")));
        assert!(!is_dumb_term(Some("")));
        assert!(!is_dumb_term(None));
    }

    #[cfg(feature = "ratatui-ui")]
    #[test]
    fn terminal_supports_ratatui_rejects_dumb_term_and_tiny_sizes() {
        assert!(!terminal_supports_ratatui_with(
            Some("dumb"),
            Some((120, 40))
        ));
        assert!(!terminal_supports_ratatui_with(
            Some("DUMB"),
            Some((120, 40))
        ));
        assert!(!terminal_supports_ratatui_with(
            Some("xterm-256color"),
            Some((79, 24))
        ));
        assert!(!terminal_supports_ratatui_with(
            Some("xterm-256color"),
            Some((80, 23))
        ));
        assert!(!terminal_supports_ratatui_with(
            Some("xterm-256color"),
            Some((0, 0))
        ));
        assert!(!terminal_supports_ratatui_with(
            Some("xterm-256color"),
            None
        ));
        assert!(terminal_supports_ratatui_with(
            Some("xterm-256color"),
            Some((80, 24))
        ));
        assert!(terminal_supports_ratatui_with(None, Some((100, 30))));
    }
}
