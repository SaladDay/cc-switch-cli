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
        ("PgUp/PgDn", texts::tui_key_page()),
        ("←→", texts::tui_key_pane()),
    ];
    keys.extend(crate::cli::tui::keymap::sessions::key_bar_items(app, data));
    // One shared, labelled indicator for the whole line. The scan reports
    // liveness only — no file counts — so no number escalates.
    let scanning = app.sessions.loading || app.sessions.deep_search_active.is_some();
    let status = if app.sessions.loading && !app.sessions.loaded_once {
        texts::tui_sessions_loading_summary().to_string()
    } else if app.sessions.deep_search_active.is_some() {
        match app.sessions.deep_search_query.as_deref() {
            Some(query) => texts::tui_sessions_searching(query),
            None => texts::tui_sessions_project_filtering().to_string(),
        }
    } else {
        // Manual refresh keeps the previous immutable page visible while its
        // replacement is built; the trailing spinner says "still working".
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
    let summary_bar_width = area.width.saturating_sub(8).max(1);
    let indicator_width = if scanning {
        inline_refresh_indicator_width(app.tick, theme, None)
    } else {
        0
    };
    let summary_width = summary_bar_width.saturating_sub(indicator_width).max(1);
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
    let summary_spans =
        summary_with_refresh_indicator(summary, scanning, app.tick, theme, None, summary_bar_width);
    let frame_body = render_page_frame_spans(
        frame,
        area,
        theme,
        app,
        texts::tui_sessions_title(),
        &keys,
        Some(summary_spans),
    );

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(frame_body);

    render_session_list(frame, app, &visible, body[0], theme);
    render_session_detail(frame, app, &visible, body[1], theme);
}

/// Fixed width of the relative-time column.
const TIME_COLUMN_WIDTH: u16 = 10;
/// Fixed width of the cost column: `$999.99` plus two columns of slack.
const COST_COLUMN_WIDTH: u16 = 9;

/// Column visibility for the session list at a given pane width.
///
/// Degradation order is Title > Time > Cost. Title never leaves because it is
/// the only cell that identifies a row. Cost drops first: the Overview pane
/// shows the estimate for the selected session anyway, while Time is
/// the only per-row ordering cue the list itself provides.
fn session_list_columns(width: u16) -> (bool, bool) {
    let show_cost = width >= 30;
    let show_time = width >= 19;
    (show_time, show_cost)
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
            PaginationFooterStatus::LoadingPage(target + 1)
        } else if app.sessions.remote.failed_page().is_some() {
            PaginationFooterStatus::LoadErrorMoveRetry
        } else if page + 1 == total_rows.div_ceil(crate::session_manager::paged_manifest::PAGE_SIZE)
            && selected == visible.len() - 1
        {
            PaginationFooterStatus::End
        } else if app.sessions.remote.next_page_is_pending() {
            PaginationFooterStatus::PreparingNext
        } else if app.sessions.remote.next_page_is_ready() {
            PaginationFooterStatus::NextReady
        } else {
            PaginationFooterStatus::Idle
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

    // The enlarged pane leaves room for all three columns on a standard
    // 80-column terminal. Below that the columns are dropped in priority order
    // (see `session_list_columns`): Title always stays, Time outranks Cost.
    let (show_time, show_cost) = session_list_columns(inner.width);
    let mut header_cells = vec![Cell::from(texts::tui_sessions_header_title())];
    if show_time {
        header_cells.push(Cell::from(
            Text::from(texts::tui_sessions_header_time()).alignment(Alignment::Right),
        ));
    }
    if show_cost {
        header_cells.push(Cell::from(
            Text::from(texts::tui_sessions_header_cost()).alignment(Alignment::Right),
        ));
    }
    let header =
        Row::new(header_cells).style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

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
            let mut cells = vec![Cell::from(title_line)];
            if show_time {
                cells.push(Cell::from(
                    Text::from(Line::styled(time, Style::default().fg(theme.comment)))
                        .alignment(Alignment::Right),
                ));
            }
            if show_cost {
                cells.push(Cell::from(
                    Text::from(Line::from(session_cost_spans(
                        session.usage.as_ref(),
                        theme,
                    )))
                    .alignment(Alignment::Right),
                ));
            }
            Row::new(cells)
        });

    let mut widths = vec![Constraint::Min(0)];
    if show_time {
        widths.push(Constraint::Length(TIME_COLUMN_WIDTH));
    }
    if show_cost {
        widths.push(Constraint::Length(COST_COLUMN_WIDTH));
    }
    let table = Table::new(rows, widths)
        .header(header)
        // Cost sits right against Time without this; two columns of air is what
        // separates the three fields once they no longer differ in color alone.
        .column_spacing(2)
        .block(Block::default().borders(Borders::NONE))
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    // The rows are pre-sliced to the window, so the highlight index is relative
    // to `start`.
    let mut state = TableState::default();
    if app.sessions.pagination.is_row_focused() {
        state.select(Some(selected - start));
    }
    frame.render_stateful_widget(
        table,
        inset_horizontal(inner, CONTENT_INSET_LEFT),
        &mut state,
    );
}

