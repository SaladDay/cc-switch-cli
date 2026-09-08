use super::*;

pub(super) fn render_omp_models(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let summary = format!(
        "{} · {}",
        texts::tui_omp_models_summary(data.omp.models.len()),
        data.omp.models_path.display()
    );
    let body = render_page_frame(
        frame,
        area,
        theme,
        app,
        texts::menu_omp_models(),
        &crate::cli::tui::keymap::omp_models::key_bar_items(app, data),
        Some(summary),
    );
    if let Some(error) = &data.omp.models_error {
        render_empty_state(
            frame,
            body,
            theme,
            texts::tui_omp_config_error_title(),
            error,
        );
        return;
    }
    if data.omp.models.is_empty() {
        render_empty_state(
            frame,
            body,
            theme,
            texts::tui_omp_models_empty_title(),
            texts::tui_omp_models_empty_subtitle(),
        );
        return;
    }
    let table_area = inset_left(body, CONTENT_INSET_LEFT);
    let columns = omp_model_columns(table_area.width);
    let header = Row::new(
        columns
            .iter()
            .map(|column| Cell::from(omp_model_column_header(*column)))
            .collect::<Vec<_>>(),
    )
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));
    let rows = data.omp.models.iter().map(|model| {
        Row::new(
            columns
                .iter()
                .map(|column| Cell::from(omp_model_column_value(*column, model, data)))
                .collect::<Vec<_>>(),
        )
    });
    let table = Table::new(rows, omp_model_column_constraints(&columns))
        .header(header)
        .block(Block::default().borders(Borders::NONE))
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));
    let mut state = TableState::default();
    state.select(Some(
        app.omp_model_idx
            .min(data.omp.models.len().saturating_sub(1)),
    ));
    frame.render_stateful_widget(table, table_area, &mut state);
}

#[derive(Clone, Copy)]
enum OmpModelColumn {
    Provider,
    Model,
    Name,
    Api,
    Reasoning,
    Context,
    MaxTokens,
}

fn omp_model_columns(width: u16) -> Vec<OmpModelColumn> {
    // Keep the full metadata view on wide terminals, but prefer the identity
    // and transport columns on smaller screens. The complete model object is
    // still available through Enter, so no information is lost when columns
    // are hidden for readability.
    if width >= 118 {
        vec![
            OmpModelColumn::Provider,
            OmpModelColumn::Model,
            OmpModelColumn::Name,
            OmpModelColumn::Api,
            OmpModelColumn::Reasoning,
            OmpModelColumn::Context,
            OmpModelColumn::MaxTokens,
        ]
    } else if width >= 70 {
        vec![
            OmpModelColumn::Provider,
            OmpModelColumn::Model,
            OmpModelColumn::Api,
            OmpModelColumn::Reasoning,
            OmpModelColumn::Context,
        ]
    } else {
        vec![
            OmpModelColumn::Provider,
            OmpModelColumn::Model,
            OmpModelColumn::Api,
        ]
    }
}

fn omp_model_column_header(column: OmpModelColumn) -> &'static str {
    match column {
        OmpModelColumn::Provider => texts::tui_omp_models_provider_header(),
        OmpModelColumn::Model => texts::tui_omp_models_model_header(),
        OmpModelColumn::Name => texts::tui_omp_models_name_header(),
        OmpModelColumn::Api => texts::tui_omp_models_api_header(),
        OmpModelColumn::Reasoning => texts::tui_omp_models_reasoning_header(),
        OmpModelColumn::Context => texts::tui_omp_models_context_header(),
        OmpModelColumn::MaxTokens => texts::tui_omp_models_max_tokens_header(),
    }
}

fn omp_model_column_value(
    column: OmpModelColumn,
    model: &crate::omp_config::OMPNativeModel,
    data: &UiData,
) -> String {
    let obj = model.config.as_object();
    let text = |key: &str| {
        obj.and_then(|o| o.get(key))
            .map(|value| match value {
                Value::String(s) => s.clone(),
                _ => value.to_string(),
            })
            .unwrap_or_else(|| texts::tui_na().to_string())
    };
    match column {
        OmpModelColumn::Provider => model.provider_id.clone(),
        OmpModelColumn::Model => model.model_id.clone(),
        OmpModelColumn::Name => text("name"),
        OmpModelColumn::Api => obj
            .and_then(|o| o.get("api"))
            .or_else(|| {
                data.omp
                    .providers
                    .get(&model.provider_id)
                    .and_then(|p| p.get("api"))
            })
            .map(|value| match value {
                Value::String(s) => s.clone(),
                _ => value.to_string(),
            })
            .unwrap_or_else(|| texts::tui_na().to_string()),
        OmpModelColumn::Reasoning => text("reasoning"),
        OmpModelColumn::Context => text("contextWindow"),
        OmpModelColumn::MaxTokens => text("maxTokens"),
    }
}

