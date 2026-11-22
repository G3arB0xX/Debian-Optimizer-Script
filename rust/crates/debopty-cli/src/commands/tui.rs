use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use console::Term;
use debopty_bridge::{AppId, BashAction, BashBridge, EngineMode};
use debopty_core::apps::devops::{
    fish::{install_fish, uninstall_fish},
    lego::{
        LegoDomainSpec, LegoFlag, add_lego_domain, build_lego_troubleshoot_cmd, install_lego,
        list_lego_domains, remove_lego_domain, run_lego_once, toggle_lego_flag, uninstall_lego,
    },
    micro::{install_micro, uninstall_micro},
    yazi::{YaziInstallOptions, install_yazi, uninstall_yazi},
};
use debopty_core::apps::easytier::{install_easytier, uninstall_easytier};
use debopty_core::apps::ferron::{install_ferron, uninstall_ferron};
use debopty_core::apps::freship::{
    FreshipSelection, FreshipWorkMode, fetch_freship_map, format_freship_journal_logs,
    install_freship, probe_freship_status, reconfigure_freship, sync_freship_data,
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
    DEFAULT_REPO, SelfInstallOptions, SelfUninstallOptions, SelfUpdateOptions, self_install,
    self_uninstall, self_update,
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
use debopty_core::network::{
    DnsFixMode, DnsFixOptions, UreqHttpClient, check_and_fix_dns, get_latest_github_release,
    global_netcheck,
};
use debopty_core::reconcile::{ReconcileContext, ReconcileMode, audit_system, reconcile_system};
use debopty_core::security::SecurityContext;
use debopty_core::security::f2b_sync::run_f2b_sync;
use debopty_core::security::firewall::patch_nftables_ndp;
use debopty_core::security::ssh::{StdioPrompter, check_ssh_security};
use debopty_core::status::probe_status;
use debopty_core::sys::{CommandRunner, RealCommandRunner};
use debopty_core::system::{
    SystemContext, run_base_optimization, setup_tmux, toggle_ip_forwarding, uninstall_tmux,
};
use debopty_core::telemetry::{TelemetryCollector, TelemetryHistory, probe_system_specs};
use debopty_core::template::TemplateEngine;
use debopty_core::version;
use debopty_tui::layout::{render_header_box, render_separator};
use debopty_tui::{
    AppState, ConsoleRenderer, MotionPolicy, NoticeKind, TuiOutcome, TuiRenderer, UiBackendKind,
    is_non_interactive, run_tui_loop_with_state,
};

use crate::agent::{AgentCtx, emit_envelope};
use crate::commands::core::build_f2b_options;
use crate::commands::reconcile::print_reconcile_human;
use crate::{Cli, CliError, env_truthy, non_empty_env};

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
        None => Ok(UiBackendKind::Auto),
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
    // Ratatui responsive single-pane mode renders cleanly down to 40x16 (better than fixed Console box).
    size.is_some_and(|(w, h)| w >= 40 && h >= 16)
}

#[cfg(feature = "ratatui-ui")]
fn terminal_size_supports_ratatui() -> bool {
    let term = std::env::var("TERM").ok();
    terminal_supports_ratatui_with(term.as_deref(), detect_terminal_size())
}

/// Writes bytes to stdout without ever panicking on `BrokenPipe`.
/// Returns `false` when stdout is closed (`BrokenPipe`), signalling clean exit.
fn write_stdout_checked(bytes: &[u8]) -> bool {
    let mut stdout = io::stdout().lock();
    match stdout.write_all(bytes).and_then(|()| stdout.flush()) {
        Ok(()) => true,
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => false,
        Err(_) => false,
    }
}

fn writeln_safe(line: &str) -> bool {
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    write_stdout_checked(buf.as_bytes())
}

/// Reads a trimmed line from stdin; returns `None` on EOF (`Ok(0)`) or broken pipe.
fn prompt_line_checked(prompt: &str) -> Option<String> {
    if !write_stdout_checked(prompt.as_bytes()) {
        return None;
    }
    let mut line = String::new();
    match io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim().to_string()),
    }
}

/// Reads a sensitive token without echoing characters when attached to a TTY.
fn prompt_secret_checked(prompt: &str) -> Option<String> {
    if !write_stdout_checked(prompt.as_bytes()) {
        return None;
    }
    if io::stdin().is_terminal() {
        let term = Term::stderr();
        match term.read_secure_line() {
            Ok(secret) => Some(secret.trim().to_string()),
            Err(_) => None,
        }
    } else {
        let mut line = String::new();
        match io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim().to_string()),
        }
    }
}

/// Waits for user to press Enter; returns `false` on EOF / SSH disconnect.
fn wait_enter_or_eof(prompt: &str) -> bool {
    if !write_stdout_checked(prompt.as_bytes()) {
        return false;
    }
    let mut line = String::new();
    matches!(io::stdin().lock().read_line(&mut line), Ok(n) if n > 0)
}

