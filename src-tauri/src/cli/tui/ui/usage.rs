use crate::cli::tui::app::{UsageMetric, UsagePane};
use crate::cli::tui::data::{
    UsageLogRow, UsageLogTextField, UsageModelStatsRow, UsageProviderStatsRow,
    UsageSummarySnapshot, UsageTrendBucket,
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
        .title(format!(" {} ", usage_text("Usage Statistics", "使用统计")));
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(8),
            Constraint::Min(0),
        ])
        .split(inner);

    let keys = crate::cli::tui::keymap::usage::key_bar_items(app, data);
    render_page_key_bar(frame, chunks[0], theme, &keys, app.focus == Focus::Content);

    render_summary_bar(frame, chunks[1], theme, usage_summary_line(app, data));
    render_usage_metrics(frame, app, data, chunks[2], theme);

    render_usage_trend(frame, app, data, chunks[3], theme);
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
        .title(breadcrumb_title(&[
            usage_text("Usage Statistics", "使用统计"),
            usage_text("Details", "详情"),
        ]));
    frame.render_widget(outer.clone(), area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(outer.inner(area));

    render_page_key_bar(
        frame,
        chunks[0],
        theme,
        &[
            ("Tab", texts::tui_key_pane()),
            ("↑↓/Pg", texts::tui_key_select()),
            ("Enter", texts::tui_key_details()),
            ("r", texts::tui_key_refresh()),
            ("Esc", texts::tui_key_close()),
        ],
        app.focus == Focus::Content,
    );

    render_usage_detail_tabs(frame, app, chunks[1], theme);
    render_summary_bar(
        frame,
        chunks[2],
        theme,
        usage_detail_summary_line(app, data),
    );
    render_usage_detail_table(frame, app, data, chunks[3], theme);
}

pub(super) fn render_usage_log_detail(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
    rowid: i64,
) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(breadcrumb_title(&[
            usage_text("Usage Statistics", "使用统计"),
            usage_text("Details", "详情"),
            usage_text("Log", "日志"),
        ]));
    frame.render_widget(outer.clone(), area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(outer.inner(area));

    render_page_key_bar(
        frame,
        chunks[0],
        theme,
        &[
            ("r", texts::tui_key_refresh()),
            ("Esc", texts::tui_key_close()),
        ],
        app.focus == Focus::Content,
    );

    let row = app
        .usage
        .log_detail_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.row_for(&app.app_type, app.usage.range, rowid))
        .or_else(|| {
            app.usage
                .log_pager
                .source_matches(&app.app_type, app.usage.range)
                .then(|| {
                    app.usage
                        .log_pager
                        .current_rows(data.usage.recent_logs_for(app.usage.range))
                        .iter()
                        .find(|row| row.cursor_rowid == rowid)
                })
                .flatten()
        });
    render_usage_detail_body(frame, row, chunks[1], theme);
}

fn render_usage_metrics(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    if area.width < 16 || area.height == 0 {
        return;
    }
    let loading = current_usage_is_loading(app, data);

    if area.width < 36 || area.height < 4 {
        if loading {
            render_usage_loading(frame, area, theme);
        } else {
            let summary = data.usage.summary_for(app.usage.range);
            render_usage_metrics_untitled_compact(frame, summary, area, theme);
        }
        return;
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.dim))
        .title(format!(" {} ", usage_text("Overview", "概览")));
    let block_inner = block.inner(area);
    let inner = inset_horizontal(block_inner, CONTENT_INSET_LEFT, CONTENT_INSET_LEFT);
    if inner.width < 20 || inner.height == 0 {
        if loading {
            render_usage_loading(frame, area, theme);
        } else {
            let summary = data.usage.summary_for(app.usage.range);
            render_usage_metrics_untitled_compact(frame, summary, area, theme);
        }
        return;
    }

    frame.render_widget(block.clone(), area);

    if loading {
        render_usage_loading(frame, inner, theme);
        return;
    }

    let summary = data.usage.summary_for(app.usage.range);
    if inner.height < 4 {
        render_usage_metrics_compact(frame, summary, inner, theme);
        return;
    }

    if inner.height >= 6 {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(3),
            ])
            .split(inner);

        render_usage_metric_row(frame, rows[0], &usage_primary_metrics(summary), theme);
        render_usage_metric_row(
            frame,
            rows[1],
            &usage_secondary_metrics(summary, app.app_type.as_str()),
            theme,
        );
        render_usage_metric_row(frame, rows[2], &usage_tertiary_metrics(summary), theme);
        render_usage_cache_hit_line(frame, summary, rows[3], theme);
        return;
    }

    if inner.height >= 5 {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(3),
            ])
            .split(inner);

        render_usage_metric_row(frame, rows[0], &usage_primary_metrics(summary), theme);
        render_usage_metric_row(
            frame,
            rows[1],
            &usage_secondary_metrics(summary, app.app_type.as_str()),
            theme,
        );
        render_usage_cache_hit_line(frame, summary, rows[2], theme);
        return;
    }

    if inner.height >= 4 {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Min(0),
            ])
            .split(inner);

        render_usage_metric_row(frame, rows[0], &usage_primary_metrics(summary), theme);
        render_usage_cache_hit_line(frame, summary, rows[1], theme);
        return;
    }

    render_usage_metrics_compact(frame, summary, inner, theme);
}

