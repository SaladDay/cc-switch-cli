use crate::cli::tui::data;

use super::*;

fn openclaw_status_label(row: &ProviderRow) -> &'static str {
    if row.is_default_model {
        texts::tui_openclaw_status_default()
    } else if row.is_in_config {
        texts::tui_openclaw_status_in_config_and_saved()
    } else if row.is_saved {
        texts::tui_openclaw_status_saved_only()
    } else {
        texts::tui_openclaw_status_untracked()
    }
}

fn opencode_status_label(row: &ProviderRow) -> &'static str {
    if row.is_in_config {
        texts::tui_provider_status_in_config()
    } else if row.is_saved {
        texts::tui_provider_status_saved_only()
    } else {
        texts::tui_provider_status_untracked()
    }
}

pub(super) fn provider_rows_filtered<'a>(app: &App, data: &'a UiData) -> Vec<&'a ProviderRow> {
    let query = app.filter.query_lower();
    data.providers
        .rows
        .iter()
        .filter(|row| match &query {
            None => true,
            Some(q) => {
                data::provider_display_name(&app.app_type, row)
                    .to_lowercase()
                    .contains(q)
                    || row.provider.name.to_lowercase().contains(q)
                    || row.id.to_lowercase().contains(q)
            }
        })
        .collect()
}

fn provider_name_with_quota_line(
    app: &App,
    data: &UiData,
    row: &ProviderRow,
    show_quota: bool,
    theme: &super::theme::Theme,
) -> Line<'static> {
    let mut spans = vec![Span::raw(data::provider_display_name(&app.app_type, row))];
    if show_quota {
        if let Some(quota) = quota_compact_line(data.quota.state_for(&row.id), theme, true) {
            spans.push(Span::styled("  (", Style::default().fg(theme.comment)));
            spans.extend(quota.spans);
            spans.push(Span::styled(")", Style::default().fg(theme.comment)));
        }
    }
    Line::from(spans)
}

pub(super) fn render_providers(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let header_style = Style::default().fg(theme.dim).add_modifier(Modifier::BOLD);
    let table_style = Style::default();

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(texts::menu_manage_providers());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    let visible = provider_rows_filtered(app, data);
    let selected_supports_quota = visible
        .get(app.provider_idx)
        .is_some_and(|row| data::quota_target_for_provider(&app.app_type, row).is_some());

    if app.focus == Focus::Content {
        let mut keys = vec![("Enter", texts::tui_key_details())];
        if matches!(
            app.app_type,
            crate::app_config::AppType::OpenCode | crate::app_config::AppType::OpenClaw
        ) {
            keys.extend([
                ("s", texts::tui_key_add_remove()),
                ("a", texts::tui_key_add()),
                ("d", texts::tui_key_delete()),
                ("t", texts::tui_key_speedtest()),
            ]);
            if let Some(row) = visible.get(app.provider_idx) {
                keys.push(("e", texts::tui_key_edit()));
                if selected_supports_quota {
                    keys.push(("r", texts::tui_key_refresh()));
                }
                if matches!(app.app_type, crate::app_config::AppType::OpenClaw) && row.is_in_config
                {
                    keys.push(("x", texts::tui_key_set_default()));
                }
            }
            if matches!(app.app_type, crate::app_config::AppType::OpenCode) {
                keys.push(("c", texts::tui_key_stream_check()));
            }
        } else {
            keys.extend([
                ("s", texts::tui_key_switch()),
                ("a", texts::tui_key_add()),
                ("e", texts::tui_key_edit()),
                ("d", texts::tui_key_delete()),
            ]);
            if selected_supports_quota {
                keys.push(("r", texts::tui_key_refresh()));
            }
            keys.push(("t", texts::tui_key_speedtest()));
            if crate::cli::tui::app::supports_temporary_provider_launch(&app.app_type) {
                keys.push(("o", texts::tui_key_launch_temp()));
            }
            keys.push(("c", texts::tui_key_stream_check()));
        }
        render_key_bar_center(frame, chunks[0], theme, &keys);
    }

    let header = Row::new(vec![
        Cell::from(""),
        Cell::from(texts::header_name()),
        Cell::from(texts::tui_header_api_url()),
    ])
    .style(header_style);

    let rows = visible.iter().enumerate().map(|(idx, row)| {
        let marker = if matches!(app.app_type, crate::app_config::AppType::OpenClaw) {
            if row.is_default_model {
                "*"
            } else if row.is_in_config {
                "+"
            } else {
                ""
            }
        } else if matches!(app.app_type, crate::app_config::AppType::OpenCode) {
            if row.is_in_config {
                "+"
            } else {
                ""
            }
        } else if row.is_current {
            texts::tui_marker_active()
        } else {
            texts::tui_marker_inactive()
        };
        let api = row.api_url.as_deref().unwrap_or(texts::tui_na());
        let show_quota = row.is_current || idx == app.provider_idx;
        Row::new(vec![
            Cell::from(marker),
            Cell::from(provider_name_with_quota_line(
                app, data, row, show_quota, theme,
            )),
            Cell::from(api),
        ])
    });

    let constraints = vec![
        Constraint::Length(2),
        Constraint::Percentage(48),
        Constraint::Percentage(52),
    ];

    let table = Table::new(rows, constraints)
        .header(header)
        .style(table_style)
        .block(Block::default().borders(Borders::NONE))
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.provider_idx));

    frame.render_stateful_widget(table, inset_left(chunks[1], CONTENT_INSET_LEFT), &mut state);
}

