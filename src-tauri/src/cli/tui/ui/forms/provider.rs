use super::*;
use serde_json::json;
use std::collections::BTreeSet;

// The regular provider preview reuses the save-shape serializer so the preview
// cannot drift from what Ctrl+S persists. That serializer is intentionally
// allowed to clone its inputs, so rendering must prove the complete source is
// small before calling it. These limits make redraw work independent of an
// imported provider's total size; oversized data remains fully editable and is
// only omitted from the passive preview.
const PROVIDER_JSON_PREVIEW_MAX_SOURCE_BYTES: usize = 64 * 1024;
const PROVIDER_JSON_PREVIEW_MAX_COLLECTION_ITEMS: usize = 128;
const PROVIDER_JSON_PREVIEW_MAX_NODES: usize = 2_048;
const PROVIDER_JSON_PREVIEW_MAX_DEPTH: usize = 16;
const PROVIDER_PASSIVE_PARSE_MAX_BYTES: usize = 8 * 1024;

struct ProviderJsonPreviewBudget {
    bytes_left: usize,
    nodes_left: usize,
}

impl ProviderJsonPreviewBudget {
    fn new() -> Self {
        Self {
            bytes_left: PROVIDER_JSON_PREVIEW_MAX_SOURCE_BYTES,
            nodes_left: PROVIDER_JSON_PREVIEW_MAX_NODES,
        }
    }

    fn take_node(&mut self) -> bool {
        if self.nodes_left == 0 {
            return false;
        }
        self.nodes_left -= 1;
        true
    }

    fn take_text(&mut self, text: &str) -> bool {
        if !self.take_node() || text.len() > self.bytes_left {
            return false;
        }
        self.bytes_left -= text.len();
        true
    }

    fn take_json(&mut self, value: &Value) -> bool {
        self.take_json_at_depth(value, 0)
    }

