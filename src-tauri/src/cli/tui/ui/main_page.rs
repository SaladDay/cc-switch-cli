use crate::cli::tui::data;

use super::*;

fn opencode_configured_provider_count(data: &UiData) -> usize {
    data.providers
        .rows
        .iter()
        .filter(|row| row.is_in_config)
        .count()
}

fn main_provider_status(app: &App, data: &UiData) -> String {
    if matches!(app.app_type, AppType::OpenCode) {
        return texts::tui_provider_config_count(
            opencode_configured_provider_count(data),
            data.providers.rows.len(),
        );
    }

    data.providers
        .rows
        .iter()
        .find(|p| p.is_current)
        .map(|row| data::provider_display_name(&app.app_type, row))
        .unwrap_or_else(|| texts::none().to_string())
}

fn main_api_url(app: &App, data: &UiData) -> String {
    let api_url = if matches!(app.app_type, AppType::OpenCode) {
        data.providers
            .rows
            .iter()
            .find(|p| p.is_in_config)
            .and_then(|p| p.api_url.as_deref())
    } else {
        data.providers
            .rows
            .iter()
            .find(|p| p.is_current)
            .and_then(|p| p.api_url.as_deref())
    };

    api_url.unwrap_or(texts::tui_na()).to_string()
}

pub(super) fn render_main(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let current_provider = main_provider_status(app, data);

    let mcp_enabled = data
        .mcp
        .rows
        .iter()
        .filter(|s| s.server.apps.is_enabled_for(&app.app_type))
        .count();
    let skills_enabled = data
        .skills
        .installed
        .iter()
        .filter(|skill| skill.apps.is_enabled_for(&app.app_type))
        .count();

    let api_url = main_api_url(app, data);

    let label_width = 14;
    let value_style = Style::default().fg(theme.cyan);
    let provider_name_style = if theme.no_color {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    };

    let proxy_running = data.proxy.running;
    let current_app_routed = data
        .proxy
        .routes_current_app_through_proxy(&app.app_type)
        .unwrap_or(false);
    let uptime_text = if proxy_running {
        format_uptime_compact(data.proxy.uptime_seconds)
    } else {
        texts::tui_proxy_dashboard_uptime_stopped().to_string()
    };
    let proxy_last_error_text = data
        .proxy
        .last_error
        .clone()
        .unwrap_or_else(|| texts::none().to_string());
    let current_quota_line = data
        .providers
        .rows
        .iter()
        .find(|row| row.is_current)
        .filter(|row| data::quota_target_for_provider(&app.app_type, row).is_some())
        .and_then(|row| quota_compact_line(data.quota.state_for(&row.id), theme, true));

    let mut connection_lines = vec![
        kv_line(
            theme,
            texts::provider_label(),
            label_width,
            vec![
                Span::styled(current_provider.clone(), provider_name_style),
                // Do not claim a connection state until a real health check has run.
                Span::raw("   "),
                Span::styled(
                    format!("{} ", texts::tui_label_mcp_short()),
                    Style::default()
                        .fg(theme.comment)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        "[{}/{} {}]",
                        mcp_enabled,
                        data.mcp.rows.len(),
                        texts::tui_label_mcp_servers_active()
                    ),
                    value_style,
                ),
                Span::raw("   "),
                Span::styled(
                    format!("{} ", texts::tui_label_skills()),
                    Style::default()
                        .fg(theme.comment)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        "[{}/{} {}]",
                        skills_enabled,
                        data.skills.installed.len(),
                        texts::tui_label_mcp_servers_active()
                    ),
                    if data.skills.installed.is_empty() {
                        Style::default().fg(theme.surface)
                    } else {
                        value_style
                    },
                ),
            ],
        ),
        kv_line(
            theme,
            texts::tui_label_api_url(),
            label_width,
            vec![Span::styled(api_url, value_style)],
        ),
    ];
    if let Some(quota) = current_quota_line {
        connection_lines.push(kv_line(
            theme,
            texts::tui_label_quota(),
            label_width,
            quota.spans,
        ));
    }

    let webdav = data.config.webdav_sync.as_ref();
    let is_config_value_set = |value: &str| !value.trim().is_empty();
    let webdav_enabled = webdav.map(|cfg| cfg.enabled).unwrap_or(false);
    let is_configured = webdav
        .map(|cfg| {
            is_config_value_set(&cfg.base_url)
                && is_config_value_set(&cfg.username)
                && is_config_value_set(&cfg.password)
        })
        .unwrap_or(false);
    let webdav_status = webdav.map(|cfg| &cfg.status);
    let last_error = webdav_status
        .and_then(|status| status.last_error.as_deref())
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let has_error = webdav_enabled && is_configured && last_error.is_some();
    let is_ok = webdav_enabled
        && is_configured
        && !has_error
        && webdav_status
            .and_then(|status| status.last_sync_at)
            .is_some();

    let webdav_status_text = if !webdav_enabled || !is_configured {
        texts::tui_webdav_status_not_configured().to_string()
    } else if has_error {
        let detail = last_error
            .map(|err| truncate_to_display_width(err, 22))
            .unwrap_or_default();
        if detail.is_empty() {
            texts::tui_webdav_status_error().to_string()
        } else {
            texts::tui_webdav_status_error_with_detail(&detail)
        }
    } else if is_ok {
        texts::tui_webdav_status_ok().to_string()
    } else {
        texts::tui_webdav_status_configured().to_string()
    };

    let webdav_status_style = if theme.no_color {
        Style::default()
    } else if has_error {
        Style::default().fg(theme.warn)
    } else if is_ok {
        Style::default().fg(theme.ok)
    } else {
        Style::default().fg(theme.surface)
    };

    let last_sync_at = webdav_status.and_then(|status| status.last_sync_at);
    let webdav_last_sync_text = last_sync_at
        .and_then(format_sync_time_local_to_minute)
        .unwrap_or_else(|| texts::tui_webdav_status_never_synced().to_string());
    let webdav_last_sync_style = if last_sync_at.is_some() {
        value_style
    } else {
        Style::default().fg(theme.surface)
    };

    let webdav_lines = vec![
        kv_line(
            theme,
            texts::tui_label_webdav_status(),
            label_width,
            vec![Span::styled(
                webdav_status_text.clone(),
                webdav_status_style,
            )],
        ),
        kv_line(
            theme,
            texts::tui_label_webdav_last_sync(),
            label_width,
            vec![Span::styled(
                webdav_last_sync_text.clone(),
                webdav_last_sync_style,
            )],
        ),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(texts::welcome_title());
    frame.render_widget(block.clone(), area);

    let inner = block.inner(area);
    let content = inset_left(inner, CONTENT_INSET_LEFT);
    let bottom_hero_height = if current_app_routed { 11 } else { 7 };
    let connection_card_height = (connection_lines.len() as u16 + 2).max(4);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(bottom_hero_height)])
        .split(content);

    let top_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(connection_card_height),
            Constraint::Length(4),
            Constraint::Length(6),
            Constraint::Min(0),
        ])
        .split(chunks[0]);

    let card_border = Style::default().fg(theme.dim);
    render_connection_card(frame, top_chunks[1], theme, &connection_lines, card_border);
    render_webdav_card(frame, top_chunks[2], theme, &webdav_lines, card_border);
    render_local_env_check_card(frame, app, top_chunks[3], theme, card_border);

    let hero_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(chunks[1].height.saturating_sub(1)),
            Constraint::Length(1),
        ])
        .split(chunks[1]);

    if current_app_routed {
        render_proxy_activity_dashboard(
            frame,
            hero_chunks[0],
            theme,
            &app.proxy_input_activity_samples,
            &app.proxy_output_activity_samples,
            &uptime_text,
            &proxy_last_error_text,
            data.proxy.last_error.is_some(),
            &format!("{}:{}", data.proxy.listen_address, data.proxy.listen_port),
            data.proxy.estimated_input_tokens_total,
            data.proxy.estimated_output_tokens_total,
        );
    } else {
        render_logo_hero(frame, hero_chunks[0], theme);
    }

    frame.render_widget(
        Paragraph::new(Line::raw(texts::tui_main_hint()))
            .alignment(Alignment::Center)
            .style(
                Style::default()
                    .fg(theme.surface)
                    .add_modifier(Modifier::ITALIC),
            ),
        hero_chunks[1],
    );
}

