use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use cc_switch_lib::{Database, Provider, ProviderMeta, ProxyService};
use serde_json::{json, Value};

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

#[derive(Clone)]
struct UpstreamState {
    response_body: Arc<Value>,
}

async fn handle_chat_completions(State(state): State<UpstreamState>) -> impl IntoResponse {
    (StatusCode::OK, Json((*state.response_body).clone()))
}

async fn forward_openai_chat_response(upstream_response: Value) -> Value {
    let upstream_state = UpstreamState {
        response_body: Arc::new(upstream_response),
    };
    let upstream_router = Router::new()
        .route("/v1/chat/completions", post(handle_chat_completions))
        .with_state(upstream_state);

    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("read upstream address");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let provider = Provider {
        id: "claude-response-parity".to_string(),
        name: "Claude Response Parity".to_string(),
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
        in_failover_queue: false,
    };
    db.save_provider("claude", &provider)
        .expect("save test provider");
    db.set_current_provider("claude", &provider.id)
        .expect("set current provider");

    db.set_app_proxy_preferred_port("claude", 0)
        .expect("update claude app proxy port");

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

    assert!(response.status().is_success());
    let body: Value = response.json().await.expect("parse proxy response");

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();

    body
}

#[tokio::test]
async fn response_content_parts_and_message_refusal_match_upstream() {
    let body = forward_openai_chat_response(json!({
        "id": "chatcmpl-123",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Hello"},
                    {"type": "output_text", "text": " world"},
                    {"type": "refusal", "refusal": "cannot comply"}
                ],
                "refusal": "policy block"
            },
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5}
    }))
    .await;

    assert_eq!(
        body["content"],
        json!([
            {"type": "text", "text": "Hello"},
            {"type": "text", "text": " world"},
            {"type": "text", "text": "cannot comply"},
            {"type": "text", "text": "policy block"}
        ])
    );
}

#[tokio::test]
async fn response_legacy_function_call_falls_back_to_tool_use() {
    let body = forward_openai_chat_response(json!({
        "id": "chatcmpl-123",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "function_call": {
                    "id": "call_123",
                    "name": "get_weather",
                    "arguments": "{\"location\":\"Tokyo\"}"
                }
            },
            "finish_reason": "function_call"
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5}
    }))
    .await;

    assert_eq!(
        body["content"],
        json!([
            {
                "type": "tool_use",
                "id": "call_123",
                "name": "get_weather",
                "input": {"location": "Tokyo"}
            }
        ])
    );
    assert_eq!(body["stop_reason"], "tool_use");
}

#[tokio::test]
async fn response_finish_reason_mappings_match_upstream() {
    let content_filter = forward_openai_chat_response(json!({
        "id": "chatcmpl-123",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "Blocked"},
            "finish_reason": "content_filter"
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 1}
    }))
    .await;
    assert_eq!(content_filter["stop_reason"], "end_turn");

    let tool_use_fallback = forward_openai_chat_response(json!({
        "id": "chatcmpl-123",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_123",
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "arguments": "{\"location\":\"Tokyo\"}"
                    }
                }]
            }
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5}
    }))
    .await;
    assert_eq!(tool_use_fallback["stop_reason"], "tool_use");
}

#[tokio::test]
async fn response_prompt_tokens_details_cached_tokens_maps_to_cache_read_input_tokens() {
    let body = forward_openai_chat_response(json!({
        "id": "chatcmpl-123",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "Hello"},
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 100,
            "completion_tokens": 50,
            "prompt_tokens_details": {
                "cached_tokens": 80
            }
        }
    }))
    .await;

    // input_tokens is fresh input: prompt_tokens(100) - cache_read(80) = 20.
    assert_eq!(body["usage"]["input_tokens"], 20);
    assert_eq!(body["usage"]["output_tokens"], 50);
    assert_eq!(body["usage"]["cache_read_input_tokens"], 80);
}

#[tokio::test]
async fn response_direct_usage_cache_fields_match_upstream() {
    let body = forward_openai_chat_response(json!({
        "id": "chatcmpl-123",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "Hello"},
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 100,
            "completion_tokens": 50,
            "cache_read_input_tokens": 60,
            "cache_creation_input_tokens": 20
        }
    }))
    .await;

    // input_tokens is fresh input: prompt(100) - cache_read(60) - cache_creation(20) = 20.
    assert_eq!(body["usage"]["input_tokens"], 20);
    assert_eq!(body["usage"]["output_tokens"], 50);
    assert_eq!(body["usage"]["cache_read_input_tokens"], 60);
    assert_eq!(body["usage"]["cache_creation_input_tokens"], 20);
}
