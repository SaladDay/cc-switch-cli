//! Route mode end-to-end coverage: weighted distribution through a live
//! proxy, per-member model mapping, in-request failover, failover-queue
//! exclusion, lifecycle guard errors, startup-recovery flag persistence,
//! and hung-member timeout failover.
//!
//! Mock upstreams bind on 127.0.0.1:0; each route member points at its own
//! upstream so per-provider hit counts are observable.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use cc_switch_lib::{
    get_claude_settings_path, read_json_file, AppState, Database, Provider, ProxyService,
    RouteModeGroup, RouteModeMember,
};
use serde_json::{json, Value};
use serial_test::serial;

use support::{ensure_test_home, lock_test_mutex, reset_test_fs};

#[path = "support.rs"]
mod support;

async fn bind_test_listener() -> tokio::net::TcpListener {
    let mut last_error = None;
    for _ in 0..20 {
        match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => return listener,
            Err(error) => {
                last_error = Some(error);
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    }

    panic!(
        "bind upstream listener: {:?}",
        last_error.expect("listener bind should produce an error")
    );
}

/// Shared per-upstream counters: index 0 tracks hits, index 1 the last
/// request model seen by that upstream.
#[derive(Clone, Default)]
struct UpstreamState {
    hits: Arc<AtomicUsize>,
    last_model: Arc<std::sync::Mutex<Option<String>>>,
}

impl UpstreamState {
    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    fn record(&self, model: Option<String>) {
        self.hits.fetch_add(1, Ordering::SeqCst);
        *self.last_model.lock().expect("lock upstream model") = model;
    }

    fn last_model(&self) -> Option<String> {
        self.last_model.lock().expect("lock upstream model").clone()
    }
}

async fn handle_anthropic_ok(
    State(state): State<UpstreamState>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    state.record(body.get("model").and_then(Value::as_str).map(String::from));

    (
        StatusCode::OK,
        Json(json!({
            "id": "msg_route_mode",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "ok"}],
            "model": body.get("model").cloned().unwrap_or_else(|| json!("")),
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {"input_tokens": 1, "output_tokens": 1}
        })),
    )
}

async fn handle_anthropic_500(
    State(state): State<UpstreamState>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    state.record(body.get("model").and_then(Value::as_str).map(String::from));

    (StatusCode::INTERNAL_SERVER_ERROR, "upstream exploded")
}

async fn handle_anthropic_hang(
    State(state): State<UpstreamState>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    state.record(body.get("model").and_then(Value::as_str).map(String::from));
    tokio::time::sleep(Duration::from_secs(30)).await;

    (StatusCode::OK, "unreachable")
}

fn anthropic_provider(id: &str, name: &str, base_url: &str) -> Provider {
    Provider {
        id: id.to_string(),
        name: name.to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": base_url,
                "ANTHROPIC_API_KEY": "sk-route-mode"
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
    }
}

fn anthropic_request(model: &str) -> Value {
    json!({
        "model": model,
        "max_tokens": 64,
        "messages": [{"role": "user", "content": "hello"}]
    })
}

async fn save_route_group(db: &Database, members: Vec<RouteModeMember>) {
    let group = RouteModeGroup {
        enabled: false,
        members,
    };
    db.save_route_mode_group("claude", &group)
        .expect("save route group");
    db.set_route_mode_enabled("claude", true)
        .expect("enable route mode");
}

async fn start_proxy(db: Arc<Database>) -> (ProxyService, String) {
    db.set_app_proxy_preferred_port("claude", 0)
        .expect("set claude proxy port");
    let service = ProxyService::new(db);
    let mut config = service.get_config().await.expect("read proxy config");
    config.listen_port = 0;
    let proxy = service
        .start_with_runtime_config(config)
        .await
        .expect("start proxy service");
    let base = format!("http://{}:{}", proxy.address, proxy.port);
    (service, base)
}

