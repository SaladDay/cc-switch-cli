//! Host binding for one MCP file. Core owns execution and retained recovery.

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

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
    original_link: Option<PathBuf>,
    recovery_link: Option<tempfile::TempDir>,
    phase: Phase,
    receipt: Option<OperationReceipt<ConfigWriteTarget, McpConfigTarget>>,
}

enum Phase {
    Ready,
    Publishing,
    Recovering,
}

impl NativeFile {
    pub(super) fn observe(target: McpConfigTarget, path: &Path) -> Result<Self, AppError> {
        let resource = bind_config_write(path)?;
        let original_link = leaf_link(resource.path())?;
        let original = match fs::read(resource.path()) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(AppError::io(resource.path(), error)),
        };
        Ok(Self {
            target,
            resource,
            original,
            original_link,
            recovery_link: None,
            phase: Phase::Ready,
            receipt: None,
        })
    }

    pub(super) fn path(&self) -> &Path {
        self.resource.path()
    }

    pub(super) fn original(&self) -> Option<&[u8]> {
        self.original.as_deref()
    }

    pub(super) fn set_creation_permissions(
        &mut self,
        permissions: fs::Permissions,
    ) -> Result<(), AppError> {
        self.resource.set_creation_permissions(permissions)
    }

    pub(super) fn publish(&mut self, contents: &str) -> Result<(), AppError> {
        if !matches!(self.phase, Phase::Ready) {
            return Err(AppError::Config("MCP file already published".into()));
        }
        // Prepare recovery before touching the live leaf. In particular, a
        // Windows host without symlink privileges must fail before publication.
        if let Some(target) = &self.original_link {
            self.recovery_link = Some(stage_link(self.path(), target)?);
        }
        self.phase = Phase::Publishing;
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
        read_contents(resource.path(), maximum)
    }

    fn compare_exchange(
        &mut self,
        resource: &ConfigWriteTarget,
        expected: Option<&[u8]>,
        replacement: Option<&[u8]>,
    ) -> Result<CompareExchangeOutcome, AppError> {
        // This host executes exactly one write. Every later exchange is Core's
        // recovery, including recovery of a write that returned an I/O error.
        let recovering = match std::mem::replace(&mut self.phase, Phase::Recovering) {
            Phase::Publishing => false,
            Phase::Recovering => true,
            Phase::Ready => return Err(AppError::Config("MCP publication not started".into())),
        };
        #[cfg(test)]
        BEFORE_EXCHANGE.with(|slot| {
            if let Some(hook) = slot.borrow_mut().as_mut() {
                hook(resource, replacement)?;
            }
            Ok::<_, AppError>(())
        })?;
        let link = leaf_link(resource.path())?;
        if recovering {
            if let Some(original_link) = &self.original_link {
                // An uncertain write may have left the original link untouched.
                if link.as_ref() == Some(original_link)
                    && contents_match(resource.path(), replacement)?
                {
                    return Ok(CompareExchangeOutcome::Applied);
                }
                // Content-only recovery cannot prove that a changed link is
                // ours to replace. Return an error, not a content-only conflict
                // that Core could interpret as already recovered.
                if link.is_some() || !contents_match(resource.path(), expected)? {
                    return Err(AppError::Conflict(
                        "MCP link changed during recovery".into(),
                    ));
                }
                let referent = resource
                    .path()
                    .parent()
                    .expect("bound parent")
                    .join(original_link);
                if !contents_match(&referent, replacement)? {
                    return Err(AppError::Conflict(
                        "MCP link referent changed during recovery".into(),
                    ));
                }
                let staged = self
                    .recovery_link
                    .as_ref()
                    .ok_or_else(|| AppError::Config("Missing MCP recovery link".into()))?
                    .path()
                    .join("link");
                fs::rename(&staged, resource.path())
                    .map_err(|e| AppError::io(resource.path(), e))?;
                return Ok(CompareExchangeOutcome::Applied);
            }
            if link.is_some() {
                return Err(AppError::Conflict(
                    "MCP leaf became a link during recovery".into(),
                ));
            }
        } else if link != self.original_link {
            return Ok(CompareExchangeOutcome::Conflict);
        }
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

fn leaf_link(path: &Path) -> Result<Option<PathBuf>, AppError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => fs::read_link(path)
            .map(Some)
            .map_err(|e| AppError::io(path, e)),
        Ok(_) => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AppError::io(path, error)),
    }
}

fn read_contents(path: &Path, maximum: usize) -> Result<OperationRead, AppError> {
    // Reopen the path to observe external atomic replacements of a referent.
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(OperationRead::Missing)
        }
        Err(error) => return Err(AppError::io(path, error)),
    };
    let mut contents = Vec::new();
    file.take((maximum as u64).saturating_add(1))
        .read_to_end(&mut contents)
        .map_err(|e| AppError::io(path, e))?;
    Ok(if contents.len() > maximum {
        OperationRead::TooLarge
    } else {
        OperationRead::Contents(contents)
    })
}

fn contents_match(path: &Path, expected: Option<&[u8]>) -> Result<bool, AppError> {
    Ok(
        match read_contents(path, expected.map_or(0, <[u8]>::len))? {
            OperationRead::Missing => expected.is_none(),
            OperationRead::Contents(contents) => expected == Some(contents.as_slice()),
            OperationRead::TooLarge => false,
        },
    )
}

fn stage_link(path: &Path, target: &Path) -> Result<tempfile::TempDir, AppError> {
    let parent = path.parent().expect("bound parent");
    let staged = tempfile::Builder::new()
        .prefix(".cc-switch-mcp-recovery-")
        .tempdir_in(parent)
        .map_err(|e| AppError::io(parent, e))?;
    let link = staged.path().join("link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, &link).map_err(|e| AppError::io(&link, e))?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(target, &link).map_err(|e| AppError::io(&link, e))?;
    #[cfg(not(any(unix, windows)))]
    return Err(AppError::Config(
        "MCP link recovery is unsupported on this platform".into(),
    ));
    Ok(staged)
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

#[cfg(test)]
mod tests;
