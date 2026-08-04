use reqwest::RequestBuilder;
use serde_json::Value as JsonValue;
use std::collections::HashSet;
use toml::Value as TomlValue;

use crate::{
    provider::{CodexChatReasoningConfig, Provider},
    proxy::error::ProxyError,
};

use super::{AuthInfo, AuthStrategy, ProviderAdapter};

pub struct CodexAdapter;

/// Which generated-catalog tool profile a Codex provider should use.
///
/// Derived from the same chat/responses detection as request routing (which
/// honors `meta.apiFormat`, the `settingsConfig.api_format`/`apiFormat`
/// fallbacks, config.toml `wire_api`, and the base-url shape) rather than only
/// `meta.apiFormat`, so imported/legacy native-Responses providers still get
/// the native (apply_patch-stripped) catalog.
pub fn codex_provider_catalog_tool_profile(
    provider: &Provider,
) -> crate::codex_config::CodexCatalogToolProfile {
    if codex_provider_uses_anthropic(provider) {
        crate::codex_config::CodexCatalogToolProfile::Anthropic
    } else if codex_provider_uses_chat_completions(provider) {
        crate::codex_config::CodexCatalogToolProfile::ProxyChat
    } else {
        crate::codex_config::CodexCatalogToolProfile::NativeResponses
    }
}

/// Whether this Codex provider's real upstream should be called through
/// OpenAI Chat Completions, even if the local Codex client is talking to CC
/// Switch through the Responses API.
pub fn codex_provider_uses_chat_completions(provider: &Provider) -> bool {
    if let Some(api_format) = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.api_format.as_deref())
        .or_else(|| {
            provider
                .settings_config
                .get("api_format")
                .and_then(|v| v.as_str())
        })
        .or_else(|| {
            provider
                .settings_config
                .get("apiFormat")
                .and_then(|v| v.as_str())
        })
    {
        return is_chat_wire_api(api_format);
    }

    if let Some(wire_api) = provider
        .settings_config
        .get("config")
        .and_then(|v| v.as_str())
        .and_then(extract_codex_wire_api_from_toml)
    {
        return is_chat_wire_api(&wire_api);
    }

    codex_provider_base_url(provider)
        .map(|url| is_chat_completions_url(&url))
        .unwrap_or(false)
}

pub fn should_convert_codex_responses_to_chat(provider: &Provider, endpoint: &str) -> bool {
    let path = endpoint
        .split_once('?')
        .map_or(endpoint, |(path, _query)| path);

    matches!(
        path,
        "/responses" | "/v1/responses" | "/responses/compact" | "/v1/responses/compact"
    ) && codex_provider_uses_chat_completions(provider)
}

/// Whether this Codex provider's real upstream speaks the native Anthropic
/// Messages protocol (`/v1/messages`). The local Codex client always talks to
/// CC Switch through Responses, so CC Switch bridges Responses ⇄ Anthropic.
///
/// Only explicit format configuration is accepted. Anthropic gateway URLs vary
/// too widely for safe base-URL inference.
pub fn codex_provider_uses_anthropic(provider: &Provider) -> bool {
    if let Some(api_format) = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.api_format.as_deref())
        .or_else(|| {
            provider
                .settings_config
                .get("api_format")
                .and_then(|v| v.as_str())
        })
        .or_else(|| {
            provider
                .settings_config
                .get("apiFormat")
                .and_then(|v| v.as_str())
        })
    {
        return is_anthropic_wire_api(api_format);
    }

    provider
        .settings_config
        .get("config")
        .and_then(|v| v.as_str())
        .and_then(extract_codex_wire_api_from_toml)
        .map(|wire_api| is_anthropic_wire_api(&wire_api))
        .unwrap_or(false)
}

pub fn should_convert_codex_responses_to_anthropic(provider: &Provider, endpoint: &str) -> bool {
    let path = endpoint
        .split_once('?')
        .map_or(endpoint, |(path, _query)| path);

    matches!(
        path,
        "/responses" | "/v1/responses" | "/responses/compact" | "/v1/responses/compact"
    ) && codex_provider_uses_anthropic(provider)
}

