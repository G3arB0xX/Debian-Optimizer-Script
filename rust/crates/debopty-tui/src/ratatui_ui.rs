use std::collections::BTreeMap;
use std::env;
use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use debopty_bridge::{AppId, BashAction};
use debopty_core::status::{StatusSnapshot, TRACKED_TOOLS};
use ratatui::Terminal;
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState,
};

use crate::console_ui::is_non_interactive;
use crate::layout::{
    pad_right_to_width, tail_truncate_to_width, truncate_to_width, visual_width, wrap_text_to_width,
};
use crate::menu::{MenuAction, MenuEntry, MenuId, MenuItem, StatusBadge, WorkspaceTab};
use crate::renderer::{MenuEvent, TuiRenderer};
use crate::state::{ActivePane, AppState, NoticeKind, TRANSITION_TOTAL_TICKS};
use crate::terminal_guard::{CrosstermOps, TerminalGuard};

/// Minimum terminal width required to display the side-by-side status telemetry panel.
const SPLIT_PANE_MIN_WIDTH: u16 = 90;
/// 30fps tick cadence (~33ms) during active animations / split-pane heartbeat.
const TICK_INTERVAL: Duration = Duration::from_millis(33);
/// Reduced idle cadence (~120ms) when in single-pane mode with no active transition/toast.
const IDLE_TICK_INTERVAL: Duration = Duration::from_millis(120);
/// Braille spinner frames for live engine heartbeat (stepped at ~10fps = tick/3).
const BRAILLE_SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// Braille bar glyphs (bottom-to-top fill) for micro-sparklines.
const BRAILLE_LEVELS: &[char] = &['⣀', '⣄', '⣤', '⣦', '⣶', '⣷', '⣿'];
/// Canonical 4-stack grouping covering all 18 `TRACKED_TOOLS` for dense Telemetry bars & matrix.
const TOOL_STACKS: &[(&str, &[&str])] = &[
    ("运维", &["fish", "micro", "yazi", "lego", "tmux"]),
    ("运行", &["podman", "rustc", "go"]),
    ("服务", &["caddy", "ferron", "realm", "derper", "xray"]),
    (
        "网络",
        &["warp-cli", "usque", "easytier-core", "tailscale", "freship"],
    ),
];

/// Centralized WCAG-compliant layered color & surface palette for U16/U17/U18 Ratatui design.
///
/// Layered elevation hierarchy (TrueColor):
/// - Canvas `#0B0E14` -> Bento Card `#161A26` -> Floating Modal/Palette/Toast `#1E2436` -> Active Row `#242E47`
/// - Subtle hairline borders `#2A324B` vs glowing focus border `#38BDF8`
/// - Filled active pill tabs & tinted semantic capsule badges
/// - Graceful 256-color, light-terminal-safe ANSI-16, and strict `NO_COLOR`/`TERM=dumb` monochrome fallbacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiPalette {
    pub monochrome: bool,
    pub bg_base: Color,
    pub bg_surface: Color,
    pub bg_highlight: Color,
    pub bg_toast: Color,
    pub pill_active_bg: Color,
    pub pill_active_fg: Color,
    pub pill_inactive_bg: Color,
    pub capsule_ok_bg: Color,
    pub capsule_off_bg: Color,
    pub capsule_warn_bg: Color,
    pub accent_primary: Color,
    pub accent_secondary: Color,
    pub border_active: Color,
    pub border_inactive: Color,
    pub title_badge_fg: Color,
    pub text_primary: Color,
    pub text_muted: Color,
    pub text_inactive: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
    pub modal_border: Color,
    pub dim_backdrop_fg: Color,
    pub shimmer_fg: Color,
}

impl UiPalette {
    /// Deep-obsidian U18 layered TrueColor palette (WCAG AA/AAA verified on `#161A26` & `#1E2436`).
    pub const fn truecolor() -> Self {
        Self {
            monochrome: false,
            bg_base: Color::Rgb(11, 14, 20),    // #0B0E14 Deep Canvas
            bg_surface: Color::Rgb(22, 26, 38), // #161A26 Elevated Bento Card
            bg_highlight: Color::Rgb(36, 46, 71), // #242E47 Active Selection Row
            bg_toast: Color::Rgb(30, 36, 54),   // #1E2436 Floating Modal / Palette / Toast
            pill_active_bg: Color::Rgb(56, 189, 248), // #38BDF8 Filled Active Pill Tab
            pill_active_fg: Color::Rgb(11, 14, 20), // #0B0E14 High-contrast dark ink on cyan pill
            pill_inactive_bg: Color::Rgb(30, 36, 54), // #1E2436 Subtle Inactive Pill Tab
            capsule_ok_bg: Color::Rgb(19, 46, 36), // #132E24 Tinted Emerald Capsule
            capsule_off_bg: Color::Rgb(29, 34, 50), // #1D2232 Tinted Slate Capsule
            capsule_warn_bg: Color::Rgb(46, 36, 20), // #2E2414 Tinted Amber Capsule
            accent_primary: Color::Rgb(56, 189, 248), // #38BDF8 Electric Cyan
            accent_secondary: Color::Rgb(129, 140, 248), // #818CF8 Indigo Accent
            border_active: Color::Rgb(56, 189, 248), // #38BDF8 Glowing Focus Border
            border_inactive: Color::Rgb(42, 50, 75), // #2A324B Hairline Inactive Border
            title_badge_fg: Color::Rgb(125, 211, 252), // #7DD3FC Sky Title
            text_primary: Color::Rgb(241, 245, 249), // #F1F5F9 Primary Ink
            text_muted: Color::Rgb(148, 163, 184), // #94A3B8 Secondary Ink
            text_inactive: Color::Rgb(124, 139, 161), // #7C8BA1 Inactive Ink (AA)
            success: Color::Rgb(74, 222, 128),  // #4ADE80 Emerald
            warning: Color::Rgb(251, 191, 36),  // #FBBF24 Amber
            danger: Color::Rgb(248, 113, 113),  // #F87171 Rose
            modal_border: Color::Rgb(248, 113, 113), // #F87171 Destructive Rose
            dim_backdrop_fg: Color::Rgb(62, 73, 98), // #3E4962 Dimmed Backdrop
            shimmer_fg: Color::Rgb(224, 242, 254), // #E0F2FE Shimmer Edge
        }
    }

    /// Graceful 256-color indexed palette for standard SSH `TERM=xterm-256color`.
    pub const fn ansi256() -> Self {
        Self {
            monochrome: false,
            bg_base: Color::Indexed(233),
            bg_surface: Color::Indexed(235),
            bg_highlight: Color::Indexed(238),
            bg_toast: Color::Indexed(236),
            pill_active_bg: Color::Indexed(81),
            pill_active_fg: Color::Indexed(233),
            pill_inactive_bg: Color::Indexed(236),
            capsule_ok_bg: Color::Indexed(22),
            capsule_off_bg: Color::Indexed(236),
            capsule_warn_bg: Color::Indexed(58),
            accent_primary: Color::Indexed(81),
            accent_secondary: Color::Indexed(105),
            border_active: Color::Indexed(81),
            border_inactive: Color::Indexed(239),
            title_badge_fg: Color::Indexed(117),
            text_primary: Color::Indexed(255),
            text_muted: Color::Indexed(248),
            text_inactive: Color::Indexed(245),
            success: Color::Indexed(84),
            warning: Color::Indexed(221),
            danger: Color::Indexed(203),
            modal_border: Color::Indexed(203),
            dim_backdrop_fg: Color::Indexed(240),
            shimmer_fg: Color::Indexed(195),
        }
    }

    /// Light-terminal-safe ANSI 16-color fallback (`text_primary: Color::Reset`, no white-on-white).
    pub const fn ansi16() -> Self {
        Self {
            monochrome: false,
            bg_base: Color::Reset,
            bg_surface: Color::Reset,
            bg_highlight: Color::Reset,
            bg_toast: Color::Reset,
            pill_active_bg: Color::Cyan,
            pill_active_fg: Color::Black,
            pill_inactive_bg: Color::Reset,
            capsule_ok_bg: Color::Reset,
            capsule_off_bg: Color::Reset,
            capsule_warn_bg: Color::Reset,
            accent_primary: Color::Cyan,
            accent_secondary: Color::Blue,
            border_active: Color::Cyan,
            border_inactive: Color::DarkGray,
            title_badge_fg: Color::Cyan,
            text_primary: Color::Reset,
            text_muted: Color::DarkGray,
            text_inactive: Color::DarkGray,
            success: Color::Green,
            warning: Color::Yellow,
            danger: Color::Red,
            modal_border: Color::Red,
            dim_backdrop_fg: Color::DarkGray,
            shimmer_fg: Color::Cyan,
        }
    }

    /// Strict monochrome palette for `NO_COLOR` or `TERM=dumb`.
    pub const fn monochrome() -> Self {
        Self {
            monochrome: true,
            bg_base: Color::Reset,
            bg_surface: Color::Reset,
            bg_highlight: Color::Reset,
            bg_toast: Color::Reset,
            pill_active_bg: Color::Reset,
            pill_active_fg: Color::Reset,
            pill_inactive_bg: Color::Reset,
            capsule_ok_bg: Color::Reset,
            capsule_off_bg: Color::Reset,
            capsule_warn_bg: Color::Reset,
            accent_primary: Color::Reset,
            accent_secondary: Color::Reset,
            border_active: Color::Reset,
            border_inactive: Color::Reset,
            title_badge_fg: Color::Reset,
            text_primary: Color::Reset,
            text_muted: Color::Reset,
            text_inactive: Color::Reset,
            success: Color::Reset,
            warning: Color::Reset,
            danger: Color::Reset,
            modal_border: Color::Reset,
            dim_backdrop_fg: Color::Reset,
            shimmer_fg: Color::Reset,
        }
    }

    /// Pure environment-based palette detector for deterministic testing.
    pub fn detect_from(no_color: bool, term: Option<&str>, colorterm: Option<&str>) -> Self {
        let is_dumb = term
            .map(str::trim)
            .is_some_and(|t| t.eq_ignore_ascii_case("dumb"));
        if no_color || is_dumb {
            return Self::monochrome();
        }
        let is_truecolor = colorterm.map(str::trim).is_some_and(|c| {
            c.eq_ignore_ascii_case("truecolor") || c.eq_ignore_ascii_case("24bit")
        });
        if is_truecolor {
            return Self::truecolor();
        }
        let is_256 = term
            .map(str::trim)
            .is_some_and(|t| t.to_ascii_lowercase().contains("256color"));
        if is_256 {
            Self::ansi256()
        } else {
            Self::ansi16()
        }
    }

    /// Detects the best palette for the current terminal environment.
    pub fn detect() -> Self {
        let no_color = env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        let term = env::var("TERM").ok();
        let colorterm = env::var("COLORTERM").ok();
        Self::detect_from(no_color, term.as_deref(), colorterm.as_deref())
    }

    /// Concise display label of the active color capability for Telemetry.
    pub const fn mode_label(&self) -> &'static str {
        if self.monochrome {
            "Mono"
        } else if matches!(self.bg_base, Color::Rgb(_, _, _)) {
            "TrueColor"
        } else if matches!(self.bg_base, Color::Indexed(_)) {
            "256-Color"
        } else {
            "ANSI-16"
        }
    }
}

/// Returns whether smooth 30fps animations should be enabled for the given env flags.
pub fn should_enable_animations(no_color: bool, term: Option<&str>, ci: Option<&str>) -> bool {
    if no_color || ci.is_some_and(|v| !v.is_empty()) {
        return false;
    }
    !term
        .map(str::trim)
        .is_some_and(|t| t.eq_ignore_ascii_case("dumb"))
}

struct ActiveSession {
    // Field order matters: `terminal` drops before `guard` so `guard` restores
    // the alternate screen and raw mode last.
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    guard: TerminalGuard<CrosstermOps>,
}

/// Queries the active terminal dimensions `(cols, rows)` via `crossterm::terminal::size()`.
pub fn detect_terminal_size() -> Option<(u16, u16)> {
    crossterm::terminal::size().ok()
}

/// Opt-in full-screen Ratatui renderer with lazy terminal initialization, U13/U15 aesthetics,
/// adaptive tick motion polish, and RAII cleanup.
pub struct RatatuiRenderer {
    session: Option<ActiveSession>,
    palette: UiPalette,
    animations: bool,
    filter_active: bool,
    last_split_mode: bool,
    has_active_motion: bool,
}

impl RatatuiRenderer {
    /// Creates a new `RatatuiRenderer` without touching terminal ioctls.
    ///
    /// Terminal raw mode and alternate screen are entered lazily on the first `draw()`
    /// call so constructing under `CI=true` or non-TTY environments is always safe.
    pub fn new() -> Self {
        let no_color = env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        let term = env::var("TERM").ok();
        let ci = env::var("CI").ok();
        let palette = UiPalette::detect();
        let animations = should_enable_animations(no_color, term.as_deref(), ci.as_deref());
        Self {
            session: None,
            palette,
            animations,
            filter_active: false,
            last_split_mode: true,
            has_active_motion: false,
        }
    }

    fn ensure_session(&mut self) -> io::Result<&mut ActiveSession> {
        if self.session.is_none() {
            let guard = TerminalGuard::enter()?;
            let backend = CrosstermBackend::new(io::stdout());
            let terminal = Terminal::new(backend)?;
            self.session = Some(ActiveSession { terminal, guard });
        }
        match self.session.as_mut() {
            Some(session) => Ok(session),
            None => Err(io::Error::other("ratatui session unavailable")),
        }
    }
}

impl Default for RatatuiRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Maps a `crossterm::event::KeyEvent` to `MenuEvent` in normal navigation mode.
///
/// Accepts `KeyEventKind::Press` for all bound keys, plus `KeyEventKind::Repeat` exclusively
/// for `Up`/`Down`/`PageUp`/`PageDown` navigation. Rejects `Release`, non-scroll `Repeat`,
/// Vim-only `h/j/k/l`, and `CONTROL`/`ALT` modifiers on plain bindings (except `Ctrl+C` -> `Quit`).
pub fn map_crossterm_key(key: KeyEvent) -> Option<MenuEvent> {
    match key.kind {
        KeyEventKind::Press => {}
        KeyEventKind::Repeat
            if matches!(
                key.code,
                KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown
            ) => {}
        _ => return None,
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c' | 'C') => Some(MenuEvent::Quit),
            KeyCode::Char('k' | 'K') => Some(MenuEvent::PaletteOpen),
            _ => None,
        };
    }

    let has_ctrl_or_alt = key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);

    match key.code {
        KeyCode::Up => Some(MenuEvent::Up),
        KeyCode::Down => Some(MenuEvent::Down),
        KeyCode::Left | KeyCode::Char('[') if !has_ctrl_or_alt => Some(MenuEvent::PrevTab),
        KeyCode::Right | KeyCode::Char(']') if !has_ctrl_or_alt => Some(MenuEvent::NextTab),
        KeyCode::PageUp if !has_ctrl_or_alt => Some(MenuEvent::PageUp),
        KeyCode::PageDown if !has_ctrl_or_alt => Some(MenuEvent::PageDown),
        KeyCode::Home if !has_ctrl_or_alt => Some(MenuEvent::Home),
        KeyCode::End if !has_ctrl_or_alt => Some(MenuEvent::End),
        KeyCode::Enter if !has_ctrl_or_alt => Some(MenuEvent::Select),
        KeyCode::Esc | KeyCode::Backspace if !has_ctrl_or_alt => Some(MenuEvent::Back),
        KeyCode::Tab if !has_ctrl_or_alt => Some(MenuEvent::TogglePane),
        KeyCode::Char('/') if !has_ctrl_or_alt => Some(MenuEvent::FilterStart),
        KeyCode::Char('q' | 'Q') if !has_ctrl_or_alt => Some(MenuEvent::Quit),
        KeyCode::Char('r' | 'R') if !has_ctrl_or_alt => Some(MenuEvent::Refresh),
        KeyCode::Char('y' | 'Y') if !has_ctrl_or_alt => Some(MenuEvent::ConfirmYes),
        KeyCode::Char('n' | 'N') if !has_ctrl_or_alt => Some(MenuEvent::ConfirmNo),
        KeyCode::Char(ch) if !has_ctrl_or_alt && ch.is_ascii_digit() => {
            Some(MenuEvent::Digit((ch as u8) - b'0'))
        }
        _ => None,
    }
}