async fn start_proxy_for_app(db: Arc<Database>, app_type: &str) -> (ProxyService, String) {
    db.set_app_proxy_preferred_port(app_type, 0)
        .unwrap_or_else(|_| panic!("set {app_type} proxy port"));
    let service = ProxyService::new(db);
    let mut config = service.get_config().await.expect("read proxy config");
    config.listen_port = 0;
    let proxy = service
        .start_with_runtime_config(config)
        .await
        .expect("start proxy service");
    let base = format!("http://{}:{}", proxy.address, proxy.port);
    (service, base)
}

async fn send_request(base: &str, model: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{base}/v1/messages"))
        .header("anthropic-version", "2023-06-01")
        .json(&anthropic_request(model))
        .send()
        .await
        .expect("send request to proxy")
}

struct MockUpstream {
    state: UpstreamState,
    base_url: String,
    handle: tokio::task::JoinHandle<()>,
}

/// Spawns an ok-upstream and returns (mock, axum state) — the state must be
/// cloned into the router before spawning, so this helper wires both.
async fn spawn_ok_upstream() -> (MockUpstream, UpstreamState) {
    let state = UpstreamState::default();
    let router = Router::new()
        .route("/v1/messages", post(handle_anthropic_ok))
        .with_state(state.clone());
    let listener = bind_test_listener().await;
    let addr = listener.local_addr().expect("read upstream address");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (
        MockUpstream {
            state: state.clone(),
            base_url: format!("http://{addr}"),
            handle,
        },
        state,
    )
}

/// Spawns a Codex /v1/responses upstream and returns (mock, axum state).
async fn spawn_codex_upstream() -> (MockUpstream, UpstreamState) {
    let state = UpstreamState::default();
    let router = Router::new()
        .route("/v1/responses", post(handle_codex_responses_ok))
        .with_state(state.clone());
    let listener = bind_test_listener().await;
    let addr = listener.local_addr().expect("read upstream address");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (
        MockUpstream {
            state: state.clone(),
            base_url: format!("http://{addr}"),
            handle,
        },
        state,
    )
}

fn drop_upstreams(upstreams: Vec<MockUpstream>) {
    for upstream in upstreams {
        upstream.handle.abort();
    }
}

#[tokio::test]
#[serial]
async fn weighted_distribution_is_three_to_one_over_forty_requests() {
    let (upstream_a, _state_a) = spawn_ok_upstream().await;
    let (upstream_b, _state_b) = spawn_ok_upstream().await;

    let db = Arc::new(Database::memory().expect("create memory database"));
    db.save_provider(
        "claude",
        &anthropic_provider("p-a", "Alpha", &upstream_a.base_url),
    )
    .expect("save provider a");
    db.save_provider(
        "claude",
        &anthropic_provider("p-b", "Beta", &upstream_b.base_url),
    )
    .expect("save provider b");
    save_route_group(
        &db,
        vec![
            RouteModeMember::new("p-a", 3),
            RouteModeMember::new("p-b", 1),
        ],
    )
    .await;

    let (service, base) = start_proxy(db).await;

    let mut a_count = 0;
    let mut b_count = 0;
    for _ in 0..40 {
        let before_a = upstream_a.state.hits();
        let response = send_request(&base, "claude-sonnet-4-6").await;
        assert!(
            response.status().is_success(),
            "route mode request should succeed"
        );
        if upstream_a.state.hits() == before_a + 1 {
            a_count += 1;
        } else {
            b_count += 1;
        }
    }

    assert_eq!(
        (a_count, b_count),
        (30, 10),
        "3:1 weights over 40 requests: the exact smooth-WRR cycle"
    );

    service.stop().await.expect("stop proxy service");
    drop_upstreams(vec![upstream_a, upstream_b]);
}

