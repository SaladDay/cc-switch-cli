use chrono::{Local, TimeZone};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Cell, Clear, Gauge, List, ListItem, ListState, Paragraph, Row,
        Table, TableState, Wrap,
    },
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use serde_json::Value;

use super::{
    app::{App, ConfigItem, ConfirmAction, Focus, LoadingKind, Overlay, ToastKind, WebDavConfigItem},
    data::{McpRow, ProviderRow, UiData},
    form::{
        CodexPreviewSection, FormFocus, FormState, GeminiAuthType, McpAddField, ProviderAddField,
    },
    route::{NavItem, Route},
    theme::theme_for,
};

fn pane_border_style(app: &App, pane: Focus, theme: &super::theme::Theme) -> Style {
    if app.focus == pane {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.dim)
    }
}

fn selection_style(theme: &super::theme::Theme) -> Style {
    if theme.no_color {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
            .fg(Color::Black)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    }
}

fn inactive_chip_style(theme: &super::theme::Theme) -> Style {
    if theme.no_color {
        Style::default()
    } else {
        Style::default().fg(Color::White).bg(Color::DarkGray)
    }
}

fn active_chip_style(theme: &super::theme::Theme) -> Style {
    if theme.no_color {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
            .fg(Color::Black)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    }
}

fn pad1(s: &str) -> String {
    format!(" {s}")
}

fn dracula_comment(theme: &super::theme::Theme) -> Style {
    if theme.no_color {
        Style::default().fg(theme.dim)
    } else {
        Style::default().fg(Color::Rgb(98, 114, 164)) // #6272a4
    }
}

fn dracula_cyan(theme: &super::theme::Theme) -> Style {
    if theme.no_color {
        Style::default()
    } else {
        Style::default().fg(Color::Rgb(139, 233, 253)) // #8be9fd
    }
}

fn dracula_dark(theme: &super::theme::Theme) -> Style {
    if theme.no_color {
        Style::default().fg(theme.dim)
    } else {
        Style::default().fg(Color::Rgb(68, 71, 90)) // #44475a
    }
}

fn strip_trailing_colon(label: &str) -> &str {
    label.trim_end_matches([':', '：'])
}

fn pad_to_display_width(label: &str, width: usize) -> String {
    let clean = strip_trailing_colon(label);
    let w = UnicodeWidthStr::width(clean);
    if w >= width {
        clean.to_string()
    } else {
        format!("{clean}{}", " ".repeat(width - w))
    }
}

fn truncate_to_display_width(text: &str, width: u16) -> String {
    let width = width as usize;
    if width == 0 {
        return String::new();
    }

    if UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }

    if width == 1 {
        return "…".to_string();
    }

    let mut out = String::new();
    let mut used = 0usize;
    for c in text.chars() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if used.saturating_add(w) > width.saturating_sub(1) {
            break;
        }
        out.push(c);
        used = used.saturating_add(w);
    }
    out.push('…');
    out
}

fn format_sync_time_local_to_minute(ts: i64) -> Option<String> {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%Y/%m/%d %H:%M").to_string())
}

fn kv_line<'a>(
    theme: &super::theme::Theme,
    label: &'a str,
    label_width: usize,
    value_spans: Vec<Span<'a>>,
) -> Line<'a> {
    let mut spans = vec![
        Span::raw(" "), // internal padding: keep content away from │
        Span::styled(
            pad_to_display_width(label, label_width),
            dracula_comment(theme).add_modifier(Modifier::BOLD),
        ),
        Span::raw(": "),
    ];
    spans.extend(value_spans);
    Line::from(spans)
}

fn highlight_symbol(theme: &super::theme::Theme) -> &'static str {
    if theme.no_color {
        texts::tui_highlight_symbol()
    } else {
        ""
    }
}

const CONTENT_INSET_LEFT: u16 = 1;

fn key_bar_line(theme: &super::theme::Theme, items: &[(&str, &str)]) -> Line<'static> {
    if theme.no_color {
        let mut parts = Vec::new();
        for (k, v) in items {
            parts.push(format!("{k}={v}"));
        }
        return Line::raw(parts.join("  "));
    }

    let base = inactive_chip_style(theme);
    let key = base.add_modifier(Modifier::BOLD);

    let mut spans: Vec<Span<'static>> = vec![Span::styled(" ", base)];
    for (idx, (k, v)) in items.iter().enumerate() {
        if idx > 0 {
            spans.push(Span::styled("  ", base));
        }
        spans.push(Span::styled((*k).to_string(), key));
        spans.push(Span::styled(" ", base));
        spans.push(Span::styled((*v).to_string(), base));
    }
    spans.push(Span::styled(" ", base));
    Line::from(spans)
}

fn render_key_bar(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &super::theme::Theme,
    items: &[(&str, &str)],
) {
    frame.render_widget(
        Paragraph::new(key_bar_line(theme, items))
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_key_bar_center(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &super::theme::Theme,
    items: &[(&str, &str)],
) {
    frame.render_widget(
        Paragraph::new(key_bar_line(theme, items))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn inset_left(area: Rect, left: u16) -> Rect {
    if area.width <= left {
        return Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: area.height,
        };
    }
    Rect {
        x: area.x + left,
        y: area.y,
        width: area.width - left,
        height: area.height,
    }
}

pub fn render(frame: &mut Frame<'_>, app: &App, data: &UiData) {
    let theme = theme_for(&app.app_type);

    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let header_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.dim));
    frame.render_widget(header_block.clone(), root[0]);
    render_header(frame, app, data, header_block.inner(root[0]), &theme);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(nav_pane_width(&theme)),
            Constraint::Min(0),
        ])
        .split(root[1]);

    render_nav(frame, app, body[0], &theme);
    render_content(frame, app, data, body[1], &theme);
    render_footer(frame, app, root[2], &theme);

    render_overlay(frame, app, data, &theme);
}

fn render_header(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(12),
            Constraint::Min(0),
            Constraint::Length(28),
        ])
        .split(area);

    let title = Paragraph::new(Line::from(vec![Span::styled(
        format!("  {}", texts::tui_app_title()),
        if theme.no_color {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        },
    )]))
    .alignment(Alignment::Left);
    frame.render_widget(title, chunks[0]);

    let selected = match app.app_type {
        AppType::Claude => 0,
        AppType::Codex => 1,
        AppType::Gemini => 2,
    };
    let tabs_line = Line::from(vec![
        Span::styled(
            format!("  {}  ", AppType::Claude.as_str()),
            if selected == 0 {
                active_chip_style(theme)
            } else {
                inactive_chip_style(theme)
            },
        ),
        Span::raw(" "),
        Span::styled(
            format!("  {}  ", AppType::Codex.as_str()),
            if selected == 1 {
                active_chip_style(theme)
            } else {
                inactive_chip_style(theme)
            },
        ),
        Span::raw(" "),
        Span::styled(
            format!("  {}  ", AppType::Gemini.as_str()),
            if selected == 2 {
                active_chip_style(theme)
            } else {
                inactive_chip_style(theme)
            },
        ),
    ]);
    let tabs = Paragraph::new(tabs_line).alignment(Alignment::Center);
    frame.render_widget(tabs, chunks[1]);

    let current_provider = data
        .providers
        .rows
        .iter()
        .find(|p| p.is_current)
        .map(|p| p.provider.name.as_str())
        .unwrap_or(texts::none());

    let provider_text = format!(
        "{}: {}",
        strip_trailing_colon(texts::provider_label()),
        current_provider
    );
    let badge_content = format!("  {}  ", provider_text);
    let badge_width = (UnicodeWidthStr::width(badge_content.as_str()) as u16).min(chunks[2].width);
    let right_padding = 1u16;
    let badge_area = Rect {
        x: chunks[2].x.saturating_add(
            chunks[2]
                .width
                .saturating_sub(badge_width.saturating_add(right_padding)),
        ),
        y: chunks[2].y,
        width: badge_width,
        height: 1,
    };

    let badge_style = if theme.no_color {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
            .fg(Color::Black)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    };

    frame.render_widget(
        Paragraph::new(Line::from(Span::raw(badge_content)))
            .alignment(Alignment::Center)
            .style(badge_style)
            .block(Block::default().borders(Borders::NONE)),
        badge_area,
    );
}

fn split_nav_label(label: &str) -> (&str, &str) {
    if let Some((icon, rest)) = label.split_once(' ') {
        (icon, rest)
    } else {
        ("", label)
    }
}

fn nav_label(item: NavItem) -> &'static str {
    match item {
        NavItem::Main => texts::menu_home(),
        NavItem::Providers => texts::menu_manage_providers(),
        NavItem::Mcp => texts::menu_manage_mcp(),
        NavItem::Prompts => texts::menu_manage_prompts(),
        NavItem::Config => texts::menu_manage_config(),
        NavItem::Skills => texts::menu_manage_skills(),
        NavItem::Settings => texts::menu_settings(),
        NavItem::Exit => texts::menu_exit(),
    }
}

fn nav_label_variants(item: NavItem) -> (&'static str, &'static str) {
    match item {
        NavItem::Main => texts::menu_home_variants(),
        NavItem::Providers => texts::menu_manage_providers_variants(),
        NavItem::Mcp => texts::menu_manage_mcp_variants(),
        NavItem::Prompts => texts::menu_manage_prompts_variants(),
        NavItem::Config => texts::menu_manage_config_variants(),
        NavItem::Skills => texts::menu_manage_skills_variants(),
        NavItem::Settings => texts::menu_settings_variants(),
        NavItem::Exit => texts::menu_exit_variants(),
    }
}

fn nav_pane_width(theme: &super::theme::Theme) -> u16 {
    const NAV_BORDER_WIDTH: u16 = 2;
    const NAV_ICON_COL_WIDTH: u16 = 3;
    const NAV_TEXT_MIN_WIDTH: u16 = 10;
    const NAV_TEXT_EXTRA_WIDTH: u16 = 5;
    let highlight_width = UnicodeWidthStr::width(highlight_symbol(theme)) as u16;

    let max_text_width = NavItem::ALL
        .iter()
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
        .saturating_add(text_col_width)
}
fn render_nav(frame: &mut Frame<'_>, app: &App, area: Rect, theme: &super::theme::Theme) {
    let rows = NavItem::ALL.iter().map(|item| {
        let (icon, text) = split_nav_label(nav_label(*item));
        Row::new(vec![Cell::from(pad1(icon)), Cell::from(text)])
    });

    let table = Table::new(rows, [Constraint::Length(3), Constraint::Min(10)])
        .column_spacing(0)
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

fn render_content(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let (filter_area, content_area) = split_filter_area(area, app);

    if let Some(filter_area) = filter_area {
        render_filter_bar(frame, app, filter_area, theme);
    }

    if let Some(editor) = &app.editor {
        render_editor(frame, app, editor, content_area, theme);
        return;
    }

    if let Some(form) = &app.form {
        render_add_form(frame, app, data, form, content_area, theme);
        return;
    }

    match &app.route {
        Route::Main => render_main(frame, app, data, content_area, theme),
        Route::Providers => render_providers(frame, app, data, content_area, theme),
        Route::ProviderDetail { id } => {
            render_provider_detail(frame, app, data, content_area, theme, id)
        }
        Route::Mcp => render_mcp(frame, app, data, content_area, theme),
        Route::Prompts => render_prompts(frame, app, data, content_area, theme),
        Route::Config => render_config(frame, app, data, content_area, theme),
        Route::ConfigWebDav => render_config_webdav(frame, app, data, content_area, theme),
        Route::Skills => render_skills_installed(frame, app, data, content_area, theme),
        Route::SkillsDiscover => render_skills_discover(frame, app, data, content_area, theme),
        Route::SkillsRepos => render_skills_repos(frame, app, data, content_area, theme),
        Route::SkillsUnmanaged => render_skills_unmanaged(frame, app, data, content_area, theme),
        Route::SkillDetail { directory } => {
            render_skill_detail(frame, app, data, content_area, theme, directory)
        }
        Route::Settings => render_settings(frame, app, content_area, theme),
    }
}

fn skills_installed_filtered<'a>(
    app: &App,
    data: &'a UiData,
) -> Vec<&'a crate::services::skill::InstalledSkill> {
    let query = app.filter.query_lower();
    data.skills
        .installed
        .iter()
        .filter(|skill| match &query {
            None => true,
            Some(q) => {
                skill.name.to_lowercase().contains(q)
                    || skill.directory.to_lowercase().contains(q)
                    || skill.id.to_lowercase().contains(q)
            }
        })
        .collect()
}

