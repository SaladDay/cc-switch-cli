use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::any,
    Json, Router,
};
use serde_json::{json, Value};
use tokio::{sync::Mutex, task::JoinHandle};

use crate::{database::Database, provider::Provider, proxy::provider_router::ProviderRouter};

mod error_paths;
mod provider_failover;
mod request_building;

#[derive(Clone, Default)]
struct UpstreamHits {
    count: Arc<AtomicUsize>,
    paths: Arc<Mutex<Vec<String>>>,
    headers: Arc<Mutex<Vec<HeaderMap>>>,
}

#[derive(Clone)]
struct MockUpstream {
    status: StatusCode,
    body: Value,
    hits: UpstreamHits,
}

#[derive(Clone)]
struct ScriptedUpstream {
    responses: Arc<Mutex<VecDeque<(StatusCode, Value)>>>,
    hits: UpstreamHits,
    bodies: Arc<Mutex<Vec<Value>>>,
}

#[derive(Clone)]
struct DelayedScriptedUpstream {
    responses: Arc<Mutex<VecDeque<(Duration, StatusCode, Value)>>>,
    hits: UpstreamHits,
    bodies: Arc<Mutex<Vec<Value>>>,
}

#[derive(Clone)]
enum ScriptedStreamingBody {
    Json(Value),
    Sse(&'static str),
}

#[derive(Clone)]
struct ScriptedStreamingUpstream {
    responses: Arc<Mutex<VecDeque<(StatusCode, ScriptedStreamingBody)>>>,
    hits: UpstreamHits,
    bodies: Arc<Mutex<Vec<Value>>>,
}

#[derive(Clone)]
struct DelayedScriptedStreamingUpstream {
    responses: Arc<Mutex<VecDeque<(Duration, StatusCode, ScriptedStreamingBody)>>>,
    hits: UpstreamHits,
    bodies: Arc<Mutex<Vec<Value>>>,
}

async fn handle_mock_upstream(State(mock): State<MockUpstream>, uri: Uri) -> impl IntoResponse {
    mock.hits.count.fetch_add(1, Ordering::SeqCst);
    mock.hits.paths.lock().await.push(uri.path().to_string());
    (mock.status, Json(mock.body))
}

async fn spawn_mock_upstream(
    status: StatusCode,
    body: Value,
) -> (String, UpstreamHits, JoinHandle<()>) {
    let hits = UpstreamHits::default();
    let mock = MockUpstream {
        status,
        body,
        hits: hits.clone(),
    };
    let app = Router::new()
        .route("/*path", any(handle_mock_upstream))
        .with_state(mock);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream listener");
    let address = listener.local_addr().expect("upstream listener address");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    (format!("http://{address}"), hits, handle)
}

async fn handle_scripted_upstream(
    State(mock): State<ScriptedUpstream>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    mock.hits.count.fetch_add(1, Ordering::SeqCst);
    mock.hits.paths.lock().await.push(uri.path().to_string());
    mock.hits.headers.lock().await.push(headers);
    mock.bodies.lock().await.push(body);

    let (status, body) = mock.responses.lock().await.pop_front().unwrap_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({"error": "missing scripted response"}),
    ));
    (status, Json(body))
}

async fn spawn_scripted_upstream(
    responses: Vec<(StatusCode, Value)>,
) -> (String, UpstreamHits, Arc<Mutex<Vec<Value>>>, JoinHandle<()>) {
    let hits = UpstreamHits::default();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mock = ScriptedUpstream {
        responses: Arc::new(Mutex::new(VecDeque::from(responses))),
        hits: hits.clone(),
        bodies: bodies.clone(),
    };
    let app = Router::new()
        .route("/*path", any(handle_scripted_upstream))
        .with_state(mock);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind scripted upstream listener");
    let address = listener
        .local_addr()
        .expect("scripted upstream listener address");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    (format!("http://{address}"), hits, bodies, handle)
}

