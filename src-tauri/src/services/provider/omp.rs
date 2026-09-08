use super::{ProviderService, SwitchResult};
use crate::app_config::AppType;
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta, UsageScript};
use crate::store::AppState;
use indexmap::IndexMap;
use serde_json::Value;

const OMP_APP: &str = "omp";

pub(super) fn list(state: &AppState) -> Result<IndexMap<String, Provider>, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app(OMP_APP));
    match crate::omp_config::read_omp_native_providers() {
        Ok(native) => {
            if let Err(error) = sync_native_locked(state, &native) {
                log::warn!("Failed to sync OMP providers from native config: {error}");
            }
        }
        Err(error) => {
            log::warn!("Failed to read OMP providers; showing saved catalog: {error}");
        }
    }
    state.db.get_all_providers(OMP_APP)
}

pub(super) fn import_from_live(state: &AppState) -> Result<usize, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app(OMP_APP));
    let native = crate::omp_config::read_omp_native_providers()?;
    sync_native_locked(state, &native)
}

pub(super) fn add(
    state: &AppState,
    mut provider: Provider,
    add_to_live: bool,
) -> Result<bool, AppError> {
    let app_type = AppType::Omp;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));
    strip_unsupported_omp_metadata(&mut provider);
    ProviderService::validate_provider_settings(&app_type, &provider)?;
    if add_to_live {
        crate::omp_config::validate_provider_for_live_write(
            &provider.id,
            &provider.settings_config,
        )?;
    }
    ProviderService::normalize_usage_script_credential_overrides(&app_type, &mut provider);

    if state
        .db
        .get_provider_by_id(&provider.id, app_type.as_str())?
        .is_some()
    {
        return Err(AppError::InvalidInput(format!(
            "OMP provider '{}' already exists",
            provider.id
        )));
    }

    if !add_to_live && crate::omp_config::omp_provider_exists(&provider.id)? {
        return Err(AppError::InvalidInput(format!(
            "OMP provider key '{}' already exists in models.yml",
            provider.id
        )));
    }

    let native_inserted = if add_to_live {
        crate::omp_config::insert_omp_provider(&provider.id, &provider.settings_config)?
    } else {
        false
    };

    if let Err(error) = state.db.save_provider(app_type.as_str(), &provider) {
        if native_inserted {
            if let Err(rollback) = crate::omp_config::remove_omp_provider_if_matches(
                &provider.id,
                &provider.settings_config,
            ) {
                return Err(AppError::Config(format!(
                    "failed to save OMP provider: {error}; native rollback failed: {rollback}"
                )));
            }
        }
        return Err(error);
    }
    Ok(true)
}

pub(super) fn update_usage_script(
    state: &AppState,
    id: &str,
    script: UsageScript,
) -> Result<bool, AppError> {
    let app_type = AppType::Omp;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));
    ProviderService::validate_usage_script(&script)?;

    let mut provider = state
        .db
        .get_provider_by_id(id, app_type.as_str())?
        .ok_or_else(|| AppError::InvalidInput(format!("OMP provider '{id}' not found")))?;
    provider
        .meta
        .get_or_insert_with(ProviderMeta::default)
        .usage_script = Some(script);
    strip_unsupported_omp_metadata(&mut provider);
    ProviderService::normalize_usage_script_credential_overrides(&app_type, &mut provider);
    state.db.save_provider(app_type.as_str(), &provider)?;
    Ok(true)
}

pub(super) fn clear_usage_script(state: &AppState, id: &str) -> Result<bool, AppError> {
    let app_type = AppType::Omp;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));

    let mut provider = state
        .db
        .get_provider_by_id(id, app_type.as_str())?
        .ok_or_else(|| AppError::InvalidInput(format!("OMP provider '{id}' not found")))?;
    if let Some(meta) = provider.meta.as_mut() {
        meta.usage_script = None;
    }
    strip_unsupported_omp_metadata(&mut provider);
    state.db.save_provider(app_type.as_str(), &provider)?;
    Ok(true)
}

pub(super) fn update(
    state: &AppState,
    original_id: Option<&str>,
    mut provider: Provider,
) -> Result<bool, AppError> {
    let app_type = AppType::Omp;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));
    let original_id = original_id.unwrap_or(&provider.id).to_string();
    if original_id != provider.id {
        return Err(AppError::InvalidInput(
            "OMP provider keys cannot be renamed".to_string(),
        ));
    }

    let existing = state
        .db
        .get_provider_by_id(&original_id, app_type.as_str())?
        .ok_or_else(|| AppError::InvalidInput(format!("OMP provider '{original_id}' not found")))?;
    strip_unsupported_omp_metadata(&mut provider);
    // Ordinary CC Switch-created providers use strict OMP semantic checks.
    // A provider imported from OMP may intentionally be opaque to us (for
    // example a built-in node containing only future fields); allow edits to
    // such an existing entry while retaining all typed-field validation.
    // Only extension-owned native nodes are opaque. A malformed ordinary
    // provider must not be able to bypass OMP's semantic requirements merely
    // because its current persisted form is invalid.
    let allow_opaque_update = is_opaque_extension_config(&existing.settings_config);
    let strict_validation = ProviderService::validate_provider_settings(&app_type, &provider);
    if let Err(error) = strict_validation {
        if !allow_opaque_update {
            return Err(error);
        }
        crate::omp_config::validate_provider_node_for_import(
            &original_id,
            &provider.settings_config,
        )?;
        if let Some(meta) = provider.meta.as_ref() {
            if let Some(script) = meta.usage_script.as_ref() {
                ProviderService::validate_usage_script(script)?;
            }
        }
    }
    ProviderService::normalize_usage_script_credential_overrides(&app_type, &mut provider);

    // Compare-and-swap against the native value that was imported into the
    // database. If a user edited models.yml after loading the provider, fail
    // with Conflict instead of silently replacing that external change.
    let previous_native = if crate::omp_config::read_omp_native_provider(&original_id)?.is_some() {
        if allow_opaque_update {
            crate::omp_config::replace_omp_provider_if_present_for_import_checked(
                &original_id,
                &existing.settings_config,
                &provider.settings_config,
            )?
        } else {
            crate::omp_config::replace_omp_provider_if_present_checked(
                &original_id,
                &existing.settings_config,
                &provider.settings_config,
            )?
        }
    } else {
        None
    };
    if let Err(error) = state.db.save_provider(app_type.as_str(), &provider) {
        if let Some(previous_native) = previous_native.as_ref() {
            let rollback_result = if allow_opaque_update {
                crate::omp_config::replace_omp_provider_for_import(
                    &original_id,
                    &provider.settings_config,
                    previous_native,
                )
            } else {
                crate::omp_config::replace_omp_provider(
                    &original_id,
                    &provider.settings_config,
                    previous_native,
                )
            };
            if let Err(rollback) = rollback_result {
                return Err(AppError::Config(format!(
                    "failed to save OMP provider: {error}; native rollback failed: {rollback}"
                )));
            }
        }
        return Err(error);
    }
    Ok(true)
}

