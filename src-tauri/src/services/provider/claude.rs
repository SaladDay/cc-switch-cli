use super::*;

impl ProviderService {
    const CLAUDE_PRESERVED_SHARED_LIVE_KEYS: &[&str] = &[
        "allowedHttpHookUrls",
        "disableAllHooks",
        "enabledMcpjsonServers",
        "disabledMcpjsonServers",
        "enableAllProjectMcpServers",
        "extraKnownMarketplaces",
        "fileSuggestion",
        "hooks",
        "httpHookAllowedEnvVars",
        "permissions",
        "respectGitignore",
        "statusLine",
    ];

    pub(super) fn parse_common_claude_config_snippet(snippet: &str) -> Result<Value, AppError> {
        let value: Value = serde_json::from_str(snippet).map_err(|e| {
            AppError::localized(
                "common_config.claude.invalid_json",
                format!("Claude 通用配置片段不是有效的 JSON：{e}"),
                format!("Claude common config snippet is not valid JSON: {e}"),
            )
        })?;
        if !value.is_object() {
            return Err(AppError::localized(
                "common_config.claude.not_object",
                "Claude 通用配置片段必须是 JSON 对象",
                "Claude common config snippet must be a JSON object",
            ));
        }
        Ok(value)
    }

    pub(super) fn parse_common_claude_config_snippet_for_strip(
        snippet: &str,
    ) -> Result<Value, AppError> {
        let mut value = Self::parse_common_claude_config_snippet(snippet)?;
        let _ = Self::normalize_claude_models_in_value(&mut value);
        Ok(value)
    }

