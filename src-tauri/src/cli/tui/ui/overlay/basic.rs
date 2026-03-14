use super::super::theme;
use super::super::*;

pub(super) fn render_help_overlay(frame: &mut Frame<'_>, content_area: Rect, theme: &theme::Theme) {
    let area = centered_rect(OVERLAY_LG.0, OVERLAY_LG.1, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(texts::tui_help_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_key_bar_center(frame, chunks[0], theme, &[("Esc", texts::tui_key_close())]);

    let body_area = inset_top(chunks[1], 1);
    let lines = texts::tui_help_text()
        .lines()
        .map(|s| Line::raw(s.to_string()))
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body_area);
}

pub(super) fn render_provider_switch_first_use_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    title: &str,
    message: &str,
    selected: usize,
) {
    let area = centered_rect_fixed(72, 12, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, true))
        .title(title.to_string());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(4),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    render_key_bar_center(
        frame,
        chunks[0],
        theme,
        &[
            ("←→", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
            ("Esc", texts::tui_key_cancel()),
        ],
    );

    frame.render_widget(
        Paragraph::new(centered_message_lines(
            message,
            chunks[1].width,
            chunks[1].height,
        ))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false }),
        chunks[1],
    );

    let import_style = if selected == 0 {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default().fg(theme.dim)
    };
    let continue_style = if selected == 1 {
        Style::default()
            .fg(theme.warn)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default().fg(theme.dim)
    };
    let cancel_style = if selected == 2 {
        Style::default()
            .fg(theme.dim)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default().fg(theme.dim)
    };

    let buttons = Line::from(vec![
        Span::styled(
            format!(
                "[ {} ]",
                texts::tui_provider_switch_first_use_import_button()
            ),
            import_style,
        ),
        Span::raw("  "),
        Span::styled(
            format!(
                "[ {} ]",
                texts::tui_provider_switch_first_use_continue_button()
            ),
            continue_style,
        ),
        Span::raw("  "),
        Span::styled(
            format!(
                "[ {} ]",
                texts::tui_provider_switch_first_use_cancel_button()
            ),
            cancel_style,
        ),
    ]);
    frame.render_widget(
        Paragraph::new(buttons).alignment(Alignment::Center),
        chunks[2],
    );
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

    if matches!(confirm.action, ConfirmAction::EditorSaveBeforeClose) {
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
    } else if matches!(
        confirm.action,
        ConfirmAction::ProviderApiFormatProxyNotice
            | ConfirmAction::ProviderSwitchSharedConfigNotice
    ) {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[
                ("Enter", texts::tui_key_close()),
                ("Esc", texts::tui_key_close()),
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
        "•".repeat(input.buffer.chars().count())
    } else {
        input.buffer.clone()
    };
    let cursor = full.chars().count();
    let start = cursor.saturating_sub(available);
    let visible = full.chars().skip(start).take(available).collect::<String>();
    frame.render_widget(
        Paragraph::new(Line::from(Span::raw(visible)))
            .wrap(Wrap { trim: false })
            .style(Style::default()),
        input_inner,
    );

    let cursor_x = input_inner.x + (cursor.saturating_sub(start) as u16);
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
            ("Enter", texts::tui_key_view()),
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

pub(super) fn render_common_snippet_view_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    title: &str,
    lines: &[String],
    scroll: usize,
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

    render_key_bar_center(
        frame,
        chunks[0],
        theme,
        &[
            ("a", texts::tui_key_apply()),
            ("c", texts::tui_key_clear()),
            ("e", texts::tui_key_edit()),
            ("↑↓", texts::tui_key_scroll()),
            ("Esc", texts::tui_key_close()),
        ],
    );

    let body_area = inset_top(chunks[1], 1);
    render_scrolling_lines(frame, body_area, lines, scroll);
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
