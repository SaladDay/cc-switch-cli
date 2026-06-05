use crate::cli::tui::app::{UsageMetric, UsagePane};
use crate::cli::tui::data::{
    UsageLogRow, UsageModelStatsRow, UsageProviderStatsRow, UsageTrendBucket,
};

use super::*;

pub(super) fn render_usage(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(usage_text("Usage Statistics", "使用统计"));
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(9),
            Constraint::Min(0),
        ])
        .split(inner);

    if app.focus == Focus::Content {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[
                ("1", usage_text("Today", "今日")),
                ("2", usage_text("7 days", "7天")),
                ("3", usage_text("30 days", "30天")),
                ("m", usage_text("metric", "指标")),
                ("Tab/←→", texts::tui_key_pane()),
                ("L", usage_text("logs", "日志")),
                ("r", texts::tui_key_refresh()),
            ],
        );
    }

    render_summary_bar(frame, chunks[1], theme, usage_summary_line(app, data));
    render_usage_metrics(frame, app, data, chunks[2], theme);

    let narrow = chunks[3].width < 110;
    let body = Layout::default()
        .direction(if narrow {
            Direction::Vertical
        } else {
            Direction::Horizontal
        })
        .constraints(if narrow {
            [Constraint::Percentage(55), Constraint::Percentage(45)]
        } else {
            [Constraint::Percentage(58), Constraint::Percentage(42)]
        })
        .split(chunks[3]);
    render_usage_trend(frame, app, data, body[0], theme);
    render_usage_focus_table(frame, app, data, body[1], theme);
}

pub(super) fn render_usage_logs(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(usage_text("Usage Logs", "用量日志"));
    frame.render_widget(outer.clone(), area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(outer.inner(area));

    if app.focus == Focus::Content {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[
                ("↑↓/Pg", texts::tui_key_select()),
                ("Enter/d", texts::tui_key_details()),
                ("r", texts::tui_key_refresh()),
                ("Esc", texts::tui_key_close()),
            ],
        );
    }

    render_summary_bar(frame, chunks[1], theme, usage_logs_summary_line(data));
    render_usage_logs_table(frame, app, data, chunks[2], theme);
}

pub(super) fn render_usage_log_detail(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
    request_id: &str,
) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(usage_text("Usage Log Detail", "用量日志详情"));
    frame.render_widget(outer.clone(), area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(outer.inner(area));

    if app.focus == Focus::Content {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[
                ("r", texts::tui_key_refresh()),
                ("Esc", texts::tui_key_close()),
            ],
        );
    }

    let row = data
        .usage
        .recent_logs
        .iter()
        .find(|row| row.request_id == request_id);
    render_usage_detail_body(frame, row, chunks[1], theme);
}

