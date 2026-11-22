use std::collections::BTreeMap;
use std::env;
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use debopty_bridge::{AppId, BashAction};
use debopty_core::status::{StatusSnapshot, TRACKED_TOOLS};
use debopty_core::telemetry::{
    SystemSpecs, TelemetryHistory, format_braille_curve, format_bytes_rate, format_count_compact,
    format_kib_compact, format_uptime_compact, shorten_cpu_model,
};
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
use crate::motion::{
    DEFAULT_TRANSITION_DURATION, MotionPolicy, SSH_SLOW_DRAW_THRESHOLD, TransitionKind, WakeKind,
    WakeSchedule,
};
use crate::renderer::{MenuEvent, TuiRenderer};
use crate::state::{ActivePane, AppState, NoticeKind};
use crate::terminal_guard::{CrosstermOps, TerminalGuard};

/// Minimum terminal width required to display the side-by-side status telemetry panel.
const SPLIT_PANE_MIN_WIDTH: u16 = 90;
/// Legacy tick denominator for `ShowcaseOptions::transition` backward compatibility.
const LEGACY_TRANSITION_TICKS: u8 = 8;
/// Canonical 4-stack grouping covering all 18 `TRACKED_TOOLS` for Telemetry summary.
const TOOL_STACKS: &[(&str, &[&str])] = &[
    ("运维", &["fish", "micro", "yazi", "lego", "tmux"]),
    ("运行", &["podman", "rustc", "go"]),
    ("服务", &["caddy", "ferron", "realm", "derper", "xray"]),
    (
        "网络",
        &["warp-cli", "usque", "easytier-core", "tailscale", "freship"],
    ),
];

/// Flat dark slate, translucent-safe palette for U23/U24/U25/U26 Ratatui design.
///
/// Key visual guarantees:
/// - `bg_base == Color::Reset` across ALL palettes so outer canvas & inter-card gutters inherit
///   the user's opaque or translucent/blurred terminal background cleanly.
/// - TrueColor/256-color render borderless solid slate cards separated by `Color::Reset` gutters.
/// - ANSI-16 and strict `NO_COLOR`/`TERM=dumb` fall back to clean rounded borders on `Color::Reset`.
/// - Zero neon glare, zero cosine border pulses, zero shimmer flashes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiPalette {
    pub monochrome: bool,
    pub bg_base: Color,
    pub bg_surface: Color,
    pub bg_surface_focused: Color,
    pub bg_toast: Color,
    pub bg_inset: Color,
    pub bg_highlight: Color,
    pub pill_active_bg: Color,
    pub pill_active_fg: Color,
    pub pill_inactive_bg: Color,
    pub capsule_ok_bg: Color,
    pub capsule_off_bg: Color,
    pub capsule_warn_bg: Color,
    pub capsule_danger_bg: Color,
    pub accent_primary: Color,
    pub accent_secondary: Color,
    pub border_active: Color,
    pub border_inactive: Color,
    pub text_primary: Color,
    pub text_muted: Color,
    pub text_inactive: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
    pub spark_net: Color,
    pub dim_backdrop_fg: Color,
}

impl UiPalette {
    /// Restrained flat dark slate TrueColor palette with `Color::Reset` translucent outer canvas.
    pub const fn truecolor() -> Self {
        let bg_surface = Color::Rgb(21, 27, 38); // #151B26
        Self {
            monochrome: false,
            bg_base: Color::Reset,
            bg_surface,
            bg_surface_focused: Color::Rgb(25, 32, 46), // #19202E
            bg_toast: Color::Rgb(29, 37, 52),           // #1D2534
            bg_inset: Color::Rgb(16, 21, 31),           // #10151F
            bg_highlight: Color::Rgb(36, 46, 66),       // #242E42
            pill_active_bg: Color::Rgb(43, 55, 78),     // #2B374E
            pill_active_fg: Color::Rgb(241, 245, 249),  // #F1F5F9
            pill_inactive_bg: bg_surface,
            capsule_ok_bg: Color::Rgb(20, 42, 33),
            capsule_off_bg: Color::Rgb(28, 35, 49),
            capsule_warn_bg: Color::Rgb(44, 34, 19),
            capsule_danger_bg: Color::Rgb(48, 25, 30),
            accent_primary: Color::Rgb(122, 162, 247),   // #7AA2F7
            accent_secondary: Color::Rgb(154, 168, 190), // #9AA8BE
            border_active: Color::Rgb(78, 93, 120),      // #4E5D78
            border_inactive: Color::Rgb(36, 45, 62),     // #242D3E
            text_primary: Color::Rgb(230, 237, 243),     // #E6EDF3
            text_muted: Color::Rgb(139, 148, 158),       // #8B949E
            text_inactive: Color::Rgb(123, 134, 152),    // #7B8698
            success: Color::Rgb(86, 184, 129),           // #56B881
            warning: Color::Rgb(210, 153, 34),           // #D29922
            danger: Color::Rgb(224, 108, 117),           // #E06C75
            spark_net: Color::Rgb(115, 179, 200),        // #73B3C8
            dim_backdrop_fg: Color::Rgb(64, 74, 92),
        }
    }

    /// Graceful 256-color indexed slate palette for SSH `TERM=xterm-256color`.
    pub const fn ansi256() -> Self {
        let bg_surface = Color::Indexed(234);
        Self {
            monochrome: false,
            bg_base: Color::Reset,
            bg_surface,
            bg_surface_focused: Color::Indexed(235),
            bg_toast: Color::Indexed(236),
            bg_inset: Color::Indexed(233),
            bg_highlight: Color::Indexed(237),
            pill_active_bg: Color::Indexed(238),
            pill_active_fg: Color::Indexed(255),
            pill_inactive_bg: bg_surface,
            capsule_ok_bg: Color::Indexed(22),
            capsule_off_bg: Color::Indexed(235),
            capsule_warn_bg: Color::Indexed(58),
            capsule_danger_bg: Color::Indexed(52),
            accent_primary: Color::Indexed(111),
            accent_secondary: Color::Indexed(146),
            border_active: Color::Indexed(60),
            border_inactive: Color::Indexed(237),
            text_primary: Color::Indexed(254),
            text_muted: Color::Indexed(246),
            text_inactive: Color::Indexed(244),
            success: Color::Indexed(78),
            warning: Color::Indexed(178),
            danger: Color::Indexed(167),
            spark_net: Color::Indexed(74),
            dim_backdrop_fg: Color::Indexed(239),
        }
    }

    /// Light-terminal-safe ANSI 16-color fallback (`Color::Reset` backgrounds + rounded borders).
    pub const fn ansi16() -> Self {
        Self {
            monochrome: false,
            bg_base: Color::Reset,
            bg_surface: Color::Reset,
            bg_surface_focused: Color::Reset,
            bg_toast: Color::Reset,
            bg_inset: Color::Reset,
            bg_highlight: Color::Reset,
            pill_active_bg: Color::Blue,
            pill_active_fg: Color::White,
            pill_inactive_bg: Color::Reset,
            capsule_ok_bg: Color::Reset,
            capsule_off_bg: Color::Reset,
            capsule_warn_bg: Color::Reset,
            capsule_danger_bg: Color::Reset,
            accent_primary: Color::Blue,
            accent_secondary: Color::Cyan,
            border_active: Color::Blue,
            border_inactive: Color::DarkGray,
            text_primary: Color::Reset,
            text_muted: Color::DarkGray,
            text_inactive: Color::DarkGray,
            success: Color::Green,
            warning: Color::Yellow,
            danger: Color::Red,
            spark_net: Color::Cyan,
            dim_backdrop_fg: Color::DarkGray,
        }
    }

    /// Strict monochrome palette for `NO_COLOR` or `TERM=dumb`.
    pub const fn monochrome() -> Self {
        Self {
            monochrome: true,
            bg_base: Color::Reset,
            bg_surface: Color::Reset,
            bg_surface_focused: Color::Reset,
            bg_toast: Color::Reset,
            bg_inset: Color::Reset,
            bg_highlight: Color::Reset,
            pill_active_bg: Color::Reset,
            pill_active_fg: Color::Reset,
            pill_inactive_bg: Color::Reset,
            capsule_ok_bg: Color::Reset,
            capsule_off_bg: Color::Reset,
            capsule_warn_bg: Color::Reset,
            capsule_danger_bg: Color::Reset,
            accent_primary: Color::Reset,
            accent_secondary: Color::Reset,
            border_active: Color::Reset,
            border_inactive: Color::Reset,
            text_primary: Color::Reset,
            text_muted: Color::Reset,
            text_inactive: Color::Reset,
            success: Color::Reset,
            warning: Color::Reset,
            danger: Color::Reset,
            spark_net: Color::Reset,
            dim_backdrop_fg: Color::Reset,
        }
    }

    /// Returns `true` when solid borderless cards separated by `Color::Reset` gutters are used
    /// (`truecolor` and `ansi256`), and `false` for `ansi16` / `monochrome`.
    pub const fn uses_solid_cards(&self) -> bool {
        !self.monochrome && !matches!(self.bg_surface, Color::Reset)
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

/// Returns whether animations should be enabled for the given environment flags.
pub fn should_enable_animations(no_color: bool, term: Option<&str>, ci: Option<&str>) -> bool {
    MotionPolicy::detect_from(no_color, term, ci, None, None).animations_enabled
}

/// Interactive screen hit-map populated during each Ratatui frame draw for U25 mouse dispatch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UiHitMap {
    pub tab_rects: Vec<(Rect, WorkspaceTab)>,
    pub cmd_chip_rect: Option<Rect>,
    pub help_chip_rect: Option<Rect>,
    pub menu_area: Option<Rect>,
    pub menu_row_rects: Vec<(Rect, usize)>,
    pub telemetry_area: Option<Rect>,
    pub inspector_area: Option<Rect>,
    pub palette_modal_rect: Option<Rect>,
    pub palette_row_rects: Vec<(Rect, usize)>,
    pub help_modal_rect: Option<Rect>,
    pub confirm_modal_rect: Option<Rect>,
    pub confirm_yes_rect: Option<Rect>,
    pub confirm_no_rect: Option<Rect>,
}

fn rect_contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x && col < rect.right() && row >= rect.y && row < rect.bottom()
}

/// Maps a terminal `MouseEvent` against the last rendered `UiHitMap` and current `AppState`.
///
/// Strictly ignores `Moved`, `Drag`, and `Up` events to avoid SSH hover/drag traffic.
pub fn hit_test_mouse(map: &UiHitMap, mouse: MouseEvent, state: &AppState) -> Option<MenuEvent> {
    hit_test_mouse_with_flags(
        map,
        mouse,
        state.pending_confirm().is_some(),
        state.help_active(),
        state.palette_active(),
    )
}

fn hit_test_mouse_with_flags(
    map: &UiHitMap,
    mouse: MouseEvent,
    has_confirm: bool,
    has_help: bool,
    has_palette: bool,
) -> Option<MenuEvent> {
    let (col, row) = (mouse.column, mouse.row);
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if has_confirm {
                if map
                    .confirm_yes_rect
                    .is_some_and(|r| rect_contains(r, col, row))
                {
                    return Some(MenuEvent::ConfirmYes);
                }
                if map
                    .confirm_no_rect
                    .is_some_and(|r| rect_contains(r, col, row))
                {
                    return Some(MenuEvent::ConfirmNo);
                }
                if let Some(modal_rect) = map.confirm_modal_rect
                    && !rect_contains(modal_rect, col, row)
                {
                    return Some(MenuEvent::ConfirmNo);
                }
                return None;
            }

            if has_help {
                if map
                    .help_chip_rect
                    .is_some_and(|r| rect_contains(r, col, row))
                {
                    return Some(MenuEvent::HelpToggle);
                }
                if let Some(modal_rect) = map.help_modal_rect
                    && !rect_contains(modal_rect, col, row)
                {
                    return Some(MenuEvent::Back);
                }
                return None;
            }

            if has_palette {
                for &(rect, idx) in &map.palette_row_rects {
                    if rect_contains(rect, col, row) {
                        return Some(MenuEvent::ClickPaletteItem(idx));
                    }
                }
                if let Some(modal_rect) = map.palette_modal_rect
                    && !rect_contains(modal_rect, col, row)
                {
                    return Some(MenuEvent::Back);
                }
                return None;
            }

            if map
                .help_chip_rect
                .is_some_and(|r| rect_contains(r, col, row))
            {
                return Some(MenuEvent::HelpToggle);
            }
            for &(rect, tab) in &map.tab_rects {
                if rect_contains(rect, col, row) {
                    return Some(MenuEvent::ClickTab(tab));
                }
            }
            if map
                .cmd_chip_rect
                .is_some_and(|r| rect_contains(r, col, row))
            {
                return Some(MenuEvent::PaletteOpen);
            }
            for &(rect, idx) in &map.menu_row_rects {
                if rect_contains(rect, col, row) {
                    return Some(MenuEvent::ClickMenuItem(idx));
                }
            }
            if map.menu_area.is_some_and(|r| rect_contains(r, col, row)) {
                return Some(MenuEvent::FocusPane(ActivePane::Menu));
            }
            if map
                .telemetry_area
                .is_some_and(|r| rect_contains(r, col, row))
                || map
                    .inspector_area
                    .is_some_and(|r| rect_contains(r, col, row))
            {
                return Some(MenuEvent::FocusPane(ActivePane::Telemetry));
            }
            None
        }
        MouseEventKind::ScrollUp => {
            if has_confirm || has_help {
                return None;
            }
            if has_palette {
                return Some(MenuEvent::Up);
            }
            if map
                .telemetry_area
                .is_some_and(|r| rect_contains(r, col, row))
                || map
                    .inspector_area
                    .is_some_and(|r| rect_contains(r, col, row))
            {
                return Some(MenuEvent::ScrollTelemetry(-2));
            }
            if map.menu_area.is_some_and(|r| rect_contains(r, col, row)) {
                return Some(MenuEvent::Up);
            }
            None
        }
        MouseEventKind::ScrollDown => {
            if has_confirm || has_help {
                return None;
            }
            if has_palette {
                return Some(MenuEvent::Down);
            }
            if map
                .telemetry_area
                .is_some_and(|r| rect_contains(r, col, row))
                || map
                    .inspector_area
                    .is_some_and(|r| rect_contains(r, col, row))
            {
                return Some(MenuEvent::ScrollTelemetry(2));
            }
            if map.menu_area.is_some_and(|r| rect_contains(r, col, row)) {
                return Some(MenuEvent::Down);
            }
            None
        }
        _ => None,
    }
}

struct ActiveSession {
    // Field order matters: `terminal` drops before `guard` so `guard` restores
    // mouse capture, alternate screen, and raw mode last.
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    guard: TerminalGuard<CrosstermOps>,
}

/// Queries the active terminal dimensions `(cols, rows)` via `crossterm::terminal::size()`.
pub fn detect_terminal_size() -> Option<(u16, u16)> {
    crossterm::terminal::size().ok()
}

/// Full-screen Ratatui renderer with lazy terminal initialization, U23/U26 translucent-safe
/// flat cards, U25 mouse hit-map, wall-clock wake scheduling, SSH slow-draw snapping, and RAII cleanup.
pub struct RatatuiRenderer {
    session: Option<ActiveSession>,
    palette: UiPalette,
    filter_active: bool,
    last_split_mode: bool,
    hit_map: UiHitMap,
    next_wake: Option<WakeSchedule>,
    last_draw_duration: Duration,
    slow_draw_threshold: Duration,
    pending_confirm_active: bool,
    help_active: bool,
    palette_active: bool,
}

