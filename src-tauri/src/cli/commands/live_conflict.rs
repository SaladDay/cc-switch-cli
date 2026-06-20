use inquire::Select;

use crate::cli::i18n::texts;
use crate::error::AppError;
use crate::services::provider::live_merge::{self, ConfigConflict};

pub(crate) struct PromptConflictResolver;

impl live_merge::ConflictResolver for PromptConflictResolver {
    fn resolve_conflict(
        &mut self,
        conflict: &ConfigConflict,
    ) -> Result<live_merge::ConflictChoice, AppError> {
        let keep_local = "Keep local value".to_string();
        let use_incoming = "Use cc-switch value".to_string();
        let prompt = format!(
            "Live configuration conflict\nApplication: {}\nTarget: {}\nField: {}\nLocal value: {}\ncc-switch value: {}\nChoose value:",
            conflict.app_type.as_str(),
            conflict.target,
            conflict.path,
            conflict.local,
            conflict.incoming,
        );

        let selected = Select::new(&prompt, vec![keep_local.clone(), use_incoming.clone()])
            .prompt()
            .map_err(|err| match err {
                inquire::error::InquireError::OperationCanceled
                | inquire::error::InquireError::OperationInterrupted => {
                    AppError::Message(texts::selection_cancelled().to_string())
                }
                other => AppError::Message(texts::input_failed_error(&other.to_string())),
            })?;

        if selected == use_incoming {
            Ok(live_merge::ConflictChoice::UseIncoming)
        } else {
            Ok(live_merge::ConflictChoice::KeepLocal)
        }
    }
}