fn render_proxy_activity_dashboard(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &super::theme::Theme,
    input_activity_samples: &[u64],
    output_activity_samples: &[u64],
    uptime_text: &str,
    proxy_last_error_text: &str,
    has_proxy_error: bool,
    listen_text: &str,
    input_tokens_total: u64,
    output_tokens_total: u64,
) -> Rect {
    let has_token_traffic = input_tokens_total > 0 || output_tokens_total > 0;
    let title_output_style = if has_token_traffic {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.surface)
    };
    let title_input_style = if has_token_traffic {
        Style::default().fg(theme.cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.surface)
    };
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.accent))
        .title(Line::from(vec![
            Span::raw(format!(" {}   ", texts::tui_home_section_proxy())),
            Span::styled(
                format!("▲ {}", format_estimated_token_compact(output_tokens_total)),
                title_output_style,
            ),
            Span::styled(" / ", Style::default().fg(theme.comment)),
            Span::styled(
                format!("▼ {}", format_estimated_token_compact(input_tokens_total)),
                title_input_style,
            ),
            Span::raw(" "),
        ]));
    frame.render_widget(outer.clone(), area);

    let inner = outer.inner(area);
    let label_style = Style::default()
        .fg(theme.comment)
        .add_modifier(Modifier::BOLD);
    let mut meta_spans = Vec::new();
    let mut meta_plain = String::new();
    let mut push_segment = |label: &'static str, value: &str, style: Style| {
        if !meta_spans.is_empty() {
            meta_spans.push(Span::raw("  "));
            meta_plain.push_str("  ");
        }
        meta_spans.push(Span::styled(format!("{label}: "), label_style));
        meta_spans.push(Span::styled(value.to_string(), style));
        meta_plain.push_str(label);
        meta_plain.push_str(": ");
        meta_plain.push_str(value);
    };

    push_segment(
        texts::tui_label_listen(),
        listen_text,
        Style::default().fg(theme.cyan),
    );
    push_segment(
        texts::tui_label_uptime(),
        uptime_text,
        Style::default().fg(theme.cyan),
    );
    if has_proxy_error {
        push_segment(
            texts::tui_label_last_proxy_error(),
            proxy_last_error_text,
            Style::default().fg(theme.warn),
        );
    }

    let max_text_height = inner.height.saturating_sub(2).clamp(1, 4);
    let text_height = wrapped_display_line_count(&meta_plain, inner.width).min(max_text_height);
    let graph_height = inner.height.saturating_sub(text_height).max(2);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(text_height),
            Constraint::Length(graph_height),
            Constraint::Min(0),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(meta_spans)).wrap(Wrap { trim: false }),
        sections[0],
    );

    let upper_height = (graph_height / 2).max(1);
    let lower_height = graph_height.saturating_sub(upper_height).max(1);
    let wave_width = sections[1].width.saturating_sub(1);
    let mut graph_lines = Vec::new();
    let upper_style = Style::default().fg(theme.accent);
    let lower_style = if theme.no_color {
        Style::default()
    } else {
        Style::default().fg(theme.cyan)
    };

    graph_lines.extend(
        proxy_wave_lines(
            wave_width,
            upper_height,
            true,
            output_activity_samples,
            &DOTS,
            false,
        )
        .into_iter()
        .map(|row| Line::from(vec![Span::raw(" "), Span::styled(row, upper_style)])),
    );
    graph_lines.extend(
        proxy_wave_lines(
            wave_width,
            lower_height,
            true,
            input_activity_samples,
            &REV_DOTS,
            true,
        )
        .into_iter()
        .map(|row| Line::from(vec![Span::raw(" "), Span::styled(row, lower_style)])),
    );

    frame.render_widget(
        Paragraph::new(graph_lines).wrap(Wrap { trim: false }),
        sections[1],
    );

    inner
}

