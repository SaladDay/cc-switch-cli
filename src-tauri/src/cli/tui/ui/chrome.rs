use crate::cli::tui::data;

use super::*;

pub(super) fn render_header(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let title_text = format!("  {}", texts::tui_app_title());
    let title_width = UnicodeWidthStr::width(title_text.as_str()) as u16;

    let title = Paragraph::new(Line::from(vec![Span::styled(
        title_text,
        if theme.no_color {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        },
    )]))
    .alignment(Alignment::Left);

    let visible_apps = crate::settings::get_visible_apps();
    let tabs_line = Line::from(
        super::config::ordered_visible_app_types(&visible_apps)
            .into_iter()
            .enumerate()
            .flat_map(|(idx, app_type)| {
                let style = if app_type == app.app_type {
                    active_chip_style(theme)
                } else {
                    inactive_chip_style(theme)
                };
                let mut spans = Vec::with_capacity(2);
                if idx > 0 {
                    spans.push(Span::raw(" "));
                }
                spans.push(Span::styled(format!(" {} ", app_type.as_str()), style));
                spans
            })
            .collect::<Vec<_>>(),
    );

    let current_provider = data
        .providers
        .rows
        .iter()
        .find(|p| p.is_current)
        .map(|row| data::provider_display_name(&app.app_type, row))
        .unwrap_or_else(|| texts::none().to_string());

    let current_app_routed = data
        .proxy
        .routes_current_app_through_proxy(&app.app_type)
        .unwrap_or(false);

    let proxy_text = texts::tui_header_proxy_status(current_app_routed);
    let proxy_badge = format!("  {proxy_text}  ");
    let proxy_badge_width = UnicodeWidthStr::width(proxy_badge.as_str()) as u16;
    let proxy_style = if current_app_routed {
        selection_style(theme)
    } else if theme.no_color {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White).bg(theme.surface)
    };

    let provider_text_full = format!(
        "{}: {}",
        strip_trailing_colon(texts::provider_label()),
        current_provider
    );
    let available_after_title = area.width.saturating_sub(title_width);
    let full_provider_badge = format!("  {provider_text_full}  ");
    let full_provider_badge_width = UnicodeWidthStr::width(full_provider_badge.as_str()) as u16;
    let full_right_width = proxy_badge_width + 1 + full_provider_badge_width;
    let provider_badge = if full_right_width <= available_after_title {
        full_provider_badge
    } else {
        let provider_badge_budget =
            available_after_title.saturating_sub(proxy_badge_width.saturating_add(1));
        if provider_badge_budget >= 5 {
            let provider_text = truncate_to_display_width(
                &provider_text_full,
                provider_badge_budget.saturating_sub(4),
            );
            format!("  {provider_text}  ")
        } else {
            String::new()
        }
    };
    let provider_badge_width = UnicodeWidthStr::width(provider_badge.as_str()) as u16;
    let right_width = proxy_badge_width
        + if provider_badge.is_empty() {
            0
        } else {
            1 + provider_badge_width
        };

    let title_area = Rect::new(area.x, area.y, title_width.min(area.width), area.height);
    let right_x = area
        .right()
        .saturating_sub(right_width)
        .max(title_area.right());
    let right_area = Rect::new(
        right_x,
        area.y,
        area.right().saturating_sub(right_x),
        area.height,
    );
    let tabs_x = title_area.right();
    let tabs_right = right_area.x;
    let tabs_area = Rect::new(
        tabs_x,
        area.y,
        tabs_right.saturating_sub(tabs_x),
        area.height,
    );

    frame.render_widget(
        Paragraph::new(tabs_line).alignment(Alignment::Center),
        tabs_area,
    );
    frame.render_widget(title, title_area);

    frame.render_widget(
        Paragraph::new(Line::from(if provider_badge.is_empty() {
            vec![Span::styled(proxy_badge, proxy_style)]
        } else {
            vec![
                Span::styled(proxy_badge, proxy_style),
                Span::raw(" "),
                Span::styled(provider_badge, selection_style(theme)),
            ]
        }))
        .alignment(Alignment::Right),
        right_area,
    );
}

pub(super) fn split_nav_label(label: &str) -> (&str, &str) {
    if let Some((icon, rest)) = label.split_once(' ') {
        (icon, rest)
    } else {
        ("", label)
    }
}

pub(super) fn nav_label(item: NavItem) -> &'static str {
    match item {
        NavItem::Main => texts::menu_home(),
        NavItem::Providers => texts::menu_manage_providers(),
        NavItem::Mcp => texts::menu_manage_mcp(),
        NavItem::Prompts => texts::menu_manage_prompts(),
        NavItem::Config => texts::menu_manage_config(),
        NavItem::Skills => texts::menu_manage_skills(),
        NavItem::OpenClawWorkspace => texts::menu_openclaw_workspace(),
        NavItem::OpenClawEnv => texts::menu_openclaw_env(),
        NavItem::OpenClawTools => texts::menu_openclaw_tools(),
        NavItem::OpenClawAgents => texts::menu_openclaw_agents(),
        NavItem::Settings => texts::menu_settings(),
        NavItem::Exit => texts::menu_exit(),
    }
}

