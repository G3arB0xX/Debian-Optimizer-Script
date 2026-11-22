use std::io;

use crate::menu::WorkspaceTab;
use crate::state::{ActivePane, AppState};

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
    /// Switch to the next top Workspace Pill Tab (`Right` / `]`).
    NextTab,
    /// Switch to the previous top Workspace Pill Tab (`Left` / `[`).
    PrevTab,
    /// Toggle the global fuzzy Command Palette (`Ctrl+K` / `Ctrl+P`).
    PaletteOpen,
    /// Active animation/toast clock tick (scheduled only while a transition or toast is active).
    Tick,
    /// Periodic live telemetry sampling tick.
    TelemetryTick,
    /// Mouse left-click on a top Workspace Pill Tab.
    ClickTab(WorkspaceTab),
    /// Mouse left-click on a visible menu row index.
    ClickMenuItem(usize),
    /// Mouse left-click on a visible Command Palette row index.
    ClickPaletteItem(usize),
    /// Mouse left-click focusing a split-pane surface.
    FocusPane(ActivePane),
    /// Mouse wheel scroll delta applied to the Telemetry pane.
    ScrollTelemetry(i16),
    /// Scroll live task execution log by signed line delta (U39).
    ScrollTaskLog(i16),
    /// Dismiss completed live task execution panel (`x` / `Esc`) (U39).
    DismissTaskLog,
    /// Toggle a checkbox/boolean option inside an in-TUI modal (`Space`) (U41).
    FormToggle,
    /// Mouse left-click on a selectable row inside an in-TUI modal (U41).
    ClickModalRow(usize),
    /// Toggle the Help & Shortcuts overlay (`?` / `F1`).
    HelpToggle,
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
