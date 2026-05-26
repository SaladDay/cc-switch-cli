use std::path::Path;

use super::*;

pub(super) fn render_sessions(
    frame: &mut Frame<'_>,
    app: &App,
    _data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let visible = app::visible_sessions(&app.filter, &app.app_type, &app.sessions.rows);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(texts::tui_sessions_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(inner);

    if app.focus == Focus::Content {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[
                ("↑↓", texts::tui_key_select()),
                ("←→/h/l", texts::tui_key_focus()),
                ("Enter", texts::tui_key_view()),
                ("R", texts::tui_key_restore()),
                ("d", texts::tui_key_delete()),
                ("r", texts::tui_key_refresh()),
            ],
        );
    }

    let summary = if app.sessions.loading && !app.sessions.loaded_once {
        texts::tui_sessions_loading_summary().to_string()
    } else {
        texts::tui_sessions_summary(app.sessions.rows.len(), visible.len())
    };
    render_summary_bar(frame, chunks[1], theme, summary);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(44), Constraint::Percentage(56)])
        .split(chunks[2]);

    render_session_list(frame, app, &visible, body[0], theme);
    render_session_detail(frame, app, &visible, body[1], theme);
}

fn render_session_list(
    frame: &mut Frame<'_>,
    app: &App,
    visible: &[&crate::session_manager::SessionMeta],
    area: Rect,
    theme: &super::theme::Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(session_pane_border_style(app, SessionsPane::List, theme))
        .title(texts::menu_manage_sessions());
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);

    if app.sessions.loading && !app.sessions.loaded_once {
        render_centered_lines(
            frame,
            inner,
            vec![Line::styled(
                texts::tui_sessions_loading_summary(),
                Style::default().fg(theme.comment),
            )],
        );
        return;
    }

    if let Some(error) = app.sessions.last_error.as_deref() {
        render_centered_lines(
            frame,
            inner,
            vec![
                Line::styled(
                    texts::tui_sessions_error_title(),
                    Style::default().fg(theme.warn).add_modifier(Modifier::BOLD),
                ),
                Line::raw(""),
                Line::styled(error.to_string(), Style::default().fg(theme.comment)),
            ],
        );
        return;
    }

    if visible.is_empty() {
        render_centered_lines(
            frame,
            inner,
            vec![
                Line::styled(
                    texts::tui_sessions_empty_title(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Line::raw(""),
                Line::styled(
                    texts::tui_sessions_empty_subtitle(),
                    Style::default().fg(theme.comment),
                ),
            ],
        );
        return;
    }

    let header = Row::new(vec![
        Cell::from(texts::tui_sessions_header_title()),
        Cell::from(texts::tui_sessions_header_time()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let rows = visible.iter().map(|session| {
        let title = session_title(session);
        let time = session
            .last_active_at
            .or(session.created_at)
            .map(|timestamp| format_relative_time(timestamp, app.sessions.time_anchor_ms))
            .unwrap_or_else(|| texts::tui_na().to_string());
        let project = session
            .project_dir
            .as_deref()
            .map(path_basename)
            .filter(|value| !value.is_empty());
        let title_line = match project {
            Some(project) => Line::from(vec![
                Span::raw(title),
                Span::styled(format!("  {project}"), Style::default().fg(theme.comment)),
            ]),
            None => Line::raw(title),
        };
        Row::new(vec![Cell::from(title_line), Cell::from(time)])
    });

    let table = Table::new(rows, [Constraint::Percentage(72), Constraint::Length(12)])
        .header(header)
        .block(Block::default().borders(Borders::NONE))
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.sessions.selected_idx));
    frame.render_stateful_widget(table, inset_left(inner, CONTENT_INSET_LEFT), &mut state);
}

fn render_session_detail(
    frame: &mut Frame<'_>,
    app: &App,
    visible: &[&crate::session_manager::SessionMeta],
    area: Rect,
    theme: &super::theme::Theme,
) {
    let selected = selected_session(app, visible);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(6), Constraint::Min(0)])
        .split(area);

    render_session_overview(frame, selected, chunks[0], theme);
    render_session_messages(frame, app, chunks[1], theme);
}

