use super::super::theme;
use super::super::*;
use super::frame::{overlay_frame, overlay_frame_at, OverlaySize};
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
    // Keep the percentage-based width, but cap the height to the four role rows
    // rather than filling 62% of the screen (which left a large empty table).
    // Height = outer borders(2) + key bar(1) + top gap(1) + table[border(2)+header(1)+4 rows] + hint(3).
    let wide = centered_rect(OVERLAY_MD.0, OVERLAY_MD.1, content_area);
    let desired_h = 14u16.min(content_area.height);
    let area = Rect {
        x: wide.x,
        width: wide.width,
        y: content_area.y + content_area.height.saturating_sub(desired_h) / 2,
        height: desired_h,
    };

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

    let body = overlay_frame_at(
        frame,
        area,
        theme,
        texts::tui_claude_model_config_popup_title(),
        &key_items,
        overlay_border_style(theme, false),
    );

    // Split the body into the model table and a fixed hint/input box at the bottom.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3)])
        .split(body);
    let body_area = chunks[0];
    let hint_area = chunks[1];

    if let Some(FormState::ProviderAdd(provider)) = app.form.as_ref() {
        let labels = [
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
                .title(format!(" {} ", texts::tui_form_fields_title())),
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
        frame.render_widget(hint_block.clone(), hint_area);
        let hint_inner = hint_block.inner(hint_area);

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

    // Size the height to the option rows so there is no large gap below them:
    // borders(2) + key bar(1) + gap(1) + one row per choice + bottom margin.
    let height = (choices.len() as u16)
        .saturating_add(5)
        .min(content_area.height);
    let body_area = overlay_frame_at(
        frame,
        centered_rect_fixed(58, height, content_area),
        theme,
        texts::tui_claude_api_format_popup_title(),
        &[
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_close()),
        ],
        overlay_border_style(theme, false),
    );

    let items = choices.iter().copied().map(|api_format| {
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
    let options = match app.form.as_ref() {
        Some(FormState::ProviderAdd(provider)) => provider.available_usage_query_templates(),
        _ => Vec::new(),
    };

    let body_area = overlay_frame(
        frame,
        content_area,
        theme,
        texts::tui_usage_query_template(),
        &[
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_close()),
        ],
        OverlaySize::FitRows {
            width: 42,
            body_rows: options.len() as u16,
        },
        overlay_border_style(theme, false),
    );

    let Some(FormState::ProviderAdd(provider)) = app.form.as_ref() else {
        return;
    };
    let current = provider.usage_query_template;
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

pub(super) fn render_user_agent_picker_overlay(
    frame: &mut Frame<'_>,
    app: &App,
    content_area: Rect,
    theme: &theme::Theme,
    selected: usize,
) {
    let body_area = overlay_frame(
        frame,
        content_area,
        theme,
        texts::tui_user_agent_picker_title(),
        &[
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_close()),
        ],
        OverlaySize::FitRows {
            width: 54,
            body_rows: (crate::cli::tui::form::user_agent_picker_option_count() + 1) as u16,
        },
        overlay_border_style(theme, false),
    );

    let current_selection = app
        .form
        .as_ref()
        .and_then(|form| match form {
            FormState::ProviderAdd(provider) => {
                Some(crate::cli::tui::form::user_agent_picker_selection(
                    &provider.custom_user_agent.value,
                ))
            }
            _ => None,
        })
        .unwrap_or(crate::cli::tui::form::USER_AGENT_PICKER_NO_OVERRIDE_INDEX);
    let option_item = |index: usize, label: &str| {
        let marker = if current_selection == index {
            texts::tui_marker_active()
        } else {
            texts::tui_marker_inactive()
        };
        ListItem::new(Line::raw(format!("{marker}  {label}")))
    };
    let mut items = vec![option_item(
        crate::cli::tui::form::USER_AGENT_PICKER_CUSTOM_INDEX,
        texts::tui_user_agent_custom_option(),
    )];
    items.push(ListItem::new(Line::styled(
        texts::tui_user_agent_presets_heading(),
        Style::default().fg(theme.dim).add_modifier(Modifier::BOLD),
    )));
    items.extend(
        crate::cli::tui::form::USER_AGENT_PRESETS
            .iter()
            .enumerate()
            .map(|(preset_index, preset)| {
                option_item(
                    crate::cli::tui::form::USER_AGENT_PICKER_PRESET_OFFSET + preset_index,
                    preset,
                )
            }),
    );
    items.push(option_item(
        crate::cli::tui::form::USER_AGENT_PICKER_NO_OVERRIDE_INDEX,
        texts::tui_user_agent_no_override_option(),
    ));
    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = ListState::default();
    let selected =
        selected.min(crate::cli::tui::form::user_agent_picker_option_count().saturating_sub(1));
    state.select(Some(if selected == 0 { 0 } else { selected + 1 }));
    frame.render_stateful_widget(list, body_area, &mut state);
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

    let body_area = overlay_frame_at(
        frame,
        centered_rect_fixed(62, height.max(8), content_area),
        theme,
        texts::tui_label_chatgpt_account(),
        &[
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_close()),
        ],
        overlay_border_style(theme, false),
    );

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

    let actions = [
        texts::tui_key_set_default().to_string(),
        texts::tui_key_delete().to_string(),
    ];

    let body = overlay_frame(
        frame,
        content_area,
        theme,
        account_label,
        &[
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_close()),
        ],
        OverlaySize::FitRows {
            width: 48,
            body_rows: actions.len() as u16,
        },
        overlay_border_style(theme, false),
    );

    let items = actions
        .into_iter()
        .map(|label| ListItem::new(Line::raw(label)));
    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));
    let mut state = ListState::default();
    state.select(Some(selected.min(1)));
    frame.render_stateful_widget(list, body, &mut state);
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
    let menu_items = app::provider_test_menu_items(&app.app_type);

    let body_area = overlay_frame(
        frame,
        content_area,
        theme,
        texts::tui_provider_test_menu_title(),
        &[
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_close()),
        ],
        OverlaySize::FitRows {
            width: 50,
            body_rows: menu_items.len() as u16,
        },
        overlay_border_style(theme, false),
    );

    let items = menu_items
        .iter()
        .copied()
        .map(|item| ListItem::new(Line::raw(app::provider_test_menu_item_label(item))));

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = ListState::default();
    state.select(Some(selected.min(menu_items.len().saturating_sub(1))));
    frame.render_stateful_widget(list, body_area, &mut state);
}

