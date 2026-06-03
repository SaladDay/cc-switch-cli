use serde_json::json;

use crate::{
    app_config::AppType,
    provider::{Provider, ProviderMeta},
};

use super::service::StreamCheckService;
use super::types::{AuthStrategy, HealthStatus, StreamCheckConfig};

fn claude_gemini_native_provider(key: &str) -> Provider {
    let mut provider = Provider::with_id(
        "p1".to_string(),
        "Provider One".to_string(),
        json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://generativelanguage.googleapis.com",
                "GEMINI_API_KEY": key
            }
        }),
        None,
    );
    provider.meta = Some(ProviderMeta {
        api_format: Some("gemini_native".to_string()),
        ..Default::default()
    });
    provider
}

#[test]
fn stream_check_default_config_matches_upstream_mvp() {
    let config = StreamCheckConfig::default();
    assert_eq!(config.timeout_secs, 45);
    assert_eq!(config.max_retries, 2);
    assert_eq!(config.degraded_threshold_ms, 6000);
    assert_eq!(config.test_prompt, "Who are you?");
}

#[test]
fn stream_check_determine_status_uses_threshold() {
    assert_eq!(
        StreamCheckService::determine_status(3000, 6000),
        HealthStatus::Operational
    );
    assert_eq!(
        StreamCheckService::determine_status(6000, 6000),
        HealthStatus::Operational
    );
    assert_eq!(
        StreamCheckService::determine_status(6001, 6000),
        HealthStatus::Degraded
    );
}

#[test]
fn stream_check_should_retry_transient_errors() {
    assert!(StreamCheckService::should_retry("Request timeout"));
    assert!(StreamCheckService::should_retry("request timed out"));
    assert!(StreamCheckService::should_retry("connection abort"));
    assert!(!StreamCheckService::should_retry("API Key invalid"));
}

#[test]
fn stream_check_parse_model_with_effort_supports_at_and_hash() {
    let (model, effort) = StreamCheckService::parse_model_with_effort("gpt-5.1-codex@low");
    assert_eq!(model, "gpt-5.1-codex");
    assert_eq!(effort, Some("low".to_string()));

    let (model, effort) = StreamCheckService::parse_model_with_effort("o1-preview#high");
    assert_eq!(model, "o1-preview");
    assert_eq!(effort, Some("high".to_string()));

    let (model, effort) = StreamCheckService::parse_model_with_effort("gpt-4o-mini");
    assert_eq!(model, "gpt-4o-mini");
    assert_eq!(effort, None);
}

#[test]
fn stream_check_provider_test_config_overrides_global_defaults() {
    let config = StreamCheckConfig::default();
    let mut provider = crate::provider::Provider::with_id(
        "p1".to_string(),
        "Provider One".to_string(),
        json!({"env": {"ANTHROPIC_BASE_URL": "https://example.com"}}),
        None,
    );
    provider.meta = Some(crate::provider::ProviderMeta {
        test_config: Some(crate::provider::ProviderTestConfig {
            enabled: true,
            test_model: Some("claude-override".to_string()),
            timeout_secs: Some(12),
            test_prompt: Some("ping".to_string()),
            degraded_threshold_ms: Some(3456),
            max_retries: Some(4),
        }),
        ..Default::default()
    });

    let merged = StreamCheckService::merge_provider_config(&provider, &config);
    assert_eq!(merged.timeout_secs, 12);
    assert_eq!(merged.max_retries, 4);
    assert_eq!(merged.degraded_threshold_ms, 3456);
    assert_eq!(merged.claude_model, "claude-override");
    assert_eq!(merged.codex_model, "claude-override");
    assert_eq!(merged.gemini_model, "claude-override");
    assert_eq!(merged.test_prompt, "ping");
}

#[test]
fn stream_check_claude_gemini_native_keeps_upstream_anthropic_model_env() {
    let config = StreamCheckConfig::default();
    let mut provider = Provider::with_id(
        "p1".to_string(),
        "Provider One".to_string(),
        json!({
            "env": {
                "ANTHROPIC_MODEL": "claude-old",
                "GEMINI_MODEL": "models/gemini-2.5-pro"
            }
        }),
        None,
    );
    provider.meta = Some(ProviderMeta {
        api_format: Some("gemini_native".to_string()),
        ..Default::default()
    });

    assert_eq!(
        StreamCheckService::resolve_test_model(&AppType::Claude, &provider, &config),
        "claude-old"
    );
}

