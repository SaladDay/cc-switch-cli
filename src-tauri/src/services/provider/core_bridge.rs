//! Narrow host bridge for cc-switch-core provider and live-document contracts.
//!
//! The CLI keeps ownership of persistence, paths, and file I/O. In particular,
//! these observations retain the exact bytes read from disk and must not reuse
//! the parsed `LiveSnapshot` used by the existing rollback path.

use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

use cc_switch_core::{
    builtin_app_adapter, execute_dependency_ordered_plan, execute_operation_plan,
    AppType as CoreAppType, CodexDocumentProjection, CompareExchangeOutcome, ContentExpectation,
    LiveDocumentSet, LogicalTarget, NativeAction, NativeDocumentProjection, NativeImportStep,
    NativePlanContext, NativePlanError, NativePlanPolicy, NativePlanRequest,
    NativePolicyPlanRequest, NativeProviderAccess, NativeProviderMode, ObservedDocument,
    OperationExecutionError, OperationFailure, OperationHost, OperationPlan, OperationRead,
    OperationReceipt, PlannedWrite, ProviderSnapshot, MAX_OPERATION_CONTENT_BYTES,
    OPERATION_CONTRACT_MAJOR,
};

use super::{PreparedCodexAuthWrite, PreparedLiveWrite};
use crate::{app_config::AppType, error::AppError, provider::Provider};

pub(super) fn provider_snapshot(app: &AppType, provider: &Provider) -> ProviderSnapshot {
    ProviderSnapshot::new(
        provider.id.clone(),
        app.as_core(),
        provider.name.clone(),
        provider.settings_config.clone(),
    )
}

/// Rehydrates Core-owned fields while retaining every CLI-owned provider field.
pub(super) fn provider_from_snapshot(
    expected_app: &AppType,
    snapshot: ProviderSnapshot,
    existing: Option<Provider>,
) -> Result<Provider, AppError> {
    if snapshot.app != expected_app.as_core() {
        return Err(AppError::Config(format!(
            "Core provider belongs to '{}', expected '{}'",
            snapshot.app.as_str(),
            expected_app.as_str()
        )));
    }

    if let Some(mut provider) = existing {
        if provider.id != snapshot.id {
            return Err(AppError::Config(format!(
                "Core provider id '{}' does not match existing provider '{}'",
                snapshot.id, provider.id
            )));
        }
        provider.name = snapshot.name;
        provider.settings_config = snapshot.settings;
        return Ok(provider);
    }

    Ok(Provider::with_id(
        snapshot.id,
        snapshot.name,
        snapshot.settings,
        None,
    ))
}

/// Drives Core's pure import projection while the CLI retains path and I/O ownership.
pub(super) fn native_import_settings(app: &AppType) -> Result<serde_json::Value, AppError> {
    let adapter = builtin_app_adapter(&app.as_core());
    let mut inventory = CoreDocumentInventory::new(app);

    loop {
        let step = adapter
            .project_native_import(&inventory.snapshot()?)
            .map_err(|error| {
                AppError::Config(format!(
                    "Core native import failed for '{}': {error}",
                    app.as_str()
                ))
            })?;
        match step {
            NativeImportStep::Observe { target } => inventory.observe(target)?,
            NativeImportStep::Ready { candidates } => {
                return single_import_settings(app, candidates)
            }
        }
    }
}

/// Projects only the supplied host snapshot and never performs additional I/O.
pub(super) fn native_import_settings_from_observations(
    app: &AppType,
    observations: impl IntoIterator<Item = (LogicalTarget, Option<Vec<u8>>)>,
) -> Result<serde_json::Value, AppError> {
    let adapter = builtin_app_adapter(&app.as_core());
    let mut inventory = CoreDocumentInventory::new(app);
    for (target, contents) in observations {
        inventory.record_observation(target, contents)?;
    }

    match adapter
        .project_native_import(&inventory.snapshot()?)
        .map_err(|error| {
            AppError::Config(format!(
                "Core native import failed for '{}': {error}",
                app.as_str()
            ))
        })? {
        NativeImportStep::Ready { candidates } => single_import_settings(app, candidates),
        NativeImportStep::Observe { target } => Err(AppError::Config(format!(
            "Core native import for '{}' requires {target:?}, which is not in the supplied snapshot",
            app.as_str()
        ))),
    }
}

fn single_import_settings(
    app: &AppType,
    mut candidates: Vec<cc_switch_core::NativeImportCandidate>,
) -> Result<serde_json::Value, AppError> {
    if candidates.len() != 1 {
        return Err(AppError::Config(format!(
            "Core native import for '{}' returned {} candidates",
            app.as_str(),
            candidates.len()
        )));
    }
    Ok(candidates.remove(0).provider.settings)
}

/// Incremental, app-scoped inventory used by Core's pure projection APIs.
pub(super) struct CoreDocumentInventory {
    app: CoreAppType,
    observations: HashMap<LogicalTarget, Option<Vec<u8>>>,
}

impl CoreDocumentInventory {
    pub(super) fn new(app: &AppType) -> Self {
        Self {
            app: app.as_core(),
            observations: HashMap::new(),
        }
    }

