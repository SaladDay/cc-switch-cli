use axum::{
    body::Body, extract::State, http::HeaderMap, response::IntoResponse, routing::post, Json,
    Router,
};
use cc_switch_lib::{Database, Provider, ProviderMeta, ProxyService};
use serde_json::{json, Value};
use serial_test::serial;
use std::sync::Arc;

use crate::helpers::{
    bind_test_listener, handle_streaming_chat, record_upstream_request,
    set_claude_proxy_port_to_ephemeral, wait_for_proxy_status, UpstreamState,
};

async fn handle_streaming_chat_priced(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    record_upstream_request(&state, &headers, body).await;

    let stream = async_stream::stream! {
        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(
            b"data: {\"id\":\"chatcmpl-stream-priced\",\"model\":\"gpt-5.2\",\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
        ));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(
            b"data: {\"id\":\"chatcmpl-stream-priced\",\"model\":\"gpt-5.2\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":7}}\n\n",
        ));
        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"data: [DONE]\n\n"));
    };

    (
        axum::http::StatusCode::OK,
        [("content-type", "text/event-stream")],
        Body::from_stream(stream),
    )
}

fn request_log_insert_lines(db: &Database) -> Vec<String> {
    db.export_sql_string()
        .expect("export sql string")
        .lines()
        .filter(|line| line.contains("INSERT INTO \"proxy_request_logs\""))
        .map(|line| line.to_string())
        .collect()
}

fn parse_insert_values(line: &str) -> Vec<String> {
    let values_start = line.find("VALUES").expect("insert values keyword");
    let start = line[values_start..]
        .find('(')
        .map(|offset| values_start + offset + 1)
        .expect("insert values start");
    let end = line.rfind(')').expect("insert values end");
    let values = &line[start..end];

    let mut parsed = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = values.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\'' => {
                if in_quotes && chars.peek() == Some(&'\'') {
                    current.push('\'');
                    chars.next();
                } else {
                    in_quotes = !in_quotes;
                }
            }
            ',' if !in_quotes => {
                parsed.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    parsed.push(current.trim().to_string());
    parsed
}

async fn wait_for_request_log_lines(db: &Database, expected: usize) -> Vec<String> {
    for _ in 0..20 {
        let lines = request_log_insert_lines(db);
        if lines.len() >= expected {
            return lines;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    request_log_insert_lines(db)
}

#[tokio::test]
#[serial]
async fn stream_openai_chat_logs_request_with_session_id_and_usage() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route("/v1/chat/completions", post(handle_streaming_chat_priced))
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
        id: "claude-openai-chat-stream-log".to_string(),
        name: "Claude OpenAI Chat Stream Log".to_string(),
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
        .json(&json!({
            "model": "claude-3-7-sonnet",
            "stream": true,
            "max_tokens": 64,
            "metadata": {
                "session_id": "claude-stream-session"
            },
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }))
        .send()
        .await
        .expect("send request to proxy");

    assert!(response.status().is_success());
    let body = response.text().await.expect("read streaming response body");
    assert!(body.contains("event: message_delta"));

    let log_lines = wait_for_request_log_lines(&db, 1).await;
    assert_eq!(log_lines.len(), 1);
    let log_values = parse_insert_values(&log_lines[0]);
    assert_eq!(log_values[1], "claude-openai-chat-stream-log");
    assert_eq!(log_values[3], "gpt-5.2");
    assert_eq!(log_values[4], "claude-3-7-sonnet");
    assert_eq!(log_values[5], "gpt-5.2");
    assert_eq!(log_values[6], "11");
    assert_eq!(log_values[7], "7");
    assert_eq!(log_values[10], "2");
    assert_eq!(log_values[11], "0.00001925");
    assert_eq!(log_values[12], "0.000098");
    assert_eq!(log_values[15], "0.0002345");
    assert_eq!(log_values[19], "200");
    assert_eq!(log_values[21], "claude-stream-session");
    assert_eq!(log_values[23], "1");
    assert_eq!(log_values[24], "2");

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_claude_streaming_client_abort_counts_as_failure() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route("/v1/chat/completions", post(handle_streaming_chat))
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
        id: "claude-openai-chat-stream-client-abort".to_string(),
        name: "Claude OpenAI Chat Stream Client Abort".to_string(),
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

    set_claude_proxy_port_to_ephemeral(&db).await;
    let service = ProxyService::new(db);

    let mut config = service.get_config().await.expect("read proxy config");
    config.listen_port = 0;
    let proxy = service
        .start_with_runtime_config(config)
        .await
        .expect("start proxy service");
    let client = reqwest::Client::new();
    let mut response = client
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
        "proxy should establish the stream"
    );
    let first_chunk = response
        .chunk()
        .await
        .expect("read first streaming response chunk")
        .expect("first stream chunk should exist");
    assert!(String::from_utf8_lossy(&first_chunk).contains("event: message_start"));

    drop(response);

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
        .contains("before completion"));

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}