/// Maps a `crossterm::event::KeyEvent` while inline `/` fuzzy filter or `Ctrl+K` palette mode is active.
pub fn map_crossterm_key_filtered(key: KeyEvent) -> Option<MenuEvent> {
    if !matches!(key.kind, KeyEventKind::Press) {
        if matches!(key.kind, KeyEventKind::Repeat)
            && matches!(
                key.code,
                KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::PageUp
                    | KeyCode::PageDown
                    | KeyCode::Backspace
            )
        {
            // allow repeating navigation and backspace while typing filter
        } else {
            return None;
        }
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c' | 'C') => Some(MenuEvent::Quit),
            KeyCode::Char('k' | 'K') => Some(MenuEvent::PaletteOpen),
            KeyCode::Char('u' | 'U') => Some(MenuEvent::FilterClearLine),
            KeyCode::Char('w' | 'W') => Some(MenuEvent::FilterDeleteWord),
            _ => None,
        };
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        return None;
    }

    match key.code {
        KeyCode::Up => Some(MenuEvent::Up),
        KeyCode::Down => Some(MenuEvent::Down),
        KeyCode::PageUp => Some(MenuEvent::PageUp),
        KeyCode::PageDown => Some(MenuEvent::PageDown),
        KeyCode::Home => Some(MenuEvent::Home),
        KeyCode::End => Some(MenuEvent::End),
        KeyCode::Enter => Some(MenuEvent::Select),
        KeyCode::Esc => Some(MenuEvent::FilterClear),
        KeyCode::Backspace => Some(MenuEvent::FilterBackspace),
        KeyCode::Char(ch) if !ch.is_control() => Some(MenuEvent::FilterChar(ch)),
        _ => None,
    }
}

impl TuiRenderer for RatatuiRenderer {
    fn draw(&mut self, state: &AppState) -> io::Result<()> {
        self.filter_active = state.is_text_input_active();
        self.animations = state.animations_enabled();
        self.has_active_motion = state.transition_ticks() > 0 || state.notice().is_some();
        let palette = self.palette;
        let session = self.ensure_session()?;
        let mut split_mode = false;
        session.terminal.draw(|frame| {
            split_mode = frame.area().width >= SPLIT_PANE_MIN_WIDTH;
            render_frame_with_palette(frame, state, &palette);
        })?;
        self.last_split_mode = split_mode;
        Ok(())
    }

    fn read_event(&mut self) -> io::Result<MenuEvent> {
        loop {
            if self.animations {
                let cadence = if self.last_split_mode || self.has_active_motion {
                    TICK_INTERVAL
                } else {
                    IDLE_TICK_INTERVAL
                };
                if !event::poll(cadence)? {
                    return Ok(MenuEvent::Tick);
                }
            }
            match event::read()? {
                Event::Key(key) => {
                    let mapped = if self.filter_active {
                        map_crossterm_key_filtered(key)
                    } else {
                        map_crossterm_key(key)
                    };
                    if let Some(ev) = mapped {
                        if ev == MenuEvent::TogglePane && !self.last_split_mode {
                            continue;
                        }
                        return Ok(ev);
                    }
                }
                Event::Resize(cols, _) => {
                    self.last_split_mode = cols >= SPLIT_PANE_MIN_WIDTH;
                    return Ok(MenuEvent::Resize);
                }
                _ => {}
            }
        }
    }

    fn suspend_for_child(&mut self) -> io::Result<()> {
        if let Some(session) = self.session.as_mut() {
            session.guard.suspend()?;
        }
        Ok(())
    }

    fn resume_after_child(&mut self) -> io::Result<()> {
        if let Some(session) = self.session.as_mut() {
            session.guard.resume()?;
            session.terminal.clear()?;
        }
        Ok(())
    }

    fn sync_viewport(&self, state: &mut AppState) {
        state.set_split_pane_available(self.last_split_mode);
    }

    fn is_non_interactive(&self) -> bool {
        is_non_interactive()
    }
}

/// Pure, backend-agnostic Ratatui frame renderer for `AppState` using auto-detected palette.
pub fn render_frame(frame: &mut ratatui::Frame<'_>, state: &AppState) {
    let palette = UiPalette::detect();
    render_frame_with_palette(frame, state, &palette);
}

/// Pure, backend-agnostic Ratatui frame renderer with an explicit `UiPalette`.
pub fn render_frame_with_palette(
    frame: &mut ratatui::Frame<'_>,
    state: &AppState,
    palette: &UiPalette,
) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }

    if !palette.monochrome && palette.bg_base != Color::Reset {
        let bg_block = Block::default().style(Style::default().bg(palette.bg_base));
        frame.render_widget(bg_block, area);
    }

    let header_h = if area.height >= 28 && area.width >= 60 {
        4
    } else {
        3
    };
    let vertical = Layout::vertical([
        Constraint::Length(header_h),
        Constraint::Min(3),
        Constraint::Length(3),
    ])
    .split(area);

    let split_mode = area.width >= SPLIT_PANE_MIN_WIDTH;

    render_header(frame, vertical[0], state, palette);

    let toast_anchor = if split_mode {
        let panes = Layout::horizontal([Constraint::Percentage(46), Constraint::Percentage(54)])
            .split(vertical[1]);
        render_menu_panel(frame, panes[0], state, palette, true);
        if panes[1].height >= 25 {
            // 3-Zone Bento Master-Detail: Telemetry (top-right, 19 rows = 17 inner for all 18 tools) + Live Action Inspector (bottom-right).
            let right_cards =
                Layout::vertical([Constraint::Length(19), Constraint::Min(6)]).split(panes[1]);
            render_status_panel(frame, right_cards[0], state, palette, false);
            render_inspector_panel(frame, right_cards[1], state, palette);
            right_cards[1]
        } else {
            render_status_panel(frame, panes[1], state, palette, true);
            panes[1]
        }
    } else {
        render_menu_panel(frame, vertical[1], state, palette, false);
        vertical[1]
    };

    let has_modal = state.pending_confirm().is_some() || state.palette_active();
    let show_floating_toast = !has_modal
        && state.notice().is_some()
        && toast_anchor.width >= 24
        && toast_anchor.height >= 5;

    render_footer(
        frame,
        vertical[2],
        state,
        palette,
        split_mode,
        !show_floating_toast,
    );

    if state.animations_enabled() && state.transition_ticks() > 0 && !palette.monochrome {
        apply_wipe_transition(frame, vertical[1], state.transition_ticks(), palette);
    }

    if show_floating_toast && let Some(msg) = state.notice() {
        render_toast(frame, toast_anchor, area, msg, state.notice_kind(), palette);
    }

    if state.palette_active() {
        apply_dimmed_backdrop(frame, vertical[0], palette);
        apply_dimmed_backdrop(frame, vertical[1], palette);
        render_palette_modal(frame, vertical[1], area, state, palette);
    } else if let Some(confirm) = state.pending_confirm() {
        apply_dimmed_backdrop(frame, vertical[0], palette);
        apply_dimmed_backdrop(frame, vertical[1], palette);
        render_confirm_modal(
            frame,
            vertical[1],
            area,
            &confirm.message,
            confirm.is_destructive(),
            palette,
        );
    }
}

/// Smooth continuous cosine RGB pulse between `border_active` and `accent_secondary`.
fn active_border_color(state: &AppState, palette: &UiPalette) -> Color {
    if palette.monochrome {
        return Color::Reset;
    }
    if !state.animations_enabled() {
        return palette.border_active;
    }
    match (palette.border_active, palette.accent_secondary) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let phase = (state.tick_count() as f64) * std::f64::consts::TAU / 60.0;
            let t = 0.5 * (1.0 - phase.cos());
            let lerp = |a: u8, b: u8| -> u8 {
                (f64::from(a) + (f64::from(b) - f64::from(a)) * t).round() as u8
            };
            Color::Rgb(lerp(r1, r2), lerp(g1, g2), lerp(b1, b2))
        }
        _ => {
            if (state.tick_count() / 30) % 2 == 1 {
                palette.accent_secondary
            } else {
                palette.border_active
            }
        }
    }
}

fn title_badge(text: &str, active: bool, palette: &UiPalette, max_width: u16) -> Line<'static> {
    let border_fg = if active {
        palette.border_active
    } else {
        palette.border_inactive
    };
    let fg = if active {
        palette.title_badge_fg
    } else {
        palette.text_muted
    };
    let mut text_style = Style::default().fg(fg);
    if active {
        text_style = text_style.add_modifier(Modifier::BOLD);
    }
    // Reserve space for rounded corners + `┤ ` and ` ├` (8 columns total).
    let max_label_w = max_width.saturating_sub(8).max(4) as usize;
    let fitted = if visual_width(text) > max_label_w {
        let trunc = truncate_to_width(text, max_label_w.saturating_sub(1));
        format!("{trunc}…")
    } else {
        text.to_string()
    };
    Line::from(vec![
        Span::styled("┤ ", Style::default().fg(border_fg)),
        Span::styled(fitted, text_style),
        Span::styled(" ├", Style::default().fg(border_fg)),
    ])
}

fn capsule_style(ok: bool, warn_on_off: bool, palette: &UiPalette) -> (Style, Style) {
    if palette.monochrome {
        return (Style::default(), Style::default());
    }
    if ok {
        let bg = palette.capsule_ok_bg;
        (
            Style::default().fg(palette.success).bg(bg),
            Style::default().fg(palette.success).bg(bg),
        )
    } else if warn_on_off {
        let bg = palette.capsule_warn_bg;
        (
            Style::default().fg(palette.warning).bg(bg),
            Style::default().fg(palette.warning).bg(bg),
        )
    } else {
        let bg = palette.capsule_off_bg;
        (
            Style::default().fg(palette.text_inactive).bg(bg),
            Style::default().fg(palette.text_inactive).bg(bg),
        )
    }
}

fn render_header(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &AppState,
    palette: &UiPalette,
) {
    let screen = state.current_screen();
    let status = &state.status;
    let accent = active_border_color(state, palette);

    let title_text = if screen.id == MenuId::Main {
        "◆ debopty".to_string()
    } else {
        format!("◆ {}", screen.title)
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent))
        .style(Style::default().bg(palette.bg_surface))
        .title(title_badge(&title_text, true, palette, area.width));

    if area.width >= 44 {
        let right_max = (area.width / 2).saturating_sub(4).max(8);
        block = block.title(
            title_badge(status.version_display.as_str(), false, palette, right_max)
                .alignment(Alignment::Right),
        );
    }

    let inner_w = area.width.saturating_sub(2) as usize;
    let usable_w = inner_w.saturating_sub(2);
    let total_tools = status.tools.len();
    let installed_tools = status.tools.values().filter(|&&v| v).count();
    let spinner = if state.animations_enabled() {
        BRAILLE_SPINNER[((state.tick_count() / 3) as usize) % BRAILLE_SPINNER.len()]
    } else {
        "✦"
    };
    let region = if status.is_cn_region {
        "CN · 镜像"
    } else {
        "海外 · 直连"
    };

    let (left_spans, left_w): (Vec<Span<'static>>, usize) = if screen.id == MenuId::Main {
        let engine = format!("{spinner} Pure Rust (21/21)");
        let full_w = visual_width(&engine) + 3 + visual_width(region);
        if full_w <= usable_w {
            (
                vec![
                    Span::styled(
                        engine,
                        Style::default()
                            .fg(palette.success)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" │ ", Style::default().fg(palette.border_inactive)),
                    Span::styled(region, Style::default().fg(palette.text_primary)),
                ],
                full_w,
            )
        } else {
            let fitted = truncate_to_width(&format!("{engine} │ {region}"), usable_w);
            let w = visual_width(&fitted);
            (
                vec![Span::styled(fitted, Style::default().fg(palette.success))],
                w,
            )
        }
    } else {
        let fitted = truncate_to_width(&screen.subtitle, usable_w);
        let w = visual_width(&fitted);
        (
            vec![Span::styled(
                fitted,
                Style::default().fg(palette.title_badge_fg),
            )],
            w,
        )
    };

    let opt_dot = if status.base_optimized { "●" } else { "○" };
    let (opt_dot_s, opt_txt_s) = capsule_style(status.base_optimized, true, palette);
    let fwd_dot = if status.ip_forward { "●" } else { "○" };
    let (fwd_dot_s, fwd_txt_s) = capsule_style(status.ip_forward, false, palette);
    let count_pill = format!(" {installed_tools}/{total_tools} ");
    let count_pill_style = if palette.monochrome {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(palette.accent_primary)
            .bg(palette.pill_inactive_bg)
            .add_modifier(Modifier::BOLD)
    };
    // ` ● 优化 ` (8) + ` ` (1) + ` ● 转发 ` (8) + ` ` (1) + ` 8/18 ` (6) = 24
    let full_right_w = 8 + 1 + 8 + 1 + visual_width(&count_pill);
    let compact_right_w = visual_width(&count_pill);

    let mut row1_spans = Vec::with_capacity(16);
    row1_spans.push(Span::raw(" "));
    row1_spans.extend(left_spans);

    if usable_w >= left_w + 2 + full_right_w {
        let gap = usable_w - left_w - full_right_w;
        row1_spans.push(Span::raw(" ".repeat(gap)));
        row1_spans.push(Span::styled(format!(" {opt_dot}"), opt_dot_s));
        row1_spans.push(Span::styled(" 优化 ", opt_txt_s));
        row1_spans.push(Span::raw(" "));
        row1_spans.push(Span::styled(format!(" {fwd_dot}"), fwd_dot_s));
        row1_spans.push(Span::styled(" 转发 ", fwd_txt_s));
        row1_spans.push(Span::raw(" "));
        row1_spans.push(Span::styled(count_pill, count_pill_style));
    } else if usable_w >= left_w + 2 + compact_right_w {
        let gap = usable_w - left_w - compact_right_w;
        row1_spans.push(Span::raw(" ".repeat(gap)));
        row1_spans.push(Span::styled(count_pill, count_pill_style));
    }

    let mut header_lines = vec![Line::from(row1_spans)];

    // Row 2 (when 4-row Header active): Modern Filled Workspace Pill Tabs + `[Ctrl+K]` trigger chip.
    if area.height >= 4 {
        let active_tab = state.workspace_tab();
        let mut row2_spans = vec![Span::raw(" ")];
        let mut tabs_w = 0usize;
        for (idx, tab) in WorkspaceTab::ALL.iter().enumerate() {
            let pill_txt = format!(" {} ", tab.label());
            let pill_w = visual_width(&pill_txt);
            let sep_w = usize::from(idx > 0);
            if tabs_w + sep_w + pill_w > usable_w {
                break;
            }
            if idx > 0 {
                row2_spans.push(Span::raw(" "));
                tabs_w += 1;
            }
            let is_active = *tab == active_tab && screen.id == MenuId::Main;
            let pill_style = if is_active {
                if palette.monochrome {
                    Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default()
                        .fg(palette.pill_active_fg)
                        .bg(palette.pill_active_bg)
                        .add_modifier(Modifier::BOLD)
                }
            } else if palette.monochrome {
                Style::default().add_modifier(Modifier::DIM)
            } else {
                Style::default()
                    .fg(palette.text_muted)
                    .bg(palette.pill_inactive_bg)
            };
            row2_spans.push(Span::styled(pill_txt, pill_style));
            tabs_w += pill_w;
        }

        let nav_hint = "[←→] 切页";
        let cmd_chip = " Ctrl+K 命令 ";
        let full_chip_w = visual_width(nav_hint) + 2 + visual_width(cmd_chip);
        let cmd_only_w = visual_width(cmd_chip);
        let cmd_style = if palette.monochrome {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(palette.title_badge_fg)
                .bg(palette.pill_inactive_bg)
                .add_modifier(Modifier::BOLD)
        };
        if usable_w >= tabs_w + 2 + full_chip_w {
            let gap = usable_w - tabs_w - full_chip_w;
            row2_spans.push(Span::raw(" ".repeat(gap)));
            row2_spans.push(Span::styled(
                nav_hint,
                Style::default().fg(palette.text_muted),
            ));
            row2_spans.push(Span::raw("  "));
            row2_spans.push(Span::styled(cmd_chip, cmd_style));
        } else if usable_w >= tabs_w + 2 + cmd_only_w {
            let gap = usable_w - tabs_w - cmd_only_w;
            row2_spans.push(Span::raw(" ".repeat(gap)));
            row2_spans.push(Span::styled(cmd_chip, cmd_style));
        }

        header_lines.push(Line::from(row2_spans));
    }

    let paragraph = Paragraph::new(header_lines).block(block);
    frame.render_widget(paragraph, area);
}

