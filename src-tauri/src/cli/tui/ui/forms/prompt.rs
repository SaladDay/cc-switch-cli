use super::*;

pub(crate) fn render_prompt_meta_form(
    frame: &mut Frame<'_>,
    app: &App,
    prompt: &super::form::PromptMetaFormState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let title = match &prompt.mode {
        super::form::FormMode::Add => texts::tui_prompt_create_title().to_string(),
        super::form::FormMode::Edit { .. } => texts::tui_prompt_rename_title().to_string(),
    };
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(title);
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    let fields = prompt.fields();
    let selected = fields
        .get(prompt.field_idx.min(fields.len().saturating_sub(1)))
        .copied();
    render_key_bar(
        frame,
        chunks[0],
        theme,
        &prompt_meta_form_key_items(prompt.editing, prompt.focus, selected),
    );

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(36), Constraint::Min(30)])
        .split(chunks[1]);

    let fields_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(
            matches!(prompt.focus, FormFocus::Fields),
            theme,
        ))
        .title(texts::tui_form_fields_title());
    frame.render_widget(fields_block.clone(), body[0]);
    let fields_inner = fields_block.inner(body[0]);

    let fields_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3)])
        .split(fields_inner);

    let rows_data = fields
        .iter()
        .map(|field| prompt_meta_field_label_and_value(prompt, *field))
        .collect::<Vec<_>>();

    let label_col_width = field_label_column_width(
        rows_data
            .iter()
            .map(|(label, _value)| label.as_str())
            .chain(std::iter::once(texts::tui_header_field())),
        1,
    );

    let header = Row::new(vec![
        Cell::from(cell_pad(texts::tui_header_field())),
        Cell::from(texts::tui_header_value()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let rows = rows_data.iter().map(|(label, value)| {
        Row::new(vec![Cell::from(cell_pad(label)), Cell::from(value.clone())])
    });

    let table = Table::new(
        rows,
        [Constraint::Length(label_col_width), Constraint::Min(10)],
    )
    .header(header)
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    if !fields.is_empty() {
        state.select(Some(prompt.field_idx.min(fields.len() - 1)));
    }
    frame.render_stateful_widget(table, fields_chunks[0], &mut state);

    let editor_active = matches!(prompt.focus, FormFocus::Fields) && prompt.editing;
    let editor_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(editor_active, theme))
        .title(if editor_active {
            texts::tui_form_editing_title()
        } else {
            texts::tui_form_input_title()
        });
    frame.render_widget(editor_block.clone(), fields_chunks[1]);
    let editor_inner = editor_block.inner(fields_chunks[1]);

    if let Some(field) = selected {
        let input = prompt.input(field);
        let (visible, cursor_x) =
            visible_text_window(&input.value, input.cursor, editor_inner.width as usize);
        frame.render_widget(
            Paragraph::new(Line::raw(visible)).wrap(Wrap { trim: false }),
            editor_inner,
        );
        if editor_active {
            let x = editor_inner.x + cursor_x.min(editor_inner.width.saturating_sub(1));
            let y = editor_inner.y;
            frame.set_cursor_position((x, y));
        }
    }

    render_prompt_content_editor(frame, prompt, body[1], theme);
}

fn render_prompt_content_editor(
    frame: &mut Frame<'_>,
    prompt: &super::form::PromptMetaFormState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let active = matches!(prompt.focus, FormFocus::Content);
    let content_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(active, theme))
        .title(texts::tui_editor_text_field_title());
    frame.render_widget(content_block.clone(), area);
    let content_inner = content_block.inner(area);

    let height = content_inner.height as usize;
    let width = content_inner.width.max(1);

    let mut shown = Vec::new();
    let start = prompt
        .content
        .scroll
        .min(prompt.content.lines.len().saturating_sub(1));
    for line in prompt.content.lines.iter().skip(start) {
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

    frame.render_widget(Paragraph::new(shown), content_inner);

    if active {
        let (row_in_view, col_in_view) = prompt.content.cursor_visual_offset_from_scroll(width);
        if row_in_view < height {
            let x = content_inner.x + col_in_view.min(content_inner.width.saturating_sub(1));
            let y = content_inner.y + row_in_view as u16;
            frame.set_cursor_position((x, y));
        }
    }
}

fn prompt_meta_field_label_and_value(
    prompt: &super::form::PromptMetaFormState,
    field: PromptMetaField,
) -> (String, String) {
    let label = match field {
        PromptMetaField::Id => texts::tui_label_id().to_string(),
        PromptMetaField::Name => texts::header_name().to_string(),
        PromptMetaField::Description => texts::header_description().to_string(),
    };
    let value = prompt.input(field).value.trim().to_string();
    (
        label,
        if value.is_empty() {
            texts::tui_na().to_string()
        } else {
            value
        },
    )
}

fn prompt_meta_form_key_items(
    editing: bool,
    focus: FormFocus,
    _selected_field: Option<PromptMetaField>,
) -> Vec<(&'static str, &'static str)> {
    let mut keys = Vec::new();

    match focus {
        FormFocus::Content => {
            keys.extend([
                ("Shift+Tab", texts::tui_key_focus()),
                ("↑↓←→", texts::tui_key_move()),
                ("Ctrl+O", texts::tui_key_external_editor()),
                ("Ctrl+S", texts::tui_key_save()),
                ("Esc", texts::tui_key_close()),
            ]);
        }
        FormFocus::Fields if editing => {
            keys.extend([
                ("Tab", texts::tui_key_focus()),
                ("Ctrl+S", texts::tui_key_save()),
                ("Esc", texts::tui_key_close()),
            ]);
            keys.extend([
                ("←→", texts::tui_key_move()),
                ("Enter", texts::tui_key_exit_edit()),
            ]);
        }
        _ => {
            keys.extend([
                ("Tab", texts::tui_key_focus()),
                ("Ctrl+S", texts::tui_key_save()),
                ("Esc", texts::tui_key_close()),
            ]);
            keys.extend([
                ("↑↓", texts::tui_key_select()),
                ("Enter", texts::tui_key_edit_mode()),
            ]);
        }
    }

    keys
}
