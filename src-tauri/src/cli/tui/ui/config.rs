use super::*;

use crate::cli::tui::app::LocalProxySettingsItem;
use unicode_width::UnicodeWidthStr;

pub(super) fn config_items_filtered(app: &App) -> Vec<ConfigItem> {
    app::visible_config_items(&app.filter, &app.app_type)
}

pub(super) fn config_item_label(item: &ConfigItem) -> &'static str {
    app::config_item_label(item)
}

pub(super) fn webdav_config_items_filtered(app: &App) -> Vec<WebDavConfigItem> {
    app::visible_webdav_config_items(&app.filter)
}

pub(super) fn webdav_config_item_label(item: &WebDavConfigItem) -> &'static str {
    app::webdav_config_item_label(item)
}

pub(super) fn local_proxy_settings_item_label(item: &LocalProxySettingsItem) -> &'static str {
    match item {
        LocalProxySettingsItem::ListenAddress => texts::tui_settings_proxy_listen_address_label(),
        LocalProxySettingsItem::ListenPort => texts::tui_settings_proxy_listen_port_label(),
        LocalProxySettingsItem::AutoFailover => crate::t!("Automatic failover", "自动故障转移"),
    }
}

pub(super) fn ordered_visible_app_types(apps: &crate::settings::VisibleApps) -> Vec<AppType> {
    apps.ordered_enabled()
}

fn visible_apps_summary(apps: &crate::settings::VisibleApps) -> String {
    let labels = ordered_visible_app_types(apps)
        .into_iter()
        .map(|app_type| app_type.as_str().to_string())
        .collect::<Vec<_>>();

    if labels.is_empty() {
        texts::none().to_string()
    } else {
        labels.join(", ")
    }
}

pub(super) fn render_config(
    frame: &mut Frame<'_>,
    app: &App,
    _data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let items = config_items_filtered(app);
    let rows = items
        .iter()
        .map(|item| Row::new(vec![Cell::from(config_item_label(item))]));

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(texts::tui_config_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    if app.focus == Focus::Content {
        let mut keys = vec![("Enter", texts::tui_key_select())];
        if matches!(items.get(app.config_idx), Some(ConfigItem::CommonSnippet)) {
            keys.push(("e", texts::tui_key_edit_snippet()));
        }
        render_key_bar_center(frame, chunks[0], theme, &keys);
    }

    let table = Table::new(rows, [Constraint::Min(10)])
        .block(Block::default().borders(Borders::NONE))
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.config_idx));
    frame.render_stateful_widget(table, inset_left(chunks[1], CONTENT_INSET_LEFT), &mut state);
}

pub(super) fn render_config_webdav(
    frame: &mut Frame<'_>,
    app: &App,
    _data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let items = webdav_config_items_filtered(app);
    let rows = items
        .iter()
        .map(|item| Row::new(vec![Cell::from(webdav_config_item_label(item))]));

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(texts::tui_config_webdav_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    if app.focus == Focus::Content {
        let mut keys = vec![("Enter", texts::tui_key_select())];
        if matches!(
            items.get(app.config_webdav_idx),
            Some(WebDavConfigItem::Settings)
        ) {
            keys.push(("e", texts::tui_key_edit()));
        }
        render_key_bar_center(frame, chunks[0], theme, &keys);
    }

    let table = Table::new(rows, [Constraint::Min(10)])
        .block(Block::default().borders(Borders::NONE))
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.config_webdav_idx));
    frame.render_stateful_widget(table, inset_left(chunks[1], CONTENT_INSET_LEFT), &mut state);
}

pub(super) fn render_config_openclaw_route(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let Some(item) = ConfigItem::from_openclaw_route(&app.route) else {
        return;
    };

    let title = item
        .detail_title()
        .expect("OpenClaw config route should define a title");

    match item {
        ConfigItem::OpenClawEnv => render_openclaw_env_route(
            frame,
            app,
            area,
            theme,
            title,
            data.config.openclaw_env.as_ref(),
            data.config.openclaw_config_path.as_deref(),
            data.config.openclaw_warnings.as_deref(),
        ),
        ConfigItem::OpenClawTools => render_openclaw_tools_route(
            frame,
            app,
            data,
            area,
            theme,
            title,
            data.config.openclaw_config_path.as_deref(),
            data.config.openclaw_warnings.as_deref(),
        ),
        ConfigItem::OpenClawAgents => render_openclaw_agents_route(
            frame,
            app,
            data,
            area,
            theme,
            title,
            data.config.openclaw_config_path.as_deref(),
            data.config.openclaw_warnings.as_deref(),
        ),
        _ => {}
    }
}

fn wrapped_display_line_count(text: &str, width: u16) -> u16 {
    if width == 0 {
        return 1;
    }

    UnicodeWidthStr::width(text).max(1).div_ceil(width as usize) as u16
}

fn section_block_height(lines: &[String], text_width: u16) -> u16 {
    lines
        .iter()
        .map(|line| wrapped_display_line_count(line, text_width))
        .sum::<u16>()
        .saturating_add(2)
}

fn section_line_heights(lines: &[String], wraps: &[bool], text_width: u16) -> Vec<u16> {
    debug_assert_eq!(lines.len(), wraps.len());

    lines
        .iter()
        .zip(wraps.iter().copied())
        .map(|(line, wrap)| {
            if wrap {
                wrapped_display_line_count(line, text_width)
            } else {
                1
            }
        })
        .collect()
}

fn section_line_window(
    line_heights: &[u16],
    available_height: u16,
    selected_line: Option<usize>,
) -> std::ops::Range<usize> {
    if line_heights.is_empty() || available_height < 3 {
        return 0..0;
    }

    let inner_height = available_height.saturating_sub(2).max(1);
    let total_height = line_heights.iter().copied().sum::<u16>();
    if total_height <= inner_height {
        return 0..line_heights.len();
    }

    let selected_line = selected_line
        .filter(|index| *index < line_heights.len())
        .unwrap_or(0);
    let mut used = line_heights[selected_line].min(inner_height);
    let mut start = selected_line;
    while start > 0 {
        let next = line_heights[start - 1];
        if used + next > inner_height {
            break;
        }
        start -= 1;
        used += next;
    }

    let mut end = start;
    let mut consumed = 0;
    while end < line_heights.len() {
        let next = line_heights[end];
        if consumed + next > inner_height {
            break;
        }
        consumed += next;
        end += 1;
    }

    if end <= selected_line {
        end = (selected_line + 1).min(line_heights.len());
    }

    start..end
}

fn split_section_heights(
    available_height: u16,
    first_full_height: u16,
    second_full_height: u16,
    prioritize_second: bool,
) -> (u16, u16) {
    if first_full_height + second_full_height <= available_height {
        return (first_full_height, second_full_height);
    }

    let first_min = first_full_height.min(3);
    let second_min = second_full_height.min(3);

    if prioritize_second {
        if available_height < first_min + second_min {
            let second_height = second_min.min(available_height);
            return (
                available_height.saturating_sub(second_height),
                second_height,
            );
        }

        let second_height = second_full_height.min(available_height.saturating_sub(first_min));
        let first_height = first_full_height.min(available_height.saturating_sub(second_height));
        (first_height, second_height)
    } else {
        if available_height < first_min + second_min {
            let first_height = first_min.min(available_height);
            return (first_height, available_height.saturating_sub(first_height));
        }

        let first_height = first_full_height.min(available_height.saturating_sub(second_min));
        let second_height = second_full_height.min(available_height.saturating_sub(first_height));
        (first_height, second_height)
    }
}

fn render_warning_banner(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &super::theme::Theme,
    warnings: &[crate::openclaw_config::OpenClawHealthWarning],
) {
    let banner = warning_banner_lines(warnings).join("\n");
    frame.render_widget(
        Paragraph::new(banner)
            .style(Style::default().fg(theme.warn))
            .wrap(Wrap { trim: false }),
        inset_left(area, CONTENT_INSET_LEFT),
    );
}

fn warning_banner_lines(warnings: &[crate::openclaw_config::OpenClawHealthWarning]) -> Vec<String> {
    let mut lines = vec![texts::tui_openclaw_config_warning_title().to_string()];
    lines.extend(
        warnings
            .iter()
            .map(|warning| match warning.path.as_deref() {
                Some(path) => format!("- {} ({path})", warning.message),
                None => format!("- {}", warning.message),
            }),
    );
    lines
}

