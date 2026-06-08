use super::super::theme;
use super::super::*;

pub(super) fn render_help_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    help: &crate::cli::tui::help::HelpState,
) {
    let area = centered_rect(OVERLAY_LG.0, OVERLAY_LG.1, content_area);
    frame.render_widget(Clear, area);

    let content = &help.content;
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(content.title.clone());
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

    render_key_bar_center(
        frame,
        chunks[0],
        theme,
        &[
            ("↑↓", texts::tui_key_scroll()),
            ("Esc", texts::tui_key_close()),
        ],
    );

    frame.render_widget(
        Paragraph::new(Line::styled(
            content.eyebrow.clone(),
            Style::default().fg(theme.comment),
        )),
        inset_top(chunks[1], 1),
    );

    let body_area = chunks[2];
    render_scrolling_lines(frame, body_area, &content.lines, help.scroll);
}

pub(super) fn render_confirm_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    confirm: &crate::cli::tui::app::ConfirmOverlay,
) {
    let area = centered_rect_fixed(OVERLAY_FIXED_MD.0, OVERLAY_FIXED_MD.1, content_area);
    frame.render_widget(Clear, area);
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, true))
        .title(confirm.title.clone());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);
    let body_area = inset_top(chunks[1], 1);

    if matches!(
        confirm.action,
        ConfirmAction::EditorSaveBeforeClose | ConfirmAction::FormSaveBeforeClose
    ) {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[
                ("Enter", texts::tui_key_save_and_exit()),
                ("N", texts::tui_key_exit_without_save()),
                ("Esc", texts::tui_key_cancel()),
            ],
        );
    } else if matches!(confirm.action, ConfirmAction::ProviderApiFormatProxyNotice) {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[
                ("Enter", texts::tui_key_close()),
                ("Esc", texts::tui_key_close()),
            ],
        );
    } else if matches!(confirm.action, ConfirmAction::VisibleAppsAutoDetection) {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[
                ("Enter", texts::tui_key_use_auto()),
                ("N", texts::tui_key_keep_current()),
                ("Esc", texts::tui_key_keep_current()),
            ],
        );
    } else if matches!(
        confirm.action,
        ConfirmAction::VisibleAppsSwitchToManual { .. }
    ) {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[
                ("Enter", texts::tui_key_switch_to_manual()),
                ("Esc", texts::tui_key_cancel()),
            ],
        );
    } else if matches!(
        confirm.action,
        ConfirmAction::CommonConfigNotice | ConfirmAction::UsageQueryNotice
    ) {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[("Enter", texts::tui_key_close())],
        );
    } else if matches!(confirm.action, ConfirmAction::ManagedAuthCancelLogin) {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[
                ("Enter", texts::tui_key_cancel_login()),
                ("Esc", texts::tui_key_keep_waiting()),
            ],
        );
    } else {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[
                ("Enter", texts::tui_key_yes()),
                ("Esc", texts::tui_key_cancel()),
            ],
        );
    }

    frame.render_widget(
        Paragraph::new(centered_message_lines(
            &confirm.message,
            body_area.width,
            body_area.height,
        ))
        .alignment(Alignment::Center),
        body_area,
    );
}

pub(super) fn render_text_input_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    input: &crate::cli::tui::app::TextInputState,
) {
    let area = centered_rect_fixed(OVERLAY_FIXED_LG.0, 12, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(input.title.clone())
        .style(if theme.no_color {
            Style::default()
        } else {
            Style::default().bg(Color::Black)
        });

    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(inner);

    render_key_bar_center(
        frame,
        chunks[0],
        theme,
        &[
            ("Enter", texts::tui_key_submit()),
            ("Esc", texts::tui_key_cancel()),
        ],
    );

    frame.render_widget(
        Paragraph::new(vec![Line::raw(input.prompt.clone()), Line::raw("")])
            .wrap(Wrap { trim: false }),
        chunks[1],
    );

    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.accent))
        .title(texts::tui_input_title())
        .style(if theme.no_color {
            Style::default()
        } else {
            Style::default().bg(Color::Black)
        });
    let input_inner = input_block.inner(chunks[2]);
    frame.render_widget(input_block, chunks[2]);

    let available = input_inner.width as usize;
    let full = if input.secret {
        "•".repeat(input.input.value.chars().count())
    } else {
        input.input.value.clone()
    };
    let cursor = input.input.cursor.min(full.chars().count());
    let (visible, cursor_x) = visible_text_window(&full, cursor, available);
    frame.render_widget(
        Paragraph::new(Line::from(Span::raw(visible)))
            .wrap(Wrap { trim: false })
            .style(Style::default()),
        input_inner,
    );

    let cursor_x = input_inner.x + cursor_x.min(input_inner.width.saturating_sub(1));
    let cursor_y = input_inner.y;
    frame.set_cursor_position((cursor_x, cursor_y));
}

