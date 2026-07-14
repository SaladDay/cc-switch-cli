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

    let keys = crate::cli::tui::keymap::prompts::key_bar_items(app, data);
    let body = render_page_frame(
        frame,
        area,
        theme,
        app,
        &format!(
            "{} · {}",
            texts::menu_manage_prompts(),
            app.app_type.as_str()
        ),
        &keys,
        Some(prompts_summary(data)),
    );

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

    if data.prompts.rows.is_empty() {
        render_empty_state(
            frame,
            body,
            theme,
            texts::tui_prompts_empty_title(),
            texts::tui_prompts_empty_subtitle(),
            &[("a", texts::tui_key_add())],
        );
        return;
    }

    let mut state = TableState::default();
    state.select(Some(app.prompt_idx));
    frame.render_stateful_widget(table, inset_left(body, CONTENT_INSET_LEFT), &mut state);
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
