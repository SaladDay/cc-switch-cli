use super::super::theme;
use super::super::*;
use super::frame::{overlay_frame, overlay_frame_at, OverlaySize};
use crate::cli::tui::form;
use crate::cli::tui::form::{HermesModelField, ProviderAddFormState};
use crate::cli::tui::text_edit::TextInput;

const OPENCLAW_AGENTS_MODEL_PICKER_MAX_VISIBLE_ROWS: usize = 32;
const OPENCLAW_AGENTS_MODEL_PICKER_MAX_LABEL_BYTES: usize = 4 * 1024;
const OPENCLAW_AGENTS_MODEL_PICKER_MAX_LABEL_WIDTH: usize = 512;
const SESSION_PROJECT_PICKER_WIDTH: u16 = 86;
const SESSION_PROJECT_PICKER_HEIGHT: u16 = 24;
const SESSION_PROJECT_PICKER_WIDE_MIN_WIDTH: u16 = 64;

pub(super) fn render_claude_model_picker_overlay(
    frame: &mut Frame<'_>,
    app: &App,
    content_area: Rect,
    theme: &theme::Theme,
    selected: usize,
    column: ClaudeModelPickerColumn,
    editing: bool,
) {
    // Keep the percentage-based width, but cap the height to the three role rows
    // rather than filling 62% of the screen (which left a large empty table).
    // Height = outer borders(2) + key bar(1) + top gap(1) + table[border(2)+header(1)+3 rows] + hint(3).
    let wide = centered_rect(OVERLAY_MD.0, OVERLAY_MD.1, content_area);
    let desired_h = 13u16.min(content_area.height);
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
    } else if column == ClaudeModelPickerColumn::OneM {
        vec![
            ("↑↓", texts::tui_key_select()),
            ("←→", texts::tui_key_column()),
            ("Enter", texts::tui_key_toggle()),
            ("Esc", texts::tui_key_close()),
        ]
    } else {
        vec![
            ("↑↓", texts::tui_key_select()),
            ("←→", texts::tui_key_column()),
            ("Enter", texts::tui_key_edit()),
            ("Space", texts::tui_key_fetch_model()),
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
            texts::tui_claude_default_haiku_model_label(),
            texts::tui_claude_default_sonnet_model_label(),
            texts::tui_claude_default_opus_model_label(),
        ];

        let raw_label_col_width = field_label_column_width(
            labels
                .iter()
                .copied()
                .chain(std::iter::once(texts::tui_header_field())),
            1,
        );

        // Reserve the 1M cell before allocating room to labels and model IDs.
        // This keeps the checkbox visible even when the terminal is narrow or a
        // provider returns an unusually long model name.
        let table_inner_width = body_area.width.saturating_sub(2);
        let column_spacing = 2u16;
        let usable_width = table_inner_width.saturating_sub(column_spacing);
        let one_m_col_width = 6u16.min(usable_width);
        let role_and_model_width = usable_width.saturating_sub(one_m_col_width);
        let min_model_width = 4u16.min(role_and_model_width);
        let label_col_width =
            raw_label_col_width.min(role_and_model_width.saturating_sub(min_model_width));
        let model_col_width = role_and_model_width.saturating_sub(label_col_width);
        let label_text_width = label_col_width.saturating_sub(1);
        let model_text_width = model_col_width.saturating_sub(1);

        let selected = selected.min(labels.len().saturating_sub(1));
        let column = if ProviderAddFormState::claude_model_supports_one_m(selected) {
            column
        } else {
            ClaudeModelPickerColumn::Model
        };

        let header = Row::new(vec![
            Cell::from(cell_pad(&truncate_to_display_width(
                texts::tui_header_field(),
                label_text_width,
            ))),
            Cell::from(cell_pad(&truncate_to_display_width(
                texts::tui_header_value(),
                model_text_width,
            ))),
            Cell::from(cell_pad("1M")),
        ])
        .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

        let rows = labels.iter().enumerate().map(|(idx, label)| {
            let value = provider
                .claude_model_input(idx)
                .map(|input| bounded_trimmed_text_for_display(&input.value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| texts::tui_na().to_string());
            let label = truncate_to_display_width(label, label_text_width);
            let value = truncate_to_display_width(&value, model_text_width);
            let row_selected = idx == selected;
            let supports_one_m = ProviderAddFormState::claude_model_supports_one_m(idx);
            let one_m = if supports_one_m {
                if provider.claude_model_one_m_enabled(idx) {
                    "[x]"
                } else {
                    "[ ]"
                }
            } else {
                "—"
            };

            let row_style = claude_model_picker_row_style(theme, row_selected);
            let model_style = if row_selected && column == ClaudeModelPickerColumn::Model {
                selection_style(theme)
            } else {
                row_style
            };
            let one_m_style = if !supports_one_m {
                Style::default().fg(theme.dim)
            } else if row_selected && column == ClaudeModelPickerColumn::OneM {
                selection_style(theme)
            } else {
                row_style
            };

            Row::new(vec![
                Cell::from(cell_pad(&label)).style(row_style),
                Cell::from(cell_pad(&value)).style(model_style),
                Cell::from(cell_pad(one_m)).style(one_m_style),
            ])
        });

        let table = Table::new(
            rows,
            [
                Constraint::Length(label_col_width),
                Constraint::Length(model_col_width),
                Constraint::Length(one_m_col_width),
            ],
        )
        .header(header)
        .column_spacing(1)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", texts::tui_form_fields_title())),
        );

        frame.render_widget(table, body_area);

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
        } else if column == ClaudeModelPickerColumn::OneM {
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
                        texts::tui_hint_toggle_one_m_declaration(),
                        Style::default().fg(theme.dim),
                    ),
                ]))
                .alignment(Alignment::Center),
                hint_inner,
            );
        } else {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(texts::tui_hint_press(), Style::default().fg(theme.dim)),
                    Span::styled(
                        "Space",
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

fn claude_model_picker_row_style(theme: &theme::Theme, selected: bool) -> Style {
    if !selected {
        return Style::default();
    }
    if theme.no_color {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.accent)
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

pub(super) fn render_s3_preset_picker_overlay(
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
        texts::tui_s3_service_preset(),
        &[
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_close()),
        ],
        OverlaySize::FitRows {
            width: 44,
            body_rows: form::S3Preset::ALL.len() as u16,
        },
        overlay_border_style(theme, false),
    );

    let current = match app.form.as_ref() {
        Some(FormState::S3Sync(form)) => form.preset,
        _ => return,
    };
    let items = form::S3Preset::ALL.iter().copied().map(|preset| {
        let marker = if preset == current {
            texts::tui_marker_active()
        } else {
            texts::tui_marker_inactive()
        };
        ListItem::new(Line::raw(format!("{marker}  {}", preset.label())))
    });
    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));
    let mut state = ListState::default();
    state.select(Some(
        selected.min(form::S3Preset::ALL.len().saturating_sub(1)),
    ));
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

