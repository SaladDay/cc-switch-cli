use regex::Regex;
use reqwest::{Client, Proxy};

use crate::{
    app_config::AppType,
    error::AppError,
    provider::{Provider, ProviderProxyConfig},
};

use super::service::StreamCheckService;
use super::types::{AuthInfo, AuthStrategy, StreamCheckConfig};

impl StreamCheckService {
    pub(crate) fn resolve_test_model(
        app_type: &AppType,
        provider: &Provider,
        config: &StreamCheckConfig,
    ) -> String {
        match app_type {
            AppType::Claude => Self::extract_env_model(provider, "ANTHROPIC_MODEL")
                .unwrap_or_else(|| config.claude_model.clone()),
            AppType::Codex => {
                Self::extract_codex_model(provider).unwrap_or_else(|| config.codex_model.clone())
            }
            AppType::Gemini => Self::extract_env_model(provider, "GEMINI_MODEL")
                .unwrap_or_else(|| config.gemini_model.clone()),
            AppType::OpenCode => provider
                .settings_config
                .get("models")
                .and_then(|value| value.as_object())
                .and_then(|models| models.keys().next().cloned())
                .unwrap_or_else(|| config.codex_model.clone()),
            AppType::OpenClaw => {
                Self::extract_openclaw_model(provider).unwrap_or_else(|| config.codex_model.clone())
            }
        }
    }

    pub(crate) fn extract_env_model(provider: &Provider, key: &str) -> Option<String> {
        provider
            .settings_config
            .get("env")
            .and_then(|env| env.get(key))
            .and_then(|value| value.as_str())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    pub(crate) fn extract_codex_model(provider: &Provider) -> Option<String> {
        let config_text = provider
            .settings_config
            .get("config")
            .and_then(|value| value.as_str())?;
        if config_text.trim().is_empty() {
            return None;
        }

        let re = Regex::new(r#"(?m)^model\s*=\s*[\"']([^\"']+)[\"']"#).ok()?;
        re.captures(config_text)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().trim().to_string())
            .filter(|value| !value.is_empty())
    }

    pub(crate) fn extract_base_url(
        provider: &Provider,
        app_type: &AppType,
    ) -> Result<String, AppError> {
        match app_type {
            AppType::Claude => provider
                .settings_config
                .get("env")
                .and_then(|value| value.as_object())
                .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
                .and_then(|value| value.as_str())
                .or_else(|| {
                    provider
                        .settings_config
                        .get("base_url")
                        .and_then(|value| value.as_str())
                })
                .or_else(|| {
                    provider
                        .settings_config
                        .get("baseURL")
                        .and_then(|value| value.as_str())
                })
                .or_else(|| {
                    provider
                        .settings_config
                        .get("apiEndpoint")
                        .and_then(|value| value.as_str())
                })
                .map(|value| value.trim_end_matches('/').to_string())
                .ok_or_else(|| {
                    AppError::localized(
                        "provider.claude.base_url.missing",
                        "缺少 ANTHROPIC_BASE_URL 配置",
                        "Missing ANTHROPIC_BASE_URL configuration",
                    )
                }),
            AppType::Codex => {
                if let Some(url) = provider
                    .settings_config
                    .get("base_url")
                    .and_then(|value| value.as_str())
                {
                    return Ok(url.trim_end_matches('/').to_string());
                }
                if let Some(url) = provider
                    .settings_config
                    .get("baseURL")
                    .and_then(|value| value.as_str())
                {
                    return Ok(url.trim_end_matches('/').to_string());
                }

                let config = provider.settings_config.get("config");
                if let Some(url) = config
                    .and_then(|value| value.get("base_url"))
                    .and_then(|v| v.as_str())
                {
                    return Ok(url.trim_end_matches('/').to_string());
                }
                if let Some(config_text) = config.and_then(|value| value.as_str()) {
                    if let Some(start) = config_text.find("base_url = \"") {
                        let rest = &config_text[start + 12..];
                        if let Some(end) = rest.find('"') {
                            return Ok(rest[..end].trim_end_matches('/').to_string());
                        }
                    }
                    if let Some(start) = config_text.find("base_url = '") {
                        let rest = &config_text[start + 12..];
                        if let Some(end) = rest.find('\'') {
                            return Ok(rest[..end].trim_end_matches('/').to_string());
                        }
                    }
                }

                Err(AppError::localized(
                    "provider.codex.base_url.missing",
                    "config.toml 中缺少 base_url 配置",
                    "base_url is missing from config.toml",
                ))
            }
            AppType::Gemini => {
                use crate::gemini_config::json_to_env;
                let env_map = json_to_env(&provider.settings_config)?;
                Ok(env_map
                    .get("GOOGLE_GEMINI_BASE_URL")
                    .cloned()
                    .or_else(|| env_map.get("GEMINI_BASE_URL").cloned())
                    .or_else(|| env_map.get("BASE_URL").cloned())
                    .unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string())
                    .trim_end_matches('/')
                    .to_string())
            }
            AppType::OpenCode => Ok(provider
                .settings_config
                .get("options")
                .and_then(|value| value.get("baseURL"))
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .trim_end_matches('/')
                .to_string()),
            AppType::OpenClaw => Self::extract_openclaw_base_url(provider).ok_or_else(|| {
                AppError::localized(
                    "provider.openclaw.base_url.missing",
                    "缺少 OpenClaw baseUrl 配置",
                    "Missing OpenClaw baseUrl configuration",
                )
            }),
        }
    }

