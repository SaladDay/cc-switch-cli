use super::*;

pub(super) fn render_skills_installed(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let keys = crate::cli::tui::keymap::skills_installed::key_bar_items(app, data);
    let body = render_page_frame(
        frame,
        area,
        theme,
        app,
        texts::menu_manage_skills(),
        &keys,
        Some(installed_summary(app, data)),
    );

    let visible = skills_installed_filtered(app, data);

    let header = Row::new(vec![
        Cell::from(texts::header_name()),
        Cell::from(crate::app_config::AppType::Claude.as_str()),
        Cell::from(crate::app_config::AppType::Codex.as_str()),
        Cell::from(crate::app_config::AppType::Gemini.as_str()),
        Cell::from(crate::app_config::AppType::OpenCode.as_str()),
        Cell::from(crate::app_config::AppType::Hermes.as_str()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let rows = visible.iter().map(|skill| {
        let display_name = skill_display_name(&skill.name, &skill.directory);
        let display_name = if app.skill_updates.contains_key(&skill.id) {
            format!("{display_name} {}", texts::tui_skills_update_marker())
        } else {
            display_name.to_string()
        };
        Row::new(vec![
            Cell::from(display_name),
            Cell::from(skill_marker(skill.apps.claude)),
            Cell::from(skill_marker(skill.apps.codex)),
            Cell::from(skill_marker(skill.apps.gemini)),
            Cell::from(skill_marker(skill.apps.opencode)),
            Cell::from(skill_marker(skill.apps.hermes)),
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
            Constraint::Length(8),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));

    if data.skills.installed.is_empty() {
        render_empty_state(
            frame,
            body,
            theme,
            texts::tui_skills_empty_title(),
            texts::tui_skills_empty_subtitle(),
        );
        return;
    }

    let mut state = TableState::default();
    state.select(Some(app.skills_idx));
    frame.render_stateful_widget(table, inset_left(body, CONTENT_INSET_LEFT), &mut state);
}

fn installed_summary(app: &App, data: &UiData) -> String {
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
    let enabled_hermes = data
        .skills
        .installed
        .iter()
        .filter(|s| s.apps.hermes)
        .count();

    let counts = texts::tui_skills_installed_counts(
        enabled_claude,
        enabled_codex,
        enabled_gemini,
        enabled_opencode,
        enabled_hermes,
    );
    if app.skill_updates.is_empty() {
        counts
    } else {
        format!(
            "{counts} · {}",
            texts::tui_skills_updates_available(app.skill_updates.len())
        )
    }
}

fn skill_marker(enabled: bool) -> &'static str {
    if enabled {
        texts::tui_marker_active()
    } else {
        texts::tui_marker_inactive()
    }
}