struct UsageMetricCard {
    label: &'static str,
    value: String,
}

fn usage_primary_metrics(summary: &UsageSummarySnapshot) -> [UsageMetricCard; 4] {
    [
        UsageMetricCard {
            label: usage_text("Real Tokens", "真实 Token"),
            value: format_token_compact(summary.total_tokens()),
        },
        UsageMetricCard {
            label: usage_text("Requests", "请求"),
            value: summary.total_requests.to_string(),
        },
        UsageMetricCard {
            label: usage_text("Cost", "费用"),
            value: format_money(summary.total_cost_usd),
        },
        UsageMetricCard {
            label: usage_text("Success", "成功率"),
            value: format_percent(summary.success_rate()),
        },
    ]
}

fn usage_secondary_metrics(summary: &UsageSummarySnapshot, app_type: &str) -> [UsageMetricCard; 4] {
    [
        UsageMetricCard {
            label: usage_text("Input", "输入"),
            value: format_token_compact(summary.input_tokens),
        },
        UsageMetricCard {
            label: usage_text("Output", "输出"),
            value: format_token_compact(summary.output_tokens),
        },
        UsageMetricCard {
            label: usage_text("Cache Read", "缓存读取"),
            value: format_token_compact(summary.cache_read_tokens),
        },
        UsageMetricCard {
            label: usage_text("Cache Write", "缓存写入"),
            value: format_cache_write_tokens(app_type, summary.cache_creation_tokens, true),
        },
    ]
}

fn format_cache_write_tokens(app_type: &str, tokens: u64, compact: bool) -> String {
    let protocol_does_not_report_writes =
        app_type.eq_ignore_ascii_case("codex") || app_type.eq_ignore_ascii_case("gemini");
    if protocol_does_not_report_writes && tokens == 0 {
        return "N/A".to_string();
    }

    if compact {
        format_token_compact(tokens)
    } else {
        tokens.to_string()
    }
}

fn usage_tertiary_metrics(summary: &UsageSummarySnapshot) -> [UsageMetricCard; 4] {
    [
        UsageMetricCard {
            label: usage_text("Errors", "错误"),
            value: summary
                .total_requests
                .saturating_sub(summary.success_count)
                .to_string(),
        },
        UsageMetricCard {
            label: usage_text("Avg Latency", "平均延迟"),
            value: format_ms(summary.avg_latency_ms),
        },
        UsageMetricCard {
            label: usage_text("Cache Tokens", "缓存 Token"),
            value: format_token_compact(
                summary
                    .cache_read_tokens
                    .saturating_add(summary.cache_creation_tokens),
            ),
        },
        UsageMetricCard {
            label: usage_text("Cost / Req", "单次费用"),
            value: format_money_per_request(summary.total_cost_usd, summary.total_requests),
        },
    ]
}

fn usage_metric_value_style(theme: &super::theme::Theme) -> Style {
    Style::default().fg(theme.accent)
}

fn inset_horizontal(area: Rect, left: u16, right: u16) -> Rect {
    let total = left.saturating_add(right);
    if area.width <= total {
        return area;
    }

    Rect {
        x: area.x + left,
        y: area.y,
        width: area.width - total,
        height: area.height,
    }
}