#[tokio::test]
#[serial]
async fn route_mapping_rewrites_matched_model_and_leaves_unmapped_verbatim() {
    let (upstream_a, _state_a) = spawn_ok_upstream().await;
    let (upstream_b, _state_b) = spawn_ok_upstream().await;

    let db = Arc::new(Database::memory().expect("create memory database"));
    db.save_provider(
        "claude",
        &anthropic_provider("p-a", "Alpha", &upstream_a.base_url),
    )
    .expect("save provider a");
    db.save_provider(
        "claude",
        &anthropic_provider("p-b", "Beta", &upstream_b.base_url),
    )
    .expect("save provider b");

    let mut member_a = RouteModeMember::new("p-a", 1);
    member_a
        .model_mappings
        .insert("m-a".to_string(), "m-b".to_string());
    save_route_group(&db, vec![member_a, RouteModeMember::new("p-b", 1)]).await;

    let (service, base) = start_proxy(db).await;

    // Equal weights alternate, so request 1 hits the mapped member and
    // request 2 hits the unmapped one.
    let first = send_request(&base, "m-a").await;
    assert!(first.status().is_success());
    let second = send_request(&base, "m-a").await;
    assert!(second.status().is_success());

    assert_eq!(
        upstream_a.state.last_model().as_deref(),
        Some("m-b"),
        "mapped member must receive the mapped model"
    );
    assert_eq!(
        upstream_b.state.last_model().as_deref(),
        Some("m-a"),
        "unmapped member must receive the request model verbatim"
    );

    service.stop().await.expect("stop proxy service");
    drop_upstreams(vec![upstream_a, upstream_b]);
}

#[tokio::test]
#[serial]
async fn upstream_500_fails_over_to_backup_and_records_breaker_failure() {
    let listener = bind_test_listener().await;
    let addr = listener
        .local_addr()
        .expect("read failing upstream address");
    let failing_state = UpstreamState::default();
    let failing_router = Router::new()
        .route("/v1/messages", post(handle_anthropic_500))
        .with_state(failing_state.clone());
    let failing_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, failing_router).await;
    });
    let failing = MockUpstream {
        state: failing_state.clone(),
        base_url: format!("http://{addr}"),
        handle: failing_handle,
    };

    let (upstream_b, _state_b) = spawn_ok_upstream().await;

    let db = Arc::new(Database::memory().expect("create memory database"));
    db.save_provider(
        "claude",
        &anthropic_provider("p-a", "Failing", &failing.base_url),
    )
    .expect("save failing provider");
    db.save_provider(
        "claude",
        &anthropic_provider("p-b", "Backup", &upstream_b.base_url),
    )
    .expect("save backup provider");
    save_route_group(
        &db,
        vec![
            RouteModeMember::new("p-a", 1),
            RouteModeMember::new("p-b", 1),
        ],
    )
    .await;

    let (service, base) = start_proxy(db.clone()).await;

    let response = send_request(&base, "claude-sonnet-4-6").await;
    assert!(
        response.status().is_success(),
        "first request must land on the backup after the 500"
    );

    assert_eq!(failing.state.hits(), 1, "failing member got one attempt");
    assert_eq!(upstream_b.state.hits(), 1, "backup served the same request");

    let health = db
        .get_provider_health("p-a", "claude")
        .await
        .expect("read provider health");
    assert_eq!(
        health.consecutive_failures, 1,
        "breaker must record the upstream failure"
    );

    service.stop().await.expect("stop proxy service");
    drop_upstreams(vec![failing, upstream_b]);
}

#[tokio::test]
#[serial]
async fn failover_queued_non_member_receives_no_requests() {
    let (upstream_a, _state_a) = spawn_ok_upstream().await;
    let (upstream_b, _state_b) = spawn_ok_upstream().await;
    let (upstream_c, _state_c) = spawn_ok_upstream().await;

    let db = Arc::new(Database::memory().expect("create memory database"));
    db.save_provider(
        "claude",
        &anthropic_provider("p-a", "Alpha", &upstream_a.base_url),
    )
    .expect("save provider a");
    db.save_provider(
        "claude",
        &anthropic_provider("p-b", "Beta", &upstream_b.base_url),
    )
    .expect("save provider b");
    let mut queued = anthropic_provider("p-c", "Queued", &upstream_c.base_url);
    queued.in_failover_queue = true;
    db.save_provider("claude", &queued)
        .expect("save queued provider");
    db.add_to_failover_queue("claude", "p-c")
        .expect("queue the non-member");
    save_route_group(
        &db,
        vec![
            RouteModeMember::new("p-a", 1),
            RouteModeMember::new("p-b", 1),
        ],
    )
    .await;

    let (service, base) = start_proxy(db).await;

    for _ in 0..6 {
        let response = send_request(&base, "claude-sonnet-4-6").await;
        assert!(response.status().is_success());
    }

    assert_eq!(
        upstream_c.state.hits(),
        0,
        "route mode must ignore the failover queue"
    );

    service.stop().await.expect("stop proxy service");
    drop_upstreams(vec![upstream_a, upstream_b, upstream_c]);
}