/// Whether a converted Codex Responses request may send `prompt_cache_key` to
/// its Chat Completions upstream. Unknown OpenAI-compatible gateways default to
/// false because many reject unsupported request fields with HTTP 400.
pub fn should_send_codex_chat_prompt_cache_key(provider: &Provider) -> bool {
    match provider
        .meta
        .as_ref()
        .and_then(|meta| meta.prompt_cache_routing.as_deref())
        .unwrap_or("auto")
    {
        "enabled" => return true,
        "disabled" => return false,
        _ => {}
    }

    let Some(base_url) = codex_provider_base_url(provider) else {
        return false;
    };
    let Ok(url) = url::Url::parse(&base_url) else {
        return false;
    };

    match url.host_str() {
        Some("api.openai.com") => true,
        Some("api.kimi.com") => {
            let path = url.path().trim_end_matches('/');
            path == "/coding" || path.starts_with("/coding/")
        }
        _ => false,
    }
}

/// Add a stable cache-routing key after Responses -> Chat conversion. An
/// explicit client key wins; otherwise only a real client-provided session ID
/// is eligible. Generated per-request UUIDs must never be used here.
pub fn inject_codex_chat_prompt_cache_key(
    provider: &Provider,
    chat_body: &mut JsonValue,
    explicit_key: Option<&str>,
    client_session_id: Option<&str>,
) -> bool {
    if !should_send_codex_chat_prompt_cache_key(provider) {
        return false;
    }

    let key = explicit_key
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .or_else(|| {
            client_session_id
                .map(str::trim)
                .filter(|session_id| !session_id.is_empty())
        });
    let Some(key) = key else {
        return false;
    };

    chat_body["prompt_cache_key"] = JsonValue::String(key.to_string());
    true
}

/// Extract the real upstream model configured for a Codex provider.
pub fn codex_provider_upstream_model(provider: &Provider) -> Option<String> {
    provider
        .settings_config
        .get("model")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            provider
                .settings_config
                .get("config")
                .and_then(|v| v.as_str())
                .and_then(extract_codex_model_from_toml)
        })
}