fn render_session_detail(
    frame: &mut Frame<'_>,
    app: &App,
    visible: &app::SessionRowsView<'_>,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let selected = selected_session(app, visible);
    // Six overview rows plus the block borders; Tokens and Cost each own a
    // labelled row now, so the pane needs one line more than the message list
    // (which absorbs the change through its `Min(0)` share).
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(0)])
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
    let usage = session.usage.as_ref();
    let token_spans = session_token_spans(usage, theme);
    let cost_spans = session_cost_spans(usage, theme);

    let lines = vec![
        overview_field_line(
            texts::tui_sessions_overview_time_label(),
            &time,
            inner.width,
            theme,
        ),
        overview_field_line(
            texts::tui_sessions_overview_workdir_label(),
            project,
            inner.width,
            theme,
        ),
        overview_field_line(
            texts::tui_sessions_overview_summary_label(),
            &title,
            inner.width,
            theme,
        ),
        overview_field_spans_line(
            texts::tui_sessions_overview_tokens_label(),
            token_spans,
            inner.width,
            theme,
        ),
        overview_field_spans_line(
            texts::tui_sessions_header_cost(),
            cost_spans,
            inner.width,
            theme,
        ),
        overview_field_line(
            texts::tui_sessions_resume_command(),
            resume_command,
            inner.width,
            theme,
        ),
    ];

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Sessions pin the cost to two decimals so every row shares one decimal
/// point; the Usage and Home surfaces keep their own magnitude-dependent
/// precision through [`super::usage::format_money`].
fn format_session_money(value: f64) -> String {
    format!("${value:.2}")
}

fn format_session_cost(usage: &crate::session_manager::SessionUsageSummary) -> Option<String> {
    usage
        .estimated_cost_usd
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(format_session_money)
}

fn format_session_tokens(usage: &crate::session_manager::SessionUsageSummary) -> String {
    super::usage::format_token_breakdown_compact(
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_read_tokens,
        usage.cache_creation_tokens,
    )
}

fn unavailable_span(theme: &super::theme::Theme) -> Span<'static> {
    Span::styled("-", Style::default().fg(theme.comment))
}

fn session_cost_spans(
    usage: Option<&crate::session_manager::SessionUsageSummary>,
    theme: &super::theme::Theme,
) -> Vec<Span<'static>> {
    let Some(cost) = usage.and_then(format_session_cost) else {
        return vec![unavailable_span(theme)];
    };
    vec![Span::raw(cost)]
}

fn session_token_spans(
    usage: Option<&crate::session_manager::SessionUsageSummary>,
    theme: &super::theme::Theme,
) -> Vec<Span<'static>> {
    let Some(usage) = usage else {
        return vec![unavailable_span(theme)];
    };
    vec![Span::raw(format_session_tokens(usage))]
}