impl RatatuiRenderer {
    /// Creates a new `RatatuiRenderer` without touching terminal ioctls.
    ///
    /// Terminal raw mode, alternate screen, and mouse capture are entered lazily on the first
    /// `draw()` call so constructing under `CI=true` or non-TTY environments is always safe.
    pub fn new() -> Self {
        Self {
            session: None,
            palette: UiPalette::detect(),
            filter_active: false,
            last_split_mode: true,
            hit_map: UiHitMap::default(),
            next_wake: None,
            last_draw_duration: Duration::ZERO,
            slow_draw_threshold: SSH_SLOW_DRAW_THRESHOLD,
            pending_confirm_active: false,
            help_active: false,
            palette_active: false,
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

/// Maps a `crossterm::event::KeyEvent` to `MenuEvent` in normal navigation mode (U25 Micro/Yazi keymap).
///
/// Supports:
/// - Quit: `Ctrl+Q`, `Ctrl+C`, `q`, `Q`
/// - Palette: `Ctrl+K`, `Ctrl+P`, `Alt+P`
/// - Filter: `/`, `Ctrl+F`
/// - Navigation: `Up`/`k`, `Down`/`j` (including `Repeat`), `PageUp`, `PageDown`, `Home`/`g`, `End`/`G`
/// - Tabs / Submenu drill: `Left`/`Alt+Left`/`[`/`h` (`PrevTab`), `Right`/`Alt+Right`/`]`/`l` (`NextTab`)
/// - Pane toggle: `Tab`; Confirm: `Enter`, `y`/`Y`, `n`/`N`; Back: `Esc`, `Backspace`; Refresh: `r`/`R`; Digits: `0-9`
pub fn map_crossterm_key(key: KeyEvent) -> Option<MenuEvent> {
    let has_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let has_alt = key.modifiers.contains(KeyModifiers::ALT);
    let has_ctrl_or_alt = has_ctrl || has_alt;

    match key.kind {
        KeyEventKind::Press => {}
        KeyEventKind::Repeat
            if !has_ctrl_or_alt
                && matches!(
                    key.code,
                    KeyCode::Up
                        | KeyCode::Down
                        | KeyCode::PageUp
                        | KeyCode::PageDown
                        | KeyCode::Char('j' | 'k')
                ) => {}
        _ => return None,
    }

    if has_ctrl {
        return match key.code {
            KeyCode::Char('q' | 'Q' | 'c' | 'C') => Some(MenuEvent::Quit),
            KeyCode::Char('k' | 'K' | 'p' | 'P') => Some(MenuEvent::PaletteOpen),
            KeyCode::Char('f' | 'F') => Some(MenuEvent::FilterStart),
            _ => None,
        };
    }

    if has_alt {
        return match key.code {
            KeyCode::Char('p' | 'P') => Some(MenuEvent::PaletteOpen),
            KeyCode::Left => Some(MenuEvent::PrevTab),
            KeyCode::Right => Some(MenuEvent::NextTab),
            _ => None,
        };
    }

    match key.code {
        KeyCode::Up | KeyCode::Char('k') => Some(MenuEvent::Up),
        KeyCode::Down | KeyCode::Char('j') => Some(MenuEvent::Down),
        KeyCode::Left | KeyCode::Char('[' | 'h') => Some(MenuEvent::PrevTab),
        KeyCode::Right | KeyCode::Char(']' | 'l') => Some(MenuEvent::NextTab),
        KeyCode::PageUp => Some(MenuEvent::PageUp),
        KeyCode::PageDown => Some(MenuEvent::PageDown),
        KeyCode::Home | KeyCode::Char('g') => Some(MenuEvent::Home),
        KeyCode::End | KeyCode::Char('G') => Some(MenuEvent::End),
        KeyCode::Enter => Some(MenuEvent::Select),
        KeyCode::Esc | KeyCode::Backspace => Some(MenuEvent::Back),
        KeyCode::Tab => Some(MenuEvent::TogglePane),
        KeyCode::Char('/') => Some(MenuEvent::FilterStart),
        KeyCode::Char('?') | KeyCode::F(1) => Some(MenuEvent::HelpToggle),
        KeyCode::Char('q' | 'Q') => Some(MenuEvent::Quit),
        KeyCode::Char('r' | 'R') => Some(MenuEvent::Refresh),
        KeyCode::Char('y' | 'Y') => Some(MenuEvent::ConfirmYes),
        KeyCode::Char('n' | 'N') => Some(MenuEvent::ConfirmNo),
        KeyCode::Char(ch) if ch.is_ascii_digit() => Some(MenuEvent::Digit((ch as u8) - b'0')),
        _ => None,
    }
}

/// Maps a `crossterm::event::KeyEvent` while inline `/` fuzzy filter or `Ctrl+K` palette mode is active.
///
/// Plain `j`/`k`/`h`/`l`/`g`/`G`/`q` are typed directly into the query string; `Ctrl+Q`/`Ctrl+C` quit.
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
            KeyCode::Char('q' | 'Q' | 'c' | 'C') => Some(MenuEvent::Quit),
            KeyCode::Char('k' | 'K' | 'p' | 'P') => Some(MenuEvent::PaletteOpen),
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
        KeyCode::F(1) => Some(MenuEvent::HelpToggle),
        KeyCode::Char(ch) if !ch.is_control() => Some(MenuEvent::FilterChar(ch)),
        _ => None,
    }
}

impl TuiRenderer for RatatuiRenderer {
    fn draw(&mut self, state: &AppState) -> io::Result<()> {
        let now = Instant::now();
        self.filter_active = state.is_text_input_active();
        self.pending_confirm_active = state.pending_confirm().is_some();
        self.help_active = state.help_active();
        self.palette_active = state.palette_active();
        self.slow_draw_threshold = state.motion_policy().slow_draw_threshold;

        let palette = self.palette;
        let session = self.ensure_session()?;
        let mut split_mode = false;
        let mut next_hit_map = UiHitMap::default();
        let draw_start = Instant::now();
        session.terminal.draw(|frame| {
            split_mode = frame.area().width >= SPLIT_PANE_MIN_WIDTH;
            render_frame_internal(frame, state, &palette, false, &mut next_hit_map, now);
        })?;
        self.last_draw_duration = draw_start.elapsed();
        self.last_split_mode = split_mode;
        self.hit_map = next_hit_map;
        self.next_wake = state.next_wake_at(Instant::now(), split_mode);
        Ok(())
    }

    fn read_event(&mut self) -> io::Result<MenuEvent> {
        loop {
            if let Some(wake) = self.next_wake
                && !event::poll(wake.timeout)?
            {
                return Ok(match wake.kind {
                    WakeKind::AnimFrame => MenuEvent::Tick,
                    WakeKind::TelemetrySample => MenuEvent::TelemetryTick,
                });
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
                Event::Mouse(mouse) => {
                    if let Some(ev) = hit_test_mouse_with_flags(
                        &self.hit_map,
                        mouse,
                        self.pending_confirm_active,
                        self.help_active,
                        self.palette_active,
                    ) {
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
        if self.last_draw_duration >= self.slow_draw_threshold
            && state.active_transition().is_some()
        {
            state.clear_transition();
        }
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
    let mut hit_map = UiHitMap::default();
    render_frame_internal(frame, state, palette, false, &mut hit_map, Instant::now());
}

fn render_frame_internal(
    frame: &mut ratatui::Frame<'_>,
    state: &AppState,
    palette: &UiPalette,
    flat_modals: bool,
    hit_map: &mut UiHitMap,
    now: Instant,
) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }

    let solid = palette.uses_solid_cards();
    // Translucent-safe outer margin: leave 1-col L/R + 1-row T/B `Color::Reset` frame on solid cards.
    let canvas = if solid && area.width >= 50 && area.height >= 18 {
        Rect::new(area.x + 1, area.y + 1, area.width - 2, area.height - 2)
    } else {
        area
    };

    let show_tabs = area.height >= 28 && canvas.width >= 60;
    let (header_rect, body_rect, footer_rect) = if solid {
        let header_h = if show_tabs { 2 } else { 1 };
        let use_gaps = canvas.height >= 10;
        if use_gaps {
            let v = Layout::vertical([
                Constraint::Length(header_h),
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(canvas);
            (v[0], v[2], v[4])
        } else {
            let v = Layout::vertical([
                Constraint::Length(header_h),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(canvas);
            (v[0], v[1], v[2])
        }
    } else {
        let header_h = if show_tabs { 4 } else { 3 };
        let v = Layout::vertical([
            Constraint::Length(header_h),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .split(canvas);
        (v[0], v[1], v[2])
    };

    let split_mode = area.width >= SPLIT_PANE_MIN_WIDTH;

    render_header(frame, header_rect, state, palette, show_tabs, hit_map);

    let (menu_rect, toast_anchor) = if split_mode {
        let (left_rect, right_rect) = if solid {
            let left_w = ((u32::from(body_rect.width.saturating_sub(2)) * 43) / 100) as u16;
            let panes = Layout::horizontal([
                Constraint::Length(left_w.max(30)),
                Constraint::Length(2),
                Constraint::Min(20),
            ])
            .split(body_rect);
            (panes[0], panes[2])
        } else {
            let panes =
                Layout::horizontal([Constraint::Percentage(43), Constraint::Percentage(57)])
                    .split(body_rect);
            (panes[0], panes[1])
        };

        render_menu_panel(frame, left_rect, state, palette, true, hit_map);

        let is_submenu = state.current_menu() != MenuId::Main;
        let min_dual_h = if solid { 21 } else { 25 };
        if right_rect.height >= min_dual_h {
            let (tele_rect, insp_rect) = if solid {
                let insp_h = if is_submenu {
                    13u16
                        .max(right_rect.height / 2)
                        .min(right_rect.height.saturating_sub(9))
                } else if right_rect.height >= 29 {
                    right_rect.height.saturating_sub(20).max(9)
                } else {
                    9u16.min(right_rect.height.saturating_sub(11))
                };
                let tele_h = right_rect.height.saturating_sub(insp_h + 1);
                let stack = Layout::vertical([
                    Constraint::Length(tele_h),
                    Constraint::Length(1),
                    Constraint::Length(insp_h),
                ])
                .split(right_rect);
                (stack[0], stack[2])
            } else {
                let insp_h = if is_submenu {
                    13u16.min(right_rect.height.saturating_sub(10))
                } else {
                    10u16.min(right_rect.height.saturating_sub(12))
                };
                let stack =
                    Layout::vertical([Constraint::Min(10), Constraint::Length(insp_h)])
                        .split(right_rect);
                (stack[0], stack[1])
            };

            render_status_panel(frame, tele_rect, state, palette, false, hit_map);
            render_inspector_panel(frame, insp_rect, state, palette, hit_map);
            (left_rect, insp_rect)
        } else {
            render_status_panel(frame, right_rect, state, palette, true, hit_map);
            (left_rect, right_rect)
        }
    } else {
        render_menu_panel(frame, body_rect, state, palette, false, hit_map);
        (body_rect, body_rect)
    };

    // Cubic-eased interior fade transition on Menu card (0 neon shimmer, 0 border redraw).
    if state.animations_enabled()
        && !palette.monochrome
        && let Some(tr) = state.active_transition()
        && !matches!(tr.kind, TransitionKind::ModalOpen)
    {
        let eased = tr.eased_progress_at(now);
        if eased < 0.999 {
            let content_area = if solid {
                Rect::new(
                    menu_rect.x,
                    menu_rect.y.saturating_add(1),
                    menu_rect.width,
                    menu_rect.height.saturating_sub(1),
                )
            } else {
                menu_rect.inner(Margin {
                    vertical: 1,
                    horizontal: 1,
                })
            };
            apply_eased_menu_transition(frame, content_area, tr.kind, eased, palette);
        }
    }

    let has_modal =
        state.pending_confirm().is_some() || state.help_active() || state.palette_active();
    let min_toast_h = if solid { 3 } else { 5 };
    let show_floating_toast = !has_modal
        && state.notice().is_some()
        && toast_anchor.width >= 24
        && toast_anchor.height >= min_toast_h;

    render_footer(
        frame,
        footer_rect,
        state,
        palette,
        split_mode,
        !show_floating_toast,
        hit_map,
    );

    if show_floating_toast && let Some(msg) = state.notice() {
        render_toast(
            frame,
            toast_anchor,
            area,
            msg,
            state.notice_kind(),
            palette,
            flat_modals,
        );
    }

    let modal_unfold = if state.animations_enabled()
        && let Some(tr) = state.active_transition()
        && matches!(tr.kind, TransitionKind::ModalOpen)
    {
        tr.eased_progress_at(now)
    } else {
        1.0
    };

    if let Some(confirm) = state.pending_confirm() {
        apply_dimmed_backdrop(frame, header_rect, palette);
        apply_dimmed_backdrop(frame, body_rect, palette);
        render_confirm_modal(
            frame,
            body_rect,
            area,
            &confirm.message,
            confirm.is_destructive(),
            palette,
            flat_modals,
            modal_unfold,
            hit_map,
        );
    } else if state.help_active() {
        apply_dimmed_backdrop(frame, header_rect, palette);
        apply_dimmed_backdrop(frame, body_rect, palette);
        render_help_modal(
            frame,
            body_rect,
            area,
            state,
            palette,
            flat_modals,
            modal_unfold,
            hit_map,
        );
    } else if state.palette_active() {
        apply_dimmed_backdrop(frame, header_rect, palette);
        apply_dimmed_backdrop(frame, body_rect, palette);
        render_palette_modal(
            frame,
            body_rect,
            area,
            state,
            palette,
            flat_modals,
            modal_unfold,
            hit_map,
        );
    }
}

fn fill_card_surface(frame: &mut ratatui::Frame<'_>, area: Rect, bg: Color) {
    if bg != Color::Reset {
        frame.render_widget(Block::default().style(Style::default().bg(bg)), area);
    }
}

fn capsule_style(ok: bool, warn_on_off: bool, palette: &UiPalette) -> (Style, Style) {
    if palette.monochrome {
        return (Style::default(), Style::default());
    }
    if ok {
        let bg = palette.capsule_ok_bg;
        let s = Style::default().fg(palette.success).bg(bg);
        (s, s)
    } else if warn_on_off {
        let bg = palette.capsule_warn_bg;
        let s = Style::default().fg(palette.warning).bg(bg);
        (s, s)
    } else {
        let bg = palette.capsule_off_bg;
        let s = Style::default().fg(palette.text_inactive).bg(bg);
        (s, s)
    }
}

fn render_header(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &AppState,
    palette: &UiPalette,
    show_tabs: bool,
    hit_map: &mut UiHitMap,
) {
    let solid = palette.uses_solid_cards();
    let card_bg = if solid {
        palette.bg_surface
    } else {
        Color::Reset
    };
    let screen = state.current_screen();
    let status = &state.status;
    let specs = state.telemetry_specs();
    let history = state.telemetry_history();

    let inner = if solid {
        fill_card_surface(frame, area, card_bg);
        area
    } else {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(palette.border_active))
            .style(Style::default().bg(card_bg));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        inner
    };

    if inner.width < 4 || inner.height == 0 {
        return;
    }

    let usable_w = inner.width.saturating_sub(2) as usize;
    let total_tools = status.tools.len();
    let installed_tools = status.tools.values().filter(|&&v| v).count();
    let region = if status.is_cn_region { "CN" } else { "Global" };
    let uptime = format_uptime_compact(history.latest.uptime_secs);
    let version = status.version_display.as_str();

    // Build progressive right-side info candidates (highest richness first).
    let right_candidates = [
        format!("up {uptime} · {region} · {installed_tools}/{total_tools} · {version}"),
        format!("{region} · {installed_tools}/{total_tools} · {version}"),
        format!("{installed_tools}/{total_tools} · {version}"),
        version.to_string(),
    ];

    // Build progressive left-side identity candidates (zero vanity strings).
    let left_candidates: Vec<String> = if screen.id == MenuId::Main {
        vec![
            format!(
                "debopty · {} · {} · {} · {}",
                specs.hostname, specs.os_name, specs.kernel, specs.arch
            ),
            format!(
                "debopty · {} · {} · {}",
                specs.hostname, specs.os_name, specs.arch
            ),
            format!("debopty · {} · {}", specs.hostname, specs.os_name),
            format!("debopty · {}", specs.hostname),
            "debopty".to_string(),
        ]
    } else {
        vec![
            format!(
                "debopty · {} · {} · {}",
                screen.title, specs.hostname, specs.os_name
            ),
            format!("debopty · {} · {}", screen.title, specs.hostname),
            format!("debopty · {}", screen.title),
            "debopty".to_string(),
        ]
    };

    let mut chosen_left = truncate_to_width("debopty", usable_w);
    let mut chosen_right: Option<String> = None;
    'pick: for lc in &left_candidates {
        let lw = visual_width(lc);
        for rc in &right_candidates {
            let rw = visual_width(rc);
            if lw + 2 + rw <= usable_w {
                chosen_left = lc.clone();
                chosen_right = Some(rc.clone());
                break 'pick;
            }
        }
    }
    if chosen_right.is_none() {
        for lc in &left_candidates {
            if visual_width(lc) <= usable_w {
                chosen_left = lc.clone();
                break;
            }
        }
    }

    let brand_style = Style::default()
        .fg(palette.accent_primary)
        .bg(card_bg)
        .add_modifier(Modifier::BOLD);
    let meta_style = Style::default().fg(palette.text_primary).bg(card_bg);
    let muted_style = Style::default().fg(palette.text_muted).bg(card_bg);
    let sp_style = Style::default().bg(card_bg);

    let mut row0_spans = vec![Span::styled(" ", sp_style)];
    if let Some(rest) = chosen_left.strip_prefix("debopty") {
        row0_spans.push(Span::styled("debopty", brand_style));
        if !rest.is_empty() {
            row0_spans.push(Span::styled(rest.to_string(), meta_style));
        }
    } else {
        row0_spans.push(Span::styled(chosen_left.clone(), brand_style));
    }

    let left_w = visual_width(&chosen_left);
    if let Some(rc) = chosen_right {
        let rw = visual_width(&rc);
        let gap = usable_w.saturating_sub(left_w + rw);
        row0_spans.push(Span::styled(" ".repeat(gap), sp_style));
        row0_spans.push(Span::styled(rc, muted_style));
    }
    frame.render_widget(
        Paragraph::new(Line::from(row0_spans)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    if show_tabs && inner.height >= 2 {
        let tab_y = inner.y + 1;
        let active_tab = state.workspace_tab();
        let mut row1_spans = vec![Span::styled(" ", sp_style)];
        let mut cursor_x = inner.x.saturating_add(1);
        let mut tabs_w = 0usize;

        for (idx, tab) in WorkspaceTab::ALL.iter().enumerate() {
            let pill_txt = format!(" {} ", tab.label());
            let pill_w = visual_width(&pill_txt);
            let sep_w = usize::from(idx > 0);
            if tabs_w + sep_w + pill_w > usable_w {
                break;
            }
            if idx > 0 {
                row1_spans.push(Span::styled(" ", sp_style));
                cursor_x = cursor_x.saturating_add(1);
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
            let rect = Rect::new(cursor_x, tab_y, pill_w as u16, 1);
            hit_map.tab_rects.push((rect, *tab));
            row1_spans.push(Span::styled(pill_txt, pill_style));
            cursor_x = cursor_x.saturating_add(pill_w as u16);
            tabs_w += pill_w;
        }

        let cmd_chip = " Ctrl+K 命令 ";
        let cmd_w = visual_width(cmd_chip);
        let cmd_bg = if palette.monochrome {
            Color::Reset
        } else {
            palette.capsule_off_bg
        };
        let cmd_style = if palette.monochrome {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(palette.accent_primary)
                .bg(cmd_bg)
                .add_modifier(Modifier::BOLD)
        };
        if usable_w >= tabs_w + 2 + cmd_w {
            let gap = usable_w - tabs_w - cmd_w;
            row1_spans.push(Span::styled(" ".repeat(gap), sp_style));
            let chip_x = inner.x.saturating_add(1 + (tabs_w + gap) as u16);
            hit_map.cmd_chip_rect = Some(Rect::new(chip_x, tab_y, cmd_w as u16, 1));
            row1_spans.push(Span::styled(cmd_chip, cmd_style));
        }

        frame.render_widget(
            Paragraph::new(Line::from(row1_spans)),
            Rect::new(inner.x, tab_y, inner.width, 1),
        );
    }
}

fn render_card_header_row(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    title: &str,
    right_hint: Option<&str>,
    focused: bool,
    card_bg: Color,
    palette: &UiPalette,
) {
    if area.width < 4 || area.height == 0 {
        return;
    }
    let usable_w = area.width.saturating_sub(2) as usize;
    let sp_style = Style::default().bg(card_bg);
    let title_style = if focused {
        Style::default()
            .fg(palette.text_primary)
            .bg(card_bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(palette.text_muted)
            .bg(card_bg)
            .add_modifier(Modifier::BOLD)
    };
    let marker_style = Style::default()
        .fg(if focused {
            palette.accent_primary
        } else {
            palette.border_active
        })
        .bg(card_bg);
    let hint_style = Style::default().fg(palette.text_muted).bg(card_bg);

    let hint_w = right_hint.map(visual_width).unwrap_or(0);
    let title_budget = if hint_w > 0 && usable_w > hint_w + 4 {
        usable_w - hint_w - 2
    } else {
        usable_w
    };
    let fitted_title = truncate_to_width(title, title_budget.saturating_sub(2));
    let left_w = 2 + visual_width(&fitted_title);

    let mut spans = vec![
        Span::styled(if focused { "▎" } else { " " }, marker_style),
        Span::styled(" ", sp_style),
        Span::styled(fitted_title, title_style),
    ];
    if let Some(hint) = right_hint
        && left_w + 2 + hint_w <= usable_w
    {
        let gap = usable_w - left_w - hint_w;
        spans.push(Span::styled(" ".repeat(gap), sp_style));
        spans.push(Span::styled(hint.to_string(), hint_style));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_menu_section_line(
    label: &str,
    content_w: usize,
    card_bg: Color,
    palette: &UiPalette,
) -> Line<'static> {
    let sp_style = Style::default().bg(card_bg);
    let label_style = Style::default()
        .fg(palette.text_muted)
        .bg(card_bg)
        .add_modifier(Modifier::BOLD);
    let rule_style = Style::default().fg(palette.border_inactive).bg(card_bg);
    let usable_w = content_w.saturating_sub(2).max(1);
    let lw = visual_width(label);
    if lw + 3 >= usable_w {
        return Line::from(vec![
            Span::styled(" ", sp_style),
            Span::styled(truncate_to_width(label, usable_w), label_style),
        ]);
    }
    let tail = "─".repeat(usable_w - lw - 2);
    Line::from(vec![
        Span::styled(" ", sp_style),
        Span::styled(label.to_string(), label_style),
        Span::styled(format!("  {tail}"), rule_style),
    ])
}

fn render_menu_panel(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &AppState,
    palette: &UiPalette,
    split_mode: bool,
    hit_map: &mut UiHitMap,
) {
    hit_map.menu_area = Some(area);
    let solid = palette.uses_solid_cards();
    let screen = state.current_screen();
    let is_focused = !split_mode || state.active_pane() == ActivePane::Menu;
    let card_bg = if solid {
        if is_focused {
            palette.bg_surface_focused
        } else {
            palette.bg_surface
        }
    } else {
        Color::Reset
    };

    let filter_query = state.filter_query();
    let filtering = !filter_query.is_empty();
    let total_items = screen.items().len();
    let matched_items = if filtering {
        state.filtered_items()
    } else {
        Vec::new()
    };

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
    } else if screen.id == MenuId::Main {
        "功能菜单".to_string()
    } else {
        format!("功能菜单 · {}", screen.title)
    };

    let list_area = if solid {
        fill_card_surface(frame, area, card_bg);
        Rect::new(
            area.x,
            area.y.saturating_add(1),
            area.width,
            area.height.saturating_sub(1),
        )
    } else {
        area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        })
    };

    let inner_w = list_area.width as usize;
    let viewport_h = list_area.height as usize;

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

    let raw_row_count = if filtering {
        matched_items.len().max(1)
    } else {
        screen.entries.len() + spacer_rows
    };
    let needs_scrollbar = raw_row_count > viewport_h && viewport_h >= 2;
    let content_w = if needs_scrollbar {
        inner_w.saturating_sub(1).max(1)
    } else {
        inner_w
    };
    let divider_w = content_w.saturating_sub(2).max(1);

    let mut list_items = Vec::with_capacity(raw_row_count);
    let mut row_item_indices: Vec<Option<usize>> = Vec::with_capacity(raw_row_count);
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
                Style::default().fg(palette.warning).bg(card_bg),
            ))));
            row_item_indices.push(None);
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
                    card_bg,
                    palette,
                )));
                row_item_indices.push(Some(idx));
            }
        }
    } else {
        let mut seen_sections = 0usize;
        for entry in &screen.entries {
            match entry {
                MenuEntry::Section(label) => {
                    if extra_section_spacers && seen_sections > 0 {
                        list_items.push(ListItem::new(Line::from(Span::styled(
                            "",
                            Style::default().bg(card_bg),
                        ))));
                        row_item_indices.push(None);
                    }
                    seen_sections += 1;
                    list_items.push(ListItem::new(render_menu_section_line(
                        label, content_w, card_bg, palette,
                    )));
                    row_item_indices.push(None);
                }
                MenuEntry::Separator => {
                    let line = Line::from(Span::styled(
                        format!(" {}", "─".repeat(divider_w)),
                        Style::default().fg(palette.border_inactive).bg(card_bg),
                    ));
                    list_items.push(ListItem::new(line));
                    row_item_indices.push(None);
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
                        card_bg,
                        palette,
                    )));
                    row_item_indices.push(Some(item_idx));
                    item_idx += 1;
                }
            }
        }
    }

    let scroll_hint = if needs_scrollbar && area.width >= 20 {
        let sel_pos = selected_row.unwrap_or(0) + 1;
        Some(format!("↕ {sel_pos}/{raw_row_count}"))
    } else {
        None
    };

    if solid {
        render_card_header_row(
            frame,
            Rect::new(area.x, area.y, area.width, 1),
            &panel_title,
            scroll_hint.as_deref(),
            is_focused,
            card_bg,
            palette,
        );
    } else {
        let border_color = if is_focused {
            palette.border_active
        } else {
            palette.border_inactive
        };
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color))
            .style(Style::default().bg(card_bg))
            .title(Span::styled(
                format!(" {panel_title} "),
                Style::default()
                    .fg(if is_focused {
                        palette.text_primary
                    } else {
                        palette.text_muted
                    })
                    .add_modifier(Modifier::BOLD),
            ));
        if let Some(ref hint) = scroll_hint {
            block = block.title_bottom(
                Line::from(Span::styled(
                    format!(" {hint} "),
                    Style::default().fg(palette.text_muted),
                ))
                .alignment(Alignment::Right),
            );
        }
        frame.render_widget(block, area);
    }

    let scroll_offset = if viewport_h > 0
        && let Some(sel_row) = selected_row
        && sel_row >= viewport_h
    {
        sel_row + 1 - viewport_h
    } else {
        0
    };

    // Record exact hit-map rectangles for every visible clickable menu item.
    for (vis_idx, opt_item_idx) in row_item_indices
        .iter()
        .skip(scroll_offset)
        .take(viewport_h)
        .enumerate()
    {
        if let Some(idx) = opt_item_idx {
            let y = list_area.y.saturating_add(vis_idx as u16);
            hit_map
                .menu_row_rects
                .push((Rect::new(list_area.x, y, content_w as u16, 1), *idx));
        }
    }

    let list = List::new(list_items);
    let mut list_state = ListState::default();
    *list_state.offset_mut() = scroll_offset;
    list_state.select(selected_row);
    frame.render_stateful_widget(list, list_area, &mut list_state);

    if needs_scrollbar {
        let mut sb_state = ScrollbarState::new(raw_row_count)
            .position(selected_row.unwrap_or(0))
            .viewport_content_length(viewport_h);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("┃")
            .track_style(Style::default().fg(palette.border_inactive).bg(card_bg))
            .thumb_style(
                Style::default()
                    .fg(if is_focused {
                        palette.accent_primary
                    } else {
                        palette.text_muted
                    })
                    .bg(card_bg),
            );
        frame.render_stateful_widget(scrollbar, list_area, &mut sb_state);
    }
}