fn render_menu_section_line(label: &str, content_w: usize, palette: &UiPalette) -> Line<'static> {
    let divider_w = content_w.saturating_sub(2).max(1);
    let prefix = format!("── {label} ");
    let prefix_w = visual_width(&prefix);
    if prefix_w >= divider_w {
        return Line::from(vec![
            Span::raw(" "),
            Span::styled(
                truncate_to_width(&prefix, divider_w),
                Style::default()
                    .fg(palette.text_muted)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
    }
    let tail = "─".repeat(divider_w - prefix_w);
    Line::from(vec![
        Span::raw(" "),
        Span::styled("── ", Style::default().fg(palette.border_inactive)),
        Span::styled(
            label.to_string(),
            Style::default()
                .fg(palette.text_muted)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {tail}"),
            Style::default().fg(palette.border_inactive),
        ),
    ])
}

fn render_menu_panel(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &AppState,
    palette: &UiPalette,
    split_mode: bool,
) {
    let screen = state.current_screen();
    let is_focused = !split_mode || state.active_pane() == ActivePane::Menu;
    let inner_w = area.width.saturating_sub(2) as usize;
    let viewport_h = area.height.saturating_sub(2) as usize;
    let filter_query = state.filter_query();
    let filtering = !filter_query.is_empty();
    let total_items = screen.items().len();

    let extra_section_spacers = !filtering && viewport_h >= 22;
    let section_count = screen
        .entries
        .iter()
        .filter(|e| matches!(e, MenuEntry::Section(_)))
        .count();
    let spacer_rows = if extra_section_spacers {
        section_count.saturating_sub(1)
    } else {
        0
    };

    let matched_items = if filtering {
        state.filtered_items()
    } else {
        Vec::new()
    };

    let raw_row_count = if filtering {
        matched_items.len().max(1)
    } else {
        screen.entries.len() + spacer_rows
    };
    let needs_scrollbar = raw_row_count > viewport_h && area.height >= 4;
    let content_w = if needs_scrollbar {
        inner_w.saturating_sub(1).max(1)
    } else {
        inner_w
    };
    let divider_w = content_w.saturating_sub(2).max(1);

    let mut list_items = Vec::with_capacity(raw_row_count);
    let mut item_idx = 0usize;
    let mut selected_row = None;

    if filtering {
        if matched_items.is_empty() {
            let empty_msg = format!(" 无匹配项: \"{filter_query}\"  [Esc] 清除");
            let fitted = if visual_width(&empty_msg) > content_w {
                if content_w > 1 {
                    format!("{}…", truncate_to_width(&empty_msg, content_w - 1))
                } else {
                    truncate_to_width(&empty_msg, content_w)
                }
            } else {
                empty_msg
            };
            list_items.push(ListItem::new(Line::from(Span::styled(
                fitted,
                Style::default().fg(palette.warning),
            ))));
        } else {
            for (idx, item) in matched_items.iter().enumerate() {
                let is_selected = idx == state.selected_idx();
                if is_selected {
                    selected_row = Some(idx);
                }
                list_items.push(ListItem::new(build_menu_line(
                    item,
                    is_selected,
                    is_focused,
                    content_w,
                    palette,
                )));
            }
        }
    } else {
        let mut seen_sections = 0usize;
        for entry in &screen.entries {
            match entry {
                MenuEntry::Section(label) => {
                    if extra_section_spacers && seen_sections > 0 {
                        list_items.push(ListItem::new(Line::from("")));
                    }
                    seen_sections += 1;
                    list_items.push(ListItem::new(render_menu_section_line(
                        label, content_w, palette,
                    )));
                }
                MenuEntry::Separator => {
                    let line = Line::from(Span::styled(
                        format!(" {}", "─".repeat(divider_w)),
                        Style::default().fg(palette.border_inactive),
                    ));
                    list_items.push(ListItem::new(line));
                }
                MenuEntry::Item(item) => {
                    let is_selected = item_idx == state.selected_idx();
                    if is_selected {
                        selected_row = Some(list_items.len());
                    }
                    list_items.push(ListItem::new(build_menu_line(
                        item,
                        is_selected,
                        is_focused,
                        content_w,
                        palette,
                    )));
                    item_idx += 1;
                }
            }
        }
    }

    let panel_title = if state.filter_active() {
        let max_label_w = area.width.saturating_sub(8).max(4) as usize;
        let prefix = "功能菜单 / ";
        let count_suffix = if filtering {
            format!(" ({}/{})", matched_items.len(), total_items)
        } else {
            String::new()
        };
        let suffix_w = visual_width(&count_suffix);
        if visual_width(prefix) + 4 + suffix_w <= max_label_w {
            let q_budget = max_label_w.saturating_sub(visual_width(prefix) + 1 + suffix_w);
            let fitted_q = tail_truncate_to_width(filter_query, q_budget);
            format!("{prefix}{fitted_q}▏{count_suffix}")
        } else {
            let q_budget = max_label_w.saturating_sub(visual_width(prefix) + 1);
            let fitted_q = tail_truncate_to_width(filter_query, q_budget);
            format!("{prefix}{fitted_q}▏")
        }
    } else {
        "功能菜单".to_string()
    };

    let border_color = if is_focused {
        active_border_color(state, palette)
    } else {
        palette.border_inactive
    };

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(palette.bg_surface))
        .title(title_badge(&panel_title, is_focused, palette, area.width));

    if needs_scrollbar && area.width >= 20 {
        let sel_pos = selected_row.unwrap_or(0) + 1;
        block = block.title_bottom(
            Line::from(Span::styled(
                format!(" ↕ {sel_pos}/{raw_row_count} "),
                Style::default().fg(palette.text_muted),
            ))
            .alignment(Alignment::Right),
        );
    }

    let list = List::new(list_items).block(block);
    let mut list_state = ListState::default();
    list_state.select(selected_row);
    frame.render_stateful_widget(list, area, &mut list_state);

    if needs_scrollbar {
        let mut sb_state = ScrollbarState::new(raw_row_count)
            .position(selected_row.unwrap_or(0))
            .viewport_content_length(viewport_h);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("┃")
            .track_style(Style::default().fg(palette.border_inactive))
            .thumb_style(Style::default().fg(if is_focused {
                palette.accent_primary
            } else {
                palette.text_muted
            }));
        frame.render_stateful_widget(
            scrollbar,
            area.inner(Margin {
                vertical: 1,
                horizontal: 1,
            }),
            &mut sb_state,
        );
    }
}

fn build_menu_line(
    item: &MenuItem,
    selected: bool,
    focused: bool,
    inner_width: usize,
    palette: &UiPalette,
) -> Line<'static> {
    let active_selected = selected && focused;
    let use_highlight_bg =
        active_selected && !palette.monochrome && palette.bg_highlight != Color::Reset;
    let row_bg = if use_highlight_bg {
        palette.bg_highlight
    } else {
        palette.bg_surface
    };

    let highlight = if !focused {
        if palette.monochrome {
            Style::default().add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(palette.text_muted).bg(row_bg)
        }
    } else if palette.monochrome {
        Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else if palette.bg_highlight == Color::Reset {
        Style::default()
            .fg(palette.accent_primary)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default()
            .fg(palette.accent_primary)
            .bg(row_bg)
            .add_modifier(Modifier::BOLD)
    };
    let bar_style = if !focused {
        if palette.monochrome {
            Style::default().add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(palette.border_inactive).bg(row_bg)
        }
    } else if palette.monochrome {
        Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else if palette.bg_highlight == Color::Reset {
        Style::default()
            .fg(palette.accent_primary)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(palette.accent_primary)
            .bg(row_bg)
            .add_modifier(Modifier::BOLD)
    };
    let normal_text = Style::default().fg(palette.text_primary).bg(row_bg);
    let idx_style = if selected {
        highlight
    } else {
        Style::default().fg(palette.text_muted).bg(row_bg)
    };
    let gutter_style = Style::default()
        .fg(if active_selected {
            palette.accent_secondary
        } else {
            palette.border_inactive
        })
        .bg(row_bg);
    let label_style = if selected { highlight } else { normal_text };
    let spacer_style = Style::default().bg(row_bg);

    let pointer = if selected { "▎" } else { " " };
    let id_col = format!(" {:>2} ", item.id);
    // Fixed tabular prefix: `▎` (1) + ` 10 ` (4) + `│ ` (2) = 7 columns exact.
    let prefix_w = 7usize;

    match item.status {
        Some(badge) => {
            let badge_plain = badge.plain_text();
            // Capsule badge: ` ● 已优化 ` => badge_w + 2 cols padding inside tinted capsule.
            let capsule_w = visual_width(&badge_plain) + 2;
            // Layout: prefix (7) + label + gap (1) + capsule (10) + trailing pad (1)
            let max_label_w = inner_width.saturating_sub(prefix_w + capsule_w + 2);
            let fitted_label = truncate_to_width(item.label, max_label_w);
            let padded_label = pad_right_to_width(&fitted_label, max_label_w);

            let (cap_bg, dot_fg, text_fg, dot, text) = match badge {
                StatusBadge::Active(label) => (
                    if palette.monochrome {
                        row_bg
                    } else {
                        palette.capsule_ok_bg
                    },
                    palette.success,
                    if active_selected {
                        palette.text_primary
                    } else {
                        palette.success
                    },
                    "●",
                    label,
                ),
                StatusBadge::Inactive(label) => {
                    let is_warn = label == "未执行";
                    (
                        if palette.monochrome {
                            row_bg
                        } else if is_warn {
                            palette.capsule_warn_bg
                        } else {
                            palette.capsule_off_bg
                        },
                        if is_warn {
                            palette.warning
                        } else {
                            palette.text_inactive
                        },
                        if is_warn {
                            palette.warning
                        } else if active_selected {
                            palette.text_muted
                        } else {
                            palette.text_inactive
                        },
                        "○",
                        label,
                    )
                }
            };

            let dot_style = Style::default().fg(dot_fg).bg(cap_bg);
            let badge_text_style = Style::default().fg(text_fg).bg(cap_bg);
            let used = prefix_w + visual_width(&padded_label) + 1 + capsule_w;
            let trail = " ".repeat(inner_width.saturating_sub(used));

            Line::from(vec![
                Span::styled(pointer, if selected { bar_style } else { normal_text }),
                Span::styled(id_col, idx_style),
                Span::styled("│ ", gutter_style),
                Span::styled(padded_label, label_style),
                Span::styled(" ", spacer_style),
                Span::styled(format!(" {dot} "), dot_style),
                Span::styled(format!("{text} "), badge_text_style),
                Span::styled(trail, spacer_style),
            ])
        }
        None => {
            let is_nav = matches!(item.action, MenuAction::Navigate(_));
            let suffix = if is_nav { "› " } else { " " };
            let suffix_w = visual_width(suffix);
            let max_label_w = inner_width.saturating_sub(prefix_w + suffix_w);
            let fitted_label = truncate_to_width(item.label, max_label_w);
            let padded_label = pad_right_to_width(&fitted_label, max_label_w);
            let suffix_style = Style::default()
                .fg(if active_selected {
                    palette.accent_primary
                } else {
                    palette.text_inactive
                })
                .bg(row_bg);
            Line::from(vec![
                Span::styled(pointer, if selected { bar_style } else { normal_text }),
                Span::styled(id_col, idx_style),
                Span::styled("│ ", gutter_style),
                Span::styled(padded_label, label_style),
                Span::styled(suffix, suffix_style),
            ])
        }
    }
}

/// Builds a Braille micro-sparkline representing deployed tool coverage + live pulse.
fn render_braille_sparkline(installed: usize, total: usize, tick: u64, slots: usize) -> String {
    let slots = slots.clamp(4, 16);
    let ratio = if total == 0 {
        0.0
    } else {
        (installed as f64) / (total as f64)
    };
    let filled_slots = ((ratio * (slots as f64)).round() as usize).min(slots);
    let mut out = String::with_capacity(slots * 3);
    for i in 0..slots {
        let level = if i < filled_slots {
            let wave = ((tick as usize).wrapping_add(i)) % 3;
            let idx =
                (BRAILLE_LEVELS.len() - 1).saturating_sub(if tick == 0 { 0 } else { wave % 2 });
            BRAILLE_LEVELS[idx]
        } else if i == filled_slots && installed > 0 {
            BRAILLE_LEVELS[2]
        } else {
            BRAILLE_LEVELS[0]
        };
        out.push(level);
    }
    out
}

fn telemetry_section_header(
    label: &str,
    right_hint: Option<&str>,
    usable_w: usize,
    palette: &UiPalette,
) -> Line<'static> {
    let prefix = format!("── {label} ");
    let prefix_w = visual_width(&prefix);
    let rule_style = Style::default().fg(palette.border_inactive);
    let label_style = Style::default()
        .fg(palette.title_badge_fg)
        .add_modifier(Modifier::BOLD);
    let hint_style = Style::default().fg(palette.text_muted);

    if let Some(hint) = right_hint {
        let hint_seg = format!(" {hint}");
        let hint_w = visual_width(&hint_seg);
        if prefix_w + hint_w + 2 <= usable_w {
            let fill = "─".repeat(usable_w - prefix_w - hint_w);
            return Line::from(vec![
                Span::raw(" "),
                Span::styled("── ", rule_style),
                Span::styled(label.to_string(), label_style),
                Span::styled(format!(" {fill}"), rule_style),
                Span::styled(hint_seg, hint_style),
            ]);
        }
    }

    if prefix_w >= usable_w {
        return Line::from(vec![
            Span::raw(" "),
            Span::styled(truncate_to_width(&prefix, usable_w), label_style),
        ]);
    }
    let fill = "─".repeat(usable_w - prefix_w);
    Line::from(vec![
        Span::raw(" "),
        Span::styled("── ", rule_style),
        Span::styled(label.to_string(), label_style),
        Span::styled(format!(" {fill}"), rule_style),
    ])
}

fn render_bar_spans(
    installed: usize,
    total: usize,
    bar_w: usize,
    palette: &UiPalette,
) -> Vec<Span<'static>> {
    if bar_w == 0 {
        return Vec::new();
    }
    let filled = if total == 0 {
        0
    } else {
        ((installed * bar_w * 2 + total) / (total * 2)).min(bar_w)
    };
    let empty = bar_w.saturating_sub(filled);
    let fill_color = if total > 0 && installed * 3 >= total * 2 {
        palette.success
    } else if installed > 0 {
        palette.accent_primary
    } else {
        palette.text_inactive
    };
    let mut spans = Vec::with_capacity(2);
    if filled > 0 {
        spans.push(Span::styled(
            "━".repeat(filled),
            Style::default().fg(fill_color),
        ));
    }
    if empty > 0 {
        spans.push(Span::styled(
            "─".repeat(empty),
            Style::default().fg(palette.border_inactive),
        ));
    }
    spans
}

#[allow(clippy::too_many_arguments)]
fn push_kv_cell(
    spans: &mut Vec<Span<'static>>,
    key: &str,
    key_w: usize,
    val: &str,
    val_style: Style,
    cell_w: usize,
    pad_cell: bool,
    palette: &UiPalette,
) {
    let key_cell = pad_right_to_width(&truncate_to_width(key, key_w), key_w);
    let val_budget = cell_w.saturating_sub(key_w);
    let fitted_val = truncate_to_width(val, val_budget);
    let val_cell = if pad_cell {
        pad_right_to_width(&fitted_val, val_budget)
    } else {
        fitted_val
    };
    spans.push(Span::styled(
        key_cell,
        Style::default().fg(palette.text_muted),
    ));
    spans.push(Span::styled(val_cell, val_style));
}

#[allow(clippy::too_many_arguments)]
fn push_stack_bar_cell(
    spans: &mut Vec<Span<'static>>,
    label: &str,
    key_w: usize,
    installed: usize,
    total: usize,
    cell_w: usize,
    pad_right: bool,
    palette: &UiPalette,
) {
    let pct = (installed * 100).checked_div(total).unwrap_or(0);
    let key_cell = pad_right_to_width(label, key_w);
    let stats = format!("{installed}/{total} {pct:>3}% ");
    let prefix_w = key_w + visual_width(&stats);
    let bar_w = cell_w.saturating_sub(prefix_w);
    let pct_color = if total > 0 && installed * 3 >= total * 2 {
        palette.success
    } else if installed > 0 {
        palette.accent_primary
    } else {
        palette.text_inactive
    };
    spans.push(Span::styled(
        key_cell,
        Style::default().fg(palette.text_muted),
    ));
    spans.push(Span::styled(stats, Style::default().fg(pct_color)));
    if bar_w > 0 {
        spans.extend(render_bar_spans(installed, total, bar_w, palette));
    } else if pad_right && prefix_w < cell_w {
        spans.push(Span::raw(" ".repeat(cell_w - prefix_w)));
    }
}

