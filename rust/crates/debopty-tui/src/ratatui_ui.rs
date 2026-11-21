use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::console_ui::is_non_interactive;
use crate::layout::{pad_right_to_width, truncate_to_width, visual_width};
use crate::menu::{MenuEntry, MenuItem, StatusBadge};
use crate::renderer::{MenuEvent, TuiRenderer};
use crate::state::AppState;
use crate::terminal_guard::{CrosstermOps, TerminalGuard};

/// Minimum terminal width required to display the side-by-side status telemetry panel.
const SPLIT_PANE_MIN_WIDTH: u16 = 90;

struct ActiveSession {
    // Field order matters: `terminal` drops before `guard` so `guard` restores
    // the alternate screen and raw mode last.
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    guard: TerminalGuard<CrosstermOps>,
}

/// Opt-in full-screen Ratatui renderer with lazy terminal initialization and RAII cleanup.
pub struct RatatuiRenderer {
    session: Option<ActiveSession>,
}

impl RatatuiRenderer {
    /// Creates a new `RatatuiRenderer` without touching terminal ioctls.
    ///
    /// Terminal raw mode and alternate screen are entered lazily on the first `draw()`
    /// call so constructing under `CI=true` or non-TTY environments is always safe.
    pub fn new() -> Self {
        Self { session: None }
    }

    fn ensure_session(&mut self) -> io::Result<&mut ActiveSession> {
        if self.session.is_none() {
            let guard = TerminalGuard::enter()?;
            let backend = CrosstermBackend::new(io::stdout());
            let terminal = Terminal::new(backend)?;
            self.session = Some(ActiveSession { terminal, guard });
        }
        Ok(self.session.as_mut().expect("session just initialized"))
    }
}

impl Default for RatatuiRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Maps a `crossterm::event::KeyEvent` to `MenuEvent`.
///
/// Filters to `KeyEventKind::Press` and supports standard navigation (`Up`/`Down`/`Enter`/
/// `Esc`/`Backspace`/`Ctrl+C`/`0..9`/`r`/`R`/`y`/`Y`/`n`/`N`), strictly excluding Vim-only
/// `h/j/k/l` bindings.
pub fn map_crossterm_key(key: KeyEvent) -> Option<MenuEvent> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'C'))
    {
        return Some(MenuEvent::Quit);
    }

    match key.code {
        KeyCode::Up => Some(MenuEvent::Up),
        KeyCode::Down => Some(MenuEvent::Down),
        KeyCode::Enter => Some(MenuEvent::Select),
        KeyCode::Esc | KeyCode::Backspace => Some(MenuEvent::Back),
        KeyCode::Char('r' | 'R') => Some(MenuEvent::Refresh),
        KeyCode::Char('y' | 'Y') => Some(MenuEvent::ConfirmYes),
        KeyCode::Char('n' | 'N') => Some(MenuEvent::ConfirmNo),
        KeyCode::Char(ch) if ch.is_ascii_digit() => Some(MenuEvent::Digit((ch as u8) - b'0')),
        _ => None,
    }
}

impl TuiRenderer for RatatuiRenderer {
    fn draw(&mut self, state: &AppState) -> io::Result<()> {
        let session = self.ensure_session()?;
        session.terminal.draw(|frame| render_frame(frame, state))?;
        Ok(())
    }

