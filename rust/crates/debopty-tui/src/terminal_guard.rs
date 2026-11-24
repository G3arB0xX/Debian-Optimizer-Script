use std::io::{self, Write};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::cursor::{Hide, Show};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

/// Click + wheel + SGR extended mouse reporting (`?1000h` + `?1006h`) without `?1003h` hover storms.
pub const ENABLE_MOUSE_CLICK_SGR_SEQ: &[u8] = b"\x1b[?1000h\x1b[?1006h";
/// Disables SGR, normal click, button-drag, and any inherited all-motion hover mouse reporting.
pub const DISABLE_MOUSE_ALL_SEQ: &[u8] = b"\x1b[?1006l\x1b[?1000l\x1b[?1002l\x1b[?1003l";
/// DEC Mode 2026 Begin Synchronized Update escape sequence.
pub const BEGIN_SYNC_UPDATE_SEQ: &[u8] = b"\x1b[?2026h";
/// DEC Mode 2026 End Synchronized Update escape sequence.
pub const END_SYNC_UPDATE_SEQ: &[u8] = b"\x1b[?2026l";

static PANIC_HOOK_ONCE: Once = Once::new();
static PANIC_TERMINAL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Returns `true` when `CrosstermOps` has armed the panic-hook terminal restoration gate.
pub fn panic_restore_is_armed() -> bool {
    PANIC_TERMINAL_ACTIVE.load(Ordering::SeqCst)
}

/// Low-level terminal lifecycle operations abstracted for deterministic unit testing.
pub trait TerminalOps {
    fn enable_raw_mode(&mut self) -> io::Result<()>;
    fn disable_raw_mode(&mut self) -> io::Result<()>;
    fn enter_alt_screen(&mut self) -> io::Result<()>;
    fn leave_alt_screen(&mut self) -> io::Result<()>;

