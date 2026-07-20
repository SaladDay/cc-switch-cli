use std::path::Path;

use super::*;

pub(super) fn render_sessions(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let visible = app::visible_sessions_for_state(
        &app.filter,
        &app.app_type,
        app.sessions.provider_id.as_deref(),
        &app.sessions.project_scope,
        &app.sessions.rows,
        app.sessions.detail_key.as_deref(),
        app.sessions.messages_loaded,
        &app.sessions.messages,
        app.sessions.deep_search_query.as_deref(),
        &app.sessions.deep_search_results,
        app.sessions
            .materialized_view_is_current(app.filter.query_lower().as_deref()),
        app.sessions.rows_revision,
        app.sessions.messages_revision,
        app.sessions.deep_search_seq,
        &app.sessions.visibility_cache,
    );

    // Only primary navigation is shown here. h/l remain documented aliases in
    // contextual help; the action chips come from the dispatch keymap.
    let mut keys = vec![
        ("↑↓", texts::tui_key_select()),
        ("←→", texts::tui_key_pane()),
    ];
    keys.extend(crate::cli::tui::keymap::sessions::key_bar_items(app, data));
    let status = if app.sessions.loading && !app.sessions.loaded_once {
        texts::tui_sessions_loading_summary().to_string()
    } else if app.sessions.deep_search_active.is_some() {
        // Show spinner animation while deep search is running
        let spinner = match app.tick % 4 {
            0 => "⠋",
            1 => "⠙",
            2 => "⠹",
            _ => "⠸",
        };
        match app.sessions.deep_search_query.as_deref() {
            Some(query) => format!("{spinner} {}", texts::tui_sessions_searching(query)),
            None => format!("{spinner} {}", texts::tui_sessions_project_filtering()),
        }
    } else if app.sessions.loading {
        // Stale-while-revalidate: a cached snapshot is on screen (loaded_once) and
        // interactive, but the background revalidating scan is still running. Keep
        // the count visible and prefix a spinner so the header reads as refreshing.
        let spinner = match app.tick % 4 {
            0 => "⠋",
            1 => "⠙",
            2 => "⠹",
            _ => "⠸",
        };
        format!(
            "{spinner} {}",
            texts::tui_sessions_summary(app.sessions.logical_total_rows(), visible.len())
        )
    } else {
        texts::tui_sessions_summary(app.sessions.logical_total_rows(), visible.len())
    };
    let provider = crate::cli::tui::runtime_actions::app_display_name(&app.app_type);
    let project = match &app.sessions.project_scope {
        crate::session_manager::project_scope::SessionProjectScope::All => {
            texts::tui_sessions_all_projects()
        }
        crate::session_manager::project_scope::SessionProjectScope::Unknown => {
            texts::tui_sessions_unknown_project()
        }
        crate::session_manager::project_scope::SessionProjectScope::Exact {
            display_path, ..
        } => display_path.as_str(),
    };
    let summary_width = area.width.saturating_sub(8).max(1);
    let fixed = texts::tui_sessions_scope_summary(provider, "", &status);
    let project_width = summary_width
        .saturating_sub(UnicodeWidthStr::width(fixed.as_str()) as u16)
        .saturating_add(1)
        .max(5);
    let project = truncate_path_middle(project, project_width);
    let summary = truncate_to_display_width(
        &texts::tui_sessions_scope_summary(provider, &project, &status),
        summary_width,
    );
    let frame_body = render_page_frame(
        frame,
        area,
        theme,
        app,
        texts::tui_sessions_title(),
        &keys,
        Some(summary),
    );

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(44), Constraint::Percentage(56)])
        .split(frame_body);

    render_session_list(frame, app, &visible, body[0], theme);
    render_session_detail(frame, app, &visible, body[1], theme);
}