    fn take_json_at_depth(&mut self, value: &Value, depth: usize) -> bool {
        if depth >= PROVIDER_JSON_PREVIEW_MAX_DEPTH {
            return false;
        }
        if !self.take_node() {
            return false;
        }

        match value {
            Value::Object(map) => {
                if map.len() > PROVIDER_JSON_PREVIEW_MAX_COLLECTION_ITEMS {
                    return false;
                }
                map.iter().all(|(key, value)| {
                    self.take_text(key) && self.take_json_at_depth(value, depth.saturating_add(1))
                })
            }
            Value::Array(items) => {
                if items.len() > PROVIDER_JSON_PREVIEW_MAX_COLLECTION_ITEMS {
                    return false;
                }
                items
                    .iter()
                    .all(|value| self.take_json_at_depth(value, depth.saturating_add(1)))
            }
            Value::String(text) => {
                if text.len() > self.bytes_left {
                    return false;
                }
                self.bytes_left -= text.len();
                true
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => true,
        }
    }

    fn take_json_collection(&mut self, values: &[Value]) -> bool {
        values.len() <= PROVIDER_JSON_PREVIEW_MAX_COLLECTION_ITEMS
            && values.iter().all(|value| self.take_json(value))
    }
}

fn provider_json_preview_source_fits(
    provider: &super::form::ProviderAddFormState,
    common_snippet: &str,
) -> bool {
    let mut budget = ProviderJsonPreviewBudget::new();

    // Keep this list deliberately exhaustive for string data read by
    // `to_provider_json_value*`. Length checks are O(1); once the aggregate
    // budget is exceeded we return before any trim, clone, parse, or merge.
    let scalar_text = [
        provider.id.value.as_str(),
        provider.name.value.as_str(),
        provider.website_url.value.as_str(),
        provider.notes.value.as_str(),
        provider.claude_api_key.value.as_str(),
        provider.claude_base_url.value.as_str(),
        provider.claude_model.value.as_str(),
        provider.claude_haiku_model.value.as_str(),
        provider.claude_sonnet_model.value.as_str(),
        provider.claude_opus_model.value.as_str(),
        provider.codex_base_url.value.as_str(),
        provider.codex_model.value.as_str(),
        provider.codex_env_key.value.as_str(),
        provider.codex_api_key.value.as_str(),
        provider.custom_user_agent.value.as_str(),
        provider.gemini_api_key.value.as_str(),
        provider.gemini_base_url.value.as_str(),
        provider.gemini_model.value.as_str(),
        provider.usage_query_api_key.value.as_str(),
        provider.usage_query_base_url.value.as_str(),
        provider.usage_query_access_token.value.as_str(),
        provider.usage_query_user_id.value.as_str(),
        provider.usage_query_timeout.value.as_str(),
        provider.usage_query_auto_interval.value.as_str(),
        provider.usage_query_code.as_str(),
        provider.usage_query_coding_plan_provider.value.as_str(),
        provider.opencode_npm_package.value.as_str(),
        provider.opencode_api_key.value.as_str(),
        provider.opencode_base_url.value.as_str(),
        provider.opencode_model_id.value.as_str(),
        provider.opencode_model_name.value.as_str(),
        provider.opencode_model_context_limit.value.as_str(),
        provider.opencode_model_output_limit.value.as_str(),
        provider.hermes_api_mode.as_str(),
        provider.hermes_api_key.value.as_str(),
        provider.hermes_base_url.value.as_str(),
        provider.hermes_model_input.value.as_str(),
        provider.hermes_rate_limit_delay.value.as_str(),
    ];
    if !scalar_text.into_iter().all(|text| budget.take_text(text)) {
        return false;
    }

    for text in [
        provider.copy_source_id.as_deref(),
        provider.codex_oauth_account_id.as_deref(),
        provider.opencode_model_original_id(),
    ]
    .into_iter()
    .flatten()
    {
        if !budget.take_text(text) {
            return false;
        }
    }

    for text in [
        provider.codex_chat_reasoning.thinking_param.as_deref(),
        provider.codex_chat_reasoning.effort_param.as_deref(),
        provider.codex_chat_reasoning.effort_value_mode.as_deref(),
        provider.codex_chat_reasoning.output_format.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if !budget.take_text(text) {
            return false;
        }
    }

    if provider.codex_model_catalog.len() > PROVIDER_JSON_PREVIEW_MAX_COLLECTION_ITEMS {
        return false;
    }
    for row in &provider.codex_model_catalog {
        if !budget.take_text(&row.model)
            || !budget.take_text(&row.display_name)
            || !budget.take_text(&row.context_window)
        {
            return false;
        }
    }

    if provider.local_proxy_header_overrides.len() > PROVIDER_JSON_PREVIEW_MAX_COLLECTION_ITEMS {
        return false;
    }
    for (key, value) in &provider.local_proxy_header_overrides {
        if !budget.take_text(key) || !budget.take_text(value) {
            return false;
        }
    }

    if !budget.take_json(&provider.extra)
        || provider
            .local_proxy_body_override
            .as_ref()
            .is_some_and(|value| !budget.take_json(value))
        || !budget.take_json_collection(&provider.openclaw_models)
        || !budget.take_json_collection(&provider.hermes_models)
    {
        return false;
    }

    // The save projection trims the snippet before it checks app support or
    // the form toggle, so the render guard must always account for it too.
    budget.take_text(common_snippet)
}

fn provider_api_format_label(provider: &super::form::ProviderAddFormState) -> String {
    let api_format = provider.claude_api_format.as_str();
    if matches!(provider.app_type, AppType::Codex) {
        texts::tui_codex_api_format_value(api_format).to_string()
    } else {
        texts::tui_claude_api_format_value(api_format).to_string()
    }
}

fn codex_oauth_account_for_display(provider: &super::form::ProviderAddFormState) -> String {
    provider
        .codex_oauth_account_id
        .as_deref()
        .map(bounded_trimmed_text_for_display)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| texts::tui_managed_accounts_follow_default().to_string())
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

    if matches!(
        provider.page,
        super::form::ProviderFormPage::LocalProxySettings
    ) {
        render_local_proxy_settings_form(frame, app, provider, area, theme);
        return;
    }

    if matches!(provider.page, super::form::ProviderFormPage::UsageQuery) {
        render_usage_query_form(frame, app, provider, area, theme);
        return;
    }

    if matches!(
        provider.page,
        super::form::ProviderFormPage::ClaudeQuickConfig
    ) {
        render_quick_config_form(
            frame,
            app,
            provider,
            area,
            theme,
            QuickConfigPage {
                title: texts::tui_label_claude_quick_config(),
                fields: &provider.claude_quick_config_fields(),
                selected_idx: provider.claude_quick_config_idx,
            },
        );
        return;
    }

    if matches!(
        provider.page,
        super::form::ProviderFormPage::CodexQuickConfig
    ) {
        render_quick_config_form(
            frame,
            app,
            provider,
            area,
            theme,
            QuickConfigPage {
                title: texts::tui_label_codex_quick_config(),
                fields: &provider.codex_quick_config_fields(),
                selected_idx: provider.codex_quick_config_idx,
            },
        );
        return;
    }

