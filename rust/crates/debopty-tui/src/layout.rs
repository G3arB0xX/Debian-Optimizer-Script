use unicode_width::UnicodeWidthStr;

use crate::menu::MenuItem;

/// Default interior column width of the header box (between `│` borders).
pub const DEFAULT_BOX_INNER_WIDTH: usize = 54;

/// Strips ANSI CSI escape sequences (e.g. `\x1b[1;32m`) before measuring visual width.
pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for next_ch in chars.by_ref() {
                if ('@'..='~').contains(&next_ch) {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Computes exact terminal display column width for mixed ASCII, CJK, and emoji text.
pub fn visual_width(input: &str) -> usize {
    let clean = strip_ansi(input);
    UnicodeWidthStr::width(clean.as_str())
}

/// Truncates `text` cleanly to at most `max_width` terminal columns without splitting UTF-8
/// or exceeding `max_width` on FE0F emoji variation sequences and CJK wide characters.
pub fn truncate_to_width(text: &str, max_width: usize) -> String {
    if visual_width(text) <= max_width {
        return text.to_string();
    }
    let clean = strip_ansi(text);
    let mut out = String::new();
    for ch in clean.chars() {
        out.push(ch);
        if UnicodeWidthStr::width(out.as_str()) > max_width {
            out.pop();
            break;
        }
    }
    out
}

/// Pads `text` on the right with ASCII spaces to reach at least `target_width` display columns.
pub fn pad_right_to_width(text: &str, target_width: usize) -> String {
    let current = visual_width(text);
    if current >= target_width {
        text.to_string()
    } else {
        format!("{text}{}", " ".repeat(target_width - current))
    }
}

/// Centers `text` inside an exact `inner_width` visual column span.
pub fn center_in_width(text: &str, inner_width: usize) -> String {
    let fitted = if visual_width(text) > inner_width {
        truncate_to_width(text, inner_width)
    } else {
        text.to_string()
    };
    let w = visual_width(&fitted);
    let pad_total = inner_width.saturating_sub(w);
    let pad_left = pad_total / 2;
    let pad_right = pad_total - pad_left;
    format!(
        "{}{}{}",
        " ".repeat(pad_left),
        fitted,
        " ".repeat(pad_right)
    )
}

/// Renders a header box whose every row has identical visual width `inner_width + 2`.
pub fn render_header_box(title: &str, subtitle: Option<&str>, inner_width: usize) -> Vec<String> {
    let bar = "─".repeat(inner_width);
    let mut lines = Vec::with_capacity(4);
    lines.push(format!("┌{bar}┐"));
    lines.push(format!("│{}│", center_in_width(title, inner_width)));
    if let Some(sub) = subtitle
        && !sub.is_empty()
    {
        lines.push(format!("│{}│", center_in_width(sub, inner_width)));
    }
    lines.push(format!("└{bar}┘"));
    lines
}

/// Renders a horizontal separator aligned to the box width.
pub fn render_separator(inner_width: usize) -> String {
    format!(" {}", "─".repeat(inner_width))
}

/// Renders a single menu row with exact visual column alignment (plain text, no ANSI).
pub fn render_menu_item_plain(item: &MenuItem, selected: bool, inner_width: usize) -> String {
    let pointer = if selected { "▸" } else { " " };
    let prefix = format!("{pointer} {:>2}. ", item.id);
    let icon_cell = pad_right_to_width(item.icon, 3);
    let left_part = format!("{prefix}{icon_cell}{}", item.label);

    match item.status {
        Some(badge) => {
            let badge_text = badge.plain_text();
            let badge_w = visual_width(&badge_text);
            let max_left = inner_width.saturating_sub(badge_w + 1);
            let fitted_left = if visual_width(&left_part) > max_left {
                truncate_to_width(&left_part, max_left)
            } else {
                left_part
            };
            let padded_left = pad_right_to_width(&fitted_left, max_left);
            format!("{padded_left} {badge_text}")
        }
        None => left_part,
    }
}
