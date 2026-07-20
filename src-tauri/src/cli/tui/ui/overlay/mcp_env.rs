use super::super::theme;
use super::super::*;
use super::frame::{overlay_frame, OverlaySize};

pub(super) fn render_mcp_env_picker_overlay(
    frame: &mut Frame<'_>,
    app: &App,
    content_area: Rect,
    theme: &theme::Theme,
    selected: usize,
) {
    let body = overlay_frame(
        frame,
        content_area,
        theme,
        texts::tui_mcp_env_title(),
        &[
            ("↑↓", texts::tui_key_select()),
            ("a", texts::tui_key_add()),
            ("Enter", texts::tui_key_edit()),
            ("Del/Backspace", texts::tui_key_delete()),
            ("Esc", texts::tui_key_close()),
        ],
        OverlaySize::Fixed(64, 16),
        overlay_border_style(theme, false),
    );

    let Some(FormState::McpAdd(mcp)) = app.form.as_ref() else {
        return;
    };

    if mcp.env_rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::raw(texts::tui_mcp_env_empty_state()))
                .alignment(Alignment::Center),
            body,
        );
        return;
    }

    let selected = selected.min(mcp.env_rows.len().saturating_sub(1));
    let visible = visible_selection_window(mcp.env_rows.len(), selected, body.height as usize);
    let visible_start = visible.start;
    let items = mcp.env_rows[visible.clone()].iter().map(|row| {
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

pub(super) fn render_mcp_env_entry_editor_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    overlay: &Overlay,
) {
    let Overlay::McpEnvEntryEditor(editor) = overlay else {
        return;
    };

    let title = if editor.row.is_some() {
        texts::tui_mcp_env_edit_entry_title()
    } else {
        texts::tui_mcp_env_add_entry_title()
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
        (
            texts::tui_mcp_env_key_label(),
            &editor.key,
            editor.key_active(),
        ),
        (
            texts::tui_mcp_env_value_label(),
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