    fn read_event(&mut self) -> io::Result<MenuEvent> {
        loop {
            if let Event::Key(key) = event::read()?
                && let Some(mapped) = map_crossterm_key(key)
            {
                return Ok(mapped);
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

/// Pure, backend-agnostic Ratatui frame renderer for `AppState`.
pub fn render_frame(frame: &mut ratatui::Frame<'_>, state: &AppState) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }

    let vertical = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(3),
    ])
    .split(area);

    render_header(frame, vertical[0], state);

    if area.width >= SPLIT_PANE_MIN_WIDTH {
        let panes = Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
            .split(vertical[1]);
        render_menu_panel(frame, panes[0], state);
        render_status_panel(frame, panes[1], state);
    } else {
        render_menu_panel(frame, vertical[1], state);
    }

    render_footer(frame, vertical[2], state);

    if let Some(confirm) = state.pending_confirm() {
        render_confirm_modal(frame, area, &confirm.message);
    }
}

fn render_header(frame: &mut ratatui::Frame<'_>, area: Rect, state: &AppState) {
    let screen = state.current_screen();
    let title = Line::from(Span::styled(
        format!(" {} ", screen.title),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title)
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
        Style::default().fg(Color::Gray),
    )))
    .alignment(Alignment::Center)
    .block(block);

    frame.render_widget(paragraph, area);
}

