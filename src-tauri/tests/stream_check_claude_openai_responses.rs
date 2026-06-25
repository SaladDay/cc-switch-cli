use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, Method, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use cc_switch_lib::{AppType, Provider, ProviderMeta, StreamCheckConfig, StreamCheckService};
use serde_json::json;
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
    request_method: Arc<Mutex<Option<Method>>>,
    authorization: Arc<Mutex<Option<String>>>,
    api_key: Arc<Mutex<Option<String>>>,
}

async fn handle_reachability_probe(
    State(state): State<UpstreamState>,
    method: Method,
    headers: HeaderMap,
) -> impl IntoResponse {
    *state.request_method.lock().await = Some(method);
    *state.authorization.lock().await = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    *state.api_key.lock().await = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    StatusCode::UNAUTHORIZED
}

#[tokio::test]
async fn stream_check_claude_openai_responses_uses_base_url_reachability() {
    let upstream_state = UpstreamState::default();
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

    let result = StreamCheckService::check_with_retry(
        &AppType::Claude,
        &provider,
        &StreamCheckConfig::default(),
    )
    .await
    .expect("stream check should complete");

    assert!(result.success);
    assert_eq!(result.message, "Reachable");
    assert_eq!(result.http_status, Some(401));
    assert!(result.model_used.is_empty());
    assert_eq!(
        upstream_state.request_method.lock().await.as_ref(),
        Some(&Method::GET)
    );
    assert_eq!(upstream_state.authorization.lock().await.as_deref(), None);
    assert_eq!(upstream_state.api_key.lock().await.as_deref(), None);

    upstream_handle.abort();
}
