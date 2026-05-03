use std::{
    collections::VecDeque,
    env,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{any, post},
    Json, Router,
};
use cc_switch_lib::{Database, Provider, ProviderMeta, ProxyService};
use reqwest::Method;
use serde_json::{json, Value};
use serial_test::serial;
use tokio::sync::Mutex;

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs};

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
        "bind listener failed: {:?}",
        last_error.expect("listener bind should produce an error")
    );
}

#[derive(Clone, Default)]
struct CountingUpstreamState {
    attempts: Arc<AtomicUsize>,
}

#[derive(Clone, Default)]
struct UpstreamState {
    request_body: Arc<Mutex<Option<Value>>>,
}

#[derive(Clone, Default)]
struct ScriptedUpstreamState {
    attempts: Arc<AtomicUsize>,
    request_bodies: Arc<Mutex<Vec<Value>>>,
    responses: Arc<Mutex<VecDeque<(StatusCode, Value)>>>,
}

#[derive(Clone, Default)]
struct ProxyState {
    hit_uris: Arc<Mutex<Vec<String>>>,
}

struct ProxyEnvGuard {
    saved: Vec<(&'static str, Option<String>)>,
}

impl ProxyEnvGuard {
    fn set(proxy_url: Option<&str>) -> Self {
        let proxy_keys = [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
        ];
        let bypass_keys = ["NO_PROXY", "no_proxy"];

        let saved = proxy_keys
            .into_iter()
            .chain(bypass_keys)
            .into_iter()
            .map(|key| {
                let old = env::var(key).ok();
                if bypass_keys.contains(&key) {
                    env::remove_var(key);
                } else {
                    match proxy_url {
                        Some(url) => env::set_var(key, url),
                        None => env::remove_var(key),
                    }
                }
                (key, old)
            })
            .collect();

        Self { saved }
    }
}

impl Drop for ProxyEnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.saved {
            match value {
                Some(value) => env::set_var(key, value),
                None => env::remove_var(key),
            }
        }
    }
}

#[test]
#[serial]
fn proxy_env_guard_keeps_no_proxy_unset_when_installing_proxy_vars() {
    let _test_env_guard = ProxyEnvGuard::set(None);
    env::set_var("NO_PROXY", "localhost,127.0.0.1");
    env::set_var("no_proxy", "localhost,127.0.0.1");

    {
        let _guard = ProxyEnvGuard::set(Some("http://127.0.0.1:18080"));

        assert_eq!(
            env::var("HTTP_PROXY").ok().as_deref(),
            Some("http://127.0.0.1:18080")
        );
        assert!(
            env::var("NO_PROXY").is_err(),
            "NO_PROXY should be cleared so reqwest does not bypass the configured proxy"
        );
        assert!(
            env::var("no_proxy").is_err(),
            "no_proxy should be cleared so reqwest does not bypass the configured proxy"
        );
    }

    assert!(env::var("HTTP_PROXY").is_err());
    assert_eq!(
        env::var("NO_PROXY").ok().as_deref(),
        Some("localhost,127.0.0.1")
    );
    assert_eq!(
        env::var("no_proxy").ok().as_deref(),
        Some("localhost,127.0.0.1")
    );
}

#[test]
#[serial]
fn proxy_env_guard_drop_restores_original_proxy_env_values() {
    let _test_env_guard = ProxyEnvGuard::set(None);

    env::set_var("HTTP_PROXY", "http://original-proxy.example:8080");
    env::set_var("NO_PROXY", "restore-upper.example");
    env::set_var("no_proxy", "restore-lower.example");

    {
        let _guard = ProxyEnvGuard::set(Some("http://127.0.0.1:18080"));

        assert_eq!(
            env::var("HTTP_PROXY").ok().as_deref(),
            Some("http://127.0.0.1:18080")
        );
        assert!(env::var("NO_PROXY").is_err());
        assert!(env::var("no_proxy").is_err());
    }

    assert_eq!(
        env::var("HTTP_PROXY").ok().as_deref(),
        Some("http://original-proxy.example:8080")
    );
    assert_eq!(
        env::var("NO_PROXY").ok().as_deref(),
        Some("restore-upper.example")
    );
    assert_eq!(
        env::var("no_proxy").ok().as_deref(),
        Some("restore-lower.example")
    );
}

