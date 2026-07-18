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

    let mut keys = vec![("Enter", texts::tui_key_select())];
    if matches!(items.get(app.config_idx), Some(ConfigItem::CommonSnippet)) {
        keys.push(("e", texts::tui_key_edit_snippet()));
    }
    let body = render_page_frame(
        frame,
        area,
        theme,
        app,
        texts::tui_config_title(),
        &keys,
        None,
    );

    let table = Table::new(rows, [Constraint::Min(10)])
        .block(Block::default().borders(Borders::NONE))
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.config_idx));
    frame.render_stateful_widget(table, inset_left(body, CONTENT_INSET_LEFT), &mut state);
}

pub(super) fn render_config_webdav(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let items = webdav_config_items_filtered(app);
    let configured = data.config.webdav_sync.is_some();
    let enabled = data
        .config
        .webdav_sync
        .as_ref()
        .is_some_and(|settings| settings.enabled);
    let rows = items.iter().map(|item| {
        let label = match item {
            WebDavConfigItem::EnableDisable if enabled => texts::tui_config_item_webdav_disable(),
            _ => webdav_config_item_label(item),
        };
        let style = if item.available(configured, enabled) {
            Style::default()
        } else {
            Style::default().fg(theme.dim)
        };
        Row::new(vec![Cell::from(label)]).style(style)
    });

    let keys = vec![("Enter", texts::tui_key_select())];
    let body = render_page_frame(
        frame,
        area,
        theme,
        app,
        &breadcrumb_path(&[
            texts::tui_config_title(),
            texts::tui_config_cloud_sync_title(),
            texts::tui_config_webdav_title(),
        ]),
        &keys,
        None,
    );

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(0)])
        .split(body);
    render_webdav_sync_summary(frame, data, chunks[0], theme);

    let table = Table::new(rows, [Constraint::Min(10)])
        .block(Block::default().borders(Borders::NONE))
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.config_webdav_idx));
    frame.render_stateful_widget(table, inset_left(chunks[1], CONTENT_INSET_LEFT), &mut state);
}

pub(super) fn render_config_cloud_sync(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let body = render_page_frame(
        frame,
        area,
        theme,
        app,
        &breadcrumb_path(&[
            texts::tui_config_title(),
            texts::tui_config_cloud_sync_title(),
        ]),
        &[("Enter", texts::tui_key_manage())],
        None,
    );

    let rows = CloudSyncBackend::ALL.iter().copied().map(|backend| {
        let (status, style) = cloud_backend_status(backend, data, theme);
        Row::new(vec![
            Cell::from(backend.label()),
            Cell::from(status).style(style),
        ])
    });
    let table = Table::new(rows, [Constraint::Length(22), Constraint::Min(10)])
        .header(
            Row::new(vec![
                Cell::from(texts::tui_cloud_sync_backend()),
                Cell::from(texts::tui_cloud_sync_status()),
            ])
            .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD)),
        )
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));
    let mut state = TableState::default();
    state.select(Some(
        app.config_cloud_sync_idx
            .min(CloudSyncBackend::ALL.len().saturating_sub(1)),
    ));
    frame.render_stateful_widget(table, inset_left(body, CONTENT_INSET_LEFT), &mut state);
}

pub(super) fn render_config_s3(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let body = render_page_frame(
        frame,
        area,
        theme,
        app,
        &breadcrumb_path(&[
            texts::tui_config_title(),
            texts::tui_config_cloud_sync_title(),
            texts::tui_config_s3_title(),
        ]),
        &[("Enter", texts::tui_key_select())],
        None,
    );
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Min(0)])
        .split(body);
    render_s3_sync_summary(frame, data, chunks[0], theme);

    let configured = data.config.s3_sync.is_some();
    let enabled = data
        .config
        .s3_sync
        .as_ref()
        .is_some_and(|settings| settings.enabled);
    let rows = S3ConfigItem::ALL.iter().copied().map(|item| {
        let available = item.available(configured, enabled);
        let style = if available {
            Style::default()
        } else {
            Style::default().fg(theme.dim)
        };
        Row::new(vec![Cell::from(item.label(enabled))]).style(style)
    });
    let table = Table::new(rows, [Constraint::Min(10)])
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));
    let mut state = TableState::default();
    state.select(Some(
        app.config_s3_idx
            .min(S3ConfigItem::ALL.len().saturating_sub(1)),
    ));
    frame.render_stateful_widget(table, inset_left(chunks[1], CONTENT_INSET_LEFT), &mut state);
}

fn cloud_backend_status(
    backend: CloudSyncBackend,
    data: &UiData,
    theme: &super::theme::Theme,
) -> (String, Style) {
    let (configured, enabled, has_error) = match backend {
        CloudSyncBackend::WebDav => data
            .config
            .webdav_sync
            .as_ref()
            .map_or((false, false, false), |settings| {
                (true, settings.enabled, settings.status.last_error.is_some())
            }),
        CloudSyncBackend::S3Compatible => data
            .config
            .s3_sync
            .as_ref()
            .map_or((false, false, false), |settings| {
                (true, settings.enabled, settings.status.last_error.is_some())
            }),
    };
    if !configured {
        return (
            texts::tui_webdav_status_not_configured().to_string(),
            Style::default().fg(theme.dim),
        );
    }
    if !enabled {
        return (
            texts::tui_cloud_sync_disabled().to_string(),
            Style::default().fg(theme.dim),
        );
    }
    if has_error {
        (
            texts::tui_webdav_status_error().to_string(),
            Style::default().fg(theme.warn),
        )
    } else {
        (
            texts::tui_cloud_sync_enabled().to_string(),
            Style::default().fg(theme.ok),
        )
    }
}

fn render_webdav_sync_summary(
    frame: &mut Frame<'_>,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let settings = data.config.webdav_sync.as_ref();
    let state = settings.map_or_else(
        || texts::tui_webdav_status_not_configured().to_string(),
        |settings| {
            if settings.enabled {
                texts::tui_cloud_sync_enabled().to_string()
            } else {
                texts::tui_cloud_sync_disabled().to_string()
            }
        },
    );
    let last_sync = settings
        .and_then(|settings| settings.status.last_sync_at)
        .and_then(format_sync_time_local_to_minute)
        .unwrap_or_else(|| texts::tui_webdav_status_never_synced().to_string());
    let remote = settings.map_or_else(
        || texts::tui_na().to_string(),
        |settings| format!("{}/v2/db-v6/{}", settings.remote_root, settings.profile),
    );
    render_cloud_summary_lines(frame, area, theme, &state, &last_sync, &remote);
}

fn render_s3_sync_summary(
    frame: &mut Frame<'_>,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let settings = data.config.s3_sync.as_ref();
    let state = settings.map_or_else(
        || texts::tui_webdav_status_not_configured().to_string(),
        |settings| {
            if settings.enabled {
                texts::tui_cloud_sync_enabled().to_string()
            } else {
                texts::tui_cloud_sync_disabled().to_string()
            }
        },
    );
    let last_sync = settings
        .and_then(|settings| settings.status.last_sync_at)
        .and_then(format_sync_time_local_to_minute)
        .unwrap_or_else(|| texts::tui_webdav_status_never_synced().to_string());
    let remote = settings.map_or_else(
        || texts::tui_na().to_string(),
        |settings| {
            format!(
                "{}/{}/v2/db-v6/{}",
                settings.bucket, settings.remote_root, settings.profile
            )
        },
    );
    render_cloud_summary_lines(frame, area, theme, &state, &last_sync, &remote);
}

fn render_cloud_summary_lines(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &super::theme::Theme,
    state: &str,
    last_sync: &str,
    remote: &str,
) {
    let label_width = usize::from(field_label_column_width(
        [
            texts::tui_label_webdav_status(),
            texts::tui_label_webdav_last_sync(),
            texts::tui_cloud_sync_remote_path(),
        ],
        0,
    ));
    let available = area
        .width
        .saturating_sub(u16::try_from(label_width).unwrap_or(u16::MAX))
        .saturating_sub(2);
    let lines = vec![
        kv_line(
            theme,
            texts::tui_label_webdav_status(),
            label_width,
            vec![Span::raw(state.to_string())],
        ),
        kv_line(
            theme,
            texts::tui_label_webdav_last_sync(),
            label_width,
            vec![Span::raw(last_sync.to_string())],
        ),
        kv_line(
            theme,
            texts::tui_cloud_sync_remote_path(),
            label_width,
            vec![Span::raw(truncate_to_display_width(remote, available))],
        ),
    ];
    frame.render_widget(Paragraph::new(lines), inset_left(area, CONTENT_INSET_LEFT));
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
    if text.is_empty() {
        return 1;
    }

    saturating_line_height_sum(text.lines().map(|line| {
        let wrapped = UnicodeWidthStr::width(line)
            .max(1)
            .div_ceil(usize::from(width));
        u16::try_from(wrapped).unwrap_or(u16::MAX)
    }))
}

fn saturating_line_height_sum(heights: impl IntoIterator<Item = u16>) -> u16 {
    heights
        .into_iter()
        .fold(0_u16, |total, height| total.saturating_add(height))
}