pub(super) fn render_external_editor_picker_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    selected: usize,
    editors: &[crate::cli::editor::DetectedEditor],
) {
    let body_rows = u16::try_from(editors.len().saturating_add(1)).unwrap_or(u16::MAX);
    let body_area = overlay_frame(
        frame,
        content_area,
        theme,
        texts::tui_settings_preferred_editor_label(),
        &[
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_cancel()),
        ],
        OverlaySize::FitRows {
            width: 72,
            body_rows,
        },
        overlay_border_style(theme, false),
    );

    let configured = crate::settings::get_preferred_editor();
    let detected_selection = configured
        .as_deref()
        .and_then(|command| editors.iter().position(|editor| editor.command == command));
    let custom_active = configured.is_some() && detected_selection.is_none();
    let symbol_width =
        u16::try_from(UnicodeWidthStr::width(highlight_symbol(theme))).unwrap_or(u16::MAX);
    let available_width = body_area.width.saturating_sub(symbol_width);
    let option = |active: bool, label: String| {
        let marker = if active {
            texts::tui_marker_active()
        } else {
            texts::tui_marker_inactive()
        };
        ListItem::new(Line::raw(truncate_to_display_width(
            &format!("{marker}  {label}"),
            available_width,
        )))
    };

    let mut items = Vec::with_capacity(editors.len().saturating_add(1));
    items.extend(editors.iter().enumerate().map(|(index, editor)| {
        let label = bounded_trimmed_text_for_display(&editor.label);
        let command = bounded_trimmed_text_for_display(&editor.command);
        option(
            detected_selection == Some(index),
            format!("{label}  ·  {command}"),
        )
    }));
    items.push(option(
        custom_active,
        texts::tui_settings_preferred_editor_custom().to_string(),
    ));

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));
    let mut state = ListState::default();
    state.select(Some(selected.min(editors.len())));
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

    let row_count = status.accounts.len() + usize::from(binding);
    let selected = selected.min(row_count.saturating_sub(1));
    let visible = visible_selection_window(row_count, selected, body_area.height.max(1) as usize);
    let visible_start = visible.start;
    let items = visible.filter_map(|row| {
        if binding && row == 0 {
            let marker = if selected_account_id.is_none() {
                texts::tui_marker_active()
            } else {
                texts::tui_marker_inactive()
            };
            return Some(ListItem::new(Line::raw(format!(
                "{marker}  {}",
                texts::tui_managed_accounts_follow_default()
            ))));
        }

        let account = status
            .accounts
            .get(row.saturating_sub(usize::from(binding)))?;
        let marker = if selected_account_id == Some(account.id.as_str()) {
            texts::tui_marker_active()
        } else {
            texts::tui_marker_inactive()
        };
        let login = bounded_trimmed_text_for_display(&account.login);
        let suffix = if account.is_default {
            format!(" ({})", texts::tui_managed_accounts_default())
        } else {
            String::new()
        };
        Some(ListItem::new(Line::raw(format!(
            "{marker}  {login}{suffix}"
        ))))
    });

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));
    let mut state = ListState::default();
    state.select(Some(selected.saturating_sub(visible_start)));
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
    let provider_name = bounded_trimmed_text_for_display(&provider.name.value);
    let title = texts::tui_hermes_models_title(&provider_name);
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

    let field_count = provider.hermes_models.len().saturating_mul(3);
    if field_count == 0 {
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
        // Each model consumes three data rows and, except for the first, one
        // separator. Slice before constructing rows so periodic redraws stay
        // proportional to the viewport rather than the full imported catalog.
        let selected_field_idx = provider.hermes_models_field_idx.min(field_count - 1);
        let selected_model_idx = selected_field_idx / 3;
        let table_row_capacity = chunks[0].height.saturating_sub(3) as usize;
        let model_capacity = (table_row_capacity.saturating_add(1) / 4).max(1);
        let visible_models = visible_selection_window(
            provider.hermes_models.len(),
            selected_model_idx,
            model_capacity,
        );
        let mut rows_data = Vec::with_capacity(visible_models.len().saturating_mul(3));
        for model_idx in visible_models.clone() {
            for field_offset in 0..3 {
                rows_data.push(hermes_model_field_label_and_value(
                    provider,
                    hermes_model_field_at(model_idx, field_offset),
                ));
            }
        }
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
        let mut rows = Vec::with_capacity(
            rows_data
                .len()
                .saturating_add(visible_models.len().saturating_sub(1)),
        );
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
        let selected_model_offset = selected_model_idx.saturating_sub(visible_models.start);
        state.select(Some(
            selected_model_offset
                .saturating_mul(4)
                .saturating_add(selected_field_idx % 3),
        ));
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

fn hermes_model_field_at(model_idx: usize, field_offset: usize) -> HermesModelField {
    match field_offset {
        0 => HermesModelField::Id(model_idx),
        1 => HermesModelField::Name(model_idx),
        _ => HermesModelField::ContextLength(model_idx),
    }
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
                .map(bounded_trimmed_text_for_display)
                .or_else(|| value.as_i64().map(|number| number.to_string()))
                .or_else(|| value.as_u64().map(|number| number.to_string()))
        })
        .filter(|value| !value.is_empty())
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
    fetching: bool,
    models: &[String],
    filtered_indices: Option<&[usize]>,
    filter_incomplete: bool,
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

    let mut list_area = chunks[1];
    if fetching {
        let text = texts::tui_loading().to_string();
        let p = Paragraph::new(Line::styled(text, Style::default().fg(theme.accent)))
            .alignment(Alignment::Center);
        frame.render_widget(p, list_area);
        return;
    }

    if let Some(err) = error {
        let err = bounded_trimmed_text_for_display(err);
        let p = Paragraph::new(Line::styled(err, Style::default().fg(theme.err)))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
        frame.render_widget(p, list_area);
        return;
    }

    if filter_incomplete {
        let limited_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(list_area);
        frame.render_widget(
            Paragraph::new(Line::styled(
                texts::tui_model_fetch_results_limited(),
                Style::default().fg(theme.warn),
            ))
            .alignment(Alignment::Center),
            limited_chunks[0],
        );
        list_area = limited_chunks[1];
    }

    let filtered_len = filtered_indices.map_or(models.len(), <[usize]>::len);
    if filtered_len == 0 {
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

    let selected_idx = selected_idx.min(filtered_len - 1);
    let visible =
        visible_selection_window(filtered_len, selected_idx, list_area.height.max(1) as usize);
    let visible_start = visible.start;
    let items = visible.filter_map(|filtered_index| {
        let model_index = match filtered_indices {
            Some(indices) => indices.get(filtered_index).copied()?,
            None => filtered_index,
        };
        let model = models.get(model_index)?;
        Some(ListItem::new(Line::raw(bounded_trimmed_text_for_display(
            model,
        ))))
    });

    let list = List::new(items)
        .block(Block::default().borders(Borders::NONE))
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(selected_idx.saturating_sub(visible_start)));

    frame.render_stateful_widget(list, list_area, &mut state);
}

