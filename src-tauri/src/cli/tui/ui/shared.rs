use super::*;
use crate::cli::tui::data;

pub(super) fn pane_border_style(app: &App, pane: Focus, theme: &super::theme::Theme) -> Style {
    if app.focus == pane {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.dim)
    }
}

pub(super) fn selection_style(theme: &super::theme::Theme) -> Style {
    if theme.no_color {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
            .fg(Color::Black)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    }
}

pub(super) fn inactive_chip_style(theme: &super::theme::Theme) -> Style {
    if theme.no_color {
        Style::default()
    } else {
        Style::default().fg(Color::White).bg(theme.surface)
    }
}

pub(super) fn active_chip_style(theme: &super::theme::Theme) -> Style {
    if theme.no_color {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
            .fg(Color::Black)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    }
}

/// Border style for overlay dialogs.
/// `attention = true` for overlays that require user action (Confirm, Update prompts).
/// `attention = false` for informational overlays (Help, TextView, pickers).
pub(super) fn overlay_border_style(theme: &super::theme::Theme, attention: bool) -> Style {
    if attention {
        Style::default().fg(theme.accent)
    } else {
        Style::default().fg(theme.dim)
    }
}

pub(super) fn transient_feedback_color(theme: &super::theme::Theme, kind: &ToastKind) -> Color {
    match kind {
        ToastKind::Info | ToastKind::Success => theme.accent,
        ToastKind::Warning => theme.warn,
        ToastKind::Error => theme.err,
    }
}

/// Left-pad a cell value with one space for visual inset inside table rows.
pub(super) fn cell_pad(s: &str) -> String {
    format!(" {s}")
}

pub(super) fn strip_trailing_colon(label: &str) -> &str {
    label.trim_end_matches([':', '：'])
}

pub(super) fn pad_to_display_width(label: &str, width: usize) -> String {
    let clean = strip_trailing_colon(label);
    let w = UnicodeWidthStr::width(clean);
    if w >= width {
        clean.to_string()
    } else {
        format!("{clean}{}", " ".repeat(width - w))
    }
}

pub(super) fn truncate_to_display_width(text: &str, width: u16) -> String {
    let width = width as usize;
    if width == 0 {
        return String::new();
    }

    if UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }

    if width == 1 {
        return "…".to_string();
    }

    let mut out = String::new();
    let mut used = 0usize;
    for c in text.chars() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if used.saturating_add(w) > width.saturating_sub(1) {
            break;
        }
        out.push(c);
        used = used.saturating_add(w);
    }
    out.push('…');
    out
}

pub(super) fn format_sync_time_local_to_minute(ts: i64) -> Option<String> {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%Y/%m/%d %H:%M").to_string())
}

pub(super) fn format_uptime_compact(total_seconds: u64) -> String {
    let days = total_seconds / 86_400;
    let hours = (total_seconds % 86_400) / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;

    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    if seconds > 0 || parts.is_empty() {
        parts.push(format!("{seconds}s"));
    }

    parts.join(" ")
}

pub(super) fn format_estimated_token_compact(total: u64) -> String {
    if total < 1_000 {
        return format!("~{total}");
    }

    if total < 10_000 {
        return format!("~{:.1}k", total as f64 / 1_000.0);
    }

    if total < 1_000_000 {
        return format!("~{}k", total / 1_000);
    }

    if total < 10_000_000 {
        return format!("~{:.1}M", total as f64 / 1_000_000.0);
    }

    format!("~{}M", total / 1_000_000)
}

fn quota_tier_label(name: &str) -> String {
    match name {
        "five_hour" => texts::tui_quota_tier_five_hour().to_string(),
        "seven_day" => texts::tui_quota_tier_seven_day().to_string(),
        "seven_day_opus" => texts::tui_quota_tier_seven_day_opus().to_string(),
        "seven_day_sonnet" => texts::tui_quota_tier_seven_day_sonnet().to_string(),
        "weekly_limit" => texts::tui_quota_tier_weekly_limit().to_string(),
        "premium" => texts::tui_quota_tier_premium().to_string(),
        "gemini_pro" => texts::tui_quota_tier_gemini_pro().to_string(),
        "gemini_flash" => texts::tui_quota_tier_gemini_flash().to_string(),
        "gemini_flash_lite" => texts::tui_quota_tier_gemini_flash_lite().to_string(),
        other => other.replace('_', " "),
    }
}

