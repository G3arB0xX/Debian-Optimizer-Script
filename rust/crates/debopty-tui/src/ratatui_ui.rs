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
use crate::menu::{MenuEntry, MenuId, MenuItem, StatusBadge};
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

/// Centralized WCAG-compliant color & aesthetic palette for U4/U13/U15 Ratatui design.
///
/// Supports TrueColor (TokyoNight / CyberCyan `#0f1117`/`#181825`), 256-color indexed terminals,
/// light-terminal-safe 16-color ANSI fallback, and strict monochrome under `NO_COLOR`/`TERM=dumb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiPalette {
    pub monochrome: bool,
    pub bg_base: Color,
    pub bg_surface: Color,
    pub bg_highlight: Color,
    pub bg_toast: Color,
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
    /// Deep-dark TokyoNight / CyberCyan TrueColor palette (WCAG AA/AAA verified on `#181825`).
    pub const fn truecolor() -> Self {
        Self {
            monochrome: false,
            bg_base: Color::Rgb(15, 17, 23),             // #0f1117
            bg_surface: Color::Rgb(24, 24, 37),          // #181825
            bg_highlight: Color::Rgb(30, 41, 59),        // #1e293b (full-row selection bar)
            bg_toast: Color::Rgb(30, 34, 51),            // #1e2233 (elevated toast surface)
            accent_primary: Color::Rgb(56, 189, 248),    // Electric Cyan #38bdf8 (8.19:1)
            accent_secondary: Color::Rgb(129, 140, 248), // Indigo Glow #818cf8 (5.48:1)
            border_active: Color::Rgb(56, 189, 248),
            border_inactive: Color::Rgb(71, 85, 105), // Slate #475569 (decorative border only)
            title_badge_fg: Color::Rgb(125, 211, 252), // Sky #7dd3fc (10.6:1)
            text_primary: Color::Rgb(241, 245, 249),  // #f1f5f9 (16.02:1 AAA)
            text_muted: Color::Rgb(148, 163, 184),    // #94a3b8 (6.85:1 AA)
            text_inactive: Color::Rgb(124, 139, 161), // #7c8ba1 (5.14:1 AA, never dimmed)
            success: Color::Rgb(74, 222, 128),        // Emerald #4ade80 (10.07:1 AAA)
            warning: Color::Rgb(251, 191, 36),        // Amber #fbbf24 (10.52:1 AAA)
            danger: Color::Rgb(248, 113, 113),        // Rose #f87171 (6.35:1 AA)
            modal_border: Color::Rgb(248, 113, 113),  // Destructive default rose
            dim_backdrop_fg: Color::Rgb(71, 85, 105),
            shimmer_fg: Color::Rgb(224, 242, 254), // Ice #e0f2fe shimmer edge
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
            accent_primary: Color::Indexed(81),
            accent_secondary: Color::Indexed(105),
            border_active: Color::Indexed(81),
            border_inactive: Color::Indexed(240),
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
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'C'))
    {
        return Some(MenuEvent::Quit);
    }

    let has_ctrl_or_alt = key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);

    match key.code {
        KeyCode::Up => Some(MenuEvent::Up),
        KeyCode::Down => Some(MenuEvent::Down),
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

/// Maps a `crossterm::event::KeyEvent` while inline `/` fuzzy filter mode is active.
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
        self.filter_active = state.filter_active();
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

    let vertical = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(3),
    ])
    .split(area);

    let split_mode = area.width >= SPLIT_PANE_MIN_WIDTH;

    render_header(frame, vertical[0], state, palette);

    let toast_anchor = if split_mode {
        let panes = Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(vertical[1]);
        render_menu_panel(frame, panes[0], state, palette, true);
        render_status_panel(frame, panes[1], state, palette);
        panes[1]
    } else {
        render_menu_panel(frame, vertical[1], state, palette, false);
        vertical[1]
    };

    let show_floating_toast = state.pending_confirm().is_none()
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

    if let Some(confirm) = state.pending_confirm() {
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

fn render_header(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &AppState,
    palette: &UiPalette,
) {
    let screen = state.current_screen();
    let accent = active_border_color(state, palette);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent))
        .style(Style::default().bg(palette.bg_surface))
        .title(title_badge(&screen.title, true, palette, area.width))
        .title_alignment(Alignment::Center);

    let subtitle_text = if screen.subtitle.is_empty() {
        screen.title.as_str()
    } else {
        screen.subtitle.as_str()
    };
    let inner_w = area.width.saturating_sub(2) as usize;
    let fitted = truncate_to_width(subtitle_text, inner_w);
    let paragraph = Paragraph::new(Line::from(Span::styled(
        fitted,
        Style::default().fg(palette.text_muted),
    )))
    .alignment(Alignment::Center)
    .block(block);

    frame.render_widget(paragraph, area);
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

    let raw_row_count = if filtering {
        state.filtered_items().len().max(1)
    } else {
        screen.entries.len()
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
        let matched = state.filtered_items();
        if matched.is_empty() {
            let empty_msg = format!(" 无匹配项: \"{filter_query}\" (按 Esc 清除筛选)");
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
            for (idx, item) in matched.iter().enumerate() {
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
        for (entry_idx, entry) in screen.entries.iter().enumerate() {
            match entry {
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
                        selected_row = Some(entry_idx);
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
        let q_budget = max_label_w.saturating_sub(visual_width(prefix) + 1);
        let fitted_q = tail_truncate_to_width(filter_query, q_budget);
        format!("{prefix}{fitted_q}▏")
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
    let normal = Style::default().fg(palette.text_primary).bg(row_bg);
    let spacer_style = Style::default().bg(row_bg);

    let pointer = if selected { "▎" } else { " " };
    let id_text = format!("{:>2}.", item.id);
    let icon_cell = pad_right_to_width(item.icon, 3);
    let left_body = format!(" {id_text} {icon_cell}{}", item.label);

    match item.status {
        Some(badge) => {
            let badge_plain = badge.plain_text();
            let badge_w = visual_width(&badge_plain);
            // Layout: `▎` (1) + left_body + gap (1) + badge + trailing pad (1)
            let max_body_w = inner_width.saturating_sub(badge_w + 3);
            let fitted_body = if visual_width(&left_body) > max_body_w {
                truncate_to_width(&left_body, max_body_w)
            } else {
                left_body
            };
            let padded_body = pad_right_to_width(&fitted_body, max_body_w);

            let (dot_style, label_style, dot, text) = match badge {
                StatusBadge::Active(label) => (
                    Style::default().fg(palette.success).bg(row_bg),
                    Style::default()
                        .fg(if active_selected {
                            palette.text_primary
                        } else {
                            palette.text_muted
                        })
                        .bg(row_bg),
                    "●",
                    label,
                ),
                StatusBadge::Inactive(label) => (
                    Style::default().fg(palette.text_inactive).bg(row_bg),
                    Style::default()
                        .fg(if active_selected {
                            palette.text_muted
                        } else {
                            palette.text_inactive
                        })
                        .bg(row_bg),
                    "○",
                    label,
                ),
            };

            let used = 1 + visual_width(&padded_body) + 1 + badge_w;
            let trail = " ".repeat(inner_width.saturating_sub(used));

            Line::from(vec![
                Span::styled(pointer, if selected { bar_style } else { normal }),
                Span::styled(padded_body, if selected { highlight } else { normal }),
                Span::styled(" ", spacer_style),
                Span::styled(dot, dot_style),
                Span::styled(" ", spacer_style),
                Span::styled(text, label_style),
                Span::styled(trail, spacer_style),
            ])
        }
        None => {
            let max_body_w = inner_width.saturating_sub(1);
            let fitted_body = truncate_to_width(&left_body, max_body_w);
            let padded_body = pad_right_to_width(&fitted_body, max_body_w);
            Line::from(vec![
                Span::styled(pointer, if selected { bar_style } else { normal }),
                Span::styled(padded_body, if selected { highlight } else { normal }),
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

fn render_status_panel(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &AppState,
    palette: &UiPalette,
) {
    let status = &state.status;
    let is_focused = state.active_pane() == ActivePane::Telemetry;
    let inner_w = area.width.saturating_sub(2) as usize;
    let viewport_h = area.height.saturating_sub(2) as usize;
    let total_tools = status.tools.len();
    let installed_tools = status
        .tools
        .values()
        .filter(|&&installed| installed)
        .count();
    let pct = (installed_tools * 100)
        .checked_div(total_tools)
        .unwrap_or(0);

    // Pre-calculate scrollbar visibility so divider, KV rows, and 2-col tool matrix leave a clean gap before `│`/`┃`.
    let two_col_no_sb = inner_w.saturating_sub(2) >= 32;
    let two_col_with_sb = inner_w.saturating_sub(3) >= 30;
    let est_tool_rows = if two_col_no_sb {
        total_tools.div_ceil(2)
    } else {
        total_tools
    };
    let needs_scrollbar = (8 + est_tool_rows) > viewport_h && area.height >= 4;
    let content_w = if needs_scrollbar {
        inner_w.saturating_sub(1).max(1)
    } else {
        inner_w
    };
    let use_two_col = if needs_scrollbar {
        two_col_with_sb
    } else {
        two_col_no_sb
    };

    let region_label = if status.is_cn_region {
        "中国大陆 (镜像加速)"
    } else {
        "海外地区 (直连模式)"
    };
    let base_opt_style = if status.base_optimized {
        Style::default().fg(palette.success)
    } else {
        Style::default().fg(palette.warning)
    };
    let base_opt_label = if status.base_optimized {
        "● 已优化"
    } else {
        "○ 未执行"
    };
    let ip_fwd_style = if status.ip_forward {
        Style::default().fg(palette.success)
    } else {
        Style::default().fg(palette.text_inactive)
    };
    let ip_fwd_label = if status.ip_forward {
        "● 已开启"
    } else {
        "○ 已关闭"
    };
    let podman_gc_style = if status.podman_gc_enabled {
        Style::default().fg(palette.success)
    } else {
        Style::default().fg(palette.text_inactive)
    };
    let podman_gc_label = if status.podman_gc_enabled {
        "● 已开启"
    } else {
        "○ 已关闭"
    };

    // ~10fps spinner cadence (step every 3 ticks at 30fps) to avoid 30fps blur.
    let spinner = if state.animations_enabled() {
        BRAILLE_SPINNER[((state.tick_count() / 3) as usize) % BRAILLE_SPINNER.len()]
    } else {
        "✦"
    };

    let kv_budget = content_w.saturating_sub(2);
    let make_kv = |key: &'static str, val: String, val_style: Style| -> Line<'static> {
        let key_w = visual_width(key);
        let max_val_w = kv_budget.saturating_sub(key_w + 1);
        let fitted_val = truncate_to_width(&val, max_val_w);
        Line::from(vec![
            Span::raw(" "),
            Span::styled(key, Style::default().fg(palette.text_muted)),
            Span::styled(fitted_val, val_style),
        ])
    };

    let count_pct = format!("{installed_tools}/{total_tools} ({pct}%) ");
    let spark_prefix_w = 1 + visual_width("已部署工具: ") + visual_width(&count_pct);
    let spark_slots = kv_budget.saturating_sub(spark_prefix_w).clamp(4, 12);
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
    let spark_color = if total_tools > 0 && installed_tools * 3 >= total_tools * 2 {
        palette.success
    } else if installed_tools > 0 {
        palette.accent_primary
    } else {
        palette.warning
    };

    let divider_len = content_w.saturating_sub(2).max(1);
    let mut lines = vec![
        make_kv(
            "引擎: ",
            format!("{spinner} Pure Rust (21/21)"),
            Style::default()
                .fg(palette.success)
                .add_modifier(Modifier::BOLD),
        ),
        make_kv(
            "版本: ",
            status.version_display.clone(),
            Style::default().fg(palette.accent_primary),
        ),
        make_kv(
            "网络: ",
            region_label.to_string(),
            Style::default().fg(palette.text_primary),
        ),
        make_kv("基础优化: ", base_opt_label.to_string(), base_opt_style),
        make_kv("路由转发: ", ip_fwd_label.to_string(), ip_fwd_style),
        make_kv("Podman GC: ", podman_gc_label.to_string(), podman_gc_style),
        Line::from(vec![
            Span::raw(" "),
            Span::styled("已部署工具: ", Style::default().fg(palette.text_muted)),
            Span::styled(
                count_pct,
                Style::default()
                    .fg(palette.accent_primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(sparkline, Style::default().fg(spark_color)),
        ]),
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "─".repeat(divider_len),
                Style::default().fg(palette.border_inactive),
            ),
        ]),
    ];

    // 2-column compact tool matrix (`btop`/`k9s` density) when width allows; 1-column fallback otherwise.
    let tool_entries: Vec<(&String, bool)> = status.tools.iter().map(|(k, &v)| (k, v)).collect();
    let usable_w = content_w.saturating_sub(2);
    if use_two_col {
        let col_w = (usable_w.saturating_sub(1)) / 2;
        for chunk in tool_entries.chunks(2) {
            let mut spans = vec![Span::raw(" ")];
            for (ci, &(tool, installed)) in chunk.iter().enumerate() {
                let (dot, dot_style, name_style) = if installed {
                    (
                        "●",
                        Style::default().fg(palette.success),
                        Style::default().fg(palette.text_primary),
                    )
                } else {
                    (
                        "○",
                        Style::default().fg(palette.text_inactive),
                        Style::default().fg(palette.text_inactive),
                    )
                };
                let name_budget = col_w.saturating_sub(2);
                let fitted_name = truncate_to_width(tool, name_budget);
                let padded_name = if ci == 0 {
                    pad_right_to_width(&fitted_name, name_budget)
                } else {
                    fitted_name
                };
                spans.push(Span::styled(format!("{dot} "), dot_style));
                spans.push(Span::styled(padded_name, name_style));
                if ci == 0 && chunk.len() > 1 {
                    spans.push(Span::raw(" "));
                }
            }
            lines.push(Line::from(spans));
        }
    } else {
        for (tool, installed) in tool_entries {
            let (dot, dot_style, name_style, state_label) = if installed {
                (
                    "●",
                    Style::default().fg(palette.success),
                    Style::default().fg(palette.text_primary),
                    "已就绪",
                )
            } else {
                (
                    "○",
                    Style::default().fg(palette.text_inactive),
                    Style::default().fg(palette.text_inactive),
                    "未部署",
                )
            };
            let name_w = usable_w.saturating_sub(9).max(6);
            let fitted = pad_right_to_width(&truncate_to_width(tool, name_w), name_w);
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(format!("{dot} "), dot_style),
                Span::styled(fitted, name_style),
                Span::styled(state_label, dot_style),
            ]));
        }
    }

    let total_lines = lines.len();
    let max_scroll = total_lines.saturating_sub(viewport_h) as u16;
    let clamped_scroll = state.telemetry_scroll().min(max_scroll);
    let needs_scrollbar = total_lines > viewport_h && area.height >= 4;

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

    let (line, border_color) = if let Some(confirm) = state.pending_confirm() {
        let color = if confirm.is_destructive() {
            palette.danger
        } else {
            palette.warning
        };
        let full = "[y] 确认执行  [n/Esc/Enter] 取消";
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
        let prefix = "筛选模式: /";
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
        let full =
            format!(">>> 已输入编号: {prefix}_ (按 Enter 执行 {prefix}，或继续输入第二位数字)");
        let compact = format!("已输入编号: {prefix}_ [Enter] 执行");
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
            ("[↑↓/PgUp/PgDn]", "滚动遥测"),
            ("[Tab/Esc]", "返回菜单"),
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
                ("[0-9]", "直达"),
                ("[Enter]", "执行"),
                ("[/]", "筛选"),
                ("[Tab]", "面板"),
                ("[R]", "刷新"),
                ("[Esc/q]", "退出"),
            ]
        } else {
            &[
                ("[↑↓]", "选择"),
                ("[0-9]", "直达"),
                ("[Enter]", "执行"),
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
        NoticeKind::Success => "✔",
        NoticeKind::Warning => "⚠",
        NoticeKind::Danger => "✖",
        NoticeKind::Info => "ℹ",
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
        .min(56) as usize;
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

    let msg_wrapped = wrap_text_to_width(&format!("? {message}"), inner_text_w);
    let hint_text = if inner_text_w >= 24 {
        "按 y 确认执行，按 n / Esc / Enter 取消"
    } else {
        "y 确认 / n·Esc 取消"
    };
    let hint_wrapped = wrap_text_to_width(hint_text, inner_text_w);

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
        lines.push(Line::from(""));
    }
    for row in &hint_wrapped {
        lines.push(Line::from(Span::styled(
            row.clone(),
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
        "⚠ 确认操作"
    } else {
        "确认操作"
    };
    let max_title_w = modal_width.saturating_sub(8).max(4) as usize;
    let fitted_title = truncate_to_width(title_text, max_title_w);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(modal_color))
        .style(Style::default().bg(palette.bg_surface))
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

/// Rich rendering options for `debopty ui-preview` (TrueColor ANSI, tick, transition, filter, pane, toast).
#[derive(Debug, Clone, Default)]
pub struct ShowcaseOptions {
    pub ansi: bool,
    pub tick: u64,
    pub transition: u8,
    pub filter: Option<String>,
    pub pane: ActivePane,
    pub toast: Option<String>,
    pub show_confirm: bool,
    pub animations: bool,
}

/// Renders a single deterministic showcase frame with full U13/U14 options (ANSI, tick, filter, pane, toast).
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
    let enable_anim = opts.animations || opts.tick > 0 || opts.transition > 0;
    state.set_animations_enabled(enable_anim);
    state.set_tick_count(opts.tick);
    state.set_transition_ticks(opts.transition);
    state.set_split_pane_available(width >= SPLIT_PANE_MIN_WIDTH);
    state.set_active_pane(opts.pane);

    if let Some(ref q) = opts.filter {
        state.open_filter_preview(q.clone());
    }
    if let Some(ref toast_msg) = opts.toast {
        state.set_notice(toast_msg.clone());
    }
    if opts.show_confirm {
        let (msg, action) = match menu {
            MenuId::Podman => (
                "确定要移除 Podman 环境吗？[y/N]".to_string(),
                BashAction::UninstallApp(AppId::Podman),
            ),
            MenuId::Maintenance => (
                "确定要彻底卸载 debopty 及相关资产吗？[y/N]".to_string(),
                BashAction::ScriptUninstall,
            ),
            _ => (
                "确定要执行该高危操作吗？[y/N]".to_string(),
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
        assert!(rendered.contains("中国大陆 (镜像加速)"));
        assert!(rendered.contains("一键系统级基础优化"));
        assert!(rendered.contains("系统遥测"));
        assert!(rendered.contains("已部署工具: 3/"));
        assert!(rendered.contains("已就绪"));
        assert!(rendered.contains("未部署"));
        assert!(rendered.contains('╭')); // Rounded borders
        assert!(rendered.contains("┤ ")); // Title badge
        // All 18 tools fit in 2-column matrix without clipping even at height 24!
        assert!(rendered.contains("yazi"));
        assert!(rendered.contains("usque"));
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
        assert!(rendered.contains("一键系统级基础优化"));
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
        assert!(rendered.contains("确定要移除 Podman 环境吗？[y/N]"));
        assert!(rendered.contains("按 y 确认执行"));
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
        assert!(prefix_rendered.contains("已输入编号: 1_"));

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
        assert!(notice_rendered.contains("已刷新状态显示。"));
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
        assert!(rendered.contains("筛选模式: /podman"));
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
        assert!(gallery[4].1.contains("脚本"));
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
            toast: Some("稳态收敛完成：已修复 3 项，保留 5 项用户配置".to_string()),
            ..ShowcaseOptions::default()
        };

        for (w, h) in [(120u16, 36u16), (90, 24), (60, 18), (45, 16)] {
            let rendered = render_showcase_frame_with_options(MenuId::Main, &status, w, h, &opts);
            assert!(rendered.contains('✔'), "missing toast icon at {w}x{h}");
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

        // At 120x36 split mode, panes[0] = 0..72 and panes[1] = 72..120; col 72 must remain unbroken border.
        let mut terminal = Terminal::new(TestBackend::new(120, 36)).expect("test backend");
        let mut state = AppState::new(status);
        state.set_notice("稳态收敛完成：已修复 3 项，保留 5 项用户配置");
        terminal
            .draw(|frame| render_frame_with_palette(frame, &state, &UiPalette::truecolor()))
            .expect("draw ok");
        let buf = terminal.backend().buffer();
        for y in 4..32 {
            assert_eq!(
                buf[(72, y)].symbol(),
                "│",
                "toast severed Telemetry left border at y={y}"
            );
        }
        assert_eq!(buf[(72, 32)].symbol(), "╰");
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
        // Row 4, col 1 is the first menu item pointer `▎`.
        assert_eq!(buf_t[(1, 4)].symbol(), "▎");
        assert_eq!(buf_t[(1, 4)].fg, palette.border_inactive);
        assert_eq!(buf_t[(1, 4)].bg, palette.bg_surface);
    }
}