fn warning_banner_height(
    warnings: &[crate::openclaw_config::OpenClawHealthWarning],
    text_width: u16,
) -> u16 {
    warning_banner_lines(warnings)
        .iter()
        .map(|line| wrapped_display_line_count(line, text_width))
        .sum()
}

fn render_section_block(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &super::theme::Theme,
    title: Option<&str>,
    lines: &[String],
    emphasized: bool,
) {
    if area.width < 3 || area.height < 3 {
        return;
    }

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(if emphasized {
            theme.accent
        } else {
            theme.comment
        }));
    if let Some(title) = title {
        block = block.title(title);
    }
    frame.render_widget(block.clone(), area);
    frame.render_widget(
        Paragraph::new(lines.join("\n")).wrap(Wrap { trim: false }),
        inset_left(block.inner(area), 1),
    );
}

fn inline_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| other.to_string()),
    }
}

fn inline_env_value(key: &str, value: &Value) -> String {
    let mut map = serde_json::Map::new();
    map.insert(key.to_string(), value.clone());

    redact_sensitive_json(&Value::Object(map))
        .get(key)
        .map(inline_value)
        .unwrap_or_else(|| inline_value(value))
}

fn pad_display_width(text: &str, width: usize) -> String {
    let used = UnicodeWidthStr::width(text);
    if used >= width {
        return text.to_string();
    }

    format!("{text}{}", " ".repeat(width - used))
}

fn compact_two_column_lines(lines: &[String], total_width: u16) -> Option<Vec<String>> {
    if lines.len() != 4 {
        return None;
    }

    let gap = 4usize;
    let total_width = total_width as usize;
    let left_width = lines
        .iter()
        .step_by(2)
        .map(|line| UnicodeWidthStr::width(line.as_str()))
        .max()
        .unwrap_or(0);
    let right_width = lines
        .iter()
        .skip(1)
        .step_by(2)
        .map(|line| UnicodeWidthStr::width(line.as_str()))
        .max()
        .unwrap_or(0);

    if left_width + gap + right_width > total_width {
        return None;
    }

    Some(vec![
        format!(
            "{}{}",
            pad_display_width(&lines[0], left_width + gap),
            lines[1]
        ),
        format!(
            "{}{}",
            pad_display_width(&lines[2], left_width + gap),
            lines[3]
        ),
    ])
}

struct OpenClawEnvStyledRow {
    plain_text: String,
    line: Line<'static>,
}

fn openclaw_env_protected_value_style(theme: &super::theme::Theme) -> Style {
    if theme.no_color {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.warn).add_modifier(Modifier::BOLD)
    }
}

fn openclaw_env_row(
    theme: &super::theme::Theme,
    label_width: usize,
    key: &str,
    value: &str,
) -> OpenClawEnvStyledRow {
    let padded_key = pad_display_width(key, label_width);
    let plain_text = format!("  {padded_key}  {value}");
    let value_span = if value == redacted_secret_placeholder() {
        Span::styled(value.to_string(), openclaw_env_protected_value_style(theme))
    } else {
        Span::raw(value.to_string())
    };

    OpenClawEnvStyledRow {
        plain_text,
        line: Line::from(vec![
            Span::raw("  "),
            Span::raw(padded_key),
            Span::raw("  "),
            value_span,
        ]),
    }
}

fn openclaw_env_empty_row(theme: &super::theme::Theme) -> OpenClawEnvStyledRow {
    let text = format!("  {}", texts::tui_openclaw_config_env_empty());

    OpenClawEnvStyledRow {
        plain_text: text.clone(),
        line: Line::styled(text, Style::default().fg(theme.comment)),
    }
}

fn openclaw_env_section_block_height(rows: &[OpenClawEnvStyledRow], text_width: u16) -> u16 {
    rows.iter()
        .map(|row| wrapped_display_line_count(&row.plain_text, text_width))
        .sum::<u16>()
        .saturating_add(2)
}

fn render_openclaw_env_section_block(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &super::theme::Theme,
    rows: &[OpenClawEnvStyledRow],
) {
    if area.width < 3 || area.height < 3 {
        return;
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.comment));
    frame.render_widget(block.clone(), area);

    let inner = inset_left(block.inner(area), 1);
    if inner.width == 0 || inner.height == 0 || rows.is_empty() {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(rows.iter().map(|row| {
            Constraint::Length(wrapped_display_line_count(&row.plain_text, inner.width))
        }))
        .split(inner);

    for (row, chunk) in rows.iter().zip(chunks.into_iter()) {
        frame.render_widget(
            Paragraph::new(row.line.clone()).wrap(Wrap { trim: false }),
            *chunk,
        );
    }
}

fn append_json_lines(lines: &mut Vec<String>, value: &Value) {
    let pretty = serde_json::to_string_pretty(&redact_sensitive_json(value))
        .unwrap_or_else(|_| "{}".to_string());
    lines.extend(pretty.lines().map(|line| format!("  {line}")));
}

