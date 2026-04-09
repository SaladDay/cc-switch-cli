use reqwest::RequestBuilder;
use serde_json::json;

use crate::{provider::Provider, proxy::error::ProxyError};

use super::{AuthInfo, AuthStrategy, ProviderAdapter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderType {
    Claude,
    ClaudeAuth,
    OpenRouter,
    GitHubCopilot,
}

pub struct ClaudeAdapter;

pub fn get_claude_api_format(provider: &Provider) -> &'static str {
    if let Some(meta) = provider.meta.as_ref() {
        if let Some(api_format) = meta.api_format.as_deref() {
            return match api_format {
                "openai_chat" => "openai_chat",
                "openai_responses" => "openai_responses",
                _ => "anthropic",
            };
        }
    }

    if let Some(api_format) = provider
        .settings_config
        .get("api_format")
        .and_then(|v| v.as_str())
    {
        return match api_format {
            "openai_chat" => "openai_chat",
            "openai_responses" => "openai_responses",
            _ => "anthropic",
        };
    }

    let raw = provider.settings_config.get("openrouter_compat_mode");
    let enabled = match raw {
        Some(serde_json::Value::Bool(v)) => *v,
        Some(serde_json::Value::Number(num)) => num.as_i64().unwrap_or(0) != 0,
        Some(serde_json::Value::String(value)) => {
            let normalized = value.trim().to_lowercase();
            normalized == "true" || normalized == "1"
        }
        _ => false,
    };

    if enabled {
        "openai_chat"
    } else {
        "anthropic"
    }
}

impl ClaudeAdapter {
    pub fn new() -> Self {
        Self
    }

    pub fn provider_type(&self, provider: &Provider) -> ProviderType {
        if self.is_github_copilot(provider) {
            return ProviderType::GitHubCopilot;
        }
        if self.is_openrouter(provider) {
            return ProviderType::OpenRouter;
        }
        if self.is_bearer_only_mode(provider) {
            return ProviderType::ClaudeAuth;
        }
        ProviderType::Claude
    }

    fn is_openrouter(&self, provider: &Provider) -> bool {
        self.extract_base_url(provider)
            .map(|base_url| base_url.contains("openrouter.ai"))
            .unwrap_or(false)
    }

    fn is_github_copilot(&self, provider: &Provider) -> bool {
        if let Some(meta) = provider.meta.as_ref() {
            if meta.provider_type.as_deref() == Some("github_copilot") {
                return true;
            }
        }

        self.extract_base_url(provider)
            .map(|base_url| base_url.contains("githubcopilot.com"))
            .unwrap_or(false)
    }

    fn get_api_format(&self, provider: &Provider) -> &'static str {
        get_claude_api_format(provider)
    }

    fn is_bearer_only_mode(&self, provider: &Provider) -> bool {
        if let Some(auth_mode) = provider
            .settings_config
            .get("auth_mode")
            .and_then(|v| v.as_str())
        {
            if auth_mode == "bearer_only" {
                return true;
            }
        }

        if let Some(env) = provider.settings_config.get("env") {
            if let Some(auth_mode) = env.get("AUTH_MODE").and_then(|v| v.as_str()) {
                if auth_mode == "bearer_only" {
                    return true;
                }
            }
        }

        false
    }

    fn extract_key(&self, provider: &Provider) -> Option<String> {
        if let Some(env) = provider.settings_config.get("env") {
            if let Some(key) = env
                .get("ANTHROPIC_AUTH_TOKEN")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                return Some(key.to_string());
            }
            if let Some(key) = env
                .get("ANTHROPIC_API_KEY")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                return Some(key.to_string());
            }
            if let Some(key) = env
                .get("OPENROUTER_API_KEY")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                return Some(key.to_string());
            }
            if let Some(key) = env
                .get("OPENAI_API_KEY")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                return Some(key.to_string());
            }
        }

        provider
            .settings_config
            .get("apiKey")
            .or_else(|| provider.settings_config.get("api_key"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    }
}

