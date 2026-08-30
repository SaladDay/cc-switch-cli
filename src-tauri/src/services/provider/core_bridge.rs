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
    builtin_app_adapter, execute_operation_plan, AppType as CoreAppType, CompareExchangeOutcome,
    ContentExpectation, LiveDocumentSet, LogicalTarget, NativeImportStep, ObservedDocument,
    OperationExecutionError, OperationFailure, OperationHost, OperationPlan, OperationRead,
    PlannedWrite, ProviderSnapshot, MAX_OPERATION_CONTENT_BYTES, OPERATION_CONTRACT_MAJOR,
};

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

/// Uses the shared executor while retaining the CLI's established JSON bytes
/// and host-owned path and filesystem behavior. The caller must hold the
/// per-application switch lock; ordinary filesystems cannot exclude writers
/// that ignore that synchronization protocol.
pub(super) fn execute_claude_settings_under_lock(
    settings: &serde_json::Value,
) -> Result<(), AppError> {
    let contents = serde_json::to_string_pretty(settings)
        .map_err(|source| AppError::JsonSerialize { source })?;
    let target = LogicalTarget::ClaudeSettings;
    let mut host = CliOperationHost;
    let resource = host.resolve(target)?;
    let original = match host.read(&resource, MAX_OPERATION_CONTENT_BYTES)? {
        OperationRead::Missing => None,
        OperationRead::Contents(contents) => Some(contents),
        OperationRead::TooLarge => {
            return Err(oversized_live_config_error(
                &resource,
                MAX_OPERATION_CONTENT_BYTES,
            ))
        }
    };
    let plan = OperationPlan {
        contract_major: OPERATION_CONTRACT_MAJOR,
        app_id: CoreAppType::Claude.as_str().to_owned(),
        writes: vec![PlannedWrite {
            target,
            expected: ContentExpectation::for_contents(original.as_deref()),
            contents: Some(contents),
        }],
    };

    execute_operation_plan(&plan, &mut host)
        .map(drop)
        .map_err(map_execution_error)
}

fn map_execution_error(error: OperationExecutionError<AppError>) -> AppError {
    if error.rollback_failures().is_empty() {
        if let OperationFailure::Conflict { target } = error.failure() {
            return AppError::Conflict(format!(
                "Core live target {target:?} changed while the write was being prepared"
            ));
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
    fn core_executor_keeps_the_cli_claude_json_write_contract() {
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

        execute_claude_settings_under_lock(&settings).expect("execute Core plan");

        let expected = serde_json::to_string_pretty(&settings).expect("serialize expected JSON");
        assert_eq!(
            std::fs::read(&path).expect("read Claude settings"),
            expected.as_bytes()
        );
    }

    #[test]
    fn cli_operation_host_reports_stale_core_preconditions_without_overwriting() {
        let home = tempfile::tempdir().expect("temporary home");
        let _env = TestEnvGuard::isolated(home.path());
        let target = LogicalTarget::ClaudeSettings;
        let path = target_path(target).expect("Claude path");
        std::fs::create_dir_all(path.parent().expect("settings parent"))
            .expect("create settings parent");
        std::fs::write(&path, b"external").expect("seed external edit");
        let plan = OperationPlan {
            contract_major: OPERATION_CONTRACT_MAJOR,
            app_id: CoreAppType::Claude.as_str().to_owned(),
            writes: vec![PlannedWrite {
                target,
                expected: ContentExpectation::for_contents(Some(b"stale")),
                contents: Some("{}".to_owned()),
            }],
        };
        let mut host = CliOperationHost;

        let error = execute_operation_plan(&plan, &mut host).expect_err("stale precondition");

        assert!(matches!(
            error.failure(),
            OperationFailure::Conflict {
                target: LogicalTarget::ClaudeSettings
            }
        ));
        assert_eq!(
            std::fs::read(&path).expect("read preserved edit"),
            b"external"
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
