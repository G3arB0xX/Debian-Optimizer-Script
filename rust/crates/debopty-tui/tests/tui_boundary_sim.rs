use std::collections::{BTreeMap, VecDeque};
use std::io;

use debopty_core::status::{StatusSnapshot, TRACKED_TOOLS};
use debopty_tui::layout::{truncate_to_width, visual_width};
use debopty_tui::{
    AppState, MenuEvent, MenuId, TuiError, TuiOutcome, TuiRenderer, is_non_interactive_with_tty,
    run_tui_loop, run_tui_loop_with_state,
};

fn sample_status() -> StatusSnapshot {
    let mut tools = BTreeMap::new();
    for &tool in TRACKED_TOOLS {
        tools.insert(tool.to_string(), false);
    }
    tools.insert("fish".to_string(), true);
    tools.insert("podman".to_string(), true);
    StatusSnapshot {
        version_display: "1.0.7 (sim)".to_string(),
        is_cn_region: true,
        base_optimized: true,
        ip_forward: false,
        podman_gc_enabled: true,
        tools,
    }
}

struct ScriptedRenderer {
    events: VecDeque<io::Result<MenuEvent>>,
    fail_draw_kind: Option<io::ErrorKind>,
    draws: usize,
    suspends: usize,
    resumes: usize,
}

impl ScriptedRenderer {
    fn new(events: impl IntoIterator<Item = io::Result<MenuEvent>>) -> Self {
        Self {
            events: events.into_iter().collect(),
            fail_draw_kind: None,
            draws: 0,
            suspends: 0,
            resumes: 0,
        }
    }
}

impl TuiRenderer for ScriptedRenderer {
    fn draw(&mut self, _state: &AppState) -> io::Result<()> {
        self.draws += 1;
        if let Some(kind) = self.fail_draw_kind {
            return Err(io::Error::new(kind, "simulated ssh drop on draw"));
        }
        Ok(())
    }

    fn read_event(&mut self) -> io::Result<MenuEvent> {
        self.events.pop_front().unwrap_or(Ok(MenuEvent::Quit))
    }

    fn suspend_for_child(&mut self) -> io::Result<()> {
        self.suspends += 1;
        Ok(())
    }

    fn resume_after_child(&mut self) -> io::Result<()> {
        self.resumes += 1;
        Ok(())
    }
}

#[test]
fn truncate_to_width_sweep_0_to_24_fe0f_cjk_and_ansi() {
    let samples = [
        "🛠️ 运维与终端工具集",
        "🛰️ Xray Core 节点管理",
        "⚙️ 脚本维护 (更新/卸载)",
        "🗂️ Yazi 极速终端文件管理器",
        "🗑️ 彻底卸载脚本及资产",
        "🩺 系统冲突巡检与稳态接管",
        "\x1b[1;32m🛠️ 中文Mixed\x1b[0m\x1b[36m🛰️⚙️\x1b[0m",
    ];

    for sample in samples {
        for max_w in 0..=24 {
            let truncated = truncate_to_width(sample, max_w);
            assert!(
                visual_width(&truncated) <= max_w,
                "width overflow for {sample:?} at max_w={max_w}: got {truncated:?} (w={})",
                visual_width(&truncated)
            );
        }
    }
}

#[test]
fn non_interactive_tty_and_ci_full_truth_table() {
    let truth_table = [
        (None, true, true, false),
        (Some(""), true, true, false),
        (None, false, true, true),
        (None, true, false, true),
        (None, false, false, true),
        (Some("true"), true, true, true),
        (Some("1"), true, true, true),
        (Some("false"), true, true, true), // Any non-empty CI marks non-interactive
    ];
    for (ci, stdin_tty, stdout_tty, expected) in truth_table {
        assert_eq!(
            is_non_interactive_with_tty(ci, stdin_tty, stdout_tty),
            expected,
            "ci={ci:?} stdin={stdin_tty} stdout={stdout_tty}"
        );
    }
}

