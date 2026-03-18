use colored::Color;
use colored::Colorize;
use std::sync::{OnceLock, RwLock};

use crate::app_config::AppType;

use inquire::set_global_render_config;
use inquire::ui::{Color as InquireColor, RenderConfig, StyleSheet, Styled};

static TUI_THEME_APP: OnceLock<RwLock<Option<AppType>>> = OnceLock::new();

fn tui_theme_app_cell() -> &'static RwLock<Option<AppType>> {
    TUI_THEME_APP.get_or_init(|| RwLock::new(None))
}

pub fn set_tui_theme_app(app_type: Option<AppType>) {
    *tui_theme_app_cell()
        .write()
        .expect("tui theme app lock poisoned") = app_type;

    apply_inquire_theme();
}

fn get_tui_theme_app() -> Option<AppType> {
    tui_theme_app_cell()
        .read()
        .expect("tui theme app lock poisoned")
        .clone()
}

fn inquire_color_for_app(app_type: &AppType) -> InquireColor {
    match app_type {
        AppType::Codex => InquireColor::LightGreen,
        AppType::Claude => InquireColor::LightCyan,
        AppType::Gemini => InquireColor::LightMagenta,
        AppType::OpenCode => InquireColor::LightGreen,
        AppType::OpenClaw => InquireColor::LightGreen,
    }
}

fn apply_inquire_theme() {
    if std::env::var("NO_COLOR").is_ok() {
        set_global_render_config(RenderConfig::empty());
        return;
    }

    let Some(app_type) = get_tui_theme_app() else {
        set_global_render_config(RenderConfig::default());
        return;
    };

    let accent = inquire_color_for_app(&app_type);

    let cfg = RenderConfig::default_colored()
        .with_prompt_prefix(Styled::new("?").with_fg(accent))
        .with_answered_prompt_prefix(Styled::new(">").with_fg(accent))
        .with_highlighted_option_prefix(Styled::new(">").with_fg(accent))
        .with_selected_option(Some(StyleSheet::new().with_fg(accent)))
        .with_selected_checkbox(Styled::new("[x]").with_fg(accent))
        .with_help_message(StyleSheet::new().with_fg(accent))
        .with_answer(StyleSheet::new().with_fg(accent));

    set_global_render_config(cfg);
}

pub fn success(text: &str) -> String {
    text.green().to_string()
}

pub fn error(text: &str) -> String {
    text.red().to_string()
}

pub fn warning(text: &str) -> String {
    text.yellow().to_string()
}

pub fn info(text: &str) -> String {
    text.cyan().to_string()
}

fn highlight_color_for_app(app_type: &AppType) -> Color {
    match app_type {
        AppType::Codex => Color::BrightGreen,
        AppType::Claude => Color::BrightCyan,
        AppType::Gemini => Color::BrightMagenta,
        AppType::OpenCode => Color::BrightGreen,
        AppType::OpenClaw => Color::BrightGreen,
    }
}

pub fn highlight(text: &str) -> String {
    let Some(app_type) = get_tui_theme_app() else {
        return text.bright_blue().bold().to_string();
    };

    text.color(highlight_color_for_app(&app_type))
        .bold()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    struct ColorOverrideGuard;

    impl ColorOverrideGuard {
        fn force_on() -> Self {
            colored::control::set_override(true);
            Self
        }
    }

    impl Drop for ColorOverrideGuard {
        fn drop(&mut self) {
            colored::control::unset_override();
            set_tui_theme_app(None);
        }
    }

    #[test]
    #[serial]
    fn highlight_uses_app_theme_in_tui() {
        let _guard = ColorOverrideGuard::force_on();

        set_tui_theme_app(Some(AppType::Codex));
        assert_eq!(
            highlight("x"),
            "x".color(Color::BrightGreen).bold().to_string()
        );

        set_tui_theme_app(Some(AppType::Claude));
        assert_eq!(
            highlight("x"),
            "x".color(Color::BrightCyan).bold().to_string()
        );

        set_tui_theme_app(Some(AppType::Gemini));
        assert_eq!(
            highlight("x"),
            "x".color(Color::BrightMagenta).bold().to_string()
        );
    }
}
