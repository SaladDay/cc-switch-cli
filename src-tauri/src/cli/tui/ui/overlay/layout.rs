use super::super::theme;
use super::super::*;

pub(crate) fn compact_message_overlay_rect(content_area: Rect, title: &str, message: &str) -> Rect {
    compact_lines_overlay_rect(content_area, title, &[message.to_string()])
}

pub(crate) fn should_use_compact_lines_overlay(
    content_area: Rect,
    title: &str,
    lines: &[String],
) -> bool {
    if lines.is_empty() || lines.len() > 8 {
        return false;
    }

    let area = compact_lines_overlay_rect(content_area, title, lines);
    area.width < content_area.width.saturating_sub(6) && area.height <= 13
}

pub(crate) fn compact_lines_overlay_rect(
    content_area: Rect,
    title: &str,
    lines: &[String],
) -> Rect {
    let max_width = content_area
        .width
        .saturating_sub(4)
        .clamp(1, TOAST_MAX_WIDTH);
    let min_width = 36.min(max_width);
    let content_width = lines
        .iter()
        .map(|line| UnicodeWidthStr::width(line.as_str()))
        .max()
        .unwrap_or(0)
        .max(UnicodeWidthStr::width(title)) as u16;
    let width = content_width.saturating_add(8).clamp(min_width, max_width);

    let inner_width = width.saturating_sub(OVERLAY_CHROME_COLS).max(1);
    let wrapped_height = lines
        .iter()
        .map(|line| wrap_message_lines(line, inner_width).len().max(1) as u16)
        .sum::<u16>()
        .max(1);
    let max_height = content_area.height.saturating_sub(4).max(1);
    let height = wrapped_height
        .saturating_add(OVERLAY_CHROME_ROWS)
        .max(6)
        .min(max_height);

    centered_rect_fixed(width, height, content_area)
}

/// Message dialogs keep the classic fixed footprint for short messages and
/// grow vertically for long ones so no paragraph is silently clipped.
pub(crate) fn adaptive_message_overlay_rect(
    content_area: Rect,
    min_size: (u16, u16),
    message: &str,
) -> Rect {
    let width = min_size.0.min(content_area.width);
    let inner_width = width.saturating_sub(OVERLAY_CHROME_COLS).max(1);
    let body_lines = wrap_message_lines(message, inner_width).len() as u16;
    let height = body_lines
        .saturating_add(OVERLAY_CHROME_ROWS)
        .max(min_size.1)
        .min(content_area.height);

    centered_rect_fixed(width, height, content_area)
}

pub(crate) fn confirm_overlay_rect(content_area: Rect, message: &str) -> Rect {
    adaptive_message_overlay_rect(content_area, OVERLAY_FIXED_MD, message)
}

/// Short confirmations read best centered; multi-paragraph or long messages
/// read as prose and get a left-aligned block instead. Keyed off the message
/// structure so the same dialog aligns consistently across languages.
pub(crate) fn message_block_alignment(message: &str, width: u16) -> Alignment {
    if message.contains('\n') || wrap_message_lines(message, width).len() > 2 {
        Alignment::Left
    } else {
        Alignment::Center
    }
}

pub(crate) fn centered_text_lines(lines: &[String], width: u16, height: u16) -> Vec<Line<'static>> {
    let mut wrapped = Vec::new();
    for line in lines {
        wrapped.extend(wrap_message_lines(line, width));
    }
    if wrapped.is_empty() {
        wrapped.push(String::new());
    }

    let pad = height.saturating_sub(wrapped.len() as u16) / 2;
    let mut out = Vec::with_capacity(pad as usize + wrapped.len());
    for _ in 0..pad {
        out.push(Line::raw(""));
    }
    out.extend(wrapped.into_iter().map(Line::raw));
    out
}

pub(crate) fn content_pane_rect(area: Rect, theme: &theme::Theme) -> Rect {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(nav_pane_width(theme)),
            Constraint::Min(0),
        ])
        .split(root[1]);

    body[1]
}

pub(crate) fn centered_message_lines(message: &str, width: u16, height: u16) -> Vec<Line<'static>> {
    let lines = wrap_message_lines(message, width);
    let pad = height.saturating_sub(lines.len() as u16) / 2;
    let mut out = Vec::with_capacity(pad as usize + lines.len());
    for _ in 0..pad {
        out.push(Line::raw(""));
    }
    out.extend(lines.into_iter().map(Line::raw));
    out
}