pub(super) fn nav_label_variants(item: NavItem) -> (&'static str, &'static str) {
    match item {
        NavItem::Main => texts::menu_home_variants(),
        NavItem::Providers => texts::menu_manage_providers_variants(),
        NavItem::Mcp => texts::menu_manage_mcp_variants(),
        NavItem::Prompts => texts::menu_manage_prompts_variants(),
        NavItem::Config => texts::menu_manage_config_variants(),
        NavItem::Skills => texts::menu_manage_skills_variants(),
        NavItem::OpenClawWorkspace => texts::menu_openclaw_workspace_variants(),
        NavItem::OpenClawEnv => texts::menu_openclaw_env_variants(),
        NavItem::OpenClawTools => texts::menu_openclaw_tools_variants(),
        NavItem::OpenClawAgents => texts::menu_openclaw_agents_variants(),
        NavItem::Settings => texts::menu_settings_variants(),
        NavItem::Exit => texts::menu_exit_variants(),
    }
}

pub(super) fn nav_pane_width(theme: &super::theme::Theme) -> u16 {
    const NAV_BORDER_WIDTH: u16 = 2;
    const NAV_ICON_COL_WIDTH: u16 = 3;
    const NAV_COL_SPACING: u16 = 1;
    const NAV_TEXT_MIN_WIDTH: u16 = 10;
    const NAV_TEXT_EXTRA_WIDTH: u16 = 2;
    let highlight_width = UnicodeWidthStr::width(highlight_symbol(theme)) as u16;

    let max_text_width = NavItem::ALL
        .iter()
        .chain(NavItem::OPENCLAW_ALL.iter())
        .flat_map(|item| {
            let (en, zh) = nav_label_variants(*item);
            [en, zh]
        })
        .map(|label| {
            let (_icon, text) = split_nav_label(label);
            UnicodeWidthStr::width(text) as u16
        })
        .max()
        .unwrap_or(NAV_TEXT_MIN_WIDTH);

    let text_col_width = max_text_width
        .saturating_add(NAV_TEXT_EXTRA_WIDTH)
        .max(NAV_TEXT_MIN_WIDTH);

    NAV_BORDER_WIDTH
        .saturating_add(highlight_width)
        .saturating_add(NAV_ICON_COL_WIDTH)
        .saturating_add(NAV_COL_SPACING)
        .saturating_add(text_col_width)
}
pub(super) fn render_nav(
    frame: &mut Frame<'_>,
    app: &App,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let rows = app.nav_items().iter().map(|item| {
        let (icon, text) = split_nav_label(nav_label(*item));
        let icon_clean = cell_pad(icon).replace('\u{FE0F}', "");
        Row::new(vec![Cell::from(icon_clean), Cell::from(text)])
    });

    let table = Table::new(rows, [Constraint::Length(3), Constraint::Min(10)])
        .column_spacing(1)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(pane_border_style(app, Focus::Nav, theme))
                .title(texts::tui_nav_title()),
        )
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.nav_idx));
    frame.render_stateful_widget(table, area, &mut state);
}