    let title = match &provider.mode {
        super::form::FormMode::Add => texts::tui_provider_add_title().to_string(),
        super::form::FormMode::Edit { .. } => {
            let name = bounded_trimmed_text_for_display(&provider.name.value);
            texts::tui_provider_edit_title(&name)
        }
    };
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(format!(" {} ", title));
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

    let fields = provider.fields();
    let selected_idx = match provider.text_edit_target() {
        Some(super::form::ProviderTextField::Main(field)) => fields
            .iter()
            .position(|candidate| *candidate == field)
            .unwrap_or(provider.field_idx.min(fields.len().saturating_sub(1))),
        _ => provider.field_idx.min(fields.len().saturating_sub(1)),
    };
    let selected_field_for_keys = fields.get(selected_idx).copied();

    render_key_bar(
        frame,
        chunks[0],
        theme,
        &add_form_key_items(
            provider.focus,
            provider.is_editing_main_text(),
            selected_field_for_keys,
        ),
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

    let split = form_can_split(chunks[2].width, label_col_width, theme);
    let panes = split.then(|| {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
            .split(chunks[2])
    });
    let fields_area = if split {
        Some(panes.as_ref().expect("split panes")[0])
    } else if !matches!(provider.focus, FormFocus::JsonPreview) {
        Some(chunks[2])
    } else {
        None
    };
    let preview_area = if split {
        Some(panes.as_ref().expect("split panes")[1])
    } else if matches!(provider.focus, FormFocus::JsonPreview) {
        Some(chunks[2])
    } else {
        None
    };

    if let Some(fields_area) = fields_area {
        render_provider_inline_fields(
            frame,
            provider,
            &fields,
            &rows_data,
            selected_idx,
            fields_area,
            theme,
        );
    }
    if let Some(preview_area) = preview_area {
        render_provider_preview(frame, provider, data, preview_area, !split, theme);
    }
}

fn render_provider_inline_fields(
    frame: &mut Frame<'_>,
    provider: &super::form::ProviderAddFormState,
    fields: &[ProviderAddField],
    rows_data: &[(String, String)],
    selected_idx: usize,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let fields_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(
            matches!(provider.focus, FormFocus::Fields),
            theme,
        ))
        .title(format!(" {} ", texts::tui_form_fields_title()));
    frame.render_widget(fields_block.clone(), area);
    let table_area = fields_block.inner(area);
    let raw_label_width = field_label_column_width(
        fields
            .iter()
            .zip(rows_data.iter())
            .filter(|(field, _)| !provider_field_is_divider(**field))
            .map(|(_, row)| row.0.as_str())
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
    let editor_active = matches!(provider.focus, FormFocus::Fields);
    let mut cursor_x = None;
    let mut row_heights = Vec::with_capacity(fields.len());
    let mut rows = Vec::with_capacity(fields.len());

    for (idx, (field, (label, value))) in fields.iter().zip(rows_data.iter()).enumerate() {
        if provider_field_is_divider(*field) {
            row_heights.push(1);
            rows.push(
                Row::new(vec![
                    Cell::from(cell_pad(&"┄".repeat(40))),
                    Cell::from("┄".repeat(200)),
                ])
                .style(Style::default().fg(theme.dim)),
            );
            continue;
        }

        let editing = editor_active && provider.is_editing_main_field(*field);
        let display = if editing {
            provider.input(*field).map_or_else(
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
        let error = provider.main_field_error(*field);
        let height = inline_row_height(error, value_width);
        row_heights.push(height);
        rows.push(
            Row::new(vec![
                Cell::from(cell_pad(label)),
                inline_field_cell(display, error, value_width, theme),
            ])
            .height(height),
        );
    }

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

fn render_provider_preview(
    frame: &mut Frame<'_>,
    provider: &super::form::ProviderAddFormState,
    data: &UiData,
    area: Rect,
    compact: bool,
    theme: &super::theme::Theme,
) {
    if matches!(provider.app_type, AppType::Codex) {
        if compact {
            match provider.codex_preview_section {
                CodexPreviewSection::Auth => {
                    let auth_text = codex_auth_preview_text(provider);
                    render_form_text_preview(
                        frame,
                        texts::tui_codex_auth_json_title(),
                        &auth_text,
                        provider.codex_auth_scroll,
                        true,
                        area,
                        theme,
                    );
                }
                CodexPreviewSection::Config => {
                    let config_text = codex_config_preview_text(provider);
                    render_form_text_preview(
                        frame,
                        texts::tui_codex_config_toml_title(),
                        &config_text,
                        provider.codex_config_scroll,
                        true,
                        area,
                        theme,
                    );
                }
            }
            return;
        }

        let auth_text = codex_auth_preview_text(provider);
        let config_text = codex_config_preview_text(provider);

        let preview = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

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
            &config_text,
            provider.codex_config_scroll,
            config_active,
            preview[1],
            theme,
        );
    } else {
        // JSON Preview (settingsConfig only, matching upstream UI)
        if !provider_json_preview_source_fits(provider, &data.config.common_snippet) {
            render_form_json_preview_with_title(
                frame,
                texts::tui_provider_config_title(),
                texts::tui_preview_omitted_too_large(),
                0,
                matches!(provider.focus, FormFocus::JsonPreview),
                area,
                theme,
            );
            return;
        }

        // Schema parity matters here, so reuse the save projection only after
        // the complete source has passed the hard budget above. In particular,
        // no unbounded secret, URL, nested object, or common snippet reaches
        // this allocating path during a redraw.
        let provider_json_value = provider
            .to_provider_json_value_with_common_config(&data.config.common_snippet)
            .unwrap_or_else(|_| provider.to_provider_json_value());
        let json_value = provider_json_value
            .get("settingsConfig")
            .map(bounded_json_preview)
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        let json_text =
            serde_json::to_string_pretty(&json_value).unwrap_or_else(|_| "{}".to_string());
        let highlighted_lines = common_json_preview_highlight_lines(
            &provider.app_type,
            &json_value,
            &json_text,
            &data.config.common_snippet,
            provider.include_common_config,
        );
        render_form_json_preview_with_highlights_and_title(
            frame,
            &json_text,
            provider.json_scroll,
            matches!(provider.focus, FormFocus::JsonPreview),
            area,
            theme,
            (texts::tui_provider_config_title(), &highlighted_lines),
        );
    }
}

pub(crate) fn codex_auth_preview_text(provider: &super::form::ProviderAddFormState) -> String {
    let mut auth_value = provider
        .extra
        .get("settingsConfig")
        .and_then(Value::as_object)
        .and_then(|settings| settings.get("auth"))
        .filter(|auth| auth.is_object())
        .map(bounded_json_preview)
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));

    if !provider.is_codex_official_provider() {
        let api_key = bounded_trimmed_text_for_display(&provider.codex_api_key.value);
        if let Value::Object(auth) = &mut auth_value {
            if !api_key.is_empty() {
                auth.insert("OPENAI_API_KEY".to_string(), Value::String(api_key));
            } else {
                auth.remove("OPENAI_API_KEY");
            }
        }
    }

    serde_json::to_string_pretty(&auth_value).unwrap_or_else(|_| "{}".to_string())
}

pub(crate) fn codex_config_preview_text(provider: &super::form::ProviderAddFormState) -> String {
    const MAX_PREVIEW_CATALOG_ROWS: usize = 128;

    let scalar_bytes = [
        provider.existing_codex_config_text(),
        provider.id.value.as_str(),
        provider.name.value.as_str(),
        provider.codex_base_url.value.as_str(),
        provider.codex_model.value.as_str(),
        provider.codex_env_key.value.as_str(),
    ]
    .into_iter()
    .try_fold(0usize, |total, text| total.checked_add(text.len()));
    let catalog_is_too_large = provider.codex_local_routing_enabled()
        && (provider.codex_model_catalog.len() > MAX_PREVIEW_CATALOG_ROWS
            || provider
                .codex_model_catalog
                .iter()
                .try_fold(0usize, |total, row| total.checked_add(row.model.len()))
                .is_none_or(|bytes| bytes > TOML_PREVIEW_MAX_INPUT_BYTES));

    if scalar_bytes.is_none_or(|bytes| bytes > TOML_PREVIEW_MAX_INPUT_BYTES) || catalog_is_too_large
    {
        return texts::tui_preview_omitted_too_large().to_string();
    }
    bounded_toml_preview(&provider.effective_codex_config_text())
}

struct QuickConfigPage<'a> {
    title: &'a str,
    fields: &'a [ProviderAddField],
    selected_idx: usize,
}

