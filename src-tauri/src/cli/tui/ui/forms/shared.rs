use super::super::*;
use std::collections::BTreeSet;

pub(crate) fn focus_block_style(active: bool, theme: &super::theme::Theme) -> Style {
    if active {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.dim)
    }
}

pub(crate) fn add_form_key_items(
    focus: FormFocus,
    editing: bool,
    selected_field: Option<ProviderAddField>,
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
                    Some(
                        ProviderAddField::ClaudeModelConfig
                        | ProviderAddField::ClaudeQuickConfig
                        | ProviderAddField::CodexQuickConfig
                        | ProviderAddField::CodexOAuthAccount
                        | ProviderAddField::CodexLocalRouting
                        | ProviderAddField::LocalProxySettings
                        | ProviderAddField::CommonSnippet
                        | ProviderAddField::UsageQuery
                        | ProviderAddField::OpenClawModels
                        | ProviderAddField::HermesModels,
                    ) => texts::tui_key_open(),
                    Some(
                        ProviderAddField::ClaudeHideAttribution
                        | ProviderAddField::ClaudeTeammates
                        | ProviderAddField::ClaudeToolSearch
                        | ProviderAddField::ClaudeDisableAutoUpgrade
                        | ProviderAddField::CodexGoalMode
                        | ProviderAddField::CodexRemoteCompaction
                        | ProviderAddField::CodexFastMode
                        | ProviderAddField::OpenClawUserAgent
                        | ProviderAddField::IncludeCommonConfig,
                    ) => texts::tui_key_toggle(),
                    Some(
                        ProviderAddField::GeminiAuthType
                        | ProviderAddField::OpenClawApiProtocol
                        | ProviderAddField::HermesApiMode,
                    ) => texts::tui_key_select(),
                    _ => texts::tui_key_edit_mode(),
                };
                keys.extend([("↑↓", texts::tui_key_select()), ("Enter", enter_action)]);
                if matches!(
                    selected_field,
                    Some(
                        ProviderAddField::CodexModel
                            | ProviderAddField::GeminiModel
                            | ProviderAddField::OpenCodeModelId
                    )
                ) {
                    keys.push(("f", texts::tui_key_fetch_model()));
                }
            }
        }
        FormFocus::JsonPreview => {
            keys.extend([
                ("Enter", texts::tui_key_edit_mode()),
                ("↑↓", texts::tui_key_scroll()),
            ]);
        }
        FormFocus::Content => {}
    }

    keys
}

pub(crate) fn quick_config_form_key_items() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Ctrl+S", texts::tui_key_save()),
        ("Esc", texts::tui_key_no()),
        ("↑↓", texts::tui_key_select()),
        ("Enter", texts::tui_key_toggle()),
    ]
}

pub(crate) fn codex_local_routing_form_key_items(
    selected_field: Option<super::form::CodexLocalRoutingField>,
) -> Vec<(&'static str, &'static str)> {
    if matches!(
        selected_field,
        Some(super::form::CodexLocalRoutingField::ModelCatalog)
    ) {
        // Focus is on the inline model-catalog table.
        return vec![
            ("Ctrl+S", texts::tui_key_save()),
            ("Esc", texts::tui_key_no()),
            ("↑↓", texts::tui_key_select()),
            ("←→", texts::tui_key_select()),
            ("Enter", texts::tui_key_edit()),
            ("a", texts::tui_key_add()),
            ("f", texts::tui_key_fetch_model()),
            ("Del", texts::tui_key_delete()),
        ];
    }

    vec![
        ("Ctrl+S", texts::tui_key_save()),
        ("Esc", texts::tui_key_no()),
        ("↑↓", texts::tui_key_select()),
        ("Enter", texts::tui_key_toggle()),
    ]
}

