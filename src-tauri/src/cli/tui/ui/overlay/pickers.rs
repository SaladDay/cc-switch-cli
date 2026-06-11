use super::super::theme;
use super::super::*;
use crate::cli::tui::form::{HermesModelField, ProviderAddFormState};
use crate::cli::tui::text_edit::TextInput;

pub(super) fn render_claude_model_picker_overlay(
    frame: &mut Frame<'_>,
    app: &App,
    content_area: Rect,
    theme: &theme::Theme,
    selected: usize,
    editing: bool,
) {
    let area = centered_rect(OVERLAY_MD.0, OVERLAY_MD.1, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(texts::tui_claude_model_config_popup_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(inner);

    let key_items: Vec<(&str, &str)> = if editing {
        vec![
            ("←→/Home/End", texts::tui_key_move()),
            ("Esc/Enter", texts::tui_key_exit_edit()),
        ]
    } else {
        vec![
            ("↑↓", texts::tui_key_select()),
            ("Space", texts::tui_key_edit()),
            ("a", texts::tui_key_fill_all()),
            ("Enter", texts::tui_key_fetch_model()),
            ("Esc", texts::tui_key_close()),
        ]
    };
    render_key_bar_center(frame, chunks[0], theme, &key_items);

    let body_area = inset_top(chunks[1], 1);

    if let Some(FormState::ProviderAdd(provider)) = app.form.as_ref() {
        let labels = [
            texts::tui_claude_model_main_label(),
            texts::tui_claude_reasoning_model_label(),
            texts::tui_claude_default_haiku_model_label(),
            texts::tui_claude_default_sonnet_model_label(),
            texts::tui_claude_default_opus_model_label(),
        ];

        let label_col_width = field_label_column_width(
            labels
                .iter()
                .copied()
                .chain(std::iter::once(texts::tui_header_field())),
            1,
        );

        let header = Row::new(vec![
            Cell::from(cell_pad(texts::tui_header_field())),
            Cell::from(texts::tui_header_value()),
        ])
        .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

        let rows = labels.iter().enumerate().map(|(idx, label)| {
            let value = provider
                .claude_model_input(idx)
                .map(|input| input.value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| texts::tui_na().to_string());
            Row::new(vec![Cell::from(cell_pad(label)), Cell::from(value)])
        });

        let table = Table::new(
            rows,
            [Constraint::Length(label_col_width), Constraint::Min(10)],
        )
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(texts::tui_form_fields_title()),
        )
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

        let mut state = TableState::default();
        state.select(Some(selected.min(labels.len().saturating_sub(1))));
        frame.render_stateful_widget(table, body_area, &mut state);

        let hint_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(if editing {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.dim)
            })
            .title(if editing {
                texts::tui_form_editing_title()
            } else {
                texts::tui_form_input_title()
            });
        frame.render_widget(hint_block.clone(), chunks[2]);
        let hint_inner = hint_block.inner(chunks[2]);

        if editing {
            if let Some(input) = provider.claude_model_input(selected) {
                let (visible, cursor_x) =
                    visible_text_window(&input.value, input.cursor, hint_inner.width as usize);
                frame.render_widget(
                    Paragraph::new(Line::raw(visible)).wrap(Wrap { trim: false }),
                    hint_inner,
                );
                let x = hint_inner.x + cursor_x.min(hint_inner.width.saturating_sub(1));
                let y = hint_inner.y;
                frame.set_cursor_position((x, y));
            }
        } else {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(texts::tui_hint_press(), Style::default().fg(theme.dim)),
                    Span::styled(
                        "Enter",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        texts::tui_hint_auto_fetch_models_from_api(),
                        Style::default().fg(theme.dim),
                    ),
                ]))
                .alignment(Alignment::Center),
                hint_inner,
            );
        }
    } else {
        frame.render_widget(
            Paragraph::new(Line::raw(texts::tui_provider_not_found())),
            body_area,
        );
    }
}