fn render_quick_config_form(
    frame: &mut Frame<'_>,
    app: &App,
    provider: &super::form::ProviderAddFormState,
    area: Rect,
    theme: &super::theme::Theme,
    page: QuickConfigPage<'_>,
) {
    let QuickConfigPage {
        title,
        fields,
        selected_idx,
    } = page;
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(format!(" {title} "));
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_key_bar(frame, chunks[0], theme, &quick_config_form_key_items());

    let fields_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(true, theme))
        .title(format!(" {} ", texts::tui_form_fields_title()));
    frame.render_widget(fields_block.clone(), chunks[1]);
    let fields_inner = fields_block.inner(chunks[1]);

    let rows_data = fields
        .iter()
        .map(|field| provider_field_label_and_value(provider, *field))
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
        Row::new(vec![
            Cell::from(cell_pad(label)),
            Cell::from(truncated_value_cell(
                value,
                fields_inner.width,
                label_col_width,
                theme,
            )),
        ])
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
        state.select(Some(selected_idx.min(fields.len() - 1)));
    }
    frame.render_stateful_widget(table, fields_inner, &mut state);
}

fn render_local_proxy_settings_form(
    frame: &mut Frame<'_>,
    app: &App,
    provider: &super::form::ProviderAddFormState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(format!(" {} ", texts::tui_label_local_proxy_settings()));
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let fields = provider.local_proxy_settings_fields();
    let selected = provider.selected_local_proxy_settings_field();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);
    render_key_bar(
        frame,
        chunks[0],
        theme,
        &local_proxy_settings_form_key_items(selected),
    );

    let fields_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(true, theme))
        .title(format!(" {} ", texts::tui_form_fields_title()));
    frame.render_widget(fields_block.clone(), chunks[1]);
    let fields_inner = fields_block.inner(chunks[1]);
    let rows_data = fields
        .iter()
        .map(|field| local_proxy_settings_field_label_and_value(provider, *field))
        .collect::<Vec<_>>();
    let label_col_width = field_label_column_width(
        rows_data
            .iter()
            .map(|(label, _)| label.as_str())
            .chain(std::iter::once(texts::tui_header_field())),
        1,
    );
    let header = Row::new(vec![
        Cell::from(cell_pad(texts::tui_header_field())),
        Cell::from(texts::tui_header_value()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));
    let highlight_width = unicode_width::UnicodeWidthStr::width(highlight_symbol(theme)) as u16;
    let value_width = fields_inner
        .width
        .saturating_sub(highlight_width)
        .saturating_sub(label_col_width)
        .saturating_sub(1);
    let rows = fields.iter().zip(rows_data.iter()).map(|(field, row)| {
        let value_style = if matches!(field, super::form::LocalProxySettingsField::UserAgent)
            && !provider.custom_user_agent_is_valid()
        {
            Style::default().fg(theme.err)
        } else {
            Style::default()
        };
        let value = truncate_to_display_width(&row.1, value_width);
        Row::new(vec![
            Cell::from(cell_pad(&row.0)),
            Cell::from(value).style(value_style),
        ])
    });
    let table = Table::new(
        rows,
        [Constraint::Length(label_col_width), Constraint::Min(8)],
    )
    .header(header)
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));
    let mut state = TableState::default();
    state.select(Some(
        provider
            .local_proxy_settings_field_idx
            .min(fields.len().saturating_sub(1)),
    ));
    frame.render_stateful_widget(table, fields_inner, &mut state);
}