pub(crate) fn local_proxy_settings_form_key_items(
    selected_field: Option<super::form::LocalProxySettingsField>,
) -> Vec<(&'static str, &'static str)> {
    let mut keys = vec![
        ("Ctrl+S", texts::tui_key_save()),
        ("Esc", texts::tui_key_no()),
        ("↑↓", texts::tui_key_select()),
    ];
    match selected_field {
        Some(super::form::LocalProxySettingsField::UserAgent) => {
            keys.push(("Enter", texts::tui_key_select()));
        }
        Some(
            super::form::LocalProxySettingsField::HeaderOverrides
            | super::form::LocalProxySettingsField::BodyOverrides,
        ) => keys.push(("Enter", texts::tui_key_open())),
        None => {}
    }
    keys.push(("?", texts::tui_help_title()));
    keys
}

pub(crate) fn codex_model_catalog_form_key_items(
    has_rows: bool,
) -> Vec<(&'static str, &'static str)> {
    let mut keys = vec![
        ("Ctrl+S", texts::tui_key_save()),
        ("Esc", texts::tui_key_no()),
        ("f", texts::tui_key_fetch_model()),
        ("+", texts::tui_key_add()),
    ];
    if has_rows {
        keys.extend([
            ("↑↓", texts::tui_key_select()),
            ("←→", texts::tui_key_select()),
            ("Enter", texts::tui_key_edit()),
            ("Del", texts::tui_key_delete()),
        ]);
    }
    keys
}

pub(crate) fn usage_query_form_key_items(
    focus: FormFocus,
    editing: bool,
    selected_field: Option<super::form::UsageQueryField>,
    extractor_available: bool,
) -> Vec<(&'static str, &'static str)> {
    let mut keys = vec![
        ("Ctrl+S", texts::tui_key_save()),
        ("Esc", texts::tui_key_no()),
    ];
    if extractor_available {
        keys.insert(0, ("Tab", texts::tui_key_focus()));
    }

    match focus {
        FormFocus::Fields => {
            if editing {
                return vec![
                    ("Enter", texts::tui_key_apply()),
                    ("Ctrl+S", texts::tui_key_save()),
                    ("Esc", texts::tui_key_cancel()),
                ];
            }
            keys.push(("↑↓", texts::tui_key_select()));
            if !editing {
                let enter_action = match selected_field {
                    Some(super::form::UsageQueryField::Enabled) => texts::tui_key_toggle(),
                    Some(
                        super::form::UsageQueryField::Template
                        | super::form::UsageQueryField::CodingPlanProvider,
                    ) => texts::tui_key_select(),
                    Some(super::form::UsageQueryField::Script) => texts::tui_key_open(),
                    Some(_) => texts::tui_key_edit_mode(),
                    None => texts::tui_key_edit_mode(),
                };
                keys.push(("Enter", enter_action));
            }
        }
        FormFocus::JsonPreview => {
            if extractor_available {
                keys.push(("Enter", texts::tui_key_open()));
            }
        }
        FormFocus::Content => {
            if extractor_available {
                keys.push(("Enter", texts::tui_key_view()));
            }
        }
        FormFocus::Templates => {}
    }

    keys
}

pub(crate) fn render_form_template_chips(
    frame: &mut Frame<'_>,
    labels: &[&str],
    selected_idx: usize,
    active: bool,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let template_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(active, theme))
        .title(format!(" {} ", texts::tui_form_templates_title()));
    frame.render_widget(template_block.clone(), area);
    let template_inner = template_block.inner(area);

    let mut spans: Vec<Span<'static>> = Vec::new();
    for (idx, label) in labels.iter().enumerate() {
        let selected = idx == selected_idx;
        let style = if selected {
            active_chip_style(theme)
        } else {
            inactive_chip_style(theme)
        };
        spans.push(Span::styled(format!(" {label} "), style));
        spans.push(Span::raw(" "));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).wrap(Wrap { trim: false }),
        template_inner,
    );
}