#[test]
fn state_machine_backspace_prefix_resize_refresh_and_error_skip_resume() {
    // 1. Backspace clears digit_prefix first without exiting Main
    let mut state = AppState::new(sample_status());
    assert_eq!(state.handle_event(MenuEvent::Digit(1)), None);
    assert_eq!(state.digit_prefix(), Some(1));
    assert_eq!(state.handle_event(MenuEvent::Back), None);
    assert_eq!(state.digit_prefix(), None);
    assert_eq!(state.current_menu(), MenuId::Main);
    assert_eq!(state.handle_event(MenuEvent::Back), Some(TuiOutcome::Exit));

    // 2. Resize & Refresh trigger redraw/reprobe without suspend/resume
    let mut renderer = ScriptedRenderer::new([
        Ok(MenuEvent::Resize),
        Ok(MenuEvent::Refresh),
        Ok(MenuEvent::Resize),
        Ok(MenuEvent::Quit),
    ]);
    let mut state = AppState::new(sample_status());
    let mut refreshes = 0usize;
    run_tui_loop_with_state(&mut renderer, &mut state, &mut |outcome, _| {
        assert_eq!(outcome, TuiOutcome::Refresh);
        refreshes += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(refreshes, 1);
    assert_eq!(renderer.suspends, 0);
    assert_eq!(renderer.resumes, 0);
    assert_eq!(renderer.draws, 4);

    // 3. Failed child action skips resume_after_child
    let mut fail_renderer = ScriptedRenderer::new([Ok(MenuEvent::Digit(2))]);
    let mut state = AppState::new(sample_status());
    let err = run_tui_loop_with_state(&mut fail_renderer, &mut state, &mut |_, _| {
        Err(TuiError::Io(io::Error::other("child crashed")))
    })
    .unwrap_err();
    assert!(err.to_string().contains("child crashed"));
    assert_eq!(fail_renderer.suspends, 1);
    assert_eq!(fail_renderer.resumes, 0);

    // 4. RW-TUI-01: BrokenPipe & UnexpectedEof on draw() or read_event() exit Ok cleanly
    for kind in [io::ErrorKind::BrokenPipe, io::ErrorKind::UnexpectedEof] {
        let mut draw_drop = ScriptedRenderer::new([]);
        draw_drop.fail_draw_kind = Some(kind);
        let mut st = AppState::new(sample_status());
        run_tui_loop(&mut draw_drop, &mut st, &mut |_| Ok(())).unwrap();

        let mut read_drop = ScriptedRenderer::new([Err(io::Error::new(kind, "ssh drop"))]);
        run_tui_loop(&mut read_drop, &mut st, &mut |_| Ok(())).unwrap();
    }
}

#[cfg(feature = "ratatui-ui")]
mod ratatui_boundary {
    use std::cell::RefCell;
    use std::rc::Rc;

    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use debopty_bridge::AppId;
    use debopty_tui::{
        TerminalGuard, TerminalOps, map_crossterm_key, panic_restore_is_armed, render_frame,
    };
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Op {
        EnableRaw,
        DisableRaw,
        EnterAlt,
        LeaveAlt,
    }

    struct ChaosTerminalOps {
        log: Rc<RefCell<Vec<Op>>>,
        fail_disable_raw_first: bool,
        fail_leave_alt_broken_pipe_first: bool,
        disable_raw_calls: usize,
        leave_alt_calls: usize,
    }

    impl ChaosTerminalOps {
        fn new(log: Rc<RefCell<Vec<Op>>>) -> Self {
            Self {
                log,
                fail_disable_raw_first: false,
                fail_leave_alt_broken_pipe_first: false,
                disable_raw_calls: 0,
                leave_alt_calls: 0,
            }
        }
    }

    impl TerminalOps for ChaosTerminalOps {
        fn enable_raw_mode(&mut self) -> io::Result<()> {
            self.log.borrow_mut().push(Op::EnableRaw);
            Ok(())
        }

        fn disable_raw_mode(&mut self) -> io::Result<()> {
            self.disable_raw_calls += 1;
            self.log.borrow_mut().push(Op::DisableRaw);
            if self.fail_disable_raw_first && self.disable_raw_calls == 1 {
                return Err(io::Error::other("disable_raw transient failure"));
            }
            Ok(())
        }

        fn enter_alt_screen(&mut self) -> io::Result<()> {
            self.log.borrow_mut().push(Op::EnterAlt);
            Ok(())
        }

        fn leave_alt_screen(&mut self) -> io::Result<()> {
            self.leave_alt_calls += 1;
            self.log.borrow_mut().push(Op::LeaveAlt);
            if self.fail_leave_alt_broken_pipe_first && self.leave_alt_calls == 1 {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "broken pipe on leave_alt",
                ));
            }
            Ok(())
        }
    }

    fn key(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn rw_tui_01_broken_pipe_term_dumb_and_zero_resize_cleanup() {
        let status = sample_status();
        let all_menus = [
            MenuId::Main,
            MenuId::Devops,
            MenuId::Podman,
            MenuId::RustStack,
            MenuId::GoStack,
            MenuId::WarpStack,
            MenuId::Maintenance,
            MenuId::AppCrud(AppId::Fish),
        ];
        let sizes = [
            (0u16, 0u16),
            (1, 1),
            (2, 2),
            (5, 3),
            (10, 4),
            (20, 6),
            (79, 23),
            (89, 24),
            (90, 24),
            (200, 60),
        ];

        for &(w, h) in &sizes {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            for menu in all_menus {
                let mut state = AppState::new(status.clone());
                match menu {
                    MenuId::Main => {}
                    MenuId::Devops => {
                        state.handle_event(MenuEvent::Digit(3));
                    }
                    MenuId::Podman => {
                        state.handle_event(MenuEvent::Digit(4));
                    }
                    MenuId::RustStack => {
                        state.handle_event(MenuEvent::Digit(5));
                    }
                    MenuId::GoStack => {
                        state.handle_event(MenuEvent::Digit(6));
                    }
                    MenuId::WarpStack => {
                        state.handle_event(MenuEvent::Digit(8));
                    }
                    MenuId::Maintenance => {
                        state.handle_event(MenuEvent::Digit(1));
                        state.handle_event(MenuEvent::Digit(2));
                    }
                    MenuId::AppCrud(_) => {
                        state.handle_event(MenuEvent::Digit(3));
                        state.handle_event(MenuEvent::Digit(1));
                    }
                }
                terminal
                    .draw(|frame| render_frame(frame, &state))
                    .unwrap_or_else(|e| panic!("render panic at {w}x{h} {menu:?}: {e}"));
            }

            // Confirm modal open at every size including 0x0 and 20x6
            let mut modal_state = AppState::new(status.clone());
            modal_state.handle_event(MenuEvent::Digit(4));
            modal_state.handle_event(MenuEvent::Digit(2));
            assert!(modal_state.pending_confirm().is_some());
            terminal
                .draw(|frame| render_frame(frame, &modal_state))
                .unwrap();
        }

        // Guarded renderer simulates BrokenPipe on draw and confirms RAII Drop leaves alt + disables raw
        struct GuardedDropRenderer {
            _guard: TerminalGuard<ChaosTerminalOps>,
        }
        impl TuiRenderer for GuardedDropRenderer {
            fn draw(&mut self, _state: &AppState) -> io::Result<()> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "ssh disconnect"))
            }
            fn read_event(&mut self) -> io::Result<MenuEvent> {
                Ok(MenuEvent::Quit)
            }
            fn suspend_for_child(&mut self) -> io::Result<()> {
                Ok(())
            }
            fn resume_after_child(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let log = Rc::new(RefCell::new(Vec::new()));
        {
            let guard = TerminalGuard::enter_with(ChaosTerminalOps::new(Rc::clone(&log))).unwrap();
            let mut r = GuardedDropRenderer { _guard: guard };
            let mut st = AppState::new(status);
            run_tui_loop(&mut r, &mut st, &mut |_| Ok(())).unwrap();
        }
        assert_eq!(
            *log.borrow(),
            vec![Op::EnableRaw, Op::EnterAlt, Op::LeaveAlt, Op::DisableRaw]
        );
    }

    #[test]
    fn terminal_guard_retry_flags_and_panic_arm_gate() {
        // 1. disable_raw_mode failure on suspend retries ONLY disable_raw_mode on Drop
        let log1 = Rc::new(RefCell::new(Vec::new()));
        {
            let mut ops = ChaosTerminalOps::new(Rc::clone(&log1));
            ops.fail_disable_raw_first = true;
            let mut guard = TerminalGuard::enter_with(ops).unwrap();
            assert!(!panic_restore_is_armed());
            assert!(guard.suspend().is_err());
            assert!(!guard.is_alt_active());
            assert!(guard.is_raw_active());
        }
        assert_eq!(
            *log1.borrow(),
            vec![
                Op::EnableRaw,
                Op::EnterAlt,
                Op::LeaveAlt,
                Op::DisableRaw,
                Op::DisableRaw,
            ]
        );

        // 2. leave_alt_screen BrokenPipe on suspend retries ONLY leave_alt_screen on Drop
        let log2 = Rc::new(RefCell::new(Vec::new()));
        {
            let mut ops = ChaosTerminalOps::new(Rc::clone(&log2));
            ops.fail_leave_alt_broken_pipe_first = true;
            let mut guard = TerminalGuard::enter_with(ops).unwrap();
            let err = guard.suspend().unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
            assert!(guard.is_alt_active());
            assert!(!guard.is_raw_active());
        }
        assert_eq!(
            *log2.borrow(),
            vec![
                Op::EnableRaw,
                Op::EnterAlt,
                Op::LeaveAlt,
                Op::DisableRaw,
                Op::LeaveAlt,
            ]
        );
    }

    #[test]
    fn crossterm_keymap_rejects_ctrl_y_and_non_arrow_repeat_accepts_arrow_repeat() {
        assert_eq!(
            map_crossterm_key(key(
                KeyCode::Char('y'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press
            )),
            None
        );
        assert_eq!(
            map_crossterm_key(key(
                KeyCode::Char('1'),
                KeyModifiers::ALT,
                KeyEventKind::Press
            )),
            None
        );
        assert_eq!(
            map_crossterm_key(key(KeyCode::Up, KeyModifiers::NONE, KeyEventKind::Repeat)),
            Some(MenuEvent::Up)
        );
        assert_eq!(
            map_crossterm_key(key(KeyCode::Down, KeyModifiers::NONE, KeyEventKind::Repeat)),
            Some(MenuEvent::Down)
        );
        assert_eq!(
            map_crossterm_key(key(
                KeyCode::Enter,
                KeyModifiers::NONE,
                KeyEventKind::Repeat
            )),
            None
        );
        assert_eq!(
            map_crossterm_key(key(
                KeyCode::Char('y'),
                KeyModifiers::NONE,
                KeyEventKind::Repeat
            )),
            None
        );
        assert_eq!(
            map_crossterm_key(key(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Quit)
        );
        for vim in ['h', 'j', 'k', 'l'] {
            assert_eq!(
                map_crossterm_key(key(
                    KeyCode::Char(vim),
                    KeyModifiers::NONE,
                    KeyEventKind::Press
                )),
                None
            );
        }
    }
}