pub(super) fn delete(state: &AppState, id: &str) -> Result<(), AppError> {
    let app_type = AppType::Omp;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));
    let Some(provider) = state.db.get_provider_by_id(id, app_type.as_str())? else {
        return Ok(());
    };
    // Delete is intentionally keyed by provider ID. Once the user confirms
    // deleting the provider itself, supported field edits do not change that
    // intent; the latest native value is retained only for rollback.
    let removed = crate::omp_config::remove_omp_provider_checked(id, &provider.settings_config)?;

    if let Err(error) = state.db.delete_provider(app_type.as_str(), id) {
        if let Some(removed) = removed.as_ref() {
            if let Err(rollback) = crate::omp_config::restore_omp_provider_if_missing(id, removed) {
                return Err(AppError::Config(format!(
                    "failed to delete OMP provider: {error}; native rollback failed: {rollback}"
                )));
            }
        }
        return Err(error);
    }
    Ok(())
}

pub(super) fn remove(state: &AppState, id: &str) -> Result<(), AppError> {
    let app_type = AppType::Omp;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));
    let provider = state
        .db
        .get_provider_by_id(id, app_type.as_str())?
        .ok_or_else(|| AppError::InvalidInput(format!("OMP provider '{id}' not found")))?;
    // Removing a provider from an additive live config intentionally captures
    // the latest native node and stores it in the catalog for restoration. It
    // therefore does not use the destructive delete CAS path: external edits
    // are preserved rather than silently discarded.
    let Some(removed) = crate::omp_config::remove_omp_provider(id)? else {
        return Ok(());
    };
    let mut synced = provider;
    merge_native_config(&mut synced, removed.clone());
    if let Err(error) = state.db.save_provider(app_type.as_str(), &synced) {
        if let Err(rollback) = crate::omp_config::restore_omp_provider_if_missing(id, &removed) {
            return Err(AppError::Config(format!(
                "failed to preserve OMP provider before removal: {error}; native rollback failed: {rollback}"
            )));
        }
        return Err(error);
    }
    Ok(())
}

pub(super) fn enable(state: &AppState, id: &str) -> Result<SwitchResult, AppError> {
    let app_type = AppType::Omp;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));
    let provider = state
        .db
        .get_provider_by_id(id, app_type.as_str())?
        .ok_or_else(|| AppError::InvalidInput(format!("OMP provider '{id}' not found")))?;
    let was_disabled = crate::omp_config::read_omp_disabled_providers()?.contains(id);

    if let Some(native) = crate::omp_config::read_omp_native_provider(id)? {
        // A native entry may have been edited externally since it was last
        // imported. Validate it before treating the provider as enabled: the
        // import-only validator is reserved for extension-owned opaque nodes,
        // while ordinary providers must still satisfy OMP's semantic rules.
        if is_opaque_extension_config(&native) {
            crate::omp_config::validate_provider_node_for_import(id, &native)?;
        } else {
            crate::omp_config::validate_provider_node(id, &native)?;
        }
        // OMP applies disabledProviders before credential resolution. Restoring
        // a models.yml entry alone would therefore still leave the provider
        // unusable, so enabling also clears the effective disable entry.
        crate::omp_config::set_omp_provider_disabled(id, false)?;
        let mut synced = provider;
        merge_native_config(&mut synced, native);
        if let Err(error) = state.db.save_provider(app_type.as_str(), &synced) {
            if was_disabled {
                if let Err(rollback) = crate::omp_config::set_omp_provider_disabled(id, true) {
                    return Err(AppError::Config(format!(
                        "failed to save enabled OMP provider: {error}; disabledProviders rollback failed: {rollback}"
                    )));
                }
            }
            return Err(error);
        }
        return Ok(SwitchResult::default());
    }

    // Existing DB entries may have originated from an OMP-native node that is
    // opaque to CC Switch (for example a built-in provider with only
    // forward-compatible fields). Restore that exact node instead of applying
    // the stricter custom-provider requirements used for new live entries.
    if is_opaque_extension_config(&provider.settings_config) {
        crate::omp_config::validate_provider_node_for_import(
            &provider.id,
            &provider.settings_config,
        )?;
    } else {
        crate::omp_config::validate_provider_node(&provider.id, &provider.settings_config)?;
    }
    crate::omp_config::set_omp_provider_disabled(id, false)?;
    if let Err(error) =
        crate::omp_config::restore_omp_provider_if_missing(id, &provider.settings_config)
    {
        if was_disabled {
            if let Err(rollback) = crate::omp_config::set_omp_provider_disabled(id, true) {
                return Err(AppError::Config(format!(
                    "failed to restore OMP provider: {error}; disabledProviders rollback failed: {rollback}"
                )));
            }
        }
        return Err(error);
    }
    Ok(SwitchResult::default())
}