fn render_skills_installed(
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
        .title(texts::skills_management());
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
                ("x", texts::tui_key_toggle()),
                ("i", texts::tui_skills_action_import_existing()),
            ],
        );
    }

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
    let summary = texts::tui_skills_installed_counts(enabled_claude, enabled_codex, enabled_gemini);

    let summary_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.dim));
    frame.render_widget(
        Paragraph::new(Line::raw(format!("  {summary}")))
            .style(Style::default().fg(theme.dim))
            .wrap(Wrap { trim: false })
            .block(summary_block),
        chunks[1],
    );

    let visible = skills_installed_filtered(app, data);
    if visible.is_empty() {
        let empty_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(0),
                Constraint::Length(7),
                Constraint::Min(0),
            ])
            .split(chunks[2]);

        let icon_style = if theme.no_color {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        };

        let empty_lines = vec![
            Line::raw(""),
            Line::from(Span::styled("✦", icon_style)),
            Line::raw(""),
            Line::from(Span::styled(
                texts::tui_skills_empty_title(),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::raw(""),
            Line::from(Span::styled(
                texts::tui_skills_empty_subtitle(),
                Style::default().fg(theme.dim),
            )),
        ];

        frame.render_widget(
            Paragraph::new(empty_lines)
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: false }),
            empty_chunks[1],
        );
        return;
    }

    let header_style = Style::default().fg(theme.dim).add_modifier(Modifier::BOLD);
    let header = Row::new(vec![
        Cell::from(texts::tui_header_directory()),
        Cell::from(texts::header_name()),
        Cell::from(texts::tui_header_claude_short()),
        Cell::from(texts::tui_header_codex_short()),
        Cell::from(texts::tui_header_gemini_short()),
    ])
    .style(header_style);

    let rows = visible.iter().map(|skill| {
        Row::new(vec![
            Cell::from(skill.directory.clone()),
            Cell::from(skill.name.clone()),
            Cell::from(if skill.apps.claude { "✓" } else { " " }),
            Cell::from(if skill.apps.codex { "✓" } else { " " }),
            Cell::from(if skill.apps.gemini { "✓" } else { " " }),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(55),
            Constraint::Percentage(35),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
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

fn render_skills_discover(
    frame: &mut Frame<'_>,
    app: &App,
    _data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let title = format!(
        "{} — {}",
        texts::tui_skills_discover_title(),
        if app.skills_discover_query.trim().is_empty() {
            texts::tui_skills_discover_query_empty()
        } else {
            app.skills_discover_query.as_str()
        }
    );

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(title);
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(0)])
        .split(inner);

    if app.focus == Focus::Content {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[
                ("Enter", texts::tui_key_install()),
                ("f", texts::tui_key_search()),
            ],
        );
    }

    let query = app.filter.query_lower();
    let visible = app
        .skills_discover_results
        .iter()
        .filter(|skill| match &query {
            None => true,
            Some(q) => {
                skill.name.to_lowercase().contains(q)
                    || skill.directory.to_lowercase().contains(q)
                    || skill.key.to_lowercase().contains(q)
                    || skill.description.to_lowercase().contains(q)
            }
        })
        .collect::<Vec<_>>();

    if visible.is_empty() {
        frame.render_widget(
            Paragraph::new(texts::tui_skills_discover_hint())
                .style(Style::default().fg(theme.dim))
                .wrap(Wrap { trim: false }),
            inset_left(chunks[1], CONTENT_INSET_LEFT),
        );
        return;
    }

    let header_style = Style::default().fg(theme.dim).add_modifier(Modifier::BOLD);
    let header = Row::new(vec![
        Cell::from(""),
        Cell::from(texts::tui_header_directory()),
        Cell::from(texts::header_name()),
        Cell::from(texts::tui_header_repo()),
    ])
    .style(header_style);

    let rows = visible.iter().map(|skill| {
        let repo = match (&skill.repo_owner, &skill.repo_name) {
            (Some(owner), Some(name)) => format!("{owner}/{name}"),
            _ => "-".to_string(),
        };
        Row::new(vec![
            Cell::from(if skill.installed { "✓" } else { " " }),
            Cell::from(skill.directory.clone()),
            Cell::from(skill.name.clone()),
            Cell::from(repo),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Percentage(35),
            Constraint::Percentage(40),
            Constraint::Percentage(25),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.skills_discover_idx));
    frame.render_stateful_widget(table, inset_left(chunks[1], CONTENT_INSET_LEFT), &mut state);
}

fn render_skills_repos(
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
        .title(texts::tui_skills_repos_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    if app.focus == Focus::Content {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[
                ("a", texts::tui_key_add()),
                ("d", texts::tui_key_delete()),
                ("x", texts::tui_key_toggle()),
            ],
        );
    }

    frame.render_widget(
        Paragraph::new(texts::tui_skills_repos_hint())
            .style(Style::default().fg(theme.dim))
            .wrap(Wrap { trim: false }),
        inset_left(chunks[1], CONTENT_INSET_LEFT),
    );

    let query = app.filter.query_lower();
    let visible = data
        .skills
        .repos
        .iter()
        .filter(|repo| match &query {
            None => true,
            Some(q) => {
                repo.owner.to_lowercase().contains(q)
                    || repo.name.to_lowercase().contains(q)
                    || repo.branch.to_lowercase().contains(q)
            }
        })
        .collect::<Vec<_>>();

    if visible.is_empty() {
        frame.render_widget(
            Paragraph::new(texts::tui_skills_repos_empty())
                .style(Style::default().fg(theme.dim))
                .wrap(Wrap { trim: false }),
            inset_left(chunks[2], CONTENT_INSET_LEFT),
        );
        return;
    }

    let header_style = Style::default().fg(theme.dim).add_modifier(Modifier::BOLD);
    let header = Row::new(vec![
        Cell::from(""),
        Cell::from(texts::tui_header_repo()),
        Cell::from(texts::tui_header_branch()),
    ])
    .style(header_style);

    let rows = visible.iter().map(|repo| {
        let repo_name = format!("{}/{}", repo.owner, repo.name);
        Row::new(vec![
            Cell::from(if repo.enabled { "✓" } else { " " }),
            Cell::from(repo_name),
            Cell::from(repo.branch.clone()),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Percentage(70),
            Constraint::Percentage(30),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.skills_repo_idx));
    frame.render_stateful_widget(table, inset_left(chunks[2], CONTENT_INSET_LEFT), &mut state);
}

fn render_skills_unmanaged(
    frame: &mut Frame<'_>,
    app: &App,
    _data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(texts::tui_skills_unmanaged_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    if app.focus == Focus::Content {
        render_key_bar_center(
            frame,
            chunks[0],
            theme,
            &[
                ("Space", texts::tui_key_select()),
                ("i", texts::tui_key_import()),
                ("r", texts::tui_key_refresh()),
            ],
        );
    }

    frame.render_widget(
        Paragraph::new(texts::tui_skills_unmanaged_hint())
            .style(Style::default().fg(theme.dim))
            .wrap(Wrap { trim: false }),
        inset_left(chunks[1], CONTENT_INSET_LEFT),
    );

    let query = app.filter.query_lower();
    let visible = app
        .skills_unmanaged_results
        .iter()
        .filter(|skill| match &query {
            None => true,
            Some(q) => {
                skill.name.to_lowercase().contains(q)
                    || skill.directory.to_lowercase().contains(q)
                    || skill
                        .description
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(q)
                    || skill.found_in.iter().any(|s| s.to_lowercase().contains(q))
            }
        })
        .collect::<Vec<_>>();

    if visible.is_empty() {
        frame.render_widget(
            Paragraph::new(texts::tui_skills_unmanaged_empty())
                .style(Style::default().fg(theme.dim))
                .wrap(Wrap { trim: false }),
            inset_left(chunks[2], CONTENT_INSET_LEFT),
        );
        return;
    }

    let header_style = Style::default().fg(theme.dim).add_modifier(Modifier::BOLD);
    let header = Row::new(vec![
        Cell::from(""),
        Cell::from(texts::tui_header_directory()),
        Cell::from(texts::header_name()),
        Cell::from(texts::tui_header_found_in()),
    ])
    .style(header_style);

    let rows = visible.iter().map(|skill| {
        Row::new(vec![
            Cell::from(
                if app.skills_unmanaged_selected.contains(&skill.directory) {
                    "✓"
                } else {
                    " "
                },
            ),
            Cell::from(skill.directory.clone()),
            Cell::from(skill.name.clone()),
            Cell::from(skill.found_in.join(", ")),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Percentage(45),
            Constraint::Percentage(35),
            Constraint::Percentage(20),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.skills_unmanaged_idx));
    frame.render_stateful_widget(table, inset_left(chunks[2], CONTENT_INSET_LEFT), &mut state);
}

fn render_skill_detail(
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
                ("x", texts::tui_key_toggle()),
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
        Span::raw(format!(
            "claude={}  codex={}  gemini={}",
            skill.apps.claude, skill.apps.codex, skill.apps.gemini
        )),
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

fn render_editor(
    frame: &mut Frame<'_>,
    app: &App,
    editor: &super::app::EditorState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(editor.title.clone());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    let keys = vec![
        ("↑↓←→", texts::tui_key_move()),
        ("Ctrl+S", texts::tui_key_save()),
        ("Esc", texts::tui_key_close()),
    ];
    render_key_bar(frame, chunks[0], theme, &keys);

    let field_title = match editor.kind {
        super::app::EditorKind::Json => texts::tui_editor_json_field_title(),
        super::app::EditorKind::Plain => texts::tui_editor_text_field_title(),
    };
    let field_border_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);

    let field = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(field_border_style)
        .title(format!("-{}", field_title));

    frame.render_widget(field.clone(), chunks[1]);
    let field_inner = field.inner(chunks[1]);

    let height = field_inner.height as usize;
    let width = field_inner.width.max(1);

    let mut shown = Vec::new();
    let start = editor.scroll.min(editor.lines.len().saturating_sub(1));
    for line in editor.lines.iter().skip(start) {
        for segment in super::app::EditorState::wrap_line_segments(line, width) {
            if shown.len() >= height {
                break;
            }
            shown.push(Line::raw(segment));
        }
        if shown.len() >= height {
            break;
        }
    }

    frame.render_widget(Paragraph::new(shown), field_inner);

    let (row_in_view, col_in_view) = editor.cursor_visual_offset_from_scroll(width);
    if row_in_view < height {
        let x = field_inner.x + col_in_view.min(field_inner.width.saturating_sub(1));
        let y = field_inner.y + row_in_view as u16;
        frame.set_cursor_position((x, y));
    }
}

fn focus_block_style(active: bool, theme: &super::theme::Theme) -> Style {
    if active {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.dim)
    }
}

fn add_form_key_items(
    focus: FormFocus,
    editing: bool,
    _codex_split_preview: bool,
) -> Vec<(&'static str, &'static str)> {
    let mut keys = vec![
        ("Tab", texts::tui_key_focus()),
        ("Ctrl+S", texts::tui_key_save()),
        ("Esc", texts::tui_key_close()),
    ];

    match focus {
        FormFocus::Templates => keys.extend([
            ("←→", texts::tui_key_select()),
            ("Enter", texts::tui_key_apply()),
        ]),
        FormFocus::Fields => {
            if editing {
                keys.extend([
                    ("←→", texts::tui_key_move()),
                    ("Enter", texts::tui_key_exit_edit()),
                ]);
            } else {
                keys.extend([
                    ("↑↓", texts::tui_key_select()),
                    ("Enter", texts::tui_key_edit_mode()),
                    ("Space", texts::tui_key_toggle()),
                ]);
            }
        }
        FormFocus::JsonPreview => {
            keys.extend([
                ("Enter", texts::tui_key_edit_mode()),
                ("↑↓", texts::tui_key_scroll()),
            ]);
        }
    }

    keys
}

fn render_form_template_chips(
    frame: &mut Frame<'_>,
    labels: &[&str],
    selected_idx: usize,
    active: bool,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let template_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(active, theme))
        .title(texts::tui_form_templates_title());
    frame.render_widget(template_block.clone(), area);
    let template_inner = template_block.inner(area);

    let mut spans: Vec<Span<'static>> = Vec::new();
    for (idx, label) in labels.iter().enumerate() {
        let selected = idx == selected_idx;
        let style = if selected {
            active_chip_style(theme)
        } else {
            inactive_chip_style(theme)
        };
        spans.push(Span::styled(format!(" {label} "), style));
        spans.push(Span::raw(" "));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).wrap(Wrap { trim: false }),
        template_inner,
    );
}

fn visible_text_window(text: &str, cursor: usize, width: usize) -> (String, u16) {
    if width == 0 {
        return (String::new(), 0);
    }

    let chars = text.chars().collect::<Vec<_>>();
    let cursor = cursor.min(chars.len());

    let mut cum: Vec<usize> = Vec::with_capacity(chars.len() + 1);
    cum.push(0);
    for c in &chars {
        let w = UnicodeWidthChar::width(*c).unwrap_or(0);
        cum.push(cum.last().unwrap_or(&0).saturating_add(w));
    }

    let cursor_x = cum.get(cursor).copied().unwrap_or(0);
    let target = cursor_x.saturating_sub(width.saturating_sub(1));
    let mut start_idx = 0usize;
    while start_idx < cum.len() && cum[start_idx] < target {
        start_idx += 1;
    }

    let mut end_idx = start_idx;
    while end_idx < chars.len() && cum[end_idx + 1].saturating_sub(cum[start_idx]) <= width {
        end_idx += 1;
    }

    let visible = chars
        .get(start_idx..end_idx)
        .unwrap_or_default()
        .iter()
        .collect::<String>();
    let cursor_in_window = cursor_x.saturating_sub(*cum.get(start_idx).unwrap_or(&0));

    (visible, cursor_in_window.min(width) as u16)
}

fn render_form_json_preview(
    frame: &mut Frame<'_>,
    json_text: &str,
    scroll: usize,
    active: bool,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let json_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(active, theme))
        .title(texts::tui_form_json_title());
    frame.render_widget(json_block.clone(), area);
    let json_inner = json_block.inner(area);

    let lines = json_text
        .lines()
        .map(|s| Line::raw(s.to_string()))
        .collect::<Vec<_>>();

    let height = json_inner.height as usize;
    if height == 0 {
        return;
    }
    let max_start = lines.len().saturating_sub(height);
    let start = scroll.min(max_start);
    let end = (start + height).min(lines.len());

    frame.render_widget(
        Paragraph::new(lines[start..end].to_vec()).wrap(Wrap { trim: false }),
        json_inner,
    );
}

fn render_form_text_preview(
    frame: &mut Frame<'_>,
    title: &str,
    text: &str,
    scroll: usize,
    active: bool,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(active, theme))
        .title(title);
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);

    let lines = text
        .lines()
        .map(|s| Line::raw(s.to_string()))
        .collect::<Vec<_>>();

    let height = inner.height as usize;
    if height == 0 {
        return;
    }
    let max_start = lines.len().saturating_sub(height);
    let start = scroll.min(max_start);
    let end = (start + height).min(lines.len());

    frame.render_widget(
        Paragraph::new(lines[start..end].to_vec()).wrap(Wrap { trim: false }),
        inner,
    );
}

fn render_add_form(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    form: &FormState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    match form {
        FormState::ProviderAdd(provider) => {
            render_provider_add_form(frame, app, data, provider, area, theme)
        }
        FormState::McpAdd(mcp) => render_mcp_add_form(frame, app, mcp, area, theme),
    }
}

fn render_provider_add_form(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    provider: &super::form::ProviderAddFormState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let title = match &provider.mode {
        super::form::FormMode::Add => texts::tui_provider_add_title().to_string(),
        super::form::FormMode::Edit { .. } => {
            texts::tui_provider_edit_title(provider.name.value.trim())
        }
    };
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(title);
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let template_height = if matches!(provider.mode, super::form::FormMode::Add) {
        3
    } else {
        0
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(template_height),
            Constraint::Min(0),
        ])
        .split(inner);

    render_key_bar(
        frame,
        chunks[0],
        theme,
        &add_form_key_items(
            provider.focus,
            provider.editing,
            matches!(provider.app_type, AppType::Codex),
        ),
    );

    if matches!(provider.mode, super::form::FormMode::Add) {
        let labels = provider.template_labels();
        render_form_template_chips(
            frame,
            &labels,
            provider.template_idx,
            matches!(provider.focus, FormFocus::Templates),
            chunks[1],
            theme,
        );
    }

    // Body: fields + JSON preview
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(chunks[2]);

    // Fields
    let fields_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(
            matches!(provider.focus, FormFocus::Fields),
            theme,
        ))
        .title(texts::tui_form_fields_title());
    frame.render_widget(fields_block.clone(), body[0]);
    let fields_inner = fields_block.inner(body[0]);

    let show_codex_official_tip = provider.is_codex_official_provider();

    let fields_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if show_codex_official_tip {
            vec![
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(3),
            ]
        } else {
            vec![Constraint::Min(0), Constraint::Length(3)]
        })
        .split(fields_inner);

    let fields = provider.fields();
    let rows_data = fields
        .iter()
        .map(|field| provider_field_label_and_value(provider, *field))
        .collect::<Vec<_>>();

    let label_col_width = field_label_column_width(
        fields
            .iter()
            .zip(rows_data.iter())
            .filter(|(field, _row)| !matches!(field, ProviderAddField::CommonConfigDivider))
            .map(|(_field, (label, _value))| label.as_str())
            .chain(std::iter::once(texts::tui_header_field())),
        1,
    );

    let header = Row::new(vec![
        Cell::from(pad1(texts::tui_header_field())),
        Cell::from(texts::tui_header_value()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let rows = fields
        .iter()
        .zip(rows_data.iter())
        .map(|(field, (label, value))| {
            if matches!(field, ProviderAddField::CommonConfigDivider) {
                let dashes_left = "┄".repeat(40);
                let dashes_right = "┄".repeat(200);
                Row::new(vec![
                    Cell::from(pad1(&dashes_left)),
                    Cell::from(dashes_right),
                ])
                .style(Style::default().fg(theme.dim))
            } else {
                Row::new(vec![Cell::from(pad1(label)), Cell::from(value.clone())])
            }
        });

    let table = Table::new(
        rows,
        [Constraint::Length(label_col_width), Constraint::Min(10)],
    )
    .header(header)
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    if !fields.is_empty() {
        state.select(Some(provider.field_idx.min(fields.len() - 1)));
    }
    let (tip_area, table_area, editor_area) = if show_codex_official_tip {
        (Some(fields_chunks[0]), fields_chunks[1], fields_chunks[2])
    } else {
        (None, fields_chunks[0], fields_chunks[1])
    };

    if let Some(area) = tip_area {
        let tip = texts::tui_codex_official_no_api_key_tip();
        frame.render_widget(
            Paragraph::new(Line::raw(format!("  {}", tip)))
                .style(Style::default().fg(theme.warn).add_modifier(Modifier::BOLD))
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    frame.render_stateful_widget(table, table_area, &mut state);

    // Editor / help line
    let editor_active = matches!(provider.focus, FormFocus::Fields) && provider.editing;
    let editor_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(editor_active, theme))
        .title(if editor_active {
            texts::tui_form_editing_title()
        } else {
            texts::tui_form_input_title()
        });
    frame.render_widget(editor_block.clone(), editor_area);
    let editor_inner = editor_block.inner(editor_area);

    let selected = fields
        .get(provider.field_idx.min(fields.len().saturating_sub(1)))
        .copied();
    if let Some(field) = selected {
        if let Some(input) = provider.input(field) {
            let (visible, cursor_x) =
                visible_text_window(&input.value, input.cursor, editor_inner.width as usize);
            frame.render_widget(
                Paragraph::new(Line::raw(visible)).wrap(Wrap { trim: false }),
                editor_inner,
            );

            if editor_active {
                let x = editor_inner.x + cursor_x.min(editor_inner.width.saturating_sub(1));
                let y = editor_inner.y;
                frame.set_cursor_position((x, y));
            }
        } else {
            let (line, _cursor_col) =
                provider_field_editor_line(provider, selected, editor_inner.width as usize);
            frame.render_widget(
                Paragraph::new(line).wrap(Wrap { trim: false }),
                editor_inner,
            );
        }
    } else {
        frame.render_widget(
            Paragraph::new(Line::raw("")).wrap(Wrap { trim: false }),
            editor_inner,
        );
    }

    if matches!(provider.app_type, AppType::Codex) {
        let provider_json_value = provider.to_provider_json_value();
        let settings_value = provider_json_value
            .get("settingsConfig")
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));

        let auth_value = settings_value
            .get("auth")
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        let auth_value = if auth_value.is_object() {
            auth_value
        } else {
            Value::Object(serde_json::Map::new())
        };
        let auth_text =
            serde_json::to_string_pretty(&auth_value).unwrap_or_else(|_| "{}".to_string());

        let config_text = settings_value
            .get("config")
            .and_then(|value| value.as_str())
            .unwrap_or("");

        let preview = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(body[1]);

        let preview_active = matches!(provider.focus, FormFocus::JsonPreview);
        let auth_active =
            preview_active && matches!(provider.codex_preview_section, CodexPreviewSection::Auth);
        let config_active =
            preview_active && matches!(provider.codex_preview_section, CodexPreviewSection::Config);

        render_form_text_preview(
            frame,
            texts::tui_codex_auth_json_title(),
            &auth_text,
            provider.codex_auth_scroll,
            auth_active,
            preview[0],
            theme,
        );
        render_form_text_preview(
            frame,
            texts::tui_codex_config_toml_title(),
            config_text,
            provider.codex_config_scroll,
            config_active,
            preview[1],
            theme,
        );
    } else {
        // JSON Preview (settingsConfig only, matching upstream UI)
        let provider_json_value = provider
            .to_provider_json_value_with_common_config(&data.config.common_snippet)
            .unwrap_or_else(|_| provider.to_provider_json_value());
        let json_value = provider_json_value
            .get("settingsConfig")
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        let json_text =
            serde_json::to_string_pretty(&json_value).unwrap_or_else(|_| "{}".to_string());
        render_form_json_preview(
            frame,
            &json_text,
            provider.json_scroll,
            matches!(provider.focus, FormFocus::JsonPreview),
            body[1],
            theme,
        );
    }
}

fn provider_field_label_and_value(
    provider: &super::form::ProviderAddFormState,
    field: ProviderAddField,
) -> (String, String) {
    let label = match field {
        ProviderAddField::Id => texts::tui_label_id().to_string(),
        ProviderAddField::Name => texts::header_name().to_string(),
        ProviderAddField::WebsiteUrl => {
            strip_trailing_colon(texts::website_url_label()).to_string()
        }
        ProviderAddField::Notes => strip_trailing_colon(texts::notes_label()).to_string(),
        ProviderAddField::ClaudeBaseUrl => texts::tui_label_base_url().to_string(),
        ProviderAddField::ClaudeApiKey => texts::tui_label_api_key().to_string(),
        ProviderAddField::ClaudeModelConfig => texts::tui_label_claude_model_config().to_string(),
        ProviderAddField::CodexBaseUrl => texts::tui_label_base_url().to_string(),
        ProviderAddField::CodexModel => texts::model_label().to_string(),
        ProviderAddField::CodexWireApi => {
            strip_trailing_colon(texts::codex_wire_api_label()).to_string()
        }
        ProviderAddField::CodexRequiresOpenaiAuth => {
            strip_trailing_colon(texts::codex_auth_mode_label()).to_string()
        }
        ProviderAddField::CodexEnvKey => {
            strip_trailing_colon(texts::codex_env_key_label()).to_string()
        }
        ProviderAddField::CodexApiKey => texts::tui_label_api_key().to_string(),
        ProviderAddField::GeminiAuthType => {
            strip_trailing_colon(texts::auth_type_label()).to_string()
        }
        ProviderAddField::GeminiApiKey => texts::tui_label_api_key().to_string(),
        ProviderAddField::GeminiBaseUrl => texts::tui_label_base_url().to_string(),
        ProviderAddField::GeminiModel => texts::model_label().to_string(),
        ProviderAddField::CommonConfigDivider => "- - - - - - - - -".to_string(),
        ProviderAddField::CommonSnippet => texts::tui_config_item_common_snippet().to_string(),
        ProviderAddField::IncludeCommonConfig => texts::tui_form_attach_common_config().to_string(),
    };

    let value = match field {
        ProviderAddField::CodexWireApi => provider.codex_wire_api.as_str().to_string(),
        ProviderAddField::CodexRequiresOpenaiAuth => {
            if provider.codex_requires_openai_auth {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        ProviderAddField::ClaudeModelConfig => {
            texts::tui_claude_model_config_summary(provider.claude_model_configured_count())
        }
        ProviderAddField::IncludeCommonConfig => {
            if provider.include_common_config {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        ProviderAddField::GeminiAuthType => match provider.gemini_auth_type {
            GeminiAuthType::OAuth => "oauth".to_string(),
            GeminiAuthType::ApiKey => "api_key".to_string(),
        },
        ProviderAddField::CommonConfigDivider => "- - - - - - - - - -".to_string(),
        ProviderAddField::CommonSnippet => texts::tui_key_open().to_string(),
        _ => provider
            .input(field)
            .map(|v| v.value.trim().to_string())
            .unwrap_or_default(),
    };

    (
        label,
        if value.is_empty() {
            texts::tui_na().to_string()
        } else {
            value
        },
    )
}

fn provider_field_editor_line(
    provider: &super::form::ProviderAddFormState,
    selected: Option<ProviderAddField>,
    _width: usize,
) -> (Line<'static>, usize) {
    let Some(field) = selected else {
        return (Line::raw(""), 0);
    };

    if let Some(input) = provider.input(field) {
        let shown = if matches!(
            field,
            ProviderAddField::ClaudeApiKey
                | ProviderAddField::CodexApiKey
                | ProviderAddField::GeminiApiKey
        ) {
            input.value.clone()
        } else {
            input.value.clone()
        };
        (Line::raw(shown), input.cursor)
    } else {
        let text = match field {
            ProviderAddField::CodexWireApi => {
                format!("wire_api = {}", provider.codex_wire_api.as_str())
            }
            ProviderAddField::CodexRequiresOpenaiAuth => format!(
                "requires_openai_auth = {}",
                provider.codex_requires_openai_auth
            ),
            ProviderAddField::ClaudeModelConfig => {
                texts::tui_claude_model_config_open_hint().to_string()
            }
            ProviderAddField::CommonConfigDivider => String::new(),
            ProviderAddField::IncludeCommonConfig => {
                format!("apply_common_config = {}", provider.include_common_config)
            }
            ProviderAddField::GeminiAuthType => {
                format!("auth_type = {}", provider.gemini_auth_type.as_str())
            }
            _ => String::new(),
        };
        (Line::raw(text), 0)
    }
}

fn render_mcp_add_form(
    frame: &mut Frame<'_>,
    app: &App,
    mcp: &super::form::McpAddFormState,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let title = match &mcp.mode {
        super::form::FormMode::Add => texts::tui_mcp_add_title().to_string(),
        super::form::FormMode::Edit { .. } => texts::tui_mcp_edit_title(mcp.name.value.trim()),
    };
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(title);
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let template_height = if matches!(mcp.mode, super::form::FormMode::Add) {
        3
    } else {
        0
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(template_height),
            Constraint::Min(0),
        ])
        .split(inner);

    render_key_bar(
        frame,
        chunks[0],
        theme,
        &add_form_key_items(mcp.focus, mcp.editing, false),
    );

    if matches!(mcp.mode, super::form::FormMode::Add) {
        let labels = mcp.template_labels();
        render_form_template_chips(
            frame,
            &labels,
            mcp.template_idx,
            matches!(mcp.focus, FormFocus::Templates),
            chunks[1],
            theme,
        );
    }

    // Body: fields + JSON preview
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(chunks[2]);

    // Fields
    let fields_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(
            matches!(mcp.focus, FormFocus::Fields),
            theme,
        ))
        .title(texts::tui_form_fields_title());
    frame.render_widget(fields_block.clone(), body[0]);
    let fields_inner = fields_block.inner(body[0]);

    let fields_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3)])
        .split(fields_inner);

    let fields = mcp.fields();
    let rows_data = fields
        .iter()
        .map(|field| mcp_field_label_and_value(mcp, *field))
        .collect::<Vec<_>>();

    let label_col_width = field_label_column_width(
        rows_data
            .iter()
            .map(|(label, _value)| label.as_str())
            .chain(std::iter::once(texts::tui_header_field())),
        1,
    );

    let header = Row::new(vec![
        Cell::from(pad1(texts::tui_header_field())),
        Cell::from(texts::tui_header_value()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let rows = rows_data
        .iter()
        .map(|(label, value)| Row::new(vec![Cell::from(pad1(label)), Cell::from(value.clone())]));

    let table = Table::new(
        rows,
        [Constraint::Length(label_col_width), Constraint::Min(10)],
    )
    .header(header)
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    if !fields.is_empty() {
        state.select(Some(mcp.field_idx.min(fields.len() - 1)));
    }
    frame.render_stateful_widget(table, fields_chunks[0], &mut state);

    // Editor
    let editor_active = matches!(mcp.focus, FormFocus::Fields) && mcp.editing;
    let editor_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(focus_block_style(editor_active, theme))
        .title(if editor_active {
            texts::tui_form_editing_title()
        } else {
            texts::tui_form_input_title()
        });
    frame.render_widget(editor_block.clone(), fields_chunks[1]);
    let editor_inner = editor_block.inner(fields_chunks[1]);

    let selected = fields
        .get(mcp.field_idx.min(fields.len().saturating_sub(1)))
        .copied();
    if let Some(field) = selected {
        if let Some(input) = mcp.input(field) {
            let (visible, cursor_x) =
                visible_text_window(&input.value, input.cursor, editor_inner.width as usize);
            frame.render_widget(
                Paragraph::new(Line::raw(visible)).wrap(Wrap { trim: false }),
                editor_inner,
            );
            if editor_active {
                let x = editor_inner.x + cursor_x.min(editor_inner.width.saturating_sub(1));
                let y = editor_inner.y;
                frame.set_cursor_position((x, y));
            }
        } else {
            let (line, _cursor) = mcp_field_editor_line(mcp, selected, editor_inner.width as usize);
            frame.render_widget(
                Paragraph::new(line).wrap(Wrap { trim: false }),
                editor_inner,
            );
        }
    }

    // JSON Preview
    let json_text = serde_json::to_string_pretty(&mcp.to_mcp_server_json_value())
        .unwrap_or_else(|_| "{}".to_string());
    render_form_json_preview(
        frame,
        &json_text,
        mcp.json_scroll,
        matches!(mcp.focus, FormFocus::JsonPreview),
        body[1],
        theme,
    );
}

fn mcp_field_label_and_value(
    mcp: &super::form::McpAddFormState,
    field: McpAddField,
) -> (String, String) {
    let label = match field {
        McpAddField::Id => texts::tui_label_id().to_string(),
        McpAddField::Name => texts::header_name().to_string(),
        McpAddField::Command => texts::tui_label_command().to_string(),
        McpAddField::Args => texts::tui_label_args().to_string(),
        McpAddField::AppClaude => texts::tui_label_app_claude().to_string(),
        McpAddField::AppCodex => texts::tui_label_app_codex().to_string(),
        McpAddField::AppGemini => texts::tui_label_app_gemini().to_string(),
    };

    let value = match field {
        McpAddField::AppClaude => {
            if mcp.apps.claude {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        McpAddField::AppCodex => {
            if mcp.apps.codex {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        McpAddField::AppGemini => {
            if mcp.apps.gemini {
                format!("[{}]", texts::tui_marker_active())
            } else {
                "[ ]".to_string()
            }
        }
        _ => mcp
            .input(field)
            .map(|v| v.value.trim().to_string())
            .unwrap_or_default(),
    };

    (
        label,
        if value.is_empty() {
            texts::tui_na().to_string()
        } else {
            value
        },
    )
}

fn mcp_field_editor_line(
    mcp: &super::form::McpAddFormState,
    selected: Option<McpAddField>,
    _width: usize,
) -> (Line<'static>, usize) {
    let Some(field) = selected else {
        return (Line::raw(""), 0);
    };

    let text = match field {
        McpAddField::AppClaude => format!("claude = {}", mcp.apps.claude),
        McpAddField::AppCodex => format!("codex = {}", mcp.apps.codex),
        McpAddField::AppGemini => format!("gemini = {}", mcp.apps.gemini),
        _ => String::new(),
    };

    (Line::raw(text), 0)
}

fn split_filter_area(area: Rect, app: &App) -> (Option<Rect>, Rect) {
    let show = app.filter.active || !app.filter.buffer.trim().is_empty();
    if !show {
        return (None, area);
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Min(0)])
        .split(area);

    (Some(chunks[0]), chunks[1])
}

fn field_label_column_width<'a, I>(labels: I, left_padding: u16) -> u16
where
    I: IntoIterator<Item = &'a str>,
{
    let max = labels
        .into_iter()
        .map(|label| UnicodeWidthStr::width(label) as u16)
        .max()
        .unwrap_or(0);
    max.saturating_add(left_padding)
}

fn render_filter_bar(frame: &mut Frame<'_>, app: &App, area: Rect, theme: &super::theme::Theme) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(if app.filter.active {
            Style::default().fg(theme.accent)
        } else {
            Style::default().fg(theme.dim)
        })
        .title(texts::tui_filter_title());

    frame.render_widget(outer.clone(), area);

    let inner = outer.inner(area);
    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(if app.filter.active {
            Style::default().fg(theme.accent)
        } else {
            Style::default().fg(theme.dim)
        })
        .title(texts::tui_filter_icon());

    let input_inner = input_block.inner(inner);
    frame.render_widget(input_block, inner);
    let available = input_inner.width as usize;
    let full = app.filter.buffer.clone();
    let cursor = full.chars().count();
    let start = cursor.saturating_sub(available);
    let visible = full.chars().skip(start).take(available).collect::<String>();

    frame.render_widget(
        Paragraph::new(Line::from(Span::raw(visible))).wrap(Wrap { trim: false }),
        input_inner,
    );

    if app.filter.active {
        let cursor_x = input_inner.x + (cursor.saturating_sub(start) as u16);
        let cursor_y = input_inner.y;
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

fn render_main(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let current_provider = data
        .providers
        .rows
        .iter()
        .find(|p| p.is_current)
        .map(|p| p.provider.name.as_str())
        .unwrap_or(texts::none());

    let mcp_enabled = data
        .mcp
        .rows
        .iter()
        .filter(|s| s.server.apps.is_enabled_for(&app.app_type))
        .count();

    let api_url = data
        .providers
        .rows
        .iter()
        .find(|p| p.is_current)
        .and_then(|p| p.api_url.as_deref())
        .unwrap_or(texts::tui_na());

    let is_online = api_url != texts::tui_na();
    let provider_status = if theme.no_color {
        String::new()
    } else if is_online {
        format!(" ({})", texts::tui_home_status_online())
    } else {
        format!(" ({})", texts::tui_home_status_offline())
    };
    let status_dot = if theme.no_color {
        if is_online {
            "● "
        } else {
            "○ "
        }
    } else {
        "● "
    };
    let status_dot_style = if theme.no_color {
        Style::default()
    } else if is_online {
        Style::default().fg(theme.ok)
    } else {
        Style::default().fg(theme.warn)
    };

    let label_width = 14;
    let value_style = dracula_cyan(theme);
    let provider_name_style = if theme.no_color {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    };

    let provider_value_spans = vec![
        Span::styled(status_dot, status_dot_style),
        Span::styled(current_provider.to_string(), provider_name_style),
        Span::raw(provider_status),
    ];

    let connection_lines = vec![
        kv_line(
            theme,
            texts::provider_label(),
            label_width,
            provider_value_spans,
        ),
        kv_line(
            theme,
            texts::tui_label_api_url(),
            label_width,
            vec![Span::styled(api_url.to_string(), value_style)],
        ),
        kv_line(
            theme,
            texts::mcp_servers_label(),
            label_width,
            vec![Span::styled(
                format!(
                    "{}/{} {}",
                    mcp_enabled,
                    data.mcp.rows.len(),
                    texts::tui_label_mcp_servers_active()
                ),
                value_style,
            )],
        ),
    ];

    let webdav = data.config.webdav_sync.as_ref();
    let is_config_value_set = |value: &str| !value.trim().is_empty();
    let webdav_enabled = webdav.map(|cfg| cfg.enabled).unwrap_or(false);
    let is_configured = webdav
        .map(|cfg| {
            is_config_value_set(&cfg.base_url)
                && is_config_value_set(&cfg.username)
                && is_config_value_set(&cfg.password)
        })
        .unwrap_or(false);
    let webdav_status = webdav.map(|cfg| &cfg.status);
    let last_error = webdav_status
        .and_then(|status| status.last_error.as_deref())
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let has_error = webdav_enabled && is_configured && last_error.is_some();
    let is_ok = webdav_enabled
        && is_configured
        && !has_error
        && webdav_status
            .and_then(|status| status.last_sync_at)
            .is_some();

    let webdav_status_text = if !webdav_enabled || !is_configured {
        texts::tui_webdav_status_not_configured().to_string()
    } else if has_error {
        let detail = last_error
            .map(|err| truncate_to_display_width(err, 22))
            .unwrap_or_default();
        if detail.is_empty() {
            texts::tui_webdav_status_error().to_string()
        } else {
            texts::tui_webdav_status_error_with_detail(&detail)
        }
    } else if is_ok {
        texts::tui_webdav_status_ok().to_string()
    } else {
        texts::tui_webdav_status_configured().to_string()
    };

    let webdav_status_style = if theme.no_color {
        Style::default()
    } else if has_error {
        Style::default().fg(theme.warn)
    } else if is_ok {
        Style::default().fg(theme.ok)
    } else {
        dracula_dark(theme)
    };

    let last_sync_at = webdav_status.and_then(|status| status.last_sync_at);
    let webdav_last_sync_text = last_sync_at
        .and_then(format_sync_time_local_to_minute)
        .unwrap_or_else(|| texts::tui_webdav_status_never_synced().to_string());
    let webdav_last_sync_style = if last_sync_at.is_some() {
        value_style
    } else {
        dracula_dark(theme)
    };

    let webdav_lines = vec![
        kv_line(
            theme,
            texts::tui_label_webdav_status(),
            label_width,
            vec![Span::styled(webdav_status_text, webdav_status_style)],
        ),
        kv_line(
            theme,
            texts::tui_label_webdav_last_sync(),
            label_width,
            vec![Span::styled(webdav_last_sync_text, webdav_last_sync_style)],
        ),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(texts::welcome_title());

    let inner = block.inner(area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(18), Constraint::Min(0)])
        .split(inner);

    frame.render_widget(block, area);

    let top = inset_left(chunks[0], CONTENT_INSET_LEFT);
    let top_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(5),
            Constraint::Length(1),
            Constraint::Length(4),
            Constraint::Length(1),
            Constraint::Length(6),
            Constraint::Min(0),
        ])
        .split(top);

    let card_border = Style::default().fg(theme.dim);

    // Connection card.
    frame.render_widget(
        Paragraph::new(connection_lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Plain)
                    .border_style(card_border)
                    .title(format!(" {} ", texts::tui_home_section_connection())),
            )
            .wrap(Wrap { trim: false }),
        top_chunks[1],
    );

    frame.render_widget(
        Paragraph::new(webdav_lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Plain)
                    .border_style(card_border)
                    .title(format!(" {} ", texts::tui_home_section_webdav())),
            )
            .wrap(Wrap { trim: false }),
        top_chunks[3],
    );

    render_local_env_check_card(frame, app, top_chunks[5], theme, card_border);

    let logo_style = if theme.no_color {
        dracula_dark(theme)
    } else {
        dracula_dark(theme)
    };
    let logo_lines = texts::tui_home_ascii_logo()
        .lines()
        .map(|s| Line::from(Span::styled(s.to_string(), logo_style)))
        .collect::<Vec<_>>();

    let logo_height = (logo_lines.len() as u16).min(chunks[1].height);
    let logo_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(logo_height),
            Constraint::Length(1),
        ])
        .split(chunks[1]);

    frame.render_widget(
        Paragraph::new(logo_lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false }),
        logo_chunks[1],
    );

    frame.render_widget(
        Paragraph::new(Line::raw(texts::tui_main_hint()))
            .alignment(Alignment::Center)
            .style(dracula_dark(theme).add_modifier(Modifier::ITALIC)),
        logo_chunks[2],
    );
}