async fn handle_delayed_scripted_upstream(
    State(mock): State<DelayedScriptedUpstream>,
    uri: Uri,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    mock.hits.count.fetch_add(1, Ordering::SeqCst);
    mock.hits.paths.lock().await.push(uri.path().to_string());
    mock.bodies.lock().await.push(body);

    let (delay, status, body) = mock.responses.lock().await.pop_front().unwrap_or((
        Duration::from_millis(0),
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({"error": "missing scripted response"}),
    ));
    tokio::time::sleep(delay).await;
    (status, Json(body))
}

async fn spawn_delayed_scripted_upstream(
    responses: Vec<(Duration, StatusCode, Value)>,
) -> (String, UpstreamHits, Arc<Mutex<Vec<Value>>>, JoinHandle<()>) {
    let hits = UpstreamHits::default();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mock = DelayedScriptedUpstream {
        responses: Arc::new(Mutex::new(VecDeque::from(responses))),
        hits: hits.clone(),
        bodies: bodies.clone(),
    };
    let app = Router::new()
        .route("/*path", any(handle_delayed_scripted_upstream))
        .with_state(mock);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind delayed scripted upstream listener");
    let address = listener
        .local_addr()
        .expect("delayed scripted upstream listener address");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    (format!("http://{address}"), hits, bodies, handle)
}

async fn handle_scripted_streaming_upstream(
    State(mock): State<ScriptedStreamingUpstream>,
    uri: Uri,
    Json(body): Json<Value>,
) -> Response {
    mock.hits.count.fetch_add(1, Ordering::SeqCst);
    mock.hits.paths.lock().await.push(uri.path().to_string());
    mock.bodies.lock().await.push(body);

    let (status, body) = mock.responses.lock().await.pop_front().unwrap_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        ScriptedStreamingBody::Json(json!({"error": {"message": "missing scripted response"}})),
    ));

    match body {
        ScriptedStreamingBody::Json(body) => (status, Json(body)).into_response(),
        ScriptedStreamingBody::Sse(body) => Response::builder()
            .status(status)
            .header("content-type", "text/event-stream")
            .body(Body::from(body))
            .expect("build scripted streaming response"),
    }
}

async fn spawn_scripted_streaming_upstream(
    responses: Vec<(StatusCode, ScriptedStreamingBody)>,
) -> (String, UpstreamHits, Arc<Mutex<Vec<Value>>>, JoinHandle<()>) {
    let hits = UpstreamHits::default();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mock = ScriptedStreamingUpstream {
        responses: Arc::new(Mutex::new(VecDeque::from(responses))),
        hits: hits.clone(),
        bodies: bodies.clone(),
    };
    let app = Router::new()
        .route("/*path", any(handle_scripted_streaming_upstream))
        .with_state(mock);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind scripted streaming upstream listener");
    let address = listener
        .local_addr()
        .expect("scripted streaming upstream listener address");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    (format!("http://{address}"), hits, bodies, handle)
}

async fn handle_delayed_scripted_streaming_upstream(
    State(mock): State<DelayedScriptedStreamingUpstream>,
    uri: Uri,
    Json(body): Json<Value>,
) -> Response {
    mock.hits.count.fetch_add(1, Ordering::SeqCst);
    mock.hits.paths.lock().await.push(uri.path().to_string());
    mock.bodies.lock().await.push(body);

    let (delay, status, body) = mock.responses.lock().await.pop_front().unwrap_or((
        Duration::from_millis(0),
        StatusCode::INTERNAL_SERVER_ERROR,
        ScriptedStreamingBody::Json(json!({"error": {"message": "missing scripted response"}})),
    ));
    tokio::time::sleep(delay).await;

    match body {
        ScriptedStreamingBody::Json(body) => (status, Json(body)).into_response(),
        ScriptedStreamingBody::Sse(body) => Response::builder()
            .status(status)
            .header("content-type", "text/event-stream")
            .body(Body::from(body))
            .expect("build delayed scripted streaming response"),
    }
}

