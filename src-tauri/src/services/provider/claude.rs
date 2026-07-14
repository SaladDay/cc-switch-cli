use super::*;

impl ProviderService {
    #[allow(dead_code)]
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

    #[allow(dead_code)]
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

    #[allow(dead_code)]
    pub(super) fn strip_common_claude_config_from_provider(
        provider: &mut Provider,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        common_config::normalize_provider_common_config_for_storage(
            &AppType::Claude,
            provider,
            common_config_snippet,
        )
    }

    pub(super) fn prepare_switch_claude(
        config: &mut MultiAppConfig,
        provider_id: &str,
        effective_current_provider: Option<&str>,
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

        Self::backfill_claude_current(config, provider_id, effective_current_provider)?;

        if let Some(manager) = config.get_manager_mut(&AppType::Claude) {
            manager.current = provider_id.to_string();
        }

        Ok(provider)
    }

    pub(super) fn backfill_claude_current(
        config: &mut MultiAppConfig,
        next_provider: &str,
        effective_current_provider: Option<&str>,
    ) -> Result<(), AppError> {
        let settings_path = get_claude_settings_path();
        if !settings_path.exists() {
            return Ok(());
        }

        let current_id = effective_current_provider.unwrap_or_default();
        if current_id.is_empty() || current_id == next_provider {
            return Ok(());
        }

        let current_provider = config
            .get_manager(&AppType::Claude)
            .and_then(|manager| manager.providers.get(current_id))
            .cloned();
        let Some(current_provider) = current_provider else {
            return Ok(());
        };

        let mut live = read_json_file::<Value>(&settings_path)?;
        let _ = Self::normalize_claude_models_in_value(&mut live);
        live = common_config::strip_common_config_from_live_settings(
            &AppType::Claude,
            &current_provider,
            live,
            config.common_config_snippets.claude.as_deref(),
        );
        if let Some(manager) = config.get_manager_mut(&AppType::Claude) {
            if let Some(current) = manager.providers.get_mut(current_id) {
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

        let Some(manager) = config.get_manager_mut(&AppType::Claude) else {
            return Ok(());
        };

        for provider in manager.providers.values_mut() {
            common_config::migrate_provider_subset_usage_for_storage(
                &AppType::Claude,
                provider,
                Some(old_snippet),
            )?;
        }

        Ok(())
    }

    pub(crate) fn write_claude_live_force(
        provider: &Provider,
        common_config_snippet: Option<&str>,
        apply_common_config: bool,
    ) -> Result<(), AppError> {
        let prepared = Self::prepare_claude_live_write(
            provider,
            common_config_snippet,
            None,
            apply_common_config,
            true,
        )?;
        Self::apply_claude_live_write(&prepared)
    }

    pub(super) fn prepare_claude_live_write(
        provider: &Provider,
        common_config_snippet: Option<&str>,
        _previous_common_config_snippet: Option<&str>,
        apply_common_config: bool,
        force_sync: bool,
    ) -> Result<PreparedLiveWrite, AppError> {
        if !force_sync && !crate::sync_policy::should_sync_live(&AppType::Claude) {
            return Ok(PreparedLiveWrite::Noop);
        }

        // Upstream parity (sync_claude_live): build the provider's effective
        // settings (provider config + common-config snippet) and OVERWRITE
        // settings.json. Non-provider fields survive via the common-config
        // snippet, not via a merge with the existing live file.
        let mut settings = Self::build_effective_live_snapshot(
            &AppType::Claude,
            provider,
            common_config_snippet,
            apply_common_config,
        )?;
        // Upstream parity (sanitize_claude_settings_for_live): CC-Switch's
        // internal-only fields must never be written into Claude Code's
        // settings.json. The stored provider snapshot keeps them (meta /
        // settings_config), so this only strips them from the live file.
        Self::sanitize_claude_settings_for_live(&mut settings);

        Ok(PreparedLiveWrite::Claude { settings })
    }

    /// Mirror of upstream `sanitize_claude_settings_for_live`: remove CC-Switch
    /// internal-only fields that must not leak into Claude Code's settings.json.
    fn sanitize_claude_settings_for_live(settings: &mut Value) {
        if let Some(obj) = settings.as_object_mut() {
            obj.remove("api_format");
            obj.remove("apiFormat");
            obj.remove("openrouter_compat_mode");
            obj.remove("openrouterCompatMode");
        }
    }

    pub(super) fn apply_claude_live_write(prepared: &PreparedLiveWrite) -> Result<(), AppError> {
        let PreparedLiveWrite::Claude { settings } = prepared else {
            return Ok(());
        };
        write_json_file(&get_claude_settings_path(), settings)
    }
}
