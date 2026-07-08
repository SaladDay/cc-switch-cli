use crate::cli::tui::data;
use std::collections::{HashMap, HashSet};

use super::*;

/// Dracula purple — used for input (downstream) graph to contrast with accent-colored output.
const DRACULA_PURPLE: (u8, u8, u8) = (189, 147, 249);

/// 图例中最低显示 token 数（近期窗口增量）；低于此值的 provider 会从图例中隐藏，避免 0% 干扰主图例
const LEGEND_MIN_RECENT_TOKENS: u64 = 1_000;

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
            .fg(theme.fg_strong)
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
    let auto_failover_queue_len = data
        .providers
        .rows
        .iter()
        .filter(|row| row.provider.in_failover_queue)
        .count();
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
        .title(format!(" {} ", texts::welcome_title()));
    frame.render_widget(block.clone(), area);

    let inner = block.inner(area);
    let content = inset_left(inner, CONTENT_INSET_LEFT);
    let bottom_hero_height = if current_app_routed { 10 } else { 6 };
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
            Constraint::Length(8),
            Constraint::Min(0),
        ])
        .split(chunks[0]);

    let card_border = Style::default().fg(theme.dim);
    render_connection_card(frame, top_chunks[1], theme, &connection_lines, card_border);
    render_webdav_card(frame, top_chunks[2], theme, &webdav_lines, card_border);
    render_local_env_check_card(frame, app, top_chunks[3], theme, card_border);

    if current_app_routed {
        // 收集近期 token 活动按 provider 聚合（用于多色图例，与点阵图同口径）
        let route_hits =
            collect_route_hits_for_dashboard(data, &app.proxy_provider_activity_samples);
        render_proxy_activity_dashboard(
            frame,
            chunks[1],
            theme,
            &app.proxy_input_activity_samples,
            &app.proxy_output_activity_samples,
            &app.proxy_provider_activity_samples,
            &uptime_text,
            &proxy_last_error_text,
            data.proxy.last_error.is_some(),
            &format!("{}:{}", data.proxy.listen_address, data.proxy.listen_port),
            data.proxy.auto_failover_enabled,
            auto_failover_queue_len,
            data.proxy.estimated_input_tokens_total,
            data.proxy.estimated_output_tokens_total,
            &route_hits,
        );
    } else {
        render_logo_hero(frame, chunks[1], theme);
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "dashboard renderer receives precomputed proxy display metrics"
)]
fn render_proxy_activity_dashboard(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &super::theme::Theme,
    input_activity_samples: &[u64],
    output_activity_samples: &[u64],
    provider_activity_samples: &HashMap<String, Vec<u64>>,
    uptime_text: &str,
    proxy_last_error_text: &str,
    has_proxy_error: bool,
    listen_text: &str,
    auto_failover_enabled: bool,
    auto_failover_queue_len: usize,
    input_tokens_total: u64,
    output_tokens_total: u64,
    route_hits: &[ProviderHitInfo],
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
        Style::default()
            .fg(Color::Rgb(
                DRACULA_PURPLE.0,
                DRACULA_PURPLE.1,
                DRACULA_PURPLE.2,
            ))
            .add_modifier(Modifier::BOLD)
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
    if auto_failover_enabled {
        let auto_failover_value = if auto_failover_queue_len > 0 {
            format!(
                "{} · {} {}",
                crate::t!("enabled", "开启"),
                crate::t!("Queue", "队列"),
                auto_failover_queue_len
            )
        } else {
            crate::t!("enabled", "开启").to_string()
        };
        push_segment(
            crate::t!("Automatic failover", "自动故障转移"),
            auto_failover_value.as_str(),
            Style::default().fg(theme.ok),
        );
    }
    if has_proxy_error {
        push_segment(
            texts::tui_label_last_proxy_error(),
            proxy_last_error_text,
            Style::default().fg(theme.warn),
        );
    }

    // 多色 Provider 近期流量图例（与点阵图共用近期 token 口径）
    // 过滤掉过小流量（< LEGEND_MIN_RECENT_TOKENS tok）的 provider
    let display_hits: Vec<&ProviderHitInfo> = route_hits
        .iter()
        .filter(|h| h.recent_tokens >= LEGEND_MIN_RECENT_TOKENS)
        .take(5)
        .collect();
    if !display_hits.is_empty() {
        // 总量基于所有 route_hits（含 < LEGEND_MIN_RECENT_TOKENS 的），让百分比统计更准
        let total_tokens: u64 = route_hits.iter().map(|h| h.recent_tokens).sum();
        if total_tokens > 0 {
            let legend_label = crate::t!("Recent tokens", "近期流量");
            meta_spans.push(Span::raw("  "));
            meta_spans.push(Span::styled(format!("{legend_label}: "), label_style));
            meta_plain.push_str("  ");
            meta_plain.push_str(legend_label);
            meta_plain.push_str(": ");
            for (i, hit) in display_hits.iter().enumerate() {
                if i > 0 {
                    meta_spans.push(Span::raw(", "));
                    meta_plain.push_str(", ");
                }
                let pct = (hit.recent_tokens as f64 / total_tokens as f64) * 100.0;
                let tok_text = format_estimated_token_compact(hit.recent_tokens);
                let text = format!("{} {}% ({})", hit.display_name, pct as i32, tok_text);
                meta_spans.push(Span::styled(
                    text.clone(),
                    Style::default().fg(hit.color).add_modifier(Modifier::BOLD),
                ));
                meta_plain.push_str(&text);
            }
        }
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

    // 从图例数据构建 provider_id → 颜色映射（与 legend 颜色一致）
    let mut provider_color_map: HashMap<String, Color> = route_hits
        .iter()
        .map(|h| (h.provider_id.clone(), h.color))
        .collect();

    // 补全颜色：直接切换的 provider（不在 route_hits 中）但仍在活动 sample 里。
    // 复用图例同款调色板，按 i % 8 取色，确保点阵有颜色。
    let palette: [Color; 8] = PER_PROVIDER_PALETTE_RGBS.map(|rgb| Color::Rgb(rgb.0, rgb.1, rgb.2));
    let palette_len = palette.len();
    if palette_len > 0 {
        // 先收齐所有缺失颜色的 provider_id，避免借用冲突
        let missing: Vec<String> = provider_activity_samples
            .keys()
            .filter(|id| !provider_color_map.contains_key(*id))
            .cloned()
            .collect();
        for (i, provider_id) in missing.iter().enumerate() {
            provider_color_map.insert(provider_id.clone(), palette[i % palette_len]);
        }
    }

    let visible_provider_ids: HashSet<String> =
        route_hits.iter().map(|h| h.provider_id.clone()).collect();
    let visible_samples: Vec<(&String, &Vec<u64>)> = provider_activity_samples
        .iter()
        .filter(|(id, _)| visible_provider_ids.contains(*id))
        .collect();

    // 点阵每列实际占据的行数（从底部算）。颜色只填点阵字符所在的区间，避免 minor
    // provider 颜色被分配到点阵空白行而不可见（图例颜色与点阵颜色对不上的根因）。
    let upper_filled =
        column_filled_rows(wave_width as usize, upper_height, output_activity_samples);
    let lower_filled =
        column_filled_rows(wave_width as usize, lower_height, input_activity_samples);

    let upper_color_stacks = compute_column_color_stacks(
        visible_samples.iter().copied(),
        wave_width as usize,
        &provider_color_map,
        upper_height as usize,
        &upper_filled,
    );
    let lower_color_stacks = compute_column_color_stacks(
        visible_samples.iter().copied(),
        wave_width as usize,
        &provider_color_map,
        lower_height as usize,
        &lower_filled,
    );

    let upper_rows = proxy_wave_lines(
        wave_width,
        upper_height,
        true,
        output_activity_samples,
        &DOTS,
        false,
    );
    let lower_rows = proxy_wave_lines(
        wave_width,
        lower_height,
        true,
        input_activity_samples,
        &REV_DOTS,
        true,
    );

    let default_upper = Style::default().fg(theme.accent);
    let default_lower = if theme.no_color {
        Style::default()
    } else {
        Style::default().fg(Color::Rgb(
            DRACULA_PURPLE.0,
            DRACULA_PURPLE.1,
            DRACULA_PURPLE.2,
        ))
    };

    // 上半部分（output），每列按 provider 颜色
    for (row_idx, row) in upper_rows.iter().enumerate() {
        let mut spans = vec![Span::raw(" ")];
        for (col_idx, ch) in row.chars().enumerate() {
            let style = match stack_color_at(&upper_color_stacks, col_idx, row_idx) {
                Some(provider_color) => {
                    if theme.no_color {
                        Style::default().add_modifier(Modifier::BOLD)
                    } else {
                        // 上半部使用 provider 颜色，稍微调亮
                        Style::default().fg(provider_color)
                    }
                }
                None => default_upper,
            };
            spans.push(Span::styled(ch.to_string(), style));
        }
        graph_lines.push(Line::from(spans));
    }

    // 下半部分（input），使用与上半部相同的 per-provider 颜色
    for (row_idx, row) in lower_rows.iter().enumerate() {
        let mut spans = vec![Span::raw(" ")];
        for (col_idx, ch) in row.chars().enumerate() {
            let style = match stack_color_at(&lower_color_stacks, col_idx, row_idx) {
                Some(provider_color) => {
                    if theme.no_color {
                        Style::default().add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(provider_color)
                    }
                }
                None => default_lower,
            };
            spans.push(Span::styled(ch.to_string(), style));
        }
        graph_lines.push(Line::from(spans));
    }

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

/// 点阵图多色 palette（与 legend 共用同一组颜色）
const PER_PROVIDER_PALETTE_RGBS: [(u8, u8, u8); 8] = [
    (189, 147, 249), // 紫
    (135, 206, 250), // 天蓝
    (255, 160, 122), // 浅三文鱼
    (144, 238, 144), // 浅绿
    (221, 160, 221), // 李子紫
    (255, 215, 0),   // 金
    (127, 255, 212), // 碧绿
    (176, 196, 222), // 淡钢蓝
];

/// 根据 per-provider 活动样本，计算每列的垂直颜色栈。
/// 颜色只填充该列点阵实际占据的行（`column_filled_rows`，从底部算），
/// 并在区间内按 token 占比分配行高：dominant 在底部，minor 紧贴其上。
/// 这样每个 provider 的颜色都落在有点阵字符的行上，minor provider 也可见。
#[allow(clippy::needless_range_loop)]
fn compute_column_color_stacks<'a>(
    provider_activity_samples: impl IntoIterator<Item = (&'a String, &'a Vec<u64>)>,
    num_columns: usize,
    provider_color_map: &HashMap<String, Color>,
    stack_height: usize,
    column_filled_rows: &[usize],
) -> Vec<Vec<Option<Color>>> {
    if num_columns == 0 || stack_height == 0 {
        return vec![vec![None; stack_height]; num_columns];
    }

    let provider_activity_samples = provider_activity_samples.into_iter().collect::<Vec<_>>();
    if provider_activity_samples.is_empty() {
        return vec![vec![None; stack_height]; num_columns];
    }

    let mut color_stacks = vec![vec![None; stack_height]; num_columns];
    for col in 0..num_columns {
        // 该列点阵实际占据的行数（从底部算）。颜色只填这个区间，避免 minor
        // provider 的颜色被分配到点阵空白行（图例与点阵颜色对不上的根因）。
        let filled = column_filled_rows
            .get(col)
            .copied()
            .unwrap_or(0)
            .min(stack_height);
        if filled == 0 {
            continue;
        }

        let mut entries = Vec::new();
        for (provider_id, samples) in &provider_activity_samples {
            let tokens = samples.get(col).copied().unwrap_or(0);
            if tokens > 0 {
                if let Some(color) = provider_color_map.get(*provider_id).copied() {
                    entries.push((provider_id.as_str(), tokens, color));
                }
            }
        }
        if entries.is_empty() {
            continue;
        }

        entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        let total_tokens = entries.iter().map(|(_, tokens, _)| *tokens).sum::<u64>();
        // 在 [0, filled) 内分配行数，dominant 占高 idx（点阵底部），minor 占低 idx（顶部字符行）。
        let mut rows = allocate_provider_rows(&entries, total_tokens, filled);
        rows.reverse();

        let base = stack_height - filled;
        let mut idx = 0;
        for (entry_idx, row_count) in rows {
            let color = entries[entry_idx].2;
            for _ in 0..row_count {
                if idx >= filled {
                    break;
                }
                color_stacks[col][base + idx] = Some(color);
                idx += 1;
            }
        }
    }
    color_stacks
}

fn stack_color_at(
    color_stacks: &[Vec<Option<Color>>],
    col_idx: usize,
    row_idx: usize,
) -> Option<Color> {
    color_stacks
        .get(col_idx)
        .and_then(|stack| stack.get(row_idx))
        .copied()
        .flatten()
}

/// 计算点阵每列实际占据的行数（从底部算），与 `proxy_wave_lines` 的渲染口径一致。
/// 颜色栈据此只填充点阵有字符的区间，确保 provider 颜色落在可见的字符行上。
fn column_filled_rows(width: usize, height: u16, samples: &[u64]) -> Vec<usize> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let recent = super::proxy_wave::recent_samples(width, true, samples);
    let scaled = super::proxy_wave::scale_samples(height, &recent, true);
    scaled
        .iter()
        .map(|v| (*v as usize).div_ceil(8))
        .map(|rows| rows.min(height as usize))
        .collect()
}