fn build_menu_line(
    item: &MenuItem,
    selected: bool,
    focused: bool,
    inner_width: usize,
    card_bg: Color,
    palette: &UiPalette,
) -> Line<'static> {
    let active_selected = selected && focused;
    let use_highlight_bg =
        active_selected && !palette.monochrome && palette.bg_highlight != Color::Reset;
    let row_bg = if use_highlight_bg {
        palette.bg_highlight
    } else {
        card_bg
    };

    let bar_style = if !focused {
        if palette.monochrome {
            Style::default().add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(palette.border_inactive).bg(row_bg)
        }
    } else if palette.monochrome {
        Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default()
            .fg(palette.accent_primary)
            .bg(row_bg)
            .add_modifier(Modifier::BOLD)
    };

    let label_style = if active_selected {
        if palette.monochrome {
            Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else if palette.bg_highlight == Color::Reset {
            Style::default()
                .fg(palette.accent_primary)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default()
                .fg(palette.text_primary)
                .bg(row_bg)
                .add_modifier(Modifier::BOLD)
        }
    } else if selected && !focused {
        if palette.monochrome {
            Style::default().add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(palette.text_muted).bg(row_bg)
        }
    } else {
        Style::default().fg(palette.text_primary).bg(row_bg)
    };

    let idx_style = if active_selected {
        if palette.monochrome {
            Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default()
                .fg(palette.accent_primary)
                .bg(row_bg)
                .add_modifier(Modifier::BOLD)
        }
    } else {
        Style::default().fg(palette.text_muted).bg(row_bg)
    };
    let sp_style = Style::default().bg(row_bg);

    // Editorial flat prefix: `▎` (1) + ` {:>2}  ` (5) = 6 columns exact.
    let pointer = if selected { "▎" } else { " " };
    let id_col = format!(" {:>2}  ", item.id);
    let prefix_w = 6usize;

    match item.status {
        Some(badge) => {
            let badge_plain = badge.plain_text();
            let badge_w = visual_width(&badge_plain);
            // Editorial flat status: NO noisy tinted boxes on every row!
            // Layout: prefix (6) + label + gap (1) + badge + trailing pad (1)
            let max_label_w = inner_width.saturating_sub(prefix_w + badge_w + 2);
            let fitted_label = truncate_to_width(item.label, max_label_w);
            let padded_label = pad_right_to_width(&fitted_label, max_label_w);

            let (dot_fg, text_fg, dot, text) = match badge {
                StatusBadge::Active(label) => (
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

            let used = prefix_w + visual_width(&padded_label) + 1 + badge_w;
            let trail = " ".repeat(inner_width.saturating_sub(used));

            Line::from(vec![
                Span::styled(pointer, if selected { bar_style } else { sp_style }),
                Span::styled(id_col, idx_style),
                Span::styled(padded_label, label_style),
                Span::styled(" ", sp_style),
                Span::styled(format!("{dot} "), Style::default().fg(dot_fg).bg(row_bg)),
                Span::styled(text, Style::default().fg(text_fg).bg(row_bg)),
                Span::styled(trail, sp_style),
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
                Span::styled(pointer, if selected { bar_style } else { sp_style }),
                Span::styled(id_col, idx_style),
                Span::styled(padded_label, label_style),
                Span::styled(suffix, suffix_style),
            ])
        }
    }
}

fn telemetry_section_header(
    label: &str,
    right_hint: Option<&str>,
    usable_w: usize,
    card_bg: Color,
    palette: &UiPalette,
) -> Line<'static> {
    let sp_style = Style::default().bg(card_bg);
    let label_style = Style::default()
        .fg(palette.accent_secondary)
        .bg(card_bg)
        .add_modifier(Modifier::BOLD);
    let rule_style = Style::default().fg(palette.border_inactive).bg(card_bg);
    let hint_style = Style::default().fg(palette.text_muted).bg(card_bg);

    let lw = visual_width(label);
    if let Some(hint) = right_hint {
        let hint_seg = format!("  {hint}");
        let hw = visual_width(&hint_seg);
        if lw + 2 + hw + 2 <= usable_w {
            let fill = "─".repeat(usable_w - lw - 2 - hw);
            return Line::from(vec![
                Span::styled(" ", sp_style),
                Span::styled(label.to_string(), label_style),
                Span::styled(format!("  {fill}"), rule_style),
                Span::styled(hint_seg, hint_style),
            ]);
        }
    }

    if lw + 2 >= usable_w {
        return Line::from(vec![
            Span::styled(" ", sp_style),
            Span::styled(truncate_to_width(label, usable_w), label_style),
        ]);
    }
    let fill = "─".repeat(usable_w - lw - 2);
    Line::from(vec![
        Span::styled(" ", sp_style),
        Span::styled(label.to_string(), label_style),
        Span::styled(format!("  {fill}"), rule_style),
    ])
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
    card_bg: Color,
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
        Style::default().fg(palette.text_muted).bg(card_bg),
    ));
    spans.push(Span::styled(val_cell, val_style.bg(card_bg)));
}

#[allow(clippy::too_many_arguments)]
fn push_sparkline_row(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    curve: String,
    readout: &str,
    curve_color: Color,
    readout_color: Color,
    usable_w: usize,
    card_bg: Color,
    palette: &UiPalette,
) {
    let sp = Style::default().bg(card_bg);
    let lbl = pad_right_to_width(label, 5);
    let curve_w = visual_width(&curve);
    let readout_budget = usable_w.saturating_sub(5 + 1 + curve_w + 2);
    let fitted_r = truncate_to_width(readout, readout_budget);
    let r_w = visual_width(&fitted_r);
    let gap = usable_w.saturating_sub(5 + 1 + curve_w + r_w);
    lines.push(Line::from(vec![
        Span::styled(" ", sp),
        Span::styled(lbl, Style::default().fg(palette.text_muted).bg(card_bg)),
        Span::styled(" ", sp),
        Span::styled(curve, Style::default().fg(curve_color).bg(card_bg)),
        Span::styled(" ".repeat(gap), sp),
        Span::styled(fitted_r, Style::default().fg(readout_color).bg(card_bg)),
    ]));
}

