use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use debopty_bridge::{AppId, BashAction, BashBridge, EngineMode};
use debopty_core::config::ConfigStore;
use debopty_core::network::UreqHttpClient;
use debopty_core::security::f2b_sync::run_f2b_sync;
use debopty_core::status::probe_status;
use debopty_core::sys::RealCommandRunner;
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

#[cfg(feature = "ratatui-ui")]
fn detect_terminal_size() -> Option<(u16, u16)> {
    if let Ok(out) = std::process::Command::new("stty")
        .arg("size")
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        && out.status.success()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        let mut parts = text.split_whitespace();
        if let (Some(rows_s), Some(cols_s)) = (parts.next(), parts.next())
            && let (Ok(rows), Ok(cols)) = (rows_s.parse::<u16>(), cols_s.parse::<u16>())
        {
            return Some((cols, rows));
        }
    }

    let cols = non_empty_env("COLUMNS").and_then(|v| v.parse::<u16>().ok())?;
    let rows = non_empty_env("LINES").and_then(|v| v.parse::<u16>().ok())?;
    Some((cols, rows))
}

#[cfg(feature = "ratatui-ui")]
fn terminal_size_supports_ratatui() -> bool {
    detect_terminal_size().is_some_and(|(w, h)| w >= 80 && h >= 24)
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

fn execute_tui_action(
    action: BashAction,
    st: &mut AppState,
    config_path: &Path,
    script_dir: Option<&Path>,
    template_dir: Option<&Path>,
    runner: &RealCommandRunner,
) {
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
            let http = UreqHttpClient::new();
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
        other => match BashBridge::discover(script_dir, Some(config_path)) {
            Ok(bridge) => match bridge.execute(&other, &[]) {
                Ok(code) => {
                    if code != 0 {
                        st.set_notice(format!("操作 {other} 退出码: {code}"));
                    }
                }
                Err(err) => {
                    st.set_notice(format!("执行 {other} 失败: {err}"));
                }
            },
            Err(err) => {
                st.set_notice(format!("未找到 Bash 脚本目录: {err}"));
            }
        },
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
    let template_dir = cli.template_dir.clone();
    let script_dir = cli.script_dir.clone();

    run_tui_loop_with_state(renderer.as_mut(), &mut state, &mut |outcome, st| {
        match outcome {
            TuiOutcome::Exit => {}
            TuiOutcome::Execute(action) => {
                execute_tui_action(
                    action,
                    st,
                    config_path,
                    script_dir.as_deref(),
                    template_dir.as_deref(),
                    &runner,
                );
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
