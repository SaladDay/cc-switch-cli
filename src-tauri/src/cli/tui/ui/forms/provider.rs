use super::*;
use serde_json::json;
use std::collections::BTreeSet;

fn provider_api_format_label(provider: &super::form::ProviderAddFormState) -> String {
    let api_format = provider.claude_api_format.as_str();
    if matches!(provider.app_type, AppType::Codex) {
        texts::tui_codex_api_format_value(api_format).to_string()
    } else {
        texts::tui_claude_api_format_value(api_format).to_string()
    }
}

fn should_redact_provider_field(
    provider: &super::form::ProviderAddFormState,
    field: ProviderAddField,
) -> bool {
    matches!(provider.app_type, AppType::OpenClaw)
        && matches!(field, ProviderAddField::OpenCodeApiKey)
}

fn common_json_preview_value(app_type: &AppType, common_snippet: &str) -> Option<Value> {
    if common_snippet.trim().is_empty() {
        return None;
    }

    match app_type {
        AppType::Claude => serde_json::from_str::<Value>(common_snippet).ok(),
        AppType::Gemini => serde_json::from_str::<Value>(common_snippet)
            .ok()
            .map(|env| json!({ "env": env })),
        AppType::Codex | AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => None,
    }
    .filter(Value::is_object)
}

fn sorted_json_object_keys(value: &Value) -> Vec<String> {
    value
        .as_object()
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default()
}

fn mark_common_json_lines(
    full: &Value,
    common: &Value,
    path: &mut Vec<String>,
    lines: &[&str],
    highlighted: &mut BTreeSet<usize>,
) {
    let Some(common_obj) = common.as_object() else {
        return;
    };

    for key in sorted_json_object_keys(common) {
        let Some(common_child) = common_obj.get(&key) else {
            continue;
        };
        let Some(full_child) = full.get(&key) else {
            continue;
        };
        path.push(key.clone());

        let key_line = find_json_path_line(lines, path);
        if !common_child.is_object() {
            if let Some(line_idx) = key_line {
                highlighted.insert(line_idx);
            }
        } else if let Some(common_child_obj) = common_child.as_object() {
            if common_child_obj.is_empty() {
                if let Some(line_idx) = key_line {
                    highlighted.insert(line_idx);
                }
            } else {
                mark_common_json_lines(full_child, common_child, path, lines, highlighted);
            }
        }

        path.pop();
    }
}

fn find_json_path_line(lines: &[&str], path: &[String]) -> Option<usize> {
    let mut stack: Vec<String> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let closing = trimmed
            .chars()
            .take_while(|ch| *ch == '}' || *ch == ']')
            .count();
        for _ in 0..closing.min(stack.len()) {
            stack.pop();
        }

        if let Some(key) = json_line_key(trimmed) {
            let mut candidate = stack.clone();
            candidate.push(key.to_string());
            if candidate == path {
                return Some(idx);
            }
            if json_line_opens_container(trimmed) {
                stack.push(key.to_string());
            }
        }
    }

    None
}

fn json_line_key(trimmed_line: &str) -> Option<&str> {
    let rest = trimmed_line.strip_prefix('"')?;
    let (key, rest) = rest.split_once("\":")?;
    if rest.trim_start().is_empty() {
        return None;
    }
    Some(key)
}

fn json_line_opens_container(trimmed_line: &str) -> bool {
    let Some((_, rest)) = trimmed_line.split_once("\":") else {
        return false;
    };
    let value = rest.trim_start();
    value.starts_with('{') || value.starts_with('[')
}

fn common_json_preview_highlight_lines(
    app_type: &AppType,
    json_value: &Value,
    json_text: &str,
    common_snippet: &str,
    include_common_config: bool,
) -> BTreeSet<usize> {
    if !include_common_config {
        return BTreeSet::new();
    }

    let Some(common) = common_json_preview_value(app_type, common_snippet) else {
        return BTreeSet::new();
    };

    let lines = json_text.lines().collect::<Vec<_>>();
    let mut highlighted = BTreeSet::new();
    mark_common_json_lines(
        json_value,
        &common,
        &mut Vec::new(),
        &lines,
        &mut highlighted,
    );
    highlighted
}