fn sync_native_locked(
    state: &AppState,
    native: &IndexMap<String, Value>,
) -> Result<usize, AppError> {
    // Import providers independently. Native OMP files may contain entries
    // owned by extensions (or stale/malformed entries); one bad node should
    // not hide every other provider from the CC Switch catalog.
    let saved = state.db.get_all_providers(OMP_APP)?;
    let mut changed = 0;

    for (id, config) in native {
        if let Err(error) = crate::omp_config::validate_provider_node_for_import(id, config) {
            log::warn!("Skipping invalid OMP provider '{id}' from models.yml: {error}");
            continue;
        }
        let mut provider = saved.get(id).cloned().unwrap_or_else(|| {
            let name = native_provider_name(config).unwrap_or(id).to_string();
            let mut imported = Provider::with_id(id.clone(), name, config.clone(), None);
            imported.category = Some("custom".to_string());
            imported.icon = Some("omp".to_string());
            imported
        });
        let is_new = !saved.contains_key(id);
        let previous_name = provider.name.clone();
        let previous_config = provider.settings_config.clone();
        merge_native_config(&mut provider, config.clone());
        if !is_new && provider.name == previous_name && provider.settings_config == previous_config
        {
            continue;
        }

        state.db.save_provider(OMP_APP, &provider)?;
        changed += 1;
    }

    Ok(changed)
}

fn merge_native_config(provider: &mut Provider, config: Value) {
    if let Some(name) = native_provider_name(&config) {
        provider.name = name.to_string();
    }
    provider.settings_config = config;
    // OMP's documented provider schema has no provider-level `name`; for
    // normal custom entries the CC Switch catalog owns the display name and
    // keeps it out of models.yml.  Keep the field on opaque extension-owned
    // nodes, though, so remove/enable round-trips do not silently lose data
    // that CC Switch cannot interpret.
    strip_managed_native_name(&mut provider.settings_config);
}

fn native_provider_name(config: &Value) -> Option<&str> {
    config
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
}

fn strip_native_name(config: &mut Value) {
    if let Some(object) = config.as_object_mut() {
        object.remove("name");
    }
}

pub(crate) fn is_opaque_extension_config(config: &Value) -> bool {
    config
        .as_object()
        .is_some_and(|object| object.contains_key("extension"))
}

fn strip_managed_native_name(config: &mut Value) {
    if !is_opaque_extension_config(config) {
        strip_native_name(config);
    }
}

