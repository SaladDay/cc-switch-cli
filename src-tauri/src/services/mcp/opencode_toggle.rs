//! OpenCode's JSON container policy, using the shared MCP file executor.

use cc_switch_core::{McpConfigTarget, McpNativeSnapshot};
use serde_json::{json, Value};

use super::{
    native_file::NativeFile,
    toggle::{NativeChange, NativeToggle},
};
use crate::{
    error::AppError, mcp::convert_to_opencode_mcp_spec, opencode_config::get_opencode_config_path,
};

pub(super) struct OpenCodeToggle(NativeFile);

impl OpenCodeToggle {
    pub(super) fn observe() -> Result<Self, AppError> {
        NativeFile::observe(McpConfigTarget::OpenCode, &get_opencode_config_path()).map(Self)
    }
}

impl NativeToggle for OpenCodeToggle {
    fn apply_batch(
        &mut self,
        changes: &[NativeChange<'_>],
    ) -> Result<Vec<Option<McpNativeSnapshot>>, AppError> {
        if changes.is_empty() {
            return Ok(Vec::new());
        }
        if changes
            .iter()
            .any(|change| change.previous_snapshot.is_some())
        {
            return Err(AppError::Database(
                "Unsupported OpenCode MCP native snapshot".into(),
            ));
        }
        let entries = changes
            .iter()
            .map(|change| {
                change
                    .enabled()
                    .then(|| convert_to_opencode_mcp_spec(change.server))
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()?;
        // Keep strict JSON parsing and the missing-file schema on both enable
        // and disable. Core's bounded document parser is not this host's policy.
        let mut config: Value = match self.0.original() {
            None => json!({"$schema": "https://opencode.ai/config.json"}),
            Some(bytes) => {
                let text = std::str::from_utf8(bytes).map_err(|error| {
                    AppError::io(
                        self.0.path(),
                        std::io::Error::new(std::io::ErrorKind::InvalidData, error),
                    )
                })?;
                serde_json::from_str(text).map_err(|error| AppError::json(self.0.path(), error))?
            }
        };
        for (change, entry) in changes.iter().zip(entries) {
            let id = change.id;
            if let Some(entry) = entry {
                // The old writer promoted a null root. Other non-object roots or
                // collections cannot accept an entry; fail without publishing.
                if config.is_null() {
                    config = json!({});
                }
                let root = config.as_object_mut().ok_or_else(|| {
                    AppError::McpValidation("OpenCode config must be a JSON object".into())
                })?;
                let mcp = root
                    .entry("mcp")
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .ok_or_else(|| {
                        AppError::McpValidation("OpenCode mcp must be a JSON object".into())
                    })?;
                mcp.insert(id.to_owned(), entry);
            } else if let Some(mcp) = config.get_mut("mcp").and_then(Value::as_object_mut) {
                // Removing one entry must not reorder its siblings.
                mcp.shift_remove(id);
            }
        }
        let contents = serde_json::to_string_pretty(&config)
            .map_err(|source| AppError::JsonSerialize { source })?;
        self.0.publish(&contents)?;
        Ok(vec![None; changes.len()])
    }

    fn rollback(&mut self) -> Result<(), AppError> {
        self.0.rollback()
    }
}

#[cfg(test)]
mod tests;
