use super::*;

impl ProviderService {
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
                Self::strip_common_gemini_config_from_provider(provider, Some(old_snippet))?;
            }

            return Ok(());
        };

        let Some(manager) = config.get_manager_mut(&AppType::Gemini) else {
            return Ok(());
        };

        if let Some(current_provider) = manager.providers.get_mut(&current_provider_id) {
            Self::strip_common_gemini_config_from_provider(current_provider, Some(old_snippet))?;
        }

        for (provider_id, provider) in manager.providers.iter_mut() {
            if provider_id == &current_provider_id {
                continue;
            }

            if let Err(err) =
                Self::strip_common_gemini_config_from_provider(provider, Some(old_snippet))
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

    pub(crate) fn write_gemini_live(
        provider: &Provider,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        Self::write_gemini_live_impl(provider, common_config_snippet, false)
    }

    pub(crate) fn write_gemini_live_force(
        provider: &Provider,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        Self::write_gemini_live_impl(provider, common_config_snippet, true)
    }

    pub(super) fn write_gemini_live_impl(
        provider: &Provider,
        common_config_snippet: Option<&str>,
        force_sync: bool,
    ) -> Result<(), AppError> {
        use crate::gemini_config::{
            get_gemini_settings_path, json_to_env, validate_gemini_settings_strict,
            write_gemini_env_atomic,
        };

        // 一次性检测认证类型，避免重复检测
        let auth_type = Self::detect_gemini_auth_type(provider);

        if !force_sync && !crate::sync_policy::should_sync_live(&AppType::Gemini) {
            // still update CC-Switch app-level settings, but do not create any ~/.gemini files
            match auth_type {
                GeminiAuthType::GoogleOfficial => {
                    Self::ensure_google_oauth_security_flag(provider)?
                }
                GeminiAuthType::ApiKey => Self::ensure_api_key_security_flag(provider)?,
            }
            return Ok(());
        }

        let content_to_write = Self::build_effective_live_snapshot(
            &AppType::Gemini,
            provider,
            common_config_snippet,
            true,
        )?;

        let mut env_map = json_to_env(&content_to_write)?;

        // 准备要写入 ~/.gemini/settings.json 的配置（缺省时保留现有文件内容）
        let settings_path = get_gemini_settings_path();
        let mut config_to_write = if let Some(config_value) = content_to_write.get("config") {
            if config_value.is_null() {
                None // null → 保留现有文件
            } else if let Some(provider_config) = config_value.as_object() {
                if provider_config.is_empty() {
                    None // 空对象 {} → 保留现有文件
                } else {
                    // 有内容 → 合并到现有 settings.json（保留现有 key，如 mcpServers），供应商优先
                    let mut merged = if settings_path.exists() {
                        read_json_file(&settings_path)?
                    } else {
                        json!({})
                    };

                    if !merged.is_object() {
                        merged = json!({});
                    }

                    let merged_map = merged.as_object_mut().ok_or_else(|| {
                        AppError::localized(
                            "gemini.validation.invalid_settings",
                            "Gemini 现有 settings.json 格式错误: 必须是对象",
                            "Gemini existing settings.json invalid: must be a JSON object",
                        )
                    })?;
                    for (key, value) in provider_config {
                        merged_map.insert(key.clone(), value.clone());
                    }

                    Some(merged)
                }
            } else {
                return Err(AppError::localized(
                    "gemini.validation.invalid_config",
                    "Gemini 配置格式错误: config 必须是对象或 null",
                    "Gemini config invalid: config must be an object or null",
                ));
            }
        } else {
            None
        };

        if config_to_write.is_none() {
            if settings_path.exists() {
                config_to_write = Some(read_json_file(&settings_path)?);
            } else {
                config_to_write = Some(json!({})); // 新建空配置
            }
        }

        match auth_type {
            GeminiAuthType::GoogleOfficial => {
                // Google 官方使用 OAuth，清空 env
                env_map.clear();
                write_gemini_env_atomic(&env_map)?;
            }
            GeminiAuthType::ApiKey => {
                // API Key 供应商（所有第三方服务）
                // 统一处理：验证配置 + 写入 .env 文件
                validate_gemini_settings_strict(&content_to_write)?;
                write_gemini_env_atomic(&env_map)?;
            }
        }

        if let Some(config_value) = config_to_write {
            write_json_file(&settings_path, &config_value)?;
        }

        match auth_type {
            GeminiAuthType::GoogleOfficial => Self::ensure_google_oauth_security_flag(provider)?,
            GeminiAuthType::ApiKey => Self::ensure_api_key_security_flag(provider)?,
        }

        Ok(())
    }
}