fn render_submenu_header(title: &str, subtitle: &str, status_lines: &[String]) -> bool {
    let width = Term::stdout()
        .size_checked()
        .map(|(_, cols)| (cols as usize).saturating_sub(2).clamp(44, 72))
        .unwrap_or(58);
    if !writeln_safe("") {
        return false;
    }
    for line in render_header_box(title, Some(subtitle), width) {
        if !writeln_safe(&line) {
            return false;
        }
    }
    for s in status_lines {
        if !writeln_safe(&format!("  {s}")) {
            return false;
        }
    }
    writeln_safe(&render_separator(width))
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

fn apply_app_result(st: &mut AppState, label: &str, res: debopty_core::Result<AppReport>) -> bool {
    match res {
        Ok(report) => {
            for w in &report.warnings {
                let _ = writeln_safe(&format!("  [warn] {w}"));
            }
            for p in &report.preserved_user_custom {
                let _ = writeln_safe(&format!("  [preserved] {p}"));
            }
            st.set_notice_with_kind(NoticeKind::Success, report.summary);
            true
        }
        Err(err) => {
            st.set_notice_with_kind(
                NoticeKind::Danger,
                format!("{label}失败 [{}]: {err}", err.error_code()),
            );
            false
        }
    }
}

fn pick_index(prompt: &str, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    loop {
        let raw = prompt_line_checked(prompt)?;
        if raw.is_empty() || raw == "0" || raw.eq_ignore_ascii_case("q") {
            return None;
        }
        if let Ok(num) = raw.parse::<usize>()
            && (1..=count).contains(&num)
        {
            return Some(num - 1);
        }
        if !writeln_safe(&format!("  输入无效，请输入 1-{count}（或 0 取消）")) {
            return None;
        }
    }
}

/// Interactive cascading region + work-mode picker for FreshIP (replaces silent `None` default).
fn prompt_freship_selection(
    ctx: &AppContext,
    http: &UreqHttpClient,
    st: &mut AppState,
) -> Option<FreshipSelection> {
    let _ = writeln_safe("正在拉取 FreshIP 节点地区列表...");
    let (map, used_cache) = match fetch_freship_map(ctx, http) {
        Ok(pair) => pair,
        Err(err) => {
            st.set_notice_with_kind(
                NoticeKind::Danger,
                format!("获取 FreshIP 地区列表失败 [{}]: {err}", err.error_code()),
            );
            return None;
        }
    };
    if used_cache {
        let _ = writeln_safe("  [info] 远程拉取失败，已使用本地离线缓存 map.json");
    }

    let _ = writeln_safe("\n--- 步骤 1/5: 选择大洲 ---");
    for (i, cont) in map.continents.iter().enumerate() {
        let _ = writeln_safe(&format!("  {}. {} ({})", i + 1, cont.name, cont.id));
    }
    let _ = writeln_safe("  0. 取消");
    let c_idx = pick_index("请选择大洲编号: ", map.continents.len())?;
    let continent = &map.continents[c_idx];

    let _ = writeln_safe(&format!(
        "\n--- 步骤 2/5: 选择国家/地区 ({}) ---",
        continent.name
    ));
    for (i, country) in continent.countries.iter().enumerate() {
        let _ = writeln_safe(&format!("  {}. {} ({})", i + 1, country.name, country.id));
    }
    let _ = writeln_safe("  0. 取消");
    let co_idx = pick_index("请选择国家编号: ", continent.countries.len())?;
    let country = &continent.countries[co_idx];

    let state = if country.states.len() == 1 {
        &country.states[0]
    } else {
        let _ = writeln_safe(&format!("\n--- 步骤 3/5: 选择州/省 ({}) ---", country.name));
        for (i, s) in country.states.iter().enumerate() {
            let display = if s.name.is_empty() { &s.id } else { &s.name };
            let _ = writeln_safe(&format!("  {}. {} ({})", i + 1, display, s.id));
        }
        let _ = writeln_safe("  0. 取消");
        let s_idx = pick_index("请选择州/省编号: ", country.states.len())?;
        &country.states[s_idx]
    };

    let city = if state.cities.len() == 1 {
        &state.cities[0]
    } else {
        let _ = writeln_safe("\n--- 步骤 4/5: 选择目标城市 ---");
        for (i, ci) in state.cities.iter().enumerate() {
            let _ = writeln_safe(&format!("  {}. {} ({})", i + 1, ci.name, ci.id));
        }
        let _ = writeln_safe("  0. 取消");
        let ci_idx = pick_index("请选择城市编号: ", state.cities.len())?;
        &state.cities[ci_idx]
    };

    let _ = writeln_safe("\n--- 步骤 5/5: 选择网络栈模式 ---");
    let _ = writeln_safe("  1. 自动探测本机公网栈 (推荐)");
    let _ = writeln_safe("  2. 仅 IPv4 (ipv4_only)");
    let _ = writeln_safe("  3. 仅 IPv6 (ipv6_only)");
    let _ = writeln_safe("  4. 双栈并行 (dual_stack)");
    let mode_raw = prompt_line_checked("请选择网络栈模式 [默认 1]: ")?;
    let work_mode = match mode_raw.as_str() {
        "" | "1" => None,
        "2" => Some(FreshipWorkMode::Ipv4Only),
        "3" => Some(FreshipWorkMode::Ipv6Only),
        "4" => Some(FreshipWorkMode::DualStack),
        _ => {
            let _ = writeln_safe("  [info] 未识别选项，使用自动探测模式。");
            None
        }
    };

    Some(FreshipSelection {
        country_id: country.id.clone(),
        state_id: state.id.clone(),
        city_id: city.id.clone(),
        city_name: city.name.clone(),
        kw_filename: country.keyword_file.clone(),
        work_mode,
        bind_ipv4: None,
        bind_ipv6: None,
    })
}

fn handle_native_xray_submenu(
    ctx: &AppContext,
    http: &UreqHttpClient,
    runner: &RealCommandRunner,
    st: &mut AppState,
) {
    loop {
        let status = probe_xray_status(ctx, runner);
        let status_lines = vec![
            format!(
                "安装状态: {}   运行状态: {}",
                if status.installed {
                    "● 已安装"
                } else {
                    "○ 未安装"
                },
                if status.active {
                    "● 运行中"
                } else {
                    "○ 未运行"
                }
            ),
            format!(
                "路由规则: {}   每周自动更新: {}",
                status.ruleset.as_str(),
                if status.geo_auto_update {
                    "● 已开启"
                } else {
                    "○ 已关闭"
                }
            ),
        ];
        if !render_submenu_header("Xray Core 节点管理", "Main > Xray", &status_lines) {
            st.request_exit();
            return;
        }
        let items = [
            "  1. 安装 / 升级 Xray Core (--beta)",
            "  2. 切换路由规则集 (official <-> loyalsoldier)",
            "  3. 切换 GeoIP/GeoSite 每周一 03:30 自动更新 Cron",
            "  4. 卸载 Xray Core",
            "  0. 返回主菜单",
        ];
        for item in items {
            if !writeln_safe(item) {
                st.request_exit();
                return;
            }
        }
        let Some(choice) = prompt_line_checked("请选择 [0-4]: ") else {
            st.request_exit();
            return;
        };
        match choice.as_str() {
            "1" => {
                apply_app_result(st, "安装 Xray", install_xray(ctx, http, runner));
            }
            "2" => {
                apply_app_result(
                    st,
                    "切换 Xray 规则集",
                    toggle_xray_ruleset(ctx, http, runner),
                );
            }
            "3" => {
                apply_app_result(st, "切换 Xray Cron", toggle_xray_cron(ctx, runner));
            }
            "4" => match prompt_line_checked("确认卸载 Xray Core？[y/N]: ") {
                Some(ans) if ans.eq_ignore_ascii_case("y") => {
                    apply_app_result(st, "卸载 Xray", uninstall_xray(ctx, http, runner));
                }
                None => {
                    st.request_exit();
                    return;
                }
                _ => st.set_notice_with_kind(NoticeKind::Warning, "已取消卸载操作。"),
            },
            "" | "0" | "q" | "Q" => break,
            other => {
                let _ = writeln_safe(&format!("无效选项: {other}"));
            }
        }
        if let Some(msg) = st.notice()
            && !writeln_safe(&format!(">> {msg}"))
        {
            st.request_exit();
            return;
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
        let mode_str = status.work_mode.map(|m| m.as_str()).unwrap_or("unknown");
        let status_lines = vec![
            format!(
                "状态: {}   区域: {}/{}   模式: {mode_str}",
                if status.installed {
                    "● 已部署"
                } else {
                    "○ 未部署"
                },
                status.region_code,
                status.region_name,
            ),
            format!(
                "IPv4 定时器: {}   IPv6 定时器: {}",
                if status.v4_timer_active {
                    "● 运行中"
                } else {
                    "○ 已暂停"
                },
                if status.v6_timer_active {
                    "● 运行中"
                } else {
                    "○ 已暂停"
                }
            ),
        ];
        if !render_submenu_header("FreshIP IP 养护管理", "Main > FreshIP", &status_lines) {
            st.request_exit();
            return;
        }
        let items = [
            "  1. 重新配置区域与双栈模式 (交互式节点选择)",
            "  2. 暂停 / 恢复 定时养护任务",
            "  3. 立即同步热点数据",
            "  4. 查看近期运行日志",
            "  5. 卸载 FreshIP",
            "  0. 返回主菜单",
        ];
        for item in items {
            if !writeln_safe(item) {
                st.request_exit();
                return;
            }
        }
        let Some(choice) = prompt_line_checked("请选择 [0-5]: ") else {
            st.request_exit();
            return;
        };
        match choice.as_str() {
            "1" => {
                if let Some(sel) = prompt_freship_selection(ctx, http, st) {
                    apply_app_result(
                        st,
                        "重配 FreshIP",
                        reconfigure_freship(ctx, Some(sel), http, runner),
                    );
                } else if st.notice().is_none() {
                    st.set_notice_with_kind(NoticeKind::Warning, "已取消 FreshIP 区域重配。");
                }
            }
            "2" => {
                apply_app_result(
                    st,
                    "切换 FreshIP 定时器",
                    toggle_freship_timers(ctx, None, runner),
                );
            }
            "3" => {
                apply_app_result(st, "同步 FreshIP 数据", sync_freship_data(ctx, runner));
            }
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
                    let _ = writeln_safe("暂无 FreshIP 运行日志。");
                } else {
                    for line in lines {
                        if !writeln_safe(&line) {
                            st.request_exit();
                            return;
                        }
                    }
                }
            }
            "5" => match prompt_line_checked("确认卸载 FreshIP？[y/N]: ") {
                Some(ans) if ans.eq_ignore_ascii_case("y") => {
                    if apply_app_result(st, "卸载 FreshIP", uninstall_freship(ctx, runner)) {
                        break;
                    }
                }
                None => {
                    st.request_exit();
                    return;
                }
                _ => st.set_notice_with_kind(NoticeKind::Warning, "已取消卸载操作。"),
            },
            "" | "0" | "q" | "Q" => break,
            other => {
                let _ = writeln_safe(&format!("无效选项: {other}"));
            }
        }
        if let Some(msg) = st.notice()
            && !writeln_safe(&format!(">> {msg}"))
        {
            st.request_exit();
            return;
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
        let mut status_lines = Vec::new();
        if domains.is_empty() {
            status_lines.push("已托管域名: (暂无)".to_string());
        } else {
            for d in &domains {
                let days = d
                    .days_left
                    .map(|v| format!("{v}d"))
                    .unwrap_or_else(|| "-".to_string());
                status_lines.push(format!(
                    "{} [{} | renew={} | skip_prop={} | ferron={} | cert={}({})]",
                    d.primary_domain,
                    d.provider,
                    d.auto_renew,
                    d.dns_skip_propagation,
                    d.ferron_push,
                    d.cert_exists,
                    days
                ));
            }
        }
        if !render_submenu_header(
            "Lego SSL 自动化证书管理",
            "Main > DevOps > Lego",
            &status_lines,
        ) {
            st.request_exit();
            return;
        }
        let items = [
            "  1. 安装 / 升级 Lego",
            "  2. 添加域名托管 (支持 SAN / 多 DNS 提供商 / 脱敏密钥输入)",
            "  3. 切换域名开关 (自动续期 / 跳过传播检测 / Ferron 推送)",
            "  4. 立即签发或续期指定域名 (issue / renew / auto)",
            "  5. 生成手动排障命令 (不含明文密钥)",
            "  6. 移除域名托管 (保留证书文件)",
            "  7. 卸载 Lego",
            "  0. 返回上级菜单",
        ];
        for item in items {
            if !writeln_safe(item) {
                st.request_exit();
                return;
            }
        }
        let Some(choice) = prompt_line_checked("请选择 [0-7]: ") else {
            st.request_exit();
            return;
        };
        match choice.as_str() {
            "1" => {
                apply_app_result(st, "安装 Lego", install_lego(ctx, http, runner));
            }
            "2" => {
                let Some(primary_domain) = prompt_line_checked("主域名 (如 example.com): ")
                else {
                    st.request_exit();
                    return;
                };
                if primary_domain.is_empty() {
                    st.set_notice_with_kind(NoticeKind::Warning, "主域名不能为空，已取消添加。");
                } else {
                    let Some(san_raw) = prompt_line_checked(
                        "额外 SAN 域名 (逗号分隔，如 *.example.com，留空跳过): ",
                    ) else {
                        st.request_exit();
                        return;
                    };
                    let san_domains: Vec<String> = san_raw
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(ToString::to_string)
                        .collect();

                    let Some(email) = prompt_line_checked("ACME 通知邮箱: ") else {
                        st.request_exit();
                        return;
                    };
                    let Some(provider_raw) =
                        prompt_line_checked("DNS 提供商标识 [默认 cloudflare]: ")
                    else {
                        st.request_exit();
                        return;
                    };
                    let provider = if provider_raw.is_empty() {
                        "cloudflare".to_string()
                    } else {
                        provider_raw
                    };
                    let Some(cf_token) = prompt_secret_checked("DNS API Token (不回显): ")
                    else {
                        st.request_exit();
                        return;
                    };
                    let _ = writeln_safe("");

                    if email.is_empty() || cf_token.is_empty() {
                        st.set_notice_with_kind(
                            NoticeKind::Warning,
                            "ACME 邮箱与 API Token 均为必填项，已取消添加。",
                        );
                    } else {
                        let Some(issue_ans) =
                            prompt_line_checked("添加后是否立即尝试签发证书？[y/N]: ")
                        else {
                            st.request_exit();
                            return;
                        };
                        let spec = LegoDomainSpec {
                            primary_domain,
                            san_domains,
                            email,
                            cf_token,
                            provider,
                            issue_immediately: issue_ans.eq_ignore_ascii_case("y"),
                        };
                        apply_app_result(st, "添加 Lego 域名", add_lego_domain(ctx, &spec, runner));
                    }
                }
            }
            "3" => {
                let Some(domain) = prompt_line_checked("输入目标主域名: ") else {
                    st.request_exit();
                    return;
                };
                if domain.is_empty() {
                    st.set_notice_with_kind(NoticeKind::Warning, "域名不能为空。");
                } else {
                    let _ = writeln_safe("  1. 自动续期 (auto-renew)");
                    let _ = writeln_safe("  2. 跳过 DNS 传播等待 (dns-skip-propagation)");
                    let _ = writeln_safe("  3. 自动推送至 Ferron 证书库 (ferron-push)");
                    let Some(flag_choice) = prompt_line_checked("选择要切换的开关 [1-3]: ")
                    else {
                        st.request_exit();
                        return;
                    };
                    let flag = match flag_choice.as_str() {
                        "1" => Some(LegoFlag::AutoRenew),
                        "2" => Some(LegoFlag::DnsSkipPropagation),
                        "3" => Some(LegoFlag::FerronPush),
                        _ => None,
                    };
                    if let Some(flag) = flag {
                        apply_app_result(
                            st,
                            "切换 Lego 开关",
                            toggle_lego_flag(ctx, &domain, flag, runner),
                        );
                    } else {
                        st.set_notice_with_kind(NoticeKind::Warning, "无效开关编号，已取消。");
                    }
                }
            }
            "4" => {
                let Some(domain) = prompt_line_checked("输入目标主域名: ") else {
                    st.request_exit();
                    return;
                };
                if domain.is_empty() {
                    st.set_notice_with_kind(NoticeKind::Warning, "域名不能为空。");
                } else {
                    let Some(mode_raw) =
                        prompt_line_checked("执行模式 (issue / renew / auto) [默认 renew]: ")
                    else {
                        st.request_exit();
                        return;
                    };
                    let mode = match mode_raw.trim().to_ascii_lowercase().as_str() {
                        "" | "renew" => "renew",
                        "issue" => "issue",
                        "auto" | "run" => "auto",
                        other => {
                            st.set_notice_with_kind(
                                NoticeKind::Warning,
                                format!("未知模式 '{other}'，请输入 issue / renew / auto。"),
                            );
                            continue;
                        }
                    };
                    apply_app_result(
                        st,
                        "执行 Lego 证书任务",
                        run_lego_once(ctx, &domain, mode, runner),
                    );
                }
            }
            "5" => {
                let Some(domain) = prompt_line_checked("输入要排查的主域名: ") else {
                    st.request_exit();
                    return;
                };
                if !domain.is_empty() {
                    match build_lego_troubleshoot_cmd(ctx, &domain) {
                        Ok(cmd) => {
                            let _ = writeln_safe("\n=== Lego 手动排障命令 ===");
                            let _ = writeln_safe(&cmd);
                            let _ = writeln_safe("");
                            st.set_notice_with_kind(
                                NoticeKind::Info,
                                format!("已生成 {domain} 手动排障命令。"),
                            );
                        }
                        Err(err) => {
                            st.set_notice_with_kind(
                                NoticeKind::Danger,
                                format!("生成排障命令失败 [{}]: {err}", err.error_code()),
                            );
                        }
                    }
                }
            }
            "6" => {
                let Some(domain) = prompt_line_checked("输入要移除托管的主域名: ")
                else {
                    st.request_exit();
                    return;
                };
                if !domain.is_empty() {
                    apply_app_result(st, "移除域名托管", remove_lego_domain(ctx, &domain));
                }
            }
            "7" => match prompt_line_checked("确认卸载 Lego？[y/N]: ") {
                Some(ans) if ans.eq_ignore_ascii_case("y") => {
                    apply_app_result(st, "卸载 Lego", uninstall_lego(ctx, runner));
                }
                None => {
                    st.request_exit();
                    return;
                }
                _ => st.set_notice_with_kind(NoticeKind::Warning, "已取消卸载操作。"),
            },
            "" | "0" | "q" | "Q" => break,
            other => {
                let _ = writeln_safe(&format!("无效选项: {other}"));
            }
        }
        if let Some(msg) = st.notice()
            && !writeln_safe(&format!(">> {msg}"))
        {
            st.request_exit();
            return;
        }
    }
}

/// Pure gate deciding whether to execute the 8 legacy interactive startup hooks.
pub(crate) fn should_run_startup_hooks(
    is_root: bool,
    non_interactive: bool,
    skip_env: bool,
) -> bool {
    is_root && !non_interactive && !skip_env
}

/// Executes the 8 legacy startup hooks (`deb_optimizer.sh:212-245`) natively in Rust
/// with graceful offline degradation when running interactively as root.
fn run_startup_hooks(
    config_path: &Path,
    template_dir: Option<&Path>,
    runner: &RealCommandRunner,
) -> Result<(), CliError> {
    let http = UreqHttpClient::new();
    let store = ConfigStore::new(config_path);
    let mut cfg = store.load().unwrap_or_default();
    let is_cn_initial = cfg.is_cn_region.trim().eq_ignore_ascii_case("true");
    let app_ctx = build_tui_app_context(config_path, template_dir, is_cn_initial);

    // 1. Self-install if not yet installed under /opt/debopty/bin/debopty
    if !Path::new("/opt/debopty/bin/debopty").is_file() {
        let _ = self_install(&app_ctx, &SelfInstallOptions::default(), runner);
    }

    // 2. Startup update check (best-effort, non-blocking on offline hosts)
    let repo = non_empty_env("DEBOPTY_REPO").unwrap_or_else(|| DEFAULT_REPO.to_string());
    if let Ok(Some(latest_tag)) = get_latest_github_release(&repo, is_cn_initial, &http) {
        let latest_ver = latest_tag.trim().trim_start_matches('v');
        let cur = version::compiled_identity();
        if let Some(cur_ver) = cur.semver.as_deref()
            && !latest_ver.is_empty()
            && latest_ver != cur_ver
        {
            let _ = writeln_safe(&format!(
                "[debopty] 发现新版本 {latest_tag} (当前 v{cur_ver})，可在「脚本维护」中一键升级。"
            ));
        }
    }

    // 3. DNS health check (initial stage)
    let dns_initial_opts = DnsFixOptions {
        is_cn_region: is_cn_initial,
        ..DnsFixOptions::default()
    };
    let _ = check_and_fix_dns(DnsFixMode::Initial, &dns_initial_opts, &http, runner);

    // 4. Global netcheck (uses cached IS_CN_REGION when already populated)
    let is_cn = match global_netcheck(&store, false, &http) {
        Ok(rep) => rep.is_cn_region,
        Err(_) => is_cn_initial,
    };

    // 5. DNS health check (calibrate stage with detected region)
    let dns_cal_opts = DnsFixOptions {
        is_cn_region: is_cn,
        ..DnsFixOptions::default()
    };
    let _ = check_and_fix_dns(DnsFixMode::Calibrate, &dns_cal_opts, &http, runner);

    // 6. Patch legacy nftables IPv6 NDP rules
    let sec_ctx = SecurityContext {
        config_path: config_path.to_path_buf(),
        template_dir: template_dir.map(Path::to_path_buf),
        f2b_backend_override: non_empty_env("DEBOPTY_F2B_BACKEND"),
        binary_path: std::env::current_exe().ok(),
        ..SecurityContext::default()
    };
    let _ = patch_nftables_ndp(&sec_ctx, runner);

    // 6.5 Sync DevOps SOT links quietly on startup
    let app_ctx_cn = build_tui_app_context(config_path, template_dir, is_cn);
    let _ = sync_devops_sot_links(&app_ctx_cn, true, runner);

    // 7. First-run mandatory optimization flow when BASE_OPTIMIZED != "true"
    cfg = store.load().unwrap_or(cfg);
    if !cfg.base_optimized {
        let _ = writeln_safe("[debopty] 检测到系统尚未完成首次基础优化，正在启动安全加固与调优...");
        let prompter = StdioPrompter;
        let ssh_report = check_ssh_security(&sec_ctx, &prompter, runner)?;
        if ssh_report.rolled_back {
            return Err(CliError::Usage(
                "SSH 安全加固验证未通过并已回滚，已中止首次优化流程。".to_string(),
            ));
        }
        let sys_ctx = build_tui_system_context(config_path, template_dir, is_cn);
        let _ = run_base_optimization(&sys_ctx, &http, runner)?;
    }

    Ok(())
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
                Ok(enabled) => st.set_notice_with_kind(
                    NoticeKind::Success,
                    if enabled {
                        "路由转发已开启"
                    } else {
                        "路由转发已关闭"
                    },
                ),
                Err(err) => {
                    st.set_notice_with_kind(NoticeKind::Danger, format!("切换路由转发失败: {err}"))
                }
            }
        }
        BashAction::InstallApp(AppId::Tmux) => {
            let ctx = build_tui_system_context(config_path, template_dir, st.status.is_cn_region);
            match setup_tmux(&ctx, runner) {
                Ok(path) => st.set_notice_with_kind(
                    NoticeKind::Success,
                    format!("Tmux 已部署: {}", path.display()),
                ),
                Err(err) => {
                    st.set_notice_with_kind(NoticeKind::Danger, format!("部署 Tmux 失败: {err}"))
                }
            }
        }
        BashAction::UninstallApp(AppId::Tmux) => {
            let ctx = build_tui_system_context(config_path, template_dir, st.status.is_cn_region);
            match uninstall_tmux(&ctx, runner) {
                Ok(()) => st.set_notice_with_kind(NoticeKind::Success, "Tmux 已卸载"),
                Err(err) => {
                    st.set_notice_with_kind(NoticeKind::Danger, format!("卸载 Tmux 失败: {err}"))
                }
            }
        }
        BashAction::RunBaseOptimization => {
            let ctx = build_tui_system_context(config_path, template_dir, st.status.is_cn_region);
            match run_base_optimization(&ctx, &http, runner) {
                Ok(report) => st.set_notice_with_kind(
                    NoticeKind::Success,
                    format!(
                        "基础优化完成 ({})",
                        if report.is_cn_region {
                            "CN · 镜像"
                        } else {
                            "海外 · 直连"
                        }
                    ),
                ),
                Err(err) => {
                    st.set_notice_with_kind(NoticeKind::Danger, format!("基础优化失败: {err}"))
                }
            }
        }
        BashAction::InstallApp(AppId::Yazi) => {
            let media_preview = match prompt_line_checked(
                "是否安装 Yazi 多媒体预览组件 (ffmpeg/7z/jq/poppler/fd/rg/fzf)？[y/N]: ",
            ) {
                Some(ans) => Some(ans.eq_ignore_ascii_case("y")),
                None => {
                    st.request_exit();
                    return;
                }
            };
            let res = install_yazi(
                &app_ctx,
                &YaziInstallOptions { media_preview },
                &http,
                runner,
            );
            apply_app_result(st, "安装 Yazi", res);
        }
        BashAction::InstallApp(AppId::Freship) => {
            if let Some(sel) = prompt_freship_selection(&app_ctx, &http, st) {
                let res = install_freship(&app_ctx, Some(sel), &http, runner);
                apply_app_result(st, "安装 FreshIP", res);
            } else if st.notice().is_none() {
                st.set_notice_with_kind(NoticeKind::Warning, "已取消 FreshIP 安装。");
            }
        }
        BashAction::InstallApp(app) => {
            let res = match app {
                AppId::Fish => install_fish(&app_ctx, &http, runner),
                AppId::Micro => install_micro(&app_ctx, &http, runner),
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
                AppId::Yazi | AppId::Freship | AppId::Tmux => unreachable!("handled above"),
            };
            apply_app_result(st, &format!("安装 {}", app.display_name()), res);
        }
        BashAction::UninstallApp(AppId::Podman) => {
            let Some(data_ans) =
                prompt_line_checked("是否一并删除 /home/apps/srv 容器业务数据目录？[y/N]: ")
            else {
                st.request_exit();
                return;
            };
            let Some(user_ans) = prompt_line_checked("是否一并删除 apps 系统用户？[y/N]: ")
            else {
                st.request_exit();
                return;
            };
            let opts = PodmanUninstallOptions {
                delete_data: data_ans.eq_ignore_ascii_case("y"),
                delete_user: user_ans.eq_ignore_ascii_case("y"),
            };
            let res = uninstall_podman(&app_ctx, &opts, runner);
            apply_app_result(st, "卸载 Podman", res);
        }
        BashAction::UninstallApp(app) => {
            let res = match app {
                AppId::Fish => uninstall_fish(&app_ctx, runner),
                AppId::Micro => uninstall_micro(&app_ctx, runner),
                AppId::Yazi => uninstall_yazi(&app_ctx, runner),
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
                AppId::Podman | AppId::Tmux => unreachable!("handled above"),
            };
            apply_app_result(st, &format!("卸载 {}", app.display_name()), res);
        }
        BashAction::TogglePodmanGc => {
            apply_app_result(st, "切换 Podman GC", toggle_podman_gc(&app_ctx, runner));
        }
        BashAction::GenerateWarpXray => match generate_warp_xray(&app_ctx, &http, runner) {
            Ok(out) => {
                if !writeln_safe(&format!(
                    "\n=== Cloudflare WARP Xray WireGuard Outbound ===\n{}",
                    out.json
                )) {
                    st.request_exit();
                    return;
                }
                st.set_notice_with_kind(NoticeKind::Success, "WARP 出站 JSON 已生成");
            }
            Err(err) => {
                st.set_notice_with_kind(
                    NoticeKind::Danger,
                    format!("生成 WARP 配置失败 [{}]: {err}", err.error_code()),
                );
            }
        },
        BashAction::ManageXray => handle_native_xray_submenu(&app_ctx, &http, runner, st),
        BashAction::ManageFreship => handle_native_freship_submenu(&app_ctx, &http, runner, st),
        BashAction::ManageLego => handle_native_lego_submenu(&app_ctx, &http, runner, st),
        BashAction::ScriptUpdate => {
            apply_app_result(
                st,
                "升级 debopty",
                self_update(&app_ctx, &SelfUpdateOptions::default(), &http, runner),
            );
        }
        BashAction::ScriptUninstall => {
            let Some(ans) = prompt_line_checked("是否同时删除 /etc/debopti 配置？[y/N]: ")
            else {
                st.set_notice_with_kind(NoticeKind::Warning, "已取消卸载");
                st.request_exit();
                return;
            };
            let keep = !ans.eq_ignore_ascii_case("y");
            let ok = apply_app_result(
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
            if let Some(msg) = st.notice() {
                let _ = writeln_safe(&format!("\n{msg}"));
            }
            if ok {
                let _ = wait_enter_or_eof("\n卸载完成，按 Enter 退出...");
                st.request_exit();
            }
        }
        BashAction::LegacyFullEntry => {
            st.set_notice_with_kind(NoticeKind::Info, "已就绪");
        }
    }
}

