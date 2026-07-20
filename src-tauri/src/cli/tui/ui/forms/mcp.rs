use super::*;

const MCP_PREVIEW_MAX_ARGS: usize = 128;

pub(crate) fn render_mcp_add_form(
    frame: &mut Frame<'_>,
    app: &App,
    mcp: &super::form::McpAddFormState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let title = match &mcp.mode {
        super::form::FormMode::Add => texts::tui_mcp_add_title().to_string(),
        super::form::FormMode::Edit { .. } => {
            let name = bounded_trimmed_text_for_display(&mcp.name.value);
            texts::tui_mcp_edit_title(&name)
        }
    };
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(format!(" {} ", title));
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

    let fields = mcp.fields();
    let selected_idx = mcp
        .text_edit_target()
        .and_then(|field| fields.iter().position(|candidate| *candidate == field))
        .unwrap_or(mcp.field_idx.min(fields.len().saturating_sub(1)));
    let selected = fields.get(selected_idx).copied();
    render_key_bar(
        frame,
        chunks[0],
        theme,
        &mcp_add_form_key_items(mcp.focus, mcp.text_edit.is_some(), selected),
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

    let split = form_can_split(chunks[2].width, label_col_width, theme);
    let panes = split.then(|| {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
            .split(chunks[2])
    });
    let fields_area = if split {
        Some(panes.as_ref().expect("split panes")[0])
    } else if !matches!(mcp.focus, FormFocus::JsonPreview) {
        Some(chunks[2])
    } else {
        None
    };
    let preview_area = if split {
        Some(panes.as_ref().expect("split panes")[1])
    } else if matches!(mcp.focus, FormFocus::JsonPreview) {
        Some(chunks[2])
    } else {
        None
    };

    if let Some(fields_area) = fields_area {
        render_mcp_inline_fields(
            frame,
            mcp,
            &fields,
            &rows_data,
            selected_idx,
            fields_area,
            theme,
        );
    }

    if let Some(preview_area) = preview_area {
        let json_value = mcp_preview_value(mcp);
        let json_text =
            serde_json::to_string_pretty(&json_value).unwrap_or_else(|_| "{}".to_string());
        render_form_json_preview(
            frame,
            &json_text,
            mcp.json_scroll,
            matches!(mcp.focus, FormFocus::JsonPreview),
            preview_area,
            theme,
        );
    }
}

fn mcp_preview_value(mcp: &super::form::McpAddFormState) -> Value {
    const MAX_ENV_KEYS: usize = 128;
    const MAX_TAGS: usize = 128;

    let source = mcp.source_server();
    let mut root = serde_json::Map::new();
    if let Some(description) = source.description.as_deref() {
        root.insert(
            "description".to_string(),
            Value::String(bounded_trimmed_text_for_display(description)),
        );
    }
    if let Some(homepage) = source.homepage.as_deref() {
        root.insert(
            "homepage".to_string(),
            Value::String(bounded_trimmed_text_for_display(homepage)),
        );
    }
    if let Some(docs) = source.docs.as_deref() {
        root.insert(
            "docs".to_string(),
            Value::String(bounded_trimmed_text_for_display(docs)),
        );
    }
    if !source.tags.is_empty() {
        let mut tags = source
            .tags
            .iter()
            .take(MAX_TAGS)
            .map(|tag| Value::String(bounded_trimmed_text_for_display(tag)))
            .collect::<Vec<_>>();
        let hidden = source.tags.len().saturating_sub(tags.len());
        if hidden > 0 {
            tags.push(Value::String(format!(
                "[preview truncated: {hidden} more entries]"
            )));
        }
        root.insert("tags".to_string(), Value::Array(tags));
    }
    root.insert(
        "id".to_string(),
        Value::String(bounded_trimmed_text_for_display(&mcp.id.value)),
    );
    root.insert(
        "name".to_string(),
        Value::String(bounded_trimmed_text_for_display(&mcp.name.value)),
    );

    let mut server = match bounded_json_preview(&source.server) {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    for key in ["type", "command", "args", "env", "url"] {
        server.remove(key);
    }
    if mcp.server_type.is_remote() {
        server.remove("cwd");
    } else {
        server.remove("headers");
    }

    server.insert(
        "type".to_string(),
        Value::String(mcp.server_type.as_str().to_string()),
    );
    if mcp.server_type.is_remote() {
        server.insert(
            "url".to_string(),
            Value::String(bounded_trimmed_text_for_display(&mcp.url.value)),
        );
    } else {
        server.insert(
            "command".to_string(),
            Value::String(bounded_trimmed_text_for_display(&mcp.command.value)),
        );
        let (args, hidden_args) = mcp.args_preview(MCP_PREVIEW_MAX_ARGS);
        let mut args = args
            .into_iter()
            .map(|arg| Value::String(bounded_trimmed_text_for_display(arg)))
            .collect::<Vec<_>>();
        if hidden_args > 0 {
            args.push(Value::String(format!(
                "[preview truncated: {hidden_args} more entries]"
            )));
        }
        server.insert("args".to_string(), Value::Array(args));
        if !mcp.env_rows.is_empty() {
            let mut env = serde_json::Map::new();
            for row in mcp.env_rows.iter().take(MAX_ENV_KEYS) {
                let key = row.key.chars().take(128).collect::<String>();
                env.insert(
                    key,
                    Value::String(bounded_trimmed_text_for_display(&row.value)),
                );
            }
            let hidden = mcp.env_rows.len().saturating_sub(MAX_ENV_KEYS);
            if hidden > 0 {
                env.insert(
                    "…".to_string(),
                    Value::String(format!("[preview truncated: {hidden} more entries]")),
                );
            }
            server.insert("env".to_string(), Value::Object(env));
        }
    }
    root.insert("server".to_string(), Value::Object(server));
    root.insert(
        "apps".to_string(),
        serde_json::json!({
            "claude": mcp.apps.claude,
            "codex": mcp.apps.codex,
            "gemini": mcp.apps.gemini,
            "opencode": mcp.apps.opencode,
            "hermes": mcp.apps.hermes,
        }),
    );
    Value::Object(root)
}

fn render_mcp_inline_fields(
    frame: &mut Frame<'_>,
    mcp: &super::form::McpAddFormState,
    fields: &[McpAddField],
    rows_data: &[(String, String)],
    selected_idx: usize,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(
            matches!(mcp.focus, FormFocus::Fields),
            theme,
        ))
        .title(format!(" {} ", texts::tui_form_fields_title()));
    frame.render_widget(block.clone(), area);
    let table_area = block.inner(area);
    let raw_label_width = field_label_column_width(
        rows_data
            .iter()
            .map(|row| row.0.as_str())
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
    let mut cursor_x = None;
    let mut row_heights = Vec::with_capacity(fields.len());
    let rows = fields
        .iter()
        .zip(rows_data.iter())
        .enumerate()
        .map(|(idx, (field, (label, value)))| {
            let editing =
                matches!(mcp.focus, FormFocus::Fields) && mcp.text_edit_target() == Some(*field);
            let display = if editing {
                mcp.input(*field).map_or_else(
                    || truncated_value_cell(value, table_area.width, label_col_width, theme),
                    |input| {
                        let (visible, x) = inline_input_window(input, value_width);
                        if idx == selected_idx {
                            cursor_x = Some(x);
                        }
                        visible
                    },
                )
            } else {
                truncated_value_cell(value, table_area.width, label_col_width, theme)
            };
            let error = mcp.field_error(*field);
            let height = inline_row_height(error, value_width);
            row_heights.push(height);
            Row::new(vec![
                Cell::from(cell_pad(label)),
                inline_field_cell(display, error, value_width, theme),
            ])
            .height(height)
        })
        .collect::<Vec<_>>();
    let header = Row::new(vec![
        Cell::from(cell_pad(texts::tui_header_field())),
        Cell::from(texts::tui_header_value()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));
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

pub(crate) fn mcp_field_label_and_value(
    mcp: &super::form::McpAddFormState,
    field: McpAddField,
) -> (String, String) {
    let label = match field {
        McpAddField::Id => texts::tui_label_id().to_string(),
        McpAddField::Name => texts::header_name().to_string(),
        McpAddField::Type => texts::tui_label_mcp_type().to_string(),
        McpAddField::Command => texts::tui_label_command().to_string(),
        McpAddField::Args => texts::tui_label_args().to_string(),
        McpAddField::Url => texts::tui_label_url().to_string(),
        McpAddField::Env => texts::tui_label_env().to_string(),
        McpAddField::AppClaude => texts::tui_label_app_claude().to_string(),
        McpAddField::AppCodex => texts::tui_label_app_codex().to_string(),
        McpAddField::AppGemini => texts::tui_label_app_gemini().to_string(),
        McpAddField::AppOpenCode => texts::tui_label_app_opencode().to_string(),
        McpAddField::AppHermes => texts::tui_label_app_hermes().to_string(),
    };

    let value = match field {
        McpAddField::Type => mcp.server_type.label().to_string(),
        McpAddField::Command => bounded_trimmed_text_for_display(&mcp.command.value),
        McpAddField::Args => mcp_args_summary(mcp),
        McpAddField::Url => bounded_trimmed_text_for_display(&mcp.url.value),
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
        McpAddField::AppOpenCode => {
            if mcp.apps.opencode {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        McpAddField::AppHermes => {
            if mcp.apps.hermes {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        McpAddField::Id | McpAddField::Name => mcp
            .input(field)
            .map(|v| bounded_trimmed_text_for_display(&v.value))
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

fn mcp_args_summary(mcp: &super::form::McpAddFormState) -> String {
    const MAX_SUMMARY_ARGS: usize = 8;
    const MAX_ARG_WIDTH: u16 = 48;

    let (args, hidden) = mcp.args_preview(MAX_SUMMARY_ARGS);
    let mut summary = args
        .into_iter()
        .map(|arg| {
            serde_json::to_string(&truncate_to_display_width(arg, MAX_ARG_WIDTH))
                .unwrap_or_else(|_| "\"\"".to_string())
        })
        .collect::<Vec<_>>()
        .join(" ");
    if hidden > 0 {
        summary.push_str(&format!(" … (+{hidden})"));
    }
    bounded_trimmed_text_for_display(&summary)
}

fn mcp_add_form_key_items(
    focus: FormFocus,
    editing: bool,
    selected_field: Option<McpAddField>,
) -> Vec<(&'static str, &'static str)> {
    if editing && matches!(focus, FormFocus::Fields) {
        return vec![
            ("Enter", texts::tui_key_apply()),
            ("Ctrl+S", texts::tui_key_save()),
            ("Esc", texts::tui_key_cancel()),
        ];
    }

    let mut keys = vec![
        ("Tab", texts::tui_key_focus()),
        ("Ctrl+S", texts::tui_key_save()),
        ("Esc", texts::tui_key_close()),
    ];

    match focus {
        FormFocus::Templates => keys.extend([
            ("←→", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
        ]),
        FormFocus::Fields => {
            if !editing {
                let enter_action = match selected_field {
                    Some(McpAddField::Type | McpAddField::Env) => texts::tui_key_open(),
                    Some(
                        McpAddField::AppClaude
                        | McpAddField::AppCodex
                        | McpAddField::AppGemini
                        | McpAddField::AppOpenCode
                        | McpAddField::AppHermes,
                    ) => texts::tui_key_toggle(),
                    _ => texts::tui_key_edit_mode(),
                };
                keys.extend([("↑↓", texts::tui_key_select()), ("Enter", enter_action)]);
            }
        }
        FormFocus::JsonPreview => {
            keys.push(("↑↓", texts::tui_key_scroll()));
        }
        FormFocus::Content => {}
    }

    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_toggle_key_bar_shows_enter_without_space() {
        let keys = mcp_add_form_key_items(FormFocus::Fields, false, Some(McpAddField::AppClaude));

        assert!(keys
            .iter()
            .any(|(key, label)| { *key == "Enter" && *label == texts::tui_key_toggle() }));
        assert!(keys.iter().all(|(key, _)| *key != "Space"));
    }
}