pub(super) fn render_claude_api_format_picker_overlay(
    frame: &mut Frame<'_>,
    app: &App,
    content_area: Rect,
    theme: &theme::Theme,
    selected: usize,
) {
    let area = centered_rect_fixed(58, 10, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(texts::tui_claude_api_format_popup_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_key_bar_center(
        frame,
        chunks[0],
        theme,
        &[
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_close()),
        ],
    );

    let body_area = Rect {
        x: chunks[1].x.saturating_add(2),
        y: chunks[1].y.saturating_add(1),
        width: chunks[1].width.saturating_sub(4),
        height: chunks[1].height.saturating_sub(2),
    };
    let (app_type, current) = app
        .form
        .as_ref()
        .and_then(|form| match form {
            FormState::ProviderAdd(provider) => {
                Some((provider.app_type.clone(), provider.claude_api_format))
            }
            _ => None,
        })
        .unwrap_or_else(|| {
            (
                app.app_type.clone(),
                crate::cli::tui::form::ClaudeApiFormat::Anthropic,
            )
        });

    let choices = crate::cli::tui::form::ClaudeApiFormat::choices_for_app(&app_type);
    let items = choices.into_iter().copied().map(|api_format| {
        let marker = if api_format == current {
            texts::tui_marker_active()
        } else {
            texts::tui_marker_inactive()
        };
        let label = if matches!(app_type, crate::app_config::AppType::Codex) {
            texts::tui_codex_api_format_value(api_format.as_str())
        } else {
            texts::tui_claude_api_format_value(api_format.as_str())
        };
        ListItem::new(Line::from(Span::raw(format!("{marker}  {}", label))))
    });

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = ListState::default();
    state.select(Some(selected.min(choices.len().saturating_sub(1))));
    frame.render_stateful_widget(list, body_area, &mut state);
}

pub(super) fn render_usage_query_template_picker_overlay(
    frame: &mut Frame<'_>,
    app: &App,
    content_area: Rect,
    theme: &theme::Theme,
    selected: usize,
) {
    let area = centered_rect_fixed(42, 10, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(texts::tui_usage_query_template());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_key_bar_center(
        frame,
        chunks[0],
        theme,
        &[
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_close()),
        ],
    );

    let body_area = Rect {
        x: chunks[1].x.saturating_add(2),
        y: chunks[1].y.saturating_add(1),
        width: chunks[1].width.saturating_sub(4),
        height: chunks[1].height.saturating_sub(2),
    };

    if let Some(FormState::ProviderAdd(provider)) = app.form.as_ref() {
        let current = provider.usage_query_template;
        let options = provider.available_usage_query_templates();
        let items = options.iter().map(|template| {
            let marker = if *template == current {
                texts::tui_marker_active()
            } else {
                texts::tui_marker_inactive()
            };
            ListItem::new(Line::from(Span::raw(format!(
                "{marker}  {}",
                template.label()
            ))))
        });

        let list = List::new(items)
            .highlight_style(selection_style(theme))
            .highlight_symbol(highlight_symbol(theme));

        let mut state = ListState::default();
        state.select(Some(selected.min(options.len().saturating_sub(1))));
        frame.render_stateful_widget(list, body_area, &mut state);
    }
}

pub(super) fn render_managed_account_picker_overlay(
    frame: &mut Frame<'_>,
    app: &App,
    content_area: Rect,
    theme: &theme::Theme,
    selected: usize,
    binding: bool,
    selected_account_id: Option<&str>,
) {
    let height = app
        .managed_auth_status
        .as_ref()
        .map(|status| status.accounts.len())
        .unwrap_or(0)
        .saturating_add(if binding { 6 } else { 5 })
        .min(18) as u16;
    let area = centered_rect_fixed(62, height.max(8), content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(texts::tui_label_chatgpt_account());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_key_bar_center(
        frame,
        chunks[0],
        theme,
        &[
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_close()),
        ],
    );

    let body_area = inset_top(chunks[1], 1);
    let Some(status) = app.managed_auth_status.as_ref() else {
        frame.render_widget(
            Paragraph::new(Line::styled(
                texts::tui_managed_accounts_not_loaded(),
                Style::default().fg(theme.dim),
            ))
            .alignment(Alignment::Center),
            body_area,
        );
        return;
    };

    if status.accounts.is_empty() && !binding {
        frame.render_widget(
            Paragraph::new(Line::styled(
                texts::tui_managed_accounts_not_authenticated(),
                Style::default().fg(theme.dim),
            ))
            .alignment(Alignment::Center),
            body_area,
        );
        return;
    }

    let mut items = Vec::new();
    if binding {
        let marker = if selected_account_id.is_none() {
            texts::tui_marker_active()
        } else {
            texts::tui_marker_inactive()
        };
        items.push(ListItem::new(Line::raw(format!(
            "{marker}  {}",
            texts::tui_managed_accounts_follow_default()
        ))));
    }

    for account in &status.accounts {
        let marker = if selected_account_id == Some(account.id.as_str()) {
            texts::tui_marker_active()
        } else {
            texts::tui_marker_inactive()
        };
        let suffix = if account.is_default {
            format!(" ({})", texts::tui_managed_accounts_default())
        } else {
            String::new()
        };
        items.push(ListItem::new(Line::raw(format!(
            "{marker}  {}{suffix}",
            account.login
        ))));
    }

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));
    let mut state = ListState::default();
    let row_count = status.accounts.len() + usize::from(binding);
    state.select(Some(selected.min(row_count.saturating_sub(1))));
    frame.render_stateful_widget(list, body_area, &mut state);
}