fn wrapped_display_line_count(text: &str, width: u16) -> u16 {
    if width == 0 {
        return 1;
    }

    UnicodeWidthStr::width(text).max(1).div_ceil(width as usize) as u16
}

fn render_logo_hero(frame: &mut Frame<'_>, area: Rect, theme: &super::theme::Theme) {
    let logo_lines = logo_hero_lines(theme);
    let logo_height = (logo_lines.len() as u16).min(area.height);
    let logo_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(logo_height),
            Constraint::Min(0),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(logo_lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false }),
        logo_chunks[1],
    );
}

fn logo_hero_lines(theme: &super::theme::Theme) -> Vec<Line<'static>> {
    let logo_style = Style::default().fg(theme.surface);
    texts::tui_home_ascii_logo()
        .lines()
        .map(|s| Line::from(Span::styled(s.to_string(), logo_style)))
        .collect::<Vec<_>>()
}

fn render_connection_card(
    frame: &mut Frame<'_>,
    area: Rect,
    _theme: &super::theme::Theme,
    connection_lines: &[Line<'_>],
    card_border: Style,
) {
    frame.render_widget(
        Paragraph::new(connection_lines.to_vec())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Plain)
                    .border_style(card_border)
                    .title(format!(" {} ", texts::tui_home_section_connection())),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_webdav_card(
    frame: &mut Frame<'_>,
    area: Rect,
    _theme: &super::theme::Theme,
    webdav_lines: &[Line<'_>],
    card_border: Style,
) {
    frame.render_widget(
        Paragraph::new(webdav_lines.to_vec())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Plain)
                    .border_style(card_border)
                    .title(format!(" {} ", texts::tui_home_section_webdav())),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_local_env_check_card(
    frame: &mut Frame<'_>,
    app: &App,
    area: Rect,
    theme: &super::theme::Theme,
    card_border: Style,
) {
    use crate::services::local_env_check::{LocalTool, ToolCheckStatus};

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(card_border)
        .title(format!(" {} ", texts::tui_home_section_local_env_check()));
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Length(2)])
        .split(inner);

    let cols0 = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[0]);
    let cols1 = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[1]);

    let cells = [
        (LocalTool::Claude, "Claude", cols0[0]),
        (LocalTool::Codex, "Codex", cols0[1]),
        (LocalTool::Gemini, "Gemini", cols1[0]),
        (LocalTool::OpenCode, "OpenCode", cols1[1]),
    ];

    for (tool, display_name, cell_area) in cells {
        let status = if app.local_env_loading {
            None
        } else {
            app.local_env_results
                .iter()
                .find(|r| r.tool == tool)
                .map(|r| &r.status)
        };

        let (icon, icon_style) = if app.local_env_loading {
            ("…", Style::default().fg(theme.surface))
        } else {
            match status {
                Some(ToolCheckStatus::Ok { .. }) => (
                    "✓",
                    if theme.no_color {
                        Style::default()
                    } else {
                        Style::default().fg(theme.ok)
                    },
                ),
                Some(ToolCheckStatus::NotInstalledOrNotExecutable) | None => (
                    "!",
                    if theme.no_color {
                        Style::default()
                    } else {
                        Style::default().fg(theme.warn)
                    },
                ),
                Some(ToolCheckStatus::Error { .. }) => (
                    "!",
                    if theme.no_color {
                        Style::default()
                    } else {
                        Style::default().fg(theme.warn)
                    },
                ),
            }
        };

        let name_style = if theme.no_color {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        };

        let detail_style = if theme.no_color {
            Style::default()
        } else {
            Style::default().fg(theme.surface)
        };

        let value_style = Style::default().fg(theme.cyan);
        let (detail_text, detail_line_style) = if app.local_env_loading {
            ("".to_string(), detail_style)
        } else {
            match status {
                Some(ToolCheckStatus::Ok { version }) => (version.clone(), value_style),
                Some(ToolCheckStatus::NotInstalledOrNotExecutable) | None => (
                    texts::tui_local_env_not_installed().to_string(),
                    detail_style,
                ),
                Some(ToolCheckStatus::Error { message }) => (message.clone(), detail_style),
            }
        };

        let detail_width = cell_area.width.saturating_sub(1);
        let detail_text = truncate_to_display_width(&detail_text, detail_width);

        let lines = vec![
            Line::from(vec![
                Span::raw(" "),
                Span::styled(">_ ", Style::default().fg(theme.surface)),
                Span::styled(display_name.to_string(), name_style),
                Span::raw(" "),
                Span::styled(icon.to_string(), icon_style),
            ]),
            Line::from(vec![
                Span::raw(" "),
                Span::styled(detail_text, detail_line_style),
            ]),
        ];

        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), cell_area);
    }
}

#[cfg(test)]
pub(super) fn proxy_activity_wave(width: u16, current_app_routed: bool, samples: &[u64]) -> String {
    proxy_wave_lines(width, 1, current_app_routed, samples, &DOTS, false)
        .into_iter()
        .next()
        .unwrap_or_default()
}
