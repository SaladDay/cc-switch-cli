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

fn section_block_height_mixed(lines: &[String], wraps: &[bool], text_width: u16) -> u16 {
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

fn render_section_block_mixed(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &super::theme::Theme,
    title: Option<&str>,
    lines: &[String],
    wraps: &[bool],
    emphasized: bool,
) {
    if area.width < 3 || area.height < 3 {
        return;
    }

    debug_assert_eq!(lines.len(), wraps.len());

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

    let inner = inset_left(block.inner(area), 1);
    if inner.width == 0 || inner.height == 0 || lines.is_empty() {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(lines.iter().zip(wraps.iter().copied()).map(|(line, wrap)| {
            Constraint::Length(if wrap {
                wrapped_display_line_count(line, inner.width)
            } else {
                1
            })
        }))
        .split(inner);

    for ((line, wrap), chunk) in lines
        .iter()
        .zip(wraps.iter().copied())
        .zip(chunks.into_iter())
    {
        let paragraph = if wrap {
            Paragraph::new(line.clone()).wrap(Wrap { trim: false })
        } else {
            Paragraph::new(line.clone())
        };
        frame.render_widget(paragraph, *chunk);
    }
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

    let mut env_rows = section
        .map(|section| {
            section
                .vars
                .iter()
                .map(|(key, value)| format!("  {key}: {}", inline_env_value(key, value)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    env_rows.sort_by_key(|line| line.to_ascii_lowercase());
    if env_rows.is_empty() {
        env_rows.push(format!("  {}", texts::none()));
    }
    let body_area = inset_left(chunks[2], CONTENT_INSET_LEFT);
    let section_text_width = body_area.width.saturating_sub(3);
    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(section_block_height(&env_rows, section_text_width)),
            Constraint::Min(0),
        ])
        .split(body_area);

    frame.render_widget(
        Paragraph::new(texts::tui_openclaw_config_env_description()).wrap(Wrap { trim: false }),
        body[0],
    );
    render_section_block(frame, body[1], theme, None, &env_rows, false);
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
            Paragraph::new(texts::tui_openclaw_tools_description()).wrap(Wrap { trim: false }),
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
    let nested_row = |selected: bool, value: String| {
        if selected {
            format!("  > {value}")
        } else {
            format!("    {value}")
        }
    };

    let mut profile_lines = vec![nested_row(
        is_selected(app::OpenClawToolsSection::Profile, 0),
        format!(
            "{}: {}",
            texts::tui_openclaw_tools_profile_label(),
            form.current_profile_label()
        ),
    )];
    if let Some(value) = form.unsupported_profile() {
        profile_lines.push(String::new());
        profile_lines.push(texts::tui_openclaw_tools_unsupported_profile_title().to_string());
        profile_lines.push(format!(
            "    {}",
            texts::tui_openclaw_tools_unsupported_profile_description(value)
        ));
    }

    let mut rules_lines = vec![texts::tui_openclaw_tools_allow_list_label().to_string()];
    let mut rules_selected_line = None;
    for (index, value) in form.allow.iter().enumerate() {
        if is_selected(app::OpenClawToolsSection::Allow, index) {
            rules_selected_line = Some(rules_lines.len());
        }
        rules_lines.push(nested_row(
            is_selected(app::OpenClawToolsSection::Allow, index),
            value.clone(),
        ));
    }
    if is_selected(app::OpenClawToolsSection::Allow, form.allow.len()) {
        rules_selected_line = Some(rules_lines.len());
    }
    rules_lines.push(nested_row(
        is_selected(app::OpenClawToolsSection::Allow, form.allow.len()),
        texts::tui_openclaw_tools_add_allow_rule().to_string(),
    ));
    rules_lines.push(String::new());
    rules_lines.push(texts::tui_openclaw_tools_deny_list_label().to_string());
    for (index, value) in form.deny.iter().enumerate() {
        if is_selected(app::OpenClawToolsSection::Deny, index) {
            rules_selected_line = Some(rules_lines.len());
        }
        rules_lines.push(nested_row(
            is_selected(app::OpenClawToolsSection::Deny, index),
            value.clone(),
        ));
    }
    if is_selected(app::OpenClawToolsSection::Deny, form.deny.len()) {
        rules_selected_line = Some(rules_lines.len());
    }
    rules_lines.push(nested_row(
        is_selected(app::OpenClawToolsSection::Deny, form.deny.len()),
        texts::tui_openclaw_tools_add_deny_rule().to_string(),
    ));
    if !form.extra.is_empty() {
        rules_lines.push(String::new());
        rules_lines.push(texts::tui_openclaw_tools_extra_fields_label().to_string());
        append_json_lines(
            &mut rules_lines,
            &Value::Object(
                form.extra
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        );
    }

    let section_text_width = body_area.width.saturating_sub(3);
    let profile_height = section_block_height(&profile_lines, section_text_width);
    let rules_wraps = vec![true; rules_lines.len()];
    let rules_line_heights = section_line_heights(&rules_lines, &rules_wraps, section_text_width);
    let rules_height = rules_line_heights
        .iter()
        .copied()
        .sum::<u16>()
        .saturating_add(2);
    let remaining_height = body_area.height.saturating_sub(1);
    let (profile_height, rules_height) = split_section_heights(
        remaining_height,
        profile_height,
        rules_height,
        matches!(
            form.section,
            app::OpenClawToolsSection::Allow | app::OpenClawToolsSection::Deny
        ),
    );
    let rules_window = section_line_window(&rules_line_heights, rules_height, rules_selected_line);
    let visible_rules_lines = &rules_lines[rules_window.clone()];
    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(profile_height),
            Constraint::Length(rules_height),
            Constraint::Min(0),
        ])
        .split(body_area);

    frame.render_widget(
        Paragraph::new(texts::tui_openclaw_tools_description()).wrap(Wrap { trim: false }),
        body[0],
    );
    render_section_block(
        frame,
        body[1],
        theme,
        Some(texts::tui_openclaw_tools_profile_block_title()),
        &profile_lines,
        false,
    );
    render_section_block(
        frame,
        body[2],
        theme,
        Some(texts::tui_openclaw_tools_rules_block_title()),
        visible_rules_lines,
        false,
    );
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
        let message_lines = vec![texts::tui_openclaw_agents_load_failed_message().to_string()];
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
            Paragraph::new(texts::tui_openclaw_agents_description()).wrap(Wrap { trim: false }),
            body[0],
        );
        render_section_block(frame, body[1], theme, None, &message_lines, false);
        return;
    }

    let Some(form) = form.as_ref() else {
        return;
    };

    let is_selected = |section: app::OpenClawAgentsSection, row: usize| {
        form.section == section && form.row == row
    };
    let inline_runtime_row = |selected: bool, label: &str, value: String| {
        let row = format!("{label}: [{value}]");
        if selected {
            format!("  > {row}")
        } else {
            format!("    {row}")
        }
    };
    let action_row = |selected: bool, value: &str| {
        if selected {
            format!("  > + {value}")
        } else {
            format!("    + {value}")
        }
    };
    let inline_model_row = |selected: bool, label: &str, value: String| {
        let row = format!("{label}: {value}");
        if selected {
            format!("  > {row}")
        } else {
            format!("    {row}")
        }
    };
    let disabled_row = |value: &str| format!("    {value}");
    let available_fallback_options = form.available_fallback_options(&model_options);

    let mut model_lines = vec![inline_model_row(
        is_selected(app::OpenClawAgentsSection::PrimaryModel, 0),
        texts::tui_openclaw_agents_primary_model(),
        openclaw_agents_model_label(&form.primary_model, &model_options),
    )];
    let mut model_selected_line =
        is_selected(app::OpenClawAgentsSection::PrimaryModel, 0).then_some(0);
    for (index, value) in form.fallbacks.iter().enumerate() {
        if is_selected(app::OpenClawAgentsSection::FallbackModels, index) {
            model_selected_line = Some(model_lines.len());
        }
        model_lines.push(inline_model_row(
            is_selected(app::OpenClawAgentsSection::FallbackModels, index),
            texts::tui_openclaw_agents_fallback_models(),
            openclaw_agents_model_label(value, &model_options),
        ));
    }
    if available_fallback_options.is_empty() {
        model_lines.push(disabled_row(
            texts::tui_openclaw_agents_add_fallback_disabled(),
        ));
    } else {
        if is_selected(
            app::OpenClawAgentsSection::FallbackModels,
            form.fallbacks.len(),
        ) {
            model_selected_line = Some(model_lines.len());
        }
        model_lines.push(action_row(
            is_selected(
                app::OpenClawAgentsSection::FallbackModels,
                form.fallbacks.len(),
            ),
            texts::tui_openclaw_agents_add_fallback(),
        ));
    }
    if !form.model_extra.is_empty() {
        model_lines.push(String::new());
        model_lines.push(texts::tui_openclaw_agents_preserved_fields_label().to_string());
        append_json_lines(
            &mut model_lines,
            &Value::Object(
                form.model_extra
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        );
    }

    let section_text_width = body_area.width.saturating_sub(3);
    let mut runtime_lines = Vec::new();
    let mut runtime_wraps = Vec::new();
    let mut runtime_selected_line = None;
    fn push_runtime_line(lines: &mut Vec<String>, wraps: &mut Vec<bool>, line: String, wrap: bool) {
        lines.push(line);
        wraps.push(wrap);
    }
    if form.has_legacy_timeout {
        push_runtime_line(
            &mut runtime_lines,
            &mut runtime_wraps,
            texts::tui_openclaw_agents_legacy_timeout_title().to_string(),
            true,
        );
        push_runtime_line(
            &mut runtime_lines,
            &mut runtime_wraps,
            format!(
                "  {}",
                if form.has_unmigratable_legacy_timeout() {
                    texts::tui_openclaw_agents_legacy_timeout_invalid_description()
                } else {
                    texts::tui_openclaw_agents_legacy_timeout_description()
                }
            ),
            true,
        );
        push_runtime_line(&mut runtime_lines, &mut runtime_wraps, String::new(), true);
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
            runtime_selected_line = Some(runtime_lines.len());
        }
        push_runtime_line(
            &mut runtime_lines,
            &mut runtime_wraps,
            inline_runtime_row(
                is_selected(app::OpenClawAgentsSection::Runtime, row),
                label,
                value,
            ),
            false,
        );
    }
    if form.has_preserved_non_string_runtime_values() {
        push_runtime_line(&mut runtime_lines, &mut runtime_wraps, String::new(), true);
        push_runtime_line(
            &mut runtime_lines,
            &mut runtime_wraps,
            texts::tui_openclaw_agents_preserved_runtime_notice().to_string(),
            true,
        );
    }
    if !form.defaults_extra.is_empty() {
        push_runtime_line(&mut runtime_lines, &mut runtime_wraps, String::new(), true);
        push_runtime_line(
            &mut runtime_lines,
            &mut runtime_wraps,
            texts::tui_openclaw_agents_preserved_fields_label().to_string(),
            true,
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
        for line in defaults_extra_lines {
            push_runtime_line(&mut runtime_lines, &mut runtime_wraps, line, true);
        }
    }

    let model_wraps = vec![true; model_lines.len()];
    let model_line_heights = section_line_heights(&model_lines, &model_wraps, section_text_width);
    let model_height = model_line_heights
        .iter()
        .copied()
        .sum::<u16>()
        .saturating_add(2);
    let runtime_line_heights =
        section_line_heights(&runtime_lines, &runtime_wraps, section_text_width);
    let runtime_height = runtime_line_heights
        .iter()
        .copied()
        .sum::<u16>()
        .saturating_add(2);
    let remaining_height = body_area.height.saturating_sub(1);
    let (model_height, runtime_height) = split_section_heights(
        remaining_height,
        model_height,
        runtime_height,
        form.section == app::OpenClawAgentsSection::Runtime,
    );
    let model_window = section_line_window(&model_line_heights, model_height, model_selected_line);
    let visible_model_lines = &model_lines[model_window.clone()];
    let runtime_window =
        section_line_window(&runtime_line_heights, runtime_height, runtime_selected_line);
    let visible_runtime_lines = &runtime_lines[runtime_window.clone()];
    let visible_runtime_wraps = &runtime_wraps[runtime_window];

    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(model_height),
            Constraint::Length(runtime_height),
            Constraint::Min(0),
        ])
        .split(body_area);

    frame.render_widget(
        Paragraph::new(texts::tui_openclaw_agents_description()).wrap(Wrap { trim: false }),
        body[0],
    );
    render_section_block(
        frame,
        body[1],
        theme,
        Some(texts::tui_openclaw_agents_model_section()),
        visible_model_lines,
        false,
    );
    render_section_block_mixed(
        frame,
        body[2],
        theme,
        Some(texts::tui_openclaw_agents_runtime_section()),
        visible_runtime_lines,
        visible_runtime_wraps,
        false,
    );
}

