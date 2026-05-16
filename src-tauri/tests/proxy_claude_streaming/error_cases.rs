use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use cc_switch_lib::{Database, Provider, ProviderMeta, ProxyService};
use serde_json::{json, Value};
use serial_test::serial;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::helpers::{
    bind_test_listener, handle_slow_streaming_chat, record_upstream_request, wait_for_proxy_status,
    UpstreamState,
};

#[derive(Clone, Default)]
struct RetryStreamingState {
    attempts: Arc<AtomicUsize>,
}

#[derive(Clone, Default)]
struct ScriptedStreamingErrorState {
    attempts: Arc<AtomicUsize>,
    responses: Arc<Mutex<VecDeque<(StatusCode, Value)>>>,
}

async fn handle_idle_streaming_chat(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    record_upstream_request(&state, &headers, body).await;

    let stream = async_stream::stream! {
        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(
            b"data: {\"id\":\"chatcmpl-stream\",\"model\":\"gpt-4o-mini\",\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
        ));
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"data: [DONE]\n\n"));
    };

    (
        StatusCode::OK,
        [("content-type", "text/event-stream")],
        Body::from_stream(stream),
    )
}

async fn handle_delayed_first_chunk_streaming_chat(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    record_upstream_request(&state, &headers, body).await;

    let stream = async_stream::stream! {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(
            b"data: {\"id\":\"chatcmpl-stream\",\"model\":\"gpt-4o-mini\",\"choices\":[{\"delta\":{\"content\":\"late\"}}]}\n\n",
        ));
        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"data: [DONE]\n\n"));
    };

    (
        StatusCode::OK,
        [("content-type", "text/event-stream")],
        Body::from_stream(stream),
    )
}

async fn handle_delayed_headers_and_first_chunk_streaming_chat(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    record_upstream_request(&state, &headers, body).await;

    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    let stream = async_stream::stream! {
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(
            b"data: {\"id\":\"chatcmpl-stream\",\"model\":\"gpt-4o-mini\",\"choices\":[{\"delta\":{\"content\":\"late\"}}]}\n\n",
        ));
        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"data: [DONE]\n\n"));
    };

    (
        StatusCode::OK,
        [("content-type", "text/event-stream")],
        Body::from_stream(stream),
    )
}

async fn handle_slow_json_error_body(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    record_upstream_request(&state, &headers, body).await;

    let stream = async_stream::stream! {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(br#"{"error":"upstream slow body"}"#));
    };

    (
        StatusCode::BAD_REQUEST,
        [("content-type", "application/json")],
        Body::from_stream(stream),
    )
}

async fn handle_non_standard_json_error_body(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    record_upstream_request(&state, &headers, body).await;

    (
        StatusCode::BAD_REQUEST,
        [("content-type", "application/json")],
        Json(json!({
            "message": "upstream rejected the request"
        })),
    )
}

async fn handle_standard_json_error_body(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    record_upstream_request(&state, &headers, body).await;

    (
        StatusCode::BAD_REQUEST,
        [("content-type", "application/json")],
        Json(json!({
            "error": {
                "message": "upstream rejected the request",
                "type": "invalid_request_error"
            }
        })),
    )
}

async fn handle_plain_text_error_body(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    record_upstream_request(&state, &headers, body).await;

    (
        StatusCode::TOO_MANY_REQUESTS,
        [("content-type", "text/plain; charset=utf-8")],
        "upstream rate limit",
    )
}

async fn handle_slow_json_success_body(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    record_upstream_request(&state, &headers, body).await;

    let stream = async_stream::stream! {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(
            br#"{"id":"chatcmpl-slow-fallback","object":"chat.completion","created":123,"model":"gpt-4o-mini","choices":[{"index":0,"message":{"role":"assistant","content":"late hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":11,"completion_tokens":7,"total_tokens":18}}"#,
        ));
    };

    (
        StatusCode::OK,
        [("content-type", "application/json")],
        Body::from_stream(stream),
    )
}

