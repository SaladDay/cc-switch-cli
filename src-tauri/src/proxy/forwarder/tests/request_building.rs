#![expect(
    clippy::await_holding_lock,
    reason = "tests serialize global auth/config state while exercising async request preparation"
)]

use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use serde_json::{json, Value};

use super::{
    bedrock_claude_provider, claude_provider, claude_request_body, spawn_scripted_upstream,
    test_router,
};
use crate::{
    app_config::AppType,
    provider::{AuthBinding, AuthBindingSource, Provider, ProviderMeta},
    proxy::{
        forwarder::{ForwardOptions, RequestForwarder},
        providers::copilot_auth::CopilotModel,
        types::{CopilotOptimizerConfig, OptimizerConfig, RectifierConfig},
    },
    services::{copilot_auth::TestCopilotAuthManagerGuard, CodexOAuthService, CopilotAuthService},
    test_support::lock_test_home_and_settings,
};
#[tokio::test]
async fn bedrock_claude_prepare_request_injects_optimizer_and_cache_breakpoints() {
    let (base_url, hits, bodies, server) =
        spawn_scripted_upstream(vec![(StatusCode::OK, json!({"ok": true}))]).await;
    let provider = bedrock_claude_provider("p1", &base_url);
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router)
        .expect("create forwarder")
        .with_optimizer_config(OptimizerConfig {
            enabled: true,
            thinking_optimizer: true,
            cache_injection: true,
            cache_ttl: "5m".to_string(),
        });

    let body = json!({
        "model": "anthropic.claude-sonnet-4-5-20250514-v1:0",
        "max_tokens": 32,
        "tools": [{"name": "tool_a"}],
        "system": [{"type": "text", "text": "sys"}],
        "messages": [{
            "role": "assistant",
            "content": [{"type": "text", "text": "hello"}]
        }]
    });

    let response = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            body,
            &HeaderMap::new(),
            vec![provider],
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
            RectifierConfig::default(),
        )
        .await
        .expect("bedrock claude request should succeed");

    assert_eq!(response.response.status, StatusCode::OK);
    assert_eq!(hits.count.load(Ordering::SeqCst), 1);

    let sent = bodies.lock().await;
    let sent = sent.first().expect("captured upstream request body");
    assert_eq!(sent["thinking"]["type"], "enabled");
    assert_eq!(sent["thinking"]["budget_tokens"], 31);
    assert!(sent["tools"][0].get("cache_control").is_some());
    assert!(sent["system"][0].get("cache_control").is_some());
    assert!(sent["messages"][0]["content"][0]
        .get("cache_control")
        .is_some());

    server.abort();
}

#[tokio::test]
async fn non_bedrock_claude_prepare_request_skips_optimizer_and_cache_injection() {
    let (base_url, hits, bodies, server) =
        spawn_scripted_upstream(vec![(StatusCode::OK, json!({"ok": true}))]).await;
    let provider = claude_provider("p1", &base_url, None);
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router)
        .expect("create forwarder")
        .with_optimizer_config(OptimizerConfig {
            enabled: true,
            thinking_optimizer: true,
            cache_injection: true,
            cache_ttl: "5m".to_string(),
        });

    let body = json!({
        "model": "anthropic.claude-sonnet-4-5-20250514-v1:0",
        "max_tokens": 32,
        "tools": [{"name": "tool_a"}],
        "system": [{"type": "text", "text": "sys"}],
        "messages": [{
            "role": "assistant",
            "content": [{"type": "text", "text": "hello"}]
        }]
    });

    let response = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            body,
            &HeaderMap::new(),
            vec![provider],
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
            RectifierConfig::default(),
        )
        .await
        .expect("regular claude request should succeed");

    assert_eq!(response.response.status, StatusCode::OK);
    assert_eq!(hits.count.load(Ordering::SeqCst), 1);

    let sent = bodies.lock().await;
    let sent = sent.first().expect("captured upstream request body");
    assert!(sent.get("thinking").is_none());
    assert!(sent["tools"][0].get("cache_control").is_none());
    assert!(sent["system"][0].get("cache_control").is_none());
    assert!(sent["messages"][0]["content"][0]
        .get("cache_control")
        .is_none());

    server.abort();
}

#[tokio::test]
async fn non_copilot_claude_prepare_request_strips_one_m_suffix_after_mapping() {
    let mut provider = claude_provider("p1", "https://example.com", None);
    provider.settings_config["env"] = json!({
        "ANTHROPIC_DEFAULT_SONNET_MODEL": "deepseek-v4-pro [1M]"
    });
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    let request = forwarder
        .prepare_request(
            &AppType::Claude,
            &provider,
            "/v1/messages",
            &json!({
                "model": "claude-sonnet-4-6",
                "max_tokens": 32,
                "messages": [{
                    "role": "user",
                    "content": "hello"
                }]
            }),
            &HeaderMap::new(),
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare regular Claude request")
        .build()
        .expect("build regular Claude request");

    let body = request_body_json(&request);
    assert_eq!(body["model"], "deepseek-v4-pro");
}

#[tokio::test]
async fn deepseek_native_claude_prepare_request_normalizes_tool_thinking_history_before_send() {
    let (base_url, hits, bodies, server) =
        spawn_scripted_upstream(vec![(StatusCode::OK, json!({"ok": true}))]).await;
    let provider = claude_provider("deepseek", &base_url, None);
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    let body = json!({
        "model": "deepseek-v4-pro",
        "max_tokens": 32,
        "messages": [{
            "role": "assistant",
            "content": [
                {"type": "tool_use", "id": "call_123", "name": "read_file", "input": {"path": "README.md"}}
            ]
        }]
    });

    let response = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            body,
            &HeaderMap::new(),
            vec![provider],
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
            RectifierConfig::default(),
        )
        .await
        .expect("DeepSeek native Claude request should succeed");

    assert_eq!(response.response.status, StatusCode::OK);
    assert_eq!(hits.count.load(Ordering::SeqCst), 1);

    let sent = bodies.lock().await;
    let sent = sent.first().expect("captured upstream request body");
    let content = sent["messages"][0]["content"]
        .as_array()
        .expect("assistant content should be array");
    assert_eq!(content[0]["type"], "thinking");
    assert_eq!(content[0]["thinking"], "tool call");
    assert_eq!(content[1]["type"], "tool_use");

    server.abort();
}