#[test]
fn stream_check_claude_gemini_native_extracts_google_api_key_auth() {
    let mut provider = Provider::with_id(
        "p1".to_string(),
        "Provider One".to_string(),
        json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://generativelanguage.googleapis.com",
                "GEMINI_API_KEY": "gemini-key"
            }
        }),
        None,
    );
    provider.meta = Some(ProviderMeta {
        api_format: Some("gemini_native".to_string()),
        ..Default::default()
    });

    let auth = StreamCheckService::extract_auth(
        &provider,
        &AppType::Claude,
        "https://generativelanguage.googleapis.com",
    )
    .expect("extract Claude Gemini auth");

    assert_eq!(auth.strategy, AuthStrategy::Google);
    assert_eq!(auth.api_key, "gemini-key");
    assert_eq!(auth.access_token, None);
}

#[test]
fn stream_check_claude_gemini_native_extracts_google_oauth_json_token() {
    let provider = claude_gemini_native_provider(
        r#"{"access_token":"access-123","refresh_token":"refresh-123"}"#,
    );

    let auth = StreamCheckService::extract_auth(
        &provider,
        &AppType::Claude,
        "https://generativelanguage.googleapis.com",
    )
    .expect("extract Claude Gemini OAuth auth");

    assert_eq!(auth.strategy, AuthStrategy::GoogleOAuth);
    assert_eq!(auth.access_token.as_deref(), Some("access-123"));
}

#[test]
fn stream_check_claude_gemini_native_trims_google_oauth_json() {
    let provider = claude_gemini_native_provider(
        "\n  {\"access_token\":\"access-123\",\"refresh_token\":\"refresh-123\"}\n",
    );

    let auth = StreamCheckService::extract_auth(
        &provider,
        &AppType::Claude,
        "https://generativelanguage.googleapis.com",
    )
    .expect("extract Claude Gemini whitespace-padded OAuth JSON auth");

    assert_eq!(auth.strategy, AuthStrategy::GoogleOAuth);
    assert_eq!(
        auth.api_key,
        r#"{"access_token":"access-123","refresh_token":"refresh-123"}"#
    );
    assert_eq!(auth.access_token.as_deref(), Some("access-123"));
}

#[test]
fn stream_check_claude_gemini_native_trims_raw_google_oauth_token() {
    let provider = claude_gemini_native_provider("\nya29.raw-token-value\n");

    let auth = StreamCheckService::extract_auth(
        &provider,
        &AppType::Claude,
        "https://generativelanguage.googleapis.com",
    )
    .expect("extract Claude Gemini raw OAuth auth");

    assert_eq!(auth.strategy, AuthStrategy::GoogleOAuth);
    assert_eq!(auth.api_key, "ya29.raw-token-value");
    assert_eq!(auth.access_token.as_deref(), Some("ya29.raw-token-value"));
}

#[test]
fn stream_check_claude_gemini_native_refresh_only_json_does_not_expose_empty_bearer() {
    let provider = claude_gemini_native_provider(
        r#"{"refresh_token":"rt-abc","client_id":"cid","client_secret":"cs"}"#,
    );

    let auth = StreamCheckService::extract_auth(
        &provider,
        &AppType::Claude,
        "https://generativelanguage.googleapis.com",
    )
    .expect("extract Claude Gemini refresh-only OAuth auth");

    assert_eq!(auth.strategy, AuthStrategy::GoogleOAuth);
    assert_eq!(auth.access_token, None);
}

#[test]
fn stream_check_claude_gemini_native_empty_access_token_json_does_not_expose_empty_bearer() {
    let provider = claude_gemini_native_provider(
        r#"{"access_token":"","refresh_token":"rt-abc","client_id":"cid","client_secret":"cs"}"#,
    );

    let auth = StreamCheckService::extract_auth(
        &provider,
        &AppType::Claude,
        "https://generativelanguage.googleapis.com",
    )
    .expect("extract Claude Gemini expired OAuth auth");

    assert_eq!(auth.strategy, AuthStrategy::GoogleOAuth);
    assert_eq!(auth.access_token, None);
}

#[test]
fn stream_check_resolves_claude_gemini_native_url() {
    let url = StreamCheckService::resolve_claude_stream_url(
        "https://generativelanguage.googleapis.com",
        AuthStrategy::Google,
        "gemini_native",
        false,
        "models/gemini-2.5-pro",
    );

    assert_eq!(
        url,
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
    );
}

#[test]
fn stream_check_resolves_claude_gemini_native_full_openai_compat_url() {
    let url = StreamCheckService::resolve_claude_stream_url(
        "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions",
        AuthStrategy::Google,
        "gemini_native",
        true,
        "gemini-2.5-flash",
    );

    assert_eq!(
        url,
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
    );
}

#[test]
fn stream_check_preserves_claude_gemini_native_opaque_full_url() {
    let url = StreamCheckService::resolve_claude_stream_url(
        "https://relay.example/custom/generate-content",
        AuthStrategy::Google,
        "gemini_native",
        true,
        "gemini-2.5-flash",
    );

    assert_eq!(url, "https://relay.example/custom/generate-content?alt=sse");
}