fn overview_field_line(
    label: &'static str,
    value: &str,
    pane_width: u16,
    theme: &super::theme::Theme,
) -> Line<'static> {
    overview_field_spans_line(label, vec![Span::raw(value.to_string())], pane_width, theme)
}

/// One Overview row: a 12-column dim label plus a two-space gutter, then the
/// value clipped to whatever is left of `pane_width`.
///
/// The value budget is derived from the rendered label instead of a fixed
/// margin so a short label (Tokens, Cost) spends every remaining column while
/// an over-long one (Resume Command) still cannot push the line past the pane
/// and wrap a row out of the fixed-height block.
fn overview_field_spans_line(
    label: &'static str,
    value: Vec<Span<'static>>,
    pane_width: u16,
    theme: &super::theme::Theme,
) -> Line<'static> {
    let label = format!("{}  ", pad_to_display_width(label, 12));
    let value_width = pane_width.saturating_sub(UnicodeWidthStr::width(label.as_str()) as u16);
    let mut spans = Vec::with_capacity(value.len() + 1);
    spans.push(Span::styled(label, Style::default().fg(theme.dim)));
    spans.extend(truncate_spans_to_display_width(value, value_width));
    Line::from(spans)
}

/// Truncate a styled value to `width` display columns, spending the budget span
/// by span so independently styled fragments share one width budget.
fn truncate_spans_to_display_width(spans: Vec<Span<'static>>, width: u16) -> Vec<Span<'static>> {
    let mut remaining = width;
    let mut out = Vec::with_capacity(spans.len());
    for span in spans {
        if remaining == 0 {
            break;
        }
        let span_width = UnicodeWidthStr::width(span.content.as_ref());
        if span_width <= usize::from(remaining) {
            remaining -= span_width as u16;
            out.push(span);
            continue;
        }
        let style = span.style;
        out.push(Span::styled(
            truncate_to_display_width(span.content.as_ref(), remaining),
            style,
        ));
        break;
    }
    out
}

fn render_session_messages(
    frame: &mut Frame<'_>,
    app: &App,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(session_pane_border_style(app, SessionsPane::Detail, theme))
        .title(format!(" {} ", texts::tui_sessions_messages_title()));
    let total_messages = app.sessions.logical_total_messages();
    if app.sessions.messages_loaded && total_messages > 0 {
        let page = app.sessions.message_remote.current_page();
        let page_start =
            page.saturating_mul(crate::session_manager::transcript::TRANSCRIPT_PAGE_SIZE);
        let page_end = page_start
            .saturating_add(app.sessions.messages.len())
            .min(total_messages);
        let selected = app.sessions.selected_message_absolute();
        let status = if let Some(target) = app.sessions.message_remote.pending_cross_page() {
            PaginationFooterStatus::LoadingPage(target + 1)
        } else if app.sessions.message_remote.failed_page().is_some() {
            PaginationFooterStatus::LoadErrorMoveRetry
        } else if selected + 1 == total_messages {
            PaginationFooterStatus::End
        } else {
            PaginationFooterStatus::Idle
        };
        block = with_pagination_footer(
            block,
            area.width,
            PaginationFooter {
                page: page + 1,
                start: page_start + 1,
                end: page_end,
                total: total_messages,
                status,
            },
            theme,
            app.tick,
        );
    }
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
            source_mtime_ns: None,
            last_active_at: None,
            source_path: None,
            usage: None,
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

    #[test]
    fn session_cost_and_tokens_render_estimates_without_markers() {
        use crate::session_manager::SessionUsageSummary;

        let usage = SessionUsageSummary {
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 30,
            cache_creation_tokens: 40,
            estimated_cost_usd: Some(1.25),
        };
        assert_eq!(format_session_cost(&usage), Some("$1.25".to_string()));
        assert!(format_session_tokens(&usage).starts_with("In: 10"));
    }

    #[test]
    fn session_cost_keeps_two_decimals_across_magnitudes() {
        assert_eq!(format_session_money(0.4), "$0.40");
        assert_eq!(format_session_money(1.25), "$1.25");
        assert_eq!(format_session_money(12.5), "$12.50");
        assert_eq!(format_session_money(125.0), "$125.00");
        // The widest realistic amount still fits the fixed Cost column.
        assert!(
            UnicodeWidthStr::width(format_session_money(999.99).as_str())
                <= usize::from(COST_COLUMN_WIDTH)
        );
    }

    #[test]
    fn invalid_cost_estimates_fail_closed_at_the_rendering_boundary() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.01] {
            let usage = crate::session_manager::SessionUsageSummary {
                estimated_cost_usd: Some(value),
                ..crate::session_manager::SessionUsageSummary::default()
            };
            assert_eq!(format_session_cost(&usage), None);
        }
    }

    #[test]
    fn unavailable_cost_does_not_hide_available_tokens() {
        let usage = crate::session_manager::SessionUsageSummary {
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            estimated_cost_usd: None,
        };

        assert_eq!(format_session_cost(&usage), None);
        let tokens = format_session_tokens(&usage);
        assert!(tokens.contains("In: 10"), "{tokens}");
    }

    #[test]
    fn narrow_session_list_drops_cost_before_time_and_never_the_title() {
        // Below the Time threshold only the flexible Title column survives.
        assert_eq!(session_list_columns(0), (false, false));
        assert_eq!(session_list_columns(18), (false, false));
        // Time comes back first...
        assert_eq!(session_list_columns(19), (true, false));
        assert_eq!(session_list_columns(29), (true, false));
        // ...and Cost only once the pane can hold all three.
        assert_eq!(session_list_columns(30), (true, true));
        assert_eq!(session_list_columns(200), (true, true));
    }

    fn test_theme() -> crate::cli::tui::theme::Theme {
        crate::cli::tui::theme::theme_for(&crate::app_config::AppType::Claude)
    }

    fn span_texts(spans: &[Span<'static>]) -> Vec<String> {
        spans.iter().map(|span| span.content.to_string()).collect()
    }

    #[test]
    fn overview_usage_rows_render_values_without_markers() {
        use crate::session_manager::SessionUsageSummary;

        let theme = test_theme();
        let usage = SessionUsageSummary {
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 30,
            cache_creation_tokens: 40,
            estimated_cost_usd: Some(1.25),
        };

        let tokens = session_token_spans(Some(&usage), &theme);
        assert_eq!(tokens.len(), 1);
        assert!(tokens[0].content.starts_with("In: 10"), "{:?}", tokens[0]);
        let cost = session_cost_spans(Some(&usage), &theme);
        assert_eq!(span_texts(&cost), vec!["$1.25".to_string()]);
        assert_eq!(
            cost[0].style.fg, None,
            "available Cost values should use the default white foreground"
        );

        // No usage at all renders a muted placeholder on both rows.
        let tokens = session_token_spans(None, &theme);
        assert_eq!(span_texts(&tokens), vec!["-".to_string()]);
        assert_eq!(tokens[0].style.fg, Some(theme.comment));
        let cost = session_cost_spans(None, &theme);
        assert_eq!(span_texts(&cost), vec!["-".to_string()]);
        assert_eq!(cost[0].style.fg, Some(theme.comment));
    }

    #[test]
    fn overview_rows_share_the_label_column_and_clip_the_value_only() {
        let theme = test_theme();
        // 14 columns of label plus an 8-column value budget.
        let line = overview_field_spans_line(
            "Tokens",
            vec![Span::raw("In: 1.0k - Out: 2.0k".to_string())],
            22,
            &theme,
        );

        // 12-column label plus the two-space gutter, exactly like every other
        // Overview row.
        assert!(line.spans[0].content.starts_with("Tokens"));
        assert_eq!(UnicodeWidthStr::width(line.spans[0].content.as_ref()), 14);
        let value: String = line.spans[1..]
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(UnicodeWidthStr::width(value.as_str()), 8);
        assert!(value.starts_with("In:"), "{value}");
        assert!(value.ends_with('…'), "{value}");
    }
}