pub(super) fn render_session_project_picker_overlay(
    frame: &mut Frame<'_>,
    app: &App,
    content_area: Rect,
    theme: &theme::Theme,
    picker: &app::SessionProjectPickerState,
) {
    let body = overlay_frame(
        frame,
        content_area,
        theme,
        texts::tui_sessions_project_picker_title(),
        &[
            ("↑↓", texts::tui_key_select()),
            ("Shift+←→", texts::tui_key_scroll()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_close()),
        ],
        OverlaySize::Fixed(SESSION_PROJECT_PICKER_WIDTH, SESSION_PROJECT_PICKER_HEIGHT),
        overlay_border_style(theme, false),
    );

    // Keep one full-path viewport even on a short terminal. The selected row
    // also uses the same viewport when there is not enough room for this pane.
    let preview_height = if body.height >= 7 { 3 } else { 0 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(preview_height),
        ])
        .split(body);
    render_session_project_filter_input(frame, picker, chunks[0], theme);

    let option_count = app::session_project_option_count(&app.sessions, picker);
    let filtered_len = picker
        .filtered_indices
        .as_ref()
        .map_or(option_count, Vec::len);
    let selected_idx = picker.selected_idx.min(filtered_len.saturating_sub(1));
    let selected_option = session_project_picker_option_at(app, picker, selected_idx, option_count);

    render_session_project_options(
        frame,
        app,
        picker,
        chunks[1],
        theme,
        option_count,
        filtered_len,
        selected_idx,
    );
    if preview_height > 0 {
        render_session_project_preview(
            frame,
            selected_option,
            &app.sessions.project_scope,
            picker.path_scroll,
            chunks[2],
            theme,
        );
    }
}

