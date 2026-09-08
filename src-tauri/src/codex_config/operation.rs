//! CLI filesystem binding for Core's Codex write executor.
//!
//! This lock serializes these writers through observation and rollback. Other
//! processes (and native writers not yet migrated here) can ignore it; ordinary
//! filesystem reads plus rename cannot exclude those races.

use std::{
    fs,
    io::{Read, Seek},
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};

use cc_switch_core::{
    execute_operation_plan_with_content_limit, CompareExchangeOutcome, ContentExpectation,
    LogicalTarget, OperationExecutionError, OperationFailure, OperationHost, OperationPlan,
    OperationRead, PlannedWrite, OPERATION_CONTRACT_MAJOR,
};

use crate::{
    config::{bind_config_write, delete_file, ConfigWriteTarget},
    error::AppError,
};

static LIVE_WRITE_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn lock_live_write() -> Result<MutexGuard<'static, ()>, AppError> {
    LIVE_WRITE_LOCK.lock().map_err(AppError::from)
}

#[cfg(test)]
type ExchangeHook = Box<dyn FnOnce(&ConfigWriteTarget) -> Result<(), AppError>>;

#[cfg(test)]
thread_local! {
    static BEFORE_CONFIG_EXCHANGE: std::cell::RefCell<Option<ExchangeHook>> = const { std::cell::RefCell::new(None) };
}

pub(super) struct CodexOperation {
    host: FileHost,
    plan: OperationPlan,
    maximum_content_bytes: usize,
}

impl CodexOperation {
    pub(super) fn observe() -> Result<Self, AppError> {
        let guard = lock_live_write()?;
        let targets = [
            (LogicalTarget::CodexAuth, super::get_codex_auth_path()),
            (LogicalTarget::CodexConfig, super::get_codex_config_path()),
        ];
        let mut resources = Vec::new();
        let mut symlinks = Vec::new();
        let mut writes = Vec::new();
        let mut maximum_content_bytes = 0;
        for (target, path) in targets {
            let resource = bind_config_write(&path)?;
            let link = fs::read_link(resource.path()).ok();
            let mut source = if resource.path().exists() {
                Some(fs::File::open(resource.path()).map_err(|e| AppError::io(&path, e))?)
            } else {
                None
            };
            // Preserve the CLI's existing accepted file sizes. Subsequent Core
            // reads are bounded by these observed and prepared document sizes.
            let contents = if let Some(file) = &mut source {
                let mut contents = Vec::new();
                file.read_to_end(&mut contents)
                    .map_err(|e| AppError::io(&path, e))?;
                Some(contents)
            } else {
                None
            };
            if let Some(link) = link {
                symlinks.push(BoundSymlink {
                    resource: resource.clone(),
                    link,
                    source,
                });
            }
            maximum_content_bytes =
                maximum_content_bytes.max(contents.as_ref().map_or(0, Vec::len));
            writes.push(PlannedWrite {
                target,
                expected: ContentExpectation::for_contents(contents.as_deref()),
                contents: None,
            });
            resources.push((target, resource));
        }
        Ok(Self {
            host: FileHost {
                resources,
                symlinks,
                _guard: guard,
            },
            plan: OperationPlan {
                contract_major: OPERATION_CONTRACT_MAJOR,
                app_id: "codex".to_string(),
                writes,
            },
            maximum_content_bytes,
        })
    }

    pub(super) fn execute(mut self, auth: Option<String>, config: String) -> Result<(), AppError> {
        let mut auth = auth;
        let mut config = Some(config);
        for write in &mut self.plan.writes {
            write.contents = match write.target {
                LogicalTarget::CodexAuth => auth.take(),
                _ => config.take(),
            };
            self.maximum_content_bytes = self
                .maximum_content_bytes
                .max(write.contents.as_ref().map_or(0, String::len));
        }
        execute_operation_plan_with_content_limit(
            &self.plan,
            &mut self.host,
            self.maximum_content_bytes,
        )
        .map(|_| ())
        .map_err(map_execution_error)
    }
}

struct FileHost {
    resources: Vec<(LogicalTarget, ConfigWriteTarget)>,
    symlinks: Vec<BoundSymlink>,
    _guard: MutexGuard<'static, ()>,
}