pub(super) fn render_hermes_models_picker_overlay(
    frame: &mut Frame<'_>,
    app: &App,
    content_area: Rect,
    theme: &theme::Theme,
    editing: bool,
) {
    let Some(FormState::ProviderAdd(provider)) = app.form.as_ref() else {
        return;
    };

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

    // +1 row over the historic height (24 -> 25) compensates for the frame's
    // gap row so the model table keeps its full 16-row capacity.
    let title = texts::tui_hermes_models_title(provider.name.value.trim());
    let body = overlay_frame_at(
        frame,
        centered_rect_fixed(86, 25, content_area),
        theme,
        &title,
        &key_items,
        overlay_border_style(theme, false),
    );

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(2),
            Constraint::Length(3),
        ])
        .split(body);

    let fields = provider.hermes_model_fields();
    if fields.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                texts::tui_hermes_models_no_models(),
                Style::default().fg(theme.dim),
            ))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
            chunks[0],
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
                .title(format!(" {} ", texts::tui_label_hermes_models())),
        )
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

        let mut state = TableState::default();
        state.select(Some(hermes_model_visual_row_index(
            provider.hermes_models_field_idx.min(fields.len() - 1),
        )));
        frame.render_stateful_widget(table, chunks[0], &mut state);
    }

    let footer = if provider.hermes_models.is_empty() {
        texts::tui_hermes_models_fetch_hint()
    } else {
        texts::tui_hermes_models_hint()
    };
    frame.render_widget(
        Paragraph::new(Line::styled(footer, Style::default().fg(theme.dim)))
            .wrap(Wrap { trim: true }),
        chunks[1],
    );

    render_hermes_model_picker_input(frame, provider, chunks[2], theme, editing);
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