async fn handle_anthropic_messages(
    State(state): State<UpstreamState>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    *state.request_body.lock().await = Some(body.clone());

    (
        StatusCode::OK,
        Json(json!({
            "id": "msg_alignment_test",
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "text",
                "text": "ok"
            }],
            "model": body.get("model").cloned().unwrap_or_else(|| json!("")),
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {
                "input_tokens": 1,
                "output_tokens": 1
            }
        })),
    )
}

async fn handle_failing_anthropic_messages(
    State(state): State<CountingUpstreamState>,
    Json(_body): Json<Value>,
) -> impl IntoResponse {
    state.attempts.fetch_add(1, Ordering::SeqCst);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": {"message": "primary unavailable"}})),
    )
}

async fn handle_successful_anthropic_messages(
    State(state): State<CountingUpstreamState>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    state.attempts.fetch_add(1, Ordering::SeqCst);
    (
        StatusCode::OK,
        Json(json!({
            "id": "msg_failover_success",
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "text",
                "text": "failover ok"
            }],
            "model": body.get("model").cloned().unwrap_or_else(|| json!("")),
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {
                "input_tokens": 1,
                "output_tokens": 1
            }
        })),
    )
}

async fn handle_scripted_anthropic_messages(
    State(state): State<ScriptedUpstreamState>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    state.attempts.fetch_add(1, Ordering::SeqCst);
    state.request_bodies.lock().await.push(body);

    let (status, body) = state.responses.lock().await.pop_front().unwrap_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({"error": {"message": "missing scripted response"}}),
    ));

    (status, Json(body))
}

async fn handle_bad_proxy(State(state): State<ProxyState>, request: Request) -> impl IntoResponse {
    state.hit_uris.lock().await.push(request.uri().to_string());
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({"error": "default proxy should not be used"})),
    )
}

async fn handle_forward_proxy(State(state): State<ProxyState>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    state.hit_uris.lock().await.push(parts.uri.to_string());

    let target_url = proxy_target_url(&parts.headers, &parts.uri);
    let body_bytes = to_bytes(body, usize::MAX)
        .await
        .expect("read proxy request body");

    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build proxy forwarding client");
    let method = Method::from_bytes(parts.method.as_str().as_bytes())
        .expect("convert proxied request method");

    let mut upstream_request = client.request(method, target_url);
    for (name, value) in &parts.headers {
        if name.as_str().eq_ignore_ascii_case("proxy-connection") {
            continue;
        }
        upstream_request = upstream_request.header(name, value);
    }

    let upstream_response = upstream_request
        .body(body_bytes)
        .send()
        .await
        .expect("forward request through provider proxy");
    let status = upstream_response.status();
    let headers = upstream_response.headers().clone();
    let bytes = upstream_response
        .bytes()
        .await
        .expect("read proxied upstream response body");

    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    response.headers_mut().extend(headers);
    response
}

fn proxy_target_url(headers: &HeaderMap, uri: &Uri) -> String {
    if uri.scheme_str().is_some() {
        return uri.to_string();
    }

    let host = headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .expect("proxy request should include host header");
    format!("http://{host}{uri}")
}

fn provider_meta_from_json(value: Value) -> ProviderMeta {
    serde_json::from_value(value).expect("parse provider meta")
}

fn anthro_request_with_private_params() -> Value {
    json!({
        "model": "claude-3-5-sonnet-20241022",
        "max_tokens": 64,
        "messages": [{
            "role": "user",
            "_message_secret": "drop-me",
            "content": [{
                "type": "text",
                "text": "hello",
                "_content_secret": "drop-me-too"
            }]
        }],
        "metadata": {
            "keep": "ok",
            "_trace_id": "hidden"
        },
        "_top_secret": "hidden"
    })
}