fn render_session_project_filter_input(
    frame: &mut Frame<'_>,
    picker: &app::SessionProjectPickerState,
    area: Rect,
    theme: &theme::Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .title(format!(" {} ", texts::tui_sessions_project_filter_title()));
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);
    let (visible, cursor_x) = visible_text_window(
        &picker.input.value,
        picker.input.cursor,
        inner.width as usize,
    );
    let (text, style) = if picker.input.value.is_empty() {
        (
            texts::tui_sessions_project_filter_placeholder().to_string(),
            Style::default().fg(theme.dim),
        )
    } else {
        (visible, Style::default())
    };
    frame.render_widget(
        Paragraph::new(Line::styled(text, style)).wrap(Wrap { trim: false }),
        inner,
    );
    if inner.width > 0 && inner.height > 0 {
        frame.set_cursor_position((
            inner.x + cursor_x.min(inner.width.saturating_sub(1)),
            inner.y,
        ));
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "project picker rendering keeps catalog and viewport state explicit"
)]
fn render_session_project_options(
    frame: &mut Frame<'_>,
    app: &App,
    picker: &app::SessionProjectPickerState,
    area: Rect,
    theme: &theme::Theme,
    option_count: usize,
    filtered_len: usize,
    selected_idx: usize,
) {
    if !app.sessions.project_catalog_is_current() {
        if app.sessions.project_catalog_loading {
            render_session_project_status(
                frame,
                area,
                texts::tui_sessions_projects_loading(),
                Style::default().fg(theme.accent),
            );
        } else if let Some(error) = app.sessions.project_catalog_error.as_deref() {
            let error = bounded_trimmed_text_for_display(error);
            render_session_project_status(frame, area, &error, Style::default().fg(theme.err));
        } else {
            render_session_project_status(
                frame,
                area,
                texts::tui_sessions_projects_loading(),
                Style::default().fg(theme.dim),
            );
        }
        return;
    }

    if app.sessions.project_filter_active.is_some() {
        render_session_project_status(
            frame,
            area,
            texts::tui_sessions_project_filtering(),
            Style::default().fg(theme.accent),
        );
        return;
    }

    if let Some(error) = picker.filter_error.as_deref() {
        let error = bounded_trimmed_text_for_display(error);
        render_session_project_status(frame, area, &error, Style::default().fg(theme.err));
        return;
    }

    if filtered_len == 0 {
        render_session_project_status(
            frame,
            area,
            texts::tui_sessions_projects_no_matches(),
            Style::default().fg(theme.dim),
        );
        return;
    }

    let wide = area.width >= SESSION_PROJECT_PICKER_WIDE_MIN_WIDTH;
    let item_height = if wide { 1 } else { 2 };
    let capacity = (area.height as usize / item_height).max(1);
    let visible = visible_selection_window(filtered_len, selected_idx, capacity);
    let visible_start = visible.start;
    let row_width = area
        .width
        .saturating_sub(UnicodeWidthStr::width(highlight_symbol(theme)) as u16);
    let items = visible.filter_map(|filtered_index| {
        let option = session_project_picker_option_at(app, picker, filtered_index, option_count)?;
        let active = session_project_option_is_active(option, &app.sessions.project_scope);
        Some(session_project_list_item(
            option,
            active,
            row_width,
            wide,
            (filtered_index == selected_idx).then_some(picker.path_scroll),
            area.height < 2,
            theme,
        ))
    });
    let list = List::new(items)
        .block(Block::default().borders(Borders::NONE))
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));
    let mut state = ListState::default();
    state.select(Some(selected_idx.saturating_sub(visible_start)));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_session_project_status(frame: &mut Frame<'_>, area: Rect, text: &str, style: Style) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let status_area = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(1) / 2,
        width: area.width,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Line::styled(text.to_string(), style)).alignment(Alignment::Center),
        status_area,
    );
}

fn session_project_picker_option_at<'a>(
    app: &'a App,
    picker: &'a app::SessionProjectPickerState,
    filtered_index: usize,
    option_count: usize,
) -> Option<app::SessionProjectOption<'a>> {
    let option_index = match picker.filtered_indices.as_deref() {
        Some(indices) => indices.get(filtered_index).copied()?,
        None => filtered_index,
    };
    (option_index < option_count)
        .then(|| app::session_project_option_at(&app.sessions, picker, option_index))
        .flatten()
}

fn session_project_option_is_active(
    option: app::SessionProjectOption<'_>,
    scope: &crate::session_manager::project_scope::SessionProjectScope,
) -> bool {
    use crate::session_manager::project_scope::SessionProjectScope;

    match (option, scope) {
        (app::SessionProjectOption::All { .. }, SessionProjectScope::All)
        | (app::SessionProjectOption::Unknown { .. }, SessionProjectScope::Unknown) => true,
        (
            app::SessionProjectOption::Exact {
                normalized_path, ..
            },
            SessionProjectScope::Exact {
                normalized_path: active,
                ..
            },
        ) => normalized_path == active,
        _ => false,
    }
}

