use super::super::theme;
use super::super::*;
use super::frame::{overlay_frame, OverlaySize};
use crate::cli::tui::form::McpKeyValueKind;

pub(super) fn render_mcp_key_value_picker_overlay(
    frame: &mut Frame<'_>,
    app: &App,
    content_area: Rect,
    theme: &theme::Theme,
    kind: McpKeyValueKind,
    selected: usize,
) {
    let keys = [
        ("↑↓", texts::tui_key_select()),
        ("a", texts::tui_key_add()),
        ("Enter", texts::tui_key_edit()),
        ("Del/Backspace", texts::tui_key_delete()),
        ("Esc", texts::tui_key_close()),
    ];

    let title = match kind {
        McpKeyValueKind::Env => texts::tui_mcp_env_title(),
        McpKeyValueKind::Headers => texts::tui_mcp_headers_title(),
    };
    let body = overlay_frame(
        frame,
        content_area,
        theme,
        title,
        &keys,
        OverlaySize::Fixed(64, 16),
        overlay_border_style(theme, false),
    );

    let Some(FormState::McpAdd(mcp)) = app.form.as_ref() else {
        return;
    };
    let rows = mcp.key_value_rows(kind);

    if rows.is_empty() {
        let empty_state = match kind {
            McpKeyValueKind::Env => texts::tui_mcp_env_empty_state(),
            McpKeyValueKind::Headers => texts::tui_mcp_headers_empty_state(),
        };
        frame.render_widget(
            Paragraph::new(Line::raw(empty_state)).alignment(Alignment::Center),
            body,
        );
        return;
    }

    let selected = selected.min(rows.len().saturating_sub(1));
    let visible = visible_selection_window(rows.len(), selected, body.height as usize);
    let visible_start = visible.start;
    let items = rows[visible.clone()].iter().map(|row| {
        let key = bounded_trimmed_text_for_display(&row.key);
        let value = bounded_trimmed_text_for_display(&row.value);
        ListItem::new(Line::raw(format!("{key} = {value}")))
    });

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = ListState::default();
    state.select(Some(selected.saturating_sub(visible_start)));
    frame.render_stateful_widget(list, body, &mut state);
}

pub(super) fn render_mcp_key_value_entry_editor_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    overlay: &Overlay,
) {
    let Overlay::McpKeyValueEntryEditor(editor) = overlay else {
        return;
    };

    let title = match (editor.kind, editor.row.is_some()) {
        (McpKeyValueKind::Env, true) => texts::tui_mcp_env_edit_entry_title(),
        (McpKeyValueKind::Env, false) => texts::tui_mcp_env_add_entry_title(),
        (McpKeyValueKind::Headers, true) => texts::tui_mcp_headers_edit_entry_title(),
        (McpKeyValueKind::Headers, false) => texts::tui_mcp_headers_add_entry_title(),
    };

    let body = overlay_frame(
        frame,
        content_area,
        theme,
        title,
        &[
            ("Tab", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_cancel()),
        ],
        OverlaySize::Fixed(64, 12),
        overlay_border_style(theme, false),
    );

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(body);

    let fields = [
        (texts::tui_mcp_key_label(), &editor.key, editor.key_active()),
        (
            texts::tui_mcp_value_label(),
            &editor.value,
            editor.value_active(),
        ),
    ];

    for (idx, (label, input, active)) in fields.into_iter().enumerate() {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(if active {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.dim)
            })
            .title(format!(" {} ", label));
        let input_area = chunks[idx];
        let input_inner = block.inner(input_area);
        frame.render_widget(block, input_area);

        let (visible, cursor_x) = inline_input_window(input, input_inner.width);
        frame.render_widget(
            Paragraph::new(Line::raw(visible)).wrap(Wrap { trim: false }),
            input_inner,
        );

        if active {
            frame.set_cursor_position((
                input_inner.x + cursor_x.min(input_inner.width.saturating_sub(1)),
                input_inner.y,
            ));
        }
    }
}