pub(super) fn render_managed_account_action_picker_overlay(
    frame: &mut Frame<'_>,
    app: &App,
    content_area: Rect,
    theme: &theme::Theme,
    account_id: &str,
    selected: usize,
) {
    let area = centered_rect_fixed(48, 8, content_area);
    frame.render_widget(Clear, area);

    let account_label = app
        .managed_auth_status
        .as_ref()
        .and_then(|status| {
            status
                .accounts
                .iter()
                .find(|account| account.id == account_id)
                .map(|account| account.login.as_str())
        })
        .unwrap_or(account_id);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(account_label);
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_key_bar_center(
        frame,
        chunks[0],
        theme,
        &[
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_close()),
        ],
    );

    let items = [
        texts::tui_key_set_default().to_string(),
        texts::tui_key_delete().to_string(),
    ]
    .into_iter()
    .map(|label| ListItem::new(Line::raw(label)));
    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));
    let mut state = ListState::default();
    state.select(Some(selected.min(1)));
    frame.render_stateful_widget(list, inset_top(chunks[1], 1), &mut state);
}

pub(super) fn render_provider_test_menu_overlay(
    frame: &mut Frame<'_>,
    app: &App,
    _data: &UiData,
    content_area: Rect,
    theme: &theme::Theme,
    _provider_id: &str,
    selected: usize,
) {
    let area = centered_rect_fixed(50, 8, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(texts::tui_provider_test_menu_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_key_bar_center(
        frame,
        chunks[0],
        theme,
        &[
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_close()),
        ],
    );

    let body_area = inset_top(chunks[1], 1);
    let items = app::provider_test_menu_items(&app.app_type)
        .into_iter()
        .map(|item| ListItem::new(Line::raw(app::provider_test_menu_item_label(item))));

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = ListState::default();
    state.select(Some(
        selected.min(
            app::provider_test_menu_items(&app.app_type)
                .len()
                .saturating_sub(1),
        ),
    ));
    frame.render_stateful_widget(list, body_area, &mut state);
}

pub(super) fn render_hermes_models_picker_overlay(
    frame: &mut Frame<'_>,
    app: &App,
    content_area: Rect,
    theme: &theme::Theme,
    editing: bool,
) {
    let area = centered_rect_fixed(86, 24, content_area);
    frame.render_widget(Clear, area);

    let Some(FormState::ProviderAdd(provider)) = app.form.as_ref() else {
        return;
    };

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(texts::tui_hermes_models_title(provider.name.value.trim()));
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(2),
            Constraint::Length(3),
        ])
        .split(inner);

    let key_items: Vec<(&str, &str)> = if editing {
        vec![
            ("←→/Home/End", texts::tui_key_move()),
            ("Enter/Esc", texts::tui_key_exit_edit()),
        ]
    } else {
        vec![
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_edit()),
            ("f", texts::tui_key_fetch_model()),
            ("a", texts::tui_key_add()),
            ("d", texts::tui_key_delete()),
            ("Esc", texts::tui_key_close()),
        ]
    };
    render_key_bar_center(frame, chunks[0], theme, &key_items);

    let fields = provider.hermes_model_fields();
    if fields.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                texts::tui_hermes_models_no_models(),
                Style::default().fg(theme.dim),
            ))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
            inset_top(chunks[1], 1),
        );
    } else {
        let rows_data = fields
            .iter()
            .map(|field| hermes_model_field_label_and_value(provider, *field))
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
        let mut rows = Vec::with_capacity(rows_data.len() + provider.hermes_models.len());
        for (idx, (label, value)) in rows_data.iter().enumerate() {
            if idx > 0 && idx % 3 == 0 {
                rows.push(
                    Row::new(vec![
                        Cell::from(cell_pad(&"┄".repeat(40))),
                        Cell::from("┄".repeat(200)),
                    ])
                    .style(Style::default().fg(theme.dim)),
                );
            }
            rows.push(Row::new(vec![
                Cell::from(cell_pad(label)),
                Cell::from(value.clone()),
            ]));
        }
        let table = Table::new(
            rows,
            [Constraint::Length(label_col_width), Constraint::Min(10)],
        )
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .title(texts::tui_label_hermes_models()),
        )
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

        let mut state = TableState::default();
        state.select(Some(hermes_model_visual_row_index(
            provider.hermes_models_field_idx.min(fields.len() - 1),
        )));
        frame.render_stateful_widget(table, chunks[1], &mut state);
    }

    let footer = if provider.hermes_models.is_empty() {
        texts::tui_hermes_models_fetch_hint()
    } else {
        texts::tui_hermes_models_hint()
    };
    frame.render_widget(
        Paragraph::new(Line::styled(footer, Style::default().fg(theme.dim)))
            .wrap(Wrap { trim: true }),
        chunks[2],
    );

    render_hermes_model_picker_input(frame, provider, chunks[3], theme, editing);
}

