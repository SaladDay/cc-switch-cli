use reqwest::RequestBuilder;
use serde_json::json;

use crate::{provider::Provider, proxy::error::ProxyError};

use super::{
    gemini_shadow::GeminiShadowStore, transform_gemini::AnthropicToolSchemaHints, AuthInfo,
    AuthStrategy, ProviderAdapter, ProviderType,
};

pub struct ClaudeAdapter;

pub fn get_claude_api_format(provider: &Provider) -> &'static str {
    if let Some(meta) = provider.meta.as_ref() {
        if meta.provider_type.as_deref() == Some("codex_oauth") {
            return "openai_responses";
        }
    }

    if let Some(meta) = provider.meta.as_ref() {
        if let Some(api_format) = meta.api_format.as_deref() {
            return match api_format {
                "openai_chat" => "openai_chat",
                "openai_responses" => "openai_responses",
                "gemini_native" => "gemini_native",
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
            "gemini_native" => "gemini_native",
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

pub fn claude_api_format_needs_transform(api_format: &str) -> bool {
    matches!(
        api_format,
        "openai_chat" | "openai_responses" | "gemini_native"
    )
}

fn is_reasoning_content_compatible_identifier(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("moonshot") || value.contains("kimi") || value.contains("deepseek")
}

fn should_preserve_reasoning_content_for_openai_chat(
    provider: &Provider,
    body: &serde_json::Value,
) -> bool {
    if body
        .get("model")
        .and_then(|m| m.as_str())
        .is_some_and(is_reasoning_content_compatible_identifier)
    {
        return true;
    }

    let settings = &provider.settings_config;
    let base_urls = [
        settings
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
            .and_then(|v| v.as_str()),
        settings.get("base_url").and_then(|v| v.as_str()),
        settings.get("baseURL").and_then(|v| v.as_str()),
        settings.get("apiEndpoint").and_then(|v| v.as_str()),
    ];

    base_urls
        .into_iter()
        .flatten()
        .any(is_reasoning_content_compatible_identifier)
}

pub fn transform_claude_request_for_api_format(
    body: serde_json::Value,
    provider: &Provider,
    api_format: &str,
    session_id: Option<&str>,
) -> Result<serde_json::Value, ProxyError> {
    transform_claude_request_for_api_format_with_shadow(
        body, provider, api_format, session_id, None,
    )
}

pub fn transform_claude_request_for_api_format_with_shadow(
    body: serde_json::Value,
    provider: &Provider,
    api_format: &str,
    session_id: Option<&str>,
    shadow_store: Option<&GeminiShadowStore>,
) -> Result<serde_json::Value, ProxyError> {
    let explicit_cache_key = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.prompt_cache_key.as_deref());
    let session_cache_key = session_id
        .map(str::trim)
        .filter(|session_id| !session_id.is_empty());

    match api_format {
        "openai_responses" => super::transform_responses::anthropic_to_responses(
            body,
            explicit_cache_key.or(session_cache_key),
            provider.is_codex_oauth(),
            provider.codex_fast_mode_enabled(),
        ),
        "openai_chat" => {
            let preserve_reasoning_content =
                should_preserve_reasoning_content_for_openai_chat(provider, &body);
            if preserve_reasoning_content {
                super::transform::anthropic_to_openai_with_reasoning_content(
                    body,
                    explicit_cache_key,
                    true,
                )
            } else {
                super::transform::anthropic_to_openai(body, explicit_cache_key)
            }
        }
        "gemini_native" => super::transform_gemini::anthropic_to_gemini_with_shadow(
            body,
            shadow_store,
            Some(&provider.id),
            session_id,
        ),
        _ => Ok(body),
    }
}

impl ClaudeAdapter {
    pub fn new() -> Self {
        Self
    }

    pub fn provider_type(&self, provider: &Provider) -> ProviderType {
        if self.get_api_format(provider) == "gemini_native" {
            return match self.extract_key(provider) {
                Some(key) if key.starts_with("ya29.") || key.starts_with('{') => {
                    ProviderType::GeminiCli
                }
                _ => ProviderType::Gemini,
            };
        }
        if self.is_codex_oauth(provider) {
            return ProviderType::CodexOAuth;
        }
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

    fn is_codex_oauth(&self, provider: &Provider) -> bool {
        if let Some(meta) = provider.meta.as_ref() {
            if meta.provider_type.as_deref() == Some("codex_oauth") {
                return true;
            }
        }
        false
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
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                return Some(key.to_string());
            }
            if let Some(key) = env
                .get("ANTHROPIC_API_KEY")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                return Some(key.to_string());
            }
            if let Some(key) = env
                .get("OPENROUTER_API_KEY")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                return Some(key.to_string());
            }
            if let Some(key) = env
                .get("OPENAI_API_KEY")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                return Some(key.to_string());
            }
            if let Some(key) = env
                .get("GEMINI_API_KEY")
                .and_then(|v| v.as_str())
                .map(str::trim)
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
            .map(str::trim)
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
        if self.is_codex_oauth(provider) {
            return Ok("https://chatgpt.com/backend-api/codex".to_string());
        }

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

        if provider_type == ProviderType::CodexOAuth {
            return Some(AuthInfo::new(
                "codex_oauth_placeholder".to_string(),
                AuthStrategy::CodexOAuth,
            ));
        }

        if matches!(
            provider_type,
            ProviderType::Gemini | ProviderType::GeminiCli
        ) {
            return self.extract_key(provider).map(|key| match provider_type {
                ProviderType::GeminiCli => {
                    match super::gemini::GeminiAdapter::new().parse_oauth_credentials(&key) {
                        Some(creds) if !creds.access_token.is_empty() => {
                            AuthInfo::with_access_token(key, creds.access_token)
                        }
                        Some(_) => {
                            log::warn!(
                                "[Gemini OAuth] access_token missing or empty for provider `{}`; \
                                 bearer auth will likely fail with 401. Refresh \
                                 ~/.gemini/oauth_creds.json via the gemini CLI to obtain a new token.",
                                provider.id
                            );
                            AuthInfo::new(key, AuthStrategy::GoogleOAuth)
                        }
                        None => AuthInfo::new(key, AuthStrategy::GoogleOAuth),
                    }
                }
                ProviderType::Gemini => AuthInfo::new(key, AuthStrategy::Google),
                _ => unreachable!("Gemini provider type was checked above"),
            });
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
        if base_url == "https://chatgpt.com/backend-api/codex" {
            let _ = endpoint;
            return "https://chatgpt.com/backend-api/codex/responses".to_string();
        }

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
            AuthStrategy::GitHubCopilot => {
                let request_id = uuid::Uuid::new_v4().to_string();
                request
                    .header("Authorization", format!("Bearer {}", auth.api_key))
                    .header(
                        "editor-version",
                        super::copilot_auth::COPILOT_EDITOR_VERSION,
                    )
                    .header(
                        "editor-plugin-version",
                        super::copilot_auth::COPILOT_PLUGIN_VERSION,
                    )
                    .header(
                        "copilot-integration-id",
                        super::copilot_auth::COPILOT_INTEGRATION_ID,
                    )
                    .header("user-agent", super::copilot_auth::COPILOT_USER_AGENT)
                    .header(
                        "x-github-api-version",
                        super::copilot_auth::COPILOT_API_VERSION,
                    )
                    .header("openai-intent", "conversation-agent")
                    .header("x-initiator", "user")
                    .header("x-interaction-type", "conversation-agent")
                    .header("x-vscode-user-agent-library-version", "electron-fetch")
                    .header("x-request-id", &request_id)
                    .header("x-agent-task-id", request_id)
            }
            AuthStrategy::CodexOAuth => request
                .header("Authorization", format!("Bearer {}", auth.api_key))
                .header("originator", "cc-switch"),
            AuthStrategy::Google => request.header("x-goog-api-key", &auth.api_key),
            AuthStrategy::GoogleOAuth => {
                let token = auth.access_token.as_ref().unwrap_or(&auth.api_key);
                request
                    .header("Authorization", format!("Bearer {token}"))
                    .header("x-goog-api-client", "GeminiCLI/1.0")
            }
            AuthStrategy::Bearer => {
                request.header("Authorization", format!("Bearer {}", auth.api_key))
            }
        }
    }

    fn needs_transform(&self, provider: &Provider) -> bool {
        if self.is_codex_oauth(provider) {
            return true;
        }
        if self.is_github_copilot(provider) {
            return true;
        }

        claude_api_format_needs_transform(self.get_api_format(provider))
    }

    fn transform_request(
        &self,
        body: serde_json::Value,
        provider: &Provider,
    ) -> Result<serde_json::Value, ProxyError> {
        transform_claude_request_for_api_format(body, provider, self.get_api_format(provider), None)
    }

    fn transform_response(&self, body: serde_json::Value) -> Result<serde_json::Value, ProxyError> {
        if body.get("error").is_some()
            && body.get("choices").is_none()
            && body.get("output").is_none()
        {
            return Ok(openai_error_to_anthropic(body));
        }

        if body.get("candidates").is_some() || body.get("promptFeedback").is_some() {
            super::transform_gemini::gemini_to_anthropic(body)
        } else if body.get("output").is_some() {
            super::transform_responses::responses_to_anthropic(body)
        } else {
            super::transform::openai_to_anthropic(body)
        }
    }
}

