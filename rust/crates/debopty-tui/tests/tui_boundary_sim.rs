use std::collections::{BTreeMap, VecDeque};
use std::io;

use debopty_core::status::{StatusSnapshot, TRACKED_TOOLS};
use debopty_tui::layout::{
    tail_truncate_to_width, truncate_to_width, truncate_with_ellipsis, visual_width,
    wrap_text_to_width,
};
use debopty_tui::{
    ActivePane, AppState, MenuEvent, MenuId, TuiError, TuiOutcome, TuiRenderer,
    is_non_interactive_with_tty, run_tui_loop, run_tui_loop_with_state,
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

    // 4. RW-TUI-01: BrokenPipe & UnexpectedEof on draw(), read_event(), suspend_for_child(), resume_after_child() exit Ok cleanly
    for kind in [io::ErrorKind::BrokenPipe, io::ErrorKind::UnexpectedEof] {
        let mut draw_drop = ScriptedRenderer::new([]);
        draw_drop.fail_draw_kind = Some(kind);
        let mut st = AppState::new(sample_status());
        run_tui_loop(&mut draw_drop, &mut st, &mut |_| Ok(())).unwrap();

        let mut read_drop = ScriptedRenderer::new([Err(io::Error::new(kind, "ssh drop"))]);
        run_tui_loop(&mut read_drop, &mut st, &mut |_| Ok(())).unwrap();
    }

    // 5. FE-C2: ScriptUninstall triggers confirmation modal & request_exit skips resume_after_child
    let mut uninstall_renderer = ScriptedRenderer::new([
        Ok(MenuEvent::Digit(1)),
        Ok(MenuEvent::Digit(2)),
        Ok(MenuEvent::Digit(2)),
        Ok(MenuEvent::ConfirmYes),
    ]);
    let mut st = AppState::new(sample_status());
    let mut ran_uninstall = false;
    run_tui_loop_with_state(&mut uninstall_renderer, &mut st, &mut |outcome, state| {
        assert_eq!(
            outcome,
            TuiOutcome::Execute(debopty_bridge::BashAction::ScriptUninstall)
        );
        ran_uninstall = true;
        state.request_exit();
        Ok(())
    })
    .unwrap();
    assert!(ran_uninstall);
    assert_eq!(uninstall_renderer.suspends, 1);
    assert_eq!(uninstall_renderer.resumes, 0);
}

