use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Uri},
    routing::post,
    Json, Router,
};
use cc_switch_lib::{Database, Provider, ProxyService};
use serde_json::{json, Value};
use serial_test::serial;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    oauth: Arc<Mutex<Option<String>>>,
}

#[derive(Clone, Default)]
struct RetryUpstreamState {
    attempts: Arc<AtomicUsize>,
}

async fn handle_responses(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    *state.request_body.lock().await = Some(body);
    *state.authorization.lock().await = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    Json(json!({
        "id": "resp_123",
        "object": "response",
        "status": "completed",
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "codex ok"}]
        }]
    }))
}

async fn handle_gemini(
    State(state): State<UpstreamState>,
    Path(path): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    *state.request_body.lock().await = Some(body);
    *state.api_key.lock().await = headers
        .get("x-goog-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    *state.oauth.lock().await = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    Json(json!({
        "path": path,
        "candidates": [{
            "content": {
                "parts": [{"text": "gemini ok"}]
            }
        }]
    }))
}

async fn handle_retrying_codex_response(State(state): State<RetryUpstreamState>) -> Json<Value> {
    let attempt = state.attempts.fetch_add(1, Ordering::SeqCst);
    if attempt == 0 {
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    }

    Json(json!({
        "id": "resp_retry",
        "object": "response",
        "status": "completed",
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": format!("attempt {}", attempt + 1)}]
        }]
    }))
}

async fn handle_slow_codex_response(State(state): State<RetryUpstreamState>) -> Json<Value> {
    state.attempts.fetch_add(1, Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

    Json(json!({
        "id": "resp_slow",
        "object": "response",
        "status": "completed",
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "slow success"}]
        }]
    }))
}

async fn handle_error_codex_response() -> impl axum::response::IntoResponse {
    (
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        Json(json!({
            "error": {"message": "rate limited"}
        })),
    )
}

