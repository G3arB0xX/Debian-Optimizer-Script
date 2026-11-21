pub mod console_ui;
pub mod error;
pub mod layout;
pub mod menu;
pub mod renderer;
pub mod state;

pub use console_ui::{ConsoleRenderer, is_non_interactive, is_non_interactive_with};
pub use error::{Result, TuiError};
pub use menu::{MenuAction, MenuEntry, MenuId, MenuItem, MenuScreen, StatusBadge, build_menu};
pub use renderer::{MenuEvent, TuiRenderer};
pub use state::{AppState, ConfirmPrompt, TuiOutcome};

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
        renderer.draw(state)?;
        let event = renderer.read_event()?;
        let Some(outcome) = state.handle_event(event) else {
            continue;
        };

        match outcome {
            TuiOutcome::Exit => return Ok(()),
            TuiOutcome::Execute(_) | TuiOutcome::RunNativeF2bSync => {
                renderer.suspend_for_child()?;
                let action_res = on_action(outcome, state);
                renderer.resume_after_child()?;
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
            // Main item 12 -> Maintenance -> ScriptUpdate -> Back
            MenuEvent::Digit(1),
            MenuEvent::Digit(2),
            MenuEvent::Digit(1),
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
            ]
        );
        assert_eq!(renderer.suspends, dispatched.len());
        assert_eq!(renderer.resumes, dispatched.len());
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
    fn keymap_uses_standard_navigation_and_excludes_vim_only_bindings() {
        assert_eq!(map_key_to_event(Key::ArrowUp), Some(MenuEvent::Up));
        assert_eq!(map_key_to_event(Key::ArrowDown), Some(MenuEvent::Down));
        assert_eq!(map_key_to_event(Key::Enter), Some(MenuEvent::Select));
        assert_eq!(map_key_to_event(Key::Escape), Some(MenuEvent::Back));
        assert_eq!(map_key_to_event(Key::Char('0')), Some(MenuEvent::Digit(0)));
        assert_eq!(map_key_to_event(Key::Char('9')), Some(MenuEvent::Digit(9)));
        assert_eq!(map_key_to_event(Key::CtrlC), Some(MenuEvent::Quit));
        assert_eq!(
            map_key_to_event(Key::Char('y')),
            Some(MenuEvent::ConfirmYes)
        );
        assert_eq!(map_key_to_event(Key::Char('n')), Some(MenuEvent::ConfirmNo));
        assert_eq!(map_key_to_event(Key::Char('r')), Some(MenuEvent::Refresh));

        // Vim-only navigation keys MUST NOT be bound.
        for vim_key in ['h', 'j', 'k', 'l'] {
            assert_eq!(map_key_to_event(Key::Char(vim_key)), None);
        }
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
        assert_eq!(build_menu(MenuId::Maintenance, &status).items().len(), 3); // 1-2 + 0
        assert_eq!(
            build_menu(MenuId::AppCrud(AppId::Fish), &status)
                .items()
                .len(),
            3
        ); // 1-2 + 0
    }
}