fn build_telemetry_lines(
    state: &AppState,
    content_w: usize,
    include_inspector: bool,
    card_bg: Color,
    palette: &UiPalette,
) -> Vec<Line<'static>> {
    let status = &state.status;
    let specs = state.telemetry_specs();
    let history = state.telemetry_history();
    let latest = &history.latest;
    let usable_w = content_w.saturating_sub(2).max(1);
    let total_tools = status.tools.len();
    let installed_tools = status.tools.values().filter(|&&v| v).count();

    let sp = Style::default().bg(card_bg);
    let primary_style = Style::default().fg(palette.text_primary).bg(card_bg);
    let muted_style = Style::default().fg(palette.text_muted).bg(card_bg);
    let accent_style = Style::default().fg(palette.accent_primary).bg(card_bg);
    let sep_style = Style::default().fg(palette.border_inactive).bg(card_bg);

    let two_col = usable_w >= 40;
    let key_w = if usable_w >= 52 { 6 } else { 5 };
    let left_w = (usable_w.saturating_sub(3)) / 2;
    let right_w = usable_w.saturating_sub(3 + left_w);

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(24);

    // ── Section 1: 硬件与内核 ──
    lines.push(telemetry_section_header(
        "硬件与内核",
        None,
        usable_w,
        card_bg,
        palette,
    ));

    let cpu_str = format!("{} ({}C)", shorten_cpu_model(&specs.cpu_model), specs.cpu_cores);
    let os_str = format!("{} {}", specs.os_name, specs.arch);
    let disk_str = match (specs.disk_root_used_gib_x10, specs.disk_root_total_gib_x10) {
        (Some(u), Some(t)) if t > 0 => format!("{:.1}/{:.1}G", f64::from(u) / 10.0, f64::from(t) / 10.0),
        _ => "-".to_string(),
    };
    let mem_total = if latest.mem_total_kib > 0 {
        latest.mem_total_kib
    } else {
        specs.mem_total_kib
    };
    let swap_total = if latest.swap_total_kib > 0 {
        latest.swap_total_kib
    } else {
        specs.swap_total_kib
    };
    let mem_str = if swap_total > 0 {
        format!(
            "{}/{} · Sw {}",
            format_kib_compact(latest.mem_used_kib),
            format_kib_compact(mem_total),
            format_kib_compact(swap_total)
        )
    } else {
        format!(
            "{}/{}",
            format_kib_compact(latest.mem_used_kib),
            format_kib_compact(mem_total)
        )
    };
    let proc_str = format!(
        "up {} · {}/{}",
        format_uptime_compact(latest.uptime_secs),
        latest.procs_running,
        latest.procs_total
    );

    let hw_rows: [(&str, &str, Style, &str, &str, Style); 3] = [
        ("CPU", &cpu_str, primary_style, "系统", &os_str, primary_style),
        (
            "内核",
            specs.kernel.as_str(),
            muted_style,
            "磁盘",
            &disk_str,
            primary_style,
        ),
        ("内存", &mem_str, primary_style, "运行", &proc_str, muted_style),
    ];
    for (lk, lv, ls, rk, rv, rs) in hw_rows {
        if two_col {
            let mut spans = vec![Span::styled(" ", sp)];
            push_kv_cell(&mut spans, lk, key_w, lv, ls, left_w, true, card_bg, palette);
            spans.push(Span::styled(" │ ", sep_style));
            push_kv_cell(&mut spans, rk, key_w, rv, rs, right_w, false, card_bg, palette);
            lines.push(Line::from(spans));
        } else {
            let mut s1 = vec![Span::styled(" ", sp)];
            push_kv_cell(&mut s1, lk, key_w, lv, ls, usable_w, false, card_bg, palette);
            lines.push(Line::from(s1));
            let mut s2 = vec![Span::styled(" ", sp)];
            push_kv_cell(&mut s2, rk, key_w, rv, rs, usable_w, false, card_bg, palette);
            lines.push(Line::from(s2));
        }
    }

    // ── Section 2: 参数与安全 ──
    lines.push(telemetry_section_header(
        "参数与安全",
        None,
        usable_w,
        card_bg,
        palette,
    ));

    let cc_str = format!("{} · {}", specs.tcp_cc, specs.default_qdisc);
    let fwd_str = format!(
        "v4 {}  v6 {}",
        if specs.ipv4_forward || status.ip_forward { "●" } else { "○" },
        if specs.ipv6_forward { "●" } else { "○" }
    );
    let fwd_style = if specs.ipv4_forward || status.ip_forward {
        Style::default().fg(palette.success).bg(card_bg)
    } else {
        Style::default().fg(palette.text_inactive).bg(card_bg)
    };
    let sw_label = specs
        .swappiness
        .map(|s| format!("sw {s}"))
        .unwrap_or_else(|| "sw -".to_string());
    let limit_str = format!("{} · {sw_label}", format_count_compact(specs.file_max));
    let base_gc_str = format!(
        "优化 {}  GC {}",
        if status.base_optimized { "●" } else { "○" },
        if status.podman_gc_enabled { "●" } else { "○" }
    );
    let base_gc_style = if status.base_optimized {
        Style::default().fg(palette.success).bg(card_bg)
    } else {
        Style::default().fg(palette.warning).bg(card_bg)
    };
    let auth_str = match specs.ssh_password_auth {
        Some(false) => "仅密钥",
        Some(true) => "允许密码",
        None => "默认",
    };
    let ssh_str = format!(":{} · {auth_str}", specs.ssh_port);
    let ssh_style = match specs.ssh_password_auth {
        Some(false) => Style::default().fg(palette.success).bg(card_bg),
        Some(true) => Style::default().fg(palette.warning).bg(card_bg),
        None => primary_style,
    };
    let nft_seg = if specs.nftables_configured {
        format!("nft({})", specs.fw_open_ports.len())
    } else {
        "nft off".to_string()
    };
    let f2b_seg = if specs.fail2ban_configured {
        let bk = if specs.fail2ban_backend.is_empty() {
            "on"
        } else {
            specs.fail2ban_backend.as_str()
        };
        format!("f2b({bk})")
    } else {
        "f2b off".to_string()
    };
    let fw_str = format!("{nft_seg} · {f2b_seg}");
    let fw_style = if specs.nftables_configured && specs.fail2ban_configured {
        Style::default().fg(palette.success).bg(card_bg)
    } else if specs.nftables_configured || specs.fail2ban_configured {
        accent_style
    } else {
        Style::default().fg(palette.text_inactive).bg(card_bg)
    };

    let sys_rows: [(&str, &str, Style, &str, &str, Style); 3] = [
        ("拥塞", &cc_str, accent_style, "转发", &fwd_str, fwd_style),
        ("限额", &limit_str, muted_style, "基线", &base_gc_str, base_gc_style),
        ("SSH", &ssh_str, ssh_style, "防护", &fw_str, fw_style),
    ];
    for (lk, lv, ls, rk, rv, rs) in sys_rows {
        if two_col {
            let mut spans = vec![Span::styled(" ", sp)];
            push_kv_cell(&mut spans, lk, key_w, lv, ls, left_w, true, card_bg, palette);
            spans.push(Span::styled(" │ ", sep_style));
            push_kv_cell(&mut spans, rk, key_w, rv, rs, right_w, false, card_bg, palette);
            lines.push(Line::from(spans));
        } else {
            let mut s1 = vec![Span::styled(" ", sp)];
            push_kv_cell(&mut s1, lk, key_w, lv, ls, usable_w, false, card_bg, palette);
            lines.push(Line::from(s1));
            let mut s2 = vec![Span::styled(" ", sp)];
            push_kv_cell(&mut s2, rk, key_w, rv, rs, usable_w, false, card_bg, palette);
            lines.push(Line::from(s2));
        }
    }

    // ── Section 3: 实时负载 (4 Genuine 2x4 Braille Curves) ──
    lines.push(telemetry_section_header(
        "实时负载",
        None,
        usable_w,
        card_bg,
        palette,
    ));

    let curve_w = usable_w.saturating_sub(5 + 1 + 2 + 15).clamp(6, 28);
    let cpu_color = if latest.cpu_pct >= 85.0 {
        palette.danger
    } else if latest.cpu_pct >= 65.0 {
        palette.warning
    } else {
        palette.accent_primary
    };
    let cpu_curve = format_braille_curve(&history.cpu_pct, curve_w, 0.0, 100.0);
    let cpu_readout = format!("{:>5.1}%", latest.cpu_pct);
    push_sparkline_row(
        &mut lines,
        "CPU",
        cpu_curve,
        &cpu_readout,
        cpu_color,
        cpu_color,
        usable_w,
        card_bg,
        palette,
    );

    let load_peak = history
        .load1
        .iter()
        .copied()
        .fold(f64::from(specs.cpu_cores.max(1)), f64::max);
    let load_color = if latest.load1 > f64::from(specs.cpu_cores.max(1)) {
        palette.warning
    } else {
        palette.accent_secondary
    };
    let load_curve = format_braille_curve(&history.load1, curve_w, 0.0, load_peak);
    let load_readout = format!("{:.2} / {:.2}", latest.load1, latest.load5);
    push_sparkline_row(
        &mut lines,
        "负载",
        load_curve,
        &load_readout,
        load_color,
        palette.text_primary,
        usable_w,
        card_bg,
        palette,
    );

    let mem_color = if latest.mem_pct >= 90.0 {
        palette.danger
    } else if latest.mem_pct >= 75.0 {
        palette.warning
    } else {
        palette.success
    };
    let mem_curve = format_braille_curve(&history.mem_pct, curve_w, 0.0, 100.0);
    let mem_readout = format!(
        "{:>4.1}% {}",
        latest.mem_pct,
        format_kib_compact(latest.mem_used_kib)
    );
    push_sparkline_row(
        &mut lines,
        "内存",
        mem_curve,
        &mem_readout,
        mem_color,
        palette.text_primary,
        usable_w,
        card_bg,
        palette,
    );

    let net_peak = history
        .net_kbps
        .iter()
        .copied()
        .fold(10.0_f64, f64::max);
    let net_curve = format_braille_curve(&history.net_kbps, curve_w, 0.0, net_peak);
    let net_readout = format!(
        "↓{} ↑{}",
        format_bytes_rate(latest.net_rx_bps),
        format_bytes_rate(latest.net_tx_bps)
    );
    push_sparkline_row(
        &mut lines,
        "网络",
        net_curve,
        &net_readout,
        palette.spark_net,
        palette.spark_net,
        usable_w,
        card_bg,
        palette,
    );

    // ── Section 4: 已部署服务 (4 Compact Stack Lines Covering All 18 TRACKED_TOOLS) ──
    let svc_hint = format!("{installed_tools}/{total_tools} 就绪");
    lines.push(telemetry_section_header(
        "已部署服务",
        Some(&svc_hint),
        usable_w,
        card_bg,
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

    let tag_w = 5usize;
    for (group_label, entries) in groups {
        let mut cur_spans = vec![
            Span::styled(" ", sp),
            Span::styled(
                pad_right_to_width(group_label, tag_w),
                Style::default().fg(palette.accent_secondary).bg(card_bg),
            ),
        ];
        let mut cur_w = tag_w;
        for (idx, &(tool, installed)) in entries.iter().enumerate() {
            let gap_str = if idx == 0 { "" } else { " " };
            let item_w = gap_str.len() + 2 + visual_width(tool);
            if idx > 0 && cur_w + item_w > usable_w {
                lines.push(Line::from(cur_spans));
                cur_spans = vec![
                    Span::styled(" ", sp),
                    Span::styled(" ".repeat(tag_w), sp),
                ];
                cur_w = tag_w;
            } else if idx > 0 {
                cur_spans.push(Span::styled(" ", sp));
                cur_w += 1;
            }
            let (dot, dot_style, name_style) = if installed {
                (
                    "●",
                    Style::default().fg(palette.success).bg(card_bg),
                    primary_style,
                )
            } else {
                (
                    "○",
                    Style::default().fg(palette.text_inactive).bg(card_bg),
                    Style::default().fg(palette.text_inactive).bg(card_bg),
                )
            };
            let fitted = truncate_to_width(tool, usable_w.saturating_sub(cur_w + 2));
            cur_spans.push(Span::styled(format!("{dot} "), dot_style));
            cur_spans.push(Span::styled(fitted.clone(), name_style));
            cur_w += 2 + visual_width(&fitted);
        }
        lines.push(Line::from(cur_spans));
    }

    // Inline Inspector fallback when split height is too compact for separate Inspector card.
    if include_inspector {
        let items = state.filtered_items();
        if let Some(sel) = items.get(state.selected_idx().min(items.len().saturating_sub(1))) {
            let meta = sel.action.inspector_meta();
            let detail = specs.module_details.get(sel.action.detail_key());
            lines.push(telemetry_section_header(
                "操作检视",
                None,
                usable_w,
                card_bg,
                palette,
            ));
            let mut r1 = vec![Span::styled(" ", sp)];
            push_kv_cell(
                &mut r1,
                "目标",
                key_w,
                sel.label,
                primary_style.add_modifier(Modifier::BOLD),
                usable_w,
                false,
                card_bg,
                palette,
            );
            lines.push(Line::from(r1));

            let path_val = detail
                .filter(|d| !d.config_path.is_empty())
                .map(|d| d.config_path.as_str())
                .unwrap_or(meta.path);
            let mut r2 = vec![Span::styled(" ", sp)];
            push_kv_cell(
                &mut r2,
                "配置",
                key_w,
                path_val,
                muted_style,
                usable_w,
                false,
                card_bg,
                palette,
            );
            lines.push(Line::from(r2));

            let mut r3 = vec![Span::styled(" ", sp)];
            push_kv_cell(
                &mut r3,
                "单元",
                key_w,
                meta.unit,
                muted_style,
                usable_w,
                false,
                card_bg,
                palette,
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
    hit_map: &mut UiHitMap,
) {
    hit_map.telemetry_area = Some(area);
    let solid = palette.uses_solid_cards();
    let is_focused = state.active_pane() == ActivePane::Telemetry;
    let card_bg = if solid {
        if is_focused {
            palette.bg_surface_focused
        } else {
            palette.bg_surface
        }
    } else {
        Color::Reset
    };

    let body_area = if solid {
        fill_card_surface(frame, area, card_bg);
        Rect::new(
            area.x,
            area.y.saturating_add(1),
            area.width,
            area.height.saturating_sub(1),
        )
    } else {
        area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        })
    };

    let inner_w = body_area.width as usize;
    let viewport_h = body_area.height as usize;

    let mut lines = build_telemetry_lines(state, inner_w, include_inspector, card_bg, palette);
    let needs_scrollbar = lines.len() > viewport_h && viewport_h >= 2;
    if needs_scrollbar {
        let content_w = inner_w.saturating_sub(1).max(1);
        lines = build_telemetry_lines(state, content_w, include_inspector, card_bg, palette);
    }

    let total_lines = lines.len();
    let max_scroll = total_lines.saturating_sub(viewport_h) as u16;
    let clamped_scroll = state.telemetry_scroll().min(max_scroll);

    let title_str = if is_focused {
        "系统概览 · ↑↓ 滚动"
    } else {
        "系统概览"
    };
    let scroll_hint = if needs_scrollbar && area.width >= 18 {
        let end_row = (clamped_scroll as usize + viewport_h).min(total_lines);
        Some(format!("↕ {end_row}/{total_lines}"))
    } else {
        None
    };

    if solid {
        render_card_header_row(
            frame,
            Rect::new(area.x, area.y, area.width, 1),
            title_str,
            scroll_hint.as_deref(),
            is_focused,
            card_bg,
            palette,
        );
    } else {
        let border_color = if is_focused {
            palette.border_active
        } else {
            palette.border_inactive
        };
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color))
            .style(Style::default().bg(card_bg))
            .title(Span::styled(
                format!(" {title_str} "),
                Style::default()
                    .fg(if is_focused {
                        palette.text_primary
                    } else {
                        palette.text_muted
                    })
                    .add_modifier(Modifier::BOLD),
            ));
        if let Some(ref hint) = scroll_hint {
            block = block.title_bottom(
                Line::from(Span::styled(
                    format!(" {hint} "),
                    Style::default().fg(palette.text_muted),
                ))
                .alignment(Alignment::Right),
            );
        }
        frame.render_widget(block, area);
    }

    let paragraph = Paragraph::new(lines).scroll((clamped_scroll, 0));
    frame.render_widget(paragraph, body_area);

    if needs_scrollbar {
        let mut sb_state = ScrollbarState::new(total_lines)
            .position(clamped_scroll as usize)
            .viewport_content_length(viewport_h);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("┃")
            .track_style(Style::default().fg(palette.border_inactive).bg(card_bg))
            .thumb_style(
                Style::default()
                    .fg(if is_focused {
                        palette.accent_primary
                    } else {
                        palette.text_muted
                    })
                    .bg(card_bg),
            );
        frame.render_stateful_widget(scrollbar, body_area, &mut sb_state);
    }
}

