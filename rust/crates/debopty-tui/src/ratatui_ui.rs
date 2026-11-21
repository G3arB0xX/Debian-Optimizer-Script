use std::collections::BTreeMap;
use std::env;
use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use debopty_bridge::{AppId, BashAction};
use debopty_core::status::{StatusSnapshot, TRACKED_TOOLS};
use ratatui::Terminal;
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
};

use crate::console_ui::is_non_interactive;
use crate::layout::{pad_right_to_width, truncate_to_width, visual_width};
use crate::menu::{MenuEntry, MenuId, MenuItem, StatusBadge};
use crate::renderer::{MenuEvent, TuiRenderer};
use crate::state::{ActivePane, AppState};
use crate::terminal_guard::{CrosstermOps, TerminalGuard};

/// Minimum terminal width required to display the side-by-side status telemetry panel.
const SPLIT_PANE_MIN_WIDTH: u16 = 90;
/// 30fps tick cadence (~33ms) for zero-bloat buffer effects when animations are enabled.
const TICK_INTERVAL: Duration = Duration::from_millis(33);
/// Braille spinner frames for live engine heartbeat.
const BRAILLE_SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// Braille bar glyphs (bottom-to-top fill) for micro-sparklines.
const BRAILLE_LEVELS: &[char] = &['⣀', '⣄', '⣤', '⣦', '⣶', '⣷', '⣿'];

/// Centralized color & aesthetic palette for U4/U13 Ratatui art-design tuning.
///
/// Supports TrueColor (`#0f1117` / `#181825` deep dark surface + electric accents),
/// graceful 16-color ANSI fallback, and strict monochrome degradation under `NO_COLOR`/`TERM=dumb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiPalette {
    pub monochrome: bool,
    pub bg_base: Color,
    pub bg_surface: Color,
    pub accent_primary: Color,
    pub accent_secondary: Color,
    pub border_active: Color,
    pub border_inactive: Color,
    pub title_badge_fg: Color,
    pub text_primary: Color,
    pub text_muted: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
    pub modal_border: Color,
    pub dim_backdrop_fg: Color,
}

impl UiPalette {
    /// Deep-dark TrueColor palette inspired by `btop`/`lazygit`/`yazi`/`k9s`.
    pub const fn truecolor() -> Self {
        Self {
            monochrome: false,
            bg_base: Color::Rgb(15, 17, 23),             // #0f1117
            bg_surface: Color::Rgb(24, 24, 37),          // #181825
            accent_primary: Color::Rgb(56, 189, 248),    // Electric Cyan #38bdf8
            accent_secondary: Color::Rgb(129, 140, 248), // Indigo Glow #818cf8
            border_active: Color::Rgb(56, 189, 248),
            border_inactive: Color::Rgb(71, 85, 105), // Slate #475569
            title_badge_fg: Color::Rgb(125, 211, 252),
            text_primary: Color::Rgb(241, 245, 249),
            text_muted: Color::Rgb(148, 163, 184),
            success: Color::Rgb(74, 222, 128), // Emerald #4ade80
            warning: Color::Rgb(251, 191, 36), // Amber #fbbf24
            danger: Color::Rgb(248, 113, 113), // Rose #f87171
            modal_border: Color::Rgb(251, 191, 36),
            dim_backdrop_fg: Color::Rgb(71, 85, 105),
        }
    }

    /// Graceful ANSI 16-color fallback when TrueColor is not advertised.
    pub const fn ansi16() -> Self {
        Self {
            monochrome: false,
            bg_base: Color::Reset,
            bg_surface: Color::Reset,
            accent_primary: Color::Cyan,
            accent_secondary: Color::Blue,
            border_active: Color::Cyan,
            border_inactive: Color::DarkGray,
            title_badge_fg: Color::Cyan,
            text_primary: Color::White,
            text_muted: Color::Gray,
            success: Color::Green,
            warning: Color::Yellow,
            danger: Color::Red,
            modal_border: Color::Yellow,
            dim_backdrop_fg: Color::DarkGray,
        }
    }

