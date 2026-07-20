use std::sync::Arc;

use axum::{routing::post, Router};
use cc_switch_lib::{Database, Provider, ProviderMeta, ProxyService};
use serde_json::{json, Value};

use crate::helpers::{
    bind_test_listener, handle_chat_completions_priced, handle_invalid_chat_completions,
    parse_insert_values, set_claude_proxy_port_to_ephemeral, wait_for_request_log_lines,
    UpstreamState,
};

#[tokio::test]
async fn proxy_claude_openai_chat_success_logs_request_with_session_id_and_usage() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route("/v1/chat/completions", post(handle_chat_completions_priced))
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
        id: "claude-openai-chat-log-success".to_string(),
        name: "Claude OpenAI Chat Log Success".to_string(),
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
    db.set_default_cost_multiplier("claude", "2")
        .await
        .expect("set default cost multiplier");

    set_claude_proxy_port_to_ephemeral(&db).await;
    let service = ProxyService::new(db.clone());

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
            "metadata": {
                "session_id": "claude-session-success"
            },
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }))
        .send()
        .await
        .expect("send request to proxy");

    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    let _: Value = response.json().await.expect("parse proxy response");

    let log_lines = wait_for_request_log_lines(&db, 1).await;
    assert_eq!(log_lines.len(), 1);
    let log_values = parse_insert_values(&log_lines[0]);
    assert_eq!(log_values[1], "claude-openai-chat-log-success");
    assert_eq!(log_values[3], "gpt-5.2");
    assert_eq!(log_values[4], "claude-3-7-sonnet");
    assert_eq!(log_values[5], "gpt-5.2");
    assert_eq!(log_values[6], "11");
    assert_eq!(log_values[7], "7");
    assert_eq!(log_values[10], "2");
    assert_eq!(log_values[11], "0.00001925");
    assert_eq!(log_values[12], "0.000098");
    assert_eq!(log_values[15], "0.0002345");
    assert_eq!(log_values[19], "201");
    assert_eq!(log_values[21], "claude-session-success");
    assert_eq!(log_values[24], "2");

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
async fn proxy_claude_buffered_transform_failure_logs_error_request_with_session_id() {
    let upstream_router = Router::new().route(
        "/v1/chat/completions",
        post(handle_invalid_chat_completions),
    );

    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("read upstream address");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let provider = Provider {
        id: "claude-openai-chat-log-failure".to_string(),
        name: "Claude OpenAI Chat Log Failure".to_string(),
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

    set_claude_proxy_port_to_ephemeral(&db).await;
    let service = ProxyService::new(db.clone());

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
            "metadata": {
                "session_id": "claude-session-failure"
            },
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }))
        .send()
        .await
        .expect("send request to proxy");

    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);

    let body: Value = response.json().await.expect("parse proxy error response");
    assert_eq!(
        body.pointer("/error/type").and_then(|value| value.as_str()),
        Some("proxy_error")
    );
    assert!(body
        .pointer("/error/message")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .contains("parse upstream json failed"));

    let log_lines = wait_for_request_log_lines(&db, 1).await;
    assert_eq!(log_lines.len(), 1);
    let log_values = parse_insert_values(&log_lines[0]);
    assert_eq!(log_values[1], "claude-openai-chat-log-failure");
    assert_eq!(log_values[3], "claude-3-7-sonnet");
    assert_eq!(log_values[4], "claude-3-7-sonnet");
    assert_eq!(log_values[5], "");
    assert_eq!(log_values[10], "2");
    assert_eq!(log_values[19], "502");
    assert_eq!(log_values[21], "claude-session-failure");
    assert!(log_values[20].contains("parse upstream json failed"));

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}
