//! Codex's single-document MCP binding; catalog coordination lives in `toggle`.

use std::sync::MutexGuard;

use cc_switch_core::{codex::McpDocument, McpConfigTarget, McpNativeSnapshot};

use super::{
    native_file::NativeFile,
    toggle::{NativeAction, NativeChange, NativeToggle},
};
use crate::{
    codex_config::{get_codex_config_path, operation::lock_live_write},
    error::AppError,
    mcp::{json_server_to_codex_entry, parse_codex_mcp_document},
};

pub(super) struct CodexToggle {
    file: NativeFile,
    _guard: MutexGuard<'static, ()>,
}

impl CodexToggle {
    pub(super) fn observe() -> Result<Self, AppError> {
        let guard = lock_live_write()?;
        Ok(Self {
            file: NativeFile::observe(McpConfigTarget::Codex, &get_codex_config_path())?,
            _guard: guard,
        })
    }
}

impl NativeToggle for CodexToggle {
    fn apply_batch(
        &mut self,
        changes: &[NativeChange<'_>],
    ) -> Result<Vec<Option<McpNativeSnapshot>>, AppError> {
        if changes.is_empty() {
            return Ok(Vec::new());
        }
        // Codex has no removable-entry snapshot contract. Do not discard a
        // stored payload that this host cannot interpret.
        if changes
            .iter()
            .any(|change| change.previous_snapshot.is_some())
        {
            return Err(AppError::Database(
                "Unsupported Codex MCP native snapshot".into(),
            ));
        }
        let enabling = changes.iter().any(NativeChange::enabled);
        if !enabling && self.file.original().is_none() {
            return Ok(vec![None; changes.len()]);
        }
        let mut document = match self.file.original() {
            None => McpDocument::default(),
            Some(bytes) => {
                let text = std::str::from_utf8(bytes).map_err(|error| {
                    AppError::io(
                        self.file.path(),
                        std::io::Error::new(std::io::ErrorKind::InvalidData, error),
                    )
                })?;
                match parse_codex_mcp_document(text) {
                    Ok(document) => document,
                    Err(error) if !enabling => {
                        log::warn!("解析 Codex config.toml 失败: {error}，跳过删除操作");
                        return Ok(vec![None; changes.len()]);
                    }
                    Err(error) => {
                        return Err(AppError::McpValidation(format!(
                            "解析 config.toml 失败: {error}"
                        )))
                    }
                }
            }
        };
        for change in changes {
            let NativeChange { id, server, .. } = *change;
            if change.enabled() {
                let mut entry = json_server_to_codex_entry(server)?;
                // Selection owns this flag even when an imported catalog entry
                // contains an older native value. Other conversion rules stay put.
                if change.action == NativeAction::Enable {
                    entry
                        .insert_native_value("enabled", "true")
                        .map_err(|error| AppError::McpValidation(error.to_string()))?;
                }
                // Only migrate this ID; the old whole-section cleanup also removed
                // unrelated legacy entries during a single-server enable.
                document.remove_server(id);
                if document.upsert_server(id, entry) {
                    log::warn!("config.toml 的 mcp_servers 不是表，已重置为空表");
                }
            } else {
                let removed = document.remove_server(id);
                if removed.malformed_official_collection {
                    log::warn!("config.toml 的 mcp_servers 不是表，无法删除服务器 '{id}'");
                }
            }
        }
        self.file.publish(&document.render())?;
        Ok(vec![None; changes.len()])
    }

    fn rollback(&mut self) -> Result<(), AppError> {
        self.file.rollback()
    }
}

#[cfg(test)]
mod tests;