fn session_project_list_item(
    option: app::SessionProjectOption<'_>,
    active: bool,
    width: u16,
    wide: bool,
    selected_path_scroll: Option<usize>,
    compact_height: bool,
    theme: &theme::Theme,
) -> ListItem<'static> {
    let marker = if active {
        texts::tui_marker_active()
    } else {
        texts::tui_marker_inactive()
    };
    let label = session_project_option_label(option);
    let count = texts::tui_sessions_project_count(option.session_count());
    let path = match option {
        app::SessionProjectOption::Exact { display_path, .. } => display_path,
        _ => "",
    };

    if compact_height && !path.is_empty() {
        let prefix = format!("{marker} ");
        let fixed_width = UnicodeWidthStr::width(prefix.as_str()) as u16
            + UnicodeWidthStr::width(count.as_str()) as u16
            + 1;
        let path = project_path_window(
            path,
            selected_path_scroll.unwrap_or_default(),
            width.saturating_sub(fixed_width),
        );
        return ListItem::new(Line::from(vec![
            Span::raw(prefix),
            Span::styled(path, Style::default().fg(theme.comment)),
            Span::raw(" "),
            Span::styled(count, Style::default().fg(theme.dim)),
        ]));
    }

    if wide {
        return ListItem::new(session_project_wide_line(
            marker,
            &label,
            path,
            &count,
            width,
            selected_path_scroll,
            theme,
        ));
    }

    let first = session_project_label_count_line(marker, &label, &count, width);
    let prefix = "   ";
    let path_width = width.saturating_sub(UnicodeWidthStr::width(prefix) as u16);
    let path = match selected_path_scroll {
        Some(scroll) => project_path_window(path, scroll, path_width),
        None => {
            truncate_middle_to_display_width(&bounded_trimmed_text_for_display(path), path_width)
        }
    };
    let second = Line::from(vec![
        Span::raw(prefix.to_string()),
        Span::styled(path, Style::default().fg(theme.comment)),
    ]);
    ListItem::new(Text::from(vec![first, second]))
}

fn session_project_option_label(option: app::SessionProjectOption<'_>) -> String {
    match option {
        app::SessionProjectOption::All { .. } => texts::tui_sessions_all_projects().to_string(),
        app::SessionProjectOption::Unknown { .. } => {
            texts::tui_sessions_unknown_project().to_string()
        }
        app::SessionProjectOption::Exact { display_path, .. } => {
            let trimmed = display_path.trim_end_matches(['/', '\\']);
            let basename = trimmed
                .rsplit(['/', '\\'])
                .find(|part| !part.is_empty())
                .unwrap_or(trimmed);
            let basename = bounded_trimmed_text_for_display(basename);
            if basename.is_empty() {
                bounded_trimmed_text_for_display(display_path)
            } else {
                basename
            }
        }
    }
}

fn session_project_label_count_line(
    marker: &str,
    label: &str,
    count: &str,
    width: u16,
) -> Line<'static> {
    let prefix = format!("{marker}  ");
    let prefix_width = UnicodeWidthStr::width(prefix.as_str()) as u16;
    let count_width = UnicodeWidthStr::width(count) as u16;
    let label_width =
        width.saturating_sub(prefix_width.saturating_add(count_width).saturating_add(2));
    let label = truncate_to_display_width(label, label_width);
    let padding = width
        .saturating_sub(prefix_width)
        .saturating_sub(UnicodeWidthStr::width(label.as_str()) as u16)
        .saturating_sub(count_width)
        .max(1);
    Line::raw(format!(
        "{prefix}{label}{}{count}",
        " ".repeat(padding as usize)
    ))
}

fn session_project_wide_line(
    marker: &str,
    label: &str,
    path: &str,
    count: &str,
    width: u16,
    selected_path_scroll: Option<usize>,
    theme: &theme::Theme,
) -> Line<'static> {
    let prefix = format!("{marker}  ");
    let prefix_width = UnicodeWidthStr::width(prefix.as_str()) as u16;
    let count_width = UnicodeWidthStr::width(count) as u16;
    let remaining = width.saturating_sub(prefix_width);
    if path.is_empty() || remaining < count_width.saturating_add(18) {
        return session_project_label_count_line(marker, label, count, width);
    }

    let label_width = (remaining / 4).clamp(10, 22);
    let path_width = remaining
        .saturating_sub(label_width)
        .saturating_sub(count_width)
        .saturating_sub(4);
    let label = truncate_to_display_width(label, label_width);
    let label_padding = label_width.saturating_sub(UnicodeWidthStr::width(label.as_str()) as u16);
    let path = match selected_path_scroll {
        Some(scroll) => project_path_window(path, scroll, path_width),
        None => {
            truncate_middle_to_display_width(&bounded_trimmed_text_for_display(path), path_width)
        }
    };
    let path_padding = path_width.saturating_sub(UnicodeWidthStr::width(path.as_str()) as u16);
    Line::from(vec![
        Span::raw(prefix),
        Span::raw(label),
        Span::raw(" ".repeat(label_padding as usize + 2)),
        Span::styled(path, Style::default().fg(theme.comment)),
        Span::raw(" ".repeat(path_padding as usize + 2)),
        Span::styled(count.to_string(), Style::default().fg(theme.dim)),
    ])
}