pub(crate) fn local_proxy_settings_field_label_and_value(
    provider: &super::form::ProviderAddFormState,
    field: super::form::LocalProxySettingsField,
) -> (String, String) {
    match field {
        super::form::LocalProxySettingsField::UserAgent => {
            let value = bounded_trimmed_text_for_display(&provider.custom_user_agent.value);
            (
                texts::tui_label_custom_user_agent().to_string(),
                if value.is_empty() {
                    texts::tui_na().to_string()
                } else {
                    value
                },
            )
        }
        super::form::LocalProxySettingsField::HeaderOverrides => (
            texts::tui_label_local_proxy_header_overrides().to_string(),
            provider.local_proxy_header_overrides_summary(),
        ),
        super::form::LocalProxySettingsField::BodyOverrides => (
            texts::tui_label_local_proxy_body_overrides().to_string(),
            provider.local_proxy_body_override_summary(),
        ),
    }
}

fn render_codex_local_routing_form(
    frame: &mut Frame<'_>,
    app: &App,
    provider: &super::form::ProviderAddFormState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let name = bounded_trimmed_text_for_display(&provider.name.value);
    let title = texts::tui_codex_local_routing_title(&name);
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(format!(" {} ", title));
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

    // Only the toggle + reasoning fields render as the top field table; the
    // model catalog is shown inline as a full-width editable table below.
    let top_fields: Vec<_> = fields
        .iter()
        .copied()
        .filter(|field| !matches!(field, super::form::CodexLocalRoutingField::ModelCatalog))
        .collect();
    let on_table = matches!(
        selected_field,
        Some(super::form::CodexLocalRoutingField::ModelCatalog)
    );
    let show_table = provider.codex_local_routing_enabled();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_key_bar(
        frame,
        chunks[0],
        theme,
        &codex_local_routing_form_key_items(selected_field),
    );

    let body = if show_table {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(top_fields.len() as u16 + 3),
                Constraint::Min(0),
            ])
            .split(chunks[1])
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0)])
            .split(chunks[1])
    };

    let fields_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(!on_table, theme))
        .title(format!(" {} ", texts::tui_form_fields_title()));
    frame.render_widget(fields_block.clone(), body[0]);
    let fields_inner = fields_block.inner(body[0]);

    let rows_data = top_fields
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
        Row::new(vec![
            Cell::from(cell_pad(label)),
            Cell::from(truncated_value_cell(
                value,
                fields_inner.width,
                label_col_width,
                theme,
            )),
        ])
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
    if !on_table && !top_fields.is_empty() {
        state.select(Some(
            provider
                .codex_local_routing_field_idx
                .min(top_fields.len() - 1),
        ));
    }
    frame.render_stateful_widget(table, fields_inner, &mut state);

    if show_table {
        render_codex_model_catalog_inline(frame, provider, body[1], theme, on_table);
    }
}

