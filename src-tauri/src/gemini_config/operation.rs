//! Shared CLI Gemini filesystem session. The lock spans native observations,
//! publication and retained Core receipts. Force/Skill writers and other
//! processes do not yet share it; reads plus rename cannot exclude them.

use std::{
    collections::HashSet,
    fs,
    io::{Read, Seek},
    path::{Component, Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

use cc_switch_core::{
    execute_dependency_ordered_plan_with_content_limit, execute_operation_plan_with_content_limit,
    CompareExchangeOutcome, ContentExpectation, LogicalTarget, OperationExecutionError,
    OperationFailure, OperationHost, OperationPlan, OperationRead, OperationReceipt, PlannedWrite,
    OPERATION_CONTRACT_MAJOR,
};

use crate::{
    config::{bind_config_write, delete_file, ConfigWriteTarget},
    error::AppError,
};

static SETTINGS_WRITE_LOCK: Mutex<()> = Mutex::new(());

struct ObservedFile {
    target: LogicalTarget,
    path: PathBuf,
    observed_path: Option<Result<PathBuf, AppError>>,
    original: Option<String>,
    link: Option<(PathBuf, HashSet<PathBuf>, Option<fs::File>)>,
}

impl ObservedFile {
    fn observe(target: LogicalTarget, path: PathBuf) -> Result<Self, AppError> {
        // No mkdir or permission changes while reading/validating. Defer path
        // errors until publication so native JSON/entry validation keeps priority.
        let observed_path = resolve_entry(&path);
        let link = fs::read_link(&path).ok();
        let mut source = if path.exists() {
            Some(fs::File::open(&path).map_err(|e| AppError::io(&path, e))?)
        } else {
            None
        };
        let original = if let Some(source) = &mut source {
            let mut text = String::new();
            source
                .read_to_string(&mut text)
                .map_err(|e| AppError::io(&path, e))?;
            Some(text)
        } else {
            None
        };
        let link = link.map(|link| {
            let referents = observe_link_referents(&path, &link);
            (link, referents, source)
        });
        Ok(Self {
            target,
            path,
            observed_path: Some(observed_path),
            original,
            link,
        })
    }
}

pub(crate) struct GeminiOperation {
    files: Vec<ObservedFile>,
    host: FileHost,
    receipts: Vec<OperationReceipt<ConfigWriteTarget>>,
}

impl GeminiOperation {
    pub(crate) fn observe_settings(path: &Path) -> Result<Self, AppError> {
        Self::observe(vec![(LogicalTarget::GeminiSettings, path.to_path_buf())])
    }

    pub(crate) fn observe_provider() -> Result<Self, AppError> {
        Self::observe(vec![
            (LogicalTarget::GeminiEnv, super::get_gemini_env_path()),
            (
                LogicalTarget::GeminiSettings,
                super::get_gemini_settings_path(),
            ),
        ])
    }

    fn observe(targets: Vec<(LogicalTarget, PathBuf)>) -> Result<Self, AppError> {
        let guard = SETTINGS_WRITE_LOCK.lock().map_err(AppError::from)?;
        let files = targets
            .into_iter()
            .map(|(target, path)| ObservedFile::observe(target, path))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            files,
            host: FileHost {
                resources: Vec::new(),
                links: Vec::new(),
                published: Vec::new(),
                _guard: guard,
            },
            receipts: Vec::new(),
        })
    }

    fn settings(&self) -> &ObservedFile {
        self.files
            .iter()
            .find(|file| file.target == LogicalTarget::GeminiSettings)
            .expect("Gemini sessions always observe settings")
    }

    pub(crate) fn settings_path(&self) -> &Path {
        &self.settings().path
    }

    pub(crate) fn contents(&self) -> Option<&str> {
        self.settings().original.as_deref()
    }

    pub(crate) fn write_settings(&mut self, contents: String) -> Result<(), AppError> {
        self.execute(vec![(LogicalTarget::GeminiSettings, contents)], false)
    }

    pub(crate) fn write_provider(&mut self, env: String, settings: String) -> Result<(), AppError> {
        self.execute(
            vec![
                (LogicalTarget::GeminiEnv, env),
                (LogicalTarget::GeminiSettings, settings),
            ],
            true,
        )
    }

    fn execute(
        &mut self,
        replacements: Vec<(LogicalTarget, String)>,
        dependent: bool,
    ) -> Result<(), AppError> {
        let mut writes = Vec::new();
        let mut maximum = 0;
        for (target, contents) in &replacements {
            let file = self
                .files
                .iter_mut()
                .find(|file| file.target == *target)
                .ok_or_else(|| AppError::Config(format!("Unobserved Gemini target: {target:?}")))?;
            if !self.host.resources.iter().any(|(bound, _)| bound == target) {
                let observed_path = file.observed_path.take().ok_or_else(|| {
                    AppError::Config("Gemini operation cannot retry a failed binding".into())
                })??;
                if resolve_entry(&file.path)? != observed_path {
                    return Err(AppError::Conflict(
                        "Gemini path changed while preparing configuration".into(),
                    ));
                }
                let resource = bind_config_write(&file.path)?;
                if resource.path() != observed_path {
                    return Err(AppError::Conflict(
                        "Gemini path changed while preparing configuration".into(),
                    ));
                }
                if let Some((link, referents, source)) = file.link.take() {
                    self.host.links.push(BoundLink {
                        resource: resource.clone(),
                        link,
                        referents,
                        source,
                    });
                }
                self.host.resources.push((*target, resource));
            }
            if let Some((_, resource)) = self
                .host
                .resources
                .iter()
                .find(|(bound, _)| bound == target)
            {
                if resolve_entry(&file.path)? != resource.path() {
                    return Err(AppError::Conflict(
                        "Gemini path changed during configuration publication".into(),
                    ));
                }
            }
            maximum = maximum
                .max(contents.len())
                .max(file.original.as_ref().map_or(0, String::len));
            writes.push(PlannedWrite {
                target: *target,
                expected: ContentExpectation::for_contents(
                    file.original.as_deref().map(str::as_bytes),
                ),
                contents: Some(contents.clone()),
            });
        }
        let plan = OperationPlan {
            contract_major: OPERATION_CONTRACT_MAJOR,
            app_id: "gemini".into(),
            writes,
        };
        let receipt = if dependent {
            execute_dependency_ordered_plan_with_content_limit(&plan, &mut self.host, maximum)
        } else {
            execute_operation_plan_with_content_limit(&plan, &mut self.host, maximum)
        };
        let receipt = receipt.map_err(|error| map_execution_error(error, dependent))?;
        if let Some(previous) = self.receipts.last_mut() {
            if let Err(receipt) = previous.try_coalesce_last_write(receipt) {
                // Keep both records recoverable, but do not continue accumulating
                // versions if the shared ownership contract cannot combine them.
                self.receipts.push(receipt);
                return Err(AppError::Config(
                    "Gemini recovery records are not consecutive".into(),
                ));
            }
        } else {
            self.receipts.push(receipt);
        }
        // Advance only to bytes successfully published by this session. A later
        // arbitrary file observation never becomes a new ownership claim.
        for (target, contents) in replacements {
            self.files
                .iter_mut()
                .find(|file| file.target == target)
                .expect("published target was observed")
                .original = Some(contents);
        }
        Ok(())
    }

    pub(crate) fn rollback(&mut self) -> Result<(), AppError> {
        let mut failures = Vec::new();
        while let Some(receipt) = self.receipts.pop() {
            if let Err(error) = receipt.rollback(&mut self.host) {
                failures.extend(
                    error
                        .into_failures()
                        .into_iter()
                        .map(|failure| failure.to_string()),
                );
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(AppError::localized(
                "gemini.live.rollback_failed",
                format!("Gemini 配置回滚未完成: {}", failures.join("; ")),
                format!("Gemini config rollback incomplete: {}", failures.join("; ")),
            ))
        }
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

// Capture each link entry, not just its terminal file: a later owned write can
// replace an intermediate entry or create a missing terminal. Repeated entries
// stop traversal, including cycles that a native replacement can later break.
fn observe_link_referents(path: &Path, link: &Path) -> HashSet<PathBuf> {
    let mut referents = HashSet::new();
    let mut next = path.parent().map(|parent| parent.join(link));
    while let Some(path) = next {
        let Ok(entry) = resolve_entry(&path) else {
            break;
        };
        if referents.contains(&entry) {
            break;
        }
        next = fs::read_link(&entry)
            .ok()
            .and_then(|link| entry.parent().map(|parent| parent.join(link)));
        referents.insert(entry);
    }
    referents
}

struct FileHost {
    resources: Vec<(LogicalTarget, ConfigWriteTarget)>,
    links: Vec<BoundLink>,
    published: Vec<PublishedWrite>,
    _guard: MutexGuard<'static, ()>,
}

struct PublishedWrite {
    resource: ConfigWriteTarget,
    expected: ContentExpectation,
    maximum: usize,
}

struct BoundLink {
    resource: ConfigWriteTarget,
    link: PathBuf,
    referents: HashSet<PathBuf>,
    source: Option<fs::File>,
}

impl OperationHost for FileHost {
    type Resource = ConfigWriteTarget;
    type Error = AppError;

    fn resolve(&mut self, target: LogicalTarget) -> Result<Self::Resource, Self::Error> {
        self.resources
            .iter()
            .find(|(bound, _)| *bound == target)
            .map(|(_, resource)| resource.clone())
            .ok_or_else(|| AppError::Config(format!("Unobserved Gemini target: {target:?}")))
    }

    fn read(
        &mut self,
        resource: &Self::Resource,
        maximum: usize,
    ) -> Result<OperationRead, AppError> {
        read_entry(
            resource,
            maximum,
            self.links
                .iter_mut()
                .find(|link| link.resource == *resource)
                .and_then(|link| {
                    self.published
                        .iter()
                        .find(|written| link.referents.contains(written.resource.path()))
                        .map(|written| (link, written))
                }),
        )
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
        // A retained source is needed only when our preceding write replaced
        // its referent. Otherwise re-open it so external atomic replacements
        // remain visible, including standalone MCP writes through leaf links.
        // Keep an untouched leaf binding through a failed attempt. Its referent
        // may contain our preceding write; restoring that write also restores
        // what the surviving link reads. A replaced leaf is always read afresh.
        let matches = match self.read(resource, expected.map_or(0, <[u8]>::len))? {
            OperationRead::Missing => expected.is_none(),
            OperationRead::Contents(contents) => expected == Some(contents.as_slice()),
            OperationRead::TooLarge => false,
        };
        if !matches {
            return Ok(CompareExchangeOutcome::Conflict);
        }
        match replacement {
            Some(contents) => {
                let env = self.resources.iter().any(|(target, bound)| {
                    *target == LogicalTarget::GeminiEnv && bound == resource
                });
                #[cfg(unix)]
                if env {
                    use std::os::unix::fs::PermissionsExt;
                    let parent = resource.path().parent().expect("bound target has parent");
                    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                        .map_err(|e| AppError::io(parent, e))?;
                }
                resource.write(contents)?;
                #[cfg(unix)]
                if env {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(resource.path(), fs::Permissions::from_mode(0o600))
                        .map_err(|e| AppError::io(resource.path(), e))?;
                }
                #[cfg(not(unix))]
                let _ = env;
            }
            None => delete_file(resource.path())?,
        }
        self.links.retain(|link| link.resource != *resource);
        let published = PublishedWrite {
            resource: resource.clone(),
            expected: ContentExpectation::for_contents(replacement),
            maximum: replacement.map_or(0, <[u8]>::len),
        };
        if let Some(previous) = self
            .published
            .iter_mut()
            .find(|written| written.resource == *resource)
        {
            *previous = published;
        } else {
            self.published.push(published);
        }
        Ok(CompareExchangeOutcome::Applied)
    }
}

fn read_entry(
    resource: &ConfigWriteTarget,
    maximum: usize,
    link: Option<(&mut BoundLink, &PublishedWrite)>,
) -> Result<OperationRead, AppError> {
    if let Some((link, published)) =
        link.filter(|(link, _)| fs::read_link(resource.path()).ok().as_ref() == Some(&link.link))
    {
        // The old handle represents this leaf only while its visible referent
        // still contains our latest write. A past write is not ongoing ownership.
        if fs::canonicalize(resource.path()).ok().as_deref() != Some(published.resource.path()) {
            return Err(AppError::Conflict(
                "Gemini link target changed during configuration publication".into(),
            ));
        }
        let owned = match read_entry(&published.resource, published.maximum, None)? {
            OperationRead::Missing => published.expected.matches(None),
            OperationRead::Contents(contents) => published.expected.matches(Some(&contents)),
            OperationRead::TooLarge => false,
        };
        if !owned {
            return Err(AppError::Conflict(
                "Gemini link source changed during configuration publication".into(),
            ));
        }
        let Some(source) = &mut link.source else {
            return Ok(OperationRead::Missing);
        };
        source
            .rewind()
            .map_err(|e| AppError::io(resource.path(), e))?;
        return read_bounded(source, resource, maximum);
    }
    if !resource.path().exists() {
        return Ok(OperationRead::Missing);
    }
    let file = fs::File::open(resource.path()).map_err(|e| AppError::io(resource.path(), e))?;
    read_bounded(file, resource, maximum)
}

fn read_bounded(
    source: impl Read,
    resource: &ConfigWriteTarget,
    maximum: usize,
) -> Result<OperationRead, AppError> {
    let mut contents = Vec::new();
    source
        .take((maximum as u64).saturating_add(1))
        .read_to_end(&mut contents)
        .map_err(|e| AppError::io(resource.path(), e))?;
    Ok(if contents.len() > maximum {
        OperationRead::TooLarge
    } else {
        OperationRead::Contents(contents)
    })
}

fn map_execution_error(error: OperationExecutionError<AppError>, dependent: bool) -> AppError {
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
    if dependent {
        return AppError::localized(
            "gemini.live.rollback_failed",
            format!("{error}；Gemini 配置回滚未完成: {failures}"),
            format!("{error}; Gemini config rollback incomplete: {failures}"),
        );
    }
    AppError::localized(
        "gemini.mcp.rollback_failed",
        format!("{error}；Gemini MCP 配置回滚未完成: {failures}"),
        format!("{error}; Gemini MCP config rollback incomplete: {failures}"),
    )
}

#[cfg(test)]
type ExchangeHook = Box<dyn FnMut(&ConfigWriteTarget, Option<&[u8]>) -> Result<(), AppError>>;

#[cfg(test)]
pub(crate) fn with_hook<T>(hook: ExchangeHook, action: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            BEFORE_EXCHANGE.with(|slot| *slot.borrow_mut() = None);
        }
    }
    BEFORE_EXCHANGE.with(|slot| *slot.borrow_mut() = Some(hook));
    let _reset = Reset;
    action()
}

#[cfg(test)]
thread_local! {
    static BEFORE_EXCHANGE: std::cell::RefCell<Option<ExchangeHook>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod tests;