    pub(crate) fn extract_auth(
        provider: &Provider,
        app_type: &AppType,
        base_url: &str,
    ) -> Result<AuthInfo, AppError> {
        match app_type {
            AppType::Claude => {
                let strategy = Self::detect_claude_auth_strategy(provider, base_url);
                let api_key = Self::extract_claude_key(provider).ok_or_else(|| {
                    AppError::localized(
                        "provider.claude.api_key.missing",
                        "缺少 API Key",
                        "API key is missing",
                    )
                })?;
                Ok(AuthInfo::new(api_key, strategy))
            }
            AppType::Codex => Self::extract_codex_key(provider)
                .map(|key| AuthInfo::new(key, AuthStrategy::Bearer))
                .ok_or_else(|| {
                    AppError::localized(
                        "provider.codex.api_key.missing",
                        "缺少 API Key",
                        "API key is missing",
                    )
                }),
            AppType::Gemini => Self::extract_gemini_auth(provider),
            AppType::OpenCode => provider
                .settings_config
                .get("options")
                .and_then(|value| value.get("apiKey"))
                .and_then(|value| value.as_str())
                .map(|key| AuthInfo::new(key.to_string(), AuthStrategy::Bearer))
                .ok_or_else(|| {
                    AppError::localized(
                        "provider.opencode.api_key.missing",
                        "缺少 API Key",
                        "API key is missing",
                    )
                }),
            AppType::OpenClaw => Self::extract_openclaw_key(provider)
                .map(|key| AuthInfo::new(key, AuthStrategy::Bearer))
                .ok_or_else(|| {
                    AppError::localized(
                        "provider.openclaw.api_key.missing",
                        "缺少 API Key",
                        "API key is missing",
                    )
                }),
        }
    }

    pub(crate) fn extract_openclaw_provider_config(provider: &Provider) -> serde_json::Value {
        crate::openclaw_config::provider_config_from_settings(&provider.settings_config)
    }

