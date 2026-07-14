use std::sync::Arc;

use axum::{
    extract::State,
    http::{header::USER_AGENT, HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::IntoResponse,
    routing::get,
    Router,
};
use serde_json::json;
use tokio::sync::Mutex;

use crate::{
    app_config::AppType,
    provider::{Provider, ProviderMeta},
};

use super::service::StreamCheckService;
use super::types::{HealthStatus, StreamCheckConfig};

fn make_provider(settings_config: serde_json::Value) -> Provider {
    Provider::with_id(
        "p1".to_string(),
        "Provider One".to_string(),
        settings_config,
        None,
    )
}

async fn bind_test_listener() -> tokio::net::TcpListener {
    let mut last_error = None;
    for _ in 0..20 {
        match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => return listener,
            Err(error) => {
                last_error = Some(error);
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        }
    }

    panic!(
        "bind upstream listener: {:?}",
        last_error.expect("listener bind should produce an error")
    );
}

#[derive(Clone, Default)]
struct ReachabilityState {
    request_method: Arc<Mutex<Option<Method>>>,
    request_uri: Arc<Mutex<Option<Uri>>>,
    request_user_agent: Arc<Mutex<Option<HeaderValue>>>,
}

async fn handle_reachability_probe(
    State(state): State<ReachabilityState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> impl IntoResponse {
    *state.request_method.lock().await = Some(method);
    *state.request_uri.lock().await = Some(uri);
    *state.request_user_agent.lock().await = headers.get(USER_AGENT).cloned();
    StatusCode::NOT_FOUND
}

#[test]
fn stream_check_default_config_matches_upstream_reachability() {
    let config = StreamCheckConfig::default();
    assert_eq!(config.timeout_secs, 8);
    assert_eq!(config.max_retries, 1);
    assert_eq!(config.degraded_threshold_ms, 6000);
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
    assert!(!StreamCheckService::should_retry(
        "Connection failed: dns error"
    ));
}

#[test]
fn stream_check_build_result_treats_any_http_status_as_reachable() {
    for status in [200u16, 204, 401, 403, 404, 429, 500, 503] {
        let result = StreamCheckService::build_result(Ok(status), 100, 6000);
        assert!(result.success, "status {status} should be reachable");
        assert_eq!(result.status, HealthStatus::Operational);
        assert_eq!(result.message, "Reachable");
        assert_eq!(result.http_status, Some(status));
        assert!(result.model_used.is_empty());
    }
}

#[test]
fn stream_check_build_result_marks_slow_reachable_response_degraded() {
    let result = StreamCheckService::build_result(Ok(200), 6001, 6000);
    assert!(result.success);
    assert_eq!(result.status, HealthStatus::Degraded);
}

#[test]
fn stream_check_provider_test_config_overrides_reachability_timing() {
    let config = StreamCheckConfig::default();
    let mut provider = make_provider(json!({"env": {"ANTHROPIC_BASE_URL": "https://example.com"}}));
    provider.meta = Some(crate::provider::ProviderMeta {
        test_config: Some(crate::provider::ProviderTestConfig {
            enabled: true,
            test_model: Some("ignored-by-reachability".to_string()),
            timeout_secs: Some(12),
            test_prompt: Some("ignored".to_string()),
            degraded_threshold_ms: Some(3456),
            max_retries: Some(4),
        }),
        ..Default::default()
    });

    let merged = StreamCheckService::merge_provider_config(&provider, &config);
    assert_eq!(merged.timeout_secs, 12);
    assert_eq!(merged.max_retries, 4);
    assert_eq!(merged.degraded_threshold_ms, 3456);
}

#[tokio::test]
async fn stream_check_codex_openai_chat_uses_base_url_reachability_probe() {
    let upstream_state = ReachabilityState::default();
    let upstream_router = Router::new()
        .route("/", get(handle_reachability_probe))
        .with_state(upstream_state.clone());
    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("read upstream address");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let mut provider = Provider::with_id(
        "codex-openai-chat-check".to_string(),
        "Codex OpenAI Chat Check".to_string(),
        json!({
            "base_url": format!("http://{}", upstream_addr),
            "apiKey": "sk-test-codex",
            "api_format": "openai_chat"
        }),
        None,
    );
    provider.meta = Some(ProviderMeta {
        api_format: Some("openai_chat".to_string()),
        custom_user_agent: Some("  cc-switch-cli/stream-check  ".to_string()),
        ..ProviderMeta::default()
    });

    let result = StreamCheckService::check_with_retry(
        &AppType::Codex,
        &provider,
        &StreamCheckConfig::default(),
    )
    .await
    .expect("Codex chat provider should use reachability probe");

    assert!(result.success);
    assert_eq!(result.message, "Reachable");
    assert_eq!(result.http_status, Some(404));
    assert!(result.model_used.is_empty());
    assert_eq!(
        upstream_state.request_method.lock().await.as_ref(),
        Some(&Method::GET)
    );
    assert_eq!(
        upstream_state
            .request_uri
            .lock()
            .await
            .as_ref()
            .map(Uri::path),
        Some("/")
    );
    assert_eq!(
        upstream_state.request_user_agent.lock().await.as_ref(),
        Some(&HeaderValue::from_static("cc-switch-cli/stream-check"))
    );

    upstream_handle.abort();
}

#[tokio::test]
async fn stream_check_ignores_invalid_custom_user_agent() {
    let upstream_state = ReachabilityState::default();
    let upstream_router = Router::new()
        .route("/", get(handle_reachability_probe))
        .with_state(upstream_state.clone());
    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("read upstream address");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let mut provider = Provider::with_id(
        "openclaw-invalid-user-agent".to_string(),
        "OpenClaw Invalid User-Agent".to_string(),
        json!({"baseUrl": format!("http://{}", upstream_addr)}),
        None,
    );
    provider.meta = Some(ProviderMeta {
        custom_user_agent: Some("invalid\nuser-agent".to_string()),
        ..ProviderMeta::default()
    });

    let result = StreamCheckService::check_with_retry(
        &AppType::OpenClaw,
        &provider,
        &StreamCheckConfig::default(),
    )
    .await
    .expect("invalid custom User-Agent should not fail reachability check");

    assert!(result.success);
    assert_eq!(
        upstream_state.request_user_agent.lock().await.as_ref(),
        None
    );

    upstream_handle.abort();
}

#[test]
fn stream_check_resolves_opencode_base_url_explicit_wins() {
    let provider = make_provider(json!({
        "npm": "@ai-sdk/openai",
        "options": { "baseURL": "https://proxy.local/v1", "apiKey": "k" },
        "models": {},
    }));
    let resolved =
        StreamCheckService::resolve_opencode_base_url(&provider, Some("@ai-sdk/openai")).unwrap();
    assert_eq!(resolved, "https://proxy.local/v1");
}

#[test]
fn stream_check_resolves_opencode_base_url_falls_back_for_known_npm() {
    let provider = make_provider(json!({
        "npm": "@ai-sdk/anthropic",
        "options": { "apiKey": "k" },
        "models": {},
    }));
    let resolved =
        StreamCheckService::resolve_opencode_base_url(&provider, Some("@ai-sdk/anthropic"))
            .unwrap();
    assert_eq!(resolved, "https://api.anthropic.com");
}

#[test]
fn stream_check_resolves_opencode_base_url_errors_for_openai_compatible_without_url() {
    let provider = make_provider(json!({
        "npm": "@ai-sdk/openai-compatible",
        "options": { "apiKey": "k" },
        "models": {},
    }));
    let result =
        StreamCheckService::resolve_opencode_base_url(&provider, Some("@ai-sdk/openai-compatible"));
    assert!(result.is_err());
}

#[tokio::test]
async fn stream_check_openclaw_reachability_does_not_require_auth() {
    let upstream_router = Router::new().route("/", get(|| async { StatusCode::NO_CONTENT }));
    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("read upstream address");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let provider = Provider::with_id(
        "openclaw-check".to_string(),
        "OpenClaw Check".to_string(),
        json!({
            "baseUrl": format!("http://{}", upstream_addr),
            "models": [{ "id": "gpt-4.1-mini" }]
        }),
        None,
    );

    let result = StreamCheckService::check_with_retry(
        &AppType::OpenClaw,
        &provider,
        &StreamCheckConfig::default(),
    )
    .await
    .expect("OpenClaw reachability should not require auth");

    assert!(result.success);
    assert_eq!(result.http_status, Some(204));

    upstream_handle.abort();
}
