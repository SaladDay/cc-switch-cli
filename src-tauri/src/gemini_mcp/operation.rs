//! Gemini MCP's filesystem binding for the shared executor. This process-local
//! lock spans observation, publication and receipt ownership. Provider/Skill
//! writers and other processes do not yet share it; reads plus rename cannot
//! exclude programs that ignore the lock.

use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

use cc_switch_core::{
    execute_operation_plan_with_content_limit, CompareExchangeOutcome, ContentExpectation,
    LogicalTarget, OperationExecutionError, OperationFailure, OperationHost, OperationPlan,
    OperationRead, OperationReceipt, PlannedWrite, OPERATION_CONTRACT_MAJOR,
};

use crate::{
    config::{bind_config_write, delete_file, ConfigWriteTarget},
    error::AppError,
};

static SETTINGS_WRITE_LOCK: Mutex<()> = Mutex::new(());

pub(super) struct SettingsOperation {
    path: PathBuf,
    observed_path: Result<PathBuf, AppError>,
    original: Option<String>,
    guard: MutexGuard<'static, ()>,
}

impl SettingsOperation {
    pub(super) fn observe(path: &Path) -> Result<Self, AppError> {
        let guard = SETTINGS_WRITE_LOCK.lock().map_err(AppError::from)?;
        // No mkdir or permission changes while reading/validating. Defer path
        // errors until publication so native JSON/entry validation keeps priority.
        let observed_path = resolve_entry(path);
        let original = super::read_json_text(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            observed_path,
            original,
            guard,
        })
    }

    pub(super) fn contents(&self) -> Option<&str> {
        self.original.as_deref()
    }

    pub(super) fn execute(self, contents: String) -> Result<SettingsWrite, AppError> {
        let observed_path = self.observed_path?;
        if resolve_entry(&self.path)? != observed_path {
            return Err(AppError::Conflict(
                "Gemini settings path changed while preparing MCP configuration".into(),
            ));
        }
        let resource = bind_config_write(&self.path)?;
        if resource.path() != observed_path {
            return Err(AppError::Conflict(
                "Gemini settings path changed while preparing MCP configuration".into(),
            ));
        }
        // Keep the native host's accepted sizes; later observations and recovery
        // are bounded by the original/prepared bytes, not a new fixed limit.
        let maximum = contents
            .len()
            .max(self.original.as_ref().map_or(0, String::len));
        let plan = OperationPlan {
            contract_major: OPERATION_CONTRACT_MAJOR,
            app_id: "gemini".into(),
            writes: vec![PlannedWrite {
                target: LogicalTarget::GeminiSettings,
                expected: ContentExpectation::for_contents(
                    self.original.as_deref().map(str::as_bytes),
                ),
                contents: Some(contents),
            }],
        };
        let mut host = FileHost {
            resource,
            _guard: self.guard,
        };
        let receipt = execute_operation_plan_with_content_limit(&plan, &mut host, maximum)
            .map_err(map_execution_error)?;
        Ok(SettingsWrite { host, receipt })
    }
}

// Resolve existing ancestors without following the leaf link or creating missing
// directories. A changed parent alias must not move the write to a different file
// merely because its contents happen to match the original observation.
fn resolve_entry(path: &Path) -> Result<PathBuf, AppError> {
    if path.is_relative() {
        let current = std::env::current_dir().map_err(|e| AppError::io(".", e))?;
        return resolve_entry(&current.join(path));
    }
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config("无效的路径".into()))?;
    let component = path
        .components()
        .next_back()
        .ok_or_else(|| AppError::Config("无效的文件名".into()))?;
    let mut parent = match fs::canonicalize(parent) {
        Ok(parent) => parent,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => resolve_entry(parent)?,
        Err(error) => return Err(AppError::io(parent, error)),
    };
    match component {
        Component::Normal(name) => Ok(parent.join(name)),
        Component::ParentDir => {
            parent.pop();
            Ok(parent)
        }
        _ => Err(AppError::Config("无效的文件名".into())),
    }
}

pub(super) struct SettingsWrite {
    host: FileHost,
    receipt: OperationReceipt<ConfigWriteTarget>,
}

impl SettingsWrite {
    /// Standalone MCP calls finish here. A wider transaction must retain its
    /// receipts and lock until its own outcome, rather than re-read a backup.
    pub(super) fn finish(self) {
        drop(self.receipt);
        drop(self.host);
    }
}

struct FileHost {
    resource: ConfigWriteTarget,
    _guard: MutexGuard<'static, ()>,
}

impl OperationHost for FileHost {
    type Resource = ConfigWriteTarget;
    type Error = AppError;

    fn resolve(&mut self, target: LogicalTarget) -> Result<Self::Resource, Self::Error> {
        if target != LogicalTarget::GeminiSettings {
            return Err(AppError::Config(format!(
                "Unobserved Gemini MCP target: {target:?}"
            )));
        }
        Ok(self.resource.clone())
    }

    fn read(
        &mut self,
        resource: &Self::Resource,
        maximum: usize,
    ) -> Result<OperationRead, AppError> {
        if !resource.path().exists() {
            return Ok(OperationRead::Missing);
        }
        let file = fs::File::open(resource.path()).map_err(|e| AppError::io(resource.path(), e))?;
        let mut contents = Vec::new();
        file.take((maximum as u64).saturating_add(1))
            .read_to_end(&mut contents)
            .map_err(|e| AppError::io(resource.path(), e))?;
        if contents.len() > maximum {
            Ok(OperationRead::TooLarge)
        } else {
            Ok(OperationRead::Contents(contents))
        }
    }

    fn compare_exchange(
        &mut self,
        resource: &Self::Resource,
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
            Some(contents) => resource.write(contents)?,
            None => delete_file(resource.path())?,
        }
        Ok(CompareExchangeOutcome::Applied)
    }
}

fn map_execution_error(error: OperationExecutionError<AppError>) -> AppError {
    let (failure, rollback_failures) = error.into_parts();
    let error = match failure {
        OperationFailure::Resolve { source, .. }
        | OperationFailure::Read { source, .. }
        | OperationFailure::Write { source, .. } => source,
        OperationFailure::Conflict { .. } | OperationFailure::ObservedContentTooLarge { .. } => {
            AppError::Conflict(failure.to_string())
        }
        _ => AppError::Config(failure.to_string()),
    };
    if rollback_failures.is_empty() {
        return error;
    }
    let failures = rollback_failures
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    AppError::localized(
        "gemini.mcp.rollback_failed",
        format!("{error}；Gemini MCP 配置回滚未完成: {failures}"),
        format!("{error}; Gemini MCP config rollback incomplete: {failures}"),
    )
}

#[cfg(test)]
type ExchangeHook = Box<dyn FnMut(&ConfigWriteTarget, Option<&[u8]>) -> Result<(), AppError>>;

#[cfg(test)]
thread_local! {
    static BEFORE_EXCHANGE: std::cell::RefCell<Option<ExchangeHook>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod tests;