fn render_local_env_check_card(
    frame: &mut Frame<'_>,
    app: &App,
    area: Rect,
    theme: &super::theme::Theme,
    card_border: Style,
) {
    use crate::services::local_env_check::{LocalTool, ToolCheckStatus};

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(card_border)
        .title(format!(" {} ", texts::tui_home_section_local_env_check()));
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Length(2)])
        .split(inner);

    let cols0 = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[0]);
    let cols1 = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[1]);

    let cells = [
        (LocalTool::Claude, "Claude", cols0[0]),
        (LocalTool::Codex, "Codex", cols0[1]),
        (LocalTool::Gemini, "Gemini", cols1[0]),
        (LocalTool::OpenCode, "OpenCode", cols1[1]),
    ];

    for (tool, display_name, cell_area) in cells {
        let status = if app.local_env_loading {
            None
        } else {
            app.local_env_results
                .iter()
                .find(|r| r.tool == tool)
                .map(|r| &r.status)
        };

        let (icon, icon_style) = if app.local_env_loading {
            ("…", dracula_dark(theme))
        } else {
            match status {
                Some(ToolCheckStatus::Ok { .. }) => (
                    "✓",
                    if theme.no_color {
                        Style::default()
                    } else {
                        Style::default().fg(theme.ok)
                    },
                ),
                Some(ToolCheckStatus::NotInstalledOrNotExecutable) | None => (
                    "!",
                    if theme.no_color {
                        Style::default()
                    } else {
                        Style::default().fg(theme.warn)
                    },
                ),
                Some(ToolCheckStatus::Error { .. }) => (
                    "!",
                    if theme.no_color {
                        Style::default()
                    } else {
                        Style::default().fg(theme.warn)
                    },
                ),
            }
        };

        let name_style = if theme.no_color {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        };

        let detail_style = if theme.no_color {
            Style::default()
        } else {
            dracula_dark(theme)
        };

        let value_style = dracula_cyan(theme);
        let (detail_text, detail_line_style) = if app.local_env_loading {
            ("".to_string(), detail_style)
        } else {
            match status {
                Some(ToolCheckStatus::Ok { version }) => (version.clone(), value_style),
                Some(ToolCheckStatus::NotInstalledOrNotExecutable) | None => (
                    texts::tui_local_env_not_installed().to_string(),
                    detail_style,
                ),
                Some(ToolCheckStatus::Error { message }) => (message.clone(), detail_style),
            }
        };

        let detail_width = cell_area.width.saturating_sub(1);
        let detail_text = truncate_to_display_width(&detail_text, detail_width);

        let lines = vec![
            Line::from(vec![
                Span::raw(" "),
                Span::styled(">_ ", dracula_dark(theme)),
                Span::styled(display_name.to_string(), name_style),
                Span::raw(" "),
                Span::styled(icon.to_string(), icon_style),
            ]),
            Line::from(vec![
                Span::raw(" "),
                Span::styled(detail_text, detail_line_style),
            ]),
        ];

        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), cell_area);
    }
}

