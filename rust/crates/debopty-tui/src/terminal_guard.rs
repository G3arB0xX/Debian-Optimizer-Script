use std::io;
use std::sync::Once;

use crossterm::cursor::{Hide, Show};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

static PANIC_HOOK_ONCE: Once = Once::new();

/// Low-level terminal lifecycle operations abstracted for deterministic unit testing.
pub trait TerminalOps {
    fn enable_raw_mode(&mut self) -> io::Result<()>;
    fn disable_raw_mode(&mut self) -> io::Result<()>;
    fn enter_alt_screen(&mut self) -> io::Result<()>;
    fn leave_alt_screen(&mut self) -> io::Result<()>;
}

/// Real terminal operations backed by `crossterm` on `io::stdout()`.
#[derive(Debug, Default, Clone, Copy)]
pub struct CrosstermOps;

impl TerminalOps for CrosstermOps {
    fn enable_raw_mode(&mut self) -> io::Result<()> {
        enable_raw_mode()
    }

    fn disable_raw_mode(&mut self) -> io::Result<()> {
        disable_raw_mode()
    }

    fn enter_alt_screen(&mut self) -> io::Result<()> {
        execute!(io::stdout(), EnterAlternateScreen, Hide)
    }

    fn leave_alt_screen(&mut self) -> io::Result<()> {
        execute!(io::stdout(), Show, LeaveAlternateScreen)
    }
}

/// Best-effort terminal restoration used by the panic hook (never panics).
fn restore_terminal_best_effort() {
    let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
    let _ = disable_raw_mode();
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

/// RAII guard managing raw mode and alternate-screen lifecycle with safe child-process
/// suspend/resume and automatic rollback on drop or partial initialization failure.
pub struct TerminalGuard<O: TerminalOps = CrosstermOps> {
    ops: O,
    active: bool,
}

impl TerminalGuard<CrosstermOps> {
    /// Enters raw mode and alternate screen on the real terminal.
    pub fn enter() -> io::Result<Self> {
        Self::enter_with(CrosstermOps)
    }
}

impl<O: TerminalOps> TerminalGuard<O> {
    /// Enters raw mode and alternate screen using injectable `TerminalOps`.
    ///
    /// Installs the panic hook once before mutating terminal state. If entering the
    /// alternate screen fails, raw mode is immediately rolled back before returning `Err`.
    pub fn enter_with(mut ops: O) -> io::Result<Self> {
        install_panic_hook();
        ops.enable_raw_mode()?;
        if let Err(err) = ops.enter_alt_screen() {
            let _ = ops.disable_raw_mode();
            return Err(err);
        }
        Ok(Self { ops, active: true })
    }

    /// Returns `true` while raw mode and alternate screen are currently active.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Returns a shared reference to the underlying `TerminalOps`.
    pub fn ops(&self) -> &O {
        &self.ops
    }

    /// Returns a mutable reference to the underlying `TerminalOps`.
    pub fn ops_mut(&mut self) -> &mut O {
        &mut self.ops
    }

    /// Idempotently leaves the alternate screen and disables raw mode (e.g. before
    /// spawning an interactive child process).
    pub fn suspend(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        let alt_res = self.ops.leave_alt_screen();
        let raw_res = self.ops.disable_raw_mode();
        alt_res.and(raw_res)
    }

    /// Idempotently re-enables raw mode and enters the alternate screen after a child
    /// process finishes. Rolls back raw mode if `enter_alt_screen` fails.
    pub fn resume(&mut self) -> io::Result<()> {
        if self.active {
            return Ok(());
        }
        self.ops.enable_raw_mode()?;
        if let Err(err) = self.ops.enter_alt_screen() {
            let _ = self.ops.disable_raw_mode();
            return Err(err);
        }
        self.active = true;
        Ok(())
    }
}

impl<O: TerminalOps> Drop for TerminalGuard<O> {
    fn drop(&mut self) {
        if self.active {
            let _ = self.ops.leave_alt_screen();
            let _ = self.ops.disable_raw_mode();
            self.active = false;
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
    }

    struct MockTerminalOps {
        log: Rc<RefCell<Vec<OpCall>>>,
        fail_enter_alt_on_call: Option<usize>,
        enter_alt_calls: usize,
    }

    impl MockTerminalOps {
        fn new(log: Rc<RefCell<Vec<OpCall>>>) -> Self {
            Self {
                log,
                fail_enter_alt_on_call: None,
                enter_alt_calls: 0,
            }
        }

        fn failing_enter_alt(log: Rc<RefCell<Vec<OpCall>>>, fail_on_call: usize) -> Self {
            Self {
                log,
                fail_enter_alt_on_call: Some(fail_on_call),
                enter_alt_calls: 0,
            }
        }
    }

    impl TerminalOps for MockTerminalOps {
        fn enable_raw_mode(&mut self) -> io::Result<()> {
            self.log.borrow_mut().push(OpCall::EnableRaw);
            Ok(())
        }

        fn disable_raw_mode(&mut self) -> io::Result<()> {
            self.log.borrow_mut().push(OpCall::DisableRaw);
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
            self.log.borrow_mut().push(OpCall::LeaveAlt);
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

            guard.suspend().expect("first suspend ok");
            assert!(!guard.is_active());
            // Second suspend is a no-op.
            guard.suspend().expect("second suspend ok");

            guard.resume().expect("first resume ok");
            assert!(guard.is_active());
            // Second resume is a no-op.
            guard.resume().expect("second resume ok");
        }

        assert_eq!(
            *log.borrow(),
            vec![
                OpCall::EnableRaw,
                OpCall::EnterAlt,
                OpCall::LeaveAlt,
                OpCall::DisableRaw,
                OpCall::EnableRaw,
                OpCall::EnterAlt,
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
                OpCall::LeaveAlt,
                OpCall::DisableRaw,
                OpCall::EnableRaw,
                OpCall::EnterAlt,
                OpCall::DisableRaw,
            ]
        );
    }
}
