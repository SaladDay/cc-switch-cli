use std::io::{self, IsTerminal, Write};
use std::sync::RwLock;

use inquire::error::InquireError;
use inquire::{Confirm, MultiSelect, Select, Text};

use crate::app_config::AppType;
use crate::app_config::MultiAppConfig;
use crate::cli::i18n::texts;
use crate::error::AppError;
use crate::store::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppSwitchDirection {
    Previous,
    Next,
}

pub fn cycle_app_type(current: &AppType, direction: AppSwitchDirection) -> AppType {
    match (current, direction) {
        (AppType::Claude, AppSwitchDirection::Next) => AppType::Codex,
        (AppType::Codex, AppSwitchDirection::Next) => AppType::Gemini,
        (AppType::Gemini, AppSwitchDirection::Next) => AppType::Claude,
        (AppType::Claude, AppSwitchDirection::Previous) => AppType::Gemini,
        (AppType::Codex, AppSwitchDirection::Previous) => AppType::Claude,
        (AppType::Gemini, AppSwitchDirection::Previous) => AppType::Codex,
    }
}

pub fn app_switch_direction_from_key(key: &console::Key) -> Option<AppSwitchDirection> {
    match key {
        console::Key::ArrowLeft => Some(AppSwitchDirection::Previous),
        console::Key::ArrowRight => Some(AppSwitchDirection::Next),
        _ => None,
    }
}

pub fn get_state() -> Result<AppState, AppError> {
    let config = MultiAppConfig::load()?;
    Ok(AppState {
        config: RwLock::new(config),
    })
}

pub fn clear_screen() {
    if !io::stdout().is_terminal() {
        return;
    }

    let term = console::Term::stdout();
    let _ = term.clear_screen();
    let _ = io::stdout().flush();
}

pub fn handle_inquire<T>(result: Result<T, InquireError>) -> Result<Option<T>, AppError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(None),
        Err(err) => Err(AppError::Message(err.to_string())),
    }
}

pub fn prompt_select<T>(message: &str, options: Vec<T>) -> Result<Option<T>, AppError>
where
    T: Clone + std::fmt::Display,
{
    handle_inquire(
        Select::new(message, options)
            .with_help_message(texts::select_filter_help())
            .prompt(),
    )
}

pub fn prompt_multiselect<T>(message: &str, options: Vec<T>) -> Result<Option<Vec<T>>, AppError>
where
    T: Clone + std::fmt::Display,
{
    handle_inquire(
        MultiSelect::new(message, options)
            .with_help_message(texts::select_filter_help())
            .prompt(),
    )
}

pub fn prompt_multiselect_with_help<T>(
    message: &str,
    options: Vec<T>,
    help_message: &str,
) -> Result<Option<Vec<T>>, AppError>
where
    T: Clone + std::fmt::Display,
{
    handle_inquire(
        MultiSelect::new(message, options)
            .with_help_message(help_message)
            .prompt(),
    )
}

pub fn prompt_confirm(message: &str, default: bool) -> Result<Option<bool>, AppError> {
    handle_inquire(
        Confirm::new(message)
            .with_default(default)
            .with_help_message(texts::esc_to_go_back_help())
            .prompt(),
    )
}

pub fn prompt_text(message: &str) -> Result<Option<String>, AppError> {
    handle_inquire(
        Text::new(message)
            .with_help_message(texts::esc_to_go_back_help())
            .prompt(),
    )
}

pub fn prompt_text_with_default(message: &str, default: &str) -> Result<Option<String>, AppError> {
    handle_inquire(
        Text::new(message)
            .with_default(default)
            .with_help_message(texts::esc_to_go_back_help())
            .prompt(),
    )
}

pub fn pause() {
    print!("{} ", texts::press_enter());
    let _ = io::stdout().flush();
    let mut input = String::new();
    let _ = io::stdin().read_line(&mut input);
}

#[cfg(test)]
mod tests {
    use super::*;
    use console::Key;

    #[test]
    fn cycle_app_type_next_wraps() {
        assert_eq!(
            cycle_app_type(&AppType::Claude, AppSwitchDirection::Next),
            AppType::Codex
        );
        assert_eq!(
            cycle_app_type(&AppType::Codex, AppSwitchDirection::Next),
            AppType::Gemini
        );
        assert_eq!(
            cycle_app_type(&AppType::Gemini, AppSwitchDirection::Next),
            AppType::Claude
        );
    }

    #[test]
    fn cycle_app_type_previous_wraps() {
        assert_eq!(
            cycle_app_type(&AppType::Claude, AppSwitchDirection::Previous),
            AppType::Gemini
        );
        assert_eq!(
            cycle_app_type(&AppType::Codex, AppSwitchDirection::Previous),
            AppType::Claude
        );
        assert_eq!(
            cycle_app_type(&AppType::Gemini, AppSwitchDirection::Previous),
            AppType::Codex
        );
    }

    #[test]
    fn app_switch_direction_from_key_maps_arrows() {
        assert_eq!(
            app_switch_direction_from_key(&Key::ArrowLeft),
            Some(AppSwitchDirection::Previous)
        );
        assert_eq!(
            app_switch_direction_from_key(&Key::ArrowRight),
            Some(AppSwitchDirection::Next)
        );
        assert_eq!(app_switch_direction_from_key(&Key::Enter), None);
    }
}