fn render_usage_metrics(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let summary = data.usage.summary_for(app.usage.range);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.dim))
        .title(format!(" {} ", usage_text("Overview", "概览")));
    frame.render_widget(block.clone(), area);
    let inner = inset_left(block.inner(area), CONTENT_INSET_LEFT);

    if inner.width < 92 || inner.height < 5 {
        let lines = vec![
            Line::from(vec![
                Span::styled(usage_text("Cost ", "费用 "), Style::default().fg(theme.dim)),
                Span::styled(
                    format_money(summary.total_cost_usd),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    usage_text("Tokens ", "Token "),
                    Style::default().fg(theme.dim),
                ),
                Span::styled(
                    format_token_compact(summary.total_tokens()),
                    Style::default().fg(theme.ok).add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(usage_text("Req ", "请求 "), Style::default().fg(theme.dim)),
                Span::styled(
                    summary.total_requests.to_string(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled(
                    usage_text("Success ", "成功率 "),
                    Style::default().fg(theme.dim),
                ),
                Span::styled(
                    format_percent(summary.success_rate()),
                    success_rate_style(summary.success_rate(), theme).add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    usage_text("Latency ", "延迟 "),
                    Style::default().fg(theme.dim),
                ),
                Span::styled(
                    format_ms(summary.avg_latency_ms),
                    Style::default()
                        .fg(theme.comment)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled(usage_text("TTFT ", "首字 "), Style::default().fg(theme.dim)),
                Span::styled(
                    format_ms(summary.avg_first_token_ms),
                    Style::default()
                        .fg(theme.comment)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    usage_text("Cache ", "缓存 "),
                    Style::default().fg(theme.dim),
                ),
                Span::styled(
                    format_percent(summary.cache_hit_rate()),
                    Style::default()
                        .fg(theme.comment)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
        ];
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Length(3)])
        .split(inner);

    let primary = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(rows[0]);
    let secondary = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(rows[1]);

    let token_breakdown = format!(
        "{} / {}",
        format_token_compact(summary.input_tokens),
        format_token_compact(summary.output_tokens)
    );
    let cache_tokens = summary
        .cache_read_tokens
        .saturating_add(summary.cache_creation_tokens);

    let primary_metrics = [
        UsageMetricCard {
            label: usage_text("Cost", "费用"),
            value: format_money(summary.total_cost_usd),
            meta: usage_text("selected range", "当前范围").to_string(),
            value_style: Style::default().fg(theme.accent),
        },
        UsageMetricCard {
            label: usage_text("Tokens", "Token"),
            value: format_token_compact(summary.total_tokens()),
            meta: token_breakdown,
            value_style: Style::default().fg(theme.ok),
        },
        UsageMetricCard {
            label: usage_text("Requests", "请求"),
            value: summary.total_requests.to_string(),
            meta: format!(
                "{} {}",
                summary.success_count,
                usage_text("success", "成功")
            ),
            value_style: Style::default().fg(Color::White),
        },
        UsageMetricCard {
            label: usage_text("Success", "成功率"),
            value: format_percent(summary.success_rate()),
            meta: usage_text("healthy responses", "健康响应").to_string(),
            value_style: success_rate_style(summary.success_rate(), theme),
        },
    ];
    for (idx, card) in primary_metrics.iter().enumerate() {
        render_usage_metric_card(frame, primary[idx], card, theme);
    }

    let secondary_metrics = [
        UsageMetricCard {
            label: usage_text("Input / Output", "输入 / 输出"),
            value: format!(
                "{} / {}",
                format_token_compact(summary.input_tokens),
                format_token_compact(summary.output_tokens)
            ),
            meta: usage_text("prompt vs completion", "提示词 / 输出").to_string(),
            value_style: Style::default().fg(theme.cyan),
        },
        UsageMetricCard {
            label: usage_text("Cache", "缓存"),
            value: format_percent(summary.cache_hit_rate()),
            meta: format!(
                "{} {}",
                format_token_compact(cache_tokens),
                usage_text("tokens", "Token")
            ),
            value_style: Style::default().fg(theme.comment),
        },
        UsageMetricCard {
            label: usage_text("Latency", "延迟"),
            value: format_ms(summary.avg_latency_ms),
            meta: usage_text("average", "平均").to_string(),
            value_style: Style::default().fg(theme.comment),
        },
        UsageMetricCard {
            label: usage_text("TTFT", "首字"),
            value: format_ms(summary.avg_first_token_ms),
            meta: usage_text("first token", "首字延迟").to_string(),
            value_style: Style::default().fg(theme.comment),
        },
    ];
    for (idx, card) in secondary_metrics.iter().enumerate() {
        render_usage_metric_card(frame, secondary[idx], card, theme);
    }
}

struct UsageMetricCard {
    label: &'static str,
    value: String,
    meta: String,
    value_style: Style,
}

fn render_usage_metric_card(
    frame: &mut Frame<'_>,
    area: Rect,
    card: &UsageMetricCard,
    theme: &super::theme::Theme,
) {
    if area.width < 8 || area.height < 2 {
        return;
    }

    let label_width = area.width.saturating_sub(1);
    let value_width = area.width.saturating_sub(1);
    let meta_width = area.width.saturating_sub(1);
    let lines = vec![
        Line::styled(
            truncate_to_display_width(card.label, label_width),
            Style::default().fg(theme.dim),
        ),
        Line::styled(
            truncate_to_display_width(&card.value, value_width),
            card.value_style.add_modifier(Modifier::BOLD),
        ),
        Line::styled(
            truncate_to_display_width(&card.meta, meta_width),
            Style::default().fg(theme.comment),
        ),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn render_usage_trend(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let title = format!(
        " {} · {} · {} ",
        usage_text("Usage Trend", "使用趋势"),
        app.usage.range.label(),
        usage_metric_label(app.usage.metric)
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.dim))
        .title(title);
    frame.render_widget(block.clone(), area);
    let inner = inset_left(block.inner(area), CONTENT_INSET_LEFT);

    let trend = data.usage.trend_for(app.usage.range);
    if trend
        .iter()
        .all(|bucket| usage_bucket_value(bucket, app.usage.metric) == 0.0)
    {
        render_centered_usage_lines(
            frame,
            inner,
            vec![
                Line::styled(
                    usage_text("No usage recorded for this range", "当前范围暂无用量记录"),
                    Style::default().fg(theme.comment),
                ),
                Line::styled(
                    usage_text(
                        "Proxy and synced session logs will appear here",
                        "代理和已同步会话日志会显示在这里",
                    ),
                    Style::default().fg(theme.dim),
                ),
            ],
        );
        return;
    }

    let content = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(0)])
        .split(inner);
    frame.render_widget(
        Paragraph::new(vec![
            usage_trend_summary_line(trend, app.usage.metric, theme),
            usage_trend_range_line(trend, theme),
        ])
        .wrap(Wrap { trim: false }),
        content[0],
    );

    let visible = fit_trend_points(trend, content[1].width);
    if content[1].width >= 44 && content[1].height >= 7 && visible.len() >= 2 {
        render_usage_line_chart(frame, &visible, app.usage.metric, content[1], theme);
    } else {
        render_usage_sparkline(frame, &visible, app.usage.metric, content[1], theme);
    }
}

fn usage_trend_summary_line(
    trend: &[UsageTrendBucket],
    metric: UsageMetric,
    theme: &super::theme::Theme,
) -> Line<'static> {
    let latest_value = trend
        .last()
        .map(|bucket| usage_bucket_value(bucket, metric))
        .unwrap_or_default();
    let peak = trend.iter().max_by(|lhs, rhs| {
        usage_bucket_value(lhs, metric)
            .partial_cmp(&usage_bucket_value(rhs, metric))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let peak_label = peak
        .map(|bucket| bucket.label.clone())
        .unwrap_or_else(|| "-".to_string());
    let peak_value = peak
        .map(|bucket| usage_bucket_value(bucket, metric))
        .unwrap_or_default();
    let total = trend
        .iter()
        .map(|bucket| usage_bucket_value(bucket, metric))
        .sum::<f64>();

    Line::from(vec![
        Span::styled(
            usage_text("Latest ", "最新 "),
            Style::default().fg(theme.dim),
        ),
        Span::styled(
            format_metric_value(latest_value, metric),
            usage_metric_style(metric, theme).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(usage_text("Peak ", "峰值 "), Style::default().fg(theme.dim)),
        Span::styled(
            format!("{} {}", peak_label, format_metric_value(peak_value, metric)),
            usage_metric_style(metric, theme).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            usage_text("Total ", "总计 "),
            Style::default().fg(theme.dim),
        ),
        Span::styled(
            format_metric_value(total, metric),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

fn usage_trend_range_line(
    trend: &[UsageTrendBucket],
    theme: &super::theme::Theme,
) -> Line<'static> {
    let first = trend
        .first()
        .map(|bucket| bucket.label.clone())
        .unwrap_or_else(|| "-".to_string());
    let last = trend
        .last()
        .map(|bucket| bucket.label.clone())
        .unwrap_or_else(|| "-".to_string());
    Line::from(vec![
        Span::styled(
            usage_text("Range ", "区间 "),
            Style::default().fg(theme.dim),
        ),
        Span::styled(
            format!("{first} -> {last}"),
            Style::default().fg(theme.comment),
        ),
    ])
}

fn render_usage_line_chart(
    frame: &mut Frame<'_>,
    buckets: &[&UsageTrendBucket],
    metric: UsageMetric,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let mut points = buckets
        .iter()
        .enumerate()
        .map(|(idx, bucket)| (idx as f64, usage_bucket_value(bucket, metric)))
        .collect::<Vec<_>>();
    if points.len() == 1 {
        points.push((1.0, points[0].1));
    }

    let max_value = points
        .iter()
        .map(|(_, value)| *value)
        .fold(0.0, f64::max)
        .max(1.0);
    let last_x = (points.len().saturating_sub(1)).max(1) as f64;
    let first_label = buckets
        .first()
        .map(|bucket| bucket.label.clone())
        .unwrap_or_else(|| "-".to_string());
    let last_label = buckets
        .last()
        .map(|bucket| bucket.label.clone())
        .unwrap_or_else(|| "-".to_string());

    let dataset = Dataset::default()
        .name(usage_metric_label(metric))
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(usage_metric_style(metric, theme).add_modifier(Modifier::BOLD))
        .data(&points);
    let chart = Chart::new(vec![dataset])
        .legend_position(None)
        .x_axis(
            Axis::default()
                .style(Style::default().fg(theme.dim))
                .bounds([0.0, last_x])
                .labels([
                    Line::styled(
                        truncate_to_display_width(&first_label, 8),
                        Style::default().fg(theme.comment),
                    ),
                    Line::styled(
                        truncate_to_display_width(&last_label, 8),
                        Style::default().fg(theme.comment),
                    ),
                ]),
        )
        .y_axis(
            Axis::default()
                .style(Style::default().fg(theme.dim))
                .bounds([0.0, max_value * 1.05])
                .labels([
                    Line::styled("0", Style::default().fg(theme.comment)),
                    Line::styled(
                        format_metric_value(max_value, metric),
                        Style::default().fg(theme.comment),
                    ),
                ]),
        );
    frame.render_widget(chart, area);
}

fn render_usage_sparkline(
    frame: &mut Frame<'_>,
    buckets: &[&UsageTrendBucket],
    metric: UsageMetric,
    area: Rect,
    theme: &super::theme::Theme,
) {
    if buckets.is_empty() {
        return;
    }

    let values = buckets
        .iter()
        .map(|bucket| usage_bucket_value(bucket, metric))
        .collect::<Vec<_>>();
    let first = buckets
        .first()
        .map(|bucket| bucket.label.clone())
        .unwrap_or_else(|| "-".to_string());
    let last = buckets
        .last()
        .map(|bucket| bucket.label.clone())
        .unwrap_or_else(|| "-".to_string());
    let latest = values.last().copied().unwrap_or_default();
    let lines = vec![
        Line::styled(
            usage_sparkline(&values),
            usage_metric_style(metric, theme).add_modifier(Modifier::BOLD),
        ),
        Line::from(vec![
            Span::styled(format!("{first} -> {last}"), Style::default().fg(theme.dim)),
            Span::raw("  "),
            Span::styled(
                format_metric_value(latest, metric),
                usage_metric_style(metric, theme),
            ),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn render_usage_focus_table(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    match app.usage.pane {
        UsagePane::Providers => render_top_providers_table(
            frame,
            app,
            data.usage.top_providers_for(app.usage.range),
            area,
            theme,
        ),
        UsagePane::Models => render_top_models_table(
            frame,
            app,
            data.usage.top_models_for(app.usage.range),
            area,
            theme,
        ),
        UsagePane::Recent => {
            render_recent_usage_table(frame, app, &data.usage.recent_logs, area, theme)
        }
    }
}

fn render_top_providers_table(
    frame: &mut Frame<'_>,
    app: &App,
    rows: &[UsageProviderStatsRow],
    area: Rect,
    theme: &super::theme::Theme,
) {
    let block = usage_focus_block(
        app,
        UsagePane::Providers,
        usage_text("Top Providers", "供应商排行"),
        theme,
    );
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);
    if rows.is_empty() {
        render_empty_table(frame, inner, theme);
        return;
    }

    let header = Row::new(vec![
        Cell::from(usage_text("Provider", "供应商")),
        Cell::from(usage_text("Req", "请求")),
        Cell::from(usage_text("Success", "成功")),
        Cell::from(usage_text("Tokens", "Token")),
        Cell::from(usage_text("Cost", "费用")),
        Cell::from(usage_text("Avg", "平均")),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));
    let table_rows = rows.iter().map(|row| {
        Row::new(vec![
            Cell::from(display_provider_name(
                row.provider_name.as_deref(),
                &row.provider_id,
            )),
            Cell::from(row.request_count.to_string()),
            Cell::from(format_success_rate(row.success_count, row.request_count)),
            Cell::from(format_token_compact(row.total_tokens)),
            Cell::from(format_money(row.total_cost_usd)),
            Cell::from(format_ms(row.avg_latency_ms)),
        ])
    });
    let table = Table::new(
        table_rows,
        [
            Constraint::Min(16),
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(7),
        ],
    )
    .header(header)
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));
    let mut state = TableState::default();
    if matches!(app.usage.pane, UsagePane::Providers) {
        state.select(Some(app.usage.selected_idx));
    }
    frame.render_stateful_widget(table, inset_left(inner, CONTENT_INSET_LEFT), &mut state);
}

fn render_top_models_table(
    frame: &mut Frame<'_>,
    app: &App,
    rows: &[UsageModelStatsRow],
    area: Rect,
    theme: &super::theme::Theme,
) {
    let block = usage_focus_block(
        app,
        UsagePane::Models,
        usage_text("Top Models", "模型排行"),
        theme,
    );
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);
    if rows.is_empty() {
        render_empty_table(frame, inner, theme);
        return;
    }

    let header = Row::new(vec![
        Cell::from(usage_text("Model", "模型")),
        Cell::from(usage_text("Req", "请求")),
        Cell::from(usage_text("Success", "成功")),
        Cell::from(usage_text("Tokens", "Token")),
        Cell::from(usage_text("Cost", "费用")),
        Cell::from(usage_text("Avg", "平均")),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));
    let table_rows = rows.iter().map(|row| {
        Row::new(vec![
            Cell::from(row.model.clone()),
            Cell::from(row.request_count.to_string()),
            Cell::from(format_success_rate(row.success_count, row.request_count)),
            Cell::from(format_token_compact(row.total_tokens)),
            Cell::from(format_money(row.total_cost_usd)),
            Cell::from(format_ms(row.avg_latency_ms)),
        ])
    });
    let table = Table::new(
        table_rows,
        [
            Constraint::Min(16),
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(7),
        ],
    )
    .header(header)
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));
    let mut state = TableState::default();
    if matches!(app.usage.pane, UsagePane::Models) {
        state.select(Some(app.usage.selected_idx));
    }
    frame.render_stateful_widget(table, inset_left(inner, CONTENT_INSET_LEFT), &mut state);
}

fn render_recent_usage_table(
    frame: &mut Frame<'_>,
    app: &App,
    rows: &[UsageLogRow],
    area: Rect,
    theme: &super::theme::Theme,
) {
    let block = usage_focus_block(
        app,
        UsagePane::Recent,
        usage_text("Recent Logs", "最近日志"),
        theme,
    );
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);
    if rows.is_empty() {
        render_empty_table(frame, inner, theme);
        return;
    }

    let header = Row::new(vec![
        Cell::from(usage_text("Time", "时间")),
        Cell::from(usage_text("Model", "模型")),
        Cell::from(usage_text("Status", "状态")),
        Cell::from(usage_text("Cost", "费用")),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));
    let table_rows = rows.iter().take(8).map(|row| {
        Row::new(vec![
            Cell::from(format_log_time(row.created_at, false)),
            Cell::from(row.model.clone()),
            Cell::from(status_label(row.status_code)),
            Cell::from(format_money(row.total_cost_usd)),
        ])
        .style(status_style(row, theme))
    });
    let table = Table::new(
        table_rows,
        [
            Constraint::Length(11),
            Constraint::Percentage(48),
            Constraint::Length(8),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));
    let mut state = TableState::default();
    if matches!(app.usage.pane, UsagePane::Recent) {
        state.select(Some(app.usage.selected_idx));
    }
    frame.render_stateful_widget(table, inset_left(inner, CONTENT_INSET_LEFT), &mut state);
}

fn render_usage_logs_table(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    if data.usage.recent_logs.is_empty() {
        render_centered_usage_lines(
            frame,
            area,
            vec![Line::styled(
                usage_text("No usage logs found", "暂无用量日志"),
                Style::default().fg(theme.comment),
            )],
        );
        return;
    }

    if area.width < 96 {
        let header = Row::new(vec![
            Cell::from(usage_text("Time", "时间")),
            Cell::from(usage_text("Model", "模型")),
            Cell::from(usage_text("Status", "状态")),
            Cell::from(usage_text("Cost", "费用")),
        ])
        .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));
        let rows = data.usage.recent_logs.iter().map(|row| {
            Row::new(vec![
                Cell::from(format_log_time(row.created_at, true)),
                Cell::from(row.model.clone()),
                Cell::from(status_label(row.status_code)),
                Cell::from(format_money(row.total_cost_usd)),
            ])
            .style(status_style(row, theme))
        });
        let table = Table::new(
            rows,
            [
                Constraint::Length(17),
                Constraint::Min(16),
                Constraint::Length(8),
                Constraint::Length(10),
            ],
        )
        .header(header)
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));
        let mut state = TableState::default();
        state.select(Some(app.usage.logs_idx));
        frame.render_stateful_widget(table, inset_left(area, CONTENT_INSET_LEFT), &mut state);
        return;
    }

    let header = Row::new(vec![
        Cell::from(usage_text("Time", "时间")),
        Cell::from(usage_text("Provider", "供应商")),
        Cell::from(usage_text("Model", "模型")),
        Cell::from(usage_text("Status", "状态")),
        Cell::from(usage_text("Tokens", "Token")),
        Cell::from(usage_text("Cost", "费用")),
        Cell::from(usage_text("Latency", "延迟")),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));
    let rows = data.usage.recent_logs.iter().map(|row| {
        Row::new(vec![
            Cell::from(format_log_time(row.created_at, true)),
            Cell::from(display_provider_name(
                row.provider_name.as_deref(),
                &row.provider_id,
            )),
            Cell::from(row.model.clone()),
            Cell::from(status_label(row.status_code)),
            Cell::from(format_token_compact(row.total_tokens())),
            Cell::from(format_money(row.total_cost_usd)),
            Cell::from(format!("{}ms", row.latency_ms)),
        ])
        .style(status_style(row, theme))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(17),
            Constraint::Percentage(20),
            Constraint::Percentage(27),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(9),
        ],
    )
    .header(header)
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));
    let mut state = TableState::default();
    state.select(Some(app.usage.logs_idx));
    frame.render_stateful_widget(table, inset_left(area, CONTENT_INSET_LEFT), &mut state);
}