fn line_heights_fit(available_height: u16, heights: impl IntoIterator<Item = u16>) -> bool {
    let available_height = usize::from(available_height);
    heights
        .into_iter()
        .map(usize::from)
        .try_fold(0usize, |total, height| {
            let next = total.saturating_add(height);
            (next <= available_height).then_some(next)
        })
        .is_some()
}

fn section_block_height(lines: &[String], text_width: u16) -> u16 {
    saturating_line_height_sum(
        lines
            .iter()
            .map(|line| wrapped_display_line_count(line, text_width)),
    )
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
    let total_height = saturating_line_height_sum(line_heights.iter().copied());
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
        if used.saturating_add(next) > inner_height {
            break;
        }
        start -= 1;
        used = used.saturating_add(next);
    }

    let mut end = start;
    let mut consumed = 0_u16;
    while end < line_heights.len() {
        let next = line_heights[end];
        if consumed.saturating_add(next) > inner_height {
            break;
        }
        consumed = consumed.saturating_add(next);
        end += 1;
    }

    if end <= selected_line {
        end = selected_line.saturating_add(1).min(line_heights.len());
    }

    start..end
}

fn split_section_heights(
    available_height: u16,
    first_full_height: u16,
    second_full_height: u16,
    prioritize_second: bool,
) -> (u16, u16) {
    if line_heights_fit(available_height, [first_full_height, second_full_height]) {
        return (first_full_height, second_full_height);
    }

    let first_min = first_full_height.min(3);
    let second_min = second_full_height.min(3);

    if prioritize_second {
        if available_height < first_min.saturating_add(second_min) {
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
        if available_height < first_min.saturating_add(second_min) {
            let first_height = first_min.min(available_height);
            return (first_height, available_height.saturating_sub(first_height));
        }

        let first_height = first_full_height.min(available_height.saturating_sub(second_min));
        let second_height = second_full_height.min(available_height.saturating_sub(first_height));
        (first_height, second_height)
    }
}

const OPENCLAW_WARNING_PREVIEW_ITEMS: usize = 32;

struct OpenClawWarningPreview<'a> {
    items: Vec<&'a crate::openclaw_config::OpenClawHealthWarning>,
    truncated: bool,
}

impl<'a> OpenClawWarningPreview<'a> {
    fn collect(
        warnings: impl IntoIterator<Item = &'a crate::openclaw_config::OpenClawHealthWarning>,
    ) -> Self {
        let mut warnings = warnings.into_iter();
        let items = warnings
            .by_ref()
            .take(OPENCLAW_WARNING_PREVIEW_ITEMS)
            .collect::<Vec<_>>();
        let truncated = warnings.next().is_some();
        Self { items, truncated }
    }

    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

fn render_warning_banner(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &super::theme::Theme,
    warnings: &OpenClawWarningPreview<'_>,
) {
    let banner = warning_banner_lines(warnings).join("\n");
    frame.render_widget(
        Paragraph::new(banner)
            .style(Style::default().fg(theme.warn))
            .wrap(Wrap { trim: false }),
        inset_left(area, CONTENT_INSET_LEFT),
    );
}

fn warning_banner_lines(warnings: &OpenClawWarningPreview<'_>) -> Vec<String> {
    let mut lines = vec![texts::tui_openclaw_config_warning_title().to_string()];
    lines.extend(warnings.items.iter().map(|warning| {
        let message = bounded_trimmed_text_for_display(&warning.message);
        match warning.path.as_deref() {
            Some(path) => {
                let path = bounded_trimmed_text_for_display(path);
                format!("- {message} ({path})")
            }
            None => format!("- {message}"),
        }
    }));
    if warnings.truncated {
        lines.push(crate::t!(
            "… more warnings".to_string(),
            "… 还有更多警告".to_string()
        ));
    }
    lines
}

fn warning_banner_height(warnings: &OpenClawWarningPreview<'_>, text_width: u16) -> u16 {
    saturating_line_height_sum(
        warning_banner_lines(warnings)
            .iter()
            .map(|line| wrapped_display_line_count(line, text_width)),
    )
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
        block = block.title(format!(" {} ", title));
    }
    frame.render_widget(block.clone(), area);
    frame.render_widget(
        Paragraph::new(lines.join("\n")).wrap(Wrap { trim: false }),
        inset_left(block.inner(area), 1),
    );
}

fn inline_env_value(value: &Value, width: u16) -> String {
    let plaintext = match value {
        Value::String(text) => bounded_trimmed_text_for_display(text),
        _ => {
            let node_limit = usize::from(width).clamp(4, 32);
            let preview = bounded_json_preview_with_node_limit(value, node_limit);
            let serialized = serde_json::to_string(&preview).unwrap_or_else(|_| "null".to_string());
            bounded_trimmed_text_for_display(&serialized)
        }
    };
    truncate_to_display_width(&plaintext, width)
}

fn pad_display_width(text: &str, width: usize) -> String {
    let used = UnicodeWidthStr::width(text);
    if used >= width {
        return text.to_string();
    }

    format!("{text}{}", " ".repeat(width - used))
}

struct OpenClawEnvStyledRow {
    plain_text: String,
    line: Line<'static>,
}

