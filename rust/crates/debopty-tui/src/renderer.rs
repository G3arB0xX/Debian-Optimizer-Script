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

/// Upper bound on queued events drained in a single burst before redrawing.
pub const MAX_EVENT_BATCH: usize = 64;

/// Coalesces high-frequency queued terminal events (`Resize`, wheel scroll deltas, and duplicate
/// ticks) without ever dropping or reordering semantic navigation, filter, or action events.
pub fn coalesce_events(raw: Vec<MenuEvent>) -> Vec<MenuEvent> {
    let mut out: Vec<MenuEvent> = Vec::with_capacity(raw.len());
    for ev in raw {
        match ev {
            MenuEvent::Resize => {
                if out.last() != Some(&MenuEvent::Resize) {
                    out.push(MenuEvent::Resize);
                }
            }
            MenuEvent::Tick => {
                if out.last() != Some(&MenuEvent::Tick) {
                    out.push(MenuEvent::Tick);
                }
            }
            MenuEvent::TelemetryTick => {
                if out.last() != Some(&MenuEvent::TelemetryTick) {
                    out.push(MenuEvent::TelemetryTick);
                }
            }
            MenuEvent::ScrollTelemetry(delta) => {
                if let Some(MenuEvent::ScrollTelemetry(prev)) = out.last_mut() {
                    let sum = prev.saturating_add(delta);
                    if sum == 0 {
                        out.pop();
                    } else {
                        *prev = sum;
                    }
                } else if delta != 0 {
                    out.push(MenuEvent::ScrollTelemetry(delta));
                }
            }
            MenuEvent::ScrollTaskLog(delta) => {
                if let Some(MenuEvent::ScrollTaskLog(prev)) = out.last_mut() {
                    let sum = prev.saturating_add(delta);
                    if sum == 0 {
                        out.pop();
                    } else {
                        *prev = sum;
                    }
                } else if delta != 0 {
                    out.push(MenuEvent::ScrollTaskLog(delta));
                }
            }
            other => out.push(other),
        }
    }
    out
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

    /// Drains any immediately ready queued terminal events for burst coalescing.
    /// Default no-op keeps scripted/mock renderers 100% deterministic.
    fn drain_queued_events(&mut self, _out: &mut Vec<MenuEvent>) -> io::Result<()> {
        Ok(())
    }

    /// Returns `true` when running under CI or non-TTY stdin so the loop can exit safely.
    fn is_non_interactive(&self) -> bool {
        false
    }
}
