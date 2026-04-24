use super::*;

pub(super) fn render_skills_repos(
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
        .title(texts::tui_skills_repos_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    if app.focus == Focus::Content {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[
                ("a", texts::tui_key_add()),
                ("d", texts::tui_key_delete()),
                ("x", texts::tui_key_toggle()),
            ],
        );
    }

    frame.render_widget(
        Paragraph::new(texts::tui_skills_repos_hint())
            .style(Style::default().fg(theme.dim))
            .wrap(Wrap { trim: false }),
        inset_left(chunks[1], CONTENT_INSET_LEFT),
    );

    let query = app.filter.query_lower();
    let visible = data
        .skills
        .repos
        .iter()
        .filter(|repo| match &query {
            None => true,
            Some(q) => {
                repo.owner.to_lowercase().contains(q)
                    || repo.name.to_lowercase().contains(q)
                    || repo.branch.to_lowercase().contains(q)
                    || repo
                        .token_env
                        .as_deref()
                        .is_some_and(|token_env| token_env.to_lowercase().contains(q))
            }
        })
        .collect::<Vec<_>>();

    if visible.is_empty() {
        frame.render_widget(
            Paragraph::new(texts::tui_skills_repos_empty())
                .style(Style::default().fg(theme.dim))
                .wrap(Wrap { trim: false }),
            inset_left(chunks[2], CONTENT_INSET_LEFT),
        );
        return;
    }

    let header = Row::new(vec![
        Cell::from(""),
        Cell::from(texts::tui_header_repo()),
        Cell::from(texts::tui_header_branch()),
        Cell::from(texts::tui_header_auth()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let rows = visible.iter().map(|repo| {
        let repo_name = format!("{}/{}", repo.owner, repo.name);
        let auth = repo
            .token_env
            .as_deref()
            .map(|token_env| format!("env:{token_env}"))
            .unwrap_or_default();
        Row::new(vec![
            Cell::from(if repo.enabled {
                texts::tui_marker_active()
            } else {
                texts::tui_marker_inactive()
            }),
            Cell::from(repo_name),
            Cell::from(repo.branch.clone()),
            Cell::from(auth),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Percentage(55),
            Constraint::Percentage(15),
            Constraint::Percentage(30),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.skills_repo_idx));
    frame.render_stateful_widget(table, inset_left(chunks[2], CONTENT_INSET_LEFT), &mut state);
}