fn render_usage_metric_row(
    frame: &mut Frame<'_>,
    area: Rect,
    cards: &[UsageMetricCard; 4],
    theme: &super::theme::Theme,
) {
    if area.width < 20 || area.height == 0 {
        return;
    }

    let Some((gap, base_width)) = usage_metric_row_spacing(area.width) else {
        return;
    };
    let gaps_width = gap * 3;
    let available_width = area.width.saturating_sub(gaps_width);

    let mut columns = [Rect::new(area.x, area.y, 0, area.height); 4];
    let mut x = area.x;
    let mut remainder = available_width % 4;
    for (idx, column) in columns.iter_mut().enumerate() {
        let extra = u16::from(remainder > 0);
        remainder = remainder.saturating_sub(1);
        let width = base_width + extra;
        *column = Rect::new(x, area.y, width, area.height);
        x = x.saturating_add(width);
        if idx + 1 < cards.len() {
            x = x.saturating_add(gap);
        }
    }

    for (idx, card) in cards.iter().enumerate() {
        render_usage_metric_card(frame, columns[idx], card, theme);
    }
}

pub(super) fn usage_metric_row_spacing(width: u16) -> Option<(u16, u16)> {
    if width < 20 {
        return None;
    }

    let preferred_gap = if width >= 84 { 4 } else { 2 };
    let gap = if width.saturating_sub(preferred_gap * 3) / 4 >= 8 {
        preferred_gap
    } else {
        1
    };
    let column_width = width.saturating_sub(gap * 3) / 4;
    (column_width >= 8).then_some((gap, column_width))
}

fn render_usage_metric_card(
    frame: &mut Frame<'_>,
    area: Rect,
    card: &UsageMetricCard,
    theme: &super::theme::Theme,
) {
    if area.width < 8 || area.height == 0 {
        return;
    }

    let label_value_gap = if area.width >= 12 { 2 } else { 1 };
    let max_value_width = area.width.saturating_sub(label_value_gap + 1);
    if max_value_width == 0 {
        return;
    }
    let value_width = (UnicodeWidthStr::width(card.value.as_str()) as u16)
        .min(max_value_width)
        .max(1);
    let label_width = area
        .width
        .saturating_sub(value_width)
        .saturating_sub(label_value_gap);
    if label_width < 2 {
        return;
    }

    let label = truncate_to_display_width(card.label, label_width);
    let value = truncate_to_display_width(&card.value, value_width);
    if label.is_empty() || value.is_empty() {
        return;
    }

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(label, Style::default().fg(theme.dim)),
            Span::raw(" ".repeat(label_value_gap as usize)),
            Span::styled(
                value,
                usage_metric_value_style(theme).add_modifier(Modifier::BOLD),
            ),
        ])),
        area,
    );
}