    /// Reads one requested target without parsing or normalizing its contents.
    pub(super) fn observe(&mut self, target: LogicalTarget) -> Result<(), AppError> {
        self.ensure_declared_target(target)?;
        if self.observations.contains_key(&target) {
            return Err(AppError::Config(format!(
                "Core target {target:?} was already observed"
            )));
        }

        let path = target_path(target)?;
        let contents = read_optional_bounded(&path)?;
        self.observations.insert(target, contents);
        Ok(())
    }

    /// Builds Core's complete target inventory, keeping unread targets distinct
    /// from targets that were checked and found missing.
    pub(super) fn snapshot(&self) -> Result<LiveDocumentSet, AppError> {
        let documents = builtin_app_adapter(&self.app)
            .targets()
            .iter()
            .copied()
            .map(|target| match self.observations.get(&target) {
                Some(Some(contents)) => ObservedDocument::present(target, contents.clone()),
                Some(None) => ObservedDocument::missing(target),
                None => ObservedDocument::unobserved(target),
            });

        LiveDocumentSet::try_new(self.app.clone(), documents)
            .map_err(|error| AppError::Config(format!("Invalid Core document inventory: {error}")))
    }

    fn record_observation(
        &mut self,
        target: LogicalTarget,
        contents: Option<Vec<u8>>,
    ) -> Result<(), AppError> {
        self.ensure_declared_target(target)?;
        if self.observations.contains_key(&target) {
            return Err(AppError::Config(format!(
                "Core target {target:?} was already observed"
            )));
        }
        self.observations.insert(target, contents);
        Ok(())
    }

    fn ensure_declared_target(&self, target: LogicalTarget) -> Result<(), AppError> {
        if builtin_app_adapter(&self.app).targets().contains(&target) {
            return Ok(());
        }
        Err(AppError::InvalidInput(format!(
            "Core target {target:?} is not declared for '{}'",
            self.app.as_str()
        )))
    }
}

pub(super) fn target_path(target: LogicalTarget) -> Result<PathBuf, AppError> {
    match target {
        LogicalTarget::ClaudeSettings => Ok(crate::config::get_claude_settings_path()),
        LogicalTarget::CodexAuth => Ok(crate::codex_config::get_codex_auth_path()),
        LogicalTarget::CodexConfig => Ok(crate::codex_config::get_codex_config_path()),
        LogicalTarget::CodexModelCatalog => Ok(crate::codex_config::get_codex_model_catalog_path()),
        LogicalTarget::GeminiEnv => Ok(crate::gemini_config::get_gemini_env_path()),
        LogicalTarget::GeminiSettings => Ok(crate::gemini_config::get_gemini_settings_path()),
        LogicalTarget::OpenCodeConfig => Ok(crate::opencode_config::get_opencode_config_path()),
        LogicalTarget::OpenClawConfig => Ok(crate::openclaw_config::get_openclaw_config_path()),
        LogicalTarget::HermesConfig => Ok(crate::hermes_config::get_hermes_config_path()),
        LogicalTarget::PiModels => crate::pi_config::get_pi_models_path(),
        LogicalTarget::ClaudeDesktopNormalConfig
        | LogicalTarget::ClaudeDesktopThreepConfig
        | LogicalTarget::ClaudeDesktopProfile
        | LogicalTarget::ClaudeDesktopMeta
        | LogicalTarget::GrokConfig => Err(AppError::InvalidInput(format!(
            "Core target {target:?} is not supported by cc-switch-cli"
        ))),
    }
}

struct CliOperationHost;

pub(super) struct CoreOperationReceipt(OperationReceipt<PathBuf>);

impl CoreOperationReceipt {
    pub(super) fn rollback(self) -> Result<(), AppError> {
        self.0
            .rollback(&mut CliOperationHost)
            .map_err(|error| AppError::Message(format!("Core live rollback failed: {error}")))
    }
}

pub(super) enum ClaudeSettingsWrite {
    Compatibility,
    Core(CoreOperationReceipt),
}

#[derive(Debug)]
pub(super) enum ClaudeSettingsWriteError {
    Compatibility(AppError),
    Core(AppError),
}

impl ClaudeSettingsWriteError {
    fn into_error(self) -> AppError {
        match self {
            Self::Compatibility(error) | Self::Core(error) => error,
        }
    }
}

