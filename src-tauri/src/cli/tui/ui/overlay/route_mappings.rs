use super::super::theme;
use super::super::*;
use super::frame::{overlay_frame, OverlaySize};
use crate::cli::tui::app::{RouteMappingEntryEditingState, RouteMappingsEditorState};

pub(super) fn render_route_mappings_editor_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    overlay: &Overlay,
) {
    let Overlay::RouteMappingsEditor(editor) = overlay else {
        return;
    };

    if let Some(editing) = editor.editing.as_ref() {
        render_entry_editor(frame, content_area, theme, editor, editing);
        return;
    }

    let keys = [
        ("↑↓", texts::tui_key_select()),
        ("a", texts::tui_key_add()),
        ("Enter", texts::tui_key_edit()),
        ("d", texts::tui_key_delete()),
        ("Esc", texts::tui_key_apply()),
    ];

    let body = overlay_frame(
        frame,
        content_area,
        theme,
        &texts::tui_route_mappings_title(&editor.provider_name),
        &keys,
        OverlaySize::Fixed(64, 16),
        overlay_border_style(theme, false),
    );

    if editor.rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::raw(texts::tui_route_mappings_empty_hint()))
                .alignment(Alignment::Center),
            body,
        );
        return;
    }

    let selected = editor.selected.min(editor.rows.len().saturating_sub(1));
    let visible = visible_selection_window(editor.rows.len(), selected, body.height as usize);
    let visible_start = visible.start;
    let items = editor.rows[visible.clone()]
        .iter()
        .map(|(request, actual)| {
            let request = bounded_trimmed_text_for_display(request);
            let actual = bounded_trimmed_text_for_display(actual);
            ListItem::new(Line::raw(format!(
                "{request} {} {actual}",
                texts::tui_route_mappings_arrow()
            )))
        });

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = ListState::default();
    state.select(Some(selected.saturating_sub(visible_start)));
    frame.render_stateful_widget(list, body, &mut state);
}

fn render_entry_editor(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    editor: &RouteMappingsEditorState,
    editing: &RouteMappingEntryEditingState,
) {
    let mode = if editing.row.is_some() {
        texts::tui_key_edit()
    } else {
        texts::tui_key_add()
    };
    let title = format!(
        "{} ({mode})",
        texts::tui_route_mappings_title(&editor.provider_name)
    );

    // Ctrl+F only fetches while the actual-model field is active; a plain
    // `f` must stay a letter because the row editor is live-typing. Ctrl+R
    // likewise only picks request models while the request field is active.
    let mut keys = vec![
        ("Tab", texts::tui_key_select()),
        ("Enter", texts::tui_key_apply()),
    ];
    if editing.request_active() {
        keys.push(("Ctrl+R", texts::tui_route_mappings_request_pick_key()));
    }
    if editing.actual_active() {
        keys.push(("Ctrl+F", texts::tui_route_mappings_fetch_key()));
    }
    keys.push(("Esc", texts::tui_key_cancel()));

    let body = overlay_frame(
        frame,
        content_area,
        theme,
        &title,
        &keys,
        // Wide enough that all four key chips (including Ctrl+F fetch) fit
        // the key bar in both languages.
        OverlaySize::Fixed(72, 12),
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
            texts::tui_route_mappings_request_label(),
            &editing.request,
            editing.request_active(),
        ),
        (
            texts::tui_route_mappings_actual_label(),
            &editing.actual,
            editing.actual_active(),
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
