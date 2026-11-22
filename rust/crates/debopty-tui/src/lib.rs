pub mod console_ui;
pub mod error;
pub mod layout;
pub mod menu;
pub mod motion;
#[cfg(feature = "ratatui-ui")]
pub mod ratatui_ui;
pub mod renderer;
pub mod state;
#[cfg(feature = "ratatui-ui")]
pub mod terminal_guard;

use std::fmt;
use std::str::FromStr;

pub use console_ui::{
    ConsoleRenderer, is_non_interactive, is_non_interactive_with, is_non_interactive_with_tty,
};
pub use error::{Result, TuiError};
pub use menu::{
    ActionInspectorMeta, MenuAction, MenuEntry, MenuId, MenuItem, MenuScreen, PaletteEntry,
    StatusBadge, WorkspaceTab, build_menu, build_palette_entries, build_workspace_menu,
};
pub use motion::{
    ACTIVE_FRAME_INTERVAL, DEFAULT_TRANSITION_DURATION, LOCAL_TELEMETRY_INTERVAL, MotionPolicy,
    MotionTransition, SSH_SLOW_DRAW_THRESHOLD, SSH_TELEMETRY_INTERVAL, SSH_TRANSITION_DURATION,
    TOAST_TTL_DURATION, TransitionKind, WakeKind, WakeSchedule, ease_out_cubic,
};
#[cfg(feature = "ratatui-ui")]
pub use ratatui_ui::{
    RatatuiRenderer, ShowcaseOptions, UiHitMap, UiPalette, detect_terminal_size, hit_test_mouse,
    map_crossterm_key, map_crossterm_key_filtered, render_frame, render_frame_with_palette,
    render_showcase_frame, render_showcase_frame_with_options, render_showcase_gallery,
    render_showcase_gallery_with_options, sample_showcase_status, should_enable_animations,
};
pub use renderer::{MenuEvent, TuiRenderer};
pub use state::{
    ActivePane, AppState, ConfirmPrompt, DEFAULT_TOAST_TTL_TICKS, NoticeKind,
    TRANSITION_TOTAL_TICKS, TuiOutcome,
};
#[cfg(feature = "ratatui-ui")]
pub use terminal_guard::{
    CrosstermOps, TerminalGuard, TerminalOps, install_panic_hook, panic_restore_is_armed,
};

/// Selectable interactive UI backend (`console` remains default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UiBackendKind {
    #[default]
    Console,
    Ratatui,
    Auto,
}

impl fmt::Display for UiBackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Console => f.write_str("console"),
            Self::Ratatui => f.write_str("ratatui"),
            Self::Auto => f.write_str("auto"),
        }
    }
}

impl FromStr for UiBackendKind {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "console" => Ok(Self::Console),
            "ratatui" => Ok(Self::Ratatui),
            "auto" => Ok(Self::Auto),
            other => Err(format!(
                "invalid UI backend '{other}': expected one of console, ratatui, auto"
            )),
        }
    }
}

/// Returns `true` when compiled with the opt-in `ratatui-ui` Cargo feature.
pub const fn has_ratatui_ui() -> bool {
    cfg!(feature = "ratatui-ui")
}

/// Runs the interactive TUI event loop using a pluggable `TuiRenderer`.
///
/// Exits immediately with `Ok(())` when `renderer.is_non_interactive()` is `true`
/// (e.g. `CI=true` or non-TTY stdin on `ConsoleRenderer`).
pub fn run_tui_loop(
    renderer: &mut dyn TuiRenderer,
    state: &mut AppState,
    on_action: &mut dyn FnMut(TuiOutcome) -> Result<()>,
) -> Result<()> {
    run_tui_loop_with_state(renderer, state, &mut |outcome, _state| on_action(outcome))
}

fn is_clean_disconnect(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::UnexpectedEof
    )
}