fn build_telemetry_lines(
    state: &AppState,
    content_w: usize,
    include_inspector: bool,
    palette: &UiPalette,
) -> Vec<Line<'static>> {
    let status = &state.status;
    let usable_w = content_w.saturating_sub(2).max(1);
    let total_tools = status.tools.len();
    let installed_tools = status.tools.values().filter(|&&v| v).count();
    let missing_tools = total_tools.saturating_sub(installed_tools);
    let pct = (installed_tools * 100)
        .checked_div(total_tools)
        .unwrap_or(0);

    // ~10fps spinner cadence (step every 3 ticks at 30fps) to avoid 30fps blur.
    let spinner = if state.animations_enabled() {
        BRAILLE_SPINNER[((state.tick_count() / 3) as usize) % BRAILLE_SPINNER.len()]
    } else {
        "✦"
    };

    let region_label = if status.is_cn_region {
        "CN · 镜像"
    } else {
        "海外 · 直连"
    };
    let (base_opt_label, base_opt_style) = if status.base_optimized {
        ("● 已优化", Style::default().fg(palette.success))
    } else {
        ("○ 未执行", Style::default().fg(palette.warning))
    };
    let (ip_fwd_label, ip_fwd_style) = if status.ip_forward {
        ("● 已开启", Style::default().fg(palette.success))
    } else {
        ("○ 已关闭", Style::default().fg(palette.text_inactive))
    };
    let (podman_gc_label, podman_gc_style) = if status.podman_gc_enabled {
        ("● 已开启", Style::default().fg(palette.success))
    } else {
        ("○ 已关闭", Style::default().fg(palette.text_inactive))
    };

    let engine_val = format!("{spinner} Pure Rust");
    let engine_style = Style::default()
        .fg(palette.success)
        .add_modifier(Modifier::BOLD);
    let ver_style = Style::default().fg(palette.accent_primary);
    let primary_style = Style::default().fg(palette.text_primary);
    let muted_style = Style::default().fg(palette.text_muted);
    let sep_style = Style::default().fg(palette.border_inactive);

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(28);

    // ── Section 1: Runtime State (Strictly Aligned 2-Column KV Grid) ──
    lines.push(telemetry_section_header("运行态", None, usable_w, palette));

    let kv_pairs: [(&str, &str, Style, &str, &str, Style); 4] = [
        (
            "引擎",
            &engine_val,
            engine_style,
            "版本",
            status.version_display.as_str(),
            ver_style,
        ),
        (
            "区域",
            region_label,
            primary_style,
            "色彩",
            palette.mode_label(),
            muted_style,
        ),
        (
            "优化",
            base_opt_label,
            base_opt_style,
            "转发",
            ip_fwd_label,
            ip_fwd_style,
        ),
        (
            "GC",
            podman_gc_label,
            podman_gc_style,
            "模块",
            "21/21 Native",
            ver_style,
        ),
    ];

    let two_col_kv = usable_w >= 38;
    let key_w = if usable_w >= 50 { 6 } else { 5 };
    if two_col_kv {
        let left_w = (usable_w.saturating_sub(3)) / 2;
        let right_w = usable_w.saturating_sub(3 + left_w);
        for (lk, lv, ls, rk, rv, rs) in kv_pairs {
            let mut spans = vec![Span::raw(" ")];
            push_kv_cell(&mut spans, lk, key_w, lv, ls, left_w, true, palette);
            spans.push(Span::styled(" │ ", sep_style));
            push_kv_cell(&mut spans, rk, key_w, rv, rs, right_w, false, palette);
            lines.push(Line::from(spans));
        }
    } else {
        for (lk, lv, ls, rk, rv, rs) in kv_pairs {
            let mut s1 = vec![Span::raw(" ")];
            push_kv_cell(&mut s1, lk, key_w, lv, ls, usable_w, false, palette);
            lines.push(Line::from(s1));
            let mut s2 = vec![Span::raw(" ")];
            push_kv_cell(&mut s2, rk, key_w, rv, rs, usable_w, false, palette);
            lines.push(Line::from(s2));
        }
    }

    // ── Section 2: Coverage Bars + Sparkline (Total + 4 Functional Stacks) ──
    let cov_hint = format!("{installed_tools}/{total_tools} · {pct}%");
    lines.push(telemetry_section_header(
        "覆盖率",
        Some(&cov_hint),
        usable_w,
        palette,
    ));

    let total_key = pad_right_to_width("总计", key_w);
    let ratio_str = format!("{installed_tools}/{total_tools}");
    let total_stat = format!("{ratio_str:<5} {pct:>3}%  ");
    let total_prefix_w = key_w + visual_width(&total_stat);
    let spark_slots = if usable_w >= 52 {
        10
    } else if usable_w >= 40 {
        6
    } else {
        0
    };
    let spark_reserve = if spark_slots > 0 { spark_slots + 2 } else { 0 };
    let total_bar_w = usable_w.saturating_sub(total_prefix_w + spark_reserve);
    let spark_color = if total_tools > 0 && installed_tools * 3 >= total_tools * 2 {
        palette.success
    } else if installed_tools > 0 {
        palette.accent_primary
    } else {
        palette.warning
    };

    let mut total_spans = vec![
        Span::raw(" "),
        Span::styled(total_key, muted_style),
        Span::styled(
            total_stat,
            Style::default()
                .fg(palette.accent_primary)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    total_spans.extend(render_bar_spans(
        installed_tools,
        total_tools,
        total_bar_w,
        palette,
    ));
    if spark_slots > 0 {
        let sparkline = render_braille_sparkline(
            installed_tools,
            total_tools,
            if state.animations_enabled() {
                state.tick_count() / 6
            } else {
                0
            },
            spark_slots,
        );
        total_spans.push(Span::raw("  "));
        total_spans.push(Span::styled(sparkline, Style::default().fg(spark_color)));
    }
    lines.push(Line::from(total_spans));

    let stack_stats: Vec<(&'static str, usize, usize)> = TOOL_STACKS
        .iter()
        .map(|&(label, keys)| {
            let mut present = 0usize;
            let mut count = 0usize;
            for &k in keys {
                if let Some(&inst) = status.tools.get(k) {
                    count += 1;
                    if inst {
                        present += 1;
                    }
                }
            }
            (label, present, count)
        })
        .collect();

    if usable_w >= 40 {
        let left_w = (usable_w.saturating_sub(3)) / 2;
        let right_w = usable_w.saturating_sub(3 + left_w);
        for pair in stack_stats.chunks(2) {
            let mut spans = vec![Span::raw(" ")];
            if let Some(&(l_lbl, l_inst, l_tot)) = pair.first() {
                push_stack_bar_cell(
                    &mut spans, l_lbl, key_w, l_inst, l_tot, left_w, true, palette,
                );
            }
            if let Some(&(r_lbl, r_inst, r_tot)) = pair.get(1) {
                spans.push(Span::styled(" │ ", sep_style));
                push_stack_bar_cell(
                    &mut spans, r_lbl, key_w, r_inst, r_tot, right_w, false, palette,
                );
            }
            lines.push(Line::from(spans));
        }
    } else {
        for &(lbl, inst, tot) in &stack_stats {
            let mut spans = vec![Span::raw(" ")];
            push_stack_bar_cell(&mut spans, lbl, key_w, inst, tot, usable_w, false, palette);
            lines.push(Line::from(spans));
        }
    }

    // ── Section 3: Grouped Aligned Tool Matrix (3-col / 2-col / 1-col) ──
    let matrix_hint = format!("{installed_tools} 就绪 · {missing_tools} 未部署");
    lines.push(telemetry_section_header(
        "工具矩阵",
        Some(&matrix_hint),
        usable_w,
        palette,
    ));

    let mut groups: Vec<(&str, Vec<(&str, bool)>)> = Vec::with_capacity(TOOL_STACKS.len() + 1);
    for &(label, keys) in TOOL_STACKS {
        let mut entries: Vec<(&str, bool)> = keys
            .iter()
            .filter_map(|&k| status.tools.get(k).map(|&v| (k, v)))
            .collect();
        entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        if !entries.is_empty() {
            groups.push((label, entries));
        }
    }
    let mut extras: Vec<(&str, bool)> = status
        .tools
        .iter()
        .filter(|(k, _)| {
            !TOOL_STACKS
                .iter()
                .any(|(_, keys)| keys.contains(&k.as_str()))
        })
        .map(|(k, &v)| (k.as_str(), v))
        .collect();
    if !extras.is_empty() {
        extras.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        groups.push(("其他", extras));
    }

    let tag_w = if usable_w >= 40 { 6usize } else { 0usize };
    let grid_w = usable_w.saturating_sub(tag_w);
    let cols = if grid_w >= 48 {
        3usize
    } else if grid_w >= 28 {
        2usize
    } else {
        1usize
    };
    let col_w = (grid_w.saturating_sub(cols.saturating_sub(1))) / cols;

    for (group_label, entries) in groups {
        for (row_i, chunk) in entries.chunks(cols).enumerate() {
            let mut spans = vec![Span::raw(" ")];
            if tag_w > 0 {
                let tag_text = if row_i == 0 {
                    pad_right_to_width(group_label, tag_w)
                } else {
                    " ".repeat(tag_w)
                };
                spans.push(Span::styled(
                    tag_text,
                    Style::default().fg(palette.accent_secondary),
                ));
            }
            for (ci, &(tool, installed)) in chunk.iter().enumerate() {
                let (dot, dot_style, name_style) = if installed {
                    ("●", Style::default().fg(palette.success), primary_style)
                } else {
                    (
                        "○",
                        Style::default().fg(palette.text_inactive),
                        Style::default().fg(palette.text_inactive),
                    )
                };
                let name_budget = col_w.saturating_sub(2);
                let fitted_name = truncate_to_width(tool, name_budget);
                let is_last = ci + 1 == chunk.len();
                let padded_name = if is_last {
                    fitted_name
                } else {
                    pad_right_to_width(&fitted_name, name_budget)
                };
                spans.push(Span::styled(format!("{dot} "), dot_style));
                spans.push(Span::styled(padded_name, name_style));
                if !is_last {
                    spans.push(Span::raw(" "));
                }
            }
            lines.push(Line::from(spans));
        }
    }

    // ── Section 4 (Fallback inline inspector when viewport height < 25 cannot fit separate Bento card) ──
    if include_inspector {
        let items = state.filtered_items();
        if let Some(sel) = items.get(state.selected_idx().min(items.len().saturating_sub(1))) {
            let meta = sel.action.inspector_meta();
            lines.push(telemetry_section_header(
                "实时检视",
                None,
                usable_w,
                palette,
            ));
            let mut r1 = vec![Span::raw(" ")];
            push_kv_cell(
                &mut r1,
                "目标",
                key_w,
                sel.label,
                Style::default()
                    .fg(palette.text_primary)
                    .add_modifier(Modifier::BOLD),
                usable_w,
                false,
                palette,
            );
            lines.push(Line::from(r1));

            let mut r2 = vec![Span::raw(" ")];
            push_kv_cell(
                &mut r2,
                "单元",
                key_w,
                meta.unit,
                muted_style,
                usable_w,
                false,
                palette,
            );
            lines.push(Line::from(r2));

            let mut r3 = vec![Span::raw(" ")];
            push_kv_cell(
                &mut r3, "命令", key_w, meta.cli, ver_style, usable_w, false, palette,
            );
            lines.push(Line::from(r3));
        }
    }

    lines
}

fn render_status_panel(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &AppState,
    palette: &UiPalette,
    include_inspector: bool,
) {
    let is_focused = state.active_pane() == ActivePane::Telemetry;
    let inner_w = area.width.saturating_sub(2) as usize;
    let viewport_h = area.height.saturating_sub(2) as usize;

    let mut lines = build_telemetry_lines(state, inner_w, include_inspector, palette);
    let needs_scrollbar = lines.len() > viewport_h && area.height >= 4;
    if needs_scrollbar {
        let content_w = inner_w.saturating_sub(1).max(1);
        lines = build_telemetry_lines(state, content_w, include_inspector, palette);
    }

    let total_lines = lines.len();
    let max_scroll = total_lines.saturating_sub(viewport_h) as u16;
    let clamped_scroll = state.telemetry_scroll().min(max_scroll);

    let border_color = if is_focused {
        active_border_color(state, palette)
    } else {
        palette.border_inactive
    };

    let title_str = if is_focused {
        "系统遥测 · ↑↓滚动"
    } else {
        "系统遥测"
    };

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(palette.bg_surface))
        .title(title_badge(title_str, is_focused, palette, area.width));

    if needs_scrollbar && area.width >= 18 {
        let end_row = (clamped_scroll as usize + viewport_h).min(total_lines);
        block = block.title_bottom(
            Line::from(Span::styled(
                format!(" ↕ {end_row}/{total_lines} "),
                Style::default().fg(palette.text_muted),
            ))
            .alignment(Alignment::Right),
        );
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .scroll((clamped_scroll, 0));
    frame.render_widget(paragraph, area);

    if needs_scrollbar {
        let mut sb_state = ScrollbarState::new(total_lines)
            .position(clamped_scroll as usize)
            .viewport_content_length(viewport_h);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("┃")
            .track_style(Style::default().fg(palette.border_inactive))
            .thumb_style(Style::default().fg(if is_focused {
                palette.accent_primary
            } else {
                palette.text_muted
            }));
        frame.render_stateful_widget(
            scrollbar,
            area.inner(Margin {
                vertical: 1,
                horizontal: 1,
            }),
            &mut sb_state,
        );
    }
}

/// Dedicated bottom-right Bento card displaying live structured KV specs for the selected action (U17/U18).
fn render_inspector_panel(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &AppState,
    palette: &UiPalette,
) {
    let inner_w = area.width.saturating_sub(2) as usize;
    let usable_w = inner_w.saturating_sub(2).max(1);
    let key_w = if usable_w >= 50 { 6 } else { 5 };
    let two_col = usable_w >= 38;
    let left_w = (usable_w.saturating_sub(3)) / 2;
    let right_w = usable_w.saturating_sub(3 + left_w);

    let sep_style = Style::default().fg(palette.border_inactive);
    let muted_style = Style::default().fg(palette.text_muted);
    let primary_style = Style::default().fg(palette.text_primary);
    let ver_style = Style::default().fg(palette.accent_primary);

    let items = state.filtered_items();
    let selected = items.get(state.selected_idx().min(items.len().saturating_sub(1)));

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(6);
    if let Some(sel) = selected {
        let meta = sel.action.inspector_meta();
        let key_shortcut = format!("[{}] · Enter", sel.id);

        if two_col {
            // Row 1: Target | Status Capsule
            let mut r1 = vec![Span::raw(" ")];
            push_kv_cell(
                &mut r1,
                "目标",
                key_w,
                sel.label,
                Style::default()
                    .fg(palette.text_primary)
                    .add_modifier(Modifier::BOLD),
                left_w,
                true,
                palette,
            );
            r1.push(Span::styled(" │ ", sep_style));
            r1.push(Span::styled(pad_right_to_width("状态", key_w), muted_style));
            match sel.status {
                Some(StatusBadge::Active(lbl)) => {
                    let (ds, ts) = capsule_style(true, false, palette);
                    r1.push(Span::styled(" ● ", ds));
                    r1.push(Span::styled(format!("{lbl} "), ts));
                }
                Some(StatusBadge::Inactive(lbl)) => {
                    let (ds, ts) = capsule_style(false, lbl == "未执行", palette);
                    r1.push(Span::styled(" ○ ", ds));
                    r1.push(Span::styled(format!("{lbl} "), ts));
                }
                None => {
                    r1.push(Span::styled(meta.kind, muted_style));
                }
            }
            lines.push(Line::from(r1));

            // Row 2: Unit/Service | Ports/Mode
            let mut r2 = vec![Span::raw(" ")];
            push_kv_cell(
                &mut r2,
                "单元",
                key_w,
                meta.unit,
                primary_style,
                left_w,
                true,
                palette,
            );
            r2.push(Span::styled(" │ ", sep_style));
            push_kv_cell(
                &mut r2,
                "端口",
                key_w,
                meta.ports,
                muted_style,
                right_w,
                false,
                palette,
            );
            lines.push(Line::from(r2));

            // Row 3: Config Path (full width)
            let mut r3 = vec![Span::raw(" ")];
            push_kv_cell(
                &mut r3,
                "路径",
                key_w,
                meta.path,
                muted_style,
                usable_w,
                false,
                palette,
            );
            lines.push(Line::from(r3));

            // Row 4: Equivalent CLI Command (full width)
            let mut r4 = vec![Span::raw(" ")];
            push_kv_cell(
                &mut r4, "命令", key_w, meta.cli, ver_style, usable_w, false, palette,
            );
            lines.push(Line::from(r4));

            // Row 5: Shortcut | Action Kind
            let mut r5 = vec![Span::raw(" ")];
            push_kv_cell(
                &mut r5,
                "键位",
                key_w,
                &key_shortcut,
                ver_style,
                left_w,
                true,
                palette,
            );
            r5.push(Span::styled(" │ ", sep_style));
            push_kv_cell(
                &mut r5,
                "类型",
                key_w,
                meta.kind,
                muted_style,
                right_w,
                false,
                palette,
            );
            lines.push(Line::from(r5));
        } else {
            let mut r1 = vec![Span::raw(" ")];
            push_kv_cell(
                &mut r1,
                "目标",
                key_w,
                sel.label,
                primary_style,
                usable_w,
                false,
                palette,
            );
            lines.push(Line::from(r1));
            let mut r2 = vec![Span::raw(" ")];
            push_kv_cell(
                &mut r2,
                "单元",
                key_w,
                meta.unit,
                muted_style,
                usable_w,
                false,
                palette,
            );
            lines.push(Line::from(r2));
            let mut r3 = vec![Span::raw(" ")];
            push_kv_cell(
                &mut r3, "命令", key_w, meta.cli, ver_style, usable_w, false, palette,
            );
            lines.push(Line::from(r3));
        }
    } else {
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled("无选中项", muted_style),
        ]));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.border_inactive))
        .style(Style::default().bg(palette.bg_surface))
        .title(title_badge("实时检视", false, palette, area.width));

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