fn render_usage_detail_body(
    frame: &mut Frame<'_>,
    row: Option<&UsageLogRow>,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let Some(row) = row else {
        render_centered_usage_lines(
            frame,
            area,
            vec![Line::styled(
                usage_text(
                    "This log is no longer in the recent cache",
                    "这条日志已不在最近缓存中",
                ),
                Style::default().fg(theme.comment),
            )],
        );
        return;
    };

    let provider = display_provider_name(row.provider_name.as_deref(), &row.provider_id);
    let source = row.data_source.as_deref().unwrap_or("proxy");
    let stream = if row.is_streaming {
        usage_text("yes", "是")
    } else {
        usage_text("no", "否")
    };
    let request_model = row.request_model.as_deref().unwrap_or("-");
    let session_id = row.session_id.as_deref().unwrap_or("-");
    let provider_type = row.provider_type.as_deref().unwrap_or("-");
    let first_token = row
        .first_token_ms
        .map(|value| format!("{value}ms"))
        .unwrap_or_else(|| "-".to_string());
    let duration = row
        .duration_ms
        .map(|value| format!("{value}ms"))
        .unwrap_or_else(|| "-".to_string());
    let error = row.error_message.as_deref().unwrap_or("-");
    let lines = vec![
        detail_line(usage_text("Request", "请求"), &row.request_id, theme),
        detail_line(
            usage_text("Time", "时间"),
            &format_log_time(row.created_at, true),
            theme,
        ),
        detail_line(usage_text("App", "应用"), &row.app_type, theme),
        detail_line(usage_text("Provider", "供应商"), &provider, theme),
        detail_line(
            usage_text("Provider Type", "供应商类型"),
            provider_type,
            theme,
        ),
        detail_line(usage_text("Model", "模型"), &row.model, theme),
        detail_line(
            usage_text("Request Model", "请求模型"),
            request_model,
            theme,
        ),
        detail_line(
            usage_text("Status", "状态"),
            &status_label(row.status_code),
            theme,
        ),
        detail_line(
            usage_text("Tokens", "Token"),
            &format!("{}", row.total_tokens()),
            theme,
        ),
        detail_line(
            usage_text("Input", "输入"),
            &row.input_tokens.to_string(),
            theme,
        ),
        detail_line(
            usage_text("Output", "输出"),
            &row.output_tokens.to_string(),
            theme,
        ),
        detail_line(
            usage_text("Cache Read", "缓存读取"),
            &row.cache_read_tokens.to_string(),
            theme,
        ),
        detail_line(
            usage_text("Cache Create", "缓存创建"),
            &row.cache_creation_tokens.to_string(),
            theme,
        ),
        detail_line(
            usage_text("Cost", "费用"),
            &format_money(row.total_cost_usd),
            theme,
        ),
        detail_line(
            usage_text("Latency", "延迟"),
            &format!("{}ms", row.latency_ms),
            theme,
        ),
        detail_line(usage_text("First Token", "首字"), &first_token, theme),
        detail_line(usage_text("Duration", "耗时"), &duration, theme),
        detail_line(usage_text("Streaming", "流式"), stream, theme),
        detail_line(usage_text("Session", "会话"), session_id, theme),
        detail_line(usage_text("Source", "来源"), source, theme),
        detail_line(usage_text("Error", "错误"), error, theme),
    ];
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        inset_left(area, CONTENT_INSET_LEFT),
    );
}

