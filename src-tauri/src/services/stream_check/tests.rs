use serde_json::json;

use super::service::StreamCheckService;
use super::types::{AuthInfo, AuthStrategy, HealthStatus, StreamCheckConfig};

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
fn stream_check_detects_github_copilot_auth_strategy() {
    let provider = crate::provider::Provider {
        id: "copilot".to_string(),
        name: "GitHub Copilot".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://api.githubcopilot.com"
            }
        }),
        website_url: None,
        category: None,
        created_at: None,
        sort_index: None,
        notes: None,
        meta: Some(crate::provider::ProviderMeta {
            provider_type: Some("github_copilot".to_string()),
            ..Default::default()
        }),
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };

    assert_eq!(
        StreamCheckService::detect_claude_auth_strategy(&provider, "https://api.githubcopilot.com"),
        AuthStrategy::GitHubCopilot
    );
}

#[test]
fn stream_check_auth_info_keeps_github_copilot_access_token() {
    let mut auth = AuthInfo::new("placeholder".to_string(), AuthStrategy::GitHubCopilot);
    auth.access_token = Some("session-token".to_string());

    assert_eq!(auth.access_token.as_deref(), Some("session-token"));
}