fn hermes_model_visual_row_index(field_idx: usize) -> usize {
    field_idx + field_idx / 3
}

fn hermes_model_field_label_and_value(
    provider: &ProviderAddFormState,
    field: HermesModelField,
) -> (String, String) {
    match field {
        HermesModelField::Id(index) => (
            texts::tui_hermes_model_id_label(index + 1),
            hermes_model_string(provider, index, "id"),
        ),
        HermesModelField::Name(index) => (
            texts::tui_hermes_model_name_label(index + 1),
            hermes_model_string(provider, index, "name"),
        ),
        HermesModelField::ContextLength(index) => (
            texts::tui_hermes_model_context_length_label(index + 1),
            hermes_model_string(provider, index, "context_length"),
        ),
    }
}

fn hermes_model_string(provider: &ProviderAddFormState, index: usize, key: &str) -> String {
    provider
        .hermes_models
        .get(index)
        .and_then(|model| model.get(key))
        .and_then(|value| {
            value
                .as_str()
                .map(str::to_string)
                .or_else(|| value.as_i64().map(|number| number.to_string()))
                .or_else(|| value.as_u64().map(|number| number.to_string()))
        })
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| texts::tui_na().to_string())
}

fn render_hermes_model_picker_input(
    frame: &mut Frame<'_>,
    provider: &ProviderAddFormState,
    area: Rect,
    theme: &theme::Theme,
    editing: bool,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(if editing {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.dim)
        })
        .title(if editing {
            texts::tui_form_editing_title()
        } else {
            texts::tui_form_input_title()
        });
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);

    if provider.selected_hermes_model_field().is_none() {
        frame.render_widget(
            Paragraph::new(Line::raw(texts::tui_hermes_models_add_hint()))
                .wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }

    let input = &provider.hermes_model_input;
    let (visible, cursor_x) = visible_text_window(&input.value, input.cursor, inner.width as usize);
    frame.render_widget(
        Paragraph::new(Line::raw(visible)).wrap(Wrap { trim: false }),
        inner,
    );

    if editing {
        let x = inner.x + cursor_x.min(inner.width.saturating_sub(1));
        frame.set_cursor_position((x, inner.y));
    }
}