pub(crate) fn render_provider_add_form(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    provider: &super::form::ProviderAddFormState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    if matches!(
        provider.page,
        super::form::ProviderFormPage::CodexLocalRouting
    ) {
        render_codex_local_routing_form(frame, app, provider, area, theme);
        return;
    }

    if matches!(
        provider.page,
        super::form::ProviderFormPage::CodexModelCatalog
    ) {
        render_codex_model_catalog_form(frame, app, provider, area, theme);
        return;
    }

    if matches!(provider.page, super::form::ProviderFormPage::UsageQuery) {
        render_usage_query_form(frame, app, provider, area, theme);
        return;
    }

    let title = match &provider.mode {
        super::form::FormMode::Add => texts::tui_provider_add_title().to_string(),
        super::form::FormMode::Edit { .. } => {
            texts::tui_provider_edit_title(provider.name.value.trim())
        }
    };
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(title);
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let template_height = if matches!(provider.mode, super::form::FormMode::Add) {
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

    let selected_field_for_keys = provider
        .fields()
        .get(
            provider
                .field_idx
                .min(provider.fields().len().saturating_sub(1)),
        )
        .copied();

    render_key_bar(
        frame,
        chunks[0],
        theme,
        &add_form_key_items(provider.focus, provider.editing, selected_field_for_keys),
    );

    if matches!(provider.mode, super::form::FormMode::Add) {
        let labels = provider.template_labels();
        render_form_template_chips(
            frame,
            &labels,
            provider.template_idx,
            matches!(provider.focus, FormFocus::Templates),
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
            matches!(provider.focus, FormFocus::Fields),
            theme,
        ))
        .title(texts::tui_form_fields_title());
    frame.render_widget(fields_block.clone(), body[0]);
    let fields_inner = fields_block.inner(body[0]);

    let fields_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Min(0), Constraint::Length(3)])
        .split(fields_inner);

    let fields = provider.fields();
    let rows_data = fields
        .iter()
        .map(|field| provider_field_label_and_value(provider, *field))
        .collect::<Vec<_>>();

    let label_col_width = field_label_column_width(
        fields
            .iter()
            .zip(rows_data.iter())
            .filter(|(field, _row)| !provider_field_is_divider(**field))
            .map(|(_field, (label, _value))| label.as_str())
            .chain(std::iter::once(texts::tui_header_field())),
        1,
    );

    let header = Row::new(vec![
        Cell::from(cell_pad(texts::tui_header_field())),
        Cell::from(texts::tui_header_value()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let rows = fields
        .iter()
        .zip(rows_data.iter())
        .map(|(field, (label, value))| {
            if provider_field_is_divider(*field) {
                let dashes_left = "┄".repeat(40);
                let dashes_right = "┄".repeat(200);
                Row::new(vec![
                    Cell::from(cell_pad(&dashes_left)),
                    Cell::from(dashes_right),
                ])
                .style(Style::default().fg(theme.dim))
            } else {
                Row::new(vec![Cell::from(cell_pad(label)), Cell::from(value.clone())])
            }
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
        state.select(Some(provider.field_idx.min(fields.len() - 1)));
    }
    let table_area = fields_chunks[0];
    let editor_area = fields_chunks[1];
    frame.render_stateful_widget(table, table_area, &mut state);

    // Editor / help line
    let editor_active = matches!(provider.focus, FormFocus::Fields) && provider.editing;
    let editor_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(editor_active, theme))
        .title(if editor_active {
            texts::tui_form_editing_title()
        } else {
            texts::tui_form_input_title()
        });
    frame.render_widget(editor_block.clone(), editor_area);
    let editor_inner = editor_block.inner(editor_area);

    let selected = fields
        .get(provider.field_idx.min(fields.len().saturating_sub(1)))
        .copied();
    if let Some(field) = selected {
        if let Some(input) = provider.input(field) {
            if !editor_active && should_redact_provider_field(provider, field) {
                frame.render_widget(
                    Paragraph::new(Line::raw(redacted_secret_placeholder()))
                        .wrap(Wrap { trim: false }),
                    editor_inner,
                );
            } else {
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
        } else {
            let (line, _cursor_col) =
                provider_field_editor_line(provider, selected, editor_inner.width as usize);
            frame.render_widget(
                Paragraph::new(line).wrap(Wrap { trim: false }),
                editor_inner,
            );
        }
    } else {
        frame.render_widget(
            Paragraph::new(Line::raw("")).wrap(Wrap { trim: false }),
            editor_inner,
        );
    }

    if matches!(provider.app_type, AppType::Codex) {
        let provider_json_value = provider.to_provider_json_value();
        let settings_value = provider_json_value
            .get("settingsConfig")
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));

        let auth_value = settings_value
            .get("auth")
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        let auth_value = if auth_value.is_object() {
            auth_value
        } else {
            Value::Object(serde_json::Map::new())
        };
        let auth_text =
            serde_json::to_string_pretty(&auth_value).unwrap_or_else(|_| "{}".to_string());

        let config_text = settings_value
            .get("config")
            .and_then(|value| value.as_str())
            .unwrap_or("");

        let preview = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(body[1]);

        let preview_active = matches!(provider.focus, FormFocus::JsonPreview);
        let auth_active =
            preview_active && matches!(provider.codex_preview_section, CodexPreviewSection::Auth);
        let config_active =
            preview_active && matches!(provider.codex_preview_section, CodexPreviewSection::Config);

        render_form_text_preview(
            frame,
            texts::tui_codex_auth_json_title(),
            &auth_text,
            provider.codex_auth_scroll,
            auth_active,
            preview[0],
            theme,
        );
        render_form_text_preview(
            frame,
            texts::tui_codex_config_toml_title(),
            config_text,
            provider.codex_config_scroll,
            config_active,
            preview[1],
            theme,
        );
    } else {
        // JSON Preview (settingsConfig only, matching upstream UI)
        let provider_json_value = provider
            .to_provider_json_value_with_common_config(&data.config.common_snippet)
            .unwrap_or_else(|_| provider.to_provider_json_value());
        let json_value = provider_json_value
            .get("settingsConfig")
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        let json_value = if matches!(provider.app_type, AppType::OpenClaw) {
            redact_sensitive_json(&json_value)
        } else {
            json_value
        };
        let json_text =
            serde_json::to_string_pretty(&json_value).unwrap_or_else(|_| "{}".to_string());
        let highlighted_lines = common_json_preview_highlight_lines(
            &provider.app_type,
            &json_value,
            &json_text,
            &data.config.common_snippet,
            provider.include_common_config,
        );
        render_form_json_preview_with_highlights(
            frame,
            &json_text,
            provider.json_scroll,
            matches!(provider.focus, FormFocus::JsonPreview),
            body[1],
            theme,
            &highlighted_lines,
        );
    }
}

fn render_codex_local_routing_form(
    frame: &mut Frame<'_>,
    app: &App,
    provider: &super::form::ProviderAddFormState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let title = texts::tui_codex_local_routing_title(provider.name.value.trim());
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(title);
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let fields = provider.codex_local_routing_fields();
    let selected_field = fields
        .get(
            provider
                .codex_local_routing_field_idx
                .min(fields.len().saturating_sub(1)),
        )
        .copied();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(inner);

    render_key_bar(
        frame,
        chunks[0],
        theme,
        &codex_local_routing_form_key_items(selected_field),
    );

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(chunks[1]);

    let fields_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(
            matches!(provider.focus, FormFocus::Fields),
            theme,
        ))
        .title(texts::tui_form_fields_title());
    frame.render_widget(fields_block.clone(), body[0]);
    let fields_inner = fields_block.inner(body[0]);

    let rows_data = fields
        .iter()
        .map(|field| codex_local_routing_field_label_and_value(provider, *field))
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
        state.select(Some(
            provider.codex_local_routing_field_idx.min(fields.len() - 1),
        ));
    }
    frame.render_stateful_widget(table, fields_inner, &mut state);

    render_codex_model_catalog_preview(frame, provider, body[1], theme);
    render_codex_local_routing_input(frame, provider, selected_field, chunks[2], theme);
}