pub(super) fn render_provider_detail(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
    id: &str,
) {
    let Some(row) = data.providers.rows.iter().find(|p| p.id == id) else {
        frame.render_widget(
            Paragraph::new(texts::tui_provider_not_found()).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Plain)
                    .border_style(pane_border_style(app, Focus::Content, theme))
                    .title(texts::tui_provider_title()),
            ),
            area,
        );
        return;
    };

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(texts::tui_provider_detail_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    if app.focus == Focus::Content {
        let mut keys = if matches!(
            app.app_type,
            crate::app_config::AppType::OpenCode | crate::app_config::AppType::OpenClaw
        ) {
            let keys = vec![
                ("s", texts::tui_key_add_remove()),
                ("e", texts::tui_key_edit()),
            ];
            keys
        } else {
            let keys = vec![("s", texts::tui_key_switch()), ("e", texts::tui_key_edit())];
            keys
        };
        if data::quota_target_for_provider(&app.app_type, row).is_some() {
            keys.push(("r", texts::tui_key_refresh()));
        }
        keys.push(("t", texts::tui_key_speedtest()));
        if matches!(app.app_type, crate::app_config::AppType::OpenClaw) && row.is_in_config {
            keys.push(("x", texts::tui_key_set_default()));
        } else if matches!(app.app_type, crate::app_config::AppType::OpenCode) {
            keys.push(("c", texts::tui_key_stream_check()));
        } else if !matches!(
            app.app_type,
            crate::app_config::AppType::OpenCode | crate::app_config::AppType::OpenClaw
        ) {
            if matches!(
                app.app_type,
                crate::app_config::AppType::Claude | crate::app_config::AppType::Codex
            ) {
                keys.push(("o", texts::tui_key_launch_temp()));
            }
            keys.push(("c", texts::tui_key_stream_check()));
        }
        render_key_bar_center(frame, chunks[0], theme, &keys);
    }

    let mut lines = vec![
        Line::from(vec![
            Span::styled(texts::tui_label_id(), Style::default().fg(theme.accent)),
            Span::raw(": "),
            Span::raw(row.id.clone()),
        ]),
        Line::from(vec![
            Span::styled(texts::header_name(), Style::default().fg(theme.accent)),
            Span::raw(": "),
            Span::raw(data::provider_display_name(&app.app_type, row)),
        ]),
        Line::raw(""),
    ];

    if let Some(url) = row.api_url.as_deref() {
        lines.push(Line::from(vec![
            Span::styled(
                texts::tui_label_api_url(),
                Style::default().fg(theme.accent),
            ),
            Span::raw(": "),
            Span::raw(url),
        ]));
    }

    if matches!(app.app_type, crate::app_config::AppType::OpenClaw) {
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled(
                texts::tui_label_openclaw_status(),
                Style::default().fg(theme.accent),
            ),
            Span::raw(": "),
            Span::raw(openclaw_status_label(row)),
        ]));
        if let Some(model_id) = row
            .default_model_id
            .as_deref()
            .or(row.primary_model_id.as_deref())
        {
            lines.push(Line::from(vec![
                Span::styled(
                    texts::tui_label_openclaw_model(),
                    Style::default().fg(theme.accent),
                ),
                Span::raw(": "),
                Span::raw(model_id),
            ]));
        }
    }

    if matches!(app.app_type, crate::app_config::AppType::OpenCode) {
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled(
                texts::tui_label_provider_config_status(),
                Style::default().fg(theme.accent),
            ),
            Span::raw(": "),
            Span::raw(opencode_status_label(row)),
        ]));
    }

    if matches!(app.app_type, crate::app_config::AppType::Claude) {
        lines.push(Line::from(vec![
            Span::styled(
                texts::tui_label_claude_hide_attribution(),
                Style::default().fg(theme.accent),
            ),
            Span::raw(": "),
            Span::raw(
                if crate::cli::tui::form::claude_hide_attribution_enabled(
                    &row.provider.settings_config,
                ) {
                    texts::tui_marker_active()
                } else {
                    texts::tui_marker_inactive()
                },
            ),
        ]));

        if let Some(env) = row
            .provider
            .settings_config
            .get("env")
            .and_then(|v| v.as_object())
        {
            let api_key = env
                .get("ANTHROPIC_AUTH_TOKEN")
                .or_else(|| env.get("ANTHROPIC_API_KEY"))
                .and_then(|v| v.as_str())
                .map(mask_api_key)
                .unwrap_or_else(|| texts::tui_na().to_string());
            let base_url = env
                .get("ANTHROPIC_BASE_URL")
                .and_then(|v| v.as_str())
                .unwrap_or(texts::tui_na());

            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                Span::styled(
                    texts::tui_label_base_url(),
                    Style::default().fg(theme.accent),
                ),
                Span::raw(": "),
                Span::raw(base_url),
            ]));
            let api_format = crate::proxy::providers::get_claude_api_format(&row.provider);

            lines.push(Line::from(vec![
                Span::styled(
                    texts::tui_label_claude_api_format(),
                    Style::default().fg(theme.accent),
                ),
                Span::raw(": "),
                Span::raw(texts::tui_claude_api_format_value(api_format)),
            ]));
            lines.push(Line::from(vec![
                Span::styled(
                    texts::tui_label_api_key(),
                    Style::default().fg(theme.accent),
                ),
                Span::raw(": "),
                Span::raw(api_key),
            ]));
        }
    }

    lines.extend(quota_detail_lines(app, data, row, theme));

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::NONE))
            .wrap(Wrap { trim: false }),
        inset_left(chunks[1], CONTENT_INSET_LEFT),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::AppType;
    use crate::provider::Provider;
    use crate::services::{CredentialStatus, QuotaTier, SubscriptionQuota};
    use ratatui::buffer::Buffer;
    use serde_json::json;

    fn all_text(buf: &Buffer) -> String {
        let mut all = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                all.push_str(buf[(x, y)].symbol());
            }
            all.push('\n');
        }
        all
    }

    fn current_official_claude_data() -> UiData {
        let mut data = super::super::tests::minimal_data(&AppType::Claude);
        let mut provider = Provider::with_id(
            "official".to_string(),
            "Claude Official".to_string(),
            json!({"env": {}}),
            None,
        );
        provider.category = Some("official".to_string());
        data.providers.current_id = "official".to_string();
        data.providers.rows = vec![ProviderRow {
            id: "official".to_string(),
            provider,
            api_url: None,
            is_current: true,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: None,
            default_model_id: None,
        }];

        let target =
            data::quota_target_for_provider(&AppType::Claude, &data.providers.rows[0]).unwrap();
        data.quota.finish(
            target,
            SubscriptionQuota {
                tool: "claude".to_string(),
                credential_status: CredentialStatus::Valid,
                credential_message: None,
                success: true,
                tiers: vec![
                    QuotaTier {
                        name: "five_hour".to_string(),
                        utilization: 42.0,
                        resets_at: None,
                    },
                    QuotaTier {
                        name: "seven_day".to_string(),
                        utilization: 70.0,
                        resets_at: None,
                    },
                ],
                extra_usage: None,
                error: None,
                queried_at: Some(chrono::Utc::now().timestamp_millis()),
            },
        );
        data
    }

    #[test]
    fn claude_provider_list_key_bar_shows_launch_temp_hint() {
        let _lock = super::super::tests::lock_env();
        let _no_color = super::super::tests::EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = super::super::tests::minimal_data(&app.app_type);
        let all = all_text(&super::super::tests::render(&app, &data));

        assert!(
            all.contains(&format!("o {}", texts::tui_key_launch_temp())),
            "{all}"
        );
    }

    #[test]
    fn codex_provider_list_key_bar_shows_launch_temp_hint() {
        let _lock = super::super::tests::lock_env();
        let _no_color = super::super::tests::EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = super::super::tests::minimal_data(&app.app_type);
        let all = all_text(&super::super::tests::render(&app, &data));

        assert!(
            all.contains(&format!("o {}", texts::tui_key_launch_temp())),
            "{all}"
        );
    }

    #[test]
    fn claude_provider_detail_key_bar_shows_launch_temp_hint() {
        let _lock = super::super::tests::lock_env();
        let _no_color = super::super::tests::EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ProviderDetail {
            id: "p1".to_string(),
        };
        app.focus = Focus::Content;

        let data = super::super::tests::minimal_data(&app.app_type);
        let all = all_text(&super::super::tests::render(&app, &data));

        assert!(
            all.contains(&format!("o {}", texts::tui_key_launch_temp())),
            "{all}"
        );
    }

    #[test]
    fn codex_provider_detail_key_bar_shows_launch_temp_hint() {
        let _lock = super::super::tests::lock_env();
        let _no_color = super::super::tests::EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::ProviderDetail {
            id: "p1".to_string(),
        };
        app.focus = Focus::Content;

        let data = super::super::tests::minimal_data(&app.app_type);
        let all = all_text(&super::super::tests::render(&app, &data));

        assert!(
            all.contains(&format!("o {}", texts::tui_key_launch_temp())),
            "{all}"
        );
    }

    #[test]
    fn official_provider_list_shows_inline_quota_and_refresh_hint() {
        let _lock = super::super::tests::lock_env();
        let _no_color = super::super::tests::EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        let data = current_official_claude_data();
        let all = all_text(&super::super::tests::render(&app, &data));

        assert!(!all.contains(texts::tui_header_quota()), "{all}");
        assert!(
            all.contains(&format!("r {}", texts::tui_key_refresh())),
            "{all}"
        );
        assert!(all.contains("Claude Official"), "{all}");
        assert!(all.contains("5h 42%"), "{all}");
        assert!(all.contains("s ago"), "{all}");
    }

    #[test]
    fn provider_list_shows_selected_non_current_quota_result() {
        let _lock = super::super::tests::lock_env();
        let _no_color = super::super::tests::EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.provider_idx = 1;
        let mut data = current_official_claude_data();
        data.providers.current_id = "custom".to_string();
        data.providers.rows[0].is_current = false;
        data.providers.rows.insert(
            0,
            ProviderRow {
                id: "custom".to_string(),
                provider: Provider::with_id(
                    "custom".to_string(),
                    "Custom".to_string(),
                    json!({"env": {"ANTHROPIC_BASE_URL": "https://api.example.com"}}),
                    None,
                ),
                api_url: Some("https://api.example.com".to_string()),
                is_current: true,
                is_in_config: true,
                is_saved: true,
                is_default_model: false,
                primary_model_id: None,
                default_model_id: None,
            },
        );
        let all = all_text(&super::super::tests::render(&app, &data));

        assert!(
            all.contains(&format!("r {}", texts::tui_key_refresh())),
            "{all}"
        );
        assert!(!all.contains(texts::tui_header_quota()), "{all}");
        assert!(all.contains("5h 42%"), "{all}");
        assert!(all.contains("s ago"), "{all}");
    }

    #[test]
    fn official_provider_detail_shows_quota_details() {
        let _lock = super::super::tests::lock_env();
        let _no_color = super::super::tests::EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ProviderDetail {
            id: "official".to_string(),
        };
        app.focus = Focus::Content;
        let data = current_official_claude_data();
        let all = all_text(&super::super::tests::render(&app, &data));

        assert!(all.contains(texts::tui_label_quota()), "{all}");
        assert!(
            all.contains(&format!("r {}", texts::tui_key_refresh())),
            "{all}"
        );
        assert!(all.contains("5h: 42%"), "{all}");
        assert!(all.contains("7d: 70%"), "{all}");
    }

    #[cfg(not(unix))]
    #[test]
    fn claude_provider_list_key_bar_hides_launch_temp_hint_on_non_unix() {
        let _lock = super::super::tests::lock_env();
        let _no_color = super::super::tests::EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = super::super::tests::minimal_data(&app.app_type);
        let all = all_text(&super::super::tests::render(&app, &data));

        assert!(
            !all.contains(&format!("o {}", texts::tui_key_launch_temp())),
            "{all}"
        );
    }

    #[cfg(not(unix))]
    #[test]
    fn codex_provider_detail_key_bar_hides_launch_temp_hint_on_non_unix() {
        let _lock = super::super::tests::lock_env();
        let _no_color = super::super::tests::EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::ProviderDetail {
            id: "p1".to_string(),
        };
        app.focus = Focus::Content;

        let data = super::super::tests::minimal_data(&app.app_type);
        let all = all_text(&super::super::tests::render(&app, &data));

        assert!(
            !all.contains(&format!("o {}", texts::tui_key_launch_temp())),
            "{all}"
        );
    }
}
