use std::sync::Arc;

use axum::{http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use cc_switch_lib::{Database, Provider, ProxyService};
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

async fn handle_upstream_failure() -> impl IntoResponse {
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({
            "error": {
                "message": "rate limit exceeded for workspace alpha"
            }
        })),
    )
}

#[tokio::test]
async fn upstream_failure_body_passthrough_keeps_client_body_and_records_summary() {
    let upstream_router = Router::new().route("/v1/messages", post(handle_upstream_failure));

    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("read upstream address");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let provider = Provider {
        id: "claude-upstream-error-summary".to_string(),
        name: "Claude Upstream Error Summary".to_string(),
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

    db.set_app_proxy_preferred_port("claude", 0)
        .expect("update claude proxy port");

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

    assert_eq!(response.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);
    let body: Value = response.json().await.expect("parse proxy response body");
    assert_eq!(
        body,
        json!({
            "error": {
                "message": "rate limit exceeded for workspace alpha"
            }
        })
    );

    let status = service.get_status().await;
    assert_eq!(
        status.last_error,
        Some("upstream returned 429: rate limit exceeded for workspace alpha".to_string())
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}