/// Full-width inline model-catalog table shown inside the model-mapping page.
/// `active` highlights the current cell when the table zone is focused.
fn render_codex_model_catalog_inline(
    frame: &mut Frame<'_>,
    provider: &super::form::ProviderAddFormState,
    area: Rect,
    theme: &super::theme::Theme,
    active: bool,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(active, theme));
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

    let header = Row::new(vec![
        Cell::from(cell_pad(texts::tui_codex_model_catalog_model_header())),
        Cell::from(cell_pad(texts::tui_codex_model_catalog_display_header())),
        Cell::from(cell_pad(texts::tui_codex_model_catalog_context_header())),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let selected_idx = provider
        .codex_model_catalog_idx
        .min(provider.codex_model_catalog.len().saturating_sub(1));
    let selected_field = provider.codex_model_catalog_field;
    let row_capacity = inner.height.saturating_sub(1) as usize;
    let visible = visible_selection_window(
        provider.codex_model_catalog.len(),
        selected_idx,
        row_capacity.max(1),
    );
    let rows = visible.map(|idx| {
        let row = &provider.codex_model_catalog[idx];
        let model = bounded_trimmed_text_for_display(&row.model);
        let display_name = bounded_trimmed_text_for_display(&row.display_name);
        let context_window = codex_model_catalog_context_for_display(&row.context_window);
        let cell = |value: &str, field: super::form::CodexModelCatalogField| {
            if active {
                codex_model_catalog_cell(value, idx, selected_idx, field, selected_field, theme)
            } else {
                Cell::from(cell_pad(value))
            }
        };
        Row::new(vec![
            cell(&model, super::form::CodexModelCatalogField::Model),
            cell(
                &display_name,
                super::form::CodexModelCatalogField::DisplayName,
            ),
            cell(
                &context_window,
                super::form::CodexModelCatalogField::ContextWindow,
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
    frame.render_widget(table, inner);
}

fn render_codex_model_catalog_form(
    frame: &mut Frame<'_>,
    app: &App,
    provider: &super::form::ProviderAddFormState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let name = bounded_trimmed_text_for_display(&provider.name.value);
    let title = texts::tui_codex_model_catalog_title(&name);
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
        .title(format!(" {} ", texts::tui_codex_model_catalog()));
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
    let row_capacity = table_area.height.saturating_sub(1) as usize;
    let visible = visible_selection_window(
        provider.codex_model_catalog.len(),
        selected_idx,
        row_capacity.max(1),
    );
    let rows = visible.map(|idx| {
        let row = &provider.codex_model_catalog[idx];
        let model = bounded_trimmed_text_for_display(&row.model);
        let display_name = bounded_trimmed_text_for_display(&row.display_name);
        let context_window = codex_model_catalog_context_for_display(&row.context_window);
        Row::new(vec![
            codex_model_catalog_cell(
                &model,
                idx,
                selected_idx,
                super::form::CodexModelCatalogField::Model,
                selected_field,
                theme,
            ),
            codex_model_catalog_cell(
                &display_name,
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

fn codex_model_catalog_context_for_display(raw: &str) -> String {
    if raw.len() > PROVIDER_PASSIVE_PARSE_MAX_BYTES {
        return bounded_trimmed_text_for_display(raw);
    }
    bounded_trimmed_text_for_display(&super::form::codex_model_catalog_context_window_label(raw))
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

fn render_usage_query_form(
    frame: &mut Frame<'_>,
    app: &App,
    provider: &super::form::ProviderAddFormState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let name = bounded_trimmed_text_for_display(&provider.name.value);
    let title = texts::tui_usage_query_title(&name);
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(format!(" {} ", title));
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let fields = provider.usage_query_table_fields();
    let selected_idx = match provider.text_edit_target() {
        Some(super::form::ProviderTextField::UsageQuery(field)) => fields
            .iter()
            .position(|candidate| *candidate == field)
            .unwrap_or(
                provider
                    .usage_query_field_idx
                    .min(fields.len().saturating_sub(1)),
            ),
        _ => provider
            .usage_query_field_idx
            .min(fields.len().saturating_sub(1)),
    };
    let selected_field = fields.get(selected_idx).copied();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_key_bar(
        frame,
        chunks[0],
        theme,
        &usage_query_form_key_items(
            provider.focus,
            provider.is_editing_usage_query_text(),
            selected_field,
            provider.usage_query_extractor_available(),
        ),
    );

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

    let split = form_can_split(chunks[1].width, label_col_width, theme);
    let panes = split.then(|| {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
            .split(chunks[1])
    });
    if split || matches!(provider.focus, FormFocus::Fields) {
        let fields_area = if split {
            panes.as_ref().expect("split panes")[0]
        } else {
            chunks[1]
        };
        render_usage_query_inline_fields(
            frame,
            provider,
            &fields,
            &rows_data,
            selected_idx,
            fields_area,
            theme,
        );
    }
    if split {
        render_usage_query_side_panel(
            frame,
            provider,
            panes.as_ref().expect("split panes")[1],
            theme,
        );
    } else {
        match provider.focus {
            FormFocus::JsonPreview => {
                render_usage_query_script_preview(frame, provider, true, chunks[1], theme)
            }
            FormFocus::Content => render_usage_query_script_help(frame, true, chunks[1], theme),
            FormFocus::Templates | FormFocus::Fields => {}
        }
    }
}

fn render_usage_query_inline_fields(
    frame: &mut Frame<'_>,
    provider: &super::form::ProviderAddFormState,
    fields: &[super::form::UsageQueryField],
    rows_data: &[(String, String)],
    selected_idx: usize,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(
            matches!(provider.focus, FormFocus::Fields),
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
            let editing = matches!(provider.focus, FormFocus::Fields)
                && provider.is_editing_usage_query_field(*field);
            let display = if editing {
                provider.usage_query_input(*field).map_or_else(
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
            let error = provider.usage_query_field_error(*field);
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
        .title(format!(" {} ", texts::tui_usage_query_info()));
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
        .title(format!(
            " {} ",
            texts::tui_usage_query_script_preview_title()
        ));
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);

    let script = usage_query_script_preview(provider);
    let script_preview = script.trim();
    let mut lines = Vec::new();
    if let Some(error) = provider.usage_query_field_error(super::form::UsageQueryField::Script) {
        lines.push(Line::styled(
            format!("! {error}"),
            Style::default().fg(theme.err),
        ));
        if !script_preview.is_empty() {
            lines.push(Line::raw(""));
        }
    }
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

fn usage_query_script_preview(provider: &super::form::ProviderAddFormState) -> String {
    if provider.usage_query_code.len() > PROVIDER_PASSIVE_PARSE_MAX_BYTES {
        return texts::tui_preview_omitted_too_large().to_string();
    }
    provider.usage_query_code.clone()
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
        .title(format!(" {} ", texts::tui_usage_query_script_help_title()));
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
            .map(|input| bounded_trimmed_text_for_display(&input.value))
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
        ProviderAddField::ClaudeApiFormat => {
            if provider.app_type == AppType::Codex {
                texts::tui_label_codex_upstream_format().to_string()
            } else {
                texts::tui_label_claude_api_format().to_string()
            }
        }
        ProviderAddField::ClaudeApiKey => texts::tui_label_api_key().to_string(),
        ProviderAddField::ClaudeModelConfig => texts::tui_label_claude_model_config().to_string(),
        ProviderAddField::ClaudeFallbackModel => {
            texts::tui_label_claude_fallback_model().to_string()
        }
        ProviderAddField::ClaudeQuickConfig => texts::tui_label_claude_quick_config().to_string(),
        ProviderAddField::CodexQuickConfig => texts::tui_label_codex_quick_config().to_string(),
        ProviderAddField::CodexGoalMode => texts::tui_label_codex_goal_mode().to_string(),
        ProviderAddField::CodexRemoteCompaction => {
            texts::tui_label_codex_remote_compaction().to_string()
        }
        ProviderAddField::ClaudeHideAttribution => {
            texts::tui_label_claude_hide_attribution().to_string()
        }
        ProviderAddField::ClaudeTeammates => texts::tui_label_claude_teammates().to_string(),
        ProviderAddField::ClaudeToolSearch => texts::tui_label_claude_tool_search().to_string(),
        ProviderAddField::ClaudeDisableAutoUpgrade => {
            texts::tui_label_claude_disable_auto_upgrade().to_string()
        }
        ProviderAddField::CodexOAuthAccount => texts::tui_label_chatgpt_account().to_string(),
        ProviderAddField::CodexFastMode => texts::tui_label_codex_fast_mode().to_string(),
        ProviderAddField::CodexBaseUrl => texts::tui_label_base_url().to_string(),
        ProviderAddField::CodexModel => texts::model_label().to_string(),
        ProviderAddField::CodexLocalRouting => texts::tui_label_codex_model_mapping().to_string(),
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
        ProviderAddField::LocalProxySettings => texts::tui_label_local_proxy_settings().to_string(),
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
        ProviderAddField::ClaudeAdvancedDivider => "- - - - - - - - -".to_string(),
        ProviderAddField::CodexAdvancedDivider => "- - - - - - - - -".to_string(),
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
        ProviderAddField::ClaudeQuickConfig => {
            texts::tui_claude_quick_config_summary(provider.claude_quick_config_enabled_count())
        }
        ProviderAddField::CodexQuickConfig => texts::tui_codex_quick_config_summary(
            provider.codex_quick_config_enabled_count(),
            provider.codex_quick_config_fields().len(),
        ),
        ProviderAddField::CodexGoalMode => {
            if provider.codex_goal_mode {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        ProviderAddField::CodexRemoteCompaction => {
            if provider.codex_remote_compaction {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        ProviderAddField::ClaudeHideAttribution => {
            if provider.claude_hide_attribution {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        ProviderAddField::ClaudeTeammates => {
            if provider.claude_teammates {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        ProviderAddField::ClaudeToolSearch => {
            if provider.claude_tool_search {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        ProviderAddField::ClaudeDisableAutoUpgrade => {
            if provider.claude_disable_auto_upgrade {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        ProviderAddField::CodexOAuthAccount => codex_oauth_account_for_display(provider),
        ProviderAddField::CodexFastMode => {
            if provider.codex_fast_mode {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        ProviderAddField::CodexLocalRouting => {
            if provider.codex_local_routing_enabled {
                format!(
                    "{} · {} ›",
                    texts::enabled(),
                    provider.codex_model_catalog_summary()
                )
            } else {
                texts::disabled().to_string()
            }
        }
        ProviderAddField::LocalProxySettings => provider.local_proxy_settings_summary(),
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
            bounded_trimmed_text_for_display(&provider.opencode_npm_package.value)
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
        ProviderAddField::HermesRateLimitDelay => {
            bounded_trimmed_text_for_display(&provider.hermes_rate_limit_delay.value)
        }
        ProviderAddField::HermesAdvancedDivider => "- - - - - - - - - -".to_string(),
        ProviderAddField::CommonConfigDivider => "- - - - - - - - - -".to_string(),
        ProviderAddField::CommonSnippet => format!("{} ›", texts::tui_key_open()),
        ProviderAddField::UsageQueryDivider => String::new(),
        ProviderAddField::UsageQuery => {
            if provider.usage_query_enabled {
                format!(
                    "{} · {} ›",
                    texts::enabled(),
                    provider.usage_query_template_value()
                )
            } else {
                texts::disabled().to_string()
            }
        }
        _ => provider
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
                texts::tui_toggle_on().to_string()
            } else {
                texts::tui_toggle_off().to_string()
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

fn provider_field_is_divider(field: ProviderAddField) -> bool {
    matches!(
        field,
        ProviderAddField::ClaudeAdvancedDivider
            | ProviderAddField::CodexAdvancedDivider
            | ProviderAddField::HermesAdvancedDivider
            | ProviderAddField::CommonConfigDivider
            | ProviderAddField::UsageQueryDivider
    )
}