    /// 归一化 Claude 模型键：读旧键(ANTHROPIC_SMALL_FAST_MODEL)，写新键(DEFAULT_*), 并删除旧键
    pub(crate) fn normalize_claude_models_in_value(settings: &mut Value) -> bool {
        let mut changed = false;
        let env = match settings.get_mut("env") {
            Some(v) if v.is_object() => v.as_object_mut().unwrap(),
            _ => return changed,
        };

        let model = env
            .get("ANTHROPIC_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let small_fast = env
            .get("ANTHROPIC_SMALL_FAST_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let current_haiku = env
            .get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let current_sonnet = env
            .get("ANTHROPIC_DEFAULT_SONNET_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let current_opus = env
            .get("ANTHROPIC_DEFAULT_OPUS_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let target_haiku = current_haiku
            .or_else(|| small_fast.clone())
            .or_else(|| model.clone());
        let target_sonnet = current_sonnet
            .or_else(|| model.clone())
            .or_else(|| small_fast.clone());
        let target_opus = current_opus
            .or_else(|| model.clone())
            .or_else(|| small_fast.clone());

        if env.get("ANTHROPIC_DEFAULT_HAIKU_MODEL").is_none() {
            if let Some(v) = target_haiku {
                env.insert(
                    "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(),
                    Value::String(v),
                );
                changed = true;
            }
        }
        if env.get("ANTHROPIC_DEFAULT_SONNET_MODEL").is_none() {
            if let Some(v) = target_sonnet {
                env.insert(
                    "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
                    Value::String(v),
                );
                changed = true;
            }
        }
        if env.get("ANTHROPIC_DEFAULT_OPUS_MODEL").is_none() {
            if let Some(v) = target_opus {
                env.insert("ANTHROPIC_DEFAULT_OPUS_MODEL".to_string(), Value::String(v));
                changed = true;
            }
        }

        if env.remove("ANTHROPIC_SMALL_FAST_MODEL").is_some() {
            changed = true;
        }

        changed
    }

    pub(super) fn normalize_provider_if_claude(app_type: &AppType, provider: &mut Provider) {
        if matches!(app_type, AppType::Claude) {
            let mut v = provider.settings_config.clone();
            if Self::normalize_claude_models_in_value(&mut v) {
                provider.settings_config = v;
            }
        }
    }

    pub(super) fn strip_common_claude_config_from_provider(
        provider: &mut Provider,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        let apply_common_config = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.apply_common_config)
            .unwrap_or(true);
        if !apply_common_config {
            return Ok(());
        }

        let Some(snippet) = common_config_snippet.map(str::trim) else {
            return Ok(());
        };
        if snippet.is_empty() {
            return Ok(());
        }

        let common = Self::parse_common_claude_config_snippet_for_strip(snippet)?;
        strip_common_values(&mut provider.settings_config, &common);
        Ok(())
    }

    pub(crate) fn strip_preserved_claude_shared_live_settings(value: &mut Value) {
        let Some(obj) = value.as_object_mut() else {
            return;
        };
        for key in Self::CLAUDE_PRESERVED_SHARED_LIVE_KEYS {
            obj.remove(*key);
        }
    }

    pub(crate) fn preserve_claude_shared_live_settings(value: &mut Value) -> Result<(), AppError> {
        let settings_path = get_claude_settings_path();
        if !settings_path.exists() {
            return Ok(());
        }

        let existing = read_json_file::<Value>(&settings_path)?;
        let (Some(existing_obj), Some(target_obj)) = (existing.as_object(), value.as_object_mut())
        else {
            return Ok(());
        };

        for key in Self::CLAUDE_PRESERVED_SHARED_LIVE_KEYS {
            if target_obj.contains_key(*key) {
                continue;
            }
            if let Some(existing_value) = existing_obj.get(*key) {
                target_obj.insert((*key).to_string(), existing_value.clone());
            }
        }

        Ok(())
    }

    pub(super) fn prepare_switch_claude(
        config: &mut MultiAppConfig,
        provider_id: &str,
    ) -> Result<Provider, AppError> {
        let provider = config
            .get_manager(&AppType::Claude)
            .ok_or_else(|| Self::app_not_found(&AppType::Claude))?
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

        Self::backfill_claude_current(config, provider_id)?;

        if let Some(manager) = config.get_manager_mut(&AppType::Claude) {
            manager.current = provider_id.to_string();
        }

        Ok(provider)
    }

    pub(super) fn backfill_claude_current(
        config: &mut MultiAppConfig,
        next_provider: &str,
    ) -> Result<(), AppError> {
        let settings_path = get_claude_settings_path();
        if !settings_path.exists() {
            return Ok(());
        }

        let current_id = config
            .get_manager(&AppType::Claude)
            .map(|m| m.current.clone())
            .unwrap_or_default();
        if current_id.is_empty() || current_id == next_provider {
            return Ok(());
        }

        let mut live = read_json_file::<Value>(&settings_path)?;
        let _ = Self::normalize_claude_models_in_value(&mut live);
        if let Some(snippet) = config.common_config_snippets.claude.as_deref() {
            let snippet = snippet.trim();
            if !snippet.is_empty() {
                let common = Self::parse_common_claude_config_snippet_for_strip(snippet)?;
                strip_common_values(&mut live, &common);
            }
        }
        Self::strip_preserved_claude_shared_live_settings(&mut live);
        if let Some(manager) = config.get_manager_mut(&AppType::Claude) {
            if let Some(current) = manager.providers.get_mut(&current_id) {
                current.settings_config = live;
            }
        }

        Ok(())
    }

    pub(super) fn migrate_claude_common_config_snippet(
        config: &mut MultiAppConfig,
        old_snippet: &str,
    ) -> Result<(), AppError> {
        let old_snippet = old_snippet.trim();
        if old_snippet.is_empty() {
            return Ok(());
        }

        let common = Self::parse_common_claude_config_snippet_for_strip(old_snippet)?;
        let Some(manager) = config.get_manager_mut(&AppType::Claude) else {
            return Ok(());
        };

        for provider in manager.providers.values_mut() {
            if provider
                .meta
                .as_ref()
                .and_then(|meta| meta.apply_common_config)
                .unwrap_or(true)
            {
                strip_common_values(&mut provider.settings_config, &common);
            }
        }

        Ok(())
    }

    pub(super) fn write_claude_live(
        provider: &Provider,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        if !crate::sync_policy::should_sync_live(&AppType::Claude) {
            return Ok(());
        }

        let settings_path = get_claude_settings_path();
        let mut provider_content = provider.settings_config.clone();
        let _ = Self::normalize_claude_models_in_value(&mut provider_content);

        let content_to_write = if let Some(snippet) = common_config_snippet {
            let snippet = snippet.trim();
            if snippet.is_empty() {
                provider_content
            } else {
                let common = Self::parse_common_claude_config_snippet(snippet)?;
                let mut merged = common;
                merge_json_values(&mut merged, &provider_content);
                let _ = Self::normalize_claude_models_in_value(&mut merged);
                merged
            }
        } else {
            provider_content
        };

        let mut content_to_write = content_to_write;
        Self::preserve_claude_shared_live_settings(&mut content_to_write)?;

        write_json_file(&settings_path, &content_to_write)?;
        Ok(())
    }
}