fn usage_focus_block(
    app: &App,
    pane: UsagePane,
    title: &'static str,
    theme: &super::theme::Theme,
) -> Block<'static> {
    let style = if app.usage.pane == pane {
        Style::default().fg(theme.accent)
    } else {
        Style::default().fg(theme.dim)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(style)
        .title(match pane {
            UsagePane::Providers | UsagePane::Models => {
                format!(" {title} · {} ", app.usage.range.label())
            }
            UsagePane::Recent => format!(" {title} "),
        })
}

fn render_empty_table(frame: &mut Frame<'_>, area: Rect, theme: &super::theme::Theme) {
    render_centered_usage_lines(
        frame,
        area,
        vec![Line::styled(
            usage_text("No data for the selected range", "当前范围暂无数据"),
            Style::default().fg(theme.comment),
        )],
    );
}

fn render_centered_usage_lines(frame: &mut Frame<'_>, area: Rect, lines: Vec<Line<'static>>) {
    let line_count = lines.len() as u16;
    let y = area.y + area.height.saturating_sub(line_count) / 2;
    let centered = Rect::new(area.x, y, area.width, line_count.min(area.height));
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), centered);
}

fn detail_line(
    label: &'static str,
    value: impl AsRef<str>,
    theme: &super::theme::Theme,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<14}"), Style::default().fg(theme.dim)),
        Span::raw(" "),
        Span::styled(
            value.as_ref().to_string(),
            Style::default().fg(Color::White),
        ),
    ])
}