fn project_path_window(text: &str, scroll: usize, width: u16) -> String {
    let width = width as usize;
    if width == 0 || text.is_empty() {
        return String::new();
    }

    if scroll == usize::MAX {
        let mut full = String::new();
        let mut used = 0usize;
        let mut fits = true;
        for ch in text.chars() {
            let visible = if ch.is_control() { '�' } else { ch };
            let char_width = UnicodeWidthChar::width(visible).unwrap_or(1).max(1);
            if used.saturating_add(char_width) > width {
                fits = false;
                break;
            }
            full.push(visible);
            used = used.saturating_add(char_width);
        }
        if fits {
            return full;
        }
    }

    let mut start = if scroll == usize::MAX {
        let suffix_budget = width.saturating_sub(1);
        let mut used = 0usize;
        let mut suffix_start = text.len();
        for (offset, ch) in text.char_indices().rev() {
            let char_width = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
            if used.saturating_add(char_width) > suffix_budget {
                break;
            }
            suffix_start = offset;
            used = used.saturating_add(char_width);
        }
        suffix_start
    } else {
        scroll.min(text.len())
    };
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }

    let has_left = start > 0;
    let content_width = width.saturating_sub(usize::from(has_left));
    let mut value = String::new();
    let mut used = 0usize;
    let mut end_byte = start;
    for (offset, ch) in text[start..].char_indices() {
        let visible = if ch.is_control() { '�' } else { ch };
        let char_width = UnicodeWidthChar::width(visible).unwrap_or(1).max(1);
        if used.saturating_add(char_width) > content_width {
            break;
        }
        value.push(visible);
        used = used.saturating_add(char_width);
        end_byte = start + offset + ch.len_utf8();
    }
    let has_right = end_byte < text.len();
    if has_right && content_width > 0 {
        let target = content_width.saturating_sub(1);
        while used > target {
            let Some(ch) = value.pop() else {
                break;
            };
            used = used.saturating_sub(UnicodeWidthChar::width(ch).unwrap_or(1).max(1));
        }
        value.push('…');
    }
    if has_left {
        value.insert(0, '…');
    }
    value
}

fn truncate_middle_to_display_width(text: &str, width: u16) -> String {
    let width = width as usize;
    if width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    if width == 1 {
        return "…".to_string();
    }

    let available = width - 1;
    let left_width = available / 2;
    let right_width = available - left_width;
    let mut left = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used.saturating_add(ch_width) > left_width {
            break;
        }
        left.push(ch);
        used = used.saturating_add(ch_width);
    }

    let mut right_reversed = Vec::new();
    used = 0;
    for ch in text.chars().rev() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used.saturating_add(ch_width) > right_width {
            break;
        }
        right_reversed.push(ch);
        used = used.saturating_add(ch_width);
    }
    right_reversed.reverse();
    format!("{left}…{}", right_reversed.into_iter().collect::<String>())
}

fn render_session_project_preview(
    frame: &mut Frame<'_>,
    option: Option<app::SessionProjectOption<'_>>,
    scope: &crate::session_manager::project_scope::SessionProjectScope,
    path_scroll: usize,
    area: Rect,
    theme: &theme::Theme,
) {
    let value = match option {
        Some(app::SessionProjectOption::All { .. }) => texts::tui_sessions_all_projects(),
        Some(app::SessionProjectOption::Unknown { .. }) => texts::tui_sessions_unknown_project(),
        Some(app::SessionProjectOption::Exact { display_path, .. }) => display_path,
        None => match scope {
            crate::session_manager::project_scope::SessionProjectScope::All => {
                texts::tui_sessions_all_projects()
            }
            crate::session_manager::project_scope::SessionProjectScope::Unknown => {
                texts::tui_sessions_unknown_project()
            }
            crate::session_manager::project_scope::SessionProjectScope::Exact {
                display_path,
                ..
            } => display_path,
        },
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.dim))
        .title(format!(" {} ", texts::tui_sessions_project_directory()));
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);
    frame.render_widget(
        Paragraph::new(project_path_window(value, path_scroll, inner.width))
            .style(Style::default().fg(theme.comment)),
        inner,
    );
}

pub(super) fn render_openclaw_agents_fallback_picker_overlay(
    frame: &mut Frame<'_>,
    app: &App,
    content_area: Rect,
    theme: &theme::Theme,
    selected: usize,
    active: Option<usize>,
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

    let selected_idx = (has_selection && !options.is_empty())
        .then(|| selected.min(options.len().saturating_sub(1)));
    let visible = openclaw_agents_picker_visible_window(
        options.len(),
        selected_idx.unwrap_or(0),
        body_area.height as usize,
    );
    let visible_start = visible.start;
    let visible_end = visible.end;
    let items = visible.map(|option_idx| {
        let option = &options[option_idx];
        let marker = if active == Some(option_idx) {
            texts::tui_marker_active()
        } else {
            texts::tui_marker_inactive()
        };
        let prefix_width = UnicodeWidthStr::width(highlight_symbol(theme))
            .saturating_add(UnicodeWidthStr::width(marker))
            .saturating_add(2);
        let label_width = (body_area.width as usize).saturating_sub(prefix_width);
        let label = bounded_openclaw_agents_model_label(&option.label, label_width);
        ListItem::new(Line::from(vec![
            Span::raw(marker),
            Span::raw("  "),
            Span::raw(label),
        ]))
    });

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = ListState::default();
    state.select(
        selected_idx
            .filter(|selected| (visible_start..visible_end).contains(selected))
            .map(|selected| selected.saturating_sub(visible_start)),
    );
    frame.render_stateful_widget(list, body_area, &mut state);
}

