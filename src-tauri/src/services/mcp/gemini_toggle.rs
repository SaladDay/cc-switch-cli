//! Gemini retains its settings operation and native snapshot policy.

use cc_switch_core::McpNativeSnapshot;

use super::{toggle::NativeToggle, *};
use crate::gemini_config::{get_gemini_settings_path, operation::GeminiOperation};

#[cfg(test)]
use crate::database::Database;
#[cfg(test)]
mod tests;

impl McpService {
    pub(super) fn toggle_gemini_coordinated(
        state: &AppState,
        id: &str,
        enabled: bool,
    ) -> Result<(), AppError> {
        Self::toggle_coordinated(state, id, AppType::Gemini, enabled, || {
            GeminiOperation::observe_settings(&get_gemini_settings_path())
        })
    }
}

impl NativeToggle for GeminiOperation {
    fn apply(
        &mut self,
        id: &str,
        server: &serde_json::Value,
        enabled: bool,
        previous_snapshot: Option<&McpNativeSnapshot>,
    ) -> Result<Option<McpNativeSnapshot>, AppError> {
        crate::gemini_mcp::toggle_with_operation(self, id, server, enabled, previous_snapshot)
    }

    fn rollback(&mut self) -> Result<(), AppError> {
        GeminiOperation::rollback(self)
    }
}