fn usage_summary_line(app: &App, data: &UiData) -> String {
    let summary = data.usage.summary_for(app.usage.range);
    if i18n::is_chinese() {
        format!(
            "{} · {} 请求 · {} tokens · {} · 平均延迟 {}",
            app.usage.range.label(),
            summary.total_requests,
            format_token_compact(summary.total_tokens()),
            format_money(summary.total_cost_usd),
            format_ms(summary.avg_latency_ms)
        )
    } else {
        format!(
            "{} · {} requests · {} tokens · {} · {} avg latency",
            app.usage.range.label(),
            summary.total_requests,
            format_token_compact(summary.total_tokens()),
            format_money(summary.total_cost_usd),
            format_ms(summary.avg_latency_ms)
        )
    }
}

fn usage_logs_summary_line(data: &UiData) -> String {
    if i18n::is_chinese() {
        format!(
            "显示最近 {} 条 · 共 {} 条",
            data.usage.recent_logs.len(),
            data.usage.logs_total
        )
    } else {
        format!(
            "Latest {} rows shown · {} total rows",
            data.usage.recent_logs.len(),
            data.usage.logs_total
        )
    }
}

fn usage_text(en: &'static str, zh: &'static str) -> &'static str {
    if i18n::is_chinese() {
        zh
    } else {
        en
    }
}

