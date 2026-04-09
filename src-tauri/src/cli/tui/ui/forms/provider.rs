use super::*;

fn claude_api_format_label(api_format: crate::cli::tui::form::ClaudeApiFormat) -> String {
    texts::tui_claude_api_format_value(api_format.as_str()).to_string()
}

fn should_redact_provider_field(
    provider: &super::form::ProviderAddFormState,
    field: ProviderAddField,
) -> bool {
    matches!(provider.app_type, AppType::OpenClaw)
        && matches!(field, ProviderAddField::OpenCodeApiKey)
}

pub(crate) fn render_provider_add_form(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    provider: &super::form::ProviderAddFormState,
    area: Rect,
    theme: &super::theme::Theme,
) {
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
            .filter(|(field, _row)| !matches!(field, ProviderAddField::CommonConfigDivider))
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
            if matches!(field, ProviderAddField::CommonConfigDivider) {
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
        render_form_json_preview(
            frame,
            &json_text,
            provider.json_scroll,
            matches!(provider.focus, FormFocus::JsonPreview),
            body[1],
            theme,
        );
    }
}

pub(crate) fn provider_field_label_and_value(
    provider: &super::form::ProviderAddFormState,
    field: ProviderAddField,
) -> (String, String) {
    let label = match field {
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
        ProviderAddField::CodexBaseUrl => texts::tui_label_base_url().to_string(),
        ProviderAddField::CodexModel => texts::model_label().to_string(),
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
        ProviderAddField::CommonConfigDivider => "- - - - - - - - -".to_string(),
        ProviderAddField::CommonSnippet => texts::tui_config_item_common_snippet().to_string(),
        ProviderAddField::IncludeCommonConfig => texts::tui_form_attach_common_config().to_string(),
    };

    let value = match field {
        ProviderAddField::ClaudeApiFormat => claude_api_format_label(provider.claude_api_format),
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
        ProviderAddField::CommonConfigDivider => "- - - - - - - - - -".to_string(),
        ProviderAddField::CommonSnippet => texts::tui_key_open().to_string(),
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
        ) {
            input.value.clone()
        } else {
            input.value.clone()
        };
        (Line::raw(shown), input.cursor)
    } else {
        let text = match field {
            ProviderAddField::ClaudeApiFormat => {
                format!(
                    "api_format = {}",
                    texts::tui_claude_api_format_value(provider.claude_api_format.as_str())
                )
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
            _ => String::new(),
        };
        (Line::raw(text), 0)
    }
}
