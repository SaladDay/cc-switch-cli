//! Host binding for one MCP file. Core owns execution and retained recovery.

use std::{fs, io::Read, path::Path};

use cc_switch_core::{
    execute_mcp_write_with_content_limit, CompareExchangeOutcome, ContentExpectation,
    McpConfigTarget, OperationExecutionError, OperationFailure, OperationHost, OperationRead,
    OperationReceipt,
};

use crate::{
    config::{bind_config_write, delete_file, ConfigWriteTarget},
    error::AppError,
};

pub(super) struct NativeFile {
    target: McpConfigTarget,
    resource: ConfigWriteTarget,
    original: Option<Vec<u8>>,
    receipt: Option<OperationReceipt<ConfigWriteTarget, McpConfigTarget>>,
}

impl NativeFile {
    pub(super) fn observe(target: McpConfigTarget, path: &Path) -> Result<Self, AppError> {
        let resource = bind_config_write(path)?;
        let original = if resource.path().exists() {
            Some(fs::read(resource.path()).map_err(|e| AppError::io(resource.path(), e))?)
        } else {
            None
        };
        Ok(Self {
            target,
            resource,
            original,
            receipt: None,
        })
    }

    pub(super) fn path(&self) -> &Path {
        self.resource.path()
    }

    pub(super) fn original(&self) -> Option<&[u8]> {
        self.original.as_deref()
    }

    pub(super) fn publish(&mut self, contents: &str) -> Result<(), AppError> {
        let maximum = contents
            .len()
            .max(self.original.as_ref().map_or(0, Vec::len));
        let expected = ContentExpectation::for_contents(self.original());
        self.receipt = Some(
            execute_mcp_write_with_content_limit(self.target, &expected, contents, self, maximum)
                .map_err(map_execution_error)?,
        );
        Ok(())
    }

    pub(super) fn rollback(&mut self) -> Result<(), AppError> {
        if let Some(receipt) = self.receipt.take() {
            receipt
                .rollback(self)
                .map_err(|error| AppError::Config(error.to_string()))?;
        }
        Ok(())
    }
}

impl OperationHost<McpConfigTarget> for NativeFile {
    type Resource = ConfigWriteTarget;
    type Error = AppError;

    fn resolve(&mut self, target: McpConfigTarget) -> Result<Self::Resource, AppError> {
        if target != self.target {
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
pub(super) fn with_exchange_hook<T>(hook: ExchangeHook, action: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            BEFORE_EXCHANGE.with(|slot| slot.borrow_mut().take());
        }
    }
    BEFORE_EXCHANGE.with(|slot| *slot.borrow_mut() = Some(hook));
    let _reset = Reset;
    action()
}
