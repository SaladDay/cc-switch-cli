use reqwest::RequestBuilder;
use serde_json::{json, Value};

use crate::{provider::Provider, proxy::error::ProxyError};

use super::{
    gemini_shadow::GeminiShadowStore, transform_gemini::AnthropicToolSchemaHints, AuthInfo,
    AuthStrategy, ProviderAdapter, ProviderType,
};

pub struct ClaudeAdapter;

const ANTHROPIC_THINKING_PLACEHOLDER: &str = "tool call";
const ANTHROPIC_REDACTED_THINKING_PLACEHOLDER: &str = "[redacted thinking]";
const REASONING_VENDOR_HINTS: &[&str] = &["moonshot", "kimi", "deepseek", "mimo", "xiaomimimo"];

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

fn is_reasoning_vendor_identifier(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    REASONING_VENDOR_HINTS
        .iter()
        .any(|hint| value.contains(hint))
}

fn should_preserve_reasoning_content_for_openai_chat(provider: &Provider, body: &Value) -> bool {
    if body
        .get("model")
        .and_then(|m| m.as_str())
        .is_some_and(is_reasoning_vendor_identifier)
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
        .any(is_reasoning_vendor_identifier)
}

fn should_normalize_anthropic_tool_thinking_history(
    provider: &Provider,
    body: &Value,
    api_format: &str,
) -> bool {
    if api_format.trim() != "anthropic" {
        return false;
    }

    if body
        .get("model")
        .and_then(|m| m.as_str())
        .is_some_and(is_reasoning_vendor_identifier)
    {
        return true;
    }

    let settings = &provider.settings_config;
    [
        settings
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
            .and_then(|v| v.as_str()),
        settings.get("base_url").and_then(|v| v.as_str()),
        settings.get("baseURL").and_then(|v| v.as_str()),
        settings.get("apiEndpoint").and_then(|v| v.as_str()),
    ]
    .into_iter()
    .flatten()
    .any(is_reasoning_vendor_identifier)
}

pub fn normalize_anthropic_tool_thinking_history_for_provider(
    body: &mut Value,
    provider: &Provider,
    api_format: &str,
) -> bool {
    if !should_normalize_anthropic_tool_thinking_history(provider, body, api_format) {
        return false;
    }

    normalize_anthropic_tool_thinking_history(body)
}

