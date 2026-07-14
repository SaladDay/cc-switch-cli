use super::*;

pub(super) fn render_skills_discover(
    frame: &mut Frame<'_>,
    app: &App,
    _data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let title = format!(
        "{} › {} — {}",
        texts::menu_manage_skills(),
        texts::tui_skills_discover_title(),
        if app.skills_discover_query.trim().is_empty() {
            texts::tui_skills_discover_query_empty()
        } else {
            app.skills_discover_query.as_str()
        }
    );

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(format!(" {} ", icons::strip_icon(&title)));
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

    render_skills_discover_source_tabs(frame, app, chunks[0], theme);

    render_page_key_bar(
        frame,
        chunks[1],
        theme,
        &[
            ("Enter", texts::tui_key_install()),
            ("f", texts::tui_key_search()),
            ("r", texts::tui_key_refresh()),
            ("e", texts::tui_key_repo_manager()),
        ],
        app.focus == Focus::Content,
    );

    let query = app.filter.query_lower();
    let visible = app
        .skills_discover_results
        .iter()
        .filter(|skill| match &query {
            None => true,
            Some(q) => {
                skill.name.to_lowercase().contains(q)
                    || skill.directory.to_lowercase().contains(q)
                    || skill.key.to_lowercase().contains(q)
                    || skill.description.to_lowercase().contains(q)
            }
        })
        .collect::<Vec<_>>();

    if app.skills_discover_loading {
        render_skills_discover_loading(frame, chunks[2], theme);
        return;
    }

    if visible.is_empty() {
        let empty_text = if matches!(
            app.skills_discover_source,
            app::SkillsDiscoverSource::Marketplace
        ) && app.skills_discover_query.trim().chars().count() < 2
        {
            texts::tui_skills_skillssh_search_prompt()
        } else {
            texts::tui_skills_discover_empty()
        };
        frame.render_widget(
            Paragraph::new(empty_text)
                .style(Style::default().fg(theme.dim))
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: false }),
            inset_left(chunks[2], CONTENT_INSET_LEFT),
        );
        return;
    }

    let header = Row::new(vec![
        Cell::from(""),
        Cell::from(texts::header_name()),
        Cell::from(texts::tui_header_repo()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let rows = visible.iter().map(|skill| {
        let repo = match (&skill.repo_owner, &skill.repo_name) {
            (Some(owner), Some(name)) => format!("{owner}/{name}"),
            _ => "-".to_string(),
        };
        Row::new(vec![
            Cell::from(if skill.installed {
                texts::tui_marker_active()
            } else {
                texts::tui_marker_inactive()
            }),
            Cell::from(skill_display_name(&skill.name, &skill.directory).to_string()),
            Cell::from(repo),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Percentage(70),
            Constraint::Percentage(30),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.skills_discover_idx));
    frame.render_stateful_widget(table, inset_left(chunks[2], CONTENT_INSET_LEFT), &mut state);
}

fn render_skills_discover_loading(frame: &mut Frame<'_>, area: Rect, theme: &super::theme::Theme) {
    let line = Line::styled(texts::tui_loading(), Style::default().fg(theme.comment));
    let y = area.y + area.height.saturating_sub(1) / 2;
    let centered = Rect::new(area.x, y, area.width, 1.min(area.height));
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), centered);
}

fn render_skills_discover_source_tabs(
    frame: &mut Frame<'_>,
    app: &App,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let tabs = [
        (
            app::SkillsDiscoverSource::Repos,
            texts::tui_skills_source_repos(),
        ),
        (
            app::SkillsDiscoverSource::Marketplace,
            texts::tui_skills_source_marketplace(),
        ),
    ];
    let mut spans = Vec::new();
    for (source, label) in tabs {
        let style = if app.skills_discover_source == source {
            active_chip_style(theme)
        } else {
            inactive_chip_style(theme)
        };
        spans.push(Span::styled(format!(" {label} "), style));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(
        texts::tui_skills_source_switch_hint(),
        Style::default().fg(theme.dim),
    ));

    frame.render_widget(
        Paragraph::new(Line::from(spans)).wrap(Wrap { trim: false }),
        inset_left(area, CONTENT_INSET_LEFT),
    );
}