fn render_session_list(
    frame: &mut Frame<'_>,
    app: &App,
    visible: &app::SessionRowsView<'_>,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(session_pane_border_style(app, SessionsPane::List, theme))
        .title(format!(
            " {} ",
            icons::strip_icon(texts::menu_manage_sessions())
        ));
    let total_rows = app.sessions.logical_total_rows();
    if !visible.is_empty() && total_rows > 0 {
        let selected = app.sessions.selected_idx.min(visible.len() - 1);
        let page = app.sessions.remote.current_page();
        let page_start = page.saturating_mul(crate::session_manager::paged_manifest::PAGE_SIZE);
        let page_end = page_start.saturating_add(visible.len()).saturating_sub(1);
        let pending_page = app.sessions.remote.pending_cross_page();
        let status = if let Some(target) = pending_page {
            if app.sessions.remote.is_page_pending(target) {
                PaginationFooterStatus::LoadingPage(target + 1)
            } else {
                PaginationFooterStatus::PreparingNext
            }
        } else if app.sessions.remote.failed_page().is_some() {
            PaginationFooterStatus::LoadError
        } else {
            match app.sessions.pagination.focus() {
                app::paged_list::PagedListFocus::Boundary(app::paged_list::PageBoundary::Next) => {
                    PaginationFooterStatus::NextBoundary
                }
                app::paged_list::PagedListFocus::Boundary(
                    app::paged_list::PageBoundary::Previous,
                ) => PaginationFooterStatus::PreviousBoundary,
                app::paged_list::PagedListFocus::Empty | app::paged_list::PagedListFocus::Row
                    if page + 1
                        == total_rows
                            .div_ceil(crate::session_manager::paged_manifest::PAGE_SIZE)
                        && selected == visible.len() - 1 =>
                {
                    PaginationFooterStatus::End
                }
                app::paged_list::PagedListFocus::Empty | app::paged_list::PagedListFocus::Row => {
                    PaginationFooterStatus::Idle
                }
            }
        };
        block = with_pagination_footer(
            block,
            area.width,
            PaginationFooter {
                page: page + 1,
                start: page_start + 1,
                end: page_end + 1,
                total: total_rows,
                status,
            },
            theme,
            app.tick,
        );
    }
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

    if visible.is_empty() {
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

    // Only build Row objects for the rows actually on screen. Without this the
    // table allocates a title/time/Line/Span for every filtered session each
    // frame (O(n)); windowing keeps it O(viewport) even with thousands of rows.
    let total = visible.len();
    let selected = app.sessions.selected_idx.min(total.saturating_sub(1));
    let page_start = 0;
    let page_end = total;
    let page_len = page_end.saturating_sub(page_start);
    let start = page_start
        + message_window_start(page_len, selected.saturating_sub(page_start), inner.height);
    let visible_rows = inner.height.saturating_sub(1).max(1) as usize;
    let end = (start + visible_rows).min(page_end);

    let rows = (start..end)
        .filter_map(|index| visible.get(index))
        .map(|session| {
            let title = session_title(session);
            let time = session
                .last_active_at
                .or(session.created_at)
                .map(|timestamp| format_relative_time(timestamp, app.sessions.time_anchor_ms))
                .unwrap_or_else(|| texts::tui_na().to_string());
            let project = matches!(
                app.sessions.project_scope,
                crate::session_manager::project_scope::SessionProjectScope::All
            )
            .then(|| {
                session
                    .project_dir
                    .as_deref()
                    .map(path_basename)
                    .filter(|value| !value.is_empty())
            })
            .flatten();
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

    // The rows are pre-sliced to the window, so the highlight index is relative
    // to `start`.
    let mut state = TableState::default();
    if app.sessions.pagination.is_row_focused() {
        state.select(Some(selected - start));
    }
    frame.render_stateful_widget(table, inset_left(inner, CONTENT_INSET_LEFT), &mut state);
}

fn render_session_detail(
    frame: &mut Frame<'_>,
    app: &App,
    visible: &app::SessionRowsView<'_>,
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
        .title(format!(" {} ", texts::tui_sessions_overview_title()));
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
        .title(format!(
            " {} ",
            texts::tui_sessions_messages_preview_title(app.sessions.messages_truncated)
        ));
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

    let visible_messages = app::visible_session_messages(&app.sessions);
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

    if visible_messages.is_empty() {
        render_centered_lines(
            frame,
            inner,
            vec![Line::styled(
                texts::tui_sessions_messages_filtered_empty(),
                Style::default().fg(theme.comment),
            )],
        );
        return;
    }

    let selected_visible_idx =
        selected_message_visible_index(&visible_messages, app.sessions.message_idx).unwrap_or(0);
    let visible = visible_message_window(&visible_messages, selected_visible_idx, inner.height);
    let rows = visible.map(|(_, message)| {
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
        state.select(Some(selected_visible_idx.saturating_sub(
            message_window_start(visible_messages.len(), selected_visible_idx, inner.height),
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
    visible: &app::SessionRowsView<'a>,
) -> Option<&'a crate::session_manager::SessionMeta> {
    let selected = visible.get(app.sessions.selected_idx);
    let Some(key) = app.sessions.detail_key.as_deref() else {
        return selected;
    };
    if selected.is_some_and(|session| app::session_key_matches(session, key)) {
        return selected;
    }
    visible
        .iter()
        .find(|session| app::session_key_matches(session, key))
        .or(selected)
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

fn truncate_path_middle(path: &str, width: u16) -> String {
    let width = width as usize;
    if width == 0 {
        return String::new();
    }
    let path = bounded_trimmed_text_for_display(path);
    if UnicodeWidthStr::width(path.as_str()) <= width {
        return path;
    }
    if width == 1 {
        return "…".to_string();
    }

    let prefix_budget = (width - 1) / 3;
    let suffix_budget = width - 1 - prefix_budget;
    let mut prefix = String::new();
    let mut prefix_width = 0usize;
    for ch in path.chars() {
        let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if prefix_width.saturating_add(char_width) > prefix_budget {
            break;
        }
        prefix.push(ch);
        prefix_width = prefix_width.saturating_add(char_width);
    }

    let mut suffix = Vec::new();
    let mut suffix_width = 0usize;
    for ch in path.chars().rev() {
        let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if suffix_width.saturating_add(char_width) > suffix_budget {
            break;
        }
        suffix.push(ch);
        suffix_width = suffix_width.saturating_add(char_width);
    }
    suffix.reverse();
    format!("{prefix}…{}", suffix.into_iter().collect::<String>())
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
    const DISPLAY_LIMIT: usize = 120;

    let mut preview = String::with_capacity(128);
    let mut display_width = 0usize;
    let mut pending_space = false;
    let mut truncated = false;

    for ch in content.chars() {
        if ch.is_whitespace() {
            pending_space |= !preview.is_empty();
            continue;
        }

        let separator_width = usize::from(pending_space && !preview.is_empty());
        let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if display_width
            .saturating_add(separator_width)
            .saturating_add(char_width)
            > DISPLAY_LIMIT
        {
            truncated = true;
            break;
        }
        if separator_width != 0 {
            preview.push(' ');
            display_width += 1;
        }
        preview.push(ch);
        display_width = display_width.saturating_add(char_width);
        pending_space = false;
    }

    if truncated {
        while display_width.saturating_add(1) > DISPLAY_LIMIT {
            let Some(last) = preview.pop() else {
                break;
            };
            display_width =
                display_width.saturating_sub(UnicodeWidthChar::width(last).unwrap_or(0));
        }
        preview.push('…');
    }
    preview
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

fn selected_message_visible_index(
    messages: &app::SessionMessagesView<'_>,
    selected: usize,
) -> Option<usize> {
    messages.visible_index_of(selected)
}

fn visible_message_window<'a>(
    messages: &'a app::SessionMessagesView<'a>,
    selected: usize,
    height: u16,
) -> impl Iterator<Item = (usize, &'a crate::session_manager::SessionMessage)> + 'a {
    let visible_rows = height.saturating_sub(1).max(1) as usize;
    let start = message_window_start(messages.len(), selected, height);
    let end = (start + visible_rows).min(messages.len());
    (start..end).filter_map(|index| messages.get(index))
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
    fn project_paths_keep_their_basename_when_narrow() {
        let value = truncate_path_middle("/very/long/workspace/repository", 18);

        assert!(value.starts_with("/very"));
        assert!(value.ends_with("repository"));
        assert!(UnicodeWidthStr::width(value.as_str()) <= 18);
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
