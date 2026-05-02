use super::super::*;

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
            if editing {
                keys.extend([
                    ("←→", texts::tui_key_move()),
                    ("Enter", texts::tui_key_exit_edit()),
                ]);
            } else {
                let enter_action = match selected_field {
                    Some(ProviderAddField::CodexModel | ProviderAddField::GeminiModel) => {
                        texts::tui_key_fetch_model()
                    }
                    Some(
                        ProviderAddField::ClaudeModelConfig
                        | ProviderAddField::CommonSnippet
                        | ProviderAddField::OpenClawModels,
                    ) => texts::tui_key_open(),
                    Some(
                        ProviderAddField::GeminiAuthType
                        | ProviderAddField::ClaudeHideAttribution
                        | ProviderAddField::OpenClawApiProtocol
                        | ProviderAddField::OpenClawUserAgent,
                    ) => texts::tui_key_toggle(),
                    _ => texts::tui_key_edit_mode(),
                };
                keys.extend([
                    ("↑↓", texts::tui_key_select()),
                    ("Enter", enter_action),
                    ("Space", texts::tui_key_toggle()),
                ]);
            }
        }
        FormFocus::JsonPreview => {
            keys.extend([
                ("Enter", texts::tui_key_edit_mode()),
                ("↑↓", texts::tui_key_scroll()),
            ]);
        }
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
        .title(texts::tui_form_templates_title());
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

    let chars = text.chars().collect::<Vec<_>>();
    let cursor = cursor.min(chars.len());

    let mut cum: Vec<usize> = Vec::with_capacity(chars.len() + 1);
    cum.push(0);
    for c in &chars {
        let w = UnicodeWidthChar::width(*c).unwrap_or(0);
        cum.push(cum.last().unwrap_or(&0).saturating_add(w));
    }

    let cursor_x = cum.get(cursor).copied().unwrap_or(0);
    let target = cursor_x.saturating_sub(width.saturating_sub(1));
    let mut start_idx = 0usize;
    while start_idx < cum.len() && cum[start_idx] < target {
        start_idx += 1;
    }

    let mut end_idx = start_idx;
    while end_idx < chars.len() && cum[end_idx + 1].saturating_sub(cum[start_idx]) <= width {
        end_idx += 1;
    }

    let visible = chars
        .get(start_idx..end_idx)
        .unwrap_or_default()
        .iter()
        .collect::<String>();
    let cursor_in_window = cursor_x.saturating_sub(*cum.get(start_idx).unwrap_or(&0));

    (visible, cursor_in_window.min(width) as u16)
}

pub(crate) fn render_form_json_preview(
    frame: &mut Frame<'_>,
    json_text: &str,
    scroll: usize,
    active: bool,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let json_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(active, theme))
        .title(texts::tui_form_json_title());
    frame.render_widget(json_block.clone(), area);
    let json_inner = json_block.inner(area);

    let lines = json_text
        .lines()
        .map(|s| Line::raw(s.to_string()))
        .collect::<Vec<_>>();

    let height = json_inner.height as usize;
    if height == 0 {
        return;
    }
    let max_start = lines.len().saturating_sub(height);
    let start = scroll.min(max_start);
    let end = (start + height).min(lines.len());

    frame.render_widget(
        Paragraph::new(lines[start..end].to_vec()).wrap(Wrap { trim: false }),
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
        .title(title);
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);

    let lines = text
        .lines()
        .map(|s| Line::raw(s.to_string()))
        .collect::<Vec<_>>();

    let height = inner.height as usize;
    if height == 0 {
        return;
    }
    let max_start = lines.len().saturating_sub(height);
    let start = scroll.min(max_start);
    let end = (start + height).min(lines.len());

    frame.render_widget(
        Paragraph::new(lines[start..end].to_vec()).wrap(Wrap { trim: false }),
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
    }
}
