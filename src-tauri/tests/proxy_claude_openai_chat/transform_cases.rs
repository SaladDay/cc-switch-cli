use std::sync::Arc;

use axum::{routing::post, Router};
use cc_switch_lib::{Database, Provider, ProviderMeta, ProxyService};
use serde_json::{json, Value};

use crate::helpers::{
    bind_test_listener, capture_openai_chat_upstream_body, handle_chat_completions,
    handle_responses, provider_meta_from_json, set_claude_proxy_port_to_ephemeral, UpstreamState,
};

#[tokio::test]
async fn cache_openai_chat_uses_meta_prompt_cache_key_override() {
    let upstream_body = capture_openai_chat_upstream_body(
        "provider-fallback-id",
        provider_meta_from_json(json!({
            "apiFormat": "openai_chat",
            "promptCacheKey": "custom-cache-key"
        })),
        json!({
            "model": "claude-3-7-sonnet",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }),
    )
    .await;

    assert_eq!(
        upstream_body
            .get("prompt_cache_key")
            .and_then(|value| value.as_str()),
        Some("custom-cache-key")
    );
}

#[tokio::test]
async fn cache_openai_chat_omits_prompt_cache_key_without_explicit_override() {
    let upstream_body = capture_openai_chat_upstream_body(
        "provider-fallback-id",
        provider_meta_from_json(json!({
            "apiFormat": "openai_chat"
        })),
        json!({
            "model": "claude-3-7-sonnet",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }),
    )
    .await;

    assert!(upstream_body.get("prompt_cache_key").is_none());
}

#[tokio::test]
async fn cache_openai_chat_strips_cache_control_metadata() {
    let upstream_body = capture_openai_chat_upstream_body(
        "provider-fallback-id",
        provider_meta_from_json(json!({
            "apiFormat": "openai_chat"
        })),
        json!({
            "model": "claude-3-7-sonnet",
            "max_tokens": 64,
            "system": [{
                "type": "text",
                "text": "system prompt",
                "cache_control": { "type": "ephemeral" }
            }],
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "hello",
                    "cache_control": { "type": "ephemeral", "ttl": "5m" }
                }]
            }],
            "tools": [{
                "name": "get_weather",
                "description": "Get weather",
                "input_schema": { "type": "object" },
                "cache_control": { "type": "ephemeral" }
            }]
        }),
    )
    .await;

    assert!(
        upstream_body.pointer("/messages/0/cache_control").is_none(),
        "system message cache_control should be stripped"
    );
    assert_eq!(
        upstream_body
            .pointer("/messages/1/content")
            .and_then(|v| v.as_str()),
        Some("hello"),
        "single text block should be simplified to plain string"
    );
    assert!(
        upstream_body.pointer("/tools/0/cache_control").is_none(),
        "tool cache_control should be stripped"
    );
}