fn quota_percent_text(utilization: f64) -> String {
    format!("{:.0}%", utilization.clamp(0.0, 100.0))
}

fn quota_utilization_style(theme: &super::theme::Theme, utilization: f64) -> Style {
    if theme.no_color {
        return Style::default();
    }

    if utilization >= 90.0 {
        Style::default().fg(theme.err)
    } else if utilization >= 70.0 {
        Style::default().fg(theme.warn)
    } else {
        Style::default().fg(theme.ok)
    }
}

fn quota_countdown(resets_at: Option<&str>) -> Option<String> {
    let resets_at = resets_at?;
    let reset = chrono::DateTime::parse_from_rfc3339(resets_at).ok()?;
    let diff_ms = reset.timestamp_millis() - chrono::Utc::now().timestamp_millis();
    if diff_ms <= 0 {
        return None;
    }

    let total_minutes = diff_ms / 60_000;
    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;
    if hours > 24 {
        Some(format!("{}d{}h", hours / 24, hours % 24))
    } else if hours > 0 {
        Some(format!("{hours}h{minutes}m"))
    } else {
        Some(format!("{minutes}m"))
    }
}

fn quota_relative_time(timestamp_ms: i64) -> String {
    let diff_secs = ((chrono::Utc::now().timestamp_millis() - timestamp_ms).max(0)) / 1000;
    if diff_secs < 60 {
        texts::tui_quota_just_now().to_string()
    } else if diff_secs < 3600 {
        texts::tui_quota_minutes_ago(diff_secs / 60)
    } else if diff_secs < 86_400 {
        texts::tui_quota_hours_ago(diff_secs / 3600)
    } else {
        texts::tui_quota_days_ago(diff_secs / 86_400)
    }
}

fn quota_relative_time_compact(timestamp_ms: i64) -> String {
    let diff_secs = ((chrono::Utc::now().timestamp_millis() - timestamp_ms).max(0)) / 1000;
    let (value, unit) = if diff_secs < 60 {
        (diff_secs.max(1), "s")
    } else if diff_secs < 3600 {
        (diff_secs / 60, "m")
    } else if diff_secs < 86_400 {
        (diff_secs / 3600, "h")
    } else {
        (diff_secs / 86_400, "d")
    };

    if i18n::is_chinese() {
        format!("{value}{unit}前")
    } else {
        format!("{value}{unit} ago")
    }
}

pub(super) fn quota_compact_line(
    state: Option<&data::ProviderQuotaState>,
    theme: &super::theme::Theme,
    quiet_missing: bool,
) -> Option<Line<'static>> {
    let Some(state) = state else {
        return None;
    };

    if state.loading && state.quota.is_none() {
        return Some(Line::from(Span::styled(
            texts::tui_quota_loading().to_string(),
            Style::default().fg(theme.surface),
        )));
    }

    if state.last_error.is_some() && state.quota.is_none() {
        return Some(Line::from(Span::styled(
            texts::tui_quota_query_failed().to_string(),
            Style::default().fg(theme.warn),
        )));
    }

    let quota = state.quota.as_ref()?;
    match quota.credential_status {
        crate::services::CredentialStatus::NotFound => {
            if quiet_missing {
                return None;
            }
            return Some(Line::from(Span::styled(
                texts::tui_quota_not_available().to_string(),
                Style::default().fg(theme.surface),
            )));
        }
        crate::services::CredentialStatus::ParseError => {
            if quiet_missing {
                return None;
            }
            return Some(Line::from(Span::styled(
                texts::tui_quota_parse_error().to_string(),
                Style::default().fg(theme.warn),
            )));
        }
        crate::services::CredentialStatus::Expired if !quota.success => {
            return Some(Line::from(Span::styled(
                texts::tui_quota_expired().to_string(),
                Style::default().fg(theme.warn),
            )));
        }
        _ => {}
    }

    if !quota.success {
        return Some(Line::from(Span::styled(
            texts::tui_quota_query_failed().to_string(),
            Style::default().fg(theme.err),
        )));
    }

    let tiers = quota
        .tiers
        .iter()
        .filter(|tier| tier.name != "seven_day_sonnet")
        .take(2)
        .collect::<Vec<_>>();
    if tiers.is_empty() {
        if quiet_missing {
            return None;
        }
        return Some(Line::from(Span::styled(
            texts::tui_quota_not_available().to_string(),
            Style::default().fg(theme.surface),
        )));
    }

    let mut spans = Vec::new();
    for (idx, tier) in tiers.iter().enumerate() {
        if idx > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            format!("{} ", quota_tier_label(&tier.name)),
            Style::default().fg(theme.comment),
        ));
        spans.push(Span::styled(
            quota_percent_text(tier.utilization),
            quota_utilization_style(theme, tier.utilization),
        ));
    }
    if let Some(checked) = quota.queried_at.map(quota_relative_time_compact) {
        if !spans.is_empty() {
            spans.push(Span::styled(" | ", Style::default().fg(theme.comment)));
        }
        spans.push(Span::styled(checked, Style::default().fg(theme.surface)));
    }
    if state.loading {
        if !spans.is_empty() {
            spans.push(Span::styled(" | ", Style::default().fg(theme.comment)));
        }
        spans.push(Span::styled(
            texts::tui_quota_loading().to_string(),
            Style::default().fg(theme.surface),
        ));
    }
    Some(Line::from(spans))
}

