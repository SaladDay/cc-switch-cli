//! Codex's single-document MCP binding; catalog coordination lives in `toggle`.

use std::{fs, io::Read, sync::MutexGuard};

use cc_switch_core::{
    codex::McpDocument, execute_mcp_write_with_content_limit, CompareExchangeOutcome,
    ContentExpectation, McpConfigTarget, McpNativeSnapshot, OperationExecutionError,
    OperationFailure, OperationHost, OperationRead, OperationReceipt,
};

use super::toggle::NativeToggle;
use crate::{
    codex_config::{get_codex_config_path, operation::lock_live_write},
    config::{bind_config_write, delete_file, ConfigWriteTarget},
    error::AppError,
    mcp::{json_server_to_codex_entry, parse_codex_mcp_document},
};

pub(super) struct CodexToggle {
    resource: ConfigWriteTarget,
    original: Option<Vec<u8>>,
    receipt: Option<OperationReceipt<ConfigWriteTarget, McpConfigTarget>>,
    _guard: MutexGuard<'static, ()>,
}

impl CodexToggle {
    pub(super) fn observe() -> Result<Self, AppError> {
        let guard = lock_live_write()?;
        let resource = bind_config_write(&get_codex_config_path())?;
        let original = if resource.path().exists() {
            Some(fs::read(resource.path()).map_err(|e| AppError::io(resource.path(), e))?)
        } else {
            None
        };
        Ok(Self {
            resource,
            original,
            receipt: None,
            _guard: guard,
        })
    }
}

impl NativeToggle for CodexToggle {
    fn apply(
        &mut self,
        id: &str,
        server: &serde_json::Value,
        enabled: bool,
        previous_snapshot: Option<&McpNativeSnapshot>,
    ) -> Result<Option<McpNativeSnapshot>, AppError> {
        // Codex has no removable-entry snapshot contract. Do not discard a
        // stored payload that this host cannot interpret.
        if previous_snapshot.is_some() {
            return Err(AppError::Database(
                "Unsupported Codex MCP native snapshot".into(),
            ));
        }
        if !enabled && self.original.is_none() {
            return Ok(None);
        }
        let mut document = match self.original.as_deref() {
            None => McpDocument::default(),
            Some(bytes) => {
                let text = std::str::from_utf8(bytes).map_err(|error| {
                    AppError::io(
                        self.resource.path(),
                        std::io::Error::new(std::io::ErrorKind::InvalidData, error),
                    )
                })?;
                match parse_codex_mcp_document(text) {
                    Ok(document) => document,
                    Err(error) if !enabled => {
                        log::warn!("解析 Codex config.toml 失败: {error}，跳过删除操作");
                        return Ok(None);
                    }
                    Err(error) => {
                        return Err(AppError::McpValidation(format!(
                            "解析 config.toml 失败: {error}"
                        )))
                    }
                }
            }
        };
        if enabled {
            let mut entry = json_server_to_codex_entry(server)?;
            // Selection owns this flag even when an imported catalog entry
            // contains an older native value. Other conversion rules stay put.
            entry
                .insert_native_value("enabled", "true")
                .map_err(|error| AppError::McpValidation(error.to_string()))?;
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
        let contents = document.render();
        let maximum = contents
            .len()
            .max(self.original.as_ref().map_or(0, Vec::len));
        let expected = ContentExpectation::for_contents(self.original.as_deref());
        self.receipt = Some(
            execute_mcp_write_with_content_limit(
                McpConfigTarget::Codex,
                &expected,
                &contents,
                self,
                maximum,
            )
            .map_err(map_execution_error)?,
        );
        Ok(None)
    }

    fn rollback(&mut self) -> Result<(), AppError> {
        if let Some(receipt) = self.receipt.take() {
            receipt
                .rollback(self)
                .map_err(|error| AppError::Config(error.to_string()))?;
        }
        Ok(())
    }
}

impl OperationHost<McpConfigTarget> for CodexToggle {
    type Resource = ConfigWriteTarget;
    type Error = AppError;

    fn resolve(&mut self, target: McpConfigTarget) -> Result<Self::Resource, AppError> {
        if target != McpConfigTarget::Codex {
            return Err(AppError::Config("Unobserved MCP resource".into()));
        }
        Ok(self.resource.clone())
    }

    fn read(
        &mut self,
        resource: &ConfigWriteTarget,
        maximum: usize,
    ) -> Result<OperationRead, AppError> {
        // A single-file operation never replaces its own leaf's referent.
        // Reopening observes external atomic replacements through leaf links.
        if !resource.path().exists() {
            return Ok(OperationRead::Missing);
        }
        let mut contents = Vec::new();
        fs::File::open(resource.path())
            .map_err(|e| AppError::io(resource.path(), e))?
            .take((maximum as u64).saturating_add(1))
            .read_to_end(&mut contents)
            .map_err(|e| AppError::io(resource.path(), e))?;
        Ok(if contents.len() > maximum {
            OperationRead::TooLarge
        } else {
            OperationRead::Contents(contents)
        })
    }

    fn compare_exchange(
        &mut self,
        resource: &ConfigWriteTarget,
        expected: Option<&[u8]>,
        replacement: Option<&[u8]>,
    ) -> Result<CompareExchangeOutcome, AppError> {
        #[cfg(test)]
        BEFORE_EXCHANGE.with(|slot| {
            if let Some(hook) = slot.borrow_mut().as_mut() {
                hook(resource, replacement)?;
            }
            Ok::<_, AppError>(())
        })?;
        let matches = match self.read(resource, expected.map_or(0, <[u8]>::len))? {
            OperationRead::Missing => expected.is_none(),
            OperationRead::Contents(contents) => expected == Some(contents.as_slice()),
            OperationRead::TooLarge => false,
        };
        if !matches {
            return Ok(CompareExchangeOutcome::Conflict);
        }
        match replacement {
            Some(bytes) => resource.write(bytes)?,
            None => delete_file(resource.path())?,
        }
        Ok(CompareExchangeOutcome::Applied)
    }
}

fn map_execution_error(error: OperationExecutionError<AppError, McpConfigTarget>) -> AppError {
    let (failure, rollback) = error.into_parts();
    let error = match failure {
        OperationFailure::Resolve { source, .. }
        | OperationFailure::Read { source, .. }
        | OperationFailure::Write { source, .. } => source,
        OperationFailure::Conflict { .. } | OperationFailure::ObservedContentTooLarge { .. } => {
            AppError::Conflict(failure.to_string())
        }
        _ => AppError::Config(failure.to_string()),
    };
    if rollback.is_empty() {
        return error;
    }
    AppError::Config(format!(
        "{error}; MCP native recovery incomplete: {}",
        rollback
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    ))
}

#[cfg(test)]
type ExchangeHook = Box<dyn FnMut(&ConfigWriteTarget, Option<&[u8]>) -> Result<(), AppError>>;

#[cfg(test)]
thread_local! {
    static BEFORE_EXCHANGE: std::cell::RefCell<Option<ExchangeHook>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod tests;