#[expect(
    clippy::too_many_arguments,
    reason = "model picker renderer receives transient search state"
)]
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
    let title = texts::tui_model_fetch_popup_title(fetching);
    let body = overlay_frame_at(
        frame,
        centered_rect_fixed(OVERLAY_FIXED_LG.0, OVERLAY_FIXED_LG.1, content_area),
        theme,
        &title,
        &[
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_close()),
        ],
        overlay_border_style(theme, false),
    );

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(body);

    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .title(format!(" {} ", texts::tui_model_fetch_search_title()));

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

    // The fallback list can be long, so keep a fixed height and let it scroll.
    let body_area = overlay_frame(
        frame,
        content_area,
        theme,
        openclaw_agents_picker_title(app),
        &key_items,
        OverlaySize::Fixed(OVERLAY_FIXED_LG.0, 12),
        overlay_border_style(theme, false),
    );

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

    let body_area = overlay_frame(
        frame,
        content_area,
        theme,
        texts::tui_openclaw_tools_profile_block_title(),
        &key_items,
        OverlaySize::FitRows {
            width: 58,
            body_rows: app::OPENCLAW_TOOLS_PROFILE_PICKER_LEN as u16,
        },
        overlay_border_style(theme, false),
    );

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
    let body = overlay_frame_at(
        frame,
        centered_rect_fixed(OVERLAY_FIXED_LG.0, 16, content_area),
        theme,
        crate::t!("Failover Queue", "故障转移队列"),
        // The overlay is fixed at 70 cols; keep these chips short enough
        // that the reorder hint always fits (Enter still toggles as a
        // hidden Space alias).
        &[
            ("↑↓", texts::tui_key_select()),
            ("f", crate::t!("auto failover", "自动故障转移")),
            ("Space", texts::tui_key_toggle()),
            ("</>/K/J", texts::tui_key_move()),
            ("Esc", texts::tui_key_close()),
        ],
        overlay_border_style(theme, false),
    );

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .split(body);

    let status = if data.proxy.auto_failover_enabled {
        crate::t!("Automatic failover: enabled", "自动故障转移：已开启")
    } else {
        crate::t!("Automatic failover: disabled", "自动故障转移：已关闭")
    };
    frame.render_widget(
        Paragraph::new(status)
            .style(Style::default().fg(theme.dim))
            .alignment(Alignment::Center),
        chunks[0],
    );

    let body_area = chunks[1];
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
        chunks[2],
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
    let transports = [
        crate::cli::tui::form::McpTransport::Stdio,
        crate::cli::tui::form::McpTransport::Http,
        crate::cli::tui::form::McpTransport::Sse,
    ];

    let body_area = overlay_frame(
        frame,
        content_area,
        theme,
        texts::tui_mcp_type_title(),
        &[
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_cancel()),
        ],
        OverlaySize::FitRows {
            width: 58,
            body_rows: transports.len() as u16,
        },
        overlay_border_style(theme, false),
    );

    let items = transports
        .iter()
        .map(|transport| ListItem::new(Line::raw(transport.label())));

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = ListState::default();
    state.select(Some(selected.min(transports.len().saturating_sub(1))));
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
    let body = overlay_frame_at(
        frame,
        centered_rect_fixed(OVERLAY_FIXED_LG.0, OVERLAY_FIXED_LG.1, content_area),
        theme,
        texts::tui_skills_import_title(),
        &[
            ("Space", texts::tui_key_select()),
            ("Enter", texts::tui_key_import()),
            ("r", texts::tui_key_refresh()),
            ("Esc", texts::tui_key_close()),
        ],
        overlay_border_style(theme, true),
    );

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(0)])
        .split(body);

    frame.render_widget(
        Paragraph::new(texts::tui_skills_import_description())
            .style(Style::default().fg(theme.dim))
            .wrap(Wrap { trim: false }),
        chunks[0],
    );

    let body_area = chunks[1];
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
    let methods = [
        crate::services::skill::SyncMethod::Auto,
        crate::services::skill::SyncMethod::Symlink,
        crate::services::skill::SyncMethod::Copy,
    ];

    let body_area = overlay_frame(
        frame,
        content_area,
        theme,
        texts::tui_skills_sync_method_title(),
        &[
            ("←→", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_cancel()),
        ],
        OverlaySize::FitRows {
            width: OVERLAY_FIXED_LG.0,
            body_rows: methods.len() as u16,
        },
        overlay_border_style(theme, false),
    );

    let current = data.skills.sync_method;

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

#[expect(
    clippy::too_many_arguments,
    reason = "app picker renderer receives list state and display labels"
)]
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
    let body_area = overlay_frame(
        frame,
        content_area,
        theme,
        &title,
        &[
            (toggle_key_label, texts::tui_key_toggle()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_cancel()),
        ],
        OverlaySize::FitRows {
            width: OVERLAY_FIXED_LG.0,
            body_rows: app_types.len() as u16,
        },
        overlay_border_style(theme, false),
    );

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
