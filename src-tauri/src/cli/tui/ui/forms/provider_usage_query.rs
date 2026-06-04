use super::*;

pub(super) fn render_usage_query_form(
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
        render_usage_query_info_panel(frame, area, theme);
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

fn render_usage_query_info_panel(frame: &mut Frame<'_>, area: Rect, theme: &super::theme::Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.dim))
        .title(texts::tui_usage_query_info());
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);

    frame.render_widget(
        Paragraph::new(Line::default()).wrap(Wrap { trim: false }),
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