async fn start_proxy_service(
    upstream_addr: std::net::SocketAddr,
    meta: Option<ProviderMeta>,
) -> ProxyService {
    let db = Arc::new(Database::memory().expect("create memory database"));
    let provider = Provider {
        id: "claude-forwarder-alignment".to_string(),
        name: "Claude Forwarder Alignment".to_string(),
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
        meta,
        icon: None,
        icon_color: None,
        in_failover_queue: false,
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

    service.start().await.expect("start proxy service");
    service
}

async fn start_proxy_service_with_rectifier_config(
    upstream_addr: std::net::SocketAddr,
    meta: Option<ProviderMeta>,
    rectifier_config: Value,
) -> ProxyService {
    let db = Arc::new(Database::memory().expect("create memory database"));
    let provider = Provider {
        id: "claude-forwarder-alignment-rectifier".to_string(),
        name: "Claude Forwarder Alignment Rectifier".to_string(),
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
        meta,
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
        &serde_json::to_string(&rectifier_config).expect("serialize rectifier config"),
    )
    .expect("store rectifier config");

    let service = ProxyService::new(db);
    let mut config = service.get_config().await.expect("read proxy config");
    config.listen_port = 0;
    service
        .update_config(&config)
        .await
        .expect("update proxy config");

    service.start().await.expect("start proxy service");
    service
}

async fn send_claude_request(service: &ProxyService, body: &Value) -> reqwest::Response {
    let status = service.get_status().await;
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build test client")
        .post(format!(
            "http://{}:{}/v1/messages",
            status.address, status.port
        ))
        .header("anthropic-version", "2023-06-01")
        .json(body)
        .send()
        .await
        .expect("send request to proxy")
}

#[tokio::test]
#[serial]
async fn proxy_claude_auto_failover_uses_activated_queue_providers() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let primary_state = CountingUpstreamState::default();
    let primary_listener = bind_test_listener().await;
    let primary_addr = primary_listener
        .local_addr()
        .expect("read primary upstream address");
    let primary_state_for_server = primary_state.clone();
    let primary_handle = tokio::spawn(async move {
        let _ = axum::serve(
            primary_listener,
            Router::new()
                .route("/v1/messages", post(handle_failing_anthropic_messages))
                .with_state(primary_state_for_server),
        )
        .await;
    });

    let secondary_state = CountingUpstreamState::default();
    let secondary_listener = bind_test_listener().await;
    let secondary_addr = secondary_listener
        .local_addr()
        .expect("read secondary upstream address");
    let secondary_state_for_server = secondary_state.clone();
    let secondary_handle = tokio::spawn(async move {
        let _ = axum::serve(
            secondary_listener,
            Router::new()
                .route("/v1/messages", post(handle_successful_anthropic_messages))
                .with_state(secondary_state_for_server),
        )
        .await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let primary_provider = Provider {
        id: "primary".to_string(),
        name: "Primary".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("http://{}", primary_addr),
                "ANTHROPIC_API_KEY": "sk-test-primary"
            }
        }),
        website_url: None,
        category: Some("claude".to_string()),
        created_at: None,
        sort_index: Some(0),
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };
    let secondary_provider = Provider {
        id: "secondary".to_string(),
        name: "Secondary".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("http://{}", secondary_addr),
                "ANTHROPIC_API_KEY": "sk-test-secondary"
            }
        }),
        website_url: None,
        category: Some("claude".to_string()),
        created_at: None,
        sort_index: Some(1),
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };

    db.save_provider("claude", &primary_provider)
        .expect("save primary provider");
    db.save_provider("claude", &secondary_provider)
        .expect("save secondary provider");
    db.set_current_provider("claude", "primary")
        .expect("set current provider");
    db.add_to_failover_queue("claude", "primary")
        .expect("activate primary failover queue entry");
    db.add_to_failover_queue("claude", "secondary")
        .expect("activate secondary failover queue entry");

    let queue_before = db
        .get_failover_queue("claude")
        .expect("read queue before request");
    assert_eq!(queue_before[0].provider_id, "primary");
    assert_eq!(queue_before[1].provider_id, "secondary");

    let mut app_proxy = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("read claude app proxy config");
    app_proxy.auto_failover_enabled = true;
    db.update_proxy_config_for_app(app_proxy)
        .await
        .expect("enable auto failover");

    let service = ProxyService::new(db.clone());
    let mut config = service.get_config().await.expect("read proxy config");
    config.listen_port = 0;
    service
        .update_config(&config)
        .await
        .expect("update proxy config");
    service.start().await.expect("start proxy service");

    let response = send_claude_request(
        &service,
        &json!({
            "model": "claude-3-7-sonnet-20250219",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "hello"
                }]
            }]
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.expect("read failover response body");
    assert_eq!(body["content"][0]["text"], json!("failover ok"));

    assert_eq!(primary_state.attempts.load(Ordering::SeqCst), 1);
    assert_eq!(secondary_state.attempts.load(Ordering::SeqCst), 1);
    assert_eq!(
        db.get_current_provider("claude")
            .expect("read current provider after request")
            .as_deref(),
        Some("secondary")
    );

    let status = service.get_status().await;
    assert_eq!(status.current_provider_id.as_deref(), Some("secondary"));
    assert_eq!(status.current_provider.as_deref(), Some("Secondary"));
    assert_eq!(status.failover_count, 1);
    assert_eq!(status.active_targets.len(), 1);
    assert_eq!(status.active_targets[0].provider_id, "secondary");

    let queue_after = db
        .get_failover_queue("claude")
        .expect("read queue after request");
    assert_eq!(queue_after[0].provider_id, "primary");
    assert_eq!(queue_after[1].provider_id, "secondary");

    service.stop().await.expect("stop proxy service");
    primary_handle.abort();
    secondary_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_claude_successful_failover_syncs_current_provider_and_status() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let upstream_state = UpstreamState::default();
    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("read upstream address");
    let upstream_state_for_server = upstream_state.clone();
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(
            upstream_listener,
            Router::new()
                .route("/v1/messages", post(handle_anthropic_messages))
                .with_state(upstream_state_for_server),
        )
        .await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let original_provider = Provider {
        id: "claude-original".to_string(),
        name: "Claude Original".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("http://{}", upstream_addr),
                "ANTHROPIC_API_KEY": "sk-test-original"
            }
        }),
        website_url: None,
        category: Some("claude".to_string()),
        created_at: None,
        sort_index: Some(1),
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: true,
    };
    let failover_provider = Provider {
        id: "claude-failover".to_string(),
        name: "Claude Failover".to_string(),
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
        meta: None,
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
    let mut app_proxy = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("read claude app proxy config");
    app_proxy.auto_failover_enabled = true;
    db.update_proxy_config_for_app(app_proxy)
        .await
        .expect("enable auto failover");

    let service = ProxyService::new(db.clone());
    let mut config = service.get_config().await.expect("read proxy config");
    config.listen_port = 0;
    service
        .update_config(&config)
        .await
        .expect("update proxy config");
    service.start().await.expect("start proxy service");

    let response = send_claude_request(
        &service,
        &json!({
            "model": "claude-3-7-sonnet-20250219",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "hello"
                }]
            }]
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    assert_eq!(
        db.get_current_provider("claude")
            .expect("read current provider after request")
            .as_deref(),
        Some("claude-failover")
    );

    let status = service.get_status().await;
    assert_eq!(
        status.current_provider_id.as_deref(),
        Some("claude-failover")
    );
    assert_eq!(status.current_provider.as_deref(), Some("Claude Failover"));
    assert_eq!(status.failover_count, 1);
    assert_eq!(status.active_targets.len(), 1);
    assert_eq!(status.active_targets[0].provider_id, "claude-failover");

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn proxy_claude_failed_failover_keeps_state_unsynced() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let upstream_state = ScriptedUpstreamState {
        responses: Arc::new(Mutex::new(VecDeque::from(vec![
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({"error": {"message": "first failed"}}),
            ),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({"error": {"message": "second failed"}}),
            ),
        ]))),
        ..Default::default()
    };
    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("read upstream address");
    let upstream_state_for_server = upstream_state.clone();
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(
            upstream_listener,
            Router::new()
                .route("/v1/messages", post(handle_scripted_anthropic_messages))
                .with_state(upstream_state_for_server),
        )
        .await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let original_provider = Provider {
        id: "claude-original-error".to_string(),
        name: "Claude Original Error".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("http://{}", upstream_addr),
                "ANTHROPIC_API_KEY": "sk-test-original"
            }
        }),
        website_url: None,
        category: Some("claude".to_string()),
        created_at: None,
        sort_index: Some(1),
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: true,
    };
    let failover_provider = Provider {
        id: "claude-failover-error".to_string(),
        name: "Claude Failover Error".to_string(),
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
        meta: None,
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
    let mut app_proxy = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("read claude app proxy config");
    app_proxy.auto_failover_enabled = true;
    app_proxy.max_retries = 0;
    db.update_proxy_config_for_app(app_proxy)
        .await
        .expect("enable auto failover");

    let service = ProxyService::new(db.clone());
    let mut config = service.get_config().await.expect("read proxy config");
    config.listen_port = 0;
    service
        .update_config(&config)
        .await
        .expect("update proxy config");
    service.start().await.expect("start proxy service");

    let response = send_claude_request(
        &service,
        &json!({
            "model": "claude-3-7-sonnet-20250219",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "hello"
                }]
            }]
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    assert_eq!(
        db.get_current_provider("claude")
            .expect("read current provider after failed request")
            .as_deref(),
        Some("claude-original-error")
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
async fn default_http_client_bypasses_self_proxy_with_recursion_guard() {
    let upstream_state = UpstreamState::default();
    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("read upstream address");
    let upstream_state_for_server = upstream_state.clone();
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(
            upstream_listener,
            Router::new()
                .route("/v1/messages", post(handle_anthropic_messages))
                .with_state(upstream_state_for_server),
        )
        .await;
    });

    let service = start_proxy_service(upstream_addr, None).await;
    let proxy_status = service.get_status().await;
    let _env_guard = ProxyEnvGuard::set(Some(&format!("http://127.0.0.1:{}", proxy_status.port)));

    let response = send_claude_request(
        &service,
        &json!({
            "model": "claude-3-5-sonnet-20241022",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }),
    )
    .await;

    assert!(
        response.status().is_success(),
        "fallback global client should bypass self proxy recursion"
    );

    let upstream_body = upstream_state
        .request_body
        .lock()
        .await
        .clone()
        .expect("upstream should receive request body when recursion is guarded");
    assert_eq!(
        upstream_body.get("model").and_then(|value| value.as_str()),
        Some("claude-3-5-sonnet-20241022")
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn provider_proxy_config_overrides_default_http_client_for_claude_requests() {
    let upstream_state = UpstreamState::default();
    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("read upstream address");
    let upstream_state_for_server = upstream_state.clone();
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(
            upstream_listener,
            Router::new()
                .route("/v1/messages", post(handle_anthropic_messages))
                .with_state(upstream_state_for_server),
        )
        .await;
    });

    let good_proxy_state = ProxyState::default();
    let good_proxy_listener = bind_test_listener().await;
    let good_proxy_addr = good_proxy_listener
        .local_addr()
        .expect("read good proxy address");
    let good_proxy_state_for_server = good_proxy_state.clone();
    let good_proxy_handle = tokio::spawn(async move {
        let _ = axum::serve(
            good_proxy_listener,
            Router::new()
                .fallback(any(handle_forward_proxy))
                .with_state(good_proxy_state_for_server),
        )
        .await;
    });

    let bad_proxy_state = ProxyState::default();
    let bad_proxy_listener = bind_test_listener().await;
    let bad_proxy_addr = bad_proxy_listener
        .local_addr()
        .expect("read bad proxy address");
    let bad_proxy_state_for_server = bad_proxy_state.clone();
    let bad_proxy_handle = tokio::spawn(async move {
        let _ = axum::serve(
            bad_proxy_listener,
            Router::new()
                .fallback(any(handle_bad_proxy))
                .with_state(bad_proxy_state_for_server),
        )
        .await;
    });

    let _env_guard = ProxyEnvGuard::set(Some(&format!("http://{}", bad_proxy_addr)));

    let service = start_proxy_service(
        upstream_addr,
        Some(provider_meta_from_json(json!({
            "proxyConfig": {
                "enabled": true,
                "proxyType": "http",
                "proxyHost": "127.0.0.1",
                "proxyPort": good_proxy_addr.port()
            }
        }))),
    )
    .await;

    let response = send_claude_request(
        &service,
        &json!({
            "model": "claude-3-5-sonnet-20241022",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }),
    )
    .await;

    assert!(
        response.status().is_success(),
        "provider proxy config should override the default proxy-bound client"
    );

    let good_hits = good_proxy_state.hit_uris.lock().await.clone();
    let bad_hits = bad_proxy_state.hit_uris.lock().await.clone();
    assert!(
        !good_hits.is_empty(),
        "good proxy should receive the proxied upstream request"
    );
    assert!(
        bad_hits.is_empty(),
        "bad proxy must stay unused when provider proxy config is present"
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
    good_proxy_handle.abort();
    bad_proxy_handle.abort();
}