fn render_openclaw_env_route(
    frame: &mut Frame<'_>,
    app: &App,
    area: Rect,
    theme: &super::theme::Theme,
    title: &'static str,
    section: Option<&crate::openclaw_config::OpenClawEnvConfig>,
    config_path: Option<&std::path::Path>,
    warnings: Option<&[crate::openclaw_config::OpenClawHealthWarning]>,
) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(title);
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let config_path_display = config_path.map(|path| path.display().to_string());
    let section_warnings = warnings
        .unwrap_or_default()
        .iter()
        .filter(|warning| {
            openclaw_warning_matches_section(warning, "env.", config_path_display.as_deref())
        })
        .cloned()
        .collect::<Vec<_>>();
    let has_warnings = !section_warnings.is_empty();
    let warning_height = if has_warnings {
        warning_banner_height(
            &section_warnings,
            inner.width.saturating_sub(CONTENT_INSET_LEFT),
        )
    } else {
        0
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(warning_height),
            Constraint::Length(if has_warnings { 1 } else { 0 }),
            Constraint::Min(0),
        ])
        .split(inner);

    if app.focus == Focus::Content {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[
                ("Enter", texts::tui_key_edit()),
                ("e", texts::tui_key_edit()),
                ("Esc", texts::tui_key_close()),
            ],
        );
    }

    if has_warnings {
        render_warning_banner(frame, chunks[1], theme, &section_warnings);
    }

    let mut env_entries = section
        .map(|section| {
            section
                .vars
                .iter()
                .map(|(key, value)| (key.clone(), inline_env_value(key, value)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    env_entries.sort_by_key(|(key, _)| key.to_ascii_lowercase());
    let label_width = env_entries
        .iter()
        .map(|(key, _)| UnicodeWidthStr::width(key.as_str()))
        .max()
        .unwrap_or(0);
    let env_rows = if env_entries.is_empty() {
        vec![openclaw_env_empty_row(theme)]
    } else {
        env_entries
            .into_iter()
            .map(|(key, value)| openclaw_env_row(theme, label_width, &key, &value))
            .collect::<Vec<_>>()
    };
    let body_area = inset_left(chunks[3], CONTENT_INSET_LEFT);
    let section_text_width = body_area.width.saturating_sub(3);
    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(openclaw_env_section_block_height(
                &env_rows,
                section_text_width,
            )),
            Constraint::Min(0),
        ])
        .split(body_area);

    frame.render_widget(
        Paragraph::new(Line::styled(
            texts::tui_openclaw_config_env_description(),
            Style::default().fg(theme.comment),
        ))
        .wrap(Wrap { trim: false }),
        body[0],
    );
    render_openclaw_env_section_block(frame, body[2], theme, &env_rows);
}

struct OpenClawToolsStyledRow {
    plain_text: String,
    line: Line<'static>,
    wrap: bool,
}

fn openclaw_tools_selected_row_style(theme: &super::theme::Theme, selected: bool) -> Style {
    if selected {
        selection_style(theme)
    } else {
        Style::default()
    }
}

fn openclaw_tools_profile_row(
    theme: &super::theme::Theme,
    label: &str,
    value: &str,
    selected: bool,
) -> OpenClawToolsStyledRow {
    let plain_text = format!("{label}: {value}");
    let row_style = openclaw_tools_selected_row_style(theme, selected);
    let line = if selected {
        Line::styled(plain_text.clone(), row_style)
    } else {
        Line::from(vec![
            Span::styled(
                format!("{label}:"),
                Style::default()
                    .fg(theme.comment)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::raw(value.to_string()),
        ])
    };

    OpenClawToolsStyledRow {
        plain_text,
        line,
        wrap: true,
    }
}

fn openclaw_tools_section_label_row(
    theme: &super::theme::Theme,
    label: &str,
) -> OpenClawToolsStyledRow {
    OpenClawToolsStyledRow {
        plain_text: label.to_string(),
        line: Line::styled(
            label.to_string(),
            Style::default()
                .fg(theme.comment)
                .add_modifier(Modifier::BOLD),
        ),
        wrap: false,
    }
}

fn openclaw_tools_rule_row(
    theme: &super::theme::Theme,
    value: &str,
    selected: bool,
) -> OpenClawToolsStyledRow {
    let plain_text = value.to_string();
    let row_style = openclaw_tools_selected_row_style(theme, selected);

    OpenClawToolsStyledRow {
        plain_text: plain_text.clone(),
        line: if selected {
            Line::styled(plain_text, row_style)
        } else {
            Line::from(plain_text)
        },
        wrap: true,
    }
}

fn openclaw_tools_add_row(
    theme: &super::theme::Theme,
    label: &str,
    selected: bool,
) -> OpenClawToolsStyledRow {
    let plain_text = label.to_string();
    let row_style = openclaw_tools_selected_row_style(theme, selected);
    let plus_style = if selected {
        row_style
    } else if theme.no_color {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    };
    let label_style = if selected {
        row_style
    } else if theme.no_color {
        Style::default()
    } else {
        Style::default().fg(theme.cyan)
    };
    let (prefix, suffix) = label
        .strip_prefix("+ ")
        .map_or(("", label), |rest| ("+ ", rest));

    OpenClawToolsStyledRow {
        plain_text,
        line: if prefix.is_empty() {
            Line::styled(label.to_string(), label_style)
        } else {
            Line::from(vec![
                Span::styled(prefix.to_string(), plus_style),
                Span::styled(suffix.to_string(), label_style),
            ])
        },
        wrap: true,
    }
}

fn openclaw_tools_separator_row(theme: &super::theme::Theme) -> OpenClawToolsStyledRow {
    openclaw_tools_note_row("- ".repeat(128), Style::default().fg(theme.dim), false)
}

fn openclaw_tools_note_row(text: String, style: Style, wrap: bool) -> OpenClawToolsStyledRow {
    OpenClawToolsStyledRow {
        plain_text: text.clone(),
        line: Line::styled(text, style),
        wrap,
    }
}

fn openclaw_tools_section_block_height(rows: &[OpenClawToolsStyledRow], text_width: u16) -> u16 {
    section_line_heights(
        &rows
            .iter()
            .map(|row| row.plain_text.clone())
            .collect::<Vec<_>>(),
        &rows.iter().map(|row| row.wrap).collect::<Vec<_>>(),
        text_width,
    )
    .into_iter()
    .sum::<u16>()
    .saturating_add(2)
}

fn openclaw_tools_section_border_style(theme: &super::theme::Theme, primary: bool) -> Style {
    if primary {
        if theme.no_color {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.dim).add_modifier(Modifier::BOLD)
        }
    } else {
        Style::default().fg(theme.dim)
    }
}

fn openclaw_tools_section_title_style(theme: &super::theme::Theme, primary: bool) -> Style {
    if primary {
        if theme.no_color {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(theme.comment)
                .add_modifier(Modifier::BOLD)
        }
    } else {
        Style::default().fg(theme.comment)
    }
}

fn render_openclaw_tools_section_block(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &super::theme::Theme,
    title: Option<&str>,
    rows: &[OpenClawToolsStyledRow],
    primary: bool,
) {
    if area.width < 3 || area.height < 3 {
        return;
    }

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(openclaw_tools_section_border_style(theme, primary));
    if let Some(title) = title {
        block = block.title(Line::styled(
            title.to_string(),
            openclaw_tools_section_title_style(theme, primary),
        ));
    }
    frame.render_widget(block.clone(), area);

    let inner = inset_left(block.inner(area), 1);
    if inner.width == 0 || inner.height == 0 || rows.is_empty() {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(rows.iter().map(|row| {
            Constraint::Length(if row.wrap {
                wrapped_display_line_count(&row.plain_text, inner.width)
            } else {
                1
            })
        }))
        .split(inner);

    for (row, chunk) in rows.iter().zip(chunks.into_iter()) {
        let paragraph = if row.wrap {
            Paragraph::new(row.line.clone()).wrap(Wrap { trim: false })
        } else {
            Paragraph::new(row.line.clone())
        };
        frame.render_widget(paragraph, *chunk);
    }
}

fn render_openclaw_tools_route(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
    title: &'static str,
    config_path: Option<&std::path::Path>,
    warnings: Option<&[crate::openclaw_config::OpenClawHealthWarning]>,
) {
    let load_failed = app::openclaw_tools_load_failed(data);
    let form = (!load_failed).then(|| {
        app.openclaw_tools_form.clone().unwrap_or_else(|| {
            app::OpenClawToolsFormState::from_snapshot(data.config.openclaw_tools.as_ref())
        })
    });

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(title);
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let config_path_display = config_path.map(|path| path.display().to_string());
    let parse_warnings = warnings
        .unwrap_or_default()
        .iter()
        .filter(|warning| {
            warning.code == "config_parse_failed"
                && openclaw_warning_matches_section(
                    warning,
                    "tools.",
                    config_path_display.as_deref(),
                )
        })
        .cloned()
        .collect::<Vec<_>>();
    let has_parse_warning = !parse_warnings.is_empty();
    let warning_height = if has_parse_warning {
        warning_banner_height(
            &parse_warnings,
            inner.width.saturating_sub(CONTENT_INSET_LEFT),
        )
    } else {
        0
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(warning_height),
            Constraint::Min(0),
        ])
        .split(inner);

    if app.focus == Focus::Content {
        let key_bar_items = if load_failed {
            vec![("Esc", texts::tui_key_close())]
        } else {
            vec![
                ("Enter", texts::tui_key_edit()),
                ("e", texts::tui_key_edit()),
                ("Del/Backspace", texts::tui_key_delete()),
                ("Esc", texts::tui_key_close()),
            ]
        };
        render_key_bar_center(frame, chunks[0], theme, &key_bar_items);
    }

    if has_parse_warning {
        render_warning_banner(frame, chunks[1], theme, &parse_warnings);
    }

    let body_area = inset_left(chunks[2], CONTENT_INSET_LEFT);
    if load_failed {
        let message_lines = vec![texts::tui_openclaw_tools_load_failed_message().to_string()];
        let section_text_width = body_area.width.saturating_sub(3);
        let body = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(section_block_height(&message_lines, section_text_width)),
                Constraint::Min(0),
            ])
            .split(body_area);
        frame.render_widget(
            Paragraph::new(Line::styled(
                texts::tui_openclaw_tools_description(),
                Style::default().fg(theme.comment),
            ))
            .wrap(Wrap { trim: false }),
            body[0],
        );
        render_section_block(frame, body[1], theme, None, &message_lines, false);
        return;
    }

    let Some(form) = form.as_ref() else {
        return;
    };

    let is_selected =
        |section: app::OpenClawToolsSection, row: usize| form.section == section && form.row == row;

    let mut profile_rows = vec![openclaw_tools_profile_row(
        theme,
        texts::tui_openclaw_tools_profile_label(),
        &form.current_profile_label(),
        is_selected(app::OpenClawToolsSection::Profile, 0),
    )];
    if let Some(value) = form.unsupported_profile() {
        profile_rows.push(openclaw_tools_note_row(
            texts::tui_openclaw_tools_unsupported_profile_title().to_string(),
            Style::default().fg(theme.comment),
            true,
        ));
        profile_rows.push(openclaw_tools_note_row(
            texts::tui_openclaw_tools_unsupported_profile_description(value),
            Style::default().fg(theme.dim),
            true,
        ));
    }

    let mut rules_rows = vec![openclaw_tools_section_label_row(
        theme,
        texts::tui_openclaw_tools_allow_list_label(),
    )];
    let mut rules_selected_line = None;
    for (index, value) in form.allow.iter().enumerate() {
        if is_selected(app::OpenClawToolsSection::Allow, index) {
            rules_selected_line = Some(rules_rows.len());
        }
        rules_rows.push(openclaw_tools_rule_row(
            theme,
            value,
            is_selected(app::OpenClawToolsSection::Allow, index),
        ));
    }
    if is_selected(app::OpenClawToolsSection::Allow, form.allow.len()) {
        rules_selected_line = Some(rules_rows.len());
    }
    rules_rows.push(openclaw_tools_add_row(
        theme,
        texts::tui_openclaw_tools_add_allow_rule(),
        is_selected(app::OpenClawToolsSection::Allow, form.allow.len()),
    ));
    rules_rows.push(openclaw_tools_separator_row(theme));
    rules_rows.push(openclaw_tools_section_label_row(
        theme,
        texts::tui_openclaw_tools_deny_list_label(),
    ));
    for (index, value) in form.deny.iter().enumerate() {
        if is_selected(app::OpenClawToolsSection::Deny, index) {
            rules_selected_line = Some(rules_rows.len());
        }
        rules_rows.push(openclaw_tools_rule_row(
            theme,
            value,
            is_selected(app::OpenClawToolsSection::Deny, index),
        ));
    }
    if is_selected(app::OpenClawToolsSection::Deny, form.deny.len()) {
        rules_selected_line = Some(rules_rows.len());
    }
    rules_rows.push(openclaw_tools_add_row(
        theme,
        texts::tui_openclaw_tools_add_deny_rule(),
        is_selected(app::OpenClawToolsSection::Deny, form.deny.len()),
    ));
    if !form.extra.is_empty() {
        rules_rows.push(openclaw_tools_section_label_row(
            theme,
            texts::tui_openclaw_tools_extra_fields_label(),
        ));
        let mut extra_lines = Vec::new();
        append_json_lines(
            &mut extra_lines,
            &Value::Object(
                form.extra
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        );
        rules_rows.extend(
            extra_lines.into_iter().map(|line| {
                openclaw_tools_note_row(line, Style::default().fg(theme.comment), true)
            }),
        );
    }

    let section_text_width = body_area.width.saturating_sub(3);
    let profile_plain_lines = profile_rows
        .iter()
        .map(|row| row.plain_text.clone())
        .collect::<Vec<_>>();
    let profile_wraps = profile_rows.iter().map(|row| row.wrap).collect::<Vec<_>>();
    let profile_line_heights =
        section_line_heights(&profile_plain_lines, &profile_wraps, section_text_width);
    let profile_height = openclaw_tools_section_block_height(&profile_rows, section_text_width);
    let rules_plain_lines = rules_rows
        .iter()
        .map(|row| row.plain_text.clone())
        .collect::<Vec<_>>();
    let rules_wraps = rules_rows.iter().map(|row| row.wrap).collect::<Vec<_>>();
    let rules_line_heights =
        section_line_heights(&rules_plain_lines, &rules_wraps, section_text_width);
    let rules_height = openclaw_tools_section_block_height(&rules_rows, section_text_width);
    let remaining_height = body_area.height.saturating_sub(2);
    let (profile_height, rules_height) = split_section_heights(
        remaining_height,
        profile_height,
        rules_height,
        matches!(
            form.section,
            app::OpenClawToolsSection::Allow | app::OpenClawToolsSection::Deny
        ),
    );
    let profile_window = section_line_window(
        &profile_line_heights,
        profile_height,
        is_selected(app::OpenClawToolsSection::Profile, 0).then_some(0),
    );
    let visible_profile_rows = &profile_rows[profile_window];
    let rules_window = section_line_window(&rules_line_heights, rules_height, rules_selected_line);
    let visible_rules_rows = &rules_rows[rules_window];
    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(profile_height),
            Constraint::Length(rules_height),
            Constraint::Min(0),
        ])
        .split(body_area);

    frame.render_widget(
        Paragraph::new(Line::styled(
            texts::tui_openclaw_tools_description(),
            Style::default().fg(theme.comment),
        ))
        .wrap(Wrap { trim: false }),
        body[0],
    );
    render_openclaw_tools_section_block(
        frame,
        body[2],
        theme,
        Some(texts::tui_openclaw_tools_profile_block_title()),
        visible_profile_rows,
        false,
    );
    render_openclaw_tools_section_block(
        frame,
        body[3],
        theme,
        Some(texts::tui_openclaw_tools_rules_block_title()),
        visible_rules_rows,
        true,
    );
}