fn provider_rows_filtered<'a>(app: &App, data: &'a UiData) -> Vec<&'a ProviderRow> {
    let query = app.filter.query_lower();
    data.providers
        .rows
        .iter()
        .filter(|row| match &query {
            None => true,
            Some(q) => {
                row.provider.name.to_lowercase().contains(q) || row.id.to_lowercase().contains(q)
            }
        })
        .collect()
}

fn render_providers(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let header_style = Style::default().fg(theme.dim).add_modifier(Modifier::BOLD);
    let table_style = Style::default();

    let selected_style = if theme.no_color {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
            .fg(Color::Black)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    };

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(texts::menu_manage_providers());
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
                ("Enter", texts::tui_key_details()),
                ("s", texts::tui_key_switch()),
                ("a", texts::tui_key_add()),
                ("e", texts::tui_key_edit()),
                ("d", texts::tui_key_delete()),
                ("t", texts::tui_key_speedtest()),
            ],
        );
    }

    let visible = provider_rows_filtered(app, data);

    let header = Row::new(vec![
        Cell::from(""),
        Cell::from(texts::header_name()),
        Cell::from(texts::tui_header_api_url()),
    ])
    .style(header_style);

    let rows = visible.iter().map(|row| {
        let marker = if row.is_current {
            texts::tui_marker_active()
        } else {
            texts::tui_marker_inactive()
        };
        let api = row.api_url.as_deref().unwrap_or(texts::tui_na());
        Row::new(vec![
            Cell::from(marker),
            Cell::from(row.provider.name.clone()),
            Cell::from(api),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Percentage(45),
            Constraint::Percentage(55),
        ],
    )
    .header(header)
    .style(table_style)
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selected_style)
    .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.provider_idx));

    frame.render_stateful_widget(table, inset_left(chunks[1], CONTENT_INSET_LEFT), &mut state);
}

