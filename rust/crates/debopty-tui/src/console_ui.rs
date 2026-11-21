use std::io::{self, IsTerminal};

use console::{Key, Style, Term};

use crate::layout::{
    DEFAULT_BOX_INNER_WIDTH, center_in_width, pad_right_to_width, render_separator, visual_width,
};
use crate::menu::{MenuEntry, MenuItem, StatusBadge};
use crate::renderer::{MenuEvent, TuiRenderer};
use crate::state::AppState;

/// Returns `true` when `CI` is non-empty or stdin is not attached to a terminal.
pub fn is_non_interactive() -> bool {
    is_non_interactive_with(
        std::env::var("CI").ok().as_deref(),
        io::stdin().is_terminal(),
    )
}

/// Pure helper for deterministic unit testing of non-interactive detection.
pub fn is_non_interactive_with(ci_env: Option<&str>, stdin_is_tty: bool) -> bool {
    ci_env.is_some_and(|v| !v.is_empty()) || !stdin_is_tty
}

/// Console renderer backed by `console` and `unicode-width`.
pub struct ConsoleRenderer {
    term: Term,
    inner_width: usize,
    cursor_hidden: bool,
}

impl ConsoleRenderer {
    pub fn new() -> Self {
        Self::with_width(DEFAULT_BOX_INNER_WIDTH)
    }

    pub fn with_width(inner_width: usize) -> Self {
        Self {
            term: Term::stdout(),
            inner_width: inner_width.max(44),
            cursor_hidden: false,
        }
    }

    fn format_styled_item(&self, item: &MenuItem, selected: bool) -> String {
        let cyan_bold = Style::new().cyan().bold();
        let dim = Style::new().dim();
        let green = Style::new().green();

        let pointer = if selected {
            cyan_bold.apply_to("▸").to_string()
        } else {
            " ".to_string()
        };
        let id_text = if selected {
            cyan_bold.apply_to(format!("{:>2}.", item.id)).to_string()
        } else {
            format!("{:>2}.", item.id)
        };
        let icon_cell = pad_right_to_width(item.icon, 3);
        let label_text = if selected {
            cyan_bold.apply_to(item.label).to_string()
        } else {
            item.label.to_string()
        };

        let left = format!("{pointer} {id_text} {icon_cell}{label_text}");
        match item.status {
            Some(badge) => {
                let badge_plain = badge.plain_text();
                let badge_w = visual_width(&badge_plain);
                let status_col = self.inner_width.saturating_sub(badge_w).max(38);
                let padded_left = pad_right_to_width(&left, status_col);
                let styled_badge = match badge {
                    StatusBadge::Active(text) => {
                        format!("{} {}", green.apply_to("●"), dim.apply_to(text))
                    }
                    StatusBadge::Inactive(text) => dim.apply_to(format!("○ {text}")).to_string(),
                };
                format!("{padded_left} {styled_badge}")
            }
            None => left,
        }
    }
}

impl Default for ConsoleRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ConsoleRenderer {
    fn drop(&mut self) {
        if self.cursor_hidden {
            let _ = self.term.show_cursor();
            self.cursor_hidden = false;
        }
    }
}

/// Maps `console::Key` to `MenuEvent` (arrows + digits + Enter + Esc/0; zero Vim-only bindings).
pub fn map_key_to_event(key: Key) -> Option<MenuEvent> {
    match key {
        Key::ArrowUp => Some(MenuEvent::Up),
        Key::ArrowDown => Some(MenuEvent::Down),
        Key::Enter => Some(MenuEvent::Select),
        Key::Escape | Key::Backspace => Some(MenuEvent::Back),
        Key::CtrlC => Some(MenuEvent::Quit),
        Key::Char('r' | 'R') => Some(MenuEvent::Refresh),
        Key::Char('y' | 'Y') => Some(MenuEvent::ConfirmYes),
        Key::Char('n' | 'N') => Some(MenuEvent::ConfirmNo),
        Key::Char(ch) if ch.is_ascii_digit() => Some(MenuEvent::Digit((ch as u8) - b'0')),
        _ => None,
    }
}

impl TuiRenderer for ConsoleRenderer {
    fn draw(&mut self, state: &AppState) -> io::Result<()> {
        let screen = state.current_screen();
        let cyan = Style::new().cyan();
        let bold = Style::new().bold();
        let dim = Style::new().dim();
        let yellow = Style::new().yellow().bold();

        if !self.cursor_hidden && self.term.is_term() {
            let _ = self.term.hide_cursor();
            self.cursor_hidden = true;
        }

        self.term.clear_screen()?;

        let bar = "─".repeat(self.inner_width);
        self.term
            .write_line(&format!("{}", cyan.apply_to(format!("┌{bar}┐"))))?;
        self.term.write_line(&format!(
            "{}{}{}",
            cyan.apply_to("│"),
            bold.apply_to(center_in_width(&screen.title, self.inner_width)),
            cyan.apply_to("│")
        ))?;
        if !screen.subtitle.is_empty() {
            self.term.write_line(&format!(
                "{}{}{}",
                cyan.apply_to("│"),
                dim.apply_to(center_in_width(&screen.subtitle, self.inner_width)),
                cyan.apply_to("│")
            ))?;
        }
        self.term
            .write_line(&format!("{}", cyan.apply_to(format!("└{bar}┘"))))?;

        let mut item_idx = 0usize;
        for entry in &screen.entries {
            match entry {
                MenuEntry::Separator => {
                    self.term.write_line(&format!(
                        "{}",
                        dim.apply_to(render_separator(self.inner_width))
                    ))?;
                }
                MenuEntry::Item(item) => {
                    let is_selected = item_idx == state.selected_idx();
                    self.term
                        .write_line(&self.format_styled_item(item, is_selected))?;
                    item_idx += 1;
                }
            }
        }
        self.term.write_line(&format!(
            "{}",
            dim.apply_to(render_separator(self.inner_width))
        ))?;

        if let Some(confirm) = state.pending_confirm() {
            self.term.write_line("")?;
            self.term.write_line(&format!(
                " {} {}",
                yellow.apply_to("?"),
                yellow.apply_to(&confirm.message)
            ))?;
            self.term.write_line(&format!(
                " {}",
                dim.apply_to("按 y 确认执行，按 n / Esc / Enter 取消")
            ))?;
        } else {
            if let Some(notice) = state.notice() {
                self.term
                    .write_line(&format!(" {}", yellow.apply_to(notice)))?;
            }
            if let Some(prefix) = state.digit_prefix() {
                self.term.write_line(&format!(
                    " >>> 已输入编号: {prefix}_ (按 Enter 执行 {prefix}，或继续输入第二位数字)"
                ))?;
            } else {
                self.term.write_line(&format!(
                    " {}",
                    dim.apply_to("↑/↓ 选择 | 数字键直达 | Enter 确认 | Esc/0 返回 | R 刷新")
                ))?;
            }
        }

        self.term.flush()
    }

    fn read_event(&mut self) -> io::Result<MenuEvent> {
        loop {
            let key = self.term.read_key()?;
            if let Some(event) = map_key_to_event(key) {
                return Ok(event);
            }
        }
    }

    fn suspend_for_child(&mut self) -> io::Result<()> {
        if self.cursor_hidden {
            let _ = self.term.show_cursor();
            self.cursor_hidden = false;
        }
        self.term.write_line("")?;
        self.term.flush()
    }

    fn resume_after_child(&mut self) -> io::Result<()> {
        self.term.flush()
    }

    fn is_non_interactive(&self) -> bool {
        is_non_interactive()
    }
}