fn openclaw_agents_model_label(value: &str, options: &[app::OpenClawModelOption]) -> String {
    if value.trim().is_empty() {
        return texts::tui_openclaw_agents_not_set().to_string();
    }

    options
        .iter()
        .find(|option| option.value == value)
        .map(|option| option.label.clone())
        .unwrap_or_else(|| texts::tui_openclaw_agents_not_in_list(value))
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

    let selected_workspace = |index: usize| {
        if app.workspace_idx == index {
            "> "
        } else {
            "  "
        }
    };
    let selected_daily_memory =
        if app.workspace_idx == crate::commands::workspace::ALLOWED_FILES.len() {
            "> "
        } else {
            "  "
        };

    let mut workspace_lines = vec![format!(
        "  {}: {}",
        texts::tui_openclaw_workspace_directory_label(),
        data.config.openclaw_workspace.directory_path.display()
    )];
    workspace_lines.push(String::new());
    for (index, filename) in crate::commands::workspace::ALLOWED_FILES.iter().enumerate() {
        let exists = data
            .config
            .openclaw_workspace
            .file_exists
            .get(*filename)
            .copied()
            .unwrap_or(false);
        workspace_lines.push(format!(
            "{}{filename}  {}",
            selected_workspace(index),
            if exists {
                texts::tui_openclaw_workspace_status_exists()
            } else {
                texts::tui_openclaw_workspace_status_missing()
            }
        ));
    }

    let mut daily_memory_lines = vec![format!(
        "{}{}: {}",
        selected_daily_memory,
        texts::tui_openclaw_workspace_daily_memory_label(),
        texts::tui_openclaw_workspace_daily_memory_count(
            data.config.openclaw_workspace.daily_memory_files.len(),
        )
    )];
    daily_memory_lines.push(format!(
        "  {}: {}",
        texts::tui_openclaw_daily_memory_directory_label(),
        data.config
            .openclaw_workspace
            .directory_path
            .join("memory")
            .display()
    ));
    if let Some(latest) = data.config.openclaw_workspace.daily_memory_files.first() {
        daily_memory_lines.push(format!("  {}  {}", latest.filename, latest.preview));
    }
    let body_area = inset_left(chunks[1], CONTENT_INSET_LEFT);
    let section_text_width = body_area.width.saturating_sub(3);
    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(section_block_height(&workspace_lines, section_text_width)),
            Constraint::Length(section_block_height(
                &daily_memory_lines,
                section_text_width,
            )),
            Constraint::Min(0),
        ])
        .split(body_area);

    render_section_block(
        frame,
        body[0],
        theme,
        Some(texts::tui_openclaw_workspace_files_block_title()),
        &workspace_lines,
        false,
    );
    render_section_block(
        frame,
        body[1],
        theme,
        Some(texts::tui_openclaw_workspace_daily_memory_label()),
        &daily_memory_lines,
        false,
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

    if app.focus == Focus::Content && !data.proxy.running {
        render_key_bar_center(frame, chunks[0], theme, &[("Enter", texts::tui_key_edit())]);
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