pub(super) fn render_footer(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let current_app_routed = data
        .proxy
        .routes_current_app_through_proxy(&app.app_type)
        .unwrap_or(false);
    let app_supports_proxy_control = data.proxy.takeover_enabled_for(&app.app_type).is_some();
    let proxy_action_available = matches!(app.route, Route::Main)
        && app_supports_proxy_control
        && (!data.proxy.running || data.proxy.managed_runtime || current_app_routed);
    let proxy_footer_label = if current_app_routed {
        texts::tui_key_proxy_off()
    } else {
        texts::tui_key_proxy_on()
    };

    let spans = if app.filter.active {
        vec![Span::styled(
            texts::tui_footer_filter_mode(),
            Style::default().fg(theme.dim),
        )]
    } else {
        if theme.no_color {
            let proxy_segment = if proxy_action_available {
                format!("  P {}", proxy_footer_label)
            } else {
                String::new()
            };
            vec![Span::styled(
                format!(
                    "{} {}  {} {}{}",
                    texts::tui_footer_group_nav(),
                    texts::tui_footer_nav_keys(),
                    texts::tui_footer_group_actions(),
                    texts::tui_footer_action_keys_global(),
                    proxy_segment,
                ),
                Style::default(),
            )]
        } else {
            let nav_bg = super::theme::terminal_palette_color((101, 113, 160)); // #6571A0
            let act_bg = super::theme::terminal_palette_color((248, 248, 248)); // #F8F8F8
            let nav_fg = super::theme::terminal_palette_color((255, 255, 255));
            let act_fg = super::theme::terminal_palette_color((108, 108, 108));
            let nav_label_style = Style::default()
                .fg(nav_fg)
                .bg(nav_bg)
                .add_modifier(Modifier::BOLD);
            let act_label_style = Style::default()
                .fg(act_fg)
                .bg(act_bg)
                .add_modifier(Modifier::BOLD);
            let nav_key_style = Style::default()
                .fg(nav_fg)
                .bg(nav_bg)
                .add_modifier(Modifier::BOLD);
            let nav_desc_style = Style::default().fg(nav_fg).bg(nav_bg);
            let act_key_style = Style::default()
                .fg(act_fg)
                .bg(act_bg)
                .add_modifier(Modifier::BOLD);
            let act_desc_style = Style::default().fg(act_fg).bg(act_bg);
            let nav_sep = Span::styled("  ", nav_desc_style);
            let act_sep = Span::styled("  ", act_desc_style);

            let nav_items: &[(&str, &str)] = if i18n::is_chinese() {
                &[("←→", "菜单/内容"), ("↑↓", "移动")]
            } else {
                &[("←→", "menu/content"), ("↑↓", "move")]
            };

            let act_items_base: &[(&str, &str)] = if i18n::is_chinese() {
                &[
                    ("[ ]", "切换应用"),
                    ("/", "过滤"),
                    ("Esc", "返回"),
                    ("?", "帮助"),
                ]
            } else {
                &[
                    ("[ ]", "switch app"),
                    ("/", "filter"),
                    ("Esc", "back"),
                    ("?", "help"),
                ]
            };

            let mut act_items = act_items_base.to_vec();
            if proxy_action_available {
                act_items.push(("P", proxy_footer_label));
            }

            let mut v = Vec::new();
            // NAV block
            v.push(Span::styled(" NAV ", nav_label_style));
            for (i, (key, desc)) in nav_items.iter().enumerate() {
                if i > 0 {
                    v.push(nav_sep.clone());
                }
                v.push(Span::styled(format!(" {} ", key), nav_key_style));
                v.push(Span::styled(format!(" {}", desc), nav_desc_style));
            }
            v.push(Span::styled(" ", nav_desc_style));
            // gap between blocks
            v.push(Span::raw(" "));
            // ACT block
            v.push(Span::styled(" ACT ", act_label_style));
            for (i, (key, desc)) in act_items.iter().enumerate() {
                if i > 0 {
                    v.push(act_sep.clone());
                }
                v.push(Span::styled(format!(" {} ", key), act_key_style));
                v.push(Span::styled(format!(" {}", desc), act_desc_style));
            }
            v.push(Span::styled(" ", act_desc_style));
            v
        }
    };

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub(super) fn render_toast(frame: &mut Frame<'_>, app: &App, theme: &super::theme::Theme) {
    let Some(toast) = &app.toast else {
        return;
    };

    let content_area = content_pane_rect(frame.area(), theme);
    let (prefix, color) = match toast.kind {
        ToastKind::Info => (
            texts::tui_toast_prefix_info(),
            transient_feedback_color(theme, &toast.kind),
        ),
        ToastKind::Success => (
            texts::tui_toast_prefix_success(),
            transient_feedback_color(theme, &toast.kind),
        ),
        ToastKind::Warning => (
            texts::tui_toast_prefix_warning(),
            transient_feedback_color(theme, &toast.kind),
        ),
        ToastKind::Error => (
            texts::tui_toast_prefix_error(),
            transient_feedback_color(theme, &toast.kind),
        ),
    };
    let message = format!("{} {}", prefix.trim(), toast.message);
    let area = toast_rect(content_area, &message);

    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(color).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(theme.surface));
    frame.render_widget(outer.clone(), area);

    let inner = outer.inner(area);
    let text_style = if theme.no_color {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(color)
            .bg(theme.surface)
            .add_modifier(Modifier::BOLD)
    };

    frame.render_widget(
        Paragraph::new(centered_message_lines(&message, inner.width, inner.height))
            .alignment(Alignment::Center)
            .style(text_style)
            .wrap(Wrap { trim: false }),
        inner,
    );
}

pub(super) fn toast_rect(content_area: Rect, message: &str) -> Rect {
    let max_width = content_area
        .width
        .saturating_sub(4)
        .max(1)
        .min(TOAST_MAX_WIDTH);
    let min_width = TOAST_MIN_WIDTH.min(max_width);
    let width = (UnicodeWidthStr::width(message) as u16)
        .saturating_add(8)
        .clamp(min_width, max_width);

    let inner_width = width.saturating_sub(2).max(1);
    let wrapped_lines = wrap_message_lines(message, inner_width).len() as u16;
    let max_height = content_area.height.saturating_sub(4).max(1);
    let min_height = TOAST_MIN_HEIGHT.min(max_height);
    let height = wrapped_lines
        .saturating_add(2)
        .max(min_height)
        .min(max_height);

    centered_rect_fixed(width, height, content_area)
}