/// U24 Refined Inspector / Subitem Detail Card (`操作检视`).
///
/// Displays item title + semantic status pill + `[id] ↵`, service & port badge chips,
/// aligned 2-column `ModuleDetail::kv_facts`, config path `✓`/`○` status + CLI hint,
/// and recessed sanitized config preview (`bg_inset` + `▏` gutter).
fn render_inspector_panel(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &AppState,
    palette: &UiPalette,
    hit_map: &mut UiHitMap,
) {
    hit_map.inspector_area = Some(area);
    let solid = palette.uses_solid_cards();
    let card_bg = if solid {
        palette.bg_surface
    } else {
        Color::Reset
    };

    let body_area = if solid {
        fill_card_surface(frame, area, card_bg);
        Rect::new(
            area.x,
            area.y.saturating_add(1),
            area.width,
            area.height.saturating_sub(1),
        )
    } else {
        area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        })
    };

    let items = state.filtered_items();
    let selected = items.get(state.selected_idx().min(items.len().saturating_sub(1)));
    let right_hint = selected.map(|sel| format!("[{}] ↵", sel.id));

    if solid {
        render_card_header_row(
            frame,
            Rect::new(area.x, area.y, area.width, 1),
            "操作检视",
            right_hint.as_deref(),
            false,
            card_bg,
            palette,
        );
    } else {
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(palette.border_inactive))
            .style(Style::default().bg(card_bg))
            .title(Span::styled(
                " 操作检视 ",
                Style::default()
                    .fg(palette.text_muted)
                    .add_modifier(Modifier::BOLD),
            ));
        if let Some(ref hint) = right_hint {
            block = block.title(
                Line::from(Span::styled(
                    format!(" {hint} "),
                    Style::default().fg(palette.accent_primary),
                ))
                .alignment(Alignment::Right),
            );
        }
        frame.render_widget(block, area);
    }

    if body_area.width < 8 || body_area.height == 0 {
        return;
    }

    let inner_w = body_area.width as usize;
    let usable_w = inner_w.saturating_sub(2).max(1);
    let inner_h = body_area.height as usize;
    let key_w = if usable_w >= 52 { 6 } else { 5 };
    let two_col = usable_w >= 40;
    let left_w = (usable_w.saturating_sub(3)) / 2;
    let right_w = usable_w.saturating_sub(3 + left_w);

    let sp = Style::default().bg(card_bg);
    let sep_style = Style::default().fg(palette.border_inactive).bg(card_bg);
    let muted_style = Style::default().fg(palette.text_muted).bg(card_bg);
    let primary_style = Style::default().fg(palette.text_primary).bg(card_bg);

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(inner_h);

    if let Some(sel) = selected {
        let meta = sel.action.inspector_meta();
        let detail = state
            .telemetry_specs()
            .module_details
            .get(sel.action.detail_key());

        let ports_list: Vec<String> = if let Some(d) = detail
            && !d.ports.is_empty()
        {
            d.ports.clone()
        } else if meta.ports != "-" && !meta.ports.is_empty() {
            meta.ports
                .split([',', '/', ' '])
                .map(str::trim)
                .filter(|s| !s.is_empty() && *s != "-")
                .take(3)
                .map(ToString::to_string)
                .collect()
        } else {
            Vec::new()
        };

        // Row 0: Title + Semantic Status Pill + Service State Chip (+ inline ports on compact height)
        let mut r0 = vec![Span::styled(" ", sp)];
        let mut used_w = 0usize;
        let title_budget = usable_w.saturating_sub(18).max(8);
        let fitted_title = truncate_to_width(sel.label, title_budget);
        used_w += visual_width(&fitted_title);
        r0.push(Span::styled(
            fitted_title,
            primary_style.add_modifier(Modifier::BOLD),
        ));

        // Status pill
        let pill_spans: Vec<Span<'static>> = match sel.status {
            Some(StatusBadge::Active(lbl)) => {
                let (ds, ts) = capsule_style(true, false, palette);
                vec![
                    Span::styled(" ● ", ds),
                    Span::styled(format!("{lbl} "), ts),
                ]
            }
            Some(StatusBadge::Inactive(lbl)) => {
                let (ds, ts) = capsule_style(false, lbl == "未执行", palette);
                vec![
                    Span::styled(" ○ ", ds),
                    Span::styled(format!("{lbl} "), ts),
                ]
            }
            None => {
                let chip_bg = if palette.monochrome {
                    card_bg
                } else {
                    palette.capsule_off_bg
                };
                vec![Span::styled(
                    format!(" {} ", meta.kind),
                    Style::default().fg(palette.accent_secondary).bg(chip_bg),
                )]
            }
        };
        let pill_w: usize = pill_spans.iter().map(|s| visual_width(s.content.as_ref())).sum();
        if used_w + 2 + pill_w <= usable_w {
            r0.push(Span::styled("  ", sp));
            r0.extend(pill_spans);
            used_w += 2 + pill_w;
        }

        // Service state badge chip
        if let Some(d) = detail
            && !d.service_state.is_empty()
        {
            let chip_txt = format!(" {} ", d.service_state);
            let chip_w = visual_width(&chip_txt);
            if used_w + 1 + chip_w <= usable_w {
                let chip_style = if palette.monochrome {
                    Style::default().add_modifier(Modifier::DIM)
                } else {
                    match d.service_state.as_str() {
                        "active" => Style::default()
                            .fg(palette.success)
                            .bg(palette.capsule_ok_bg),
                        "configured" | "timer" | "cli" => Style::default()
                            .fg(palette.accent_primary)
                            .bg(palette.capsule_off_bg),
                        _ => Style::default()
                            .fg(palette.text_inactive)
                            .bg(palette.capsule_off_bg),
                    }
                };
                r0.push(Span::styled(" ", sp));
                r0.push(Span::styled(chip_txt, chip_style));
                used_w += 1 + chip_w;
            }
        }

        let expanded_layout = inner_h >= 11;
        // On compact height (e.g. Main card = 8..10 body rows), pack port chips right on Row 0.
        if !expanded_layout {
            for port in &ports_list {
                let p_txt = format!(" {port} ");
                let pw = visual_width(&p_txt);
                if used_w + 1 + pw <= usable_w {
                    let p_style = if palette.monochrome {
                        Style::default()
                    } else {
                        Style::default()
                            .fg(palette.spark_net)
                            .bg(palette.bg_inset)
                    };
                    r0.push(Span::styled(" ", sp));
                    r0.push(Span::styled(p_txt, p_style));
                    used_w += 1 + pw;
                }
            }
        }
        lines.push(Line::from(r0));

        // On expanded submenu Inspector (>= 12 rows total), dedicate a crisp chip row for ports + unit.
        if expanded_layout {
            let mut r_chips = vec![Span::styled(" ", sp)];
            let mut cw = 0usize;
            if !ports_list.is_empty() {
                r_chips.push(Span::styled(
                    pad_right_to_width("端口", key_w),
                    muted_style,
                ));
                cw += key_w;
                for (i, port) in ports_list.iter().enumerate() {
                    let p_txt = format!(" {port} ");
                    let pw = visual_width(&p_txt) + usize::from(i > 0);
                    if cw + pw > usable_w {
                        break;
                    }
                    if i > 0 {
                        r_chips.push(Span::styled(" ", sp));
                    }
                    let p_style = if palette.monochrome {
                        Style::default()
                    } else {
                        Style::default()
                            .fg(palette.spark_net)
                            .bg(palette.bg_inset)
                    };
                    r_chips.push(Span::styled(p_txt, p_style));
                    cw += pw;
                }
                if meta.unit != "-" && cw + 3 + visual_width(meta.unit) <= usable_w {
                    r_chips.push(Span::styled(" · ", sep_style));
                    r_chips.push(Span::styled(meta.unit, muted_style));
                }
            } else {
                push_kv_cell(
                    &mut r_chips,
                    "单元",
                    key_w,
                    meta.unit,
                    muted_style,
                    usable_w,
                    false,
                    card_bg,
                    palette,
                );
            }
            lines.push(Line::from(r_chips));
        }

        // Aligned 2-col KV facts from `ModuleDetail::kv_facts` (or fallback to `meta`).
        let facts: Vec<(String, String)> = if let Some(d) = detail
            && !d.kv_facts.is_empty()
        {
            d.kv_facts.iter().take(4).cloned().collect()
        } else {
            vec![
                ("单元".to_string(), meta.unit.to_string()),
                ("端口".to_string(), meta.ports.to_string()),
                ("类型".to_string(), meta.kind.to_string()),
            ]
        };

        if two_col {
            for pair in facts.chunks(2) {
                if lines.len() + 2 >= inner_h {
                    break;
                }
                let mut row = vec![Span::styled(" ", sp)];
                if let Some((lk, lv)) = pair.first() {
                    push_kv_cell(
                        &mut row,
                        lk,
                        key_w,
                        lv,
                        primary_style,
                        left_w,
                        true,
                        card_bg,
                        palette,
                    );
                }
                if let Some((rk, rv)) = pair.get(1) {
                    row.push(Span::styled(" │ ", sep_style));
                    push_kv_cell(
                        &mut row,
                        rk,
                        key_w,
                        rv,
                        primary_style,
                        right_w,
                        false,
                        card_bg,
                        palette,
                    );
                }
                lines.push(Line::from(row));
            }
        } else {
            for (k, v) in facts.iter().take(2) {
                if lines.len() + 2 >= inner_h {
                    break;
                }
                let mut row = vec![Span::styled(" ", sp)];
                push_kv_cell(
                    &mut row,
                    k,
                    key_w,
                    v,
                    primary_style,
                    usable_w,
                    false,
                    card_bg,
                    palette,
                );
                lines.push(Line::from(row));
            }
        }

        // Config path with `✓`/`○` existence marker
        if lines.len() < inner_h {
            let (cfg_path, cfg_exists) = match detail {
                Some(d) if !d.config_path.is_empty() => (d.config_path.as_str(), Some(d.config_exists)),
                _ => (meta.path, None),
            };
            let mut r_cfg = vec![
                Span::styled(" ", sp),
                Span::styled(pad_right_to_width("配置", key_w), muted_style),
            ];
            let budget = usable_w.saturating_sub(key_w);
            match cfg_exists {
                Some(true) => {
                    r_cfg.push(Span::styled(
                        "✓ ",
                        Style::default().fg(palette.success).bg(card_bg),
                    ));
                    r_cfg.push(Span::styled(
                        truncate_to_width(cfg_path, budget.saturating_sub(2)),
                        primary_style,
                    ));
                }
                Some(false) => {
                    r_cfg.push(Span::styled(
                        "○ ",
                        Style::default().fg(palette.text_inactive).bg(card_bg),
                    ));
                    r_cfg.push(Span::styled(
                        truncate_to_width(cfg_path, budget.saturating_sub(2)),
                        muted_style,
                    ));
                }
                None => {
                    r_cfg.push(Span::styled(truncate_to_width(cfg_path, budget), muted_style));
                }
            }
            lines.push(Line::from(r_cfg));
        }

        // Inset sanitized config preview (`bg_inset` + `▏` gutter); CLI command folded into `?` Help overlay (U31).
        if let Some(d) = detail
            && !d.config_excerpt.is_empty()
            && lines.len() < inner_h
        {
            if expanded_layout && lines.len() + 2 < inner_h {
                lines.push(telemetry_section_header(
                    "配置预览",
                    None,
                    usable_w,
                    card_bg,
                    palette,
                ));
            }
            let inset_bg = if palette.monochrome {
                Color::Reset
            } else {
                palette.bg_inset
            };
            let gutter_style = Style::default()
                .fg(palette.border_active)
                .bg(inset_bg);
            let code_style = Style::default()
                .fg(palette.text_muted)
                .bg(inset_bg);
            let avail = inner_h.saturating_sub(lines.len()).min(3);
            let code_w = usable_w.saturating_sub(2);
            for raw_line in d.config_excerpt.iter().take(avail) {
                let fitted = pad_right_to_width(&truncate_to_width(raw_line, code_w), code_w);
                lines.push(Line::from(vec![
                    Span::styled(" ", sp),
                    Span::styled("▏ ", gutter_style),
                    Span::styled(fitted, code_style),
                ]));
            }
        }
    } else {
        lines.push(Line::from(vec![
            Span::styled(" ", sp),
            Span::styled("无选中项", muted_style),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), body_area);
}

/// Renders restrained keycap chips that fit strictly inside `max_width` with zero mid-word clipping.
fn build_responsive_keycaps(
    chips: &[(&str, &str)],
    compact_chips: &[(&str, &str)],
    max_width: usize,
    bg: Color,
    palette: &UiPalette,
) -> Line<'static> {
    let sp = Style::default().bg(bg);
    let key_style = Style::default()
        .fg(palette.accent_primary)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(palette.text_muted).bg(bg);

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
                spans.push(Span::styled("  ", sp));
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

    let mut spans = Vec::new();
    let mut used = 0usize;
    let mut truncated = false;
    for &(k, l) in compact_chips {
        let sep = if spans.is_empty() { 0 } else { 2 };
        let chip_w = visual_width(k) + if l.is_empty() { 0 } else { 1 + visual_width(l) };
        let reserve = 2;
        if used + sep + chip_w <= max_width.saturating_sub(reserve) {
            if sep > 0 {
                spans.push(Span::styled("  ", sp));
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
        let minimal = truncate_to_width("[↑↓] [Enter] [Ctrl+Q]", max_width);
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
    hit_map: &mut UiHitMap,
) {
    let solid = palette.uses_solid_cards();
    let card_bg = if solid {
        palette.bg_surface
    } else {
        Color::Reset
    };
    let inner_area = if solid {
        fill_card_surface(frame, area, card_bg);
        area
    } else {
        area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        })
    };
    let inner_w = inner_area.width.saturating_sub(2) as usize;

    let mut show_help_chip = false;
    let (line, border_color) = if state.help_active() {
        let full = &[("?/F1/Esc", "关闭帮助"), ("Ctrl+Q", "退出")];
        let compact = &[("Esc", "关闭"), ("Ctrl+Q", "退出")];
        (
            build_responsive_keycaps(full, compact, inner_w, card_bg, palette),
            palette.border_active,
        )
    } else if state.palette_active() {
        let full = &[("↑↓", "选择"), ("Enter", "执行"), ("Esc", "关闭")];
        let compact = &[("Enter", "执行"), ("Esc", "关闭")];
        (
            build_responsive_keycaps(full, compact, inner_w, card_bg, palette),
            palette.border_active,
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
                Style::default()
                    .fg(color)
                    .bg(card_bg)
                    .add_modifier(Modifier::BOLD),
            )),
            color,
        )
    } else if state.filter_active() {
        let prefix = "/";
        let prefix_w = visual_width(prefix);
        let full_chips: &[(&str, &str)] = &[("Enter", "执行"), ("Esc", "清除")];
        let minimal_chips: &[(&str, &str)] = &[("Esc", "清除")];
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
            .bg(card_bg)
            .add_modifier(Modifier::BOLD);
        let label_style = Style::default().fg(palette.text_muted).bg(card_bg);
        let sp = Style::default().bg(card_bg);

        let mut spans = vec![Span::styled(format!("{prefix}{fitted_q}▏"), key_style)];
        for &(k, l) in chosen_chips {
            spans.push(Span::styled("  ", sp));
            spans.push(Span::styled(k.to_string(), key_style));
            spans.push(Span::styled(format!(" {l}"), label_style));
        }
        (Line::from(spans), palette.border_active)
    } else if let Some(prefix) = state.digit_prefix() {
        let full = format!("编号: {prefix}_  Enter 执行  0-9 续输");
        let compact = format!("编号: {prefix}_  Enter 执行");
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
                    .bg(card_bg)
                    .add_modifier(Modifier::BOLD),
            )),
            palette.border_active,
        )
    } else if show_inline_notice && let Some(notice) = state.notice() {
        let color = notice_color(state.notice_kind(), palette);
        let fitted = truncate_to_width(notice, inner_w);
        (
            Line::from(Span::styled(
                fitted,
                Style::default()
                    .fg(color)
                    .bg(card_bg)
                    .add_modifier(Modifier::BOLD),
            )),
            color,
        )
    } else {
        // U31 ultra-minimal footer: 3-4 concise core hints + clickable `? 帮助` chip.
        show_help_chip = true;
        let help_reserve = if inner_w >= 20 { 10usize } else { 0usize };
        let hint_budget = inner_w.saturating_sub(help_reserve);
        type KeycapSlice<'a> = &'a [(&'a str, &'a str)];
        let (full, compact): (KeycapSlice<'_>, KeycapSlice<'_>) =
            if split_mode && state.active_pane() == ActivePane::Telemetry {
                (
                    &[("↑↓", "滚动"), ("Tab", "菜单"), ("Ctrl+Q", "退出")],
                    &[("↑↓", "滚动"), ("Tab", "菜单")],
                )
            } else if state.current_menu() == MenuId::Main {
                if split_mode {
                    (
                        &[
                            ("↑↓", "选择"),
                            ("Enter", "执行"),
                            ("Ctrl+K", "搜索"),
                            ("Ctrl+Q", "退出"),
                        ],
                        &[("↑↓", "选择"), ("Enter", "执行"), ("Ctrl+Q", "退出")],
                    )
                } else {
                    (
                        &[("↑↓", "选择"), ("Enter", "执行"), ("Ctrl+Q", "退出")],
                        &[("↑↓", "选择"), ("Enter", "执行")],
                    )
                }
            } else {
                (
                    &[
                        ("↑↓", "选择"),
                        ("Enter", "执行"),
                        ("Esc", "返回"),
                        ("Ctrl+Q", "退出"),
                    ],
                    &[("Enter", "执行"), ("Esc", "返回"), ("Ctrl+Q", "退出")],
                )
            };
        (
            build_responsive_keycaps(full, compact, hint_budget, card_bg, palette),
            palette.border_inactive,
        )
    };

    if !solid {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color))
            .style(Style::default().bg(card_bg));
        frame.render_widget(block, area);
    }

    if inner_area.width < 4 || inner_area.height == 0 {
        return;
    }

    if show_help_chip && inner_w >= 20 {
        let help_chip = " ? 帮助 ";
        let chip_w = visual_width(help_chip);
        let left_w: usize = line
            .spans
            .iter()
            .map(|s| visual_width(s.content.as_ref()))
            .sum();
        let sp = Style::default().bg(card_bg);
        let chip_bg = if palette.monochrome {
            Color::Reset
        } else {
            palette.capsule_off_bg
        };
        let chip_style = if palette.monochrome {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(palette.accent_primary)
                .bg(chip_bg)
                .add_modifier(Modifier::BOLD)
        };
        let mut row_spans = vec![Span::styled(" ", sp)];
        row_spans.extend(line.spans);
        if left_w + 2 + chip_w <= inner_w {
            let gap = inner_w - left_w - chip_w;
            row_spans.push(Span::styled(" ".repeat(gap), sp));
            let chip_x = inner_area.x.saturating_add(1 + (left_w + gap) as u16);
            hit_map.help_chip_rect = Some(Rect::new(chip_x, inner_area.y, chip_w as u16, 1));
            row_spans.push(Span::styled(help_chip, chip_style));
        }
        frame.render_widget(Paragraph::new(Line::from(row_spans)), inner_area);
    } else {
        frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), inner_area);
    }
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
/// BEFORE `Clear` overwrites the overlay rectangle.
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