pub fn run_default_entrypoint(
    cli: &Cli,
    config_path: &Path,
    agent: &AgentCtx,
) -> Result<i32, CliError> {
    let engine_mode = resolve_engine_mode(cli.engine.as_deref())?;
    if engine_mode == EngineMode::Bash {
        let bridge = BashBridge::discover(cli.script_dir.as_deref(), Some(config_path))?;
        return Ok(bridge.execute(&BashAction::LegacyFullEntry, &[])?);
    }

    let ui_kind = resolve_ui_backend(cli.ui.as_deref())?;
    let non_interactive = agent.non_interactive || agent.json;
    let mut renderer = build_renderer(ui_kind, non_interactive)?;

    if non_interactive {
        if agent.json {
            return emit_envelope(
                "tui",
                agent.dry_run,
                &serde_json::json!({
                    "skipped_tui": true,
                    "reason": "non-interactive or JSON mode",
                    "next_command": "debopty schema --json",
                    "hint": "Run `debopty schema --json` to discover structured CLI commands.",
                }),
            );
        }
        let _ = writeln_safe("非交互终端/CI 环境，已跳过 TUI（使用 --help 查看子命令）");
        return Ok(0);
    }

    let runner = RealCommandRunner;
    let is_root = unsafe { libc::geteuid() == 0 };
    if should_run_startup_hooks(
        is_root,
        non_interactive,
        env_truthy("DEBOPTY_SKIP_STARTUP_HOOKS"),
    ) {
        run_startup_hooks(config_path, cli.template_dir.as_deref(), &runner)?;
    }

    let store = ConfigStore::new(config_path);
    let cfg = store.load()?;
    let id = version::compiled_identity();
    let initial_status = probe_status(&cfg, &id, &runner);
    let initial_specs = probe_system_specs(Path::new("/"));

    let mut state =
        AppState::new(initial_status).with_telemetry(initial_specs, TelemetryHistory::default());
    state.attach_live_collector(TelemetryCollector::new("/"));
    state.set_motion_policy(MotionPolicy::detect());
    let template_dir = cli.template_dir.clone();

    run_tui_loop_with_state(renderer.as_mut(), &mut state, &mut |outcome, st| {
        match outcome {
            TuiOutcome::Exit => {}
            TuiOutcome::Refresh => {
                // Reload ConfigStore & probe_status below without suspending or pausing.
            }
            TuiOutcome::Execute(action) => {
                execute_tui_action(action, st, config_path, template_dir.as_deref(), &runner);
                if st.should_exit() {
                    return Ok(());
                }
                if action.needs_pause_after() {
                    if let Some(notice) = st.notice() {
                        let _ = writeln_safe(&format!("\n{notice}"));
                    }
                    if !wait_enter_or_eof("\n执行完毕，按 Enter 返回菜单...") {
                        st.request_exit();
                        return Ok(());
                    }
                }
            }
            TuiOutcome::RunNativeF2bSync => {
                let opts = build_f2b_options(false, false, template_dir.as_deref());
                let templates = TemplateEngine::new(template_dir.clone());
                match run_f2b_sync(&opts, &templates, &runner) {
                    Ok(report) => {
                        st.set_notice_with_kind(
                            NoticeKind::Success,
                            format!(
                                "端口同步完成 (SSH={}, 公网={})",
                                report.ssh_port, report.all_public_ports_f2b
                            ),
                        );
                    }
                    Err(err) => {
                        st.set_notice_with_kind(
                            NoticeKind::Danger,
                            format!("Fail2ban 端口同步失败: {err}"),
                        );
                    }
                }
                if let Some(notice) = st.notice() {
                    let _ = writeln_safe(&format!("\n{notice}"));
                }
                if !wait_enter_or_eof("\n执行完毕，按 Enter 返回菜单...") {
                    st.request_exit();
                    return Ok(());
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
                            st.set_notice_with_kind(
                                NoticeKind::Success,
                                format!("系统稳态 · 保留 {} 项自定义", audit.preserved_count),
                            );
                        } else {
                            match prompt_line_checked("\n应用稳态收敛并自动备份？[y/N]: ")
                            {
                                Some(ans) if ans.eq_ignore_ascii_case("y") => {
                                    match reconcile_system(&ctx, ReconcileMode::Apply, &runner) {
                                        Ok(applied) => {
                                            print_reconcile_human(&applied);
                                            let bak_hint = applied
                                                .backup_dir
                                                .as_ref()
                                                .map(|p| format!(" · 备份 {}", p.display()))
                                                .unwrap_or_default();
                                            st.set_notice_with_kind(
                                                NoticeKind::Success,
                                                format!(
                                                    "收敛完成 · 修复 {} · 保留 {}{bak_hint}",
                                                    applied.changes_made, applied.preserved_count
                                                ),
                                            );
                                        }
                                        Err(err) => {
                                            st.set_notice_with_kind(
                                                NoticeKind::Danger,
                                                format!(
                                                    "稳态收敛失败 [{}]: {err}",
                                                    err.error_code()
                                                ),
                                            );
                                        }
                                    }
                                }
                                None => {
                                    st.request_exit();
                                    return Ok(());
                                }
                                _ => {
                                    st.set_notice_with_kind(NoticeKind::Warning, "已取消收敛");
                                }
                            }
                        }
                    }
                    Err(err) => {
                        st.set_notice_with_kind(
                            NoticeKind::Danger,
                            format!("系统冲突巡检失败 [{}]: {err}", err.error_code()),
                        );
                    }
                }
                if !wait_enter_or_eof("\n执行完毕，按 Enter 返回菜单...") {
                    st.request_exit();
                    return Ok(());
                }
            }
        }

        if let Ok(reloaded_cfg) = store.load() {
            let refreshed = probe_status(&reloaded_cfg, &id, &runner);
            st.refresh_status(refreshed);
        }
        st.refresh_telemetry_specs(Path::new("/"));
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

    #[test]
    fn startup_hooks_gate_requires_root_interactive_and_not_skipped() {
        assert!(should_run_startup_hooks(true, false, false));
        assert!(!should_run_startup_hooks(false, false, false));
        assert!(!should_run_startup_hooks(true, true, false));
        assert!(!should_run_startup_hooks(true, false, true));
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
            Some((39, 16))
        ));
        assert!(!terminal_supports_ratatui_with(
            Some("xterm-256color"),
            Some((40, 15))
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
            Some((60, 18))
        ));
        assert!(terminal_supports_ratatui_with(
            Some("xterm-256color"),
            Some((80, 24))
        ));
        assert!(terminal_supports_ratatui_with(None, Some((100, 30))));
    }
}
