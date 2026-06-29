use super::*;

pub(super) fn mcp_rows_filtered<'a>(app: &App, data: &'a UiData) -> Vec<&'a McpRow> {
    let query = app.filter.query_lower();
    data.mcp
        .rows
        .iter()
        .filter(|row| match &query {
            None => true,
            Some(q) => {
                row.server.name.to_lowercase().contains(q) || row.id.to_lowercase().contains(q)
            }
        })
        .collect()
}

pub(super) fn render_mcp(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let visible = mcp_rows_filtered(app, data);

    let header = Row::new(vec![
        Cell::from(texts::header_name()),
        Cell::from(crate::app_config::AppType::Claude.as_str()),
        Cell::from(crate::app_config::AppType::Codex.as_str()),
        Cell::from(crate::app_config::AppType::Gemini.as_str()),
        Cell::from(crate::app_config::AppType::OpenCode.as_str()),
        Cell::from(crate::app_config::AppType::Hermes.as_str()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let rows = visible.iter().map(|row| {
        Row::new(vec![
            Cell::from(row.server.name.clone()),
            Cell::from(if row.server.apps.claude {
                texts::tui_marker_active()
            } else {
                texts::tui_marker_inactive()
            }),
            Cell::from(if row.server.apps.codex {
                texts::tui_marker_active()
            } else {
                texts::tui_marker_inactive()
            }),
            Cell::from(if row.server.apps.gemini {
                texts::tui_marker_active()
            } else {
                texts::tui_marker_inactive()
            }),
            Cell::from(if row.server.apps.opencode {
                texts::tui_marker_active()
            } else {
                texts::tui_marker_inactive()
            }),
            Cell::from(if row.server.apps.hermes {
                texts::tui_marker_active()
            } else {
                texts::tui_marker_inactive()
            }),
        ])
    });

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(texts::menu_manage_mcp());
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
                ("Space", texts::tui_key_toggle()),
                ("m", texts::tui_key_apps()),
                ("a", texts::tui_key_add()),
                ("e", texts::tui_key_edit()),
                ("i", texts::tui_mcp_action_import_existing()),
                ("d", texts::tui_key_delete()),
            ],
        );
    }

    let summary = texts::tui_mcp_server_counts(
        data.mcp
            .rows
            .iter()
            .filter(|row| row.server.apps.claude)
            .count(),
        data.mcp
            .rows
            .iter()
            .filter(|row| row.server.apps.codex)
            .count(),
        data.mcp
            .rows
            .iter()
            .filter(|row| row.server.apps.gemini)
            .count(),
        data.mcp
            .rows
            .iter()
            .filter(|row| row.server.apps.opencode)
            .count(),
        data.mcp
            .rows
            .iter()
            .filter(|row| row.server.apps.hermes)
            .count(),
    );
    render_summary_bar(frame, chunks[1], theme, summary);

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(50),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.mcp_idx));

    frame.render_stateful_widget(table, inset_left(chunks[2], CONTENT_INSET_LEFT), &mut state);
}