#[cfg(test)]
std::thread_local! {
    static TEST_READ_REPLACEMENT: std::cell::RefCell<Option<(PathBuf, usize, Vec<u8>)>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn replace_before_target_operation_read(
    target: LogicalTarget,
    read_number: usize,
    contents: Vec<u8>,
) {
    assert!(read_number > 0, "operation read number must be positive");
    let path = target_path(target).expect("test target must resolve");
    TEST_READ_REPLACEMENT.with(|replacement| {
        assert!(
            replacement
                .replace(Some((path, read_number, contents)))
                .is_none(),
            "operation-read replacement already registered"
        );
    });
}

impl OperationHost for CliOperationHost {
    type Resource = PathBuf;
    type Error = AppError;

    fn resolve(&mut self, target: LogicalTarget) -> Result<Self::Resource, Self::Error> {
        target_path(target)
    }

    fn read(
        &mut self,
        resource: &Self::Resource,
        maximum: usize,
    ) -> Result<OperationRead, Self::Error> {
        #[cfg(test)]
        let replacement = TEST_READ_REPLACEMENT.with(|replacement| {
            let mut replacement = replacement.borrow_mut();
            let Some((target, reads_remaining, _)) = replacement.as_mut() else {
                return None;
            };
            if target != resource {
                return None;
            }
            *reads_remaining -= 1;
            if *reads_remaining == 0 {
                replacement.take().map(|(_, _, contents)| contents)
            } else {
                None
            }
        });
        #[cfg(test)]
        if let Some(contents) = replacement {
            std::fs::write(resource, contents).map_err(|error| AppError::io(resource, error))?;
        }

        read_optional_bounded_state(resource, maximum)
    }

    fn compare_exchange(
        &mut self,
        resource: &Self::Resource,
        expected: Option<&[u8]>,
        replacement: Option<&[u8]>,
    ) -> Result<CompareExchangeOutcome, Self::Error> {
        let current =
            read_optional_bounded_state(resource, expected.map_or(0, |contents| contents.len()))?;
        let matches = match current {
            OperationRead::Missing => expected.is_none(),
            OperationRead::Contents(contents) => expected == Some(contents.as_slice()),
            OperationRead::TooLarge => false,
        };
        if !matches {
            return Ok(CompareExchangeOutcome::Conflict);
        }
        match replacement {
            Some(contents) => crate::config::atomic_write(resource, contents),
            None => crate::config::delete_file(resource),
        }?;
        Ok(CompareExchangeOutcome::Applied)
    }
}

/// Retains the existing sync-path projection while using Core's executor.
pub(super) fn execute_claude_settings_under_lock(
    settings: &serde_json::Value,
) -> Result<(), AppError> {
    execute_claude_settings(settings, false)
        .map(drop)
        .map_err(ClaudeSettingsWriteError::into_error)
}

/// Uses Core's Claude adapter and executor for an ordinary provider switch.
pub(super) fn execute_claude_settings_with_adapter_under_lock(
    settings: &serde_json::Value,
) -> Result<ClaudeSettingsWrite, ClaudeSettingsWriteError> {
    execute_claude_settings(settings, true)
}

/// Retains the CLI's JSON bytes and host-owned path behavior. Legacy inputs
/// outside Core's bounds keep the old overwrite behavior. The caller must hold
/// the per-application switch lock.
fn execute_claude_settings(
    settings: &serde_json::Value,
    use_adapter: bool,
) -> Result<ClaudeSettingsWrite, ClaudeSettingsWriteError> {
    let contents = serde_json::to_string_pretty(settings)
        .map_err(|source| ClaudeSettingsWriteError::Core(AppError::JsonSerialize { source }))?;
    let target = LogicalTarget::ClaudeSettings;
    let mut host = CliOperationHost;
    let resource = host
        .resolve(target)
        .map_err(ClaudeSettingsWriteError::Core)?;
    if contents.len() > MAX_OPERATION_CONTENT_BYTES
        || (use_adapter && (!settings.is_object() || contents.len() == MAX_OPERATION_CONTENT_BYTES))
    {
        return crate::config::write_json_file(&resource, settings)
            .map(|_| ClaudeSettingsWrite::Compatibility)
            .map_err(ClaudeSettingsWriteError::Compatibility);
    }
    let original = match host.read(&resource, MAX_OPERATION_CONTENT_BYTES) {
        Ok(OperationRead::Missing) => None,
        Ok(OperationRead::Contents(contents)) => Some(contents),
        Ok(OperationRead::TooLarge) | Err(_) => {
            return crate::config::write_json_file(&resource, settings)
                .map(|_| ClaudeSettingsWrite::Compatibility)
                .map_err(ClaudeSettingsWriteError::Compatibility)
        }
    };
    let plan = if use_adapter {
        claude_adapter_plan(settings, &contents, target, original)
            .map_err(ClaudeSettingsWriteError::Core)?
    } else {
        OperationPlan {
            contract_major: OPERATION_CONTRACT_MAJOR,
            app_id: CoreAppType::Claude.as_str().to_owned(),
            writes: vec![PlannedWrite {
                target,
                expected: ContentExpectation::for_contents(original.as_deref()),
                contents: Some(contents),
            }],
        }
    };

    execute_operation_plan(&plan, &mut host)
        .map(|receipt| ClaudeSettingsWrite::Core(CoreOperationReceipt(receipt)))
        .map_err(|error| ClaudeSettingsWriteError::Core(map_execution_error(error)))
}

fn claude_adapter_plan(
    settings: &serde_json::Value,
    contents: &str,
    target: LogicalTarget,
    original: Option<Vec<u8>>,
) -> Result<OperationPlan, AppError> {
    let document = match original {
        Some(contents) => ObservedDocument::present(target, contents),
        None => ObservedDocument::missing(target),
    };
    let documents = LiveDocumentSet::try_new(CoreAppType::Claude, [document])
        .map_err(|error| AppError::Config(format!("Invalid Core Claude snapshot: {error}")))?;
    let provider = ProviderSnapshot::new(
        "cli-effective",
        CoreAppType::Claude,
        "CLI effective settings",
        settings.clone(),
    );
    let request = NativePlanRequest {
        action: NativeAction::Apply,
        provider: &provider,
        documents: &documents,
        mode: NativeProviderMode::Custom,
        access: NativeProviderAccess::Writable,
        context: NativePlanContext::Standard {
            common_config: None,
        },
    };
    let adapter = builtin_app_adapter(&CoreAppType::Claude);
    let mut plan = adapter
        .plan_native(&request)
        .map_err(|error| AppError::Config(format!("Core Claude projection failed: {error}")))?;
    let projected = plan
        .writes
        .first_mut()
        .and_then(|write| write.contents.as_mut())
        .ok_or_else(|| AppError::Config("Core Claude projection did not produce a write".into()))?;
    if projected.strip_suffix('\n') == Some(contents) {
        projected.pop();
    } else if projected.as_str() != contents {
        return Err(AppError::Config(
            "Core Claude projection changed the prepared CLI settings".into(),
        ));
    }
    Ok(plan)
}

pub(super) enum CodexPreparedWrite {
    Compatibility,
    Core(CoreOperationReceipt),
}

/// Executes the CLI's established Codex projection through Core's typed policy
/// and multi-file executor. Inputs outside Core's contract are left for the
/// caller's compatibility writer; this function has not written in that case.
pub(super) fn execute_prepared_codex_under_lock(
    prepared: &PreparedLiveWrite,
) -> Result<CodexPreparedWrite, AppError> {
    let PreparedLiveWrite::Codex { auth, config } = prepared else {
        return Err(AppError::Config(
            "Core Codex execution requires a prepared Codex write".into(),
        ));
    };

    let auth_contents = match auth {
        PreparedCodexAuthWrite::Write(auth) => Some(
            serde_json::to_string_pretty(auth)
                .map_err(|source| AppError::JsonSerialize { source })?,
        ),
        PreparedCodexAuthWrite::Preserve | PreparedCodexAuthWrite::Delete => None,
    };
    let catalog_contents = config
        .model_catalog
        .as_ref()
        .map(serde_json::to_string_pretty)
        .transpose()
        .map_err(|source| AppError::JsonSerialize { source })?;
    if [&auth_contents, &catalog_contents]
        .into_iter()
        .flatten()
        .any(|contents| contents.len() > MAX_OPERATION_CONTENT_BYTES)
        || config.config_text.len() > MAX_OPERATION_CONTENT_BYTES
    {
        return Ok(CodexPreparedWrite::Compatibility);
    }

    let auth_projection = match auth {
        PreparedCodexAuthWrite::Preserve => NativeDocumentProjection::Preserve,
        PreparedCodexAuthWrite::Write(_) => {
            let Some(contents) = auth_contents.as_deref() else {
                return Err(AppError::Config(
                    "Serialized Codex auth is unexpectedly missing".into(),
                ));
            };
            NativeDocumentProjection::Write(contents)
        }
        PreparedCodexAuthWrite::Delete => NativeDocumentProjection::Delete,
    };
    let catalog_projection = catalog_contents.as_deref().map_or(
        NativeDocumentProjection::Preserve,
        NativeDocumentProjection::Write,
    );
    let provider = ProviderSnapshot::new(
        "cli-effective",
        CoreAppType::Codex,
        "CLI effective settings",
        serde_json::json!({}),
    );
    let policy = NativePlanPolicy::CodexDocuments(CodexDocumentProjection {
        auth: auth_projection,
        config: &config.config_text,
        model_catalog: catalog_projection,
    });
    let adapter = builtin_app_adapter(&CoreAppType::Codex);
    let targets = adapter
        .required_native_targets_for_policy(NativeAction::Apply, &provider, &policy)
        .map_err(|error| {
            AppError::Config(format!("Core Codex target selection failed: {error}"))
        })?;

    let mut host = CliOperationHost;
    let mut inventory = CoreDocumentInventory::new(&AppType::Codex);
    for target in targets {
        let resource = host.resolve(target)?;
        let original = match host.read(&resource, MAX_OPERATION_CONTENT_BYTES) {
            Ok(OperationRead::Missing) => None,
            Ok(OperationRead::Contents(contents)) => Some(contents),
            Ok(OperationRead::TooLarge) | Err(_) => return Ok(CodexPreparedWrite::Compatibility),
        };
        inventory.record_observation(target, original)?;
    }
    let documents = inventory.snapshot()?;
    let request = NativePolicyPlanRequest {
        action: NativeAction::Apply,
        provider: &provider,
        documents: &documents,
        access: NativeProviderAccess::Writable,
        policy,
    };
    let plan = match adapter.plan_native_policy(&request) {
        Ok(plan) => plan,
        Err(NativePlanError::InputTooLarge { .. } | NativePlanError::InvalidProjection { .. }) => {
            return Ok(CodexPreparedWrite::Compatibility)
        }
        Err(error) => {
            return Err(AppError::Config(format!(
                "Core Codex projection failed: {error}"
            )))
        }
    };
    execute_dependency_ordered_plan(&plan, &mut host)
        .map(|receipt| CodexPreparedWrite::Core(CoreOperationReceipt(receipt)))
        .map_err(map_execution_error)
}

fn map_execution_error(error: OperationExecutionError<AppError>) -> AppError {
    if error.rollback_failures().is_empty() {
        match error.failure() {
            OperationFailure::Conflict { target } => {
                return AppError::Conflict(format!(
                    "Core live target {target:?} changed while the write was being prepared"
                ));
            }
            OperationFailure::Read { target, .. } => {
                return AppError::Conflict(format!(
                    "Core live target {target:?} could not be re-read after its baseline was established"
                ));
            }
            OperationFailure::ObservedContentTooLarge { target, .. } => {
                return AppError::Conflict(format!(
                    "Core live target {target:?} grew beyond the observation limit after its baseline was established"
                ));
            }
            _ => {}
        }
    }
    AppError::Message(format!("Core live operation failed: {error}"))
}

fn read_optional_bounded(path: &Path) -> Result<Option<Vec<u8>>, AppError> {
    match read_optional_bounded_state(path, MAX_OPERATION_CONTENT_BYTES)? {
        OperationRead::Missing => Ok(None),
        OperationRead::Contents(contents) => Ok(Some(contents)),
        OperationRead::TooLarge => Err(oversized_live_config_error(
            path,
            MAX_OPERATION_CONTENT_BYTES,
        )),
    }
}

fn read_optional_bounded_state(path: &Path, maximum: usize) -> Result<OperationRead, AppError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(OperationRead::Missing),
        Err(error) => return Err(AppError::io(path, error)),
    };
    let mut bytes = Vec::new();
    file.take((maximum as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| AppError::io(path, error))?;
    if bytes.len() > maximum {
        return Ok(OperationRead::TooLarge);
    }
    Ok(OperationRead::Contents(bytes))
}

