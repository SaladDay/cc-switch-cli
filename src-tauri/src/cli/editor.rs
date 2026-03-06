use crate::cli::i18n::texts;
use crate::AppError;

pub fn open_external_editor(initial_content: &str) -> Result<String, AppError> {
    edit::edit(initial_content)
        .map_err(|e| AppError::Message(format!("{}: {}", texts::editor_failed(), e)))
}