async fn handle_gemini_streaming(
    State(state): State<UpstreamState>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl axum::response::IntoResponse {
    *state.request_body.lock().await = Some(body);
    *state.api_key.lock().await = headers
        .get("x-goog-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    *state.oauth.lock().await = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let path = uri
        .path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| uri.path().to_string());

    let stream = async_stream::stream! {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        let chunk = format!(
            "data: {{\"path\":\"{}\",\"candidates\":[{{\"content\":{{\"parts\":[{{\"text\":\"gemini stream\"}}]}}}}]}}\n\n",
            path,
        );
        yield Ok::<_, std::io::Error>(bytes::Bytes::from(chunk));
    };

    (
        [("content-type", "text/event-stream")],
        Body::from_stream(stream),
    )
}

#[tokio::test]
#[serial]
async fn proxy_codex_responses_passthroughs_current_provider() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route("/v1/responses", post(handle_responses))
        .with_state(upstream_state.clone());

    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener.local_addr().expect("upstream addr");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let provider = Provider {
        id: "codex-official".to_string(),
        name: "Codex Official".to_string(),
        settings_config: json!({
            "auth": {"OPENAI_API_KEY": "sk-test-codex"},
            "config": format!("base_url = \"http://{}\"\nwire_api = \"responses\"\n", upstream_addr)
        }),
        website_url: None,
        category: Some("codex".to_string()),
        created_at: None,
        sort_index: None,
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };
    db.save_provider("codex", &provider)
        .expect("save codex provider");
    db.set_current_provider("codex", &provider.id)
        .expect("set current codex provider");

    let mut codex_proxy = db
        .get_proxy_config_for_app("codex")
        .await
        .expect("get codex proxy config");
    codex_proxy.auto_failover_enabled = false;
    db.update_proxy_config_for_app(codex_proxy)
        .await
        .expect("update codex proxy config");

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
            "http://{}:{}/v1/responses",
            proxy.address, proxy.port
        ))
        .json(&json!({
            "model": "gpt-5-codex",
            "input": "hello codex"
        }))
        .send()
        .await
        .expect("send codex request to proxy");

    assert!(
        response.status().is_success(),
        "codex proxy should return success"
    );
    let body: Value = response.json().await.expect("parse codex response");
    assert_eq!(
        body.pointer("/output/0/content/0/text")
            .and_then(|v| v.as_str()),
        Some("codex ok")
    );

    let upstream_body = upstream_state
        .request_body
        .lock()
        .await
        .clone()
        .expect("codex upstream should receive body");
    assert_eq!(
        upstream_body.get("input").and_then(|v| v.as_str()),
        Some("hello codex")
    );
    assert_eq!(
        upstream_state.authorization.lock().await.as_deref(),
        Some("Bearer sk-test-codex")
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_codex_responses_compact_passthroughs_current_provider() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route("/v1/responses/compact", post(handle_responses))
        .with_state(upstream_state.clone());

    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener.local_addr().expect("upstream addr");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let provider = Provider {
        id: "codex-official-compact".to_string(),
        name: "Codex Official Compact".to_string(),
        settings_config: json!({
            "auth": {"OPENAI_API_KEY": "sk-test-codex"},
            "config": format!("base_url = \"http://{}\"\nwire_api = \"responses\"\n", upstream_addr)
        }),
        website_url: None,
        category: Some("codex".to_string()),
        created_at: None,
        sort_index: None,
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };
    db.save_provider("codex", &provider)
        .expect("save codex provider");
    db.set_current_provider("codex", &provider.id)
        .expect("set current codex provider");

    let mut codex_proxy = db
        .get_proxy_config_for_app("codex")
        .await
        .expect("get codex proxy config");
    codex_proxy.auto_failover_enabled = false;
    db.update_proxy_config_for_app(codex_proxy)
        .await
        .expect("update codex proxy config");

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
            "http://{}:{}/v1/responses/compact",
            proxy.address, proxy.port
        ))
        .json(&json!({
            "model": "gpt-5-codex",
            "input": "hello compact codex"
        }))
        .send()
        .await
        .expect("send codex compact request to proxy");

    assert!(
        response.status().is_success(),
        "codex compact proxy route should return success"
    );
    let body: Value = response.json().await.expect("parse codex compact response");
    assert_eq!(
        body.pointer("/output/0/content/0/text")
            .and_then(|v| v.as_str()),
        Some("codex ok")
    );

    let upstream_body = upstream_state
        .request_body
        .lock()
        .await
        .clone()
        .expect("codex compact upstream should receive body");
    assert_eq!(
        upstream_body.get("input").and_then(|v| v.as_str()),
        Some("hello compact codex")
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_gemini_passthroughs_v1beta_requests() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route("/v1beta/*path", post(handle_gemini))
        .with_state(upstream_state.clone());

    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener.local_addr().expect("upstream addr");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let provider = Provider {
        id: "gemini-direct".to_string(),
        name: "Gemini Direct".to_string(),
        settings_config: json!({
            "env": {
                "GOOGLE_GEMINI_BASE_URL": format!("http://{}", upstream_addr),
                "GEMINI_API_KEY": "gemini-test-key"
            }
        }),
        website_url: None,
        category: Some("gemini".to_string()),
        created_at: None,
        sort_index: None,
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };
    db.save_provider("gemini", &provider)
        .expect("save gemini provider");
    db.set_current_provider("gemini", &provider.id)
        .expect("set current gemini provider");

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
            "http://{}:{}/v1beta/models/gemini-2.0-flash:generateContent",
            proxy.address, proxy.port
        ))
        .json(&json!({
            "contents": [{
                "role": "user",
                "parts": [{"text": "hello gemini"}]
            }]
        }))
        .send()
        .await
        .expect("send gemini request to proxy");

    assert!(
        response.status().is_success(),
        "gemini proxy should return success"
    );
    let body: Value = response.json().await.expect("parse gemini response");
    assert_eq!(
        body.pointer("/candidates/0/content/parts/0/text")
            .and_then(|v| v.as_str()),
        Some("gemini ok")
    );
    assert_eq!(
        body.get("path").and_then(|v| v.as_str()),
        Some("models/gemini-2.0-flash:generateContent")
    );

    let upstream_body = upstream_state
        .request_body
        .lock()
        .await
        .clone()
        .expect("gemini upstream should receive body");
    assert_eq!(
        upstream_body
            .pointer("/contents/0/parts/0/text")
            .and_then(|v| v.as_str()),
        Some("hello gemini")
    );
    assert_eq!(
        upstream_state.api_key.lock().await.as_deref(),
        Some("gemini-test-key")
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_gemini_prefixed_route_passthroughs_v1beta_requests() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route("/v1beta/*path", post(handle_gemini))
        .with_state(upstream_state.clone());

    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener.local_addr().expect("upstream addr");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let provider = Provider {
        id: "gemini-direct-prefixed".to_string(),
        name: "Gemini Direct Prefixed".to_string(),
        settings_config: json!({
            "env": {
                "GOOGLE_GEMINI_BASE_URL": format!("http://{}", upstream_addr),
                "GEMINI_API_KEY": "gemini-test-key"
            }
        }),
        website_url: None,
        category: Some("gemini".to_string()),
        created_at: None,
        sort_index: None,
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };
    db.save_provider("gemini", &provider)
        .expect("save gemini provider");
    db.set_current_provider("gemini", &provider.id)
        .expect("set current gemini provider");

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
            "http://{}:{}/gemini/v1beta/models/gemini-2.0-flash:generateContent",
            proxy.address, proxy.port
        ))
        .json(&json!({
            "contents": [{
                "role": "user",
                "parts": [{"text": "hello gemini"}]
            }]
        }))
        .send()
        .await
        .expect("send gemini request to proxy");

    assert!(
        response.status().is_success(),
        "prefixed gemini proxy route should return success"
    );
    let body: Value = response.json().await.expect("parse gemini response");
    assert_eq!(
        body.get("path").and_then(|v| v.as_str()),
        Some("models/gemini-2.0-flash:generateContent")
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_codex_retries_with_app_non_streaming_timeout_policy() {
    let upstream_state = RetryUpstreamState::default();
    let upstream_router = Router::new()
        .route("/v1/responses", post(handle_retrying_codex_response))
        .with_state(upstream_state.clone());

    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener.local_addr().expect("upstream addr");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let provider = Provider {
        id: "codex-timeout-retry".to_string(),
        name: "Codex Timeout Retry".to_string(),
        settings_config: json!({
            "auth": {"OPENAI_API_KEY": "sk-test-codex"},
            "config": format!("base_url = \"http://{}\"\nwire_api = \"responses\"\n", upstream_addr)
        }),
        website_url: None,
        category: Some("codex".to_string()),
        created_at: None,
        sort_index: None,
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: true,
    };
    db.save_provider("codex", &provider)
        .expect("save codex provider");
    db.set_current_provider("codex", &provider.id)
        .expect("set current codex provider");

    let mut codex_proxy = db
        .get_proxy_config_for_app("codex")
        .await
        .expect("get codex proxy config");
    codex_proxy.auto_failover_enabled = true;
    codex_proxy.enabled = true;
    codex_proxy.max_retries = 1;
    codex_proxy.non_streaming_timeout = 1;
    db.update_proxy_config_for_app(codex_proxy)
        .await
        .expect("update codex proxy config");
    let saved_codex_proxy = db
        .get_proxy_config_for_app("codex")
        .await
        .expect("reload codex proxy config");
    assert_eq!(saved_codex_proxy.max_retries, 1);
    assert_eq!(saved_codex_proxy.non_streaming_timeout, 1);

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
            "http://{}:{}/v1/responses",
            proxy.address, proxy.port
        ))
        .json(&json!({
            "model": "gpt-5-codex",
            "input": "hello codex"
        }))
        .send()
        .await
        .expect("send codex request to proxy");

    assert!(
        response.status().is_success(),
        "retry request should succeed"
    );
    let body: Value = response.json().await.expect("parse retry response");
    assert_eq!(
        body.pointer("/output/0/content/0/text")
            .and_then(|v| v.as_str()),
        Some("attempt 2"),
        "second upstream attempt should win after the first one times out"
    );
    assert_eq!(
        upstream_state.attempts.load(Ordering::SeqCst),
        2,
        "proxy should use the codex app policy to retry timed out requests"
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_codex_non_streaming_bypasses_timeout_when_failover_disabled() {
    let upstream_state = RetryUpstreamState::default();
    let upstream_router = Router::new()
        .route("/v1/responses", post(handle_slow_codex_response))
        .with_state(upstream_state.clone());

    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener.local_addr().expect("upstream addr");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let provider = Provider {
        id: "codex-timeout-bypass".to_string(),
        name: "Codex Timeout Bypass".to_string(),
        settings_config: json!({
            "auth": {"OPENAI_API_KEY": "sk-test-codex"},
            "config": format!("base_url = \"http://{}\"\nwire_api = \"responses\"\n", upstream_addr)
        }),
        website_url: None,
        category: Some("codex".to_string()),
        created_at: None,
        sort_index: None,
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };
    db.save_provider("codex", &provider)
        .expect("save codex provider");
    db.set_current_provider("codex", &provider.id)
        .expect("set current codex provider");

    let mut codex_proxy = db
        .get_proxy_config_for_app("codex")
        .await
        .expect("get codex proxy config");
    codex_proxy.auto_failover_enabled = false;
    codex_proxy.max_retries = 0;
    codex_proxy.non_streaming_timeout = 1;
    db.update_proxy_config_for_app(codex_proxy)
        .await
        .expect("update codex proxy config");

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
            "http://{}:{}/v1/responses",
            proxy.address, proxy.port
        ))
        .json(&json!({
            "model": "gpt-5-codex",
            "input": "hello codex"
        }))
        .send()
        .await
        .expect("send codex request to proxy");

    assert!(response.status().is_success());
    let body: Value = response.json().await.expect("parse response");
    assert_eq!(
        body.pointer("/output/0/content/0/text")
            .and_then(|v| v.as_str()),
        Some("slow success")
    );
    assert_eq!(upstream_state.attempts.load(Ordering::SeqCst), 1);

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_codex_error_passthrough_does_not_sync_failover_state() {
    let upstream_router = Router::new().route("/v1/responses", post(handle_error_codex_response));

    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener.local_addr().expect("upstream addr");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let current_provider = Provider {
        id: "codex-current-error".to_string(),
        name: "Codex Current Error".to_string(),
        settings_config: json!({
            "auth": {"OPENAI_API_KEY": "sk-test-current"},
            "config": format!("base_url = \"http://{}\"\nwire_api = \"responses\"\n", upstream_addr)
        }),
        website_url: None,
        category: Some("codex".to_string()),
        created_at: None,
        sort_index: Some(1),
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };
    let failover_provider = Provider {
        id: "codex-failover-error".to_string(),
        name: "Codex Failover Error".to_string(),
        settings_config: json!({
            "auth": {"OPENAI_API_KEY": "sk-test-failover"},
            "config": format!("base_url = \"http://{}\"\nwire_api = \"responses\"\n", upstream_addr)
        }),
        website_url: None,
        category: Some("codex".to_string()),
        created_at: None,
        sort_index: Some(0),
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: true,
    };
    db.save_provider("codex", &current_provider)
        .expect("save current provider");
    db.save_provider("codex", &failover_provider)
        .expect("save failover provider");
    db.set_current_provider("codex", &current_provider.id)
        .expect("set current codex provider");

    let mut codex_proxy = db
        .get_proxy_config_for_app("codex")
        .await
        .expect("get codex proxy config");
    codex_proxy.auto_failover_enabled = true;
    codex_proxy.max_retries = 0;
    db.update_proxy_config_for_app(codex_proxy)
        .await
        .expect("update codex proxy config");

    let service = ProxyService::new(db.clone());
    let mut runtime_config = service.get_config().await.expect("read proxy config");
    runtime_config.listen_port = 0;

    let proxy = service
        .start_with_runtime_config(runtime_config)
        .await
        .expect("start proxy service");
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "http://{}:{}/v1/responses",
            proxy.address, proxy.port
        ))
        .json(&json!({
            "model": "gpt-5-codex",
            "input": "hello codex"
        }))
        .send()
        .await
        .expect("send codex request to proxy");

    assert_eq!(response.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        db.get_current_provider("codex")
            .expect("read current provider after codex error")
            .as_deref(),
        Some("codex-current-error")
    );

    let status = service.get_status().await;
    assert_eq!(status.current_provider_id, None);
    assert_eq!(status.current_provider, None);
    assert_eq!(status.failover_count, 0);
    assert!(status.active_targets.is_empty());

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_gemini_query_streaming_uses_stream_timeouts() {
    let upstream_state = UpstreamState::default();
    let upstream_router = Router::new()
        .route("/v1beta/*path", post(handle_gemini_streaming))
        .with_state(upstream_state.clone());

    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener.local_addr().expect("upstream addr");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let provider = Provider {
        id: "gemini-streaming".to_string(),
        name: "Gemini Streaming".to_string(),
        settings_config: json!({
            "env": {
                "GOOGLE_GEMINI_BASE_URL": format!("http://{}", upstream_addr),
                "GEMINI_API_KEY": "gemini-test-key"
            }
        }),
        website_url: None,
        category: Some("gemini".to_string()),
        created_at: None,
        sort_index: None,
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: true,
    };
    db.save_provider("gemini", &provider)
        .expect("save gemini provider");
    db.set_current_provider("gemini", &provider.id)
        .expect("set current gemini provider");

    let mut gemini_proxy = db
        .get_proxy_config_for_app("gemini")
        .await
        .expect("get gemini proxy config");
    gemini_proxy.auto_failover_enabled = true;
    gemini_proxy.enabled = true;
    gemini_proxy.max_retries = 0;
    gemini_proxy.streaming_first_byte_timeout = 1;
    db.update_proxy_config_for_app(gemini_proxy)
        .await
        .expect("update gemini proxy config");

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
            "http://{}:{}/v1beta/models/gemini-2.0-flash:streamGenerateContent?alt=sse",
            proxy.address, proxy.port
        ))
        .json(&json!({
            "contents": [{
                "role": "user",
                "parts": [{"text": "hello gemini"}]
            }]
        }))
        .send()
        .await
        .expect("send gemini streaming request to proxy");

    assert!(
        response.status().is_success(),
        "stream should be established before timeout"
    );
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream")
    );
    let body_result = response.text().await;
    assert!(
        body_result.is_err(),
        "query-based Gemini streaming should use streaming timeouts instead of buffered non-stream handling"
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}
