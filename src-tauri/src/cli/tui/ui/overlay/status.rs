use super::super::theme;
use super::super::*;

pub(super) fn render_loading_overlay(
    frame: &mut Frame<'_>,
    app: &App,
    content_area: Rect,
    theme: &theme::Theme,
    kind: LoadingKind,
    title: &str,
    message: &str,
) {
    let area = centered_rect_fixed(OVERLAY_FIXED_MD.0, OVERLAY_FIXED_MD.1, content_area);
    frame.render_widget(Clear, area);

    let spinner = match app.tick % 4 {
        1 => "/",
        2 => "-",
        3 => "\\",
        _ => "|",
    };
    let full_title = format!("{spinner} {title}");

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(full_title);
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    let esc_label = match kind {
        LoadingKind::UpdateCheck => texts::tui_key_cancel(),
        _ => texts::tui_key_close(),
    };
    render_key_bar_center(frame, chunks[0], theme, &[("Esc", esc_label)]);
    let body_area = inset_top(chunks[1], 1);
    frame.render_widget(
        Paragraph::new(centered_message_lines(
            message,
            body_area.width,
            body_area.height,
        ))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false }),
        body_area,
    );
}

pub(super) fn render_speedtest_running_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    url: &str,
) {
    let title = texts::tui_speedtest_title();
    let message = texts::tui_speedtest_running(url);
    render_compact_message_overlay(frame, content_area, theme, title, &message);
}

pub(super) fn render_speedtest_result_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    url: &str,
    lines: &[String],
    scroll: usize,
) {
    render_result_overlay(
        frame,
        content_area,
        theme,
        texts::tui_speedtest_title(),
        texts::tui_speedtest_title_with_url(url),
        lines,
        scroll,
    );
}

pub(super) fn render_stream_check_running_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    provider_name: &str,
) {
    let title = texts::tui_stream_check_title();
    let message = texts::tui_stream_check_running(provider_name);
    render_compact_message_overlay(frame, content_area, theme, title, &message);
}

pub(super) fn render_stream_check_result_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    provider_name: &str,
    lines: &[String],
    scroll: usize,
) {
    render_result_overlay(
        frame,
        content_area,
        theme,
        texts::tui_stream_check_title(),
        texts::tui_stream_check_title_with_provider(provider_name),
        lines,
        scroll,
    );
}

pub(super) fn render_update_available_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    current: &str,
    latest: &str,
    selected: usize,
) {
    let area = centered_rect_fixed(OVERLAY_FIXED_MD.0, OVERLAY_FIXED_MD.1, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, true))
        .title(texts::tui_update_available_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(2),
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

    let version_line = texts::tui_update_version_info(current, latest);
    frame.render_widget(
        Paragraph::new(Line::raw(version_line)).alignment(Alignment::Center),
        chunks[1],
    );

    let update_label = format!("[ {} ]", texts::tui_update_btn_update());
    let cancel_label = format!("[ {} ]", texts::tui_update_btn_cancel());
    let update_style = if selected == 0 {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default().fg(theme.dim)
    };
    let cancel_style = if selected == 1 {
        Style::default()
            .fg(theme.warn)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default().fg(theme.dim)
    };

    let buttons = Line::from(vec![
        Span::styled(update_label, update_style),
        Span::raw("  "),
        Span::styled(cancel_label, cancel_style),
    ]);
    frame.render_widget(
        Paragraph::new(buttons).alignment(Alignment::Center),
        chunks[2],
    );
}

