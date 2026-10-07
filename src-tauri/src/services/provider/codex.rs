use super::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

impl ProviderService {
    /// Match upstream backfill: the stored catalog is authoritative; live files
    /// only contain a lossy projection that clients or proxy cycles can remove.
    pub(super) fn preserve_codex_model_catalog_for_backfill(
        provider: &Provider,
        settings: &mut Value,
    ) {
        if let Some(catalog) = provider.settings_config.get("modelCatalog") {
            if let Some(settings) = settings.as_object_mut() {
                settings.insert("modelCatalog".to_string(), catalog.clone());
            }
        }
    }

    /// Shared launches derive their routing/storage settings at startup. Only
    /// login changes belong back in the provider record; never save AppState's
    /// full snapshot while other providers may be finishing concurrently.
    pub(crate) fn capture_codex_launch_auth(
        db: &crate::Database,
        provider_id: &str,
        codex_home: &Path,
    ) -> Result<(), AppError> {
        let auth_path = codex_home.join("auth.json");
        let auth = if auth_path.exists() {
            read_json_file::<Value>(&auth_path)?
        } else {
            serde_json::json!({})
        };
        if !auth.is_object() {
            return Err(AppError::Config("Codex auth.json must be an object".into()));
        }
        let source_path = codex_home.join(".auth-source");
        let source =
            fs::read_to_string(&source_path).map_err(|err| AppError::io(&source_path, err))?;
        if format!("{:x}", Sha256::digest(auth.to_string().as_bytes())) == source {
            return Ok(());
        }
        db.update_codex_provider_auth(provider_id, &auth, &source)
    }

    pub(crate) fn capture_codex_temp_launch_snapshot(
        state: &AppState,
        provider_id: &str,
        codex_home: &Path,
    ) -> Result<(), AppError> {
        let (provider, common_snippet) = {
            let guard = state.config.read().map_err(AppError::from)?;
            let provider = guard
                .get_manager(&AppType::Codex)
                .and_then(|manager| manager.providers.get(provider_id))
                .cloned()
                .ok_or_else(|| {
                    AppError::localized(
                        "provider.not_found",
                        format!("供应商不存在: {provider_id}"),
                        format!("Provider not found: {provider_id}"),
                    )
                })?;
            (provider, guard.common_config_snippets.codex.clone())
        };

        let config_path = codex_home.join("config.toml");
        let cfg_text = if config_path.exists() {
            fs::read_to_string(&config_path).map_err(|err| AppError::io(&config_path, err))?
        } else {
            provider
                .settings_config
                .get("config")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        crate::codex_config::validate_config_toml(&cfg_text)?;
        let cfg_text_for_storage = Self::strip_codex_mcp_servers_from_snapshot_config(&cfg_text)?;

        let auth_path = codex_home.join("auth.json");
        let auth = if auth_path.exists() {
            read_json_file::<Value>(&auth_path)?
        } else {
            Value::Object(serde_json::Map::new())
        };

        let mut raw_settings = serde_json::Map::new();
        raw_settings.insert("auth".to_string(), auth);
        raw_settings.insert("config".to_string(), Value::String(cfg_text_for_storage));
        let mut settings_to_store = Value::Object(raw_settings);
        Self::preserve_codex_model_catalog_for_backfill(&provider, &mut settings_to_store);
        if Self::codex_live_write_category(&provider) == Some("official") {
            crate::codex_config::strip_codex_unified_session_bucket_from_settings(
                &mut settings_to_store,
            )?;
        }

        let settings_to_store = Self::normalize_settings_config_for_storage(
            &AppType::Codex,
            &provider,
            settings_to_store,
            common_snippet.as_deref(),
        )?;

        {
            let mut guard = state.config.write().map_err(AppError::from)?;
            if let Some(manager) = guard.get_manager_mut(&AppType::Codex) {
                if let Some(target) = manager.providers.get_mut(provider_id) {
                    target.settings_config = settings_to_store;
                }
            }
        }

        state.save()
    }

    pub(super) fn extract_codex_common_config_from_config_toml(
        config_toml: &str,
    ) -> Result<String, AppError> {
        let config_toml = config_toml.trim();
        if config_toml.is_empty() {
            return Ok(String::new());
        }

        let mut doc = config_toml
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| AppError::Message(format!("TOML parse error: {e}")))?;

        // Remove provider-specific fields.
        let root = doc.as_table_mut();
        root.remove("model");
        root.remove("model_provider");
        // Legacy/alt formats might use a top-level base_url.
        root.remove("base_url");
        root.remove("wire_api");
        // Profiles can carry provider-specific model_provider overrides. Keep
        // unrelated profile settings in the common config snippet.
        root.remove("profile");
        // Remove entire model_providers table (provider-specific configuration)
        root.remove("model_providers");
        root.remove("mcp_servers");
        if let Some(mcp_tbl) = root
            .get_mut("mcp")
            .and_then(|item| item.as_table_like_mut())
        {
            mcp_tbl.remove("servers");
            if mcp_tbl.is_empty() {
                root.remove("mcp");
            }
        }
        root.remove("experimental_bearer_token");
        root.remove("model_catalog_json");
        if root
            .get(crate::codex_config::CODEX_WEB_SEARCH_FIELD)
            .and_then(|item| item.as_str())
            == Some(crate::codex_config::CODEX_WEB_SEARCH_DISABLED)
        {
            root.remove(crate::codex_config::CODEX_WEB_SEARCH_FIELD);
        }

        if let Some(profiles) = root
            .get_mut("profiles")
            .and_then(|item| item.as_table_like_mut())
        {
            let profile_keys: Vec<String> =
                profiles.iter().map(|(key, _)| key.to_string()).collect();
            for profile_key in profile_keys {
                let Some(profile) = profiles
                    .get_mut(&profile_key)
                    .and_then(|item| item.as_table_like_mut())
                else {
                    continue;
                };
                profile.remove("model");
                profile.remove("model_provider");
                if profile.is_empty() {
                    profiles.remove(&profile_key);
                }
            }
            if profiles.is_empty() {
                root.remove("profiles");
            }
        }

        // Clean up multiple empty lines (keep at most one blank line).
        let mut cleaned = String::new();
        let mut blank_run = 0usize;
        for line in doc.to_string().lines() {
            if line.trim().is_empty() {
                blank_run += 1;
                if blank_run <= 1 {
                    cleaned.push('\n');
                }
                continue;
            }
            blank_run = 0;
            cleaned.push_str(line);
            cleaned.push('\n');
        }

        Ok(cleaned.trim().to_string())
    }

