use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use cc_switch_lib::{Database, ProxyStatus};
use serde_json::Value;
use std::sync::Arc;
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

pub(crate) async fn read_proxy_status(
    client: &reqwest::Client,
    address: &str,
    port: u16,
) -> ProxyStatus {
    client
        .get(format!("http://{}:{}/status", address, port))
        .send()
        .await
        .expect("read proxy status response")
        .json()
        .await
        .expect("parse proxy status")
}

pub(crate) async fn wait_for_proxy_status<F>(
    client: &reqwest::Client,
    address: &str,
    port: u16,
    predicate: F,
) -> ProxyStatus
where
    F: Fn(&ProxyStatus) -> bool,
{
    for _ in 0..20 {
        let status = read_proxy_status(client, address, port).await;
        if predicate(&status) {
            return status;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    read_proxy_status(client, address, port).await
}

#[derive(Clone, Default)]
pub(crate) struct UpstreamState {
    pub(crate) request_body: Arc<Mutex<Option<Value>>>,
    pub(crate) authorization: Arc<Mutex<Option<String>>>,
    pub(crate) api_key: Arc<Mutex<Option<String>>>,
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
}

pub(crate) async fn handle_streaming_chat(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    record_upstream_request(&state, &headers, body).await;

    let stream = async_stream::stream! {
        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(
            b"data: {\"id\":\"chatcmpl-stream\",\"model\":\"gpt-4o-mini\",\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
        ));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(
            b"data: {\"id\":\"chatcmpl-stream\",\"model\":\"gpt-4o-mini\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":7}}\n\n",
        ));
        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(
            b"data: [DONE]\n\n",
        ));
    };

    (
        StatusCode::OK,
        [("content-type", "text/event-stream")],
        Body::from_stream(stream),
    )
}

pub(crate) async fn handle_slow_streaming_chat(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    record_upstream_request(&state, &headers, body).await;

    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    let sse = concat!(
        "data: {\"id\":\"chatcmpl-stream\",\"model\":\"gpt-4o-mini\",\"choices\":[{\"delta\":{\"content\":\"late\"}}]}\n\n",
        "data: [DONE]\n\n"
    );

    (StatusCode::OK, [("content-type", "text/event-stream")], sse)
}