pub(super) fn render_model_fetch_picker_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    input: &TextInput,
    query: &str,
    fetching: bool,
    models: &[String],
    error: Option<&str>,
    selected_idx: usize,
) {
    let area = centered_rect_fixed(OVERLAY_FIXED_LG.0, OVERLAY_FIXED_LG.1, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(texts::tui_model_fetch_popup_title(fetching));
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(inner);

    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .title(texts::tui_model_fetch_search_title());

    frame.render_widget(input_block.clone(), chunks[0]);
    let input_inner = input_block.inner(chunks[0]);

    let (visible, cursor_x) =
        visible_text_window(&input.value, input.cursor, input_inner.width as usize);
    let (input_text, input_style) = if input.value.is_empty() {
        (
            texts::tui_model_fetch_search_placeholder().to_string(),
            Style::default().fg(theme.dim),
        )
    } else {
        (visible, Style::default())
    };

    frame.render_widget(
        Paragraph::new(Line::styled(input_text, input_style)).wrap(Wrap { trim: false }),
        input_inner,
    );

    let x = input_inner.x + cursor_x.min(input_inner.width.saturating_sub(1));
    let y = input_inner.y;
    frame.set_cursor_position((x, y));

    let list_area = chunks[1];
    if fetching {
        let text = texts::tui_loading().to_string();
        let p = Paragraph::new(Line::styled(text, Style::default().fg(theme.accent)))
            .alignment(Alignment::Center);
        frame.render_widget(p, list_area);
        return;
    }

    if let Some(err) = error {
        let p = Paragraph::new(Line::styled(err, Style::default().fg(theme.err)))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
        frame.render_widget(p, list_area);
        return;
    }

    let filtered: Vec<&String> = if query.trim().is_empty() {
        models.iter().collect()
    } else {
        let q = query.trim().to_lowercase();
        models
            .iter()
            .filter(|m| m.to_lowercase().contains(&q))
            .collect()
    };

    if filtered.is_empty() {
        let hint = if models.is_empty() {
            texts::tui_model_fetch_no_models().to_string()
        } else {
            texts::tui_model_fetch_no_matches().to_string()
        };
        let p = Paragraph::new(Line::styled(hint, Style::default().fg(theme.dim)))
            .alignment(Alignment::Center);
        frame.render_widget(p, list_area);
        return;
    }

    let items: Vec<ListItem> = filtered
        .iter()
        .map(|m| ListItem::new(Line::raw(*m)))
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::NONE))
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(selected_idx));

    frame.render_stateful_widget(list, list_area, &mut state);
}

pub(super) fn render_openclaw_agents_fallback_picker_overlay(
    frame: &mut Frame<'_>,
    app: &App,
    content_area: Rect,
    theme: &theme::Theme,
    selected: usize,
    options: &[app::OpenClawModelOption],
) {
    let area = centered_rect_fixed(OVERLAY_FIXED_LG.0, 12, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(openclaw_agents_picker_title(app));
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    let has_selection = selected != app::OPENCLAW_AGENTS_MODEL_PICKER_NONE;
    let key_items = if has_selection {
        vec![
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_cancel()),
        ]
    } else {
        vec![
            ("↑↓", texts::tui_key_select()),
            ("Esc", texts::tui_key_cancel()),
        ]
    };
    render_key_bar_center(frame, chunks[0], theme, &key_items);

    let body_area = inset_top(chunks[1], 1);
    let current_value = openclaw_agents_picker_current_value(app);
    let items = options.iter().map(|option| {
        let marker = if current_value == Some(option.value.as_str()) {
            texts::tui_marker_active()
        } else {
            texts::tui_marker_inactive()
        };
        ListItem::new(Line::raw(format!("{marker}  {}", option.label)))
    });

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = ListState::default();
    state.select(
        (selected != app::OPENCLAW_AGENTS_MODEL_PICKER_NONE)
            .then_some(selected.min(options.len().saturating_sub(1))),
    );
    frame.render_stateful_widget(list, body_area, &mut state);
}

fn openclaw_agents_picker_title(app: &App) -> &'static str {
    let Some(form) = app.openclaw_agents_form.as_ref() else {
        return texts::tui_openclaw_agents_add_fallback();
    };

    match form.section {
        app::OpenClawAgentsSection::PrimaryModel => texts::tui_openclaw_agents_primary_model(),
        app::OpenClawAgentsSection::FallbackModels if form.row < form.fallbacks.len() => {
            texts::tui_openclaw_agents_fallback_models()
        }
        app::OpenClawAgentsSection::FallbackModels | app::OpenClawAgentsSection::Runtime => {
            texts::tui_openclaw_agents_add_fallback()
        }
    }
}