fn normalize_anthropic_tool_thinking_history(body: &mut Value) -> bool {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return false;
    };

    let mut changed = false;
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }

        let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        if !content
            .iter()
            .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        {
            continue;
        }

        let mut has_thinking = false;
        for block in content.iter_mut() {
            match block.get("type").and_then(Value::as_str) {
                Some("thinking") => {
                    let has_non_empty_thinking = block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.trim().is_empty());
                    if let Some(obj) = block.as_object_mut() {
                        if obj.remove("signature").is_some() {
                            changed = true;
                        }
                        if !has_non_empty_thinking {
                            obj.insert(
                                "thinking".to_string(),
                                json!(ANTHROPIC_THINKING_PLACEHOLDER),
                            );
                            changed = true;
                        }
                    }
                    has_thinking = true;
                }
                Some("redacted_thinking") => {
                    *block = json!({
                        "type": "thinking",
                        "thinking": ANTHROPIC_REDACTED_THINKING_PLACEHOLDER
                    });
                    has_thinking = true;
                    changed = true;
                }
                _ => {}
            }
        }

        if !has_thinking {
            content.insert(
                0,
                json!({
                    "type": "thinking",
                    "thinking": ANTHROPIC_THINKING_PLACEHOLDER
                }),
            );
            changed = true;
        }
    }

    changed
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
            let mut result = if preserve_reasoning_content {
                super::transform::anthropic_to_openai_with_reasoning_content(
                    body,
                    explicit_cache_key,
                    true,
                )?
            } else {
                super::transform::anthropic_to_openai(body, explicit_cache_key)?
            };
            // Streaming requests must opt into upstream usage reporting, otherwise the
            // OpenAI-compatible upstream omits usage from the SSE stream and the converted
            // Anthropic message_delta reports all-zero tokens (see #323).
            super::transform::inject_openai_stream_include_usage(&mut result);
            Ok(result)
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

    /// Infer the default Anthropic auth strategy from which `ANTHROPIC_*` env
    /// var the user filled in, matching Anthropic SDK semantics:
    /// - `ANTHROPIC_AUTH_TOKEN` → `ClaudeAuth` (sends `Authorization: Bearer`)
    /// - `ANTHROPIC_API_KEY`    → `Anthropic`  (sends `x-api-key`)
    ///
    /// Precedence matches [`Self::extract_key`]. Returns `None` when neither is
    /// set so the caller can fall back (e.g. a direct `apiKey` field).
    fn infer_anthropic_auth_strategy(&self, provider: &Provider) -> Option<AuthStrategy> {
        let env = provider.settings_config.get("env")?;

        let has_value = |key: &str| -> bool {
            env.get(key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .is_some()
        };

        if has_value("ANTHROPIC_AUTH_TOKEN") {
            return Some(AuthStrategy::ClaudeAuth);
        }
        if has_value("ANTHROPIC_API_KEY") {
            return Some(AuthStrategy::Anthropic);
        }
        None
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
            // Align with Anthropic SDK semantics: ANTHROPIC_AUTH_TOKEN → Bearer,
            // ANTHROPIC_API_KEY → x-api-key. Emitting the wrong/extra header
            // breaks strict Anthropic-protocol endpoints such as OpenCode Go
            // (issue #330); a direct `apiKey` field keeps the x-api-key default.
            _ => self
                .infer_anthropic_auth_strategy(provider)
                .unwrap_or(AuthStrategy::Anthropic),
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
            AuthStrategy::Anthropic => request.header("x-api-key", &auth.api_key),
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

    fn create_provider(settings_config: serde_json::Value) -> Provider {
        Provider::with_id(
            "test-provider".to_string(),
            "Test Provider".to_string(),
            settings_config,
            None,
        )
    }

    #[test]
    fn anthropic_auth_token_env_uses_claude_auth_strategy() {
        // ANTHROPIC_AUTH_TOKEN carries Bearer semantics in the Anthropic SDK, so
        // it must resolve to ClaudeAuth (Authorization: Bearer only) — not the
        // x-api-key Anthropic strategy. This is the OpenCode Go case (#330).
        let adapter = ClaudeAdapter::new();
        let provider = create_provider(json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://opencode.ai/zen/go",
                "ANTHROPIC_AUTH_TOKEN": "sk-auth-token"
            }
        }));

        let auth = adapter
            .extract_auth(&provider)
            .expect("auth should resolve");
        assert_eq!(auth.api_key, "sk-auth-token");
        assert_eq!(auth.strategy, AuthStrategy::ClaudeAuth);
    }

    #[test]
    fn anthropic_api_key_env_uses_anthropic_strategy() {
        let adapter = ClaudeAdapter::new();
        let provider = create_provider(json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://api.anthropic.com",
                "ANTHROPIC_API_KEY": "sk-api-key"
            }
        }));

        let auth = adapter
            .extract_auth(&provider)
            .expect("auth should resolve");
        assert_eq!(auth.api_key, "sk-api-key");
        assert_eq!(auth.strategy, AuthStrategy::Anthropic);
    }

    #[test]
    fn anthropic_both_env_vars_prefer_auth_token() {
        // extract_key prefers AUTH_TOKEN; the inferred strategy must stay aligned.
        let adapter = ClaudeAdapter::new();
        let provider = create_provider(json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://api.anthropic.com",
                "ANTHROPIC_AUTH_TOKEN": "sk-from-auth-token",
                "ANTHROPIC_API_KEY": "sk-from-api-key"
            }
        }));

        let auth = adapter
            .extract_auth(&provider)
            .expect("auth should resolve");
        assert_eq!(auth.api_key, "sk-from-auth-token");
        assert_eq!(auth.strategy, AuthStrategy::ClaudeAuth);
    }

    #[test]
    fn direct_apikey_field_falls_back_to_anthropic_strategy() {
        // No ANTHROPIC_* env means no explicit preference → Anthropic (x-api-key).
        let adapter = ClaudeAdapter::new();
        let provider = create_provider(json!({
            "apiKey": "sk-direct",
            "env": {
                "ANTHROPIC_BASE_URL": "https://api.anthropic.com"
            }
        }));

        let auth = adapter
            .extract_auth(&provider)
            .expect("auth should resolve");
        assert_eq!(auth.api_key, "sk-direct");
        assert_eq!(auth.strategy, AuthStrategy::Anthropic);
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
    fn openai_chat_transform_preserves_reasoning_content_for_mimo_provider() {
        let provider: Provider = serde_json::from_value(json!({
            "id": "mimo",
            "name": "MiMo",
            "settingsConfig": {
                "api_format": "openai_chat",
                "env": {
                    "ANTHROPIC_BASE_URL": "https://api.mimo.example",
                    "ANTHROPIC_AUTH_TOKEN": "token-1"
                }
            }
        }))
        .expect("provider should deserialize");
        let body = json!({
            "model": "xiaomimimo-v1",
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
    fn deepseek_anthropic_tool_history_injects_missing_thinking() {
        let provider = create_provider(json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://api.deepseek.com/anthropic",
                "ANTHROPIC_API_KEY": "test-key"
            }
        }));
        let mut body = json!({
            "model": "deepseek-v4-pro",
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will inspect the repo."},
                    {"type": "tool_use", "id": "call_123", "name": "read_file", "input": {"path": "README.md"}}
                ]
            }]
        });

        let changed = normalize_anthropic_tool_thinking_history_for_provider(
            &mut body,
            &provider,
            "anthropic",
        );

        assert!(changed);
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["thinking"], ANTHROPIC_THINKING_PLACEHOLDER);
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[2]["type"], "tool_use");
    }

    #[test]
    fn kimi_anthropic_tool_history_injects_missing_thinking() {
        let provider = create_provider(json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://api.kimi.com/coding",
                "ANTHROPIC_API_KEY": "test-key"
            }
        }));
        let mut body = json!({
            "model": "kimi-for-coding",
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "tool_use", "id": "call_123", "name": "read_file", "input": {"path": "README.md"}}
                ]
            }]
        });

        let changed = normalize_anthropic_tool_thinking_history_for_provider(
            &mut body,
            &provider,
            "anthropic",
        );

        assert!(changed);
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["thinking"], ANTHROPIC_THINKING_PLACEHOLDER);
        assert_eq!(content[1]["type"], "tool_use");
    }

    #[test]
    fn deepseek_anthropic_tool_history_rewrites_redacted_thinking() {
        let provider = create_provider(json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://api.deepseek.com/anthropic",
                "ANTHROPIC_API_KEY": "test-key"
            }
        }));
        let mut body = json!({
            "model": "deepseek-v4-pro",
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "redacted_thinking", "data": "opaque"},
                    {"type": "tool_use", "id": "call_123", "name": "read_file", "input": {"path": "README.md"}}
                ]
            }]
        });

        let changed = normalize_anthropic_tool_thinking_history_for_provider(
            &mut body,
            &provider,
            "anthropic",
        );

        assert!(changed);
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(
            content[0]["thinking"],
            ANTHROPIC_REDACTED_THINKING_PLACEHOLDER
        );
        assert!(content[0].get("data").is_none());
    }

    #[test]
    fn deepseek_anthropic_tool_history_keeps_thinking_text_but_drops_signature() {
        let provider = create_provider(json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://api.deepseek.com/anthropic",
                "ANTHROPIC_API_KEY": "test-key"
            }
        }));
        let mut body = json!({
            "model": "deepseek-v4-pro",
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "Need to inspect the file.", "signature": "anthropic-signature"},
                    {"type": "tool_use", "id": "call_123", "name": "read_file", "input": {"path": "README.md"}}
                ]
            }]
        });

        let changed = normalize_anthropic_tool_thinking_history_for_provider(
            &mut body,
            &provider,
            "anthropic",
        );

        assert!(changed);
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["thinking"], "Need to inspect the file.");
        assert!(content[0].get("signature").is_none());
    }

    #[test]
    fn generic_anthropic_tool_history_is_not_modified() {
        let provider = create_provider(json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://api.example.com/anthropic",
                "ANTHROPIC_API_KEY": "test-key"
            }
        }));
        let mut body = json!({
            "model": "claude-sonnet-4.6",
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "tool_use", "id": "call_123", "name": "read_file", "input": {"path": "README.md"}}
                ]
            }]
        });
        let original = body.clone();

        let changed = normalize_anthropic_tool_thinking_history_for_provider(
            &mut body,
            &provider,
            "anthropic",
        );

        assert!(!changed);
        assert_eq!(body, original);
    }

    #[test]
    fn openai_chat_tool_history_is_not_native_normalized() {
        let provider = create_provider(json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://api.deepseek.com/anthropic",
                "ANTHROPIC_API_KEY": "test-key"
            }
        }));
        let mut body = json!({
            "model": "deepseek-v4-pro",
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "tool_use", "id": "call_123", "name": "read_file", "input": {"path": "README.md"}}
                ]
            }]
        });
        let original = body.clone();

        let changed = normalize_anthropic_tool_thinking_history_for_provider(
            &mut body,
            &provider,
            "openai_chat",
        );

        assert!(!changed);
        assert_eq!(body, original);
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

    #[test]
    fn openai_chat_stream_injects_include_usage() {
        // #323: streamed requests must opt into upstream usage reporting, otherwise the
        // OpenAI-compatible upstream omits usage and the converted message_delta is 0.
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
            "messages": [{"role": "user", "content": "hello"}],
            "stream": true
        });

        let result =
            transform_claude_request_for_api_format(body, &provider, "openai_chat", None).unwrap();

        assert_eq!(result["stream_options"]["include_usage"], true);
    }

    #[test]
    fn openai_chat_non_stream_omits_stream_options() {
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

        let result =
            transform_claude_request_for_api_format(body, &provider, "openai_chat", None).unwrap();

        assert!(result.get("stream_options").is_none());
    }
}
