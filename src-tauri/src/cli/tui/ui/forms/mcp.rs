use super::*;

pub(crate) fn render_mcp_add_form(
    frame: &mut Frame<'_>,
    app: &App,
    mcp: &super::form::McpAddFormState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let title = match &mcp.mode {
        super::form::FormMode::Add => texts::tui_mcp_add_title().to_string(),
        super::form::FormMode::Edit { .. } => texts::tui_mcp_edit_title(mcp.name.value.trim()),
    };
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(title);
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let template_height = if matches!(mcp.mode, super::form::FormMode::Add) {
        3
    } else {
        0
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(template_height),
            Constraint::Min(0),
        ])
        .split(inner);

    render_key_bar(
        frame,
        chunks[0],
        theme,
        &add_form_key_items(mcp.focus, mcp.editing, None),
    );

    if matches!(mcp.mode, super::form::FormMode::Add) {
        let labels = mcp.template_labels();
        render_form_template_chips(
            frame,
            &labels,
            mcp.template_idx,
            matches!(mcp.focus, FormFocus::Templates),
            chunks[1],
            theme,
        );
    }

    // Body: fields + JSON preview
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(chunks[2]);

    // Fields
    let fields_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(
            matches!(mcp.focus, FormFocus::Fields),
            theme,
        ))
        .title(texts::tui_form_fields_title());
    frame.render_widget(fields_block.clone(), body[0]);
    let fields_inner = fields_block.inner(body[0]);

    let fields_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3)])
        .split(fields_inner);

    let fields = mcp.fields();
    let rows_data = fields
        .iter()
        .map(|field| mcp_field_label_and_value(mcp, *field))
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
        state.select(Some(mcp.field_idx.min(fields.len() - 1)));
    }
    frame.render_stateful_widget(table, fields_chunks[0], &mut state);

    // Editor
    let editor_active = matches!(mcp.focus, FormFocus::Fields) && mcp.editing;
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

    let selected = fields
        .get(mcp.field_idx.min(fields.len().saturating_sub(1)))
        .copied();
    if let Some(field) = selected {
        if let Some(input) = mcp.input(field) {
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
        } else {
            let (line, _cursor) = mcp_field_editor_line(mcp, selected, editor_inner.width as usize);
            frame.render_widget(
                Paragraph::new(line).wrap(Wrap { trim: false }),
                editor_inner,
            );
        }
    }

    // JSON Preview
    let json_text = serde_json::to_string_pretty(&mcp.to_mcp_server_json_value())
        .unwrap_or_else(|_| "{}".to_string());
    render_form_json_preview(
        frame,
        &json_text,
        mcp.json_scroll,
        matches!(mcp.focus, FormFocus::JsonPreview),
        body[1],
        theme,
    );
}

pub(crate) fn mcp_field_label_and_value(
    mcp: &super::form::McpAddFormState,
    field: McpAddField,
) -> (String, String) {
    let label = match field {
        McpAddField::Id => texts::tui_label_id().to_string(),
        McpAddField::Name => texts::header_name().to_string(),
        McpAddField::Command => texts::tui_label_command().to_string(),
        McpAddField::Args => texts::tui_label_args().to_string(),
        McpAddField::Env => texts::tui_label_env().to_string(),
        McpAddField::AppClaude => texts::tui_label_app_claude().to_string(),
        McpAddField::AppCodex => texts::tui_label_app_codex().to_string(),
        McpAddField::AppGemini => texts::tui_label_app_gemini().to_string(),
    };

    let value = match field {
        McpAddField::Env => mcp.env_summary(),
        McpAddField::AppClaude => {
            if mcp.apps.claude {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        McpAddField::AppCodex => {
            if mcp.apps.codex {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        McpAddField::AppGemini => {
            if mcp.apps.gemini {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        _ => mcp
            .input(field)
            .map(|v| v.value.trim().to_string())
            .unwrap_or_default(),
    };

    (
        label,
        if value.is_empty() {
            texts::tui_na().to_string()
        } else {
            value
        },
    )
}

pub(crate) fn mcp_field_editor_line(
    mcp: &super::form::McpAddFormState,
    selected: Option<McpAddField>,
    _width: usize,
) -> (Line<'static>, usize) {
    let Some(field) = selected else {
        return (Line::raw(""), 0);
    };

    let text = match field {
        McpAddField::Env => texts::tui_mcp_env_editor_hint().to_string(),
        McpAddField::AppClaude => format!("claude = {}", mcp.apps.claude),
        McpAddField::AppCodex => format!("codex = {}", mcp.apps.codex),
        McpAddField::AppGemini => format!("gemini = {}", mcp.apps.gemini),
        _ => String::new(),
    };

    (Line::raw(text), 0)
}