struct OpenClawAgentsStyledRow {
    plain_text: String,
    line: Line<'static>,
    wrap: bool,
}

fn openclaw_agents_plain_row_prefix(selected: bool) -> &'static str {
    let _ = selected;
    "  "
}

fn openclaw_agents_styled_row_prefix(
    theme: &super::theme::Theme,
    selected: bool,
    row_style: Style,
) -> Vec<Span<'static>> {
    let rail_style = if selected {
        if theme.no_color {
            row_style
        } else {
            Style::default().bg(theme.accent)
        }
    } else {
        Style::default()
    };

    vec![Span::styled(" ", rail_style), Span::styled(" ", row_style)]
}

fn openclaw_agents_selected_row_style(theme: &super::theme::Theme, selected: bool) -> Style {
    if !selected {
        return Style::default();
    }

    if theme.no_color {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default().bg(theme.surface)
    }
}

fn openclaw_agents_field_row(
    theme: &super::theme::Theme,
    label_width: usize,
    label: &str,
    value: &str,
    trailing_status: Option<&str>,
    selected: bool,
    wrap: bool,
) -> OpenClawAgentsStyledRow {
    let label_padding = " ".repeat(
        label_width
            .saturating_sub(UnicodeWidthStr::width(label))
            .saturating_add(0),
    );
    let mut plain_text = format!(
        "{}{label}:{label_padding} {value}",
        openclaw_agents_plain_row_prefix(selected)
    );
    let trailing_status = trailing_status.filter(|status| !status.trim().is_empty());
    if let Some(status) = trailing_status {
        plain_text.push_str(" (");
        plain_text.push_str(status);
        plain_text.push(')');
    }

    let row_style = openclaw_agents_selected_row_style(theme, selected);
    let label_style = if selected && theme.no_color {
        row_style
    } else {
        row_style.fg(ratatui::style::Color::White)
    };
    let value_style = if selected && theme.no_color {
        row_style
    } else {
        row_style.fg(theme.cyan)
    };
    let status_style = if selected && theme.no_color {
        row_style
    } else {
        row_style.fg(theme.comment)
    };
    let mut spans = openclaw_agents_styled_row_prefix(theme, selected, row_style);
    spans.extend([
        Span::styled(label.to_string(), label_style),
        Span::styled(":", label_style),
        Span::styled(label_padding, row_style),
        Span::styled(" ", row_style),
        Span::styled(value.to_string(), value_style),
    ]);
    if let Some(status) = trailing_status {
        spans.push(Span::styled(" (", row_style));
        spans.push(Span::styled(status.to_string(), status_style));
        spans.push(Span::styled(
            ")",
            row_style.fg(status_style.fg.unwrap_or(theme.comment)),
        ));
    }

    OpenClawAgentsStyledRow {
        plain_text,
        line: Line::from(spans),
        wrap,
    }
}

fn openclaw_agents_action_row(
    theme: &super::theme::Theme,
    label_width: usize,
    label: &str,
    selected: bool,
) -> OpenClawAgentsStyledRow {
    let action_indent = " ".repeat(label_width.saturating_add(2));
    let plain_text = format!(
        "{}{action_indent}+ {label}",
        openclaw_agents_plain_row_prefix(selected)
    );
    let row_style = openclaw_agents_selected_row_style(theme, selected);
    let plus_style = if selected && theme.no_color {
        row_style
    } else {
        row_style.fg(theme.accent).add_modifier(Modifier::BOLD)
    };
    let label_style = if selected && theme.no_color {
        row_style
    } else {
        row_style.fg(theme.cyan)
    };

    OpenClawAgentsStyledRow {
        plain_text,
        line: Line::from({
            let mut spans = openclaw_agents_styled_row_prefix(theme, selected, row_style);
            spans.extend([
                Span::styled(action_indent, row_style),
                Span::styled("+ ", plus_style),
                Span::styled(label.to_string(), label_style),
            ]);
            spans
        }),
        wrap: false,
    }
}

fn openclaw_agents_disabled_row(
    theme: &super::theme::Theme,
    label_width: usize,
    value: &str,
) -> OpenClawAgentsStyledRow {
    let value_indent = " ".repeat(label_width.saturating_add(2));
    let plain_text = format!("  {value_indent}{value}");

    OpenClawAgentsStyledRow {
        plain_text,
        line: Line::from(vec![
            Span::raw("  "),
            Span::raw(value_indent),
            Span::styled(value.to_string(), Style::default().fg(theme.comment)),
        ]),
        wrap: false,
    }
}