async fn spawn_delayed_scripted_streaming_upstream(
    responses: Vec<(Duration, StatusCode, ScriptedStreamingBody)>,
) -> (String, UpstreamHits, Arc<Mutex<Vec<Value>>>, JoinHandle<()>) {
    let hits = UpstreamHits::default();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mock = DelayedScriptedStreamingUpstream {
        responses: Arc::new(Mutex::new(VecDeque::from(responses))),
        hits: hits.clone(),
        bodies: bodies.clone(),
    };
    let app = Router::new()
        .route("/*path", any(handle_delayed_scripted_streaming_upstream))
        .with_state(mock);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind delayed scripted streaming upstream listener");
    let address = listener
        .local_addr()
        .expect("delayed scripted streaming upstream listener address");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    (format!("http://{address}"), hits, bodies, handle)
}

async fn handle_delayed_body_upstream(State(hits): State<UpstreamHits>, uri: Uri) -> Response {
    hits.count.fetch_add(1, Ordering::SeqCst);
    hits.paths.lock().await.push(uri.path().to_string());

    let stream = async_stream::stream! {
        tokio::time::sleep(Duration::from_millis(150)).await;
        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(br#"{"ok":true}"#));
    };

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::from_stream(stream))
        .expect("build delayed body response")
}

async fn spawn_delayed_body_upstream() -> (String, UpstreamHits, JoinHandle<()>) {
    let hits = UpstreamHits::default();
    let app = Router::new()
        .route("/*path", any(handle_delayed_body_upstream))
        .with_state(hits.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream listener");
    let address = listener.local_addr().expect("upstream listener address");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    (format!("http://{address}"), hits, handle)
}

async fn handle_failing_body_upstream(State(hits): State<UpstreamHits>, uri: Uri) -> Response {
    hits.count.fetch_add(1, Ordering::SeqCst);
    hits.paths.lock().await.push(uri.path().to_string());

    let stream = futures::stream::once(async {
        Err::<bytes::Bytes, std::io::Error>(std::io::Error::other("first chunk failed"))
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .expect("build failing body response")
}

async fn spawn_failing_body_upstream() -> (String, UpstreamHits, JoinHandle<()>) {
    let hits = UpstreamHits::default();
    let app = Router::new()
        .route("/*path", any(handle_failing_body_upstream))
        .with_state(hits.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind failing body listener");
    let address = listener
        .local_addr()
        .expect("failing body listener address");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    (format!("http://{address}"), hits, handle)
}

async fn closed_base_url() -> String {
    "http://127.0.0.1:9".to_string()
}

fn claude_provider(id: &str, base_url: &str, api_format: Option<&str>) -> Provider {
    let mut settings = json!({
        "base_url": base_url,
        "apiKey": format!("key-{id}"),
    });
    if let Some(api_format) = api_format {
        settings["api_format"] = json!(api_format);
    }

    Provider::with_id(id.to_string(), format!("Provider {id}"), settings, None)
}

fn codex_provider(id: &str, base_url: &str, official: bool) -> Provider {
    let mut provider = Provider::with_id(
        id.to_string(),
        format!("Provider {id}"),
        json!({
            "base_url": base_url,
            "apiKey": format!("key-{id}"),
        }),
        None,
    );
    if official {
        provider.category = Some("official".to_string());
    }
    provider
}

fn bedrock_claude_provider(id: &str, base_url: &str) -> Provider {
    let settings = json!({
        "env": {
            "ANTHROPIC_BASE_URL": base_url,
            "ANTHROPIC_API_KEY": format!("key-{id}"),
            "CLAUDE_CODE_USE_BEDROCK": "1"
        }
    });

    Provider::with_id(id.to_string(), format!("Bedrock {id}"), settings, None)
}

fn claude_request_body() -> Value {
    json!({
        "model": "claude-3-7-sonnet-20250219",
        "max_tokens": 32,
        "messages": [{
            "role": "user",
            "content": [{
                "type": "text",
                "text": "hello"
            }]
        }]
    })
}

async fn test_router() -> (Arc<Database>, Arc<ProviderRouter>) {
    let db = Arc::new(Database::memory().expect("memory db"));
    let mut config = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("load proxy config");
    config.circuit_failure_threshold = 1;
    config.circuit_timeout_seconds = 0;
    db.update_proxy_config_for_app(config)
        .await
        .expect("update proxy config");
    let router = Arc::new(ProviderRouter::new(db.clone()));
    (db, router)
}
