use crate::cli::tui::app::failover_queue_position;
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

fn additive_status_label(app_type: &AppType, row: &ProviderRow) -> &'static str {
    if matches!(app_type, AppType::Hermes) && row.is_default_model {
        texts::tui_provider_status_in_use()
    } else if row.is_default_model {
        texts::tui_openclaw_status_default()
    } else {
        opencode_status_label(row)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderProxyBadge {
    NeedsProxy,
    NoProxySupport,
}

impl ProviderProxyBadge {
    fn label(self) -> &'static str {
        match self {
            Self::NeedsProxy => texts::tui_provider_needs_proxy_label(),
            Self::NoProxySupport => texts::tui_provider_no_proxy_support_label(),
        }
    }
}

fn provider_category_is(row: &ProviderRow, category: &str) -> bool {
    row.provider
        .category
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case(category))
}

fn provider_proxy_badge(app_type: &AppType, row: &ProviderRow) -> Option<ProviderProxyBadge> {
    match app_type {
        AppType::Claude if provider_category_is(row, "official") => {
            Some(ProviderProxyBadge::NoProxySupport)
        }
        AppType::Claude => {
            let api_format = crate::proxy::providers::get_provider_api_format(&row.provider);
            crate::proxy::providers::api_format_needs_transform(api_format)
                .then_some(ProviderProxyBadge::NeedsProxy)
        }
        AppType::Codex if provider_category_is(row, "official") => {
            Some(ProviderProxyBadge::NoProxySupport)
        }
        AppType::Codex => {
            crate::proxy::providers::codex_provider_uses_chat_completions(&row.provider)
                .then_some(ProviderProxyBadge::NeedsProxy)
        }
        _ => None,
    }
}

fn provider_proxy_badge_style(badge: ProviderProxyBadge, theme: &super::theme::Theme) -> Style {
    if theme.no_color {
        return match badge {
            ProviderProxyBadge::NeedsProxy => Style::default().add_modifier(Modifier::BOLD),
            ProviderProxyBadge::NoProxySupport => Style::default(),
        };
    }

    match badge {
        ProviderProxyBadge::NeedsProxy => Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
        ProviderProxyBadge::NoProxySupport => Style::default().fg(theme.comment),
    }
}

fn provider_switch_key_label(app_type: &AppType) -> &'static str {
    if app_type.is_additive_mode() {
        texts::tui_key_add_remove()
    } else {
        texts::tui_key_switch()
    }
}

fn provider_default_key_label(app_type: &AppType) -> &'static str {
    if matches!(app_type, AppType::Hermes) {
        texts::tui_key_enable()
    } else {
        texts::tui_key_set_default()
    }
}

fn failover_queue_label(data: &UiData, provider_id: &str) -> String {
    failover_queue_position(data, provider_id)
        .map(|position| format!("#{position}"))
        .unwrap_or_default()
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
    if let Some(badge) = provider_proxy_badge(&app.app_type, row) {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("[{}]", badge.label()),
            provider_proxy_badge_style(badge, theme),
        ));
    }
    if show_quota {
        if let Some(quota) = quota_compact_line(data.quota.state_for(&row.id), theme, true) {
            spans.push(Span::styled("  (", Style::default().fg(theme.comment)));
            spans.extend(quota.spans);
            spans.push(Span::styled(")", Style::default().fg(theme.comment)));
        }
    }
    Line::from(spans)
}