fn openclaw_agents_picker_current_value(app: &App) -> Option<&str> {
    let form = app.openclaw_agents_form.as_ref()?;

    match form.section {
        app::OpenClawAgentsSection::PrimaryModel => Some(form.primary_model.as_str()),
        app::OpenClawAgentsSection::FallbackModels => {
            form.fallbacks.get(form.row).map(String::as_str)
        }
        app::OpenClawAgentsSection::Runtime => None,
    }
}

pub(super) fn render_openclaw_tools_profile_picker_overlay(
    frame: &mut Frame<'_>,
    app: &App,
    content_area: Rect,
    theme: &theme::Theme,
    selected: Option<usize>,
) {
    let area = centered_rect_fixed(58, 12, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(texts::tui_openclaw_tools_profile_block_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    let key_items = if selected.is_some() {
        vec![
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_cancel()),
        ]
    } else {
        vec![
            ("↑↓", texts::tui_key_select()),
            ("Esc", texts::tui_key_cancel()),
        ]
    };
    render_key_bar_center(frame, chunks[0], theme, &key_items);

    let body_area = inset_top(chunks[1], 1);
    let current = app
        .openclaw_tools_form
        .as_ref()
        .and_then(|form| app::openclaw_tools_profile_picker_index(form.profile.as_deref()));

    let items = (0..app::OPENCLAW_TOOLS_PROFILE_PICKER_LEN).map(|index| {
        let marker = if current == Some(index) {
            texts::tui_marker_active()
        } else {
            texts::tui_marker_inactive()
        };
        ListItem::new(Line::from(Span::raw(format!(
            "{marker}  {}",
            app::openclaw_tools_profile_picker_label(index)
        ))))
    });

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = ListState::default();
    state
        .select(selected.map(|selected| {
            selected.min(app::OPENCLAW_TOOLS_PROFILE_PICKER_LEN.saturating_sub(1))
        }));
    frame.render_stateful_widget(list, body_area, &mut state);
}

pub(super) fn render_failover_queue_manager_overlay(
    frame: &mut Frame<'_>,
    data: &UiData,
    content_area: Rect,
    theme: &theme::Theme,
    selected: usize,
) {
    let area = centered_rect_fixed(OVERLAY_FIXED_LG.0, 16, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(crate::t!("Failover Queue", "故障转移队列"));
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .split(inner);

    render_key_bar_center(
        frame,
        chunks[0],
        theme,
        &[
            ("↑↓", texts::tui_key_select()),
            ("f", crate::t!("enable/disable", "启用/禁用")),
            ("Space/Enter", texts::tui_key_toggle()),
            ("</>/u/d", texts::tui_key_move()),
            ("Esc", texts::tui_key_close()),
        ],
    );

    let status = if data.proxy.auto_failover_enabled {
        crate::t!("Automatic failover: enabled", "自动故障转移：已开启")
    } else {
        crate::t!("Automatic failover: disabled", "自动故障转移：已关闭")
    };
    frame.render_widget(
        Paragraph::new(status)
            .style(Style::default().fg(theme.dim))
            .alignment(Alignment::Center),
        chunks[1],
    );

    let body_area = inset_top(chunks[2], 1);
    let rows = app::failover_queue_rows(data);
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(crate::t!("No providers configured.", "暂无提供商配置。"))
                .style(Style::default().fg(theme.dim))
                .alignment(Alignment::Center),
            body_area,
        );
    } else {
        let header = Row::new(vec![
            Cell::from(""),
            Cell::from(crate::t!("Queue", "队列")),
            Cell::from(texts::header_name()),
            Cell::from(texts::tui_header_api_url()),
        ])
        .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

        let table_rows = rows.iter().map(|row| {
            let marker = if row.provider.in_failover_queue {
                texts::tui_marker_active()
            } else {
                texts::tui_marker_inactive()
            };
            let queue = app::failover_queue_position(data, &row.id)
                .map(|position| format!("#{position}"))
                .unwrap_or_else(|| "-".to_string());
            let api_url = row.api_url.as_deref().unwrap_or_else(|| texts::tui_na());

            Row::new(vec![
                Cell::from(marker),
                Cell::from(queue),
                Cell::from(row.provider.name.as_str()),
                Cell::from(api_url.to_string()),
            ])
        });

        let table = Table::new(
            table_rows,
            [
                Constraint::Length(2),
                Constraint::Length(8),
                Constraint::Percentage(35),
                Constraint::Percentage(65),
            ],
        )
        .header(header)
        .block(Block::default().borders(Borders::NONE))
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

        let mut state = TableState::default();
        state.select(Some(selected.min(rows.len().saturating_sub(1))));
        frame.render_stateful_widget(table, body_area, &mut state);
    }

    frame.render_widget(
        Paragraph::new(if data.proxy.auto_failover_enabled {
            crate::t!(
                "Auto failover uses only checked providers, in queue order.",
                "自动故障转移仅按队列顺序使用已勾选的提供商。"
            )
        } else {
            crate::t!(
                "Direct provider selection is used. Enable failover to route by queue priority.",
                "当前使用直接供应商选择。开启故障转移后将按队列优先级路由。"
            )
        })
        .style(Style::default().fg(theme.dim))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false }),
        chunks[3],
    );
}

