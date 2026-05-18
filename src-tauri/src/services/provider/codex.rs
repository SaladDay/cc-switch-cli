use super::*;
use indexmap::IndexMap;
use std::fs;
use std::path::Path;
use uuid::Uuid;

#[derive(Debug, Clone)]
struct LiveCodexCatalogProvider {
    key: String,
    name: String,
    settings_config: Value,
    is_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexImportMatchKind {
    Key,
    Name,
}

impl ProviderService {
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

        let mut settings_to_store = Self::normalize_settings_config_for_storage(
            &AppType::Codex,
            &provider,
            Value::Object(raw_settings),
            common_snippet.as_deref(),
        )?;
        Self::restore_codex_model_provider_for_storage_best_effort(
            &provider,
            &mut settings_to_store,
        );

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
        // Remove entire model_providers table (provider-specific configuration)
        root.remove("model_providers");
        // Codex writes trust decisions for local workspaces at runtime. These
        // must stay with the provider snapshot being backfilled, not become
        // common config that is merged into every provider.
        root.remove("projects");
        root.remove("trusted_workspaces");

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
        Self::normalize_existing_provider_snapshots_for_storage_best_effort(
            config,
            &AppType::Codex,
            Some(extracted.as_str()),
        );
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

    fn codex_provider_key_from_config_text(config_toml: &str) -> Option<String> {
        let doc = config_toml.parse::<toml_edit::DocumentMut>().ok()?;
        let provider_key = doc
            .get("model_provider")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())?;

        doc.get("model_providers")
            .and_then(|value| value.as_table_like())
            .and_then(|providers| providers.get(provider_key))
            .map(|_| provider_key.to_string())
    }

    pub(super) fn provider_codex_model_provider_key(provider: &Provider) -> Option<String> {
        provider
            .meta
            .as_ref()
            .and_then(|meta| meta.codex_model_provider_key.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| {
                provider
                    .settings_config
                    .get("config")
                    .and_then(Value::as_str)
                    .and_then(Self::codex_provider_key_from_config_text)
            })
    }

    fn compact_codex_key_suffix(raw: &str) -> String {
        raw.chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .map(|ch| ch.to_ascii_lowercase())
            .take(8)
            .collect()
    }

    fn unique_codex_provider_key_for_conflict(
        provider: &Provider,
        occupied: &std::collections::HashSet<String>,
        conflicting_key: &str,
    ) -> String {
        let mut candidates = Vec::new();
        for raw in [provider.id.trim(), provider.name.trim()] {
            if raw.is_empty() {
                continue;
            }
            let candidate = crate::codex_config::clean_codex_provider_key(raw);
            if candidate != conflicting_key && !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }

        if candidates.is_empty() {
            let suffix = Self::compact_codex_key_suffix(&provider.id);
            if !suffix.is_empty() {
                candidates.push(format!("{conflicting_key}_{suffix}"));
            }
        }

        let base = candidates
            .first()
            .cloned()
            .unwrap_or_else(|| format!("{conflicting_key}_provider"));
        let suffix = Self::compact_codex_key_suffix(&provider.id);
        if !suffix.is_empty() {
            let suffixed = format!("{base}_{suffix}");
            if !candidates.contains(&suffixed) {
                candidates.push(suffixed);
            }
        }

        for candidate in candidates {
            if !occupied.contains(&candidate) && candidate != conflicting_key {
                return candidate;
            }
        }

        let mut index = 2usize;
        loop {
            let candidate = format!("{base}_{index}");
            if !occupied.contains(&candidate) && candidate != conflicting_key {
                return candidate;
            }
            index += 1;
        }
    }

    fn rewrite_provider_codex_model_provider_key(
        provider: &mut Provider,
        target_key: &str,
    ) -> Result<bool, AppError> {
        let target_key = crate::codex_config::clean_codex_provider_key(target_key);
        let current_key = Self::provider_codex_model_provider_key(provider);
        let mut changed = current_key.as_deref() != Some(target_key.as_str());

        if let Some(config_text) = provider
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            let rewritten = crate::codex_config::rewrite_codex_config_model_provider_key(
                &config_text,
                &target_key,
            )?;
            if rewritten != config_text {
                changed = true;
                if let Some(settings_obj) = provider.settings_config.as_object_mut() {
                    settings_obj.insert("config".to_string(), Value::String(rewritten));
                }
            }
        }

        provider
            .meta
            .get_or_insert_with(Default::default)
            .codex_model_provider_key = Some(target_key);

