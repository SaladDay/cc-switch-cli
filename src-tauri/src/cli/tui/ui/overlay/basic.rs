use super::super::theme;
use super::super::*;
use super::action_dialog::{render_action_dialog, ActionDialogSpec};
use super::frame::{overlay_frame, overlay_frame_at, OverlaySize};

pub(super) fn render_help_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    help: &crate::cli::tui::help::HelpState,
) {
    let content = &help.content;
    let body = overlay_frame(
        frame,
        content_area,
        theme,
        &content.title,
        &[
            ("↑↓", texts::tui_key_scroll()),
            ("Esc", texts::tui_key_close()),
        ],
        OverlaySize::Percent(OVERLAY_LG.0, OVERLAY_LG.1),
        overlay_border_style(theme, false),
    );

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(0)])
        .split(body);

    frame.render_widget(
        Paragraph::new(Line::styled(
            content.eyebrow.clone(),
            Style::default().fg(theme.comment),
        )),
        chunks[0],
    );
    render_scrolling_lines(frame, chunks[1], &content.lines, help.scroll);
}

pub(super) fn render_confirm_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    confirm: &crate::cli::tui::app::ConfirmOverlay,
) {
    let keys: &[(&str, &str)] = match &confirm.action {
        ConfirmAction::EditorSaveBeforeClose | ConfirmAction::FormSaveBeforeClose => &[
            ("Enter", texts::tui_key_save_and_exit()),
            ("N", texts::tui_key_exit_without_save()),
            ("Esc", texts::tui_key_cancel()),
        ],
        ConfirmAction::ProviderApiFormatProxyNotice => &[("Enter", texts::tui_key_close())],
        ConfirmAction::VisibleAppsAutoDetection => &[
            ("Enter", texts::tui_key_use_auto()),
            ("Esc", texts::tui_key_keep_current()),
        ],
        ConfirmAction::VisibleAppsSwitchToManual { .. } => &[
            ("Enter", texts::tui_key_switch_to_manual()),
            ("Esc", texts::tui_key_cancel()),
        ],
        ConfirmAction::CommonConfigNotice | ConfirmAction::UsageQueryNotice => {
            &[("Enter", texts::tui_key_close())]
        }
        ConfirmAction::ManagedAuthCancelLogin => &[
            ("Enter", texts::tui_key_cancel_login()),
            ("Esc", texts::tui_key_keep_waiting()),
        ],
        ConfirmAction::RebuildCodexUsage => &[
            ("Enter", texts::tui_key_backup_and_rebuild()),
            ("Esc", texts::tui_key_cancel()),
        ],
        _ => &[
            ("Enter", texts::tui_key_yes()),
            ("Esc", texts::tui_key_cancel()),
        ],
    };

    let body_area = overlay_frame_at(
        frame,
        confirm_overlay_rect(content_area, &confirm.message),
        theme,
        &confirm.title,
        keys,
        overlay_border_style(theme, true),
    );

    let alignment = message_block_alignment(&confirm.message, body_area.width);
    frame.render_widget(
        Paragraph::new(centered_message_lines(
            &confirm.message,
            body_area.width,
            body_area.height,
        ))
        .alignment(alignment),
        body_area,
    );
}

pub(super) fn render_codex_history_confirm_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    confirm: &crate::cli::tui::app::CodexHistoryConfirmState,
) {
    if let Some(actions) = confirm.enable_action_rows() {
        render_action_dialog(
            frame,
            content_area,
            theme,
            ActionDialogSpec {
                title: confirm.title(),
                message: confirm.message(),
                actions: &actions,
                min_size: (OVERLAY_FIXED_LG.0, OVERLAY_FIXED_MD.1),
                border: overlay_border_style(theme, true),
            },
        );
        return;
    }

    let checkbox = confirm.restore_checkbox_label().map(|label| {
        format!(
            "[{}] {label}",
            if confirm.restore_checked { "x" } else { " " }
        )
    });
    let display = if let Some(checkbox) = checkbox.as_ref() {
        format!("{}\n\n{checkbox}", confirm.message())
    } else {
        confirm.message().to_string()
    };
    let keys: &[(&str, &str)] = if checkbox.is_some() {
        &[
            ("Space", texts::tui_key_toggle()),
            (
                "Enter",
                texts::codex_unified_history_disable_confirm_label(),
            ),
            ("Esc", texts::tui_key_cancel()),
        ]
    } else {
        &[
            (
                "Enter",
                texts::codex_unified_history_disable_confirm_label(),
            ),
            ("Esc", texts::tui_key_cancel()),
        ]
    };

    let body_area = overlay_frame_at(
        frame,
        confirm_overlay_rect(content_area, &display),
        theme,
        confirm.title(),
        keys,
        overlay_border_style(theme, true),
    );
    let alignment = message_block_alignment(&display, body_area.width);
    frame.render_widget(
        Paragraph::new(centered_message_lines(
            &display,
            body_area.width,
            body_area.height,
        ))
        .alignment(alignment),
        body_area,
    );
}

pub(super) fn render_text_input_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    input: &crate::cli::tui::app::TextInputState,
) {
    let body = overlay_frame(
        frame,
        content_area,
        theme,
        &input.title,
        &[
            ("Enter", texts::tui_key_submit()),
            ("Esc", texts::tui_key_cancel()),
        ],
        OverlaySize::Fixed(OVERLAY_FIXED_LG.0, 12),
        overlay_border_style(theme, false),
    );

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(body);

    let prompt_lines = input
        .prompt
        .lines()
        .map(|line| Line::raw(line.to_string()))
        .chain(std::iter::once(Line::raw("")))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(prompt_lines).wrap(Wrap { trim: false }),
        chunks[0],
    );

    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.accent))
        .title(format!(" {} ", texts::tui_input_title()));
    let input_inner = input_block.inner(chunks[1]);
    frame.render_widget(input_block, chunks[1]);

    let (visible, cursor_x) = inline_input_window(&input.input, input_inner.width);
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
    let body = overlay_frame(
        frame,
        content_area,
        theme,
        texts::tui_backup_picker_title(),
        &[
            ("Enter", texts::tui_key_restore()),
            ("Esc", texts::tui_key_cancel()),
        ],
        OverlaySize::Percent(OVERLAY_LG.0, OVERLAY_LG.1),
        overlay_border_style(theme, false),
    );

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
    frame.render_stateful_widget(list, body, &mut state);
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
    let mut keys = vec![("↑↓", texts::tui_key_scroll())];
    if has_action {
        keys.push(("T", texts::tui_key_toggle()));
    }
    keys.push(("Esc", texts::tui_key_close()));

    let body = overlay_frame(
        frame,
        content_area,
        theme,
        title,
        &keys,
        OverlaySize::Percent(OVERLAY_LG.0, OVERLAY_LG.1),
        overlay_border_style(theme, false),
    );
    render_scrolling_lines(frame, body, lines, scroll);
}

pub(super) fn render_common_snippet_picker_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    selected: usize,
) {
    let labels = ["Claude", "Codex", "Gemini", "OpenCode"];
    let body = overlay_frame(
        frame,
        content_area,
        theme,
        texts::tui_config_item_common_snippet(),
        &[
            ("↑↓", texts::tui_key_select()),
            ("Enter", texts::tui_key_edit()),
            ("Esc", texts::tui_key_close()),
        ],
        OverlaySize::FitRows {
            width: 48,
            body_rows: labels.len() as u16,
        },
        overlay_border_style(theme, false),
    );

    let items = labels
        .iter()
        .map(|label| ListItem::new(Line::from(Span::raw(label.to_string()))));

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, body, &mut state);
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