fn render_provider_detail(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
    id: &str,
) {
    let Some(row) = data.providers.rows.iter().find(|p| p.id == id) else {
        frame.render_widget(
            Paragraph::new(texts::tui_provider_not_found()).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Plain)
                    .border_style(pane_border_style(app, Focus::Content, theme))
                    .title(texts::tui_provider_title()),
            ),
            area,
        );
        return;
    };

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(texts::tui_provider_detail_title());
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
                ("s", texts::tui_key_switch()),
                ("e", texts::tui_key_edit()),
                ("t", texts::tui_key_speedtest()),
            ],
        );
    }

    let mut lines = vec![
        Line::from(vec![
            Span::styled(texts::tui_label_id(), Style::default().fg(theme.accent)),
            Span::raw(": "),
            Span::raw(row.id.clone()),
        ]),
        Line::from(vec![
            Span::styled(texts::header_name(), Style::default().fg(theme.accent)),
            Span::raw(": "),
            Span::raw(row.provider.name.clone()),
        ]),
        Line::raw(""),
    ];

    if let Some(url) = row.api_url.as_deref() {
        lines.push(Line::from(vec![
            Span::styled(
                texts::tui_label_api_url(),
                Style::default().fg(theme.accent),
            ),
            Span::raw(": "),
            Span::raw(url),
        ]));
    }

    if matches!(app.app_type, crate::app_config::AppType::Claude) {
        if let Some(env) = row
            .provider
            .settings_config
            .get("env")
            .and_then(|v| v.as_object())
        {
            let api_key = env
                .get("ANTHROPIC_AUTH_TOKEN")
                .or_else(|| env.get("ANTHROPIC_API_KEY"))
                .and_then(|v| v.as_str())
                .map(mask_api_key)
                .unwrap_or_else(|| texts::tui_na().to_string());
            let base_url = env
                .get("ANTHROPIC_BASE_URL")
                .and_then(|v| v.as_str())
                .unwrap_or(texts::tui_na());

            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                Span::styled(
                    texts::tui_label_base_url(),
                    Style::default().fg(theme.accent),
                ),
                Span::raw(": "),
                Span::raw(base_url),
            ]));
            lines.push(Line::from(vec![
                Span::styled(
                    texts::tui_label_api_key(),
                    Style::default().fg(theme.accent),
                ),
                Span::raw(": "),
                Span::raw(api_key),
            ]));
        }
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::NONE))
            .wrap(Wrap { trim: false }),
        inset_left(chunks[1], CONTENT_INSET_LEFT),
    );
}

fn mcp_rows_filtered<'a>(app: &App, data: &'a UiData) -> Vec<&'a McpRow> {
    let query = app.filter.query_lower();
    data.mcp
        .rows
        .iter()
        .filter(|row| match &query {
            None => true,
            Some(q) => {
                row.server.name.to_lowercase().contains(q) || row.id.to_lowercase().contains(q)
            }
        })
        .collect()
}

fn render_mcp(
    frame: &mut Frame<'_>,
    app: &App,
    data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let visible = mcp_rows_filtered(app, data);

    let header = Row::new(vec![
        Cell::from(texts::header_name()),
        Cell::from(crate::app_config::AppType::Claude.as_str()),
        Cell::from(crate::app_config::AppType::Codex.as_str()),
        Cell::from(crate::app_config::AppType::Gemini.as_str()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let rows = visible.iter().map(|row| {
        Row::new(vec![
            Cell::from(row.server.name.clone()),
            Cell::from(if row.server.apps.claude {
                texts::tui_marker_active()
            } else {
                texts::tui_marker_inactive()
            }),
            Cell::from(if row.server.apps.codex {
                texts::tui_marker_active()
            } else {
                texts::tui_marker_inactive()
            }),
            Cell::from(if row.server.apps.gemini {
                texts::tui_marker_active()
            } else {
                texts::tui_marker_inactive()
            }),
        ])
    });

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(texts::menu_manage_mcp());
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
                ("x", texts::tui_key_toggle()),
                ("m", texts::tui_key_apps()),
                ("a", texts::tui_key_add()),
                ("e", texts::tui_key_edit()),
                ("i", texts::tui_key_import()),
                ("v", texts::tui_key_validate()),
                ("d", texts::tui_key_delete()),
            ],
        );
    }

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(55),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(7),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.mcp_idx));

    frame.render_stateful_widget(table, inset_left(chunks[1], CONTENT_INSET_LEFT), &mut state);
}

fn render_prompts(
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

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(texts::menu_manage_prompts());
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
                ("Enter", texts::tui_key_view()),
                ("a", texts::tui_key_activate()),
                ("x", texts::tui_key_deactivate_active()),
                ("e", texts::tui_key_edit()),
                ("d", texts::tui_key_delete()),
            ],
        );
    }

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

    let mut state = TableState::default();
    state.select(Some(app.prompt_idx));
    frame.render_stateful_widget(table, inset_left(chunks[1], CONTENT_INSET_LEFT), &mut state);
}

fn config_items_filtered(app: &App) -> Vec<ConfigItem> {
    let Some(q) = app.filter.query_lower() else {
        return ConfigItem::ALL.to_vec();
    };
    ConfigItem::ALL
        .iter()
        .cloned()
        .filter(|item| config_item_label(item).to_lowercase().contains(&q))
        .collect()
}

fn config_item_label(item: &ConfigItem) -> &'static str {
    match item {
        ConfigItem::Path => texts::tui_config_item_show_path(),
        ConfigItem::ShowFull => texts::tui_config_item_show_full(),
        ConfigItem::Export => texts::tui_config_item_export(),
        ConfigItem::Import => texts::tui_config_item_import(),
        ConfigItem::Backup => texts::tui_config_item_backup(),
        ConfigItem::Restore => texts::tui_config_item_restore(),
        ConfigItem::Validate => texts::tui_config_item_validate(),
        ConfigItem::CommonSnippet => texts::tui_config_item_common_snippet(),
        ConfigItem::WebDavSync => texts::tui_config_item_webdav_sync(),
        ConfigItem::Reset => texts::tui_config_item_reset(),
    }
}

fn webdav_config_items_filtered(app: &App) -> Vec<WebDavConfigItem> {
    let Some(q) = app.filter.query_lower() else {
        return WebDavConfigItem::ALL.to_vec();
    };
    WebDavConfigItem::ALL
        .iter()
        .cloned()
        .filter(|item| webdav_config_item_label(item).to_lowercase().contains(&q))
        .collect()
}

fn webdav_config_item_label(item: &WebDavConfigItem) -> &'static str {
    match item {
        WebDavConfigItem::Settings => texts::tui_config_item_webdav_settings(),
        WebDavConfigItem::CheckConnection => texts::tui_config_item_webdav_check_connection(),
        WebDavConfigItem::Upload => texts::tui_config_item_webdav_upload(),
        WebDavConfigItem::Download => texts::tui_config_item_webdav_download(),
        WebDavConfigItem::Reset => texts::tui_config_item_webdav_reset(),
        WebDavConfigItem::JianguoyunQuickSetup => {
            texts::tui_config_item_webdav_jianguoyun_quick_setup()
        }
    }
}

