use crate::cli::tui::data::UsageRangePreset;

use super::*;

pub(super) fn render_pricing(
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
            pricing_text("Usage Statistics", "使用统计"),
            pricing_text("Model Pricing", "模型定价"),
        ]));
    frame.render_widget(outer.clone(), area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
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
            ("↑↓/Pg", texts::tui_key_select()),
            ("Enter", texts::tui_key_edit()),
            ("d", texts::tui_key_delete()),
            ("/", texts::tui_filter_title()),
            ("r", texts::tui_key_refresh()),
            ("Esc", texts::tui_key_close()),
        ],
        app.focus == Focus::Content,
    );

    render_summary_bar(frame, chunks[1], theme, pricing_summary_line(app, data));
    render_pricing_table(frame, app, data, chunks[2], theme);
}

fn render_pricing_table(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let rows = app::visible_pricing_rows(&app.filter, data);
    if rows.is_empty() {
        if current_pricing_is_loading(app, data) {
            render_pricing_loading(frame, area, theme);
            return;
        }

        render_centered_pricing_lines(
            frame,
            area,
            vec![Line::styled(
                pricing_text("No model pricing rows found", "暂无模型定价"),
                Style::default().fg(theme.comment),
            )],
        );
        return;
    }

    let narrow = area.width < 104;
    let header = if narrow {
        Row::new(vec![
            Cell::from(pricing_text("Model", "模型")),
            Cell::from(pricing_text("Input/M", "输入/M")),
            Cell::from(pricing_text("Output/M", "输出/M")),
            Cell::from(pricing_text("Req 30d", "请求30天")),
            Cell::from(pricing_text("Cost 30d", "费用30天")),
        ])
    } else {
        Row::new(vec![
            Cell::from(pricing_text("Model", "模型")),
            Cell::from(pricing_text("Display", "显示名")),
            Cell::from(pricing_text("Input/M", "输入/M")),
            Cell::from(pricing_text("Output/M", "输出/M")),
            Cell::from(pricing_text("Cache R/M", "缓存读/M")),
            Cell::from(pricing_text("Cache C/M", "缓存建/M")),
            Cell::from(pricing_text("Req 30d", "请求30天")),
            Cell::from(pricing_text("Cost 30d", "费用30天")),
        ])
    }
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let table_rows = rows.iter().map(|row| {
        if narrow {
            Row::new(vec![
                Cell::from(row.model_id.clone()),
                Cell::from(format_price_per_million(&row.input_cost_per_million)),
                Cell::from(format_price_per_million(&row.output_cost_per_million)),
                Cell::from(row.recent_request_count.to_string()),
                Cell::from(format_money(row.recent_total_cost_usd)),
            ])
        } else {
            Row::new(vec![
                Cell::from(row.model_id.clone()),
                Cell::from(row.display_name.clone()),
                Cell::from(format_price_per_million(&row.input_cost_per_million)),
                Cell::from(format_price_per_million(&row.output_cost_per_million)),
                Cell::from(format_price_per_million(&row.cache_read_cost_per_million)),
                Cell::from(format_price_per_million(
                    &row.cache_creation_cost_per_million,
                )),
                Cell::from(row.recent_request_count.to_string()),
                Cell::from(format_money(row.recent_total_cost_usd)),
            ])
        }
    });
    let widths = if narrow {
        vec![
            Constraint::Min(22),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(9),
            Constraint::Length(10),
        ]
    } else {
        vec![
            Constraint::Percentage(24),
            Constraint::Percentage(22),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(11),
            Constraint::Length(11),
            Constraint::Length(9),
            Constraint::Length(10),
        ]
    };
    let table = Table::new(table_rows, widths)
        .header(header)
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));
    let mut state = TableState::default();
    state.select(Some(app.pricing.selected_idx));
    frame.render_stateful_widget(table, inset_left(area, CONTENT_INSET_LEFT), &mut state);
}

fn pricing_summary_line(app: &App, data: &UiData) -> String {
    if current_pricing_is_loading(app, data) {
        return pricing_text("Loading...", "正在加载中...").to_string();
    }

    if i18n::is_chinese() {
        let summary = format!(
            "{} 个目录模型 · 30天使用 {} 个 · 30天未匹配 {} 个模型 · {} tokens · {} total",
            data.pricing.total_models(),
            data.pricing.recently_used_models(),
            data.pricing.recent_unknown_models,
            format_token_compact(data.pricing.recent_total_tokens()),
            format_money(data.pricing.recent_total_cost_usd())
        );
        let summary = if data.pricing.recent_unmatched_total_cost_usd > 0.0 {
            format!(
                "{summary} · 未匹配 {}",
                format_money(data.pricing.recent_unmatched_total_cost_usd)
            )
        } else {
            summary
        };
        if app
            .usage
            .is_loading_for(&app.app_type, UsageRangePreset::SevenDays)
        {
            format!("{}{}", pricing_refresh_prefix(app, "正在刷新"), summary)
        } else {
            summary
        }
    } else {
        let summary = format!(
            "{} catalog models · {} used 30d · {} unmatched models 30d · {} tokens · {} total",
            data.pricing.total_models(),
            data.pricing.recently_used_models(),
            data.pricing.recent_unknown_models,
            format_token_compact(data.pricing.recent_total_tokens()),
            format_money(data.pricing.recent_total_cost_usd())
        );
        let summary = if data.pricing.recent_unmatched_total_cost_usd > 0.0 {
            format!(
                "{summary} · {} unmatched",
                format_money(data.pricing.recent_unmatched_total_cost_usd)
            )
        } else {
            summary
        };
        if app
            .usage
            .is_loading_for(&app.app_type, UsageRangePreset::SevenDays)
        {
            format!("{}{}", pricing_refresh_prefix(app, "Refreshing"), summary)
        } else {
            summary
        }
    }
}

fn pricing_refresh_prefix(app: &App, label: &str) -> String {
    let spinner = match app.tick % 4 {
        0 => "⠋",
        1 => "⠙",
        2 => "⠹",
        _ => "⠸",
    };
    format!("{spinner} {label} · ")
}

fn current_pricing_is_loading(app: &App, data: &UiData) -> bool {
    app.usage
        .is_loading_for(&app.app_type, UsageRangePreset::SevenDays)
        && !data.pricing.has_data()
}

fn render_pricing_loading(frame: &mut Frame<'_>, area: Rect, theme: &super::theme::Theme) {
    render_centered_pricing_lines(
        frame,
        area,
        vec![Line::styled(
            pricing_text("Loading...", "正在加载中..."),
            Style::default().fg(theme.comment),
        )],
    );
}

fn render_centered_pricing_lines(frame: &mut Frame<'_>, area: Rect, lines: Vec<Line<'static>>) {
    let line_count = lines.len() as u16;
    let y = area.y + area.height.saturating_sub(line_count) / 2;
    let centered = Rect::new(area.x, y, area.width, line_count.min(area.height));
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), centered);
}

fn pricing_text(en: &'static str, zh: &'static str) -> &'static str {
    if i18n::is_chinese() {
        zh
    } else {
        en
    }
}

fn format_price_per_million(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "-".to_string();
    }
    match trimmed.parse::<f64>() {
        Ok(0.0) => "$0".to_string(),
        Ok(value) if value >= 100.0 => format!("${value:.0}"),
        Ok(value) if value >= 10.0 => format!("${value:.1}"),
        Ok(value) if value >= 1.0 => format!("${value:.2}"),
        Ok(value) => format!("${value:.4}"),
        Err(_) => trimmed.to_string(),
    }
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