fn render_provider_empty_state(frame: &mut Frame<'_>, area: Rect, theme: &super::theme::Theme) {
    let title_style = Style::default().add_modifier(Modifier::BOLD);
    let subtitle_style = Style::default().fg(theme.comment);
    let primary_style = if theme.no_color {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::White)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    };
    let secondary_style = if theme.no_color {
        Style::default()
    } else {
        Style::default()
            .fg(theme.dim)
            .bg(theme.surface)
            .add_modifier(Modifier::BOLD)
    };

    let content_lines = vec![
        Line::styled(texts::tui_provider_empty_title(), title_style),
        Line::raw(""),
        Line::styled(texts::tui_provider_empty_subtitle(), subtitle_style),
        Line::raw(""),
        Line::from(vec![Span::styled(
            format!("  Enter  {}  ", texts::tui_key_import_current_config()),
            primary_style,
        )]),
        Line::from(vec![Span::styled(
            format!("  a  {}  ", texts::tui_key_add_provider()),
            secondary_style,
        )]),
    ];

    let top_padding = area.height.saturating_sub(content_lines.len() as u16) / 2;
    let mut lines = Vec::with_capacity(top_padding as usize + content_lines.len());
    for _ in 0..top_padding {
        lines.push(Line::raw(""));
    }
    lines.extend(content_lines);

    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false }),
        area,
    );
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
        let mut keys = Vec::new();
        if data.providers.rows.is_empty() {
            keys.push(("Enter", texts::tui_key_import_current_config()));
            keys.push(("a", texts::tui_key_add_provider()));
        } else if visible.is_empty() {
            keys.push(("a", texts::tui_key_add()));
        } else {
            keys.push(("Enter", texts::tui_key_details()));
            keys.push(("Space", provider_switch_key_label(&app.app_type)));
            keys.push(("a", texts::tui_key_add()));
            // Theoretically we should use "duplicate" here.
            // However this word is too long to fit in the key hint area,
            // and key "d" is already used for delete, so we use "copy" here to make it shorter and avoid confusion.
            keys.push(("c", texts::tui_key_copy()));
            if let Some(row) = visible.get(app.provider_idx) {
                if !data::provider_is_read_only(&app.app_type, row) {
                    keys.push(("e", texts::tui_key_edit()));
                    keys.push(("d", texts::tui_key_delete()));
                }
            }
            keys.push(("t", texts::tui_key_test()));
            if selected_supports_quota {
                keys.push(("r", texts::tui_key_refresh()));
            }
            if crate::cli::tui::app::supports_temporary_provider_launch(&app.app_type) {
                keys.push(("o", texts::tui_key_launch_temp()));
            }
            if crate::cli::tui::app::supports_failover_controls(&app.app_type) {
                keys.push(("f", texts::tui_key_failover()));
            }
            if let Some(row) = visible.get(app.provider_idx) {
                if matches!(app.app_type, AppType::OpenClaw | AppType::Hermes) && row.is_in_config {
                    keys.push(("x", provider_default_key_label(&app.app_type)));
                }
            }
        }
        render_key_bar_center(frame, chunks[0], theme, &keys);
    }

    if data.providers.rows.is_empty() {
        render_provider_empty_state(frame, chunks[1], theme);
        return;
    }

    let failover_supported = crate::cli::tui::app::supports_failover_controls(&app.app_type);
    let header_cells = vec![
        Cell::from(""),
        Cell::from(texts::header_name()),
        Cell::from(texts::tui_header_api_url()),
    ];
    let header = Row::new(header_cells).style(header_style);

    let rows = visible.iter().enumerate().map(|(idx, row)| {
        let marker = if failover_supported && data.proxy.auto_failover_enabled {
            failover_queue_label(data, &row.id)
        } else if matches!(app.app_type, AppType::OpenClaw | AppType::Hermes) {
            if row.is_default_model {
                "*".to_string()
            } else if row.is_in_config {
                "+".to_string()
            } else {
                String::new()
            }
        } else if matches!(app.app_type, AppType::OpenCode) {
            if row.is_in_config {
                "+".to_string()
            } else {
                String::new()
            }
        } else if row.is_current {
            texts::tui_marker_active().to_string()
        } else {
            texts::tui_marker_inactive().to_string()
        };
        let api = row.api_url.as_deref().unwrap_or(texts::tui_na());
        let show_quota = row.is_current || idx == app.provider_idx;
        let cells = vec![
            Cell::from(marker),
            Cell::from(provider_name_with_quota_line(
                app, data, row, show_quota, theme,
            )),
            Cell::from(api),
        ];
        Row::new(cells)
    });

    let constraints = vec![
        Constraint::Length(3),
        Constraint::Percentage(44),
        Constraint::Percentage(46),
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
        let mut keys = vec![("Space", provider_switch_key_label(&app.app_type))];
        if !data::provider_is_read_only(&app.app_type, row) {
            keys.push(("e", texts::tui_key_edit()));
        }
        keys.push(("t", texts::tui_key_test()));
        if data::quota_target_for_provider(&app.app_type, row).is_some() {
            keys.push(("r", texts::tui_key_refresh()));
        }
        if matches!(app.app_type, AppType::OpenClaw | AppType::Hermes) && row.is_in_config {
            keys.push(("x", provider_default_key_label(&app.app_type)));
        }
        if crate::cli::tui::app::supports_temporary_provider_launch(&app.app_type) {
            keys.push(("o", texts::tui_key_launch_temp()));
        }
        if crate::cli::tui::app::supports_failover_controls(&app.app_type) {
            keys.push(("f", texts::tui_key_failover()));
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
    ];

    if let Some(badge) = provider_proxy_badge(&app.app_type, row) {
        lines.push(Line::from(vec![
            Span::styled(
                texts::tui_label_provider_proxy(),
                Style::default().fg(theme.accent),
            ),
            Span::raw(": "),
            Span::styled(badge.label(), provider_proxy_badge_style(badge, theme)),
        ]));
    }

    lines.push(Line::raw(""));

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

    if matches!(app.app_type, AppType::OpenCode | AppType::Hermes) {
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled(
                texts::tui_label_provider_config_status(),
                Style::default().fg(theme.accent),
            ),
            Span::raw(": "),
            Span::raw(additive_status_label(&app.app_type, row)),
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
            let api_format = crate::proxy::providers::get_provider_api_format(&row.provider);

            lines.push(Line::from(vec![
                Span::styled(
                    texts::tui_label_api_format(),
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

    if crate::cli::tui::app::supports_failover_controls(&app.app_type) {
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled(
                crate::t!("Failover queue", "故障转移队列"),
                Style::default().fg(theme.accent),
            ),
            Span::raw(": "),
            Span::raw(failover_queue_position(data, &row.id).map_or_else(
                || crate::t!("Not in queue", "未加入队列").to_string(),
                |position| format!("{} #{position}", crate::t!("Queued", "已加入")),
            )),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                crate::t!("Queue ordering", "队列排序"),
                Style::default().fg(theme.accent),
            ),
            Span::raw(": "),
            Span::raw(crate::t!(
                "Uses provider order; moving queue entries can also reorder providers.",
                "使用供应商顺序；移动队列项也可能重排供应商。"
            )),
        ]));
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
    use crate::cli::tui::data::ProviderUsageQuota;
    use crate::provider::{Provider, ProviderMeta, UsageData, UsageResult, UsageScript};
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
            ProviderUsageQuota::Subscription(SubscriptionQuota {
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
            }),
        );
        data
    }

    fn usage_script_data() -> UiData {
        let mut data = super::super::tests::minimal_data(&AppType::Claude);
        let mut provider = Provider::with_id(
            "usage-provider".to_string(),
            "Usage Provider".to_string(),
            json!({"env": {"ANTHROPIC_BASE_URL": "https://api.example.com"}}),
            None,
        );
        provider.meta = Some(ProviderMeta {
            usage_script: Some(UsageScript {
                enabled: true,
                language: "javascript".to_string(),
                code: "return { planName: 'default', remaining: 12, unit: 'USD' }".to_string(),
                timeout: Some(10),
                api_key: None,
                base_url: None,
                access_token: None,
                user_id: None,
                template_type: Some("general".to_string()),
                auto_query_interval: Some(5),
                coding_plan_provider: None,
            }),
            ..Default::default()
        });
        data.providers.current_id = "usage-provider".to_string();
        data.providers.rows = vec![ProviderRow {
            id: "usage-provider".to_string(),
            provider,
            api_url: Some("https://api.example.com".to_string()),
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
            ProviderUsageQuota::Script(UsageResult {
                success: true,
                data: Some(vec![UsageData {
                    plan_name: Some("default".to_string()),
                    extra: None,
                    is_valid: Some(true),
                    invalid_message: None,
                    total: None,
                    used: None,
                    remaining: Some(12.0),
                    unit: Some("USD".to_string()),
                }]),
                error: None,
            }),
        );
        data
    }

    fn claude_openai_chat_data() -> UiData {
        let mut data = super::super::tests::minimal_data(&AppType::Claude);
        data.providers.rows[0].provider = Provider::with_id(
            "p1".to_string(),
            "OpenAI Format Provider".to_string(),
            json!({"env": {"ANTHROPIC_BASE_URL": "https://api.example.com"}}),
            None,
        );
        data.providers.rows[0].provider.meta = Some(ProviderMeta {
            api_format: Some("openai_chat".to_string()),
            ..Default::default()
        });
        data
    }

    fn codex_chat_wire_api_data() -> UiData {
        let mut data = super::super::tests::minimal_data(&AppType::Codex);
        data.providers.rows[0].provider = Provider::with_id(
            "p1".to_string(),
            "Chat Wire Provider".to_string(),
            json!({
                "config": "model_provider = \"custom\"\nmodel = \"model\"\n\n[model_providers.custom]\nbase_url = \"https://api.example.com/v1\"\nwire_api = \"chat\"\nrequires_openai_auth = true\n"
            }),
            None,
        );
        data
    }

    #[test]
    fn claude_provider_list_marks_openai_format_as_needing_proxy() {
        let _lock = super::super::tests::lock_env();
        let _no_color = super::super::tests::EnvGuard::remove("NO_COLOR");
        let _lang = crate::cli::i18n::use_test_language(crate::cli::i18n::Language::English);

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        let data = claude_openai_chat_data();
        let all = all_text(&super::super::tests::render_with_size(&app, &data, 180, 40));

        assert!(all.contains("OpenAI Format Provider"), "{all}");
        assert!(all.contains("[Needs Proxy]"), "{all}");
        assert!(!all.contains("No Proxy Support"), "{all}");
    }

    #[test]
    fn claude_provider_detail_marks_official_as_no_proxy_support() {
        let _lock = super::super::tests::lock_env();
        let _no_color = super::super::tests::EnvGuard::remove("NO_COLOR");
        let _lang = crate::cli::i18n::use_test_language(crate::cli::i18n::Language::English);

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ProviderDetail {
            id: "official".to_string(),
        };
        app.focus = Focus::Content;
        let data = current_official_claude_data();
        let all = all_text(&super::super::tests::render_with_size(&app, &data, 180, 40));

        assert!(all.contains("Proxy: No Proxy Support"), "{all}");
    }

    #[test]
    fn codex_provider_list_marks_chat_wire_api_as_needing_proxy() {
        let _lock = super::super::tests::lock_env();
        let _no_color = super::super::tests::EnvGuard::remove("NO_COLOR");
        let _lang = crate::cli::i18n::use_test_language(crate::cli::i18n::Language::English);

        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        let data = codex_chat_wire_api_data();
        let all = all_text(&super::super::tests::render_with_size(&app, &data, 180, 40));

        assert!(all.contains("Chat Wire Provider"), "{all}");
        assert!(all.contains("[Needs Proxy]"), "{all}");
    }

    #[test]
    fn provider_proxy_badge_uses_chinese_text() {
        let _lock = super::super::tests::lock_env();
        let _no_color = super::super::tests::EnvGuard::remove("NO_COLOR");
        let _lang = crate::cli::i18n::use_test_language(crate::cli::i18n::Language::Chinese);

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        let data = claude_openai_chat_data();
        let all = all_text(&super::super::tests::render_with_size(&app, &data, 180, 40));
        let compact = all.replace(' ', "");

        assert!(compact.contains("[需要代理]"), "{all}");
        assert!(!all.contains("Needs Proxy"), "{all}");
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
        let all = all_text(&super::super::tests::render_with_size(&app, &data, 180, 40));

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
        let all = all_text(&super::super::tests::render_with_size(&app, &data, 180, 40));

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

    #[test]
    fn usage_script_quota_hides_default_plan_name_and_shows_checked_time() {
        let _lock = super::super::tests::lock_env();
        let _no_color = super::super::tests::EnvGuard::remove("NO_COLOR");
        let _lang = crate::cli::i18n::use_test_language(crate::cli::i18n::Language::English);

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        let data = usage_script_data();
        let all = all_text(&super::super::tests::render_with_size(&app, &data, 180, 40));

        assert!(all.contains("Usage Provider"), "{all}");
        assert!(all.contains("12 USD"), "{all}");
        assert!(all.contains("second ago"), "{all}");
        assert!(!all.contains("default"), "{all}");
    }

    #[test]
    fn usage_script_quota_uses_chinese_checked_time() {
        let _lock = super::super::tests::lock_env();
        let _no_color = super::super::tests::EnvGuard::remove("NO_COLOR");
        let _lang = crate::cli::i18n::use_test_language(crate::cli::i18n::Language::Chinese);

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ProviderDetail {
            id: "usage-provider".to_string(),
        };
        app.focus = Focus::Content;
        let data = usage_script_data();
        let all = all_text(&super::super::tests::render_with_size(&app, &data, 180, 40));

        assert!(all.contains("12 USD"), "{all}");
        assert!(all.contains("秒 前"), "{all}");
        assert!(!all.contains("default"), "{all}");
    }

    #[test]
    fn usage_script_detail_empty_data_shows_not_available_once() {
        let _lock = super::super::tests::lock_env();
        let _no_color = super::super::tests::EnvGuard::remove("NO_COLOR");
        let _lang = crate::cli::i18n::use_test_language(crate::cli::i18n::Language::English);

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ProviderDetail {
            id: "usage-provider".to_string(),
        };
        app.focus = Focus::Content;
        let mut data = usage_script_data();
        let target =
            data::quota_target_for_provider(&AppType::Claude, &data.providers.rows[0]).unwrap();
        data.quota.finish(
            target,
            ProviderUsageQuota::Script(UsageResult {
                success: true,
                data: Some(Vec::new()),
                error: None,
            }),
        );

        let all = all_text(&super::super::tests::render_with_size(&app, &data, 180, 40));

        assert!(all.contains(texts::tui_quota_not_available()), "{all}");
        assert!(!all.contains(texts::tui_quota_ok()), "{all}");
        assert_eq!(all.matches(texts::tui_label_quota()).count(), 1, "{all}");
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