fn render_codex_model_catalog_preview(
    frame: &mut Frame<'_>,
    provider: &super::form::ProviderAddFormState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.dim))
        .title(texts::tui_codex_model_catalog_preview_title());
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);

    if provider.codex_model_catalog.is_empty() {
        frame.render_widget(
            Paragraph::new(texts::tui_codex_model_catalog_empty())
                .style(Style::default().fg(theme.dim))
                .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let rows = provider
        .codex_model_catalog
        .iter()
        .enumerate()
        .map(|(idx, row)| {
            Row::new(vec![Cell::from(cell_pad(&format!(
                "{}. {}",
                idx + 1,
                row.summary_label()
            )))])
        })
        .collect::<Vec<_>>();

    let table =
        Table::new(rows, [Constraint::Min(10)]).block(Block::default().borders(Borders::NONE));
    frame.render_widget(table, inner);
}

fn render_codex_model_catalog_form(
    frame: &mut Frame<'_>,
    app: &App,
    provider: &super::form::ProviderAddFormState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let title = texts::tui_codex_model_catalog_title(provider.name.value.trim());
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

    render_key_bar(
        frame,
        chunks[0],
        theme,
        &codex_model_catalog_form_key_items(!provider.codex_model_catalog.is_empty()),
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(true, theme))
        .title(texts::tui_codex_model_catalog());
    frame.render_widget(block.clone(), chunks[1]);
    let table_area = block.inner(chunks[1]);

    if provider.codex_model_catalog.is_empty() {
        frame.render_widget(
            Paragraph::new(texts::tui_codex_model_catalog_empty())
                .style(Style::default().fg(theme.dim))
                .alignment(Alignment::Center),
            table_area,
        );
        return;
    }

    let header = Row::new(vec![
        Cell::from(cell_pad(texts::tui_codex_model_catalog_model_header())),
        Cell::from(texts::tui_codex_model_catalog_display_header()),
        Cell::from(texts::tui_codex_model_catalog_context_header()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let selected_idx = provider
        .codex_model_catalog_idx
        .min(provider.codex_model_catalog.len().saturating_sub(1));
    let selected_field = provider.codex_model_catalog_field;
    let rows = provider
        .codex_model_catalog
        .iter()
        .enumerate()
        .map(|(idx, row)| {
            let context_window =
                super::form::codex_model_catalog_context_window_label(&row.context_window);
            Row::new(vec![
                codex_model_catalog_cell(
                    &row.model,
                    idx,
                    selected_idx,
                    super::form::CodexModelCatalogField::Model,
                    selected_field,
                    theme,
                ),
                codex_model_catalog_cell(
                    &row.display_name,
                    idx,
                    selected_idx,
                    super::form::CodexModelCatalogField::DisplayName,
                    selected_field,
                    theme,
                ),
                codex_model_catalog_cell(
                    &context_window,
                    idx,
                    selected_idx,
                    super::form::CodexModelCatalogField::ContextWindow,
                    selected_field,
                    theme,
                ),
            ])
        });

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(45),
            Constraint::Percentage(35),
            Constraint::Percentage(20),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::NONE));

    frame.render_widget(table, table_area);
}

fn codex_model_catalog_cell<'a>(
    value: &str,
    row_idx: usize,
    selected_idx: usize,
    field: super::form::CodexModelCatalogField,
    selected_field: super::form::CodexModelCatalogField,
    theme: &super::theme::Theme,
) -> Cell<'a> {
    Cell::from(cell_pad(value)).style(codex_model_catalog_cell_style(
        row_idx,
        selected_idx,
        field,
        selected_field,
        theme,
    ))
}