fn render_config(
    frame: &mut Frame<'_>,
    app: &App,
    _data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let items = config_items_filtered(app);
    let rows = items
        .iter()
        .map(|item| Row::new(vec![Cell::from(config_item_label(item))]));

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(texts::tui_config_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    if app.focus == Focus::Content {
        let mut keys = vec![("Enter", texts::tui_key_select())];
        if matches!(items.get(app.config_idx), Some(ConfigItem::CommonSnippet)) {
            keys.push(("e", texts::tui_key_edit_snippet()));
        }
        render_key_bar_center(frame, chunks[0], theme, &keys);
    }

    let table = Table::new(rows, [Constraint::Min(10)])
        .block(Block::default().borders(Borders::NONE))
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.config_idx));
    frame.render_stateful_widget(table, inset_left(chunks[1], CONTENT_INSET_LEFT), &mut state);
}

fn render_config_webdav(
    frame: &mut Frame<'_>,
    app: &App,
    _data: &UiData,
    area: Rect,
    theme: &super::theme::Theme,
) {
    let items = webdav_config_items_filtered(app);
    let rows = items
        .iter()
        .map(|item| Row::new(vec![Cell::from(webdav_config_item_label(item))]));

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(texts::tui_config_webdav_title());
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    if app.focus == Focus::Content {
        let mut keys = vec![("Enter", texts::tui_key_select())];
        if matches!(
            items.get(app.config_webdav_idx),
            Some(WebDavConfigItem::Settings)
        ) {
            keys.push(("e", texts::tui_key_edit()));
        }
        render_key_bar_center(frame, chunks[0], theme, &keys);
    }

    let table = Table::new(rows, [Constraint::Min(10)])
        .block(Block::default().borders(Borders::NONE))
        .row_highlight_style(selection_style(theme))
        .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.config_webdav_idx));
    frame.render_stateful_widget(table, inset_left(chunks[1], CONTENT_INSET_LEFT), &mut state);
}

fn render_settings(frame: &mut Frame<'_>, app: &App, area: Rect, theme: &super::theme::Theme) {
    let language = crate::cli::i18n::current_language();
    let skip_claude_onboarding = crate::settings::get_skip_claude_onboarding();

    let rows_data = super::app::SettingsItem::ALL
        .iter()
        .map(|item| match item {
            super::app::SettingsItem::Language => (
                texts::tui_settings_header_language().to_string(),
                language.display_name().to_string(),
            ),
            super::app::SettingsItem::SkipClaudeOnboarding => (
                texts::skip_claude_onboarding_label().to_string(),
                if skip_claude_onboarding {
                    texts::enabled().to_string()
                } else {
                    texts::disabled().to_string()
                },
            ),
            super::app::SettingsItem::CheckForUpdates => (
                texts::tui_settings_check_for_updates().to_string(),
                format!("v{}", env!("CARGO_PKG_VERSION")),
            ),
        })
        .collect::<Vec<_>>();

    let label_col_width = field_label_column_width(
        rows_data
            .iter()
            .map(|(label, _value)| label.as_str())
            .chain(std::iter::once(texts::tui_settings_header_setting())),
        0,
    );

    let header = Row::new(vec![
        Cell::from(texts::tui_settings_header_setting()),
        Cell::from(texts::tui_settings_header_value()),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let rows = rows_data
        .iter()
        .map(|(label, value)| Row::new(vec![Cell::from(label.clone()), Cell::from(value.clone())]));

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(pane_border_style(app, Focus::Content, theme))
        .title(texts::menu_settings());
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
            &[("Enter", texts::tui_key_apply())],
        );
    }

    let table = Table::new(
        rows,
        [Constraint::Length(label_col_width), Constraint::Min(10)],
    )
    .header(header)
    .block(Block::default().borders(Borders::NONE))
    .row_highlight_style(selection_style(theme))
    .highlight_symbol(highlight_symbol(theme));

    let mut state = TableState::default();
    state.select(Some(app.settings_idx));
    frame.render_stateful_widget(table, inset_left(chunks[1], CONTENT_INSET_LEFT), &mut state);
}

fn render_footer(frame: &mut Frame<'_>, app: &App, area: Rect, theme: &super::theme::Theme) {
    let mut spans = if app.filter.active {
        vec![Span::styled(
            texts::tui_footer_filter_mode(),
            Style::default().fg(theme.dim),
        )]
    } else {
        if theme.no_color {
            vec![Span::styled(
                format!(
                    "{} {}  {} {}",
                    texts::tui_footer_group_nav(),
                    texts::tui_footer_nav_keys(),
                    texts::tui_footer_group_actions(),
                    texts::tui_footer_action_keys_global()
                ),
                Style::default(),
            )]
        } else {
            let nav_bg = Color::DarkGray;
            let act_bg = Color::Gray;

            let nav_style = Style::default().fg(Color::White).bg(nav_bg);
            let nav_label_style = nav_style.add_modifier(Modifier::BOLD);
            let act_style = Style::default().fg(Color::White).bg(act_bg);
            let act_label_style = act_style.add_modifier(Modifier::BOLD);

            vec![
                Span::styled(" ", nav_style),
                Span::styled(texts::tui_footer_group_nav(), nav_label_style),
                Span::styled(" ", nav_style),
                Span::styled(texts::tui_footer_nav_keys(), nav_style),
                Span::styled(" ", nav_style),
                Span::raw(" "),
                Span::styled(" ", act_style),
                Span::styled(texts::tui_footer_group_actions(), act_label_style),
                Span::styled(" ", act_style),
                Span::styled(texts::tui_footer_action_keys_global(), act_style),
                Span::styled(" ", act_style),
            ]
        }
    };

    if let Some(toast) = &app.toast {
        let (prefix, color) = match toast.kind {
            ToastKind::Info => (texts::tui_toast_prefix_info(), theme.accent),
            ToastKind::Success => (texts::tui_toast_prefix_success(), theme.ok),
            ToastKind::Warning => (texts::tui_toast_prefix_warning(), theme.warn),
            ToastKind::Error => (texts::tui_toast_prefix_error(), theme.err),
        };
        spans.push(Span::raw("  "));
        spans.push(Span::styled(prefix, Style::default().fg(color)));
        spans.push(Span::raw(toast.message.clone()));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_overlay(frame: &mut Frame<'_>, app: &App, data: &UiData, theme: &super::theme::Theme) {
    let content_area = content_pane_rect(frame.area(), theme);

    match &app.overlay {
        Overlay::None => {}
        Overlay::Help => {
            let area = centered_rect(70, 70, content_area);
            frame.render_widget(Clear, area);

            let outer = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(theme.dim))
                .title(texts::tui_help_title());
            frame.render_widget(outer.clone(), area);
            let inner = outer.inner(area);

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(0)])
                .split(inner);

            render_key_bar_center(frame, chunks[0], theme, &[("Esc", texts::tui_key_close())]);

            let lines = texts::tui_help_text()
                .lines()
                .map(|s| Line::raw(s.to_string()))
                .collect::<Vec<_>>();
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), chunks[1]);
        }
        Overlay::Confirm(confirm) => {
            let area = centered_rect_fixed(60, 7, content_area);
            frame.render_widget(Clear, area);
            let outer = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(theme.dim))
                .title(confirm.title.clone());
            frame.render_widget(outer.clone(), area);
            let inner = outer.inner(area);

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(0)])
                .split(inner);

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
                frame.render_widget(
                    Paragraph::new(centered_message_lines(
                        &confirm.message,
                        chunks[1].width,
                        chunks[1].height,
                    ))
                    .alignment(Alignment::Center),
                    chunks[1],
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
                frame.render_widget(
                    Paragraph::new(centered_message_lines(
                        &confirm.message,
                        chunks[1].width,
                        chunks[1].height,
                    ))
                    .alignment(Alignment::Center),
                    chunks[1],
                );
            }
        }
        Overlay::TextInput(input) => {
            let area = centered_rect_fixed(70, 12, content_area);
            frame.render_widget(Clear, area);

            let outer = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(theme.dim))
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

            let available = input_inner.width.saturating_sub(0) as usize;
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
        Overlay::BackupPicker { selected } => {
            let area = centered_rect(80, 80, content_area);
            frame.render_widget(Clear, area);

            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(theme.dim))
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
            state.select(Some(*selected));
            frame.render_stateful_widget(list, chunks[1], &mut state);
        }
        Overlay::TextView(view) => {
            let area = centered_rect(90, 90, content_area);
            frame.render_widget(Clear, area);

            let outer = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(theme.dim))
                .title(view.title.clone());
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

            let height = chunks[1].height as usize;
            let start = view.scroll.min(view.lines.len());
            let end = (start + height).min(view.lines.len());
            let shown = view.lines[start..end]
                .iter()
                .map(|s| Line::raw(s.clone()))
                .collect::<Vec<_>>();

            frame.render_widget(Paragraph::new(shown).wrap(Wrap { trim: false }), chunks[1]);
        }
        Overlay::CommonSnippetPicker { selected } => {
            let area = centered_rect(48, 38, content_area);
            frame.render_widget(Clear, area);

            let outer = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(theme.dim))
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

            let labels = ["Claude", "Codex", "Gemini"];
            let items = labels
                .iter()
                .map(|label| ListItem::new(Line::from(Span::raw(label.to_string()))));

            let list = List::new(items)
                .highlight_style(selection_style(theme))
                .highlight_symbol(highlight_symbol(theme));

            let mut state = ListState::default();
            state.select(Some(*selected));
            frame.render_stateful_widget(list, chunks[1], &mut state);
        }
        Overlay::CommonSnippetView { view, .. } => {
            let area = centered_rect(90, 90, content_area);
            frame.render_widget(Clear, area);

            let outer = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(theme.dim))
                .title(view.title.clone());
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

            let height = chunks[1].height as usize;
            let start = view.scroll.min(view.lines.len());
            let end = (start + height).min(view.lines.len());
            let shown = view.lines[start..end]
                .iter()
                .map(|s| Line::raw(s.clone()))
                .collect::<Vec<_>>();

            frame.render_widget(Paragraph::new(shown).wrap(Wrap { trim: false }), chunks[1]);
        }
        Overlay::ClaudeModelPicker { selected, editing } => {
            let area = centered_rect(78, 62, content_area);
            frame.render_widget(Clear, area);

            let outer = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(theme.dim))
                .title(texts::tui_claude_model_config_popup_title());
            frame.render_widget(outer.clone(), area);
            let inner = outer.inner(area);

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Min(0),
                    Constraint::Length(3),
                ])
                .split(inner);

            render_key_bar_center(
                frame,
                chunks[0],
                theme,
                &[
                    ("↑↓", texts::tui_key_select()),
                    (
                        "Enter",
                        if *editing {
                            texts::tui_key_exit_edit()
                        } else {
                            texts::tui_key_edit_mode()
                        },
                    ),
                    (
                        "Esc",
                        if *editing {
                            texts::tui_key_exit_edit()
                        } else {
                            texts::tui_key_close()
                        },
                    ),
                ],
            );

            if let Some(FormState::ProviderAdd(provider)) = app.form.as_ref() {
                let labels = [
                    texts::tui_claude_model_main_label(),
                    texts::tui_claude_reasoning_model_label(),
                    texts::tui_claude_default_haiku_model_label(),
                    texts::tui_claude_default_sonnet_model_label(),
                    texts::tui_claude_default_opus_model_label(),
                ];

                let label_col_width = field_label_column_width(
                    labels
                        .iter()
                        .copied()
                        .chain(std::iter::once(texts::tui_header_field())),
                    1,
                );

                let header = Row::new(vec![
                    Cell::from(pad1(texts::tui_header_field())),
                    Cell::from(texts::tui_header_value()),
                ])
                .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

                let rows = labels.iter().enumerate().map(|(idx, label)| {
                    let value = provider
                        .claude_model_input(idx)
                        .map(|input| input.value.trim().to_string())
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| texts::tui_na().to_string());
                    Row::new(vec![Cell::from(pad1(label)), Cell::from(value)])
                });

                let table = Table::new(
                    rows,
                    [Constraint::Length(label_col_width), Constraint::Min(10)],
                )
                .header(header)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(texts::tui_form_fields_title()),
                )
                .row_highlight_style(selection_style(theme))
                .highlight_symbol(highlight_symbol(theme));

                let mut state = TableState::default();
                state.select(Some((*selected).min(labels.len().saturating_sub(1))));
                frame.render_stateful_widget(table, chunks[1], &mut state);

                let input_block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Plain)
                    .border_style(if *editing {
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.dim)
                    })
                    .title(if *editing {
                        texts::tui_form_editing_title()
                    } else {
                        texts::tui_form_input_title()
                    });
                frame.render_widget(input_block.clone(), chunks[2]);
                let input_inner = input_block.inner(chunks[2]);

                if let Some(input) = provider.claude_model_input(*selected) {
                    let (visible, cursor_x) =
                        visible_text_window(&input.value, input.cursor, input_inner.width as usize);
                    frame.render_widget(
                        Paragraph::new(Line::raw(visible)).wrap(Wrap { trim: false }),
                        input_inner,
                    );
                    if *editing {
                        let x = input_inner.x + cursor_x.min(input_inner.width.saturating_sub(1));
                        let y = input_inner.y;
                        frame.set_cursor_position((x, y));
                    }
                }
            } else {
                frame.render_widget(
                    Paragraph::new(Line::raw(texts::tui_provider_not_found())),
                    chunks[1],
                );
            }
        }
        Overlay::McpAppsPicker {
            name,
            selected,
            apps,
            ..
        } => {
            let area = centered_rect_fixed(60, 12, content_area);
            frame.render_widget(Clear, area);

            let outer = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(theme.dim))
                .title(texts::tui_mcp_apps_title(name));
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
                    ("x", texts::tui_key_toggle()),
                    ("Enter", texts::tui_key_apply()),
                    ("Esc", texts::tui_key_cancel()),
                ],
            );

            let items = [
                crate::app_config::AppType::Claude,
                crate::app_config::AppType::Codex,
                crate::app_config::AppType::Gemini,
            ]
            .into_iter()
            .map(|app_type| {
                let enabled = apps.is_enabled_for(&app_type);
                let marker = if enabled {
                    texts::tui_marker_active()
                } else {
                    texts::tui_marker_inactive()
                };

                ListItem::new(Line::from(Span::raw(format!(
                    "{marker}  {}",
                    app_type.as_str()
                ))))
            });

            let list = List::new(items)
                .highlight_style(selection_style(theme))
                .highlight_symbol(highlight_symbol(theme));

            let mut state = ListState::default();
            state.select(Some(*selected));
            frame.render_stateful_widget(list, chunks[1], &mut state);
        }
        Overlay::SkillsSyncMethodPicker { selected } => {
            let area = centered_rect_fixed(60, 12, content_area);
            frame.render_widget(Clear, area);

            let outer = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(theme.dim))
                .title(texts::tui_skills_sync_method_title());
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
                    ("←→", texts::tui_key_select()),
                    ("Enter", texts::tui_key_apply()),
                    ("Esc", texts::tui_key_cancel()),
                ],
            );

            let current = data.skills.sync_method;
            let methods = [
                crate::services::skill::SyncMethod::Auto,
                crate::services::skill::SyncMethod::Symlink,
                crate::services::skill::SyncMethod::Copy,
            ];

            let items = methods.into_iter().map(|method| {
                let marker = if method == current {
                    texts::tui_marker_active()
                } else {
                    texts::tui_marker_inactive()
                };
                ListItem::new(Line::from(Span::raw(format!(
                    "{marker}  {}",
                    texts::tui_skills_sync_method_name(method)
                ))))
            });

            let list = List::new(items)
                .highlight_style(selection_style(theme))
                .highlight_symbol(highlight_symbol(theme));

            let mut state = ListState::default();
            state.select(Some(*selected));
            frame.render_stateful_widget(list, chunks[1], &mut state);
        }
        Overlay::Loading {
            kind,
            title,
            message,
        } => {
            let area = centered_rect_fixed(60, 9, content_area);
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
                .border_style(Style::default().fg(theme.dim))
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
        }
        Overlay::SpeedtestRunning { url } => {
            let area = centered_rect_fixed(70, 7, content_area);
            frame.render_widget(Clear, area);
            let outer = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(theme.dim))
                .title(texts::tui_speedtest_title());
            frame.render_widget(outer.clone(), area);
            let inner = outer.inner(area);

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(0)])
                .split(inner);

            render_key_bar_center(frame, chunks[0], theme, &[("Esc", texts::tui_key_close())]);
            frame.render_widget(
                Paragraph::new(Line::raw(texts::tui_speedtest_running(url)))
                    .wrap(Wrap { trim: false }),
                chunks[1],
            );
        }
        Overlay::SpeedtestResult { url, lines, scroll } => {
            let area = centered_rect(90, 90, content_area);
            frame.render_widget(Clear, area);

            let title = texts::tui_speedtest_title_with_url(url);
            let outer = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(theme.dim))
                .title(title);
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

            let height = chunks[1].height as usize;
            let start = (*scroll).min(lines.len());
            let end = (start + height).min(lines.len());
            let shown = lines[start..end]
                .iter()
                .map(|s| Line::raw(s.clone()))
                .collect::<Vec<_>>();

            frame.render_widget(Paragraph::new(shown).wrap(Wrap { trim: false }), chunks[1]);
        }
        Overlay::UpdateAvailable {
            current,
            latest,
            selected,
        } => {
            let area = centered_rect_fixed(50, 8, content_area);
            frame.render_widget(Clear, area);

            let outer = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(theme.accent))
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
            let update_style = if *selected == 0 {
                Style::default()
                    .fg(theme.ok)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default().fg(theme.dim)
            };
            let cancel_style = if *selected == 1 {
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
        Overlay::UpdateDownloading { downloaded, total } => {
            let area = centered_rect_fixed(50, 6, content_area);
            frame.render_widget(Clear, area);

            let outer = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(theme.accent))
                .title(texts::tui_update_downloading_title());
            frame.render_widget(outer.clone(), area);
            let inner = outer.inner(area);

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(0)])
                .split(inner);

            render_key_bar_center(frame, chunks[0], theme, &[("Esc", texts::tui_key_hide())]);

            let progress_text = if let Some(t) = total {
                if *t > 0 {
                    let pct = ((downloaded.saturating_mul(100) / *t).min(100)) as u64;
                    texts::tui_update_downloading_progress(pct, downloaded / 1024, t / 1024)
                } else {
                    texts::tui_update_downloading_kb(*downloaded / 1024)
                }
            } else {
                texts::tui_update_downloading_kb(*downloaded / 1024)
            };

            let gauge_ratio = if let Some(t) = total {
                if *t > 0 {
                    (*downloaded as f64 / *t as f64).min(1.0)
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
                chunks[1],
            );
        }
        Overlay::UpdateResult { success, message } => {
            let area = centered_rect_fixed(50, 6, content_area);
            frame.render_widget(Clear, area);

            let border_color = if *success { theme.ok } else { theme.err };
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

            let keys = if *success {
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

            frame.render_widget(
                Paragraph::new(centered_message_lines(
                    message,
                    chunks[1].width,
                    chunks[1].height,
                ))
                .alignment(Alignment::Center),
                chunks[1],
            );
        }
    }
}

