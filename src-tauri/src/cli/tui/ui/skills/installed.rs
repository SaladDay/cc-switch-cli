use super::*;

pub(super) fn render_skills_installed(
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
        .title(texts::menu_manage_skills());
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
                ("Enter", texts::tui_key_details()),
                ("Space", texts::tui_key_toggle()),
                ("m", texts::tui_key_apps()),
                ("f", texts::tui_key_discover()),
                ("i", texts::tui_skills_action_import_existing()),
                ("d", texts::tui_key_uninstall()),
            ],
        );
    }

    render_summary_bar(frame, chunks[1], theme, installed_summary(data));

    let visible = skills_installed_filtered(app, data);

    let header = Row::new(vec![
        Cell::from(texts::header_name()),
        Cell::from(crate::app_config::AppType::Claude.as_str()),
        Cell::from(crate::app_config::AppType::Codex.as_str()),
        Cell::from(crate::app_config::AppType::Gemini.as_str()),
        Cell::from(crate::app_config::AppType::OpenCode.as_str()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let rows = visible.iter().map(|skill| {
        Row::new(vec![
            Cell::from(skill_display_name(&skill.name, &skill.directory).to_string()),
            Cell::from(skill_marker(skill.apps.claude)),
            Cell::from(skill_marker(skill.apps.codex)),
            Cell::from(skill_marker(skill.apps.gemini)),
            Cell::from(skill_marker(skill.apps.opencode)),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(50),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.skills_idx));
    frame.render_stateful_widget(table, inset_left(chunks[2], CONTENT_INSET_LEFT), &mut state);
}

fn installed_summary(data: &UiData) -> String {
    let enabled_claude = data
        .skills
        .installed
        .iter()
        .filter(|s| s.apps.claude)
        .count();
    let enabled_codex = data
        .skills
        .installed
        .iter()
        .filter(|s| s.apps.codex)
        .count();
    let enabled_gemini = data
        .skills
        .installed
        .iter()
        .filter(|s| s.apps.gemini)
        .count();
    let enabled_opencode = data
        .skills
        .installed
        .iter()
        .filter(|s| s.apps.opencode)
        .count();

    texts::tui_skills_installed_counts(
        enabled_claude,
        enabled_codex,
        enabled_gemini,
        enabled_opencode,
    )
}

fn skill_marker(enabled: bool) -> &'static str {
    if enabled {
        texts::tui_marker_active()
    } else {
        texts::tui_marker_inactive()
    }
}