pub(super) fn render_mcp_apps_picker_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    name: &str,
    selected: usize,
    apps: &crate::app_config::McpApps,
) {
    render_apps_picker_overlay(
        frame,
        content_area,
        theme,
        texts::tui_mcp_apps_title(name),
        selected,
        apps,
        "Space",
        &[
            crate::app_config::AppType::Claude,
            crate::app_config::AppType::Codex,
            crate::app_config::AppType::Gemini,
            crate::app_config::AppType::OpenCode,
            crate::app_config::AppType::Hermes,
        ],
    );
}

pub(super) fn render_mcp_type_picker_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    selected: usize,
) {
    let area = centered_rect_fixed(58, 8, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(texts::tui_mcp_type_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_key_bar_center(
        frame,
        chunks[0],
        theme,
        &[
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_cancel()),
        ],
    );

    let body_area = inset_top(chunks[1], 1);
    let items = [
        crate::cli::tui::form::McpTransport::Stdio,
        crate::cli::tui::form::McpTransport::Http,
        crate::cli::tui::form::McpTransport::Sse,
    ]
    .iter()
    .map(|transport| ListItem::new(Line::raw(transport.label())));

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = ListState::default();
    state.select(Some(selected.min(2)));
    frame.render_stateful_widget(list, body_area, &mut state);
}

pub(super) fn render_visible_apps_picker_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    selected: usize,
    apps: &crate::settings::VisibleApps,
) {
    render_apps_picker_overlay(
        frame,
        content_area,
        theme,
        texts::tui_settings_visible_apps_title().to_string(),
        selected,
        apps,
        "Space",
        &[
            crate::app_config::AppType::Claude,
            crate::app_config::AppType::Codex,
            crate::app_config::AppType::Gemini,
            crate::app_config::AppType::OpenCode,
            crate::app_config::AppType::Hermes,
            crate::app_config::AppType::OpenClaw,
        ],
    );
}

pub(super) fn render_skills_apps_picker_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    name: &str,
    selected: usize,
    apps: &crate::app_config::SkillApps,
) {
    render_apps_picker_overlay(
        frame,
        content_area,
        theme,
        texts::tui_skill_apps_title(name),
        selected,
        apps,
        "Space",
        &[
            crate::app_config::AppType::Claude,
            crate::app_config::AppType::Codex,
            crate::app_config::AppType::Gemini,
            crate::app_config::AppType::OpenCode,
            crate::app_config::AppType::Hermes,
        ],
    );
}