fn openclaw_agents_picker_visible_window(
    len: usize,
    selected: usize,
    body_height: usize,
) -> std::ops::Range<usize> {
    visible_selection_window(
        len,
        selected,
        body_height.min(OPENCLAW_AGENTS_MODEL_PICKER_MAX_VISIBLE_ROWS),
    )
}

/// Build a passive picker label without scanning or allocating the complete
/// imported value. The original option remains untouched for selection/save.
fn bounded_openclaw_agents_model_label(label: &str, available_width: usize) -> String {
    let width = available_width.min(OPENCLAW_AGENTS_MODEL_PICKER_MAX_LABEL_WIDTH);
    if width == 0 || label.is_empty() {
        return String::new();
    }

    let mut output = String::with_capacity(
        label
            .len()
            .min(OPENCLAW_AGENTS_MODEL_PICKER_MAX_LABEL_BYTES)
            .min(width.saturating_mul(4)),
    );
    let mut used_width = 0usize;
    let mut scanned_end = 0usize;
    let mut started = false;
    let mut truncated = false;

    for (byte_idx, ch) in label.char_indices() {
        let char_end = byte_idx.saturating_add(ch.len_utf8());
        if char_end > OPENCLAW_AGENTS_MODEL_PICKER_MAX_LABEL_BYTES {
            truncated = true;
            break;
        }
        scanned_end = char_end;

        if !started && ch.is_whitespace() {
            continue;
        }
        started = true;
        let display_char = if (ch.is_whitespace() && ch != ' ') || ch.is_control() {
            ' '
        } else {
            ch
        };
        let char_width = UnicodeWidthChar::width(display_char).unwrap_or(0);
        if used_width.saturating_add(char_width) > width {
            truncated = true;
            break;
        }
        output.push(display_char);
        used_width = used_width.saturating_add(char_width);
    }
    truncated |= scanned_end < label.len();

    while output.chars().next_back().is_some_and(char::is_whitespace) {
        let Some(ch) = output.pop() else {
            break;
        };
        used_width = used_width.saturating_sub(UnicodeWidthChar::width(ch).unwrap_or(0));
    }

    if truncated {
        while used_width > width.saturating_sub(1) {
            let Some(ch) = output.pop() else {
                break;
            };
            used_width = used_width.saturating_sub(UnicodeWidthChar::width(ch).unwrap_or(0));
        }
        output.push('…');
    }

    output
}

#[cfg(test)]
mod openclaw_agents_picker_tests {
    use super::*;

    #[test]
    fn large_picker_window_is_fixed_and_contains_first_middle_and_last_selections() {
        let cases = [(0, 0..32), (5_000, 4_984..5_016), (9_999, 9_968..10_000)];

        for (selected, expected) in cases {
            let actual = openclaw_agents_picker_visible_window(10_000, selected, usize::MAX);
            assert_eq!(actual, expected);
            assert_eq!(actual.len(), OPENCLAW_AGENTS_MODEL_PICKER_MAX_VISIBLE_ROWS);
            assert!(actual.contains(&selected));
        }
    }