fn openclaw_env_row(
    _theme: &super::theme::Theme,
    label_width: usize,
    key: &str,
    value: &str,
) -> OpenClawEnvStyledRow {
    let padded_key = pad_display_width(key, label_width);
    let plain_text = format!("  {padded_key}  {value}");

    OpenClawEnvStyledRow {
        plain_text,
        line: Line::from(vec![
            Span::raw("  "),
            Span::raw(padded_key),
            Span::raw("  "),
            Span::raw(value.to_string()),
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
    saturating_line_height_sum(
        rows.iter()
            .map(|row| wrapped_display_line_count(&row.plain_text, text_width)),
    )
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

    for (row, chunk) in rows.iter().zip(chunks.iter()) {
        frame.render_widget(
            Paragraph::new(row.line.clone()).wrap(Wrap { trim: false }),
            *chunk,
        );
    }
}

fn append_json_map_lines(
    lines: &mut Vec<String>,
    values: &std::collections::HashMap<String, Value>,
) {
    let preview = bounded_json_object_preview(
        values.iter().map(|(key, value)| (key.as_str(), value)),
        values.len(),
    );
    append_bounded_json_lines(lines, preview);
}

fn append_json_map_lines_excluding(
    lines: &mut Vec<String>,
    values: &std::collections::HashMap<String, Value>,
    excluded: &[&str],
) {
    let total = values.len().saturating_sub(
        excluded
            .iter()
            .filter(|key| values.contains_key(**key))
            .count(),
    );
    let preview = bounded_json_object_preview(
        values
            .iter()
            .filter(|(key, _)| !excluded.contains(&key.as_str()))
            .map(|(key, value)| (key.as_str(), value)),
        total,
    );
    append_bounded_json_lines(lines, preview);
}

fn append_bounded_json_lines(lines: &mut Vec<String>, preview: Value) {
    let pretty = serde_json::to_string_pretty(&preview).unwrap_or_else(|_| "{}".to_string());
    lines.extend(pretty.lines().map(|line| format!("  {line}")));
}

#[expect(
    clippy::too_many_arguments,
    reason = "OpenClaw route renderer receives UI context plus parsed config metadata"
)]
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
        .title(format!(" {} ", title));
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let config_path_match = app::bounded_openclaw_config_path(config_path);
    let section_warnings = OpenClawWarningPreview::collect(
        warnings
            .unwrap_or_default()
            .iter()
            .take(app::OPENCLAW_WARNING_SCAN_ITEMS)
            .filter(|warning| openclaw_warning_matches_section(warning, "env.", config_path_match)),
    );
    let has_warnings = !section_warnings.is_empty();
    let warning_height = if has_warnings {
        warning_banner_height(
            &section_warnings,
            inner.width.saturating_sub(CONTENT_INSET_LEFT),
        )
        .min(inner.height.saturating_sub(5))
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

    render_page_key_bar(
        frame,
        chunks[0],
        theme,
        &[
            ("Enter", texts::tui_key_edit()),
            ("e", texts::tui_key_edit()),
            ("Esc", texts::tui_key_close()),
        ],
        app.focus == Focus::Content,
    );

    if has_warnings {
        render_warning_banner(frame, chunks[1], theme, &section_warnings);
    }

    let body_area = inset_left(chunks[3], CONTENT_INSET_LEFT);
    let section_text_width = body_area.width.saturating_sub(3);
    const MAX_ENV_PREVIEW_ROWS: usize = 128;
    let row_capacity = usize::from(chunks[3].height.saturating_sub(4)).min(MAX_ENV_PREVIEW_ROWS);
    let total_entries = section.map_or(0, |section| section.vars.len());
    let needs_more_row = total_entries > row_capacity;
    let entry_capacity = if needs_more_row {
        row_capacity.saturating_sub(1)
    } else {
        row_capacity
    };
    let max_label_width = usize::from(section_text_width.saturating_sub(4)) / 2;
    let mut env_entries = section
        .map(|section| {
            section
                .vars
                .iter()
                .take(entry_capacity)
                .map(|(key, value)| {
                    (
                        truncate_to_display_width(key, max_label_width as u16),
                        value,
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    env_entries.sort_by_key(|(key, _)| key.to_ascii_lowercase());
    let hidden_entries = total_entries.saturating_sub(env_entries.len());
    let label_width = env_entries
        .iter()
        .map(|(key, _)| UnicodeWidthStr::width(key.as_str()))
        .chain((hidden_entries > 0).then_some(1))
        .max()
        .unwrap_or(0);
    let value_width = section_text_width
        .saturating_sub(label_width as u16)
        .saturating_sub(4);
    let mut env_rows = env_entries
        .into_iter()
        .map(|(key, value)| {
            let value = inline_env_value(value, value_width);
            openclaw_env_row(theme, label_width, &key, &value)
        })
        .collect::<Vec<_>>();
    if hidden_entries > 0 && row_capacity > 0 {
        let more = crate::t!(
            format!("{hidden_entries} more entries"),
            format!("还有 {hidden_entries} 项")
        );
        env_rows.push(openclaw_env_row(
            theme,
            label_width,
            "…",
            &truncate_to_display_width(&more, value_width),
        ));
    } else if total_entries == 0 {
        env_rows.push(openclaw_env_empty_row(theme));
    }
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

const OPENCLAW_RENDER_MAX_ROWS: usize = 128;

fn bounded_logical_row_window(
    total: usize,
    selected: Option<usize>,
    available_height: u16,
) -> std::ops::Range<usize> {
    let capacity = usize::from(available_height)
        .clamp(1, OPENCLAW_RENDER_MAX_ROWS)
        .min(total);
    if capacity == 0 {
        return 0..0;
    }

    let selected = selected.filter(|index| *index < total).unwrap_or(0);
    let start = selected
        .saturating_add(1)
        .saturating_sub(capacity)
        .min(total.saturating_sub(capacity));
    start..start.saturating_add(capacity).min(total)
}

fn logical_section_height(row_count: usize) -> u16 {
    u16::try_from(row_count)
        .unwrap_or(u16::MAX)
        .saturating_add(2)
}

fn bounded_passive_value(value: &str, width: u16) -> String {
    truncate_to_display_width(&bounded_trimmed_text_for_display(value), width)
}

struct OpenClawToolsRenderView<'a> {
    profile: Option<&'a str>,
    allow: &'a [String],
    deny: &'a [String],
    extra: Option<&'a std::collections::HashMap<String, Value>>,
    section: app::OpenClawToolsSection,
    row: usize,
}

impl<'a> OpenClawToolsRenderView<'a> {
    fn new(
        form: Option<&'a app::OpenClawToolsFormState>,
        snapshot: Option<&'a crate::openclaw_config::OpenClawToolsConfig>,
    ) -> Self {
        if let Some(form) = form {
            return Self {
                profile: form.profile.as_deref(),
                allow: &form.allow,
                deny: &form.deny,
                extra: (!form.extra.is_empty()).then_some(&form.extra),
                section: form.section,
                row: form.row,
            };
        }

        Self {
            profile: snapshot.and_then(|tools| tools.profile.as_deref()),
            allow: snapshot.map_or(&[], |tools| tools.allow.as_slice()),
            deny: snapshot.map_or(&[], |tools| tools.deny.as_slice()),
            extra: snapshot.and_then(|tools| (!tools.extra.is_empty()).then_some(&tools.extra)),
            section: app::OpenClawToolsSection::Profile,
            row: 0,
        }
    }

    fn is_selected(&self, section: app::OpenClawToolsSection, row: usize) -> bool {
        self.section == section && self.row == row
    }

    fn unsupported_profile(&self) -> Option<&'a str> {
        let profile = self.profile?;
        app::openclaw_tools_profile_picker_index(Some(profile))
            .is_none()
            .then_some(profile)
    }

    fn current_profile_label(&self, width: u16) -> String {
        if let Some(index) = app::openclaw_tools_profile_picker_index(self.profile) {
            return bounded_passive_value(app::openclaw_tools_profile_picker_label(index), width);
        }

        let suffix = format!(
            " ({})",
            texts::tui_openclaw_tools_unsupported_profile_label()
        );
        let suffix_width =
            u16::try_from(UnicodeWidthStr::width(suffix.as_str())).unwrap_or(u16::MAX);
        let profile = bounded_passive_value(
            self.profile.unwrap_or_default(),
            width.saturating_sub(suffix_width),
        );
        bounded_passive_value(&format!("{profile}{suffix}"), width)
    }
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
        wrap: false,
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
    width: u16,
    selected: bool,
) -> OpenClawToolsStyledRow {
    let plain_text = bounded_passive_value(value, width);
    let row_style = openclaw_tools_selected_row_style(theme, selected);

    OpenClawToolsStyledRow {
        plain_text: plain_text.clone(),
        line: if selected {
            Line::styled(plain_text, row_style)
        } else {
            Line::from(plain_text)
        },
        wrap: false,
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
        wrap: false,
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
    saturating_line_height_sum(section_line_heights(
        &rows
            .iter()
            .map(|row| row.plain_text.clone())
            .collect::<Vec<_>>(),
        &rows.iter().map(|row| row.wrap).collect::<Vec<_>>(),
        text_width,
    ))
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
            format!(" {title} "),
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

    for (row, chunk) in rows.iter().zip(chunks.iter()) {
        let paragraph = if row.wrap {
            Paragraph::new(row.line.clone()).wrap(Wrap { trim: false })
        } else {
            Paragraph::new(row.line.clone())
        };
        frame.render_widget(paragraph, *chunk);
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "OpenClaw route renderer receives UI context plus parsed config metadata"
)]
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
    let view = OpenClawToolsRenderView::new(
        app.openclaw_tools_form.as_ref(),
        data.config.openclaw_tools.as_ref(),
    );

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(format!(" {} ", title));
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let config_path_match = app::bounded_openclaw_config_path(config_path);
    let parse_warnings = OpenClawWarningPreview::collect(
        warnings
            .unwrap_or_default()
            .iter()
            .take(app::OPENCLAW_WARNING_SCAN_ITEMS)
            .filter(|warning| {
                warning.code == "config_parse_failed"
                    && openclaw_warning_matches_section(warning, "tools.", config_path_match)
            }),
    );
    let has_parse_warning = !parse_warnings.is_empty();
    let warning_height = if has_parse_warning {
        warning_banner_height(
            &parse_warnings,
            inner.width.saturating_sub(CONTENT_INSET_LEFT),
        )
        .min(inner.height.saturating_sub(4))
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
    render_page_key_bar(
        frame,
        chunks[0],
        theme,
        &key_bar_items,
        app.focus == Focus::Content,
    );

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

    let section_text_width = body_area.width.saturating_sub(3);
    let profile_label = texts::tui_openclaw_tools_profile_label();
    let profile_value_width = section_text_width.saturating_sub(
        u16::try_from(UnicodeWidthStr::width(profile_label))
            .unwrap_or(u16::MAX)
            .saturating_add(2),
    );
    let profile_value = view.current_profile_label(profile_value_width);
    let mut profile_rows = vec![openclaw_tools_profile_row(
        theme,
        profile_label,
        &profile_value,
        view.is_selected(app::OpenClawToolsSection::Profile, 0),
    )];
    if let Some(value) = view.unsupported_profile() {
        profile_rows.push(openclaw_tools_note_row(
            texts::tui_openclaw_tools_unsupported_profile_title().to_string(),
            Style::default().fg(theme.comment),
            true,
        ));
        let value = bounded_passive_value(value, section_text_width);
        profile_rows.push(openclaw_tools_note_row(
            texts::tui_openclaw_tools_unsupported_profile_description(&value),
            Style::default().fg(theme.dim),
            true,
        ));
    }

    let mut extra_lines = Vec::new();
    if let Some(extra) = view.extra {
        append_json_map_lines(&mut extra_lines, extra);
    }

    // The rules pane is a virtual list. Compute its logical window from list
    // lengths first, then allocate styled rows only for terminal-visible slots.
    let allow_len = view.allow.len();
    let deny_start = allow_len.saturating_add(4);
    let base_rules_len = deny_start.saturating_add(view.deny.len()).saturating_add(1);
    let rules_total = base_rules_len.saturating_add(if view.extra.is_some() {
        1usize.saturating_add(extra_lines.len())
    } else {
        0
    });
    let rules_selected_logical = match view.section {
        app::OpenClawToolsSection::Profile => None,
        app::OpenClawToolsSection::Allow => Some(1usize.saturating_add(view.row)),
        app::OpenClawToolsSection::Deny => Some(deny_start.saturating_add(view.row)),
    };

    let profile_plain_lines = profile_rows
        .iter()
        .map(|row| row.plain_text.clone())
        .collect::<Vec<_>>();
    let profile_wraps = profile_rows.iter().map(|row| row.wrap).collect::<Vec<_>>();
    let profile_line_heights =
        section_line_heights(&profile_plain_lines, &profile_wraps, section_text_width);
    let profile_height = openclaw_tools_section_block_height(&profile_rows, section_text_width);
    let rules_height = logical_section_height(rules_total);
    let remaining_height = body_area.height.saturating_sub(2);
    let (profile_height, rules_height) = split_section_heights(
        remaining_height,
        profile_height,
        rules_height,
        matches!(
            view.section,
            app::OpenClawToolsSection::Allow | app::OpenClawToolsSection::Deny
        ),
    );
    let profile_window = section_line_window(
        &profile_line_heights,
        profile_height,
        view.is_selected(app::OpenClawToolsSection::Profile, 0)
            .then_some(0),
    );
    let visible_profile_rows = &profile_rows[profile_window];
    let rules_window = bounded_logical_row_window(
        rules_total,
        rules_selected_logical,
        rules_height.saturating_sub(2),
    );
    let rules_selected_line = rules_selected_logical.and_then(|selected| {
        rules_window
            .contains(&selected)
            .then_some(selected.saturating_sub(rules_window.start))
    });
    let mut rules_rows = Vec::with_capacity(rules_window.len());
    for logical in rules_window {
        let selected = rules_selected_line == Some(rules_rows.len());
        let row = if logical == 0 {
            openclaw_tools_section_label_row(theme, texts::tui_openclaw_tools_allow_list_label())
        } else if logical <= allow_len {
            openclaw_tools_rule_row(
                theme,
                &view.allow[logical - 1],
                section_text_width,
                selected,
            )
        } else if logical == allow_len.saturating_add(1) {
            openclaw_tools_add_row(theme, texts::tui_openclaw_tools_add_allow_rule(), selected)
        } else if logical == allow_len.saturating_add(2) {
            openclaw_tools_separator_row(theme)
        } else if logical == allow_len.saturating_add(3) {
            openclaw_tools_section_label_row(theme, texts::tui_openclaw_tools_deny_list_label())
        } else if logical < deny_start.saturating_add(view.deny.len()) {
            openclaw_tools_rule_row(
                theme,
                &view.deny[logical - deny_start],
                section_text_width,
                selected,
            )
        } else if logical == deny_start.saturating_add(view.deny.len()) {
            openclaw_tools_add_row(theme, texts::tui_openclaw_tools_add_deny_rule(), selected)
        } else if logical == base_rules_len {
            openclaw_tools_section_label_row(theme, texts::tui_openclaw_tools_extra_fields_label())
        } else {
            let extra_index = logical.saturating_sub(base_rules_len.saturating_add(1));
            openclaw_tools_note_row(
                extra_lines.get(extra_index).cloned().unwrap_or_default(),
                Style::default().fg(theme.comment),
                true,
            )
        };
        rules_rows.push(row);
    }
    let visible_rules_rows = rules_rows.as_slice();
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

const OPENCLAW_RENDER_MODEL_PROVIDER_SLOTS: usize = 128;
const OPENCLAW_RENDER_MODELS_PER_PROVIDER: usize = 128;
const OPENCLAW_RENDER_MODEL_OPTIONS: usize = 512;
const OPENCLAW_RENDER_USED_MODEL_SLOTS: usize = 512;
const OPENCLAW_RENDER_MODEL_ID_MAX_BYTES: usize = 1_024;
const OPENCLAW_RENDER_MODEL_VALUE_MAX_BYTES: usize = OPENCLAW_RENDER_MODEL_ID_MAX_BYTES * 2 + 1;
const OPENCLAW_RENDER_TEXT_SCAN_CHARS: usize = 2_048;
const OPENCLAW_AGENTS_RUNTIME_KEYS: &[&str] = &[
    "workspace",
    "timeout",
    "timeoutSeconds",
    "contextTokens",
    "maxConcurrent",
];

fn bounded_is_blank(value: &str) -> bool {
    for (index, ch) in value.chars().enumerate() {
        if index >= OPENCLAW_RENDER_TEXT_SCAN_CHARS || !ch.is_whitespace() {
            return false;
        }
    }
    true
}

fn bounded_render_number(value: &str) -> bool {
    if value.len() > OPENCLAW_RENDER_MODEL_ID_MAX_BYTES {
        return false;
    }
    crate::cli::openclaw_form_normalization::parse_number(value.trim()).is_some()
}

#[derive(Clone, Copy)]
struct OpenClawRenderModelOption<'a> {
    provider_id: &'a str,
    provider_name: &'a str,
    model_id: &'a str,
    model_name: &'a str,
}

impl OpenClawRenderModelOption<'_> {
    fn matches(&self, value: &str) -> bool {
        if value.len() > OPENCLAW_RENDER_MODEL_VALUE_MAX_BYTES {
            return false;
        }
        value
            .strip_prefix(self.provider_id)
            .and_then(|rest| rest.strip_prefix('/'))
            == Some(self.model_id)
    }

    fn display_label(&self, width: u16) -> String {
        let component_width = width.saturating_sub(3) / 2;
        let provider = bounded_passive_value(self.provider_name, component_width);
        let model = bounded_passive_value(
            self.model_name,
            width.saturating_sub(
                u16::try_from(UnicodeWidthStr::width(provider.as_str()))
                    .unwrap_or(u16::MAX)
                    .saturating_add(3),
            ),
        );
        bounded_passive_value(&format!("{provider} / {model}"), width)
    }
}

struct OpenClawRenderModelPreview<'a> {
    options: Vec<OpenClawRenderModelOption<'a>>,
    complete: bool,
}

impl<'a> OpenClawRenderModelPreview<'a> {
    fn from_data(data: &'a UiData) -> Self {
        let mut options = Vec::new();
        let mut complete = data.providers.rows.len() <= OPENCLAW_RENDER_MODEL_PROVIDER_SLOTS;

        'providers: for row in data
            .providers
            .rows
            .iter()
            .take(OPENCLAW_RENDER_MODEL_PROVIDER_SLOTS)
        {
            if row.id.len() > OPENCLAW_RENDER_MODEL_ID_MAX_BYTES {
                complete = false;
                continue;
            }
            let provider_name = if bounded_is_blank(&row.provider.name) {
                row.id.as_str()
            } else {
                row.provider.name.as_str()
            };
            let Some(models) = row
                .provider
                .settings_config
                .get("models")
                .and_then(Value::as_array)
            else {
                continue;
            };
            complete &= models.len() <= OPENCLAW_RENDER_MODELS_PER_PROVIDER;

            for model in models.iter().take(OPENCLAW_RENDER_MODELS_PER_PROVIDER) {
                if options.len() >= OPENCLAW_RENDER_MODEL_OPTIONS {
                    complete = false;
                    break 'providers;
                }
                let Some(model_id) = model.get("id").and_then(Value::as_str) else {
                    continue;
                };
                if model_id.len() > OPENCLAW_RENDER_MODEL_ID_MAX_BYTES {
                    complete = false;
                    continue;
                }
                if bounded_is_blank(model_id) {
                    continue;
                }
                let model_name = model
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| !bounded_is_blank(name))
                    .unwrap_or(model_id);
                options.push(OpenClawRenderModelOption {
                    provider_id: row.id.as_str(),
                    provider_name,
                    model_id,
                    model_name,
                });
            }
        }

        Self { options, complete }
    }

    fn find(&self, value: &str) -> Option<&OpenClawRenderModelOption<'a>> {
        self.options.iter().find(|option| option.matches(value))
    }

    fn has_available(&self, primary: &str, fallbacks: &[String]) -> bool {
        // A bounded preview must never falsely disable the real picker. If any
        // source or used-value slots were omitted, keep the action enabled and
        // let the user-triggered full picker make the authoritative decision.
        if !self.complete || fallbacks.len() > OPENCLAW_RENDER_USED_MODEL_SLOTS {
            return true;
        }

        self.options.iter().any(|option| {
            !option.matches(primary) && !fallbacks.iter().any(|fallback| option.matches(fallback))
        })
    }
}

enum OpenClawAgentsRenderView<'a> {
    Form(&'a app::OpenClawAgentsFormState),
    Snapshot(Option<&'a crate::openclaw_config::OpenClawAgentsDefaults>),
}

impl<'a> OpenClawAgentsRenderView<'a> {
    fn new(
        form: Option<&'a app::OpenClawAgentsFormState>,
        snapshot: Option<&'a crate::openclaw_config::OpenClawAgentsDefaults>,
    ) -> Self {
        form.map_or(Self::Snapshot(snapshot), Self::Form)
    }

    fn primary_model(&self) -> &'a str {
        match self {
            Self::Form(form) => &form.primary_model,
            Self::Snapshot(defaults) => defaults
                .and_then(|defaults| defaults.model.as_ref())
                .map_or("", |model| model.primary.as_str()),
        }
    }

    fn fallbacks(&self) -> &'a [String] {
        match self {
            Self::Form(form) => &form.fallbacks,
            Self::Snapshot(defaults) => defaults
                .and_then(|defaults| defaults.model.as_ref())
                .map_or(&[], |model| model.fallbacks.as_slice()),
        }
    }

    fn section(&self) -> app::OpenClawAgentsSection {
        match self {
            Self::Form(form) => form.section,
            Self::Snapshot(_) => app::OpenClawAgentsSection::PrimaryModel,
        }
    }

    fn row(&self) -> usize {
        match self {
            Self::Form(form) => form.row,
            Self::Snapshot(_) => 0,
        }
    }

    fn is_selected(&self, section: app::OpenClawAgentsSection, row: usize) -> bool {
        self.section() == section && self.row() == row
    }

    fn model_extra(&self) -> Option<&'a std::collections::HashMap<String, Value>> {
        match self {
            Self::Form(form) => (!form.model_extra.is_empty()).then_some(&form.model_extra),
            Self::Snapshot(defaults) => defaults
                .and_then(|defaults| defaults.model.as_ref())
                .and_then(|model| (!model.extra.is_empty()).then_some(&model.extra)),
        }
    }

    fn has_defaults_extra(&self) -> bool {
        match self {
            Self::Form(form) => !form.defaults_extra.is_empty(),
            Self::Snapshot(defaults) => defaults.is_some_and(|defaults| {
                let excluded = OPENCLAW_AGENTS_RUNTIME_KEYS
                    .iter()
                    .filter(|key| defaults.extra.contains_key(**key))
                    .count();
                defaults.extra.len() > excluded
            }),
        }
    }

    fn append_defaults_extra_lines(&self, lines: &mut Vec<String>) {
        match self {
            Self::Form(form) => append_json_map_lines(lines, &form.defaults_extra),
            Self::Snapshot(Some(defaults)) => append_json_map_lines_excluding(
                lines,
                &defaults.extra,
                OPENCLAW_AGENTS_RUNTIME_KEYS,
            ),
            Self::Snapshot(None) => {}
        }
    }

    fn has_legacy_timeout(&self) -> bool {
        match self {
            Self::Form(form) => form.has_legacy_timeout,
            Self::Snapshot(defaults) => {
                defaults.is_some_and(|defaults| defaults.extra.contains_key("timeout"))
            }
        }
    }

    fn has_unmigratable_legacy_timeout(&self) -> bool {
        match self {
            Self::Form(form) => {
                form.has_legacy_timeout
                    && !bounded_is_blank(&form.timeout)
                    && !bounded_render_number(&form.timeout)
            }
            Self::Snapshot(defaults) => {
                let Some(value) = defaults.and_then(|defaults| defaults.extra.get("timeout"))
                else {
                    return false;
                };
                match value {
                    Value::String(value) => {
                        !bounded_is_blank(value) && !bounded_render_number(value)
                    }
                    Value::Number(_) => false,
                    Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => true,
                }
            }
        }
    }

    fn has_preserved_non_string_runtime_values(&self) -> bool {
        match self {
            Self::Form(form) => [
                (&form.timeout, form.timeout_seconds_seed.as_ref()),
                (&form.context_tokens, form.context_tokens_seed.as_ref()),
                (&form.max_concurrent, form.max_concurrent_seed.as_ref()),
            ]
            .into_iter()
            .any(|(value, seed)| {
                bounded_is_blank(value)
                    && seed.is_some_and(|seed| !seed.is_number() && !seed.is_string())
            }),
            Self::Snapshot(defaults) => defaults.is_some_and(|defaults| {
                ["timeoutSeconds", "contextTokens", "maxConcurrent"]
                    .iter()
                    .filter_map(|key| defaults.extra.get(*key))
                    .any(|value| !value.is_number() && !value.is_string())
            }),
        }
    }

    fn runtime_display(&self, field: app::OpenClawAgentsRuntimeField, width: u16) -> String {
        match self {
            Self::Form(form) => {
                let (value, preserved) = match field {
                    app::OpenClawAgentsRuntimeField::Workspace => (form.workspace.as_str(), None),
                    app::OpenClawAgentsRuntimeField::Timeout => (
                        form.timeout.as_str(),
                        bounded_is_blank(&form.timeout)
                            .then_some(form.timeout_seconds_seed.as_ref())
                            .flatten()
                            .filter(|seed| !seed.is_number() && !seed.is_string()),
                    ),
                    app::OpenClawAgentsRuntimeField::ContextTokens => (
                        form.context_tokens.as_str(),
                        bounded_is_blank(&form.context_tokens)
                            .then_some(form.context_tokens_seed.as_ref())
                            .flatten()
                            .filter(|seed| !seed.is_number() && !seed.is_string()),
                    ),
                    app::OpenClawAgentsRuntimeField::MaxConcurrent => (
                        form.max_concurrent.as_str(),
                        bounded_is_blank(&form.max_concurrent)
                            .then_some(form.max_concurrent_seed.as_ref())
                            .flatten()
                            .filter(|seed| !seed.is_number() && !seed.is_string()),
                    ),
                };
                bounded_openclaw_runtime_value(value, preserved, width)
            }
            Self::Snapshot(defaults) => {
                let extra = defaults.map(|defaults| &defaults.extra);
                match field {
                    app::OpenClawAgentsRuntimeField::Workspace => bounded_openclaw_raw_value(
                        extra.and_then(|extra| extra.get("workspace")),
                        false,
                        width,
                    ),
                    app::OpenClawAgentsRuntimeField::Timeout => {
                        let legacy = extra.and_then(|extra| extra.get("timeout"));
                        if legacy.is_some() {
                            bounded_openclaw_raw_value(legacy, false, width)
                        } else {
                            bounded_openclaw_raw_value(
                                extra.and_then(|extra| extra.get("timeoutSeconds")),
                                true,
                                width,
                            )
                        }
                    }
                    app::OpenClawAgentsRuntimeField::ContextTokens => bounded_openclaw_raw_value(
                        extra.and_then(|extra| extra.get("contextTokens")),
                        true,
                        width,
                    ),
                    app::OpenClawAgentsRuntimeField::MaxConcurrent => bounded_openclaw_raw_value(
                        extra.and_then(|extra| extra.get("maxConcurrent")),
                        true,
                        width,
                    ),
                }
            }
        }
    }
}

