use std::sync::RwLock;

use inquire::Confirm;

use crate::app_config::MultiAppConfig;
use crate::cli::i18n::texts;
use crate::error::AppError;
use crate::store::AppState;

pub fn get_state() -> Result<AppState, AppError> {
    let config = MultiAppConfig::load()?;
    Ok(AppState {
        config: RwLock::new(config),
    })
}

pub fn pause() {
    let _ = Confirm::new(texts::press_enter())
        .with_default(true)
        .with_help_message("")
        .prompt();
}