fn codex_model_catalog_cell_style(
    row_idx: usize,
    selected_idx: usize,
    field: super::form::CodexModelCatalogField,
    selected_field: super::form::CodexModelCatalogField,
    theme: &super::theme::Theme,
) -> Style {
    if row_idx == selected_idx && field == selected_field {
        selection_style(theme)
    } else if row_idx == selected_idx {
        if theme.no_color {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.accent)
        }
    } else {
        Style::default()
    }
}

fn render_codex_local_routing_input(
    frame: &mut Frame<'_>,
    provider: &super::form::ProviderAddFormState,
    selected: Option<super::form::CodexLocalRoutingField>,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.dim))
        .title(texts::tui_form_input_title());
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);

    let (line, _cursor_col) = codex_local_routing_field_editor_line(provider, selected);
    frame.render_widget(Paragraph::new(line).wrap(Wrap { trim: false }), inner);
}

fn render_usage_query_form(
    frame: &mut Frame<'_>,
    app: &App,
    provider: &super::form::ProviderAddFormState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let title = texts::tui_usage_query_title(provider.name.value.trim());
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(title);
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let fields = provider.usage_query_table_fields();
    let selected_field = fields
        .get(
            provider
                .usage_query_field_idx
                .min(fields.len().saturating_sub(1)),
        )
        .copied();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(inner);

    render_key_bar(
        frame,
        chunks[0],
        theme,
        &usage_query_form_key_items(
            provider.focus,
            provider.usage_query_editing,
            selected_field,
            provider.usage_query_extractor_available(),
        ),
    );

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(chunks[1]);

    let fields_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(
            matches!(provider.focus, FormFocus::Fields),
            theme,
        ))
        .title(texts::tui_form_fields_title());
    frame.render_widget(fields_block.clone(), body[0]);
    let fields_inner = fields_block.inner(body[0]);

    let rows_data = fields
        .iter()
        .map(|field| usage_query_field_label_and_value(provider, *field))
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
        state.select(Some(provider.usage_query_field_idx.min(fields.len() - 1)));
    }
    frame.render_stateful_widget(table, fields_inner, &mut state);

    render_usage_query_side_panel(frame, provider, body[1], theme);
    render_usage_query_input(frame, provider, selected_field, chunks[2], theme);
}

fn render_usage_query_side_panel(
    frame: &mut Frame<'_>,
    provider: &super::form::ProviderAddFormState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let extractor_available = provider.usage_query_extractor_available();
    if !extractor_available {
        render_usage_query_info_panel(frame, provider, area, theme);
        return;
    }

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);
    render_usage_query_script_preview(
        frame,
        provider,
        matches!(provider.focus, FormFocus::JsonPreview),
        sections[0],
        theme,
    );
    render_usage_query_script_help(
        frame,
        matches!(provider.focus, FormFocus::Content),
        sections[1],
        theme,
    );
}