fn usage_metric_label(metric: UsageMetric) -> &'static str {
    match metric {
        UsageMetric::Cost => usage_text("Cost", "费用"),
        UsageMetric::Tokens => usage_text("Tokens", "Token"),
        UsageMetric::Requests => usage_text("Requests", "请求"),
        UsageMetric::Errors => usage_text("Errors", "错误"),
    }
}

fn usage_bucket_value(bucket: &UsageTrendBucket, metric: UsageMetric) -> f64 {
    match metric {
        UsageMetric::Cost => bucket.total_cost_usd,
        UsageMetric::Tokens => bucket.total_tokens as f64,
        UsageMetric::Requests => bucket.request_count as f64,
        UsageMetric::Errors => bucket.error_count as f64,
    }
}

fn usage_metric_style(metric: UsageMetric, theme: &super::theme::Theme) -> Style {
    match metric {
        UsageMetric::Cost => Style::default().fg(theme.accent),
        UsageMetric::Tokens => Style::default().fg(theme.ok),
        UsageMetric::Requests => Style::default().fg(Color::White),
        UsageMetric::Errors => Style::default().fg(theme.err),
    }
}

fn format_metric_value(value: f64, metric: UsageMetric) -> String {
    match metric {
        UsageMetric::Cost => format_money(value),
        UsageMetric::Tokens => format_token_compact(value.max(0.0).round() as u64),
        UsageMetric::Requests | UsageMetric::Errors => format!("{:.0}", value),
    }
}