fn render_session_overview(
    frame: &mut Frame<'_>,
    session: Option<&crate::session_manager::SessionMeta>,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.dim))
        .title(texts::tui_sessions_overview_title());
    frame.render_widget(block.clone(), area);

    let Some(session) = session else {
        return;
    };

    let inner = inset_left(block.inner(area), CONTENT_INSET_LEFT);
    let value_width = inner.width.saturating_sub(16);
    let time = session
        .last_active_at
        .or(session.created_at)
        .map(format_timestamp)
        .unwrap_or_else(|| texts::tui_na().to_string());
    let project = session
        .project_dir
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(texts::tui_na());
    let title = session_title(session);
    let resume_command = session
        .resume_command
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(texts::tui_na());

    let lines = vec![
        overview_field_line(
            texts::tui_sessions_overview_time_label(),
            &time,
            value_width,
            theme,
        ),
        overview_field_line(
            texts::tui_sessions_overview_workdir_label(),
            project,
            value_width,
            theme,
        ),
        overview_field_line(
            texts::tui_sessions_overview_summary_label(),
            &title,
            value_width,
            theme,
        ),
        overview_field_line(
            texts::tui_sessions_resume_command(),
            resume_command,
            value_width,
            theme,
        ),
    ];

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn overview_field_line<'a>(
    label: &'static str,
    value: &'a str,
    value_width: u16,
    theme: &super::theme::Theme,
) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("{}  ", pad_to_display_width(label, 12)),
            Style::default().fg(theme.dim),
        ),
        Span::raw(truncate_to_display_width(value, value_width)),
    ])
}

fn render_session_messages(
    frame: &mut Frame<'_>,
    app: &App,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(session_pane_border_style(app, SessionsPane::Detail, theme))
        .title(texts::tui_sessions_messages_title());
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);

    if app.sessions.messages_loading {
        render_centered_lines(
            frame,
            inner,
            vec![Line::styled(
                texts::tui_sessions_messages_loading(),
                Style::default().fg(theme.comment),
            )],
        );
        return;
    }

    if let Some(error) = app.sessions.messages_error.as_deref() {
        render_centered_lines(
            frame,
            inner,
            vec![Line::styled(
                error.to_string(),
                Style::default().fg(theme.warn),
            )],
        );
        return;
    }

    if !app.sessions.messages_loaded {
        render_centered_lines(
            frame,
            inner,
            vec![Line::styled(
                texts::tui_sessions_messages_not_loaded(),
                Style::default().fg(theme.comment),
            )],
        );
        return;
    }

    if app.sessions.messages.is_empty() {
        render_centered_lines(
            frame,
            inner,
            vec![Line::styled(
                texts::tui_sessions_messages_empty(),
                Style::default().fg(theme.comment),
            )],
        );
        return;
    }

    let visible = visible_message_window(
        &app.sessions.messages,
        app.sessions.message_idx,
        inner.height,
    );
    let rows = visible.map(|message| {
        let role = texts::tui_sessions_role_label(&message.role);
        let preview = collapse_message_preview(&message.content);
        let time = message.ts.map(format_timestamp).unwrap_or_default();
        Row::new(vec![
            Cell::from(role),
            Cell::from(preview),
            Cell::from(time),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(10),
            Constraint::Percentage(70),
            Constraint::Length(16),
        ],
    )
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    if matches!(app.sessions.pane, SessionsPane::Detail) {
        state.select(Some(app.sessions.message_idx.saturating_sub(
            message_window_start(
                app.sessions.messages.len(),
                app.sessions.message_idx,
                inner.height,
            ),
        )));
    }
    frame.render_stateful_widget(table, inset_left(inner, CONTENT_INSET_LEFT), &mut state);
}

fn session_pane_border_style(app: &App, pane: SessionsPane, theme: &super::theme::Theme) -> Style {
    if app.focus == Focus::Content && app.sessions.pane == pane {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.dim)
    }
}

fn selected_session<'a>(
    app: &App,
    visible: &'a [&'a crate::session_manager::SessionMeta],
) -> Option<&'a crate::session_manager::SessionMeta> {
    app.sessions
        .detail_key
        .as_deref()
        .and_then(|key| {
            visible
                .iter()
                .copied()
                .find(|session| app::session_key(session) == key)
        })
        .or_else(|| visible.get(app.sessions.selected_idx).copied())
}

