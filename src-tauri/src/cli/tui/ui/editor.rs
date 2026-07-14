use super::*;

pub(super) fn render_editor(
    frame: &mut Frame<'_>,
    app: &App,
    editor: &super::app::EditorState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(format!(" {} ", editor.title.clone()));
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    let mut keys = vec![
        ("↑↓←→", texts::tui_key_move()),
        ("Ctrl+O", texts::tui_key_external_editor()),
        ("Ctrl+S", texts::tui_key_save()),
        ("Esc", texts::tui_key_close()),
    ];
    if let super::app::EditorSubmit::ConfigCommonSnippet { source, .. } = &editor.submit {
        keys.insert(2, ("F2", texts::tui_key_format()));
        if matches!(source, super::app::CommonSnippetViewSource::ProviderForm) {
            keys.insert(3, ("F4", texts::tui_key_extract()));
        }
    }
    render_key_bar(frame, chunks[0], theme, &keys);

    let field_title = match editor.kind {
        super::app::EditorKind::Json => texts::tui_editor_json_field_title(),
        super::app::EditorKind::Toml => texts::tui_editor_toml_field_title(),
        super::app::EditorKind::Plain => texts::tui_editor_text_field_title(),
    };
    let field_border_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);

    let field = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(field_border_style)
        .title(format!("-{}", field_title));

    frame.render_widget(field.clone(), chunks[1]);
    let field_inner = field.inner(chunks[1]);

    let height = field_inner.height as usize;
    let width = field_inner.width.max(1);

    let mut shown = Vec::new();
    let start = editor.scroll.min(editor.lines.len().saturating_sub(1));
    for line in editor.lines.iter().skip(start) {
        for segment in super::app::EditorState::wrap_line_segments(line, width) {
            if shown.len() >= height {
                break;
            }
            shown.push(Line::raw(segment));
        }
        if shown.len() >= height {
            break;
        }
    }

    frame.render_widget(Paragraph::new(shown), field_inner);

    let (row_in_view, col_in_view) = editor.cursor_visual_offset_from_scroll(width);
    if row_in_view < height {
        let x = field_inner.x + col_in_view.min(field_inner.width.saturating_sub(1));
        let y = field_inner.y + row_in_view as u16;
        frame.set_cursor_position((x, y));
    }
}