pub(super) fn quota_detail_lines(
    app: &App,
    data: &UiData,
    row: &ProviderRow,
    theme: &super::theme::Theme,
) -> Vec<Line<'static>> {
    if data::quota_target_for_provider(&app.app_type, row).is_none() {
        return Vec::new();
    }

    let label_style = Style::default().fg(theme.accent);
    let value_style = Style::default().fg(theme.cyan);
    let muted_style = Style::default().fg(theme.surface);
    let state = data.quota.state_for(&row.id);
    let mut lines = Vec::new();
    lines.push(Line::raw(""));

    let mut push_kv = |label: String, spans: Vec<Span<'static>>| {
        let mut line_spans = vec![Span::styled(label, label_style), Span::raw(": ")];
        line_spans.extend(spans);
        lines.push(Line::from(line_spans));
    };

    let Some(state) = state else {
        push_kv(
            texts::tui_label_quota().to_string(),
            vec![
                Span::styled(texts::tui_quota_not_queried().to_string(), muted_style),
                Span::raw("  "),
                Span::styled(texts::tui_quota_refresh_hint().to_string(), muted_style),
            ],
        );
        return lines;
    };

    if state.loading && state.quota.is_none() {
        push_kv(
            texts::tui_label_quota().to_string(),
            vec![Span::styled(
                texts::tui_quota_loading().to_string(),
                muted_style,
            )],
        );
        return lines;
    }

    if let Some(error) = state
        .last_error
        .as_deref()
        .filter(|_| state.quota.is_none())
    {
        push_kv(
            texts::tui_label_quota().to_string(),
            vec![
                Span::styled(
                    texts::tui_quota_query_failed().to_string(),
                    Style::default().fg(theme.warn),
                ),
                Span::raw("  "),
                Span::raw(error.to_string()),
            ],
        );
        return lines;
    }

    let Some(quota) = state.quota.as_ref() else {
        push_kv(
            texts::tui_label_quota().to_string(),
            vec![Span::styled(
                texts::tui_quota_not_queried().to_string(),
                muted_style,
            )],
        );
        return lines;
    };

    match quota.credential_status {
        crate::services::CredentialStatus::NotFound => {
            push_kv(
                texts::tui_label_quota().to_string(),
                vec![Span::styled(
                    texts::tui_quota_not_available().to_string(),
                    muted_style,
                )],
            );
            return lines;
        }
        crate::services::CredentialStatus::ParseError => {
            push_kv(
                texts::tui_label_quota().to_string(),
                vec![
                    Span::styled(
                        texts::tui_quota_parse_error().to_string(),
                        Style::default().fg(theme.warn),
                    ),
                    Span::raw("  "),
                    Span::raw(
                        quota
                            .credential_message
                            .clone()
                            .or_else(|| quota.error.clone())
                            .unwrap_or_default(),
                    ),
                ],
            );
            return lines;
        }
        crate::services::CredentialStatus::Expired if !quota.success => {
            push_kv(
                texts::tui_label_quota().to_string(),
                vec![
                    Span::styled(
                        texts::tui_quota_expired().to_string(),
                        Style::default().fg(theme.warn),
                    ),
                    Span::raw("  "),
                    Span::raw(
                        quota
                            .credential_message
                            .clone()
                            .or_else(|| quota.error.clone())
                            .unwrap_or_default(),
                    ),
                ],
            );
            return lines;
        }
        _ => {}
    }

    if !quota.success {
        push_kv(
            texts::tui_label_quota().to_string(),
            vec![
                Span::styled(
                    texts::tui_quota_query_failed().to_string(),
                    Style::default().fg(theme.err),
                ),
                Span::raw("  "),
                Span::raw(quota.error.clone().unwrap_or_default()),
            ],
        );
        return lines;
    }

    let checked = quota
        .queried_at
        .map(quota_relative_time)
        .unwrap_or_else(|| texts::tui_na().to_string());
    push_kv(
        texts::tui_label_quota().to_string(),
        vec![
            Span::styled(
                texts::tui_quota_ok().to_string(),
                Style::default().fg(theme.ok),
            ),
            Span::raw("  "),
            Span::styled(
                texts::tui_quota_last_checked(),
                Style::default().fg(theme.comment),
            ),
            Span::raw(" "),
            Span::styled(checked, value_style),
        ],
    );

    for tier in &quota.tiers {
        let mut spans = vec![Span::styled(
            quota_percent_text(tier.utilization),
            quota_utilization_style(theme, tier.utilization),
        )];
        if let Some(reset) = quota_countdown(tier.resets_at.as_deref()) {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                texts::tui_quota_resets_in(&reset),
                Style::default().fg(theme.comment),
            ));
        }
        push_kv(quota_tier_label(&tier.name), spans);
    }

    if let Some(extra) = quota.extra_usage.as_ref().filter(|extra| extra.is_enabled) {
        let mut parts = Vec::new();
        if let Some(used) = extra.used_credits {
            parts.push(format!("{used:.1}"));
        }
        if let Some(limit) = extra.monthly_limit {
            parts.push(format!("/ {limit:.1}"));
        }
        if let Some(currency) = extra.currency.as_deref() {
            parts.push(currency.to_string());
        }
        if let Some(utilization) = extra.utilization {
            parts.push(format!("({})", quota_percent_text(utilization)));
        }
        if !parts.is_empty() {
            push_kv(
                texts::tui_quota_extra_usage().to_string(),
                vec![Span::styled(parts.join(" "), value_style)],
            );
        }
    }

    lines
}

