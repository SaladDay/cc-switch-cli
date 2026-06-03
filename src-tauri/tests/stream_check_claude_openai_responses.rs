use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use cc_switch_lib::{AppType, Provider, ProviderMeta, StreamCheckConfig, StreamCheckService};
use serde_json::{json, Value};
use tokio::sync::Mutex;

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
struct UpstreamState {
    request_body: Arc<Mutex<Option<Value>>>,
    authorization: Arc<Mutex<Option<String>>>,
    api_key: Arc<Mutex<Option<String>>>,
}

async fn handle_stream_check_responses(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    *state.request_body.lock().await = Some(body);
    *state.authorization.lock().await = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    *state.api_key.lock().await = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let sse = concat!(
        "event: response.created\n",
        "data: {\"response\":{\"id\":\"resp-stream-check\",\"model\":\"gpt-4.1-mini\"}}\n\n"
    );

    (StatusCode::OK, [("content-type", "text/event-stream")], sse)
}

#[tokio::test]
async fn stream_check_claude_openai_responses_uses_responses_endpoint() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route("/v1/responses", post(handle_stream_check_responses))
        .with_state(upstream_state.clone());

    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("read upstream address");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let provider = Provider {
        id: "claude-openai-responses-check".to_string(),
        name: "Claude OpenAI Responses Check".to_string(),
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
            api_format: Some("openai_responses".to_string()),
            ..ProviderMeta::default()
        }),
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };

    let config = StreamCheckConfig::default();
    let result = StreamCheckService::check_with_retry(&AppType::Claude, &provider, &config)
        .await
        .expect("stream check should succeed");

    assert!(result.success);
    assert_eq!(result.http_status, Some(200));

    let upstream_body = upstream_state
        .request_body
        .lock()
        .await
        .clone()
        .expect("upstream should receive request body");
    assert_eq!(
        upstream_body.get("model").and_then(|v| v.as_str()),
        Some(config.claude_model.as_str())
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
        Some(config.test_prompt.as_str())
    );
    assert_eq!(
        upstream_body
            .get("max_output_tokens")
            .and_then(|v| v.as_u64()),
        Some(16)
    );
    assert_eq!(
        upstream_state.authorization.lock().await.as_deref(),
        Some("Bearer sk-test-claude")
    );
    assert_eq!(upstream_state.api_key.lock().await.as_deref(), None);

    upstream_handle.abort();
}

#[tokio::test]
async fn stream_check_openclaw_returns_unsupported_before_auth_extraction() {
    let provider = Provider::with_id(
        "openclaw-check".to_string(),
        "OpenClaw Check".to_string(),
        json!({
            "models": [
                { "id": "gpt-4.1-mini" }
            ]
        }),
        None,
    );

    let err = StreamCheckService::check_with_retry(
        &AppType::OpenClaw,
        &provider,
        &StreamCheckConfig::default(),
    )
    .await
    .expect_err("OpenClaw stream check should be rejected as unsupported");

    assert!(
        err.to_string().contains("openclaw 暂不支持流式检查"),
        "unexpected error: {err}"
    );
}