fn openclaw_agents_note_row(text: String, wrap: bool) -> OpenClawAgentsStyledRow {
    OpenClawAgentsStyledRow {
        plain_text: text.clone(),
        line: Line::from(text),
        wrap,
    }
}

fn openclaw_agents_section_block_height(rows: &[OpenClawAgentsStyledRow], text_width: u16) -> u16 {
    section_line_heights(
        &rows
            .iter()
            .map(|row| row.plain_text.clone())
            .collect::<Vec<_>>(),
        &rows.iter().map(|row| row.wrap).collect::<Vec<_>>(),
        text_width,
    )
    .into_iter()
    .sum::<u16>()
    .saturating_add(2)
}

fn render_openclaw_agents_section_block(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &super::theme::Theme,
    title: Option<&str>,
    rows: &[OpenClawAgentsStyledRow],
) {
    if area.width < 3 || area.height < 3 {
        return;
    }

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.dim));
    if let Some(title) = title {
        block = block.title(Line::styled(
            title.to_string(),
            Style::default().fg(theme.comment),
        ));
    }
    frame.render_widget(block.clone(), area);

    let inner = inset_left(block.inner(area), 1);
    if inner.width == 0 || inner.height == 0 || rows.is_empty() {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(rows.iter().map(|row| {
            Constraint::Length(if row.wrap {
                wrapped_display_line_count(&row.plain_text, inner.width)
            } else {
                1
            })
        }))
        .split(inner);

    for (row, chunk) in rows.iter().zip(chunks.into_iter()) {
        let paragraph = if row.wrap {
            Paragraph::new(row.line.clone()).wrap(Wrap { trim: false })
        } else {
            Paragraph::new(row.line.clone())
        };
        frame.render_widget(paragraph, *chunk);
    }
}

fn render_openclaw_agents_route(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
    title: &'static str,
    config_path: Option<&std::path::Path>,
    warnings: Option<&[crate::openclaw_config::OpenClawHealthWarning]>,
) {
    let load_failed = app::openclaw_agents_load_failed(data);
    let form = (!load_failed).then(|| {
        app.openclaw_agents_form.clone().unwrap_or_else(|| {
            app::OpenClawAgentsFormState::from_snapshot(
                data.config.openclaw_agents_defaults.as_ref(),
            )
        })
    });
    let model_options = app::openclaw_agents_model_options(data);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(title);
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let config_path_display = config_path.map(|path| path.display().to_string());
    let parse_warnings = warnings
        .unwrap_or_default()
        .iter()
        .filter(|warning| {
            warning.code == "config_parse_failed"
                && openclaw_warning_matches_section(
                    warning,
                    "agents.defaults.",
                    config_path_display.as_deref(),
                )
        })
        .cloned()
        .collect::<Vec<_>>();
    let has_parse_warning = !parse_warnings.is_empty();
    let warning_height = if has_parse_warning {
        warning_banner_height(
            &parse_warnings,
            inner.width.saturating_sub(CONTENT_INSET_LEFT),
        )
    } else {
        0
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(warning_height),
            Constraint::Min(0),
        ])
        .split(inner);

    if app.focus == Focus::Content {
        let key_bar_items = if load_failed {
            vec![("Esc", texts::tui_key_close())]
        } else {
            vec![
                ("Enter", texts::tui_key_edit()),
                ("Del", texts::tui_key_delete()),
                ("Esc", texts::tui_key_close()),
            ]
        };
        render_key_bar_center(frame, chunks[0], theme, &key_bar_items);
    }

    if has_parse_warning {
        render_warning_banner(frame, chunks[1], theme, &parse_warnings);
    }

    let body_area = inset_left(chunks[2], CONTENT_INSET_LEFT);
    if load_failed {
        let message_rows = vec![openclaw_agents_note_row(
            texts::tui_openclaw_agents_load_failed_message().to_string(),
            true,
        )];
        let section_text_width = body_area.width.saturating_sub(3);
        let body = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(openclaw_agents_section_block_height(
                    &message_rows,
                    section_text_width,
                )),
                Constraint::Min(0),
            ])
            .split(body_area);
        frame.render_widget(
            Paragraph::new(Line::styled(
                texts::tui_openclaw_agents_description(),
                Style::default().fg(theme.comment),
            ))
            .wrap(Wrap { trim: false }),
            body[0],
        );
        render_openclaw_agents_section_block(frame, body[2], theme, None, &message_rows);
        return;
    }

    let Some(form) = form.as_ref() else {
        return;
    };

    let is_selected = |section: app::OpenClawAgentsSection, row: usize| {
        form.section == section && form.row == row
    };
    let available_fallback_options = form.available_fallback_options(&model_options);

    let field_label_width = [
        texts::tui_openclaw_agents_primary_model(),
        texts::tui_openclaw_agents_fallback_models(),
        texts::tui_openclaw_agents_workspace(),
        texts::tui_openclaw_agents_timeout(),
        texts::tui_openclaw_agents_context_tokens(),
        texts::tui_openclaw_agents_max_concurrent(),
    ]
    .into_iter()
    .map(UnicodeWidthStr::width)
    .max()
    .unwrap_or(0);

    let (primary_value, primary_status) =
        openclaw_agents_model_value_parts(&form.primary_model, &model_options);
    let mut model_rows = vec![openclaw_agents_field_row(
        theme,
        field_label_width,
        texts::tui_openclaw_agents_primary_model(),
        &primary_value,
        primary_status,
        is_selected(app::OpenClawAgentsSection::PrimaryModel, 0),
        false,
    )];
    let mut model_selected_line =
        is_selected(app::OpenClawAgentsSection::PrimaryModel, 0).then_some(0);
    for (index, value) in form.fallbacks.iter().enumerate() {
        let (fallback_value, fallback_status) =
            openclaw_agents_model_value_parts(value, &model_options);
        if is_selected(app::OpenClawAgentsSection::FallbackModels, index) {
            model_selected_line = Some(model_rows.len());
        }
        model_rows.push(openclaw_agents_field_row(
            theme,
            field_label_width,
            texts::tui_openclaw_agents_fallback_models(),
            &fallback_value,
            fallback_status,
            is_selected(app::OpenClawAgentsSection::FallbackModels, index),
            false,
        ));
    }
    if available_fallback_options.is_empty() {
        model_rows.push(openclaw_agents_disabled_row(
            theme,
            field_label_width,
            texts::tui_openclaw_agents_add_fallback_disabled(),
        ));
    } else {
        if is_selected(
            app::OpenClawAgentsSection::FallbackModels,
            form.fallbacks.len(),
        ) {
            model_selected_line = Some(model_rows.len());
        }
        model_rows.push(openclaw_agents_action_row(
            theme,
            field_label_width,
            texts::tui_openclaw_agents_add_fallback(),
            is_selected(
                app::OpenClawAgentsSection::FallbackModels,
                form.fallbacks.len(),
            ),
        ));
    }
    if !form.model_extra.is_empty() {
        model_rows.push(openclaw_agents_note_row(String::new(), false));
        model_rows.push(openclaw_agents_note_row(
            texts::tui_openclaw_agents_preserved_fields_label().to_string(),
            true,
        ));
        let mut model_extra_lines = Vec::new();
        append_json_lines(
            &mut model_extra_lines,
            &Value::Object(
                form.model_extra
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        );
        model_rows.extend(
            model_extra_lines
                .into_iter()
                .map(|line| openclaw_agents_note_row(line, true)),
        );
    }

    let section_text_width = body_area.width.saturating_sub(3);
    let mut runtime_rows = Vec::new();
    let mut runtime_selected_line = None;
    fn push_runtime_row(rows: &mut Vec<OpenClawAgentsStyledRow>, row: OpenClawAgentsStyledRow) {
        rows.push(row);
    }
    if form.has_legacy_timeout {
        push_runtime_row(
            &mut runtime_rows,
            openclaw_agents_note_row(
                texts::tui_openclaw_agents_legacy_timeout_title().to_string(),
                true,
            ),
        );
        push_runtime_row(
            &mut runtime_rows,
            openclaw_agents_note_row(
                format!(
                    "  {}",
                    if form.has_unmigratable_legacy_timeout() {
                        texts::tui_openclaw_agents_legacy_timeout_invalid_description()
                    } else {
                        texts::tui_openclaw_agents_legacy_timeout_description()
                    }
                ),
                true,
            ),
        );
        push_runtime_row(
            &mut runtime_rows,
            openclaw_agents_note_row(String::new(), false),
        );
    }

    for (row, label, value) in [
        (
            0,
            texts::tui_openclaw_agents_workspace(),
            openclaw_agents_runtime_value(&form.workspace, None),
        ),
        (
            1,
            texts::tui_openclaw_agents_timeout(),
            openclaw_agents_runtime_value(&form.timeout, form.preserved_timeout_seconds()),
        ),
        (
            2,
            texts::tui_openclaw_agents_context_tokens(),
            openclaw_agents_runtime_value(&form.context_tokens, form.preserved_context_tokens()),
        ),
        (
            3,
            texts::tui_openclaw_agents_max_concurrent(),
            openclaw_agents_runtime_value(&form.max_concurrent, form.preserved_max_concurrent()),
        ),
    ] {
        if is_selected(app::OpenClawAgentsSection::Runtime, row) {
            runtime_selected_line = Some(runtime_rows.len());
        }
        push_runtime_row(
            &mut runtime_rows,
            openclaw_agents_field_row(
                theme,
                field_label_width,
                label,
                &format!("[{value}]"),
                None,
                is_selected(app::OpenClawAgentsSection::Runtime, row),
                false,
            ),
        );
    }
    if form.has_preserved_non_string_runtime_values() {
        push_runtime_row(
            &mut runtime_rows,
            openclaw_agents_note_row(String::new(), false),
        );
        push_runtime_row(
            &mut runtime_rows,
            openclaw_agents_note_row(
                texts::tui_openclaw_agents_preserved_runtime_notice().to_string(),
                true,
            ),
        );
    }
    if !form.defaults_extra.is_empty() {
        push_runtime_row(
            &mut runtime_rows,
            openclaw_agents_note_row(String::new(), false),
        );
        push_runtime_row(
            &mut runtime_rows,
            openclaw_agents_note_row(
                texts::tui_openclaw_agents_preserved_fields_label().to_string(),
                true,
            ),
        );
        let mut defaults_extra_lines = Vec::new();
        append_json_lines(
            &mut defaults_extra_lines,
            &Value::Object(
                form.defaults_extra
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        );
        runtime_rows.extend(
            defaults_extra_lines
                .into_iter()
                .map(|line| openclaw_agents_note_row(line, true)),
        );
    }

    let model_plain_lines = model_rows
        .iter()
        .map(|row| row.plain_text.clone())
        .collect::<Vec<_>>();
    let model_wraps = model_rows.iter().map(|row| row.wrap).collect::<Vec<_>>();
    let model_line_heights =
        section_line_heights(&model_plain_lines, &model_wraps, section_text_width);
    let model_height = openclaw_agents_section_block_height(&model_rows, section_text_width);
    let runtime_plain_lines = runtime_rows
        .iter()
        .map(|row| row.plain_text.clone())
        .collect::<Vec<_>>();
    let runtime_wraps = runtime_rows.iter().map(|row| row.wrap).collect::<Vec<_>>();
    let runtime_line_heights =
        section_line_heights(&runtime_plain_lines, &runtime_wraps, section_text_width);
    let runtime_height = openclaw_agents_section_block_height(&runtime_rows, section_text_width);
    let remaining_height = body_area.height.saturating_sub(2);
    let (model_height, runtime_height) = split_section_heights(
        remaining_height,
        model_height,
        runtime_height,
        form.section == app::OpenClawAgentsSection::Runtime,
    );
    let model_window = section_line_window(&model_line_heights, model_height, model_selected_line);
    let visible_model_rows = &model_rows[model_window.clone()];
    let runtime_window =
        section_line_window(&runtime_line_heights, runtime_height, runtime_selected_line);
    let visible_runtime_rows = &runtime_rows[runtime_window];

    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(model_height),
            Constraint::Length(runtime_height),
            Constraint::Min(0),
        ])
        .split(body_area);

    frame.render_widget(
        Paragraph::new(Line::styled(
            texts::tui_openclaw_agents_description(),
            Style::default().fg(theme.comment),
        ))
        .wrap(Wrap { trim: false }),
        body[0],
    );
    render_openclaw_agents_section_block(
        frame,
        body[2],
        theme,
        Some(texts::tui_openclaw_agents_model_section()),
        visible_model_rows,
    );
    render_openclaw_agents_section_block(
        frame,
        body[3],
        theme,
        Some(texts::tui_openclaw_agents_runtime_section()),
        visible_runtime_rows,
    );
}