fn oversized_live_config_error(path: &Path, maximum: usize) -> AppError {
    AppError::InvalidInput(format!(
        "Live configuration exceeds the {maximum}-byte Core input limit: {}",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{provider::ProviderMeta, test_support::TestEnvGuard};
    use serde_json::{json, Value};

    fn cli_owned_fields(provider: &Provider) -> Value {
        let mut value = serde_json::to_value(provider).expect("serialize provider");
        let object = value.as_object_mut().expect("provider object");
        object.remove("id");
        object.remove("name");
        object.remove("settingsConfig");
        value
    }

    #[test]
    fn provider_round_trip_preserves_cli_owned_fields() {
        let mut existing = Provider::with_id(
            "provider-1".to_string(),
            "Before".to_string(),
            json!({"futureSetting": {"preserved": true}}),
            Some("https://example.com".to_string()),
        );
        existing.category = Some("custom".to_string());
        existing.created_at = Some(123);
        existing.sort_index = Some(4);
        existing.notes = Some("host-owned".to_string());
        existing.icon = Some("anthropic".to_string());
        existing.icon_color = Some("#123456".to_string());
        existing.in_failover_queue = true;
        existing.meta = Some(ProviderMeta {
            apply_common_config: Some(true),
            cost_multiplier: Some("1.5".to_string()),
            ..ProviderMeta::default()
        });
        let cli_fields = cli_owned_fields(&existing);

        let snapshot = provider_snapshot(&AppType::Claude, &existing);
        assert_eq!(snapshot.id, "provider-1");
        assert_eq!(snapshot.app, CoreAppType::Claude);
        assert_eq!(snapshot.settings["futureSetting"]["preserved"], true);

        let replacement = ProviderSnapshot::new(
            "provider-1",
            CoreAppType::Claude,
            "After",
            json!({"env": {"ANTHROPIC_AUTH_TOKEN": "secret"}}),
        );
        let restored = provider_from_snapshot(&AppType::Claude, replacement, Some(existing))
            .expect("compatible Core snapshot");

        assert_eq!(restored.name, "After");
        assert_eq!(
            restored.settings_config["env"]["ANTHROPIC_AUTH_TOKEN"],
            "secret"
        );
        assert_eq!(cli_owned_fields(&restored), cli_fields);
    }

    #[test]
    fn provider_conversion_rejects_cross_app_and_cross_id_merges() {
        let existing = Provider::with_id(
            "provider-1".to_string(),
            "Existing".to_string(),
            json!({}),
            None,
        );
        let wrong_app = ProviderSnapshot::new("provider-1", CoreAppType::Codex, "Codex", json!({}));
        assert!(
            provider_from_snapshot(&AppType::Claude, wrong_app, Some(existing.clone())).is_err()
        );

        let wrong_id =
            ProviderSnapshot::new("provider-2", CoreAppType::Claude, "Claude", json!({}));
        assert!(provider_from_snapshot(&AppType::Claude, wrong_id, Some(existing)).is_err());
    }

    #[test]
    fn document_inventory_preserves_present_missing_and_unobserved_states() {
        let mut inventory = CoreDocumentInventory::new(&AppType::Codex);
        let raw = vec![0xff, b'\n', b'{', b'}'];
        inventory
            .record_observation(LogicalTarget::CodexAuth, Some(raw.clone()))
            .expect("record raw auth");
        inventory
            .record_observation(LogicalTarget::CodexConfig, None)
            .expect("record missing config");

        let snapshot = inventory.snapshot().expect("complete inventory");
        let auth = snapshot
            .document(LogicalTarget::CodexAuth)
            .expect("auth document");
        assert!(auth.is_observed());
        assert_eq!(auth.contents(), Some(raw.as_slice()));

        let config = snapshot
            .document(LogicalTarget::CodexConfig)
            .expect("config document");
        assert!(config.is_observed());
        assert_eq!(config.contents(), None);

        let catalog = snapshot
            .document(LogicalTarget::CodexModelCatalog)
            .expect("catalog document");
        assert!(!catalog.is_observed());
        assert_eq!(catalog.contents(), None);
    }

    #[test]
    fn observation_reads_exact_cli_owned_bytes_and_reports_missing_files() {
        let home = tempfile::tempdir().expect("temporary home");
        let _env = TestEnvGuard::isolated(home.path());
        let path = target_path(LogicalTarget::ClaudeSettings).expect("Claude path");
        std::fs::create_dir_all(path.parent().expect("settings parent"))
            .expect("create settings parent");
        let raw = b"{\n  \"token\": \"secret\"  \n}\n";
        std::fs::write(&path, raw).expect("write sandbox settings");

        let mut present = CoreDocumentInventory::new(&AppType::Claude);
        present
            .observe(LogicalTarget::ClaudeSettings)
            .expect("observe settings");
        assert_eq!(
            present
                .snapshot()
                .expect("present snapshot")
                .document(LogicalTarget::ClaudeSettings)
                .expect("settings document")
                .contents(),
            Some(raw.as_slice())
        );

        std::fs::remove_file(&path).expect("remove sandbox settings");
        let mut missing = CoreDocumentInventory::new(&AppType::Claude);
        missing
            .observe(LogicalTarget::ClaudeSettings)
            .expect("observe missing settings");
        let document = missing
            .snapshot()
            .expect("missing snapshot")
            .document(LogicalTarget::ClaudeSettings)
            .expect("settings document")
            .clone();
        assert!(document.is_observed());
        assert_eq!(document.contents(), None);
    }

    #[test]
    fn cli_path_mapping_covers_every_cli_adapter_target_only() {
        let home = tempfile::tempdir().expect("temporary home");
        let _env = TestEnvGuard::isolated(home.path());

        for app in AppType::all() {
            for target in builtin_app_adapter(&app.as_core()).targets() {
                assert!(
                    target_path(*target).is_ok(),
                    "{} target {target:?} needs a CLI path",
                    app.as_str()
                );
            }
        }

        for target in [
            LogicalTarget::ClaudeDesktopNormalConfig,
            LogicalTarget::ClaudeDesktopThreepConfig,
            LogicalTarget::ClaudeDesktopProfile,
            LogicalTarget::ClaudeDesktopMeta,
            LogicalTarget::GrokConfig,
        ] {
            assert!(target_path(target).is_err());
        }
    }

    #[test]
    fn inventory_rejects_duplicate_and_cross_app_observations() {
        let mut inventory = CoreDocumentInventory::new(&AppType::Claude);
        inventory
            .record_observation(LogicalTarget::ClaudeSettings, None)
            .expect("first observation");
        assert!(inventory
            .record_observation(LogicalTarget::ClaudeSettings, None)
            .is_err());
        assert!(inventory
            .record_observation(LogicalTarget::CodexAuth, None)
            .is_err());
    }

    #[test]
    fn supplied_codex_snapshot_is_projected_without_additional_observation() {
        let settings = native_import_settings_from_observations(
            &AppType::Codex,
            [
                (
                    LogicalTarget::CodexAuth,
                    Some(br#"{"OPENAI_API_KEY":"sk-test"}"#.to_vec()),
                ),
                (
                    LogicalTarget::CodexConfig,
                    Some(b"model = \"gpt-5\"\n".to_vec()),
                ),
            ],
        )
        .expect("project complete in-memory Codex snapshot");

        assert_eq!(settings["auth"]["OPENAI_API_KEY"], "sk-test");
        assert_eq!(settings["config"], "model = \"gpt-5\"\n");

        let incomplete = native_import_settings_from_observations(
            &AppType::Codex,
            [(LogicalTarget::CodexAuth, Some(b"{}".to_vec()))],
        )
        .expect_err("an incomplete supplied snapshot must not trigger filesystem I/O");
        assert!(incomplete
            .to_string()
            .contains("not in the supplied snapshot"));
    }

    #[test]
    fn core_adapter_keeps_the_cli_claude_json_write_contract() {
        let home = tempfile::tempdir().expect("temporary home");
        let _env = TestEnvGuard::isolated(home.path());
        let path = target_path(LogicalTarget::ClaudeSettings).expect("Claude path");
        std::fs::create_dir_all(path.parent().expect("settings parent"))
            .expect("create settings parent");
        std::fs::write(&path, b"{\"old\":true}").expect("seed Claude settings");
        let settings = json!({
            "env": {"ANTHROPIC_AUTH_TOKEN": "secret"},
            "model": "claude-sonnet"
        });

        execute_claude_settings_with_adapter_under_lock(&settings).expect("execute Core plan");

        let expected = serde_json::to_string_pretty(&settings).expect("serialize expected JSON");
        assert_eq!(
            std::fs::read(&path).expect("read Claude settings"),
            expected.as_bytes()
        );
    }

    #[test]
    fn core_receipt_rollback_preserves_a_later_external_edit() {
        let home = tempfile::tempdir().expect("temporary home");
        let _env = TestEnvGuard::isolated(home.path());
        let path = target_path(LogicalTarget::ClaudeSettings).expect("Claude path");
        crate::config::write_json_file(&path, &json!({"model": "before"}))
            .expect("write original settings");

        let write = execute_claude_settings_with_adapter_under_lock(&json!({"model": "after"}))
            .expect("execute Core plan");
        let ClaudeSettingsWrite::Core(receipt) = write else {
            panic!("bounded object should use the Core executor");
        };
        let external = br#"{"model":"external"}"#;
        std::fs::write(&path, external).expect("write external edit");

        receipt
            .rollback()
            .expect_err("guarded rollback must reject the external edit");
        assert_eq!(
            std::fs::read(&path).expect("read preserved external edit"),
            external
        );
    }

    #[test]
    fn core_executor_writes_prepared_codex_bytes_without_reprojection() {
        let home = tempfile::tempdir().expect("temporary home");
        let _env = TestEnvGuard::isolated(home.path());
        let auth = json!({"OPENAI_API_KEY": "secret"});
        let catalog = json!({"models": [{"slug": "model-a"}]});
        let prepared = PreparedLiveWrite::Codex {
            auth: PreparedCodexAuthWrite::Write(auth.clone()),
            config: crate::codex_config::PreparedCodexConfigText {
                config_text: "model = \"model-a\"\n".to_string(),
                model_catalog: Some(catalog.clone()),
            },
        };

        let write = execute_prepared_codex_under_lock(&prepared).expect("execute Core plan");
        assert!(matches!(write, CodexPreparedWrite::Core(_)));
        assert_eq!(
            std::fs::read(crate::codex_config::get_codex_auth_path()).expect("read auth"),
            serde_json::to_string_pretty(&auth)
                .expect("serialize auth")
                .as_bytes()
        );
        assert_eq!(
            std::fs::read(crate::codex_config::get_codex_config_path()).expect("read config"),
            b"model = \"model-a\"\n"
        );
        assert_eq!(
            std::fs::read(crate::codex_config::get_codex_model_catalog_path())
                .expect("read catalog"),
            serde_json::to_string_pretty(&catalog)
                .expect("serialize catalog")
                .as_bytes()
        );
    }

    #[test]
    fn core_executor_preserves_codex_auth_when_prepared_to_do_so() {
        let home = tempfile::tempdir().expect("temporary home");
        let _env = TestEnvGuard::isolated(home.path());
        let auth_path = crate::codex_config::get_codex_auth_path();
        crate::config::atomic_write(&auth_path, b"oauth-cache").expect("seed auth cache");
        let prepared = PreparedLiveWrite::Codex {
            auth: PreparedCodexAuthWrite::Preserve,
            config: crate::codex_config::PreparedCodexConfigText {
                config_text: "model = \"model-a\"\n".to_string(),
                model_catalog: None,
            },
        };

        let write = execute_prepared_codex_under_lock(&prepared).expect("execute Core plan");
        assert!(matches!(write, CodexPreparedWrite::Core(_)));
        assert_eq!(
            std::fs::read(auth_path).expect("read auth cache"),
            b"oauth-cache"
        );
    }

    #[test]
    fn oversized_prepared_codex_write_requests_compatibility_without_writing() {
        let home = tempfile::tempdir().expect("temporary home");
        let _env = TestEnvGuard::isolated(home.path());
        let config_path = crate::codex_config::get_codex_config_path();
        crate::config::atomic_write(&config_path, b"before").expect("seed config");
        let prepared = PreparedLiveWrite::Codex {
            auth: PreparedCodexAuthWrite::Preserve,
            config: crate::codex_config::PreparedCodexConfigText {
                config_text: "x".repeat(MAX_OPERATION_CONTENT_BYTES + 1),
                model_catalog: None,
            },
        };

        let write = execute_prepared_codex_under_lock(&prepared).expect("inspect prepared write");
        assert!(matches!(write, CodexPreparedWrite::Compatibility));
        assert_eq!(
            std::fs::read(config_path).expect("read unchanged config"),
            b"before"
        );
    }

    #[test]
    fn cli_operation_host_compare_exchange_preserves_external_contents() {
        let home = tempfile::tempdir().expect("temporary home");
        let _env = TestEnvGuard::isolated(home.path());
        let target = LogicalTarget::ClaudeSettings;
        let path = target_path(target).expect("Claude path");
        std::fs::create_dir_all(path.parent().expect("settings parent"))
            .expect("create settings parent");
        std::fs::write(&path, b"external").expect("seed external edit");
        let mut host = CliOperationHost;

        let outcome = host
            .compare_exchange(&path, Some(b"stale"), Some(b"replacement"))
            .expect("conditional exchange");

        assert_eq!(outcome, CompareExchangeOutcome::Conflict);
        assert_eq!(
            std::fs::read(&path).expect("read preserved edit"),
            b"external"
        );
    }

    #[test]
    fn core_executor_falls_back_for_legacy_oversized_claude_inputs() {
        let home = tempfile::tempdir().expect("temporary home");
        let _env = TestEnvGuard::isolated(home.path());
        let path = target_path(LogicalTarget::ClaudeSettings).expect("Claude path");
        std::fs::create_dir_all(path.parent().expect("settings parent"))
            .expect("create settings parent");
        std::fs::write(&path, vec![b'x'; MAX_OPERATION_CONTENT_BYTES + 1])
            .expect("seed oversized input");
        let settings = json!({"replacement": true});

        execute_claude_settings_under_lock(&settings).expect("use legacy compatibility writer");

        let expected = serde_json::to_string_pretty(&settings).expect("serialize expected JSON");
        assert_eq!(
            std::fs::read(&path).expect("read replacement"),
            expected.as_bytes()
        );
    }

    #[test]
    fn core_executor_falls_back_for_legacy_oversized_claude_outputs() {
        let home = tempfile::tempdir().expect("temporary home");
        let _env = TestEnvGuard::isolated(home.path());
        let bounded_settings =
            serde_json::Value::String("x".repeat(MAX_OPERATION_CONTENT_BYTES - 2));
        let bounded =
            serde_json::to_string_pretty(&bounded_settings).expect("serialize bounded JSON");
        assert_eq!(bounded.len(), MAX_OPERATION_CONTENT_BYTES);
        execute_claude_settings_under_lock(&bounded_settings).expect("execute bounded Core plan");

        let path = target_path(LogicalTarget::ClaudeSettings).expect("Claude path");
        assert_eq!(
            std::fs::read(&path).expect("read bounded replacement"),
            bounded.as_bytes()
        );

        let settings = serde_json::Value::String("x".repeat(MAX_OPERATION_CONTENT_BYTES));

        execute_claude_settings_under_lock(&settings).expect("use legacy compatibility writer");

        let expected = serde_json::to_string_pretty(&settings).expect("serialize expected JSON");
        assert!(expected.len() > MAX_OPERATION_CONTENT_BYTES);
        assert_eq!(
            std::fs::read(&path).expect("read oversized replacement"),
            expected.as_bytes()
        );
    }

    #[test]
    fn core_executor_falls_back_for_exact_limit_claude_object() {
        let home = tempfile::tempdir().expect("temporary home");
        let _env = TestEnvGuard::isolated(home.path());
        let empty = json!({"payload": ""});
        let overhead = serde_json::to_string_pretty(&empty)
            .expect("serialize empty object")
            .len();
        let settings = json!({
            "payload": "x".repeat(MAX_OPERATION_CONTENT_BYTES - overhead)
        });
        let expected = serde_json::to_string_pretty(&settings).expect("serialize exact-limit JSON");
        assert_eq!(expected.len(), MAX_OPERATION_CONTENT_BYTES);

        execute_claude_settings_with_adapter_under_lock(&settings)
            .expect("use exact-limit compatibility writer");

        let path = target_path(LogicalTarget::ClaudeSettings).expect("Claude path");
        assert_eq!(
            std::fs::read(&path).expect("read exact-limit replacement"),
            expected.as_bytes()
        );
    }

    #[cfg(unix)]
    #[test]
    fn core_executor_preserves_legacy_unreadable_file_replacement() {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir().expect("temporary home");
        let _env = TestEnvGuard::isolated(home.path());
        let path = target_path(LogicalTarget::ClaudeSettings).expect("Claude path");
        std::fs::create_dir_all(path.parent().expect("settings parent"))
            .expect("create settings parent");
        std::fs::write(&path, b"old").expect("seed unreadable input");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))
            .expect("remove read permission");
        let settings = json!({"replacement": true});

        execute_claude_settings_under_lock(&settings).expect("replace unreadable legacy file");

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("restore read permission");
        let expected = serde_json::to_string_pretty(&settings).expect("serialize expected JSON");
        assert_eq!(
            std::fs::read(&path).expect("read replacement"),
            expected.as_bytes()
        );
    }

    #[test]
    fn cli_operation_host_reports_oversized_reads_without_returning_partial_bytes() {
        let home = tempfile::tempdir().expect("temporary home");
        let _env = TestEnvGuard::isolated(home.path());
        let path = target_path(LogicalTarget::ClaudeSettings).expect("Claude path");
        std::fs::create_dir_all(path.parent().expect("settings parent"))
            .expect("create settings parent");
        std::fs::write(&path, b"12345").expect("seed oversized input");
        let mut host = CliOperationHost;

        let observed = host.read(&path, 4).expect("bounded read");

        assert!(matches!(observed, OperationRead::TooLarge));
    }
}