/// Renders color-coded keycap chips that fit strictly inside `max_width` with zero mid-word clipping.
fn build_responsive_keycaps(
    chips: &[(&str, &str)],
    compact_chips: &[(&str, &str)],
    max_width: usize,
    palette: &UiPalette,
) -> Line<'static> {
    let key_style = Style::default()
        .fg(palette.accent_primary)
        .add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(palette.text_muted);

    let measure_set = |set: &[(&str, &str)]| -> usize {
        let mut w = 0usize;
        for (i, &(k, l)) in set.iter().enumerate() {
            if i > 0 {
                w += 2;
            }
            w += visual_width(k);
            if !l.is_empty() {
                w += 1 + visual_width(l);
            }
        }
        w
    };

    let render_set = |set: &[(&str, &str)]| -> Line<'static> {
        let mut spans = Vec::with_capacity(set.len() * 3);
        for (i, &(k, l)) in set.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(k.to_string(), key_style));
            if !l.is_empty() {
                spans.push(Span::styled(format!(" {l}"), label_style));
            }
        }
        Line::from(spans)
    };

    if measure_set(chips) <= max_width {
        return render_set(chips);
    }
    if measure_set(compact_chips) <= max_width {
        return render_set(compact_chips);
    }

    // Greedily pack complete chips + overflow cue `…` so no CJK word is ever severed mid-word.
    let mut spans = Vec::new();
    let mut used = 0usize;
    let mut truncated = false;
    for &(k, l) in compact_chips {
        let sep = if spans.is_empty() { 0 } else { 2 };
        let chip_w = visual_width(k) + if l.is_empty() { 0 } else { 1 + visual_width(l) };
        let reserve = 2; // space for ` …`
        if used + sep + chip_w <= max_width.saturating_sub(reserve) {
            if sep > 0 {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(k.to_string(), key_style));
            if !l.is_empty() {
                spans.push(Span::styled(format!(" {l}"), label_style));
            }
            used += sep + chip_w;
        } else {
            truncated = true;
            break;
        }
    }
    if spans.is_empty() {
        let minimal = truncate_to_width("[↑↓] [Enter] [Esc] [q]", max_width);
        return Line::from(Span::styled(minimal, key_style));
    }
    if truncated && used + 2 <= max_width {
        spans.push(Span::styled(" …", label_style));
    }
    Line::from(spans)
}

fn render_footer(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &AppState,
    palette: &UiPalette,
    split_mode: bool,
    show_inline_notice: bool,
) {
    let inner_w = area.width.saturating_sub(2) as usize;

    let (line, border_color) = if state.palette_active() {
        let full = &[
            ("[↑↓]", "选择"),
            ("[Enter]", "执行"),
            ("[Ctrl+U]", "清空"),
            ("[Esc]", "关闭"),
        ];
        let compact = &[("[↑↓]", "选择"), ("[Enter]", "执行"), ("[Esc]", "关闭")];
        (
            build_responsive_keycaps(full, compact, inner_w, palette),
            palette.accent_primary,
        )
    } else if let Some(confirm) = state.pending_confirm() {
        let color = if confirm.is_destructive() {
            palette.danger
        } else {
            palette.warning
        };
        let full = "[y] 确认  [n/Esc/Enter] 取消";
        let compact = "[y] 确认  [n/Esc] 取消";
        let txt = if visual_width(full) <= inner_w {
            full.to_string()
        } else {
            truncate_to_width(compact, inner_w)
        };
        (
            Line::from(Span::styled(
                txt,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )),
            color,
        )
    } else if state.filter_active() {
        let prefix = "/";
        let prefix_w = visual_width(prefix);
        let full_chips: &[(&str, &str)] =
            &[("[↑↓]", "选择"), ("[Enter]", "执行"), ("[Esc]", "清除")];
        let compact_chips: &[(&str, &str)] = &[("[Enter]", "执行"), ("[Esc]", "清除")];
        let minimal_chips: &[(&str, &str)] = &[("[Esc]", "清除")];
        let chips_width = |chips: &[(&str, &str)]| -> usize {
            if chips.is_empty() {
                return 0;
            }
            chips
                .iter()
                .map(|&(k, l)| 2 + visual_width(k) + 1 + visual_width(l))
                .sum()
        };
        let min_query_cols = visual_width(state.filter_query()).clamp(4, 8);
        let chosen_chips: &[(&str, &str)] =
            if prefix_w + min_query_cols + 1 + chips_width(full_chips) <= inner_w {
                full_chips
            } else if prefix_w + min_query_cols + 1 + chips_width(compact_chips) <= inner_w {
                compact_chips
            } else if prefix_w + min_query_cols + 1 + chips_width(minimal_chips) <= inner_w {
                minimal_chips
            } else {
                &[]
            };
        let suffix_w = chips_width(chosen_chips);
        let q_budget = inner_w.saturating_sub(prefix_w + suffix_w + 1).max(1);
        let fitted_q = tail_truncate_to_width(state.filter_query(), q_budget);
        let key_style = Style::default()
            .fg(palette.accent_primary)
            .add_modifier(Modifier::BOLD);
        let label_style = Style::default().fg(palette.text_muted);

        let mut spans = vec![Span::styled(format!("{prefix}{fitted_q}▏"), key_style)];
        for &(k, l) in chosen_chips {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(k.to_string(), key_style));
            spans.push(Span::styled(format!(" {l}"), label_style));
        }
        (Line::from(spans), palette.accent_primary)
    } else if let Some(prefix) = state.digit_prefix() {
        let full = format!("编号: {prefix}_  [Enter] 执行  [0-9] 续输");
        let compact = format!("编号: {prefix}_  [Enter] 执行");
        let txt = if visual_width(&full) <= inner_w {
            full
        } else {
            truncate_to_width(&compact, inner_w)
        };
        (
            Line::from(Span::styled(
                txt,
                Style::default()
                    .fg(palette.accent_primary)
                    .add_modifier(Modifier::BOLD),
            )),
            palette.accent_primary,
        )
    } else if show_inline_notice && let Some(notice) = state.notice() {
        let color = notice_color(state.notice_kind(), palette);
        let fitted = truncate_to_width(notice, inner_w);
        (
            Line::from(Span::styled(
                fitted,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )),
            color,
        )
    } else if split_mode && state.active_pane() == ActivePane::Telemetry {
        let full = &[
            ("[↑↓/PgUp/PgDn]", "滚动"),
            ("[Tab/Esc]", "菜单"),
            ("[R]", "刷新"),
            ("[q]", "退出"),
        ];
        let compact = &[
            ("[↑↓]", "滚动"),
            ("[Tab]", "菜单"),
            ("[R]", "刷新"),
            ("[q]", "退出"),
        ];
        (
            build_responsive_keycaps(full, compact, inner_w, palette),
            palette.border_inactive,
        )
    } else if split_mode {
        let is_main = state.current_menu() == MenuId::Main;
        let full: &[(&str, &str)] = if is_main {
            &[
                ("[↑↓]", "选择"),
                ("[←→]", "工作区"),
                ("[Enter]", "执行"),
                ("[Ctrl+K]", "命令"),
                ("[/]", "筛选"),
                ("[Tab]", "面板"),
                ("[R]", "刷新"),
                ("[Esc/q]", "退出"),
            ]
        } else {
            &[
                ("[↑↓]", "选择"),
                ("[Enter]", "执行"),
                ("[Ctrl+K]", "命令"),
                ("[/]", "筛选"),
                ("[Tab]", "面板"),
                ("[R]", "刷新"),
                ("[Esc]", "返回"),
                ("[q]", "退出"),
            ]
        };
        let compact: &[(&str, &str)] = if is_main {
            &[
                ("[↑↓]", "选择"),
                ("[Enter]", "执行"),
                ("[/]", "筛选"),
                ("[Tab]", "面板"),
                ("[R]", "刷新"),
                ("[Esc/q]", "退出"),
            ]
        } else {
            &[
                ("[↑↓]", "选择"),
                ("[Enter]", "执行"),
                ("[/]", "筛选"),
                ("[Tab]", "面板"),
                ("[R]", "刷新"),
                ("[Esc]", "返回"),
                ("[q]", "退出"),
            ]
        };
        (
            build_responsive_keycaps(full, compact, inner_w, palette),
            palette.border_inactive,
        )
    } else {
        // Single-pane mode (`width < 90`): omit `[Tab] 面板` cleanly so zero dead hints or mid-word clipping!
        let is_main = state.current_menu() == MenuId::Main;
        let full: &[(&str, &str)] = if is_main {
            &[
                ("[↑↓]", "选择"),
                ("[0-9]", "直达"),
                ("[Enter]", "执行"),
                ("[/]", "筛选"),
                ("[R]", "刷新"),
                ("[Esc/q]", "退出"),
            ]
        } else {
            &[
                ("[↑↓]", "选择"),
                ("[0-9]", "直达"),
                ("[Enter]", "执行"),
                ("[/]", "筛选"),
                ("[R]", "刷新"),
                ("[Esc]", "返回"),
                ("[q]", "退出"),
            ]
        };
        let compact: &[(&str, &str)] = if is_main {
            &[
                ("[↑↓]", "选择"),
                ("[Enter]", "执行"),
                ("[/]", "筛选"),
                ("[R]", "刷新"),
                ("[Esc/q]", "退出"),
            ]
        } else {
            &[
                ("[↑↓]", "选择"),
                ("[Enter]", "执行"),
                ("[/]", "筛选"),
                ("[R]", "刷新"),
                ("[Esc]", "返回"),
                ("[q]", "退出"),
            ]
        };
        (
            build_responsive_keycaps(full, compact, inner_w, palette),
            palette.border_inactive,
        )
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(palette.bg_surface));
    let paragraph = Paragraph::new(line)
        .alignment(Alignment::Center)
        .block(block);

    frame.render_widget(paragraph, area);
}

fn notice_color(kind: NoticeKind, palette: &UiPalette) -> Color {
    match kind {
        NoticeKind::Success => palette.success,
        NoticeKind::Warning => palette.warning,
        NoticeKind::Danger => palette.danger,
        NoticeKind::Info => palette.accent_primary,
    }
}

fn notice_icon(kind: NoticeKind) -> &'static str {
    match kind {
        NoticeKind::Success => "✓",
        NoticeKind::Warning => "▲",
        NoticeKind::Danger => "✖",
        NoticeKind::Info => "◆",
    }
}

/// Sanitizes split wide-character (CJK / emoji) cells straddling an overlay's left or right edge
/// BEFORE `Clear` overwrites the overlay rectangle, preventing border corruption (`菜──` clobbering `╭`).
fn sanitize_overlay_boundaries(buf: &mut Buffer, overlay: Rect, bounds: Rect) {
    for y in overlay.top()..overlay.bottom() {
        if overlay.left() > bounds.left() {
            let left_prev = overlay.left() - 1;
            if visual_width(buf[(left_prev, y)].symbol()) > 1 {
                buf[(left_prev, y)].set_symbol(" ");
            }
        }
        if overlay.right() < bounds.right() && overlay.right() > bounds.left() {
            let right_last = overlay.right() - 1;
            if visual_width(buf[(right_last, y)].symbol()) > 1 {
                buf[(overlay.right(), y)].set_symbol(" ");
            }
        }
    }
}

/// Renders a non-destructive floating toast banner strictly inside `anchor_area`'s interior
/// so it never collides with panel borders, scrollbars, or the split-pane seam.
fn render_toast(
    frame: &mut ratatui::Frame<'_>,
    anchor_area: Rect,
    full_area: Rect,
    message: &str,
    kind: NoticeKind,
    palette: &UiPalette,
) {
    let inner = anchor_area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    if inner.width < 20 || inner.height < 3 {
        return;
    }
    let color = notice_color(kind, palette);
    let icon = notice_icon(kind);
    let tagged = format!("{icon} {message}");

    // Leave 1-col breathing margin on both sides inside `inner` when space permits (also clears right scrollbar).
    let max_toast_w = inner
        .width
        .saturating_sub(2)
        .max(20)
        .min(inner.width)
        .min(60) as usize;
    let min_toast_w = 20usize.min(max_toast_w);
    let desired_w = (visual_width(&tagged) + 4).clamp(min_toast_w, max_toast_w);
    let toast_w = desired_w as u16;
    let text_w = toast_w.saturating_sub(4).max(1) as usize;
    let mut wrapped = wrap_text_to_width(&tagged, text_w);
    let max_lines = inner.height.saturating_sub(2).max(1) as usize;
    if wrapped.len() > max_lines {
        wrapped.truncate(max_lines);
        if let Some(last) = wrapped.last_mut() {
            let trunc = truncate_to_width(last, text_w.saturating_sub(1));
            *last = format!("{trunc}…");
        }
    }
    let toast_h = (wrapped.len() as u16) + 2;

    let toast_x = inner
        .right()
        .saturating_sub(toast_w)
        .saturating_sub(1)
        .max(inner.left());
    let toast_y = inner.bottom().saturating_sub(toast_h).max(inner.top());
    let toast_area = Rect::new(toast_x, toast_y, toast_w, toast_h);

    sanitize_overlay_boundaries(frame.buffer_mut(), toast_area, full_area);
    frame.render_widget(Clear, toast_area);

    let bg = if palette.monochrome {
        Color::Reset
    } else {
        palette.bg_toast
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .style(Style::default().bg(bg));

    let text_style = Style::default()
        .fg(color)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let lines: Vec<Line<'static>> = wrapped
        .into_iter()
        .map(|line| Line::from(Span::styled(format!(" {line} "), text_style)))
        .collect();

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, toast_area);
}

fn apply_dimmed_backdrop(frame: &mut ratatui::Frame<'_>, area: Rect, palette: &UiPalette) {
    let buf = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            if !palette.monochrome {
                cell.set_fg(palette.dim_backdrop_fg);
                if cell.bg != Color::Reset && cell.bg != palette.bg_base {
                    cell.set_bg(palette.bg_surface);
                }
            }
            cell.set_style(cell.style().add_modifier(Modifier::DIM));
        }
    }
}

/// 8-tick `tachyonfx`-style horizontal wipe with a 2-column bright shimmer edge, CJK-safe.
fn apply_wipe_transition(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    ticks_left: u8,
    palette: &UiPalette,
) {
    let total = u16::from(TRANSITION_TOTAL_TICKS);
    let progress = total.saturating_sub(u16::from(ticks_left.min(TRANSITION_TOTAL_TICKS))) as usize;
    let reveal_x = area.left() + ((area.width as usize * progress) / (total as usize)) as u16;
    let shimmer_start = reveal_x.saturating_sub(2).max(area.left());
    let buf = frame.buffer_mut();

    for y in area.top()..area.bottom() {
        // Avoid styling only the trailing half of a wide CJK glyph at `reveal_x`.
        let mut effective_reveal = reveal_x;
        if effective_reveal > area.left()
            && effective_reveal < area.right()
            && visual_width(buf[(effective_reveal - 1, y)].symbol()) > 1
        {
            effective_reveal = effective_reveal.saturating_add(1).min(area.right());
        }

        // Bright 2-column leading shimmer band.
        for x in shimmer_start..effective_reveal.min(area.right()) {
            let cell = &mut buf[(x, y)];
            cell.set_fg(palette.shimmer_fg);
            cell.set_style(cell.style().add_modifier(Modifier::BOLD));
        }

        // Unrevealed trailing region dimmed.
        for x in effective_reveal..area.right() {
            let cell = &mut buf[(x, y)];
            cell.set_fg(palette.dim_backdrop_fg);
            cell.set_style(cell.style().add_modifier(Modifier::DIM));
        }
    }
}