/// Variant of `run_tui_loop` granting `on_action` mutable access to `AppState`
/// so callers can refresh `StatusSnapshot` immediately after each child action.
pub fn run_tui_loop_with_state(
    renderer: &mut dyn TuiRenderer,
    state: &mut AppState,
    on_action: &mut dyn FnMut(TuiOutcome, &mut AppState) -> Result<()>,
) -> Result<()> {
    if renderer.is_non_interactive() {
        return Ok(());
    }

    loop {
        if state.take_dirty()
            && let Err(err) = renderer.draw(state)
        {
            if is_clean_disconnect(&err) {
                return Ok(());
            }
            return Err(err.into());
        }
        renderer.sync_viewport(state);
        let event = match renderer.read_event() {
            Ok(ev) => ev,
            Err(err) if is_clean_disconnect(&err) => return Ok(()),
            Err(err) => return Err(err.into()),
        };
        renderer.sync_viewport(state);
        let Some(outcome) = state.handle_event(event) else {
            continue;
        };

        match outcome {
            TuiOutcome::Exit => return Ok(()),
            TuiOutcome::Refresh => {
                on_action(outcome, state)?;
                state.mark_dirty();
            }
            TuiOutcome::Execute(_)
            | TuiOutcome::RunNativeF2bSync
            | TuiOutcome::RunNativeReconcile => {
                if let Err(err) = renderer.suspend_for_child() {
                    if is_clean_disconnect(&err) {
                        return Ok(());
                    }
                    return Err(err.into());
                }
                let action_res = on_action(outcome, state);
                if action_res.is_ok() {
                    if state.should_exit() {
                        return Ok(());
                    }
                    if let Err(err) = renderer.resume_after_child() {
                        if is_clean_disconnect(&err) {
                            return Ok(());
                        }
                        return Err(err.into());
                    }
                    state.mark_dirty();
                }
                action_res?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::io;

    use console::Key;
    use debopty_bridge::{AppId, BashAction};
    use debopty_core::status::{StatusSnapshot, TRACKED_TOOLS};

    use super::*;
    use crate::console_ui::map_key_to_event;
    use crate::layout::{
        DEFAULT_BOX_INNER_WIDTH, render_header_box, render_menu_item_plain, visual_width,
    };

    struct MockRenderer {
        events: VecDeque<MenuEvent>,
        drawn_menus: Vec<MenuId>,
        suspends: usize,
        resumes: usize,
        non_interactive: bool,
    }

    impl MockRenderer {
        fn new(events: impl IntoIterator<Item = MenuEvent>) -> Self {
            Self {
                events: events.into_iter().collect(),
                drawn_menus: Vec::new(),
                suspends: 0,
                resumes: 0,
                non_interactive: false,
            }
        }

        fn non_interactive() -> Self {
            Self {
                events: VecDeque::new(),
                drawn_menus: Vec::new(),
                suspends: 0,
                resumes: 0,
                non_interactive: true,
            }
        }
    }

    impl TuiRenderer for MockRenderer {
        fn draw(&mut self, state: &AppState) -> io::Result<()> {
            self.drawn_menus.push(state.current_menu());
            Ok(())
        }

        fn read_event(&mut self) -> io::Result<MenuEvent> {
            Ok(self.events.pop_front().unwrap_or(MenuEvent::Quit))
        }

        fn suspend_for_child(&mut self) -> io::Result<()> {
            self.suspends += 1;
            Ok(())
        }

        fn resume_after_child(&mut self) -> io::Result<()> {
            self.resumes += 1;
            Ok(())
        }

        fn is_non_interactive(&self) -> bool {
            self.non_interactive
        }
    }

    fn sample_status(freship_installed: bool) -> StatusSnapshot {
        let mut tools = BTreeMap::new();
        for &tool in TRACKED_TOOLS {
            tools.insert(tool.to_string(), false);
        }
        tools.insert("fish".to_string(), true);
        tools.insert("podman".to_string(), true);
        tools.insert("freship".to_string(), freship_installed);

        StatusSnapshot {
            version_display: "1.0.7 (231e05eb)".to_string(),
            is_cn_region: true,
            base_optimized: true,
            ip_forward: true,
            podman_gc_enabled: false,
            tools,
        }
    }

    #[test]
    fn full_menu_navigation_covers_all_submenus_actions_and_confirmations() {
        let events = [
            // Main item 1 (2-digit prefix disambiguated with Enter) -> RunBaseOptimization
            MenuEvent::Digit(1),
            MenuEvent::Select,
            // Main item 2 -> ToggleIpForwarding
            MenuEvent::Digit(2),
            // Main item 3 -> Devops -> Fish AppCrud -> Install -> Back -> Lego -> Back to Main
            MenuEvent::Digit(3),
            MenuEvent::Digit(1),
            MenuEvent::Digit(1),
            MenuEvent::Digit(0),
            MenuEvent::Digit(4),
            MenuEvent::Back,
            // Main item 4 -> Podman -> Uninstall (cancel then confirm) -> Toggle GC -> Back
            MenuEvent::Digit(4),
            MenuEvent::Digit(2),
            MenuEvent::ConfirmNo,
            MenuEvent::Digit(2),
            MenuEvent::ConfirmYes,
            MenuEvent::Digit(3),
            MenuEvent::Digit(0),
            // Main item 5 -> RustStack -> Ferron -> Install -> Back -> Back
            MenuEvent::Digit(5),
            MenuEvent::Digit(3),
            MenuEvent::Digit(1),
            MenuEvent::Back,
            MenuEvent::Back,
            // Main item 6 -> GoStack -> Caddy -> Install -> Back -> Back
            MenuEvent::Digit(6),
            MenuEvent::Digit(2),
            MenuEvent::Digit(1),
            MenuEvent::Back,
            MenuEvent::Back,
            // Main item 7 -> ManageXray
            MenuEvent::Digit(7),
            // Main item 8 -> WarpStack -> GenerateWarpXray -> Back
            MenuEvent::Digit(8),
            MenuEvent::Digit(3),
            MenuEvent::Back,
            // Main item 9 -> Easytier AppCrud -> Install -> Back
            MenuEvent::Digit(9),
            MenuEvent::Digit(1),
            MenuEvent::Back,
            // Main item 10 -> Tailscale AppCrud -> Install -> Back
            MenuEvent::Digit(1),
            MenuEvent::Digit(0),
            MenuEvent::Digit(1),
            MenuEvent::Back,
            // Main item 11 -> FreshIP (uninstalled => InstallApp(Freship))
            MenuEvent::Digit(1),
            MenuEvent::Digit(1),
            // Main item 12 -> Maintenance -> ScriptUpdate -> NativeReconcile -> NativeF2bSync -> ScriptUninstall (cancel then confirm) -> Back
            MenuEvent::Digit(1),
            MenuEvent::Digit(2),
            MenuEvent::Digit(1),
            MenuEvent::Digit(3),
            MenuEvent::Digit(4),
            MenuEvent::Digit(2),
            MenuEvent::ConfirmNo,
            MenuEvent::Digit(2),
            MenuEvent::ConfirmYes,
            MenuEvent::Back,
            // Arrow navigation & Refresh then exit via 0
            MenuEvent::Down,
            MenuEvent::Up,
            MenuEvent::Refresh,
            MenuEvent::Digit(0),
        ];

        let mut renderer = MockRenderer::new(events);
        let mut state = AppState::new(sample_status(false));
        let mut dispatched = Vec::new();

        run_tui_loop(&mut renderer, &mut state, &mut |outcome| {
            dispatched.push(outcome);
            Ok(())
        })
        .expect("tui loop completes cleanly");

        assert_eq!(
            dispatched,
            vec![
                TuiOutcome::Execute(BashAction::RunBaseOptimization),
                TuiOutcome::Execute(BashAction::ToggleIpForwarding),
                TuiOutcome::Execute(BashAction::InstallApp(AppId::Fish)),
                TuiOutcome::Execute(BashAction::ManageLego),
                TuiOutcome::Execute(BashAction::UninstallApp(AppId::Podman)),
                TuiOutcome::Execute(BashAction::TogglePodmanGc),
                TuiOutcome::Execute(BashAction::InstallApp(AppId::Ferron)),
                TuiOutcome::Execute(BashAction::InstallApp(AppId::Caddy)),
                TuiOutcome::Execute(BashAction::ManageXray),
                TuiOutcome::Execute(BashAction::GenerateWarpXray),
                TuiOutcome::Execute(BashAction::InstallApp(AppId::Easytier)),
                TuiOutcome::Execute(BashAction::InstallApp(AppId::Tailscale)),
                TuiOutcome::Execute(BashAction::InstallApp(AppId::Freship)),
                TuiOutcome::Execute(BashAction::ScriptUpdate),
                TuiOutcome::RunNativeReconcile,
                TuiOutcome::RunNativeF2bSync,
                TuiOutcome::Execute(BashAction::ScriptUninstall),
                TuiOutcome::Refresh,
            ]
        );
        let child_actions = dispatched
            .iter()
            .filter(|o| {
                matches!(
                    o,
                    TuiOutcome::Execute(_)
                        | TuiOutcome::RunNativeF2bSync
                        | TuiOutcome::RunNativeReconcile
                )
            })
            .count();
        assert_eq!(renderer.suspends, child_actions);
        assert_eq!(renderer.resumes, child_actions);
    }

    #[test]
    fn freship_switches_to_manage_when_installed_and_state_reloads() {
        let mut renderer = MockRenderer::new([
            MenuEvent::Digit(1),
            MenuEvent::Digit(1),
            MenuEvent::Digit(1),
            MenuEvent::Digit(1),
            MenuEvent::Quit,
        ]);
        let mut state = AppState::new(sample_status(false));
        let mut seen = Vec::new();

        run_tui_loop_with_state(&mut renderer, &mut state, &mut |outcome, st| {
            seen.push(outcome);
            if outcome == TuiOutcome::Execute(BashAction::InstallApp(AppId::Freship)) {
                st.refresh_status(sample_status(true));
            }
            Ok(())
        })
        .expect("loop ok");

        assert_eq!(
            seen,
            vec![
                TuiOutcome::Execute(BashAction::InstallApp(AppId::Freship)),
                TuiOutcome::Execute(BashAction::ManageFreship),
            ]
        );
    }

    #[test]
    fn cjk_and_emoji_header_boxes_have_exact_equal_visual_width_across_all_menus() {
        let status = sample_status(true);
        let all_menus = [
            MenuId::Main,
            MenuId::Devops,
            MenuId::Podman,
            MenuId::RustStack,
            MenuId::GoStack,
            MenuId::WarpStack,
            MenuId::Maintenance,
            MenuId::AppCrud(AppId::Fish),
            MenuId::AppCrud(AppId::RustSdk),
            MenuId::AppCrud(AppId::Derper),
        ];

        let expected_box_width = DEFAULT_BOX_INNER_WIDTH + 2;
        for menu_id in all_menus {
            let screen = build_menu(menu_id, &status);
            let box_lines = render_header_box(
                &screen.title,
                Some(&screen.subtitle),
                DEFAULT_BOX_INNER_WIDTH,
            );
            assert_eq!(box_lines.len(), 4);
            for line in &box_lines {
                assert_eq!(
                    visual_width(line),
                    expected_box_width,
                    "header line misaligned for {menu_id:?}: {line}"
                );
            }

            // Ensure every menu item renders cleanly with non-zero visual width.
            for item in screen.items() {
                let row = render_menu_item_plain(item, false, DEFAULT_BOX_INNER_WIDTH);
                assert!(visual_width(&row) > 4);
                assert!(row.contains(item.label));
            }
        }
    }

    #[test]
    fn non_interactive_and_ci_exit_immediately_without_blocking() {
        assert!(is_non_interactive_with(Some("true"), true));
        assert!(is_non_interactive_with(Some("1"), true));
        assert!(is_non_interactive_with(None, false));
        assert!(is_non_interactive_with(Some(""), false));
        assert!(!is_non_interactive_with(None, true));
        assert!(!is_non_interactive_with(Some(""), true));

        let mut renderer = MockRenderer::non_interactive();
        let mut state = AppState::new(sample_status(false));
        let mut calls = 0usize;
        run_tui_loop(&mut renderer, &mut state, &mut |_| {
            calls += 1;
            Ok(())
        })
        .expect("non-interactive exits Ok");
        assert_eq!(calls, 0);
        assert!(renderer.drawn_menus.is_empty());
    }

    #[test]
    fn keymap_supports_micro_and_yazi_ergonomics_plus_standard_navigation() {
        assert_eq!(map_key_to_event(Key::ArrowUp), Some(MenuEvent::Up));
        assert_eq!(map_key_to_event(Key::ArrowDown), Some(MenuEvent::Down));
        assert_eq!(map_key_to_event(Key::Enter), Some(MenuEvent::Select));
        assert_eq!(map_key_to_event(Key::Escape), Some(MenuEvent::Back));
        assert_eq!(map_key_to_event(Key::Char('0')), Some(MenuEvent::Digit(0)));
        assert_eq!(map_key_to_event(Key::Char('9')), Some(MenuEvent::Digit(9)));
        assert_eq!(map_key_to_event(Key::CtrlC), Some(MenuEvent::Quit));
        assert_eq!(map_key_to_event(Key::Char('\x11')), Some(MenuEvent::Quit));
        assert_eq!(
            map_key_to_event(Key::Char('y')),
            Some(MenuEvent::ConfirmYes)
        );
        assert_eq!(map_key_to_event(Key::Char('n')), Some(MenuEvent::ConfirmNo));
        assert_eq!(map_key_to_event(Key::Char('r')), Some(MenuEvent::Refresh));
        assert_eq!(map_key_to_event(Key::Char('q')), Some(MenuEvent::Quit));
        assert_eq!(map_key_to_event(Key::Char('Q')), Some(MenuEvent::Quit));
        assert_eq!(map_key_to_event(Key::PageUp), Some(MenuEvent::PageUp));
        assert_eq!(map_key_to_event(Key::PageDown), Some(MenuEvent::PageDown));
        assert_eq!(map_key_to_event(Key::Home), Some(MenuEvent::Home));
        assert_eq!(map_key_to_event(Key::End), Some(MenuEvent::End));

        // U25 Yazi / Micro ergonomic navigation bindings.
        assert_eq!(map_key_to_event(Key::Char('j')), Some(MenuEvent::Down));
        assert_eq!(map_key_to_event(Key::Char('k')), Some(MenuEvent::Up));
        assert_eq!(map_key_to_event(Key::Char('h')), Some(MenuEvent::Back));
        assert_eq!(map_key_to_event(Key::Char('l')), Some(MenuEvent::Select));
        assert_eq!(map_key_to_event(Key::Char('g')), Some(MenuEvent::Home));
        assert_eq!(map_key_to_event(Key::Char('G')), Some(MenuEvent::End));
    }

    #[test]
    fn menu_item_counts_match_scripts_tui_sh_parity() {
        let status = sample_status(false);
        assert_eq!(build_menu(MenuId::Main, &status).items().len(), 13); // 1-12 + 0
        assert_eq!(build_menu(MenuId::Devops, &status).items().len(), 6); // 1-5 + 0
        assert_eq!(build_menu(MenuId::Podman, &status).items().len(), 4); // 1-3 + 0
        assert_eq!(build_menu(MenuId::RustStack, &status).items().len(), 4); // 1-3 + 0
        assert_eq!(build_menu(MenuId::GoStack, &status).items().len(), 4); // 1-3 + 0
        assert_eq!(build_menu(MenuId::WarpStack, &status).items().len(), 4); // 1-3 + 0
        assert_eq!(build_menu(MenuId::Maintenance, &status).items().len(), 5); // 1-4 + 0
        assert_eq!(
            build_menu(MenuId::AppCrud(AppId::Fish), &status)
                .items()
                .len(),
            3
        ); // 1-2 + 0

        // All menu icons MUST be FE0F-free single-codepoint EAW-W (width 2) to prevent border wobble.
        for menu_id in [
            MenuId::Main,
            MenuId::Devops,
            MenuId::Podman,
            MenuId::RustStack,
            MenuId::GoStack,
            MenuId::WarpStack,
            MenuId::Maintenance,
            MenuId::AppCrud(AppId::Fish),
        ] {
            for item in build_menu(menu_id, &status).items() {
                assert!(
                    !item.icon.contains('\u{FE0F}'),
                    "FE0F found in icon {:?} for {:?}",
                    item.icon,
                    item.label
                );
                assert_eq!(
                    visual_width(item.icon),
                    2,
                    "icon {:?} for {:?} must have exact width 2",
                    item.icon,
                    item.label
                );
            }
        }
    }

    #[test]
    fn ui_backend_kind_defaults_to_console_and_parses_all_variants() {
        assert_eq!(UiBackendKind::default(), UiBackendKind::Console);
        assert_eq!(UiBackendKind::Console.to_string(), "console");
        assert_eq!(UiBackendKind::Ratatui.to_string(), "ratatui");
        assert_eq!(UiBackendKind::Auto.to_string(), "auto");

        assert_eq!(
            "console".parse::<UiBackendKind>().expect("parse console"),
            UiBackendKind::Console
        );
        assert_eq!(
            "RATATUI".parse::<UiBackendKind>().expect("parse ratatui"),
            UiBackendKind::Ratatui
        );
        assert_eq!(
            " auto ".parse::<UiBackendKind>().expect("parse auto"),
            UiBackendKind::Auto
        );
        assert!("invalid".parse::<UiBackendKind>().is_err());

        #[cfg(not(feature = "ratatui-ui"))]
        assert!(!has_ratatui_ui());
        #[cfg(feature = "ratatui-ui")]
        assert!(has_ratatui_ui());
    }

    #[test]
    fn is_non_interactive_with_tty_checks_both_stdin_and_stdout() {
        assert!(!is_non_interactive_with_tty(None, true, true));
        assert!(!is_non_interactive_with_tty(Some(""), true, true));
        assert!(is_non_interactive_with_tty(None, false, true));
        assert!(is_non_interactive_with_tty(None, true, false));
        assert!(is_non_interactive_with_tty(None, false, false));
        assert!(is_non_interactive_with_tty(Some("true"), true, true));
    }

    #[test]
    fn backspace_clears_digit_prefix_without_exiting() {
        let mut state = AppState::new(sample_status(false));
        assert!(state.handle_event(MenuEvent::Digit(1)).is_none());
        assert_eq!(state.digit_prefix(), Some(1));

        // First Backspace/Esc clears digit_prefix and stays on Main.
        assert_eq!(state.handle_event(MenuEvent::Back), None);
        assert_eq!(state.digit_prefix(), None);
        assert_eq!(state.current_menu(), MenuId::Main);

        // Second Backspace/Esc on Main exits cleanly.
        assert_eq!(state.handle_event(MenuEvent::Back), Some(TuiOutcome::Exit));
    }

    #[test]
    fn resize_and_refresh_trigger_redraw_and_reprobe_without_suspend() {
        let mut renderer = MockRenderer::new([
            MenuEvent::Resize,
            MenuEvent::Refresh,
            MenuEvent::Resize,
            MenuEvent::Quit,
        ]);
        let mut state = AppState::new(sample_status(false));
        let mut refresh_count = 0usize;

        run_tui_loop_with_state(&mut renderer, &mut state, &mut |outcome, st| {
            assert_eq!(outcome, TuiOutcome::Refresh);
            refresh_count += 1;
            st.refresh_status(sample_status(true));
            Ok(())
        })
        .expect("loop ok");

        assert_eq!(refresh_count, 1);
        assert_eq!(renderer.suspends, 0);
        assert_eq!(renderer.resumes, 0);
        assert!(state.status.tools["freship"]);
        // Initial draw + after Resize + after Refresh + after Resize = 4 draws.
        assert_eq!(renderer.drawn_menus.len(), 4);
    }

    #[test]
    fn action_error_skips_resume_after_child_and_propagates() {
        let mut renderer = MockRenderer::new([MenuEvent::Digit(2)]);
        let mut state = AppState::new(sample_status(false));
        let err = run_tui_loop_with_state(&mut renderer, &mut state, &mut |_, _| {
            Err(TuiError::Io(io::Error::other("simulated child failure")))
        })
        .expect_err("error must propagate");
        assert!(err.to_string().contains("simulated child failure"));
        assert_eq!(renderer.suspends, 1);
        assert_eq!(renderer.resumes, 0);
    }

    struct FailingIoRenderer {
        fail_draw: Option<io::ErrorKind>,
        fail_read: Option<io::ErrorKind>,
    }

    impl TuiRenderer for FailingIoRenderer {
        fn draw(&mut self, _state: &AppState) -> io::Result<()> {
            if let Some(kind) = self.fail_draw {
                return Err(io::Error::new(kind, "simulated draw disconnect"));
            }
            Ok(())
        }

        fn read_event(&mut self) -> io::Result<MenuEvent> {
            if let Some(kind) = self.fail_read {
                return Err(io::Error::new(kind, "simulated read disconnect"));
            }
            Ok(MenuEvent::Quit)
        }

        fn suspend_for_child(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn resume_after_child(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn broken_pipe_and_unexpected_eof_exit_tui_loop_cleanly() {
        for kind in [io::ErrorKind::BrokenPipe, io::ErrorKind::UnexpectedEof] {
            let mut r_draw = FailingIoRenderer {
                fail_draw: Some(kind),
                fail_read: None,
            };
            let mut state = AppState::new(sample_status(false));
            run_tui_loop(&mut r_draw, &mut state, &mut |_| Ok(()))
                .expect("draw disconnect exits Ok");

            let mut r_read = FailingIoRenderer {
                fail_draw: None,
                fail_read: Some(kind),
            };
            run_tui_loop(&mut r_read, &mut state, &mut |_| Ok(()))
                .expect("read disconnect exits Ok");
        }
    }

    #[test]
    fn truncate_to_width_respects_fe0f_emoji_and_cjk_bounds() {
        use crate::layout::truncate_to_width;

        let samples = [
            "🛠️ 运维与终端工具集",
            "🛰️ Xray Core 节点管理",
            "⚙️ 脚本维护 (更新/卸载)",
            "🗂️ Yazi 极速终端文件管理器",
            "🗑️ 彻底卸载脚本及资产",
            "中文ASCII混合🛠️🛰️⚙️",
        ];
        for sample in samples {
            for max_w in 0..=20 {
                let truncated = truncate_to_width(sample, max_w);
                assert!(
                    visual_width(&truncated) <= max_w,
                    "overflow for sample={sample:?} max_w={max_w}: got {truncated:?} (w={})",
                    visual_width(&truncated)
                );
            }
        }
    }

    #[test]
    fn narrow_menu_rows_do_not_overflow_inner_width() {
        let status = sample_status(true);
        let screen = build_menu(MenuId::Main, &status);
        let console_renderer = ConsoleRenderer::with_width(44);
        for item in screen.items() {
            if let Some(badge) = item.status {
                let badge_w = visual_width(&badge.plain_text());
                let plain = render_menu_item_plain(item, true, 30);
                assert!(
                    visual_width(&plain) <= 30.max(badge_w + 1),
                    "plain narrow row overflow: {plain:?} (w={})",
                    visual_width(&plain)
                );
                let styled = console_renderer.format_styled_item(item, true);
                assert!(
                    visual_width(&styled) <= 44,
                    "styled narrow row overflow: {styled:?} (w={})",
                    visual_width(&styled)
                );
            }
        }
    }
}