fn render_menu_panel(frame: &mut ratatui::Frame<'_>, area: Rect, state: &AppState) {
    let screen = state.current_screen();
    let inner_w = area.width.saturating_sub(2) as usize;
    let divider_w = inner_w.saturating_sub(2).max(1);

    let mut list_items = Vec::with_capacity(screen.entries.len());
    let mut item_idx = 0usize;
    let mut selected_row = None;

    for (entry_idx, entry) in screen.entries.iter().enumerate() {
        match entry {
            MenuEntry::Separator => {
                let line = Line::from(Span::styled(
                    format!(" {}", "─".repeat(divider_w)),
                    Style::default().fg(Color::DarkGray),
                ));
                list_items.push(ListItem::new(line));
            }
            MenuEntry::Item(item) => {
                let is_selected = item_idx == state.selected_idx();
                if is_selected {
                    selected_row = Some(entry_idx);
                }
                list_items.push(ListItem::new(build_menu_line(item, is_selected, inner_w)));
                item_idx += 1;
            }
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " 功能菜单 ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));

    let list = List::new(list_items).block(block);
    let mut list_state = ListState::default();
    list_state.select(selected_row);
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn build_menu_line(item: &MenuItem, selected: bool, inner_width: usize) -> Line<'static> {
    let highlight = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let normal = Style::default().fg(Color::White);

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
                    Style::default().fg(Color::Green),
                    Style::default().fg(Color::Gray),
                    "●",
                    label,
                ),
                StatusBadge::Inactive(label) => (
                    Style::default().fg(Color::DarkGray),
                    Style::default().fg(Color::DarkGray),
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

fn render_status_panel(frame: &mut ratatui::Frame<'_>, area: Rect, state: &AppState) {
    let status = &state.status;
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
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Yellow)
    };
    let base_opt_label = if status.base_optimized {
        "● 已优化"
    } else {
        "○ 未执行"
    };
    let ip_fwd_style = if status.ip_forward {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let ip_fwd_label = if status.ip_forward {
        "● 已开启"
    } else {
        "○ 已关闭"
    };
    let podman_gc_style = if status.podman_gc_enabled {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let podman_gc_label = if status.podman_gc_enabled {
        "● 已开启"
    } else {
        "○ 已关闭"
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled("版本: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                status.version_display.clone(),
                Style::default().fg(Color::Cyan),
            ),
        ]),
        Line::from(vec![
            Span::styled("网络: ", Style::default().fg(Color::DarkGray)),
            Span::styled(region_label, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("基础优化: ", Style::default().fg(Color::DarkGray)),
            Span::styled(base_opt_label, base_opt_style),
        ]),
        Line::from(vec![
            Span::styled("路由转发: ", Style::default().fg(Color::DarkGray)),
            Span::styled(ip_fwd_label, ip_fwd_style),
        ]),
        Line::from(vec![
            Span::styled("Podman GC: ", Style::default().fg(Color::DarkGray)),
            Span::styled(podman_gc_label, podman_gc_style),
        ]),
        Line::from(vec![
            Span::styled("已部署工具: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{installed_tools}/{total_tools}"),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            "─".repeat(area.width.saturating_sub(4).max(1) as usize),
            Style::default().fg(Color::DarkGray),
        )),
    ];

    for (tool, &installed) in &status.tools {
        let (dot, dot_style, name_style, state_label) = if installed {
            (
                "●",
                Style::default().fg(Color::Green),
                Style::default().fg(Color::White),
                "已就绪",
            )
        } else {
            (
                "○",
                Style::default().fg(Color::DarkGray),
                Style::default().fg(Color::DarkGray),
                "未部署",
            )
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{dot} "), dot_style),
            Span::styled(pad_right_to_width(tool, 15), name_style),
            Span::styled(state_label, dot_style),
        ]));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " 系统遥测 ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_footer(frame: &mut ratatui::Frame<'_>, area: Rect, state: &AppState) {
    let (text, style, border_color) = if state.pending_confirm().is_some() {
        (
            "按 y 确认执行，按 n / Esc / Enter 取消".to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
            Color::Yellow,
        )
    } else if let Some(prefix) = state.digit_prefix() {
        (
            format!(">>> 已输入编号: {prefix}_ (按 Enter 执行 {prefix}，或继续输入第二位数字)"),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            Color::Cyan,
        )
    } else if let Some(notice) = state.notice() {
        (
            notice.to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
            Color::Yellow,
        )
    } else {
        (
            "↑/↓ 选择 | 数字键直达 | Enter 确认 | Esc/0 返回 | R 刷新".to_string(),
            Style::default().fg(Color::Gray),
            Color::DarkGray,
        )
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    let inner_w = area.width.saturating_sub(2) as usize;
    let fitted = truncate_to_width(&text, inner_w);
    let paragraph = Paragraph::new(Line::from(Span::styled(fitted, style)))
        .alignment(Alignment::Center)
        .block(block);

    frame.render_widget(paragraph, area);
}

fn render_confirm_modal(frame: &mut ratatui::Frame<'_>, area: Rect, message: &str) {
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
        .border_style(Style::default().fg(Color::Yellow))
        .title(Span::styled(
            " 确认操作 ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ))
        .title_alignment(Alignment::Center);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("? {message}"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "按 y 确认执行，按 n / Esc / Enter 取消",
            Style::default().fg(Color::Gray),
        )),
    ];

    let paragraph = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .block(block);

    frame.render_widget(paragraph, modal_area);
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

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
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

        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("debopty"));
        assert!(rendered.contains("中国大陆 (镜像加速)"));
        assert!(rendered.contains("一键系统级基础优化"));
        assert!(rendered.contains("系统遥测"));
        assert!(rendered.contains("已部署工具: 3/"));
        assert!(rendered.contains("已就绪"));
        assert!(rendered.contains("未部署"));
    }

    #[test]
    fn narrow_main_dashboard_uses_single_column_without_telemetry_panel() {
        let backend = TestBackend::new(64, 22);
        let mut terminal = Terminal::new(backend).expect("test backend");
        let state = AppState::new(sample_status());

        terminal
            .draw(|frame| render_frame(frame, &state))
            .expect("draw ok");

        let rendered = buffer_text(&terminal);
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

        let rendered = buffer_text(&terminal);
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
        let prefix_rendered = buffer_text(&terminal);
        assert!(prefix_rendered.contains("已输入编号: 1_"));

        // Clear prefix via Refresh to display yellow notice banner.
        assert!(state.handle_event(MenuEvent::Refresh).is_none());
        assert!(state.notice().is_some());
        terminal
            .draw(|frame| render_frame(frame, &state))
            .expect("draw notice ok");
        let notice_rendered = buffer_text(&terminal);
        assert!(notice_rendered.contains("已刷新状态显示。"));
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

        // Release and Repeat events must be ignored.
        assert_eq!(
            map_crossterm_key(key_event(
                KeyCode::Enter,
                KeyModifiers::NONE,
                KeyEventKind::Release
            )),
            None
        );

        // Vim-only navigation keys MUST NOT be bound.
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