    pub(crate) fn extract_openclaw_model(provider: &Provider) -> Option<String> {
        if let Some(primary_model) =
            crate::openclaw_config::primary_model_from_settings(&provider.settings_config)
        {
            let suffix = primary_model
                .rsplit_once('/')
                .map(|(_, model_id)| model_id)
                .unwrap_or(primary_model.as_str())
                .trim();
            if !suffix.is_empty() {
                return Some(suffix.to_string());
            }
        }

        Self::extract_openclaw_provider_config(provider)
            .get("models")
            .and_then(|value| value.as_array())
            .and_then(|models| models.first())
            .and_then(|model| model.get("id"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }

    pub(crate) fn extract_openclaw_base_url(provider: &Provider) -> Option<String> {
        Self::extract_openclaw_provider_config(provider)
            .get("baseUrl")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.trim_end_matches('/').to_string())
    }

    pub(crate) fn extract_openclaw_key(provider: &Provider) -> Option<String> {
        Self::extract_openclaw_provider_config(provider)
            .get("apiKey")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }

    pub(crate) fn detect_claude_auth_strategy(provider: &Provider, base_url: &str) -> AuthStrategy {
        if base_url.contains("openrouter.ai") {
            return AuthStrategy::Bearer;
        }

        let auth_mode = provider
            .settings_config
            .get("auth_mode")
            .and_then(|value| value.as_str())
            .or_else(|| {
                provider
                    .settings_config
                    .get("env")
                    .and_then(|env| env.get("AUTH_MODE"))
                    .and_then(|value| value.as_str())
            });

        match auth_mode {
            Some("bearer_only") => AuthStrategy::ClaudeAuth,
            _ => AuthStrategy::Anthropic,
        }
    }

    pub(crate) fn extract_claude_key(provider: &Provider) -> Option<String> {
        if let Some(env) = provider.settings_config.get("env") {
            for key_name in [
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_API_KEY",
                "OPENROUTER_API_KEY",
                "OPENAI_API_KEY",
            ] {
                if let Some(key) = env.get(key_name).and_then(|value| value.as_str()) {
                    if !key.trim().is_empty() {
                        return Some(key.to_string());
                    }
                }
            }
        }

        provider
            .settings_config
            .get("apiKey")
            .or_else(|| provider.settings_config.get("api_key"))
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
            .filter(|value| !value.trim().is_empty())
    }

    pub(crate) fn extract_codex_key(provider: &Provider) -> Option<String> {
        if let Some(env) = provider.settings_config.get("env") {
            if let Some(key) = env.get("OPENAI_API_KEY").and_then(|value| value.as_str()) {
                if !key.trim().is_empty() {
                    return Some(key.to_string());
                }
            }
        }

        if let Some(auth) = provider.settings_config.get("auth") {
            if let Some(key) = auth.get("OPENAI_API_KEY").and_then(|value| value.as_str()) {
                if !key.trim().is_empty() {
                    return Some(key.to_string());
                }
            }
        }

        if let Some(key) = provider
            .settings_config
            .get("apiKey")
            .or_else(|| provider.settings_config.get("api_key"))
            .and_then(|value| value.as_str())
        {
            if !key.trim().is_empty() {
                return Some(key.to_string());
            }
        }

        provider
            .settings_config
            .get("config")
            .and_then(|value| value.get("api_key").or_else(|| value.get("apiKey")))
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
            .filter(|value| !value.trim().is_empty())
    }

    pub(crate) fn extract_gemini_auth(provider: &Provider) -> Result<AuthInfo, AppError> {
        use crate::gemini_config::json_to_env;
        let env_map = json_to_env(&provider.settings_config)?;

        if let Some(token) = env_map
            .get("GOOGLE_ACCESS_TOKEN")
            .or_else(|| env_map.get("GEMINI_ACCESS_TOKEN"))
            .cloned()
            .filter(|value| !value.trim().is_empty())
        {
            return Ok(AuthInfo::with_access_token(token.clone(), token));
        }

        let key = env_map.get("GEMINI_API_KEY").cloned().ok_or_else(|| {
            AppError::localized(
                "gemini.missing_api_key",
                "缺少 GEMINI_API_KEY",
                "Missing GEMINI_API_KEY",
            )
        })?;

        let trimmed = key.trim();
        if trimmed.starts_with("ya29.") {
            return Ok(AuthInfo::with_access_token(key.clone(), key));
        }
        if trimmed.starts_with('{') {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if let Some(access_token) = value.get("access_token").and_then(|v| v.as_str()) {
                    if !access_token.trim().is_empty() {
                        return Ok(AuthInfo::with_access_token(key, access_token.to_string()));
                    }
                }
            }
        }

        Ok(AuthInfo::new(key, AuthStrategy::Google))
    }

    pub(crate) fn build_client_for_provider(provider: &Provider) -> Result<Client, AppError> {
        let mut builder = Client::builder().redirect(reqwest::redirect::Policy::limited(5));

        if let Some(proxy_config) = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.proxy_config.as_ref())
            .filter(|cfg| cfg.enabled)
        {
            builder = Self::apply_proxy(builder, proxy_config)?;
        }

        builder.build().map_err(|err| {
            AppError::localized(
                "stream_check.client_create_failed",
                format!("创建 HTTP 客户端失败: {err}"),
                format!("Failed to create HTTP client: {err}"),
            )
        })
    }

    pub(crate) fn apply_proxy(
        builder: reqwest::ClientBuilder,
        proxy_config: &ProviderProxyConfig,
    ) -> Result<reqwest::ClientBuilder, AppError> {
        let proxy_type = proxy_config
            .proxy_type
            .as_deref()
            .unwrap_or("http")
            .to_lowercase();
        if proxy_type != "http" && proxy_type != "https" {
            return Err(AppError::Message(format!(
                "stream check 暂不支持 {proxy_type} 代理"
            )));
        }

        let host = proxy_config
            .proxy_host
            .as_deref()
            .ok_or_else(|| AppError::Message("代理配置缺少 proxyHost".to_string()))?;
        let port =
            proxy_config
                .proxy_port
                .unwrap_or_else(|| if proxy_type == "https" { 443 } else { 80 });
        let proxy_url = format!("{proxy_type}://{host}:{port}");
        let mut proxy = Proxy::all(&proxy_url)
            .map_err(|err| AppError::Message(format!("无效代理配置: {err}")))?;

        if let Some(username) = proxy_config
            .proxy_username
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            proxy = proxy.basic_auth(
                username,
                proxy_config
                    .proxy_password
                    .clone()
                    .unwrap_or_default()
                    .as_str(),
            );
        }

        Ok(builder.proxy(proxy))
    }
}
