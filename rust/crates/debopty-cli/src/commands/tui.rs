use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, IsTerminal, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(any(feature = "ratatui-ui", test))]
use std::sync::{Arc, mpsc};
#[cfg(any(feature = "ratatui-ui", test))]
use std::thread;
#[cfg(feature = "ratatui-ui")]
use std::time::Instant;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use console::Term;
use debopty_bridge::{AppId, BashAction, BashBridge, EngineMode};
#[cfg(any(feature = "ratatui-ui", test))]
use debopty_core::apps::devops::lego::LegoDomainStatus;
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
#[cfg(any(feature = "ratatui-ui", test))]
use debopty_core::apps::freship::FreshipMap;
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
#[cfg(any(feature = "ratatui-ui", test))]
use debopty_core::apps::shared::emit_progress;
use debopty_core::apps::shared::{AppContext, AppReport, sync_devops_sot_links};
use debopty_core::apps::tailscale::{install_tailscale, uninstall_tailscale};
use debopty_core::apps::warp::{
    generate_warp_xray, install_usque, install_warp, uninstall_usque, uninstall_warp,
};
use debopty_core::apps::xray::{
    install_xray, probe_xray_status, toggle_xray_cron, toggle_xray_ruleset, uninstall_xray,
};
use debopty_core::config::ConfigStore;
#[cfg(any(feature = "ratatui-ui", test))]
use debopty_core::diag::{self, LogLevel};
use debopty_core::network::{
    DnsFixMode, DnsFixOptions, UreqHttpClient, check_and_fix_dns, get_latest_github_release,
    global_netcheck,
};
#[cfg(any(feature = "ratatui-ui", test))]
use debopty_core::reconcile::{FindingSeverity, FindingStatus, ReconcileReport};
use debopty_core::reconcile::{ReconcileContext, ReconcileMode, audit_system, reconcile_system};
use debopty_core::security::SecurityContext;
use debopty_core::security::f2b_sync::run_f2b_sync;
use debopty_core::security::firewall::patch_nftables_ndp;
use debopty_core::security::ssh::{StdioPrompter, check_ssh_security};
#[cfg(any(feature = "ratatui-ui", test))]
use debopty_core::status::StatusSnapshot;
use debopty_core::status::probe_status;
#[cfg(any(feature = "ratatui-ui", test))]
use debopty_core::sys::{self, LineSink, StreamingCommandRunner};
use debopty_core::sys::{CommandRunner, RealCommandRunner};
use debopty_core::system::{
    SystemContext, run_base_optimization, setup_tmux, toggle_ip_forwarding, uninstall_tmux,
};
#[cfg(any(feature = "ratatui-ui", test))]
use debopty_core::telemetry::SystemSpecs;
use debopty_core::telemetry::{TelemetryCollector, TelemetryHistory, probe_system_specs};
use debopty_core::template::TemplateEngine;
use debopty_core::version;
#[cfg(feature = "ratatui-ui")]
use debopty_tui::MenuEvent;
#[cfg(any(feature = "ratatui-ui", test))]
use debopty_tui::PickerKind;
use debopty_tui::layout::{render_header_box, render_separator};
use debopty_tui::{
    AppState, ConsoleRenderer, MenuAction, MotionPolicy, NoticeKind, PromptSubmission, TuiOutcome,
    UiBackendKind, is_non_interactive, run_tui_loop_with_state,
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

fn should_use_ratatui(ui_kind: UiBackendKind, non_interactive: bool) -> Result<bool, CliError> {
    match ui_kind {
        UiBackendKind::Console => Ok(false),
        UiBackendKind::Ratatui => {
            #[cfg(feature = "ratatui-ui")]
            {
                let _ = non_interactive;
                Ok(true)
            }
            #[cfg(not(feature = "ratatui-ui"))]
            {
                let _ = non_interactive;
                Err(CliError::Usage(
                    "当前二进制未启用 `ratatui-ui` 特性；请改用 `--ui console`，或使用 `--features ratatui-ui` 重新编译 debopty。"
                        .to_string(),
                ))
            }
        }
        UiBackendKind::Auto => {
            #[cfg(feature = "ratatui-ui")]
            {
                Ok(!non_interactive && terminal_size_supports_ratatui())
            }
            #[cfg(not(feature = "ratatui-ui"))]
            {
                let _ = non_interactive;
                Ok(false)
            }
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

/// Dispatches native `MenuAction` outcomes in `--ui console` mode.
fn execute_console_menu_action(
    action: MenuAction,
    st: &mut AppState,
    config_path: &Path,
    template_dir: Option<&Path>,
    runner: &RealCommandRunner,
) {
    let http = UreqHttpClient::new();
    let app_ctx = build_tui_app_context(config_path, template_dir, st.status.is_cn_region);
    match action {
        MenuAction::XrayToggleRuleset => {
            apply_app_result(
                st,
                "切换 Xray 规则集",
                toggle_xray_ruleset(&app_ctx, &http, runner),
            );
        }
        MenuAction::XrayToggleCron => {
            apply_app_result(st, "切换 Xray Cron", toggle_xray_cron(&app_ctx, runner));
        }
        MenuAction::FreshipReconfigure => {
            if let Some(sel) = prompt_freship_selection(&app_ctx, &http, st) {
                apply_app_result(
                    st,
                    "重配 FreshIP",
                    reconfigure_freship(&app_ctx, Some(sel), &http, runner),
                );
            } else if st.notice().is_none() {
                st.set_notice_with_kind(NoticeKind::Warning, "已取消 FreshIP 区域重配。");
            }
        }
        MenuAction::FreshipToggleTimers => {
            apply_app_result(
                st,
                "切换 FreshIP 定时器",
                toggle_freship_timers(&app_ctx, None, runner),
            );
        }
        MenuAction::FreshipSync => {
            apply_app_result(st, "同步 FreshIP 数据", sync_freship_data(&app_ctx, runner));
        }
        MenuAction::FreshipLogs => {
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
            st.set_notice_with_kind(NoticeKind::Info, "FreshIP 日志已输出");
        }
        MenuAction::LegoInstall => {
            apply_app_result(st, "安装 Lego", install_lego(&app_ctx, &http, runner));
        }
        MenuAction::LegoUninstall => {
            apply_app_result(st, "卸载 Lego", uninstall_lego(&app_ctx, runner));
        }
        MenuAction::LegoAddDomain
        | MenuAction::LegoToggleFlag
        | MenuAction::LegoRunOnce
        | MenuAction::LegoTroubleshoot
        | MenuAction::LegoRemoveDomain => {
            handle_native_lego_submenu(&app_ctx, &http, runner, st);
        }
        _ => {}
    }
}

/// Dispatches structured `PromptSubmission` payloads in `--ui console` mode if ever emitted.
fn execute_console_prompt_submission(
    submission: PromptSubmission,
    st: &mut AppState,
    config_path: &Path,
    template_dir: Option<&Path>,
    runner: &RealCommandRunner,
) {
    let http = UreqHttpClient::new();
    let app_ctx = build_tui_app_context(config_path, template_dir, st.status.is_cn_region);
    match submission {
        PromptSubmission::YaziInstall { media_preview } => {
            apply_app_result(
                st,
                "安装 Yazi",
                install_yazi(
                    &app_ctx,
                    &YaziInstallOptions {
                        media_preview: Some(media_preview),
                    },
                    &http,
                    runner,
                ),
            );
        }
        PromptSubmission::PodmanUninstall {
            delete_data,
            delete_user,
        } => {
            apply_app_result(
                st,
                "卸载 Podman",
                uninstall_podman(
                    &app_ctx,
                    &PodmanUninstallOptions {
                        delete_data,
                        delete_user,
                    },
                    runner,
                ),
            );
        }
        PromptSubmission::ScriptUninstall { delete_config } => {
            let ok = apply_app_result(
                st,
                "卸载 debopty",
                self_uninstall(
                    &app_ctx,
                    &SelfUninstallOptions {
                        keep_config: !delete_config,
                        install_dir: None,
                    },
                    runner,
                ),
            );
            if ok {
                st.request_exit();
            }
        }
        PromptSubmission::ReconcileApply => {
            let mut ctx = ReconcileContext::with_root("/");
            ctx.config_path = config_path.to_path_buf();
            ctx.template_dir = template_dir.map(Path::to_path_buf);
            ctx.is_cn_region = st.status.is_cn_region;
            ctx.f2b_backend_override = non_empty_env("DEBOPTY_F2B_BACKEND");
            match reconcile_system(&ctx, ReconcileMode::Apply, runner) {
                Ok(applied) => {
                    print_reconcile_human(&applied);
                    st.set_notice_with_kind(
                        NoticeKind::Success,
                        format!(
                            "收敛完成 · 修复 {} · 保留 {}",
                            applied.changes_made, applied.preserved_count
                        ),
                    );
                }
                Err(err) => {
                    st.set_notice_with_kind(
                        NoticeKind::Danger,
                        format!("稳态收敛失败 [{}]: {err}", err.error_code()),
                    );
                }
            }
        }
        PromptSubmission::Freship {
            selection,
            is_reconfigure,
        } => {
            let res = if is_reconfigure {
                reconfigure_freship(&app_ctx, Some(selection), &http, runner)
            } else {
                install_freship(&app_ctx, Some(selection), &http, runner)
            };
            apply_app_result(
                st,
                if is_reconfigure {
                    "重配 FreshIP"
                } else {
                    "安装 FreshIP"
                },
                res,
            );
        }
        PromptSubmission::LegoAddDomain(spec) => {
            apply_app_result(
                st,
                "添加 Lego 域名",
                add_lego_domain(&app_ctx, &spec, runner),
            );
        }
        PromptSubmission::LegoToggleFlag { domain, flag } => {
            apply_app_result(
                st,
                "切换 Lego 开关",
                toggle_lego_flag(&app_ctx, &domain, flag, runner),
            );
        }
        PromptSubmission::LegoRunOnce { domain, mode } => {
            apply_app_result(
                st,
                "执行 Lego 证书任务",
                run_lego_once(&app_ctx, &domain, &mode, runner),
            );
        }
        PromptSubmission::LegoTroubleshoot { domain } => {
            match build_lego_troubleshoot_cmd(&app_ctx, &domain) {
                Ok(cmd) => {
                    let _ = writeln_safe("\n=== Lego 手动排障命令 ===");
                    let _ = writeln_safe(&cmd);
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
        PromptSubmission::LegoRemoveDomain { domain } => {
            apply_app_result(st, "移除域名托管", remove_lego_domain(&app_ctx, &domain));
        }
    }
}

#[cfg(any(feature = "ratatui-ui", test))]
enum WorkerMsg {
    LogLine(String),
    Refreshed {
        status: Option<StatusSnapshot>,
        specs: SystemSpecs,
    },
    FreshipMapLoaded {
        map: FreshipMap,
        used_cache: bool,
        is_reconfigure: bool,
    },
    LegoDomainsLoaded {
        kind: PickerKind,
        domains: Vec<LegoDomainStatus>,
    },
    ReconcileAuditDone {
        drift_count: usize,
        preserved_count: usize,
        status: Option<StatusSnapshot>,
        specs: SystemSpecs,
    },
    Finished {
        succeeded: bool,
        summary: String,
        notice_kind: NoticeKind,
        status: Option<StatusSnapshot>,
        specs: Option<SystemSpecs>,
    },
}

#[cfg(any(feature = "ratatui-ui", test))]
fn select_poll_timeout(
    worker_active: bool,
    next_wake: Option<Duration>,
    stream_interval: Duration,
) -> Option<Duration> {
    if worker_active {
        Some(next_wake.map_or(stream_interval, |w| w.min(stream_interval)))
    } else {
        next_wake
    }
}

#[cfg(any(feature = "ratatui-ui", test))]
fn should_defer_stream_draw(
    worker_active: bool,
    worker_finished: bool,
    user_input_pending: bool,
    elapsed_since_draw: Duration,
    stream_interval: Duration,
) -> bool {
    worker_active && !worker_finished && !user_input_pending && elapsed_since_draw < stream_interval
}

#[cfg(feature = "ratatui-ui")]
fn input_mode_fingerprint(state: &AppState) -> (bool, bool, bool, bool, bool) {
    (
        state.is_text_input_active(),
        state.active_prompt().is_some(),
        state.pending_confirm().is_some(),
        state.palette_active(),
        state.help_active(),
    )
}

fn describe_outcome_label(outcome: &TuiOutcome) -> String {
    match outcome {
        TuiOutcome::Exit => "退出 debopty".to_string(),
        TuiOutcome::Refresh => "刷新系统状态".to_string(),
        TuiOutcome::RunNativeF2bSync => "Fail2ban 公网端口同步".to_string(),
        TuiOutcome::RunNativeReconcile => "系统冲突巡检 (只读审计)".to_string(),
        TuiOutcome::ElevatedApproved(inner) => describe_outcome_label(inner),
        TuiOutcome::Execute(action) => match action {
            BashAction::RunBaseOptimization => "基础系统优化 (BBR + SSH + 防火墙)".to_string(),
            BashAction::ToggleIpForwarding => "切换 IPv4/IPv6 路由转发".to_string(),
            BashAction::TogglePodmanGc => "切换 Podman 定时镜像清理".to_string(),
            BashAction::GenerateWarpXray => "生成 WARP Xray WireGuard 出站".to_string(),
            BashAction::ScriptUpdate => "检查并更新 debopty".to_string(),
            BashAction::ScriptUninstall => "卸载 debopty".to_string(),
            BashAction::InstallApp(AppId::Freship) => "加载 FreshIP 节点地区列表".to_string(),
            BashAction::InstallApp(app) => format!("安装 {}", app.display_name()),
            BashAction::UninstallApp(app) => format!("卸载 {}", app.display_name()),
            BashAction::ManageXray => "Xray Core 管理".to_string(),
            BashAction::ManageFreship => "FreshIP 管理".to_string(),
            BashAction::ManageLego => "Lego 证书管理".to_string(),
            BashAction::LegacyFullEntry => "debopty".to_string(),
        },
        TuiOutcome::RunMenuAction(act) => match act {
            MenuAction::XrayToggleRuleset => "切换 Xray 路由规则集".to_string(),
            MenuAction::XrayToggleCron => "切换 Xray Geo 自动更新 Cron".to_string(),
            MenuAction::FreshipReconfigure => "加载 FreshIP 节点地区列表".to_string(),
            MenuAction::FreshipToggleTimers => "切换 FreshIP 定时养护任务".to_string(),
            MenuAction::FreshipSync => "立即同步 FreshIP 热点数据".to_string(),
            MenuAction::FreshipLogs => "读取 FreshIP 近期运行日志".to_string(),
            MenuAction::LegoInstall => "安装 / 升级 Lego ACME".to_string(),
            MenuAction::LegoToggleFlag => "加载 Lego 托管域名列表".to_string(),
            MenuAction::LegoRunOnce => "加载 Lego 托管域名列表".to_string(),
            MenuAction::LegoTroubleshoot => "加载 Lego 托管域名列表".to_string(),
            MenuAction::LegoRemoveDomain => "加载 Lego 托管域名列表".to_string(),
            MenuAction::LegoUninstall => "卸载 Lego ACME".to_string(),
            _ => "执行任务".to_string(),
        },
        TuiOutcome::PromptSubmitted(sub) => match sub {
            PromptSubmission::YaziInstall { .. } => "安装 Yazi 文件管理器".to_string(),
            PromptSubmission::PodmanUninstall { .. } => "卸载 Podman 容器环境".to_string(),
            PromptSubmission::ScriptUninstall { .. } => "卸载 debopty".to_string(),
            PromptSubmission::ReconcileApply => "应用系统稳态收敛".to_string(),
            PromptSubmission::Freship {
                is_reconfigure: true,
                ..
            } => "重新配置 FreshIP 节点与双栈模式".to_string(),
            PromptSubmission::Freship {
                is_reconfigure: false,
                ..
            } => "安装 FreshIP IP 养护服务".to_string(),
            PromptSubmission::LegoAddDomain(spec) => {
                format!("添加 Lego 域名托管 ({})", spec.primary_domain)
            }
            PromptSubmission::LegoToggleFlag { domain, .. } => {
                format!("切换 Lego 域名开关 ({domain})")
            }
            PromptSubmission::LegoRunOnce { domain, mode } => {
                format!("执行 Lego 证书任务 ({domain} · {mode})")
            }
            PromptSubmission::LegoTroubleshoot { domain } => {
                format!("生成 Lego 排障命令 ({domain})")
            }
            PromptSubmission::LegoRemoveDomain { domain } => {
                format!("移除 Lego 域名托管 ({domain})")
            }
        },
    }
}

fn bash_action_requires_root(action: BashAction) -> bool {
    match action {
        BashAction::LegacyFullEntry => false,
        BashAction::RunBaseOptimization
        | BashAction::ToggleIpForwarding
        | BashAction::TogglePodmanGc
        | BashAction::InstallApp(_)
        | BashAction::UninstallApp(_)
        | BashAction::ManageLego
        | BashAction::ManageXray
        | BashAction::GenerateWarpXray
        | BashAction::ManageFreship
        | BashAction::ScriptUpdate
        | BashAction::ScriptUninstall => true,
    }
}

fn menu_action_requires_root(action: &MenuAction) -> bool {
    match action {
        MenuAction::Navigate(_)
        | MenuAction::Back
        | MenuAction::Exit
        | MenuAction::NativeReconcile
        | MenuAction::FreshipLogs
        | MenuAction::LegoTroubleshoot => false,
        MenuAction::Execute(act) | MenuAction::ExecuteWithConfirm { action: act, .. } => {
            bash_action_requires_root(*act)
        }
        MenuAction::NativeF2bSync
        | MenuAction::XrayToggleRuleset
        | MenuAction::XrayToggleCron
        | MenuAction::FreshipReconfigure
        | MenuAction::FreshipToggleTimers
        | MenuAction::FreshipSync
        | MenuAction::LegoInstall
        | MenuAction::LegoAddDomain
        | MenuAction::LegoToggleFlag
        | MenuAction::LegoRunOnce
        | MenuAction::LegoRemoveDomain
        | MenuAction::LegoUninstall => true,
    }
}

fn prompt_submission_requires_root(sub: &PromptSubmission) -> bool {
    match sub {
        PromptSubmission::LegoTroubleshoot { .. } => false,
        PromptSubmission::YaziInstall { .. }
        | PromptSubmission::PodmanUninstall { .. }
        | PromptSubmission::ScriptUninstall { .. }
        | PromptSubmission::ReconcileApply
        | PromptSubmission::Freship { .. }
        | PromptSubmission::LegoAddDomain(_)
        | PromptSubmission::LegoToggleFlag { .. }
        | PromptSubmission::LegoRunOnce { .. }
        | PromptSubmission::LegoRemoveDomain { .. } => true,
    }
}

/// Exhaustively classifies whether executing a `TuiOutcome` requires root privileges.
pub(crate) fn outcome_requires_root(outcome: &TuiOutcome) -> bool {
    match outcome {
        TuiOutcome::Exit
        | TuiOutcome::Refresh
        | TuiOutcome::RunNativeReconcile
        | TuiOutcome::ElevatedApproved(_) => false,
        TuiOutcome::RunNativeF2bSync => true,
        TuiOutcome::Execute(action) => bash_action_requires_root(*action),
        TuiOutcome::RunMenuAction(act) => menu_action_requires_root(act),
        TuiOutcome::PromptSubmitted(sub) => prompt_submission_requires_root(sub),
    }
}

const MAX_TUI_OUTCOME_BYTES: u64 = 1024 * 1024;
static TEMP_OUTCOME_COUNTER: AtomicU64 = AtomicU64::new(0);

struct SecureTempOutcomeFile {
    path: PathBuf,
}

impl SecureTempOutcomeFile {
    fn create(payload: &[u8]) -> io::Result<Self> {
        let pid = std::process::id();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir();
        for _ in 0..32 {
            let seq = TEMP_OUTCOME_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = dir.join(format!("debopty-tui-outcome-{pid}-{nanos}-{seq}.json"));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(mut f) => {
                    f.write_all(payload)?;
                    f.flush()?;
                    return Ok(Self { path });
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "failed to allocate unique temp outcome file",
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SecureTempOutcomeFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn run_elevated_tui_outcome(
    outcome: &TuiOutcome,
    config_path: &Path,
    template_dir: Option<&Path>,
) -> (NoticeKind, String) {
    let label = describe_outcome_label(outcome);
    let payload = match serde_json::to_vec(outcome) {
        Ok(bytes) => bytes,
        Err(err) => {
            return (NoticeKind::Danger, format!("序列化提权操作载荷失败: {err}"));
        }
    };
    let tmp = match SecureTempOutcomeFile::create(&payload) {
        Ok(f) => f,
        Err(err) => {
            return (
                NoticeKind::Danger,
                format!("创建临时提权载荷文件失败: {err}"),
            );
        }
    };
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(err) => {
            return (
                NoticeKind::Danger,
                format!("定位当前 debopty 可执行文件失败: {err}"),
            );
        }
    };

    let mut cmd = Command::new("sudo");
    cmd.arg("--").arg(&exe).arg("--config").arg(config_path);
    if let Some(dir) = template_dir {
        cmd.arg("--template-dir").arg(dir);
    }
    cmd.arg("_tui-exec-outcome")
        .arg("--file")
        .arg(tmp.path())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    match cmd.status() {
        Ok(status) if status.success() => (NoticeKind::Success, format!("已提权完成: {label}")),
        Ok(status) => match status.code() {
            Some(130) | None => (
                NoticeKind::Warning,
                format!("已取消或中断提权操作: {label}"),
            ),
            Some(code) => (
                NoticeKind::Warning,
                format!("提权执行未成功 (退出码 {code}): {label}"),
            ),
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => (
            NoticeKind::Danger,
            "未找到 sudo 命令，请使用 root 账户重新运行 debopty".to_string(),
        ),
        Err(err) => (NoticeKind::Danger, format!("调用 sudo 提权失败: {err}")),
    }
}

/// Hidden internal trampoline executed under `sudo` to run a serialized `TuiOutcome`.
pub fn handle_tui_exec_outcome(
    file: &Path,
    config_path: &Path,
    template_dir: Option<&Path>,
) -> Result<i32, CliError> {
    let is_root = unsafe { libc::geteuid() == 0 };
    if !is_root && !env_truthy("DEBOPTY_ALLOW_UNPRIVILEGED_TUI_EXEC") {
        return Err(CliError::Usage(
            "内部提权执行入口 `_tui-exec-outcome` 需要管理员权限 (root)。".to_string(),
        ));
    }

    let meta = fs::metadata(file)?;
    if meta.len() > MAX_TUI_OUTCOME_BYTES {
        return Err(CliError::Usage(format!(
            "提权载荷文件过大 ({} bytes > {} bytes 限制)。",
            meta.len(),
            MAX_TUI_OUTCOME_BYTES
        )));
    }
    let bytes = fs::read(file)?;
    let outcome: TuiOutcome = serde_json::from_slice(&bytes)
        .map_err(|err| CliError::Usage(format!("解析提权载荷 JSON 失败: {err}")))?;

    if matches!(outcome, TuiOutcome::ElevatedApproved(_)) {
        return Err(CliError::Usage(
            "拒绝嵌套的 ElevatedApproved 提权载荷。".to_string(),
        ));
    }

    let runner = RealCommandRunner;
    let store = ConfigStore::new(config_path);
    let cfg = store.load().unwrap_or_default();
    let id = version::compiled_identity();
    let initial_status = probe_status(&cfg, &id, &runner);
    let mut state = AppState::new(initial_status);

    match outcome {
        TuiOutcome::Exit | TuiOutcome::Refresh => return Ok(0),
        TuiOutcome::ElevatedApproved(_) => unreachable!("rejected above"),
        TuiOutcome::Execute(action) => {
            execute_tui_action(action, &mut state, config_path, template_dir, &runner);
        }
        TuiOutcome::RunMenuAction(menu_action) => {
            execute_console_menu_action(
                menu_action,
                &mut state,
                config_path,
                template_dir,
                &runner,
            );
        }
        TuiOutcome::PromptSubmitted(submission) => {
            execute_console_prompt_submission(
                submission,
                &mut state,
                config_path,
                template_dir,
                &runner,
            );
        }
        TuiOutcome::RunNativeF2bSync => {
            let opts = build_f2b_options(false, false, template_dir);
            let templates = TemplateEngine::new(template_dir.map(Path::to_path_buf));
            match run_f2b_sync(&opts, &templates, &runner) {
                Ok(report) => {
                    state.set_notice_with_kind(
                        NoticeKind::Success,
                        format!(
                            "端口同步完成 (SSH={}, 公网={})",
                            report.ssh_port, report.all_public_ports_f2b
                        ),
                    );
                }
                Err(err) => {
                    state.set_notice_with_kind(
                        NoticeKind::Danger,
                        format!("Fail2ban 端口同步失败: {err}"),
                    );
                }
            }
        }
        TuiOutcome::RunNativeReconcile => {
            let mut ctx = ReconcileContext::with_root("/");
            ctx.config_path = config_path.to_path_buf();
            ctx.template_dir = template_dir.map(Path::to_path_buf);
            ctx.is_cn_region = state.status.is_cn_region;
            ctx.f2b_backend_override = non_empty_env("DEBOPTY_F2B_BACKEND");
            match audit_system(&ctx, &runner) {
                Ok(audit) => {
                    print_reconcile_human(&audit);
                    state.set_notice_with_kind(
                        NoticeKind::Success,
                        format!(
                            "巡检完成 · 漂移 {} · 保留 {}",
                            audit.drift_count, audit.preserved_count
                        ),
                    );
                }
                Err(err) => {
                    state.set_notice_with_kind(
                        NoticeKind::Danger,
                        format!("系统冲突巡检失败 [{}]: {err}", err.error_code()),
                    );
                }
            }
        }
    }

    if let Some(msg) = state.notice() {
        let _ = writeln_safe(msg);
    }
    if state.notice_kind() == NoticeKind::Danger {
        Ok(1)
    } else {
        Ok(0)
    }
}

#[cfg(any(feature = "ratatui-ui", test))]
fn format_reconcile_severity(sev: FindingSeverity) -> &'static str {
    match sev {
        FindingSeverity::Critical => "CRIT",
        FindingSeverity::Warning => "WARN",
        FindingSeverity::Info => "INFO",
    }
}

#[cfg(any(feature = "ratatui-ui", test))]
fn format_reconcile_status(st: FindingStatus) -> &'static str {
    match st {
        FindingStatus::Clean => "正常",
        FindingStatus::PreservedUserCustom => "已保留用户配置",
        FindingStatus::WouldFix => "待修复",
        FindingStatus::Fixed => "已修复",
        FindingStatus::Quarantined => "已隔离",
        FindingStatus::SkippedNoSys => "已降级跳过",
    }
}

#[cfg(any(feature = "ratatui-ui", test))]
fn stream_reconcile_report(report: &ReconcileReport, tx: &mpsc::Sender<WorkerMsg>) {
    let mode_str = match report.mode {
        ReconcileMode::Audit => "只读巡检 (Audit)",
        ReconcileMode::Apply => "稳态收敛 (Apply)",
    };
    let _ = tx.send(WorkerMsg::LogLine(format!(
        "=== 系统冲突巡检与稳态接管 [{mode_str}] ==="
    )));
    let _ = tx.send(WorkerMsg::LogLine(format!(
        "冲突/漂移项: {} | 已变更项: {} | 已保留用户自定义项: {}",
        report.drift_count, report.changes_made, report.preserved_count
    )));
    if let Some(ref bak) = report.backup_dir {
        let _ = tx.send(WorkerMsg::LogLine(format!(
            "原子备份目录: {}",
            bak.display()
        )));
    }
    if report.findings.is_empty() {
        let _ = tx.send(WorkerMsg::LogLine(
            "系统状态干净，未检测到冲突或漂移。".to_string(),
        ));
        return;
    }
    for f in &report.findings {
        let _ = tx.send(WorkerMsg::LogLine(format!(
            "  [{}][{}] ({}) {} — {}",
            format_reconcile_severity(f.severity),
            format_reconcile_status(f.status),
            f.domain,
            f.target,
            f.summary
        )));
    }
}

#[cfg(any(feature = "ratatui-ui", test))]
fn stream_app_result(
    label: &str,
    res: debopty_core::Result<AppReport>,
    tx: &mpsc::Sender<WorkerMsg>,
) -> (bool, String, NoticeKind) {
    match res {
        Ok(report) => {
            for (k, v) in &report.details {
                let _ = tx.send(WorkerMsg::LogLine(format!("  {k}: {v}")));
            }
            for w in &report.warnings {
                let _ = tx.send(WorkerMsg::LogLine(format!("  [warn] {w}")));
            }
            for p in &report.preserved_user_custom {
                let _ = tx.send(WorkerMsg::LogLine(format!("  [preserved] {p}")));
            }
            (true, report.summary, NoticeKind::Success)
        }
        Err(err) => {
            let msg = format!("{label}失败 [{}]: {err}", err.error_code());
            let _ = tx.send(WorkerMsg::LogLine(format!("  [error] {msg}")));
            (false, msg, NoticeKind::Danger)
        }
    }
}

#[cfg(any(feature = "ratatui-ui", test))]
fn probe_worker_snapshot(
    config_path: &Path,
    runner: &dyn CommandRunner,
) -> (Option<StatusSnapshot>, SystemSpecs) {
    let store = ConfigStore::new(config_path);
    let id = version::compiled_identity();
    let status = store.load().ok().map(|cfg| probe_status(&cfg, &id, runner));
    let specs = probe_system_specs(Path::new("/"));
    (status, specs)
}

#[cfg(any(feature = "ratatui-ui", test))]
fn build_worker_app_context(
    config_path: &Path,
    template_dir: Option<&Path>,
    is_cn_region: bool,
) -> AppContext {
    AppContext {
        config_path: config_path.to_path_buf(),
        template_dir: template_dir.map(Path::to_path_buf),
        is_cn_region,
        non_interactive: true,
        ..AppContext::default()
    }
}

#[cfg(any(feature = "ratatui-ui", test))]
fn build_worker_system_context(
    config_path: &Path,
    template_dir: Option<&Path>,
    is_cn_region: bool,
) -> SystemContext {
    SystemContext {
        root_prefix: PathBuf::from("/"),
        is_cn_region,
        non_interactive: true,
        no_sys: false,
        config_path: config_path.to_path_buf(),
        template_dir: template_dir.map(Path::to_path_buf),
        tmux_conf_override: None,
        tmux_skip_apt: false,
        tmux_version_override: None,
    }
}

#[cfg(any(feature = "ratatui-ui", test))]
fn execute_ratatui_worker(
    outcome: TuiOutcome,
    config_path: PathBuf,
    template_dir: Option<PathBuf>,
    is_cn_region: bool,
    tx: mpsc::Sender<WorkerMsg>,
) {
    let tx_sink = tx.clone();
    let sink: LineSink = Arc::new(move |line: &str| {
        let _ = tx_sink.send(WorkerMsg::LogLine(line.to_string()));
    });
    let _log_guard = diag::install_thread_log_sink(Arc::clone(&sink), LogLevel::Info);
    let _cmd_guard = sys::install_thread_command_sink(Arc::clone(&sink));
    let runner = StreamingCommandRunner::new(Arc::clone(&sink));
    let http = UreqHttpClient::new();

    if matches!(outcome, TuiOutcome::Refresh) {
        let (status, specs) = probe_worker_snapshot(&config_path, &runner);
        let _ = tx.send(WorkerMsg::Refreshed { status, specs });
        return;
    }

    let label = describe_outcome_label(&outcome);
    emit_progress("debopty::tui", &format!("开始执行: {label}"));

    let app_ctx = build_worker_app_context(&config_path, template_dir.as_deref(), is_cn_region);
    let sys_ctx = build_worker_system_context(&config_path, template_dir.as_deref(), is_cn_region);

    let (succeeded, summary, notice_kind) = match outcome {
        TuiOutcome::Exit | TuiOutcome::Refresh | TuiOutcome::ElevatedApproved(_) => {
            unreachable!("handled above")
        }
        TuiOutcome::Execute(BashAction::InstallApp(AppId::Freship)) => {
            emit_progress("debopty::tui", "正在拉取 FreshIP 节点地区列表...");
            match fetch_freship_map(&app_ctx, &http) {
                Ok((map, used_cache)) => {
                    let _ = tx.send(WorkerMsg::FreshipMapLoaded {
                        map,
                        used_cache,
                        is_reconfigure: false,
                    });
                    return;
                }
                Err(err) => {
                    let msg = format!("获取 FreshIP 地区列表失败 [{}]: {err}", err.error_code());
                    let _ = tx.send(WorkerMsg::LogLine(format!("  [error] {msg}")));
                    let _ = tx.send(WorkerMsg::Finished {
                        succeeded: false,
                        summary: msg,
                        notice_kind: NoticeKind::Danger,
                        status: None,
                        specs: None,
                    });
                    return;
                }
            }
        }
        TuiOutcome::RunMenuAction(MenuAction::FreshipReconfigure) => {
            emit_progress("debopty::tui", "正在拉取 FreshIP 节点地区列表...");
            match fetch_freship_map(&app_ctx, &http) {
                Ok((map, used_cache)) => {
                    let _ = tx.send(WorkerMsg::FreshipMapLoaded {
                        map,
                        used_cache,
                        is_reconfigure: true,
                    });
                    return;
                }
                Err(err) => {
                    let msg = format!("获取 FreshIP 地区列表失败 [{}]: {err}", err.error_code());
                    let _ = tx.send(WorkerMsg::LogLine(format!("  [error] {msg}")));
                    let _ = tx.send(WorkerMsg::Finished {
                        succeeded: false,
                        summary: msg,
                        notice_kind: NoticeKind::Danger,
                        status: None,
                        specs: None,
                    });
                    return;
                }
            }
        }
        TuiOutcome::RunMenuAction(
            act @ (MenuAction::LegoToggleFlag
            | MenuAction::LegoRunOnce
            | MenuAction::LegoTroubleshoot
            | MenuAction::LegoRemoveDomain),
        ) => {
            let kind = match act {
                MenuAction::LegoToggleFlag => PickerKind::LegoToggleFlagDomain,
                MenuAction::LegoRunOnce => PickerKind::LegoRunOnceDomain,
                MenuAction::LegoTroubleshoot => PickerKind::LegoTroubleshootDomain,
                MenuAction::LegoRemoveDomain => PickerKind::LegoRemoveDomain,
                _ => unreachable!("matched above"),
            };
            emit_progress("debopty::tui", "正在扫描已托管的 Lego 域名列表...");
            match list_lego_domains(&app_ctx, &runner) {
                Ok(domains) => {
                    let _ = tx.send(WorkerMsg::LegoDomainsLoaded { kind, domains });
                    return;
                }
                Err(err) => {
                    let msg = format!("读取 Lego 域名列表失败 [{}]: {err}", err.error_code());
                    let _ = tx.send(WorkerMsg::LogLine(format!("  [error] {msg}")));
                    let _ = tx.send(WorkerMsg::Finished {
                        succeeded: false,
                        summary: msg,
                        notice_kind: NoticeKind::Danger,
                        status: None,
                        specs: None,
                    });
                    return;
                }
            }
        }
        TuiOutcome::RunNativeReconcile => {
            let mut ctx = ReconcileContext::with_root("/");
            ctx.config_path = config_path.clone();
            ctx.template_dir = template_dir.clone();
            ctx.is_cn_region = is_cn_region;
            ctx.f2b_backend_override = non_empty_env("DEBOPTY_F2B_BACKEND");
            match audit_system(&ctx, &runner) {
                Ok(audit) => {
                    stream_reconcile_report(&audit, &tx);
                    let (status, specs) = probe_worker_snapshot(&config_path, &runner);
                    let _ = tx.send(WorkerMsg::ReconcileAuditDone {
                        drift_count: audit.drift_count,
                        preserved_count: audit.preserved_count,
                        status,
                        specs,
                    });
                    return;
                }
                Err(err) => {
                    let msg = format!("系统冲突巡检失败 [{}]: {err}", err.error_code());
                    let _ = tx.send(WorkerMsg::LogLine(format!("  [error] {msg}")));
                    (false, msg, NoticeKind::Danger)
                }
            }
        }
        TuiOutcome::RunNativeF2bSync => {
            let opts = build_f2b_options(false, false, template_dir.as_deref());
            let templates = TemplateEngine::new(template_dir.clone());
            match run_f2b_sync(&opts, &templates, &runner) {
                Ok(report) => {
                    let summary = format!(
                        "端口同步完成 (SSH={}, 公网={})",
                        report.ssh_port, report.all_public_ports_f2b
                    );
                    let _ = tx.send(WorkerMsg::LogLine(format!("✓ {summary}")));
                    (true, summary, NoticeKind::Success)
                }
                Err(err) => {
                    let msg = format!("Fail2ban 端口同步失败: {err}");
                    let _ = tx.send(WorkerMsg::LogLine(format!("  [error] {msg}")));
                    (false, msg, NoticeKind::Danger)
                }
            }
        }
        TuiOutcome::Execute(action) => match action {
            BashAction::ToggleIpForwarding => match toggle_ip_forwarding(&sys_ctx, &runner) {
                Ok(enabled) => {
                    let msg = if enabled {
                        "路由转发已开启".to_string()
                    } else {
                        "路由转发已关闭".to_string()
                    };
                    let _ = tx.send(WorkerMsg::LogLine(format!("✓ {msg}")));
                    (true, msg, NoticeKind::Success)
                }
                Err(err) => {
                    let msg = format!("切换路由转发失败: {err}");
                    let _ = tx.send(WorkerMsg::LogLine(format!("  [error] {msg}")));
                    (false, msg, NoticeKind::Danger)
                }
            },
            BashAction::InstallApp(AppId::Tmux) => match setup_tmux(&sys_ctx, &runner) {
                Ok(path) => {
                    let msg = format!("Tmux 已部署: {}", path.display());
                    let _ = tx.send(WorkerMsg::LogLine(format!("✓ {msg}")));
                    (true, msg, NoticeKind::Success)
                }
                Err(err) => {
                    let msg = format!("部署 Tmux 失败: {err}");
                    let _ = tx.send(WorkerMsg::LogLine(format!("  [error] {msg}")));
                    (false, msg, NoticeKind::Danger)
                }
            },
            BashAction::UninstallApp(AppId::Tmux) => match uninstall_tmux(&sys_ctx, &runner) {
                Ok(()) => {
                    let msg = "Tmux 已卸载".to_string();
                    let _ = tx.send(WorkerMsg::LogLine(format!("✓ {msg}")));
                    (true, msg, NoticeKind::Success)
                }
                Err(err) => {
                    let msg = format!("卸载 Tmux 失败: {err}");
                    let _ = tx.send(WorkerMsg::LogLine(format!("  [error] {msg}")));
                    (false, msg, NoticeKind::Danger)
                }
            },
            BashAction::RunBaseOptimization => {
                match run_base_optimization(&sys_ctx, &http, &runner) {
                    Ok(report) => {
                        let msg = format!(
                            "基础优化完成 ({})",
                            if report.is_cn_region {
                                "CN · 镜像"
                            } else {
                                "海外 · 直连"
                            }
                        );
                        let _ = tx.send(WorkerMsg::LogLine(format!("✓ {msg}")));
                        (true, msg, NoticeKind::Success)
                    }
                    Err(err) => {
                        let msg = format!("基础优化失败: {err}");
                        let _ = tx.send(WorkerMsg::LogLine(format!("  [error] {msg}")));
                        (false, msg, NoticeKind::Danger)
                    }
                }
            }
            BashAction::InstallApp(AppId::Yazi) => stream_app_result(
                "安装 Yazi",
                install_yazi(
                    &app_ctx,
                    &YaziInstallOptions {
                        media_preview: Some(false),
                    },
                    &http,
                    &runner,
                ),
                &tx,
            ),
            BashAction::InstallApp(app) => {
                let res = match app {
                    AppId::Fish => install_fish(&app_ctx, &http, &runner),
                    AppId::Micro => install_micro(&app_ctx, &http, &runner),
                    AppId::Podman => install_podman(&app_ctx, &runner),
                    AppId::RustSdk => install_rust(&app_ctx, &http, &runner),
                    AppId::Realm => install_realm(&app_ctx, &http, &runner),
                    AppId::Ferron => install_ferron(&app_ctx, &http, &runner),
                    AppId::GoSdk => install_go(&app_ctx, &http, &runner),
                    AppId::Caddy => install_caddy(&app_ctx, &http, &runner),
                    AppId::Derper => install_derper(&app_ctx, &http, &runner),
                    AppId::Xray => install_xray(&app_ctx, &http, &runner),
                    AppId::Warp => install_warp(&app_ctx, &http, &runner),
                    AppId::Usque => install_usque(&app_ctx, &http, &runner),
                    AppId::Easytier => install_easytier(&app_ctx, &http, &runner),
                    AppId::Tailscale => install_tailscale(&app_ctx, &http, &runner),
                    AppId::Yazi | AppId::Freship | AppId::Tmux => {
                        unreachable!("handled above")
                    }
                };
                stream_app_result(&format!("安装 {}", app.display_name()), res, &tx)
            }
            BashAction::UninstallApp(AppId::Podman) => stream_app_result(
                "卸载 Podman",
                uninstall_podman(
                    &app_ctx,
                    &PodmanUninstallOptions {
                        delete_data: false,
                        delete_user: false,
                    },
                    &runner,
                ),
                &tx,
            ),
            BashAction::UninstallApp(app) => {
                let res = match app {
                    AppId::Fish => uninstall_fish(&app_ctx, &runner),
                    AppId::Micro => uninstall_micro(&app_ctx, &runner),
                    AppId::Yazi => uninstall_yazi(&app_ctx, &runner),
                    AppId::RustSdk => uninstall_rust(&app_ctx, &runner),
                    AppId::Realm => uninstall_realm(&app_ctx, &runner),
                    AppId::Ferron => uninstall_ferron(&app_ctx, &runner),
                    AppId::GoSdk => uninstall_go(&app_ctx, &runner),
                    AppId::Caddy => uninstall_caddy(&app_ctx, &runner),
                    AppId::Derper => uninstall_derper(&app_ctx, &runner),
                    AppId::Xray => uninstall_xray(&app_ctx, &http, &runner),
                    AppId::Warp => uninstall_warp(&app_ctx, &runner),
                    AppId::Usque => uninstall_usque(&app_ctx, &runner),
                    AppId::Easytier => uninstall_easytier(&app_ctx, &http, &runner),
                    AppId::Tailscale => uninstall_tailscale(&app_ctx, &runner),
                    AppId::Freship => uninstall_freship(&app_ctx, &runner),
                    AppId::Podman | AppId::Tmux => unreachable!("handled above"),
                };
                stream_app_result(&format!("卸载 {}", app.display_name()), res, &tx)
            }
            BashAction::TogglePodmanGc => {
                stream_app_result("切换 Podman GC", toggle_podman_gc(&app_ctx, &runner), &tx)
            }
            BashAction::GenerateWarpXray => match generate_warp_xray(&app_ctx, &http, &runner) {
                Ok(out) => {
                    let _ = tx.send(WorkerMsg::LogLine(
                        "=== Cloudflare WARP Xray WireGuard Outbound ===".to_string(),
                    ));
                    for line in out.json.lines() {
                        let _ = tx.send(WorkerMsg::LogLine(line.to_string()));
                    }
                    (
                        true,
                        "WARP 出站 JSON 已生成".to_string(),
                        NoticeKind::Success,
                    )
                }
                Err(err) => {
                    let msg = format!("生成 WARP 配置失败 [{}]: {err}", err.error_code());
                    let _ = tx.send(WorkerMsg::LogLine(format!("  [error] {msg}")));
                    (false, msg, NoticeKind::Danger)
                }
            },
            BashAction::ScriptUpdate => stream_app_result(
                "升级 debopty",
                self_update(&app_ctx, &SelfUpdateOptions::default(), &http, &runner),
                &tx,
            ),
            BashAction::ScriptUninstall => stream_app_result(
                "卸载 debopty",
                self_uninstall(
                    &app_ctx,
                    &SelfUninstallOptions {
                        keep_config: true,
                        install_dir: None,
                    },
                    &runner,
                ),
                &tx,
            ),
            BashAction::ManageXray
            | BashAction::ManageFreship
            | BashAction::ManageLego
            | BashAction::LegacyFullEntry => (true, "已就绪".to_string(), NoticeKind::Info),
        },
        TuiOutcome::RunMenuAction(act) => match act {
            MenuAction::XrayToggleRuleset => stream_app_result(
                "切换 Xray 规则集",
                toggle_xray_ruleset(&app_ctx, &http, &runner),
                &tx,
            ),
            MenuAction::XrayToggleCron => {
                stream_app_result("切换 Xray Cron", toggle_xray_cron(&app_ctx, &runner), &tx)
            }
            MenuAction::FreshipToggleTimers => stream_app_result(
                "切换 FreshIP 定时器",
                toggle_freship_timers(&app_ctx, None, &runner),
                &tx,
            ),
            MenuAction::FreshipSync => stream_app_result(
                "同步 FreshIP 数据",
                sync_freship_data(&app_ctx, &runner),
                &tx,
            ),
            MenuAction::FreshipLogs => {
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
                    let _ = tx.send(WorkerMsg::LogLine("暂无 FreshIP 运行日志。".to_string()));
                } else {
                    for line in lines {
                        let _ = tx.send(WorkerMsg::LogLine(line));
                    }
                }
                (true, "FreshIP 运行日志已加载".to_string(), NoticeKind::Info)
            }
            MenuAction::LegoInstall => {
                stream_app_result("安装 Lego", install_lego(&app_ctx, &http, &runner), &tx)
            }
            MenuAction::LegoUninstall => {
                stream_app_result("卸载 Lego", uninstall_lego(&app_ctx, &runner), &tx)
            }
            _ => (true, "操作已就绪".to_string(), NoticeKind::Info),
        },
        TuiOutcome::PromptSubmitted(sub) => match sub {
            PromptSubmission::YaziInstall { media_preview } => stream_app_result(
                "安装 Yazi",
                install_yazi(
                    &app_ctx,
                    &YaziInstallOptions {
                        media_preview: Some(media_preview),
                    },
                    &http,
                    &runner,
                ),
                &tx,
            ),
            PromptSubmission::PodmanUninstall {
                delete_data,
                delete_user,
            } => stream_app_result(
                "卸载 Podman",
                uninstall_podman(
                    &app_ctx,
                    &PodmanUninstallOptions {
                        delete_data,
                        delete_user,
                    },
                    &runner,
                ),
                &tx,
            ),
            PromptSubmission::ScriptUninstall { delete_config } => stream_app_result(
                "卸载 debopty",
                self_uninstall(
                    &app_ctx,
                    &SelfUninstallOptions {
                        keep_config: !delete_config,
                        install_dir: None,
                    },
                    &runner,
                ),
                &tx,
            ),
            PromptSubmission::ReconcileApply => {
                let mut ctx = ReconcileContext::with_root("/");
                ctx.config_path = config_path.clone();
                ctx.template_dir = template_dir.clone();
                ctx.is_cn_region = is_cn_region;
                ctx.f2b_backend_override = non_empty_env("DEBOPTY_F2B_BACKEND");
                match reconcile_system(&ctx, ReconcileMode::Apply, &runner) {
                    Ok(applied) => {
                        stream_reconcile_report(&applied, &tx);
                        let bak_hint = applied
                            .backup_dir
                            .as_ref()
                            .map(|p| format!(" · 备份 {}", p.display()))
                            .unwrap_or_default();
                        (
                            true,
                            format!(
                                "收敛完成 · 修复 {} · 保留 {}{bak_hint}",
                                applied.changes_made, applied.preserved_count
                            ),
                            NoticeKind::Success,
                        )
                    }
                    Err(err) => {
                        let msg = format!("稳态收敛失败 [{}]: {err}", err.error_code());
                        let _ = tx.send(WorkerMsg::LogLine(format!("  [error] {msg}")));
                        (false, msg, NoticeKind::Danger)
                    }
                }
            }
            PromptSubmission::Freship {
                selection,
                is_reconfigure,
            } => {
                let res = if is_reconfigure {
                    reconfigure_freship(&app_ctx, Some(selection), &http, &runner)
                } else {
                    install_freship(&app_ctx, Some(selection), &http, &runner)
                };
                stream_app_result(
                    if is_reconfigure {
                        "重配 FreshIP"
                    } else {
                        "安装 FreshIP"
                    },
                    res,
                    &tx,
                )
            }
            PromptSubmission::LegoAddDomain(spec) => stream_app_result(
                "添加 Lego 域名",
                add_lego_domain(&app_ctx, &spec, &runner),
                &tx,
            ),
            PromptSubmission::LegoToggleFlag { domain, flag } => stream_app_result(
                "切换 Lego 开关",
                toggle_lego_flag(&app_ctx, &domain, flag, &runner),
                &tx,
            ),
            PromptSubmission::LegoRunOnce { domain, mode } => stream_app_result(
                "执行 Lego 证书任务",
                run_lego_once(&app_ctx, &domain, &mode, &runner),
                &tx,
            ),
            PromptSubmission::LegoTroubleshoot { domain } => {
                match build_lego_troubleshoot_cmd(&app_ctx, &domain) {
                    Ok(cmd) => {
                        let _ =
                            tx.send(WorkerMsg::LogLine("=== Lego 手动排障命令 ===".to_string()));
                        for line in cmd.lines() {
                            let _ = tx.send(WorkerMsg::LogLine(line.to_string()));
                        }
                        (
                            true,
                            format!("已生成 {domain} 手动排障命令"),
                            NoticeKind::Info,
                        )
                    }
                    Err(err) => {
                        let msg = format!("生成排障命令失败 [{}]: {err}", err.error_code());
                        let _ = tx.send(WorkerMsg::LogLine(format!("  [error] {msg}")));
                        (false, msg, NoticeKind::Danger)
                    }
                }
            }
            PromptSubmission::LegoRemoveDomain { domain } => {
                stream_app_result("移除域名托管", remove_lego_domain(&app_ctx, &domain), &tx)
            }
        },
    };

    let (status, specs) = probe_worker_snapshot(&config_path, &runner);
    let _ = tx.send(WorkerMsg::Finished {
        succeeded,
        summary,
        notice_kind,
        status,
        specs: Some(specs),
    });
}

#[cfg(any(feature = "ratatui-ui", test))]
fn spawn_ratatui_worker(
    outcome: TuiOutcome,
    config_path: &Path,
    template_dir: Option<&Path>,
    is_cn_region: bool,
) -> mpsc::Receiver<WorkerMsg> {
    let (tx, rx) = mpsc::channel();
    let cfg = config_path.to_path_buf();
    let tpl = template_dir.map(Path::to_path_buf);
    thread::spawn(move || {
        execute_ratatui_worker(outcome, cfg, tpl, is_cn_region, tx);
    });
    rx
}

/// Applies one worker message to `AppState`. Returns `true` when the worker has completed.
#[cfg(any(feature = "ratatui-ui", test))]
fn apply_worker_msg(state: &mut AppState, msg: WorkerMsg) -> bool {
    match msg {
        WorkerMsg::LogLine(line) => {
            state.push_task_log_line(&line);
            false
        }
        WorkerMsg::Refreshed { status, specs } => {
            if let Some(st) = status {
                state.refresh_status(st);
            }
            let history = state.telemetry_history().clone();
            state.set_telemetry(specs, history);
            state.set_notice_with_kind(NoticeKind::Success, "状态已刷新");
            true
        }
        WorkerMsg::FreshipMapLoaded {
            map,
            used_cache,
            is_reconfigure,
        } => {
            state.dismiss_task_run();
            if used_cache {
                state.set_notice_with_kind(
                    NoticeKind::Info,
                    "远程拉取失败，已使用本地离线缓存 map.json",
                );
            }
            state.open_freship_picker(map, is_reconfigure);
            true
        }
        WorkerMsg::LegoDomainsLoaded { kind, domains } => {
            state.dismiss_task_run();
            state.open_lego_domain_picker(kind, &domains);
            true
        }
        WorkerMsg::ReconcileAuditDone {
            drift_count,
            preserved_count,
            status,
            specs,
        } => {
            if let Some(st) = status {
                state.refresh_status(st);
            }
            let history = state.telemetry_history().clone();
            state.set_telemetry(specs, history);
            if drift_count == 0 {
                let summary = format!("系统稳态 · 保留 {preserved_count} 项自定义");
                state.finish_task_run(true, summary.clone());
                state.set_notice_with_kind(NoticeKind::Success, summary);
            } else {
                let summary = format!("巡检完成 · 发现 {drift_count} 项漂移");
                state.finish_task_run(true, summary);
                state.open_reconcile_apply_confirm(drift_count);
            }
            true
        }
        WorkerMsg::Finished {
            succeeded,
            summary,
            notice_kind,
            status,
            specs,
        } => {
            if let Some(st) = status {
                state.refresh_status(st);
            }
            if let Some(s) = specs {
                let history = state.telemetry_history().clone();
                state.set_telemetry(s, history);
            }
            state.finish_task_run(succeeded, summary.clone());
            state.set_notice_with_kind(notice_kind, summary);
            true
        }
    }
}

#[cfg(feature = "ratatui-ui")]
fn is_clean_disconnect(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::BrokenPipe | io::ErrorKind::UnexpectedEof
    )
}

/// Non-blocking Ratatui live loop with background worker threads and 0 TUI dropouts (U39/U41/U49).
#[cfg(feature = "ratatui-ui")]
fn run_ratatui_live_loop(
    renderer: &mut debopty_tui::RatatuiRenderer,
    state: &mut AppState,
    config_path: &Path,
    template_dir: Option<&Path>,
    is_root: bool,
) -> Result<(), CliError> {
    use debopty_tui::TuiRenderer;

    if renderer.is_non_interactive() {
        return Ok(());
    }

    let is_ssh = state.motion_policy().is_ssh;
    let mut worker_rx: Option<mpsc::Receiver<WorkerMsg>> = None;
    let mut event_buf: Vec<MenuEvent> = Vec::with_capacity(16);
    let mut last_draw_at = Instant::now();
    let mut user_input_pending = true;
    let mut wake_due = false;

    loop {
        let mut worker_finished = false;
        if let Some(rx) = worker_rx.as_ref() {
            loop {
                match rx.try_recv() {
                    Ok(msg) => {
                        if apply_worker_msg(state, msg) {
                            worker_finished = true;
                            break;
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        if state.is_task_running() {
                            state.finish_task_run(false, "后台任务连接已关闭");
                            state.set_notice_with_kind(NoticeKind::Danger, "后台任务异常中断");
                        }
                        worker_finished = true;
                        break;
                    }
                }
            }
            if worker_finished {
                worker_rx = None;
            }
        }

        if state.should_exit() {
            return Ok(());
        }

        let stream_interval = renderer.recommended_stream_interval(is_ssh);
        let wake_expired = wake_due || renderer.next_wake_timeout().is_some_and(|d| d.is_zero());
        let defer_draw = !wake_expired
            && should_defer_stream_draw(
                worker_rx.is_some(),
                worker_finished,
                user_input_pending,
                last_draw_at.elapsed(),
                stream_interval,
            );

        if !defer_draw {
            if state.take_dirty() {
                if let Err(err) = renderer.draw(state) {
                    if is_clean_disconnect(&err) {
                        return Ok(());
                    }
                    return Err(CliError::Io(err));
                }
                last_draw_at = Instant::now();
            }
            user_input_pending = false;
            wake_due = false;
        }
        renderer.sync_viewport(state);

        let remaining_stream = stream_interval.saturating_sub(last_draw_at.elapsed());
        let timeout = select_poll_timeout(
            worker_rx.is_some(),
            renderer.next_wake_timeout(),
            remaining_stream,
        );
        event_buf.clear();
        if let Err(err) = renderer.poll_event_batch(timeout, &mut event_buf) {
            if is_clean_disconnect(&err) {
                return Ok(());
            }
            return Err(CliError::Io(err));
        }
        if event_buf.is_empty() {
            continue;
        }

        let mut pending_outcome: Option<TuiOutcome> = None;
        for ev in event_buf.drain(..) {
            if matches!(ev, MenuEvent::Tick | MenuEvent::TelemetryTick) {
                wake_due = true;
            } else {
                user_input_pending = true;
            }
            renderer.sync_viewport(state);
            let mode_before = input_mode_fingerprint(state);
            if let Some(outcome) = state.handle_event(ev) {
                pending_outcome = Some(outcome);
                break;
            }
            if input_mode_fingerprint(state) != mode_before {
                break;
            }
        }

        let Some(outcome) = pending_outcome else {
            continue;
        };

        if matches!(outcome, TuiOutcome::Exit) {
            return Ok(());
        }

        if worker_rx.is_some() {
            state.set_notice_with_kind(
                NoticeKind::Warning,
                "当前有任务正在后台执行，请等待完成后再操作",
            );
            continue;
        }

        if let TuiOutcome::ElevatedApproved(inner) = outcome {
            if let Err(err) = renderer.suspend_for_child() {
                if is_clean_disconnect(&err) {
                    return Ok(());
                }
                return Err(CliError::Io(err));
            }
            let (notice_kind, notice_msg) =
                run_elevated_tui_outcome(&inner, config_path, template_dir);
            if let Err(err) = renderer.resume_after_child() {
                if is_clean_disconnect(&err) {
                    return Ok(());
                }
                return Err(CliError::Io(err));
            }
            let (refreshed_status, refreshed_specs) =
                probe_worker_snapshot(config_path, &RealCommandRunner);
            if let Some(st) = refreshed_status {
                state.refresh_status(st);
            }
            let history = state.telemetry_history().clone();
            state.set_telemetry(refreshed_specs, history);
            state.set_notice_with_kind(notice_kind, notice_msg);
            continue;
        }

        if !is_root && outcome_requires_root(&outcome) {
            let label = describe_outcome_label(&outcome);
            state.open_elevation_confirm(outcome, &label);
            continue;
        }

        if matches!(outcome, TuiOutcome::Refresh) {
            state.set_notice_with_kind(NoticeKind::Info, "正在刷新系统状态...");
        } else {
            state.start_task_run(describe_outcome_label(&outcome));
        }

        worker_rx = Some(spawn_ratatui_worker(
            outcome,
            config_path,
            template_dir,
            state.status.is_cn_region,
        ));
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
    let use_ratatui = should_use_ratatui(ui_kind, non_interactive)?;

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

    #[cfg(feature = "ratatui-ui")]
    if use_ratatui {
        state.set_native_prompts(true);
        let mut renderer = debopty_tui::RatatuiRenderer::new();
        run_ratatui_live_loop(
            &mut renderer,
            &mut state,
            config_path,
            template_dir.as_deref(),
            is_root,
        )?;
        return Ok(0);
    }
    #[cfg(not(feature = "ratatui-ui"))]
    let _ = use_ratatui;

    let mut renderer = ConsoleRenderer::new();
    run_tui_loop_with_state(&mut renderer, &mut state, &mut |outcome, st| {
        if let TuiOutcome::ElevatedApproved(inner) = outcome {
            let (kind, msg) =
                run_elevated_tui_outcome(&inner, config_path, template_dir.as_deref());
            st.set_notice_with_kind(kind, msg);
            if let Some(notice) = st.notice() {
                let _ = writeln_safe(&format!("\n{notice}"));
            }
            if !wait_enter_or_eof("\n执行完毕，按 Enter 返回菜单...") {
                st.request_exit();
                return Ok(());
            }
        } else if !is_root && outcome_requires_root(&outcome) {
            let label = describe_outcome_label(&outcome);
            let prompt = format!("操作「{label}」需要管理员权限，是否通过 sudo 提权执行？ [Y/n]: ");
            match prompt_line_checked(&prompt) {
                Some(ans)
                    if ans.is_empty()
                        || ans.eq_ignore_ascii_case("y")
                        || ans.eq_ignore_ascii_case("yes") =>
                {
                    let (kind, msg) =
                        run_elevated_tui_outcome(&outcome, config_path, template_dir.as_deref());
                    st.set_notice_with_kind(kind, msg);
                    if let Some(notice) = st.notice() {
                        let _ = writeln_safe(&format!("\n{notice}"));
                    }
                    if !wait_enter_or_eof("\n执行完毕，按 Enter 返回菜单...") {
                        st.request_exit();
                        return Ok(());
                    }
                }
                Some(_) => {
                    st.set_notice_with_kind(NoticeKind::Info, "已取消提权操作");
                }
                None => {
                    st.set_notice_with_kind(NoticeKind::Info, "已取消提权操作");
                    st.request_exit();
                    return Ok(());
                }
            }
        } else {
            match outcome {
                TuiOutcome::Exit | TuiOutcome::ElevatedApproved(_) => {}
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
                                        match reconcile_system(&ctx, ReconcileMode::Apply, &runner)
                                        {
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
                                                        applied.changes_made,
                                                        applied.preserved_count
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
                TuiOutcome::RunMenuAction(menu_action) => {
                    execute_console_menu_action(
                        menu_action,
                        st,
                        config_path,
                        template_dir.as_deref(),
                        &runner,
                    );
                    if st.should_exit() {
                        return Ok(());
                    }
                    if let Some(notice) = st.notice() {
                        let _ = writeln_safe(&format!("\n{notice}"));
                    }
                    if !wait_enter_or_eof("\n执行完毕，按 Enter 返回菜单...") {
                        st.request_exit();
                        return Ok(());
                    }
                }
                TuiOutcome::PromptSubmitted(submission) => {
                    execute_console_prompt_submission(
                        submission,
                        st,
                        config_path,
                        template_dir.as_deref(),
                        &runner,
                    );
                    if st.should_exit() {
                        return Ok(());
                    }
                    if let Some(notice) = st.notice() {
                        let _ = writeln_safe(&format!("\n{notice}"));
                    }
                    if !wait_enter_or_eof("\n执行完毕，按 Enter 返回菜单...") {
                        st.request_exit();
                        return Ok(());
                    }
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

    #[test]
    fn select_poll_timeout_active_vs_idle() {
        let local_stream = Duration::from_millis(33);
        let ssh_stream = Duration::from_millis(55);

        assert_eq!(
            select_poll_timeout(true, None, local_stream),
            Some(local_stream)
        );
        assert_eq!(
            select_poll_timeout(true, Some(Duration::from_secs(5)), local_stream),
            Some(local_stream)
        );
        assert_eq!(
            select_poll_timeout(true, Some(Duration::from_millis(16)), local_stream),
            Some(Duration::from_millis(16))
        );

        assert_eq!(
            select_poll_timeout(true, None, ssh_stream),
            Some(ssh_stream)
        );
        assert_eq!(
            select_poll_timeout(true, Some(Duration::from_secs(5)), ssh_stream),
            Some(ssh_stream)
        );
        assert_eq!(
            select_poll_timeout(true, Some(Duration::from_millis(33)), ssh_stream),
            Some(Duration::from_millis(33))
        );

        assert_eq!(select_poll_timeout(false, None, ssh_stream), None);
        assert_eq!(
            select_poll_timeout(false, Some(Duration::from_millis(45)), ssh_stream),
            Some(Duration::from_millis(45))
        );
    }

    #[test]
    fn should_defer_stream_draw_paces_worker_logs_only() {
        let ssh_stream = Duration::from_millis(55);

        assert!(should_defer_stream_draw(
            true,
            false,
            false,
            Duration::from_millis(10),
            ssh_stream
        ));
        assert!(should_defer_stream_draw(
            true,
            false,
            false,
            Duration::from_millis(54),
            ssh_stream
        ));
        assert!(!should_defer_stream_draw(
            true,
            false,
            false,
            Duration::from_millis(55),
            ssh_stream
        ));
        assert!(!should_defer_stream_draw(
            true,
            false,
            false,
            Duration::from_millis(80),
            ssh_stream
        ));
        assert!(!should_defer_stream_draw(
            true,
            false,
            true,
            Duration::from_millis(5),
            ssh_stream
        ));
        assert!(!should_defer_stream_draw(
            true,
            true,
            false,
            Duration::from_millis(5),
            ssh_stream
        ));
        assert!(!should_defer_stream_draw(
            false,
            false,
            false,
            Duration::from_millis(5),
            ssh_stream
        ));
    }

    fn test_status_snapshot() -> StatusSnapshot {
        StatusSnapshot {
            version_display: "v2.0.0-test".to_string(),
            is_cn_region: false,
            base_optimized: false,
            ip_forward: false,
            podman_gc_enabled: false,
            tools: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn apply_worker_msg_streams_logs_and_finishes_task() {
        let mut state = AppState::new(test_status_snapshot());
        state.start_task_run("测试任务");
        assert!(!apply_worker_msg(
            &mut state,
            WorkerMsg::LogLine("\x1b[32mhello\x1b[0m world".to_string()),
        ));
        assert!(state.is_task_running());
        let task = state.task_run().expect("task_run active");
        assert_eq!(task.lines.len(), 1);
        assert_eq!(task.lines[0], "hello world");

        assert!(apply_worker_msg(
            &mut state,
            WorkerMsg::Finished {
                succeeded: true,
                summary: "任务已完成".to_string(),
                notice_kind: NoticeKind::Success,
                status: None,
                specs: None,
            },
        ));
        let finished = state.task_run().expect("task_run retained after finish");
        assert_eq!(finished.status, debopty_tui::TaskRunStatus::Succeeded);
        assert_eq!(finished.summary.as_deref(), Some("任务已完成"));
        assert_eq!(state.notice(), Some("任务已完成"));
    }

    #[test]
    fn apply_worker_msg_reconcile_audit_opens_confirm_on_drift() {
        let mut state = AppState::new(test_status_snapshot());
        state.start_task_run("系统冲突巡检");
        assert!(apply_worker_msg(
            &mut state,
            WorkerMsg::ReconcileAuditDone {
                drift_count: 2,
                preserved_count: 1,
                status: None,
                specs: SystemSpecs::default(),
            },
        ));
        assert!(matches!(
            state.active_prompt(),
            Some(debopty_tui::InTuiPrompt::Options(opts))
                if opts.kind == debopty_tui::OptionPromptKind::ReconcileApply { drift_count: 2 }
        ));

        let mut clean_state = AppState::new(test_status_snapshot());
        clean_state.start_task_run("系统冲突巡检");
        assert!(apply_worker_msg(
            &mut clean_state,
            WorkerMsg::ReconcileAuditDone {
                drift_count: 0,
                preserved_count: 3,
                status: None,
                specs: SystemSpecs::default(),
            },
        ));
        assert!(clean_state.active_prompt().is_none());
        assert_eq!(clean_state.notice(), Some("系统稳态 · 保留 3 项自定义"));
    }

    #[test]
    fn apply_worker_msg_freship_and_lego_open_native_pickers() {
        use debopty_core::apps::devops::lego::LegoDomainStatus;
        use debopty_core::apps::freship::{
            FreshipCity, FreshipContinent, FreshipCountry, FreshipMap, FreshipState,
        };

        let map = FreshipMap {
            version: "1".to_string(),
            continents: vec![FreshipContinent {
                id: "AS".to_string(),
                name: "Asia".to_string(),
                countries: vec![FreshipCountry {
                    id: "JP".to_string(),
                    name: "Japan".to_string(),
                    keyword_file: "jp.txt".to_string(),
                    states: vec![FreshipState {
                        id: "13".to_string(),
                        name: "Tokyo".to_string(),
                        cities: vec![FreshipCity {
                            id: "TYO".to_string(),
                            name: "Tokyo".to_string(),
                        }],
                    }],
                }],
            }],
        };

        let mut state = AppState::new(test_status_snapshot());
        state.start_task_run("加载 FreshIP");
        assert!(apply_worker_msg(
            &mut state,
            WorkerMsg::FreshipMapLoaded {
                map,
                used_cache: false,
                is_reconfigure: true,
            },
        ));
        assert!(state.task_run().is_none());
        assert!(matches!(
            state.active_prompt(),
            Some(debopty_tui::InTuiPrompt::Freship(p)) if p.is_reconfigure
        ));

        let domains = vec![LegoDomainStatus {
            primary_domain: "example.com".to_string(),
            domains: "example.com".to_string(),
            email: "ops@example.com".to_string(),
            provider: "cloudflare".to_string(),
            auto_renew: true,
            dns_skip_propagation: false,
            ferron_push: false,
            cert_exists: true,
            days_left: Some(45),
            env_path: PathBuf::from("/etc/debopti/lego/domains/example.com.env"),
        }];
        state.start_task_run("加载 Lego");
        assert!(apply_worker_msg(
            &mut state,
            WorkerMsg::LegoDomainsLoaded {
                kind: PickerKind::LegoRunOnceDomain,
                domains,
            },
        ));
        assert!(state.task_run().is_none());
        assert!(matches!(
            state.active_prompt(),
            Some(debopty_tui::InTuiPrompt::Picker(p))
                if p.kind == PickerKind::LegoRunOnceDomain && p.items.len() == 1
        ));
    }

    #[test]
    fn spawn_ratatui_worker_refresh_and_streaming_sinks() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let cfg = tmp.path().join("debopti.conf");
        std::fs::write(&cfg, "IS_CN_REGION=\"true\"\n").expect("write cfg");

        let rx = spawn_ratatui_worker(TuiOutcome::Refresh, &cfg, None, true);
        let msg = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("refresh worker message");
        assert!(matches!(
            msg,
            WorkerMsg::Refreshed {
                status: Some(_),
                ..
            }
        ));

        // Verify StreamingCommandRunner + ScopedCommandSink + ScopedLogSink stream into WorkerMsg
        let (tx, rx2) = mpsc::channel::<WorkerMsg>();
        let handle = thread::spawn(move || {
            let tx_sink = tx.clone();
            let sink: LineSink = Arc::new(move |line: &str| {
                let _ = tx_sink.send(WorkerMsg::LogLine(line.to_string()));
            });
            let _log_guard = diag::install_thread_log_sink(Arc::clone(&sink), LogLevel::Info);
            let _cmd_guard = sys::install_thread_command_sink(Arc::clone(&sink));
            emit_progress("debopty::tui::test", "progress-line-1");
            let runner = StreamingCommandRunner::new(Arc::clone(&sink));
            let _ = runner.run(
                "printf",
                &["cmd-line-1\\ncmd-line-2\\n"],
                Duration::from_secs(3),
            );
            let _ = tx.send(WorkerMsg::Finished {
                succeeded: true,
                summary: "done".to_string(),
                notice_kind: NoticeKind::Success,
                status: None,
                specs: None,
            });
        });
        handle.join().expect("worker join");

        let mut state = AppState::new(test_status_snapshot());
        state.start_task_run("stream test");
        let mut saw_finished = false;
        while let Ok(m) = rx2.try_recv() {
            if apply_worker_msg(&mut state, m) {
                saw_finished = true;
            }
        }
        assert!(saw_finished);
        let lines: Vec<String> = state
            .task_run()
            .expect("task_run")
            .lines
            .iter()
            .cloned()
            .collect();
        assert!(
            lines.iter().any(|l| l.contains("progress-line-1")),
            "expected emit_progress line in {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l == "cmd-line-1"),
            "expected cmd-line-1 in {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l == "cmd-line-2"),
            "expected cmd-line-2 in {lines:?}"
        );
    }

    #[test]
    fn outcome_requires_root_exhaustive_classification_and_label_unwrapping() {
        // Read-only outcomes never require root
        assert!(!outcome_requires_root(&TuiOutcome::Exit));
        assert!(!outcome_requires_root(&TuiOutcome::Refresh));
        assert!(!outcome_requires_root(&TuiOutcome::RunNativeReconcile));
        assert!(!outcome_requires_root(&TuiOutcome::Execute(
            BashAction::LegacyFullEntry
        )));
        assert!(!outcome_requires_root(&TuiOutcome::RunMenuAction(
            MenuAction::Navigate(debopty_tui::MenuId::Main)
        )));
        assert!(!outcome_requires_root(&TuiOutcome::RunMenuAction(
            MenuAction::FreshipLogs
        )));
        assert!(!outcome_requires_root(&TuiOutcome::PromptSubmitted(
            PromptSubmission::LegoTroubleshoot {
                domain: "example.com".to_string()
            }
        )));
        assert!(!outcome_requires_root(&TuiOutcome::ElevatedApproved(
            Box::new(TuiOutcome::RunNativeF2bSync)
        )));

        // Privileged outcomes require root
        assert!(outcome_requires_root(&TuiOutcome::RunNativeF2bSync));
        assert!(outcome_requires_root(&TuiOutcome::Execute(
            BashAction::RunBaseOptimization
        )));
        assert!(outcome_requires_root(&TuiOutcome::Execute(
            BashAction::ToggleIpForwarding
        )));
        assert!(outcome_requires_root(&TuiOutcome::Execute(
            BashAction::InstallApp(AppId::Podman)
        )));
        assert!(outcome_requires_root(&TuiOutcome::Execute(
            BashAction::UninstallApp(AppId::Xray)
        )));
        assert!(outcome_requires_root(&TuiOutcome::RunMenuAction(
            MenuAction::XrayToggleRuleset
        )));
        assert!(outcome_requires_root(&TuiOutcome::RunMenuAction(
            MenuAction::LegoInstall
        )));
        assert!(outcome_requires_root(&TuiOutcome::PromptSubmitted(
            PromptSubmission::ReconcileApply
        )));
        assert!(outcome_requires_root(&TuiOutcome::PromptSubmitted(
            PromptSubmission::YaziInstall {
                media_preview: true
            }
        )));

        // describe_outcome_label unwraps ElevatedApproved transparently
        let inner = TuiOutcome::Execute(BashAction::ToggleIpForwarding);
        assert_eq!(
            describe_outcome_label(&TuiOutcome::ElevatedApproved(Box::new(inner.clone()))),
            describe_outcome_label(&inner)
        );
    }

    #[test]
    fn tui_outcome_serde_roundtrip_and_trampoline_validation() {
        let samples = vec![
            TuiOutcome::Exit,
            TuiOutcome::Refresh,
            TuiOutcome::RunNativeF2bSync,
            TuiOutcome::RunNativeReconcile,
            TuiOutcome::Execute(BashAction::InstallApp(AppId::Podman)),
            TuiOutcome::RunMenuAction(MenuAction::XrayToggleCron),
            TuiOutcome::PromptSubmitted(PromptSubmission::PodmanUninstall {
                delete_data: false,
                delete_user: true,
            }),
            TuiOutcome::ElevatedApproved(Box::new(TuiOutcome::Execute(
                BashAction::RunBaseOptimization,
            ))),
        ];
        for sample in samples {
            let json = serde_json::to_vec(&sample).expect("serialize TuiOutcome");
            let decoded: TuiOutcome =
                serde_json::from_slice(&json).expect("deserialize TuiOutcome");
            assert_eq!(decoded, sample);
        }

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let cfg = tmp.path().join("debopti.conf");
        std::fs::write(&cfg, "IS_CN_REGION=\"false\"\n").expect("write cfg");

        unsafe {
            std::env::set_var("DEBOPTY_ALLOW_UNPRIVILEGED_TUI_EXEC", "1");
        }

        // Valid payload (Refresh -> no-op exit 0)
        let valid_file = tmp.path().join("valid.json");
        std::fs::write(
            &valid_file,
            serde_json::to_vec(&TuiOutcome::Refresh).unwrap(),
        )
        .unwrap();
        let rc = handle_tui_exec_outcome(&valid_file, &cfg, None).expect("valid trampoline");
        assert_eq!(rc, 0);

        // Invalid JSON rejected
        let bad_file = tmp.path().join("bad.json");
        std::fs::write(&bad_file, b"{not valid json").unwrap();
        assert!(handle_tui_exec_outcome(&bad_file, &cfg, None).is_err());

        // Nested ElevatedApproved rejected to prevent recursion
        let nested_file = tmp.path().join("nested.json");
        let nested = TuiOutcome::ElevatedApproved(Box::new(TuiOutcome::Refresh));
        std::fs::write(&nested_file, serde_json::to_vec(&nested).unwrap()).unwrap();
        let err = handle_tui_exec_outcome(&nested_file, &cfg, None).unwrap_err();
        assert!(err.to_string().contains("ElevatedApproved"));
    }
}
