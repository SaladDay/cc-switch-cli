use regex::Regex;

use crate::app_config::AppType;
use crate::error::AppError;
use crate::provider::{Provider, UsageData, UsageResult, UsageScript};
use crate::settings;
use crate::store::AppState;
use crate::usage_script;

use super::ProviderService;

impl ProviderService {
    /// 执行用量脚本并格式化结果（私有辅助方法）
    async fn execute_and_format_usage_result(
        script_code: &str,
        api_key: &str,
        base_url: &str,
        timeout: u64,
        access_token: Option<&str>,
        user_id: Option<&str>,
    ) -> Result<UsageResult, AppError> {
        match usage_script::execute_usage_script(
            script_code,
            api_key,
            base_url,
            timeout,
            access_token,
            user_id,
        )
        .await
        {
            Ok(data) => {
                let usage_list: Vec<UsageData> = if data.is_array() {
                    serde_json::from_value(data).map_err(|e| {
                        AppError::localized(
                            "usage_script.data_format_error",
                            format!("数据格式错误: {e}"),
                            format!("Data format error: {e}"),
                        )
                    })?
                } else {
                    let single: UsageData = serde_json::from_value(data).map_err(|e| {
                        AppError::localized(
                            "usage_script.data_format_error",
                            format!("数据格式错误: {e}"),
                            format!("Data format error: {e}"),
                        )
                    })?;
                    vec![single]
                };

                Ok(UsageResult {
                    success: true,
                    data: Some(usage_list),
                    error: None,
                })
            }
            Err(err) => {
                let lang = settings::get_settings()
                    .language
                    .unwrap_or_else(|| "zh".to_string());

                let msg = match err {
                    AppError::Localized { zh, en, .. } => {
                        if lang == "en" {
                            en
                        } else {
                            zh
                        }
                    }
                    other => other.to_string(),
                };

                Ok(UsageResult {
                    success: false,
                    data: None,
                    error: Some(msg),
                })
            }
        }
    }