/// Renders a non-destructive floating toast banner strictly inside `anchor_area`.
fn render_toast(
    frame: &mut ratatui::Frame<'_>,
    anchor_area: Rect,
    full_area: Rect,
    message: &str,
    kind: NoticeKind,
    palette: &UiPalette,
    flat_modals: bool,
) {
    let solid = palette.uses_solid_cards();
    let bordered = !solid || !flat_modals;
    let inner = if solid {
        anchor_area
    } else {
        anchor_area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        })
    };
    let min_h = if bordered { 3 } else { 1 };
    if inner.width < 20 || inner.height < min_h {
        return;
    }

    let color = notice_color(kind, palette);
    let icon = notice_icon(kind);
    let tagged = format!("{icon} {message}");

    let max_toast_w = inner
        .width
        .saturating_sub(2)
        .max(20)
        .min(inner.width)
        .min(60) as usize;
    let min_toast_w = 20usize.min(max_toast_w);
    let chrome_w = 4usize;
    let desired_w = (visual_width(&tagged) + chrome_w).clamp(min_toast_w, max_toast_w);
    let toast_w = desired_w as u16;
    let text_w = toast_w.saturating_sub(chrome_w as u16).max(1) as usize;
    let mut wrapped = wrap_text_to_width(&tagged, text_w);
    let v_chrome = if bordered { 2u16 } else { 0u16 };
    let max_lines = inner.height.saturating_sub(v_chrome).max(1) as usize;
    if wrapped.len() > max_lines {
        wrapped.truncate(max_lines);
        if let Some(last) = wrapped.last_mut() {
            let trunc = truncate_to_width(last, text_w.saturating_sub(1));
            *last = format!("{trunc}…");
        }
    }
    let toast_h = (wrapped.len() as u16) + v_chrome;

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
    let text_style = Style::default()
        .fg(color)
        .bg(bg)
        .add_modifier(Modifier::BOLD);

    if bordered {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(color))
            .style(Style::default().bg(bg));
        let lines: Vec<Line<'static>> = wrapped
            .into_iter()
            .map(|line| Line::from(Span::styled(format!(" {line} "), text_style)))
            .collect();
        frame.render_widget(Paragraph::new(lines).block(block), toast_area);
    } else {
        fill_card_surface(frame, toast_area, bg);
        let lines: Vec<Line<'static>> = wrapped
            .into_iter()
            .map(|line| {
                Line::from(vec![
                    Span::styled("▎ ", text_style),
                    Span::styled(format!("{line} "), text_style),
                ])
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), toast_area);
    }
}

fn apply_dimmed_backdrop(frame: &mut ratatui::Frame<'_>, area: Rect, palette: &UiPalette) {
    let buf = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            if !palette.monochrome {
                cell.set_fg(palette.dim_backdrop_fg);
                // Preserve `Color::Reset` so translucent outer/gutter cells remain transparent.
                if cell.bg != Color::Reset {
                    cell.set_bg(palette.bg_inset);
                }
            }
            cell.set_style(cell.style().add_modifier(Modifier::DIM));
        }
    }
}

fn lerp_rgb(a: Color, b: Color, t: f64) -> Option<Color> {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let t = t.clamp(0.0, 1.0);
            let ch = |c1: u8, c2: u8| -> u8 {
                (f64::from(c1) + (f64::from(c2) - f64::from(c1)) * t).round() as u8
            };
            Some(Color::Rgb(ch(r1, r2), ch(g1, g2), ch(b1, b2)))
        }
        _ => None,
    }
}