pub(super) fn render_update_downloading_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    downloaded: u64,
    total: Option<u64>,
) {
    let area = centered_rect_fixed(OVERLAY_FIXED_SM.0, OVERLAY_FIXED_SM.1, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, true))
        .title(texts::tui_update_downloading_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_key_bar_center(frame, chunks[0], theme, &[("Esc", texts::tui_key_hide())]);
    let body_area = inset_top(chunks[1], 1);

    let progress_text = if let Some(t) = total {
        if t > 0 {
            let pct = (downloaded.saturating_mul(100) / t).min(100);
            texts::tui_update_downloading_progress(pct, downloaded / 1024, t / 1024)
        } else {
            texts::tui_update_downloading_kb(downloaded / 1024)
        }
    } else {
        texts::tui_update_downloading_kb(downloaded / 1024)
    };

    let gauge_ratio = if let Some(t) = total {
        if t > 0 {
            (downloaded as f64 / t as f64).min(1.0)
        } else {
            0.0
        }
    } else {
        0.0
    };

    frame.render_widget(
        Gauge::default()
            .block(Block::default())
            .gauge_style(Style::default().fg(theme.accent))
            .ratio(gauge_ratio)
            .label(progress_text),
        body_area,
    );
}

pub(super) fn render_update_result_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    success: bool,
    message: &str,
) {
    let area = centered_rect_fixed(OVERLAY_FIXED_SM.0, OVERLAY_FIXED_SM.1, content_area);
    frame.render_widget(Clear, area);

    let border_color = if success {
        transient_feedback_color(theme, &ToastKind::Success)
    } else {
        transient_feedback_color(theme, &ToastKind::Error)
    };
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(border_color))
        .title(texts::tui_update_result_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    let keys = if success {
        [
            ("Enter", texts::tui_key_exit()),
            ("Esc", texts::tui_key_hide()),
        ]
    } else {
        [
            ("Enter", texts::tui_key_close()),
            ("Esc", texts::tui_key_close()),
        ]
    };
    render_key_bar_center(frame, chunks[0], theme, &keys);
    let body_area = inset_top(chunks[1], 1);

    frame.render_widget(
        Paragraph::new(centered_message_lines(
            message,
            body_area.width,
            body_area.height,
        ))
        .alignment(Alignment::Center),
        body_area,
    );
}

fn render_compact_message_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    title: &str,
    message: &str,
) {
    let area = compact_message_overlay_rect(content_area, title, message);
    frame.render_widget(Clear, area);
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(title);
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_key_bar_center(frame, chunks[0], theme, &[("Esc", texts::tui_key_close())]);
    let body_area = inset_top(chunks[1], 1);
    frame.render_widget(
        Paragraph::new(centered_message_lines(
            message,
            body_area.width,
            body_area.height,
        ))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false }),
        body_area,
    );
}

fn render_result_overlay(
    frame: &mut Frame<'_>,
    content_area: Rect,
    theme: &theme::Theme,
    compact_title: &str,
    full_title: String,
    lines: &[String],
    scroll: usize,
) {
    if should_use_compact_lines_overlay(content_area, compact_title, lines) {
        let area = compact_lines_overlay_rect(content_area, compact_title, lines);
        frame.render_widget(Clear, area);

        let outer = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(overlay_border_style(theme, false))
            .title(compact_title);
        frame.render_widget(outer.clone(), area);
        let inner = outer.inner(area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(inner);

        render_key_bar_center(frame, chunks[0], theme, &[("Esc", texts::tui_key_close())]);
        let body_area = inset_top(chunks[1], 1);
        frame.render_widget(
            Paragraph::new(centered_text_lines(
                lines,
                body_area.width,
                body_area.height,
            ))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false }),
            body_area,
        );
        return;
    }

    let area = centered_rect(OVERLAY_LG.0, OVERLAY_LG.1, content_area);
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(overlay_border_style(theme, false))
        .title(full_title);
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
            ("↑↓", texts::tui_key_scroll()),
            ("Esc", texts::tui_key_close()),
        ],
    );

    let body_area = inset_top(chunks[1], 1);
    let height = body_area.height as usize;
    let start = scroll.min(lines.len());
    let end = (start + height).min(lines.len());
    let shown = lines[start..end]
        .iter()
        .map(|s| Line::raw(s.clone()))
        .collect::<Vec<_>>();

    frame.render_widget(Paragraph::new(shown).wrap(Wrap { trim: false }), body_area);
}