    /// 查询供应商用量（使用已保存的脚本配置）
    pub async fn query_usage(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
    ) -> Result<UsageResult, AppError> {
        let (script_code, timeout, api_key, base_url, access_token, user_id) = {
            let config = state.config.read().map_err(AppError::from)?;
            let manager = config
                .get_manager(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;
            let provider = manager.providers.get(provider_id).cloned().ok_or_else(|| {
                AppError::localized(
                    "provider.not_found",
                    format!("供应商不存在: {provider_id}"),
                    format!("Provider not found: {provider_id}"),
                )
            })?;

            let usage_script = provider
                .meta
                .as_ref()
                .and_then(|m| m.usage_script.as_ref())
                .ok_or_else(|| {
                    AppError::localized(
                        "provider.usage.script.missing",
                        "未配置用量查询脚本",
                        "Usage script is not configured",
                    )
                })?;
            if !usage_script.enabled {
                return Err(AppError::localized(
                    "provider.usage.disabled",
                    "用量查询未启用",
                    "Usage query is disabled",
                ));
            }

            let (api_key, base_url) =
                Self::resolve_usage_script_credentials(&provider, &app_type, usage_script)?;

            (
                usage_script.code.clone(),
                usage_script.timeout.unwrap_or(10),
                api_key,
                base_url,
                usage_script.access_token.clone(),
                usage_script.user_id.clone(),
            )
        };

        Self::execute_and_format_usage_result(
            &script_code,
            &api_key,
            &base_url,
            timeout,
            access_token.as_deref(),
            user_id.as_deref(),
        )
        .await
    }

    /// 测试用量脚本（使用临时脚本内容，不保存）
    #[allow(clippy::too_many_arguments)]
    pub async fn test_usage_script(
        _state: &AppState,
        _app_type: AppType,
        _provider_id: &str,
        script_code: &str,
        timeout: u64,
        api_key: Option<&str>,
        base_url: Option<&str>,
        access_token: Option<&str>,
        user_id: Option<&str>,
    ) -> Result<UsageResult, AppError> {
        // 直接使用传入的凭证参数进行测试
        Self::execute_and_format_usage_result(
            script_code,
            api_key.unwrap_or(""),
            base_url.unwrap_or(""),
            timeout,
            access_token,
            user_id,
        )
        .await
    }

    /// 验证 UsageScript 配置（边界检查）
    pub(super) fn validate_usage_script(script: &UsageScript) -> Result<(), AppError> {
        // 验证自动查询间隔 (0-1440 分钟，即最大24小时)
        if let Some(interval) = script.auto_query_interval {
            if interval > 1440 {
                return Err(AppError::localized(
                    "usage_script.interval_too_large",
                    format!(
                        "自动查询间隔不能超过 1440 分钟（24小时），当前值: {interval}"
                    ),
                    format!(
                        "Auto query interval cannot exceed 1440 minutes (24 hours), current: {interval}"
                    ),
                ));
            }
        }

        Ok(())
    }

    fn extract_api_key(provider: &Provider, app_type: &AppType) -> Result<String, AppError> {
        match app_type {
            AppType::Claude => {
                let env = provider
                    .settings_config
                    .get("env")
                    .and_then(|v| v.as_object())
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.claude.env.missing",
                            "配置格式错误: 缺少 env",
                            "Invalid configuration: missing env section",
                        )
                    })?;

                super::get_claude_token_from_env(env)
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.claude.api_key.missing",
                            "缺少 API Key",
                            "API key is missing",
                        )
                    })
                    .map(|s| s.to_string())
            }
            AppType::Codex => {
                let auth = provider
                    .settings_config
                    .get("auth")
                    .and_then(|v| v.as_object())
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.codex.auth.missing",
                            "配置格式错误: 缺少 auth",
                            "Invalid configuration: missing auth section",
                        )
                    })?;

                auth.get("OPENAI_API_KEY")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.codex.api_key.missing",
                            "缺少 API Key",
                            "API key is missing",
                        )
                    })
                    .map(|s| s.to_string())
            }
            AppType::Gemini => {
                use crate::gemini_config::json_to_env;

                let env_map = json_to_env(&provider.settings_config)?;

                env_map.get("GEMINI_API_KEY").cloned().ok_or_else(|| {
                    AppError::localized(
                        "gemini.missing_api_key",
                        "缺少 GEMINI_API_KEY",
                        "Missing GEMINI_API_KEY",
                    )
                })
            }
        }
    }

    fn extract_base_url(provider: &Provider, app_type: &AppType) -> Result<String, AppError> {
        match app_type {
            AppType::Claude => provider
                .settings_config
                .get("env")
                .and_then(|v| v.as_object())
                .ok_or_else(|| {
                    AppError::localized(
                        "provider.claude.env.missing",
                        "配置格式错误: 缺少 env",
                        "Invalid configuration: missing env section",
                    )
                })?
                .get("ANTHROPIC_BASE_URL")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    AppError::localized(
                        "provider.claude.base_url.missing",
                        "缺少 ANTHROPIC_BASE_URL 配置",
                        "Missing ANTHROPIC_BASE_URL configuration",
                    )
                })
                .map(|s| s.to_string()),
            AppType::Codex => {
                let config_toml = provider
                    .settings_config
                    .get("config")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if !config_toml.contains("base_url") {
                    return Err(AppError::localized(
                        "provider.codex.base_url.missing",
                        "config.toml 中缺少 base_url 配置",
                        "base_url is missing from config.toml",
                    ));
                }

                let re = Regex::new(r#"base_url\s*=\s*["']([^"']+)["']"#).map_err(|e| {
                    AppError::localized(
                        "provider.regex_init_failed",
                        format!("正则初始化失败: {e}"),
                        format!("Failed to initialize regex: {e}"),
                    )
                })?;

                re.captures(config_toml)
                    .and_then(|caps| caps.get(1))
                    .map(|m| m.as_str().to_string())
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.codex.base_url.invalid",
                            "config.toml 中 base_url 格式错误",
                            "base_url in config.toml has invalid format",
                        )
                    })
            }
            AppType::Gemini => {
                use crate::gemini_config::json_to_env;

                let env_map = json_to_env(&provider.settings_config)?;

                Ok(env_map
                    .get("GOOGLE_GEMINI_BASE_URL")
                    .cloned()
                    .unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string()))
            }
        }
    }

    pub(super) fn extract_credentials(
        provider: &Provider,
        app_type: &AppType,
    ) -> Result<(String, String), AppError> {
        Ok((
            Self::extract_api_key(provider, app_type)?,
            Self::extract_base_url(provider, app_type)?,
        ))
    }

    pub(super) fn resolve_usage_script_credentials(
        provider: &Provider,
        app_type: &AppType,
        usage_script: &UsageScript,
    ) -> Result<(String, String), AppError> {
        let api_key = match usage_script.api_key.as_deref().map(str::trim) {
            Some(value) if !value.is_empty() => value.to_string(),
            _ => Self::extract_api_key(provider, app_type)?,
        };

        let base_url = match usage_script.base_url.as_deref().map(str::trim) {
            Some(value) if !value.is_empty() => value.to_string(),
            _ => Self::extract_base_url(provider, app_type)?,
        };

        Ok((api_key, base_url))
    }
}