fn openclaw_agents_model_value_parts(
    value: &str,
    options: &[app::OpenClawModelOption],
) -> (String, Option<&'static str>) {
    if value.trim().is_empty() {
        return (texts::tui_openclaw_agents_not_set().to_string(), None);
    }

    options
        .iter()
        .find(|option| option.value == value)
        .map(|option| (option.label.clone(), None))
        .unwrap_or_else(|| {
            (
                value.to_string(),
                Some(texts::tui_openclaw_agents_not_configured_suffix()),
            )
        })
}

fn openclaw_agents_runtime_value(value: &str, preserved: Option<&Value>) -> String {
    if value.trim().is_empty() {
        preserved
            .map(|raw| texts::tui_openclaw_agents_preserved_non_standard_value(&raw.to_string()))
            .unwrap_or_else(|| texts::tui_openclaw_agents_not_set().to_string())
    } else {
        value.to_string()
    }
}

fn openclaw_warning_matches_section(
    warning: &crate::openclaw_config::OpenClawHealthWarning,
    warning_prefix: &str,
    config_path: Option<&str>,
) -> bool {
    match warning.path.as_deref() {
        None => true,
        Some(path) if config_path == Some(path) => true,
        Some(path) => {
            let section_root = warning_prefix.trim_end_matches('.');
            path == section_root || path.starts_with(warning_prefix)
        }
    }
}

pub(super) fn render_openclaw_workspace_routes(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    match app.route {
        Route::ConfigOpenClawWorkspace => {
            render_openclaw_workspace(frame, app, data, area, theme);
        }
        Route::ConfigOpenClawDailyMemory => {
            render_openclaw_daily_memory(frame, app, data, area, theme);
        }
        _ => {}
    }
}

struct OpenClawWorkspaceStyledRow {
    plain_text: String,
    line: Line<'static>,
    wraps: bool,
}

fn openclaw_workspace_row_height(row: &OpenClawWorkspaceStyledRow, text_width: u16) -> u16 {
    if row.wraps {
        wrapped_display_line_count(&row.plain_text, text_width)
    } else {
        1
    }
}

fn openclaw_workspace_summary_height(rows: &[OpenClawWorkspaceStyledRow], text_width: u16) -> u16 {
    rows.iter()
        .map(|row| openclaw_workspace_row_height(row, text_width))
        .sum()
}

fn openclaw_workspace_section_block_height(
    rows: &[OpenClawWorkspaceStyledRow],
    text_width: u16,
) -> u16 {
    openclaw_workspace_summary_height(rows, text_width).saturating_add(2)
}

fn openclaw_workspace_section_border_style(theme: &super::theme::Theme, primary: bool) -> Style {
    let mut style = Style::default().fg(if primary { theme.comment } else { theme.dim });
    if primary {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

fn openclaw_workspace_meta_row(
    theme: &super::theme::Theme,
    label: &str,
    value: String,
    selected: bool,
    subdued: bool,
) -> OpenClawWorkspaceStyledRow {
    let plain_text = format!("  {label}: {value}");
    let line = if selected {
        Line::styled(plain_text.clone(), selection_style(theme))
    } else {
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{label}:"),
                Style::default()
                    .fg(theme.comment)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                value,
                if subdued {
                    Style::default().fg(theme.comment)
                } else {
                    Style::default()
                },
            ),
        ])
    };

    OpenClawWorkspaceStyledRow {
        plain_text,
        line,
        wraps: true,
    }
}

fn openclaw_workspace_file_row(
    theme: &super::theme::Theme,
    filename_width: usize,
    filename: &str,
    exists: bool,
    selected: bool,
) -> OpenClawWorkspaceStyledRow {
    let status = if exists {
        texts::tui_openclaw_workspace_status_exists()
    } else {
        texts::tui_openclaw_workspace_status_missing()
    };
    let padded_filename = pad_display_width(filename, filename_width);
    let plain_text = format!("  {padded_filename}  {status}");
    let line = if selected {
        Line::styled(plain_text.clone(), selection_style(theme))
    } else {
        let status_style = if exists {
            Style::default().fg(theme.ok).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.comment)
        };

        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                padded_filename,
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(status.to_string(), status_style),
        ])
    };

    OpenClawWorkspaceStyledRow {
        plain_text,
        line,
        wraps: false,
    }
}