fn strip_unsupported_omp_metadata(provider: &mut Provider) {
    strip_managed_native_name(&mut provider.settings_config);
    provider.in_failover_queue = false;
    let Some(meta) = provider.meta.take() else {
        return;
    };
    provider.meta = Some(ProviderMeta {
        usage_script: meta.usage_script,
        is_partner: meta.is_partner,
        partner_promotion_key: meta.partner_promotion_key,
        ..ProviderMeta::default()
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::omp_config::test_support::TestAgentDir;
    use crate::provider::ProviderMeta;
    use serde_json::json;
    use serial_test::serial;
    use std::fs;
    use std::sync::Arc;

    fn state() -> AppState {
        AppState::new(Arc::new(
            Database::memory().expect("create in-memory database"),
        ))
    }

    fn input(model_id: &str) -> Provider {
        Provider {
            id: "cc-switch-test".to_string(),
            name: "Test provider".to_string(),
            settings_config: json!({
                "baseUrl": "https://api.example.com/v1",
                "apiKey": "secret",
                "api": "openai-completions",
                "models": [{ "id": model_id }]
            }),
            website_url: None,
            category: Some("custom".to_string()),
            created_at: Some(1),
            sort_index: None,
            notes: None,
            meta: Some(ProviderMeta {
                endpoint_auto_select: Some(true),
                live_config_managed: Some(false),
                api_format: Some("openai_chat".to_string()),
                custom_user_agent: Some("legacy-route-agent".to_string()),
                is_partner: Some(true),
                ..ProviderMeta::default()
            }),
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    fn usage_script(code: &str) -> UsageScript {
        UsageScript {
            enabled: true,
            language: "javascript".to_string(),
            code: code.to_string(),
            timeout: Some(5),
            api_key: None,
            base_url: None,
            access_token: None,
            user_id: None,
            template_type: None,
            auto_query_interval: Some(10),
            coding_plan_provider: None,
        }
    }

    fn secure_test_dir(path: &std::path::Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                .expect("restrict OMP test directory permissions");
        }
    }

    #[test]
    #[serial]
    fn membership_is_derived_only_from_models_yml() {
        let _agent = TestAgentDir::new();
        let state = state();

        add(&state, input("model-a"), false).expect("save disabled provider");
        assert!(!crate::omp_config::omp_provider_exists("cc-switch-test").unwrap());

        let saved = state
            .db
            .get_provider_by_id("cc-switch-test", "omp")
            .unwrap()
            .unwrap();
        let meta = saved.meta.unwrap_or_default();
        assert_eq!(meta.live_config_managed, None);
        assert_eq!(meta.endpoint_auto_select, None);
        assert_eq!(meta.api_format, None);
        assert_eq!(meta.custom_user_agent, None);
        assert_eq!(meta.is_partner, Some(true));

        ProviderService::switch(&state, AppType::Omp, "cc-switch-test").expect("enable provider");
        assert!(crate::omp_config::omp_provider_exists("cc-switch-test").unwrap());

        ProviderService::remove_from_live_config(&state, AppType::Omp, "cc-switch-test")
            .expect("remove provider");
        assert!(!crate::omp_config::omp_provider_exists("cc-switch-test").unwrap());
        assert!(state
            .db
            .get_provider_by_id("cc-switch-test", "omp")
            .unwrap()
            .is_some());
    }

    #[test]
    #[serial]
    fn delete_rejects_external_native_provider_edits() {
        let _agent = TestAgentDir::new();
        let state = state();
        add(&state, input("model-a"), true).expect("create native provider");

        let path = crate::omp_config::get_omp_models_path().expect("models path");
        let changed = fs::read_to_string(&path)
            .expect("read native models")
            .replace("apiKey: secret", "apiKey: external-secret");
        fs::write(&path, changed).expect("edit native provider externally");

        let error = delete(&state, "cc-switch-test")
            .expect_err("delete must detect an external native edit");
        assert!(matches!(error, AppError::Conflict(_)));
        assert!(crate::omp_config::omp_provider_exists("cc-switch-test").unwrap());
        assert!(state
            .db
            .get_provider_by_id("cc-switch-test", OMP_APP)
            .unwrap()
            .is_some());
    }

    #[test]
    #[serial]
    fn remove_preserves_latest_external_native_provider_edits() {
        let _agent = TestAgentDir::new();
        let state = state();
        add(&state, input("model-a"), true).expect("create native provider");

        let path = crate::omp_config::get_omp_models_path().expect("models path");
        let changed = fs::read_to_string(&path)
            .expect("read native models")
            .replace("apiKey: secret", "apiKey: external-secret");
        fs::write(&path, changed).expect("edit native provider externally");

        remove(&state, "cc-switch-test").expect("remove should preserve the latest native value");
        assert!(!crate::omp_config::omp_provider_exists("cc-switch-test").unwrap());
        let saved = state
            .db
            .get_provider_by_id("cc-switch-test", OMP_APP)
            .unwrap()
            .unwrap();
        assert_eq!(
            saved.settings_config.get("apiKey"),
            Some(&json!("external-secret"))
        );
    }

    #[test]
    #[serial]
    fn list_imports_opaque_extension_provider_without_models() {
        let _agent = TestAgentDir::new();
        let state = state();
        let config = json!({"extension": {"type": "custom", "version": 1}});
        let native = IndexMap::from([("extension-provider".to_string(), config.clone())]);

        let changed = sync_native_locked(&state, &native).expect("sync extension provider");
        assert_eq!(changed, 1);
        let provider = state
            .db
            .get_provider_by_id("extension-provider", OMP_APP)
            .expect("query imported extension provider")
            .expect("extension provider should be imported");
        assert_eq!(provider.settings_config, config);
    }

    #[test]
    #[serial]
    fn malformed_non_extension_provider_cannot_bypass_strict_update_validation() {
        let _agent = TestAgentDir::new();
        let state = state();
        let existing = Provider::with_id(
            "malformed".to_string(),
            "Malformed".to_string(),
            json!({"futureField": true}),
            None,
        );
        state
            .db
            .save_provider(OMP_APP, &existing)
            .expect("save malformed provider");

        let replacement = Provider::with_id(
            "malformed".to_string(),
            "Edited".to_string(),
            json!({"futureField": true, "anotherField": true}),
            None,
        );
        let error = update(&state, Some("malformed"), replacement)
            .expect_err("ordinary malformed providers must remain strictly validated");
        assert!(matches!(error, AppError::InvalidInput(_)));
        assert!(!crate::omp_config::omp_provider_exists("malformed").unwrap());
    }

    #[test]
    #[serial]
    fn malformed_non_extension_provider_cannot_be_enabled() {
        let _agent = TestAgentDir::new();
        let state = state();
        let existing = Provider::with_id(
            "malformed-enable".to_string(),
            "Malformed".to_string(),
            json!({"futureField": true}),
            None,
        );
        state
            .db
            .save_provider(OMP_APP, &existing)
            .expect("save malformed provider");

        let error = enable(&state, "malformed-enable")
            .expect_err("ordinary malformed providers must fail strict enable validation");
        assert!(matches!(error, AppError::InvalidInput(_)));
        assert!(!crate::omp_config::omp_provider_exists("malformed-enable").unwrap());
    }

    #[test]
    #[serial]
    fn malformed_native_non_extension_provider_cannot_be_enabled() {
        let _agent = TestAgentDir::new();
        let state = state();
        let path = crate::omp_config::get_omp_models_path().expect("OMP models path");
        fs::create_dir_all(path.parent().expect("OMP models parent")).expect("create OMP dir");
        fs::write(
            &path,
            "providers:\n  malformed-native-enable:\n    futureField: true\n",
        )
        .expect("write malformed native provider");

        let existing = Provider::with_id(
            "malformed-native-enable".to_string(),
            "Malformed native".to_string(),
            json!({"futureField": true}),
            None,
        );
        state
            .db
            .save_provider(OMP_APP, &existing)
            .expect("save malformed provider");

        let error = enable(&state, "malformed-native-enable")
            .expect_err("ordinary malformed native providers must fail strict enable validation");
        assert!(matches!(error, AppError::InvalidInput(_)));
    }

    #[test]
    #[serial]
    fn update_rejects_external_native_edits_before_overwrite() {
        let _agent = TestAgentDir::new();
        let state = state();
        let baseline = input("model-a");
        add(&state, baseline.clone(), true).expect("add provider");

        let mut external = baseline.settings_config.clone();
        external["apiKey"] = json!("rotated-outside");
        crate::omp_config::replace_omp_provider(
            "cc-switch-test",
            &baseline.settings_config,
            &external,
        )
        .expect("edit native provider externally");

        let replacement = input("model-b");
        let error = update(&state, Some("cc-switch-test"), replacement)
            .expect_err("external native edits must produce a conflict");
        assert!(matches!(error, AppError::Conflict(_)));
        assert_eq!(
            crate::omp_config::read_omp_native_provider("cc-switch-test")
                .expect("read native provider")
                .expect("native provider"),
            external
        );
    }

    #[test]
    #[serial]
    fn add_keeps_display_name_out_of_omp_native_schema() {
        let _agent = TestAgentDir::new();
        let state = state();
        let mut provider = input("model-a");
        provider.settings_config["name"] = json!("Display only");
        add(&state, provider, true).expect("add provider");
        let native = crate::omp_config::read_omp_native_provider("cc-switch-test")
            .unwrap()
            .unwrap();
        assert!(native.get("name").is_none());
        assert_eq!(
            state
                .db
                .get_provider_by_id("cc-switch-test", OMP_APP)
                .unwrap()
                .unwrap()
                .settings_config
                .get("name"),
            None
        );
    }

    #[test]
    #[serial]
    fn default_selection_does_not_block_membership_changes() {
        let _agent = TestAgentDir::new();
        let state = state();
        let original = input("model-a");
        add(&state, original.clone(), true).expect("add provider");
        let settings_path = crate::omp_config::get_omp_settings_path().unwrap();
        fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        fs::write(
            &settings_path,
            r#"{"defaultProvider":"cc-switch-test","defaultModel":"model-a"}"#,
        )
        .unwrap();

        update(&state, Some("cc-switch-test"), input("model-b"))
            .expect("global default must not block model edits");
        ProviderService::remove_from_live_config(&state, AppType::Omp, "cc-switch-test")
            .expect("global default must not block removal");
        assert!(!crate::omp_config::omp_provider_exists("cc-switch-test").unwrap());

        ProviderService::switch(&state, AppType::Omp, "cc-switch-test")
            .expect("re-enable provider");
        ProviderService::delete(&state, AppType::Omp, "cc-switch-test")
            .expect("global default must not block deletion");
        assert!(state
            .db
            .get_provider_by_id("cc-switch-test", "omp")
            .unwrap()
            .is_none());
        assert_eq!(
            fs::read_to_string(settings_path).unwrap(),
            r#"{"defaultProvider":"cc-switch-test","defaultModel":"model-a"}"#
        );
        assert!(!crate::omp_config::omp_provider_exists("cc-switch-test").unwrap());
    }

    #[test]
    #[serial]
    fn set_default_model_writes_omp_model_roles_without_removing_other_providers() {
        let _agent = TestAgentDir::new();
        let state = state();
        add(&state, input("model-a"), true).expect("add provider");
        crate::omp_config::insert_omp_provider(
            "other",
            &json!({
                "baseUrl": "https://other.example/v1",
                "api": "openai-completions",
                "apiKey": "secret",
                "models": [{"id": "other-model"}]
            }),
        )
        .expect("add second native provider");

        let selector = ProviderService::set_default_model(
            &state,
            AppType::Omp,
            "cc-switch-test",
            Some("model-a"),
        )
        .expect("set OMP default model");
        assert_eq!(selector, "cc-switch-test/model-a");
        assert_eq!(
            crate::omp_config::read_omp_model_roles()
                .expect("read roles")
                .get("default"),
            Some(&"cc-switch-test/model-a".to_string())
        );
        assert!(crate::omp_config::omp_provider_exists("other").expect("other provider"));
    }

    #[test]
    #[serial]
    fn provider_membership_never_changes_omp_auth_or_defaults() {
        let _agent = TestAgentDir::new();
        let state = state();
        let agent_dir = crate::omp_config::get_omp_agent_dir().expect("agent directory");
        fs::create_dir_all(&agent_dir).expect("create agent directory");
        secure_test_dir(&agent_dir);
        let auth_path = agent_dir.join("auth.json");
        let settings_path = agent_dir.join("settings.json");
        let auth_contents = br#"{
            "anthropic": {"type":"oauth","refresh":"native-secret"},
            "openai": {"type":"api_key","key":"native-api-key"}
        }"#;
        let settings_contents =
            br#"{"defaultProvider":"anthropic","defaultModel":"claude-opus-4-6"}"#;
        fs::write(&auth_path, auth_contents).expect("write auth");
        fs::write(&settings_path, settings_contents).expect("write settings");
        let models_path = agent_dir.join("models.yml");
        fs::write(
            &models_path,
            r#"{"providers":{"anthropic":{"baseUrl":"https://native.example/v1","futureField":{"keep":true}}}}"#,
        )
        .expect("write explicit provider");

        ProviderService::list(&state, AppType::Omp).expect("import explicit provider");
        ProviderService::remove_from_live_config(&state, AppType::Omp, "anthropic")
            .expect("remove explicit provider");
        ProviderService::switch(&state, AppType::Omp, "anthropic")
            .expect("enable explicit provider");
        let mut edited = state
            .db
            .get_provider_by_id("anthropic", OMP_APP)
            .expect("read provider")
            .expect("provider");
        edited.settings_config["anotherField"] = json!(true);
        update(&state, Some("anthropic"), edited).expect("edit explicit provider");

        assert_eq!(fs::read(auth_path).expect("read auth"), auth_contents);
        assert_eq!(
            fs::read(settings_path).expect("read settings"),
            settings_contents
        );
    }

    #[test]
    #[serial]
    fn failed_duplicate_create_rolls_back_native_insertion() {
        let _agent = TestAgentDir::new();
        let state = state();
        add(&state, input("model-a"), false).expect("save DB-only provider");

        assert!(add(&state, input("model-a"), true).is_err());
        assert!(!crate::omp_config::omp_provider_exists("cc-switch-test").unwrap());
    }

    #[test]
    #[serial]
    fn new_live_provider_requires_api_key_for_custom_models() {
        let _agent = TestAgentDir::new();
        let state = state();
        let mut provider = input("model-a");
        provider
            .settings_config
            .as_object_mut()
            .expect("provider object")
            .remove("apiKey");

        let error = add(&state, provider, true).expect_err("missing OMP API key");
        assert!(error.to_string().contains("requires apiKey"));
        assert!(!crate::omp_config::omp_provider_exists("cc-switch-test").unwrap());
        assert!(state
            .db
            .get_provider_by_id("cc-switch-test", OMP_APP)
            .expect("query provider")
            .is_none());
    }

    #[test]
    #[serial]
    fn native_edits_sync_to_the_saved_provider_and_survive_removal() {
        let _agent = TestAgentDir::new();
        let state = state();
        add(&state, input("model-a"), true).expect("add provider");
        let saved = state
            .db
            .get_provider_by_id("cc-switch-test", "omp")
            .unwrap()
            .unwrap();
        let mut external = saved.settings_config.clone();
        external["name"] = json!("External edit");
        external["models"][0]["contextWindow"] = json!(1_000_000.0);
        crate::omp_config::replace_omp_provider(
            "cc-switch-test",
            &saved.settings_config,
            &external,
        )
        .expect("edit native provider");

        let listed = ProviderService::list(&state, AppType::Omp).expect("sync native providers");
        assert_eq!(listed["cc-switch-test"].name, "External edit");
        let mut expected = external.clone();
        expected.as_object_mut().unwrap().remove("name");
        assert_eq!(listed["cc-switch-test"].settings_config, expected);

        ProviderService::remove_from_live_config(&state, AppType::Omp, "cc-switch-test")
            .expect("remove externally edited provider");
        assert!(!crate::omp_config::omp_provider_exists("cc-switch-test").unwrap());
        let preserved = state
            .db
            .get_provider_by_id("cc-switch-test", "omp")
            .unwrap()
            .unwrap();
        assert_eq!(preserved.name, "External edit");
        assert_eq!(preserved.settings_config, expected);
    }

    #[test]
    #[serial]
    fn refreshed_native_config_can_be_edited_without_snapshot_state() {
        let _agent = TestAgentDir::new();
        let state = state();
        let baseline = input("model-a");
        add(&state, baseline.clone(), true).expect("add provider");

        let mut external = baseline.settings_config.clone();
        external["apiKey"] = json!("rotated-outside");
        external["futureField"] = json!({ "preserve": true });
        crate::omp_config::replace_omp_provider(
            "cc-switch-test",
            &baseline.settings_config,
            &external,
        )
        .expect("edit native provider");

        let listed = ProviderService::list(&state, AppType::Omp).expect("refresh native provider");
        let mut local = listed["cc-switch-test"].clone();
        local.name = "Local edit".to_string();
        local.settings_config["name"] = json!("Local edit");
        update(&state, Some("cc-switch-test"), local).expect("edit the refreshed native provider");
        assert_eq!(
            crate::omp_config::read_omp_native_provider("cc-switch-test")
                .expect("read native provider")
                .expect("native provider")["futureField"],
            json!({ "preserve": true })
        );
    }

    #[test]
    #[serial]
    fn enabled_provider_edit_needs_no_special_snapshot_parameter() {
        let _agent = TestAgentDir::new();
        let state = state();
        add(&state, input("model-a"), true).expect("add provider");

        let mut edited = input("model-b");
        edited.settings_config["unknownField"] = json!({ "keep": true });
        update(&state, Some("cc-switch-test"), edited.clone()).expect("edit enabled provider");

        assert_eq!(
            crate::omp_config::read_omp_native_provider("cc-switch-test")
                .expect("read native provider")
                .expect("native provider"),
            edited.settings_config
        );
    }

    #[test]
    #[serial]
    fn native_sync_imports_every_explicit_provider_node() {
        let _agent = TestAgentDir::new();
        let state = state();
        let mut stale_oauth = input("stale-model");
        stale_oauth.id = "native-oauth".to_string();
        add(&state, stale_oauth, false).expect("save stale provider");
        let path = crate::omp_config::get_omp_models_path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            r#"{
                "providers": {
                    "native-custom": {
                        "name": "Native custom",
                        "baseUrl": "https://api.example.com/v1",
                        "apiKey": "secret",
                        "api": "openai-completions",
                        "models": [{ "id": "model-a" }]
                    },
                    "anthropic": {
                        "name": "Built in",
                        "baseUrl": "https://api.anthropic.com",
                        "api": "anthropic-messages",
                        "auth": "oauth",
                        "models": [{ "id": "claude" }]
                    },
                    "openai": {
                        "baseUrl": "https://api.openai.com/v1"
                    },
                    "deepseek": {
                        "futureField": { "preserve": true }
                    },
                    "native-oauth": {
                        "name": "OAuth",
                        "oauth": "example",
                        "baseUrl": "https://api.example.com/v1",
                        "api": "openai-completions",
                        "models": [{ "id": "model-b" }]
                    }
                }
            }"#,
        )
        .unwrap();

        let providers = ProviderService::list(&state, AppType::Omp).expect("sync providers");
        assert_eq!(providers.len(), 5);
        let imported = &providers["native-custom"];
        assert_eq!(imported.name, "Native custom");
        assert_eq!(imported.category.as_deref(), Some("custom"));
        assert_eq!(imported.icon.as_deref(), Some("omp"));
        assert_eq!(providers["anthropic"].name, "Built in");
        assert_eq!(
            providers["openai"].settings_config,
            json!({"baseUrl": "https://api.openai.com/v1"})
        );
        assert_eq!(
            providers["deepseek"].settings_config["futureField"],
            json!({ "preserve": true })
        );
        assert_eq!(
            providers["native-oauth"].settings_config["oauth"],
            json!("example")
        );
    }

    #[test]
    #[serial]
    fn removal_preserves_and_can_restore_a_minimal_native_node() {
        let _agent = TestAgentDir::new();
        let state = state();
        add(&state, input("model-a"), true).expect("add provider");
        let minimal = json!({
            "name": "Extension-owned provider",
            "extension": { "type": "custom" }
        });
        let path = crate::omp_config::get_omp_models_path().expect("models path");
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "providers": {
                    "cc-switch-test": minimal.clone()
                }
            }))
            .expect("serialize models"),
        )
        .expect("replace native provider");

        ProviderService::remove_from_live_config(&state, AppType::Omp, "cc-switch-test")
            .expect("remove exact native node");
        assert!(!crate::omp_config::omp_provider_exists("cc-switch-test").unwrap());
        let expected = minimal.clone();
        assert_eq!(
            state
                .db
                .get_provider_by_id("cc-switch-test", OMP_APP)
                .expect("read saved provider")
                .expect("saved provider")
                .settings_config,
            expected
        );

        ProviderService::switch(&state, AppType::Omp, "cc-switch-test")
            .expect("restore the complete native node");
        assert_eq!(
            crate::omp_config::read_omp_native_provider("cc-switch-test")
                .expect("read restored provider"),
            Some(expected)
        );
    }

    #[test]
    #[serial]
    fn usage_metadata_update_does_not_rewrite_native_provider_settings() {
        let _agent = TestAgentDir::new();
        let state = state();
        let baseline = input("model-a");
        add(&state, baseline.clone(), true).expect("add provider");

        let mut external = baseline.settings_config.clone();
        external["apiKey"] = json!("rotated-outside");
        external["futureField"] = json!({ "preserve": true });
        crate::omp_config::replace_omp_provider(
            "cc-switch-test",
            &baseline.settings_config,
            &external,
        )
        .expect("edit native provider");

        update_usage_script(&state, "cc-switch-test", usage_script("return {}"))
            .expect("save usage metadata");
        assert_eq!(
            crate::omp_config::read_omp_native_provider("cc-switch-test")
                .expect("read native provider")
                .expect("native provider"),
            external
        );

        let providers = ProviderService::list(&state, AppType::Omp).expect("sync provider");
        let saved = &providers["cc-switch-test"];
        assert_eq!(saved.settings_config, external);
        assert_eq!(
            saved
                .meta
                .as_ref()
                .and_then(|meta| meta.usage_script.as_ref())
                .map(|script| script.code.as_str()),
            Some("return {}")
        );

        clear_usage_script(&state, "cc-switch-test").expect("clear usage metadata");
        assert_eq!(
            crate::omp_config::read_omp_native_provider("cc-switch-test")
                .expect("read native provider"),
            Some(external)
        );
        assert!(state
            .db
            .get_provider_by_id("cc-switch-test", OMP_APP)
            .expect("read saved provider")
            .expect("saved provider")
            .meta
            .is_none_or(|meta| meta.usage_script.is_none()));
    }

    #[test]
    #[serial]
    fn coomped_provider_keeps_its_display_name_after_enable_and_sync() {
        let _agent = TestAgentDir::new();
        let state = state();
        let mut copy = input("model-a");
        copy.id = "cc-switch-test-copy".to_string();
        copy.name = "Test provider copy".to_string();

        add(&state, copy, false).expect("save coomped provider");
        ProviderService::switch(&state, AppType::Omp, "cc-switch-test-copy")
            .expect("enable coomped provider");
        let providers = ProviderService::list(&state, AppType::Omp).expect("sync providers");

        assert_eq!(providers["cc-switch-test-copy"].name, "Test provider copy");
        assert!(providers["cc-switch-test-copy"]
            .settings_config
            .get("name")
            .is_none());
    }

    #[test]
    #[serial]
    fn database_only_create_does_not_overwrite_an_unsynced_native_key() {
        let _agent = TestAgentDir::new();
        let state = state();
        let path = crate::omp_config::get_omp_models_path().expect("models path");
        fs::create_dir_all(path.parent().expect("models directory"))
            .expect("create models directory");
        fs::write(
            &path,
            r#"{
                "providers": {
                    "cc-switch-test-copy": {
                        "name": "Native OAuth",
                        "oauth": "example",
                        "baseUrl": "https://api.example.com/v1",
                        "api": "openai-completions",
                        "models": [{ "id": "model-a" }]
                    }
                }
            }"#,
        )
        .expect("write native provider");

        let mut copy = input("model-a");
        copy.id = "cc-switch-test-copy".to_string();
        let error = add(&state, copy, false)
            .expect_err("an unsynced native provider key must stay reserved");

        assert!(error.to_string().contains("already exists in models.yml"));
        assert!(state
            .db
            .get_provider_by_id("cc-switch-test-copy", OMP_APP)
            .expect("read saved provider")
            .is_none());
        assert!(
            crate::omp_config::omp_provider_exists("cc-switch-test-copy")
                .expect("read native provider")
        );

        let providers = ProviderService::list(&state, AppType::Omp).expect("sync native provider");
        assert_eq!(providers["cc-switch-test-copy"].name, "Native OAuth");
    }

    #[test]
    #[serial]
    fn malformed_native_file_keeps_the_saved_catalog_visible() {
        let _agent = TestAgentDir::new();
        let state = state();
        add(&state, input("model-a"), false).expect("save provider");
        let path = crate::omp_config::get_omp_models_path().expect("models path");
        fs::create_dir_all(path.parent().expect("models directory"))
            .expect("create models directory");
        fs::write(path, "{not-json").expect("write malformed models");

        let providers = ProviderService::list(&state, AppType::Omp).expect("read saved catalog");
        assert!(providers.contains_key("cc-switch-test"));
    }

    #[test]
    #[serial]
    fn malformed_native_provider_nodes_are_skipped_during_import() {
        let _agent = TestAgentDir::new();
        let state = state();
        let path = crate::omp_config::get_omp_models_path().expect("models path");
        fs::create_dir_all(path.parent().expect("models directory"))
            .expect("create models directory");
        fs::write(
            &path,
            r#"{
                "providers": {
                    "valid": {
                        "baseUrl": "https://api.example.com/v1",
                        "apiKey": "secret",
                        "api": "openai-completions",
                        "models": [{"id": "model-a"}]
                    },
                    "bad": "not-an-object",
                    "bad-model": {
                        "baseUrl": "https://api.example.com/v1",
                        "api": "openai-completions",
                        "models": [{"id": 42}]
                    }
                }
            }"#,
        )
        .expect("write native providers");

        let providers = ProviderService::list(&state, AppType::Omp).expect("sync providers");
        assert!(providers.contains_key("valid"));
        assert!(!providers.contains_key("bad"));
        assert!(!providers.contains_key("bad-model"));
        assert!(state
            .db
            .get_provider_by_id("bad", OMP_APP)
            .expect("query skipped provider")
            .is_none());
    }

    #[test]
    #[serial]
    fn numeric_json_representation_does_not_block_removal() {
        let _agent = TestAgentDir::new();
        let state = state();
        let mut saved = input("model-a");
        saved.settings_config["models"][0]["contextWindow"] = json!(1_000_000);
        add(&state, saved, false).expect("save provider");

        let path = crate::omp_config::get_omp_models_path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        secure_test_dir(path.parent().unwrap());
        fs::write(
            path,
            r#"{
                "providers": {
                    "cc-switch-test": {
                        "name": "Test provider",
                        "baseUrl": "https://api.example.com/v1",
                        "apiKey": "secret",
                        "api": "openai-completions",
                        "models": [{ "id": "model-a", "contextWindow": 1000000.0 }]
                    }
                }
            }"#,
        )
        .unwrap();

        ProviderService::remove_from_live_config(&state, AppType::Omp, "cc-switch-test")
            .expect("remove provider with equivalent numeric representation");
        assert!(!crate::omp_config::omp_provider_exists("cc-switch-test").unwrap());
        assert_eq!(
            state
                .db
                .get_provider_by_id("cc-switch-test", "omp")
                .unwrap()
                .unwrap()
                .settings_config["models"][0]["contextWindow"]
                .as_f64(),
            Some(1_000_000.0)
        );
    }

    #[test]
    #[serial]
    fn unreadable_selection_blocks_destructive_membership_changes() {
        let _agent = TestAgentDir::new();
        let state = state();
        let settings_path = crate::omp_config::get_omp_settings_path().unwrap();
        fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        secure_test_dir(settings_path.parent().unwrap());
        fs::write(&settings_path, "{not-json").unwrap();

        add(&state, input("model-a"), true).expect("selection is unrelated to adding a provider");
        assert!(state
            .db
            .get_provider_by_id("cc-switch-test", "omp")
            .unwrap()
            .is_some());
        assert!(crate::omp_config::omp_provider_exists("cc-switch-test").unwrap());

        let original = input("model-a");
        update(&state, Some("cc-switch-test"), original.clone())
            .expect("an edit that keeps every model does not need the default selection");

        let remove_error =
            ProviderService::remove_from_live_config(&state, AppType::Omp, "cc-switch-test")
                .expect_err("unreadable role config must block destructive removal");
        assert!(matches!(remove_error, AppError::Config(_)));
        let enable_error = ProviderService::switch(&state, AppType::Omp, "cc-switch-test")
            .expect_err("unreadable settings must block a misleading enable");
        assert!(matches!(enable_error, AppError::Config(_)));
        let delete_error = ProviderService::delete(&state, AppType::Omp, "cc-switch-test")
            .expect_err("unreadable role config must block destructive deletion");
        assert!(matches!(delete_error, AppError::Config(_)));
        assert!(crate::omp_config::omp_provider_exists("cc-switch-test").unwrap());
    }
}