pub(super) fn render_backup_picker_overlay(
    frame: &mut Frame<'_>,
    data: &UiData,
    content_area: Rect,
    theme: &theme::Theme,
    selected: usize,
) {
    let area = centered_rect(OVERLAY_LG.0, OVERLAY_LG.1, content_area);
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(texts::tui_backup_picker_title());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_key_bar_center(
        frame,
        chunks[0],
        theme,
        &[
            ("Enter", texts::tui_key_restore()),
            ("Esc", texts::tui_key_cancel()),
        ],
    );

    let body_area = inset_top(chunks[1], 1);
    let items = data.config.backups.iter().map(|backup| {
        ListItem::new(Line::from(Span::raw(format!(
            "{}  ({})",
            backup.display_name, backup.id
        ))))
    });

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, body_area, &mut state);
}

pub(super) fn render_text_view_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    title: &str,
    lines: &[String],
    scroll: usize,
    has_action: bool,
) {
    let area = centered_rect(OVERLAY_LG.0, OVERLAY_LG.1, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(title.to_string());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    let mut keys = vec![("↑↓", texts::tui_key_scroll())];
    if has_action {
        keys.push(("T", texts::tui_key_toggle()));
    }
    keys.push(("Esc", texts::tui_key_close()));
    render_key_bar_center(frame, chunks[0], theme, &keys);

    let body_area = inset_top(chunks[1], 1);
    render_scrolling_lines(frame, body_area, lines, scroll);
}

pub(super) fn render_common_snippet_picker_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    selected: usize,
) {
    let area = centered_rect(48, 38, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(texts::tui_config_item_common_snippet());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_key_bar_center(
        frame,
        chunks[0],
        theme,
        &[
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_edit()),
            ("e", texts::tui_key_edit()),
            ("Esc", texts::tui_key_close()),
        ],
    );

    let body_area = inset_top(chunks[1], 1);
    let labels = ["Claude", "Codex", "Gemini", "OpenCode"];
    let items = labels
        .iter()
        .map(|label| ListItem::new(Line::from(Span::raw(label.to_string()))));

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, body_area, &mut state);
}

pub(super) fn render_provider_switch_live_conflicts_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    conflicts: &[crate::services::provider::live_merge::ConfigConflict],
) {
    let area = centered_rect_fixed(OVERLAY_FIXED_LG.0, OVERLAY_FIXED_LG.1, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, true))
        .title("Live configuration conflicts");
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_key_bar_center(
        frame,
        chunks[0],
        theme,
        &[
            ("L", "Keep local"),
            ("C", "Use cc-switch"),
            ("Esc", texts::tui_key_cancel()),
        ],
    );

    let body_area = inset_top(chunks[1], 1);
    let mut lines = vec![
        Line::raw("Choose which values should be written before cc-switch saves this switch."),
        Line::raw(""),
    ];
    let max_conflicts = body_area.height.saturating_sub(4) as usize / 6;
    for conflict in conflicts.iter().take(max_conflicts.max(1)) {
        lines.push(Line::styled(
            format!(
                "{} {} {}",
                conflict.app_type.as_str(),
                conflict.target,
                conflict.path
            ),
            Style::default().fg(theme.accent),
        ));
        lines.push(Line::raw(format!("local: {}", conflict.local)));
        lines.push(Line::raw(format!("cc-switch: {}", conflict.incoming)));
        lines.push(Line::raw(""));
    }
    if conflicts.len() > max_conflicts.max(1) {
        lines.push(Line::raw(format!(
            "... and {} more conflict(s)",
            conflicts.len() - max_conflicts.max(1)
        )));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body_area);
}

fn render_scrolling_lines(frame: &mut Frame<'_>, area: Rect, lines: &[String], scroll: usize) {
    let height = area.height as usize;
    let start = scroll.min(lines.len());
    let end = (start + height).min(lines.len());
    let shown = lines[start..end]
        .iter()
        .map(|s| Line::raw(s.clone()))
        .collect::<Vec<_>>();

    frame.render_widget(Paragraph::new(shown).wrap(Wrap { trim: false }), area);
}