fn allocate_provider_rows(
    entries: &[(&str, u64, Color)],
    total_tokens: u64,
    stack_height: usize,
) -> Vec<(usize, usize)> {
    if entries.is_empty() || total_tokens == 0 || stack_height == 0 {
        return Vec::new();
    }

    let mut allocations = entries
        .iter()
        .enumerate()
        .map(|(idx, (_, tokens, _))| {
            let exact = (*tokens as f64 / total_tokens as f64) * stack_height as f64;
            let mut rows = exact.floor() as usize;
            if rows == 0 {
                rows = 1;
            }
            (idx, rows, exact - exact.floor())
        })
        .collect::<Vec<_>>();

    let mut total_rows = allocations.iter().map(|(_, rows, _)| *rows).sum::<usize>();
    while total_rows > stack_height {
        if let Some((_, rows, _)) = allocations
            .iter_mut()
            .filter(|(_, rows, _)| *rows > 1)
            .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
        {
            *rows -= 1;
            total_rows -= 1;
        } else {
            break;
        }
    }

    while total_rows < stack_height {
        if let Some((_, rows, _)) = allocations
            .iter_mut()
            .max_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
        {
            *rows += 1;
            total_rows += 1;
        } else {
            break;
        }
    }

    allocations
        .into_iter()
        .filter_map(|(idx, rows, _)| (rows > 0).then_some((idx, rows)))
        .collect()
}

