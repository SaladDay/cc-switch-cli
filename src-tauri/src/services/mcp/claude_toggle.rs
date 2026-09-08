//! Claude keeps its path and JSON policy; Core owns entry snapshots and execution.

use std::{fs, io::Read};

use cc_switch_core::{McpConfigTarget, McpEntryEncodePolicy, McpNativeSnapshot};
use serde_json::Value;

use super::{native_file::NativeFile, toggle::NativeToggle};
use crate::{
    claude_mcp::project_server,
    config::{get_claude_mcp_path, get_default_claude_mcp_path},
    error::AppError,
};

pub(super) struct ClaudeToggle {
    file: NativeFile,
    migration_seed: Option<Vec<u8>>,
}

impl ClaudeToggle {
    pub(super) fn observe() -> Result<Self, AppError> {
        let mut file = NativeFile::observe(McpConfigTarget::Claude, &get_claude_mcp_path())?;
        let mut migration_seed = None;
        if file.original().is_none() && crate::settings::get_claude_override_dir().is_some() {
            let legacy = get_default_claude_mcp_path();
            match fs::File::open(&legacy) {
                Ok(mut source) => {
                    let permissions = source
                        .metadata()
                        .map_err(|e| AppError::io(&legacy, e))?
                        .permissions();
                    let mut bytes = Vec::new();
                    source
                        .read_to_end(&mut bytes)
                        .map_err(|e| AppError::io(&legacy, e))?;
                    file.set_creation_permissions(permissions)?;
                    migration_seed = Some(bytes);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(AppError::io(&legacy, error)),
            }
        }
        // The legacy file is read-only input. Only the observed destination is
        // published, so a failed toggle restores an absent leaf (or its link).
        Ok(Self {
            file,
            migration_seed,
        })
    }
}

impl NativeToggle for ClaudeToggle {
    fn apply(
        &mut self,
        id: &str,
        server: &Value,
        enabled: bool,
        previous_snapshot: Option<&McpNativeSnapshot>,
    ) -> Result<Option<McpNativeSnapshot>, AppError> {
        let raw = self.file.original().or(self.migration_seed.as_deref());
        let mut root: Value = match raw {
            Some(bytes) => {
                let text = std::str::from_utf8(bytes).map_err(|error| {
                    AppError::io(
                        self.file.path(),
                        std::io::Error::new(std::io::ErrorKind::InvalidData, error),
                    )
                })?;
                serde_json::from_str(text)
                    .map_err(|error| AppError::json(self.file.path(), error))?
            }
            None => serde_json::json!({}),
        };
        let root_object = root
            .as_object_mut()
            .ok_or_else(|| AppError::Config("~/.claude.json 根必须是对象".into()))?;
        let collection = root_object
            .entry("mcpServers")
            .or_insert_with(|| serde_json::json!({}));
        if !collection.is_object() {
            *collection = serde_json::json!({});
        }
        let servers = collection.as_object_mut().expect("normalized MCP object");
        let existing = servers.get(id);
        let target = McpConfigTarget::Claude;
        let policy = McpEntryEncodePolicy::PreserveFields;
        let snapshot = if enabled {
            let projected = project_server(id, server)?;
            let entry = if let Some(snapshot) = previous_snapshot.filter(|_| existing.is_none()) {
                let restored = target
                    .restore_native_entry_with_policy(snapshot, &projected, policy)
                    .map_err(|error| AppError::Config(error.to_string()))?;
                serde_json::from_str(&restored)
                    .map_err(|error| AppError::json(self.file.path(), error))?
            } else {
                target
                    .encode_server_with_policy(&projected, policy)
                    .map_err(|error| AppError::McpValidation(error.to_string()))?
            };
            servers.insert(id.to_owned(), entry);
            None
        } else {
            let snapshot = match existing.filter(|entry| entry.is_object()) {
                Some(entry) => Some(
                    target
                        .capture_native_entry(&entry.to_string())
                        .map_err(|error| AppError::Config(error.to_string()))?,
                ),
                None => previous_snapshot.cloned(),
            };
            servers.shift_remove(id);
            snapshot
        };
        let contents = serde_json::to_string_pretty(&root)
            .map_err(|source| AppError::JsonSerialize { source })?;
        self.file.publish(&contents)?;
        Ok(snapshot)
    }

    fn rollback(&mut self) -> Result<(), AppError> {
        self.file.rollback()
    }
}

#[cfg(test)]
mod tests;