fn render_usage_query_info_panel(
    frame: &mut Frame<'_>,
    provider: &super::form::ProviderAddFormState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.dim))
        .title(texts::tui_usage_query_info());
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);

    let hint = match provider.usage_query_template {
        super::form::UsageQueryTemplate::GitHubCopilot => {
            texts::tui_usage_query_copilot_auto_auth()
        }
        super::form::UsageQueryTemplate::TokenPlan => texts::tui_usage_query_token_plan_hint(),
        super::form::UsageQueryTemplate::Custom
        | super::form::UsageQueryTemplate::General
        | super::form::UsageQueryTemplate::NewApi
        | super::form::UsageQueryTemplate::Balance => "",
    };

    frame.render_widget(
        Paragraph::new(Line::styled(
            hint.to_string(),
            Style::default().fg(theme.comment),
        ))
        .wrap(Wrap { trim: false }),
        inner,
    );
}

fn render_usage_query_script_preview(
    frame: &mut Frame<'_>,
    provider: &super::form::ProviderAddFormState,
    active: bool,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(active, theme))
        .title(texts::tui_usage_query_script_preview_title());
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);

    let script_preview = provider.usage_query_code.trim();
    let mut lines = Vec::new();
    if matches!(
        provider.usage_query_template,
        super::form::UsageQueryTemplate::Balance
    ) {
        lines.push(Line::styled(
            texts::tui_usage_query_balance_hint().to_string(),
            Style::default().fg(theme.comment),
        ));
        if !script_preview.is_empty() {
            lines.push(Line::raw(""));
        }
    }

    let max_lines = inner.height.saturating_sub(lines.len() as u16) as usize;
    for line in script_preview.lines().take(max_lines.max(1)) {
        lines.push(Line::styled(
            line.to_string(),
            Style::default().fg(theme.comment),
        ));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_usage_query_script_help(
    frame: &mut Frame<'_>,
    active: bool,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(active, theme))
        .title(texts::tui_usage_query_script_help_title());
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);

    let lines = super::form::ProviderAddFormState::usage_query_script_help_lines()
        .into_iter()
        .enumerate()
        .map(|(idx, line)| {
            if idx == 0 || idx == 19 || idx == 30 {
                Line::styled(line, Style::default().fg(theme.comment))
            } else {
                Line::raw(line)
            }
        })
        .collect::<Vec<_>>();

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_usage_query_input(
    frame: &mut Frame<'_>,
    provider: &super::form::ProviderAddFormState,
    selected: Option<super::form::UsageQueryField>,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let editor_active = provider.usage_query_editing;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(editor_active, theme))
        .title(if editor_active {
            texts::tui_form_editing_title()
        } else {
            texts::tui_form_input_title()
        });
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);

    if let Some(field) = selected {
        if let Some(input) = provider.usage_query_input(field) {
            let (visible, cursor_x) =
                visible_text_window(&input.value, input.cursor, inner.width as usize);
            frame.render_widget(
                Paragraph::new(Line::raw(visible)).wrap(Wrap { trim: false }),
                inner,
            );
            if editor_active {
                let x = inner.x + cursor_x.min(inner.width.saturating_sub(1));
                frame.set_cursor_position((x, inner.y));
            }
        } else {
            let (line, _cursor_col) =
                usage_query_field_editor_line(provider, selected, inner.width as usize);
            frame.render_widget(Paragraph::new(line).wrap(Wrap { trim: false }), inner);
        }
    }
}