fn fit_trend_points<'a>(trend: &'a [UsageTrendBucket], width: u16) -> Vec<&'a UsageTrendBucket> {
    let point_budget = if width < 44 {
        width.saturating_sub(4).max(6) as usize
    } else {
        width.saturating_sub(12).max(12) as usize
    };
    if trend.len() <= point_budget {
        return trend.iter().collect();
    }

    let start = trend.len().saturating_sub(point_budget);
    trend[start..].iter().collect()
}

fn usage_sparkline(values: &[f64]) -> String {
    const BLOCKS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    if values.is_empty() {
        return String::new();
    }

    let max_value = values.iter().copied().fold(0.0, f64::max);
    if max_value <= f64::EPSILON {
        return "▁".repeat(values.len());
    }

    values
        .iter()
        .map(|value| {
            let idx = ((*value / max_value) * (BLOCKS.len() - 1) as f64).round() as usize;
            BLOCKS[idx.min(BLOCKS.len() - 1)]
        })
        .collect::<Vec<_>>()
        .join("")
}

fn format_money(value: f64) -> String {
    if value >= 100.0 {
        format!("${value:.0}")
    } else if value >= 10.0 {
        format!("${value:.1}")
    } else {
        format!("${value:.3}")
    }
}

fn format_token_compact(total: u64) -> String {
    if total < 1_000 {
        return total.to_string();
    }
    if total < 1_000_000 {
        return format!("{:.1}k", total as f64 / 1_000.0);
    }
    format!("{:.1}M", total as f64 / 1_000_000.0)
}