#[tokio::test]
#[serial]
async fn route_mode_lifecycle_guards_report_text_errors() {
    let db = Arc::new(Database::memory().expect("create memory database"));
    let service = ProxyService::new(db.clone());

    let empty = service
        .enable_route_mode_for_app("claude")
        .await
        .expect_err("empty group must be refused");
    assert!(empty.contains("at least one enabled member"), "{empty}");

    let provider = anthropic_provider("p-a", "Alpha", "http://127.0.0.1:1");
    db.save_provider("claude", &provider)
        .expect("save provider");
    save_route_group(&db, vec![RouteModeMember::new("p-a", 1)]).await;

    let unsupported = service
        .enable_route_mode_for_app("opencode")
        .await
        .expect_err("unsupported app must be refused");
    assert!(unsupported.contains("not supported"), "{unsupported}");

    // auto_failover_enabled only persists when the proxy is enabled and a
    // queued provider exists (should_persist_auto_failover rule)
    db.add_to_failover_queue("claude", "p-a")
        .expect("queue provider");
    let mut config = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("read proxy config");
    config.enabled = true;
    config.auto_failover_enabled = true;
    db.update_proxy_config_for_app(config)
        .await
        .expect("enable auto failover");

    let conflict = service
        .enable_route_mode_for_app("claude")
        .await
        .expect_err("failover conflict must be refused");
    assert!(
        conflict.contains("conflicts with automatic failover"),
        "{conflict}"
    );

    let failover_refused = service
        .enable_auto_failover_for_app("claude")
        .await
        .expect_err("auto failover must be refused while route mode is active");
    assert!(
        failover_refused.contains("route mode is active"),
        "{failover_refused}"
    );

    let disable_refused = service
        .set_managed_session_for_app("claude", false)
        .await
        .expect_err("proxy disable must be refused while route mode is active");
    assert!(
        disable_refused.contains("route mode is active"),
        "{disable_refused}"
    );
}

#[tokio::test]
#[serial]
async fn startup_recovery_keeps_route_flag_and_takeover_intent() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let original_live = json!({
        "env": {"ANTHROPIC_API_KEY": "live-key"}
    });

    let seeded = AppState::try_new().expect("create seeded app state");
    let provider = anthropic_provider("p-a", "Alpha", "http://127.0.0.1:1");
    seeded
        .db
        .save_provider("claude", &provider)
        .expect("save provider");
    let group = RouteModeGroup {
        enabled: true,
        members: vec![RouteModeMember::new("p-a", 1)],
    };
    seeded
        .db
        .save_route_mode_group("claude", &group)
        .expect("save enabled route group");
    seeded
        .db
        .save_live_backup(
            "claude",
            &serde_json::to_string(&original_live).expect("serialize claude backup"),
        )
        .await
        .expect("save claude live backup");
    let mut app_proxy = seeded
        .db
        .get_proxy_config_for_app("claude")
        .await
        .expect("read claude proxy config");
    app_proxy.enabled = true;
    seeded
        .db
        .update_proxy_config_for_app(app_proxy)
        .await
        .expect("mark claude takeover enabled");
    drop(seeded);

    let recovered =
        AppState::try_new_with_startup_recovery().expect("recreate app state after crash");

    let restored: Value =
        read_json_file(&get_claude_settings_path()).expect("read recovered claude live config");
    assert_eq!(restored, original_live, "recovery restores the live backup");
    assert!(
        recovered
            .db
            .route_mode_enabled_for_app("claude")
            .expect("read route flag after recovery"),
        "startup recovery must keep the route mode flag"
    );
    let members = recovered
        .db
        .get_route_mode_group("claude")
        .expect("read route group after recovery");
    assert_eq!(members.members.len(), 1, "member config survives recovery");
}

