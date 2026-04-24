use super::*;

impl ProviderService {
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

        let settings_config = if config_path.exists() {
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

        // Validate TOML before writing
        if !cfg_text.trim().is_empty() {
            crate::codex_config::validate_config_toml(cfg_text)?;
        }

        // Write config.toml
        let config_path = get_codex_config_path();
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }
        crate::config::write_text_file(&config_path, cfg_text)?;

        let auth_path = get_codex_auth_path();
        write_json_file(&auth_path, auth)?;

        Ok(())
    }
}