#[tokio::test]
async fn proxy_claude_openai_chat_transforms_request_and_response() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route("/v1/chat/completions", post(handle_chat_completions))
        .with_state(upstream_state.clone());

    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("read upstream address");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let provider = Provider {
        id: "claude-openai-chat".to_string(),
        name: "Claude OpenAI Chat".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("http://{}", upstream_addr),
                "ANTHROPIC_API_KEY": "sk-test-claude",
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "mapped-sonnet"
            }
        }),
        website_url: None,
        category: Some("claude".to_string()),
        created_at: None,
        sort_index: None,
        notes: None,
        meta: Some(ProviderMeta {
            api_format: Some("openai_chat".to_string()),
            ..ProviderMeta::default()
        }),
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };
    db.save_provider("claude", &provider)
        .expect("save test provider");
    db.set_current_provider("claude", &provider.id)
        .expect("set current provider");

    set_claude_proxy_port_to_ephemeral(&db).await;
    let service = ProxyService::new(db);

    let mut config = service.get_config().await.expect("read proxy config");
    config.listen_port = 0;
    let proxy = service
        .start_with_runtime_config(config)
        .await
        .expect("start proxy service");
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "http://{}:{}/v1/messages",
            proxy.address, proxy.port
        ))
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", "prompt-caching-2024-07-31")
        .header("x-forwarded-for", "203.0.113.9")
        .json(&json!({
            "model": "claude-3-7-sonnet",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }))
        .send()
        .await
        .expect("send request to proxy");

    assert_eq!(
        response.status(),
        reqwest::StatusCode::CREATED,
        "proxy should preserve the upstream status"
    );
    assert_eq!(
        response
            .headers()
            .get("x-upstream-trace")
            .and_then(|v| v.to_str().ok()),
        Some("claude-openai-chat"),
        "proxy should preserve the upstream headers"
    );
    let body: Value = response.json().await.expect("parse proxy response");

    let upstream_body = upstream_state
        .request_body
        .lock()
        .await
        .clone()
        .expect("upstream should receive request body");
    assert_eq!(
        upstream_body.get("model").and_then(|v| v.as_str()),
        Some("mapped-sonnet")
    );
    assert_eq!(
        upstream_body
            .pointer("/messages/0/role")
            .and_then(|v| v.as_str()),
        Some("user")
    );
    assert_eq!(
        upstream_body
            .pointer("/messages/0/content")
            .and_then(|v| v.as_str()),
        Some("hello")
    );

    assert_eq!(
        upstream_state.authorization.lock().await.as_deref(),
        Some("Bearer sk-test-claude")
    );
    assert_eq!(
        upstream_state.api_key.lock().await.as_deref(),
        Some("sk-test-claude")
    );
    assert_eq!(
        upstream_state.anthropic_version.lock().await.as_deref(),
        None
    );
    assert_eq!(upstream_state.anthropic_beta.lock().await.as_deref(), None);
    assert_eq!(
        upstream_state.forwarded_for.lock().await.as_deref(),
        Some("203.0.113.9")
    );

    assert_eq!(body.get("role").and_then(|v| v.as_str()), Some("assistant"));
    assert_eq!(
        body.pointer("/content/0/type").and_then(|v| v.as_str()),
        Some("text")
    );
    assert_eq!(
        body.pointer("/content/0/text").and_then(|v| v.as_str()),
        Some("hello back")
    );
    assert_eq!(
        body.pointer("/usage/input_tokens").and_then(|v| v.as_u64()),
        Some(11)
    );
    assert_eq!(
        body.pointer("/usage/output_tokens")
            .and_then(|v| v.as_u64()),
        Some(7)
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
async fn proxy_claude_openai_responses_transforms_request_and_response() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route("/v1/responses", post(handle_responses))
        .with_state(upstream_state.clone());

    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("read upstream address");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let provider = Provider {
        id: "claude-openai-responses".to_string(),
        name: "Claude OpenAI Responses".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("http://{}", upstream_addr),
                "ANTHROPIC_API_KEY": "sk-test-claude",
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "mapped-sonnet"
            }
        }),
        website_url: None,
        category: Some("claude".to_string()),
        created_at: None,
        sort_index: None,
        notes: None,
        meta: Some(ProviderMeta {
            api_format: Some("openai_responses".to_string()),
            ..ProviderMeta::default()
        }),
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };
    db.save_provider("claude", &provider)
        .expect("save test provider");
    db.set_current_provider("claude", &provider.id)
        .expect("set current provider");

    set_claude_proxy_port_to_ephemeral(&db).await;
    let service = ProxyService::new(db);

    let mut config = service.get_config().await.expect("read proxy config");
    config.listen_port = 0;
    let proxy = service
        .start_with_runtime_config(config)
        .await
        .expect("start proxy service");
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "http://{}:{}/v1/messages",
            proxy.address, proxy.port
        ))
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", "prompt-caching-2024-07-31")
        .header("x-forwarded-for", "203.0.113.10")
        .json(&json!({
            "model": "claude-3-7-sonnet",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "hello"
                }]
            }]
        }))
        .send()
        .await
        .expect("send request to proxy");

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("x-upstream-trace")
            .and_then(|v| v.to_str().ok()),
        Some("claude-openai-responses")
    );
    let body: Value = response.json().await.expect("parse proxy response");

    let upstream_body = upstream_state
        .request_body
        .lock()
        .await
        .clone()
        .expect("upstream should receive request body");
    assert_eq!(
        upstream_body.get("model").and_then(|v| v.as_str()),
        Some("mapped-sonnet")
    );
    assert_eq!(
        upstream_body
            .pointer("/input/0/role")
            .and_then(|v| v.as_str()),
        Some("user")
    );
    assert_eq!(
        upstream_body
            .pointer("/input/0/content/0/type")
            .and_then(|v| v.as_str()),
        Some("input_text")
    );
    assert_eq!(
        upstream_body
            .pointer("/input/0/content/0/text")
            .and_then(|v| v.as_str()),
        Some("hello")
    );
    assert_eq!(
        upstream_body
            .get("max_output_tokens")
            .and_then(|v| v.as_u64()),
        Some(64)
    );
    assert!(upstream_body.get("messages").is_none());

    assert_eq!(
        upstream_state.authorization.lock().await.as_deref(),
        Some("Bearer sk-test-claude")
    );
    assert_eq!(
        upstream_state.api_key.lock().await.as_deref(),
        Some("sk-test-claude")
    );
    assert_eq!(
        upstream_state.anthropic_version.lock().await.as_deref(),
        None
    );
    assert_eq!(upstream_state.anthropic_beta.lock().await.as_deref(), None);
    assert_eq!(
        upstream_state.forwarded_for.lock().await.as_deref(),
        Some("203.0.113.10")
    );

    assert_eq!(body.get("role").and_then(|v| v.as_str()), Some("assistant"));
    assert_eq!(
        body.pointer("/content/0/type").and_then(|v| v.as_str()),
        Some("text")
    );
    assert_eq!(
        body.pointer("/content/0/text").and_then(|v| v.as_str()),
        Some("hello from responses")
    );
    assert_eq!(
        body.pointer("/usage/input_tokens").and_then(|v| v.as_u64()),
        Some(13)
    );
    assert_eq!(
        body.pointer("/usage/output_tokens")
            .and_then(|v| v.as_u64()),
        Some(5)
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}