fn format_percent(value: Option<f64>) -> String {
    value
        .map(|value| format!("{:.0}%", value.clamp(0.0, 100.0)))
        .unwrap_or_else(|| "-".to_string())
}

fn format_success_rate(success: u64, total: u64) -> String {
    if total == 0 {
        "-".to_string()
    } else {
        format!("{:.0}%", success as f64 * 100.0 / total as f64)
    }
}

fn format_ms(value: Option<u64>) -> String {
    value
        .map(|value| format!("{value}ms"))
        .unwrap_or_else(|| "-".to_string())
}

fn success_rate_style(value: Option<f64>, theme: &super::theme::Theme) -> Style {
    match value {
        Some(value) if value >= 95.0 => Style::default().fg(theme.ok),
        Some(value) if value >= 80.0 => Style::default().fg(theme.warn),
        Some(_) => Style::default().fg(theme.err),
        None => Style::default().fg(theme.comment),
    }
}

fn status_label(status_code: u16) -> String {
    if (200..300).contains(&status_code) {
        "ok".to_string()
    } else {
        status_code.to_string()
    }
}

fn status_style(row: &UsageLogRow, theme: &super::theme::Theme) -> Style {
    if row.is_success() {
        Style::default()
    } else if row.status_code >= 500 {
        Style::default().fg(theme.err)
    } else {
        Style::default().fg(theme.warn)
    }
}

fn format_log_time(timestamp: i64, full: bool) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|datetime| {
            if full {
                datetime.format("%Y/%m/%d %H:%M").to_string()
            } else {
                datetime.format("%H:%M").to_string()
            }
        })
        .unwrap_or_else(|| "-".to_string())
}

fn display_provider_name(name: Option<&str>, fallback: &str) -> String {
    name.filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}
