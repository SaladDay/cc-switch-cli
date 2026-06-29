use std::sync::OnceLock;

use regex::Regex;
use tokio::sync::RwLock;

use crate::app_config::AppType;
use crate::error::AppError;
use crate::provider::{Provider, UsageData, UsageResult, UsageScript};
use crate::proxy::providers::copilot_auth::CopilotAuthManager;
use crate::settings;
use crate::store::AppState;
use crate::usage_script;

use super::ProviderService;

const TEMPLATE_TYPE_GITHUB_COPILOT: &str = "github_copilot";
const TEMPLATE_TYPE_TOKEN_PLAN: &str = "token_plan";
const TEMPLATE_TYPE_BALANCE: &str = "balance";
const COPILOT_UNIT_PREMIUM: &str = "requests";

static CLI_COPILOT_AUTH_MANAGER: OnceLock<RwLock<CopilotAuthManager>> = OnceLock::new();

impl ProviderService {
    /// 执行用量脚本并格式化结果（私有辅助方法）
    async fn execute_and_format_usage_result(
        script_code: &str,
        api_key: &str,
        base_url: &str,
        timeout: u64,
        access_token: Option<&str>,
        user_id: Option<&str>,
        template_type: Option<&str>,
    ) -> Result<UsageResult, AppError> {
        match usage_script::execute_usage_script(
            script_code,
            api_key,
            base_url,
            timeout,
            access_token,
            user_id,
            template_type,
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
        let (script_code, timeout, api_key, base_url, access_token, user_id, template_type) = {
            let providers = state.db.get_all_providers(app_type.as_str())?;
            let provider = providers.get(provider_id).ok_or_else(|| {
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
                Self::resolve_usage_script_credentials(provider, &app_type, usage_script)?;

            (
                usage_script.code.clone(),
                usage_script.timeout.unwrap_or(10),
                api_key,
                base_url,
                usage_script.access_token.clone(),
                usage_script.user_id.clone(),
                usage_script.template_type.clone(),
            )
        };

        Self::execute_and_format_usage_result(
            &script_code,
            &api_key,
            &base_url,
            timeout,
            access_token.as_deref(),
            user_id.as_deref(),
            template_type.as_deref(),
        )
        .await
    }

    /// 查询供应商用量，包含上游 Usage Query 的特殊模板分发。
    pub async fn query_provider_usage(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
    ) -> Result<UsageResult, String> {
        let providers = state
            .db
            .get_all_providers(app_type.as_str())
            .map_err(|e| format!("Failed to get providers: {e}"))?;
        let provider = providers.get(provider_id);
        let usage_script = provider
            .and_then(|p| p.meta.as_ref())
            .and_then(|m| m.usage_script.as_ref());
        let template_type = usage_script
            .and_then(|s| s.template_type.as_deref())
            .unwrap_or("");

        if template_type == TEMPLATE_TYPE_GITHUB_COPILOT {
            return Self::query_github_copilot_usage(provider).await;
        }

        if template_type == TEMPLATE_TYPE_TOKEN_PLAN {
            let Some((provider, usage_script)) = provider.zip(usage_script) else {
                return Err("Usage script is not configured".to_string());
            };
            let (api_key, base_url) =
                Self::resolve_usage_script_credentials(provider, &app_type, usage_script)
                    .map_err(|e| e.to_string())?;

            let quota = crate::services::coding_plan::get_coding_plan_quota(&base_url, &api_key)
                .await
                .map_err(|e| format!("Failed to query coding plan: {e}"))?;

            if !quota.success {
                return Ok(UsageResult {
                    success: false,
                    data: None,
                    error: quota.error,
                });
            }

            let data: Vec<UsageData> = quota
                .tiers
                .iter()
                .map(|tier| {
                    let total = 100.0;
                    let used = tier.utilization;
                    let remaining = total - used;
                    UsageData {
                        plan_name: Some(tier.name.clone()),
                        remaining: Some(remaining),
                        total: Some(total),
                        used: Some(used),
                        unit: Some("%".to_string()),
                        is_valid: Some(true),
                        invalid_message: None,
                        extra: tier.resets_at.clone(),
                    }
                })
                .collect();

            return Ok(UsageResult {
                success: true,
                data: if data.is_empty() { None } else { Some(data) },
                error: None,
            });
        }

        if template_type == TEMPLATE_TYPE_BALANCE {
            let Some((provider, usage_script)) = provider.zip(usage_script) else {
                return Err("Usage script is not configured".to_string());
            };
            let (api_key, base_url) =
                Self::resolve_usage_script_credentials(provider, &app_type, usage_script)
                    .map_err(|e| e.to_string())?;

            return crate::services::balance::get_balance(&base_url, &api_key)
                .await
                .map_err(|e| format!("Failed to query balance: {e}"));
        }

        Self::query_usage(state, app_type, provider_id)
            .await
            .map_err(|e| e.to_string())
    }

    async fn query_github_copilot_usage(
        provider: Option<&Provider>,
    ) -> Result<UsageResult, String> {
        let copilot_account_id = provider
            .and_then(|p| p.meta.as_ref())
            .and_then(|m| m.managed_account_id_for(TEMPLATE_TYPE_GITHUB_COPILOT));
        let manager = CLI_COPILOT_AUTH_MANAGER.get_or_init(|| {
            RwLock::new(CopilotAuthManager::new(crate::config::get_app_config_dir()))
        });
        let auth_manager = manager.read().await;
        let usage = match copilot_account_id.as_deref() {
            Some(account_id) => auth_manager
                .fetch_usage_for_account(account_id)
                .await
                .map_err(|e| format!("Failed to fetch Copilot usage: {e}"))?,
            None => auth_manager
                .fetch_usage()
                .await
                .map_err(|e| format!("Failed to fetch Copilot usage: {e}"))?,
        };
        let premium = &usage.quota_snapshots.premium_interactions;
        let used = premium.entitlement - premium.remaining;

        Ok(UsageResult {
            success: true,
            data: Some(vec![UsageData {
                plan_name: Some(usage.copilot_plan),
                remaining: Some(premium.remaining as f64),
                total: Some(premium.entitlement as f64),
                used: Some(used as f64),
                unit: Some(COPILOT_UNIT_PREMIUM.to_string()),
                is_valid: Some(true),
                invalid_message: None,
                extra: Some(format!("Reset: {}", usage.quota_reset_date)),
            }]),
            error: None,
        })
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
        template_type: Option<&str>,
    ) -> Result<UsageResult, AppError> {
        // 直接使用传入的凭证参数进行测试
        Self::execute_and_format_usage_result(
            script_code,
            api_key.unwrap_or(""),
            base_url.unwrap_or(""),
            timeout,
            access_token,
            user_id,
            template_type,
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

                env.get("ANTHROPIC_AUTH_TOKEN")
                    .or_else(|| env.get("ANTHROPIC_API_KEY"))
                    .and_then(|v| v.as_str())
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
            AppType::OpenCode => provider
                .settings_config
                .get("options")
                .and_then(|v| v.get("apiKey"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    AppError::localized(
                        "provider.opencode.api_key.missing",
                        "缺少 API Key",
                        "API key is missing",
                    )
                })
                .map(|s| s.to_string()),
            AppType::Hermes => provider
                .settings_config
                .get("apiKey")
                .or_else(|| provider.settings_config.get("api_key"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    AppError::localized(
                        "provider.hermes.api_key.missing",
                        "缺少 API Key",
                        "API key is missing",
                    )
                })
                .map(|s| s.to_string()),
            AppType::OpenClaw => provider
                .settings_config
                .get("apiKey")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    AppError::localized(
                        "provider.openclaw.api_key.missing",
                        "缺少 API Key",
                        "API key is missing",
                    )
                })
                .map(|s| s.to_string()),
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
            AppType::OpenCode => Ok(provider
                .settings_config
                .get("options")
                .and_then(|v| v.get("baseURL"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()),
            AppType::Hermes => Ok(provider
                .settings_config
                .get("baseUrl")
                .or_else(|| provider.settings_config.get("baseURL"))
                .or_else(|| provider.settings_config.get("endpoint"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()),
            AppType::OpenClaw => Ok(provider
                .settings_config
                .get("baseUrl")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()),
        }
    }

    #[cfg(test)]
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
        let api_key = usage_script
            .api_key
            .clone()
            .filter(|k| !k.is_empty())
            .or_else(|| Self::extract_api_key(provider, app_type).ok())
            .unwrap_or_default();

        let base_url = usage_script
            .base_url
            .clone()
            .filter(|u| !u.is_empty())
            .or_else(|| Self::extract_base_url(provider, app_type).ok())
            .map(|url| url.trim_end_matches('/').to_string())
            .unwrap_or_default();

        Ok((api_key, base_url))
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderService;
    use crate::app_config::{AppType, MultiAppConfig};
    use crate::provider::{Provider, ProviderMeta, UsageScript};
    use axum::{routing::get, Router};
    use serde_json::json;

    #[tokio::test]
    async fn query_usage_reads_provider_from_db_when_config_snapshot_is_stale() {
        let state = super::super::state_from_config(MultiAppConfig::default());

        let app = Router::new().route("/", get(|| async { axum::Json(json!({ "total": 42 })) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let address = listener.local_addr().expect("listener local addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("server should run");
        });

        let mut provider = Provider::with_id(
            "db-only".to_string(),
            "DB Only".to_string(),
            json!({}),
            None,
        );
        provider.meta = Some(ProviderMeta {
            usage_script: Some(UsageScript {
                enabled: true,
                language: "javascript".to_string(),
                code: r#"({
                    request: {
                        url: "{{baseUrl}}",
                        method: "GET"
                    },
                    extractor: function(response) {
                        return { total: response.total };
                    }
                })"#
                .to_string(),
                timeout: Some(2),
                api_key: Some("unused".to_string()),
                base_url: Some(format!("http://{address}/")),
                access_token: None,
                user_id: None,
                template_type: None,
                auto_query_interval: None,
                coding_plan_provider: None,
            }),
            ..Default::default()
        });
        state
            .db
            .save_provider(AppType::OpenClaw.as_str(), &provider)
            .expect("save provider to db only");

        let result = ProviderService::query_usage(&state, AppType::OpenClaw, "db-only")
            .await
            .expect("query usage should use db-backed provider lookup");

        assert!(
            result.success,
            "expected successful usage query: {result:?}"
        );
        assert_eq!(
            result
                .data
                .as_ref()
                .and_then(|items| items.first())
                .and_then(|usage| usage.total),
            Some(42.0)
        );

        server.abort();
    }

    #[test]
    fn resolve_usage_script_credentials_reads_codex_provider_config() {
        let provider = Provider::with_id(
            "codex".to_string(),
            "Codex Provider".to_string(),
            json!({
                "auth": {
                    "OPENAI_API_KEY": "sk-codex"
                },
                "config": "model_provider = \"custom\"\n[model_providers.custom]\nbase_url = \"https://codex.example/v1\"\n"
            }),
            None,
        );
        let script = UsageScript {
            enabled: true,
            language: "javascript".to_string(),
            code: String::new(),
            timeout: None,
            api_key: None,
            base_url: None,
            access_token: None,
            user_id: None,
            template_type: Some("general".to_string()),
            auto_query_interval: None,
            coding_plan_provider: None,
        };

        let (api_key, base_url) =
            ProviderService::resolve_usage_script_credentials(&provider, &AppType::Codex, &script)
                .expect("Codex credentials should resolve from provider config");

        assert_eq!(api_key, "sk-codex");
        assert_eq!(base_url, "https://codex.example/v1");
    }

    #[test]
    fn resolve_usage_script_credentials_reads_openclaw_provider_config() {
        let provider = Provider::with_id(
            "openclaw".to_string(),
            "OpenClaw Provider".to_string(),
            json!({
                "apiKey": "sk-openclaw",
                "baseUrl": "https://openclaw.example/v1/"
            }),
            None,
        );
        let script = UsageScript {
            enabled: true,
            language: "javascript".to_string(),
            code: String::new(),
            timeout: None,
            api_key: None,
            base_url: None,
            access_token: None,
            user_id: None,
            template_type: Some("balance".to_string()),
            auto_query_interval: None,
            coding_plan_provider: None,
        };

        let (api_key, base_url) = ProviderService::resolve_usage_script_credentials(
            &provider,
            &AppType::OpenClaw,
            &script,
        )
        .expect("OpenClaw credentials should resolve from provider config");

        assert_eq!(api_key, "sk-openclaw");
        assert_eq!(base_url, "https://openclaw.example/v1");
    }

    #[test]
    fn resolve_usage_script_credentials_prefers_usage_script_over_provider_config() {
        let provider = Provider::with_id(
            "claude".to_string(),
            "Claude Provider".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "sk-provider",
                    "ANTHROPIC_BASE_URL": "https://provider.example/v1"
                }
            }),
            None,
        );
        let script = UsageScript {
            enabled: true,
            language: "javascript".to_string(),
            code: String::new(),
            timeout: None,
            api_key: Some("sk-script".to_string()),
            base_url: Some("https://script.example/v1/".to_string()),
            access_token: None,
            user_id: None,
            template_type: Some("general".to_string()),
            auto_query_interval: None,
            coding_plan_provider: None,
        };

        let (api_key, base_url) =
            ProviderService::resolve_usage_script_credentials(&provider, &AppType::Claude, &script)
                .expect("usage script credentials should resolve");

        assert_eq!(api_key, "sk-script");
        assert_eq!(base_url, "https://script.example/v1");
    }
}