fn render_confirm_modal(
    frame: &mut ratatui::Frame<'_>,
    body_area: Rect,
    full_area: Rect,
    message: &str,
    destructive: bool,
    palette: &UiPalette,
) {
    if full_area.width < 6 || full_area.height < 5 {
        return;
    }

    let modal_color = if destructive {
        palette.danger
    } else {
        palette.warning
    };

    let bound_to_body = body_area.height >= 5;
    let container = if bound_to_body { body_area } else { full_area };

    let max_w = full_area.width.saturating_sub(2).max(6) as usize;
    let modal_width = ((full_area.width as usize) * 72 / 100)
        .clamp(36, 72)
        .min(max_w) as u16;
    // 1-col border + 1-col inner horizontal padding on both sides => modal_width - 4.
    let inner_text_w = modal_width.saturating_sub(4).max(1) as usize;

    let msg_wrapped = wrap_text_to_width(message, inner_text_w);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let compact_v = full_area.height <= 14 || container.height < 9;
    if !compact_v {
        lines.push(Line::from(""));
    }
    for row in &msg_wrapped {
        lines.push(Line::from(Span::styled(
            row.clone(),
            Style::default()
                .fg(modal_color)
                .add_modifier(Modifier::BOLD),
        )));
    }
    if !compact_v {
        let rule_w = inner_text_w.saturating_sub(4).clamp(6, 32);
        lines.push(Line::from(Span::styled(
            "─".repeat(rule_w),
            Style::default().fg(palette.border_inactive),
        )));
    }
    if inner_text_w >= 28 {
        lines.push(Line::from(vec![
            Span::styled(
                "[y]",
                Style::default()
                    .fg(modal_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" 确认   ", Style::default().fg(palette.text_primary)),
            Span::styled(
                "[n/Esc/Enter]",
                Style::default()
                    .fg(palette.text_muted)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" 取消", Style::default().fg(palette.text_muted)),
        ]));
    } else if inner_text_w >= 20 {
        lines.push(Line::from(vec![
            Span::styled(
                "[y]",
                Style::default()
                    .fg(modal_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" 确认  ", Style::default().fg(palette.text_primary)),
            Span::styled(
                "[n/Esc]",
                Style::default()
                    .fg(palette.text_muted)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" 取消", Style::default().fg(palette.text_muted)),
        ]));
    } else {
        let short = truncate_to_width("[y] 确认 / [n] 取消", inner_text_w);
        lines.push(Line::from(Span::styled(
            short,
            Style::default().fg(palette.text_muted),
        )));
    }

    let needed_h = (lines.len() as u16).saturating_add(2);
    let max_h = if bound_to_body {
        container.height
    } else {
        container
            .height
            .saturating_sub(2)
            .max(3)
            .min(container.height)
    };
    let modal_height = needed_h.min(max_h);
    let modal_x = full_area.x + full_area.width.saturating_sub(modal_width) / 2;
    let modal_y = container.y + container.height.saturating_sub(modal_height) / 2;
    let modal_area = Rect::new(modal_x, modal_y, modal_width, modal_height);

    sanitize_overlay_boundaries(frame.buffer_mut(), modal_area, full_area);
    frame.render_widget(Clear, modal_area);

    let title_text = if destructive {
        "▲ 确认操作"
    } else {
        "◆ 确认操作"
    };
    let max_title_w = modal_width.saturating_sub(8).max(4) as usize;
    let fitted_title = truncate_to_width(title_text, max_title_w);
    let modal_bg = if palette.monochrome {
        Color::Reset
    } else {
        palette.bg_toast
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(modal_color))
        .style(Style::default().bg(modal_bg))
        .title(Line::from(vec![
            Span::styled("┤ ", Style::default().fg(modal_color)),
            Span::styled(
                fitted_title,
                Style::default()
                    .fg(modal_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ├", Style::default().fg(modal_color)),
        ]))
        .title_alignment(Alignment::Center);

    let paragraph = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .block(block);

    frame.render_widget(paragraph, modal_area);
}

/// Renders the floating Global Fuzzy Command Palette (`Ctrl+K`) over the dimmed Bento canvas (U17/U18).
fn render_palette_modal(
    frame: &mut ratatui::Frame<'_>,
    body_area: Rect,
    full_area: Rect,
    state: &AppState,
    palette: &UiPalette,
) {
    if full_area.width < 20 || body_area.height < 6 {
        return;
    }

    let modal_w = ((full_area.width as usize) * 68 / 100)
        .clamp(46, 82)
        .min(full_area.width.saturating_sub(4) as usize) as u16;
    let max_h = body_area.height.saturating_sub(2).max(6);
    let modal_h = max_h.min(18);
    let modal_x = full_area.x + full_area.width.saturating_sub(modal_w) / 2;
    let modal_y = body_area.y + body_area.height.saturating_sub(modal_h) / 2;
    let modal_area = Rect::new(modal_x, modal_y, modal_w, modal_h);

    sanitize_overlay_boundaries(frame.buffer_mut(), modal_area, full_area);
    frame.render_widget(Clear, modal_area);

    let modal_bg = if palette.monochrome {
        Color::Reset
    } else {
        palette.bg_toast
    };
    let input_bg = if palette.monochrome {
        Color::Reset
    } else {
        palette.bg_highlight
    };

    let entries = state.filtered_palette_entries();
    let total_entries = crate::menu::build_palette_entries(&state.status).len();
    let count_badge = format!("{}/{}", entries.len(), total_entries);

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.border_active))
        .style(Style::default().bg(modal_bg))
        .title(title_badge("◆ 命令面板 · Ctrl+K", true, palette, modal_w));
    if modal_w >= 40 {
        block =
            block.title(title_badge(&count_badge, false, palette, 16).alignment(Alignment::Right));
    }

    let inner_w = modal_w.saturating_sub(2) as usize;
    let inner_h = modal_h.saturating_sub(2) as usize;
    let usable_w = inner_w.saturating_sub(2).max(1);

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(inner_h);

    // Row 0: Elevated search input strip
    let q_prefix = " ▸ ";
    let q_budget = inner_w.saturating_sub(visual_width(q_prefix) + 2);
    let fitted_q = tail_truncate_to_width(state.palette_query(), q_budget);
    let q_display = if fitted_q.is_empty() {
        "▏输入模块、命令或关键词...".to_string()
    } else {
        format!("{fitted_q}▏")
    };
    let q_fitted = pad_right_to_width(&truncate_to_width(&q_display, inner_w - 4), inner_w - 4);
    let q_style = if fitted_q.is_empty() {
        Style::default().fg(palette.text_inactive).bg(input_bg)
    } else {
        Style::default()
            .fg(palette.text_primary)
            .bg(input_bg)
            .add_modifier(Modifier::BOLD)
    };
    lines.push(Line::from(vec![
        Span::styled(
            q_prefix,
            Style::default()
                .fg(palette.accent_primary)
                .bg(input_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(q_fitted, q_style),
        Span::styled(" ", Style::default().bg(input_bg)),
    ]));

    // Row 1: Subtle hairline separator
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            "─".repeat(usable_w),
            Style::default().fg(palette.border_inactive),
        ),
    ]));

    // Reserve 2 rows at bottom for hairline + live CLI hint when height permits
    let show_cli_bar = inner_h >= 6;
    let list_h = inner_h.saturating_sub(if show_cli_bar { 4 } else { 2 });
    let sel_idx = state
        .palette_selected()
        .min(entries.len().saturating_sub(1));
    let scroll_start = if list_h > 0 && sel_idx >= list_h {
        sel_idx + 1 - list_h
    } else {
        0
    };

    if entries.is_empty() {
        let msg = truncate_to_width(" 无匹配命令  [Esc] 关闭", usable_w);
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(msg, Style::default().fg(palette.warning)),
        ]));
    } else {
        for (rel_i, entry) in entries.iter().skip(scroll_start).take(list_h).enumerate() {
            let abs_i = scroll_start + rel_i;
            let is_sel = abs_i == sel_idx;
            let row_bg = if is_sel && !palette.monochrome {
                palette.bg_highlight
            } else {
                modal_bg
            };
            let pointer = if is_sel { "▎" } else { " " };
            let ptr_style = Style::default()
                .fg(palette.accent_primary)
                .bg(row_bg)
                .add_modifier(Modifier::BOLD);
            let dom_bg = if palette.monochrome {
                row_bg
            } else if is_sel {
                palette.bg_surface
            } else {
                palette.capsule_off_bg
            };
            let dom_pill = format!(" {} ", entry.domain); // 6 cols
            let dom_style = Style::default()
                .fg(palette.accent_secondary)
                .bg(dom_bg)
                .add_modifier(Modifier::BOLD);

            let label_col_w = 16usize;
            let fitted_label =
                pad_right_to_width(&truncate_to_width(entry.label, label_col_w), label_col_w);
            let label_style = if is_sel {
                Style::default()
                    .fg(palette.accent_primary)
                    .bg(row_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette.text_primary).bg(row_bg)
            };

            let status_txt = match entry.status {
                Some(StatusBadge::Active(lbl)) => Some((true, false, lbl)),
                Some(StatusBadge::Inactive(lbl)) => Some((false, lbl == "未执行", lbl)),
                None => None,
            };
            let right_w = if status_txt.is_some() {
                10usize
            } else {
                2usize
            };
            let used_left = 1 + 1 + 6 + 2 + label_col_w + 1;
            let cli_budget = inner_w.saturating_sub(used_left + right_w + 1);
            let cli_str = if cli_budget >= 8 {
                pad_right_to_width(
                    &truncate_to_width(entry.action.cli_hint(), cli_budget),
                    cli_budget,
                )
            } else {
                " ".repeat(cli_budget)
            };
            let cli_style = Style::default().fg(palette.text_muted).bg(row_bg);
            let sp = Style::default().bg(row_bg);

            let mut row_spans = vec![
                Span::styled(pointer, ptr_style),
                Span::styled(" ", sp),
                Span::styled(dom_pill, dom_style),
                Span::styled("  ", sp),
                Span::styled(fitted_label, label_style),
                Span::styled(" ", sp),
                Span::styled(cli_str, cli_style),
            ];
            if let Some((ok, warn_off, lbl)) = status_txt {
                let (ds, ts) = capsule_style(ok, warn_off, palette);
                let dot = if ok { "●" } else { "○" };
                row_spans.push(Span::styled(format!(" {dot} "), ds));
                row_spans.push(Span::styled(format!("{lbl} "), ts));
                row_spans.push(Span::styled(" ", sp));
            } else {
                row_spans.push(Span::styled(
                    " › ",
                    Style::default().fg(palette.text_inactive).bg(row_bg),
                ));
            }
            lines.push(Line::from(row_spans));
        }
    }

    if show_cli_bar {
        while lines.len() + 2 < inner_h {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "─".repeat(usable_w),
                Style::default().fg(palette.border_inactive),
            ),
        ]));
        if let Some(sel) = entries.get(sel_idx) {
            let meta = sel.action.inspector_meta();
            let detail = format!("{}  │  {}", meta.unit, meta.path);
            let fitted = truncate_to_width(&detail, usable_w);
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(fitted, Style::default().fg(palette.text_muted)),
            ]));
        }
    }

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, modal_area);
}

/// Converts a `TestBackend` terminal buffer into a deterministic plain CJK-width-aligned string.
fn test_buffer_to_string(terminal: &Terminal<TestBackend>) -> String {
    let buf = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut x = 0u16;
        while x < buf.area.width {
            let sym = buf[(x, y)].symbol();
            out.push_str(sym);
            let step = visual_width(sym).max(1) as u16;
            x = x.saturating_add(step);
        }
        out.push('\n');
    }
    out
}

fn push_sgr_color(codes: &mut Vec<String>, color: Color, is_bg: bool) {
    let base = if is_bg { 40 } else { 30 };
    let bright = if is_bg { 100 } else { 90 };
    match color {
        Color::Reset => codes.push(if is_bg { "49" } else { "39" }.to_string()),
        Color::Black => codes.push(format!("{base}")),
        Color::Red => codes.push(format!("{}", base + 1)),
        Color::Green => codes.push(format!("{}", base + 2)),
        Color::Yellow => codes.push(format!("{}", base + 3)),
        Color::Blue => codes.push(format!("{}", base + 4)),
        Color::Magenta => codes.push(format!("{}", base + 5)),
        Color::Cyan => codes.push(format!("{}", base + 6)),
        Color::Gray => codes.push(format!("{}", base + 7)),
        Color::DarkGray => codes.push(format!("{bright}")),
        Color::LightRed => codes.push(format!("{}", bright + 1)),
        Color::LightGreen => codes.push(format!("{}", bright + 2)),
        Color::LightYellow => codes.push(format!("{}", bright + 3)),
        Color::LightBlue => codes.push(format!("{}", bright + 4)),
        Color::LightMagenta => codes.push(format!("{}", bright + 5)),
        Color::LightCyan => codes.push(format!("{}", bright + 6)),
        Color::White => codes.push(format!("{}", bright + 7)),
        Color::Indexed(idx) => {
            codes.push(if is_bg {
                format!("48;5;{idx}")
            } else {
                format!("38;5;{idx}")
            });
        }
        Color::Rgb(r, g, b) => {
            codes.push(if is_bg {
                format!("48;2;{r};{g};{b}")
            } else {
                format!("38;2;{r};{g};{b}")
            });
        }
    }
}

/// Converts a `TestBackend` terminal buffer into compact 24-bit TrueColor ANSI SGR text.
pub fn test_buffer_to_ansi(terminal: &Terminal<TestBackend>) -> String {
    let buf = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut prev_style: Option<(Color, Color, Modifier)> = None;
        let mut x = 0u16;
        while x < buf.area.width {
            let cell = &buf[(x, y)];
            let cur = (cell.fg, cell.bg, cell.modifier);
            if prev_style != Some(cur) {
                let mut codes = vec!["0".to_string()];
                if cell.modifier.contains(Modifier::BOLD) {
                    codes.push("1".to_string());
                }
                if cell.modifier.contains(Modifier::DIM) {
                    codes.push("2".to_string());
                }
                if cell.modifier.contains(Modifier::ITALIC) {
                    codes.push("3".to_string());
                }
                if cell.modifier.contains(Modifier::UNDERLINED) {
                    codes.push("4".to_string());
                }
                if cell.modifier.contains(Modifier::REVERSED) {
                    codes.push("7".to_string());
                }
                if cell.fg != Color::Reset {
                    push_sgr_color(&mut codes, cell.fg, false);
                }
                if cell.bg != Color::Reset {
                    push_sgr_color(&mut codes, cell.bg, true);
                }
                out.push_str("\x1b[");
                out.push_str(&codes.join(";"));
                out.push('m');
                prev_style = Some(cur);
            }
            let sym = cell.symbol();
            out.push_str(sym);
            let step = visual_width(sym).max(1) as u16;
            x = x.saturating_add(step);
        }
        out.push_str("\x1b[0m\n");
    }
    out
}

/// Deterministic rich sample `StatusSnapshot` for `debopty ui-preview --demo` and U4/U14 review.
pub fn sample_showcase_status() -> StatusSnapshot {
    let mut tools = BTreeMap::new();
    for &tool in TRACKED_TOOLS {
        tools.insert(tool.to_string(), false);
    }
    for enabled in [
        "fish",
        "micro",
        "yazi",
        "tmux",
        "podman",
        "xray",
        "ferron",
        "tailscale",
    ] {
        if tools.contains_key(enabled) {
            tools.insert(enabled.to_string(), true);
        }
    }

    StatusSnapshot {
        version_display: "2.0.0 (pure-rust)".to_string(),
        is_cn_region: true,
        base_optimized: true,
        ip_forward: true,
        podman_gc_enabled: true,
        tools,
    }
}

/// Rich rendering options for `debopty ui-preview` (TrueColor ANSI, tick, transition, filter, pane, toast, palette, tab).
#[derive(Debug, Clone, Default)]
pub struct ShowcaseOptions {
    pub ansi: bool,
    pub tick: u64,
    pub transition: u8,
    pub filter: Option<String>,
    pub palette: Option<String>,
    pub tab: WorkspaceTab,
    pub pane: ActivePane,
    pub toast: Option<String>,
    pub show_confirm: bool,
    pub animations: bool,
}