#[tokio::test]
async fn claude_prepare_request_appends_claude_code_beta_to_existing_header() {
    let mut headers = HeaderMap::new();
    headers.insert("anthropic-beta", HeaderValue::from_static("existing-beta"));

    let request = build_request(
        &AppType::Claude,
        &claude_provider("p1", "https://example.com", None),
        headers,
    )
    .await;

    assert_eq!(
        request
            .headers()
            .get("anthropic-beta")
            .and_then(|value| value.to_str().ok()),
        Some("claude-code-20250219,existing-beta")
    );
}

#[tokio::test]
async fn claude_prepare_request_sets_defaults_and_filters_blocked_caller_headers() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_static("Bearer caller-token"),
    );
    headers.insert("x-api-key", HeaderValue::from_static("caller-api-key"));
    headers.insert(
        "x-goog-api-key",
        HeaderValue::from_static("caller-goog-key"),
    );
    headers.insert("accept-encoding", HeaderValue::from_static("gzip"));
    headers.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.10"));
    headers.insert("x-real-ip", HeaderValue::from_static("203.0.113.11"));
    headers.insert(
        "x-forwarded-host",
        HeaderValue::from_static("client.example"),
    );
    headers.insert("x-forwarded-port", HeaderValue::from_static("443"));
    headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    headers.insert(
        "forwarded",
        HeaderValue::from_static("for=203.0.113.10;proto=https"),
    );
    headers.insert("cf-connecting-ip", HeaderValue::from_static("203.0.113.12"));
    headers.insert("cf-ipcountry", HeaderValue::from_static("US"));
    headers.insert("cf-ray", HeaderValue::from_static("ray-id"));
    headers.insert(
        "cf-visitor",
        HeaderValue::from_static("{\"scheme\":\"https\"}"),
    );
    headers.insert("true-client-ip", HeaderValue::from_static("203.0.113.13"));
    headers.insert("fastly-client-ip", HeaderValue::from_static("203.0.113.14"));
    headers.insert("x-azure-clientip", HeaderValue::from_static("203.0.113.15"));
    headers.insert("x-azure-fdid", HeaderValue::from_static("fdid"));
    headers.insert("x-azure-ref", HeaderValue::from_static("ref"));
    headers.insert("akamai-origin-hop", HeaderValue::from_static("1"));
    headers.insert(
        "x-akamai-config-log-detail",
        HeaderValue::from_static("detail"),
    );
    headers.insert("x-request-id", HeaderValue::from_static("request-id"));
    headers.insert(
        "x-correlation-id",
        HeaderValue::from_static("correlation-id"),
    );
    headers.insert("x-trace-id", HeaderValue::from_static("trace-id"));
    headers.insert("x-amzn-trace-id", HeaderValue::from_static("amzn-trace-id"));
    headers.insert("x-b3-traceid", HeaderValue::from_static("b3-trace"));
    headers.insert("x-b3-spanid", HeaderValue::from_static("b3-span"));
    headers.insert("x-b3-parentspanid", HeaderValue::from_static("b3-parent"));
    headers.insert("x-b3-sampled", HeaderValue::from_static("1"));
    headers.insert(
        "traceparent",
        HeaderValue::from_static("00-00000000000000000000000000000000-0000000000000000-01"),
    );
    headers.insert("tracestate", HeaderValue::from_static("vendor=value"));

    let request = build_request(
        &AppType::Claude,
        &claude_provider("p1", "https://example.com", None),
        headers,
    )
    .await;

    assert_eq!(
        header_value(&request, "anthropic-beta"),
        Some("claude-code-20250219")
    );
    assert_eq!(
        header_value(&request, "anthropic-version"),
        Some("2023-06-01")
    );
    assert_eq!(header_value(&request, "accept-encoding"), Some("gzip"));
    assert_eq!(
        header_value(&request, "authorization"),
        Some("Bearer key-p1")
    );
    assert_eq!(header_value(&request, "x-api-key"), Some("key-p1"));
    assert_eq!(header_value(&request, "x-goog-api-key"), None);
    assert_eq!(
        header_value(&request, "x-forwarded-for"),
        Some("203.0.113.10")
    );
    assert_eq!(header_value(&request, "x-real-ip"), Some("203.0.113.11"));
    assert_eq!(header_value(&request, "x-forwarded-host"), None);
    assert_eq!(header_value(&request, "x-forwarded-port"), None);
    assert_eq!(header_value(&request, "x-forwarded-proto"), None);
    assert_eq!(header_value(&request, "forwarded"), None);
    assert_eq!(header_value(&request, "cf-connecting-ip"), None);
    assert_eq!(header_value(&request, "cf-ipcountry"), None);
    assert_eq!(header_value(&request, "cf-ray"), None);
    assert_eq!(header_value(&request, "cf-visitor"), None);
    assert_eq!(header_value(&request, "true-client-ip"), None);
    assert_eq!(header_value(&request, "fastly-client-ip"), None);
    assert_eq!(header_value(&request, "x-azure-clientip"), None);
    assert_eq!(header_value(&request, "x-azure-fdid"), None);
    assert_eq!(header_value(&request, "x-azure-ref"), None);
    assert_eq!(header_value(&request, "akamai-origin-hop"), None);
    assert_eq!(header_value(&request, "x-akamai-config-log-detail"), None);
    assert_eq!(header_value(&request, "x-request-id"), None);
    assert_eq!(header_value(&request, "x-correlation-id"), None);
    assert_eq!(header_value(&request, "x-trace-id"), None);
    assert_eq!(header_value(&request, "x-amzn-trace-id"), None);
    assert_eq!(header_value(&request, "x-b3-traceid"), None);
    assert_eq!(header_value(&request, "x-b3-spanid"), None);
    assert_eq!(header_value(&request, "x-b3-parentspanid"), None);
    assert_eq!(header_value(&request, "x-b3-sampled"), None);
    assert_eq!(header_value(&request, "traceparent"), None);
    assert_eq!(header_value(&request, "tracestate"), None);
}

