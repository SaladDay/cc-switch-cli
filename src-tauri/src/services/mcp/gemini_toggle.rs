//! Gemini's single-file MCP binding retains its existing JSON and snapshot policy.

use std::{path::PathBuf, sync::MutexGuard};

use cc_switch_core::{McpConfigTarget, McpNativeSnapshot};

use super::{native_file::NativeFile, toggle::NativeToggle, *};
use crate::gemini_config::{
    get_gemini_settings_path,
    operation::{lock_live_write, resolve_entry},
};

#[cfg(test)]
use crate::database::Database;
#[cfg(test)]
mod tests;

pub(super) struct GeminiToggle {
    file: NativeFile,
    path: PathBuf,
    _guard: MutexGuard<'static, ()>,
}

pub(super) fn observe() -> Result<GeminiToggle, AppError> {
    let guard = lock_live_write()?;
    let path = get_gemini_settings_path();
    Ok(GeminiToggle {
        file: NativeFile::observe(McpConfigTarget::Gemini, &path)?,
        path,
        _guard: guard,
    })
}

impl NativeToggle for GeminiToggle {
    fn apply(
        &mut self,
        id: &str,
        server: &serde_json::Value,
        enabled: bool,
        previous_snapshot: Option<&McpNativeSnapshot>,
    ) -> Result<Option<McpNativeSnapshot>, AppError> {
        let original = self
            .file
            .original()
            .map(std::str::from_utf8)
            .transpose()
            .map_err(|error| {
                AppError::io(
                    self.file.path(),
                    std::io::Error::new(std::io::ErrorKind::InvalidData, error),
                )
            })?;
        let (contents, snapshot) = crate::gemini_mcp::prepare_toggle(
            self.file.path(),
            original,
            id,
            server,
            enabled,
            previous_snapshot,
        )?;
        // Keep Gemini's logical-path check even though recovery is bound to the
        // original directory. A retargeted parent must not report a live success.
        if resolve_entry(&self.path)? != self.file.path() {
            return Err(AppError::Conflict(
                "Gemini path changed while preparing configuration".into(),
            ));
        }
        self.file.publish(&contents)?;
        Ok(snapshot)
    }

    fn rollback(&mut self) -> Result<(), AppError> {
        self.file.rollback()
    }
}
