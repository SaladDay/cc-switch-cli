use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use cc_switch_lib::{Database, Provider, ProviderMeta, ProxyService};
use serde_json::{json, Value};
use tokio::sync::Mutex;

pub(crate) async fn bind_test_listener() -> tokio::net::TcpListener {
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

pub(crate) async fn set_claude_proxy_port_to_ephemeral(db: &Arc<Database>) {
    db.set_app_proxy_preferred_port("claude", 0)
        .expect("update claude app proxy port");
}

#[derive(Clone, Default)]
pub(crate) struct UpstreamState {
    pub(crate) request_body: Arc<Mutex<Option<Value>>>,
    pub(crate) authorization: Arc<Mutex<Option<String>>>,
    pub(crate) api_key: Arc<Mutex<Option<String>>>,
    pub(crate) anthropic_version: Arc<Mutex<Option<String>>>,
    pub(crate) anthropic_beta: Arc<Mutex<Option<String>>>,
    pub(crate) forwarded_for: Arc<Mutex<Option<String>>>,
}

pub(crate) async fn record_upstream_request(
    state: &UpstreamState,
    headers: &HeaderMap,
    body: Value,
) {
    *state.request_body.lock().await = Some(body);
    *state.authorization.lock().await = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    *state.api_key.lock().await = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    *state.anthropic_version.lock().await = headers
        .get("anthropic-version")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    *state.anthropic_beta.lock().await = headers
        .get("anthropic-beta")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    *state.forwarded_for.lock().await = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
}

pub(crate) async fn handle_chat_completions(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    record_upstream_request(&state, &headers, body).await;

    (
        StatusCode::CREATED,
        [("x-upstream-trace", "claude-openai-chat")],
        Json(json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "created": 123,
            "model": "gpt-4o-mini",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "hello back"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 11,
                "completion_tokens": 7,
                "total_tokens": 18
            }
        })),
    )
}

pub(crate) async fn handle_chat_completions_empty_choices(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    record_upstream_request(&state, &headers, body).await;

    // Mirrors NVIDIA NIM's z-ai/glm-5.2 usage-only tail body (#325).
    (
        StatusCode::OK,
        [("x-upstream-trace", "claude-openai-chat")],
        Json(json!({
            "id": "chatcmpl-empty",
            "object": "chat.completion",
            "created": 123,
            "model": "z-ai/glm-5.2",
            "choices": [],
            "usage": {
                "prompt_tokens": 42,
                "completion_tokens": 0,
                "total_tokens": 42
            }
        })),
    )
}

pub(crate) async fn handle_chat_completions_priced(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    record_upstream_request(&state, &headers, body).await;

    (
        StatusCode::CREATED,
        [("x-upstream-trace", "claude-openai-chat-priced")],
        Json(json!({
            "id": "chatcmpl-priced",
            "object": "chat.completion",
            "created": 123,
            "model": "gpt-5.2",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "hello back"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 11,
                "completion_tokens": 7,
                "total_tokens": 18
            }
        })),
    )
}

pub(crate) async fn handle_responses(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    record_upstream_request(&state, &headers, body).await;

    (
        StatusCode::OK,
        [("x-upstream-trace", "claude-openai-responses")],
        Json(json!({
            "id": "resp_test_123",
            "object": "response",
            "status": "completed",
            "model": "gpt-4.1-mini",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "hello from responses"
                }]
            }],
            "usage": {
                "input_tokens": 13,
                "output_tokens": 5,
                "total_tokens": 18
            }
        })),
    )
}

pub(crate) async fn handle_invalid_chat_completions() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        "{not-json",
    )
}

pub(crate) async fn handle_chat_completions_error() -> impl IntoResponse {
    (
        StatusCode::BAD_REQUEST,
        [("x-upstream-trace", "claude-openai-chat-error")],
        Json(json!({
            "error": {
                "message": "upstream rejected the request",
                "type": "invalid_request_error"
            }
        })),
    )
}

pub(crate) async fn handle_chat_completions_non_standard_json_error() -> impl IntoResponse {
    (
        StatusCode::BAD_REQUEST,
        [
            ("content-type", "application/json"),
            ("x-upstream-trace", "claude-openai-chat-non-standard-error"),
        ],
        Json(json!({
            "message": "upstream rejected the request"
        })),
    )
}

pub(crate) async fn handle_chat_completions_plain_error() -> impl IntoResponse {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [
            ("content-type", "text/plain; charset=utf-8"),
            ("x-upstream-trace", "claude-openai-chat-plain-error"),
        ],
        "upstream rate limit",
    )
}

pub(crate) fn provider_meta_from_json(value: Value) -> ProviderMeta {
    serde_json::from_value(value).expect("parse provider meta")
}

pub(crate) fn request_log_insert_lines(db: &Database) -> Vec<String> {
    db.export_sql_string()
        .expect("export sql string")
        .lines()
        .filter(|line| line.contains("INSERT INTO \"proxy_request_logs\""))
        .map(|line| line.to_string())
        .collect()
}

pub(crate) async fn wait_for_request_log_lines(db: &Database, expected: usize) -> Vec<String> {
    for _ in 0..20 {
        let lines = request_log_insert_lines(db);
        if lines.len() >= expected {
            return lines;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    request_log_insert_lines(db)
}

pub(crate) fn parse_insert_values(line: &str) -> Vec<String> {
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

pub(crate) async fn capture_openai_chat_upstream_body(
    provider_id: &str,
    meta: ProviderMeta,
    request_body: Value,
) -> Value {
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
        id: provider_id.to_string(),
        name: "Claude OpenAI Chat Cache Test".to_string(),
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
        meta: Some(meta),
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
        .json(&request_body)
        .send()
        .await
        .expect("send request to proxy");

    assert!(response.status().is_success());
    let _: Value = response.json().await.expect("parse proxy response");

    let upstream_body = upstream_state
        .request_body
        .lock()
        .await
        .clone()
        .expect("upstream should receive request body");

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();

    upstream_body
}
