use std::io;

use crate::state::AppState;

/// Input events produced by any `TuiRenderer` implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuEvent {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Select,
    Back,
    Digit(u8),
    Quit,
    Refresh,
    Resize,
    ConfirmYes,
    ConfirmNo,
    /// Start inline fuzzy filter mode (`/`).
    FilterStart,
    /// Append character to active filter query.
    FilterChar(char),
    /// Delete last character from active filter query.
    FilterBackspace,
    /// Clear the filter input line while staying in filter mode (`Ctrl+U`).
    FilterClearLine,
    /// Delete the previous word from the filter query (`Ctrl+W`).
    FilterDeleteWord,
    /// Clear active filter query and exit filter mode.
    FilterClear,
    /// Switch active focus pane (Menu <-> Telemetry).
    TogglePane,
    /// Periodic animation frame tick (~30fps when animations are enabled).
    Tick,
}

/// Pluggable terminal UI renderer trait (implemented by `ConsoleRenderer` in Wave 1;
/// designed for future full-screen renderers without changing `AppState`).
pub trait TuiRenderer {
    fn draw(&mut self, state: &AppState) -> io::Result<()>;
    fn read_event(&mut self) -> io::Result<MenuEvent>;
    fn suspend_for_child(&mut self) -> io::Result<()>;
    fn resume_after_child(&mut self) -> io::Result<()>;

    /// Synchronizes renderer viewport state (e.g., split-pane availability after terminal resize)
    /// back into `AppState`. Default no-op keeps non-split/mock renderers unchanged.
    fn sync_viewport(&self, _state: &mut AppState) {}

    /// Returns `true` when running under CI or non-TTY stdin so the loop can exit safely.
    fn is_non_interactive(&self) -> bool {
        false
    }
}