    /// Strict monochrome palette for `NO_COLOR` or `TERM=dumb`.
    pub const fn monochrome() -> Self {
        Self {
            monochrome: true,
            bg_base: Color::Reset,
            bg_surface: Color::Reset,
            accent_primary: Color::Reset,
            accent_secondary: Color::Reset,
            border_active: Color::Reset,
            border_inactive: Color::Reset,
            title_badge_fg: Color::Reset,
            text_primary: Color::Reset,
            text_muted: Color::Reset,
            success: Color::Reset,
            warning: Color::Reset,
            danger: Color::Reset,
            modal_border: Color::Reset,
            dim_backdrop_fg: Color::Reset,
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
            Self::truecolor()
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

/// Opt-in full-screen Ratatui renderer with lazy terminal initialization, U13 aesthetics,
/// 30fps tick motion polish, and RAII cleanup.
pub struct RatatuiRenderer {
    session: Option<ActiveSession>,
    palette: UiPalette,
    animations: bool,
    filter_active: bool,
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
/// for `Up`/`Down` arrow navigation. Rejects `Release`, non-arrow `Repeat`, Vim-only `h/j/k/l`,
/// and `CONTROL`/`ALT` modifiers on plain character bindings (except `Ctrl+C` -> `Quit`).
pub fn map_crossterm_key(key: KeyEvent) -> Option<MenuEvent> {
    match key.kind {
        KeyEventKind::Press => {}
        KeyEventKind::Repeat if matches!(key.code, KeyCode::Up | KeyCode::Down) => {}
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
        KeyCode::Enter if !has_ctrl_or_alt => Some(MenuEvent::Select),
        KeyCode::Esc | KeyCode::Backspace if !has_ctrl_or_alt => Some(MenuEvent::Back),
        KeyCode::Tab if !has_ctrl_or_alt => Some(MenuEvent::TogglePane),
        KeyCode::Char('/') if !has_ctrl_or_alt => Some(MenuEvent::FilterStart),
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
            && matches!(key.code, KeyCode::Up | KeyCode::Down | KeyCode::Backspace)
        {
            // allow repeating arrows and backspace while typing filter
        } else {
            return None;
        }
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'C'))
    {
        return Some(MenuEvent::Quit);
    }
    let has_ctrl_or_alt = key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    if has_ctrl_or_alt {
        return None;
    }

    match key.code {
        KeyCode::Up => Some(MenuEvent::Up),
        KeyCode::Down => Some(MenuEvent::Down),
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
        let palette = self.palette;
        let session = self.ensure_session()?;
        session
            .terminal
            .draw(|frame| render_frame_with_palette(frame, state, &palette))?;
        Ok(())
    }

    fn read_event(&mut self) -> io::Result<MenuEvent> {
        loop {
            if self.animations && !event::poll(TICK_INTERVAL)? {
                return Ok(MenuEvent::Tick);
            }
            match event::read()? {
                Event::Key(key) => {
                    let mapped = if self.filter_active {
                        map_crossterm_key_filtered(key)
                    } else {
                        map_crossterm_key(key)
                    };
                    if let Some(ev) = mapped {
                        return Ok(ev);
                    }
                }
                Event::Resize(_, _) => return Ok(MenuEvent::Resize),
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

    render_header(frame, vertical[0], state, palette);

    if area.width >= SPLIT_PANE_MIN_WIDTH {
        let panes = Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(vertical[1]);
        render_menu_panel(frame, panes[0], state, palette, true);
        render_status_panel(frame, panes[1], state, palette);
    } else {
        render_menu_panel(frame, vertical[1], state, palette, false);
    }

    render_footer(frame, vertical[2], state, palette);

    if state.animations_enabled() && state.transition_ticks() > 0 && !palette.monochrome {
        apply_wipe_transition(frame, vertical[1], state.transition_ticks(), palette);
    }

    if let Some(confirm) = state.pending_confirm() {
        apply_dimmed_backdrop(frame, area, palette);
        render_confirm_modal(frame, area, &confirm.message, palette);
    }
}

fn active_border_color(state: &AppState, palette: &UiPalette) -> Color {
    if palette.monochrome {
        return Color::Reset;
    }
    if state.animations_enabled() && (state.tick_count() / 15) % 2 == 1 {
        palette.accent_secondary
    } else {
        palette.border_active
    }
}

fn title_badge(text: &str, active: bool, palette: &UiPalette) -> Line<'static> {
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
    Line::from(vec![
        Span::styled("┤ ", Style::default().fg(border_fg)),
        Span::styled(text.to_string(), text_style),
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
        .title(title_badge(&screen.title, true, palette))
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
    let divider_w = inner_w.saturating_sub(2).max(1);
    let filter_query = state.filter_query();
    let filtering = !filter_query.is_empty();

    let mut list_items = Vec::with_capacity(screen.entries.len());
    let mut item_idx = 0usize;
    let mut selected_row = None;

    if filtering {
        let matched = state.filtered_items();
        if matched.is_empty() {
            list_items.push(ListItem::new(Line::from(Span::styled(
                format!(" 无匹配项: \"{filter_query}\" (按 Esc 清除筛选)"),
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
                    inner_w,
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
                        inner_w,
                        palette,
                    )));
                    item_idx += 1;
                }
            }
        }
    }

    let panel_title = if state.filter_active() {
        format!("功能菜单 / {filter_query}▏")
    } else if filtering {
        format!("功能菜单 [筛选: {filter_query}]")
    } else {
        "功能菜单".to_string()
    };

    let border_color = if is_focused {
        active_border_color(state, palette)
    } else {
        palette.border_inactive
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(palette.bg_surface))
        .title(title_badge(&panel_title, is_focused, palette));

    let list = List::new(list_items).block(block);
    let mut list_state = ListState::default();
    list_state.select(selected_row);
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn build_menu_line(
    item: &MenuItem,
    selected: bool,
    inner_width: usize,
    palette: &UiPalette,
) -> Line<'static> {
    let highlight = if palette.monochrome {
        Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default()
            .fg(palette.accent_primary)
            .add_modifier(Modifier::BOLD)
    };
    let normal = Style::default().fg(palette.text_primary);

    let pointer = if selected { "▸" } else { " " };
    let id_text = format!("{:>2}.", item.id);
    let icon_cell = pad_right_to_width(item.icon, 3);
    let left_plain = format!("{pointer} {id_text} {icon_cell}{}", item.label);

    match item.status {
        Some(badge) => {
            let badge_plain = badge.plain_text();
            let badge_w = visual_width(&badge_plain);
            let max_left_w = inner_width.saturating_sub(badge_w + 2);
            let fitted_left = if visual_width(&left_plain) > max_left_w {
                truncate_to_width(&left_plain, max_left_w)
            } else {
                left_plain
            };
            let padded_left = pad_right_to_width(&fitted_left, max_left_w);

            let (dot_style, label_style, dot, text) = match badge {
                StatusBadge::Active(label) => (
                    Style::default().fg(palette.success),
                    Style::default().fg(palette.text_muted),
                    "●",
                    label,
                ),
                StatusBadge::Inactive(label) => (
                    Style::default()
                        .fg(palette.border_inactive)
                        .add_modifier(Modifier::DIM),
                    Style::default()
                        .fg(palette.border_inactive)
                        .add_modifier(Modifier::DIM),
                    "○",
                    label,
                ),
            };

            Line::from(vec![
                Span::styled(padded_left, if selected { highlight } else { normal }),
                Span::raw(" "),
                Span::styled(dot, dot_style),
                Span::raw(" "),
                Span::styled(text, label_style),
            ])
        }
        None => {
            let fitted = truncate_to_width(&left_plain, inner_width);
            Line::from(Span::styled(
                fitted,
                if selected { highlight } else { normal },
            ))
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
    let total_tools = status.tools.len();
    let installed_tools = status
        .tools
        .values()
        .filter(|&&installed| installed)
        .count();

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
        Style::default()
            .fg(palette.border_inactive)
            .add_modifier(Modifier::DIM)
    };
    let ip_fwd_label = if status.ip_forward {
        "● 已开启"
    } else {
        "○ 已关闭"
    };
    let podman_gc_style = if status.podman_gc_enabled {
        Style::default().fg(palette.success)
    } else {
        Style::default()
            .fg(palette.border_inactive)
            .add_modifier(Modifier::DIM)
    };
    let podman_gc_label = if status.podman_gc_enabled {
        "● 已开启"
    } else {
        "○ 已关闭"
    };

    let spinner = if state.animations_enabled() {
        BRAILLE_SPINNER[(state.tick_count() as usize) % BRAILLE_SPINNER.len()]
    } else {
        "✦"
    };
    let spark_slots = (area.width.saturating_sub(24) as usize).clamp(6, 12);
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

    let mut lines = vec![
        Line::from(vec![
            Span::styled("引擎: ", Style::default().fg(palette.text_muted)),
            Span::styled(
                format!("{spinner} Pure Rust (21/21)"),
                Style::default()
                    .fg(palette.success)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("版本: ", Style::default().fg(palette.text_muted)),
            Span::styled(
                status.version_display.clone(),
                Style::default().fg(palette.accent_primary),
            ),
        ]),
        Line::from(vec![
            Span::styled("网络: ", Style::default().fg(palette.text_muted)),
            Span::styled(region_label, Style::default().fg(palette.text_primary)),
        ]),
        Line::from(vec![
            Span::styled("基础优化: ", Style::default().fg(palette.text_muted)),
            Span::styled(base_opt_label, base_opt_style),
        ]),
        Line::from(vec![
            Span::styled("路由转发: ", Style::default().fg(palette.text_muted)),
            Span::styled(ip_fwd_label, ip_fwd_style),
        ]),
        Line::from(vec![
            Span::styled("Podman GC: ", Style::default().fg(palette.text_muted)),
            Span::styled(podman_gc_label, podman_gc_style),
        ]),
        Line::from(vec![
            Span::styled("已部署工具: ", Style::default().fg(palette.text_muted)),
            Span::styled(
                format!("{installed_tools}/{total_tools} "),
                Style::default()
                    .fg(palette.accent_primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(sparkline, Style::default().fg(spark_color)),
        ]),
        Line::from(Span::styled(
            "─".repeat(area.width.saturating_sub(4).max(1) as usize),
            Style::default().fg(palette.border_inactive),
        )),
    ];

    for (tool, &installed) in &status.tools {
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
                Style::default()
                    .fg(palette.border_inactive)
                    .add_modifier(Modifier::DIM),
                Style::default()
                    .fg(palette.border_inactive)
                    .add_modifier(Modifier::DIM),
                "未部署",
            )
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{dot} "), dot_style),
            Span::styled(pad_right_to_width(tool, 15), name_style),
            Span::styled(state_label, dot_style),
        ]));
    }

    let border_color = if is_focused {
        active_border_color(state, palette)
    } else {
        palette.border_inactive
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(palette.bg_surface))
        .title(title_badge("系统遥测", is_focused, palette));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_footer(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &AppState,
    palette: &UiPalette,
) {
    let (text, style, border_color) = if state.pending_confirm().is_some() {
        (
            "[y] 确认执行  [n/Esc/Enter] 取消".to_string(),
            Style::default()
                .fg(palette.warning)
                .add_modifier(Modifier::BOLD),
            palette.warning,
        )
    } else if state.filter_active() {
        (
            format!(
                "筛选模式: /{}▏  [Enter] 执行首项  [Backspace] 退格  [Esc] 清除",
                state.filter_query()
            ),
            Style::default()
                .fg(palette.accent_primary)
                .add_modifier(Modifier::BOLD),
            palette.accent_primary,
        )
    } else if let Some(prefix) = state.digit_prefix() {
        (
            format!(">>> 已输入编号: {prefix}_ (按 Enter 执行 {prefix}，或继续输入第二位数字)"),
            Style::default()
                .fg(palette.accent_primary)
                .add_modifier(Modifier::BOLD),
            palette.accent_primary,
        )
    } else if let Some(notice) = state.notice() {
        (
            notice.to_string(),
            Style::default()
                .fg(palette.warning)
                .add_modifier(Modifier::BOLD),
            palette.warning,
        )
    } else {
        (
            "[↑↓] 选择  [0-9] 直达  [Enter] 执行  [/] 筛选  [Tab] 面板  [R] 刷新  [Esc] 返回"
                .to_string(),
            Style::default().fg(palette.text_muted),
            palette.border_inactive,
        )
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(palette.bg_surface));
    let inner_w = area.width.saturating_sub(2) as usize;
    let fitted = truncate_to_width(&text, inner_w);
    let paragraph = Paragraph::new(Line::from(Span::styled(fitted, style)))
        .alignment(Alignment::Center)
        .block(block);

    frame.render_widget(paragraph, area);
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

fn apply_wipe_transition(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    ticks_left: u8,
    palette: &UiPalette,
) {
    let fraction = (4u16.saturating_sub(u16::from(ticks_left.min(4)))) as usize;
    let reveal_x = area.left() + ((area.width as usize * fraction) / 4) as u16;
    let buf = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in reveal_x..area.right() {
            let cell = &mut buf[(x, y)];
            cell.set_fg(palette.dim_backdrop_fg);
            cell.set_style(cell.style().add_modifier(Modifier::DIM));
        }
    }
}

fn render_confirm_modal(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    message: &str,
    palette: &UiPalette,
) {
    let modal_width = ((area.width as usize) * 65 / 100)
        .clamp(36, 72)
        .min(area.width as usize) as u16;
    let modal_height = 7u16.min(area.height);
    let modal_x = area.x + area.width.saturating_sub(modal_width) / 2;
    let modal_y = area.y + area.height.saturating_sub(modal_height) / 2;
    let modal_area = Rect::new(modal_x, modal_y, modal_width, modal_height);

    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.modal_border))
        .style(Style::default().bg(palette.bg_surface))
        .title(Line::from(vec![
            Span::styled("┤ ", Style::default().fg(palette.modal_border)),
            Span::styled(
                "确认操作",
                Style::default()
                    .fg(palette.warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ├", Style::default().fg(palette.modal_border)),
        ]))
        .title_alignment(Alignment::Center);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("? {message}"),
            Style::default()
                .fg(palette.warning)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "按 y 确认执行，按 n / Esc / Enter 取消",
            Style::default().fg(palette.text_muted),
        )),
    ];

    let paragraph = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .block(block);

    frame.render_widget(paragraph, modal_area);
}

/// Converts a `TestBackend` terminal buffer into a deterministic CJK-width-aligned string.
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

/// Deterministic rich sample `StatusSnapshot` for `debopty ui-preview --demo` and U4 art review.
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

/// Renders a single deterministic non-TTY showcase frame via `TestBackend` for U4 visual review.
pub fn render_showcase_frame(
    menu: MenuId,
    status: &StatusSnapshot,
    width: u16,
    height: u16,
    show_confirm: bool,
) -> String {
    let backend = TestBackend::new(width, height);
    let Ok(mut terminal) = Terminal::new(backend) else {
        return String::new();
    };
    let mut state = AppState::new(status.clone());
    state.navigate_to(menu);
    if show_confirm {
        let (msg, action) = match menu {
            MenuId::Podman => (
                "确定要移除 Podman 环境吗？[y/N]".to_string(),
                BashAction::UninstallApp(AppId::Podman),
            ),
            MenuId::Maintenance => (
                "确定要彻底卸载 debopty 吗？[y/N]".to_string(),
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
    test_buffer_to_string(&terminal)
}

/// Renders the canonical 5-screen U4 showcase gallery (`main-wide`, `main-narrow`, `devops`,
/// `podman-confirm`, `maintenance`).
pub fn render_showcase_gallery(
    status: &StatusSnapshot,
    width: u16,
    height: u16,
) -> Vec<(String, String)> {
    let wide_w = width.max(100);
    let narrow_w = width.clamp(40, 64);
    let narrow_h = height.clamp(16, 24);
    vec![
        (
            "main-wide".to_string(),
            render_showcase_frame(MenuId::Main, status, wide_w, height, false),
        ),
        (
            "main-narrow".to_string(),
            render_showcase_frame(MenuId::Main, status, narrow_w, narrow_h, false),
        ),
        (
            "devops".to_string(),
            render_showcase_frame(MenuId::Devops, status, wide_w, height, false),
        ),
        (
            "podman-confirm".to_string(),
            render_showcase_frame(MenuId::Podman, status, wide_w, height, true),
        ),
        (
            "maintenance".to_string(),
            render_showcase_frame(MenuId::Maintenance, status, wide_w, height, false),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use debopty_core::status::{StatusSnapshot, TRACKED_TOOLS};
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
    }

    #[test]
    fn narrow_main_dashboard_uses_single_column_without_telemetry_panel() {
        let backend = TestBackend::new(64, 22);
        let mut terminal = Terminal::new(backend).expect("test backend");
        let state = AppState::new(sample_status());

        terminal
            .draw(|frame| render_frame(frame, &state))
            .expect("draw ok");

        let rendered = test_buffer_to_string(&terminal);
        assert!(rendered.contains("debopty"));
        assert!(rendered.contains("功能菜单"));
        assert!(rendered.contains("一键系统级基础优化"));
        assert!(!rendered.contains("系统遥测"));
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
    fn digit_prefix_and_notice_banners_render_in_footer() {
        let backend = TestBackend::new(96, 26);
        let mut terminal = Terminal::new(backend).expect("test backend");
        let mut state = AppState::new(sample_status());

        // Trigger digit prefix (`1` can be 1, 10, 11, 12).
        assert!(state.handle_event(MenuEvent::Digit(1)).is_none());
        assert_eq!(state.digit_prefix(), Some(1));
        terminal
            .draw(|frame| render_frame(frame, &state))
            .expect("draw prefix ok");
        let prefix_rendered = test_buffer_to_string(&terminal);
        assert!(prefix_rendered.contains("已输入编号: 1_"));

        // Clear prefix via Refresh to display yellow notice banner and trigger re-probe outcome.
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
    }

    #[test]
    fn inline_fuzzy_filter_and_pane_toggle_work_smoothly() {
        let backend = TestBackend::new(108, 28);
        let mut terminal = Terminal::new(backend).expect("test backend");
        let mut state = AppState::new(sample_status());

        assert_eq!(state.active_pane(), ActivePane::Menu);
        state.handle_event(MenuEvent::TogglePane);
        assert_eq!(state.active_pane(), ActivePane::Telemetry);
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

        state.handle_event(MenuEvent::FilterClear);
        assert!(!state.filter_active());
        assert!(state.filter_query().is_empty());
    }

    #[test]
    fn palette_detection_and_animation_degradation_honor_no_color_and_dumb_term() {
        assert_eq!(
            UiPalette::detect_from(false, Some("xterm-256color"), Some("truecolor")),
            UiPalette::truecolor()
        );
        assert_eq!(
            UiPalette::detect_from(false, Some("xterm-256color"), None),
            UiPalette::ansi16()
        );
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
            (89, 24),
            (90, 24),
            (200, 60),
        ];

        for &(w, h) in &extreme_sizes {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test backend");
            for menu in all_menus {
                let mut state = AppState::new(status.clone());
                state.navigate_to(menu);
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
}