fn codex_provider_catalog_model_ids(provider: &Provider) -> HashSet<String> {
    provider
        .settings_config
        .get("modelCatalog")
        .and_then(|catalog| catalog.get("models"))
        .and_then(|models| models.as_array())
        .map(|models| {
            models
                .iter()
                .filter_map(|model| model.get("model").and_then(|value| value.as_str()))
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// For Codex Chat providers, ensure the request uses the configured upstream
/// model before converting the request to Chat Completions.
pub fn apply_codex_chat_upstream_model(
    provider: &Provider,
    body: &mut JsonValue,
) -> Option<String> {
    if !codex_provider_uses_chat_completions(provider) {
        return None;
    }

    apply_codex_upstream_model(provider, body)
}

/// Apply the provider's configured upstream model without format gating.
/// The Anthropic bridge calls this after routing has already been resolved.
pub fn apply_codex_upstream_model(provider: &Provider, body: &mut JsonValue) -> Option<String> {
    let catalog_model_ids = codex_provider_catalog_model_ids(provider);
    if let Some(request_model) = body
        .get("model")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        if catalog_model_ids.contains(request_model) {
            return Some(request_model.to_string());
        }
    }

    let upstream_model = codex_provider_upstream_model(provider)?;
    body["model"] = JsonValue::String(upstream_model.clone());
    Some(upstream_model)
}

pub fn resolve_codex_chat_reasoning_config(
    provider: &Provider,
    body: &JsonValue,
) -> Option<CodexChatReasoningConfig> {
    if let Some(config) = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.codex_chat_reasoning.clone())
    {
        return Some(normalize_codex_chat_reasoning_config(config));
    }

    infer_codex_chat_reasoning_config(provider, body)
}

fn normalize_codex_chat_reasoning_config(
    mut config: CodexChatReasoningConfig,
) -> CodexChatReasoningConfig {
    if config.supports_effort.unwrap_or(false) && config.supports_thinking.is_none() {
        config.supports_thinking = Some(true);
    }
    config
}

fn infer_codex_chat_reasoning_config(
    provider: &Provider,
    body: &JsonValue,
) -> Option<CodexChatReasoningConfig> {
    let model = body
        .get("model")
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
        .or_else(|| codex_provider_upstream_model(provider))
        .unwrap_or_default()
        .to_ascii_lowercase();
    let base_url = codex_provider_base_url(provider)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let name = provider.name.to_ascii_lowercase();

    if let Some(config) = infer_aggregator_platform_config(&name, &base_url) {
        return Some(config);
    }

    let haystack = format!("{name} {base_url} {model}");

    if haystack.contains("deepseek") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(true),
            thinking_param: Some("thinking".to_string()),
            effort_param: Some("reasoning_effort".to_string()),
            effort_value_mode: Some("deepseek".to_string()),
            output_format: Some("reasoning_content".to_string()),
        });
    }

    if haystack.contains("stepfun") || haystack.contains("step-3.5-flash-2603") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(model.contains("2603")),
            thinking_param: Some("none".to_string()),
            effort_param: Some("reasoning_effort".to_string()),
            effort_value_mode: Some("low_high".to_string()),
            output_format: Some("reasoning".to_string()),
        });
    }

    if haystack.contains("kimi") || haystack.contains("moonshot") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("thinking".to_string()),
            effort_param: Some("none".to_string()),
            effort_value_mode: None,
            output_format: Some("reasoning_content".to_string()),
        });
    }

    if haystack.contains("glm") || haystack.contains("zhipu") || haystack.contains("z.ai") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("thinking".to_string()),
            effort_param: Some("none".to_string()),
            effort_value_mode: None,
            output_format: Some("reasoning_content".to_string()),
        });
    }

    if haystack.contains("qwen") || haystack.contains("dashscope") || haystack.contains("bailian") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("enable_thinking".to_string()),
            effort_param: Some("none".to_string()),
            effort_value_mode: None,
            output_format: Some("reasoning_content".to_string()),
        });
    }

    if haystack.contains("minimax") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("reasoning_split".to_string()),
            effort_param: Some("none".to_string()),
            effort_value_mode: None,
            output_format: Some("reasoning_details".to_string()),
        });
    }

    if haystack.contains("mimo") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("thinking".to_string()),
            effort_param: Some("none".to_string()),
            effort_value_mode: None,
            output_format: Some("reasoning_content".to_string()),
        });
    }

    None
}

fn infer_aggregator_platform_config(
    name: &str,
    base_url: &str,
) -> Option<CodexChatReasoningConfig> {
    let platform = format!("{name} {base_url}");

    if platform.contains("openrouter") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(false),
            supports_effort: Some(true),
            thinking_param: Some("none".to_string()),
            effort_param: Some("reasoning.effort".to_string()),
            effort_value_mode: Some("openrouter".to_string()),
            output_format: Some("auto".to_string()),
        });
    }

    if platform.contains("siliconflow") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("enable_thinking".to_string()),
            effort_param: Some("none".to_string()),
            effort_value_mode: None,
            output_format: Some("reasoning_content".to_string()),
        });
    }

    None
}

fn is_chat_wire_api(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "chat"
            | "chat_completions"
            | "chat-completions"
            | "openai_chat"
            | "openai-chat"
            | "openai_chat_completions"
    )
}

fn is_anthropic_wire_api(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "anthropic" | "anthropic_messages" | "anthropic-messages" | "claude" | "messages"
    )
}

fn is_chat_completions_url(value: &str) -> bool {
    value
        .trim_end_matches('/')
        .to_ascii_lowercase()
        .ends_with("/chat/completions")
}

/// `scheme://host` 之后没有路径段的纯 origin 形式。`build_url` 在这种情况下
/// 会自动补 `/v1`；其它同步生产路径的代码也需要同一判定。
pub fn is_origin_only_url(value: &str) -> bool {
    let trimmed = value.trim_end_matches('/');
    match trimmed.split_once("://") {
        Some((_scheme, rest)) => !rest.contains('/'),
        None => !trimmed.contains('/'),
    }
}