impl Default for ClaudeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderAdapter for ClaudeAdapter {
    fn name(&self) -> &'static str {
        "Claude"
    }

    fn extract_base_url(&self, provider: &Provider) -> Result<String, ProxyError> {
        if let Some(env) = provider.settings_config.get("env") {
            if let Some(url) = env.get("ANTHROPIC_BASE_URL").and_then(|v| v.as_str()) {
                return Ok(url.trim_end_matches('/').to_string());
            }
        }

        if let Some(url) = provider
            .settings_config
            .get("base_url")
            .and_then(|v| v.as_str())
        {
            return Ok(url.trim_end_matches('/').to_string());
        }

        if let Some(url) = provider
            .settings_config
            .get("baseURL")
            .and_then(|v| v.as_str())
        {
            return Ok(url.trim_end_matches('/').to_string());
        }

        if let Some(url) = provider
            .settings_config
            .get("apiEndpoint")
            .and_then(|v| v.as_str())
        {
            return Ok(url.trim_end_matches('/').to_string());
        }

        Err(ProxyError::ConfigError(
            "Claude Provider 缺少 base_url 配置".to_string(),
        ))
    }

    fn extract_auth(&self, provider: &Provider) -> Option<AuthInfo> {
        let provider_type = self.provider_type(provider);

        if provider_type == ProviderType::GitHubCopilot {
            return Some(AuthInfo::new(
                "copilot_placeholder".to_string(),
                AuthStrategy::GitHubCopilot,
            ));
        }

        let strategy = match provider_type {
            ProviderType::OpenRouter => AuthStrategy::Bearer,
            ProviderType::ClaudeAuth => AuthStrategy::ClaudeAuth,
            _ => AuthStrategy::Anthropic,
        };
        self.extract_key(provider)
            .map(|key| AuthInfo::new(key, strategy))
    }

    fn build_url(&self, base_url: &str, endpoint: &str) -> String {
        let mut base = format!(
            "{}/{}",
            base_url.trim_end_matches('/'),
            endpoint.trim_start_matches('/')
        );

        while base.contains("/v1/v1") {
            base = base.replace("/v1/v1", "/v1");
        }

        if endpoint.contains("/v1/messages")
            && !endpoint.contains("/v1/chat/completions")
            && !endpoint.contains('?')
        {
            format!("{base}?beta=true")
        } else {
            base
        }
    }

    fn add_auth_headers(&self, request: RequestBuilder, auth: &AuthInfo) -> RequestBuilder {
        match auth.strategy {
            AuthStrategy::Anthropic => request
                .header("Authorization", format!("Bearer {}", auth.api_key))
                .header("x-api-key", &auth.api_key),
            AuthStrategy::ClaudeAuth => {
                request.header("Authorization", format!("Bearer {}", auth.api_key))
            }
            AuthStrategy::GitHubCopilot => request
                .header("Authorization", format!("Bearer {}", auth.api_key))
                .header("Editor-Version", "vscode/1.85.0")
                .header("Editor-Plugin-Version", "copilot/1.150.0")
                .header("Copilot-Integration-Id", "vscode-chat"),
            AuthStrategy::Bearer | AuthStrategy::Google | AuthStrategy::GoogleOAuth => {
                request.header("Authorization", format!("Bearer {}", auth.api_key))
            }
        }
    }

    fn needs_transform(&self, provider: &Provider) -> bool {
        if self.is_github_copilot(provider) {
            return true;
        }

        matches!(
            self.get_api_format(provider),
            "openai_chat" | "openai_responses"
        )
    }

    fn transform_request(
        &self,
        body: serde_json::Value,
        provider: &Provider,
    ) -> Result<serde_json::Value, ProxyError> {
        let cache_key = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.prompt_cache_key.as_deref())
            .unwrap_or(&provider.id);

        let api_format = self.get_api_format(provider);
        let stream_include_usage = provider.stream_include_usage();

        if stream_include_usage {
            log::info!(
                "[ClaudeAdapter] Provider '{}' stream_include_usage enabled",
                provider.name
            );
        }

        match api_format {
            "openai_responses" => {
                super::transform_responses::anthropic_to_responses(body, Some(cache_key))
            }
            _ => super::transform::anthropic_to_openai(body, Some(cache_key), stream_include_usage),
        }
    }

    fn transform_response(&self, body: serde_json::Value) -> Result<serde_json::Value, ProxyError> {
        if body.get("error").is_some()
            && body.get("choices").is_none()
            && body.get("output").is_none()
        {
            return Ok(openai_error_to_anthropic(body));
        }

        if body.get("output").is_some() {
            super::transform_responses::responses_to_anthropic(body)
        } else {
            super::transform::openai_to_anthropic(body)
        }
    }
}

fn openai_error_to_anthropic(body: serde_json::Value) -> serde_json::Value {
    let error = body.get("error").cloned().unwrap_or_else(|| json!({}));
    let message = error
        .get("message")
        .and_then(|value| value.as_str())
        .unwrap_or("Upstream error");
    let error_type = error
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("invalid_request_error");

    json!({
        "type": "error",
        "error": {
            "type": error_type,
            "message": message
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider;
    use serde_json::json;

    #[test]
    fn provider_meta_provider_type_github_copilot_uses_upstream_runtime_behavior() {
        let adapter = ClaudeAdapter::new();
        let provider: Provider = serde_json::from_value(json!({
            "id": "copilot-meta",
            "name": "Copilot Meta",
            "settingsConfig": {
                "env": {
                    "ANTHROPIC_BASE_URL": "https://relay.example.com",
                    "ANTHROPIC_AUTH_TOKEN": "token-1"
                }
            },
            "meta": {
                "providerType": "github_copilot"
            }
        }))
        .expect("provider should deserialize");

        assert_eq!(
            format!("{:?}", adapter.provider_type(&provider)),
            "GitHubCopilot"
        );
        let auth = adapter
            .extract_auth(&provider)
            .expect("github copilot should resolve auth");
        assert_eq!(format!("{:?}", auth.strategy), "GitHubCopilot");
        assert!(adapter.needs_transform(&provider));
    }
}