fn openclaw_workspace_note_row(
    theme: &super::theme::Theme,
    note: String,
) -> OpenClawWorkspaceStyledRow {
    let plain_text = format!("  {note}");

    OpenClawWorkspaceStyledRow {
        plain_text: plain_text.clone(),
        line: Line::styled(plain_text, Style::default().fg(theme.comment)),
        wraps: true,
    }
}

fn openclaw_workspace_visible_row_window(
    row_count: usize,
    selected_row: Option<usize>,
    available_height: u16,
) -> std::ops::Range<usize> {
    if row_count == 0 || available_height < 3 {
        return 0..0;
    }

    let visible_rows = available_height.saturating_sub(2) as usize;
    if row_count <= visible_rows {
        return 0..row_count;
    }

    let selected_row = selected_row.filter(|index| *index < row_count).unwrap_or(0);
    let end = (selected_row + 1).max(visible_rows).min(row_count);
    let start = end.saturating_sub(visible_rows);
    start..end
}

fn openclaw_workspace_body_heights(
    available_height: u16,
    summary_full_height: u16,
    files_full_height: u16,
    daily_full_height: u16,
    prioritize_daily: bool,
) -> (u16, u16, u16) {
    const MIN_SECTION_HEIGHT: u16 = 3;

    if available_height == 0 {
        return (0, 0, 0);
    }

    let prioritized_min = if prioritize_daily {
        daily_full_height.min(MIN_SECTION_HEIGHT)
    } else {
        files_full_height.min(MIN_SECTION_HEIGHT)
    };
    let summary_height = summary_full_height.min(available_height.saturating_sub(prioritized_min));
    let remaining = available_height.saturating_sub(summary_height);
    if remaining == 0 {
        return (summary_height, 0, 0);
    }

    if remaining >= files_full_height.saturating_add(daily_full_height) {
        return (summary_height, files_full_height, daily_full_height);
    }

    let files_min = files_full_height.min(MIN_SECTION_HEIGHT);
    let daily_min = daily_full_height.min(MIN_SECTION_HEIGHT);

    if remaining < files_min.saturating_add(daily_min) {
        if prioritize_daily {
            return (summary_height, 0, remaining);
        }

        return (summary_height, remaining, 0);
    }

    let mut files_height = files_min;
    let mut daily_height = daily_min;
    let mut extra = remaining.saturating_sub(files_height.saturating_add(daily_height));
    let mut files_need = files_full_height.saturating_sub(files_height);
    let mut daily_need = daily_full_height.saturating_sub(daily_height);

    while extra > 0 && (files_need > 0 || daily_need > 0) {
        openclaw_workspace_allocate_extra_line(
            prioritize_daily,
            &mut extra,
            &mut files_height,
            &mut files_need,
            &mut daily_height,
            &mut daily_need,
        );
        openclaw_workspace_allocate_extra_line(
            !prioritize_daily,
            &mut extra,
            &mut files_height,
            &mut files_need,
            &mut daily_height,
            &mut daily_need,
        );
    }

    (summary_height, files_height, daily_height)
}

fn openclaw_workspace_allocate_extra_line(
    prefer_daily: bool,
    extra: &mut u16,
    files_height: &mut u16,
    files_need: &mut u16,
    daily_height: &mut u16,
    daily_need: &mut u16,
) {
    let (height, need) = if prefer_daily {
        (daily_height, daily_need)
    } else {
        (files_height, files_need)
    };

    if *extra == 0 || *need == 0 {
        return;
    }

    *height = height.saturating_add(1);
    *need = need.saturating_sub(1);
    *extra = extra.saturating_sub(1);
}

fn render_openclaw_workspace_section_block(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &super::theme::Theme,
    title: Option<&str>,
    primary: bool,
    rows: &[OpenClawWorkspaceStyledRow],
) {
    if area.width < 3 || area.height < 3 {
        return;
    }

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(openclaw_workspace_section_border_style(theme, primary));
    if let Some(title) = title {
        block = block.title(title);
    }
    frame.render_widget(block.clone(), area);

    let inner = inset_left(block.inner(area), 1);
    if inner.width == 0 || inner.height == 0 || rows.is_empty() {
        return;
    }

    let mut y = inner.y;
    let limit = inner.y.saturating_add(inner.height);
    for row in rows {
        if y >= limit {
            break;
        }

        let row_height = openclaw_workspace_row_height(row, inner.width);
        let available_height = limit.saturating_sub(y);
        let render_height = row_height.min(available_height);
        let row_area = Rect::new(inner.x, y, inner.width, render_height);
        let paragraph = if row.wraps {
            Paragraph::new(row.line.clone()).wrap(Wrap { trim: false })
        } else {
            Paragraph::new(row.line.clone())
        };
        frame.render_widget(paragraph, row_area);
        y = y.saturating_add(render_height);
    }
}

fn render_openclaw_workspace_summary(
    frame: &mut Frame<'_>,
    area: Rect,
    rows: &[OpenClawWorkspaceStyledRow],
) {
    if area.width == 0 || area.height == 0 || rows.is_empty() {
        return;
    }

    let mut y = area.y;
    let limit = area.y.saturating_add(area.height);
    for row in rows {
        if y >= limit {
            break;
        }

        let row_height = openclaw_workspace_row_height(row, area.width);
        let available_height = limit.saturating_sub(y);
        let render_height = row_height.min(available_height);
        let row_area = Rect::new(area.x, y, area.width, render_height);
        let paragraph = if row.wraps {
            Paragraph::new(row.line.clone()).wrap(Wrap { trim: false })
        } else {
            Paragraph::new(row.line.clone())
        };
        frame.render_widget(paragraph, row_area);
        y = y.saturating_add(render_height);
    }
}

fn render_openclaw_workspace(
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
        .title(texts::tui_openclaw_workspace_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    if app.focus == Focus::Content {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[
                ("Enter", texts::tui_key_open()),
                ("o", texts::tui_key_open_directory()),
            ],
        );
    }

    let max_filename_len = crate::commands::workspace::ALLOWED_FILES
        .iter()
        .map(|f| f.len())
        .max()
        .unwrap_or(0);
    let workspace_summary_rows = vec![openclaw_workspace_meta_row(
        theme,
        texts::tui_openclaw_workspace_directory_label(),
        data.config
            .openclaw_workspace
            .directory_path
            .display()
            .to_string(),
        false,
        false,
    )];
    let workspace_file_rows = crate::commands::workspace::ALLOWED_FILES
        .iter()
        .enumerate()
        .map(|(index, filename)| {
            let exists = data
                .config
                .openclaw_workspace
                .file_exists
                .get(*filename)
                .copied()
                .unwrap_or(false);
            openclaw_workspace_file_row(
                theme,
                max_filename_len,
                filename,
                exists,
                app.workspace_idx == index,
            )
        })
        .collect::<Vec<_>>();

    let mut daily_memory_rows = vec![openclaw_workspace_meta_row(
        theme,
        texts::tui_openclaw_workspace_daily_memory_label(),
        texts::tui_openclaw_workspace_daily_memory_count(
            data.config.openclaw_workspace.daily_memory_files.len(),
        ),
        app.workspace_idx == crate::commands::workspace::ALLOWED_FILES.len(),
        false,
    )];
    daily_memory_rows.push(openclaw_workspace_meta_row(
        theme,
        texts::tui_openclaw_daily_memory_directory_label(),
        data.config
            .openclaw_workspace
            .directory_path
            .join("memory")
            .display()
            .to_string(),
        false,
        true,
    ));
    if let Some(latest) = data.config.openclaw_workspace.daily_memory_files.first() {
        daily_memory_rows.push(openclaw_workspace_note_row(
            theme,
            format!("{}  {}", latest.filename, latest.preview),
        ));
    }

    let body_area = inset_left(chunks[1], CONTENT_INSET_LEFT);
    let summary_text_width = body_area.width;
    let section_text_width = body_area.width.saturating_sub(3);
    let summary_full_height =
        openclaw_workspace_summary_height(&workspace_summary_rows, summary_text_width);
    let files_full_height =
        openclaw_workspace_section_block_height(&workspace_file_rows, section_text_width);
    let daily_full_height =
        openclaw_workspace_section_block_height(&daily_memory_rows, section_text_width);
    let (summary_height, files_height, daily_height) = openclaw_workspace_body_heights(
        body_area.height,
        summary_full_height,
        files_full_height,
        daily_full_height,
        app.workspace_idx == crate::commands::workspace::ALLOWED_FILES.len(),
    );
    let visible_file_window = openclaw_workspace_visible_row_window(
        workspace_file_rows.len(),
        (app.workspace_idx < workspace_file_rows.len()).then_some(app.workspace_idx),
        files_height,
    );
    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(summary_height),
            Constraint::Length(files_height),
            Constraint::Length(daily_height),
            Constraint::Min(0),
        ])
        .split(body_area);

    render_openclaw_workspace_summary(frame, body[0], &workspace_summary_rows);
    render_openclaw_workspace_section_block(
        frame,
        body[1],
        theme,
        Some(texts::tui_openclaw_workspace_files_block_title()),
        true,
        &workspace_file_rows[visible_file_window],
    );
    render_openclaw_workspace_section_block(
        frame,
        body[2],
        theme,
        Some(texts::tui_openclaw_workspace_daily_memory_label()),
        false,
        &daily_memory_rows,
    );
}