#[tokio::test]
#[serial]
async fn claude_requests_strip_private_params_before_forwarding_upstream() {
    let upstream_state = UpstreamState::default();
    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("read upstream address");
    let upstream_state_for_server = upstream_state.clone();
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(
            upstream_listener,
            Router::new()
                .route("/v1/messages", post(handle_anthropic_messages))
                .with_state(upstream_state_for_server),
        )
        .await;
    });

    let _env_guard = ProxyEnvGuard::set(None);
    let service = start_proxy_service(upstream_addr, None).await;

    let response = send_claude_request(&service, &anthro_request_with_private_params()).await;
    assert!(response.status().is_success());

    let upstream_body = upstream_state
        .request_body
        .lock()
        .await
        .clone()
        .expect("upstream should receive request body");

    assert!(upstream_body.get("_top_secret").is_none());
    assert!(upstream_body.pointer("/metadata/_trace_id").is_none());
    assert!(upstream_body
        .pointer("/messages/0/_message_secret")
        .is_none());
    assert!(upstream_body
        .pointer("/messages/0/content/0/_content_secret")
        .is_none());
    assert_eq!(
        upstream_body
            .pointer("/metadata/keep")
            .and_then(|value| value.as_str()),
        Some("ok")
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
#[serial]
async fn claude_buffered_rectifier_runtime_disabled_flags_do_not_retry_same_provider() {
    let cases = [
        (
            "enabled_false",
            json!({
                "enabled": false,
                "requestThinkingSignature": true,
                "requestThinkingBudget": true
            }),
            json!({
                "model": "claude-3-7-sonnet-20250219",
                "max_tokens": 32,
                "messages": [{
                    "role": "assistant",
                    "content": [
                        { "type": "thinking", "thinking": "t", "signature": "sig" },
                        { "type": "text", "text": "hello", "signature": "text-sig" }
                    ]
                }]
            }),
            json!({"error": {"message": "messages.1.content.0: Invalid `signature` in `thinking` block"}}),
        ),
        (
            "signature_flag_false",
            json!({
                "enabled": true,
                "requestThinkingSignature": false,
                "requestThinkingBudget": true
            }),
            json!({
                "model": "claude-3-7-sonnet-20250219",
                "max_tokens": 32,
                "messages": [{
                    "role": "assistant",
                    "content": [
                        { "type": "thinking", "thinking": "t", "signature": "sig" },
                        { "type": "text", "text": "hello", "signature": "text-sig" }
                    ]
                }]
            }),
            json!({"error": {"message": "messages.1.content.0: Invalid `signature` in `thinking` block"}}),
        ),
        (
            "budget_flag_false",
            json!({
                "enabled": true,
                "requestThinkingSignature": true,
                "requestThinkingBudget": false
            }),
            json!({
                "model": "claude-3-7-sonnet-20250219",
                "max_tokens": 1024,
                "thinking": { "type": "enabled", "budget_tokens": 512 },
                "messages": [{
                    "role": "user",
                    "content": [{ "type": "text", "text": "hello" }]
                }]
            }),
            json!({"error": {"message": "thinking.budget_tokens: Input should be greater than or equal to 1024"}}),
        ),
    ];

    for (name, rectifier_config, request_body, error_body) in cases {
        let upstream_state = ScriptedUpstreamState {
            responses: Arc::new(Mutex::new(VecDeque::from(vec![
                (StatusCode::BAD_REQUEST, error_body.clone()),
                (
                    StatusCode::OK,
                    json!({"id": "msg_should_not_retry", "content": []}),
                ),
            ]))),
            ..Default::default()
        };
        let upstream_listener = bind_test_listener().await;
        let upstream_addr = upstream_listener
            .local_addr()
            .expect("read upstream address");
        let upstream_state_for_server = upstream_state.clone();
        let upstream_handle = tokio::spawn(async move {
            let _ = axum::serve(
                upstream_listener,
                Router::new()
                    .route("/v1/messages", post(handle_scripted_anthropic_messages))
                    .with_state(upstream_state_for_server),
            )
            .await;
        });

        let service =
            start_proxy_service_with_rectifier_config(upstream_addr, None, rectifier_config).await;
        let response = send_claude_request(&service, &request_body).await;

        assert_eq!(
            response.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "{name} should keep the original 400 instead of retrying"
        );
        assert_eq!(
            upstream_state.attempts.load(Ordering::SeqCst),
            1,
            "{name} should not trigger same-provider rectifier retry"
        );

        service.stop().await.expect("stop proxy service");
        upstream_handle.abort();
    }
}
