use super::*;

impl ProviderService {
    #[allow(dead_code)]
    pub(super) fn parse_common_gemini_config_snippet(snippet: &str) -> Result<Value, AppError> {
        let value: Value = serde_json::from_str(snippet).map_err(|e| {
            AppError::localized(
                "common_config.gemini.invalid_json",
                format!("Gemini 通用配置片段不是有效的 JSON：{e}"),
                format!("Gemini common config snippet is not valid JSON: {e}"),
            )
        })?;
        if !value.is_object() {
            return Err(AppError::localized(
                "common_config.gemini.not_object",
                "Gemini 通用配置片段必须是 JSON 对象",
                "Gemini common config snippet must be a JSON object",
            ));
        }
        Ok(value)
    }

    pub(super) fn prepare_switch_gemini(
        config: &mut MultiAppConfig,
        provider_id: &str,
        effective_current_provider: Option<&str>,
    ) -> Result<Provider, AppError> {
        let provider = config
            .get_manager(&AppType::Gemini)
            .ok_or_else(|| Self::app_not_found(&AppType::Gemini))?
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

        Self::backfill_gemini_current(config, provider_id, effective_current_provider)?;

        if let Some(manager) = config.get_manager_mut(&AppType::Gemini) {
            manager.current = provider_id.to_string();
        }

        Ok(provider)
    }

    #[allow(dead_code)]
    pub(super) fn strip_common_gemini_config_from_provider(
        provider: &mut Provider,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        common_config::normalize_provider_common_config_for_storage(
            &AppType::Gemini,
            provider,
            common_config_snippet,
        )
    }

    fn migrate_common_gemini_config_from_provider(
        provider: &mut Provider,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        common_config::migrate_provider_subset_usage_for_storage(
            &AppType::Gemini,
            provider,
            common_config_snippet,
        )
    }

    pub(super) fn migrate_gemini_common_config_snippet(
        config: &mut MultiAppConfig,
        strict_current_provider_id: Option<&str>,
        old_snippet: &str,
    ) -> Result<(), AppError> {
        let old_snippet = old_snippet.trim();
        if old_snippet.is_empty() {
            return Ok(());
        }

        let Some(current_provider_id) = strict_current_provider_id.and_then(|provider_id| {
            config.get_manager(&AppType::Gemini).and_then(|manager| {
                manager
                    .providers
                    .contains_key(provider_id)
                    .then(|| provider_id.to_string())
            })
        }) else {
            let Some(manager) = config.get_manager_mut(&AppType::Gemini) else {
                return Ok(());
            };

            for provider in manager.providers.values_mut() {
                Self::migrate_common_gemini_config_from_provider(provider, Some(old_snippet))?;
            }

            return Ok(());
        };

        let Some(manager) = config.get_manager_mut(&AppType::Gemini) else {
            return Ok(());
        };

        if let Some(current_provider) = manager.providers.get_mut(&current_provider_id) {
            Self::migrate_common_gemini_config_from_provider(current_provider, Some(old_snippet))?;
        }

        for (provider_id, provider) in manager.providers.iter_mut() {
            if provider_id == &current_provider_id {
                continue;
            }

            if let Err(err) =
                Self::migrate_common_gemini_config_from_provider(provider, Some(old_snippet))
            {
                log::warn!(
                    "skip migrating Gemini non-current provider snapshot '{provider_id}' from stored common config snippet: {err}"
                );
            }
        }

        Ok(())
    }

    pub(super) fn backfill_gemini_current(
        config: &mut MultiAppConfig,
        next_provider: &str,
        effective_current_provider: Option<&str>,
    ) -> Result<(), AppError> {
        use crate::gemini_config::{
            env_to_json, get_gemini_env_path, get_gemini_settings_path, read_gemini_env,
        };

        let env_path = get_gemini_env_path();
        if !env_path.exists() {
            return Ok(());
        }

        let current_id = effective_current_provider.unwrap_or_default();
        if current_id.is_empty() || current_id == next_provider {
            return Ok(());
        }

        let current_provider = config
            .get_manager(&AppType::Gemini)
            .and_then(|manager| manager.providers.get(current_id))
            .cloned();
        let Some(current_provider) = current_provider else {
            return Ok(());
        };

        let env_map = read_gemini_env()?;
        let mut live = env_to_json(&env_map);

        let settings_path = get_gemini_settings_path();
        let config_value = if settings_path.exists() {
            read_json_file(&settings_path)?
        } else {
            json!({})
        };
        if let Some(obj) = live.as_object_mut() {
            obj.insert("config".to_string(), config_value);
        }
        let live = Self::normalize_settings_config_for_storage(
            &AppType::Gemini,
            &current_provider,
            live,
            config.common_config_snippets.gemini.as_deref(),
        )?;

        if let Some(manager) = config.get_manager_mut(&AppType::Gemini) {
            if let Some(current) = manager.providers.get_mut(current_id) {
                current.settings_config = live;
            }
        }

        Ok(())
    }

