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
        .title(format!(" {} ", title));
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    let fields = prompt.fields();
    let selected_idx = prompt
        .text_edit_target()
        .and_then(|field| fields.iter().position(|candidate| *candidate == field))
        .unwrap_or(prompt.field_idx.min(fields.len().saturating_sub(1)));
    let selected = fields.get(selected_idx).copied();
    render_key_bar(
        frame,
        chunks[0],
        theme,
        &prompt_meta_form_key_items(prompt.text_edit.is_some(), prompt.focus, selected),
    );

    let split = chunks[1].width >= super::form::PROMPT_FORM_SPLIT_MIN_BODY_WIDTH;
    let panes = split.then(|| {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
            .split(chunks[1])
    });
    if split || matches!(prompt.focus, FormFocus::Fields) {
        let fields_area = if split {
            panes.as_ref().expect("split panes")[0]
        } else {
            chunks[1]
        };
        render_prompt_inline_fields(frame, prompt, &fields, selected_idx, fields_area, theme);
    }
    if split || matches!(prompt.focus, FormFocus::Content) {
        let content_area = if split {
            panes.as_ref().expect("split panes")[1]
        } else {
            chunks[1]
        };
        render_prompt_content_editor(frame, prompt, content_area, theme);
    }
}

fn render_prompt_inline_fields(
    frame: &mut Frame<'_>,
    prompt: &super::form::PromptMetaFormState,
    fields: &[PromptMetaField],
    selected_idx: usize,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let fields_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(
            matches!(prompt.focus, FormFocus::Fields),
            theme,
        ))
        .title(format!(" {} ", texts::tui_form_fields_title()));
    frame.render_widget(fields_block.clone(), area);
    let table_area = fields_block.inner(area);

    let rows_data = fields
        .iter()
        .map(|field| prompt_meta_field_label_and_value(prompt, *field))
        .collect::<Vec<_>>();

    let raw_label_width = field_label_column_width(
        rows_data
            .iter()
            .map(|(label, _value)| label.as_str())
            .chain(std::iter::once(texts::tui_header_field())),
        1,
    );
    let label_col_width = raw_label_width.min(
        table_area
            .width
            .saturating_sub(FORM_VALUE_MIN_WIDTH)
            .saturating_sub(1),
    );
    let value_width = form_value_width(table_area.width, label_col_width, theme);

    let header = Row::new(vec![
        Cell::from(cell_pad(texts::tui_header_field())),
        Cell::from(texts::tui_header_value()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let mut cursor_x = None;
    let mut row_heights = Vec::with_capacity(fields.len());
    let rows = fields
        .iter()
        .zip(rows_data.iter())
        .enumerate()
        .map(|(idx, (field, (label, value)))| {
            let editing = matches!(prompt.focus, FormFocus::Fields)
                && prompt.text_edit_target() == Some(*field);
            let display = if editing {
                let (visible, x) = inline_input_window(prompt.input(*field), value_width);
                if idx == selected_idx {
                    cursor_x = Some(x);
                }
                visible
            } else {
                truncated_value_cell(value, table_area.width, label_col_width, theme)
            };
            let error = prompt.field_error(*field);
            let height = inline_row_height(error, value_width);
            row_heights.push(height);
            Row::new(vec![
                Cell::from(cell_pad(label)),
                inline_field_cell(display, error, value_width, theme),
            ])
            .height(height)
        })
        .collect::<Vec<_>>();

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
        state.select(Some(selected_idx.min(fields.len() - 1)));
    }
    frame.render_stateful_widget(table, table_area, &mut state);
    if let Some(cursor_x) = cursor_x {
        set_inline_table_cursor(
            frame,
            table_area,
            label_col_width,
            selected_idx,
            state.offset(),
            &row_heights,
            cursor_x,
            theme,
        );
    }
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
        .title(format!(" {} ", texts::tui_editor_text_field_title()));
    frame.render_widget(content_block.clone(), area);
    let content_inner = content_block.inner(area);

    let height = content_inner.height as usize;
    let width = content_inner.width.max(1);

    // Use the rendered pane dimensions as the final authority without cloning
    // the prompt text buffers on every TUI tick.
    let viewport = ratatui::layout::Size {
        width: content_inner.width,
        height: content_inner.height,
    };
    let (scroll, scroll_subline) = prompt.content.viewport_origin(viewport);

    let shown = prompt
        .content
        .visible_wrapped_lines_from(width, height, scroll, scroll_subline)
        .into_iter()
        .map(Line::raw)
        .collect::<Vec<_>>();

    frame.render_widget(Paragraph::new(shown), content_inner);

    if active {
        let (row_in_view, col_in_view) =
            prompt
                .content
                .cursor_visual_offset_from_origin(width, scroll, scroll_subline);
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
    let value = bounded_trimmed_text_for_display(&prompt.input(field).value);
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
                ("Enter", texts::tui_key_apply()),
                ("Ctrl+S", texts::tui_key_save()),
                ("Esc", texts::tui_key_cancel()),
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