fn bounded_openclaw_json_value(value: &Value, width: u16) -> String {
    let preview = bounded_json_preview_with_node_limit(value, usize::from(width).clamp(4, 32));
    let serialized = serde_json::to_string(&preview).unwrap_or_else(|_| "null".to_string());
    bounded_passive_value(&serialized, width)
}

fn bounded_openclaw_preserved_value(value: &Value, width: u16) -> String {
    let raw = match value {
        Value::String(value) => bounded_passive_value(value, width),
        _ => bounded_openclaw_json_value(value, width),
    };
    bounded_passive_value(
        &texts::tui_openclaw_agents_preserved_non_standard_value(&raw),
        width,
    )
}

fn bounded_openclaw_runtime_value(value: &str, preserved: Option<&Value>, width: u16) -> String {
    if !bounded_is_blank(value) {
        return bounded_passive_value(value, width);
    }
    preserved.map_or_else(
        || bounded_passive_value(texts::tui_openclaw_agents_not_set(), width),
        |raw| bounded_openclaw_preserved_value(raw, width),
    )
}

fn bounded_openclaw_raw_value(value: Option<&Value>, numeric_only: bool, width: u16) -> String {
    let Some(value) = value else {
        return bounded_passive_value(texts::tui_openclaw_agents_not_set(), width);
    };
    match value {
        Value::String(value) => bounded_openclaw_runtime_value(value, None, width),
        Value::Number(value) => bounded_passive_value(&value.to_string(), width),
        Value::Bool(value) if !numeric_only => bounded_passive_value(&value.to_string(), width),
        Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) if numeric_only => {
            bounded_openclaw_preserved_value(value, width)
        }
        Value::Null | Value::Array(_) | Value::Object(_) => {
            bounded_openclaw_json_value(value, width)
        }
        Value::Bool(_) => unreachable!("non-numeric booleans handled above"),
    }
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
        row_style.fg(theme.fg_strong)
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
    saturating_line_height_sum(section_line_heights(
        &rows
            .iter()
            .map(|row| row.plain_text.clone())
            .collect::<Vec<_>>(),
        &rows.iter().map(|row| row.wrap).collect::<Vec<_>>(),
        text_width,
    ))
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
            format!(" {title} "),
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

    for (row, chunk) in rows.iter().zip(chunks.iter()) {
        let paragraph = if row.wrap {
            Paragraph::new(row.line.clone()).wrap(Wrap { trim: false })
        } else {
            Paragraph::new(row.line.clone())
        };
        frame.render_widget(paragraph, *chunk);
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "OpenClaw route renderer receives UI context plus parsed config metadata"
)]
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
    let view = OpenClawAgentsRenderView::new(
        app.openclaw_agents_form.as_ref(),
        data.config.openclaw_agents_defaults.as_ref(),
    );
    let model_preview = OpenClawRenderModelPreview::from_data(data);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(format!(" {} ", title));
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let config_path_match = app::bounded_openclaw_config_path(config_path);
    let parse_warnings = OpenClawWarningPreview::collect(
        warnings
            .unwrap_or_default()
            .iter()
            .take(app::OPENCLAW_WARNING_SCAN_ITEMS)
            .filter(|warning| {
                warning.code == "config_parse_failed"
                    && openclaw_warning_matches_section(
                        warning,
                        "agents.defaults.",
                        config_path_match,
                    )
            }),
    );
    let has_parse_warning = !parse_warnings.is_empty();
    let warning_height = if has_parse_warning {
        warning_banner_height(
            &parse_warnings,
            inner.width.saturating_sub(CONTENT_INSET_LEFT),
        )
        .min(inner.height.saturating_sub(4))
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

    let key_bar_items = if load_failed {
        vec![("Esc", texts::tui_key_close())]
    } else {
        vec![
            ("Enter", texts::tui_key_edit()),
            ("Del", texts::tui_key_delete()),
            ("Esc", texts::tui_key_close()),
        ]
    };
    render_page_key_bar(
        frame,
        chunks[0],
        theme,
        &key_bar_items,
        app.focus == Focus::Content,
    );

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
    let section_text_width = body_area.width.saturating_sub(3);
    let field_prefix_width = u16::try_from(field_label_width)
        .unwrap_or(u16::MAX)
        .saturating_add(4);
    let field_value_width = section_text_width.saturating_sub(field_prefix_width).max(1);

    let mut model_extra_lines = Vec::new();
    if let Some(model_extra) = view.model_extra() {
        append_json_map_lines(&mut model_extra_lines, model_extra);
    }
    let fallbacks = view.fallbacks();
    let model_base_len = fallbacks.len().saturating_add(2);
    let model_total = model_base_len.saturating_add(if view.model_extra().is_some() {
        2usize.saturating_add(model_extra_lines.len())
    } else {
        0
    });
    let model_selected_logical = match view.section() {
        app::OpenClawAgentsSection::PrimaryModel => Some(0),
        app::OpenClawAgentsSection::FallbackModels => Some(1usize.saturating_add(view.row())),
        app::OpenClawAgentsSection::Runtime => None,
    };
    let add_fallback_disabled =
        !model_preview.has_available(view.primary_model(), view.fallbacks());

    let mut runtime_rows = Vec::new();
    let mut runtime_selected_line = None;
    fn push_runtime_row(rows: &mut Vec<OpenClawAgentsStyledRow>, row: OpenClawAgentsStyledRow) {
        rows.push(row);
    }
    if view.has_legacy_timeout() {
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
                    if view.has_unmigratable_legacy_timeout() {
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

    for (row, field, label) in [
        (
            0,
            app::OpenClawAgentsRuntimeField::Workspace,
            texts::tui_openclaw_agents_workspace(),
        ),
        (
            1,
            app::OpenClawAgentsRuntimeField::Timeout,
            texts::tui_openclaw_agents_timeout(),
        ),
        (
            2,
            app::OpenClawAgentsRuntimeField::ContextTokens,
            texts::tui_openclaw_agents_context_tokens(),
        ),
        (
            3,
            app::OpenClawAgentsRuntimeField::MaxConcurrent,
            texts::tui_openclaw_agents_max_concurrent(),
        ),
    ] {
        if view.is_selected(app::OpenClawAgentsSection::Runtime, row) {
            runtime_selected_line = Some(runtime_rows.len());
        }
        let value = view.runtime_display(field, field_value_width.saturating_sub(2));
        let value = bounded_passive_value(&format!("[{value}]"), field_value_width);
        push_runtime_row(
            &mut runtime_rows,
            openclaw_agents_field_row(
                theme,
                field_label_width,
                label,
                &value,
                None,
                view.is_selected(app::OpenClawAgentsSection::Runtime, row),
                false,
            ),
        );
    }
    if view.has_preserved_non_string_runtime_values() {
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
    if view.has_defaults_extra() {
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
        view.append_defaults_extra_lines(&mut defaults_extra_lines);
        runtime_rows.extend(
            defaults_extra_lines
                .into_iter()
                .map(|line| openclaw_agents_note_row(line, true)),
        );
    }
    let runtime_plain_lines = runtime_rows
        .iter()
        .map(|row| row.plain_text.clone())
        .collect::<Vec<_>>();
    let runtime_wraps = runtime_rows.iter().map(|row| row.wrap).collect::<Vec<_>>();
    let runtime_line_heights =
        section_line_heights(&runtime_plain_lines, &runtime_wraps, section_text_width);
    let runtime_height = openclaw_agents_section_block_height(&runtime_rows, section_text_width);
    let model_height = logical_section_height(model_total);
    let remaining_height = body_area.height.saturating_sub(2);
    let (model_height, runtime_height) = split_section_heights(
        remaining_height,
        model_height,
        runtime_height,
        view.section() == app::OpenClawAgentsSection::Runtime,
    );
    let model_window = bounded_logical_row_window(
        model_total,
        model_selected_logical,
        model_height.saturating_sub(2),
    );
    let model_selected_line = model_selected_logical.and_then(|selected| {
        model_window
            .contains(&selected)
            .then_some(selected.saturating_sub(model_window.start))
    });
    let mut model_rows = Vec::with_capacity(model_window.len());
    for logical in model_window {
        let selected = model_selected_line == Some(model_rows.len());
        let row = if logical == 0 {
            let (value, status) = openclaw_agents_render_model_value(
                view.primary_model(),
                &model_preview,
                field_value_width,
            );
            openclaw_agents_field_row(
                theme,
                field_label_width,
                texts::tui_openclaw_agents_primary_model(),
                &value,
                status,
                selected,
                false,
            )
        } else if logical <= fallbacks.len() {
            let (value, status) = openclaw_agents_render_model_value(
                &fallbacks[logical - 1],
                &model_preview,
                field_value_width,
            );
            openclaw_agents_field_row(
                theme,
                field_label_width,
                texts::tui_openclaw_agents_fallback_models(),
                &value,
                status,
                selected,
                false,
            )
        } else if logical == fallbacks.len().saturating_add(1) {
            if add_fallback_disabled {
                openclaw_agents_disabled_row(
                    theme,
                    field_label_width,
                    texts::tui_openclaw_agents_add_fallback_disabled(),
                )
            } else {
                openclaw_agents_action_row(
                    theme,
                    field_label_width,
                    texts::tui_openclaw_agents_add_fallback(),
                    selected,
                )
            }
        } else if logical == model_base_len {
            openclaw_agents_note_row(String::new(), false)
        } else if logical == model_base_len.saturating_add(1) {
            openclaw_agents_note_row(
                texts::tui_openclaw_agents_preserved_fields_label().to_string(),
                false,
            )
        } else {
            let extra_index = logical.saturating_sub(model_base_len.saturating_add(2));
            openclaw_agents_note_row(
                model_extra_lines
                    .get(extra_index)
                    .cloned()
                    .unwrap_or_default(),
                true,
            )
        };
        model_rows.push(row);
    }
    let visible_model_rows = model_rows.as_slice();
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

fn openclaw_agents_render_model_value(
    value: &str,
    preview: &OpenClawRenderModelPreview<'_>,
    width: u16,
) -> (String, Option<&'static str>) {
    if bounded_is_blank(value) {
        return (
            bounded_passive_value(texts::tui_openclaw_agents_not_set(), width),
            None,
        );
    }

    preview
        .find(value)
        .map(|option| (option.display_label(width), None))
        .unwrap_or_else(|| {
            (
                bounded_passive_value(value, width),
                preview
                    .complete
                    .then_some(texts::tui_openclaw_agents_not_configured_suffix()),
            )
        })
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
    saturating_line_height_sum(
        rows.iter()
            .map(|row| openclaw_workspace_row_height(row, text_width)),
    )
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

    if line_heights_fit(remaining, [files_full_height, daily_full_height]) {
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
        block = block.title(format!(" {} ", title));
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
        .title(format!(" {} ", texts::tui_openclaw_workspace_title()));
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_page_key_bar(
        frame,
        chunks[0],
        theme,
        &[
            ("Enter", texts::tui_key_open()),
            ("o", texts::tui_key_open_directory()),
        ],
        app.focus == Focus::Content,
    );

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
        .title(breadcrumb_title(&[
            texts::tui_openclaw_workspace_title(),
            texts::tui_openclaw_daily_memory_title(),
        ]));
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

    render_page_key_bar(
        frame,
        chunks[0],
        theme,
        &[
            ("Enter", texts::tui_key_open()),
            ("a", texts::tui_key_create()),
            ("d", texts::tui_key_delete()),
            ("o", texts::tui_key_open_directory()),
        ],
        app.focus == Focus::Content,
    );

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

pub(super) fn render_hermes_memory(
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
        .title(format!(" {} ", texts::tui_hermes_memory_title()));
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

    render_page_key_bar(
        frame,
        chunks[0],
        theme,
        &[
            ("Enter", texts::tui_key_edit()),
            ("Space/x", texts::tui_key_toggle()),
            ("o", texts::tui_key_open_directory()),
        ],
        app.focus == Focus::Content,
    );

    frame.render_widget(
        Paragraph::new(format!(
            "{}: {}",
            texts::tui_hermes_memory_directory_label(),
            data.config.hermes_memory.directory_path.display()
        ))
        .wrap(Wrap { trim: false }),
        inset_left(chunks[1], CONTENT_INSET_LEFT),
    );

    let rows = [
        crate::hermes_config::MemoryKind::Memory,
        crate::hermes_config::MemoryKind::User,
    ]
    .into_iter()
    .map(|kind| {
        let content = data.config.hermes_memory.content(kind);
        let current = content.chars().count();
        let limit = data.config.hermes_memory.limit(kind);
        let enabled = data.config.hermes_memory.enabled(kind);
        let status = if enabled {
            texts::enabled()
        } else {
            texts::disabled()
        };
        let preview = content
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(90)
            .collect::<String>();
        Row::new(vec![
            Cell::from(hermes_memory_display_name(kind)),
            Cell::from(status),
            Cell::from(format!("{current}/{limit}")),
            Cell::from(if preview.trim().is_empty() {
                texts::tui_na().to_string()
            } else {
                preview
            }),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(18),
            Constraint::Length(12),
            Constraint::Length(14),
            Constraint::Min(10),
        ],
    )
    .header(
        Row::new(vec![
            Cell::from(texts::tui_hermes_memory_file_label()),
            Cell::from(texts::tui_hermes_memory_status_label()),
            Cell::from(texts::tui_hermes_memory_usage_label()),
            Cell::from(texts::tui_hermes_memory_preview_label()),
        ])
        .style(Style::default().fg(theme.comment)),
    )
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.hermes_memory_idx));
    frame.render_stateful_widget(table, inset_left(chunks[2], CONTENT_INSET_LEFT), &mut state);
}

fn hermes_memory_display_name(kind: crate::hermes_config::MemoryKind) -> String {
    match kind {
        crate::hermes_config::MemoryKind::Memory => {
            format!(
                "{} ({})",
                texts::tui_hermes_memory_agent_tab(),
                kind.filename()
            )
        }
        crate::hermes_config::MemoryKind::User => {
            format!(
                "{} ({})",
                texts::tui_hermes_memory_user_tab(),
                kind.filename()
            )
        }
    }
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
    let visible_apps_mode = crate::settings::get_visible_apps_settings().mode;
    let openclaw_config_dir = crate::settings::get_settings().openclaw_config_dir;
    let skip_claude_onboarding = crate::settings::get_skip_claude_onboarding();
    let claude_plugin_integration = crate::settings::get_enable_claude_plugin_integration();
    let codex_unified_session_history = crate::settings::unify_codex_session_history();
    let preferred_editor = crate::settings::get_preferred_editor();

    let rows_data = super::app::SettingsItem::ALL
        .iter()
        .map(|item| match item {
            super::app::SettingsItem::Language => (
                texts::tui_settings_header_language().to_string(),
                language.display_name().to_string(),
            ),
            super::app::SettingsItem::Theme => {
                (
                    texts::tui_settings_theme_label().to_string(),
                    texts::tui_settings_theme_mode_name(
                        crate::cli::tui::theme::configured_theme_mode(),
                    )
                    .to_string(),
                )
            }
            super::app::SettingsItem::Icons => (
                texts::tui_settings_icons_label().to_string(),
                texts::tui_settings_icon_mode_name(crate::cli::tui::icons::configured_icon_mode())
                    .to_string(),
            ),
            super::app::SettingsItem::PreferredEditor => (
                texts::tui_settings_preferred_editor_label().to_string(),
                preferred_editor
                    .clone()
                    .unwrap_or_else(|| texts::tui_settings_preferred_editor_not_set().to_string()),
            ),
            super::app::SettingsItem::VisibleAppsMode => (
                texts::tui_settings_visible_apps_mode_label().to_string(),
                match visible_apps_mode {
                    crate::settings::VisibleAppsMode::Auto => {
                        texts::tui_settings_visible_apps_mode_auto().to_string()
                    }
                    crate::settings::VisibleAppsMode::Manual => {
                        texts::tui_settings_visible_apps_mode_manual().to_string()
                    }
                },
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
            super::app::SettingsItem::ManagedAccounts => (
                texts::tui_settings_managed_accounts_title().to_string(),
                managed_accounts_summary(app),
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
            super::app::SettingsItem::CodexUnifiedSessionHistory => (
                texts::codex_unified_session_history_label().to_string(),
                if codex_unified_session_history {
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

    let body = render_page_frame(
        frame,
        area,
        theme,
        app,
        texts::menu_settings(),
        &[("Enter", texts::tui_key_apply())],
        None,
    );
    let table_area = inset_left(body, CONTENT_INSET_LEFT);

    let raw_label_col_width = field_label_column_width(
        rows_data
            .iter()
            .map(|(label, _value)| label.as_str())
            .chain(std::iter::once(texts::tui_settings_header_setting())),
        0,
    );
    let label_col_width = raw_label_col_width.min(table_area.width.saturating_sub(9));

    let header = Row::new(vec![
        Cell::from(texts::tui_settings_header_setting()),
        Cell::from(texts::tui_settings_header_value()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let rows = rows_data.iter().map(|(label, value)| {
        let value = bounded_trimmed_text_for_display(value);
        Row::new(vec![
            Cell::from(truncate_to_display_width(label, label_col_width)),
            Cell::from(truncated_value_cell(
                &value,
                table_area.width,
                label_col_width,
                theme,
            )),
        ])
    });

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
    frame.render_stateful_widget(table, table_area, &mut state);
}

fn managed_accounts_summary(app: &App) -> String {
    if app.managed_auth_loading {
        return texts::tui_loading().to_string();
    }

    let Some(status) = app.managed_auth_status.as_ref() else {
        return texts::tui_managed_accounts_not_loaded().to_string();
    };

    primary_managed_account(status)
        .map(|account| account.login.clone())
        .unwrap_or_else(|| texts::tui_managed_accounts_not_authenticated().to_string())
}

pub(super) fn render_settings_managed_accounts(
    frame: &mut Frame<'_>,
    app: &App,
    _data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let keys = managed_account_key_items(app);
    let body = render_page_frame(
        frame,
        area,
        theme,
        app,
        &breadcrumb_path(&[
            texts::menu_settings(),
            texts::tui_settings_managed_accounts_title(),
        ]),
        &keys,
        Some(managed_accounts_page_summary(app)),
    );

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(44), Constraint::Percentage(56)])
        .split(body);

    render_managed_account_list(frame, app, columns[0], theme);
    render_managed_account_details(frame, app, columns[1], theme);
}

fn managed_accounts_page_summary(app: &App) -> String {
    if app.managed_auth_loading {
        return texts::tui_managed_accounts_summary_loading().to_string();
    }

    let Some(status) = app.managed_auth_status.as_ref() else {
        return texts::tui_managed_accounts_summary_not_loaded().to_string();
    };

    if status.accounts.is_empty() {
        return texts::tui_managed_accounts_summary_empty().to_string();
    }

    let default_account = status
        .default_account_id
        .as_ref()
        .and_then(|default_id| {
            status
                .accounts
                .iter()
                .find(|account| &account.id == default_id)
                .map(|account| account.login.as_str())
        })
        .or_else(|| {
            status
                .accounts
                .iter()
                .find(|account| account.is_default)
                .map(|account| account.login.as_str())
        })
        .unwrap_or_else(|| texts::none());

    texts::tui_managed_accounts_summary_loaded(status.accounts.len(), default_account)
}

fn managed_account_key_items(app: &App) -> Vec<(&'static str, &'static str)> {
    if app.managed_auth_login.is_some() {
        return vec![("Esc", texts::tui_key_cancel())];
    }

    let mut items = vec![
        ("a", texts::tui_key_add_account()),
        ("r", texts::tui_key_refresh()),
    ];

    if app.managed_auth_loading {
        return items;
    }

    match app.managed_auth_status.as_ref() {
        None => items.push(("Enter", texts::tui_key_refresh())),
        Some(status) if status.accounts.is_empty() => {
            items.push(("Enter", texts::tui_key_add_account()));
        }
        Some(_) => {
            items.push(("Space", texts::tui_key_switch()));
            items.push(("Enter", texts::tui_key_open()));
        }
    }

    items
}

fn render_managed_account_list(
    frame: &mut Frame<'_>,
    app: &App,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(if app.focus == Focus::Content {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.dim)
        })
        .title(format!(" {} ", texts::tui_managed_accounts_list_title()));
    frame.render_widget(block.clone(), area);
    let inner = inset_left(block.inner(area), CONTENT_INSET_LEFT);

    let status = app.managed_auth_status.as_ref();
    if app.managed_auth_loading && status.is_none() {
        render_managed_account_list_state(frame, inner, texts::tui_loading(), theme);
        return;
    }

    let Some(status) = status else {
        render_managed_account_list_state(
            frame,
            inner,
            texts::tui_managed_accounts_not_loaded(),
            theme,
        );
        return;
    };

    if status.accounts.is_empty() {
        render_managed_account_list_state(
            frame,
            inner,
            texts::tui_managed_accounts_not_authenticated(),
            theme,
        );
        return;
    }

    let width = inner.width.saturating_sub(1);
    let items = status
        .accounts
        .iter()
        .map(|account| managed_account_list_item(account, width, theme));

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = ListState::default();
    state.select(Some(app.settings_managed_accounts_idx));
    frame.render_stateful_widget(list, inner, &mut state);
}

fn render_managed_account_list_state(
    frame: &mut Frame<'_>,
    area: Rect,
    text: &'static str,
    theme: &super::theme::Theme,
) {
    frame.render_widget(
        Paragraph::new(Line::styled(text, Style::default().fg(theme.comment)))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn managed_account_list_item(
    account: &crate::services::ManagedAuthAccount,
    width: u16,
    theme: &super::theme::Theme,
) -> ListItem<'static> {
    let marker_style = if account.is_default {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.dim)
    };
    let login_style = Style::default().add_modifier(Modifier::BOLD);
    let default_label = texts::tui_managed_accounts_default();
    let default_chip_width = u16::try_from(UnicodeWidthStr::width(default_label))
        .unwrap_or(u16::MAX)
        .saturating_add(2);
    let login_width = if account.is_default {
        width
            .saturating_sub(default_chip_width)
            .saturating_sub(5)
            .max(4)
    } else {
        width.saturating_sub(3).max(4)
    };
    let meta_width = width.saturating_sub(3).max(4);
    let account_id = truncate_to_display_width(&account.id, meta_width.saturating_sub(12));
    let meta = truncate_to_display_width(
        &format!(
            "{} · {} · {} {account_id}",
            texts::tui_managed_accounts_chatgpt_provider(),
            texts::tui_managed_accounts_authenticated(),
            texts::tui_label_id()
        ),
        meta_width,
    );

    let mut title_spans = vec![
        Span::styled(
            if account.is_default {
                texts::tui_marker_active()
            } else {
                texts::tui_marker_inactive()
            },
            marker_style,
        ),
        Span::raw("  "),
        Span::styled(
            truncate_to_display_width(&account.login, login_width),
            login_style,
        ),
    ];
    if account.is_default {
        title_spans.push(Span::raw("  "));
        title_spans.push(Span::styled(
            format!(" {default_label} "),
            active_chip_style(theme),
        ));
    }

    ListItem::new(vec![
        Line::from(title_spans),
        Line::from(vec![
            Span::raw("   "),
            Span::styled(meta, Style::default().fg(theme.comment)),
        ]),
    ])
}

fn render_managed_account_details(
    frame: &mut Frame<'_>,
    app: &App,
    area: Rect,
    theme: &super::theme::Theme,
) {
    render_managed_account_detail_panel(frame, app, area, theme);
}

fn render_managed_account_detail_panel(
    frame: &mut Frame<'_>,
    app: &App,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.dim))
        .title(format!(" {} ", texts::tui_managed_accounts_details_title()));
    frame.render_widget(block.clone(), area);
    let inner = inset_left(block.inner(area), CONTENT_INSET_LEFT);

    frame.render_widget(
        Paragraph::new(managed_account_detail_lines(app, theme)).wrap(Wrap { trim: false }),
        inner,
    );
}

fn managed_account_detail_lines(app: &App, theme: &super::theme::Theme) -> Vec<Line<'static>> {
    let label_width = managed_account_detail_label_width();
    let mut lines = Vec::new();

    if app.managed_auth_loading {
        lines.push(managed_account_detail_field(
            texts::tui_managed_accounts_auth_status_label(),
            texts::tui_loading().to_string(),
            Style::default().fg(theme.comment),
            label_width,
            theme,
        ));
        return lines;
    }

    let Some(status) = app.managed_auth_status.as_ref() else {
        lines.push(managed_account_detail_field(
            texts::tui_managed_accounts_auth_status_label(),
            texts::tui_managed_accounts_not_loaded().to_string(),
            Style::default().fg(theme.comment),
            label_width,
            theme,
        ));
        return lines;
    };

    let default_account = status
        .default_account_id
        .as_ref()
        .and_then(|default_id| {
            status
                .accounts
                .iter()
                .find(|account| &account.id == default_id)
                .map(|account| account.login.clone())
        })
        .unwrap_or_else(|| texts::none().to_string());

    let Some(account) = selected_managed_account(app, status) else {
        lines.push(managed_account_detail_field(
            texts::tui_managed_accounts_default_account_label(),
            default_account,
            Style::default(),
            label_width,
            theme,
        ));
        lines.push(managed_account_detail_field(
            texts::tui_managed_accounts_account_label(),
            texts::tui_managed_accounts_count(status.accounts.len()),
            Style::default(),
            label_width,
            theme,
        ));
        lines.push(managed_account_detail_field(
            texts::tui_managed_accounts_auth_status_label(),
            texts::tui_managed_accounts_not_authenticated().to_string(),
            Style::default().fg(theme.comment),
            label_width,
            theme,
        ));
        return lines;
    };

    lines.push(Line::from(vec![
        Span::styled(
            account.login.clone(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            if account.is_default {
                format!(" {} ", texts::tui_managed_accounts_default())
            } else {
                format!(" {} ", texts::tui_managed_accounts_authenticated())
            },
            if account.is_default {
                active_chip_style(theme)
            } else {
                inactive_chip_style(theme)
            },
        ),
    ]));
    lines.push(Line::raw(""));
    lines.push(managed_account_detail_field(
        texts::tui_managed_accounts_provider_column(),
        texts::tui_managed_accounts_chatgpt_provider().to_string(),
        Style::default(),
        label_width,
        theme,
    ));
    lines.push(managed_account_detail_field(
        texts::tui_managed_accounts_default_account_label(),
        default_account,
        Style::default(),
        label_width,
        theme,
    ));
    lines.push(managed_account_detail_field(
        texts::tui_managed_accounts_account_label(),
        texts::tui_managed_accounts_count(status.accounts.len()),
        Style::default(),
        label_width,
        theme,
    ));
    lines.push(Line::raw(""));
    lines.push(managed_account_detail_field(
        texts::tui_managed_accounts_account_id_label(),
        account.id.clone(),
        Style::default().fg(theme.comment),
        label_width,
        theme,
    ));
    lines.push(managed_account_detail_field(
        texts::tui_managed_accounts_authenticated_at_label(),
        format_managed_account_authenticated_at(account.authenticated_at),
        Style::default().fg(theme.comment),
        label_width,
        theme,
    ));
    lines
}

fn managed_account_detail_label_width() -> usize {
    [
        texts::tui_managed_accounts_provider_column(),
        texts::tui_managed_accounts_default_account_label(),
        texts::tui_managed_accounts_account_label(),
        texts::tui_managed_accounts_auth_status_label(),
        texts::tui_managed_accounts_account_id_label(),
        texts::tui_managed_accounts_authenticated_at_label(),
    ]
    .into_iter()
    .map(UnicodeWidthStr::width)
    .max()
    .unwrap_or(0)
}

fn managed_account_detail_field(
    label: &'static str,
    value: String,
    value_style: Style,
    label_width: usize,
    theme: &super::theme::Theme,
) -> Line<'static> {
    kv_line(
        theme,
        label,
        label_width,
        vec![Span::styled(value, value_style)],
    )
}

fn format_managed_account_authenticated_at(timestamp: i64) -> String {
    if timestamp <= 0 {
        return texts::tui_na().to_string();
    }

    format_sync_time_local_to_minute(timestamp).unwrap_or_else(|| texts::tui_na().to_string())
}

fn selected_managed_account<'a>(
    app: &App,
    status: &'a crate::services::ManagedAuthStatus,
) -> Option<&'a crate::services::ManagedAuthAccount> {
    status.accounts.get(
        app.settings_managed_accounts_idx
            .min(status.accounts.len().saturating_sub(1)),
    )
}

fn primary_managed_account(
    status: &crate::services::ManagedAuthStatus,
) -> Option<&crate::services::ManagedAuthAccount> {
    status
        .accounts
        .iter()
        .find(|account| account.is_default)
        .or_else(|| status.accounts.first())
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
        .title(breadcrumb_title(&[
            texts::menu_settings(),
            texts::tui_settings_proxy_title(),
        ]));
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

    let key_label = match LocalProxySettingsItem::ALL.get(app.settings_proxy_idx) {
        Some(LocalProxySettingsItem::AutoFailover) => texts::tui_key_toggle(),
        Some(LocalProxySettingsItem::ListenAddress) if data.proxy.running => "",
        Some(LocalProxySettingsItem::ListenPort)
            if data.proxy.has_active_worker_for(&app.app_type) =>
        {
            ""
        }
        _ => texts::tui_key_edit(),
    };
    if !key_label.is_empty() {
        render_page_key_bar(
            frame,
            chunks[0],
            theme,
            &[("Enter", key_label)],
            app.focus == Focus::Content,
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
    state.select(Some(app.settings_proxy_idx));
    frame.render_stateful_widget(table, inset_left(chunks[1], CONTENT_INSET_LEFT), &mut state);

    let hint = if !data.proxy.running {
        texts::tui_settings_proxy_restart_hint()
    } else {
        let current_app_has_active_worker = data.proxy.has_active_worker_for(&app.app_type);
        texts::tui_settings_proxy_stop_before_edit_hint(current_app_has_active_worker)
    };
    frame.render_widget(
        Paragraph::new(hint)
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.dim)),
        chunks[2],
    );
}