pub(crate) fn visible_text_window(text: &str, cursor: usize, width: usize) -> (String, u16) {
    if width == 0 {
        return (String::new(), 0);
    }

    // TextInput cursors are UTF-8 byte offsets. Walk outwards from that byte
    // boundary only far enough to fill the viewport; never scan the prefix or
    // classify the complete value first. Bound zero-width/combining characters
    // too, otherwise a tiny terminal window could still materialize a huge
    // string.
    const MAX_CHARS_PER_COLUMN: usize = 4;
    const EXTRA_ZERO_WIDTH_CHARS: usize = 16;
    let max_visible_chars = width
        .saturating_mul(MAX_CHARS_PER_COLUMN)
        .saturating_add(EXTRA_ZERO_WIDTH_CHARS);
    let cursor_byte = super::form::TextInput::clamp_byte_boundary(text, cursor);
    let mut start_byte = cursor_byte;
    let mut cursor_width = 0usize;
    for (chars_before_cursor, (byte, ch)) in text[..cursor_byte].char_indices().rev().enumerate() {
        if chars_before_cursor >= max_visible_chars {
            break;
        }
        let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if char_width > 0 && cursor_width.saturating_add(char_width) > width.saturating_sub(1) {
            break;
        }
        start_byte = byte;
        cursor_width = cursor_width.saturating_add(char_width);
    }

    let mut visible = String::new();
    let mut visible_width = 0usize;
    for ch in text[start_byte..].chars().take(max_visible_chars) {
        let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if char_width > 0 && visible_width.saturating_add(char_width) > width {
            break;
        }
        visible.push(ch);
        visible_width = visible_width.saturating_add(char_width);
    }

    (visible, cursor_width.min(width) as u16)
}

pub(crate) const FORM_VALUE_MIN_WIDTH: u16 = 24;
pub(crate) const FORM_SPLIT_MIN_BODY_WIDTH: u16 = 84;
pub(crate) fn form_value_width(
    table_width: u16,
    label_col_width: u16,
    theme: &super::theme::Theme,
) -> u16 {
    let symbol_width = UnicodeWidthStr::width(highlight_symbol(theme)) as u16;
    table_width
        .saturating_sub(symbol_width)
        .saturating_sub(label_col_width)
        .saturating_sub(1)
}

pub(crate) fn form_can_split(
    area_width: u16,
    _label_col_width: u16,
    _theme: &super::theme::Theme,
) -> bool {
    area_width >= FORM_SPLIT_MIN_BODY_WIDTH
}

pub(crate) fn inline_input_window(input: &super::form::TextInput, width: u16) -> (String, u16) {
    visible_text_window(&input.value, input.cursor, width as usize)
}

pub(crate) fn inline_field_cell(
    value: String,
    error: Option<&str>,
    width: u16,
    theme: &super::theme::Theme,
) -> Cell<'static> {
    let Some(error) = error else {
        return Cell::from(value);
    };
    let mut lines = vec![Line::raw(value)];
    lines.extend(
        super::app::EditorState::wrap_line_segments(&format!("! {error}"), width.max(1))
            .into_iter()
            .map(|line| Line::styled(line, Style::default().fg(theme.err))),
    );
    Cell::from(Text::from(lines))
}