// Replacing a leaf link owns the directory entry, not its referent. Hold the
// original read source while that link remains in place: another write in this
// plan may replace or delete the name it points to. In-place source edits are
// still visible through the handle. Replacing a referent never writes through
// to it, and a changed leaf link is read afresh like any external replacement.
// The first exchange consumes this binding. Recovery must inspect the current
// entry, never the source that was bound before a possibly failed replacement.
struct BoundSymlink {
    resource: ConfigWriteTarget,
    link: PathBuf,
    source: Option<fs::File>,
}

impl OperationHost for FileHost {
    type Resource = ConfigWriteTarget;
    type Error = AppError;

    fn resolve(&mut self, target: LogicalTarget) -> Result<Self::Resource, Self::Error> {
        self.resources
            .iter()
            .find(|(candidate, _)| *candidate == target)
            .map(|(_, resource)| resource.clone())
            .ok_or_else(|| AppError::Config(format!("Unobserved Codex target: {target:?}")))
    }

    fn read(
        &mut self,
        resource: &Self::Resource,
        maximum: usize,
    ) -> Result<OperationRead, Self::Error> {
        read_entry(
            resource,
            maximum,
            self.symlinks
                .iter_mut()
                .find(|link| link.resource == *resource),
        )
    }

    fn compare_exchange(
        &mut self,
        resource: &Self::Resource,
        expected: Option<&[u8]>,
        replacement: Option<&[u8]>,
    ) -> Result<CompareExchangeOutcome, Self::Error> {
        let mut link = self
            .symlinks
            .iter()
            .position(|link| link.resource == *resource)
            .map(|index| self.symlinks.swap_remove(index));
        #[cfg(test)]
        if resource
            .path()
            .file_name()
            .is_some_and(|name| name == "config.toml")
        {
            let hook = BEFORE_CONFIG_EXCHANGE.with(|hook| hook.borrow_mut().take());
            if let Some(hook) = hook {
                hook(resource)?;
            }
        }
        let matches = match read_entry(resource, expected.map_or(0, <[u8]>::len), link.as_mut())? {
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

fn read_entry(
    resource: &ConfigWriteTarget,
    maximum: usize,
    link: Option<&mut BoundSymlink>,
) -> Result<OperationRead, AppError> {
    if let Some(link) =
        link.filter(|link| fs::read_link(resource.path()).ok().as_ref() == Some(&link.link))
    {
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
    if contents.len() > maximum {
        Ok(OperationRead::TooLarge)
    } else {
        Ok(OperationRead::Contents(contents))
    }
}

fn map_execution_error(error: OperationExecutionError<AppError>) -> AppError {
    let (failure, rollback_failures) = error.into_parts();
    let error = match failure {
        OperationFailure::Resolve { source, .. }
        | OperationFailure::Read { source, .. }
        | OperationFailure::Write { source, .. } => source,
        OperationFailure::Conflict { .. } | OperationFailure::ObservedContentTooLarge { .. } => {
            // The bound includes every initially observed document, so a
            // larger observation means a native file changed in the meantime.
            AppError::Conflict(failure.to_string())
        }
        _ => AppError::Config(failure.to_string()),
    };
    if rollback_failures.is_empty() {
        error
    } else {
        let failures = rollback_failures
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        AppError::localized(
            "codex.live.rollback_failed",
            format!("{error}；Codex 配置回滚未完成: {failures}"),
            format!("{error}; Codex config rollback incomplete: {failures}"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{codex_config::*, test_support::TestEnvGuard};
    use serde_json::json;

    fn seed(auth: &[u8], config: &[u8]) {
        fs::create_dir_all(get_codex_config_dir()).unwrap();
        fs::write(get_codex_auth_path(), auth).unwrap();
        fs::write(get_codex_config_path(), config).unwrap();
    }

    fn with_hook<R>(hook: ExchangeHook, action: impl FnOnce() -> R) -> R {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                BEFORE_CONFIG_EXCHANGE.with(|hook| *hook.borrow_mut() = None);
            }
        }
        BEFORE_CONFIG_EXCHANGE.with(|slot| *slot.borrow_mut() = Some(hook));
        let _reset = Reset;
        action()
    }

    fn injected_error(path: &std::path::Path) -> AppError {
        AppError::io(path, std::io::Error::other("injected write failure"))
    }

    #[test]
    fn codex_writes_recover_exact_bytes_before_and_after_a_failed_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        for replace_before_error in [false, true] {
            seed(b"\xff opaque old auth", b"# old config\n");
            let error = with_hook(
                Box::new(move |resource| {
                    assert_eq!(fs::read(get_codex_auth_path()).unwrap(), b"null");
                    if replace_before_error {
                        resource.write(b"model = 'next'\n")?;
                    }
                    Err(injected_error(resource.path()))
                }),
                || write_codex_live_atomic(&json!(null), Some("model = 'next'\n")),
            )
            .unwrap_err();
            assert!(matches!(error, AppError::Io { .. }), "{error}");
            assert_eq!(
                fs::read(get_codex_auth_path()).unwrap(),
                b"\xff opaque old auth"
            );
            assert_eq!(
                fs::read(get_codex_config_path()).unwrap(),
                b"# old config\n"
            );
        }
    }

    #[test]
    fn codex_writes_preserve_external_edits_on_preflight_and_rollback_conflicts() {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        seed(b"old auth", b"# old\n");
        let operation = CodexOperation::observe().unwrap();
        fs::write(get_codex_config_path(), b"# external\n").unwrap();
        assert!(matches!(
            operation.execute(Some("null".into()), "# next\n".into()),
            Err(AppError::Conflict(_))
        ));
        assert_eq!(fs::read(get_codex_auth_path()).unwrap(), b"old auth");
        assert_eq!(fs::read(get_codex_config_path()).unwrap(), b"# external\n");

        seed(b"old auth", b"# old\n");
        let error = with_hook(
            Box::new(|resource| {
                fs::write(get_codex_auth_path(), b"external auth").unwrap();
                Err(injected_error(resource.path()))
            }),
            || write_codex_live_atomic(&json!(null), Some("# next\n")),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            AppError::Localized {
                key: "codex.live.rollback_failed",
                ..
            }
        ));
        assert_eq!(fs::read(get_codex_auth_path()).unwrap(), b"external auth");
        assert_eq!(fs::read(get_codex_config_path()).unwrap(), b"# old\n");
    }

    #[test]
    fn codex_writes_keep_large_documents_and_auth_preservation_semantics() {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let large = format!(
            "# {}\n",
            "x".repeat(cc_switch_core::MAX_OPERATION_CONTENT_BYTES)
        );
        let auth = json!(["opaque payload", large]);
        write_codex_live_atomic(&auth, Some(&large)).unwrap();
        assert_eq!(
            fs::read(get_codex_auth_path()).unwrap(),
            serde_json::to_vec_pretty(&auth).unwrap()
        );
        assert_eq!(fs::read_to_string(get_codex_config_path()).unwrap(), large);
        write_codex_live_atomic_optional_auth(None, None).unwrap();
        assert!(!get_codex_auth_path().exists());
        assert_eq!(fs::read(get_codex_config_path()).unwrap(), b"");
        fs::create_dir(get_codex_auth_path()).unwrap();
        write_codex_live_config_atomic(Some("# config only\n")).unwrap();
        assert!(
            get_codex_auth_path().is_dir(),
            "preserved auth must not be read"
        );
    }

    #[test]
    fn codex_provider_failure_does_not_repeat_snapshot_rollback() {
        use crate::{AppState, AppType, Database, MultiAppConfig, Provider, ProviderService};
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        seed(b"{\"OPENAI_API_KEY\":\"old\"}", b"model = 'old'\n");
        let mut config = MultiAppConfig::default();
        config.ensure_app(&AppType::Codex);
        let manager = config.get_manager_mut(&AppType::Codex).unwrap();
        for id in ["old", "new"] {
            manager.providers.insert(
                id.into(),
                Provider::with_id(
                    id.into(),
                    id.into(),
                    json!({"auth": {"OPENAI_API_KEY": id}, "config": format!("model = '{id}'\n")}),
                    None,
                ),
            );
        }
        manager.current = "old".into();
        let db = std::sync::Arc::new(Database::memory().unwrap());
        db.migrate_from_json(&config).unwrap();
        let state = AppState::new(db);
        let error = with_hook(
            Box::new(|resource| {
                fs::write(
                    get_codex_auth_path(),
                    b"{\"tokens\":{\"access_token\":\"external\"}}",
                )
                .unwrap();
                fs::write(resource.path(), b"# external config\n").unwrap();
                Err(injected_error(resource.path()))
            }),
            || ProviderService::switch(&state, AppType::Codex, "new"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("rollback"), "{error}");
        assert_eq!(
            fs::read(get_codex_auth_path()).unwrap(),
            b"{\"tokens\":{\"access_token\":\"external\"}}"
        );
        assert_eq!(
            fs::read(get_codex_config_path()).unwrap(),
            b"# external config\n"
        );
        assert_eq!(
            state.db.get_current_provider("codex").unwrap().as_deref(),
            Some("old")
        );
        assert_eq!(
            state
                .config
                .read()
                .unwrap()
                .get_manager(&AppType::Codex)
                .unwrap()
                .current,
            "old"
        );
    }

    #[cfg(unix)]
    #[test]
    fn codex_writes_replace_cross_target_symlinks_without_self_conflicts() {
        use std::os::unix::fs::symlink;
        for shape in [
            "config_to_auth",
            "auth_to_config",
            "chain",
            "dangling",
            "cycle",
        ] {
            for delete_auth in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let _env = TestEnvGuard::isolated(temp.path());
                fs::create_dir_all(get_codex_config_dir()).unwrap();
                let auth = get_codex_auth_path();
                let config = get_codex_config_path();
                let source = temp.path().join("source");
                fs::write(&source, b"old source").unwrap();
                match shape {
                    "config_to_auth" => {
                        fs::write(&auth, b"old auth").unwrap();
                        symlink("auth.json", &config).unwrap();
                    }
                    "auth_to_config" => {
                        fs::write(&config, b"# old config\n").unwrap();
                        symlink("config.toml", &auth).unwrap();
                    }
                    "chain" => {
                        symlink(&source, &auth).unwrap();
                        symlink("auth.json", &config).unwrap();
                    }
                    "dangling" => symlink("auth.json", &config).unwrap(),
                    _ => {
                        symlink("config.toml", &auth).unwrap();
                        symlink("auth.json", &config).unwrap();
                    }
                }
                let replacement = (!delete_auth).then(|| json!(null));
                write_codex_live_atomic_optional_auth(replacement.as_ref(), Some("# next\n"))
                    .unwrap_or_else(|error| panic!("{shape}, delete={delete_auth}: {error}"));
                assert_eq!(fs::read(&config).unwrap(), b"# next\n");
                assert!(!fs::symlink_metadata(&config)
                    .unwrap()
                    .file_type()
                    .is_symlink());
                if delete_auth && shape == "cycle" {
                    // The legacy writer skips deletion when exists() follows
                    // a cycle and reports missing; replacing config breaks it.
                    assert!(fs::symlink_metadata(&auth)
                        .unwrap()
                        .file_type()
                        .is_symlink());
                    assert_eq!(fs::read(&auth).unwrap(), b"# next\n");
                } else if delete_auth {
                    assert!(!auth.exists());
                } else {
                    assert_eq!(fs::read(&auth).unwrap(), b"null");
                }
                assert_eq!(fs::read(&source).unwrap(), b"old source");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn codex_bound_links_detect_in_place_source_edits_and_leaf_replacement() {
        use std::os::unix::fs::symlink;
        for replace_leaf in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let _env = TestEnvGuard::isolated(temp.path());
            fs::create_dir_all(get_codex_config_dir()).unwrap();
            fs::write(get_codex_auth_path(), b"old auth").unwrap();
            let source = temp.path().join("source");
            fs::write(&source, b"# old\n").unwrap();
            symlink(&source, get_codex_config_path()).unwrap();
            let operation = CodexOperation::observe().unwrap();
            if replace_leaf {
                fs::remove_file(get_codex_config_path()).unwrap();
                fs::write(get_codex_config_path(), b"# external\n").unwrap();
            } else {
                fs::write(&source, b"# external\n").unwrap();
            }
            assert!(matches!(
                operation.execute(Some("null".into()), "# next\n".into()),
                Err(AppError::Conflict(_))
            ));
            assert_eq!(fs::read(get_codex_auth_path()).unwrap(), b"old auth");
            assert_eq!(fs::read(get_codex_config_path()).unwrap(), b"# external\n");
        }
    }

    #[cfg(unix)]
    #[test]
    fn codex_link_recovery_observes_atomic_source_replacement() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        fs::create_dir_all(get_codex_config_dir()).unwrap();
        fs::write(get_codex_auth_path(), b"old auth").unwrap();
        let source = temp.path().join("source.toml");
        fs::write(&source, b"# same\n").unwrap();
        symlink(&source, get_codex_config_path()).unwrap();
        let replacement = temp.path().join("external.toml");
        fs::write(&replacement, b"# external\n").unwrap();
        let error = with_hook(
            Box::new(move |resource| {
                fs::rename(replacement, source).unwrap();
                Err(injected_error(resource.path()))
            }),
            || write_codex_live_atomic(&json!(null), Some("# same\n")),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            AppError::Localized {
                key: "codex.live.rollback_failed",
                ..
            }
        ));
        assert_eq!(fs::read(get_codex_auth_path()).unwrap(), b"old auth");
        assert!(fs::symlink_metadata(get_codex_config_path())
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read(get_codex_config_path()).unwrap(), b"# external\n");
    }

    #[cfg(unix)]
    #[test]
    fn codex_config_only_replacement_does_not_require_read_permission() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        fs::create_dir_all(get_codex_config_dir()).unwrap();
        fs::create_dir(get_codex_auth_path()).unwrap();
        let path = get_codex_config_path();
        for mode in [0o200, 0o000] {
            fs::write(&path, b"# previous\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            write_codex_live_config_atomic(Some("# next\n")).unwrap();
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                mode
            );
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(fs::read(&path).unwrap(), b"# next\n");
        }

        let referent = temp.path().join("unreadable.toml");
        fs::write(&referent, b"# user-owned\n").unwrap();
        fs::set_permissions(&referent, fs::Permissions::from_mode(0o000)).unwrap();
        fs::remove_file(&path).unwrap();
        symlink(&referent, &path).unwrap();
        write_codex_live_config_atomic(Some("# replacement\n")).unwrap();
        assert!(!fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&referent, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"# replacement\n");
        assert_eq!(fs::read(&referent).unwrap(), b"# user-owned\n");
        assert!(get_codex_auth_path().is_dir());
    }

    #[cfg(windows)]
    #[test]
    fn codex_conditional_windows_replace_keeps_old_bytes_on_failure() {
        use std::os::windows::fs::OpenOptionsExt;
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        seed(b"old auth", b"# old config\n");
        let error = with_hook(
            Box::new(|resource| {
                let held = fs::OpenOptions::new()
                    .read(true)
                    .share_mode(1) // FILE_SHARE_READ, without delete sharing.
                    .open(resource.path())
                    .unwrap();
                let result = resource.write(b"# next\n");
                drop(held);
                assert_eq!(fs::read(resource.path()).unwrap(), b"# old config\n");
                assert!(result.is_err());
                result
            }),
            || write_codex_live_atomic(&json!(null), Some("# next\n")),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            AppError::Io { .. } | AppError::IoContext { .. }
        ));
        assert_eq!(fs::read(get_codex_auth_path()).unwrap(), b"old auth");
        assert_eq!(
            fs::read(get_codex_config_path()).unwrap(),
            b"# old config\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn codex_writes_bind_parent_and_preserve_leaf_symlink_and_permission_policy() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let original = temp.path().join("original");
        let other = temp.path().join("other");
        fs::create_dir(&original).unwrap();
        fs::create_dir(&other).unwrap();
        symlink(&original, get_codex_config_dir()).unwrap();
        fs::write(original.join("auth-target"), b"old auth").unwrap();
        symlink(original.join("auth-target"), get_codex_auth_path()).unwrap();
        fs::write(get_codex_config_path(), b"# old\n").unwrap();
        fs::set_permissions(get_codex_config_path(), fs::Permissions::from_mode(0o640)).unwrap();
        let operation = CodexOperation::observe().unwrap();
        fs::remove_file(get_codex_config_dir()).unwrap();
        symlink(&other, get_codex_config_dir()).unwrap();
        operation
            .execute(Some("null".into()), "# next\n".into())
            .unwrap();
        assert_eq!(fs::read(original.join("auth-target")).unwrap(), b"old auth");
        assert_eq!(fs::read(original.join("auth.json")).unwrap(), b"null");
        assert!(!fs::symlink_metadata(original.join("auth.json"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!other.join("config.toml").exists());
        assert_eq!(
            fs::metadata(original.join("config.toml"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );

        let managed = crate::config::get_app_config_dir();
        fs::create_dir_all(&managed).unwrap();
        std::env::set_var("CODEX_HOME", &managed);
        write_codex_live_atomic(&json!({"OPENAI_API_KEY": "key"}), Some("# private\n")).unwrap();
        assert_eq!(
            fs::metadata(managed.join("auth.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