#[test]
fn split_to_narrow_resize_resets_telemetry_focus_to_menu() {
    struct ResizeSplitRenderer {
        events: VecDeque<(MenuEvent, bool)>,
        split_mode: bool,
    }

    impl TuiRenderer for ResizeSplitRenderer {
        fn draw(&mut self, _state: &AppState) -> io::Result<()> {
            Ok(())
        }

        fn read_event(&mut self) -> io::Result<MenuEvent> {
            if let Some((ev, next_split)) = self.events.pop_front() {
                self.split_mode = next_split;
                Ok(ev)
            } else {
                Ok(MenuEvent::Quit)
            }
        }

        fn suspend_for_child(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn resume_after_child(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn sync_viewport(&self, state: &mut AppState) {
            state.set_split_pane_available(self.split_mode);
        }
    }

    // Start wide (split=true) -> Tab to Telemetry -> scroll down -> resize narrow (split=false)
    // -> Down moves Menu selection (not Telemetry!) -> Digit(2) executes ToggleIpForwarding -> Quit.
    let mut renderer = ResizeSplitRenderer {
        events: VecDeque::from([
            (MenuEvent::TogglePane, true),
            (MenuEvent::Down, true),
            (MenuEvent::Resize, false),
            (MenuEvent::Down, false),
            (MenuEvent::Digit(2), false),
            (MenuEvent::Quit, false),
        ]),
        split_mode: true,
    };
    let mut state = AppState::new(sample_status());
    let mut dispatched = Vec::new();
    run_tui_loop_with_state(&mut renderer, &mut state, &mut |outcome, st| {
        assert_eq!(st.active_pane(), ActivePane::Menu);
        assert!(!st.split_pane_available());
        dispatched.push(outcome);
        Ok(())
    })
    .unwrap();

    assert_eq!(state.active_pane(), ActivePane::Menu);
    assert_eq!(state.telemetry_scroll(), 1);
    assert_eq!(
        dispatched,
        vec![TuiOutcome::Execute(
            debopty_bridge::BashAction::ToggleIpForwarding
        )]
    );
}

#[test]
fn wrap_and_tail_truncate_respect_tokens_cjk_and_width_bounds() {
    let wrapped_msg = wrap_text_to_width("? 确定要移除 Podman 环境吗？[y/N]", 32);
    assert!(
        wrapped_msg.iter().any(|l| l.contains("[y/N]")),
        "expected unbroken [y/N]: {wrapped_msg:?}"
    );
    for l in &wrapped_msg {
        assert!(visual_width(l) <= 32);
    }

    let wrapped_hint = wrap_text_to_width("按 y 确认执行，按 n / Esc / Enter 取消", 32);
    assert!(
        wrapped_hint.iter().any(|l| l.contains("Enter")),
        "expected unbroken Enter: {wrapped_hint:?}"
    );
    for l in &wrapped_hint {
        assert!(visual_width(l) <= 32);
    }

    let long_q = "中文超长筛选词测试1234567890";
    for max_w in 0..=40 {
        let tail = tail_truncate_to_width(long_q, max_w);
        assert!(
            visual_width(&tail) <= max_w,
            "tail overflow at max_w={max_w}: {tail:?}"
        );
        if max_w >= 5 && max_w < visual_width(long_q) {
            assert!(
                tail.starts_with('…'),
                "expected ellipsis at max_w={max_w}: {tail:?}"
            );
            assert!(
                tail.ends_with("7890"),
                "expected tail preserved at max_w={max_w}: {tail:?}"
            );
        }
    }
}

#[test]
fn idle_ticks_produce_zero_redraws_and_transitions_stop_at_wall_clock_deadline() {
    use std::time::{Duration, Instant};

    use debopty_tui::{MotionPolicy, TransitionKind, WakeKind};

    let mut renderer = ScriptedRenderer::new([
        Ok(MenuEvent::Tick),
        Ok(MenuEvent::Tick),
        Ok(MenuEvent::Tick),
        Ok(MenuEvent::Quit),
    ]);
    let mut state = AppState::new(sample_status());
    run_tui_loop(&mut renderer, &mut state, &mut |_| Ok(())).unwrap();
    // Only the initial frame is drawn; 3 idle ticks produce 0 extra redraws.
    assert_eq!(renderer.draws, 1);
    assert!(!state.is_dirty());

    // Wall-clock transition wakes while active and settles cleanly at its deadline.
    let mut anim_state = AppState::new(sample_status());
    let policy = MotionPolicy::detect_from(false, Some("xterm-256color"), None, None, None);
    anim_state.set_motion_policy(policy);
    assert!(anim_state.take_dirty());

    anim_state.handle_event(MenuEvent::NextTab);
    assert!(anim_state.is_dirty());
    let tr = anim_state
        .active_transition()
        .expect("tab switch starts transition");
    assert_eq!(tr.kind, TransitionKind::TabSwitch { dir: 1 });

    let mid = tr.started_at + tr.duration / 2;
    let wake = anim_state
        .next_wake_at(mid, true)
        .expect("active transition schedules wake");
    assert_eq!(wake.kind, WakeKind::AnimFrame);
    assert!(anim_state.advance_clock_at(mid));
    assert!(anim_state.active_transition().is_some());

    let after = tr.started_at + tr.duration + Duration::from_millis(5);
    assert!(anim_state.advance_clock_at(after));
    assert!(anim_state.active_transition().is_none());

    // Subsequent idle clock advances produce 0 changes and 0 wakes.
    assert!(anim_state.take_dirty());
    assert!(!anim_state.advance_clock_at(after + Duration::from_secs(1)));
    assert!(!anim_state.is_dirty());
    assert_eq!(anim_state.next_wake_at(Instant::now(), false), None);
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
        EnableMouse,
        DisableMouse,
    }

    struct ChaosTerminalOps {
        log: Rc<RefCell<Vec<Op>>>,
        fail_disable_raw_first: bool,
        fail_leave_alt_broken_pipe_first: bool,
        fail_enable_mouse_first: bool,
        fail_disable_mouse_first: bool,
        disable_raw_calls: usize,
        leave_alt_calls: usize,
        enable_mouse_calls: usize,
        disable_mouse_calls: usize,
    }

    impl ChaosTerminalOps {
        fn new(log: Rc<RefCell<Vec<Op>>>) -> Self {
            Self {
                log,
                fail_disable_raw_first: false,
                fail_leave_alt_broken_pipe_first: false,
                fail_enable_mouse_first: false,
                fail_disable_mouse_first: false,
                disable_raw_calls: 0,
                leave_alt_calls: 0,
                enable_mouse_calls: 0,
                disable_mouse_calls: 0,
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

        fn enable_mouse_capture(&mut self) -> io::Result<()> {
            self.enable_mouse_calls += 1;
            self.log.borrow_mut().push(Op::EnableMouse);
            if self.fail_enable_mouse_first && self.enable_mouse_calls == 1 {
                return Err(io::Error::other("enable_mouse transient failure"));
            }
            Ok(())
        }

        fn disable_mouse_capture(&mut self) -> io::Result<()> {
            self.disable_mouse_calls += 1;
            self.log.borrow_mut().push(Op::DisableMouse);
            if self.fail_disable_mouse_first && self.disable_mouse_calls == 1 {
                return Err(io::Error::other("disable_mouse transient failure"));
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
            MenuId::Xray,
            MenuId::Freship,
            MenuId::Lego,
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
            (60, 18),
            (71, 23),
            (72, 24),
            (80, 24),
            (120, 36),
            (160, 44),
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
                    MenuId::Xray => {
                        state.handle_event(MenuEvent::Digit(7));
                    }
                    MenuId::WarpStack => {
                        state.handle_event(MenuEvent::Digit(8));
                    }
                    MenuId::Freship => {
                        state.navigate_to(MenuId::Freship);
                    }
                    MenuId::Lego => {
                        state.handle_event(MenuEvent::Digit(3));
                        state.handle_event(MenuEvent::Digit(4));
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
            vec![
                Op::EnableRaw,
                Op::EnterAlt,
                Op::EnableMouse,
                Op::DisableMouse,
                Op::LeaveAlt,
                Op::DisableRaw,
            ]
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
            assert!(!guard.is_mouse_active());
            assert!(!guard.is_alt_active());
            assert!(guard.is_raw_active());
        }
        assert_eq!(
            *log1.borrow(),
            vec![
                Op::EnableRaw,
                Op::EnterAlt,
                Op::EnableMouse,
                Op::DisableMouse,
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
            assert!(!guard.is_mouse_active());
            assert!(guard.is_alt_active());
            assert!(!guard.is_raw_active());
        }
        assert_eq!(
            *log2.borrow(),
            vec![
                Op::EnableRaw,
                Op::EnterAlt,
                Op::EnableMouse,
                Op::DisableMouse,
                Op::LeaveAlt,
                Op::DisableRaw,
                Op::LeaveAlt,
            ]
        );

        // 3. enable_mouse_capture failure rolls back alt + raw cleanly
        let log3 = Rc::new(RefCell::new(Vec::new()));
        {
            let mut ops = ChaosTerminalOps::new(Rc::clone(&log3));
            ops.fail_enable_mouse_first = true;
            assert!(TerminalGuard::enter_with(ops).is_err());
        }
        assert_eq!(
            *log3.borrow(),
            vec![
                Op::EnableRaw,
                Op::EnterAlt,
                Op::EnableMouse,
                Op::LeaveAlt,
                Op::DisableRaw,
            ]
        );

        // 4. disable_mouse_capture failure on suspend retries ONLY DisableMouse on Drop
        let log4 = Rc::new(RefCell::new(Vec::new()));
        {
            let mut ops = ChaosTerminalOps::new(Rc::clone(&log4));
            ops.fail_disable_mouse_first = true;
            let mut guard = TerminalGuard::enter_with(ops).unwrap();
            assert!(guard.suspend().is_err());
            assert!(guard.is_mouse_active());
            assert!(!guard.is_alt_active());
            assert!(!guard.is_raw_active());
        }
        assert_eq!(
            *log4.borrow(),
            vec![
                Op::EnableRaw,
                Op::EnterAlt,
                Op::EnableMouse,
                Op::DisableMouse,
                Op::LeaveAlt,
                Op::DisableRaw,
                Op::DisableMouse,
            ]
        );
    }

    #[test]
    fn crossterm_keymap_supports_micro_yazi_and_rejects_invalid_modifiers() {
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
        assert_eq!(
            map_crossterm_key(key(
                KeyCode::Char('q'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Quit)
        );
        assert_eq!(
            map_crossterm_key(key(
                KeyCode::Char('f'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press
            )),
            Some(MenuEvent::FilterStart)
        );
        assert_eq!(
            map_crossterm_key(key(
                KeyCode::Char('p'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press
            )),
            Some(MenuEvent::PaletteOpen)
        );
        assert_eq!(
            map_crossterm_key(key(
                KeyCode::Char('j'),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Down)
        );
        assert_eq!(
            map_crossterm_key(key(
                KeyCode::Char('k'),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Up)
        );
        assert_eq!(
            map_crossterm_key(key(
                KeyCode::Char('h'),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::PrevTab)
        );
        assert_eq!(
            map_crossterm_key(key(
                KeyCode::Char('l'),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::NextTab)
        );
        assert_eq!(
            map_crossterm_key(key(
                KeyCode::Char('g'),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Home)
        );
        assert_eq!(
            map_crossterm_key(key(
                KeyCode::Char('G'),
                KeyModifiers::SHIFT,
                KeyEventKind::Press
            )),
            Some(MenuEvent::End)
        );
        assert_eq!(
            map_crossterm_key(key(
                KeyCode::Char('x'),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::DismissTaskLog)
        );
        assert_eq!(
            map_crossterm_key(key(
                KeyCode::Char(' '),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::FormToggle)
        );
    }

    #[test]
    fn u39_u40_u45_u46_u47_u48_responsive_live_panel_and_structured_inspector() {
        use std::time::Duration;

        use debopty_core::telemetry::TelemetryHistory;
        use debopty_tui::{
            ShowcaseOptions, TaskRunState, TaskRunStatus, UiPalette, WorkspaceTab,
            render_frame_with_palette, render_showcase_frame_with_options, sample_showcase_status,
            sample_showcase_task_run,
        };

        let status = sample_showcase_status();

        // 1. U48 responsive matrix across 60x18 (single), 80x24 (split 38/62), 120x36 (split 33/67), 160x44 (capped 44)
        for &(w, h, expect_overview) in &[
            (60u16, 18u16, false),
            (80, 24, true),
            (120, 36, true),
            (160, 44, true),
        ] {
            let rendered = render_showcase_frame_with_options(
                MenuId::Main,
                &status,
                w,
                h,
                &ShowcaseOptions::default(),
            );
            assert_eq!(
                rendered.contains("系统概览"),
                expect_overview,
                "unexpected overview visibility at {w}x{h}:\n{rendered}"
            );
            for (row_idx, line) in rendered.lines().enumerate() {
                assert_eq!(
                    visual_width(line),
                    w as usize,
                    "responsive width mismatch at {w}x{h} row {row_idx}: {line:?}"
                );
            }
            // U40 clean header check: top bar has debopty + CN · v2.0.0 and zero hostname/up/count clutter.
            let top = rendered.lines().nth(1).unwrap_or("");
            assert!(top.contains("debopty"), "missing debopty at {w}x{h}");
            assert!(
                !top.contains("deb-edge-01"),
                "redundant hostname at {w}x{h}"
            );
            assert!(!top.contains("up "), "redundant uptime at {w}x{h}");
            assert!(!top.contains("8/18"), "redundant tool count at {w}x{h}");
        }

        // Wide 160x44 shows full nftables(...) · fail2ban(...) in Telemetry.
        let wide_rendered = render_showcase_frame_with_options(
            MenuId::Main,
            &status,
            160,
            44,
            &ShowcaseOptions::default(),
        );
        assert!(
            wide_rendered.contains("nftables(") && wide_rendered.contains("fail2ban("),
            "expected full nftables/fail2ban labels at 160x44:\n{wide_rendered}"
        );

        // 2. U45 + U46: Non-Overview tabs & submenus hide System Overview and dedicate full right pane to structured Inspector.
        for tab in [
            WorkspaceTab::SysSec,
            WorkspaceTab::NetProxy,
            WorkspaceTab::DevOps,
            WorkspaceTab::Reconcile,
        ] {
            let tab_frame = render_showcase_frame_with_options(
                MenuId::Main,
                &status,
                120,
                36,
                &ShowcaseOptions {
                    tab,
                    ..ShowcaseOptions::default()
                },
            );
            assert!(
                !tab_frame.contains("系统概览"),
                "U45: non-Overview tab {tab:?} must NOT show System Overview:\n{tab_frame}"
            );
            assert!(
                tab_frame.contains("状态与作用")
                    && tab_frame.contains("核心参数")
                    && tab_frame.contains("配置文件与预览"),
                "U46: spacious tab {tab:?} Inspector must render 3 structured sections:\n{tab_frame}"
            );
        }

        for submenu in [MenuId::Devops, MenuId::Xray, MenuId::Freship, MenuId::Lego] {
            let sub_frame = render_showcase_frame_with_options(
                submenu,
                &status,
                120,
                36,
                &ShowcaseOptions::default(),
            );
            assert!(
                !sub_frame.contains("系统概览"),
                "U45: submenu {submenu:?} must NOT show System Overview:\n{sub_frame}"
            );
            assert!(
                sub_frame.contains("状态与作用") && sub_frame.contains("核心参数"),
                "U46: submenu {submenu:?} must show structured Inspector:\n{sub_frame}"
            );
        }

        // 3. U39 ShowcaseOptions::task_run renders live right pane (wide) and full body (narrow < 72).
        let live_frame = render_showcase_frame_with_options(
            MenuId::Main,
            &status,
            120,
            36,
            &ShowcaseOptions {
                task_run: Some(sample_showcase_task_run()),
                ..ShowcaseOptions::default()
            },
        );
        assert!(live_frame.contains("基础系统优化"));
        assert!(live_frame.contains("00:04.2"));
        assert!(live_frame.contains("执行中"));
        assert!(live_frame.contains("net.ipv4.tcp_congestion_control = bbr"));
        assert!(!live_frame.contains("系统概览"));

        let done_task = TaskRunState::preview(
            "Fail2ban 端口同步",
            TaskRunStatus::Succeeded,
            Duration::from_millis(1300),
            Some("端口同步完成".to_string()),
            vec!["[INFO] 已同步 SSH=2222".to_string()],
        );
        let narrow_done = render_showcase_frame_with_options(
            MenuId::Main,
            &status,
            60,
            18,
            &ShowcaseOptions {
                task_run: Some(done_task),
                ..ShowcaseOptions::default()
            },
        );
        assert!(narrow_done.contains("Fail2ban 端口同步"));
        assert!(narrow_done.contains("00:01.3"));
        assert!(narrow_done.contains("Esc/x"));

        // 4. U41 Form modal masks API token so plaintext secret NEVER appears in terminal buffer.
        let mut form_state = AppState::new(status.clone());
        let (specs, history) = TelemetryHistory::demo_fixture(0);
        form_state.set_telemetry(specs, history);
        form_state.open_lego_add_domain_form();
        for ch in "example.com".chars() {
            form_state.handle_event(MenuEvent::FilterChar(ch));
        }
        form_state.handle_event(MenuEvent::Down); // san
        form_state.handle_event(MenuEvent::Down); // email
        for ch in "ops@example.com".chars() {
            form_state.handle_event(MenuEvent::FilterChar(ch));
        }
        form_state.handle_event(MenuEvent::Down); // provider
        form_state.handle_event(MenuEvent::Down); // cf_token (masked!)
        for ch in "super_secret_cf_token_999".chars() {
            form_state.handle_event(MenuEvent::FilterChar(ch));
        }
        let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
        terminal
            .draw(|f| render_frame_with_palette(f, &form_state, &UiPalette::truecolor()))
            .unwrap();
        let buf = terminal.backend().buffer();
        let mut rendered_form = String::new();
        for y in 0..buf.area.height {
            let mut x = 0u16;
            while x < buf.area.width {
                let sym = buf[(x, y)].symbol();
                rendered_form.push_str(sym);
                x = x.saturating_add(visual_width(sym).max(1) as u16);
            }
            rendered_form.push('\n');
        }
        assert!(rendered_form.contains("添加 Lego 域名托管"));
        assert!(rendered_form.contains("••••••"));
        assert!(
            !rendered_form.contains("super_secret_cf_token_999"),
            "masked API token leaked into rendered frame!"
        );
    }
}

#[test]
fn u39_live_task_sanitization_scrollback_spinner_wake_and_dismiss() {
    use std::time::{Duration, Instant};

    use debopty_tui::{
        ACTIVE_FRAME_INTERVAL, TASK_LOG_MAX_LINES, TaskRunStatus, WakeKind, sanitize_live_log_line,
    };

    // 1. ANSI CSI/OSC/ESC, \r, BEL (\x07), and tab sanitization
    let dirty = "\x1b[2J\x1b[H\x1b]0;Window Title\x07\x1b[1;32mhello\x1b[0m\r\x07\tworld\x1b(B!";
    assert_eq!(sanitize_live_log_line(dirty), "hello    world!");

    // 2. Live run: Braille spinner wake ONLY while Running; 0 idle wake after finish
    let mut state = AppState::new(sample_status());
    state.start_task_run("测试任务");
    assert!(state.is_task_running());
    let now = Instant::now();
    let wake = state
        .next_wake_at(now, true)
        .expect("running task must schedule spinner wake");
    assert_eq!(wake.kind, WakeKind::AnimFrame);
    assert_eq!(wake.timeout, ACTIVE_FRAME_INTERVAL);
    assert!(state.advance_clock_at(now + Duration::from_millis(50)));

    // Concurrent action is blocked while task is running
    assert_eq!(state.handle_event(MenuEvent::Digit(2)), None);
    assert!(state.notice().unwrap_or("").contains("正在执行"));

    // Ring buffer caps at TASK_LOG_MAX_LINES (2000)
    for i in 0..(TASK_LOG_MAX_LINES + 50) {
        state.push_task_log_line(&format!("line-{i}\r\x07"));
    }
    let task = state.task_run().expect("task active");
    assert_eq!(task.lines.len(), TASK_LOG_MAX_LINES);
    assert_eq!(task.lines.front().map(String::as_str), Some("line-50"));
    assert_eq!(task.lines.back().map(String::as_str), Some("line-2049"));
    assert!(task.auto_scroll);

    // Scrolling up disables auto_scroll; End re-enables auto_scroll
    state.set_task_log_viewport_height(20);
    state.handle_event(MenuEvent::ScrollTaskLog(-5));
    let task = state.task_run().expect("task active");
    assert!(!task.auto_scroll);
    assert_eq!(task.scroll_offset, TASK_LOG_MAX_LINES - 20 - 5);

    state.handle_event(MenuEvent::FocusPane(ActivePane::Telemetry));
    state.handle_event(MenuEvent::Home);
    assert_eq!(state.task_run().expect("task").scroll_offset, 0);
    state.handle_event(MenuEvent::End);
    assert!(state.task_run().expect("task").auto_scroll);

    // Cannot dismiss while still Running via 'x' or Esc
    state.handle_event(MenuEvent::DismissTaskLog);
    assert!(state.task_run().is_some());

    // Finish task -> 0 idle wake once toast cleared
    state.clear_notice();
    state.finish_task_run(true, "执行成功");
    assert_eq!(
        state.task_run().expect("task").status,
        TaskRunStatus::Succeeded
    );
    assert_eq!(state.next_wake_at(Instant::now(), true), None);
    assert!(!state.advance_clock_at(Instant::now() + Duration::from_secs(1)));

    // Dismiss via Esc on Main (when Menu focused)
    state.set_active_pane(ActivePane::Menu);
    assert_eq!(state.handle_event(MenuEvent::Back), None);
    assert!(state.task_run().is_none());

    // Dismiss via 'x' when finished
    state.start_task_run("测试任务2");
    state.finish_task_run(false, "执行失败");
    state.handle_event(MenuEvent::DismissTaskLog);
    assert!(state.task_run().is_none());
}

#[test]
fn u43_u44_workspace_tabs_have_zero_duplicates_and_chinese_summary_hints() {
    use std::collections::HashSet;

    use debopty_tui::{MenuAction, WorkspaceTab, build_workspace_menu};

    for freship_installed in [false, true] {
        let mut st = sample_status();
        st.tools.insert("freship".to_string(), freship_installed);

        let mut seen_labels: HashSet<&str> = HashSet::new();
        let mut seen_actions: HashSet<String> = HashSet::new();
        for tab in [
            WorkspaceTab::SysSec,
            WorkspaceTab::NetProxy,
            WorkspaceTab::DevOps,
            WorkspaceTab::Reconcile,
        ] {
            let screen = build_workspace_menu(tab, &st);
            for item in screen.items() {
                if item.action == MenuAction::Exit {
                    continue;
                }
                assert!(
                    seen_labels.insert(item.label),
                    "U43 duplicate label {:?} in tab {tab:?} (freship={freship_installed})",
                    item.label
                );
                let action_key = format!("{:?}", item.action);
                assert!(
                    seen_actions.insert(action_key.clone()),
                    "U43 duplicate action {action_key} in tab {tab:?} (freship={freship_installed})"
                );
                // U44: every action has a non-empty concise Chinese summary_hint
                let hint = item.action.summary_hint();
                assert!(
                    !hint.is_empty() && hint.chars().any(|c| c as u32 > 0x7F),
                    "U44 missing Chinese summary_hint for {:?}",
                    item.action
                );
            }
        }

        let syssec_labels: Vec<&str> = build_workspace_menu(WorkspaceTab::SysSec, &st)
            .items()
            .into_iter()
            .filter(|i| i.action != MenuAction::Exit)
            .map(|i| i.label)
            .collect();
        assert_eq!(syssec_labels, vec!["基础优化", "路由转发", "Fail2ban 同步"]);

        let reconcile_labels: Vec<&str> = build_workspace_menu(WorkspaceTab::Reconcile, &st)
            .items()
            .into_iter()
            .filter(|i| i.action != MenuAction::Exit)
            .map(|i| i.label)
            .collect();
        assert_eq!(
            reconcile_labels,
            vec!["稳态巡检", "检查更新", "卸载 debopty"]
        );
    }
}

#[test]
fn u41_native_submenus_and_all_in_tui_modals_submit_without_console_dropouts() {
    use std::path::PathBuf;

    use debopty_core::apps::devops::lego::{LegoDomainStatus, LegoFlag};
    use debopty_core::apps::freship::{
        FreshipCity, FreshipContinent, FreshipCountry, FreshipMap, FreshipState, FreshipWorkMode,
    };
    use debopty_tui::{MenuAction, PickerKind, PromptSubmission};

    let mut state = AppState::new(sample_status());
    state.set_native_prompts(true);

    // 1. Yazi Install opens in-TUI option modal -> toggle preview -> confirm
    state.handle_event(MenuEvent::Digit(3)); // Devops
    state.handle_event(MenuEvent::Digit(3)); // Yazi AppCrud
    assert_eq!(state.handle_event(MenuEvent::Digit(1)), None); // Install opens modal
    assert!(state.active_prompt().is_some());
    state.handle_event(MenuEvent::FormToggle); // check media_preview
    assert_eq!(
        state.handle_event(MenuEvent::ConfirmYes),
        Some(TuiOutcome::PromptSubmitted(PromptSubmission::YaziInstall {
            media_preview: true
        }))
    );
    assert!(state.active_prompt().is_none());

    // 2. Podman Uninstall opens 2-toggle option modal -> check delete_data only -> submit
    state.navigate_to(MenuId::Podman);
    assert_eq!(state.handle_event(MenuEvent::Digit(2)), None);
    assert!(state.active_prompt().is_some());
    // Default cursor is Cancel (3); Down wraps to 0 (delete_data), Space toggles it, ConfirmYes submits
    state.handle_event(MenuEvent::Down);
    state.handle_event(MenuEvent::FormToggle);
    assert_eq!(
        state.handle_event(MenuEvent::ConfirmYes),
        Some(TuiOutcome::PromptSubmitted(
            PromptSubmission::PodmanUninstall {
                delete_data: true,
                delete_user: false,
            }
        ))
    );

    // 3. ScriptUninstall opens delete_config toggle modal -> default false -> submit
    state.navigate_to(MenuId::Maintenance);
    assert_eq!(state.handle_event(MenuEvent::Digit(2)), None);
    assert_eq!(
        state.handle_event(MenuEvent::ConfirmYes),
        Some(TuiOutcome::PromptSubmitted(
            PromptSubmission::ScriptUninstall {
                delete_config: false
            }
        ))
    );

    // 4. Reconcile Apply confirm modal
    state.open_reconcile_apply_confirm(3);
    assert_eq!(
        state.handle_event(MenuEvent::ConfirmYes),
        Some(TuiOutcome::PromptSubmitted(
            PromptSubmission::ReconcileApply
        ))
    );

    // 5. FreshIP 5-step cascading selector (auto-skips single state & single city)
    let map = FreshipMap {
        version: "1".to_string(),
        continents: vec![FreshipContinent {
            id: "asia".to_string(),
            name: "亚洲".to_string(),
            countries: vec![FreshipCountry {
                id: "jp".to_string(),
                name: "日本".to_string(),
                keyword_file: "jp.txt".to_string(),
                states: vec![FreshipState {
                    id: "tokyo".to_string(),
                    name: "东京".to_string(),
                    cities: vec![FreshipCity {
                        id: "tyo".to_string(),
                        name: "东京".to_string(),
                    }],
                }],
            }],
        }],
    };
    state.open_freship_picker(map, true);
    assert_eq!(state.handle_event(MenuEvent::Select), None); // Continent -> Country
    assert_eq!(state.handle_event(MenuEvent::Select), None); // Country -> auto-skip State+City -> WorkMode
    state.handle_event(MenuEvent::Down); // ipv4_only
    state.handle_event(MenuEvent::Down); // ipv6_only
    state.handle_event(MenuEvent::Down); // dual_stack
    let submitted = state.handle_event(MenuEvent::Select);
    match submitted {
        Some(TuiOutcome::PromptSubmitted(PromptSubmission::Freship {
            selection,
            is_reconfigure,
        })) => {
            assert!(is_reconfigure);
            assert_eq!(selection.country_id, "jp");
            assert_eq!(selection.city_id, "tyo");
            assert_eq!(selection.work_mode, Some(FreshipWorkMode::DualStack));
        }
        other => panic!("expected Freship submission, got {other:?}"),
    }

    // 6. Lego submenu + Add-Domain Form modal + Domain/Flag/Mode pickers + LegoUninstall confirm
    state.navigate_to(MenuId::Lego);
    assert_eq!(state.handle_event(MenuEvent::Digit(2)), None); // LegoAddDomain opens form
    assert!(state.is_text_input_active());
    for ch in "app.example.com".chars() {
        state.handle_event(MenuEvent::FilterChar(ch));
    }
    state.handle_event(MenuEvent::Select); // -> san_domains
    for ch in "*.app.example.com".chars() {
        state.handle_event(MenuEvent::FilterChar(ch));
    }
    state.handle_event(MenuEvent::Select); // -> email
    for ch in "admin@example.com".chars() {
        state.handle_event(MenuEvent::FilterChar(ch));
    }
    state.handle_event(MenuEvent::Select); // -> provider (default cloudflare)
    state.handle_event(MenuEvent::Select); // -> cf_token
    for ch in "cf_live_token_test".chars() {
        state.handle_event(MenuEvent::FilterChar(ch));
    }
    state.handle_event(MenuEvent::Select); // -> issue_immediately
    state.handle_event(MenuEvent::FilterChar(' ')); // toggle true
    match state.handle_event(MenuEvent::Select) {
        Some(TuiOutcome::PromptSubmitted(PromptSubmission::LegoAddDomain(spec))) => {
            assert_eq!(spec.primary_domain, "app.example.com");
            assert_eq!(spec.san_domains, vec!["*.app.example.com"]);
            assert_eq!(spec.email, "admin@example.com");
            assert_eq!(spec.provider, "cloudflare");
            assert_eq!(spec.cf_token, "cf_live_token_test");
            assert!(spec.issue_immediately);
        }
        other => panic!("expected LegoAddDomain submission, got {other:?}"),
    }

    let sample_domains = vec![LegoDomainStatus {
        primary_domain: "app.example.com".to_string(),
        domains: "app.example.com".to_string(),
        email: "admin@example.com".to_string(),
        provider: "cloudflare".to_string(),
        auto_renew: true,
        ferron_push: false,
        dns_skip_propagation: false,
        env_path: PathBuf::from("/etc/lego/envs/app.example.com.env"),
        cert_exists: true,
        days_left: Some(65),
    }];
    state.open_lego_domain_picker(PickerKind::LegoToggleFlagDomain, &sample_domains);
    assert_eq!(state.handle_event(MenuEvent::Select), None); // -> flag choice
    state.handle_event(MenuEvent::Down); // DnsSkipPropagation
    assert_eq!(
        state.handle_event(MenuEvent::Select),
        Some(TuiOutcome::PromptSubmitted(
            PromptSubmission::LegoToggleFlag {
                domain: "app.example.com".to_string(),
                flag: LegoFlag::DnsSkipPropagation,
            }
        ))
    );

    state.open_lego_domain_picker(PickerKind::LegoRunOnceDomain, &sample_domains);
    assert_eq!(state.handle_event(MenuEvent::Select), None); // -> mode choice
    state.handle_event(MenuEvent::Down); // issue
    assert_eq!(
        state.handle_event(MenuEvent::Select),
        Some(TuiOutcome::PromptSubmitted(PromptSubmission::LegoRunOnce {
            domain: "app.example.com".to_string(),
            mode: "issue".to_string(),
        }))
    );

    // LegoUninstall opens Confirm modal -> emits RunMenuAction(LegoUninstall)
    assert_eq!(state.handle_event(MenuEvent::Digit(7)), None);
    assert!(
        state
            .pending_confirm()
            .expect("lego uninstall confirm")
            .is_destructive()
    );
    assert_eq!(
        state.handle_event(MenuEvent::ConfirmYes),
        Some(TuiOutcome::RunMenuAction(MenuAction::LegoUninstall))
    );
}

#[test]
fn truncate_with_ellipsis_sweep_0_to_20_cjk_and_ascii() {
    assert_eq!(truncate_with_ellipsis("Swappiness", 0), "");
    assert_eq!(truncate_with_ellipsis("Swappiness", 1), "…");
    assert_eq!(truncate_with_ellipsis("Swappiness", 10), "Swappiness");
    assert_eq!(truncate_with_ellipsis("Swappiness", 6), "Swapp…");
    assert_eq!(truncate_with_ellipsis("中文Mixed测试", 0), "");
    assert_eq!(truncate_with_ellipsis("中文Mixed测试", 1), "…");

    for sample in ["Swappiness", "file-max", "中文Mixed测试1234", "基础优化"] {
        for max_w in 0..=20 {
            let out = truncate_with_ellipsis(sample, max_w);
            assert!(
                visual_width(&out) <= max_w,
                "overflow for {sample:?} at max_w={max_w}: {out:?}"
            );
            if max_w >= 2 && visual_width(sample) > max_w {
                assert!(
                    out.ends_with('…'),
                    "expected trailing ellipsis for {sample:?} at max_w={max_w}: {out:?}"
                );
            }
        }
    }
}

#[test]
fn console_renderer_frame_buffer_uses_in_place_sync_update_with_zero_clear_screen() {
    use debopty_tui::ConsoleRenderer;

    let renderer = ConsoleRenderer::with_width(56);
    let state = AppState::new(sample_status());

    let synced = renderer.render_frame_buffer(&state, true);
    assert!(
        !synced.contains("\x1b[2J"),
        "console frame buffer must never emit full-screen clear \\x1b[2J"
    );
    assert!(synced.starts_with("\x1b[?2026h\x1b[H"));
    assert!(synced.contains("\x1b[K\n"));
    assert!(synced.contains("\x1b[K\x1b[J"));
    assert!(synced.ends_with("\x1b[?2026l"));
    assert_eq!(synced.matches("\x1b[?2026h").count(), 1);
    assert_eq!(synced.matches("\x1b[?2026l").count(), 1);

    let unsynced = renderer.render_frame_buffer(&state, false);
    assert!(!unsynced.contains("\x1b[2J"));
    assert!(unsynced.starts_with("\x1b[H"));
    assert!(unsynced.contains("\x1b[K\n"));
    assert!(unsynced.contains("\x1b[K\x1b[J"));
    assert!(!unsynced.contains("\x1b[?2026h"));
    assert!(!unsynced.contains("\x1b[?2026l"));
}

#[test]
fn coalesce_events_collapses_resize_scroll_and_ticks_without_dropping_actions() {
    use debopty_tui::coalesce_events;

    let input = vec![
        MenuEvent::Resize,
        MenuEvent::Resize,
        MenuEvent::Resize,
        MenuEvent::ScrollTelemetry(2),
        MenuEvent::ScrollTelemetry(2),
        MenuEvent::ScrollTelemetry(-1),
        MenuEvent::Down,
        MenuEvent::Down,
        MenuEvent::ScrollTaskLog(3),
        MenuEvent::ScrollTaskLog(-3), // cancels out cleanly
        MenuEvent::Tick,
        MenuEvent::Tick,
        MenuEvent::TelemetryTick,
        MenuEvent::TelemetryTick,
        MenuEvent::Select,
        MenuEvent::Resize,
    ];
    let out = coalesce_events(input);
    assert_eq!(
        out,
        vec![
            MenuEvent::Resize,
            MenuEvent::ScrollTelemetry(3),
            MenuEvent::Down,
            MenuEvent::Down,
            MenuEvent::Tick,
            MenuEvent::TelemetryTick,
            MenuEvent::Select,
            MenuEvent::Resize,
        ]
    );

    // Clamping saturation prevents overflow panics on huge bursts.
    let huge = coalesce_events(vec![
        MenuEvent::ScrollTelemetry(i16::MAX),
        MenuEvent::ScrollTelemetry(10),
        MenuEvent::ScrollTaskLog(i16::MIN),
        MenuEvent::ScrollTaskLog(-10),
    ]);
    assert_eq!(
        huge,
        vec![
            MenuEvent::ScrollTelemetry(i16::MAX),
            MenuEvent::ScrollTaskLog(i16::MIN),
        ]
    );
}

#[test]
fn frame_pacer_adapts_cadence_and_only_snaps_on_sustained_slow_draws() {
    use std::time::Duration;

    use debopty_tui::{
        FramePacer, LOCAL_FRAME_INTERVAL, MotionPolicy, SSH_FRAME_INTERVAL, SSH_STREAM_INTERVAL,
    };

    let ssh_policy = MotionPolicy::detect_from(
        false,
        Some("xterm-256color"),
        None,
        None,
        Some("10.0.0.1 2222 10.0.0.2 22"),
    );
    assert_eq!(ssh_policy.frame_interval, SSH_FRAME_INTERVAL);

    let mut pacer = FramePacer::new(ssh_policy);
    // Fast draws keep base SSH cadence (33ms) and never snap.
    assert!(!pacer.record_draw(Duration::from_millis(6)));
    assert!(!pacer.record_draw(Duration::from_millis(8)));
    assert_eq!(pacer.effective_frame_interval(), SSH_FRAME_INTERVAL);
    assert_eq!(pacer.recommended_stream_interval(true), SSH_STREAM_INTERVAL);

    // A single transient spike (45ms >= 40ms threshold, < 2x) must NOT snap immediately!
    assert!(!pacer.record_draw(Duration::from_millis(45)));
    assert_eq!(pacer.consecutive_slow(), 1);
    // Next fast draw resets consecutive_slow without snapping.
    assert!(!pacer.record_draw(Duration::from_millis(8)));
    assert_eq!(pacer.consecutive_slow(), 0);

    // Two consecutive slow draws DO snap and widen cadence to 50ms/66ms.
    assert!(!pacer.record_draw(Duration::from_millis(45)));
    assert!(pacer.record_draw(Duration::from_millis(45)));
    assert!(pacer.effective_frame_interval() >= Duration::from_millis(50));

    // Extreme single stall (>= 2 * 40ms = 80ms) snaps immediately.
    let local_policy = MotionPolicy::detect_from(false, Some("xterm-256color"), None, None, None);
    assert_eq!(local_policy.frame_interval, LOCAL_FRAME_INTERVAL);
    let mut local_pacer = FramePacer::new(local_policy);
    assert!(!local_pacer.record_draw(Duration::from_millis(5)));
    assert!(local_pacer.record_draw(Duration::from_millis(85)));
}

#[test]
fn motion_policy_local_vs_ssh_and_env_overrides() {
    use debopty_tui::{
        ACTIVE_FRAME_INTERVAL, LOCAL_FRAME_INTERVAL, MotionPolicy, REDUCED_TRANSITION_DURATION,
        SSH_FRAME_INTERVAL, ease_out_quart, supports_synchronized_output,
    };

    let local = MotionPolicy::detect_from(false, Some("xterm-256color"), None, None, None);
    assert!(local.animations_enabled);
    assert_eq!(local.frame_interval, LOCAL_FRAME_INTERVAL);

    let ssh = MotionPolicy::detect_from(
        false,
        Some("xterm-256color"),
        None,
        None,
        Some("10.0.0.1 1234 10.0.0.2 22"),
    );
    assert!(ssh.animations_enabled);
    assert_eq!(ssh.frame_interval, SSH_FRAME_INTERVAL);

    let ssh_smooth = MotionPolicy::detect_from_with_mode(
        false,
        Some("xterm-256color"),
        None,
        None,
        Some("10.0.0.1 1234 10.0.0.2 22"),
        Some("smooth"),
    );
    assert!(ssh_smooth.animations_enabled);
    assert_eq!(ssh_smooth.frame_interval, LOCAL_FRAME_INTERVAL);

    let reduced = MotionPolicy::detect_from_with_mode(
        false,
        Some("xterm-256color"),
        None,
        None,
        None,
        Some("reduced"),
    );
    assert!(reduced.animations_enabled);
    assert_eq!(reduced.frame_interval, ACTIVE_FRAME_INTERVAL);
    assert_eq!(reduced.transition_duration, REDUCED_TRANSITION_DURATION);

    let off = MotionPolicy::detect_from_with_mode(
        false,
        Some("xterm-256color"),
        None,
        None,
        None,
        Some("off"),
    );
    assert!(!off.animations_enabled);

    // Synchronized Output support matrix.
    assert!(supports_synchronized_output(Some("xterm-256color"), None));
    assert!(supports_synchronized_output(Some("tmux-256color"), None));
    assert!(supports_synchronized_output(Some("alacritty"), Some("0")));
    assert!(supports_synchronized_output(None, None));
    assert!(!supports_synchronized_output(Some("dumb"), None));
    assert!(!supports_synchronized_output(Some("linux"), None));
    assert!(!supports_synchronized_output(Some("vt100"), None));
    assert!(!supports_synchronized_output(
        Some("xterm-256color"),
        Some("1")
    ));

    // ease_out_quart checkpoints + NaN safety.
    assert!((ease_out_quart(0.0) - 0.0).abs() < 1e-12);
    assert!((ease_out_quart(0.5) - 0.9375).abs() < 1e-12);
    assert!((ease_out_quart(1.0) - 1.0).abs() < 1e-12);
    assert!((ease_out_quart(f64::NAN) - 1.0).abs() < 1e-12);
}

#[test]
fn scroll_margin_lookahead_and_overflow_indicator_never_overflow_narrow_bounds() {
    use debopty_tui::{SCROLL_MARGIN, compute_scroll_offset};

    // 20 rows, 10 visible, margin = 2:
    assert_eq!(compute_scroll_offset(0, 20, 10, 0, SCROLL_MARGIN), 0);
    assert_eq!(compute_scroll_offset(7, 20, 10, 0, SCROLL_MARGIN), 0);
    // Moving to row 8 keeps 2 rows (9, 10) visible ahead.
    assert_eq!(compute_scroll_offset(8, 20, 10, 0, SCROLL_MARGIN), 1);
    assert_eq!(compute_scroll_offset(9, 20, 10, 1, SCROLL_MARGIN), 2);
    // Moving to bottom clamps cleanly at len - visible_rows = 10.
    assert_eq!(compute_scroll_offset(19, 20, 10, 2, SCROLL_MARGIN), 10);
    // Moving back up from bottom triggers lookahead at row 11 (offset 10 + 2 = 12).
    assert_eq!(compute_scroll_offset(11, 20, 10, 10, SCROLL_MARGIN), 9);
    // Tiny viewports never thrash or underflow.
    assert_eq!(compute_scroll_offset(0, 20, 1, 0, SCROLL_MARGIN), 0);
    assert_eq!(compute_scroll_offset(5, 20, 2, 0, SCROLL_MARGIN), 4);
    assert_eq!(compute_scroll_offset(0, 5, 10, 0, SCROLL_MARGIN), 0);

    #[cfg(feature = "ratatui-ui")]
    {
        use debopty_tui::{UiPalette, render_frame_with_palette};
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let palette = UiPalette::truecolor();
        for &(w, h) in &[(40u16, 16u16), (60, 14), (80, 24), (120, 36), (160, 44)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            let mut state = AppState::new(sample_status());
            for step in 0..13 {
                state.set_selected_idx(step);
                terminal
                    .draw(|frame| render_frame_with_palette(frame, &state, &palette))
                    .unwrap_or_else(|e| panic!("menu scroll panic at {w}x{h} step={step}: {e}"));
            }
            // Also verify overflowing Command Palette renders cleanly across all sizes.
            state.open_palette_preview("");
            terminal
                .draw(|frame| render_frame_with_palette(frame, &state, &palette))
                .unwrap_or_else(|e| panic!("palette scroll panic at {w}x{h}: {e}"));
        }
    }
}

#[cfg(feature = "ratatui-ui")]
#[test]
fn sync_frame_writer_batches_entire_ratatui_frame_into_single_flush_with_mode_2026() {
    use std::io::Write;

    use debopty_tui::{
        BEGIN_SYNC_UPDATE_SEQ, DISABLE_MOUSE_ALL_SEQ, ENABLE_MOUSE_CLICK_SGR_SEQ,
        END_SYNC_UPDATE_SEQ, SyncFrameWriter, UiPalette, render_frame_with_palette,
    };
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;
    use ratatui::{Terminal, Viewport};

    struct CountingWriter {
        bytes: Vec<u8>,
        flushes: usize,
    }

    impl CountingWriter {
        fn new() -> Self {
            Self {
                bytes: Vec::new(),
                flushes: 0,
            }
        }
    }

    impl Write for CountingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    // Verify mouse capture sequences never enable ?1003h hover storm.
    assert!(
        ENABLE_MOUSE_CLICK_SGR_SEQ
            .windows(8)
            .any(|w| w == b"\x1b[?1000h")
    );
    assert!(
        ENABLE_MOUSE_CLICK_SGR_SEQ
            .windows(8)
            .any(|w| w == b"\x1b[?1006h")
    );
    assert!(
        !ENABLE_MOUSE_CLICK_SGR_SEQ
            .windows(8)
            .any(|w| w == b"\x1b[?1003h"),
        "mouse enable must never turn on ?1003h hover reporting"
    );
    assert!(
        DISABLE_MOUSE_ALL_SEQ
            .windows(8)
            .any(|w| w == b"\x1b[?1003l")
    );

    let state = AppState::new(sample_status());
    let palette = UiPalette::truecolor();

    // 1. Sync enabled: exactly 1 flush wrapped in Mode 2026 begin..end.
    let writer = SyncFrameWriter::new(CountingWriter::new(), true);
    let handle = writer.clone();
    let backend = CrosstermBackend::new(writer);
    let mut terminal = Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, 100, 30)),
        },
    )
    .unwrap();
    handle.begin_frame().unwrap();
    terminal
        .draw(|frame| render_frame_with_palette(frame, &state, &palette))
        .unwrap();
    handle.end_frame().unwrap();

    handle.with_inner(|cw| {
        assert_eq!(cw.flushes, 1, "entire frame must commit in exactly 1 flush");
        assert!(cw.bytes.starts_with(BEGIN_SYNC_UPDATE_SEQ));
        assert!(cw.bytes.ends_with(END_SYNC_UPDATE_SEQ));
    });

    // 2. Sync disabled (e.g. TERM=dumb or DEBOPTY_NO_SYNC_OUTPUT=1): 0 sync markers, still 1 flush.
    let writer_off = SyncFrameWriter::new(CountingWriter::new(), false);
    let handle_off = writer_off.clone();
    let backend_off = CrosstermBackend::new(writer_off);
    let mut terminal_off = Terminal::with_options(
        backend_off,
        ratatui::TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, 100, 30)),
        },
    )
    .unwrap();
    handle_off.begin_frame().unwrap();
    terminal_off
        .draw(|frame| render_frame_with_palette(frame, &state, &palette))
        .unwrap();
    handle_off.end_frame().unwrap();

    handle_off.with_inner(|cw_off| {
        assert_eq!(cw_off.flushes, 1);
        assert!(
            !cw_off
                .bytes
                .windows(BEGIN_SYNC_UPDATE_SEQ.len())
                .any(|w| w == BEGIN_SYNC_UPDATE_SEQ)
        );
        assert!(
            !cw_off
                .bytes
                .windows(END_SYNC_UPDATE_SEQ.len())
                .any(|w| w == END_SYNC_UPDATE_SEQ)
        );
    });
}

#[cfg(feature = "ratatui-ui")]
#[test]
fn coordinated_dual_pane_transition_and_modal_dim_ramp_cjk_safe_across_all_sizes() {
    use debopty_tui::{MotionPolicy, TransitionKind, UiPalette, render_frame_with_palette};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let palette = UiPalette::truecolor();
    let policy = MotionPolicy::detect_from(false, Some("xterm-256color"), None, None, None);
    let sizes = [(40u16, 16u16), (80, 24), (120, 36), (160, 44)];
    let progress_steps = [0.0f64, 0.25, 0.5, 0.75, 1.0];
    let kinds = [
        TransitionKind::TabSwitch { dir: 1 },
        TransitionKind::TabSwitch { dir: -1 },
        TransitionKind::MenuPush,
        TransitionKind::MenuPop,
    ];

    for &(w, h) in &sizes {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        for &kind in &kinds {
            for &t in &progress_steps {
                let mut state = AppState::new(sample_status());
                state.set_motion_policy(policy);
                state.set_transition_preview(kind, t);
                terminal
                    .draw(|frame| render_frame_with_palette(frame, &state, &palette))
                    .unwrap_or_else(|e| {
                        panic!("transition render failed at {w}x{h} {kind:?} t={t}: {e}")
                    });
            }
        }

        // ModalOpen backdrop dim ramp from 0.0 -> 1.0 across all sizes.
        for &t in &progress_steps {
            let mut modal_state = AppState::new(sample_status());
            modal_state.set_motion_policy(policy);
            modal_state.open_confirm_preview(
                "确认执行高危操作测试？",
                debopty_bridge::BashAction::ScriptUninstall,
            );
            modal_state.set_transition_preview(TransitionKind::ModalOpen, t);
            terminal
                .draw(|frame| render_frame_with_palette(frame, &modal_state, &palette))
                .unwrap_or_else(|e| panic!("modal dim ramp failed at {w}x{h} t={t}: {e}"));
        }
    }

    // Verify dual-pane transition actually animates the right pane as well on split 120x36.
    let mut t_early = Terminal::new(TestBackend::new(120, 36)).unwrap();
    let mut s_early = AppState::new(sample_status());
    s_early.set_motion_policy(policy);
    s_early.set_transition_preview(TransitionKind::TabSwitch { dir: 1 }, 0.15);
    t_early
        .draw(|frame| render_frame_with_palette(frame, &s_early, &palette))
        .unwrap();

    let mut t_done = Terminal::new(TestBackend::new(120, 36)).unwrap();
    let mut s_done = AppState::new(sample_status());
    s_done.set_motion_policy(policy);
    s_done.set_transition_preview(TransitionKind::TabSwitch { dir: 1 }, 1.0);
    t_done
        .draw(|frame| render_frame_with_palette(frame, &s_done, &palette))
        .unwrap();

    let mut right_pane_diffs = 0usize;
    let buf_early = t_early.backend().buffer();
    let buf_done = t_done.backend().buffer();
    for y in 4..32 {
        for x in 50..115 {
            if buf_early[(x, y)] != buf_done[(x, y)] {
                right_pane_diffs += 1;
            }
        }
    }
    assert!(
        right_pane_diffs > 0,
        "expected coordinated right-pane animation during TabSwitch"
    );
}
