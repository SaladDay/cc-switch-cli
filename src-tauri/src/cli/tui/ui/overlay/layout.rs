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
    area.width < content_area.width.saturating_sub(6) && area.height <= 12
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

    let inner_width = width.saturating_sub(2).max(1);
    let wrapped_height = lines
        .iter()
        .map(|line| wrap_message_lines(line, inner_width).len().max(1) as u16)
        .sum::<u16>()
        .max(1);
    let max_height = content_area.height.saturating_sub(4).max(1);
    let height = wrapped_height.saturating_add(3).max(6).min(max_height);

    centered_rect_fixed(width, height, content_area)
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
    let mut current = String::new();
    let mut current_width = 0usize;

    for ch in message.chars() {
        if ch == '\n' {
            lines.push(current);
            current = String::new();
            current_width = 0;
            continue;
        }

        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
        if current_width + ch_width > width && !current.is_empty() {
            lines.push(current);
            current = String::new();
            current_width = 0;
        }

        current.push(ch);
        current_width = current_width.saturating_add(ch_width);
    }

    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }

    lines
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