/// Provider 命中信息（用于仪表盘多色图例和点阵图着色）
#[derive(Clone)]
struct ProviderHitInfo {
    provider_id: String,
    display_name: String,
    /// 最近 PROXY_ACTIVITY_WINDOW 窗口的 token 增量总和（近期实际流量）
    recent_tokens: u64,
    color: Color,
}

/// 从近期 token 活动样本按 provider 聚合（与点阵图同口径），分配不同颜色。
/// 聚合源为 `samples`（按 provider 的窗口 token 增量），并补齐 model_routes 中
/// enabled 但近期无流量的 provider（其 recent_tokens 为 0，会被图例阈值过滤）。
fn collect_route_hits_for_dashboard(
    data: &UiData,
    samples: &HashMap<String, Vec<u64>>,
) -> Vec<ProviderHitInfo> {
    let mut agg: HashMap<String, u64> = HashMap::new();

    // 1) 近期 token 增量是主信号：每个窗口 delta 之和
    for (provider_id, sample_vec) in samples {
        let sum: u64 = sample_vec.iter().sum();
        agg.insert(provider_id.clone(), sum);
    }

    // 2) 并集 model_routes enabled 的 provider（近期无流量的 recent_tokens 记 0，
    //    下游由 LEGEND_MIN_RECENT_TOKENS 阈值过滤）
    for row in &data.model_routes.rows {
        if !row.enabled {
            continue;
        }
        agg.entry(row.provider_id.clone()).or_insert(0);
    }

    if agg.is_empty() {
        return Vec::new();
    }

    let mut v: Vec<(String, u64)> = agg.into_iter().collect();
    // recent_tokens 降序；相同值按 provider_id 字典序，保证测试与显示稳定
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    // 使用与点阵图相同的 palette，确保颜色一致
    let palette: [Color; 8] = PER_PROVIDER_PALETTE_RGBS.map(|rgb| Color::Rgb(rgb.0, rgb.1, rgb.2));
    v.into_iter()
        .enumerate()
        .map(|(i, (provider_id, recent_tokens))| {
            let display_name = data
                .providers
                .rows
                .iter()
                .find(|p| p.id == provider_id)
                .map(|p| {
                    // 截断过长的 provider 名
                    let s = p.provider.name.clone();
                    if s.chars().count() > 8 {
                        let truncated: String = s.chars().take(6).collect();
                        format!("{truncated}…")
                    } else {
                        s
                    }
                })
                .unwrap_or_else(|| {
                    // provider 已被删除时使用 id 前 8 字符
                    provider_id.chars().take(8).collect()
                });
            ProviderHitInfo {
                provider_id: provider_id.clone(),
                display_name,
                recent_tokens,
                color: palette[i % palette.len()],
            }
        })
        .collect()
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
    use crate::services::local_env_check::LocalTool;

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(card_border)
        .title(format!(" {} ", texts::tui_home_section_local_env_check()));
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
        ])
        .split(inner);

    let row_columns = rows
        .iter()
        .map(|row| {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(*row)
        })
        .collect::<Vec<_>>();

    let cell_areas = row_columns
        .iter()
        .flat_map(|columns| columns.iter().copied())
        .collect::<Vec<_>>();

    let cells = LocalTool::all()
        .iter()
        .zip(cell_areas)
        .map(|(tool, cell_area)| (*tool, tool.display_name(), cell_area));

    for (tool, display_name, cell_area) in cells {
        render_local_env_tool_cell(frame, app, theme, tool, display_name, cell_area);
    }
}