fn extract_codex_wire_api_from_toml(config_text: &str) -> Option<String> {
    let doc = config_text.parse::<TomlValue>().ok()?;

    if let Some(active_provider) = doc.get("model_provider").and_then(|v| v.as_str()) {
        if let Some(wire_api) = doc
            .get("model_providers")
            .and_then(|providers| providers.get(active_provider))
            .and_then(|provider| provider.get("wire_api"))
            .and_then(|v| v.as_str())
        {
            return Some(wire_api.to_string());
        }
    }

    doc.get("wire_api")
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
}

fn extract_codex_model_from_toml(config_text: &str) -> Option<String> {
    let doc = config_text.parse::<TomlValue>().ok()?;

    doc.get("model")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToString::to_string)
}

fn extract_codex_base_url_from_toml(config_text: &str) -> Option<String> {
    crate::codex_config::extract_codex_base_url(config_text)
}

fn normalize_codex_base_url(base_url: &str) -> Option<String> {
    let base_url = base_url.trim().trim_end_matches('/');
    (!base_url.is_empty()).then(|| base_url.to_string())
}

fn codex_provider_base_url(provider: &Provider) -> Option<String> {
    if let Some(config) = provider.settings_config.get("config") {
        if let Some(config_text) = config.as_str() {
            if !config_text.trim().is_empty() {
                return extract_codex_base_url_from_toml(config_text)
                    .and_then(|base_url| normalize_codex_base_url(&base_url));
            }
        }

        if let Some(base_url) = config
            .get("base_url")
            .and_then(|value| value.as_str())
            .and_then(normalize_codex_base_url)
        {
            return Some(base_url);
        }
    }

    // Older provider records store the endpoint directly in settingsConfig.
    // An empty/null config payload does not supersede those legacy aliases.
    provider
        .settings_config
        .get("base_url")
        .or_else(|| provider.settings_config.get("baseURL"))
        .and_then(|value| value.as_str())
        .and_then(normalize_codex_base_url)
}

impl CodexAdapter {
    pub fn new() -> Self {
        Self
    }

    fn extract_key(&self, provider: &Provider) -> Option<String> {
        provider.configured_api_key(&crate::app_config::AppType::Codex)
    }
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderAdapter for CodexAdapter {
    fn name(&self) -> &'static str {
        "Codex"
    }

    fn extract_base_url(&self, provider: &Provider) -> Result<String, ProxyError> {
        if let Some(base_url) = codex_provider_base_url(provider) {
            return Ok(base_url);
        }

        Err(ProxyError::ConfigError(
            "Codex Provider 缺少 base_url 配置".to_string(),
        ))
    }

    fn extract_auth(&self, provider: &Provider) -> Option<AuthInfo> {
        let strategy = if codex_provider_uses_anthropic(provider) {
            let uses_x_api_key = provider
                .meta
                .as_ref()
                .and_then(|meta| meta.api_key_field.as_deref())
                .map(|field| field.eq_ignore_ascii_case("ANTHROPIC_API_KEY"))
                .unwrap_or(false);
            if uses_x_api_key {
                AuthStrategy::Anthropic
            } else {
                AuthStrategy::Bearer
            }
        } else {
            AuthStrategy::Bearer
        };

        self.extract_key(provider)
            .map(|key| AuthInfo::new(key, strategy))
    }

    fn build_url(&self, base_url: &str, endpoint: &str) -> String {
        let base_trimmed = base_url.trim_end_matches('/');
        let endpoint_trimmed = endpoint.trim_start_matches('/');
        let already_has_v1 = base_trimmed.ends_with("/v1");
        let origin_only = is_origin_only_url(base_trimmed);

        let mut url = if already_has_v1 {
            format!("{base_trimmed}/{endpoint_trimmed}")
        } else if origin_only {
            format!("{base_trimmed}/v1/{endpoint_trimmed}")
        } else {
            format!("{base_trimmed}/{endpoint_trimmed}")
        };

        while url.contains("/v1/v1") {
            url = url.replace("/v1/v1", "/v1");
        }

        url
    }

