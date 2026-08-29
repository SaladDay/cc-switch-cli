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
    builtin_app_adapter, AppType as CoreAppType, LiveDocumentSet, LogicalTarget, ObservedDocument,
    ProviderSnapshot, MAX_OPERATION_CONTENT_BYTES,
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

fn read_optional_bounded(path: &Path) -> Result<Option<Vec<u8>>, AppError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(AppError::io(path, error)),
    };
    let mut bytes = Vec::new();
    file.take(MAX_OPERATION_CONTENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| AppError::io(path, error))?;
    if bytes.len() > MAX_OPERATION_CONTENT_BYTES {
        return Err(AppError::InvalidInput(format!(
            "Live configuration exceeds the {}-byte Core input limit: {}",
            MAX_OPERATION_CONTENT_BYTES,
            path.display()
        )));
    }
    Ok(Some(bytes))
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
}