    pub(crate) fn write_gemini_live_force(
        provider: &Provider,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        let prepared =
            Self::prepare_gemini_live_write(provider, common_config_snippet, None, true)?;
        Self::apply_gemini_live_write(&prepared)
    }

    pub(super) fn prepare_gemini_live_write(
        provider: &Provider,
        common_config_snippet: Option<&str>,
        _previous_common_config_snippet: Option<&str>,
        force_sync: bool,
    ) -> Result<PreparedLiveWrite, AppError> {
        use crate::gemini_config::{
            get_gemini_settings_path, json_to_env, validate_gemini_settings_strict,
        };

        let auth_type = Self::detect_gemini_auth_type(provider);
        if !force_sync && !crate::sync_policy::should_sync_live(&AppType::Gemini) {
            return Ok(PreparedLiveWrite::GeminiSecurityFlag { auth_type });
        }

        let content_to_write = Self::build_effective_live_snapshot(
            &AppType::Gemini,
            provider,
            common_config_snippet,
            common_config_snippet.is_some(),
        )?;

        // Upstream parity (write_gemini_live): the .env file is a full OVERWRITE
        // with the provider's effective env (`json_to_env(provider.settings_config)`
        // upstream), for BOTH auth types. Google Official carries OAuth and skips
        // the API-key validation, but still writes the provider's env verbatim
        // (e.g. GEMINI_MODEL / custom vars) — it does not preserve the prior
        // file's unrelated keys.
        let env = match auth_type {
            GeminiAuthType::GoogleOfficial => json_to_env(&content_to_write)?,
            GeminiAuthType::ApiKey => {
                validate_gemini_settings_strict(&content_to_write)?;
                json_to_env(&content_to_write)?
            }
        };

        let mut incoming_config = match content_to_write.get("config") {
            Some(Value::Null) | None => json!({}),
            Some(config_value) => {
                if let Some(provider_config) = config_value.as_object() {
                    Value::Object(provider_config.clone())
                } else {
                    return Err(AppError::localized(
                        "gemini.validation.invalid_config",
                        "Gemini 配置格式错误: config 必须是对象或 null",
                        "Gemini config invalid: config must be an object or null",
                    ));
                }
            }
        };

        let config_obj = incoming_config.as_object_mut().ok_or_else(|| {
            AppError::localized(
                "gemini.validation.invalid_config",
                "Gemini 配置格式错误: config 必须是对象或 null",
                "Gemini config invalid: config must be an object or null",
            )
        })?;
        let security = config_obj
            .entry("security".to_string())
            .or_insert_with(|| json!({}));
        let security_obj = security.as_object_mut().ok_or_else(|| {
            AppError::localized(
                "gemini.validation.invalid_security",
                "Gemini 配置格式错误: security 必须是对象",
                "Gemini config invalid: security must be an object",
            )
        })?;
        let auth = security_obj
            .entry("auth".to_string())
            .or_insert_with(|| json!({}));
        let auth_obj = auth.as_object_mut().ok_or_else(|| {
            AppError::localized(
                "gemini.validation.invalid_security_auth",
                "Gemini 配置格式错误: security.auth 必须是对象",
                "Gemini config invalid: security.auth must be an object",
            )
        })?;
        auth_obj.insert(
            "selectedType".to_string(),
            Value::String(Self::gemini_security_selected_type(auth_type).to_string()),
        );

        // Upstream parity (write_gemini_live): settings.json is a SHALLOW merge
        // of the provider's config keys into the existing file, preserving
        // unrelated user fields such as mcpServers. Only the .env file is a full
        // overwrite.
        let settings_path = get_gemini_settings_path();
        let mut settings = if settings_path.exists() {
            read_json_file::<Value>(&settings_path)?
        } else {
            json!({})
        };
        if !settings.is_object() {
            settings = json!({});
        }
        if let (Some(settings_obj), Some(incoming_obj)) =
            (settings.as_object_mut(), incoming_config.as_object())
        {
            for (key, value) in incoming_obj {
                settings_obj.insert(key.clone(), value.clone());
            }
        }

        Ok(PreparedLiveWrite::Gemini {
            env,
            settings,
            auth_type,
        })
    }

    pub(super) fn apply_gemini_live_write(prepared: &PreparedLiveWrite) -> Result<(), AppError> {
        use crate::gemini_config::{get_gemini_settings_path, write_gemini_env_atomic};

        match prepared {
            PreparedLiveWrite::Gemini {
                env,
                settings,
                auth_type,
            } => {
                write_gemini_env_atomic(env)?;
                write_json_file(&get_gemini_settings_path(), settings)?;
                Self::ensure_gemini_app_security_flag(*auth_type)?;
            }
            PreparedLiveWrite::GeminiSecurityFlag { auth_type } => {
                Self::ensure_gemini_app_security_flag(*auth_type)?;
            }
            _ => {}
        }

        Ok(())
    }
}