    pub(super) fn maybe_update_codex_common_config_snippet(
        config: &mut MultiAppConfig,
        config_toml: &str,
    ) -> Result<(), AppError> {
        let existing = config
            .common_config_snippets
            .codex
            .as_deref()
            .unwrap_or_default()
            .trim();
        if !existing.is_empty() {
            return Ok(());
        }

        let extracted = Self::extract_codex_common_config_from_config_toml(config_toml)?;
        if extracted.trim().is_empty() {
            return Ok(());
        }

        config.common_config_snippets.codex = Some(extracted.clone());
        Self::migrate_codex_common_config_snippet(config, None, extracted.as_str())?;
        Ok(())
    }

    pub(super) fn strip_codex_mcp_servers_from_snapshot_config(
        config_toml: &str,
    ) -> Result<String, AppError> {
        let config_toml = config_toml.trim();
        if config_toml.is_empty() {
            return Ok(String::new());
        }

        let mut doc = config_toml
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| AppError::Config(format!("TOML parse error: {e}")))?;
        let root = doc.as_table_mut();
        root.remove("mcp_servers");

        if let Some(mcp_item) = root.get_mut("mcp") {
            if let Some(mcp_table) = mcp_item.as_table_like_mut() {
                mcp_table.remove("servers");
                if mcp_table.iter().next().is_none() {
                    root.remove("mcp");
                }
            }
        }