#[tokio::test]
#[serial]
async fn hung_member_times_out_and_backup_completes_the_request() {
    let listener = bind_test_listener().await;
    let addr = listener.local_addr().expect("read hung upstream address");
    let hung_state = UpstreamState::default();
    let hung_router = Router::new()
        .route("/v1/messages", post(handle_anthropic_hang))
        .with_state(hung_state.clone());
    let hung_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, hung_router).await;
    });
    let hung = MockUpstream {
        state: hung_state.clone(),
        base_url: format!("http://{addr}"),
        handle: hung_handle,
    };

    let (upstream_b, _state_b) = spawn_ok_upstream().await;

    let db = Arc::new(Database::memory().expect("create memory database"));
    db.save_provider("claude", &anthropic_provider("p-a", "Hung", &hung.base_url))
        .expect("save hung provider");
    db.save_provider(
        "claude",
        &anthropic_provider("p-b", "Backup", &upstream_b.base_url),
    )
    .expect("save backup provider");
    save_route_group(
        &db,
        vec![
            RouteModeMember::new("p-a", 1),
            RouteModeMember::new("p-b", 1),
        ],
    )
    .await;

    let mut app_config = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("read app proxy config");
    app_config.non_streaming_timeout = 2;
    db.update_proxy_config_for_app(app_config)
        .await
        .expect("set non-streaming timeout");

    let (service, base) = start_proxy(db).await;

    let response = send_request(&base, "claude-sonnet-4-6").await;
    assert!(
        response.status().is_success(),
        "request must complete on the backup after the hung member times out"
    );

    assert_eq!(hung.state.hits(), 1, "hung member got one attempt");
    assert_eq!(upstream_b.state.hits(), 1, "backup served the request");

    service.stop().await.expect("stop proxy service");
    drop_upstreams(vec![hung, upstream_b]);
}

#[tokio::test]
#[serial]
async fn group_edits_take_effect_on_the_next_request() {
    let (upstream_a, _state_a) = spawn_ok_upstream().await;
    let (upstream_b, _state_b) = spawn_ok_upstream().await;

    let db = Arc::new(Database::memory().expect("create memory database"));
    db.save_provider(
        "claude",
        &anthropic_provider("p-a", "Alpha", &upstream_a.base_url),
    )
    .expect("save provider a");
    db.save_provider(
        "claude",
        &anthropic_provider("p-b", "Beta", &upstream_b.base_url),
    )
    .expect("save provider b");
    save_route_group(&db, vec![RouteModeMember::new("p-a", 1)]).await;

    let (service, base) = start_proxy(db.clone()).await;

    let first = send_request(&base, "claude-sonnet-4-6").await;
    assert!(first.status().is_success());
    assert_eq!(upstream_a.state.hits(), 1);
    assert_eq!(upstream_b.state.hits(), 0);

    // Live edit without touching the proxy: add a heavy member; the very
    // next request must see it.
    save_route_group(
        &db,
        vec![
            RouteModeMember::new("p-a", 1),
            RouteModeMember::new("p-b", 3),
        ],
    )
    .await;

    let second = send_request(&base, "claude-sonnet-4-6").await;
    assert!(second.status().is_success());
    assert_eq!(
        upstream_b.state.hits(),
        1,
        "newly added member must serve the next request"
    );

    service.stop().await.expect("stop proxy service");
    drop_upstreams(vec![upstream_a, upstream_b]);
}

