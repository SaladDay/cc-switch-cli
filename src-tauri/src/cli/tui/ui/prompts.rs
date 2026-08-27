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

pub(super) fn render_pi_system_prompts(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let body = render_page_frame(
        frame,
        area,
        theme,
        app,
        texts::menu_pi_system_prompts(),
        &[
            ("Enter", texts::tui_key_view()),
            ("e", texts::tui_key_edit()),
            ("d", texts::tui_key_delete()),
        ],
        Some(crate::t!("2 native prompt files", "2 个原生提示词文件").to_string()),
    );
    let rows = data.pi_prompts.system_files.iter().map(|(kind, snapshot)| {
        let filename = match kind {
            crate::services::pi_prompt_files::PiPromptFileKind::SystemAppend => "APPEND_SYSTEM.md",
            crate::services::pi_prompt_files::PiPromptFileKind::SystemOverride => "SYSTEM.md",
        };
        let mode = match kind {
            crate::services::pi_prompt_files::PiPromptFileKind::SystemAppend => {
                if crate::cli::i18n::is_chinese() {
                    "追加"
                } else {
                    "Append"
                }
            }
            crate::services::pi_prompt_files::PiPromptFileKind::SystemOverride => {
                if crate::cli::i18n::is_chinese() {
                    "覆盖"
                } else {
                    "Override"
                }
            }
        };
        Row::new(vec![
            Cell::from(if snapshot.exists {
                texts::tui_marker_active()
            } else {
                texts::tui_marker_inactive()
            }),
            Cell::from(filename),
            Cell::from(mode),
            Cell::from(snapshot.content.chars().count().to_string()),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Length(24),
            Constraint::Length(12),
            Constraint::Min(8),
        ],
    )
    .header(
        Row::new(vec![
            "",
            crate::t!("File", "文件"),
            crate::t!("Mode", "模式"),
            crate::t!("Characters", "字符数"),
        ])
        .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));
    let mut state = TableState::default();
    state.select(Some(
        app.pi_system_prompt_idx
            .min(data.pi_prompts.system_files.len().saturating_sub(1)),
    ));
    frame.render_stateful_widget(table, inset_left(body, CONTENT_INSET_LEFT), &mut state);
}

pub(super) fn render_pi_prompt_templates(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let body = render_page_frame(
        frame,
        area,
        theme,
        app,
        texts::menu_pi_prompt_templates(),
        &[
            ("a", texts::tui_key_add()),
            ("Enter", texts::tui_key_view()),
            ("e", texts::tui_key_edit()),
            ("r", texts::tui_key_rename()),
            ("d", texts::tui_key_delete()),
        ],
        Some(crate::t!(
            format!("{} templates", data.pi_prompts.templates.len()),
            format!("{} 个模板", data.pi_prompts.templates.len())
        )),
    );
    if data.pi_prompts.templates.is_empty() {
        render_empty_state(
            frame,
            body,
            theme,
            crate::t!("No Pi prompt templates", "暂无 Pi 提示词模板"),
            crate::t!("Press a to create one", "按 a 新建模板"),
        );
        return;
    }
    let rows = data.pi_prompts.templates.iter().map(|template| {
        let preview = template
            .content
            .split_whitespace()
            .take(12)
            .collect::<Vec<_>>()
            .join(" ");
        Row::new(vec![
            Cell::from(format!("/{}", template.slug)),
            Cell::from(template.content.chars().count().to_string()),
            Cell::from(preview),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(24),
            Constraint::Length(12),
            Constraint::Min(10),
        ],
    )
    .header(
        Row::new(vec![
            crate::t!("Template", "模板"),
            crate::t!("Characters", "字符数"),
            crate::t!("Preview", "预览"),
        ])
        .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));
    let mut state = TableState::default();
    state.select(Some(
        app.pi_prompt_template_idx
            .min(data.pi_prompts.templates.len().saturating_sub(1)),
    ));
    frame.render_stateful_widget(table, inset_left(body, CONTENT_INSET_LEFT), &mut state);
}