pub(crate) fn usage_query_field_label_and_value(
    provider: &super::form::ProviderAddFormState,
    field: super::form::UsageQueryField,
) -> (String, String) {
    let label = match field {
        super::form::UsageQueryField::Enabled => texts::tui_usage_query_enable().to_string(),
        super::form::UsageQueryField::Template => texts::tui_usage_query_template().to_string(),
        super::form::UsageQueryField::ApiKey => {
            if matches!(
                provider.usage_query_template,
                super::form::UsageQueryTemplate::General
            ) {
                format!(
                    "{} ({})",
                    texts::tui_label_api_key(),
                    texts::tui_usage_query_optional()
                )
            } else {
                texts::tui_label_api_key().to_string()
            }
        }
        super::form::UsageQueryField::BaseUrl => {
            if matches!(
                provider.usage_query_template,
                super::form::UsageQueryTemplate::General
            ) {
                format!(
                    "{} ({})",
                    texts::tui_usage_query_base_url(),
                    texts::tui_usage_query_optional()
                )
            } else {
                texts::tui_usage_query_base_url().to_string()
            }
        }
        super::form::UsageQueryField::AccessToken => {
            texts::tui_usage_query_access_token().to_string()
        }
        super::form::UsageQueryField::UserId => texts::tui_usage_query_user_id().to_string(),
        super::form::UsageQueryField::Timeout => {
            texts::tui_usage_query_timeout_seconds().to_string()
        }
        super::form::UsageQueryField::AutoInterval => {
            texts::tui_usage_query_auto_interval().to_string()
        }
        super::form::UsageQueryField::CodingPlanProvider => {
            texts::tui_usage_query_coding_plan_provider().to_string()
        }
        super::form::UsageQueryField::Script => texts::tui_usage_query_script().to_string(),
    };

    let value = match field {
        super::form::UsageQueryField::Enabled => {
            if provider.usage_query_enabled {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        super::form::UsageQueryField::Template => provider.usage_query_template_label().to_string(),
        super::form::UsageQueryField::Script => texts::tui_key_open().to_string(),
        _ => provider
            .usage_query_input(field)
            .map(|input| input.value.trim().to_string())
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

pub(crate) fn usage_query_field_editor_line(
    provider: &super::form::ProviderAddFormState,
    selected: Option<super::form::UsageQueryField>,
    _width: usize,
) -> (Line<'static>, usize) {
    let Some(field) = selected else {
        return (Line::raw(""), 0);
    };

    if let Some(input) = provider.usage_query_input(field) {
        (Line::raw(input.value.clone()), input.cursor)
    } else {
        let text = match field {
            super::form::UsageQueryField::Enabled => {
                format!("enabled = {}", provider.usage_query_enabled)
            }
            super::form::UsageQueryField::Template => {
                format!("templateType = {}", provider.usage_query_template_label())
            }
            super::form::UsageQueryField::Script => {
                format!(
                    "{} ({})",
                    texts::tui_key_open(),
                    provider.usage_query_template_value()
                )
            }
            _ => String::new(),
        };
        (Line::raw(text), 0)
    }
}

pub(crate) fn provider_field_label_and_value(
    provider: &super::form::ProviderAddFormState,
    field: ProviderAddField,
) -> (String, String) {
    let label = match field {
        ProviderAddField::Id if provider.app_type == AppType::Hermes => {
            texts::tui_label_hermes_provider_key().to_string()
        }
        ProviderAddField::Id => texts::tui_label_id().to_string(),
        ProviderAddField::Name => texts::header_name().to_string(),
        ProviderAddField::WebsiteUrl => {
            strip_trailing_colon(texts::website_url_label()).to_string()
        }
        ProviderAddField::Notes => strip_trailing_colon(texts::notes_label()).to_string(),
        ProviderAddField::ClaudeBaseUrl => texts::tui_label_base_url().to_string(),
        ProviderAddField::ClaudeApiFormat => texts::tui_label_claude_api_format().to_string(),
        ProviderAddField::ClaudeApiKey => texts::tui_label_api_key().to_string(),
        ProviderAddField::ClaudeModelConfig => texts::tui_label_claude_model_config().to_string(),
        ProviderAddField::ClaudeHideAttribution => {
            texts::tui_label_claude_hide_attribution().to_string()
        }
        ProviderAddField::CodexOAuthAccount => texts::tui_label_chatgpt_account().to_string(),
        ProviderAddField::CodexFastMode => texts::tui_label_codex_fast_mode().to_string(),
        ProviderAddField::CodexBaseUrl => texts::tui_label_base_url().to_string(),
        ProviderAddField::CodexModel => texts::model_label().to_string(),
        ProviderAddField::CodexLocalRouting => texts::tui_label_codex_local_routing().to_string(),
        ProviderAddField::CodexWireApi => {
            strip_trailing_colon(texts::codex_wire_api_label()).to_string()
        }
        ProviderAddField::CodexRequiresOpenaiAuth => {
            strip_trailing_colon(texts::codex_auth_mode_label()).to_string()
        }
        ProviderAddField::CodexEnvKey => {
            strip_trailing_colon(texts::codex_env_key_label()).to_string()
        }
        ProviderAddField::CodexApiKey => texts::tui_label_api_key().to_string(),
        ProviderAddField::GeminiAuthType => {
            strip_trailing_colon(texts::auth_type_label()).to_string()
        }
        ProviderAddField::GeminiApiKey => texts::tui_label_api_key().to_string(),
        ProviderAddField::GeminiBaseUrl => texts::tui_label_base_url().to_string(),
        ProviderAddField::GeminiModel => texts::model_label().to_string(),
        ProviderAddField::OpenClawApiProtocol => texts::tui_label_openclaw_api().to_string(),
        ProviderAddField::OpenClawUserAgent => texts::tui_label_openclaw_user_agent().to_string(),
        ProviderAddField::OpenClawModels => texts::tui_label_openclaw_models().to_string(),
        ProviderAddField::OpenCodeNpmPackage => {
            if provider.app_type == AppType::OpenClaw {
                texts::tui_label_openclaw_api().to_string()
            } else {
                texts::tui_label_provider_package().to_string()
            }
        }
        ProviderAddField::OpenCodeApiKey => texts::tui_label_api_key().to_string(),
        ProviderAddField::OpenCodeBaseUrl => texts::tui_label_base_url().to_string(),
        ProviderAddField::OpenCodeModelId => texts::tui_label_opencode_model_id().to_string(),
        ProviderAddField::OpenCodeModelName => texts::tui_label_opencode_model_name().to_string(),
        ProviderAddField::OpenCodeModelContextLimit => texts::tui_label_context_limit().to_string(),
        ProviderAddField::OpenCodeModelOutputLimit => texts::tui_label_output_limit().to_string(),
        ProviderAddField::HermesApiMode => texts::tui_label_hermes_api_mode().to_string(),
        ProviderAddField::HermesApiKey => texts::tui_label_api_key().to_string(),
        ProviderAddField::HermesBaseUrl => texts::tui_label_hermes_base_url().to_string(),
        ProviderAddField::HermesModels => texts::tui_label_hermes_models().to_string(),
        ProviderAddField::HermesRateLimitDelay => {
            texts::tui_label_hermes_rate_limit_delay().to_string()
        }
        ProviderAddField::HermesAdvancedDivider => "- - - - - - - - -".to_string(),
        ProviderAddField::CommonConfigDivider => "- - - - - - - - -".to_string(),
        ProviderAddField::CommonSnippet => texts::tui_config_item_common_snippet().to_string(),
        ProviderAddField::IncludeCommonConfig => texts::tui_form_attach_common_config().to_string(),
        ProviderAddField::UsageQueryDivider => "- - - - - - - - -".to_string(),
        ProviderAddField::UsageQuery => texts::tui_config_item_usage_query().to_string(),
    };

    let value = match field {
        ProviderAddField::ClaudeApiFormat => provider_api_format_label(provider),
        ProviderAddField::CodexWireApi => provider.codex_wire_api.as_str().to_string(),
        ProviderAddField::CodexRequiresOpenaiAuth => {
            if provider.codex_requires_openai_auth {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        ProviderAddField::ClaudeModelConfig => {
            texts::tui_claude_model_config_summary(provider.claude_model_configured_count())
        }
        ProviderAddField::ClaudeHideAttribution => {
            if provider.claude_hide_attribution {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        ProviderAddField::CodexOAuthAccount => provider.codex_oauth_account_display(),
        ProviderAddField::CodexFastMode => {
            if provider.codex_fast_mode {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        ProviderAddField::CodexLocalRouting => texts::tui_key_open().to_string(),
        ProviderAddField::IncludeCommonConfig => {
            if provider.include_common_config {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        ProviderAddField::GeminiAuthType => match provider.gemini_auth_type {
            GeminiAuthType::OAuth => "oauth".to_string(),
            GeminiAuthType::ApiKey => "api_key".to_string(),
        },
        ProviderAddField::OpenClawApiProtocol => {
            provider.opencode_npm_package.value.trim().to_string()
        }
        ProviderAddField::OpenClawUserAgent => {
            if provider.openclaw_user_agent {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        ProviderAddField::OpenClawModels => provider.openclaw_models_summary(),
        ProviderAddField::HermesApiMode => {
            texts::tui_hermes_api_mode_value(provider.hermes_api_mode_value()).to_string()
        }
        ProviderAddField::HermesModels => provider.hermes_models_summary(),
        ProviderAddField::HermesRateLimitDelay => provider.hermes_rate_limit_delay.value.clone(),
        ProviderAddField::HermesAdvancedDivider => "- - - - - - - - - -".to_string(),
        ProviderAddField::CommonConfigDivider => "- - - - - - - - - -".to_string(),
        ProviderAddField::CommonSnippet => texts::tui_key_open().to_string(),
        ProviderAddField::UsageQueryDivider => String::new(),
        ProviderAddField::UsageQuery => texts::tui_key_open().to_string(),
        _ => provider
            .input(field)
            .map(|v| {
                if should_redact_provider_field(provider, field) && !v.value.trim().is_empty() {
                    redacted_secret_placeholder().to_string()
                } else {
                    v.value.trim().to_string()
                }
            })
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

pub(crate) fn provider_field_editor_line(
    provider: &super::form::ProviderAddFormState,
    selected: Option<ProviderAddField>,
    _width: usize,
) -> (Line<'static>, usize) {
    let Some(field) = selected else {
        return (Line::raw(""), 0);
    };

    if let Some(input) = provider.input(field) {
        let shown = if matches!(
            field,
            ProviderAddField::ClaudeApiKey
                | ProviderAddField::CodexApiKey
                | ProviderAddField::GeminiApiKey
                | ProviderAddField::OpenCodeApiKey
                | ProviderAddField::HermesApiKey
        ) {
            input.value.clone()
        } else {
            input.value.clone()
        };
        (Line::raw(shown), input.cursor)
    } else {
        let text = match field {
            ProviderAddField::ClaudeApiFormat => {
                let value = if matches!(provider.app_type, AppType::Codex) {
                    texts::tui_codex_api_format_value(provider.claude_api_format.as_str())
                } else {
                    texts::tui_claude_api_format_value(provider.claude_api_format.as_str())
                };
                format!("api_format = {}", value)
            }
            ProviderAddField::CodexWireApi => {
                format!("wire_api = {}", provider.codex_wire_api.as_str())
            }
            ProviderAddField::CodexRequiresOpenaiAuth => format!(
                "requires_openai_auth = {}",
                provider.codex_requires_openai_auth
            ),
            ProviderAddField::ClaudeModelConfig => {
                texts::tui_claude_model_config_open_hint().to_string()
            }
            ProviderAddField::ClaudeHideAttribution => {
                format!(
                    "attribution.commit/pr = {}",
                    if provider.claude_hide_attribution {
                        "\"\""
                    } else {
                        "<default>"
                    }
                )
            }
            ProviderAddField::CodexOAuthAccount => texts::tui_key_open().to_string(),
            ProviderAddField::CodexFastMode => {
                format!("codex_fast_mode = {}", provider.codex_fast_mode)
            }
            ProviderAddField::CodexLocalRouting => {
                format!("local_routing = {}", provider.codex_local_routing_enabled())
            }
            ProviderAddField::CommonConfigDivider => String::new(),
            ProviderAddField::IncludeCommonConfig => {
                format!("apply_common_config = {}", provider.include_common_config)
            }
            ProviderAddField::GeminiAuthType => {
                format!("auth_type = {}", provider.gemini_auth_type.as_str())
            }
            ProviderAddField::OpenClawApiProtocol => {
                format!("api = {}", provider.opencode_npm_package.value.trim())
            }
            ProviderAddField::OpenClawUserAgent => {
                format!("send_user_agent = {}", provider.openclaw_user_agent)
            }
            ProviderAddField::OpenClawModels => texts::tui_openclaw_models_open_hint().to_string(),
            ProviderAddField::HermesApiMode => {
                format!("api_mode = {}", provider.hermes_api_mode_value())
            }
            ProviderAddField::HermesModels => texts::tui_hermes_models_open_hint().to_string(),
            ProviderAddField::HermesAdvancedDivider => String::new(),
            _ => String::new(),
        };
        (Line::raw(text), 0)
    }
}

pub(crate) fn codex_local_routing_field_label_and_value(
    provider: &super::form::ProviderAddFormState,
    field: super::form::CodexLocalRoutingField,
) -> (String, String) {
    let label = match field {
        super::form::CodexLocalRoutingField::Enabled => {
            texts::tui_codex_local_routing_enable().to_string()
        }
        super::form::CodexLocalRoutingField::SupportsThinking => {
            texts::tui_codex_reasoning_supports_thinking().to_string()
        }
        super::form::CodexLocalRoutingField::SupportsEffort => {
            texts::tui_codex_reasoning_supports_effort().to_string()
        }
        super::form::CodexLocalRoutingField::ModelCatalog => {
            texts::tui_codex_model_catalog().to_string()
        }
    };

    let value = match field {
        super::form::CodexLocalRoutingField::Enabled => {
            if provider.codex_local_routing_enabled() {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        super::form::CodexLocalRoutingField::SupportsThinking => {
            if provider.codex_reasoning_supports_thinking() {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        super::form::CodexLocalRoutingField::SupportsEffort => {
            if provider.codex_reasoning_supports_effort() {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        super::form::CodexLocalRoutingField::ModelCatalog => provider.codex_model_catalog_summary(),
    };

    (label, value)
}

pub(crate) fn codex_local_routing_field_editor_line(
    provider: &super::form::ProviderAddFormState,
    selected: Option<super::form::CodexLocalRoutingField>,
) -> (Line<'static>, usize) {
    let text = match selected {
        Some(super::form::CodexLocalRoutingField::Enabled) => {
            format!("local_routing = {}", provider.codex_local_routing_enabled())
        }
        Some(super::form::CodexLocalRoutingField::SupportsThinking) => {
            format!(
                "supportsThinking = {}",
                provider.codex_reasoning_supports_thinking()
            )
        }
        Some(super::form::CodexLocalRoutingField::SupportsEffort) => {
            format!(
                "supportsEffort = {}",
                provider.codex_reasoning_supports_effort()
            )
        }
        Some(super::form::CodexLocalRoutingField::ModelCatalog) => {
            texts::tui_codex_model_catalog().to_string()
        }
        None => String::new(),
    };
    (Line::raw(text), 0)
}

fn provider_field_is_divider(field: ProviderAddField) -> bool {
    matches!(
        field,
        ProviderAddField::HermesAdvancedDivider
            | ProviderAddField::CommonConfigDivider
            | ProviderAddField::UsageQueryDivider
    )
}