    fn add_auth_headers(&self, request: RequestBuilder, auth: &AuthInfo) -> RequestBuilder {
        if auth.strategy == AuthStrategy::Anthropic {
            request.header("x-api-key", &auth.api_key)
        } else {
            request.header("Authorization", format!("Bearer {}", auth.api_key))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn create_provider(settings_config: serde_json::Value) -> Provider {
        Provider::with_id(
            "test".to_string(),
            "Test Provider".to_string(),
            settings_config,
            None,
        )
    }

    #[test]
    fn extract_base_url_uses_active_provider_instead_of_commented_assignment() {
        let adapter = CodexAdapter::new();
        let provider = create_provider(json!({
            "config": r#"model_provider = "current"
model = "gpt-5.6-sol"

# [model_providers.old]
# base_url = "https://old.example.com/v1"
# wire_api = "responses"

[model_providers.current]
base_url = "https://current.example.com/v1"
wire_api = "responses"
"#
        }));

        assert_eq!(
            adapter
                .extract_base_url(&provider)
                .expect("extract base URL"),
            "https://current.example.com/v1"
        );
    }

    #[test]
    fn extract_base_url_prefers_config_toml_over_legacy_aliases() {
        let adapter = CodexAdapter::new();
        let provider = create_provider(json!({
            "base_url": "https://stale.example.com/v1",
            "baseURL": "https://also-stale.example.com/v1",
            "config": r#"model_provider = "current"

[model_providers.current]
base_url = "https://current.example.com/v1/"
"#
        }));

        assert_eq!(
            adapter
                .extract_base_url(&provider)
                .expect("extract base URL"),
            "https://current.example.com/v1"
        );
    }

    #[test]
    fn extract_base_url_does_not_fall_back_when_config_toml_is_invalid() {
        let adapter = CodexAdapter::new();
        let provider = create_provider(json!({
            "base_url": "https://stale.example.com/v1",
            "config": "model_provider = \"current\"\n[broken"
        }));

        assert!(adapter.extract_base_url(&provider).is_err());
    }

    #[test]
    fn extract_base_url_keeps_legacy_alias_without_config_toml() {
        let adapter = CodexAdapter::new();
        for config in [None, Some(json!("")), Some(serde_json::Value::Null)] {
            let mut settings = json!({
                "baseURL": "https://legacy.example.com/v1/"
            });
            if let Some(config) = config {
                settings["config"] = config;
            }
            let provider = create_provider(settings);

            assert_eq!(
                adapter
                    .extract_base_url(&provider)
                    .expect("extract legacy base URL"),
                "https://legacy.example.com/v1"
            );
        }
    }

    #[test]
    fn test_extract_auth_falls_back_to_config_bearer_when_auth_key_empty() {
        let adapter = CodexAdapter::new();
        let provider = create_provider(json!({
            "auth": {
                "OPENAI_API_KEY": ""
            },
            "config": r#"model_provider = "custom"

[model_providers.custom]
experimental_bearer_token = "sk-config-key"
"#
        }));

        let auth = adapter.extract_auth(&provider).expect("extract auth");
        assert_eq!(auth.api_key, "sk-config-key");
        assert_eq!(auth.strategy, AuthStrategy::Bearer);
    }

    #[test]
    fn test_extract_auth_ignores_blank_keys() {
        let adapter = CodexAdapter::new();
        let provider = create_provider(json!({
            "env": {
                "OPENAI_API_KEY": "   "
            },
            "auth": {
                "OPENAI_API_KEY": "\t"
            },
            "apiKey": "",
            "config": {
                "api_key": "  "
            }
        }));

        assert!(adapter.extract_auth(&provider).is_none());
    }

    #[test]
    fn test_uses_anthropic_from_all_explicit_config_locations() {
        let settings = create_provider(json!({ "apiFormat": "anthropic" }));
        assert!(codex_provider_uses_anthropic(&settings));

        let mut meta = create_provider(json!({}));
        meta.meta = Some(crate::provider::ProviderMeta {
            api_format: Some("anthropic".to_string()),
            ..Default::default()
        });
        assert!(codex_provider_uses_anthropic(&meta));

        let toml = create_provider(json!({
            "config": r#"model_provider = "custom"

[model_providers.custom]
wire_api = "anthropic"
"#
        }));
        assert!(codex_provider_uses_anthropic(&toml));
    }

    #[test]
    fn test_anthropic_and_chat_routes_are_mutually_exclusive_and_path_guarded() {
        let anthropic = create_provider(json!({ "apiFormat": "anthropic" }));
        assert!(codex_provider_uses_anthropic(&anthropic));
        assert!(!codex_provider_uses_chat_completions(&anthropic));
        assert!(should_convert_codex_responses_to_anthropic(
            &anthropic,
            "/responses?x=1"
        ));
        assert!(should_convert_codex_responses_to_anthropic(
            &anthropic,
            "/v1/responses/compact"
        ));
        assert!(!should_convert_codex_responses_to_anthropic(
            &anthropic,
            "/chat/completions"
        ));

        let chat = create_provider(json!({ "apiFormat": "openai_chat" }));
        assert!(codex_provider_uses_chat_completions(&chat));
        assert!(!codex_provider_uses_anthropic(&chat));
    }

    #[test]
    fn test_anthropic_auth_defaults_to_bearer_and_supports_x_api_key() {
        let adapter = CodexAdapter::new();
        let mut provider = create_provider(json!({
            "apiFormat": "anthropic",
            "apiKey": "sk-anthropic"
        }));

        let auth = adapter
            .extract_auth(&provider)
            .expect("extract bearer auth");
        assert_eq!(auth.api_key, "sk-anthropic");
        assert_eq!(auth.strategy, AuthStrategy::Bearer);

        provider.meta = Some(crate::provider::ProviderMeta {
            api_format: Some("anthropic".to_string()),
            api_key_field: Some("ANTHROPIC_API_KEY".to_string()),
            ..Default::default()
        });
        let auth = adapter
            .extract_auth(&provider)
            .expect("extract x-api-key auth");
        assert_eq!(auth.api_key, "sk-anthropic");
        assert_eq!(auth.strategy, AuthStrategy::Anthropic);
    }

    #[test]
    fn test_catalog_tool_profile_honors_settings_api_format_fallback() {
        use crate::codex_config::CodexCatalogToolProfile;
        // Legacy/imported provider carries apiFormat only in settingsConfig (no
        // meta): the catalog profile must still pick NativeResponses so native
        // gateways don't get the freeform apply_patch tool.
        let responses = create_provider(json!({ "api_format": "openai_responses" }));
        assert_eq!(
            codex_provider_catalog_tool_profile(&responses),
            CodexCatalogToolProfile::NativeResponses
        );

        let chat = create_provider(json!({ "api_format": "openai_chat" }));
        assert_eq!(
            codex_provider_catalog_tool_profile(&chat),
            CodexCatalogToolProfile::ProxyChat
        );

        let anthropic = create_provider(json!({ "api_format": "anthropic" }));
        assert_eq!(
            codex_provider_catalog_tool_profile(&anthropic),
            CodexCatalogToolProfile::Anthropic
        );
    }

    #[test]
    fn prompt_cache_routing_auto_enables_known_upstreams_only() {
        let kimi = create_provider(json!({
            "config": r#"
model_provider = "custom"
[model_providers.custom]
base_url = "https://api.kimi.com/coding/v1"
wire_api = "responses"
"#
        }));
        let openai = create_provider(json!({
            "base_url": "https://api.openai.com/v1"
        }));
        let unknown = create_provider(json!({
            "base_url": "https://strict.example.com/v1"
        }));

        assert!(should_send_codex_chat_prompt_cache_key(&kimi));
        assert!(should_send_codex_chat_prompt_cache_key(&openai));
        assert!(!should_send_codex_chat_prompt_cache_key(&unknown));
    }

    #[test]
    fn prompt_cache_routing_user_override_wins_over_auto_detection() {
        let mut kimi = create_provider(json!({
            "base_url": "https://api.kimi.com/coding/v1"
        }));
        kimi.meta = Some(crate::provider::ProviderMeta {
            prompt_cache_routing: Some("disabled".to_string()),
            ..Default::default()
        });
        assert!(!should_send_codex_chat_prompt_cache_key(&kimi));

        let mut unknown = create_provider(json!({
            "base_url": "https://strict.example.com/v1"
        }));
        unknown.meta = Some(crate::provider::ProviderMeta {
            prompt_cache_routing: Some("enabled".to_string()),
            ..Default::default()
        });
        assert!(should_send_codex_chat_prompt_cache_key(&unknown));
    }

    #[test]
    fn prompt_cache_key_prefers_explicit_key_then_real_session() {
        let provider = create_provider(json!({
            "base_url": "https://api.kimi.com/coding/v1"
        }));
        let mut explicit_body = json!({ "model": "kimi-for-coding" });
        assert!(inject_codex_chat_prompt_cache_key(
            &provider,
            &mut explicit_body,
            Some("request-key"),
            Some("session-key"),
        ));
        assert_eq!(explicit_body["prompt_cache_key"], "request-key");

        let mut session_body = json!({ "model": "kimi-for-coding" });
        assert!(inject_codex_chat_prompt_cache_key(
            &provider,
            &mut session_body,
            None,
            Some("session-key"),
        ));
        assert_eq!(session_body["prompt_cache_key"], "session-key");
    }

    #[test]
    fn prompt_cache_key_is_not_injected_without_real_session_or_support() {
        let kimi = create_provider(json!({
            "base_url": "https://api.kimi.com/coding/v1"
        }));
        let mut no_session_body = json!({ "model": "kimi-for-coding" });
        assert!(!inject_codex_chat_prompt_cache_key(
            &kimi,
            &mut no_session_body,
            None,
            None,
        ));
        assert!(no_session_body.get("prompt_cache_key").is_none());

        let unknown = create_provider(json!({
            "base_url": "https://strict.example.com/v1"
        }));
        let mut unsupported_body = json!({ "model": "other" });
        assert!(!inject_codex_chat_prompt_cache_key(
            &unknown,
            &mut unsupported_body,
            Some("request-key"),
            Some("session-key"),
        ));
        assert!(unsupported_body.get("prompt_cache_key").is_none());
    }

    #[test]
    fn test_codex_provider_uses_chat_completions_from_active_wire_api() {
        let provider = create_provider(json!({
            "config": r#"
model_provider = "chat_only"
model = "gpt-5"

[model_providers.chat_only]
name = "Chat Only"
base_url = "https://example.com/v1"
wire_api = "chat"
"#
        }));

        assert!(codex_provider_uses_chat_completions(&provider));
        assert!(should_convert_codex_responses_to_chat(
            &provider,
            "/responses?stream=true"
        ));
        assert!(!should_convert_codex_responses_to_chat(
            &provider,
            "/chat/completions"
        ));
    }

    #[test]
    fn test_codex_provider_uses_chat_completions_from_full_chat_url() {
        let provider = create_provider(json!({
            "base_url": "https://example.com/v1/chat/completions"
        }));

        assert!(codex_provider_uses_chat_completions(&provider));
        assert!(should_convert_codex_responses_to_chat(
            &provider,
            "/v1/responses/compact"
        ));
    }

    #[test]
    fn test_apply_codex_chat_upstream_model_uses_provider_config_model() {
        let provider = create_provider(json!({
            "base_url": "https://api.deepseek.com/v1",
            "api_format": "openai_chat",
            "model": "deepseek-chat"
        }));
        let mut body = json!({
            "model": "gpt-5.4",
            "input": "hello"
        });

        let upstream_model = apply_codex_chat_upstream_model(&provider, &mut body);

        assert_eq!(upstream_model.as_deref(), Some("deepseek-chat"));
        assert_eq!(body["model"], "deepseek-chat");
    }

    #[test]
    fn test_apply_codex_chat_upstream_model_preserves_catalog_model_selection() {
        let provider = create_provider(json!({
            "base_url": "https://api.deepseek.com/v1",
            "api_format": "openai_chat",
            "model": "deepseek-chat",
            "modelCatalog": {
                "models": [
                    {"model": "deepseek-chat"},
                    {"model": "deepseek-reasoner"}
                ]
            }
        }));
        let mut body = json!({
            "model": "deepseek-reasoner",
            "input": "hello"
        });

        let upstream_model = apply_codex_chat_upstream_model(&provider, &mut body);

        assert_eq!(upstream_model.as_deref(), Some("deepseek-reasoner"));
        assert_eq!(body["model"], "deepseek-reasoner");
    }

    #[test]
    fn test_resolve_codex_chat_reasoning_infers_deepseek_effort_support() {
        let provider = create_provider(json!({
            "base_url": "https://api.deepseek.com/v1",
            "api_format": "openai_chat"
        }));

        let config =
            resolve_codex_chat_reasoning_config(&provider, &json!({ "model": "deepseek-v4-pro" }))
                .expect("infer deepseek reasoning");

        assert_eq!(config.supports_thinking, Some(true));
        assert_eq!(config.supports_effort, Some(true));
        assert_eq!(config.thinking_param.as_deref(), Some("thinking"));
        assert_eq!(config.effort_param.as_deref(), Some("reasoning_effort"));
        assert_eq!(config.effort_value_mode.as_deref(), Some("deepseek"));
        assert_eq!(config.output_format.as_deref(), Some("reasoning_content"));
    }

    #[test]
    fn test_resolve_codex_chat_reasoning_explicit_meta_overrides_inference() {
        let mut provider = create_provider(json!({
            "base_url": "https://api.deepseek.com/v1",
            "api_format": "openai_chat"
        }));
        provider.meta = Some(crate::provider::ProviderMeta {
            api_format: Some("openai_chat".to_string()),
            codex_chat_reasoning: Some(CodexChatReasoningConfig {
                supports_thinking: None,
                supports_effort: Some(true),
                thinking_param: Some("none".to_string()),
                effort_param: Some("reasoning.effort".to_string()),
                effort_value_mode: Some("openrouter".to_string()),
                output_format: Some("auto".to_string()),
            }),
            ..Default::default()
        });

        let config =
            resolve_codex_chat_reasoning_config(&provider, &json!({ "model": "deepseek-v4-pro" }))
                .expect("use explicit reasoning config");

        assert_eq!(config.supports_thinking, Some(true));
        assert_eq!(config.supports_effort, Some(true));
        assert_eq!(config.thinking_param.as_deref(), Some("none"));
        assert_eq!(config.effort_param.as_deref(), Some("reasoning.effort"));
        assert_eq!(config.effort_value_mode.as_deref(), Some("openrouter"));
    }

    #[test]
    fn test_resolve_codex_chat_reasoning_openrouter_platform_overrides_model() {
        let provider = create_provider(json!({
            "base_url": "https://openrouter.ai/api/v1",
            "api_format": "openai_chat"
        }));

        let config = resolve_codex_chat_reasoning_config(
            &provider,
            &json!({ "model": "deepseek/deepseek-r1" }),
        )
        .expect("infer openrouter reasoning");

        assert_eq!(config.supports_thinking, Some(false));
        assert_eq!(config.supports_effort, Some(true));
        assert_eq!(config.thinking_param.as_deref(), Some("none"));
        assert_eq!(config.effort_param.as_deref(), Some("reasoning.effort"));
        assert_eq!(config.effort_value_mode.as_deref(), Some("openrouter"));
    }

    #[test]
    fn test_resolve_codex_chat_reasoning_siliconflow_platform_overrides_minimax() {
        let provider = create_provider(json!({
            "base_url": "https://api.siliconflow.cn/v1",
            "api_format": "openai_chat"
        }));

        let config =
            resolve_codex_chat_reasoning_config(&provider, &json!({ "model": "MiniMax-M2" }))
                .expect("infer siliconflow reasoning");

        assert_eq!(config.supports_thinking, Some(true));
        assert_eq!(config.supports_effort, Some(false));
        assert_eq!(config.thinking_param.as_deref(), Some("enable_thinking"));
        assert_eq!(config.effort_param.as_deref(), Some("none"));
        assert_eq!(config.output_format.as_deref(), Some("reasoning_content"));
    }
}