async fn handle_retrying_streaming_timeout(
    State(state): State<RetryStreamingState>,
) -> impl IntoResponse {
    let attempt = state.attempts.fetch_add(1, Ordering::SeqCst);
    if attempt == 0 {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    }

    (
        StatusCode::OK,
        [("content-type", "text/event-stream")],
        "data: {\"id\":\"chatcmpl-stream\",\"model\":\"gpt-4o-mini\",\"choices\":[{\"delta\":{\"content\":\"late\"}}]}\n\ndata: [DONE]\n\n",
    )
}

async fn handle_scripted_streaming_error(
    State(state): State<ScriptedStreamingErrorState>,
    Json(_body): Json<Value>,
) -> impl IntoResponse {
    state.attempts.fetch_add(1, Ordering::SeqCst);
    let (status, body) = state.responses.lock().await.pop_front().unwrap_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({"error": {"message": "missing scripted streaming response"}}),
    ));
    (status, Json(body))
}

#[tokio::test]
#[serial]
async fn proxy_claude_streaming_first_byte_timeout_uses_app_config() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route("/v1/chat/completions", post(handle_slow_streaming_chat))
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
        id: "claude-openai-chat-stream-timeout".to_string(),
        name: "Claude OpenAI Chat Stream Timeout".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("http://{}", upstream_addr),
                "ANTHROPIC_API_KEY": "sk-test-claude"
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
        in_failover_queue: true,
    };
    db.save_provider("claude", &provider)
        .expect("save test provider");
    db.set_current_provider("claude", &provider.id)
        .expect("set current provider");

    let mut claude_proxy = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("get claude proxy config");
    claude_proxy.auto_failover_enabled = true;
    claude_proxy.enabled = true;
    claude_proxy.max_retries = 0;
    claude_proxy.streaming_first_byte_timeout = 1;
    db.update_proxy_config_for_app(claude_proxy)
        .await
        .expect("update claude proxy config");

    let service = ProxyService::new(db);
    let mut config = service.get_config().await.expect("read proxy config");
    config.listen_port = 0;
    service
        .update_config(&config)
        .await
        .expect("update proxy config");

    let proxy = service.start().await.expect("start proxy service");
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "http://{}:{}/v1/messages",
            proxy.address, proxy.port
        ))
        .json(&json!({
            "model": "claude-3-7-sonnet",
            "stream": true,
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
        reqwest::StatusCode::GATEWAY_TIMEOUT,
        "proxy should fail fast when the first streaming byte misses the configured timeout"
    );
    let body: Value = response.json().await.expect("parse timeout error body");
    assert!(
        body.pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .contains("timed out"),
        "timeout error should surface to the client"
    );
    assert_eq!(
        body.pointer("/error/type").and_then(|v| v.as_str()),
        Some("proxy_error")
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_claude_streaming_idle_timeout_uses_app_config() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route("/v1/chat/completions", post(handle_idle_streaming_chat))
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
        id: "claude-openai-chat-stream-idle-timeout".to_string(),
        name: "Claude OpenAI Chat Stream Idle Timeout".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("http://{}", upstream_addr),
                "ANTHROPIC_API_KEY": "sk-test-claude"
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
        in_failover_queue: true,
    };
    db.save_provider("claude", &provider)
        .expect("save test provider");
    db.set_current_provider("claude", &provider.id)
        .expect("set current provider");

    let mut claude_proxy = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("get claude proxy config");
    claude_proxy.auto_failover_enabled = true;
    claude_proxy.enabled = true;
    claude_proxy.max_retries = 0;
    claude_proxy.streaming_idle_timeout = 1;
    db.update_proxy_config_for_app(claude_proxy)
        .await
        .expect("update claude proxy config");

    let service = ProxyService::new(db);
    let mut config = service.get_config().await.expect("read proxy config");
    config.listen_port = 0;
    service
        .update_config(&config)
        .await
        .expect("update proxy config");

    let proxy = service.start().await.expect("start proxy service");
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "http://{}:{}/v1/messages",
            proxy.address, proxy.port
        ))
        .json(&json!({
            "model": "claude-3-7-sonnet",
            "stream": true,
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }))
        .send()
        .await
        .expect("send request to proxy");

    assert!(
        response.status().is_success(),
        "stream setup should still succeed"
    );
    let body_result = response.text().await;
    assert!(
        body_result
            .as_deref()
            .unwrap_or_default()
            .contains("event: error"),
        "anthropic SSE transform should surface the idle-timeout as an error event"
    );
    assert!(
        body_result
            .as_deref()
            .unwrap_or_default()
            .contains("stream idle timeout"),
        "idle-timeout details should be visible in the transformed SSE stream"
    );

    let completed = wait_for_proxy_status(&client, &proxy.address, proxy.port, |status| {
        status.active_connections == 0 && status.failed_requests == 1
    })
    .await;
    assert_eq!(completed.success_requests, 0);
    assert_eq!(completed.total_requests, 1);
    assert!(completed
        .last_error
        .as_deref()
        .unwrap_or_default()
        .contains("stream idle timeout"));

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_claude_streaming_idle_timeout_does_not_sync_failover_state() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route("/v1/chat/completions", post(handle_idle_streaming_chat))
        .with_state(upstream_state.clone());

    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("read upstream address");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let original_provider = Provider {
        id: "claude-stream-current".to_string(),
        name: "Claude Stream Current".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("http://{}", upstream_addr),
                "ANTHROPIC_API_KEY": "sk-test-current"
            }
        }),
        website_url: None,
        category: Some("claude".to_string()),
        created_at: None,
        sort_index: Some(1),
        notes: None,
        meta: Some(ProviderMeta {
            api_format: Some("openai_chat".to_string()),
            ..ProviderMeta::default()
        }),
        icon: None,
        icon_color: None,
        in_failover_queue: true,
    };
    let failover_provider = Provider {
        id: "claude-stream-failover".to_string(),
        name: "Claude Stream Failover".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("http://{}", upstream_addr),
                "ANTHROPIC_API_KEY": "sk-test-failover"
            }
        }),
        website_url: None,
        category: Some("claude".to_string()),
        created_at: None,
        sort_index: Some(0),
        notes: None,
        meta: Some(ProviderMeta {
            api_format: Some("openai_chat".to_string()),
            ..ProviderMeta::default()
        }),
        icon: None,
        icon_color: None,
        in_failover_queue: true,
    };
    db.save_provider("claude", &original_provider)
        .expect("save original provider");
    db.save_provider("claude", &failover_provider)
        .expect("save failover provider");
    db.set_current_provider("claude", &original_provider.id)
        .expect("set current provider");

    let mut claude_proxy = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("get claude proxy config");
    claude_proxy.auto_failover_enabled = true;
    claude_proxy.enabled = true;
    claude_proxy.max_retries = 0;
    claude_proxy.streaming_idle_timeout = 1;
    db.update_proxy_config_for_app(claude_proxy)
        .await
        .expect("update claude proxy config");

    let service = ProxyService::new(db.clone());
    let mut config = service.get_config().await.expect("read proxy config");
    config.listen_port = 0;
    service
        .update_config(&config)
        .await
        .expect("update proxy config");

    let proxy = service.start().await.expect("start proxy service");
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "http://{}:{}/v1/messages",
            proxy.address, proxy.port
        ))
        .json(&json!({
            "model": "claude-3-7-sonnet",
            "stream": true,
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }))
        .send()
        .await
        .expect("send request to proxy");

    assert!(response.status().is_success());
    let body = response
        .text()
        .await
        .expect("read idle-timeout response body");
    assert!(body.contains("stream idle timeout"));

    let completed = wait_for_proxy_status(&client, &proxy.address, proxy.port, |status| {
        status.active_connections == 0 && status.failed_requests == 1
    })
    .await;
    assert_eq!(
        db.get_current_provider("claude")
            .expect("read current provider after idle timeout")
            .as_deref(),
        Some("claude-stream-current")
    );
    assert_eq!(completed.current_provider_id, None);
    assert_eq!(completed.current_provider, None);
    assert_eq!(completed.failover_count, 0);
    assert!(completed.active_targets.is_empty());

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_claude_streaming_first_chunk_timeout_after_headers_uses_app_config() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route(
            "/v1/chat/completions",
            post(handle_delayed_first_chunk_streaming_chat),
        )
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
        id: "claude-openai-chat-stream-first-chunk-timeout".to_string(),
        name: "Claude OpenAI Chat Stream First Chunk Timeout".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("http://{}", upstream_addr),
                "ANTHROPIC_API_KEY": "sk-test-claude"
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
        in_failover_queue: true,
    };
    db.save_provider("claude", &provider)
        .expect("save test provider");
    db.set_current_provider("claude", &provider.id)
        .expect("set current provider");

    let mut claude_proxy = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("get claude proxy config");
    claude_proxy.auto_failover_enabled = true;
    claude_proxy.enabled = true;
    claude_proxy.max_retries = 0;
    claude_proxy.streaming_first_byte_timeout = 1;
    db.update_proxy_config_for_app(claude_proxy)
        .await
        .expect("update claude proxy config");

    let service = ProxyService::new(db);
    let mut runtime_config = service.get_config().await.expect("read proxy config");
    runtime_config.listen_port = 0;

    let proxy = service
        .start_with_runtime_config(runtime_config)
        .await
        .expect("start proxy service");
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "http://{}:{}/v1/messages",
            proxy.address, proxy.port
        ))
        .json(&json!({
            "model": "claude-3-7-sonnet",
            "stream": true,
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }))
        .send()
        .await
        .expect("send request to proxy");

    assert!(
        response.status().is_success(),
        "stream should be established before timeout"
    );
    let body = response.text().await.expect("read timeout event stream");
    assert!(body.contains("event: error"));
    assert!(body.contains("stream timeout after 1s"));

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_claude_streaming_first_byte_timeout_spans_headers_and_first_chunk() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route(
            "/v1/chat/completions",
            post(handle_delayed_headers_and_first_chunk_streaming_chat),
        )
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
        id: "claude-openai-chat-stream-shared-first-byte-budget".to_string(),
        name: "Claude OpenAI Chat Stream Shared First Byte Budget".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("http://{}", upstream_addr),
                "ANTHROPIC_API_KEY": "sk-test-claude"
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
        in_failover_queue: true,
    };
    db.save_provider("claude", &provider)
        .expect("save test provider");
    db.set_current_provider("claude", &provider.id)
        .expect("set current provider");

    let mut claude_proxy = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("get claude proxy config");
    claude_proxy.auto_failover_enabled = true;
    claude_proxy.enabled = true;
    claude_proxy.max_retries = 0;
    claude_proxy.streaming_first_byte_timeout = 1;
    db.update_proxy_config_for_app(claude_proxy)
        .await
        .expect("update claude proxy config");

    let service = ProxyService::new(db);
    let mut runtime_config = service.get_config().await.expect("read proxy config");
    runtime_config.listen_port = 0;

    let proxy = service
        .start_with_runtime_config(runtime_config)
        .await
        .expect("start proxy service");
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "http://{}:{}/v1/messages",
            proxy.address, proxy.port
        ))
        .json(&json!({
            "model": "claude-3-7-sonnet",
            "stream": true,
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }))
        .send()
        .await
        .expect("send request to proxy");

    assert!(
        response.status().is_success(),
        "stream should be established before timeout"
    );
    let body = response.text().await.expect("read timeout event stream");
    assert!(body.contains("event: error"));
    assert!(body.contains("stream timeout after 1s"));

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_claude_streaming_non_sse_error_body_uses_timeout_budget() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route("/v1/chat/completions", post(handle_slow_json_error_body))
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
        id: "claude-openai-chat-stream-json-error-timeout".to_string(),
        name: "Claude OpenAI Chat Stream Json Error Timeout".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("http://{}", upstream_addr),
                "ANTHROPIC_API_KEY": "sk-test-claude"
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
        in_failover_queue: true,
    };
    db.save_provider("claude", &provider)
        .expect("save test provider");
    db.set_current_provider("claude", &provider.id)
        .expect("set current provider");

    let mut claude_proxy = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("get claude proxy config");
    claude_proxy.auto_failover_enabled = true;
    claude_proxy.enabled = true;
    claude_proxy.max_retries = 0;
    claude_proxy.streaming_first_byte_timeout = 1;
    db.update_proxy_config_for_app(claude_proxy)
        .await
        .expect("update claude proxy config");

    let service = ProxyService::new(db);
    let mut runtime_config = service.get_config().await.expect("read proxy config");
    runtime_config.listen_port = 0;

    let proxy = service
        .start_with_runtime_config(runtime_config)
        .await
        .expect("start proxy service");
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "http://{}:{}/v1/messages",
            proxy.address, proxy.port
        ))
        .json(&json!({
            "model": "claude-3-7-sonnet",
            "stream": true,
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }))
        .send()
        .await
        .expect("send request to proxy");

    assert_eq!(response.status(), reqwest::StatusCode::GATEWAY_TIMEOUT);
    let body: Value = response.json().await.expect("parse timeout error body");
    assert!(
        body.pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .contains("stream timeout after 1s"),
        "non-SSE fallback body reads should still honor the streaming timeout budget"
    );
    assert_eq!(
        body.pointer("/error/type").and_then(|v| v.as_str()),
        Some("proxy_error")
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_claude_streaming_non_json_error_body_passthrough_preserves_status_and_body() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route("/v1/chat/completions", post(handle_plain_text_error_body))
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
        id: "claude-openai-chat-stream-plain-error".to_string(),
        name: "Claude OpenAI Chat Stream Plain Error".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("http://{}", upstream_addr),
                "ANTHROPIC_API_KEY": "sk-test-claude"
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
        in_failover_queue: true,
    };
    db.save_provider("claude", &provider)
        .expect("save test provider");
    db.set_current_provider("claude", &provider.id)
        .expect("set current provider");

    let service = ProxyService::new(db);
    let mut config = service.get_config().await.expect("read proxy config");
    config.listen_port = 0;
    service
        .update_config(&config)
        .await
        .expect("update proxy config");

    let proxy = service.start().await.expect("start proxy service");
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "http://{}:{}/v1/messages",
            proxy.address, proxy.port
        ))
        .json(&json!({
            "model": "claude-3-7-sonnet",
            "stream": true,
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }))
        .send()
        .await
        .expect("send request to proxy");

    assert_eq!(response.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(
        response.text().await.expect("read passthrough error body"),
        "upstream rate limit"
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_claude_streaming_non_standard_json_error_body_preserves_upstream_status_and_body() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route(
            "/v1/chat/completions",
            post(handle_non_standard_json_error_body),
        )
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
        id: "claude-openai-chat-stream-non-standard-error".to_string(),
        name: "Claude OpenAI Chat Stream Non Standard Error".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("http://{}", upstream_addr),
                "ANTHROPIC_API_KEY": "sk-test-claude"
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
        in_failover_queue: true,
    };
    db.save_provider("claude", &provider)
        .expect("save test provider");
    db.set_current_provider("claude", &provider.id)
        .expect("set current provider");

    let service = ProxyService::new(db.clone());
    let mut config = service.get_config().await.expect("read proxy config");
    config.listen_port = 0;
    service
        .update_config(&config)
        .await
        .expect("update proxy config");

    let proxy = service.start().await.expect("start proxy service");
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "http://{}:{}/v1/messages",
            proxy.address, proxy.port
        ))
        .json(&json!({
            "model": "claude-3-7-sonnet",
            "stream": true,
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }))
        .send()
        .await
        .expect("send request to proxy");

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let body: Value = response.json().await.expect("parse passthrough error body");
    assert_eq!(
        body,
        json!({
            "message": "upstream rejected the request"
        })
    );

    let status = service.get_status().await;
    assert_eq!(
        status.last_error,
        Some("upstream returned 400: upstream rejected the request".to_string())
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_claude_streaming_standard_json_error_body_transforms_to_anthropic_shape() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route(
            "/v1/chat/completions",
            post(handle_standard_json_error_body),
        )
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
        id: "claude-openai-chat-stream-standard-error".to_string(),
        name: "Claude OpenAI Chat Stream Standard Error".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("http://{}", upstream_addr),
                "ANTHROPIC_API_KEY": "sk-test-claude"
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
        in_failover_queue: true,
    };
    db.save_provider("claude", &provider)
        .expect("save test provider");
    db.set_current_provider("claude", &provider.id)
        .expect("set current provider");

    let service = ProxyService::new(db.clone());
    let mut config = service.get_config().await.expect("read proxy config");
    config.listen_port = 0;
    service
        .update_config(&config)
        .await
        .expect("update proxy config");

    let proxy = service.start().await.expect("start proxy service");
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "http://{}:{}/v1/messages",
            proxy.address, proxy.port
        ))
        .json(&json!({
            "model": "claude-3-7-sonnet",
            "stream": true,
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }))
        .send()
        .await
        .expect("send request to proxy");

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let body: Value = response.json().await.expect("parse transformed error body");
    assert_eq!(
        body.get("type").and_then(|value| value.as_str()),
        Some("error")
    );
    assert_eq!(
        body.pointer("/error/type").and_then(|value| value.as_str()),
        Some("invalid_request_error")
    );
    assert_eq!(
        body.pointer("/error/message")
            .and_then(|value| value.as_str()),
        Some("upstream rejected the request")
    );

    let status = service.get_status().await;
    assert_eq!(
        status.last_error,
        Some("upstream returned 400: upstream rejected the request".to_string())
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_claude_streaming_non_sse_success_fallback_uses_timeout_budget() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route("/v1/chat/completions", post(handle_slow_json_success_body))
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
        id: "claude-openai-chat-stream-json-success-timeout".to_string(),
        name: "Claude OpenAI Chat Stream Json Success Timeout".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("http://{}", upstream_addr),
                "ANTHROPIC_API_KEY": "sk-test-claude"
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
        in_failover_queue: true,
    };
    db.save_provider("claude", &provider)
        .expect("save test provider");
    db.set_current_provider("claude", &provider.id)
        .expect("set current provider");

    let mut claude_proxy = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("get claude proxy config");
    claude_proxy.auto_failover_enabled = true;
    claude_proxy.enabled = true;
    claude_proxy.max_retries = 0;
    claude_proxy.streaming_first_byte_timeout = 1;
    db.update_proxy_config_for_app(claude_proxy)
        .await
        .expect("update claude proxy config");

    let service = ProxyService::new(db);
    let mut runtime_config = service.get_config().await.expect("read proxy config");
    runtime_config.listen_port = 0;

    let proxy = service
        .start_with_runtime_config(runtime_config)
        .await
        .expect("start proxy service");
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "http://{}:{}/v1/messages",
            proxy.address, proxy.port
        ))
        .json(&json!({
            "model": "claude-3-7-sonnet",
            "stream": true,
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }))
        .send()
        .await
        .expect("send request to proxy");

    assert_eq!(response.status(), reqwest::StatusCode::GATEWAY_TIMEOUT);
    let body: Value = response.json().await.expect("parse timeout error body");
    assert!(
        body.pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .contains("stream timeout after 1s"),
        "transformed non-SSE success fallback should still honor the streaming timeout budget"
    );
    assert_eq!(
        body.pointer("/error/type").and_then(|v| v.as_str()),
        Some("proxy_error")
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_claude_streaming_retry_respects_total_first_byte_budget() {
    let upstream_state = RetryStreamingState::default();
    let upstream_router = Router::new()
        .route(
            "/v1/chat/completions",
            post(handle_retrying_streaming_timeout),
        )
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
        id: "claude-openai-chat-stream-retry-budget".to_string(),
        name: "Claude OpenAI Chat Stream Retry Budget".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("http://{}", upstream_addr),
                "ANTHROPIC_API_KEY": "sk-test-claude"
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
        in_failover_queue: true,
    };
    db.save_provider("claude", &provider)
        .expect("save test provider");
    db.set_current_provider("claude", &provider.id)
        .expect("set current provider");

    let mut claude_proxy = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("get claude proxy config");
    claude_proxy.auto_failover_enabled = true;
    claude_proxy.enabled = true;
    claude_proxy.max_retries = 1;
    claude_proxy.streaming_first_byte_timeout = 1;
    db.update_proxy_config_for_app(claude_proxy)
        .await
        .expect("update claude proxy config");

    let service = ProxyService::new(db);
    let mut runtime_config = service.get_config().await.expect("read proxy config");
    runtime_config.listen_port = 0;

    let proxy = service
        .start_with_runtime_config(runtime_config)
        .await
        .expect("start proxy service");
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "http://{}:{}/v1/messages",
            proxy.address, proxy.port
        ))
        .json(&json!({
            "model": "claude-3-7-sonnet",
            "stream": true,
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }))
        .send()
        .await
        .expect("send request to proxy");

    assert_eq!(response.status(), reqwest::StatusCode::GATEWAY_TIMEOUT);
    let body: Value = response.json().await.expect("parse timeout error body");
    assert!(
        body.pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .contains("request timed out after 1s"),
        "stream retries should not restart the first-byte timeout budget from scratch"
    );
    assert_eq!(
        body.pointer("/error/type").and_then(|v| v.as_str()),
        Some("proxy_error")
    );
    assert_eq!(
        upstream_state.attempts.load(Ordering::SeqCst),
        1,
        "proxy should stop retrying once the shared first-byte budget is exhausted"
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_claude_streaming_runtime_disabled_rectifier_does_not_retry_matching_errors() {
    let cases = [
        (
            "signature_error",
            json!({"error": {"message": "messages.1.content.0: Invalid `signature` in `thinking` block"}}),
        ),
        (
            "budget_error",
            json!({"error": {"message": "thinking.budget_tokens: Input should be greater than or equal to 1024"}}),
        ),
    ];

    for (name, error_body) in cases {
        let upstream_state = ScriptedStreamingErrorState {
            responses: Arc::new(Mutex::new(VecDeque::from(vec![
                (StatusCode::BAD_REQUEST, error_body.clone()),
                (
                    StatusCode::OK,
                    json!({"id": "msg_should_not_retry", "content": []}),
                ),
            ]))),
            ..Default::default()
        };
        let upstream_router = Router::new()
            .route("/v1/messages", post(handle_scripted_streaming_error))
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
            id: format!("claude-stream-no-rectifier-retry-{name}"),
            name: format!("Claude Stream No Rectifier Retry {name}"),
            settings_config: json!({
                "env": {
                    "ANTHROPIC_BASE_URL": format!("http://{}", upstream_addr),
                    "ANTHROPIC_API_KEY": "sk-test-claude"
                }
            }),
            website_url: None,
            category: Some("claude".to_string()),
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        };
        db.save_provider("claude", &provider)
            .expect("save test provider");
        db.set_current_provider("claude", &provider.id)
            .expect("set current provider");
        db.set_setting(
            "rectifier_config",
            r#"{"enabled":false,"requestThinkingSignature":true,"requestThinkingBudget":true}"#,
        )
        .expect("disable rectifier config for streaming test");

        let service = ProxyService::new(db);
        let mut config = service.get_config().await.expect("read proxy config");
        config.listen_port = 0;
        service
            .update_config(&config)
            .await
            .expect("update proxy config");

        let proxy = service.start().await.expect("start proxy service");
        let client = reqwest::Client::new();
        let response = client
            .post(format!(
                "http://{}:{}/v1/messages",
                proxy.address, proxy.port
            ))
            .json(&json!({
                "model": "claude-3-7-sonnet",
                "stream": true,
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
            reqwest::StatusCode::BAD_REQUEST,
            "{name} should return the upstream 400 when streaming rectifier is disabled"
        );
        assert_eq!(
            upstream_state.attempts.load(Ordering::SeqCst),
            1,
            "{name} should hit upstream exactly once when streaming rectifier is disabled"
        );

        service.stop().await.expect("stop proxy service");
        upstream_handle.abort();
    }
}