/// Cubic-eased interior luminance fade for Menu transitions (`TabSwitch`, `MenuPush`, `MenuPop`).
///
/// CJK-safe, zero neon shimmer, zero border redraw.
fn apply_eased_menu_transition(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    kind: TransitionKind,
    eased: f64,
    palette: &UiPalette,
) {
    if area.width == 0 || area.height == 0 || eased >= 0.999 || palette.monochrome {
        return;
    }
    let buf = frame.buffer_mut();
    let w_denom = f64::from(area.width.saturating_sub(1).max(1));
    let h_denom = f64::from(area.height.saturating_sub(1).max(1));

    for y in area.top()..area.bottom() {
        let mut x = area.left();
        while x < area.right() {
            let sym_w = visual_width(buf[(x, y)].symbol()).max(1) as u16;
            let spatial = match kind {
                TransitionKind::TabSwitch { dir } if dir >= 0 => {
                    f64::from(x.saturating_sub(area.left())) / w_denom
                }
                TransitionKind::TabSwitch { .. } => {
                    f64::from(area.right().saturating_sub(1).saturating_sub(x)) / w_denom
                }
                TransitionKind::MenuPush => f64::from(y.saturating_sub(area.top())) / h_denom,
                TransitionKind::MenuPop | TransitionKind::ModalOpen => {
                    f64::from(area.bottom().saturating_sub(1).saturating_sub(y)) / h_denom
                }
            };
            let alpha = (eased * 1.35 - spatial * 0.35).clamp(0.0, 1.0);
            if alpha < 0.99 {
                let cell = &mut buf[(x, y)];
                let target_fg = if cell.fg == Color::Reset {
                    palette.text_primary
                } else {
                    cell.fg
                };
                if let Some(blended) = lerp_rgb(palette.dim_backdrop_fg, target_fg, alpha) {
                    cell.set_fg(blended);
                } else if alpha < 0.45 {
                    cell.set_fg(palette.dim_backdrop_fg);
                    cell.set_style(cell.style().add_modifier(Modifier::DIM));
                }
            }
            x = x.saturating_add(sym_w);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_confirm_modal(
    frame: &mut ratatui::Frame<'_>,
    body_area: Rect,
    full_area: Rect,
    message: &str,
    destructive: bool,
    palette: &UiPalette,
    flat_modals: bool,
    unfold_eased: f64,
    hit_map: &mut UiHitMap,
) {
    if full_area.width < 6 || full_area.height < 5 {
        return;
    }

    let solid = palette.uses_solid_cards();
    let bordered = !solid || !flat_modals;
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
    let inner_text_w = modal_width.saturating_sub(4).max(1) as usize;
    let msg_wrapped = wrap_text_to_width(message, inner_text_w);

    let modal_bg = if palette.monochrome {
        Color::Reset
    } else {
        palette.bg_toast
    };
    let yes_bg = if palette.monochrome {
        Color::Reset
    } else if destructive {
        palette.capsule_danger_bg
    } else {
        palette.capsule_warn_bg
    };
    let no_bg = if palette.monochrome {
        Color::Reset
    } else {
        palette.capsule_off_bg
    };

    let mut lines: Vec<Line<'static>> = Vec::new();
    let compact_v = full_area.height <= 14 || container.height < 9;
    let title_text = if destructive {
        "▲ 确认操作"
    } else {
        "◆ 确认操作"
    };
    if !bordered {
        lines.push(Line::from(Span::styled(
            truncate_to_width(title_text, inner_text_w),
            Style::default()
                .fg(modal_color)
                .bg(modal_bg)
                .add_modifier(Modifier::BOLD),
        )));
    }
    if !compact_v {
        lines.push(Line::from(""));
    }
    for row in &msg_wrapped {
        lines.push(Line::from(Span::styled(
            row.clone(),
            Style::default()
                .fg(modal_color)
                .bg(modal_bg)
                .add_modifier(Modifier::BOLD),
        )));
    }
    if !compact_v {
        let rule_w = inner_text_w.saturating_sub(4).clamp(6, 32);
        lines.push(Line::from(Span::styled(
            "─".repeat(rule_w),
            Style::default().fg(palette.border_inactive).bg(modal_bg),
        )));
    }

    let btn_line_idx = lines.len();
    let (yes_txt, gap_txt, no_txt) = if inner_text_w >= 32 {
        (" [y] 确认 ", "   ", " [n/Esc/Enter] 取消 ")
    } else if inner_text_w >= 24 {
        (" [y] 确认 ", "  ", " [n/Esc] 取消 ")
    } else {
        ("[y] 确认", " ", "[n] 取消")
    };
    let btn_row_w = visual_width(yes_txt) + visual_width(gap_txt) + visual_width(no_txt);
    if btn_row_w <= inner_text_w {
        lines.push(Line::from(vec![
            Span::styled(
                yes_txt,
                Style::default()
                    .fg(modal_color)
                    .bg(yes_bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(gap_txt, Style::default().bg(modal_bg)),
            Span::styled(
                no_txt,
                Style::default()
                    .fg(palette.text_primary)
                    .bg(no_bg),
            ),
        ]));
    } else {
        let short = truncate_to_width("[y] 确认 / [n] 取消", inner_text_w);
        lines.push(Line::from(Span::styled(
            short,
            Style::default().fg(palette.text_muted).bg(modal_bg),
        )));
    }

    let v_chrome = 2u16;
    let needed_h = (lines.len() as u16).saturating_add(v_chrome);
    let max_h = if bound_to_body {
        container.height
    } else {
        container
            .height
            .saturating_sub(2)
            .max(3)
            .min(container.height)
    };
    let target_h = needed_h.min(max_h);
    let min_unfold_h = 4u16.min(target_h);
    let modal_height = ((f64::from(target_h) * (0.45 + 0.55 * unfold_eased.clamp(0.0, 1.0)))
        .round() as u16)
        .clamp(min_unfold_h, target_h);

    let modal_x = full_area.x + full_area.width.saturating_sub(modal_width) / 2;
    let modal_y = container.y + container.height.saturating_sub(modal_height) / 2;
    let modal_area = Rect::new(modal_x, modal_y, modal_width, modal_height);

    hit_map.confirm_modal_rect = Some(modal_area);

    sanitize_overlay_boundaries(frame.buffer_mut(), modal_area, full_area);
    frame.render_widget(Clear, modal_area);

    let content_inner = if bordered {
        let max_title_w = modal_width.saturating_sub(6).max(4) as usize;
        let fitted_title = truncate_to_width(title_text, max_title_w);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(modal_color))
            .style(Style::default().bg(modal_bg))
            .title(Span::styled(
                format!(" {fitted_title} "),
                Style::default()
                    .fg(modal_color)
                    .add_modifier(Modifier::BOLD),
            ))
            .title_alignment(Alignment::Center);
        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);
        inner
    } else {
        fill_card_surface(frame, modal_area, modal_bg);
        modal_area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        })
    };

    // Record exact hit rects for `[y] 确认` and `[n/Esc/Enter] 取消` buttons.
    let btn_y = content_inner.y.saturating_add(btn_line_idx as u16);
    if btn_y < content_inner.bottom() && btn_row_w <= content_inner.width as usize {
        let start_x = content_inner.x
            + content_inner
                .width
                .saturating_sub(btn_row_w as u16)
                / 2;
        let yes_w = visual_width(yes_txt) as u16;
        let gap_w = visual_width(gap_txt) as u16;
        let no_w = visual_width(no_txt) as u16;
        hit_map.confirm_yes_rect = Some(Rect::new(start_x, btn_y, yes_w, 1));
        hit_map.confirm_no_rect = Some(Rect::new(start_x + yes_w + gap_w, btn_y, no_w, 1));
    }

    let paragraph = Paragraph::new(lines).alignment(Alignment::Center);
    frame.render_widget(paragraph, content_inner);
}

/// Renders the floating Global Fuzzy Command Palette (`Ctrl+K` / `Ctrl+P`) with U25 click hit-map
/// and U26 selective rounded vs flat modal A/B support.
#[allow(clippy::too_many_arguments)]
fn render_palette_modal(
    frame: &mut ratatui::Frame<'_>,
    body_area: Rect,
    full_area: Rect,
    state: &AppState,
    palette: &UiPalette,
    flat_modals: bool,
    unfold_eased: f64,
    hit_map: &mut UiHitMap,
) {
    if full_area.width < 20 || body_area.height < 6 {
        return;
    }

    let solid = palette.uses_solid_cards();
    let bordered = !solid || !flat_modals;

    let modal_w = ((full_area.width as usize) * 68 / 100)
        .clamp(46, 82)
        .min(full_area.width.saturating_sub(4) as usize) as u16;
    let max_h = body_area.height.saturating_sub(2).max(6);
    let target_h = max_h.min(18);
    let min_unfold_h = 6u16.min(target_h);
    let modal_h = ((f64::from(target_h) * (0.45 + 0.55 * unfold_eased.clamp(0.0, 1.0))).round()
        as u16)
        .clamp(min_unfold_h, target_h);

    let modal_x = full_area.x + full_area.width.saturating_sub(modal_w) / 2;
    let modal_y = body_area.y + body_area.height.saturating_sub(modal_h) / 2;
    let modal_area = Rect::new(modal_x, modal_y, modal_w, modal_h);

    hit_map.palette_modal_rect = Some(modal_area);

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
        palette.bg_inset
    };

    let entries = state.filtered_palette_entries();
    let total_entries = crate::menu::build_palette_entries(&state.status).len();
    let count_badge = format!("{}/{}", entries.len(), total_entries);

    let content_area = if bordered {
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(palette.border_active))
            .style(Style::default().bg(modal_bg))
            .title(Span::styled(
                " 命令面板 · Ctrl+K ",
                Style::default()
                    .fg(palette.text_primary)
                    .add_modifier(Modifier::BOLD),
            ));
        if modal_w >= 40 {
            block = block.title(
                Line::from(Span::styled(
                    format!(" {count_badge} "),
                    Style::default().fg(palette.text_muted),
                ))
                .alignment(Alignment::Right),
            );
        }
        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);
        inner
    } else {
        fill_card_surface(frame, modal_area, modal_bg);
        render_card_header_row(
            frame,
            Rect::new(modal_area.x, modal_area.y, modal_area.width, 1),
            "命令面板 · Ctrl+K",
            Some(&count_badge),
            true,
            modal_bg,
            palette,
        );
        Rect::new(
            modal_area.x,
            modal_area.y.saturating_add(1),
            modal_area.width,
            modal_area.height.saturating_sub(1),
        )
    };

    let inner_w = content_area.width as usize;
    let inner_h = content_area.height as usize;
    if inner_w < 8 || inner_h < 3 {
        return;
    }
    let usable_w = inner_w.saturating_sub(2).max(1);
    let sp = Style::default().bg(modal_bg);

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(inner_h);

    // Row 0: Search input bar
    let q_prefix = " ▸ ";
    let q_budget = inner_w.saturating_sub(visual_width(q_prefix) + 2);
    let fitted_q = tail_truncate_to_width(state.palette_query(), q_budget);
    let q_display = if fitted_q.is_empty() {
        "▏输入模块、命令或关键词...".to_string()
    } else {
        format!("{fitted_q}▏")
    };
    let q_cell_w = inner_w.saturating_sub(4);
    let q_fitted = pad_right_to_width(&truncate_to_width(&q_display, q_cell_w), q_cell_w);
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

    // Row 1: Hairline divider
    lines.push(Line::from(vec![
        Span::styled(" ", sp),
        Span::styled(
            "─".repeat(usable_w),
            Style::default().fg(palette.border_inactive).bg(modal_bg),
        ),
    ]));

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

    let list_start_y = content_area.y.saturating_add(2);

    if entries.is_empty() {
        let msg = truncate_to_width(" 无匹配命令  [Esc] 关闭", usable_w);
        lines.push(Line::from(vec![
            Span::styled(" ", sp),
            Span::styled(msg, Style::default().fg(palette.warning).bg(modal_bg)),
        ]));
    } else {
        for (rel_i, entry) in entries.iter().skip(scroll_start).take(list_h).enumerate() {
            let abs_i = scroll_start + rel_i;
            let row_y = list_start_y.saturating_add(rel_i as u16);
            hit_map
                .palette_row_rects
                .push((Rect::new(content_area.x, row_y, content_area.width, 1), abs_i));

            let is_sel = abs_i == sel_idx;
            let row_bg = if is_sel && !palette.monochrome {
                palette.bg_highlight
            } else {
                modal_bg
            };
            let row_sp = Style::default().bg(row_bg);
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
            let dom_pill = format!(" {} ", entry.domain);
            let dom_style = Style::default()
                .fg(palette.accent_secondary)
                .bg(dom_bg)
                .add_modifier(Modifier::BOLD);

            let label_col_w = 16usize;
            let fitted_label =
                pad_right_to_width(&truncate_to_width(entry.label, label_col_w), label_col_w);
            let label_style = if is_sel {
                Style::default()
                    .fg(palette.text_primary)
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
            let right_w = if status_txt.is_some() { 10usize } else { 3usize };
            let used_left = 1 + 1 + visual_width(&dom_pill) + 2 + label_col_w + 1;
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

            let mut row_spans = vec![
                Span::styled(pointer, ptr_style),
                Span::styled(" ", row_sp),
                Span::styled(dom_pill, dom_style),
                Span::styled("  ", row_sp),
                Span::styled(fitted_label, label_style),
                Span::styled(" ", row_sp),
                Span::styled(cli_str, cli_style),
            ];
            if let Some((ok, warn_off, lbl)) = status_txt {
                let (dot_fg, txt_fg, dot) = if ok {
                    (palette.success, palette.success, "●")
                } else if warn_off {
                    (palette.warning, palette.warning, "○")
                } else {
                    (palette.text_inactive, palette.text_inactive, "○")
                };
                row_spans.push(Span::styled(
                    format!(" {dot} "),
                    Style::default().fg(dot_fg).bg(row_bg),
                ));
                row_spans.push(Span::styled(
                    format!("{lbl} "),
                    Style::default().fg(txt_fg).bg(row_bg),
                ));
                row_spans.push(Span::styled(" ", row_sp));
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
            lines.push(Line::from(Span::styled("", sp)));
        }
        lines.push(Line::from(vec![
            Span::styled(" ", sp),
            Span::styled(
                "─".repeat(usable_w),
                Style::default().fg(palette.border_inactive).bg(modal_bg),
            ),
        ]));
        if let Some(sel) = entries.get(sel_idx) {
            let meta = sel.action.inspector_meta();
            let detail = format!("{}  │  {}", meta.unit, meta.path);
            let fitted = truncate_to_width(&detail, usable_w);
            lines.push(Line::from(vec![
                Span::styled(" ", sp),
                Span::styled(fitted, Style::default().fg(palette.text_muted).bg(modal_bg)),
            ]));
        }
    }

    frame.render_widget(Paragraph::new(lines), content_area);
}

/// Renders the floating `?` / `F1` Help & Keybindings overlay (U31), folding full keymap
/// and the currently selected item's equivalent CLI command out of the primary view.
#[allow(clippy::too_many_arguments)]
fn render_help_modal(
    frame: &mut ratatui::Frame<'_>,
    body_area: Rect,
    full_area: Rect,
    state: &AppState,
    palette: &UiPalette,
    flat_modals: bool,
    unfold_eased: f64,
    hit_map: &mut UiHitMap,
) {
    if full_area.width < 22 || body_area.height < 6 {
        return;
    }

    let solid = palette.uses_solid_cards();
    let bordered = !solid || !flat_modals;

    let modal_w = ((full_area.width as usize) * 72 / 100)
        .clamp(46, 84)
        .min(full_area.width.saturating_sub(4) as usize) as u16;
    let max_h = body_area.height.saturating_sub(2).max(6);
    let target_h = max_h.min(18);
    let min_unfold_h = 6u16.min(target_h);
    let modal_h = ((f64::from(target_h) * (0.45 + 0.55 * unfold_eased.clamp(0.0, 1.0))).round()
        as u16)
        .clamp(min_unfold_h, target_h);

    let modal_x = full_area.x + full_area.width.saturating_sub(modal_w) / 2;
    let modal_y = body_area.y + body_area.height.saturating_sub(modal_h) / 2;
    let modal_area = Rect::new(modal_x, modal_y, modal_w, modal_h);

    hit_map.help_modal_rect = Some(modal_area);

    sanitize_overlay_boundaries(frame.buffer_mut(), modal_area, full_area);
    frame.render_widget(Clear, modal_area);

    let modal_bg = if palette.monochrome {
        Color::Reset
    } else {
        palette.bg_toast
    };

    let content_area = if bordered {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(palette.border_active))
            .style(Style::default().bg(modal_bg))
            .title(Span::styled(
                " 帮助与快捷键 · ? / F1 ",
                Style::default()
                    .fg(palette.text_primary)
                    .add_modifier(Modifier::BOLD),
            ))
            .title(
                Line::from(Span::styled(
                    " Esc 关闭 ",
                    Style::default().fg(palette.text_muted),
                ))
                .alignment(Alignment::Right),
            );
        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);
        inner
    } else {
        fill_card_surface(frame, modal_area, modal_bg);
        render_card_header_row(
            frame,
            Rect::new(modal_area.x, modal_area.y, modal_area.width, 1),
            "帮助与快捷键 · ? / F1",
            Some("Esc 关闭"),
            true,
            modal_bg,
            palette,
        );
        Rect::new(
            modal_area.x,
            modal_area.y.saturating_add(1),
            modal_area.width,
            modal_area.height.saturating_sub(1),
        )
    };

    let inner_w = content_area.width as usize;
    let inner_h = content_area.height as usize;
    if inner_w < 10 || inner_h < 3 {
        return;
    }
    let usable_w = inner_w.saturating_sub(2).max(1);
    let sp = Style::default().bg(modal_bg);
    let primary_style = Style::default().fg(palette.text_primary).bg(modal_bg);
    let muted_style = Style::default().fg(palette.text_muted).bg(modal_bg);
    let accent_style = Style::default()
        .fg(palette.accent_primary)
        .bg(modal_bg)
        .add_modifier(Modifier::BOLD);
    let sep_style = Style::default().fg(palette.border_inactive).bg(modal_bg);

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(inner_h);

    // Section 1: Current Selected Item CLI Command & Path (folded out of main Inspector)
    let items = state.filtered_items();
    if let Some(sel) = items.get(state.selected_idx().min(items.len().saturating_sub(1))) {
        let meta = sel.action.inspector_meta();
        let detail = state
            .telemetry_specs()
            .module_details
            .get(sel.action.detail_key());
        let cfg_path = detail
            .filter(|d| !d.config_path.is_empty())
            .map(|d| d.config_path.as_str())
            .unwrap_or(meta.path);

        lines.push(telemetry_section_header(
            "当前项命令",
            Some(sel.label),
            usable_w,
            modal_bg,
            palette,
        ));

        let inset_bg = if palette.monochrome {
            Color::Reset
        } else {
            palette.bg_inset
        };
        let cmd_budget = usable_w.saturating_sub(2);
        let cmd_fitted = pad_right_to_width(&truncate_to_width(meta.cli, cmd_budget), cmd_budget);
        lines.push(Line::from(vec![
            Span::styled(" ", sp),
            Span::styled("▏ ", Style::default().fg(palette.accent_primary).bg(inset_bg)),
            Span::styled(
                cmd_fitted,
                Style::default()
                    .fg(palette.accent_primary)
                    .bg(inset_bg)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        let path_info = format!("单元 {}  ·  配置 {}", meta.unit, cfg_path);
        lines.push(Line::from(vec![
            Span::styled(" ", sp),
            Span::styled(truncate_to_width(&path_info, usable_w), muted_style),
        ]));
    }

    // Section 2: Complete Keybindings & Mouse Guide
    lines.push(telemetry_section_header(
        "完整快捷键与鼠标",
        None,
        usable_w,
        modal_bg,
        palette,
    ));

    let bindings: [(&str, &str, &str, &str); 7] = [
        ("↑↓ / j k", "移动选择 / 滚动", "←→ / h l / [ ]", "切换页签 / 进退菜单"),
        ("Enter", "执行或进入子菜单", "Esc / Backspace", "返回上级 / 关闭弹窗"),
        ("g / G", "跳转首项 / 末项", "0-9", "编号快速直达"),
        ("Ctrl+K / P", "全局命令搜索", "/ / Ctrl+F", "当前列表筛选"),
        ("Tab", "切换菜单 / 概览", "R", "刷新系统状态"),
        ("鼠标左键", "点击页签 / 菜单项", "鼠标滚轮", "滚动列表 / 概览"),
        ("? / F1", "打开或关闭帮助", "Ctrl+Q / q", "退出 debopty"),
    ];

    let two_col = usable_w >= 46;
    let key_col_w = 15usize;
    if two_col {
        let left_w = (usable_w.saturating_sub(3)) / 2;
        let right_w = usable_w.saturating_sub(3 + left_w);
        for (lk, ld, rk, rd) in bindings {
            if lines.len() >= inner_h {
                break;
            }
            let l_key = pad_right_to_width(&truncate_to_width(lk, key_col_w), key_col_w);
            let l_desc_w = left_w.saturating_sub(key_col_w);
            let l_desc = pad_right_to_width(&truncate_to_width(ld, l_desc_w), l_desc_w);
            let r_key = pad_right_to_width(&truncate_to_width(rk, key_col_w), key_col_w);
            let r_desc_w = right_w.saturating_sub(key_col_w);
            let r_desc = truncate_to_width(rd, r_desc_w);
            lines.push(Line::from(vec![
                Span::styled(" ", sp),
                Span::styled(l_key, accent_style),
                Span::styled(l_desc, primary_style),
                Span::styled(" │ ", sep_style),
                Span::styled(r_key, accent_style),
                Span::styled(r_desc, primary_style),
            ]));
        }
    } else {
        for (lk, ld, rk, rd) in bindings {
            for (k, d) in [(lk, ld), (rk, rd)] {
                if lines.len() >= inner_h {
                    break;
                }
                let kw = 13usize.min(usable_w / 2);
                let kf = pad_right_to_width(&truncate_to_width(k, kw), kw);
                let df = truncate_to_width(d, usable_w.saturating_sub(kw));
                lines.push(Line::from(vec![
                    Span::styled(" ", sp),
                    Span::styled(kf, accent_style),
                    Span::styled(df, primary_style),
                ]));
            }
        }
    }

    frame.render_widget(Paragraph::new(lines), content_area);
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

/// Deterministic rich sample `StatusSnapshot` for `debopty ui-preview --demo`.
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
        version_display: "v2.0.0".to_string(),
        is_cn_region: true,
        base_optimized: true,
        ip_forward: true,
        podman_gc_enabled: true,
        tools,
    }
}

/// Rich rendering options for `debopty ui-preview`.
#[derive(Debug, Clone, Default)]
pub struct ShowcaseOptions {
    pub ansi: bool,
    pub tick: u64,
    pub transition: u8,
    pub transition_ms: Option<u64>,
    pub selected: usize,
    pub filter: Option<String>,
    pub palette: Option<String>,
    pub tab: WorkspaceTab,
    pub pane: ActivePane,
    pub toast: Option<String>,
    pub show_confirm: bool,
    pub show_help: bool,
    pub animations: bool,
    pub flat_modals: bool,
    pub telemetry: Option<(SystemSpecs, TelemetryHistory)>,
}

/// Renders a single deterministic showcase frame with full U23/U24/U25/U26 options.
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
    let (specs, history) = opts
        .telemetry
        .clone()
        .unwrap_or_else(|| TelemetryHistory::demo_fixture(opts.tick as usize));
    state.set_telemetry(specs, history);

    state.navigate_to(menu);
    if menu == MenuId::Main && opts.tab != WorkspaceTab::Overview {
        state.set_workspace_tab(opts.tab);
    }
    state.set_split_pane_available(width >= SPLIT_PANE_MIN_WIDTH);
    state.set_active_pane(opts.pane);

    if let Some(ref q) = opts.filter {
        state.open_filter_preview(q.clone());
    }
    if opts.selected > 0 {
        state.set_selected_idx(opts.selected);
    }
    if let Some(ref pq) = opts.palette {
        state.open_palette_preview(pq.clone());
    }
    if let Some(ref toast_msg) = opts.toast {
        state.set_notice(toast_msg.clone());
    }
    if opts.show_help {
        state.open_help_preview();
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

    let has_transition = opts.transition_ms.is_some() || opts.transition > 0;
    if has_transition {
        state.set_animations_enabled(true);
        let raw_progress = if let Some(ms) = opts.transition_ms {
            (ms as f64 / (DEFAULT_TRANSITION_DURATION.as_millis() as f64).max(1.0)).clamp(0.0, 1.0)
        } else {
            let total = f64::from(LEGACY_TRANSITION_TICKS);
            (1.0 - f64::from(opts.transition.min(LEGACY_TRANSITION_TICKS)) / total).clamp(0.0, 1.0)
        };
        let kind = if opts.palette.is_some() || opts.show_confirm || opts.show_help {
            TransitionKind::ModalOpen
        } else if menu == MenuId::Main && opts.tab != WorkspaceTab::Overview {
            TransitionKind::TabSwitch { dir: 1 }
        } else {
            TransitionKind::MenuPush
        };
        state.set_transition_preview(kind, raw_progress);
    } else {
        state.set_animations_enabled(opts.animations);
        state.clear_transition();
    }

    let palette = UiPalette::truecolor();
    let mut hit_map = UiHitMap::default();
    let now = Instant::now();
    let _ = terminal.draw(|frame| {
        render_frame_internal(frame, &state, &palette, opts.flat_modals, &mut hit_map, now);
    });
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

/// Renders the canonical 5-screen showcase gallery.
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

    use crossterm::event::{
        KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent,
        MouseEventKind,
    };
    use debopty_core::status::{StatusSnapshot, TRACKED_TOOLS};
    use debopty_core::telemetry::TelemetryHistory;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    fn sample_status() -> StatusSnapshot {
        let mut tools = BTreeMap::new();
        for &tool in TRACKED_TOOLS {
            tools.insert(tool.to_string(), false);
        }
        tools.insert("fish".to_string(), true);
        tools.insert("podman".to_string(), true);
        tools.insert("xray".to_string(), true);

        StatusSnapshot {
            version_display: "v2.0.0".to_string(),
            is_cn_region: true,
            base_optimized: true,
            ip_forward: true,
            podman_gc_enabled: false,
            tools,
        }
    }

    fn demo_state() -> AppState {
        let mut state = AppState::new(sample_showcase_status());
        let (specs, history) = TelemetryHistory::demo_fixture(0);
        state.set_telemetry(specs, history);
        state
    }

    fn key_event(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind,
            state: KeyEventState::NONE,
        }
    }

    fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn u23_u26_palette_is_translucent_safe_flat_slate_with_zero_neon() {
        let tc = UiPalette::truecolor();
        let c256 = UiPalette::ansi256();
        let c16 = UiPalette::ansi16();
        let mono = UiPalette::monochrome();

        for p in [tc, c256, c16, mono] {
            assert_eq!(
                p.bg_base,
                Color::Reset,
                "outer canvas & gutters must inherit terminal translucency via Color::Reset"
            );
        }
        assert!(tc.uses_solid_cards());
        assert!(c256.uses_solid_cards());
        assert!(!c16.uses_solid_cards());
        assert!(!mono.uses_solid_cards());

        assert_eq!(tc.bg_surface, Color::Rgb(21, 27, 38));
        assert_eq!(tc.bg_surface_focused, Color::Rgb(25, 32, 46));
        assert_eq!(tc.bg_toast, Color::Rgb(29, 37, 52));
        assert_eq!(tc.bg_inset, Color::Rgb(16, 21, 31));
        assert_eq!(tc.bg_highlight, Color::Rgb(36, 46, 66));
        assert_eq!(tc.pill_active_bg, Color::Rgb(43, 55, 78));
        assert_eq!(tc.pill_active_fg, Color::Rgb(241, 245, 249));
        assert_eq!(tc.accent_primary, Color::Rgb(122, 162, 247));
        assert_eq!(tc.accent_secondary, Color::Rgb(154, 168, 190));
        assert_eq!(tc.border_active, Color::Rgb(78, 93, 120));
        assert_eq!(tc.border_inactive, Color::Rgb(36, 45, 62));
        assert_eq!(tc.text_primary, Color::Rgb(230, 237, 243));
        assert_eq!(tc.success, Color::Rgb(86, 184, 129));
        assert_eq!(tc.warning, Color::Rgb(210, 153, 34));
        assert_eq!(tc.danger, Color::Rgb(224, 108, 117));
        assert_eq!(tc.spark_net, Color::Rgb(115, 179, 200));

        assert_eq!(c16.text_primary, Color::Reset);
        assert_eq!(
            UiPalette::detect_from(false, Some("xterm-256color"), Some("truecolor")),
            tc
        );
        assert_eq!(
            UiPalette::detect_from(false, Some("xterm-256color"), None),
            c256
        );
        assert_eq!(UiPalette::detect_from(false, Some("xterm"), None), c16);
        assert!(UiPalette::detect_from(true, Some("xterm-256color"), Some("truecolor")).monochrome);
        assert!(UiPalette::detect_from(false, Some("dumb"), Some("truecolor")).monochrome);
    }

    #[test]
    fn u23_u26_wide_frame_has_translucent_gutters_zero_vanity_and_real_braille_telemetry() {
        let palette = UiPalette::truecolor();
        let state = demo_state();
        let mut terminal = Terminal::new(TestBackend::new(120, 36)).expect("test backend");
        terminal
            .draw(|frame| render_frame_with_palette(frame, &state, &palette))
            .expect("draw ok");

        let rendered = test_buffer_to_string(&terminal);
        // Zero vanity strings & zero box-border title clutter
        for forbidden in [
            "Pure Rust",
            "21/21 Native",
            "色彩",
            "引擎",
            "┤ ",
            " ├",
            "pure-rust",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "vanity/legacy token {forbidden:?} must be purged:\n{rendered}"
            );
        }

        // Real hardware, sysctl, SSH, firewall, Braille curves, and all 18 tools
        for expected in [
            "debopty",
            "deb-edge-01",
            "Debian 12",
            "功能菜单",
            "系统概览",
            "硬件与内核",
            "参数与安全",
            "实时负载",
            "已部署服务",
            "操作检视",
            "bbr",
            ":2222",
            "yazi",
            "usque",
            "easytier-core",
            "v2.0.0",
        ] {
            assert!(
                rendered.contains(expected),
                "missing expected U23 telemetry/UI content {expected:?}:\n{rendered}"
            );
        }

        // Verify genuine 2x4 Braille glyphs (U+2800..=U+28FF) appear in live load sparklines
        assert!(
            rendered
                .chars()
                .any(|c| ('\u{2800}'..='\u{28FF}').contains(&c)),
            "expected genuine Braille sparkline glyphs in rendered frame"
        );

        // Verify outer frame (y=0, y=35, x=0, x=119) & inter-card horizontal gap keep Color::Reset!
        let buf = terminal.backend().buffer();
        assert_eq!(buf[(0, 0)].bg, Color::Reset);
        assert_eq!(buf[(119, 35)].bg, Color::Reset);
        assert_eq!(buf[(1, 3)].bg, Color::Reset); // 1-row translucent gutter below header
        assert_eq!(buf[(1, 33)].bg, Color::Reset); // 1-row translucent gutter above footer
        // Header card interior has flat slate surface
        assert_eq!(buf[(2, 1)].bg, palette.bg_surface);
        // Focused Menu card uses bg_surface_focused
        assert_eq!(buf[(2, 4)].bg, palette.bg_surface_focused);
    }

    #[test]
    fn u23_zero_neon_pulse_across_static_frames() {
        let status = sample_showcase_status();
        let (specs, history) = TelemetryHistory::demo_fixture(0);
        let opts_a = ShowcaseOptions {
            ansi: true,
            animations: true,
            telemetry: Some((specs.clone(), history.clone())),
            ..ShowcaseOptions::default()
        };
        let opts_b = ShowcaseOptions {
            ansi: true,
            tick: 15,
            animations: true,
            telemetry: Some((specs, history)),
            ..ShowcaseOptions::default()
        };
        let frame_a = render_showcase_frame_with_options(MenuId::Main, &status, 120, 36, &opts_a);
        let frame_b = render_showcase_frame_with_options(MenuId::Main, &status, 120, 36, &opts_b);
        assert_eq!(
            frame_a, frame_b,
            "static frame must have zero pulsing/flickering neon color changes across ticks"
        );
    }

    #[test]
    fn u24_inspector_renders_pills_badges_kv_and_inset_config_preview() {
        let palette = UiPalette::truecolor();
        let mut state = demo_state();
        // Select Xray Core (index 6 on Main overview)
        state.set_selected_idx(6);
        let mut terminal = Terminal::new(TestBackend::new(120, 36)).expect("test backend");
        terminal
            .draw(|frame| render_frame_with_palette(frame, &state, &palette))
            .expect("draw ok");

        let rendered = test_buffer_to_string(&terminal);
        assert!(rendered.contains("操作检视"));
        assert!(rendered.contains("[7] ↵"));
        assert!(rendered.contains("Xray"));
        assert!(rendered.contains("▏ ")); // inset config excerpt gutter

        // Verify inset bg_inset cells exist in the Inspector card
        let buf = terminal.backend().buffer();
        let mut found_inset = false;
        for y in 24..33 {
            for x in 50..118 {
                if buf[(x, y)].bg == palette.bg_inset {
                    found_inset = true;
                    break;
                }
            }
        }
        assert!(found_inset, "U24 Inspector must render bg_inset config preview");

        // Submenu expands Inspector height to >= 12 rows
        let mut sub_state = demo_state();
        sub_state.navigate_to(MenuId::Devops);
        let mut hit_map = UiHitMap::default();
        let mut sub_term = Terminal::new(TestBackend::new(120, 36)).expect("test backend");
        sub_term
            .draw(|frame| {
                render_frame_internal(
                    frame,
                    &sub_state,
                    &palette,
                    false,
                    &mut hit_map,
                    Instant::now(),
                );
            })
            .expect("draw submenu ok");
        let insp_h = hit_map.inspector_area.expect("inspector area").height;
        assert!(
            insp_h >= 12,
            "submenu Inspector height must expand to >= 12 rows, got {insp_h}"
        );
    }

    #[test]
    fn u25_hit_test_mouse_handles_tabs_menu_panes_modals_scroll_and_ignores_hover() {
        let palette = UiPalette::truecolor();
        let state = demo_state();
        let mut hit_map = UiHitMap::default();
        let mut terminal = Terminal::new(TestBackend::new(120, 36)).expect("test backend");
        terminal
            .draw(|frame| {
                render_frame_internal(
                    frame,
                    &state,
                    &palette,
                    false,
                    &mut hit_map,
                    Instant::now(),
                );
            })
            .expect("draw ok");

        assert_eq!(hit_map.tab_rects.len(), WorkspaceTab::ALL.len());
        assert!(hit_map.cmd_chip_rect.is_some());
        assert!(!hit_map.menu_row_rects.is_empty());

        // Click second Workspace Tab -> ClickTab(SysSec)
        let (tab_rect, tab_kind) = hit_map.tab_rects[1];
        assert_eq!(
            hit_test_mouse(
                &hit_map,
                mouse_event(MouseEventKind::Down(MouseButton::Left), tab_rect.x, tab_rect.y),
                &state
            ),
            Some(MenuEvent::ClickTab(tab_kind))
        );

        // Click Ctrl+K chip -> PaletteOpen
        let chip = hit_map.cmd_chip_rect.expect("cmd chip");
        assert_eq!(
            hit_test_mouse(
                &hit_map,
                mouse_event(MouseEventKind::Down(MouseButton::Left), chip.x, chip.y),
                &state
            ),
            Some(MenuEvent::PaletteOpen)
        );

        // Click 3rd menu item -> ClickMenuItem(2)
        let (row_rect, row_idx) = hit_map.menu_row_rects[2];
        assert_eq!(
            hit_test_mouse(
                &hit_map,
                mouse_event(MouseEventKind::Down(MouseButton::Left), row_rect.x + 2, row_rect.y),
                &state
            ),
            Some(MenuEvent::ClickMenuItem(row_idx))
        );

        // Click Telemetry pane -> FocusPane(Telemetry)
        let tele = hit_map.telemetry_area.expect("telemetry area");
        assert_eq!(
            hit_test_mouse(
                &hit_map,
                mouse_event(MouseEventKind::Down(MouseButton::Left), tele.x + 2, tele.y + 2),
                &state
            ),
            Some(MenuEvent::FocusPane(ActivePane::Telemetry))
        );

        // Scroll wheel over Telemetry & Menu
        assert_eq!(
            hit_test_mouse(
                &hit_map,
                mouse_event(MouseEventKind::ScrollDown, tele.x + 2, tele.y + 2),
                &state
            ),
            Some(MenuEvent::ScrollTelemetry(2))
        );
        assert_eq!(
            hit_test_mouse(
                &hit_map,
                mouse_event(MouseEventKind::ScrollUp, row_rect.x + 1, row_rect.y),
                &state
            ),
            Some(MenuEvent::Up)
        );

        // Hover / Drag / Up MUST return None (zero SSH hover traffic)
        for kind in [
            MouseEventKind::Moved,
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            assert_eq!(
                hit_test_mouse(&hit_map, mouse_event(kind, row_rect.x, row_rect.y), &state),
                None
            );
        }

        // Confirm modal buttons & outside-dismiss
        let mut confirm_state = demo_state();
        confirm_state.open_confirm_preview(
            "移除 Podman 环境？ [y/N]",
            BashAction::UninstallApp(AppId::Podman),
        );
        let mut confirm_map = UiHitMap::default();
        terminal
            .draw(|frame| {
                render_frame_internal(
                    frame,
                    &confirm_state,
                    &palette,
                    false,
                    &mut confirm_map,
                    Instant::now(),
                );
            })
            .expect("draw confirm ok");
        let yes_r = confirm_map.confirm_yes_rect.expect("yes button");
        let no_r = confirm_map.confirm_no_rect.expect("no button");
        assert_eq!(
            hit_test_mouse(
                &confirm_map,
                mouse_event(MouseEventKind::Down(MouseButton::Left), yes_r.x, yes_r.y),
                &confirm_state
            ),
            Some(MenuEvent::ConfirmYes)
        );
        assert_eq!(
            hit_test_mouse(
                &confirm_map,
                mouse_event(MouseEventKind::Down(MouseButton::Left), no_r.x, no_r.y),
                &confirm_state
            ),
            Some(MenuEvent::ConfirmNo)
        );
        assert_eq!(
            hit_test_mouse(
                &confirm_map,
                mouse_event(MouseEventKind::Down(MouseButton::Left), 0, 0),
                &confirm_state
            ),
            Some(MenuEvent::ConfirmNo)
        );

        // Palette row click & outside-dismiss
        let mut pal_state = demo_state();
        pal_state.open_palette_preview("proxy");
        let mut pal_map = UiHitMap::default();
        terminal
            .draw(|frame| {
                render_frame_internal(
                    frame,
                    &pal_state,
                    &palette,
                    false,
                    &mut pal_map,
                    Instant::now(),
                );
            })
            .expect("draw palette ok");
        assert!(!pal_map.palette_row_rects.is_empty());
        let (pr, pi) = pal_map.palette_row_rects[0];
        assert_eq!(
            hit_test_mouse(
                &pal_map,
                mouse_event(MouseEventKind::Down(MouseButton::Left), pr.x + 2, pr.y),
                &pal_state
            ),
            Some(MenuEvent::ClickPaletteItem(pi))
        );
        assert_eq!(
            hit_test_mouse(
                &pal_map,
                mouse_event(MouseEventKind::Down(MouseButton::Left), 0, 0),
                &pal_state
            ),
            Some(MenuEvent::Back)
        );
    }

    #[test]
    fn u25_micro_yazi_keymap_normal_and_filtered_modes() {
        // Normal mode: Micro + Yazi bindings
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('q'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Quit)
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
                KeyCode::Char('p'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press
            )),
            Some(MenuEvent::PaletteOpen)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('p'),
                KeyModifiers::ALT,
                KeyEventKind::Press
            )),
            Some(MenuEvent::PaletteOpen)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('f'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press
            )),
            Some(MenuEvent::FilterStart)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Left,
                KeyModifiers::ALT,
                KeyEventKind::Press
            )),
            Some(MenuEvent::PrevTab)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Right,
                KeyModifiers::ALT,
                KeyEventKind::Press
            )),
            Some(MenuEvent::NextTab)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('j'),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Down)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('j'),
                KeyModifiers::NONE,
                KeyEventKind::Repeat
            )),
            Some(MenuEvent::Down)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('k'),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Up)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('h'),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::PrevTab)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('l'),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::NextTab)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('g'),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Home)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('G'),
                KeyModifiers::SHIFT,
                KeyEventKind::Press
            )),
            Some(MenuEvent::End)
        );

        // Filtered mode: Ctrl+Q quits, plain j/k/h/l/g/G/q type into query!
        assert_eq!(
            map_crossterm_key_filtered(key_event(
                KeyCode::Char('q'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press
            )),
            Some(MenuEvent::Quit)
        );
        for ch in ['j', 'k', 'h', 'l', 'g', 'G', 'q'] {
            assert_eq!(
                map_crossterm_key_filtered(key_event(
                    KeyCode::Char(ch),
                    KeyModifiers::NONE,
                    KeyEventKind::Press
                )),
                Some(MenuEvent::FilterChar(ch))
            );
        }
    }

    #[test]
    fn u23_cubic_easing_transition_progresses_smoothly_without_border_corruption() {
        let status = sample_showcase_status();
        let early = render_showcase_frame_with_options(
            MenuId::Main,
            &status,
            120,
            36,
            &ShowcaseOptions {
                ansi: true,
                tab: WorkspaceTab::NetProxy,
                transition_ms: Some(40),
                ..ShowcaseOptions::default()
            },
        );
        let settled = render_showcase_frame_with_options(
            MenuId::Main,
            &status,
            120,
            36,
            &ShowcaseOptions {
                ansi: true,
                tab: WorkspaceTab::NetProxy,
                transition_ms: Some(220),
                ..ShowcaseOptions::default()
            },
        );
        assert_ne!(
            early, settled,
            "cubic-eased mid-transition frame should differ in interior luminance from settled frame"
        );
    }

    #[test]
    fn u26_flat_and_rounded_modals_and_cjk_extreme_bounds_never_panic() {
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
                let (specs, history) = TelemetryHistory::demo_fixture(0);
                state.set_telemetry(specs, history);
                state.navigate_to(menu);
                state.set_animations_enabled(true);
                state.set_transition_preview(TransitionKind::MenuPush, 0.4);
                state.set_notice("稳态巡检通过 · 保留 3 项自定义");
                terminal
                    .draw(|frame| render_frame_with_palette(frame, &state, &UiPalette::truecolor()))
                    .unwrap_or_else(|e| panic!("draw failed for {menu:?} at {w}x{h}: {e}"));
                terminal
                    .draw(|frame| render_frame_with_palette(frame, &state, &UiPalette::monochrome()))
                    .unwrap_or_else(|e| panic!("mono draw failed for {menu:?} at {w}x{h}: {e}"));
            }

            for flat_modals in [false, true] {
                let rendered_pal = render_showcase_frame_with_options(
                    MenuId::Main,
                    &status,
                    w,
                    h,
                    &ShowcaseOptions {
                        palette: Some("proxy".to_string()),
                        flat_modals,
                        ..ShowcaseOptions::default()
                    },
                );
                if w >= 45 && h >= 16 {
                    for (row_idx, line) in rendered_pal.lines().enumerate() {
                        assert_eq!(
                            visual_width(line),
                            w as usize,
                            "palette row {row_idx} width mismatch at {w}x{h} flat={flat_modals}: {line:?}"
                        );
                    }
                }
            }
        }

        // Confirm modal & gallery checks
        let gallery = render_showcase_gallery(&sample_showcase_status(), 108, 30);
        assert_eq!(gallery.len(), 5);
        assert!(gallery[0].1.contains("系统概览"));
        assert!(!gallery[1].1.contains("系统概览"));
        assert!(gallery[2].1.contains("Fish"));
        assert!(gallery[3].1.contains("确认操作"));
        assert!(gallery[4].1.contains("维护管理"));
    }

    #[test]
    fn narrow_single_pane_hides_telemetry_and_tab_hint_cleanly() {
        let rendered = render_showcase_frame(MenuId::Main, &sample_showcase_status(), 80, 24, false);
        assert!(rendered.contains("debopty"));
        assert!(rendered.contains("功能菜单"));
        assert!(rendered.contains("基础优化"));
        assert!(!rendered.contains("系统概览"));
        assert!(!rendered.contains("[Tab]"));
        for (row_idx, line) in rendered.lines().enumerate() {
            assert_eq!(
                visual_width(line),
                80,
                "80x24 row {row_idx} width mismatch: {line:?}"
            );
        }
    }

    #[test]
    fn u31_help_overlay_folds_cli_commands_and_full_keymap_with_mouse_hit_map() {
        // Keymap: `?` and `F1` map to HelpToggle
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Char('?'),
                KeyModifiers::SHIFT,
                KeyEventKind::Press
            )),
            Some(MenuEvent::HelpToggle)
        );
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::F(1),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            Some(MenuEvent::HelpToggle)
        );

        let palette = UiPalette::truecolor();
        let mut state = demo_state();
        state.set_selected_idx(6); // Xray Core

        // Primary view hides CLI `debopty app ...` clutter & exposes clickable `? 帮助` chip
        let mut main_map = UiHitMap::default();
        let mut terminal = Terminal::new(TestBackend::new(120, 36)).expect("test backend");
        terminal
            .draw(|frame| {
                render_frame_internal(
                    frame,
                    &state,
                    &palette,
                    false,
                    &mut main_map,
                    Instant::now(),
                );
            })
            .expect("draw main ok");
        let main_rendered = test_buffer_to_string(&terminal);
        assert!(
            !main_rendered.contains("debopty app"),
            "U31 primary dashboard/inspector must fold `debopty app ...` CLI commands into Help modal:\n{main_rendered}"
        );
        assert!(main_rendered.contains("? 帮助"));
        let help_chip = main_map.help_chip_rect.expect("clickable ? help chip rect");
        assert_eq!(
            hit_test_mouse(
                &main_map,
                mouse_event(MouseEventKind::Down(MouseButton::Left), help_chip.x, help_chip.y),
                &state
            ),
            Some(MenuEvent::HelpToggle)
        );

        // Help modal renders current item CLI command + full keymap + outside-click dismiss
        state.open_help_preview();
        let mut help_map = UiHitMap::default();
        terminal
            .draw(|frame| {
                render_frame_internal(
                    frame,
                    &state,
                    &palette,
                    false,
                    &mut help_map,
                    Instant::now(),
                );
            })
            .expect("draw help ok");
        let help_rendered = test_buffer_to_string(&terminal);
        assert!(help_rendered.contains("帮助与快捷键"));
        assert!(help_rendered.contains("当前项命令"));
        assert!(help_rendered.contains("debopty app"));
        assert!(help_rendered.contains("Ctrl+Q / q"));
        assert!(help_rendered.contains("鼠标滚轮"));
        let modal_rect = help_map.help_modal_rect.expect("help modal rect");
        // Inside click does not leak through to menu
        assert_eq!(
            hit_test_mouse(
                &help_map,
                mouse_event(
                    MouseEventKind::Down(MouseButton::Left),
                    modal_rect.x + 2,
                    modal_rect.y + 2
                ),
                &state
            ),
            None
        );
        // Outside click dismisses Help modal
        assert_eq!(
            hit_test_mouse(
                &help_map,
                mouse_event(MouseEventKind::Down(MouseButton::Left), 0, 0),
                &state
            ),
            Some(MenuEvent::Back)
        );
        // ShowcaseOptions::show_help renders cleanly with exact width
        let preview_help = render_showcase_frame_with_options(
            MenuId::Main,
            &sample_showcase_status(),
            120,
            36,
            &ShowcaseOptions {
                selected: 6,
                show_help: true,
                ..ShowcaseOptions::default()
            },
        );
        assert!(preview_help.contains("帮助与快捷键"));
        for (idx, line) in preview_help.lines().enumerate() {
            assert_eq!(
                visual_width(line),
                120,
                "help preview row {idx} width mismatch: {line:?}"
            );
        }
    }
}