fn render_centered_lines(frame: &mut Frame<'_>, area: Rect, content_lines: Vec<Line<'static>>) {
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

fn session_title(session: &crate::session_manager::SessionMeta) -> String {
    session
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            session
                .project_dir
                .as_deref()
                .map(path_basename)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| session.session_id.chars().take(8).collect())
}

fn path_basename(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return String::new();
    }
    Path::new(trimmed)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(trimmed)
        .to_string()
}

fn format_timestamp(timestamp_ms: i64) -> String {
    Local
        .timestamp_millis_opt(timestamp_ms)
        .single()
        .map(|dt| dt.format("%Y/%m/%d %H:%M").to_string())
        .unwrap_or_else(|| texts::tui_na().to_string())
}

fn format_date(timestamp_ms: i64) -> String {
    Local
        .timestamp_millis_opt(timestamp_ms)
        .single()
        .map(|dt| dt.format("%Y/%m/%d").to_string())
        .unwrap_or_else(|| texts::tui_na().to_string())
}

fn format_relative_time(timestamp_ms: i64, now_ms: i64) -> String {
    let diff = now_ms.saturating_sub(timestamp_ms);
    let minutes = diff / 60_000;
    let hours = diff / 3_600_000;
    let days = diff / 86_400_000;

    if minutes < 1 {
        texts::tui_sessions_just_now().to_string()
    } else if minutes < 60 {
        texts::tui_sessions_minutes_ago(minutes)
    } else if hours < 24 {
        texts::tui_sessions_hours_ago(hours)
    } else if days < 7 {
        texts::tui_sessions_days_ago(days)
    } else {
        format_date(timestamp_ms)
    }
}

fn collapse_message_preview(content: &str) -> String {
    let single_line = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    truncate_to_display_width(&single_line, 120)
}

fn message_window_start(total: usize, selected: usize, height: u16) -> usize {
    let visible_rows = height.saturating_sub(1).max(1) as usize;
    if total <= visible_rows {
        return 0;
    }
    selected
        .saturating_sub(visible_rows / 2)
        .min(total - visible_rows)
}

fn visible_message_window(
    messages: &[crate::session_manager::SessionMessage],
    selected: usize,
    height: u16,
) -> impl Iterator<Item = &crate::session_manager::SessionMessage> {
    let visible_rows = height.saturating_sub(1).max(1) as usize;
    let start = message_window_start(messages.len(), selected, height);
    let end = (start + visible_rows).min(messages.len());
    messages[start..end].iter()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_title_prefers_title_then_project_basename_then_short_id() {
        let titled = crate::session_manager::SessionMeta {
            provider_id: "codex".to_string(),
            session_id: "abcdef123456".to_string(),
            title: Some("Refactor".to_string()),
            summary: None,
            project_dir: Some("/tmp/project".to_string()),
            created_at: None,
            last_active_at: None,
            source_path: None,
            resume_command: None,
        };
        assert_eq!(session_title(&titled), "Refactor");

        let project = crate::session_manager::SessionMeta {
            title: None,
            ..titled.clone()
        };
        assert_eq!(session_title(&project), "project");

        let fallback = crate::session_manager::SessionMeta {
            title: None,
            project_dir: None,
            ..titled
        };
        assert_eq!(session_title(&fallback), "abcdef12");
    }

    #[test]
    fn message_window_centers_selected_row() {
        assert_eq!(message_window_start(100, 50, 10), 46);
        assert_eq!(message_window_start(5, 4, 10), 0);
        assert_eq!(message_window_start(100, 99, 10), 91);
    }

    #[test]
    fn relative_time_matches_upstream_thresholds() {
        let _lang = crate::cli::i18n::use_test_language(crate::cli::i18n::Language::English);
        let now = 1_735_689_900_000;

        assert_eq!(format_relative_time(now - 30_000, now), "Just now");
        assert_eq!(format_relative_time(now - 5 * 60_000, now), "5 min ago");
        assert_eq!(format_relative_time(now - 3 * 3_600_000, now), "3 hr ago");
        assert_eq!(
            format_relative_time(now - 2 * 86_400_000, now),
            "2 days ago"
        );
        assert_eq!(
            format_relative_time(now - 7 * 86_400_000, now),
            format_date(now - 7 * 86_400_000)
        );
    }
}