fn content_pane_rect(area: Rect, theme: &super::theme::Theme) -> Rect {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(nav_pane_width(theme)),
            Constraint::Min(0),
        ])
        .split(root[1]);

    body[1]
}

fn centered_message_lines(message: &str, width: u16, height: u16) -> Vec<Line<'static>> {
    let lines = wrap_message_lines(message, width);
    let pad = height.saturating_sub(lines.len() as u16) / 2;
    let mut out = Vec::with_capacity(pad as usize + lines.len());
    for _ in 0..pad {
        out.push(Line::raw(""));
    }
    out.extend(lines.into_iter().map(Line::raw));
    out
}

fn wrap_message_lines(message: &str, width: u16) -> Vec<String> {
    let width = width as usize;
    if width == 0 {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for ch in message.chars() {
        if ch == '\n' {
            lines.push(current);
            current = String::new();
            current_width = 0;
            continue;
        }

        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
        if current_width + ch_width > width && !current.is_empty() {
            lines.push(current);
            current = String::new();
            current_width = 0;
        }

        current.push(ch);
        current_width = current_width.saturating_add(ch_width);
    }

    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }

    lines
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn centered_rect_fixed(width: u16, height: u16, r: Rect) -> Rect {
    let width = width.min(r.width);
    let height = height.min(r.height);

    Rect {
        x: r.x + r.width.saturating_sub(width) / 2,
        y: r.y + r.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn mask_api_key(key: &str) -> String {
    let mut iter = key.chars();
    let prefix: String = iter.by_ref().take(8).collect();
    if iter.next().is_some() {
        format!("{prefix}...")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};
    use serde_json::json;
    use std::sync::Mutex;

    use crate::{
        app_config::AppType,
        cli::i18n::texts,
        cli::tui::{
            app::{
                App, ConfirmAction, ConfirmOverlay, EditorKind, EditorSubmit, Focus, Overlay,
                TextInputState, TextSubmit,
            },
            data::{
                ConfigSnapshot, McpSnapshot, PromptsSnapshot, ProviderRow, ProvidersSnapshot,
                SkillsSnapshot, UiData,
            },
            route::Route,
            theme::theme_for,
        },
        provider::Provider,
        services::skill::{InstalledSkill, SkillApps, SkillRepo, SyncMethod},
    };

    #[test]
    fn mask_api_key_handles_multibyte_safely() {
        let short = "你你你"; // 3 chars, 9 bytes
        let masked = super::mask_api_key(short);
        assert_eq!(masked, short);

        let long = "你".repeat(9);
        let masked = super::mask_api_key(&long);
        assert!(masked.ends_with("..."));
    }

    #[test]
    fn provider_form_shows_full_api_key_in_table_value() {
        let mut form = crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude);
        form.claude_api_key.set("sk-test-1234567890");

        let (_label, value) = super::provider_field_label_and_value(
            &form,
            crate::cli::tui::form::ProviderAddField::ClaudeApiKey,
        );
        assert_eq!(value, "sk-test-1234567890");
    }

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        match ENV_LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn remove(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                None => std::env::remove_var(self.key),
                Some(v) => std::env::set_var(self.key, v),
            }
        }
    }

    fn render(app: &App, data: &UiData) -> Buffer {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("terminal created");
        terminal
            .draw(|f| super::render(f, app, data))
            .expect("draw ok");
        terminal.backend().buffer().clone()
    }

    fn line_at(buf: &Buffer, y: u16) -> String {
        let mut out = String::new();
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out
    }

    fn all_text(buf: &Buffer) -> String {
        let mut all = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                all.push_str(buf[(x, y)].symbol());
            }
            all.push('\n');
        }
        all
    }

    fn minimal_data(_app_type: &AppType) -> UiData {
        let provider = Provider::with_id(
            "p1".to_string(),
            "Demo Provider".to_string(),
            json!({}),
            None,
        );
        UiData {
            providers: ProvidersSnapshot {
                current_id: "p0".to_string(),
                rows: vec![ProviderRow {
                    id: "p1".to_string(),
                    provider,
                    api_url: Some("https://example.com".to_string()),
                    is_current: false,
                }],
            },
            mcp: McpSnapshot::default(),
            prompts: PromptsSnapshot::default(),
            config: ConfigSnapshot::default(),
            skills: SkillsSnapshot::default(),
        }
    }

    fn installed_skill(directory: &str, name: &str) -> InstalledSkill {
        InstalledSkill {
            id: format!("local:{directory}"),
            name: name.to_string(),
            description: Some("Demo".to_string()),
            directory: directory.to_string(),
            readme_url: None,
            repo_owner: None,
            repo_name: None,
            repo_branch: None,
            apps: SkillApps {
                claude: true,
                codex: false,
                gemini: false,
                opencode: false,
            },
            installed_at: 1,
        }
    }

    #[test]
    fn add_form_template_chips_are_single_row() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.form = Some(crate::cli::tui::form::FormState::ProviderAdd(
            crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude),
        ));

        let data = minimal_data(&app.app_type);
        let buf = render(&app, &data);

        let mut chips_y = None;
        for y in 0..buf.area.height {
            let line = line_at(&buf, y);
            if line.contains("Custom") && line.contains("Claude Official") {
                chips_y = Some(y);
                break;
            }
        }

        let chips_y = chips_y.expect("template chips row missing from add form");
        let next = line_at(&buf, chips_y + 1);
        assert!(
            next.contains('└'),
            "expected template block border after chips, got: {next}"
        );
    }

    #[test]
    fn provider_form_fields_show_dashed_divider_before_common_snippet() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        app.form = Some(crate::cli::tui::form::FormState::ProviderAdd(
            crate::cli::tui::form::ProviderAddFormState::new(AppType::Claude),
        ));

        let data = minimal_data(&app.app_type);
        let buf = render(&app, &data);

        // The label is clipped to the first column width; search for a stable substring.
        let common_label = "Snipp";
        let mut common_y = None;
        for y in 0..buf.area.height {
            let line = line_at(&buf, y);
            if line.contains(common_label) {
                common_y = Some(y);
                break;
            }
        }

        let common_y = common_y.expect("Common Config Snippet row missing from provider form");
        let above = line_at(&buf, common_y.saturating_sub(1));
        assert!(
            above.contains("┄┄┄"),
            "expected dashed divider row above common snippet, got: {above}"
        );
    }

    #[test]
    fn header_is_wrapped_in_a_rect_block() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        let data = minimal_data(&app.app_type);

        let buf = render(&app, &data);

        // Header is at y=0..=2, and should have an outer border at (0,0).
        assert_eq!(buf[(0, 0)].symbol(), "┌");
    }

    #[test]
    fn nav_icons_have_left_padding_from_border() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");

        let app = App::new(Some(AppType::Claude));
        let data = minimal_data(&app.app_type);
        let buf = render(&app, &data);

        let mut home_line = None;
        for y in 0..buf.area.height {
            let line = line_at(&buf, y);
            if line.contains("Home") && line.contains("🏠") {
                home_line = Some(line);
                break;
            }
        }

        let home_line = home_line.expect("Home row missing from nav");
        let emoji_idx = home_line
            .find("🏠")
            .expect("Home emoji missing from nav row");
        let emoji_char_idx = home_line[..emoji_idx].chars().count();
        let chars: Vec<char> = home_line.chars().collect();
        assert!(
            emoji_char_idx >= 2,
            "expected at least 2 chars before emoji, got line: {home_line}"
        );
        assert_eq!(
            chars[emoji_char_idx.saturating_sub(2)],
            '│',
            "expected nav border immediately before padding space, got line: {home_line}"
        );
        assert_eq!(
            chars[emoji_char_idx.saturating_sub(1)],
            ' ',
            "expected a 1-cell padding between nav border and emoji, got line: {home_line}"
        );
    }

    #[test]
    fn providers_pane_has_border_and_selected_row_is_accent() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;
        let data = minimal_data(&app.app_type);

        let buf = render(&app, &data);
        let theme = theme_for(&app.app_type);

        let content = super::content_pane_rect(buf.area, &theme);
        let border_cell = &buf[(content.x, content.y)];
        assert_eq!(border_cell.symbol(), "┌");
        assert_eq!(border_cell.fg, theme.accent);

        // Selected row should be highlighted with theme accent background.
        // Layout:
        // - content pane border (1)
        // - hint row (1)
        // - table header row (1)
        // - first data row (selected) (1)
        let selected_row_cell = &buf[(
            content.x.saturating_add(2 + super::CONTENT_INSET_LEFT),
            content.y.saturating_add(1 + 1 + 1),
        )];
        assert_eq!(selected_row_cell.bg, theme.accent);
    }

    #[test]
    fn editor_cursor_matches_rendered_target_line() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Config;
        app.focus = Focus::Content;

        let long = "x".repeat(400);
        let marker = "<<<TARGET>>>";
        let initial = format!("{long}\n{marker}");

        app.open_editor(
            "Demo Editor",
            EditorKind::Json,
            initial,
            EditorSubmit::ConfigCommonSnippet {
                app_type: app.app_type.clone(),
            },
        );

        let editor = app.editor.as_mut().expect("editor opened");
        editor.cursor_row = 1;
        editor.cursor_col = 0;
        editor.scroll = 0;

        let data = minimal_data(&app.app_type);
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("terminal created");
        terminal
            .draw(|f| super::render(f, &app, &data))
            .expect("draw ok");

        let cursor = terminal.get_cursor_position().expect("cursor position");
        let buf = terminal.backend().buffer().clone();

        let wrap_token = "x".repeat(20);
        let wrapped_rows = (0..buf.area.height)
            .filter(|y| line_at(&buf, *y).contains(&wrap_token))
            .count();
        assert!(
            wrapped_rows >= 2,
            "expected long line to wrap onto multiple rows, got {wrapped_rows}"
        );

        let mut marker_y = None;
        for y in 0..buf.area.height {
            let line = line_at(&buf, y);
            if line.contains(marker) {
                marker_y = Some(y);
                break;
            }
        }

        let marker_y = marker_y.expect("marker line rendered");
        assert_eq!(
            cursor.y, marker_y,
            "cursor should be on the same row as the rendered marker line"
        );
    }

    #[test]
    fn home_renders_ascii_logo() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Main;
        app.focus = Focus::Content;
        let data = minimal_data(&app.app_type);

        let buf = render(&app, &data);
        let all = all_text(&buf);
        assert!(all.contains("___  ___"));
        assert!(all.contains("\\___|\\___|"));
    }

    #[test]
    fn home_does_not_repeat_welcome_title_in_body() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Main;
        app.focus = Focus::Content;
        let data = minimal_data(&app.app_type);

        let buf = render(&app, &data);
        let all = all_text(&buf);

        let needle = "CC-Switch Interactive Mode";
        let count = all.matches(needle).count();
        assert_eq!(count, 1, "expected welcome title once, got {count}");
    }

    #[test]
    fn home_shows_local_env_check_section() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Main;
        app.focus = Focus::Content;
        let data = minimal_data(&app.app_type);

        let buf = render(&app, &data);
        let all = all_text(&buf);

        assert!(all.contains("Local environment check"));
        assert!(!all.contains("Session Context"));
    }

    #[test]
    fn home_shows_webdav_section() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Main;
        app.focus = Focus::Content;
        let data = minimal_data(&app.app_type);

        let buf = render(&app, &data);
        let all = all_text(&buf);

        assert!(all.contains("WebDAV Sync"));
    }

    #[test]
    fn home_webdav_not_configured_does_not_show_error() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Main;
        app.focus = Focus::Content;

        let mut data = minimal_data(&app.app_type);
        data.config.webdav_sync = Some(crate::settings::WebDavSyncSettings {
            enabled: true,
            ..Default::default()
        });

        let buf = render(&app, &data);
        let all = all_text(&buf);

        assert!(all.contains("Not configured"));
        assert!(!all.contains("Last error"));
        assert!(!all.contains("Enabled"));
    }

    #[test]
    fn home_webdav_failure_shows_error_details() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Main;
        app.focus = Focus::Content;

        let mut data = minimal_data(&app.app_type);
        let mut webdav = crate::settings::WebDavSyncSettings {
            enabled: true,
            ..Default::default()
        };
        webdav.base_url = "https://dav.example".to_string();
        webdav.username = "demo".to_string();
        webdav.password = "app-pass".to_string();
        webdav.status.last_error = Some("auth failed".to_string());
        data.config.webdav_sync = Some(webdav);

        let buf = render(&app, &data);
        let all = all_text(&buf);

        assert!(all.contains("Error (auth failed)"));
        assert!(!all.contains("Last error"));
        assert!(!all.contains("Enabled"));
    }

    #[test]
    fn webdav_sync_time_formats_to_minute() {
        let formatted = super::format_sync_time_local_to_minute(1_735_689_600)
            .expect("timestamp should be formatable");
        assert_eq!(formatted.len(), 16);
        assert_eq!(&formatted[4..5], "/");
        assert_eq!(&formatted[7..8], "/");
        assert_eq!(&formatted[10..11], " ");
        assert_eq!(&formatted[13..14], ":");
    }

    #[test]
    fn nav_does_not_show_manage_prefix_or_view_config() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Main;
        app.focus = Focus::Nav;
        let data = minimal_data(&app.app_type);

        let buf = render(&app, &data);
        let all = all_text(&buf);

        assert!(
            !all.contains("Manage "),
            "expected nav to not include Manage prefix"
        );
        assert!(
            !all.contains("View Current Configuration"),
            "expected nav to not include View Current Configuration"
        );
    }

    #[test]
    fn skills_page_renders_sync_method_and_installed_rows() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Skills;
        app.focus = Focus::Content;

        let mut data = minimal_data(&app.app_type);
        data.skills.sync_method = SyncMethod::Copy;
        data.skills.installed = vec![installed_skill("hello-skill", "Hello Skill")];

        let buf = render(&app, &data);
        let all = all_text(&buf);

        assert!(all.contains(&texts::tui_skills_installed_counts(1, 0, 0)));
        assert!(all.contains("hello-skill"));
        assert!(all.contains("Hello Skill"));
    }

    #[test]
    fn skills_discover_page_shows_hint_when_empty() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SkillsDiscover;
        app.focus = Focus::Content;
        app.skills_discover_results = vec![];
        app.skills_discover_query = String::new();

        let data = minimal_data(&app.app_type);
        let buf = render(&app, &data);
        let all = all_text(&buf);

        assert!(all.contains(texts::tui_skills_discover_hint()));
    }

    #[test]
    fn skills_repos_page_renders_repo_rows() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::SkillsRepos;
        app.focus = Focus::Content;

        let mut data = minimal_data(&app.app_type);
        data.skills.repos = vec![SkillRepo {
            owner: "anthropics".to_string(),
            name: "skills".to_string(),
            branch: "main".to_string(),
            enabled: true,
        }];

        let buf = render(&app, &data);
        let all = all_text(&buf);

        assert!(all.contains("anthropics/skills"));
    }

    #[test]
    fn text_input_overlay_renders_inner_input_box() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Config;
        app.focus = Focus::Content;
        app.overlay = Overlay::TextInput(TextInputState {
            title: "Demo".to_string(),
            prompt: "Enter value".to_string(),
            buffer: "hello".to_string(),
            submit: TextSubmit::ConfigBackupName,
            secret: false,
        });
        let data = minimal_data(&app.app_type);

        let buf = render(&app, &data);

        let theme = theme_for(&app.app_type);
        let content = super::content_pane_rect(buf.area, &theme);
        let area = super::centered_rect_fixed(70, 12, content);
        let area_x = area.x;
        let area_y = area.y;
        let area_w = area.width;
        let area_h = area.height;

        // Outer border exists at (18,13). We also expect an inner input field border (another ┌)
        // somewhere inside the overlay.
        let mut inner_top_left_count = 0usize;
        for y in area_y..area_y.saturating_add(area_h) {
            for x in area_x..area_x.saturating_add(area_w) {
                if x == area_x && y == area_y {
                    continue;
                }
                if buf[(x, y)].symbol() == "┌" {
                    inner_top_left_count += 1;
                }
            }
        }

        assert!(
            inner_top_left_count >= 1,
            "expected an inner input box border in TextInput overlay"
        );
    }

    #[test]
    fn editor_unsaved_changes_confirm_overlay_shows_three_actions_and_is_compact() {
        let _lock = lock_env();

        let prev = std::env::var("NO_COLOR").ok();
        std::env::set_var("NO_COLOR", "1");
        let _restore_no_color = EnvGuard {
            key: "NO_COLOR",
            prev,
        };

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;
        app.overlay = Overlay::Confirm(ConfirmOverlay {
            title: texts::tui_editor_save_before_close_title().to_string(),
            message: texts::tui_editor_save_before_close_message().to_string(),
            action: ConfirmAction::EditorSaveBeforeClose,
        });
        let data = minimal_data(&app.app_type);

        let buf = render(&app, &data);
        let all = all_text(&buf);

        assert!(
            all.contains("Enter=save & exit"),
            "expected save action hint in confirm overlay key bar"
        );
        assert!(
            all.contains("N=exit w/o save"),
            "expected discard action hint in confirm overlay key bar"
        );
        assert!(
            all.contains("Esc=cancel"),
            "expected cancel action hint in confirm overlay key bar"
        );

        let theme = theme_for(&app.app_type);
        let content = super::content_pane_rect(buf.area, &theme);
        let area = super::centered_rect_fixed(60, 7, content);

        assert_eq!(buf[(area.x, area.y)].symbol(), "┌");
        assert_eq!(
            buf[(
                area.x.saturating_add(area.width.saturating_sub(1)),
                area.y.saturating_add(area.height.saturating_sub(1))
            )]
                .symbol(),
            "┘"
        );
    }

    #[test]
    fn footer_shows_only_global_actions() {
        let _lock = lock_env();

        let prev = std::env::var("NO_COLOR").ok();
        std::env::set_var("NO_COLOR", "1");
        let _restore_no_color = EnvGuard {
            key: "NO_COLOR",
            prev,
        };

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Config;
        app.focus = Focus::Content;
        app.overlay = Overlay::CommonSnippetView {
            app_type: AppType::Claude,
            view: crate::cli::tui::app::TextViewState {
                title: "Common Snippet".to_string(),
                lines: vec!["{}".to_string()],
                scroll: 0,
            },
        };
        let data = minimal_data(&app.app_type);

        let buf = render(&app, &data);
        let footer = line_at(&buf, buf.area.height - 1);

        assert!(
            footer.contains("switch app") && footer.contains("/ filter"),
            "expected footer to show global actions; got: {footer:?}"
        );
        assert!(
            !footer.contains("clear") && !footer.contains("apply"),
            "expected footer to not show overlay/page actions; got: {footer:?}"
        );
    }

    #[test]
    fn backup_picker_overlay_shows_hint() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Config;
        app.focus = Focus::Content;
        app.overlay = Overlay::BackupPicker { selected: 0 };

        let mut data = minimal_data(&app.app_type);
        data.config.backups = vec![crate::services::config::BackupInfo {
            id: "b1".to_string(),
            path: std::path::PathBuf::from("/tmp/b1.json"),
            timestamp: "20260131_000000".to_string(),
            display_name: "backup".to_string(),
        }];

        let buf = render(&app, &data);

        let mut all = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                all.push_str(buf[(x, y)].symbol());
            }
            all.push('\n');
        }

        assert!(
            all.contains("Enter")
                && all.contains("Esc")
                && (all.contains("restore") || all.contains("恢复")),
            "expected BackupPicker to show Enter/Esc restore hint"
        );
    }

    #[test]
    fn provider_detail_keys_line_does_not_include_q_back() {
        let _lock = lock_env();
        let _no_color = EnvGuard::remove("NO_COLOR");

        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ProviderDetail {
            id: "p1".to_string(),
        };
        app.focus = Focus::Content;
        let data = minimal_data(&app.app_type);

        let buf = render(&app, &data);
        let mut all = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                all.push_str(buf[(x, y)].symbol());
            }
            all.push('\n');
        }

        assert!(all.contains("speedtest"));
        assert!(
            !all.contains("q=back"),
            "provider detail inline keys should not include q=back"
        );
    }
}