fn render_local_env_tool_cell(
    frame: &mut Frame<'_>,
    app: &App,
    theme: &super::theme::Theme,
    tool: crate::services::local_env_check::LocalTool,
    display_name: &str,
    cell_area: Rect,
) {
    use crate::services::local_env_check::ToolCheckStatus;

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
            .fg(theme.fg_strong)
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

#[cfg(test)]
pub(super) fn proxy_activity_wave(width: u16, current_app_routed: bool, samples: &[u64]) -> String {
    proxy_wave_lines(width, 1, current_app_routed, samples, &DOTS, false)
        .into_iter()
        .next()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::tui::data::{ModelRouteRow, ProviderRow};
    use crate::provider::Provider;
    use serde_json::Value;

    /// 构造一个最小可用的 Provider（仅 id/name 有意义，其余留空）
    fn make_provider(id: &str, name: &str) -> Provider {
        Provider {
            id: id.to_string(),
            name: name.to_string(),
            settings_config: Value::Null,
            website_url: None,
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    /// 构造一个最小 ProviderRow
    fn make_provider_row(id: &str, name: &str) -> ProviderRow {
        ProviderRow {
            id: id.to_string(),
            provider: make_provider(id, name),
            api_url: None,
            is_current: false,
            is_in_config: false,
            is_saved: false,
            is_default_model: false,
            primary_model_id: None,
            default_model_id: None,
        }
    }

    /// 构造仅含给定 providers 的 UiData
    fn make_ui_data_with_providers(providers: &[(&str, &str)]) -> UiData {
        let mut data = UiData::default();
        data.providers.rows = providers
            .iter()
            .map(|(id, name)| make_provider_row(id, name))
            .collect();
        data
    }

    #[test]
    fn collect_aggregates_recent_tokens_from_samples() {
        let data = make_ui_data_with_providers(&[("p1", "DeepSeek"), ("p2", "Minimax")]);
        let mut samples = HashMap::new();
        samples.insert("p1".to_string(), vec![100, 200, 300]); // sum = 600
        samples.insert("p2".to_string(), vec![50, 50, 50]); // sum = 150

        let result = collect_route_hits_for_dashboard(&data, &samples);
        assert_eq!(result.len(), 2);
        // recent_tokens 降序：p1 在前
        assert_eq!(result[0].provider_id, "p1");
        assert_eq!(result[0].recent_tokens, 600);
        assert_eq!(result[1].provider_id, "p2");
        assert_eq!(result[1].recent_tokens, 150);
        assert_eq!(result[0].display_name, "DeepSeek");
        assert_eq!(result[1].display_name, "Minimax");
    }

    #[test]
    fn collect_returns_empty_when_no_samples_and_no_enabled_routes() {
        let data = UiData::default();
        let samples = HashMap::new();
        let result = collect_route_hits_for_dashboard(&data, &samples);
        assert!(
            result.is_empty(),
            "expected empty Vec, got {} entries",
            result.len()
        );
    }

    #[test]
    fn collect_unions_samples_with_model_routes_enabled_providers() {
        let mut data =
            make_ui_data_with_providers(&[("p_routed", "Routed"), ("p_direct", "Direct")]);
        // model_routes 含一个 enabled route 指向 p_routed（无 samples，近期无流量）
        data.model_routes.rows.push(ModelRouteRow {
            id: "r1".to_string(),
            pattern: "*".to_string(),
            provider_id: "p_routed".to_string(),
            provider_name: "Routed".to_string(),
            priority: 0,
            enabled: true,
            hit_count: 999, // 历史命中不应影响 recent_tokens 口径
            last_hit_at: None,
        });
        // samples 含 p_direct（直接切换，无 route）
        let mut samples = HashMap::new();
        samples.insert("p_direct".to_string(), vec![400, 400]); // sum = 800

        let result = collect_route_hits_for_dashboard(&data, &samples);
        let ids: Vec<&str> = result.iter().map(|h| h.provider_id.as_str()).collect();
        assert!(ids.contains(&"p_direct"), "p_direct should be in union");
        assert!(
            ids.contains(&"p_routed"),
            "p_routed should be in union via model_routes"
        );
        // recent_tokens 降序：p_direct(800) 在前，p_routed(0) 在后
        assert_eq!(result[0].provider_id, "p_direct");
        assert_eq!(result[1].provider_id, "p_routed");
        assert_eq!(result[1].recent_tokens, 0);
    }

    #[test]
    fn color_stacks_keep_multiple_providers_in_same_column() {
        let mut samples = HashMap::new();
        samples.insert("p1".to_string(), vec![90]);
        samples.insert("p2".to_string(), vec![10]);

        let p1 = Color::Rgb(255, 0, 0);
        let p2 = Color::Rgb(0, 255, 0);
        let colors = HashMap::from([("p1".to_string(), p1), ("p2".to_string(), p2)]);

        // 点阵画满 4 行：dominant(p1) 占底部，minor(p2) 占顶部字符行。
        let stacks = compute_column_color_stacks(samples.iter(), 1, &colors, 4, &[4]);

        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[0].len(), 4);
        assert!(
            stacks[0].contains(&Some(p1)),
            "dominant provider should be present"
        );
        assert!(
            stacks[0].contains(&Some(p2)),
            "smaller provider should still be visible in the same column"
        );
    }

    #[test]
    fn color_stacks_allow_single_provider_to_fill_column() {
        let mut samples = HashMap::new();
        samples.insert("p1".to_string(), vec![100]);
        samples.insert("p2".to_string(), vec![0]);

        let p1 = Color::Rgb(255, 0, 0);
        let p2 = Color::Rgb(0, 255, 0);
        let colors = HashMap::from([("p1".to_string(), p1), ("p2".to_string(), p2)]);

        let stacks = compute_column_color_stacks(samples.iter(), 1, &colors, 3, &[3]);

        assert_eq!(stacks[0], vec![Some(p1), Some(p1), Some(p1)]);
    }

    #[test]
    fn color_stacks_only_fill_rendered_rows() {
        // Regression: 点阵只画 2 行（filled=2），stack_height=4。颜色必须只填
        // 点阵字符所在的 [2, 4) 区间，minor(p2) 在顶部字符行(base=2)，dominant(p1)
        // 在底部，[0, 2) 的空白行保持 None，避免图例颜色与点阵颜色对不上。
        let mut samples = HashMap::new();
        samples.insert("p1".to_string(), vec![90]);
        samples.insert("p2".to_string(), vec![10]);

        let p1 = Color::Rgb(255, 0, 0);
        let p2 = Color::Rgb(0, 255, 0);
        let colors = HashMap::from([("p1".to_string(), p1), ("p2".to_string(), p2)]);

        let stacks = compute_column_color_stacks(samples.iter(), 1, &colors, 4, &[2]);

        assert_eq!(
            stacks[0],
            vec![None, None, Some(p2), Some(p1)],
            "colors must occupy only the rendered [base, stack_height) rows"
        );
    }
}