    #[test]
    fn model_label_scan_is_bounded_by_bytes_and_terminal_width() {
        let source = format!("SELECTED-{}", "你".repeat(1_000_000));
        let rendered = bounded_openclaw_agents_model_label(&source, 20);

        assert!(rendered.starts_with("SELECTED-"));
        assert!(rendered.ends_with('…'));
        assert!(UnicodeWidthStr::width(rendered.as_str()) <= 20);
        assert!(rendered.len() <= OPENCLAW_AGENTS_MODEL_PICKER_MAX_LABEL_BYTES + '…'.len_utf8());
        assert_eq!(source.len(), "SELECTED-".len() + 3_000_000);

        let leading_space = " ".repeat(OPENCLAW_AGENTS_MODEL_PICKER_MAX_LABEL_BYTES * 2);
        assert_eq!(bounded_openclaw_agents_model_label(&leading_space, 20), "…");
    }
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
    selected_provider_id: Option<&str>,
) {
    let rows = app::failover_queue_rows(data);
    let queued_count = rows
        .iter()
        .filter(|row| row.provider.in_failover_queue)
        .count();
    let visible_rows = rows.len().min(10) as u16;
    let body = overlay_frame_at(
        frame,
        centered_rect_fixed(
            78,
            visible_rows.saturating_add(7).clamp(10, 18),
            content_area,
        ),
        theme,
        crate::t!("Failover Queue", "故障转移队列"),
        &[
            ("↑↓", texts::tui_key_select()),
            ("f", crate::t!("auto failover", "自动故障转移")),
            ("Enter", texts::tui_key_add_remove()),
            ("Ctrl+↑↓", texts::tui_key_move()),
            ("Esc", texts::tui_key_close()),
        ],
        overlay_border_style(theme, false),
    );

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(body);

    let mode = if data.proxy.auto_failover_enabled {
        crate::t!("Auto: on", "自动：开")
    } else {
        crate::t!("Auto: off", "自动：关")
    };
    let active_target = data.proxy.current_app_target.as_ref().map(|target| {
        let provider_id = target.provider_id.trim();
        let display_name = rows
            .iter()
            .find(|row| !provider_id.is_empty() && row.id == provider_id)
            .map(|row| row.provider.name.trim())
            .filter(|name| !name.is_empty())
            .or_else(|| {
                let name = target.provider_name.trim();
                (!name.is_empty()).then_some(name)
            })
            .unwrap_or(provider_id);
        match app::failover_queue_position(data, provider_id) {
            Some(priority) => format!("P{priority} {display_name}"),
            None => display_name.to_string(),
        }
    });
    let status = crate::t!(
        format!(
            "{mode} · Active target: {} · {queued_count} queued",
            active_target.as_deref().unwrap_or("—")
        ),
        format!(
            "{mode} · 当前目标：{} · 队列 {queued_count}",
            active_target.as_deref().unwrap_or("—")
        )
    );
    frame.render_widget(
        Paragraph::new(status)
            .style(Style::default().fg(theme.dim))
            .alignment(Alignment::Center),
        chunks[0],
    );

    let body_area = chunks[1];
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
            Cell::from(crate::t!("Priority", "优先级")),
            Cell::from(texts::header_name()),
            Cell::from(crate::t!("Status", "状态")),
        ])
        .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

        let table_rows = rows.iter().map(|row| {
            let marker = if row.provider.in_failover_queue {
                texts::tui_marker_active()
            } else {
                texts::tui_marker_inactive()
            };
            let queue = app::failover_queue_position(data, &row.id)
                .map(|position| format!("P{position}"))
                .unwrap_or_else(|| "-".to_string());
            let is_active_target = data
                .proxy
                .current_app_target
                .as_ref()
                .is_some_and(|target| target.provider_id == row.id);
            let (status, status_style) = if is_active_target {
                (
                    crate::t!("target", "当前目标").to_string(),
                    Style::default().fg(theme.cyan).add_modifier(Modifier::BOLD),
                )
            } else if !row.provider.in_failover_queue {
                (
                    crate::t!("not queued", "未参与").to_string(),
                    Style::default().fg(theme.dim),
                )
            } else {
                match data.proxy.provider_health.get(&row.id) {
                    Some(health) if !health.is_healthy => (
                        crate::t!(
                            format!("unhealthy ({})", health.consecutive_failures),
                            format!("不健康 ({})", health.consecutive_failures)
                        ),
                        Style::default().fg(theme.err),
                    ),
                    Some(health) if health.consecutive_failures > 0 => (
                        crate::t!(
                            format!("failures ({})", health.consecutive_failures),
                            format!("失败 ({})", health.consecutive_failures)
                        ),
                        Style::default().fg(theme.warn),
                    ),
                    Some(_) => (
                        crate::t!("normal", "正常").to_string(),
                        Style::default().fg(theme.ok),
                    ),
                    None => (
                        crate::t!("no record", "无记录").to_string(),
                        Style::default().fg(theme.dim),
                    ),
                }
            };

            Row::new(vec![
                Cell::from(marker),
                Cell::from(queue),
                Cell::from(row.provider.name.as_str()),
                Cell::from(status).style(status_style),
            ])
        });

        let status_width = if body_area.width >= 60 {
            18
        } else if body_area.width >= 44 {
            14
        } else {
            10
        };

        let table = Table::new(
            table_rows,
            [
                Constraint::Length(2),
                Constraint::Length(8),
                Constraint::Min(8),
                Constraint::Length(status_width),
            ],
        )
        .header(header)
        .block(Block::default().borders(Borders::NONE))
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

        let mut state = TableState::default();
        state.select(app::failover_queue_selected_index(
            data,
            selected_provider_id,
        ));
        frame.render_stateful_widget(table, body_area, &mut state);
    }
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
            ("Space", texts::tui_key_toggle()),
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

#[cfg(test)]
mod session_project_path_tests {
    use super::*;

    #[test]
    fn selected_project_path_window_can_reach_both_ends() {
        let path = "/workspace/one/shared-name";
        let start = project_path_window(path, 0, 12);
        let end = project_path_window(path, usize::MAX, 12);

        assert!(start.starts_with("/workspace"));
        assert!(start.ends_with('…'));
        assert!(end.starts_with('…'));
        assert!(end.ends_with("shared-name"));
        assert!(UnicodeWidthStr::width(start.as_str()) <= 12);
        assert!(UnicodeWidthStr::width(end.as_str()) <= 12);
    }

    #[test]
    fn selected_project_path_window_repairs_utf8_scroll_boundary() {
        let path = "/目录/very-long-project";
        let window = project_path_window(path, 2, 10);

        assert!(window.starts_with('…'));
        assert!(UnicodeWidthStr::width(window.as_str()) <= 10);
    }
}