/// Renders a single deterministic showcase frame with full U13/U14/U17/U18 options.
pub fn render_showcase_frame_with_options(
    menu: MenuId,
    status: &StatusSnapshot,
    width: u16,
    height: u16,
    opts: &ShowcaseOptions,
) -> String {
    let backend = TestBackend::new(width, height);
    let Ok(mut terminal) = Terminal::new(backend) else {
        return String::new();
    };
    let mut state = AppState::new(status.clone());
    state.navigate_to(menu);
    if menu == MenuId::Main && opts.tab != WorkspaceTab::Overview {
        state.set_workspace_tab(opts.tab);
    }
    let enable_anim = opts.animations || opts.tick > 0 || opts.transition > 0;
    state.set_animations_enabled(enable_anim);
    state.set_tick_count(opts.tick);
    state.set_transition_ticks(opts.transition);
    state.set_split_pane_available(width >= SPLIT_PANE_MIN_WIDTH);
    state.set_active_pane(opts.pane);

    if let Some(ref q) = opts.filter {
        state.open_filter_preview(q.clone());
    }
    if let Some(ref pq) = opts.palette {
        state.open_palette_preview(pq.clone());
    }
    if let Some(ref toast_msg) = opts.toast {
        state.set_notice(toast_msg.clone());
    }
    if opts.show_confirm {
        let (msg, action) = match menu {
            MenuId::Podman => (
                "移除 Podman 环境？ [y/N]".to_string(),
                BashAction::UninstallApp(AppId::Podman),
            ),
            MenuId::Maintenance => (
                "卸载 debopty 及安装目录？ [y/N]".to_string(),
                BashAction::ScriptUninstall,
            ),
            _ => (
                "确认执行该操作？ [y/N]".to_string(),
                BashAction::UninstallApp(AppId::Podman),
            ),
        };
        state.open_confirm_preview(msg, action);
    }

    let palette = UiPalette::truecolor();
    let _ = terminal.draw(|frame| render_frame_with_palette(frame, &state, &palette));
    if opts.ansi {
        test_buffer_to_ansi(&terminal)
    } else {
        test_buffer_to_string(&terminal)
    }
}

/// Renders a single deterministic non-TTY showcase frame via `TestBackend` (plain text).
pub fn render_showcase_frame(
    menu: MenuId,
    status: &StatusSnapshot,
    width: u16,
    height: u16,
    show_confirm: bool,
) -> String {
    render_showcase_frame_with_options(
        menu,
        status,
        width,
        height,
        &ShowcaseOptions {
            show_confirm,
            ..ShowcaseOptions::default()
        },
    )
}

/// Renders the canonical 5-screen showcase gallery with optional TrueColor ANSI & motion flags.
pub fn render_showcase_gallery_with_options(
    status: &StatusSnapshot,
    width: u16,
    height: u16,
    base_opts: &ShowcaseOptions,
) -> Vec<(String, String)> {
    let wide_w = width.max(SPLIT_PANE_MIN_WIDTH);
    let narrow_w = width.clamp(40, 64);
    let narrow_h = height.clamp(16, 24);
    let mut confirm_opts = base_opts.clone();
    confirm_opts.show_confirm = true;

    vec![
        (
            "main-wide".to_string(),
            render_showcase_frame_with_options(MenuId::Main, status, wide_w, height, base_opts),
        ),
        (
            "main-narrow".to_string(),
            render_showcase_frame_with_options(MenuId::Main, status, narrow_w, narrow_h, base_opts),
        ),
        (
            "devops".to_string(),
            render_showcase_frame_with_options(MenuId::Devops, status, wide_w, height, base_opts),
        ),
        (
            "podman-confirm".to_string(),
            render_showcase_frame_with_options(
                MenuId::Podman,
                status,
                wide_w,
                height,
                &confirm_opts,
            ),
        ),
        (
            "maintenance".to_string(),
            render_showcase_frame_with_options(
                MenuId::Maintenance,
                status,
                wide_w,
                height,
                base_opts,
            ),
        ),
    ]
}