#[tokio::test]
#[serial]
async fn failover_between_members_applies_each_members_own_mappings() {
    // Member a answers 500; member b answers ok. Both carry a different
    // exact mapping for the same request model, proving each retry attempt
    // derives the body from the original request with its own member's
    // table (no mapping chaining across attempts).
    let listener = bind_test_listener().await;
    let addr = listener
        .local_addr()
        .expect("read failing upstream address");
    let failing_state = UpstreamState::default();
    let failing_router = Router::new()
        .route("/v1/messages", post(handle_anthropic_500))
        .with_state(failing_state.clone());
    let failing_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, failing_router).await;
    });
    let failing = MockUpstream {
        state: failing_state.clone(),
        base_url: format!("http://{addr}"),
        handle: failing_handle,
    };
    let (upstream_b, _state_b) = spawn_ok_upstream().await;

    let db = Arc::new(Database::memory().expect("create memory database"));
    db.save_provider(
        "claude",
        &anthropic_provider("p-a", "Failing", &failing.base_url),
    )
    .expect("save failing provider");
    db.save_provider(
        "claude",
        &anthropic_provider("p-b", "Backup", &upstream_b.base_url),
    )
    .expect("save backup provider");

    let mut member_a = RouteModeMember::new("p-a", 1);
    member_a
        .model_mappings
        .insert("m-shared".to_string(), "m-from-a".to_string());
    let mut member_b = RouteModeMember::new("p-b", 1);
    member_b
        .model_mappings
        .insert("m-shared".to_string(), "m-from-b".to_string());
    save_route_group(&db, vec![member_a, member_b]).await;

    let (service, base) = start_proxy(db).await;

    let response = send_request(&base, "m-shared").await;
    assert!(response.status().is_success());

    assert_eq!(
        failing.state.last_model().as_deref(),
        Some("m-from-a"),
        "first attempt must use member a's mapping"
    );
    assert_eq!(
        upstream_b.state.last_model().as_deref(),
        Some("m-from-b"),
        "retry attempt must use member b's mapping, not chain from a"
    );

    service.stop().await.expect("stop proxy service");
    drop_upstreams(vec![failing, upstream_b]);
}

async fn handle_anthropic_stream_ok(
    State(state): State<UpstreamState>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    state.record(body.get("model").and_then(Value::as_str).map(String::from));

    (
        StatusCode::OK,
        [("content-type", "text/event-stream")],
        "event: message_start\ndata: {\"type\":\"message_start\"}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    )
        .into_response()
}

async fn handle_codex_responses_ok(
    State(state): State<UpstreamState>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    state.record(body.get("model").and_then(Value::as_str).map(String::from));

    (
        StatusCode::OK,
        Json(json!({
            "id": "resp_route_mode",
            "object": "response",
            "model": body.get("model").cloned().unwrap_or_else(|| json!("")),
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "codex ok"}]
            }]
        })),
    )
}

