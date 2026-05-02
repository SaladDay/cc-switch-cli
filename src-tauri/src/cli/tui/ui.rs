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
use tachyonfx::{fx, pattern::RadialPattern, Interpolation};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app_config::AppType;
use crate::cli::i18n::{self, texts};
use serde_json::Value;

use super::{
    app,
    app::{
        App, ConfigItem, ConfirmAction, Focus, LoadingKind, Overlay, ToastKind, WebDavConfigItem,
    },
    data::{McpRow, ProviderRow, UiData},
    form::{
        CodexPreviewSection, FormFocus, FormState, GeminiAuthType, McpAddField, ProviderAddField,
    },
    route::{NavItem, Route},
    theme,
    theme::theme_for,
};

mod chrome;
mod config;
mod editor;
mod forms;
mod main_page;
mod mcp;
mod overlay;
mod prompts;
mod providers;
mod proxy_wave;
mod shared;
mod skills;

#[cfg(test)]
mod header_tests;

#[cfg(test)]
mod tests;

use chrome::*;
use config::*;
use editor::*;
use forms::*;
use main_page::*;
use mcp::*;
use overlay::*;
use prompts::*;
use providers::*;
use proxy_wave::*;
use shared::*;
use skills::*;

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
    render_footer(frame, app, data, root[2], &theme);

    render_overlay(frame, app, data, &theme);
    render_toast(frame, app, &theme);
}

pub(super) fn proxy_open_flash_effect(area: Rect) -> tachyonfx::Effect {
    let fg_shift = [-330.0, 20.0, 20.0];
    let timer = (500, Interpolation::SineInOut);

    let radial_hsl_xform = fx::hsl_shift_fg(fg_shift, timer)
        .with_pattern(RadialPattern::with_transition((0.5, 0.5), 13.0))
        .with_area(area);

    fx::ping_pong(radial_hsl_xform)
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
        Route::ConfigOpenClawWorkspace | Route::ConfigOpenClawDailyMemory => {
            if matches!(app.app_type, AppType::OpenClaw) {
                render_openclaw_workspace_routes(frame, app, data, content_area, theme)
            } else {
                render_config(frame, app, data, content_area, theme)
            }
        }
        Route::ConfigOpenClawEnv | Route::ConfigOpenClawTools | Route::ConfigOpenClawAgents => {
            if matches!(app.app_type, AppType::OpenClaw) {
                render_config_openclaw_route(frame, app, data, content_area, theme)
            } else {
                render_config(frame, app, data, content_area, theme)
            }
        }
        Route::ConfigWebDav => render_config_webdav(frame, app, data, content_area, theme),
        Route::Skills => render_skills_installed(frame, app, data, content_area, theme),
        Route::SkillsDiscover => render_skills_discover(frame, app, data, content_area, theme),
        Route::SkillsRepos => render_skills_repos(frame, app, data, content_area, theme),
        Route::SkillDetail { directory } => {
            render_skill_detail(frame, app, data, content_area, theme, directory)
        }
        Route::Settings => render_settings(frame, app, data, content_area, theme),
        Route::SettingsProxy => render_settings_proxy(frame, app, data, content_area, theme),
        Route::SettingsUpstreamProxy => render_settings_upstream_proxy(frame, app, data, content_area, theme),
    }
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

#[cfg(test)]
mod effect_tests {
    use super::*;

    #[test]
    fn proxy_open_flash_uses_ping_pong_sine_in_out_once() {
        let effect = proxy_open_flash_effect(Rect::new(0, 0, 80, 24));
        let dsl = effect.to_dsl().unwrap().to_string();

        assert!(dsl.contains("fx::ping_pong("), "{dsl}");
        assert!(dsl.contains("SineInOut"), "{dsl}");
        assert!(!dsl.contains("fx::repeating("), "{dsl}");
    }
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
