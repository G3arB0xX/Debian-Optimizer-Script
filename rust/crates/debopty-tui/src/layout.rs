use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
/// or leaving broken FE0F / combining-sequence base glyphs on overflow.
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
            // If a variation selector / ZWJ / combining mark widened the previous base char
            // past `max_width`, pop the preceding base glyph too so we never emit a broken half-emoji.
            if ch == '\u{FE0F}' || ch == '\u{200D}' || UnicodeWidthChar::width(ch) == Some(0) {
                while let Some(prev) = out.pop() {
                    if prev != '\u{FE0F}'
                        && prev != '\u{200D}'
                        && UnicodeWidthChar::width(prev) != Some(0)
                    {
                        break;
                    }
                }
            }
            break;
        }
    }
    out
}

/// Splits a plain string into atomic grapheme clusters (keeping FE0F/ZWJ/combining marks attached).
fn split_grapheme_clusters(text: &str) -> Vec<String> {
    let mut clusters = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        let mut cluster = String::new();
        cluster.push(ch);
        while let Some(&next) = chars.peek() {
            if next == '\u{FE0F}' || next == '\u{200D}' || UnicodeWidthChar::width(next) == Some(0)
            {
                cluster.push(next);
                chars.next();
            } else {
                break;
            }
        }
        clusters.push(cluster);
    }
    clusters
}

/// Tail-truncates `text` to at most `max_width` terminal columns with a leading `…` so active
/// input cursors (e.g., narrow filter queries) always keep the newest typed characters visible.
pub fn tail_truncate_to_width(text: &str, max_width: usize) -> String {
    if visual_width(text) <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let clean = strip_ansi(text);
    let clusters = split_grapheme_clusters(&clean);
    let budget = max_width - 1;
    let mut kept: Vec<&str> = Vec::new();
    let mut used = 0usize;
    for cluster in clusters.iter().rev() {
        let w = UnicodeWidthStr::width(cluster.as_str());
        if used + w > budget {
            break;
        }
        kept.push(cluster.as_str());
        used += w;
    }
    kept.reverse();
    let mut out = String::with_capacity(max_width + 3);
    out.push('…');
    for part in kept {
        out.push_str(part);
    }
    out
}

/// Truncates `text` to at most `max_width` display columns, appending a single-column `…`
/// when truncation occurs so truncated labels never collide silently with adjacent values.
pub fn truncate_with_ellipsis(text: &str, max_width: usize) -> String {
    if visual_width(text) <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    format!("{}…", truncate_to_width(text, max_width - 1))
}

/// Tokenizes a single line into wrap units: whitespace runs, contiguous ASCII non-whitespace tokens
/// (so `[y/N]`, `Enter`, `Podman` never split mid-word when they fit on a line), and individual
/// non-ASCII grapheme clusters (CJK/emoji).
fn tokenize_wrap_units(raw_line: &str) -> Vec<String> {
    let clusters = split_grapheme_clusters(raw_line);
    let mut tokens: Vec<String> = Vec::new();
    let mut idx = 0usize;
    while idx < clusters.len() {
        let is_ws = clusters[idx].chars().all(char::is_whitespace);
        if is_ws {
            let mut ws = String::new();
            while idx < clusters.len() && clusters[idx].chars().all(char::is_whitespace) {
                ws.push_str(&clusters[idx]);
                idx += 1;
            }
            tokens.push(ws);
        } else if clusters[idx].len() == 1 && clusters[idx].is_ascii() {
            let mut word = String::new();
            while idx < clusters.len()
                && clusters[idx].len() == 1
                && clusters[idx].is_ascii()
                && !clusters[idx].chars().all(char::is_whitespace)
            {
                word.push_str(&clusters[idx]);
                idx += 1;
            }
            tokens.push(word);
        } else {
            tokens.push(clusters[idx].clone());
            idx += 1;
        }
    }
    tokens
}

/// Wraps `text` cleanly into lines whose visual width never exceeds `max_width` columns,
/// preferring whitespace/CJK boundaries before breaking ASCII tokens (CJK-safe, emoji-safe).
pub fn wrap_text_to_width(text: &str, max_width: usize) -> Vec<String> {
    let max_w = max_width.max(1);
    let clean = strip_ansi(text);
    let mut lines: Vec<String> = Vec::new();

    for raw_line in clean.split('\n') {
        if raw_line.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut current = String::new();
        for token in tokenize_wrap_units(raw_line) {
            let is_ws = token.chars().all(char::is_whitespace);
            if current.is_empty() && is_ws {
                continue;
            }
            let mut candidate = current.clone();
            candidate.push_str(&token);
            if UnicodeWidthStr::width(candidate.as_str()) <= max_w {
                current = candidate;
                continue;
            }
            if !current.is_empty() {
                lines.push(current.trim_end().to_string());
                current = String::new();
            }
            if is_ws {
                continue;
            }
            if UnicodeWidthStr::width(token.as_str()) <= max_w {
                current.push_str(&token);
            } else {
                // Single token exceeds `max_w`: fall back to cluster-level wrapping.
                for cluster in split_grapheme_clusters(&token) {
                    let mut next_cand = current.clone();
                    next_cand.push_str(&cluster);
                    if UnicodeWidthStr::width(next_cand.as_str()) <= max_w {
                        current = next_cand;
                    } else {
                        if !current.is_empty() {
                            lines.push(current.trim_end().to_string());
                            current = String::new();
                        }
                        if UnicodeWidthStr::width(cluster.as_str()) <= max_w {
                            current.push_str(&cluster);
                        } else {
                            lines.push(truncate_to_width(&cluster, max_w));
                        }
                    }
                }
            }
        }
        if !current.is_empty() || lines.is_empty() {
            lines.push(current.trim_end().to_string());
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
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

/// Renders a labeled section divider aligned to `inner_width` (e.g. ` ── 系统 ──────────`).
pub fn render_section_divider(label: &str, inner_width: usize) -> String {
    let prefix = format!("── {label} ");
    let prefix_w = visual_width(&prefix);
    if prefix_w >= inner_width {
        return format!(" {}", truncate_to_width(&prefix, inner_width));
    }
    let tail = "─".repeat(inner_width - prefix_w);
    format!(" {prefix}{tail}")
}

/// Renders a single menu row with exact tabular column alignment (plain text, no ANSI).
pub fn render_menu_item_plain(item: &MenuItem, selected: bool, inner_width: usize) -> String {
    let pointer = if selected { "▸" } else { " " };
    let left_part = format!("{pointer} {:>2} │ {}", item.id, item.label);

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
        None => {
            if visual_width(&left_part) > inner_width {
                truncate_to_width(&left_part, inner_width)
            } else {
                left_part
            }
        }
    }
}