#[tokio::test]
async fn claude_gemini_native_prepare_request_rewrites_url_body_and_auth() {
    let mut provider = claude_provider(
        "gemini-native",
        "https://generativelanguage.googleapis.com",
        None,
    );
    provider.meta = Some(ProviderMeta {
        api_format: Some("gemini_native".to_string()),
        ..Default::default()
    });
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    let request = forwarder
        .prepare_request(
            &AppType::Claude,
            &provider,
            "/v1/messages",
            &claude_request_body(),
            &HeaderMap::new(),
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare Gemini Native Claude request")
        .build()
        .expect("build Gemini Native Claude request");

    assert_eq!(
        request.url().as_str(),
        "https://generativelanguage.googleapis.com/v1beta/models/claude-3-7-sonnet-20250219:generateContent"
    );
    assert_eq!(
        header_value(&request, "x-goog-api-key"),
        Some("key-gemini-native")
    );
    assert_eq!(header_value(&request, "authorization"), None);
    assert_eq!(header_value(&request, "anthropic-beta"), None);
    assert_eq!(header_value(&request, "anthropic-version"), None);

    let body = request_body_json(&request);
    assert!(body.get("contents").is_some());
    assert!(body.get("messages").is_none());
    assert_eq!(body["generationConfig"]["maxOutputTokens"], 32);
}

#[tokio::test]
async fn claude_gemini_native_prepare_request_preserves_opaque_full_url() {
    let mut provider =
        claude_provider("gemini-full", "https://relay.example/custom/generate", None);
    provider.meta = Some(ProviderMeta {
        api_format: Some("gemini_native".to_string()),
        is_full_url: Some(true),
        ..Default::default()
    });
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    let request = forwarder
        .prepare_request(
            &AppType::Claude,
            &provider,
            "/v1/messages",
            &json!({
                "model": "models/gemini-2.5-flash",
                "max_tokens": 4,
                "stream": true,
                "messages": [{
                    "role": "user",
                    "content": "hello"
                }]
            }),
            &HeaderMap::new(),
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare opaque full-url Gemini Native request")
        .build()
        .expect("build opaque full-url Gemini Native request");

    assert_eq!(
        request.url().as_str(),
        "https://relay.example/custom/generate?alt=sse"
    );
    assert_eq!(
        header_value(&request, "x-goog-api-key"),
        Some("key-gemini-full")
    );
}

#[tokio::test]
async fn non_claude_prepare_request_skips_claude_specific_headers() {
    let request = build_request(
        &AppType::Codex,
        &codex_provider("https://example.com"),
        HeaderMap::new(),
    )
    .await;

    assert_eq!(header_value(&request, "anthropic-beta"), None);
    assert_eq!(header_value(&request, "anthropic-version"), None);
    assert_eq!(
        header_value(&request, "authorization"),
        Some("Bearer codex-key")
    );
    assert_eq!(header_value(&request, "accept-encoding"), None);
}

#[tokio::test]
async fn streaming_passthrough_prepare_request_forces_identity_accept_encoding() {
    let mut headers = HeaderMap::new();
    headers.insert("accept-encoding", HeaderValue::from_static("gzip"));
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");
    let request_body = json!({
        "model": "gpt-5.4",
        "input": "hello",
        "stream": true
    });

    let request = forwarder
        .prepare_request(
            &AppType::Codex,
            &codex_provider("https://example.com"),
            "/v1/responses",
            &request_body,
            &headers,
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare streaming passthrough request")
        .build()
        .expect("build streaming passthrough request");

    assert_eq!(header_value(&request, "accept-encoding"), Some("identity"));
}

#[tokio::test]
async fn codex_oauth_prepare_request_injects_bound_account_headers() {
    let _lock = lock_test_home_and_settings();
    let _manager = CodexOAuthService::test_manager_with_account(
        "acc-bound",
        "rt-bound",
        Some("bound@example.com"),
        Some("at-bound"),
        None,
    )
    .await
    .expect("seed bound account");

    let provider = codex_oauth_provider(Some("acc-bound"));
    let request = build_request(&AppType::Claude, &provider, HeaderMap::new()).await;

    assert_eq!(
        request.url().as_str(),
        "https://chatgpt.com/backend-api/codex/responses"
    );
    assert_eq!(
        header_value(&request, "authorization"),
        Some("Bearer at-bound")
    );
    assert_eq!(
        header_value(&request, "chatgpt-account-id"),
        Some("acc-bound")
    );
    assert_eq!(header_value(&request, "originator"), Some("cc-switch"));
    assert_eq!(header_value(&request, "anthropic-beta"), None);
    assert_eq!(header_value(&request, "anthropic-version"), None);
}

#[tokio::test]
async fn codex_oauth_prepare_request_injects_client_session_headers() {
    let _lock = lock_test_home_and_settings();
    let _manager = CodexOAuthService::test_manager_with_account(
        "acc-session",
        "rt-session",
        Some("session@example.com"),
        Some("at-session"),
        None,
    )
    .await
    .expect("seed session account");

    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router)
        .expect("create forwarder")
        .with_session("codex_session-123".to_string(), true);
    let provider = codex_oauth_provider(Some("acc-session"));
    let request = forwarder
        .prepare_request(
            &AppType::Claude,
            &provider,
            "/v1/messages",
            &claude_request_body(),
            &HeaderMap::new(),
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare request")
        .build()
        .expect("build request");

    assert_eq!(
        header_value(&request, "session_id"),
        Some("codex_session-123")
    );
    assert_eq!(
        header_value(&request, "x-client-request-id"),
        Some("codex_session-123")
    );
    assert_eq!(
        header_value(&request, "x-codex-window-id"),
        Some("codex_session-123:0")
    );
    assert_eq!(header_value(&request, "anthropic-beta"), None);
    assert_eq!(header_value(&request, "anthropic-version"), None);
}

#[tokio::test]
async fn codex_oauth_prepare_request_skips_generated_session_headers() {
    let _lock = lock_test_home_and_settings();
    let _manager = CodexOAuthService::test_manager_with_account(
        "acc-generated",
        "rt-generated",
        Some("generated@example.com"),
        Some("at-generated"),
        None,
    )
    .await
    .expect("seed generated account");

    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router)
        .expect("create forwarder")
        .with_session("generated-session".to_string(), false);
    let provider = codex_oauth_provider(Some("acc-generated"));
    let request = forwarder
        .prepare_request(
            &AppType::Claude,
            &provider,
            "/v1/messages",
            &claude_request_body(),
            &HeaderMap::new(),
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare request")
        .build()
        .expect("build request");

    assert_eq!(header_value(&request, "session_id"), None);
    assert_eq!(header_value(&request, "x-client-request-id"), None);
    assert_eq!(header_value(&request, "x-codex-window-id"), None);
}

#[tokio::test]
async fn codex_oauth_prepare_request_falls_back_to_default_account() {
    let _lock = lock_test_home_and_settings();
    let _manager = CodexOAuthService::test_manager_with_account(
        "acc-default",
        "rt-default",
        Some("default@example.com"),
        Some("at-default"),
        None,
    )
    .await
    .expect("seed default account");

    let provider = codex_oauth_provider(None);
    let request = build_request(&AppType::Claude, &provider, HeaderMap::new()).await;

    assert_eq!(
        header_value(&request, "authorization"),
        Some("Bearer at-default")
    );
    assert_eq!(
        header_value(&request, "chatgpt-account-id"),
        Some("acc-default")
    );
}

#[tokio::test]
async fn codex_oauth_prepare_request_errors_without_available_account() {
    let _lock = lock_test_home_and_settings();
    let _manager = CodexOAuthService::test_empty_manager()
        .await
        .expect("create empty oauth manager");

    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");
    let provider = codex_oauth_provider(None);

    let error = forwarder
        .prepare_request(
            &AppType::Claude,
            &provider,
            "/v1/messages",
            &claude_request_body(),
            &HeaderMap::new(),
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect_err("prepare request should fail without codex oauth account");

    assert!(
        error.to_string().contains("Codex OAuth 认证失败"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn github_copilot_prepare_request_uses_responses_for_openai_vendor_model() {
    let _lock = lock_test_home_and_settings();
    let _auth = seed_copilot_account(
        "12345",
        "copilot-openai-token",
        vec![copilot_model("gpt-5.4", "OpenAI")],
    )
    .await;

    let provider = github_copilot_provider(Some("12345"));
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    let request = forwarder
        .prepare_request(
            &AppType::Claude,
            &provider,
            "/v1/messages?beta=true&x-id=1",
            &json!({
                "model": "gpt-5.4",
                "max_tokens": 32,
                "messages": [{
                    "role": "user",
                    "content": "hello"
                }]
            }),
            &HeaderMap::new(),
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare Copilot OpenAI vendor request")
        .build()
        .expect("build Copilot OpenAI vendor request");

    assert_eq!(
        request.url().as_str(),
        "https://api.githubcopilot.com/v1/responses?x-id=1"
    );
    assert_eq!(
        header_value(&request, "authorization"),
        Some("Bearer copilot-openai-token")
    );
    assert_eq!(
        header_value(&request, "editor-version"),
        Some("vscode/1.110.1")
    );
    assert_eq!(header_value(&request, "anthropic-beta"), None);
    assert_eq!(header_value(&request, "anthropic-version"), None);

    let body = request_body_json(&request);
    assert_eq!(body["model"], "gpt-5.4");
    assert!(body.get("input").is_some());
    assert!(body.get("messages").is_none());
}

#[tokio::test]
async fn github_copilot_prepare_request_uses_chat_for_anthropic_vendor_model() {
    let _lock = lock_test_home_and_settings();
    let _auth = seed_copilot_account(
        "67890",
        "copilot-anthropic-token",
        vec![copilot_model("claude-sonnet-4.6", "Anthropic")],
    )
    .await;

    let provider = github_copilot_provider(Some("67890"));
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    let request = forwarder
        .prepare_request(
            &AppType::Claude,
            &provider,
            "/v1/messages?beta=true&x-id=2",
            &json!({
                "model": "claude-sonnet-4-6",
                "max_tokens": 32,
                "messages": [{
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": "hello"
                    }]
                }]
            }),
            &HeaderMap::new(),
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare Copilot Anthropic vendor request")
        .build()
        .expect("build Copilot Anthropic vendor request");

    assert_eq!(
        request.url().as_str(),
        "https://api.githubcopilot.com/chat/completions?x-id=2"
    );
    assert_eq!(
        header_value(&request, "authorization"),
        Some("Bearer copilot-anthropic-token")
    );
    assert_eq!(header_value(&request, "anthropic-beta"), None);
    assert_eq!(header_value(&request, "anthropic-version"), None);

    let body = request_body_json(&request);
    assert_eq!(body["model"], "claude-sonnet-4.6");
    assert!(body["messages"].is_array());
    assert!(body.get("input").is_none());
}

#[tokio::test]
async fn github_copilot_prepare_request_detects_copilot_base_url_without_provider_type() {
    let _lock = lock_test_home_and_settings();
    let _auth = seed_copilot_account(
        "24680",
        "copilot-base-url-token",
        vec![copilot_model("gpt-5.4", "OpenAI")],
    )
    .await;

    let provider = claude_provider("copilot-url", "https://api.githubcopilot.com", None);
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    let request = forwarder
        .prepare_request(
            &AppType::Claude,
            &provider,
            "/v1/messages",
            &json!({
                "model": "gpt-5.4",
                "max_tokens": 32,
                "messages": [{
                    "role": "user",
                    "content": "hello"
                }]
            }),
            &HeaderMap::new(),
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare Copilot base-url request")
        .build()
        .expect("build Copilot base-url request");

    assert_eq!(
        request.url().as_str(),
        "https://api.githubcopilot.com/v1/responses"
    );
    assert_eq!(
        header_value(&request, "authorization"),
        Some("Bearer copilot-base-url-token")
    );
    assert_eq!(header_value(&request, "anthropic-beta"), None);
    assert_eq!(header_value(&request, "anthropic-version"), None);
}

#[tokio::test]
async fn github_copilot_prepare_request_preserves_full_url_relay() {
    let _lock = lock_test_home_and_settings();
    let _auth = seed_copilot_account(
        "13579",
        "copilot-full-url-token",
        vec![copilot_model("gpt-5.4", "OpenAI")],
    )
    .await;

    let mut provider = github_copilot_provider(Some("13579"));
    provider.settings_config["base_url"] = json!("https://relay.example/copilot/fixed?existing=1");
    provider.meta.as_mut().expect("copilot meta").is_full_url = Some(true);

    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    let request = forwarder
        .prepare_request(
            &AppType::Claude,
            &provider,
            "/v1/messages?beta=true&x-id=3",
            &json!({
                "model": "gpt-5.4",
                "max_tokens": 32,
                "messages": [{
                    "role": "user",
                    "content": "hello"
                }]
            }),
            &HeaderMap::new(),
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare Copilot full-url request")
        .build()
        .expect("build Copilot full-url request");

    assert_eq!(
        request.url().as_str(),
        "https://relay.example/copilot/fixed?existing=1&x-id=3"
    );
    assert_eq!(
        header_value(&request, "authorization"),
        Some("Bearer copilot-full-url-token")
    );
    assert_eq!(header_value(&request, "anthropic-beta"), None);
    assert_eq!(header_value(&request, "anthropic-version"), None);

    let body = request_body_json(&request);
    assert!(body.get("input").is_some());
    assert!(body.get("messages").is_none());
}

#[tokio::test]
async fn github_copilot_prepare_request_sets_agent_initiator_for_tool_results() {
    let _lock = lock_test_home_and_settings();
    let _auth = seed_copilot_account(
        "tool-result",
        "copilot-tool-token",
        vec![copilot_model("claude-sonnet-4.6", "Anthropic")],
    )
    .await;

    let provider = github_copilot_provider(Some("tool-result"));
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    let request = forwarder
        .prepare_request(
            &AppType::Claude,
            &provider,
            "/v1/messages",
            &json!({
                "model": "claude-sonnet-4-6",
                "max_tokens": 32,
                "messages": [
                    {
                        "role": "assistant",
                        "content": [{
                            "type": "tool_use",
                            "id": "call_1",
                            "name": "read_file",
                            "input": {"path": "src/lib.rs"}
                        }]
                    },
                    {
                        "role": "user",
                        "content": [
                            {
                                "type": "tool_result",
                                "tool_use_id": "call_1",
                                "content": "file contents"
                            },
                            {
                                "type": "text",
                                "text": "continue"
                            }
                        ]
                    }
                ]
            }),
            &HeaderMap::new(),
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare Copilot tool result request")
        .build()
        .expect("build Copilot tool result request");

    assert_eq!(header_value(&request, "x-initiator"), Some("agent"));
    assert_eq!(
        header_value(&request, "x-interaction-type"),
        Some("conversation-agent")
    );
    assert_eq!(header_value(&request, "x-interaction-id"), None);
}

#[tokio::test]
async fn github_copilot_prepare_request_sets_subagent_headers_and_interaction_id() {
    let _lock = lock_test_home_and_settings();
    let _auth = seed_copilot_account(
        "subagent",
        "copilot-subagent-token",
        vec![copilot_model("claude-sonnet-4.6", "Anthropic")],
    )
    .await;

    let provider = github_copilot_provider(Some("subagent"));
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    let request = forwarder
        .prepare_request(
            &AppType::Claude,
            &provider,
            "/v1/messages",
            &json!({
                "model": "claude-sonnet-4-6",
                "max_tokens": 32,
                "metadata": {
                    "session_id": "claude-session-abc"
                },
                "messages": [{
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": "<system-reminder>\n{\"__SUBAGENT_MARKER__\":{\"session_id\":\"abc\",\"agent_id\":\"explore-1\",\"agent_type\":\"Explore\"}}\n</system-reminder>\nSearch the repo"
                    }]
                }]
            }),
            &HeaderMap::new(),
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare Copilot subagent request")
        .build()
        .expect("build Copilot subagent request");

    assert_eq!(header_value(&request, "x-initiator"), Some("agent"));
    assert_eq!(
        header_value(&request, "x-interaction-type"),
        Some("conversation-subagent")
    );
    assert!(header_value(&request, "x-interaction-id").is_some());
    assert_eq!(
        header_value(&request, "x-request-id"),
        header_value(&request, "x-agent-task-id")
    );
}

#[tokio::test]
async fn github_copilot_prepare_request_uses_x_session_id_for_interaction_id_fallback() {
    let _lock = lock_test_home_and_settings();
    let _auth = seed_copilot_account(
        "session-header",
        "copilot-session-header-token",
        vec![copilot_model("claude-sonnet-4.6", "Anthropic")],
    )
    .await;

    let provider = github_copilot_provider(Some("session-header"));
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");
    let mut headers = HeaderMap::new();
    headers.insert("x-session-id", HeaderValue::from_static("short-session"));

    let request = forwarder
        .prepare_request(
            &AppType::Claude,
            &provider,
            "/v1/messages",
            &json!({
                "model": "claude-sonnet-4-6",
                "max_tokens": 32,
                "messages": [{
                    "role": "user",
                    "content": "hello"
                }]
            }),
            &headers,
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare Copilot session fallback request")
        .build()
        .expect("build Copilot session fallback request");

    assert!(header_value(&request, "x-interaction-id").is_some());
}

#[tokio::test]
async fn github_copilot_prepare_request_downgrades_warmup_model() {
    let _lock = lock_test_home_and_settings();
    let _auth = seed_copilot_account(
        "warmup",
        "copilot-warmup-token",
        vec![
            copilot_model("gpt-5.4", "OpenAI"),
            copilot_model("gpt-5-mini", "OpenAI"),
        ],
    )
    .await;

    let provider = github_copilot_provider(Some("warmup"));
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");
    let mut headers = HeaderMap::new();
    headers.insert("anthropic-beta", HeaderValue::from_static("beta-signal"));

    let request = forwarder
        .prepare_request(
            &AppType::Claude,
            &provider,
            "/v1/messages?beta=true",
            &json!({
                "model": "gpt-5.4",
                "max_tokens": 32,
                "messages": [{
                    "role": "user",
                    "content": "hello"
                }]
            }),
            &headers,
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare Copilot warmup request")
        .build()
        .expect("build Copilot warmup request");

    assert_eq!(
        request.url().as_str(),
        "https://api.githubcopilot.com/v1/responses"
    );
    assert_eq!(request_body_json(&request)["model"], "gpt-5-mini");
}

#[tokio::test]
async fn github_copilot_prepare_request_strips_thinking_before_transform() {
    let _lock = lock_test_home_and_settings();
    let _auth = seed_copilot_account(
        "thinking",
        "copilot-thinking-token",
        vec![copilot_model("deepseek-reasoner", "Anthropic")],
    )
    .await;

    let provider = github_copilot_provider(Some("thinking"));
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    let request = forwarder
        .prepare_request(
            &AppType::Claude,
            &provider,
            "/v1/messages",
            &json!({
                "model": "deepseek-reasoner",
                "max_tokens": 32,
                "messages": [
                    {
                        "role": "assistant",
                        "content": [
                            {"type": "thinking", "thinking": "secret-thought"},
                            {"type": "tool_use", "id": "call_1", "name": "lookup", "input": {}}
                        ]
                    },
                    {
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": "call_1",
                            "content": "result"
                        }]
                    }
                ]
            }),
            &HeaderMap::new(),
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare Copilot thinking request")
        .build()
        .expect("build Copilot thinking request");

    let body_text = serde_json::to_string(&request_body_json(&request)).expect("serialize body");
    assert!(!body_text.contains("secret-thought"));
}

#[tokio::test]
async fn github_copilot_prepare_request_overrides_client_fingerprint_headers() {
    let _lock = lock_test_home_and_settings();
    let _auth = seed_copilot_account(
        "fingerprint",
        "copilot-fingerprint-token",
        vec![copilot_model("claude-sonnet-4.6", "Anthropic")],
    )
    .await;

    let provider = github_copilot_provider(Some("fingerprint"));
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");
    let mut headers = HeaderMap::new();
    headers.insert("user-agent", HeaderValue::from_static("client-agent"));
    headers.insert("editor-version", HeaderValue::from_static("client-editor"));
    headers.insert("x-initiator", HeaderValue::from_static("user"));
    headers.insert("x-request-id", HeaderValue::from_static("client-request"));
    headers.insert("x-agent-task-id", HeaderValue::from_static("client-task"));

    let request = forwarder
        .prepare_request(
            &AppType::Claude,
            &provider,
            "/v1/messages",
            &json!({
                "model": "claude-sonnet-4-6",
                "max_tokens": 32,
                "messages": [
                    {
                        "role": "assistant",
                        "content": [{
                            "type": "tool_use",
                            "id": "call_1",
                            "name": "read_file",
                            "input": {}
                        }]
                    },
                    {
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": "call_1",
                            "content": "ok"
                        }]
                    }
                ]
            }),
            &headers,
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare Copilot fingerprint request")
        .build()
        .expect("build Copilot fingerprint request");

    assert_eq!(
        header_value(&request, "user-agent"),
        Some("GitHubCopilotChat/0.38.2")
    );
    assert_eq!(
        header_value(&request, "editor-version"),
        Some("vscode/1.110.1")
    );
    assert_eq!(header_value(&request, "x-initiator"), Some("agent"));
    assert_ne!(
        header_value(&request, "x-request-id"),
        Some("client-request")
    );
    assert_eq!(
        header_value(&request, "x-request-id"),
        header_value(&request, "x-agent-task-id")
    );
}

#[tokio::test]
async fn github_copilot_prepare_request_disabled_optimizer_keeps_default_headers_and_model() {
    let _lock = lock_test_home_and_settings();
    let _auth = seed_copilot_account(
        "disabled",
        "copilot-disabled-token",
        vec![
            copilot_model("gpt-5.4", "OpenAI"),
            copilot_model("gpt-5-mini", "OpenAI"),
        ],
    )
    .await;

    let provider = github_copilot_provider(Some("disabled"));
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router)
        .expect("create forwarder")
        .with_copilot_optimizer_config(CopilotOptimizerConfig {
            enabled: false,
            ..Default::default()
        })
        .with_session("claude-session-disabled".to_string(), true);
    let mut headers = HeaderMap::new();
    headers.insert("anthropic-beta", HeaderValue::from_static("beta-signal"));

    let request = forwarder
        .prepare_request(
            &AppType::Claude,
            &provider,
            "/v1/messages?beta=true",
            &json!({
                "model": "gpt-5.4",
                "max_tokens": 32,
                "messages": [{
                    "role": "user",
                    "content": "hello"
                }]
            }),
            &headers,
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare Copilot disabled optimizer request")
        .build()
        .expect("build Copilot disabled optimizer request");

    assert_eq!(header_value(&request, "x-initiator"), Some("user"));
    assert_eq!(
        header_value(&request, "x-interaction-type"),
        Some("conversation-agent")
    );
    assert_eq!(header_value(&request, "x-interaction-id"), None);
    assert_eq!(request_body_json(&request)["model"], "gpt-5.4");
}

#[tokio::test]
async fn codex_oauth_prepare_request_rejects_proxy_managed_placeholder_header() {
    let _lock = lock_test_home_and_settings();
    let _manager = CodexOAuthService::test_manager_with_account(
        "acc-placeholder",
        "rt-placeholder",
        Some("placeholder@example.com"),
        Some("at-placeholder"),
        None,
    )
    .await
    .expect("seed placeholder account");

    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");
    let provider = codex_oauth_provider(Some("acc-placeholder"));
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-extra-auth",
        HeaderValue::from_static("Bearer PROXY_MANAGED"),
    );

    let error = forwarder
        .prepare_request(
            &AppType::Claude,
            &provider,
            "/v1/messages",
            &claude_request_body(),
            &headers,
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect_err("managed upstream placeholder should be rejected before send");

    assert!(
        error.to_string().contains("PROXY_MANAGED"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn non_managed_upstream_allows_proxy_managed_placeholder_guard() {
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");
    let provider = codex_provider("https://example.com");
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-extra-auth",
        HeaderValue::from_static("Bearer PROXY_MANAGED"),
    );

    let request = forwarder
        .prepare_request(
            &AppType::Codex,
            &provider,
            "/v1/responses",
            &json!({"model": "gpt-5.4", "input": "hello"}),
            &headers,
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("non-managed upstream should not reject placeholder-like caller header")
        .build()
        .expect("build request");

    assert_eq!(
        header_value(&request, "x-extra-auth"),
        Some("Bearer PROXY_MANAGED")
    );
}

#[tokio::test]
async fn codex_chat_prepare_request_rewrites_responses_to_chat_completions() {
    let provider = codex_chat_provider("https://example.com/v1", "deepseek-chat");
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");
    let mut headers = HeaderMap::new();
    headers.insert("accept-encoding", HeaderValue::from_static("gzip"));
    let request_body = json!({
        "model": "gpt-5.4",
        "input": "hello",
        "stream": true
    });

    let request = forwarder
        .prepare_request(
            &AppType::Codex,
            &provider,
            "/v1/responses",
            &request_body,
            &headers,
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare Codex Chat bridge request")
        .build()
        .expect("build Codex Chat bridge request");

    assert_eq!(
        request.url().as_str(),
        "https://example.com/v1/chat/completions"
    );
    assert_eq!(
        header_value(&request, "authorization"),
        Some("Bearer codex-key")
    );
    assert_eq!(header_value(&request, "accept-encoding"), Some("identity"));

    let body = request_body_json(&request);
    assert_eq!(body["model"], "deepseek-chat");
    assert!(body.get("input").is_none());
    assert!(body["messages"].is_array());
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"], "hello");
    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"]["include_usage"], true);
}

#[tokio::test]
async fn codex_chat_prepare_request_preserves_responses_query() {
    let provider = codex_chat_provider("https://example.com/v1", "deepseek-chat");
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");
    let request_body = json!({
        "model": "gpt-5.4",
        "input": "hello"
    });

    let request = forwarder
        .prepare_request(
            &AppType::Codex,
            &provider,
            "/v1/responses?foo=bar&api-version=2025-01-01",
            &request_body,
            &HeaderMap::new(),
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare Codex Chat bridge request")
        .build()
        .expect("build Codex Chat bridge request");

    assert_eq!(
        request.url().as_str(),
        "https://example.com/v1/chat/completions?foo=bar&api-version=2025-01-01"
    );
}

#[tokio::test]
async fn codex_chat_prepare_request_preserves_responses_compact_query() {
    let provider = codex_chat_provider("https://example.com/v1", "deepseek-chat");
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");
    let request_body = json!({
        "model": "gpt-5.4",
        "input": "hello"
    });

    let request = forwarder
        .prepare_request(
            &AppType::Codex,
            &provider,
            "/v1/responses/compact?foo=bar",
            &request_body,
            &HeaderMap::new(),
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare Codex Chat compact bridge request")
        .build()
        .expect("build Codex Chat compact bridge request");

    assert_eq!(
        request.url().as_str(),
        "https://example.com/v1/chat/completions?foo=bar"
    );
}

#[tokio::test]
async fn codex_chat_prepare_request_uses_provider_chat_base_without_forcing_v1() {
    let provider = codex_chat_provider("https://api.deepseek.com", "deepseek-v4-pro");
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");
    let request_body = json!({
        "model": "gpt-5.4",
        "input": "hello"
    });

    let request = forwarder
        .prepare_request(
            &AppType::Codex,
            &provider,
            "/v1/responses?foo=bar",
            &request_body,
            &HeaderMap::new(),
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare Codex Chat bridge request")
        .build()
        .expect("build Codex Chat bridge request");

    assert_eq!(
        request.url().as_str(),
        "https://api.deepseek.com/chat/completions?foo=bar"
    );
}

#[tokio::test]
async fn codex_chat_prepare_request_preserves_full_chat_endpoint_base_url() {
    let provider = codex_chat_provider(
        "https://example.com/openai/v1/chat/completions",
        "deepseek-chat",
    );
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");
    let request_body = json!({
        "model": "gpt-5.4",
        "input": "hello"
    });

    let request = forwarder
        .prepare_request(
            &AppType::Codex,
            &provider,
            "/responses",
            &request_body,
            &HeaderMap::new(),
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare Codex Chat bridge request")
        .build()
        .expect("build Codex Chat bridge request");

    assert_eq!(
        request.url().as_str(),
        "https://example.com/openai/v1/chat/completions"
    );
}

#[tokio::test]
async fn codex_chat_prepare_request_preserves_query_with_full_chat_endpoint_base_url() {
    let provider = codex_chat_provider(
        "https://example.com/openai/v1/chat/completions",
        "deepseek-chat",
    );
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");
    let request_body = json!({
        "model": "gpt-5.4",
        "input": "hello"
    });

    let request = forwarder
        .prepare_request(
            &AppType::Codex,
            &provider,
            "/responses?foo=bar",
            &request_body,
            &HeaderMap::new(),
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare Codex Chat bridge request")
        .build()
        .expect("build Codex Chat bridge request");

    assert_eq!(
        request.url().as_str(),
        "https://example.com/openai/v1/chat/completions?foo=bar"
    );
}

async fn build_request(
    app_type: &AppType,
    provider: &Provider,
    headers: HeaderMap,
) -> reqwest::Request {
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    forwarder
        .prepare_request(
            app_type,
            provider,
            "/v1/messages",
            &claude_request_body(),
            &headers,
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare request")
        .build()
        .expect("build request")
}

fn codex_provider(base_url: &str) -> Provider {
    Provider::with_id(
        "codex".to_string(),
        "Codex Provider".to_string(),
        json!({
            "base_url": base_url,
            "apiKey": "codex-key",
        }),
        None,
    )
}

fn codex_chat_provider(base_url: &str, model: &str) -> Provider {
    let mut provider = Provider::with_id(
        "codex-chat".to_string(),
        "Codex Chat Provider".to_string(),
        json!({
            "base_url": base_url,
            "apiKey": "codex-key",
            "model": model,
        }),
        None,
    );
    provider.meta = Some(ProviderMeta {
        api_format: Some("openai_chat".to_string()),
        ..Default::default()
    });
    provider
}

fn codex_oauth_provider(account_id: Option<&str>) -> Provider {
    Provider {
        id: "codex-oauth".to_string(),
        name: "Codex OAuth".to_string(),
        settings_config: json!({
            "base_url": "https://ignored.example.com",
            "apiKey": "ignored-placeholder"
        }),
        website_url: None,
        category: None,
        created_at: None,
        sort_index: None,
        notes: None,
        meta: Some(ProviderMeta {
            provider_type: Some("codex_oauth".to_string()),
            auth_binding: Some(AuthBinding {
                source: AuthBindingSource::ManagedAccount,
                auth_provider: Some("codex_oauth".to_string()),
                account_id: account_id.map(str::to_string),
            }),
            ..Default::default()
        }),
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    }
}

fn github_copilot_provider(account_id: Option<&str>) -> Provider {
    Provider {
        id: "github-copilot".to_string(),
        name: "GitHub Copilot".to_string(),
        settings_config: json!({
            "base_url": "https://ignored.example.com",
            "apiKey": "ignored-placeholder"
        }),
        website_url: None,
        category: None,
        created_at: None,
        sort_index: None,
        notes: None,
        meta: Some(ProviderMeta {
            provider_type: Some("github_copilot".to_string()),
            auth_binding: Some(AuthBinding {
                source: AuthBindingSource::ManagedAccount,
                auth_provider: Some("github_copilot".to_string()),
                account_id: account_id.map(str::to_string),
            }),
            ..Default::default()
        }),
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    }
}

async fn seed_copilot_account(
    account_id: &str,
    copilot_token: &str,
    models: Vec<CopilotModel>,
) -> TestCopilotAuthManagerGuard {
    CopilotAuthService::test_manager_with_account(
        account_id,
        &format!("gho-{account_id}"),
        Some(copilot_token),
        Some("https://api.githubcopilot.com"),
        models,
    )
    .await
    .expect("seed copilot account")
}

fn copilot_model(id: &str, vendor: &str) -> CopilotModel {
    CopilotModel {
        id: id.to_string(),
        name: id.to_string(),
        vendor: vendor.to_string(),
        model_picker_enabled: true,
    }
}

fn header_value<'a>(request: &'a reqwest::Request, name: &str) -> Option<&'a str> {
    request
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
}

fn request_body_json(request: &reqwest::Request) -> Value {
    let bytes = request
        .body()
        .and_then(|body| body.as_bytes())
        .expect("request should have JSON body bytes");
    serde_json::from_slice(bytes).expect("parse request JSON body")
}