pub(super) fn render_skills_import_picker_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    skills: &[crate::services::skill::UnmanagedSkill],
    selected_idx: usize,
    selected: &std::collections::HashSet<String>,
) {
    let area = centered_rect_fixed(OVERLAY_FIXED_LG.0, OVERLAY_FIXED_LG.1, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, true))
        .title(texts::tui_skills_import_title())
        .style(if theme.no_color {
            Style::default()
        } else {
            Style::default().bg(theme.surface)
        });
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(0),
        ])
        .split(inner);

    render_key_bar_center(
        frame,
        chunks[0],
        theme,
        &[
            ("Space", texts::tui_key_select()),
            ("Enter", texts::tui_key_import()),
            ("r", texts::tui_key_refresh()),
            ("Esc", texts::tui_key_close()),
        ],
    );

    frame.render_widget(
        Paragraph::new(texts::tui_skills_import_description())
            .style(Style::default().fg(theme.dim))
            .wrap(Wrap { trim: false }),
        chunks[1],
    );

    let body_area = inset_top(chunks[2], 1);
    if skills.is_empty() {
        frame.render_widget(
            Paragraph::new(texts::tui_skills_unmanaged_empty())
                .style(Style::default().fg(theme.dim))
                .wrap(Wrap { trim: false }),
            body_area,
        );
        return;
    }

    let header = Row::new(vec![
        Cell::from(""),
        Cell::from(texts::header_name()),
        Cell::from(texts::tui_header_found_in()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let rows = skills.iter().map(|skill| {
        Row::new(vec![
            Cell::from(if selected.contains(&skill.directory) {
                texts::tui_marker_active()
            } else {
                texts::tui_marker_inactive()
            }),
            Cell::from(skill_display_name(&skill.name, &skill.directory).to_string()),
            Cell::from(skill.found_in.join(", ")),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Percentage(70),
            Constraint::Percentage(30),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(selected_idx));
    frame.render_stateful_widget(table, body_area, &mut state);
}

pub(super) fn render_skills_sync_method_picker_overlay(
    frame: &mut Frame<'_>,
    data: &UiData,
    content_area: Rect,
    theme: &theme::Theme,
    selected: usize,
) {
    let area = centered_rect_fixed(OVERLAY_FIXED_LG.0, 12, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(texts::tui_skills_sync_method_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_key_bar_center(
        frame,
        chunks[0],
        theme,
        &[
            ("←→", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_cancel()),
        ],
    );

    let body_area = inset_top(chunks[1], 1);
    let current = data.skills.sync_method;
    let methods = [
        crate::services::skill::SyncMethod::Auto,
        crate::services::skill::SyncMethod::Symlink,
        crate::services::skill::SyncMethod::Copy,
    ];

    let items = methods.into_iter().map(|method| {
        let marker = if method == current {
            texts::tui_marker_active()
        } else {
            texts::tui_marker_inactive()
        };
        ListItem::new(Line::from(Span::raw(format!(
            "{marker}  {}",
            texts::tui_skills_sync_method_name(method)
        ))))
    });

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, body_area, &mut state);
}

fn render_apps_picker_overlay<A>(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    title: String,
    selected: usize,
    apps: &A,
    toggle_key_label: &'static str,
    app_types: &[crate::app_config::AppType],
) where
    A: AppToggleState,
{
    let area = centered_rect_fixed(OVERLAY_FIXED_LG.0, 12, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(title);
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_key_bar_center(
        frame,
        chunks[0],
        theme,
        &[
            (toggle_key_label, texts::tui_key_toggle()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_cancel()),
        ],
    );

    let body_area = inset_top(chunks[1], 1);
    let items = app_types.iter().map(|app_type| {
        let marker = if apps.is_enabled_for(app_type) {
            texts::tui_marker_active()
        } else {
            texts::tui_marker_inactive()
        };
        ListItem::new(Line::from(Span::raw(format!(
            "{marker}  {}",
            app_type.as_str()
        ))))
    });

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = ListState::default();
    state.select(Some(selected.min(app_types.len().saturating_sub(1))));
    frame.render_stateful_widget(list, body_area, &mut state);
}

trait AppToggleState {
    fn is_enabled_for(&self, app_type: &crate::app_config::AppType) -> bool;
}

impl AppToggleState for crate::app_config::McpApps {
    fn is_enabled_for(&self, app_type: &crate::app_config::AppType) -> bool {
        self.is_enabled_for(app_type)
    }
}

impl AppToggleState for crate::app_config::SkillApps {
    fn is_enabled_for(&self, app_type: &crate::app_config::AppType) -> bool {
        self.is_enabled_for(app_type)
    }
}

impl AppToggleState for crate::settings::VisibleApps {
    fn is_enabled_for(&self, app_type: &crate::app_config::AppType) -> bool {
        self.is_enabled_for(app_type)
    }
}
