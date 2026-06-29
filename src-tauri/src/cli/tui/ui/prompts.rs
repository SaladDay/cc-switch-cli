use super::*;

pub(super) fn render_prompts(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let query = app.filter.query_lower();
    let visible: Vec<_> = data
        .prompts
        .rows
        .iter()
        .filter(|row| match &query {
            None => true,
            Some(q) => {
                row.prompt.name.to_lowercase().contains(q) || row.id.to_lowercase().contains(q)
            }
        })
        .collect();

    let header = Row::new(vec![
        Cell::from(""),
        Cell::from(texts::tui_header_id()),
        Cell::from(texts::header_name()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let rows = visible.iter().map(|row| {
        Row::new(vec![
            Cell::from(if row.prompt.enabled {
                texts::tui_marker_active()
            } else {
                texts::tui_marker_inactive()
            }),
            Cell::from(row.id.clone()),
            Cell::from(row.prompt.name.clone()),
        ])
    });

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(format!(
            "{} · {}",
            texts::menu_manage_prompts(),
            app.app_type.as_str()
        ));
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
                ("a", texts::tui_key_add()),
                ("Enter", texts::tui_key_view()),
                ("e", texts::tui_key_edit()),
                ("d", texts::tui_key_delete()),
            ],
        );
    }

    render_summary_bar(frame, chunks[1], theme, prompts_summary(data));

    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Length(18),
            Constraint::Min(10),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.prompt_idx));
    frame.render_stateful_widget(table, inset_left(chunks[2], CONTENT_INSET_LEFT), &mut state);
}

fn prompts_summary(data: &UiData) -> String {
    let count = data.prompts.rows.len();
    let active = data
        .prompts
        .rows
        .iter()
        .find(|row| row.prompt.enabled)
        .map(|row| row.prompt.name.as_str())
        .unwrap_or_else(|| texts::tui_prompt_no_active_summary());

    texts::tui_prompts_summary(count, active)
}