pub(super) fn kv_line<'a>(
    theme: &super::theme::Theme,
    label: &'a str,
    label_width: usize,
    value_spans: Vec<Span<'a>>,
) -> Line<'a> {
    let mut spans = vec![
        Span::raw(" "), // internal padding: keep content away from │
        Span::styled(
            pad_to_display_width(label, label_width),
            Style::default()
                .fg(theme.comment)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(": "),
    ];
    spans.extend(value_spans);
    Line::from(spans)
}

pub(super) fn highlight_symbol(theme: &super::theme::Theme) -> &'static str {
    if theme.no_color {
        texts::tui_highlight_symbol()
    } else {
        ""
    }
}

pub(super) const CONTENT_INSET_LEFT: u16 = 1;

// Overlay size tiers — percentage-based (large content)
pub(super) const OVERLAY_LG: (u16, u16) = (90, 90);
pub(super) const OVERLAY_MD: (u16, u16) = (78, 62);
// Overlay size tiers — fixed character dimensions (dialogs)
pub(super) const OVERLAY_FIXED_LG: (u16, u16) = (70, 20);
pub(super) const OVERLAY_FIXED_MD: (u16, u16) = (60, 9);
pub(super) const OVERLAY_FIXED_SM: (u16, u16) = (50, 6);
pub(super) const TOAST_MIN_WIDTH: u16 = 28;
pub(super) const TOAST_MAX_WIDTH: u16 = 72;
pub(super) const TOAST_MIN_HEIGHT: u16 = 5;