        Ok(changed)
    }

    fn repair_conflicting_custom_codex_provider_keys(
        manager: &mut crate::provider::ProviderManager,
    ) -> bool {
        let provider_ids = manager.providers.keys().cloned().collect::<Vec<_>>();
        let mut key_groups = std::collections::HashMap::<String, Vec<String>>::new();
        for provider_id in &provider_ids {
            let Some(provider) = manager.providers.get(provider_id) else {
                continue;
            };
            if Self::is_codex_official_provider(provider) {
                continue;
            }
            let Some(key) = Self::provider_codex_model_provider_key(provider) else {
                continue;
            };
            key_groups.entry(key).or_default().push(provider_id.clone());
        }

        let mut occupied = key_groups
            .keys()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        let mut changed = false;
        for (key, provider_ids) in key_groups {
            if key != "custom" || provider_ids.len() < 2 {
                continue;
            }

            for provider_id in provider_ids {
                let Some(provider) = manager.providers.get_mut(&provider_id) else {
                    continue;
                };
                let new_key =
                    Self::unique_codex_provider_key_for_conflict(provider, &occupied, &key);
                match Self::rewrite_provider_codex_model_provider_key(provider, &new_key) {
                    Ok(true) => {
                        log::warn!(
                            "auto-repaired conflicting Codex provider key for '{}' from '{}' to '{}'",
                            provider_id,
                            key,
                            new_key
                        );
                        occupied.insert(new_key);
                        changed = true;
                    }
                    Ok(false) => {
                        occupied.insert(new_key);
                    }
                    Err(err) => {
                        log::warn!(
                            "skip auto-repair for conflicting Codex provider '{}' (key '{}'): {}",
                            provider_id,
                            key,
                            err
                        );
                    }
                }
            }
        }

        changed
    }

    fn collect_codex_providers_for_live_sync(config: &mut MultiAppConfig) -> (Vec<Provider>, bool) {
        let Some(manager) = config.get_manager_mut(&AppType::Codex) else {
            return (Vec::new(), false);
        };

        let repaired = Self::repair_conflicting_custom_codex_provider_keys(manager);
        let providers = manager.providers.values().cloned().collect::<Vec<_>>();
        (providers, repaired)
    }

    fn codex_catalog_entry_from_provider(
        provider: &Provider,
    ) -> Result<Option<(String, toml_edit::Item)>, AppError> {
        let Some(config_toml) = provider
            .settings_config
            .get("config")
            .and_then(Value::as_str)
        else {
            return Ok(None);
        };
        if config_toml.trim().is_empty() {
            return Ok(None);
        }

        let doc = config_toml.parse::<toml_edit::DocumentMut>().map_err(|e| {
            AppError::Config(format!(
                "Codex provider '{}' TOML 无法解析: {e}",
                provider.id
            ))
        })?;

        let configured_key = Self::provider_codex_model_provider_key(provider);
        let model_providers = doc
            .get("model_providers")
            .and_then(|item| item.as_table_like());
        let source_key = configured_key
            .as_deref()
            .filter(|key| {
                model_providers
                    .and_then(|providers| providers.get(*key))
                    .is_some()
            })
            .map(str::to_string)
            .or_else(|| Self::codex_provider_key_from_config_text(config_toml))
            .or_else(|| {
                model_providers.and_then(|providers| {
                    let mut keys = providers.iter().map(|(key, _)| key.to_string());
                    let first = keys.next()?;
                    keys.next().is_none().then_some(first)
                })
            });

        let Some(source_key) = source_key else {
            return Ok(None);
        };
        let resolved_key = configured_key.unwrap_or_else(|| source_key.clone());
        let Some(provider_item) = model_providers.and_then(|providers| providers.get(&source_key))
        else {
            return Ok(None);
        };

        Ok(Some((resolved_key, provider_item.clone())))
    }

    fn codex_catalog_item_signature(item: &toml_edit::Item) -> String {
        item.to_string()
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
    }

    fn merge_codex_catalog_into_config_text(
        base_config_toml: &str,
        catalog_entries: &IndexMap<String, toml_edit::Item>,
        stale_keys: &[String],
    ) -> Result<String, AppError> {
        let mut doc = if base_config_toml.trim().is_empty() {
            toml_edit::DocumentMut::new()
        } else {
            base_config_toml
                .parse::<toml_edit::DocumentMut>()
                .map_err(|e| AppError::Config(format!("Codex live config TOML 无法解析: {e}")))?
        };

        if doc.get("model_providers").is_none() {
            doc["model_providers"] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let providers = doc["model_providers"].as_table_like_mut().ok_or_else(|| {
            AppError::Config("Codex live `model_providers` 必须是 TOML table".into())
        })?;

        for stale_key in stale_keys {
            providers.remove(stale_key);
        }
        for (key, item) in catalog_entries {
            providers.insert(key, item.clone());
        }

        if providers.iter().next().is_none() {
            doc.as_table_mut().remove("model_providers");
        }

        Ok(doc.to_string())
    }

    fn sync_codex_provider_catalog_entries_to_live(
        providers: &[Provider],
        stale_keys: &[String],
    ) -> Result<(), AppError> {
        let mut catalog_entries = IndexMap::new();
        let mut owners = std::collections::HashMap::<String, String>::new();
        for provider in providers {
            if Self::is_codex_official_provider(provider) {
                continue;
            }
            let catalog_entry = match Self::codex_catalog_entry_from_provider(provider) {
                Ok(entry) => entry,
                Err(err) => {
                    log::warn!(
                        "skip syncing broken Codex provider snapshot '{}' into live catalog: {err}",
                        provider.id
                    );
                    continue;
                }
            };
            let Some((key, item)) = catalog_entry else {
                continue;
            };
            if let Some(previous_owner) = owners.insert(key.clone(), provider.id.clone()) {
                return Err(AppError::Config(format!(
                    "Codex provider key 冲突: `{key}` 同时属于 `{previous_owner}` 和 `{}`",
                    provider.id
                )));
            }
            catalog_entries.insert(key, item);
        }

        let auth = if get_codex_auth_path().exists() {
            Some(read_json_file::<Value>(&get_codex_auth_path())?)
        } else {
            None
        };
        let current_text = crate::codex_config::read_and_validate_codex_config_text()?;
        let merged_text = Self::merge_codex_catalog_into_config_text(
            &current_text,
            &catalog_entries,
            stale_keys,
        )?;
        if merged_text == current_text {
            return Ok(());
        }

        crate::codex_config::write_codex_live_atomic_optional_auth(
            auth.as_ref(),
            Some(&merged_text),
        )
    }

    pub(super) fn sync_codex_provider_catalog_to_live(
        state: &AppState,
        stale_keys: &[String],
    ) -> Result<(), AppError> {
        let (providers, repaired) = {
            let mut guard = state.config.write().map_err(AppError::from)?;
            Self::collect_codex_providers_for_live_sync(&mut guard)
        };
        if repaired {
            state.save()?;
        }
        Self::sync_codex_provider_catalog_entries_to_live(&providers, stale_keys)
    }

    pub(crate) fn sync_codex_provider_catalog_to_live_from_config(
        config: &mut MultiAppConfig,
        stale_keys: &[String],
    ) -> Result<(), AppError> {
        let (providers, _repaired) = Self::collect_codex_providers_for_live_sync(config);
        Self::sync_codex_provider_catalog_entries_to_live(&providers, stale_keys)
    }

    fn build_codex_catalog_snapshot_config(
        provider_key: &str,
        provider_item: &toml_edit::Item,
        model: Option<&str>,
    ) -> String {
        let mut doc = toml_edit::DocumentMut::new();
        doc["model_provider"] = toml_edit::value(provider_key);
        if let Some(model) = model.map(str::trim).filter(|value| !value.is_empty()) {
            doc["model"] = toml_edit::value(model);
        }
        doc["model_providers"] = toml_edit::Item::Table(toml_edit::Table::new());
        if let Some(providers) = doc["model_providers"].as_table_like_mut() {
            providers.insert(provider_key, provider_item.clone());
        }
        doc.to_string().trim().to_string()
    }

    fn parse_codex_catalog_from_live(
        config_toml: &str,
        auth: &Value,
    ) -> Result<Vec<LiveCodexCatalogProvider>, AppError> {
        if config_toml.trim().is_empty() {
            return Ok(Vec::new());
        }

        let doc = config_toml
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| AppError::Config(format!("Codex live config TOML 无法解析: {e}")))?;
        let active_key = doc
            .get("model_provider")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let model = doc
            .get("model")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let Some(model_providers) = doc
            .get("model_providers")
            .and_then(|item| item.as_table_like())
        else {
            return Ok(Vec::new());
        };
        let canonical_active_key = active_key.as_ref().and_then(|active_key| {
            let active_item = model_providers.get(active_key.as_str())?;
            let active_signature = Self::codex_catalog_item_signature(active_item);
            model_providers.iter().find_map(|(key, item)| {
                (key != active_key && Self::codex_catalog_item_signature(item) == active_signature)
                    .then(|| key.to_string())
            })
        });
        let effective_active_key = canonical_active_key.as_deref().or(active_key.as_deref());

        let mut providers = Vec::new();
        for (key, item) in model_providers.iter() {
            if active_key.as_deref() == Some(key) && canonical_active_key.is_some() {
                continue;
            }
            let name = item
                .as_table_like()
                .and_then(|table| table.get("name"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(key)
                .to_string();
            let is_active = effective_active_key == Some(key);
            let auth_value = if is_active {
                auth.clone()
            } else {
                Value::Object(serde_json::Map::new())
            };
            providers.push(LiveCodexCatalogProvider {
                key: key.to_string(),
                name,
                settings_config: json!({
                    "auth": auth_value,
                    "config": Self::build_codex_catalog_snapshot_config(key, item, model.as_deref()),
                }),
                is_active,
            });
        }

        Ok(providers)
    }

    fn find_codex_import_target(
        manager: &crate::provider::ProviderManager,
        entry: &LiveCodexCatalogProvider,
    ) -> Result<Option<(String, CodexImportMatchKind)>, ()> {
        let key_matches = manager
            .providers
            .values()
            .filter(|provider| {
                Self::provider_codex_model_provider_key(provider).as_deref()
                    == Some(entry.key.as_str())
            })
            .map(|provider| provider.id.clone())
            .collect::<Vec<_>>();
        match key_matches.len() {
            0 => {}
            1 => return Ok(Some((key_matches[0].clone(), CodexImportMatchKind::Key))),
            _ => return Err(()),
        }

        let normalized_name = entry.name.trim();
        if normalized_name.is_empty() {
            return Ok(None);
        }
        let name_matches = manager
            .providers
            .values()
            .filter(|provider| provider.name.trim() == normalized_name)
            .map(|provider| provider.id.clone())
            .collect::<Vec<_>>();
        match name_matches.len() {
            0 => Ok(None),
            1 => Ok(Some((name_matches[0].clone(), CodexImportMatchKind::Name))),
            _ => Err(()),
        }
    }

    pub fn import_codex_providers_from_live(
        state: &AppState,
    ) -> Result<CodexImportReport, AppError> {
        let auth = if get_codex_auth_path().exists() {
            read_json_file::<Value>(&get_codex_auth_path())?
        } else {
            Value::Object(serde_json::Map::new())
        };
        let config_toml = crate::codex_config::read_and_validate_codex_config_text()?;
        let live_providers = Self::parse_codex_catalog_from_live(&config_toml, &auth)?;
        if live_providers.is_empty() {
            let imported = Self::import_default_config(state, AppType::Codex)?;
            return Ok(CodexImportReport {
                created: usize::from(imported),
                used_default_fallback: imported,
                ..CodexImportReport::default()
            });
        }

        let preserved_current = [AppType::Codex];
        Self::run_transaction_preserving_current_providers(
            state,
            &preserved_current,
            move |config| {
                config.ensure_app(&AppType::Codex);
                let manager = config
                    .get_manager_mut(&AppType::Codex)
                    .ok_or_else(|| Self::app_not_found(&AppType::Codex))?;
                let mut report = CodexImportReport::default();

                for entry in &live_providers {
                    let target = match Self::find_codex_import_target(manager, entry) {
                        Ok(target) => target,
                        Err(()) => {
                            report.conflicts += 1;
                            continue;
                        }
                    };

                    let mut settings_config = entry.settings_config.clone();
                    let auth_is_empty = settings_config
                        .get("auth")
                        .and_then(Value::as_object)
                        .is_some_and(|value| value.is_empty());

                    match target {
                        Some((provider_id, match_kind)) => {
                            let existing = manager
                                .providers
                                .get(&provider_id)
                                .cloned()
                                .ok_or_else(|| {
                                    AppError::localized(
                                        "provider.not_found",
                                        format!("供应商不存在: {provider_id}"),
                                        format!("Provider not found: {provider_id}"),
                                    )
                                })?;

                            if auth_is_empty {
                                if let Some(existing_auth) =
                                    existing.settings_config.get("auth").cloned()
                                {
                                    if let Some(obj) = settings_config.as_object_mut() {
                                        obj.insert("auth".to_string(), existing_auth);
                                    }
                                }
                            }

                            let mut merged = existing.clone();
                            merged.settings_config = settings_config;
                            merged
                                .meta
                                .get_or_insert_with(Default::default)
                                .codex_model_provider_key = Some(entry.key.clone());
                            manager.providers.insert(provider_id.clone(), merged);

                            match match_kind {
                                CodexImportMatchKind::Key => report.merged_by_key += 1,
                                CodexImportMatchKind::Name => report.merged_by_name += 1,
                            }
                        }
                        None => {
                            let mut provider = Provider::with_id(
                                Uuid::new_v4().to_string(),
                                entry.name.clone(),
                                settings_config,
                                None,
                            );
                            provider.category = Some("custom".to_string());
                            provider.created_at = Some(current_timestamp());
                            provider
                                .meta
                                .get_or_insert_with(Default::default)
                                .codex_model_provider_key = Some(entry.key.clone());
                            manager.providers.insert(provider.id.clone(), provider);
                            report.created += 1;
                        }
                    }

                    if !entry.is_active && auth_is_empty {
                        report.needs_auth += 1;
                    }
                }

                Ok((report, None))
            },
        )
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
                Self::strip_common_codex_config_from_provider(provider, Some(old_snippet))?;
            }

            return Ok(());
        };

        let Some(manager) = config.get_manager_mut(&AppType::Codex) else {
            return Ok(());
        };

        if let Some(current_provider) = manager.providers.get_mut(&current_provider_id) {
            Self::strip_common_codex_config_from_provider(current_provider, Some(old_snippet))?;
        }

        for (provider_id, provider) in manager.providers.iter_mut() {
            if provider_id == &current_provider_id {
                continue;
            }

            if let Err(err) =
                Self::strip_common_codex_config_from_provider(provider, Some(old_snippet))
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

        let auth_path = get_codex_auth_path();
        let config_path = get_codex_config_path();
        if !auth_path.exists() && !config_path.exists() {
            return Ok(());
        }

        let current_provider = config
            .get_manager(&AppType::Codex)
            .and_then(|manager| manager.providers.get(current_id))
            .cloned();
        let Some(current_provider) = current_provider else {
            return Ok(());
        };

        // Read auth from disk; if absent, fall back to the DB snapshot's auth
        // so that WebDAV-synced credentials are not overwritten with empty data.
        let auth = if auth_path.exists() {
            Some(read_json_file::<Value>(&auth_path)?)
        } else {
            current_provider.settings_config.get("auth").cloned()
        };

        let mut settings_config = if config_path.exists() {
            let text =
                std::fs::read_to_string(&config_path).map_err(|e| AppError::io(&config_path, e))?;
            Self::maybe_update_codex_common_config_snippet(config, &text)?;

            let mut raw_settings = serde_json::Map::new();
            if let Some(auth) = auth.clone() {
                raw_settings.insert("auth".to_string(), auth);
            }
            raw_settings.insert("config".to_string(), Value::String(text));
            Self::normalize_settings_config_for_storage(
                &AppType::Codex,
                &current_provider,
                Value::Object(raw_settings),
                config.common_config_snippets.codex.as_deref(),
            )?
        } else {
            let mut raw_settings = serde_json::Map::new();
            if let Some(auth) = auth.clone() {
                raw_settings.insert("auth".to_string(), auth);
            }
            Value::Object(raw_settings)
        };
        Self::restore_codex_model_provider_for_storage_best_effort(
            &current_provider,
            &mut settings_config,
        );

        if let Some(manager) = config.get_manager_mut(&AppType::Codex) {
            if let Some(current) = manager.providers.get_mut(current_id) {
                current.settings_config = settings_config;
            }
        }

        Ok(())
    }

    /// Write Codex live configuration.
    ///
    /// Aligned with upstream: the stored `settings_config.config` is the full config.toml text.
    /// We write it directly to `~/.codex/config.toml`, optionally merging the common config snippet.
    /// Auth is handled separately via auth.json.
    pub(super) fn write_codex_live(
        provider: &Provider,
        common_config_snippet: Option<&str>,
        apply_common_config: bool,
    ) -> Result<(), AppError> {
        if !crate::sync_policy::should_sync_live(&AppType::Codex) {
            return Ok(());
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

        let auth_to_write = if Self::is_codex_official_provider(provider)
            && auth.as_object().is_some_and(|auth| auth.is_empty())
        {
            None
        } else {
            Some(auth)
        };
        crate::codex_config::write_codex_live_atomic_optional_auth_with_stable_provider(
            auth_to_write,
            Some(cfg_text),
        )?;

        Ok(())
    }
}
