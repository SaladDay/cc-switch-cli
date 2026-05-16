use super::*;

pub(super) fn render_skill_detail(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
    directory: &str,
) {
    let Some(skill) = data
        .skills
        .installed
        .iter()
        .find(|s| s.directory.eq_ignore_ascii_case(directory))
    else {
        frame.render_widget(
            Paragraph::new(texts::tui_skill_not_found())
                .style(Style::default().fg(theme.dim))
                .wrap(Wrap { trim: false })
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Plain)
                        .border_style(pane_border_style(app, Focus::Content, theme))
                        .title(texts::tui_skills_detail_title()),
                ),
            area,
        );
        return;
    };

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(texts::tui_skills_detail_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    if app.focus == Focus::Content {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[
                ("Space", texts::tui_key_toggle()),
                ("m", texts::tui_key_apps()),
                ("d", texts::tui_key_uninstall()),
                ("s", texts::tui_key_sync()),
            ],
        );
    }

    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                texts::tui_label_directory(),
                Style::default().fg(theme.accent),
            ),
            Span::raw(": "),
            Span::raw(skill.directory.clone()),
        ]),
        Line::from(vec![
            Span::styled(texts::header_name(), Style::default().fg(theme.accent)),
            Span::raw(": "),
            Span::raw(skill.name.clone()),
        ]),
    ];

    if let Some(desc) = skill
        .description
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled(
                texts::header_description(),
                Style::default().fg(theme.accent),
            ),
            Span::raw(": "),
        ]));
        for line in desc.lines() {
            lines.push(Line::raw(line.to_string()));
        }
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled(
            texts::tui_label_enabled_for(),
            Style::default().fg(theme.accent),
        ),
        Span::raw(": "),
        Span::raw(enabled_skill_apps_text(&skill.apps)),
    ]));

    if let (Some(owner), Some(name)) = (&skill.repo_owner, &skill.repo_name) {
        lines.push(Line::from(vec![
            Span::styled(texts::tui_label_repo(), Style::default().fg(theme.accent)),
            Span::raw(": "),
            Span::raw(format!("{owner}/{name}")),
        ]));
    }
    if let Some(url) = skill.readme_url.as_deref().filter(|s| !s.trim().is_empty()) {
        lines.push(Line::from(vec![
            Span::styled(texts::tui_label_readme(), Style::default().fg(theme.accent)),
            Span::raw(": "),
            Span::raw(url.to_string()),
        ]));
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        inset_left(chunks[1], CONTENT_INSET_LEFT),
    );
}