pub(super) fn key_bar_line(theme: &super::theme::Theme, items: &[(&str, &str)]) -> Line<'static> {
    if theme.no_color {
        let mut parts = Vec::new();
        for (k, v) in items {
            parts.push(format!("{k}={v}"));
        }
        return Line::raw(parts.join("  "));
    }

    let base = inactive_chip_style(theme);
    let key = base.add_modifier(Modifier::BOLD);

    let mut spans: Vec<Span<'static>> = vec![Span::styled(" ", base)];
    for (idx, (k, v)) in items.iter().enumerate() {
        if idx > 0 {
            spans.push(Span::styled("  ", base));
        }
        spans.push(Span::styled((*k).to_string(), key));
        spans.push(Span::styled(" ", base));
        spans.push(Span::styled((*v).to_string(), base));
    }
    spans.push(Span::styled(" ", base));
    Line::from(spans)
}

/// Render a left-aligned key bar. Used for main-screen footers where keys
/// are read left-to-right in priority order.
pub(super) fn render_key_bar(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &super::theme::Theme,
    items: &[(&str, &str)],
) {
    frame.render_widget(
        Paragraph::new(key_bar_line(theme, items))
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// Render a center-aligned key bar. Used inside overlay dialogs where the
/// available actions are few and visually centered looks balanced.
pub(super) fn render_key_bar_center(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &super::theme::Theme,
    items: &[(&str, &str)],
) {
    frame.render_widget(
        Paragraph::new(key_bar_line(theme, items))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false }),
        area,
    );
}

pub(super) fn render_summary_bar(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &super::theme::Theme,
    summary: String,
) {
    let summary_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.dim));
    frame.render_widget(
        Paragraph::new(Line::raw(format!("  {summary}")))
            .style(Style::default().fg(theme.dim))
            .wrap(Wrap { trim: false })
            .block(summary_block),
        area,
    );
}

pub(super) fn inset_left(area: Rect, left: u16) -> Rect {
    if area.width <= left {
        return Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: area.height,
        };
    }
    Rect {
        x: area.x + left,
        y: area.y,
        width: area.width - left,
        height: area.height,
    }
}

pub(super) fn inset_top(area: Rect, top: u16) -> Rect {
    if area.height <= top {
        return Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: area.height,
        };
    }
    Rect {
        x: area.x,
        y: area.y + top,
        width: area.width,
        height: area.height - top,
    }
}

pub(super) fn field_label_column_width<'a, I>(labels: I, left_padding: u16) -> u16
where
    I: IntoIterator<Item = &'a str>,
{
    let max = labels
        .into_iter()
        .map(|label| UnicodeWidthStr::width(label) as u16)
        .max()
        .unwrap_or(0);
    max.saturating_add(left_padding)
}

pub(super) fn mask_api_key(key: &str) -> String {
    let mut iter = key.chars();
    let prefix: String = iter.by_ref().take(8).collect();
    if iter.next().is_some() {
        format!("{prefix}...")
    } else {
        prefix
    }
}

pub(super) fn redacted_secret_placeholder() -> &'static str {
    "[redacted]"
}

pub(super) fn redact_sensitive_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let next = if is_sensitive_display_key(key) {
                        redact_value_payload(value)
                    } else {
                        redact_sensitive_json(value)
                    };
                    (key.clone(), next)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact_sensitive_json).collect()),
        _ => value.clone(),
    }
}

fn redact_value_payload(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), redact_value_payload(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact_value_payload).collect()),
        Value::Null => Value::Null,
        _ => Value::String(redacted_secret_placeholder().to_string()),
    }
}

fn is_sensitive_display_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect::<String>();

    normalized == "authorization"
        || normalized.ends_with("authorization")
        || normalized.ends_with("apikey")
        || normalized.ends_with("token")
        || normalized.ends_with("password")
        || normalized.ends_with("secret")
        || normalized.ends_with("awsaccesskeyid")
        || normalized.ends_with("awssecretaccesskey")
        || normalized.ends_with("secretkey")
}
