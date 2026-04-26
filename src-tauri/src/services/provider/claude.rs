use super::common::merge_json_values;
use super::*;

impl ProviderService {
    const CLAUDE_PROVIDER_ENV_KEYS: &'static [&'static str] = &[
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_MODEL",
        "ANTHROPIC_REASONING_MODEL",
        "ANTHROPIC_SMALL_FAST_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
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
        common_config::normalize_provider_common_config_for_storage(
            &AppType::Claude,
            provider,
            common_config_snippet,
        )
    }

    fn strip_claude_provider_owned_live_keys(settings: &mut Value) {
        let Some(env) = settings.get_mut("env").and_then(Value::as_object_mut) else {
            return;
        };

        for key in Self::CLAUDE_PROVIDER_ENV_KEYS {
            env.remove(*key);
        }
    }

    pub(super) fn merge_claude_provider_owned_values_from_live(snapshot: &mut Value, live: &Value) {
        let Some(live_env) = live.get("env").and_then(Value::as_object) else {
            return;
        };

        if !snapshot.is_object() {
            *snapshot = json!({});
        }

        let snapshot_obj = snapshot
            .as_object_mut()
            .expect("snapshot should be object after normalization");
        let env_value = snapshot_obj
            .entry("env".to_string())
            .or_insert_with(|| json!({}));
        if !env_value.is_object() {
            *env_value = json!({});
        }
        let snapshot_env = env_value
            .as_object_mut()
            .expect("env should be object after normalization");

        for key in Self::CLAUDE_PROVIDER_ENV_KEYS {
            snapshot_env.remove(*key);
            if let Some(value) = live_env.get(*key) {
                snapshot_env.insert((*key).to_string(), value.clone());
            }
        }
    }

    pub(super) fn build_claude_live_snapshot_for_write(
        provider: &Provider,
        common_config_snippet: Option<&str>,
        previous_common_config_snippet: Option<&str>,
        apply_common_config: bool,
        existing_live_settings: Option<Value>,
    ) -> Result<Value, AppError> {
        let apply_common_config = Self::resolve_live_apply_common_config(
            &AppType::Claude,
            provider,
            common_config_snippet,
            apply_common_config,
        );

        let mut live_base = existing_live_settings.unwrap_or_else(|| json!({}));
        if !live_base.is_object() {
            live_base = json!({});
        }

        Self::strip_claude_provider_owned_live_keys(&mut live_base);

        if let Some(snippet) = previous_common_config_snippet.map(str::trim) {
            if !snippet.is_empty() {
                match common_config::remove_common_config_from_settings(
                    &AppType::Claude,
                    &live_base,
                    snippet,
                ) {
                    Ok(settings) => live_base = settings,
                    Err(err)
                        if Self::should_skip_common_config_migration_error(
                            &AppType::Claude,
                            &err,
                        ) =>
                    {
                        log::warn!(
                            "skip stripping invalid stored Claude common config snippet from live base: {err}"
                        );
                        live_base = json!({});
                    }
                    Err(err) => return Err(err),
                }
            }
        }

        let mut provider_content = common_config::build_effective_settings_with_common_config(
            &AppType::Claude,
            provider,
            common_config_snippet,
            apply_common_config,
        )?;
        let _ = Self::normalize_claude_models_in_value(&mut provider_content);
        merge_json_values(&mut live_base, &provider_content);
        let _ = Self::normalize_claude_models_in_value(&mut live_base);

        Ok(live_base)
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
                Self::merge_claude_provider_owned_values_from_live(
                    &mut current.settings_config,
                    &live,
                );
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
            common_config::normalize_provider_common_config_for_storage(
                &AppType::Claude,
                provider,
                Some(old_snippet),
            )?;
        }

        Ok(())
    }

    pub(super) fn write_claude_live(
        provider: &Provider,
        common_config_snippet: Option<&str>,
        previous_common_config_snippet: Option<&str>,
        apply_common_config: bool,
    ) -> Result<(), AppError> {
        if !crate::sync_policy::should_sync_live(&AppType::Claude) {
            return Ok(());
        }

        let settings_path = get_claude_settings_path();
        let existing_live_settings = if settings_path.exists() {
            Some(read_json_file::<Value>(&settings_path)?)
        } else {
            None
        };
        let content_to_write = Self::build_claude_live_snapshot_for_write(
            provider,
            common_config_snippet,
            previous_common_config_snippet,
            apply_common_config,
            existing_live_settings,
        )?;

        write_json_file(&settings_path, &content_to_write)?;
        Ok(())
    }
}