    fn enable_mouse_capture(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn disable_mouse_capture(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Real terminal operations backed by `crossterm` on `io::stdout()`.
#[derive(Debug, Default, Clone, Copy)]
pub struct CrosstermOps;

impl TerminalOps for CrosstermOps {
    fn enable_raw_mode(&mut self) -> io::Result<()> {
        enable_raw_mode()?;
        PANIC_TERMINAL_ACTIVE.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn disable_raw_mode(&mut self) -> io::Result<()> {
        let res = disable_raw_mode();
        if res.is_ok() {
            PANIC_TERMINAL_ACTIVE.store(false, Ordering::SeqCst);
        }
        res
    }

    fn enter_alt_screen(&mut self) -> io::Result<()> {
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        PANIC_TERMINAL_ACTIVE.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn leave_alt_screen(&mut self) -> io::Result<()> {
        let mut stdout = io::stdout();
        let _ = stdout.write_all(END_SYNC_UPDATE_SEQ);
        execute!(stdout, Show, LeaveAlternateScreen)
    }

    fn enable_mouse_capture(&mut self) -> io::Result<()> {
        let mut stdout = io::stdout();
        stdout.write_all(ENABLE_MOUSE_CLICK_SGR_SEQ)?;
        stdout.flush()?;
        PANIC_TERMINAL_ACTIVE.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn disable_mouse_capture(&mut self) -> io::Result<()> {
        let mut stdout = io::stdout();
        stdout.write_all(DISABLE_MOUSE_ALL_SEQ)?;
        stdout.flush()
    }
}

/// Best-effort terminal restoration used by the panic hook (never panics).
/// Gated by `PANIC_TERMINAL_ACTIVE` so panics while suspended do not emit spurious escapes.
fn restore_terminal_best_effort() {
    if PANIC_TERMINAL_ACTIVE.swap(false, Ordering::SeqCst) {
        let mut stdout = io::stdout();
        let _ = stdout.write_all(END_SYNC_UPDATE_SEQ);
        let _ = stdout.write_all(DISABLE_MOUSE_ALL_SEQ);
        let _ = execute!(stdout, Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

/// Idempotently installs a panic hook that restores terminal state before chaining
/// to the previously registered panic hook.
pub fn install_panic_hook() {
    PANIC_HOOK_ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            restore_terminal_best_effort();
            previous(panic_info);
        }));
    });
}

/// RAII guard managing raw mode, alternate-screen, and mouse-capture lifecycle with safe
/// child-process suspend/resume and automatic rollback on drop or partial initialization failure.
pub struct TerminalGuard<O: TerminalOps = CrosstermOps> {
    ops: O,
    raw_active: bool,
    alt_active: bool,
    mouse_active: bool,
}

impl TerminalGuard<CrosstermOps> {
    /// Enters raw mode, alternate screen, and mouse capture on the real terminal.
    pub fn enter() -> io::Result<Self> {
        Self::enter_with(CrosstermOps)
    }
}

impl<O: TerminalOps> TerminalGuard<O> {
    /// Enters raw mode, alternate screen, and mouse capture using injectable `TerminalOps`.
    ///
    /// Installs the panic hook once before mutating terminal state. Any partial failure rolls
    /// back previously enabled modes cleanly before returning `Err`.
    pub fn enter_with(mut ops: O) -> io::Result<Self> {
        install_panic_hook();
        ops.enable_raw_mode()?;
        if let Err(err) = ops.enter_alt_screen() {
            let _ = ops.disable_raw_mode();
            return Err(err);
        }
        if let Err(err) = ops.enable_mouse_capture() {
            let _ = ops.leave_alt_screen();
            let _ = ops.disable_raw_mode();
            return Err(err);
        }
        Ok(Self {
            ops,
            raw_active: true,
            alt_active: true,
            mouse_active: true,
        })
    }

    /// Returns `true` while raw mode, alternate screen, or mouse capture is currently active.
    pub fn is_active(&self) -> bool {
        self.raw_active || self.alt_active || self.mouse_active
    }

    /// Returns `true` while raw mode is currently active.
    pub fn is_raw_active(&self) -> bool {
        self.raw_active
    }

    /// Returns `true` while alternate screen is currently active.
    pub fn is_alt_active(&self) -> bool {
        self.alt_active
    }

    /// Returns `true` while mouse capture is currently active.
    pub fn is_mouse_active(&self) -> bool {
        self.mouse_active
    }

    /// Returns a shared reference to the underlying `TerminalOps`.
    pub fn ops(&self) -> &O {
        &self.ops
    }

    /// Returns a mutable reference to the underlying `TerminalOps`.
    pub fn ops_mut(&mut self) -> &mut O {
        &mut self.ops
    }

    /// Idempotently disables mouse capture, leaves the alternate screen, and disables raw mode
    /// (e.g. before spawning an interactive child process). Clears each active flag only when its
    /// teardown succeeds so `Drop` can retry any failed restoration step independently.
    pub fn suspend(&mut self) -> io::Result<()> {
        if !self.raw_active && !self.alt_active && !self.mouse_active {
            return Ok(());
        }
        let mouse_res = if self.mouse_active {
            let res = self.ops.disable_mouse_capture();
            if res.is_ok() {
                self.mouse_active = false;
            }
            res
        } else {
            Ok(())
        };
        let alt_res = if self.alt_active {
            let res = self.ops.leave_alt_screen();
            if res.is_ok() {
                self.alt_active = false;
            }
            res
        } else {
            Ok(())
        };
        let raw_res = if self.raw_active {
            let res = self.ops.disable_raw_mode();
            if res.is_ok() {
                self.raw_active = false;
            }
            res
        } else {
            Ok(())
        };
        mouse_res.and(alt_res).and(raw_res)
    }

    /// Idempotently re-enables raw mode, alternate screen, and mouse capture after a child
    /// process finishes. Rolls back cleanly on partial failure.
    pub fn resume(&mut self) -> io::Result<()> {
        if self.raw_active && self.alt_active && self.mouse_active {
            return Ok(());
        }
        if !self.raw_active {
            self.ops.enable_raw_mode()?;
            self.raw_active = true;
        }
        if !self.alt_active {
            if let Err(err) = self.ops.enter_alt_screen() {
                if self.raw_active && self.ops.disable_raw_mode().is_ok() {
                    self.raw_active = false;
                }
                return Err(err);
            }
            self.alt_active = true;
        }
        if !self.mouse_active {
            if let Err(err) = self.ops.enable_mouse_capture() {
                if self.alt_active && self.ops.leave_alt_screen().is_ok() {
                    self.alt_active = false;
                }
                if self.raw_active && self.ops.disable_raw_mode().is_ok() {
                    self.raw_active = false;
                }
                return Err(err);
            }
            self.mouse_active = true;
        }
        Ok(())
    }
}

impl<O: TerminalOps> Drop for TerminalGuard<O> {
    fn drop(&mut self) {
        if self.mouse_active && self.ops.disable_mouse_capture().is_ok() {
            self.mouse_active = false;
        }
        if self.alt_active && self.ops.leave_alt_screen().is_ok() {
            self.alt_active = false;
        }
        if self.raw_active && self.ops.disable_raw_mode().is_ok() {
            self.raw_active = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum OpCall {
        EnableRaw,
        DisableRaw,
        EnterAlt,
        LeaveAlt,
        EnableMouse,
        DisableMouse,
    }

    struct MockTerminalOps {
        log: Rc<RefCell<Vec<OpCall>>>,
        fail_enter_alt_on_call: Option<usize>,
        fail_leave_alt_on_call: Option<usize>,
        fail_disable_raw_on_call: Option<usize>,
        fail_enable_mouse_on_call: Option<usize>,
        fail_disable_mouse_on_call: Option<usize>,
        enter_alt_calls: usize,
        leave_alt_calls: usize,
        disable_raw_calls: usize,
        enable_mouse_calls: usize,
        disable_mouse_calls: usize,
    }

    impl MockTerminalOps {
        fn new(log: Rc<RefCell<Vec<OpCall>>>) -> Self {
            Self {
                log,
                fail_enter_alt_on_call: None,
                fail_leave_alt_on_call: None,
                fail_disable_raw_on_call: None,
                fail_enable_mouse_on_call: None,
                fail_disable_mouse_on_call: None,
                enter_alt_calls: 0,
                leave_alt_calls: 0,
                disable_raw_calls: 0,
                enable_mouse_calls: 0,
                disable_mouse_calls: 0,
            }
        }

        fn failing_enter_alt(log: Rc<RefCell<Vec<OpCall>>>, fail_on_call: usize) -> Self {
            let mut ops = Self::new(log);
            ops.fail_enter_alt_on_call = Some(fail_on_call);
            ops
        }

        fn failing_leave_alt_once(log: Rc<RefCell<Vec<OpCall>>>) -> Self {
            let mut ops = Self::new(log);
            ops.fail_leave_alt_on_call = Some(1);
            ops
        }

        fn failing_disable_raw_once(log: Rc<RefCell<Vec<OpCall>>>) -> Self {
            let mut ops = Self::new(log);
            ops.fail_disable_raw_on_call = Some(1);
            ops
        }

        fn failing_enable_mouse(log: Rc<RefCell<Vec<OpCall>>>, fail_on_call: usize) -> Self {
            let mut ops = Self::new(log);
            ops.fail_enable_mouse_on_call = Some(fail_on_call);
            ops
        }

        fn failing_disable_mouse_once(log: Rc<RefCell<Vec<OpCall>>>) -> Self {
            let mut ops = Self::new(log);
            ops.fail_disable_mouse_on_call = Some(1);
            ops
        }
    }

    impl TerminalOps for MockTerminalOps {
        fn enable_raw_mode(&mut self) -> io::Result<()> {
            self.log.borrow_mut().push(OpCall::EnableRaw);
            Ok(())
        }

        fn disable_raw_mode(&mut self) -> io::Result<()> {
            self.disable_raw_calls += 1;
            self.log.borrow_mut().push(OpCall::DisableRaw);
            if self.fail_disable_raw_on_call == Some(self.disable_raw_calls) {
                return Err(io::Error::other("simulated disable_raw_mode failure"));
            }
            Ok(())
        }

        fn enter_alt_screen(&mut self) -> io::Result<()> {
            self.enter_alt_calls += 1;
            self.log.borrow_mut().push(OpCall::EnterAlt);
            if self.fail_enter_alt_on_call == Some(self.enter_alt_calls) {
                return Err(io::Error::other("simulated enter_alt_screen failure"));
            }
            Ok(())
        }

        fn leave_alt_screen(&mut self) -> io::Result<()> {
            self.leave_alt_calls += 1;
            self.log.borrow_mut().push(OpCall::LeaveAlt);
            if self.fail_leave_alt_on_call == Some(self.leave_alt_calls) {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "simulated broken pipe on leave_alt_screen",
                ));
            }
            Ok(())
        }

        fn enable_mouse_capture(&mut self) -> io::Result<()> {
            self.enable_mouse_calls += 1;
            self.log.borrow_mut().push(OpCall::EnableMouse);
            if self.fail_enable_mouse_on_call == Some(self.enable_mouse_calls) {
                return Err(io::Error::other("simulated enable_mouse_capture failure"));
            }
            Ok(())
        }

        fn disable_mouse_capture(&mut self) -> io::Result<()> {
            self.disable_mouse_calls += 1;
            self.log.borrow_mut().push(OpCall::DisableMouse);
            if self.fail_disable_mouse_on_call == Some(self.disable_mouse_calls) {
                return Err(io::Error::other("simulated disable_mouse_capture failure"));
            }
            Ok(())
        }
    }

    #[test]
    fn guard_enter_suspend_resume_drop_is_balanced_and_idempotent() {
        let log = Rc::new(RefCell::new(Vec::new()));
        {
            let mut guard =
                TerminalGuard::enter_with(MockTerminalOps::new(Rc::clone(&log))).expect("enter ok");
            assert!(guard.is_active());
            assert!(guard.is_mouse_active());

            guard.suspend().expect("first suspend ok");
            assert!(!guard.is_active());
            assert!(!guard.is_mouse_active());
            // Second suspend is a no-op.
            guard.suspend().expect("second suspend ok");

            guard.resume().expect("first resume ok");
            assert!(guard.is_active());
            assert!(guard.is_mouse_active());
            // Second resume is a no-op.
            guard.resume().expect("second resume ok");
        }

        assert_eq!(
            *log.borrow(),
            vec![
                OpCall::EnableRaw,
                OpCall::EnterAlt,
                OpCall::EnableMouse,
                OpCall::DisableMouse,
                OpCall::LeaveAlt,
                OpCall::DisableRaw,
                OpCall::EnableRaw,
                OpCall::EnterAlt,
                OpCall::EnableMouse,
                OpCall::DisableMouse,
                OpCall::LeaveAlt,
                OpCall::DisableRaw,
            ]
        );
    }

    #[test]
    fn guard_drop_after_suspend_does_not_double_restore() {
        let log = Rc::new(RefCell::new(Vec::new()));
        {
            let mut guard =
                TerminalGuard::enter_with(MockTerminalOps::new(Rc::clone(&log))).expect("enter ok");
            guard.suspend().expect("suspend ok");
            assert!(!guard.is_active());
        }

        assert_eq!(
            *log.borrow(),
            vec![
                OpCall::EnableRaw,
                OpCall::EnterAlt,
                OpCall::EnableMouse,
                OpCall::DisableMouse,
                OpCall::LeaveAlt,
                OpCall::DisableRaw,
            ]
        );
    }

    #[test]
    fn guard_rolls_back_raw_mode_when_enter_alt_screen_fails() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let err = TerminalGuard::enter_with(MockTerminalOps::failing_enter_alt(Rc::clone(&log), 1))
            .err()
            .expect("enter_alt_screen failure should propagate");
        assert!(err.to_string().contains("simulated enter_alt_screen"));
        assert_eq!(
            *log.borrow(),
            vec![OpCall::EnableRaw, OpCall::EnterAlt, OpCall::DisableRaw]
        );
    }

    #[test]
    fn guard_rolls_back_alt_and_raw_when_enable_mouse_capture_fails() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let err =
            TerminalGuard::enter_with(MockTerminalOps::failing_enable_mouse(Rc::clone(&log), 1))
                .err()
                .expect("enable_mouse_capture failure should propagate");
        assert!(err.to_string().contains("simulated enable_mouse_capture"));
        assert_eq!(
            *log.borrow(),
            vec![
                OpCall::EnableRaw,
                OpCall::EnterAlt,
                OpCall::EnableMouse,
                OpCall::LeaveAlt,
                OpCall::DisableRaw,
            ]
        );
    }

    #[test]
    fn guard_resume_rolls_back_raw_mode_on_enter_alt_failure() {
        let log = Rc::new(RefCell::new(Vec::new()));
        {
            let mut guard =
                TerminalGuard::enter_with(MockTerminalOps::failing_enter_alt(Rc::clone(&log), 2))
                    .expect("initial enter ok");
            guard.suspend().expect("suspend ok");
            assert!(guard.resume().is_err());
            assert!(!guard.is_active());
        }

        assert_eq!(
            *log.borrow(),
            vec![
                OpCall::EnableRaw,
                OpCall::EnterAlt,
                OpCall::EnableMouse,
                OpCall::DisableMouse,
                OpCall::LeaveAlt,
                OpCall::DisableRaw,
                OpCall::EnableRaw,
                OpCall::EnterAlt,
                OpCall::DisableRaw,
            ]
        );
    }

    #[test]
    fn guard_resume_rolls_back_alt_and_raw_on_enable_mouse_failure() {
        let log = Rc::new(RefCell::new(Vec::new()));
        {
            let mut guard = TerminalGuard::enter_with(MockTerminalOps::failing_enable_mouse(
                Rc::clone(&log),
                2,
            ))
            .expect("initial enter ok");
            guard.suspend().expect("suspend ok");
            assert!(guard.resume().is_err());
            assert!(!guard.is_active());
        }

        assert_eq!(
            *log.borrow(),
            vec![
                OpCall::EnableRaw,
                OpCall::EnterAlt,
                OpCall::EnableMouse,
                OpCall::DisableMouse,
                OpCall::LeaveAlt,
                OpCall::DisableRaw,
                OpCall::EnableRaw,
                OpCall::EnterAlt,
                OpCall::EnableMouse,
                OpCall::LeaveAlt,
                OpCall::DisableRaw,
            ]
        );
    }

    #[test]
    fn guard_suspend_disable_mouse_failure_retries_only_mouse_on_drop() {
        let log = Rc::new(RefCell::new(Vec::new()));
        {
            let mut guard = TerminalGuard::enter_with(MockTerminalOps::failing_disable_mouse_once(
                Rc::clone(&log),
            ))
            .expect("enter ok");
            assert!(guard.suspend().is_err());
            assert!(guard.is_mouse_active());
            assert!(!guard.is_alt_active());
            assert!(!guard.is_raw_active());
            assert!(guard.is_active());
        }

        assert_eq!(
            *log.borrow(),
            vec![
                OpCall::EnableRaw,
                OpCall::EnterAlt,
                OpCall::EnableMouse,
                OpCall::DisableMouse,
                OpCall::LeaveAlt,
                OpCall::DisableRaw,
                OpCall::DisableMouse,
            ]
        );
    }

    #[test]
    fn guard_suspend_disable_raw_failure_retries_only_raw_on_drop() {
        let log = Rc::new(RefCell::new(Vec::new()));
        {
            let mut guard = TerminalGuard::enter_with(MockTerminalOps::failing_disable_raw_once(
                Rc::clone(&log),
            ))
            .expect("enter ok");
            assert!(guard.suspend().is_err());
            assert!(!guard.is_mouse_active());
            assert!(!guard.is_alt_active());
            assert!(guard.is_raw_active());
            assert!(guard.is_active());
        }

        assert_eq!(
            *log.borrow(),
            vec![
                OpCall::EnableRaw,
                OpCall::EnterAlt,
                OpCall::EnableMouse,
                OpCall::DisableMouse,
                OpCall::LeaveAlt,
                OpCall::DisableRaw,
                OpCall::DisableRaw,
            ]
        );
    }

    #[test]
    fn guard_suspend_leave_alt_broken_pipe_retries_only_alt_on_drop() {
        let log = Rc::new(RefCell::new(Vec::new()));
        {
            let mut guard =
                TerminalGuard::enter_with(MockTerminalOps::failing_leave_alt_once(Rc::clone(&log)))
                    .expect("enter ok");
            let err = guard.suspend().expect_err("broken pipe propagates");
            assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
            assert!(!guard.is_mouse_active());
            assert!(guard.is_alt_active());
            assert!(!guard.is_raw_active());
            assert!(guard.is_active());
        }

        assert_eq!(
            *log.borrow(),
            vec![
                OpCall::EnableRaw,
                OpCall::EnterAlt,
                OpCall::EnableMouse,
                OpCall::DisableMouse,
                OpCall::LeaveAlt,
                OpCall::DisableRaw,
                OpCall::LeaveAlt,
            ]
        );
    }

    #[test]
    fn panic_restore_is_not_armed_by_mock_ops() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let guard =
            TerminalGuard::enter_with(MockTerminalOps::new(Rc::clone(&log))).expect("enter ok");
        assert!(!panic_restore_is_armed());
        drop(guard);
        assert!(!panic_restore_is_armed());
    }
}