pub(crate) fn wrap_message_lines(message: &str, width: u16) -> Vec<String> {
    let width = width as usize;
    if width == 0 {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let segments: Vec<&str> = message.split('\n').collect();
    let last_idx = segments.len().saturating_sub(1);
    for (idx, segment) in segments.iter().enumerate() {
        // A trailing '\n' yields a final empty segment; skip it so "a\n"
        // still wraps to a single line.
        if idx == last_idx && idx > 0 && segment.is_empty() {
            continue;
        }
        wrap_segment(segment, width, &mut lines);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

fn char_cell_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0).max(1)
}

/// Whitespace that offers a wrap opportunity. No-break spaces exist to keep
/// their neighbors together, so they stay inside word tokens.
fn is_wrap_space(ch: char) -> bool {
    ch.is_whitespace() && ch != '\u{a0}' && ch != '\u{202f}'
}

fn wrap_segment(segment: &str, width: usize, out: &mut Vec<String>) {
    let total: usize = segment.chars().map(char_cell_width).sum();
    if total <= width {
        out.push(segment.to_string());
        return;
    }

    let produced_before = out.len();
    let mut current = String::new();
    let mut current_width = 0usize;
    let mut wrapped = false;

    for (token, token_width, is_space) in wrap_tokens(segment) {
        if is_space {
            if current.is_empty() {
                // Keep leading indentation while it can share the first
                // visual line; whitespace at the start of a continuation
                // line (and indentation displaced by an overflowing first
                // word) is dropped.
                if !wrapped && token_width <= width {
                    current_width = token_width;
                    current = token;
                }
                continue;
            }
            if current_width + token_width <= width {
                current.push_str(&token);
                current_width += token_width;
            } else {
                push_wrapped_line(&mut current, &mut current_width, out);
                wrapped = true;
            }
            continue;
        }

        if token_width <= width {
            if current_width + token_width <= width {
                current.push_str(&token);
                current_width += token_width;
            } else {
                push_wrapped_line(&mut current, &mut current_width, out);
                wrapped = true;
                current_width = token_width;
                current = token;
            }
            continue;
        }

        // Token wider than a full line: fall back to per-character breaks.
        for ch in token.chars() {
            let ch_width = char_cell_width(ch);
            if current_width + ch_width > width && !current.is_empty() {
                push_wrapped_line(&mut current, &mut current_width, out);
                wrapped = true;
            }
            current.push(ch);
            current_width += ch_width;
        }
    }

    if !current.is_empty() {
        if wrapped {
            let trimmed_len = current.trim_end().len();
            current.truncate(trimmed_len);
        }
        out.push(current);
    } else if out.len() == produced_before {
        out.push(String::new());
    }
}

fn push_wrapped_line(current: &mut String, current_width: &mut usize, out: &mut Vec<String>) {
    let trimmed_len = current.trim_end().len();
    current.truncate(trimmed_len);
    *current_width = 0;
    if current.is_empty() {
        return;
    }
    out.push(std::mem::take(current));
}

/// Split a line into wrap units: whitespace runs, unbreakable words, and
/// individually breakable wide characters (CJK ideographs, kana, full-width
/// punctuation, emoji), which may wrap at any position like CJK prose.
fn wrap_tokens(segment: &str) -> Vec<(String, usize, bool)> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut word_width = 0usize;
    let mut space = String::new();
    let mut space_width = 0usize;

    for ch in segment.chars() {
        let ch_width = char_cell_width(ch);
        if is_wrap_space(ch) {
            if !word.is_empty() {
                tokens.push((std::mem::take(&mut word), word_width, false));
                word_width = 0;
            }
            space.push(ch);
            space_width += ch_width;
            continue;
        }

        if !space.is_empty() {
            tokens.push((std::mem::take(&mut space), space_width, true));
            space_width = 0;
        }

        if UnicodeWidthChar::width(ch).unwrap_or(0) >= 2 {
            if !word.is_empty() {
                tokens.push((std::mem::take(&mut word), word_width, false));
                word_width = 0;
            }
            tokens.push((ch.to_string(), ch_width, false));
            continue;
        }

        word.push(ch);
        word_width += ch_width;
    }

    if !word.is_empty() {
        tokens.push((word, word_width, false));
    }
    if !space.is_empty() {
        tokens.push((space, space_width, true));
    }

    tokens
}

pub(crate) fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