fn render_usage_metrics_compact(
    frame: &mut Frame<'_>,
    summary: &UsageSummarySnapshot,
    area: Rect,
    theme: &super::theme::Theme,
) {
    if area.height == 0 {
        return;
    }

    if area.height == 1 || area.width < 40 {
        frame.render_widget(
            Paragraph::new(usage_metrics_primary_compact_line(summary, theme))
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    if area.height < 4 {
        render_usage_metric_row(frame, area, &usage_primary_metrics(summary), theme);
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(area);
    frame.render_widget(
        Paragraph::new(usage_metrics_primary_compact_line(summary, theme))
            .wrap(Wrap { trim: true }),
        rows[0],
    );
    render_usage_cache_hit_line(frame, summary, rows[1], theme);
}

fn render_usage_metrics_untitled_compact(
    frame: &mut Frame<'_>,
    summary: &UsageSummarySnapshot,
    area: Rect,
    theme: &super::theme::Theme,
) {
    if area.height == 0 {
        return;
    }

    if area.height >= 4 && area.width >= 20 {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Min(0),
            ])
            .split(area);
        frame.render_widget(
            Paragraph::new(usage_metrics_primary_compact_line(summary, theme))
                .wrap(Wrap { trim: true }),
            rows[0],
        );
        render_usage_cache_hit_line(frame, summary, rows[1], theme);
        return;
    }

    frame.render_widget(
        Paragraph::new(usage_metrics_primary_compact_line(summary, theme))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn usage_metrics_primary_compact_line(
    summary: &UsageSummarySnapshot,
    theme: &super::theme::Theme,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            usage_text("Tokens ", "Token "),
            Style::default().fg(theme.dim),
        ),
        Span::styled(
            format_token_compact(summary.total_tokens()),
            usage_metric_value_style(theme).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(usage_text("Req ", "请求 "), Style::default().fg(theme.dim)),
        Span::styled(
            summary.total_requests.to_string(),
            usage_metric_value_style(theme).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(usage_text("Cost ", "费用 "), Style::default().fg(theme.dim)),
        Span::styled(
            format_money(summary.total_cost_usd),
            usage_metric_value_style(theme).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            usage_text("Success ", "成功率 "),
            Style::default().fg(theme.dim),
        ),
        Span::styled(
            format_percent(summary.success_rate()),
            usage_metric_value_style(theme).add_modifier(Modifier::BOLD),
        ),
    ])
}

fn render_usage_cache_hit_line(
    frame: &mut Frame<'_>,
    summary: &UsageSummarySnapshot,
    area: Rect,
    theme: &super::theme::Theme,
) {
    if area.width < 20 || area.height < 3 {
        return;
    }

    let rate = summary.cache_hit_rate();
    let ratio = rate.unwrap_or_default().clamp(0.0, 100.0) / 100.0;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.dim));
    let inner = inset_horizontal(block.inner(area), CONTENT_INSET_LEFT, CONTENT_INSET_LEFT);
    if inner.width < 12 || inner.height == 0 {
        return;
    }

    frame.render_widget(block, area);

    let label = Line::from(vec![
        Span::styled(
            usage_text("Cache Hit ", "缓存命中率 "),
            Style::default().fg(theme.dim),
        ),
        Span::styled(
            format_percent(rate),
            usage_metric_value_style(theme).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ·", Style::default().fg(theme.dim)),
    ]);

    let gauge = LineGauge::default()
        .label(label)
        .filled_symbol(symbols::line::THICK_HORIZONTAL)
        .unfilled_symbol(symbols::line::HORIZONTAL)
        .filled_style(Style::default().fg(theme.accent))
        .unfilled_style(Style::default().fg(theme.dim))
        .ratio(ratio);
    frame.render_widget(gauge, inner);
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
        .title(format!(" {} ", title));
    frame.render_widget(block.clone(), area);
    let inner = inset_horizontal(block.inner(area), CONTENT_INSET_LEFT, 4);

    if current_usage_is_loading(app, data) {
        render_usage_loading(frame, inner, theme);
        return;
    }

    let trend = data.usage.trend_for(app.usage.range);
    if !data.usage.has_data_for(app.usage.range) {
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

    let visible = fit_trend_points(trend, inner.width);
    if inner.width >= 44 && inner.height >= 7 && !visible.is_empty() {
        render_usage_line_chart(frame, &visible, app.usage.metric, inner, theme);
    } else {
        render_usage_sparkline(frame, &visible, app.usage.metric, inner, theme);
    }
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
    let middle_label = buckets
        .get(buckets.len() / 2)
        .map(|bucket| bucket.label.clone());
    let mut x_labels = Vec::from([Line::styled(
        truncate_to_display_width(&first_label, 8),
        Style::default().fg(theme.comment),
    )]);
    if buckets.len() > 2 {
        if let Some(label) = middle_label {
            x_labels.push(Line::styled(
                truncate_to_display_width(&label, 8),
                Style::default().fg(theme.comment),
            ));
        }
    }
    x_labels.push(Line::styled(
        truncate_to_display_width(&last_label, 8),
        Style::default().fg(theme.comment),
    ));

    let y_labels = [
        Line::styled("0", Style::default().fg(theme.comment)),
        Line::styled(
            format_metric_value(max_value / 2.0, metric),
            Style::default().fg(theme.comment),
        ),
        Line::styled(
            format_metric_value(max_value, metric),
            Style::default().fg(theme.comment),
        ),
    ];

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
                .labels(x_labels),
        )
        .y_axis(
            Axis::default()
                .style(Style::default().fg(theme.dim))
                .bounds([0.0, max_value * 1.05])
                .labels(y_labels)
                .labels_alignment(Alignment::Right),
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
    frame.render_widget(
        Paragraph::new(Line::styled(
            usage_sparkline(&values),
            usage_metric_style(metric, theme).add_modifier(Modifier::BOLD),
        ))
        .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_usage_detail_tabs(
    frame: &mut Frame<'_>,
    app: &App,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let items = [
        (UsagePane::Models, usage_text("Model Stats", "模型统计")),
        (
            UsagePane::Providers,
            usage_text("Provider Stats", "Provider 统计"),
        ),
        (UsagePane::Recent, usage_text("Request Logs", "请求日志")),
    ];
    let mut spans = Vec::new();
    for (idx, (pane, label)) in items.into_iter().enumerate() {
        if idx > 0 {
            spans.push(Span::raw(" "));
        }
        let style = if app.usage.pane == pane {
            Style::default()
                .fg(theme.on_accent)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.dim)
        };
        spans.push(Span::styled(format!(" {label} "), style));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_usage_detail_table(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.accent))
        .title(format!(" {} ", usage_detail_pane_title(app.usage.pane)));
    if matches!(app.usage.pane, UsagePane::Recent) {
        if let Some(footer) = usage_logs_pagination_footer(app, data) {
            block = with_pagination_footer(block, area.width, footer, theme, app.tick);
        }
    }
    frame.render_widget(block.clone(), area);
    let inner = inset_left(block.inner(area), CONTENT_INSET_LEFT);
    let loading = current_usage_is_loading(app, data);

    match app.usage.pane {
        UsagePane::Models => render_usage_models_table(
            frame,
            app,
            data.usage.top_models_for(app.usage.range),
            inner,
            theme,
            loading,
        ),
        UsagePane::Providers => render_usage_providers_table(
            frame,
            app,
            data.usage.top_providers_for(app.usage.range),
            inner,
            theme,
            loading,
        ),
        UsagePane::Recent => render_usage_logs_table(frame, app, data, inner, theme),
    }
}

fn render_usage_providers_table(
    frame: &mut Frame<'_>,
    app: &App,
    rows: &[UsageProviderStatsRow],
    area: Rect,
    theme: &super::theme::Theme,
    loading: bool,
) {
    if rows.is_empty() {
        render_empty_table(frame, area, theme, loading);
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
                false,
                false,
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
    state.select(Some(app.usage.selected_idx));
    frame.render_stateful_widget(table, area, &mut state);
}

fn render_usage_models_table(
    frame: &mut Frame<'_>,
    app: &App,
    rows: &[UsageModelStatsRow],
    area: Rect,
    theme: &super::theme::Theme,
    loading: bool,
) {
    if rows.is_empty() {
        render_empty_table(frame, area, theme, loading);
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
    state.select(Some(app.usage.selected_idx));
    frame.render_stateful_widget(table, area, &mut state);
}

fn render_usage_logs_table(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let logs = app
        .usage
        .log_pager
        .current_rows(data.usage.recent_logs_for(app.usage.range));
    if logs.is_empty() {
        render_empty_table(frame, area, theme, current_usage_is_loading(app, data));
        return;
    }
    let selected = app.usage.logs_idx.min(logs.len() - 1);
    let visible_rows = area.height.saturating_sub(1).max(1) as usize;
    let start = if logs.len() <= visible_rows {
        0
    } else {
        selected
            .saturating_sub(visible_rows / 2)
            .min(logs.len() - visible_rows)
    };
    let end = start.saturating_add(visible_rows).min(logs.len());
    let visible_logs = &logs[start..end];

    if area.width < 96 {
        let header = Row::new(vec![
            Cell::from(usage_text("Time", "时间")),
            Cell::from(usage_text("Model", "模型")),
            Cell::from(usage_text("Status", "状态")),
            Cell::from(usage_text("Cost", "费用")),
        ])
        .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));
        let rows = visible_logs.iter().map(|row| {
            Row::new(vec![
                Cell::from(format_log_time(row.created_at, true)),
                Cell::from(bounded_display(
                    &row.model,
                    row.text_was_truncated(UsageLogTextField::Model),
                )),
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
        if app.usage.log_pager.gate.is_row_focused() {
            state.select(Some(selected - start));
        }
        frame.render_stateful_widget(table, area, &mut state);
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
    let rows = visible_logs.iter().map(|row| {
        Row::new(vec![
            Cell::from(format_log_time(row.created_at, true)),
            Cell::from(display_provider_name(
                row.provider_name.as_deref(),
                &row.provider_id,
                row.text_was_truncated(UsageLogTextField::ProviderName),
                row.text_was_truncated(UsageLogTextField::ProviderId),
            )),
            Cell::from(bounded_display(
                &row.model,
                row.text_was_truncated(UsageLogTextField::Model),
            )),
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
    if app.usage.log_pager.gate.is_row_focused() {
        state.select(Some(selected - start));
    }
    frame.render_stateful_widget(table, area, &mut state);
}

fn usage_logs_pagination_footer(app: &App, data: &UiData) -> Option<PaginationFooter> {
    use app::paged_list::{PageBoundary, PagedListFocus};

    let total = effective_usage_logs_total(app, data);
    if total == 0 {
        return None;
    }
    let pager = &app.usage.log_pager;
    let page = pager.current_page();
    let rows = pager.current_rows(data.usage.recent_logs_for(app.usage.range));
    let start = page
        .saturating_mul(crate::cli::tui::data::USAGE_LOG_PAGE_SIZE)
        .saturating_add(1);
    let end = start
        .saturating_add(rows.len().saturating_sub(1))
        .min(total);

    let boundary_status = |target: usize, ready: PaginationFooterStatus| {
        if pager.page_error(target).is_some() {
            PaginationFooterStatus::LoadError
        } else if pager.page_is_pending(target) {
            PaginationFooterStatus::LoadingPage(target + 1)
        } else if pager.page_is_available(target) {
            ready
        } else {
            PaginationFooterStatus::PreparingNext
        }
    };
    let status = match pager.gate.focus() {
        PagedListFocus::Boundary(PageBoundary::Next) => {
            boundary_status(page.saturating_add(1), PaginationFooterStatus::NextBoundary)
        }
        PagedListFocus::Boundary(PageBoundary::Previous) => page
            .checked_sub(1)
            .map_or(PaginationFooterStatus::Idle, |target| {
                boundary_status(target, PaginationFooterStatus::PreviousBoundary)
            }),
        PagedListFocus::Empty | PagedListFocus::Row if pager.gate.is_at_end() => {
            PaginationFooterStatus::End
        }
        PagedListFocus::Empty | PagedListFocus::Row => {
            let target = page.saturating_add(1);
            if pager.page_is_pending(target) {
                PaginationFooterStatus::PreparingNext
            } else if pager.page_is_available(target) {
                PaginationFooterStatus::NextReady
            } else {
                PaginationFooterStatus::Idle
            }
        }
    };
    Some(PaginationFooter {
        page: page + 1,
        start,
        end,
        total,
        status,
    })
}

fn effective_usage_logs_total(app: &App, data: &UiData) -> usize {
    let reported =
        usize::try_from(data.usage.logs_total_for(app.usage.range)).unwrap_or(usize::MAX);
    // A keyset page can prove that an asynchronously reported count was stale.
    // Prefer the pager's corrected length after it has been synchronized.
    if app.usage.log_pager.gate.is_empty() {
        reported
    } else {
        app.usage.log_pager.gate.len()
    }
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

    let provider = display_provider_name(
        row.provider_name.as_deref(),
        &row.provider_id,
        row.text_was_truncated(UsageLogTextField::ProviderName),
        row.text_was_truncated(UsageLogTextField::ProviderId),
    );
    let source = bounded_optional_display(
        row.data_source.as_deref(),
        "proxy",
        row.text_was_truncated(UsageLogTextField::DataSource),
    );
    let stream = if row.is_streaming {
        usage_text("yes", "是")
    } else {
        usage_text("no", "否")
    };
    let request_model = bounded_optional_display(
        row.request_model.as_deref(),
        "-",
        row.text_was_truncated(UsageLogTextField::RequestModel),
    );
    let session_id = bounded_optional_display(
        row.session_id.as_deref(),
        "-",
        row.text_was_truncated(UsageLogTextField::SessionId),
    );
    let provider_type = bounded_optional_display(
        row.provider_type.as_deref(),
        "-",
        row.text_was_truncated(UsageLogTextField::ProviderType),
    );
    let first_token = row
        .first_token_ms
        .map(|value| format!("{value}ms"))
        .unwrap_or_else(|| "-".to_string());
    let duration = row
        .duration_ms
        .map(|value| format!("{value}ms"))
        .unwrap_or_else(|| "-".to_string());
    let error = match (row.error_message.as_deref(), row.error_message_truncated) {
        (Some(error), true) => {
            let limit = crate::cli::tui::data::USAGE_LOG_ERROR_MESSAGE_MAX_CHARS;
            let marker = if i18n::is_chinese() {
                format!("[显示内容已截断至 {limit} 个字符]")
            } else {
                format!("[display capped at {limit} characters]")
            };
            format!("{error} {marker}")
        }
        (Some(error), false) => error.to_string(),
        (None, _) => "-".to_string(),
    };
    let lines = vec![
        detail_line(
            usage_text("Request", "请求"),
            bounded_display(
                &row.request_id,
                row.text_was_truncated(UsageLogTextField::RequestId),
            ),
            theme,
        ),
        detail_line(
            usage_text("Time", "时间"),
            format_log_time(row.created_at, true),
            theme,
        ),
        detail_line(
            usage_text("App", "应用"),
            bounded_display(
                &row.app_type,
                row.text_was_truncated(UsageLogTextField::AppType),
            ),
            theme,
        ),
        detail_line(usage_text("Provider", "供应商"), &provider, theme),
        detail_line(
            usage_text("Provider Type", "供应商类型"),
            &provider_type,
            theme,
        ),
        detail_line(
            usage_text("Model", "模型"),
            bounded_display(&row.model, row.text_was_truncated(UsageLogTextField::Model)),
            theme,
        ),
        detail_line(
            usage_text("Request Model", "请求模型"),
            &request_model,
            theme,
        ),
        detail_line(
            usage_text("Status", "状态"),
            status_label(row.status_code),
            theme,
        ),
        detail_line(
            usage_text("Tokens", "Token"),
            format!("{}", row.total_tokens()),
            theme,
        ),
        detail_line(
            usage_text("Input", "输入"),
            row.input_tokens.to_string(),
            theme,
        ),
        detail_line(
            usage_text("Output", "输出"),
            row.output_tokens.to_string(),
            theme,
        ),
        detail_line(
            usage_text("Cache Read", "缓存读取"),
            row.cache_read_tokens.to_string(),
            theme,
        ),
        detail_line(
            usage_text("Cache Create", "缓存创建"),
            format_cache_write_tokens(&row.app_type, row.cache_creation_tokens, false),
            theme,
        ),
        detail_line(
            usage_text("Cost", "费用"),
            format_money(row.total_cost_usd),
            theme,
        ),
        detail_line(
            usage_text("Latency", "延迟"),
            format!("{}ms", row.latency_ms),
            theme,
        ),
        detail_line(usage_text("First Token", "首字"), &first_token, theme),
        detail_line(usage_text("Duration", "耗时"), &duration, theme),
        detail_line(usage_text("Streaming", "流式"), stream, theme),
        detail_line(usage_text("Session", "会话"), &session_id, theme),
        detail_line(usage_text("Source", "来源"), &source, theme),
        detail_line(usage_text("Error", "错误"), &error, theme),
    ];
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        inset_left(area, CONTENT_INSET_LEFT),
    );
}

fn usage_detail_pane_title(pane: UsagePane) -> &'static str {
    match pane {
        UsagePane::Models => usage_text("Model Stats", "模型统计"),
        UsagePane::Providers => usage_text("Provider Stats", "Provider 统计"),
        UsagePane::Recent => usage_text("Request Logs", "请求日志"),
    }
}

fn render_empty_table(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &super::theme::Theme,
    loading: bool,
) {
    if loading {
        render_usage_loading(frame, area, theme);
        return;
    }

    render_centered_usage_lines(
        frame,
        area,
        vec![Line::styled(
            usage_text("No data for the selected range", "当前范围暂无数据"),
            Style::default().fg(theme.comment),
        )],
    );
}

fn render_usage_loading(frame: &mut Frame<'_>, area: Rect, theme: &super::theme::Theme) {
    render_centered_usage_lines(
        frame,
        area,
        vec![Line::styled(
            usage_text("Loading...", "正在加载中..."),
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
            Style::default().fg(theme.fg_strong),
        ),
    ])
}

/// 后台会话日志导入进行时的进度后缀（如 " · 正在导入本地用量 1234/18704"）。
/// 空闲时返回空串。数据来自 sync_progress 全局原子量——CLI 构建没有逐行
/// 通知通道，渲染时直接读取即可。
fn usage_sync_progress_suffix() -> String {
    match crate::services::session_usage::sync_progress::snapshot() {
        Some((done, total)) if total > 0 => {
            if i18n::is_chinese() {
                format!(" · 正在导入本地用量 {done}/{total}")
            } else {
                format!(" · importing local usage {done}/{total}")
            }
        }
        _ => String::new(),
    }
}

fn usage_summary_line(app: &App, data: &UiData) -> String {
    let sync_suffix = usage_sync_progress_suffix();
    if current_usage_is_loading(app, data) {
        if i18n::is_chinese() {
            return format!("{} · 正在加载中...{sync_suffix}", app.usage.range.label());
        }
        return format!("{} · Loading...{sync_suffix}", app.usage.range.label());
    }

    let summary = data.usage.summary_for(app.usage.range);
    let refresh_prefix = usage_refresh_prefix(app);
    if i18n::is_chinese() {
        format!(
            "{refresh_prefix}{} · {} 请求 · {} tokens · {} · 平均延迟 {}{sync_suffix}",
            app.usage.range.label(),
            summary.total_requests,
            format_token_compact(summary.total_tokens()),
            format_money(summary.total_cost_usd),
            format_ms(summary.avg_latency_ms)
        )
    } else {
        format!(
            "{refresh_prefix}{} · {} requests · {} tokens · {} · {} avg latency{sync_suffix}",
            app.usage.range.label(),
            summary.total_requests,
            format_token_compact(summary.total_tokens()),
            format_money(summary.total_cost_usd),
            format_ms(summary.avg_latency_ms)
        )
    }
}

fn current_usage_is_loading(app: &App, data: &UiData) -> bool {
    app.usage.is_loading_for(&app.app_type, app.usage.range)
        && !data.usage.has_data_for(app.usage.range)
}

fn usage_detail_summary_line(app: &App, data: &UiData) -> String {
    let mut line = match app.usage.pane {
        UsagePane::Models => {
            let count = data.usage.top_models_for(app.usage.range).len();
            if i18n::is_chinese() {
                format!("{} · 模型统计 · {} 条", app.usage.range.label(), count)
            } else {
                format!("{} · model stats · {} rows", app.usage.range.label(), count)
            }
        }
        UsagePane::Providers => {
            let count = data.usage.top_providers_for(app.usage.range).len();
            if i18n::is_chinese() {
                format!("{} · Provider 统计 · {} 条", app.usage.range.label(), count)
            } else {
                format!(
                    "{} · provider stats · {} rows",
                    app.usage.range.label(),
                    count
                )
            }
        }
        UsagePane::Recent => {
            let logs = app
                .usage
                .log_pager
                .current_rows(data.usage.recent_logs_for(app.usage.range));
            let total = effective_usage_logs_total(app, data);
            if i18n::is_chinese() {
                format!(
                    "请求日志 · 第 {} 页 · 本页 {} 条 · 共 {} 条",
                    app.usage.log_pager.current_page() + 1,
                    logs.len(),
                    total
                )
            } else {
                format!(
                    "request logs · page {} · {} rows · {} total rows",
                    app.usage.log_pager.current_page() + 1,
                    logs.len(),
                    total
                )
            }
        }
    };
    line.insert_str(0, &usage_refresh_prefix(app));
    line.push_str(&usage_sync_progress_suffix());
    line
}

fn usage_refresh_prefix(app: &App) -> String {
    if app.usage.manual_session_refreshing()
        || app.usage.is_loading_for(&app.app_type, app.usage.range)
        || app.usage.log_page_refresh_after_aggregate_requested()
        || app.usage.log_pager.has_refresh_pending()
    {
        let spinner = match app.tick % 4 {
            0 => "⠋",
            1 => "⠙",
            2 => "⠹",
            _ => "⠸",
        };
        format!("{spinner} {} · ", usage_text("Refreshing", "正在刷新"))
    } else {
        String::new()
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
        UsageMetric::Requests => Style::default().fg(theme.fg_strong),
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

fn fit_trend_points(trend: &[UsageTrendBucket], width: u16) -> Vec<&UsageTrendBucket> {
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

fn format_money_per_request(total_cost: f64, total_requests: u64) -> String {
    if total_requests == 0 {
        "-".to_string()
    } else {
        format_money(total_cost / total_requests as f64)
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

fn bounded_display(value: &str, truncated: bool) -> String {
    if truncated {
        format!("{value}…")
    } else {
        value.to_string()
    }
}

fn bounded_optional_display(value: Option<&str>, fallback: &str, truncated: bool) -> String {
    bounded_display(value.unwrap_or(fallback), truncated && value.is_some())
}

fn display_provider_name(
    name: Option<&str>,
    fallback: &str,
    name_truncated: bool,
    fallback_truncated: bool,
) -> String {
    if let Some(name) = name.filter(|value| !value.trim().is_empty()) {
        bounded_display(name, name_truncated)
    } else {
        bounded_display(fallback, fallback_truncated)
    }
}