fn omp_model_column_constraints(columns: &[OmpModelColumn]) -> Vec<Constraint> {
    match columns {
        [OmpModelColumn::Provider, OmpModelColumn::Model, OmpModelColumn::Name, OmpModelColumn::Api, OmpModelColumn::Reasoning, OmpModelColumn::Context, OmpModelColumn::MaxTokens] =>
        {
            vec![
                Constraint::Length(16),
                Constraint::Length(24),
                Constraint::Length(20),
                Constraint::Length(24),
                Constraint::Length(10),
                Constraint::Length(12),
                Constraint::Min(12),
            ]
        }
        [OmpModelColumn::Provider, OmpModelColumn::Model, OmpModelColumn::Api, OmpModelColumn::Reasoning, OmpModelColumn::Context] =>
        {
            vec![
                Constraint::Length(14),
                Constraint::Min(18),
                Constraint::Length(18),
                Constraint::Length(10),
                Constraint::Length(10),
            ]
        }
        _ => vec![
            Constraint::Length(12),
            Constraint::Min(12),
            Constraint::Min(8),
        ],
    }
}

#[cfg(test)]
pub(super) fn omp_model_column_count(width: u16) -> usize {
    omp_model_columns(width).len()
}

pub(super) fn render_omp_roles(
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
        texts::menu_omp_roles(),
        &crate::cli::tui::keymap::omp_roles::key_bar_items(app, data),
        Some(format!(
            "{} · {}",
            texts::tui_omp_roles_summary(data.omp.model_roles.len()),
            data.omp.roles_path.display()
        )),
    );
    if let Some(error) = &data.omp.config_error {
        render_empty_state(
            frame,
            body,
            theme,
            texts::tui_omp_config_error_title(),
            error,
        );
        return;
    }
    if data.omp.model_roles.is_empty() {
        render_empty_state(
            frame,
            body,
            theme,
            texts::tui_omp_roles_empty_title(),
            texts::tui_omp_roles_empty_subtitle(),
        );
        return;
    }
    let header = Row::new(vec![
        texts::tui_omp_roles_role_header(),
        texts::tui_omp_roles_selector_header(),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));
    let rows = data.omp.model_roles.iter().map(|(role, selector)| {
        Row::new(vec![Cell::from(role.clone()), Cell::from(selector.clone())])
    });
    let table = Table::new(rows, [Constraint::Length(18), Constraint::Min(24)])
        .header(header)
        .block(Block::default().borders(Borders::NONE))
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));
    let mut state = TableState::default();
    state.select(Some(
        app.omp_role_idx
            .min(data.omp.model_roles.len().saturating_sub(1)),
    ));
    frame.render_stateful_widget(table, inset_left(body, CONTENT_INSET_LEFT), &mut state);
}

pub(super) fn render_omp_system_prompts(
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
        texts::menu_omp_system_prompts(),
        &crate::cli::tui::keymap::omp_system_prompts::key_bar_items(app, data),
        Some(texts::tui_omp_system_prompts_summary(
            data.pi_prompts.system_files.len(),
        )),
    );
    if let Some(error) = &data.pi_prompts.read_error {
        render_empty_state(
            frame,
            body,
            theme,
            texts::tui_omp_prompt_error_title(),
            error,
        );
        return;
    }
    let rows = data.pi_prompts.system_files.iter().map(|(kind, snapshot)| {
        let filename = match kind {
            crate::services::pi_prompt_files::PiPromptFileKind::SystemAppend => "APPEND_SYSTEM.md",
            crate::services::pi_prompt_files::PiPromptFileKind::SystemOverride => "SYSTEM.md",
            crate::services::pi_prompt_files::PiPromptFileKind::TitleSystem => "TITLE_SYSTEM.md",
        };
        let mode = match kind {
            crate::services::pi_prompt_files::PiPromptFileKind::SystemAppend => {
                crate::t!("Append", "追加")
            }
            crate::services::pi_prompt_files::PiPromptFileKind::SystemOverride => {
                crate::t!("Override", "覆盖")
            }
            crate::services::pi_prompt_files::PiPromptFileKind::TitleSystem => {
                crate::t!("Title", "标题")
            }
        };
        let path = crate::services::pi_prompt_files::OmpPromptFileService::active_path(*kind)
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| texts::tui_na().to_string());
        Row::new(vec![
            Cell::from(if snapshot.exists {
                texts::tui_marker_active()
            } else {
                texts::tui_marker_inactive()
            }),
            Cell::from(filename),
            Cell::from(mode),
            Cell::from(snapshot.content.chars().count().to_string()),
            Cell::from(path),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Length(24),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Min(24),
        ],
    )
    .header(
        Row::new(vec![
            "",
            crate::t!("File", "文件"),
            crate::t!("Mode", "模式"),
            crate::t!("Characters", "字符数"),
            texts::tui_omp_prompt_active_path_header(),
        ])
        .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));
    let mut state = TableState::default();
    state.select(Some(
        app.omp_system_prompt_idx
            .min(data.pi_prompts.system_files.len().saturating_sub(1)),
    ));
    frame.render_stateful_widget(table, inset_left(body, CONTENT_INSET_LEFT), &mut state);
}