pub(crate) fn centered_rect_fixed(width: u16, height: u16, r: Rect) -> Rect {
    let width = width.min(r.width);
    let height = height.min(r.height);

    Rect {
        x: r.x + r.width.saturating_sub(width) / 2,
        y: r.y + r.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_width(line: &str) -> usize {
        line.chars().map(char_cell_width).sum()
    }

    #[test]
    fn wrap_breaks_at_word_boundaries() {
        let message = "CC Switch can show installed apps and hide apps that are not installed.";
        let lines = wrap_message_lines(message, 58);
        assert_eq!(
            lines,
            vec![
                "CC Switch can show installed apps and hide apps that are".to_string(),
                "not installed.".to_string(),
            ]
        );
    }

    #[test]
    fn wrap_breaks_cjk_at_any_character() {
        let lines = wrap_message_lines("通用配置适合保存", 6);
        assert_eq!(
            lines,
            vec![
                "通用配".to_string(),
                "置适合".to_string(),
                "保存".to_string()
            ]
        );
    }

    #[test]
    fn wrap_keeps_latin_words_whole_in_mixed_cjk_text() {
        let lines = wrap_message_lines("多个 claude 供应商", 10);
        assert_eq!(
            lines,
            vec![
                "多个".to_string(),
                "claude 供".to_string(),
                "应商".to_string(),
            ]
        );
        assert!(lines.iter().all(|line| line_width(line) <= 10));
    }

    #[test]
    fn wrap_hard_breaks_tokens_wider_than_a_line() {
        let lines = wrap_message_lines("https://example.com/path", 10);
        assert_eq!(
            lines,
            vec![
                "https://ex".to_string(),
                "ample.com/".to_string(),
                "path".to_string(),
            ]
        );
    }

    #[test]
    fn wrap_preserves_explicit_newlines() {
        assert_eq!(
            wrap_message_lines("a\n\nb", 10),
            vec!["a".to_string(), String::new(), "b".to_string()]
        );
        assert_eq!(wrap_message_lines("a\n", 10), vec!["a".to_string()]);
        assert_eq!(
            wrap_message_lines("\na", 10),
            vec![String::new(), "a".to_string()]
        );
    }

    #[test]
    fn wrap_handles_empty_input_and_zero_width() {
        assert_eq!(wrap_message_lines("", 10), vec![String::new()]);
        assert_eq!(wrap_message_lines("abc", 0), vec![String::new()]);
    }

    #[test]
    fn wrap_trims_whitespace_at_break_points() {
        assert_eq!(
            wrap_message_lines("hello     world", 5),
            vec!["hello".to_string(), "world".to_string()]
        );
    }

    #[test]
    fn wrap_keeps_no_break_space_pairs_together() {
        assert_eq!(
            wrap_message_lines("Latency 367\u{a0}ms here", 11),
            vec!["Latency".to_string(), "367\u{a0}ms here".to_string()]
        );
    }

    #[test]
    fn compact_lines_rect_accounts_for_full_chrome() {
        let content = Rect::new(0, 0, 120, 31);
        let lines = vec![
            "URL: https://example.com".to_string(),
            String::new(),
            "Latency: 367 ms".to_string(),
            "Status:  200".to_string(),
        ];
        let area = compact_lines_overlay_rect(content, "Speed Test", &lines);
        // 4 body lines + 2 borders + key bar + gap row.
        assert_eq!(area.height, 8);
    }

    #[test]
    fn message_alignment_is_keyed_off_structure() {
        assert_eq!(
            message_block_alignment("Are you sure?", 56),
            Alignment::Center
        );
        assert_eq!(message_block_alignment("a\nb", 56), Alignment::Left);
        let long = "word ".repeat(40);
        assert_eq!(message_block_alignment(&long, 20), Alignment::Left);
    }

    #[test]
    fn confirm_rect_keeps_fixed_footprint_for_short_messages() {
        let content = Rect::new(0, 0, 100, 30);
        let area = confirm_overlay_rect(content, "Are you sure?");
        assert_eq!(
            area,
            centered_rect_fixed(OVERLAY_FIXED_MD.0, OVERLAY_FIXED_MD.1, content)
        );
    }

    #[test]
    fn confirm_rect_grows_for_long_notices() {
        let content = Rect::new(0, 0, 100, 30);
        let message = "Common Config is for plugin, environment, and tool settings \
                       shared by multiple claude providers.\
                       \n\nWhen a usable snippet exists, new providers will default \
                       to attaching it.\
                       \n\nAfter adding plugins, hooks, or environment variables in \
                       this form, open Common Config, press F4 to extract from the \
                       current edits, then press Ctrl+S to save the snippet.";
        let area = confirm_overlay_rect(content, message);
        assert_eq!(area.width, OVERLAY_FIXED_MD.0);
        assert!(area.height > OVERLAY_FIXED_MD.1, "height {}", area.height);

        let body_lines = wrap_message_lines(message, area.width.saturating_sub(4)) //
            .len() as u16;
        assert_eq!(area.height, body_lines.saturating_add(4));
    }
}
