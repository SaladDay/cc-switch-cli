//! Action dialogs keep explanatory copy above a bottom-aligned list of
//! keyboard actions. Every action uses the same neutral button surface; the
//! key labels communicate the choice without implying a selected default.

use super::super::theme;
use super::super::*;
use super::frame::action_dialog_frame_at;

pub(super) struct ActionDialogSpec<'a> {
    pub title: &'a str,
    pub message: &'a str,
    pub actions: &'a [(&'a str, &'a str)],
    pub min_size: (u16, u16),
    pub border: Style,
}

pub(super) fn render_action_dialog(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    spec: ActionDialogSpec<'_>,
) {
    let sizing_text = format!(
        "{}\n\n{}",
        spec.message,
        spec.actions
            .iter()
            .map(|(key, label)| format!("{key} {label}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let area = adaptive_message_overlay_rect(content_area, spec.min_size, &sizing_text);
    let body = action_dialog_frame_at(frame, area, spec.title, spec.border);
    let button_rows = spec.actions.len().min(u16::MAX as usize) as u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(button_rows),
            Constraint::Length(1),
        ])
        .split(body);

    frame.render_widget(
        Paragraph::new(centered_message_lines(
            spec.message,
            chunks[0].width,
            chunks[0].height,
        ))
        .alignment(message_block_alignment(spec.message, chunks[0].width)),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(action_button_lines(theme, spec.actions, chunks[1].width))
            .alignment(Alignment::Center),
        chunks[1],
    );
}

fn action_button_lines(
    theme: &theme::Theme,
    actions: &[(&str, &str)],
    available_width: u16,
) -> Vec<Line<'static>> {
    let content_width = actions
        .iter()
        .map(|(key, label)| UnicodeWidthStr::width(format!("{key:<5}  {label}").as_str()))
        .max()
        .unwrap_or(0)
        .min(available_width.saturating_sub(2) as usize);
    let base = inactive_chip_style(theme);

    actions
        .iter()
        .map(|(key, label)| {
            let text =
                truncate_to_display_width(&format!("{key:<5}  {label}"), content_width as u16);
            let used = UnicodeWidthStr::width(text.as_str());
            let trailing = " ".repeat(content_width.saturating_sub(used));
            Line::styled(format!(" {text}{trailing} "), base)
        })
        .collect()
}