fn codex_provider(id: &str, name: &str, base_url: &str) -> Provider {
    Provider {
        id: id.to_string(),
        name: name.to_string(),
        settings_config: json!({
            "auth": {"OPENAI_API_KEY": "sk-test-codex"},
            "config": format!("base_url = \"http://{base_url}\"\nwire_api = \"responses\"\n")
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
    }
}

#[tokio::test]
#[serial]
async fn streaming_first_byte_timeout_fails_over_to_backup() {
    let listener = bind_test_listener().await;
    let addr = listener.local_addr().expect("read hung upstream address");
    let hung_state = UpstreamState::default();
    let hung_router = Router::new()
        .route("/v1/messages", post(handle_anthropic_hang))
        .with_state(hung_state.clone());
    let hung_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, hung_router).await;
    });
    let hung = MockUpstream {
        state: hung_state.clone(),
        base_url: format!("http://{addr}"),
        handle: hung_handle,
    };

    let stream_listener = bind_test_listener().await;
    let stream_addr = stream_listener
        .local_addr()
        .expect("read streaming upstream address");
    let stream_state = UpstreamState::default();
    let stream_router = Router::new()
        .route("/v1/messages", post(handle_anthropic_stream_ok))
        .with_state(stream_state.clone());
    let stream_handle = tokio::spawn(async move {
        let _ = axum::serve(stream_listener, stream_router).await;
    });
    let streaming = MockUpstream {
        state: stream_state.clone(),
        base_url: format!("http://{stream_addr}"),
        handle: stream_handle,
    };

    let db = Arc::new(Database::memory().expect("create memory database"));
    db.save_provider("claude", &anthropic_provider("p-a", "Hung", &hung.base_url))
        .expect("save hung provider");
    db.save_provider(
        "claude",
        &anthropic_provider("p-b", "Streaming", &streaming.base_url),
    )
    .expect("save streaming provider");
    save_route_group(
        &db,
        vec![
            RouteModeMember::new("p-a", 1),
            RouteModeMember::new("p-b", 1),
        ],
    )
    .await;

    let mut app_config = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("read app proxy config");
    app_config.streaming_first_byte_timeout = 2;
    db.update_proxy_config_for_app(app_config)
        .await
        .expect("set streaming first-byte timeout");

    let (service, base) = start_proxy(db).await;

    let mut request_body = anthropic_request("claude-sonnet-4-6");
    request_body["stream"] = json!(true);
    let response = reqwest::Client::new()
        .post(format!("{base}/v1/messages"))
        .header("anthropic-version", "2023-06-01")
        .json(&request_body)
        .send()
        .await
        .expect("send streaming request to proxy");

    assert!(
        response.status().is_success(),
        "streaming request must complete on the backup"
    );
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = response.text().await.expect("read streaming body");
    assert!(
        content_type.contains("text/event-stream") && body.contains("data:"),
        "backup must stream SSE back (content-type {content_type}, body {body})"
    );
    assert_eq!(hung.state.hits(), 1, "hung member got one attempt");
    assert_eq!(streaming.state.hits(), 1, "backup served the stream");

    service.stop().await.expect("stop proxy service");
    drop_upstreams(vec![hung, streaming]);
}

#[tokio::test]
#[serial]
async fn codex_route_mode_applies_member_mapping() {
    let (upstream_a, _state_a) = spawn_codex_upstream().await;
    let (upstream_b, _state_b) = spawn_codex_upstream().await;

    let db = Arc::new(Database::memory().expect("create memory database"));
    db.save_provider(
        "codex",
        &codex_provider(
            "c-a",
            "Codex A",
            upstream_a.base_url.trim_start_matches("http://"),
        ),
    )
    .expect("save codex provider a");
    db.save_provider(
        "codex",
        &codex_provider(
            "c-b",
            "Codex B",
            upstream_b.base_url.trim_start_matches("http://"),
        ),
    )
    .expect("save codex provider b");

    let mut member_a = RouteModeMember::new("c-a", 1);
    member_a
        .model_mappings
        .insert("gpt-5-codex".to_string(), "gpt-mapped-a".to_string());
    let group = RouteModeGroup {
        enabled: false,
        members: vec![member_a, RouteModeMember::new("c-b", 1)],
    };
    db.save_route_mode_group("codex", &group)
        .expect("save codex route group");
    db.set_route_mode_enabled("codex", true)
        .expect("enable codex route mode");

    let (service, base) = start_proxy_for_app(db, "codex").await;

    // Equal weights alternate: request 1 hits the mapped member, request 2
    // the unmapped one.
    for _ in 0..2 {
        let response = reqwest::Client::new()
            .post(format!("{base}/v1/responses"))
            .json(&json!({"model": "gpt-5-codex", "input": "hello codex"}))
            .send()
            .await
            .expect("send codex request to proxy");
        let status = response.status();
        let text = response.text().await.expect("read codex response body");
        assert!(status.is_success(), "codex request failed: {status} {text}");
    }

    assert_eq!(
        upstream_a.state.last_model().as_deref(),
        Some("gpt-mapped-a"),
        "codex body.model must be rewritten by the member mapping"
    );
    assert_eq!(
        upstream_b.state.last_model().as_deref(),
        Some("gpt-5-codex"),
        "unmapped codex member must receive the request model verbatim"
    );

    service.stop().await.expect("stop proxy service");
    drop_upstreams(vec![upstream_a, upstream_b]);
}