/// Renders the canonical 5-screen U4 showcase gallery (`main-wide`, `main-narrow`, `devops`,
/// `podman-confirm`, `maintenance`).
pub fn render_showcase_gallery(
    status: &StatusSnapshot,
    width: u16,
    height: u16,
) -> Vec<(String, String)> {
    render_showcase_gallery_with_options(status, width, height, &ShowcaseOptions::default())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use debopty_core::status::{StatusSnapshot, TRACKED_TOOLS};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::state::DEFAULT_TOAST_TTL_TICKS;

    fn sample_status() -> StatusSnapshot {
        let mut tools = BTreeMap::new();
        for &tool in TRACKED_TOOLS {
            tools.insert(tool.to_string(), false);
        }
        tools.insert("fish".to_string(), true);
        tools.insert("podman".to_string(), true);
        tools.insert("xray".to_string(), true);

        StatusSnapshot {
            version_display: "1.0.7 (231e05eb)".to_string(),
            is_cn_region: true,
            base_optimized: true,
            ip_forward: true,
            podman_gc_enabled: false,
            tools,
        }
    }

    fn key_event(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn wide_main_dashboard_renders_split_pane_cjk_and_telemetry() {
        let backend = TestBackend::new(110, 32);
        let mut terminal = Terminal::new(backend).expect("test backend");
        let state = AppState::new(sample_status());

        terminal
            .draw(|frame| render_frame(frame, &state))
            .expect("draw ok");

        let rendered = test_buffer_to_string(&terminal);
        assert!(rendered.contains("debopty"));
        assert!(rendered.contains("Pure Rust (21/21)"));
        assert!(rendered.contains("CN · 镜像"));
        assert!(rendered.contains("基础优化"));
        assert!(rendered.contains("系统遥测"));
        assert!(rendered.contains("运行态"));
        assert!(rendered.contains("覆盖率"));
        assert!(rendered.contains("工具矩阵"));
        assert!(rendered.contains("实时检视"));
        assert!(rendered.contains("3/18"));
        assert!(rendered.contains("已就绪"));
        assert!(rendered.contains("未部署"));
        assert!(rendered.contains('╭')); // Rounded borders
        assert!(rendered.contains("┤ ")); // Title badge
        // All 18 tools fit in grouped matrix without clipping!
        assert!(rendered.contains("yazi"));
        assert!(rendered.contains("usque"));
        assert!(rendered.contains("easytier-core"));
    }

    #[test]
    fn narrow_main_dashboard_uses_single_column_without_telemetry_panel_or_tab_hint() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test backend");
        let state = AppState::new(sample_status());

        terminal
            .draw(|frame| render_frame(frame, &state))
            .expect("draw ok");

        let rendered = test_buffer_to_string(&terminal);
        assert!(rendered.contains("debopty"));
        assert!(rendered.contains("功能菜单"));
        assert!(rendered.contains("基础优化"));
        assert!(rendered.contains("已优化")); // FE-M4: base_optimized badge visible in single-pane!
        assert!(!rendered.contains("系统遥测"));
        assert!(!rendered.contains("[Tab]")); // FE-H2: no Tab hint in single-pane
        assert!(!rendered.contains("[Esc] 返│")); // FE-H2: never clipped mid-word
    }

    #[test]
    fn confirm_modal_preserves_borders_and_cjk_boundaries_at_60x18_and_100x24() {
        for (w, h) in [(60u16, 18u16), (100, 24), (80, 24), (45, 16)] {
            let rendered =
                render_showcase_frame(MenuId::Podman, &sample_showcase_status(), w, h, true);
            assert!(
                rendered.contains("确认操作"),
                "missing modal title at {w}x{h}"
            );
            for (row_idx, line) in rendered.lines().enumerate() {
                assert_eq!(
                    visual_width(line),
                    w as usize,
                    "row {row_idx} width mismatch at {w}x{h}: {line:?}"
                );
                assert!(
                    !line.contains("菜──"),
                    "split CJK clobbered modal border at {w}x{h}: {line:?}"
                );
            }
        }
    }

    #[test]
    fn podman_confirm_modal_renders_centered_overlay() {
        let backend = TestBackend::new(100, 28);
        let mut terminal = Terminal::new(backend).expect("test backend");
        let mut state = AppState::new(sample_status());
        // Navigate to Podman (4) -> Uninstall (2) to trigger confirmation modal.
        assert!(state.handle_event(MenuEvent::Digit(4)).is_none());
        assert!(state.handle_event(MenuEvent::Digit(2)).is_none());
        assert!(state.pending_confirm().is_some());

        terminal
            .draw(|frame| render_frame(frame, &state))
            .expect("draw ok");

        let rendered = test_buffer_to_string(&terminal);
        assert!(rendered.contains("确认操作"));
        assert!(rendered.contains("移除 Podman 环境？ [y/N]"));
        assert!(rendered.contains("[y] 确认"));
        assert!(!rendered.contains("按 y 确认执行"));
    }

    #[test]
    fn digit_prefix_and_notice_banners_render_and_toast_auto_dismisses() {
        let backend = TestBackend::new(96, 26);
        let mut terminal = Terminal::new(backend).expect("test backend");
        let mut state = AppState::new(sample_status());
        state.set_animations_enabled(true);
        state.set_transition_ticks(0);

        // Trigger digit prefix (`1` can be 1, 10, 11, 12).
        assert!(state.handle_event(MenuEvent::Digit(1)).is_none());
        assert_eq!(state.digit_prefix(), Some(1));
        terminal
            .draw(|frame| render_frame(frame, &state))
            .expect("draw prefix ok");
        let prefix_rendered = test_buffer_to_string(&terminal);
        assert!(prefix_rendered.contains("编号: 1_"));

        // Clear prefix via Refresh to display floating toast banner while preserving footer!
        assert_eq!(
            state.handle_event(MenuEvent::Refresh),
            Some(crate::state::TuiOutcome::Refresh)
        );
        assert!(state.notice().is_some());
        terminal
            .draw(|frame| render_frame(frame, &state))
            .expect("draw notice ok");
        let notice_rendered = test_buffer_to_string(&terminal);
        assert!(notice_rendered.contains("状态已刷新"));
        assert!(notice_rendered.contains("[Enter]")); // Footer preserved!

        // Auto-dismiss after DEFAULT_TOAST_TTL_TICKS ticks
        for _ in 0..DEFAULT_TOAST_TTL_TICKS {
            state.handle_event(MenuEvent::Tick);
        }
        assert!(state.notice().is_none());
    }

    #[test]
    fn inline_fuzzy_filter_and_pane_toggle_and_telemetry_scroll_work_smoothly() {
        let backend = TestBackend::new(108, 28);
        let mut terminal = Terminal::new(backend).expect("test backend");
        let mut state = AppState::new(sample_status());

        assert_eq!(state.active_pane(), ActivePane::Menu);
        state.handle_event(MenuEvent::TogglePane);
        assert_eq!(state.active_pane(), ActivePane::Telemetry);
        // Up/Down scroll telemetry without moving Menu selection
        assert_eq!(state.selected_idx(), 0);
        state.handle_event(MenuEvent::Down);
        assert_eq!(state.telemetry_scroll(), 1);
        assert_eq!(state.selected_idx(), 0);
        state.handle_event(MenuEvent::Up);
        assert_eq!(state.telemetry_scroll(), 0);
        state.handle_event(MenuEvent::TogglePane);
        assert_eq!(state.active_pane(), ActivePane::Menu);

        state.handle_event(MenuEvent::FilterStart);
        assert!(state.filter_active());
        for ch in "podman".chars() {
            state.handle_event(MenuEvent::FilterChar(ch));
        }
        assert_eq!(state.filtered_items().len(), 1);
        terminal
            .draw(|frame| render_frame(frame, &state))
            .expect("draw filtered ok");
        let rendered = test_buffer_to_string(&terminal);
        assert!(rendered.contains("Podman"));
        assert!(rendered.contains("/podman▏"));
        assert!(rendered.contains("(1/13)"));
        assert!(!rendered.contains("执行首项")); // FE-H7 fixed!

        // Ctrl+W deletes word, Ctrl+U clears line
        state.handle_event(MenuEvent::FilterChar(' '));
        state.handle_event(MenuEvent::FilterChar('x'));
        state.handle_event(MenuEvent::FilterDeleteWord);
        assert_eq!(state.filter_query(), "podman ");
        state.handle_event(MenuEvent::FilterClearLine);
        assert!(state.filter_query().is_empty());
        assert!(state.filter_active());

        state.handle_event(MenuEvent::FilterClear);
        assert!(!state.filter_active());
        assert!(state.filter_query().is_empty());
    }

    #[test]
    fn palette_detection_covers_truecolor_256_16_and_monochrome_wcag() {
        assert_eq!(
            UiPalette::detect_from(false, Some("xterm-256color"), Some("truecolor")),
            UiPalette::truecolor()
        );
        assert_eq!(
            UiPalette::detect_from(false, Some("xterm-256color"), None),
            UiPalette::ansi256()
        );
        assert_eq!(
            UiPalette::detect_from(false, Some("xterm"), None),
            UiPalette::ansi16()
        );
        // FE-H4: ansi16 uses Color::Reset for text_primary (safe on light terminals!)
        assert_eq!(UiPalette::ansi16().text_primary, Color::Reset);
        assert!(UiPalette::detect_from(true, Some("xterm-256color"), Some("truecolor")).monochrome);
        assert!(UiPalette::detect_from(false, Some("dumb"), Some("truecolor")).monochrome);

        assert!(should_enable_animations(
            false,
            Some("xterm-256color"),
            None
        ));
        assert!(!should_enable_animations(
            true,
            Some("xterm-256color"),
            None
        ));
        assert!(!should_enable_animations(false, Some("dumb"), None));
        assert!(!should_enable_animations(
            false,
            Some("xterm-256color"),
            Some("true")
        ));
    }

    #[test]
    fn ansi_showcase_emits_truecolor_sgr_and_cosine_pulse() {
        let status = sample_showcase_status();
        let ansi = render_showcase_frame_with_options(
            MenuId::Main,
            &status,
            120,
            36,
            &ShowcaseOptions {
                ansi: true,
                tick: 15,
                transition: 4,
                toast: Some("已刷新状态显示。".to_string()),
                ..ShowcaseOptions::default()
            },
        );
        assert!(ansi.contains("\x1b["), "expected ANSI SGR sequences");
        assert!(ansi.contains("38;2;"), "expected 24-bit TrueColor fg SGR");
        assert!(ansi.contains("48;2;"), "expected 24-bit TrueColor bg SGR");
        assert!(ansi.contains("已刷新状态显示。"));
    }

    #[test]
    fn showcase_gallery_renders_all_five_deterministic_frames() {
        let status = sample_showcase_status();
        let gallery = render_showcase_gallery(&status, 108, 30);
        assert_eq!(gallery.len(), 5);
        let names: Vec<&str> = gallery.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "main-wide",
                "main-narrow",
                "devops",
                "podman-confirm",
                "maintenance"
            ]
        );
        assert!(gallery[0].1.contains("Pure Rust (21/21)"));
        assert!(!gallery[1].1.contains("系统遥测"));
        assert!(gallery[2].1.contains("Fish"));
        assert!(gallery[3].1.contains("确认操作"));
        assert!(gallery[4].1.contains("维护管理"));
    }

    #[test]
    fn ratatui_render_frame_never_panics_on_extreme_sizes() {
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
        let extreme_sizes = [
            (0u16, 0u16),
            (1, 1),
            (2, 2),
            (5, 3),
            (10, 5),
            (20, 6),
            (36, 12),
            (45, 16),
            (60, 18),
            (80, 24),
            (89, 24),
            (90, 24),
            (120, 36),
            (200, 60),
        ];

        for &(w, h) in &extreme_sizes {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test backend");
            for menu in all_menus {
                let mut state = AppState::new(status.clone());
                state.navigate_to(menu);
                state.set_animations_enabled(true);
                state.set_transition_ticks(4);
                state.set_notice("测试通知 Toast 横幅");
                terminal
                    .draw(|frame| render_frame(frame, &state))
                    .unwrap_or_else(|e| panic!("draw failed for {menu:?} at {w}x{h}: {e}"));
            }

            let mut modal_state = AppState::new(status.clone());
            modal_state.handle_event(MenuEvent::Digit(4));
            modal_state.handle_event(MenuEvent::Digit(2));
            assert!(modal_state.pending_confirm().is_some());
            terminal
                .draw(|frame| render_frame(frame, &modal_state))
                .unwrap_or_else(|e| panic!("modal draw failed at {w}x{h}: {e}"));
        }
    }

    #[test]
    fn map_crossterm_key_supports_standard_keys_ignores_release_and_rejects_vim_keys() {
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Up,
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Up)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Down,
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Down)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::PageUp,
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::PageUp)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::PageDown,
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::PageDown)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Home,
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Home)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::End,
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::End)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('q'),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Quit)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('Q'),
                KeyModifiers::SHIFT,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Quit)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Enter,
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Select)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Esc,
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Back)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Backspace,
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Back)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Tab,
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::TogglePane)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('/'),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::FilterStart)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Quit)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('0'),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Digit(0))
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('9'),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Digit(9))
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('r'),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Refresh)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('R'),
                KeyModifiers::SHIFT,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Refresh)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('y'),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::ConfirmYes)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('Y'),
                KeyModifiers::SHIFT,
                KeyEventKind::Press
            )),
            Some(MenuEvent::ConfirmYes)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('n'),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::ConfirmNo)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('N'),
                KeyModifiers::SHIFT,
                KeyEventKind::Press
            )),
            Some(MenuEvent::ConfirmNo)
        );

        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Enter,
                KeyModifiers::NONE,
                KeyEventKind::Release
            )),
            None
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Up,
                KeyModifiers::NONE,
                KeyEventKind::Release
            )),
            None
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Up,
                KeyModifiers::NONE,
                KeyEventKind::Repeat
            )),
            Some(MenuEvent::Up)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Down,
                KeyModifiers::NONE,
                KeyEventKind::Repeat
            )),
            Some(MenuEvent::Down)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Enter,
                KeyModifiers::NONE,
                KeyEventKind::Repeat
            )),
            None
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('y'),
                KeyModifiers::NONE,
                KeyEventKind::Repeat
            )),
            None
        );

        // Filter Ctrl+U and Ctrl+W bindings
        assert_eq!(
            map_crossterm_key_filtered(key_event(
                KeyCode::Char('u'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press
            )),
            Some(MenuEvent::FilterClearLine)
        );
        assert_eq!(
            map_crossterm_key_filtered(key_event(
                KeyCode::Char('w'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press
            )),
            Some(MenuEvent::FilterDeleteWord)
        );

        for modifier in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
            for code in [
                KeyCode::Char('y'),
                KeyCode::Char('Y'),
                KeyCode::Char('n'),
                KeyCode::Char('r'),
                KeyCode::Char('1'),
                KeyCode::Enter,
                KeyCode::Backspace,
            ] {
                assert_eq!(
                    map_crossterm_key(key_event(code, modifier, KeyEventKind::Press)),
                    None,
                    "modifier {modifier:?} + {code:?} must be rejected"
                );
            }
        }

        // Vim-only navigation keys MUST NOT be bound in normal mode.
        for vim_key in ['h', 'j', 'k', 'l', 'H', 'J', 'K', 'L'] {
            assert_eq!(
                map_crossterm_key(key_event(
                    KeyCode::Char(vim_key),
                    KeyModifiers::NONE,
                    KeyEventKind::Press
                )),
                None
            );
        }
    }

    #[test]
    fn r2_confirm_modal_footer_keeps_full_danger_contrast_and_narrow_bounds() {
        let palette = UiPalette::truecolor();
        let status = sample_showcase_status();

        // R2-FE-P1-1: confirm footer MUST NOT be dimmed by backdrop
        let mut terminal = Terminal::new(TestBackend::new(60, 18)).expect("test backend");
        let mut state = AppState::new(status.clone());
        state.navigate_to(MenuId::Podman);
        state.open_confirm_preview(
            "确定要移除 Podman 环境吗？[y/N]",
            BashAction::UninstallApp(AppId::Podman),
        );
        terminal
            .draw(|frame| render_frame_with_palette(frame, &state, &palette))
            .expect("draw ok");
        let buf = terminal.backend().buffer();
        // Footer occupies rows 15..18; row 16 has `[y] 确认执行  [n/Esc/Enter] 取消`.
        let mut found_bright_y = false;
        for x in 0..60 {
            let cell = &buf[(x, 16)];
            if cell.symbol() == "y" {
                assert_eq!(
                    cell.fg, palette.danger,
                    "confirm footer fg must be bright danger"
                );
                assert!(
                    cell.modifier.contains(Modifier::BOLD),
                    "confirm footer must be bold"
                );
                assert!(
                    !cell.modifier.contains(Modifier::DIM),
                    "confirm footer must NOT be dimmed"
                );
                found_bright_y = true;
                break;
            }
        }
        assert!(found_bright_y, "confirm footer `[y]` cell not found");

        // R2-FE-P2-2: 45x16 never splits `[y/N]` or `Enter` across lines
        let rendered_45 = render_showcase_frame(MenuId::Podman, &status, 45, 16, true);
        assert!(
            rendered_45.contains("[y/N]"),
            "45x16 split [y/N]:\n{rendered_45}"
        );
        assert!(
            rendered_45.contains("Enter"),
            "45x16 split Enter:\n{rendered_45}"
        );

        // R2-FE-P2-2: 36x12 never overlaps header bottom border (row 2) or footer top border (row 9)
        let rendered_36 = render_showcase_frame(MenuId::Podman, &status, 36, 12, true);
        let rows_36: Vec<&str> = rendered_36.lines().collect();
        assert_eq!(rows_36.len(), 12);
        assert!(
            rows_36[2].starts_with('╰') && rows_36[2].ends_with('╯') && !rows_36[2].contains('╭'),
            "36x12 modal overlapped header bottom border: {:?}",
            rows_36[2]
        );
        assert!(
            rows_36[9].starts_with('╭') && rows_36[9].ends_with('╮') && !rows_36[9].contains('╰'),
            "36x12 modal overlapped footer top border: {:?}",
            rows_36[9]
        );
    }

    #[test]
    fn r2_toast_fits_inside_pane_interior_without_border_or_seam_collision() {
        let status = sample_showcase_status();
        let opts = ShowcaseOptions {
            toast: Some("收敛完成 · 修复 3 项 · 保留 5 项自定义".to_string()),
            ..ShowcaseOptions::default()
        };

        for (w, h) in [(120u16, 36u16), (90, 24), (60, 18), (45, 16)] {
            let rendered = render_showcase_frame_with_options(MenuId::Main, &status, w, h, &opts);
            assert!(rendered.contains('✓'), "missing toast icon at {w}x{h}");
            let rows: Vec<&str> = rendered.lines().collect();
            let body_bottom = rows[(h - 4) as usize];
            assert!(
                !body_bottom.contains("╰─") || !body_bottom.contains("╯╯"),
                "toast collided with body bottom border at {w}x{h}: {body_bottom:?}"
            );
            assert!(
                !body_bottom.contains("──╰"),
                "toast overwrote body bottom border at {w}x{h}: {body_bottom:?}"
            );
        }

        // At 120x36 split mode (46%/54%), panes[0] = 0..55 and panes[1] = 55..120; col 55 must remain unbroken border.
        let mut terminal = Terminal::new(TestBackend::new(120, 36)).expect("test backend");
        let mut state = AppState::new(status);
        state.set_notice("收敛完成 · 修复 3 项 · 保留 5 项自定义");
        terminal
            .draw(|frame| render_frame_with_palette(frame, &state, &UiPalette::truecolor()))
            .expect("draw ok");
        let buf = terminal.backend().buffer();
        // Right side has 2 stacked Bento cards: Telemetry (y=4..23) + Inspector (y=23..33).
        for y in 24..32 {
            assert_eq!(
                buf[(55, y)].symbol(),
                "│",
                "toast severed Inspector left border at y={y}"
            );
        }
        assert_eq!(buf[(55, 32)].symbol(), "╰");
        assert_eq!(buf[(119, 32)].symbol(), "╯");
    }

    #[test]
    fn r2_main_footer_copy_filter_keycaps_tail_scroll_and_telemetry_polish() {
        let palette = UiPalette::truecolor();
        let status = sample_showcase_status();

        // R2-FE-P2-3: Main shows `[Esc/q] 退出` (no `[Esc] 返回`); submenu shows `[Esc] 返回` + `[q] 退出`
        let main_120 = render_showcase_frame(MenuId::Main, &status, 120, 36, false);
        assert!(main_120.contains("[Esc/q] 退出"));
        assert!(!main_120.contains("[Esc] 返回"));
        let main_80 = render_showcase_frame(MenuId::Main, &status, 80, 24, false);
        assert!(main_80.contains("[Esc/q] 退出"));
        assert!(!main_80.contains("[Esc] 返回"));
        let devops_120 = render_showcase_frame(MenuId::Devops, &status, 120, 36, false);
        assert!(devops_120.contains("[Esc] 返回"));
        assert!(devops_120.contains("[q] 退出"));

        // R2-FE-P2-4: Filter footer styles keycaps in cyan bold & tail-scrolls narrow queries
        let mut term_f = Terminal::new(TestBackend::new(120, 36)).expect("test backend");
        let mut st_f = AppState::new(status.clone());
        st_f.open_filter_preview("podman");
        term_f
            .draw(|frame| render_frame_with_palette(frame, &st_f, &palette))
            .expect("draw ok");
        let buf_f = term_f.backend().buffer();
        let mut found_cyan_esc = false;
        for x in 0..115 {
            if buf_f[(x, 34)].symbol() == "["
                && buf_f[(x + 1, 34)].symbol() == "E"
                && buf_f[(x + 2, 34)].symbol() == "s"
            {
                assert_eq!(buf_f[(x, 34)].fg, palette.accent_primary);
                assert!(buf_f[(x, 34)].modifier.contains(Modifier::BOLD));
                found_cyan_esc = true;
                break;
            }
        }
        assert!(
            found_cyan_esc,
            "filter footer `[Esc]` must be cyan bold keycap"
        );

        let narrow_filter = render_showcase_frame_with_options(
            MenuId::Main,
            &status,
            45,
            16,
            &ShowcaseOptions {
                filter: Some("中文超长筛选词测试1234567890".to_string()),
                ..ShowcaseOptions::default()
            },
        );
        assert!(
            narrow_filter.contains("1234567890▏"),
            "narrow filter must tail-scroll to keep latest chars visible:\n{narrow_filter}"
        );
        assert!(
            narrow_filter.contains("无匹配项:") && narrow_filter.contains("…"),
            "truncated empty-match row must end with ellipsis:\n{narrow_filter}"
        );

        // R2-FE-P2-5: Telemetry divider never collides with scrollbar at 90x20
        let tele_90 = render_showcase_frame_with_options(
            MenuId::Main,
            &status,
            90,
            20,
            &ShowcaseOptions {
                pane: ActivePane::Telemetry,
                ..ShowcaseOptions::default()
            },
        );
        for line in tele_90.lines() {
            assert!(
                !line.contains("─││") && !line.contains("─┃│"),
                "telemetry divider touched scrollbar: {line:?}"
            );
        }

        // R2-FE-P2-5: Unfocused Menu selection is dimmed/muted when Telemetry pane is focused
        let mut term_t = Terminal::new(TestBackend::new(120, 36)).expect("test backend");
        let mut st_t = AppState::new(status);
        st_t.set_active_pane(ActivePane::Telemetry);
        term_t
            .draw(|frame| render_frame_with_palette(frame, &st_t, &palette))
            .expect("draw ok");
        let buf_t = term_t.backend().buffer();
        // Header occupies y=0..4; Menu top border at y=4; `── 系统 ──` at y=5; first menu item `▎` at (1, 6).
        assert_eq!(buf_t[(1, 6)].symbol(), "▎");
        assert_eq!(buf_t[(1, 6)].fg, palette.border_inactive);
        assert_eq!(buf_t[(1, 6)].bg, palette.bg_surface);
    }

    #[test]
    fn u16_tabular_alignment_density_and_zero_slop_typography() {
        let palette = UiPalette::truecolor();
        let status = sample_showcase_status();
        let mut terminal = Terminal::new(TestBackend::new(120, 36)).expect("test backend");
        let state = AppState::new(status);
        terminal
            .draw(|frame| render_frame_with_palette(frame, &state, &palette))
            .expect("draw ok");

        let rendered = test_buffer_to_string(&terminal);
        // Zero legacy verbose AI-slop labels
        for forbidden in [
            "一键系统级基础优化",
            "路由转发模式控制",
            "Podman 容器环境 (Rootless)",
            "中国大陆 (镜像加速)",
            "脚本维护 (更新/卸载)",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "unexpected verbose AI-slop phrase {forbidden:?}"
            );
        }

        // High-density Telemetry + 3-zone Bento Inspector + Workspace Pill Tabs present
        for expected in [
            "概览",
            "系统安全",
            "网络代理",
            "运维容器",
            "稳态维护",
            "运行态",
            "覆盖率",
            "工具矩阵",
            "实时检视",
            "21/21 Native",
            "debopty system base",
        ] {
            assert!(
                rendered.contains(expected),
                "missing U16/U17/U18 UI element {expected:?}"
            );
        }

        // Strict tabular alignment in Menu: every item row's `│` sits at exact col 6,
        // and every capsule badge dot (`●`/`○`) sits at exact col 44 (55 - 1 border - 1 trail - 10 capsule + 1 lead space).
        let buf = terminal.backend().buffer();
        let mut checked_items = 0usize;
        let mut checked_badges = 0usize;
        for y in 5..32 {
            if buf[(6, y)].symbol() == "│" {
                checked_items += 1;
                let dot = buf[(44, y)].symbol();
                if dot == "●" || dot == "○" {
                    checked_badges += 1;
                }
            }
        }
        assert_eq!(
            checked_items, 13,
            "all 13 Main menu rows must align `│` at col 6"
        );
        assert_eq!(
            checked_badges, 11,
            "all 11 badged rows must align capsule status dot at col 44"
        );

        // Strict tabular alignment in Telemetry `运行态` 4 KV rows (y=6..10): center `│` aligns at exact same x!
        let mut kv_mid_cols = Vec::new();
        for y in 6..10 {
            for x in 56..119 {
                if buf[(x, y)].symbol() == "│" {
                    kv_mid_cols.push(x);
                    break;
                }
            }
        }
        assert_eq!(kv_mid_cols.len(), 4, "expected 4 aligned 2-col KV rows");
        assert!(
            kv_mid_cols.iter().all(|&c| c == kv_mid_cols[0]),
            "Telemetry KV center gutter misaligned: {kv_mid_cols:?}"
        );
    }

    #[test]
    fn u17_u18_workspace_tabs_palette_and_layered_surfaces() {
        let palette = UiPalette::truecolor();
        let status = sample_showcase_status();

        // Keymap: Left/Right and [/] switch tabs; Ctrl+K opens palette; h/j/k/l remain unbound!
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Right,
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::NextTab)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Left,
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::PrevTab)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char(']'),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::NextTab)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('['),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::PrevTab)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('k'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press
            )),
            Some(MenuEvent::PaletteOpen)
        );

        // Workspace tab navigation state machine
        let mut state = AppState::new(status.clone());
        assert_eq!(state.workspace_tab(), WorkspaceTab::Overview);
        state.handle_event(MenuEvent::NextTab);
        assert_eq!(state.workspace_tab(), WorkspaceTab::SysSec);
        state.handle_event(MenuEvent::NextTab);
        assert_eq!(state.workspace_tab(), WorkspaceTab::NetProxy);
        state.handle_event(MenuEvent::PrevTab);
        assert_eq!(state.workspace_tab(), WorkspaceTab::SysSec);

        // Global Fuzzy Command Palette: Ctrl+K -> type "proxy" -> matches Xray/WARP/Usque/Ferron -> Enter executes
        state.handle_event(MenuEvent::PaletteOpen);
        assert!(state.palette_active());
        for ch in "proxy".chars() {
            state.handle_event(MenuEvent::FilterChar(ch));
        }
        let matches = state.filtered_palette_entries();
        assert!(
            matches.len() >= 3,
            "expected >=3 proxy palette matches, got {}",
            matches.len()
        );
        assert!(matches.iter().any(|e| e.label == "Xray Core"));

        // Render floating Command Palette modal and verify exact width + elevated #1E2436 surface
        let mut term = Terminal::new(TestBackend::new(120, 36)).expect("test backend");
        term.draw(|frame| render_frame_with_palette(frame, &state, &palette))
            .expect("draw palette ok");
        let rendered = test_buffer_to_string(&term);
        assert!(rendered.contains("命令面板 · Ctrl+K"));
        assert!(rendered.contains("Xray Core"));
        for (idx, line) in rendered.lines().enumerate() {
            assert_eq!(
                visual_width(line),
                120,
                "palette frame row {idx} width mismatch: {line:?}"
            );
        }

        // Verify layered surface elevation colors (#0B0E14, #161A26, #1E2436, #242E47)
        assert_eq!(palette.bg_base, Color::Rgb(11, 14, 20));
        assert_eq!(palette.bg_surface, Color::Rgb(22, 26, 38));
        assert_eq!(palette.bg_toast, Color::Rgb(30, 36, 54));
        assert_eq!(palette.bg_highlight, Color::Rgb(36, 46, 71));
        assert_eq!(palette.border_inactive, Color::Rgb(42, 50, 75));
        assert_eq!(palette.border_active, Color::Rgb(56, 189, 248));

        // Esc closes palette cleanly
        state.handle_event(MenuEvent::Back);
        assert!(!state.palette_active());
        assert!(state.palette_query().is_empty());
    }
}