pub(crate) fn inline_row_height(error: Option<&str>, width: u16) -> u16 {
    error.map_or(1, |message| {
        1_u16.saturating_add(super::app::EditorState::wrapped_line_height(
            &format!("! {message}"),
            width.max(1),
        ) as u16)
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn set_inline_table_cursor(
    frame: &mut Frame<'_>,
    table_area: Rect,
    label_col_width: u16,
    selected_index: usize,
    table_offset: usize,
    row_heights: &[u16],
    cursor_x: u16,
    theme: &super::theme::Theme,
) {
    if selected_index < table_offset {
        return;
    }
    let rows_before = row_heights
        .get(table_offset..selected_index)
        .unwrap_or_default()
        .iter()
        .copied()
        .sum::<u16>();
    let y = table_area.y.saturating_add(1).saturating_add(rows_before);
    let symbol_width = UnicodeWidthStr::width(highlight_symbol(theme)) as u16;
    let value_width = form_value_width(table_area.width, label_col_width, theme);
    let x = table_area
        .x
        .saturating_add(symbol_width)
        .saturating_add(label_col_width)
        .saturating_add(1)
        .saturating_add(cursor_x.min(value_width.saturating_sub(1)));
    if x < table_area.right() && y < table_area.bottom() {
        frame.set_cursor_position((x, y));
    }
}

pub(crate) fn render_form_json_preview(
    frame: &mut Frame<'_>,
    json_text: &str,
    scroll: usize,
    active: bool,
    area: Rect,
    theme: &super::theme::Theme,
) {
    render_form_json_preview_with_highlights_and_title(
        frame,
        json_text,
        scroll,
        active,
        area,
        theme,
        (texts::tui_form_json_title(), &BTreeSet::new()),
    );
}

pub(crate) fn render_form_json_preview_with_title(
    frame: &mut Frame<'_>,
    title: &str,
    json_text: &str,
    scroll: usize,
    active: bool,
    area: Rect,
    theme: &super::theme::Theme,
) {
    render_form_json_preview_with_highlights_and_title(
        frame,
        json_text,
        scroll,
        active,
        area,
        theme,
        (title, &BTreeSet::new()),
    );
}

pub(crate) fn render_form_json_preview_with_highlights_and_title(
    frame: &mut Frame<'_>,
    json_text: &str,
    scroll: usize,
    active: bool,
    area: Rect,
    theme: &super::theme::Theme,
    title_and_highlights: (&str, &BTreeSet<usize>),
) {
    let (title, highlighted_lines) = title_and_highlights;
    let json_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(active, theme))
        .title(format!(" {title} "));
    frame.render_widget(json_block.clone(), area);
    let json_inner = json_block.inner(area);

    let highlight_style = Style::default().bg(theme.surface);
    let lines = json_text
        .lines()
        .enumerate()
        .map(|(idx, s)| {
            if highlighted_lines.contains(&idx) {
                match s.find(|ch: char| !ch.is_whitespace()) {
                    Some(start) => {
                        let (indent, content) = s.split_at(start);
                        Line::from(vec![
                            Span::raw(indent.to_string()),
                            Span::styled(content.to_string(), highlight_style),
                        ])
                    }
                    None => Line::raw(s.to_string()),
                }
            } else {
                Line::raw(s.to_string())
            }
        })
        .collect::<Vec<_>>();

    if json_inner.width == 0 || json_inner.height == 0 {
        return;
    }
    let scroll_y = u16::try_from(scroll).unwrap_or(u16::MAX);

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll_y, 0)),
        json_inner,
    );
}

pub(crate) fn render_form_text_preview(
    frame: &mut Frame<'_>,
    title: &str,
    text: &str,
    scroll: usize,
    active: bool,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(active, theme))
        .title(format!(" {} ", title));
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);

    let lines = text
        .lines()
        .map(|s| Line::raw(s.to_string()))
        .collect::<Vec<_>>();

    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let scroll_y = u16::try_from(scroll).unwrap_or(u16::MAX);

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll_y, 0)),
        inner,
    );
}

pub(crate) fn render_add_form(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    form: &FormState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    match form {
        FormState::ProviderAdd(provider) => {
            render_provider_add_form(frame, app, data, provider, area, theme)
        }
        FormState::McpAdd(mcp) => render_mcp_add_form(frame, app, mcp, area, theme),
        FormState::PromptMeta(prompt) => render_prompt_meta_form(frame, app, prompt, area, theme),
        FormState::S3Sync(s3) => render_s3_sync_form(frame, app, s3, area, theme),
        FormState::WebDavSync(webdav) => render_webdav_sync_form(frame, app, webdav, area, theme),
    }
}
