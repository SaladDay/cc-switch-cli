use reqwest::{Client, Proxy};

use crate::{
    app_config::AppType,
    error::AppError,
    provider::{Provider, ProviderProxyConfig},
    proxy::providers::get_adapter,
};

use super::service::StreamCheckService;

impl StreamCheckService {
    pub(crate) fn extract_base_url(
        provider: &Provider,
        app_type: &AppType,
    ) -> Result<String, AppError> {
        match app_type {
            AppType::OpenCode => {
                let npm = Self::extract_opencode_npm(provider);
                Self::resolve_opencode_base_url(provider, npm.as_deref())
            }
            AppType::Hermes => Self::extract_hermes_base_url(provider),
            AppType::OpenClaw => Self::extract_openclaw_base_url(provider),
            AppType::Claude | AppType::Codex | AppType::Gemini => get_adapter(app_type)
                .extract_base_url(provider)
                .map(|url| url.trim().trim_end_matches('/').to_string())
                .map_err(|err| AppError::Message(format!("Failed to extract base_url: {err}"))),
        }
    }

    /// OpenCode: `{ npm, options: { baseURL, apiKey }, ... }`
    ///
    /// 用户未显式填 `options.baseURL` 时，按 `npm`（AI SDK 包）回退到包自带默认端点。
    /// `@ai-sdk/openai-compatible` 无默认端点，必须显式填。
    pub(crate) fn resolve_opencode_base_url(
        provider: &Provider,
        npm: Option<&str>,
    ) -> Result<String, AppError> {
        if let Some(explicit) = Self::extract_opencode_base_url(provider) {
            return Ok(explicit);
        }

        let fallback = match npm {
            Some("@ai-sdk/openai") => Some("https://api.openai.com/v1"),
            Some("@ai-sdk/anthropic") => Some("https://api.anthropic.com"),
            Some("@ai-sdk/google") => Some("https://generativelanguage.googleapis.com"),
            _ => None,
        };

        fallback.map(str::to_string).ok_or_else(|| {
            AppError::localized(
                "opencode_base_url_missing",
                "OpenCode 供应商缺少 options.baseURL，且当前 SDK 包没有默认端点",
                "OpenCode provider is missing `options.baseURL` and the SDK package has no default endpoint",
            )
        })
    }

    fn extract_opencode_base_url(provider: &Provider) -> Option<String> {
        provider
            .settings_config
            .get("options")
            .and_then(|value| value.get("baseURL"))
            .and_then(|value| value.as_str())
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty())
    }

    fn extract_opencode_npm(provider: &Provider) -> Option<String> {
        provider
            .settings_config
            .get("npm")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }

    fn extract_hermes_base_url(provider: &Provider) -> Result<String, AppError> {
        provider
            .settings_config
            .get("base_url")
            .or_else(|| provider.settings_config.get("baseUrl"))
            .or_else(|| provider.settings_config.get("baseURL"))
            .or_else(|| provider.settings_config.get("endpoint"))
            .and_then(|value| value.as_str())
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AppError::localized(
                    "hermes_base_url_missing",
                    "Hermes 供应商缺少 base_url",
                    "Hermes provider is missing `base_url`",
                )
            })
    }

    fn extract_openclaw_base_url(provider: &Provider) -> Result<String, AppError> {
        provider
            .settings_config
            .get("baseUrl")
            .or_else(|| provider.settings_config.get("baseURL"))
            .or_else(|| provider.settings_config.get("base_url"))
            .and_then(|value| value.as_str())
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AppError::localized(
                    "openclaw_base_url_missing",
                    "OpenClaw 供应商缺少 baseUrl",
                    "OpenClaw provider is missing `baseUrl`",
                )
            })
    }

    pub(crate) fn extract_claude_key(provider: &Provider) -> Option<String> {
        if let Some(env) = provider.settings_config.get("env") {
            for key_name in [
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_API_KEY",
                "OPENROUTER_API_KEY",
                "OPENAI_API_KEY",
                "GEMINI_API_KEY",
            ] {
                if let Some(key) = env.get(key_name).and_then(|value| value.as_str()) {
                    let key = key.trim();
                    if !key.is_empty() {
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
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
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
            .map(str::to_string)
            .filter(|value| !value.trim().is_empty())
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