        Ok(doc.to_string())
    }

    #[allow(dead_code)]
    pub(super) fn merge_toml_tables(dst: &mut toml_edit::Table, src: &toml_edit::Table) {
        for (key, src_item) in src.iter() {
            match (dst.get_mut(key), src_item.as_table()) {
                (Some(dst_item), Some(src_table)) => {
                    if let Some(dst_table) = dst_item.as_table_mut() {
                        Self::merge_toml_tables(dst_table, src_table);
                    } else {
                        *dst_item = toml_edit::Item::Table(src_table.clone());
                    }
                }
                (Some(dst_item), None) => {
                    *dst_item = src_item.clone();
                }
                (None, _) => {
                    dst.insert(key, src_item.clone());
                }
            }
        }
    }

    #[cfg(test)]
    pub(super) fn strip_toml_tables(dst: &mut toml_edit::Table, src: &toml_edit::Table) {
        let mut keys_to_remove = Vec::new();

        for (key, src_item) in src.iter() {
            let Some(dst_item) = dst.get_mut(key) else {
                continue;
            };

            match (dst_item, src_item) {
                (toml_edit::Item::Table(dst_table), toml_edit::Item::Table(src_table)) => {
                    Self::strip_toml_tables(dst_table, src_table);
                    if dst_table.is_empty() {
                        keys_to_remove.push(key.to_string());
                    }
                }
                (dst_item, src_item) => {
                    if Self::toml_items_equal(dst_item, src_item) {
                        keys_to_remove.push(key.to_string());
                    }
                }
            }
        }

        for key in keys_to_remove {
            dst.remove(&key);
        }
    }

    #[cfg(test)]
    fn toml_items_equal(left: &toml_edit::Item, right: &toml_edit::Item) -> bool {
        match (left.as_value(), right.as_value()) {
            (Some(left_value), Some(right_value)) => {
                left_value.to_string().trim() == right_value.to_string().trim()
            }
            _ => left.to_string().trim() == right.to_string().trim(),
        }
    }

    #[allow(dead_code)]
    pub(super) fn strip_common_codex_config_from_provider(
        provider: &mut Provider,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        common_config::normalize_provider_common_config_for_storage(
            &AppType::Codex,
            provider,
            common_config_snippet,
        )
    }

    fn migrate_common_codex_config_from_provider(
        provider: &mut Provider,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        common_config::migrate_provider_subset_usage_for_storage(
            &AppType::Codex,
            provider,
            common_config_snippet,
        )
    }

    pub(super) fn migrate_codex_common_config_snippet(
        config: &mut MultiAppConfig,
        strict_current_provider_id: Option<&str>,
        old_snippet: &str,
    ) -> Result<(), AppError> {
        let old_snippet = old_snippet.trim();
        if old_snippet.is_empty() {
            return Ok(());
        }

        let Some(current_provider_id) = strict_current_provider_id.and_then(|provider_id| {
            config.get_manager(&AppType::Codex).and_then(|manager| {
                manager
                    .providers
                    .contains_key(provider_id)
                    .then(|| provider_id.to_string())
            })
        }) else {
            let Some(manager) = config.get_manager_mut(&AppType::Codex) else {
                return Ok(());
            };

            for provider in manager.providers.values_mut() {
                Self::migrate_common_codex_config_from_provider(provider, Some(old_snippet))?;
            }

            return Ok(());
        };

        let Some(manager) = config.get_manager_mut(&AppType::Codex) else {
            return Ok(());
        };

        if let Some(current_provider) = manager.providers.get_mut(&current_provider_id) {
            Self::migrate_common_codex_config_from_provider(current_provider, Some(old_snippet))?;
        }

        for (provider_id, provider) in manager.providers.iter_mut() {
            if provider_id == &current_provider_id {
                continue;
            }

            if let Err(err) =
                Self::migrate_common_codex_config_from_provider(provider, Some(old_snippet))
            {
                log::warn!(
                    "skip migrating Codex non-current provider snapshot '{provider_id}' from stored common config snippet: {err}"
                );
            }
        }

        Ok(())
    }

    pub(super) fn prepare_switch_codex(
        config: &mut MultiAppConfig,
        provider_id: &str,
        effective_current_provider: Option<&str>,
    ) -> Result<Provider, AppError> {
        let provider = config
            .get_manager(&AppType::Codex)
            .ok_or_else(|| Self::app_not_found(&AppType::Codex))?
            .providers
            .get(provider_id)
            .cloned()
            .ok_or_else(|| {
                AppError::localized(
                    "provider.not_found",
                    format!("供应商不存在: {provider_id}"),
                    format!("Provider not found: {provider_id}"),
                )
            })?;

        Self::backfill_codex_current(config, provider_id, effective_current_provider)?;

        if let Some(manager) = config.get_manager_mut(&AppType::Codex) {
            manager.current = provider_id.to_string();
        }

        Ok(provider)
    }

    pub(super) fn backfill_codex_current(
        config: &mut MultiAppConfig,
        next_provider: &str,
        effective_current_provider: Option<&str>,
    ) -> Result<(), AppError> {
        let current_id = effective_current_provider.unwrap_or_default();

        if current_id.is_empty() || current_id == next_provider {
            return Ok(());
        }

        // The upstream projector treats provider rows as templates. Never
        // backfill live routing or credentials into them on a switch. Keep only
        // the CLI's common-snippet discovery for its existing editor.
        let path = get_codex_config_path();
        if path.exists() {
            let text = std::fs::read_to_string(&path).map_err(|err| AppError::io(&path, err))?;
            Self::maybe_update_codex_common_config_snippet(config, &text)?;
        }
        Ok(())
    }

    /// Write Codex live configuration.
    ///
    /// Use upstream route projection and login planning even when bypassing the
    /// initialization guard (config restore/import).
    pub(crate) fn write_codex_live_force(
        provider: &Provider,
        common_config_snippet: Option<&str>,
        apply_common_config: bool,
        rows: &[Provider],
    ) -> Result<(), AppError> {
        let prepared = Self::prepare_codex_live_write(
            provider,
            common_config_snippet,
            None,
            apply_common_config,
            true,
            rows,
            None,
        )?;
        Self::apply_codex_live_write(&prepared)
    }

    pub(super) fn prepare_codex_live_write(
        provider: &Provider,
        common_config_snippet: Option<&str>,
        previous_common_config_snippet: Option<&str>,
        apply_common_config: bool,
        force_sync: bool,
        rows: &[Provider],
        previous: Option<&Provider>,
    ) -> Result<PreparedLiveWrite, AppError> {
        if !force_sync && !crate::sync_policy::should_sync_live(&AppType::Codex) {
            return Ok(PreparedLiveWrite::Noop);
        }

        let effective = Self::build_effective_live_snapshot(
            &AppType::Codex,
            provider,
            common_config_snippet,
            apply_common_config,
        )?;
        let settings = effective
            .as_object()
            .ok_or_else(|| AppError::Config("Codex 配置必须是 JSON 对象".into()))?;

        let auth = settings
            .get("auth")
            .ok_or_else(|| AppError::Config("Codex 供应商配置缺少 'auth' 字段".to_string()))?;
        let cfg_text = settings
            .get("config")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AppError::Config("Codex 供应商配置缺少 'config' 字段或不是字符串".to_string())
            })?;

        let profile = crate::proxy::providers::codex_provider_catalog_tool_profile(provider);
        let prepared_config =
            crate::codex_config::prepare_codex_config_text_with_model_catalog_payload(
                &provider.settings_config,
                cfg_text,
                profile,
            )?;
        let is_official = Self::codex_live_write_category(provider) == Some("official");
        let clean_config_text = if is_official {
            crate::codex_config::strip_codex_unified_session_bucket(&prepared_config.config_text)?
        } else {
            prepared_config.config_text.clone()
        };
        // Align with write_codex_live_for_provider (and upstream farion1231/cc-switch):
        // when unified Codex session history is enabled, rewrite official live
        // config through the shared `custom` model_provider bucket so third-party
        // sessions remain resumeable. Provider DB templates stay clean (stripped
        // on backfill); only ~/.codex/config.toml is injected.
        let live_config_text = if is_official && crate::settings::unify_codex_session_history() {
            crate::codex_config::inject_codex_unified_session_bucket(&clean_config_text)?
        } else {
            clean_config_text
        };

        let mut plan = codex_live::prepare(is_official, auth, &live_config_text, rows, previous)?;
        plan.config.catalog = prepared_config.model_catalog.is_some();
        plan.common = codex_live::common_patch(
            common_config_snippet.filter(|_| apply_common_config),
            previous_common_config_snippet,
        )?;
        Ok(PreparedLiveWrite::Codex {
            plan,
            config: prepared_config,
        })
    }

    pub(super) fn apply_codex_live_write(prepared: &PreparedLiveWrite) -> Result<(), AppError> {
        let PreparedLiveWrite::Codex { plan, config } = prepared else {
            return Ok(());
        };
        codex_live::apply(plan, config.model_catalog.as_ref(), None)
    }
}