pub fn transform_gemini_response_for_provider(
    body: serde_json::Value,
    provider: &Provider,
    session_id: Option<&str>,
    shadow_store: Option<&GeminiShadowStore>,
    tool_schema_hints: Option<&AnthropicToolSchemaHints>,
) -> Result<serde_json::Value, ProxyError> {
    super::transform_gemini::gemini_to_anthropic_with_shadow_and_hints(
        body,
        shadow_store,
        Some(&provider.id),
        session_id,
        tool_schema_hints,
    )
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

    fn claude_gemini_native_provider(key: &str) -> Provider {
        serde_json::from_value(json!({
            "id": "claude-gemini-native",
            "name": "Claude Gemini Native",
            "settingsConfig": {
                "env": {
                    "ANTHROPIC_BASE_URL": "https://generativelanguage.googleapis.com",
                    "ANTHROPIC_API_KEY": key
                }
            },
            "meta": {
                "apiFormat": "gemini_native"
            }
        }))
        .expect("provider should deserialize")
    }

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

    #[test]
    fn provider_meta_provider_type_codex_oauth_uses_responses_runtime_behavior() {
        let adapter = ClaudeAdapter::new();
        let provider: Provider = serde_json::from_value(json!({
            "id": "codex-oauth-meta",
            "name": "Codex OAuth",
            "settingsConfig": {
                "env": {
                    "ANTHROPIC_BASE_URL": "https://relay.example.com",
                    "ANTHROPIC_AUTH_TOKEN": "token-1"
                }
            },
            "meta": {
                "providerType": "codex_oauth"
            }
        }))
        .expect("provider should deserialize");

        assert_eq!(get_claude_api_format(&provider), "openai_responses");
        assert_eq!(
            format!("{:?}", adapter.provider_type(&provider)),
            "CodexOAuth"
        );
        assert_eq!(
            adapter
                .extract_base_url(&provider)
                .expect("codex oauth base url"),
            "https://chatgpt.com/backend-api/codex"
        );
        let auth = adapter
            .extract_auth(&provider)
            .expect("codex oauth should resolve auth");
        assert_eq!(format!("{:?}", auth.strategy), "CodexOAuth");
        assert!(adapter.needs_transform(&provider));
    }

    #[test]
    fn gemini_native_oauth_access_token_is_trimmed_and_classified() {
        let adapter = ClaudeAdapter::new();
        let provider = claude_gemini_native_provider("\nya29.raw-token-value\n");

        assert_eq!(adapter.provider_type(&provider), ProviderType::GeminiCli);

        let auth = adapter
            .extract_auth(&provider)
            .expect("gemini native oauth should resolve auth");
        assert_eq!(auth.strategy, AuthStrategy::GoogleOAuth);
        assert_eq!(auth.api_key, "ya29.raw-token-value");
        assert_eq!(auth.access_token.as_deref(), Some("ya29.raw-token-value"));
    }

    #[test]
    fn gemini_native_oauth_refresh_only_json_does_not_expose_empty_bearer() {
        let adapter = ClaudeAdapter::new();
        let provider = claude_gemini_native_provider(
            r#"{"refresh_token":"rt-abc","client_id":"cid","client_secret":"cs"}"#,
        );

        assert_eq!(adapter.provider_type(&provider), ProviderType::GeminiCli);

        let auth = adapter
            .extract_auth(&provider)
            .expect("gemini native refresh-only oauth should resolve auth");
        assert_eq!(auth.strategy, AuthStrategy::GoogleOAuth);
        assert_eq!(auth.access_token, None);
    }

    #[test]
    fn gemini_native_oauth_empty_access_token_json_does_not_expose_empty_bearer() {
        let adapter = ClaudeAdapter::new();
        let provider = claude_gemini_native_provider(
            r#"{"access_token":"","refresh_token":"rt-abc","client_id":"cid","client_secret":"cs"}"#,
        );

        assert_eq!(adapter.provider_type(&provider), ProviderType::GeminiCli);

        let auth = adapter
            .extract_auth(&provider)
            .expect("gemini native expired oauth should resolve auth");
        assert_eq!(auth.strategy, AuthStrategy::GoogleOAuth);
        assert_eq!(auth.access_token, None);
    }

    #[test]
    fn gemini_native_oauth_valid_json_keeps_access_token() {
        let adapter = ClaudeAdapter::new();
        let provider = claude_gemini_native_provider(
            "\n  {\"access_token\":\"ya29.valid\",\"refresh_token\":\"rt\"}\n",
        );

        assert_eq!(adapter.provider_type(&provider), ProviderType::GeminiCli);

        let auth = adapter
            .extract_auth(&provider)
            .expect("gemini native json oauth should resolve auth");
        assert_eq!(auth.strategy, AuthStrategy::GoogleOAuth);
        assert_eq!(auth.access_token.as_deref(), Some("ya29.valid"));
    }

    #[test]
    fn openai_chat_transform_preserves_reasoning_content_for_deepseek_model() {
        let provider: Provider = serde_json::from_value(json!({
            "id": "deepseek",
            "name": "DeepSeek",
            "settingsConfig": {
                "api_format": "openai_chat",
                "env": {
                    "ANTHROPIC_BASE_URL": "https://api.deepseek.com",
                    "ANTHROPIC_AUTH_TOKEN": "token-1"
                }
            }
        }))
        .expect("provider should deserialize");
        let body = json!({
            "model": "deepseek-v4-pro",
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "I should call the tool."},
                    {"type": "tool_use", "id": "call_1", "name": "get_weather", "input": {}}
                ]
            }]
        });

        let result =
            transform_claude_request_for_api_format(body, &provider, "openai_chat", None).unwrap();

        assert_eq!(
            result["messages"][0]["reasoning_content"],
            "I should call the tool."
        );
    }

    #[test]
    fn openai_chat_transform_skips_reasoning_content_for_generic_provider() {
        let provider: Provider = serde_json::from_value(json!({
            "id": "generic",
            "name": "Generic",
            "settingsConfig": {
                "api_format": "openai_chat",
                "env": {
                    "ANTHROPIC_BASE_URL": "https://api.example.com",
                    "ANTHROPIC_AUTH_TOKEN": "token-1"
                }
            }
        }))
        .expect("provider should deserialize");
        let body = json!({
            "model": "gpt-4o",
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "I should call the tool."},
                    {"type": "tool_use", "id": "call_1", "name": "get_weather", "input": {}}
                ]
            }]
        });

        let result =
            transform_claude_request_for_api_format(body, &provider, "openai_chat", None).unwrap();

        assert!(result["messages"][0].get("reasoning_content").is_none());
    }

    #[test]
    fn openai_responses_uses_session_prompt_cache_key() {
        let provider: Provider = serde_json::from_value(json!({
            "id": "codex-oauth",
            "name": "Codex OAuth",
            "settingsConfig": {},
            "meta": {
                "providerType": "codex_oauth"
            }
        }))
        .expect("provider should deserialize");
        let body = json!({
            "model": "gpt-5.4",
            "messages": [{"role": "user", "content": "hello"}]
        });

        let result = transform_claude_request_for_api_format(
            body,
            &provider,
            "openai_responses",
            Some("codex_session-123"),
        )
        .unwrap();

        assert_eq!(result["prompt_cache_key"], "codex_session-123");
    }

    #[test]
    fn openai_responses_omits_prompt_cache_key_without_session_or_explicit_key() {
        let provider: Provider = serde_json::from_value(json!({
            "id": "codex-oauth",
            "name": "Codex OAuth",
            "settingsConfig": {},
            "meta": {
                "providerType": "codex_oauth"
            }
        }))
        .expect("provider should deserialize");
        let body = json!({
            "model": "gpt-5.4",
            "messages": [{"role": "user", "content": "hello"}]
        });

        let result =
            transform_claude_request_for_api_format(body, &provider, "openai_responses", None)
                .unwrap();

        assert!(result.get("prompt_cache_key").is_none());
    }

    #[test]
    fn openai_responses_explicit_prompt_cache_key_wins_over_session() {
        let provider: Provider = serde_json::from_value(json!({
            "id": "codex-oauth",
            "name": "Codex OAuth",
            "settingsConfig": {},
            "meta": {
                "providerType": "codex_oauth",
                "promptCacheKey": "explicit-key"
            }
        }))
        .expect("provider should deserialize");
        let body = json!({
            "model": "gpt-5.4",
            "messages": [{"role": "user", "content": "hello"}]
        });

        let result = transform_claude_request_for_api_format(
            body,
            &provider,
            "openai_responses",
            Some("codex_session-123"),
        )
        .unwrap();

        assert_eq!(result["prompt_cache_key"], "explicit-key");
    }

    #[test]
    fn openai_chat_omits_prompt_cache_key_without_explicit_key() {
        let provider: Provider = serde_json::from_value(json!({
            "id": "generic",
            "name": "Generic",
            "settingsConfig": {
                "api_format": "openai_chat",
                "env": {
                    "ANTHROPIC_BASE_URL": "https://api.example.com",
                    "ANTHROPIC_AUTH_TOKEN": "token-1"
                }
            }
        }))
        .expect("provider should deserialize");
        let body = json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "hello"}]
        });

        let result = transform_claude_request_for_api_format(
            body,
            &provider,
            "openai_chat",
            Some("session-ignored"),
        )
        .unwrap();

        assert!(result.get("prompt_cache_key").is_none());
    }
}