fn render_openclaw_daily_memory(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let using_search = !app.openclaw_daily_memory_search_query.trim().is_empty();
    let rows = if using_search {
        app.openclaw_daily_memory_search_results
            .iter()
            .map(|row| {
                Row::new(vec![
                    Cell::from(row.filename.clone()),
                    Cell::from(row.snippet.clone()),
                ])
            })
            .collect::<Vec<_>>()
    } else {
        data.config
            .openclaw_workspace
            .daily_memory_files
            .iter()
            .map(|row| {
                Row::new(vec![
                    Cell::from(row.filename.clone()),
                    Cell::from(row.preview.clone()),
                ])
            })
            .collect::<Vec<_>>()
    };

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(texts::tui_openclaw_daily_memory_title());
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

    if app.focus == Focus::Content {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[
                ("Enter", texts::tui_key_open()),
                ("a", texts::tui_key_create()),
                ("d", texts::tui_key_delete()),
                ("o", texts::tui_key_open_directory()),
            ],
        );
    }

    frame.render_widget(
        Paragraph::new(format!(
            "{}: {}",
            texts::tui_openclaw_daily_memory_directory_label(),
            data.config
                .openclaw_workspace
                .directory_path
                .join("memory")
                .display()
        ))
        .wrap(Wrap { trim: false }),
        inset_left(chunks[1], CONTENT_INSET_LEFT),
    );

    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(if using_search {
                texts::tui_openclaw_daily_memory_search_empty()
            } else {
                texts::tui_openclaw_daily_memory_empty()
            })
            .style(Style::default().fg(theme.dim))
            .wrap(Wrap { trim: false }),
            inset_left(chunks[2], CONTENT_INSET_LEFT),
        );
        return;
    }

    let table = Table::new(rows, [Constraint::Length(18), Constraint::Min(10)])
        .block(Block::default().borders(Borders::NONE))
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));
    let mut state = TableState::default();
    state.select(Some(app.daily_memory_idx));
    frame.render_stateful_widget(table, inset_left(chunks[2], CONTENT_INSET_LEFT), &mut state);
}

pub(super) fn render_settings(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let language = crate::cli::i18n::current_language();
    let visible_apps = crate::settings::get_visible_apps();
    let openclaw_config_dir = crate::settings::get_settings().openclaw_config_dir;
    let skip_claude_onboarding = crate::settings::get_skip_claude_onboarding();
    let claude_plugin_integration = crate::settings::get_enable_claude_plugin_integration();

    let rows_data = super::app::SettingsItem::ALL
        .iter()
        .map(|item| match item {
            super::app::SettingsItem::Language => (
                texts::tui_settings_header_language().to_string(),
                language.display_name().to_string(),
            ),
            super::app::SettingsItem::VisibleApps => (
                texts::tui_settings_visible_apps_label().to_string(),
                visible_apps_summary(&visible_apps),
            ),
            super::app::SettingsItem::OpenClawConfigDir => (
                texts::tui_settings_openclaw_config_dir_label().to_string(),
                openclaw_config_dir.clone().unwrap_or_else(|| {
                    texts::tui_settings_openclaw_config_dir_default_value().to_string()
                }),
            ),
            super::app::SettingsItem::SkipClaudeOnboarding => (
                texts::skip_claude_onboarding_label().to_string(),
                if skip_claude_onboarding {
                    texts::enabled().to_string()
                } else {
                    texts::disabled().to_string()
                },
            ),
            super::app::SettingsItem::ClaudePluginIntegration => (
                texts::enable_claude_plugin_integration_label().to_string(),
                if claude_plugin_integration {
                    texts::enabled().to_string()
                } else {
                    texts::disabled().to_string()
                },
            ),
            super::app::SettingsItem::Proxy => (
                texts::tui_config_item_proxy().to_string(),
                format!(
                    "{}:{}",
                    data.proxy.configured_listen_address, data.proxy.configured_listen_port,
                ),
            ),
            super::app::SettingsItem::CheckForUpdates => (
                texts::tui_settings_check_for_updates().to_string(),
                format!("v{}", env!("CARGO_PKG_VERSION")),
            ),
        })
        .collect::<Vec<_>>();

    let label_col_width = field_label_column_width(
        rows_data
            .iter()
            .map(|(label, _value)| label.as_str())
            .chain(std::iter::once(texts::tui_settings_header_setting())),
        0,
    );

    let header = Row::new(vec![
        Cell::from(texts::tui_settings_header_setting()),
        Cell::from(texts::tui_settings_header_value()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let rows = rows_data
        .iter()
        .map(|(label, value)| Row::new(vec![Cell::from(label.clone()), Cell::from(value.clone())]));

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(texts::menu_settings());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    if app.focus == Focus::Content {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[("Enter", texts::tui_key_apply())],
        );
    }

    let table = Table::new(
        rows,
        [Constraint::Length(label_col_width), Constraint::Min(10)],
    )
    .header(header)
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.settings_idx));
    frame.render_stateful_widget(table, inset_left(chunks[1], CONTENT_INSET_LEFT), &mut state);
}

pub(super) fn render_settings_proxy(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let rows_data = LocalProxySettingsItem::ALL
        .iter()
        .map(|item| match item {
            LocalProxySettingsItem::ListenAddress => (
                local_proxy_settings_item_label(item).to_string(),
                data.proxy.configured_listen_address.clone(),
            ),
            LocalProxySettingsItem::ListenPort => (
                local_proxy_settings_item_label(item).to_string(),
                data.proxy.configured_listen_port.to_string(),
            ),
            LocalProxySettingsItem::AutoFailover => (
                local_proxy_settings_item_label(item).to_string(),
                if data.proxy.auto_failover_enabled {
                    texts::enabled().to_string()
                } else {
                    texts::disabled().to_string()
                },
            ),
        })
        .collect::<Vec<_>>();

    let label_col_width = field_label_column_width(
        rows_data
            .iter()
            .map(|(label, _value)| label.as_str())
            .chain(std::iter::once(texts::tui_settings_header_setting())),
        0,
    );

    let header = Row::new(vec![
        Cell::from(texts::tui_settings_header_setting()),
        Cell::from(texts::tui_settings_header_value()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let rows = rows_data
        .iter()
        .map(|(label, value)| Row::new(vec![Cell::from(label.clone()), Cell::from(value.clone())]));

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(texts::tui_settings_proxy_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .split(inner);

    if app.focus == Focus::Content {
        let key_label = match LocalProxySettingsItem::ALL.get(app.settings_proxy_idx) {
            Some(LocalProxySettingsItem::AutoFailover) => texts::tui_key_toggle(),
            _ if data.proxy.running => "",
            _ => texts::tui_key_edit(),
        };
        if !key_label.is_empty() {
            render_key_bar_center(frame, chunks[0], theme, &[("Enter", key_label)]);
        }
    }

    let table = Table::new(
        rows,
        [Constraint::Length(label_col_width), Constraint::Min(10)],
    )
    .header(header)
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.settings_proxy_idx));
    frame.render_stateful_widget(table, inset_left(chunks[1], CONTENT_INSET_LEFT), &mut state);

    frame.render_widget(
        Paragraph::new(if data.proxy.running {
            texts::tui_settings_proxy_stop_before_edit_hint()
        } else {
            texts::tui_settings_proxy_restart_hint()
        })
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.dim)),
        chunks[2],
    );
}
