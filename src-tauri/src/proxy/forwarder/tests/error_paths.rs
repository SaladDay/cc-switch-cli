use std::{sync::atomic::Ordering, time::Duration};

use axum::http::{HeaderMap, StatusCode};
use futures::StreamExt;
use serde_json::{json, Value};

use super::{
    claude_provider, claude_request_body, closed_base_url, spawn_delayed_body_upstream,
    spawn_delayed_scripted_streaming_upstream, spawn_delayed_scripted_upstream,
    spawn_failing_body_upstream, spawn_mock_upstream, spawn_scripted_streaming_upstream,
    test_router, ScriptedStreamingBody,
};
use crate::{
    app_config::AppType,
    proxy::{
        error::ProxyError,
        forwarder::{ForwardOptions, RequestForwarder, StreamingResponse},
        response::is_sse_response,
        types::RectifierConfig,
    },
};

#[tokio::test]
async fn single_provider_buffered_claude_non_2xx_returns_upstream_error() {
    let (primary_url, primary_hits, primary_server) = spawn_mock_upstream(
        StatusCode::TOO_MANY_REQUESTS,
        json!({"error": {"message": "rate limited"}}),
    )
    .await;
    let provider = claude_provider("p1", &primary_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router.clone()).expect("create forwarder");

    db.save_provider("claude", &provider)
        .expect("save provider for health tracking");

    let error = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            claude_request_body(),
            &HeaderMap::new(),
            vec![provider],
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect_err("single-provider Claude 429 should surface as UpstreamError");

    match error {
        ProxyError::UpstreamError { status, body } => {
            assert_eq!(status, 429);
            assert_eq!(
                body.as_deref(),
                Some(r#"{"error":{"message":"rate limited"}}"#)
            );
        }
        other => panic!("expected UpstreamError, got {other:?}"),
    }
    assert_eq!(primary_hits.count.load(Ordering::SeqCst), 1);

    primary_server.abort();
}

#[tokio::test]
async fn last_provider_429_returns_upstream_error() {
    let (primary_url, _primary_hits, primary_server) = spawn_mock_upstream(
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({"error": {"message": "primary down"}}),
    )
    .await;
    let (secondary_url, _secondary_hits, secondary_server) = spawn_mock_upstream(
        StatusCode::TOO_MANY_REQUESTS,
        json!({"error": {"message": "rate limited"}}),
    )
    .await;
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");
    let provider_one = claude_provider("p1", &primary_url, None);
    let provider_two = claude_provider("p2", &secondary_url, None);

    db.save_provider("claude", &provider_one)
        .expect("save primary provider for health tracking");
    db.save_provider("claude", &provider_two)
        .expect("save secondary provider for health tracking");

    let error = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            claude_request_body(),
            &HeaderMap::new(),
            vec![provider_one, provider_two],
            ForwardOptions {
                max_retries: 1,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect_err("last provider 429 should surface as UpstreamError");

    match error {
        ProxyError::UpstreamError { status, body } => {
            assert_eq!(status, 429);
            let parsed: Value =
                serde_json::from_str(body.as_deref().expect("preserve upstream body"))
                    .expect("parse body");
            assert_eq!(parsed, json!({"error": {"message": "rate limited"}}));
        }
        other => panic!("expected UpstreamError, got {other:?}"),
    }

    primary_server.abort();
    secondary_server.abort();
}

#[tokio::test]
async fn last_streaming_provider_429_returns_upstream_error() {
    let (primary_url, _primary_hits, _primary_bodies, primary_server) =
        spawn_scripted_streaming_upstream(vec![(
            StatusCode::INTERNAL_SERVER_ERROR,
            ScriptedStreamingBody::Json(json!({"error": {"message": "primary down"}})),
        )])
        .await;
    let (secondary_url, _secondary_hits, _secondary_bodies, secondary_server) =
        spawn_scripted_streaming_upstream(vec![(
            StatusCode::TOO_MANY_REQUESTS,
            ScriptedStreamingBody::Json(json!({"error": {"message": "rate limited"}})),
        )])
        .await;
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");
    let provider_one = claude_provider("p1", &primary_url, None);
    let provider_two = claude_provider("p2", &secondary_url, None);

    db.save_provider("claude", &provider_one)
        .expect("save primary provider for health tracking");
    db.save_provider("claude", &provider_two)
        .expect("save secondary provider for health tracking");

    let error = forwarder
        .forward_response(
            &AppType::Claude,
            "/v1/messages",
            json!({
                "model": "claude-3-7-sonnet-20250219",
                "stream": true,
                "max_tokens": 32,
                "messages": [{"role": "user", "content": [{"type": "text", "text": "hello"}]}]
            }),
            &HeaderMap::new(),
            vec![provider_one, provider_two],
            ForwardOptions {
                max_retries: 1,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect_err("last streaming provider 429 should surface as UpstreamError");

    match error {
        ProxyError::UpstreamError { status, body } => {
            assert_eq!(status, 429);
            let parsed: Value =
                serde_json::from_str(body.as_deref().expect("preserve upstream body"))
                    .expect("parse body");
            assert_eq!(parsed, json!({"error": {"message": "rate limited"}}));
        }
        other => panic!("expected UpstreamError, got {other:?}"),
    }

    primary_server.abort();
    secondary_server.abort();
}

#[tokio::test]
async fn buffered_timeout_includes_body_read_budget_after_headers() {
    let (base_url, hits, server) = spawn_delayed_body_upstream().await;
    let provider = claude_provider("p1", &base_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    db.save_provider("claude", &provider)
        .expect("save provider for health tracking");

    let error = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            claude_request_body(),
            &HeaderMap::new(),
            vec![provider],
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_millis(50)),
                bypass_circuit_breaker: true,
            },
            RectifierConfig::default(),
        )
        .await
        .expect_err("buffered request should time out while waiting for body");

    assert!(matches!(error, ProxyError::Timeout(message) if message.contains("request timed out")));
    assert_eq!(hits.count.load(Ordering::SeqCst), 1);

    server.abort();
}

#[tokio::test]
async fn buffered_body_timeout_after_headers_fails_over() {
    let (slow_url, slow_hits, slow_server) = spawn_delayed_body_upstream().await;
    let (fallback_url, fallback_hits, fallback_server) =
        spawn_mock_upstream(StatusCode::OK, json!({"ok": true})).await;
    let slow_provider = claude_provider("p1", &slow_url, None);
    let fallback_provider = claude_provider("p2", &fallback_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    db.save_provider("claude", &slow_provider)
        .expect("save slow provider for health tracking");
    db.save_provider("claude", &fallback_provider)
        .expect("save fallback provider for health tracking");

    let result = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            claude_request_body(),
            &HeaderMap::new(),
            vec![slow_provider, fallback_provider],
            ForwardOptions {
                max_retries: 1,
                request_timeout: Some(Duration::from_millis(50)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect("body timeout before commit should fail over");

    assert_eq!(result.provider.id, "p2");
    assert_eq!(result.response.status, StatusCode::OK);
    assert_eq!(slow_hits.count.load(Ordering::SeqCst), 1);
    assert_eq!(fallback_hits.count.load(Ordering::SeqCst), 1);

    slow_server.abort();
    fallback_server.abort();
}

#[tokio::test]
async fn streaming_first_chunk_timeout_after_headers_fails_over_and_replays_fallback() {
    let (slow_url, slow_hits, slow_server) = spawn_delayed_body_upstream().await;
    let fallback_sse =
        "data: {\"id\":\"msg_fallback\",\"type\":\"message_start\"}\n\ndata: [DONE]\n\n";
    let (fallback_url, fallback_hits, _bodies, fallback_server) =
        spawn_scripted_streaming_upstream(vec![(
            StatusCode::OK,
            ScriptedStreamingBody::Sse(fallback_sse),
        )])
        .await;
    let slow_provider = claude_provider("p1", &slow_url, None);
    let fallback_provider = claude_provider("p2", &fallback_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    db.save_provider("claude", &slow_provider)
        .expect("save slow provider");
    db.save_provider("claude", &fallback_provider)
        .expect("save fallback provider");

    let result = forwarder
        .forward_response(
            &AppType::Claude,
            "/v1/messages",
            json!({
                "model": "claude-3-7-sonnet-20250219",
                "stream": true,
                "max_tokens": 32,
                "messages": [{"role": "user", "content": "hello"}]
            }),
            &HeaderMap::new(),
            vec![slow_provider, fallback_provider],
            ForwardOptions {
                max_retries: 1,
                request_timeout: Some(Duration::from_millis(50)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect("first chunk timeout should fail over");

    assert_eq!(result.provider.id, "p2");
    let StreamingResponse::Live(response) = result.response else {
        panic!("fallback response should remain streaming");
    };
    let chunks = response
        .bytes_stream()
        .map(|chunk| chunk.expect("read replayed fallback chunk"))
        .collect::<Vec<_>>()
        .await;
    let replayed = chunks.concat();
    assert_eq!(replayed.as_slice(), fallback_sse.as_bytes());
    assert_eq!(slow_hits.count.load(Ordering::SeqCst), 1);
    assert_eq!(fallback_hits.count.load(Ordering::SeqCst), 1);

    slow_server.abort();
    fallback_server.abort();
}

#[tokio::test]
async fn streaming_first_chunk_error_fails_over() {
    let (failing_url, failing_hits, failing_server) = spawn_failing_body_upstream().await;
    let (fallback_url, fallback_hits, _bodies, fallback_server) =
        spawn_scripted_streaming_upstream(vec![(
            StatusCode::OK,
            ScriptedStreamingBody::Sse(
                "data: {\"id\":\"msg_ok\",\"type\":\"message_start\"}\n\ndata: [DONE]\n\n",
            ),
        )])
        .await;
    let failing_provider = claude_provider("p1", &failing_url, None);
    let fallback_provider = claude_provider("p2", &fallback_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    db.save_provider("claude", &failing_provider)
        .expect("save failing provider");
    db.save_provider("claude", &fallback_provider)
        .expect("save fallback provider");

    let result = forwarder
        .forward_response(
            &AppType::Claude,
            "/v1/messages",
            json!({
                "model": "claude-3-7-sonnet-20250219",
                "stream": true,
                "max_tokens": 32,
                "messages": [{"role": "user", "content": "hello"}]
            }),
            &HeaderMap::new(),
            vec![failing_provider, fallback_provider],
            ForwardOptions {
                max_retries: 1,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect("first chunk error should fail over");

    assert_eq!(result.provider.id, "p2");
    assert_eq!(failing_hits.count.load(Ordering::SeqCst), 1);
    assert_eq!(fallback_hits.count.load(Ordering::SeqCst), 1);

    failing_server.abort();
    fallback_server.abort();
}

#[tokio::test]
async fn max_retries_does_not_retry_the_same_provider() {
    let (base_url, hits, _bodies, server) = spawn_delayed_scripted_upstream(vec![
        (
            Duration::from_millis(100),
            StatusCode::OK,
            json!({"id": "first-attempt"}),
        ),
        (
            Duration::from_millis(0),
            StatusCode::OK,
            json!({"id": "second-attempt"}),
        ),
    ])
    .await;
    let provider = claude_provider("p1", &base_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    db.save_provider("claude", &provider)
        .expect("save provider for health tracking");

    let error = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            claude_request_body(),
            &HeaderMap::new(),
            vec![provider],
            ForwardOptions {
                max_retries: 1,
                request_timeout: Some(Duration::from_millis(50)),
                bypass_circuit_breaker: true,
            },
            RectifierConfig::default(),
        )
        .await
        .expect_err("max_retries should only permit another provider attempt");

    assert!(matches!(error, ProxyError::Timeout(message) if message.contains("request timed out")));
    assert_eq!(hits.count.load(Ordering::SeqCst), 1);

    server.abort();
}

#[tokio::test]
async fn buffered_connect_error_maps_to_forward_failed() {
    let provider = claude_provider("p1", &closed_base_url().await, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    db.save_provider("claude", &provider)
        .expect("save provider for health tracking");

    let error = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            claude_request_body(),
            &HeaderMap::new(),
            vec![provider],
            ForwardOptions {
                max_retries: 2,
                request_timeout: Some(Duration::from_secs(1)),
                bypass_circuit_breaker: true,
            },
            RectifierConfig::default(),
        )
        .await
        .expect_err("connect failures should map to forward failed");

    assert!(matches!(error, ProxyError::ForwardFailed(_)));
}

#[tokio::test]
async fn buffered_rectifier_retry_shares_request_timeout_budget() {
    let (base_url, hits, bodies, server) = spawn_delayed_scripted_upstream(vec![
        (
            Duration::from_millis(20),
            StatusCode::BAD_REQUEST,
            json!({"error": {"message": "messages.1.content.0: Invalid `signature` in `thinking` block"}}),
        ),
        (
            Duration::from_millis(40),
            StatusCode::OK,
            json!({"id": "msg_123", "content": []}),
        ),
    ])
    .await;
    let provider = claude_provider("p1", &base_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    db.save_provider("claude", &provider)
        .expect("save provider for health tracking");

    let body = json!({
        "model": "claude-3-7-sonnet-20250219",
        "max_tokens": 32,
        "messages": [{
            "role": "assistant",
            "content": [
                { "type": "thinking", "thinking": "t", "signature": "sig" },
                { "type": "text", "text": "hello", "signature": "text-sig" }
            ]
        }]
    });

    let error = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            body,
            &HeaderMap::new(),
            vec![provider],
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_millis(50)),
                bypass_circuit_breaker: true,
            },
            RectifierConfig::default(),
        )
        .await
        .expect_err("rectifier retry should share a single buffered request timeout budget");

    assert!(matches!(error, ProxyError::Timeout(message) if message.contains("request timed out")));
    assert_eq!(hits.count.load(Ordering::SeqCst), 2);
    assert_eq!(bodies.lock().await.len(), 2);

    server.abort();
}

#[tokio::test]
async fn streaming_transport_timeout_fails_over_without_same_provider_retry() {
    let (primary_url, primary_hits, primary_bodies, primary_server) =
        spawn_delayed_scripted_streaming_upstream(vec![
            (
                Duration::from_millis(100),
                StatusCode::OK,
                ScriptedStreamingBody::Sse(
                    "data: {\"id\":\"primary-retry\",\"type\":\"message_start\"}\n\ndata: [DONE]\n\n",
                ),
            ),
            (
                Duration::from_millis(0),
                StatusCode::OK,
                ScriptedStreamingBody::Sse(
                    "data: {\"id\":\"primary-second\",\"type\":\"message_start\"}\n\ndata: [DONE]\n\n",
                ),
            ),
        ])
        .await;
    let (secondary_url, secondary_hits, secondary_bodies, secondary_server) =
        spawn_delayed_scripted_streaming_upstream(vec![(
            Duration::from_millis(0),
            StatusCode::OK,
            ScriptedStreamingBody::Sse(
                "data: {\"id\":\"secondary\",\"type\":\"message_start\"}\n\ndata: [DONE]\n\n",
            ),
        )])
        .await;
    let provider_one = claude_provider("p1", &primary_url, None);
    let provider_two = claude_provider("p2", &secondary_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    db.save_provider("claude", &provider_one)
        .expect("save primary provider for health tracking");
    db.save_provider("claude", &provider_two)
        .expect("save secondary provider for health tracking");

    let result = forwarder
        .forward_response(
            &AppType::Claude,
            "/v1/messages",
            json!({
                "model": "claude-3-7-sonnet-20250219",
                "stream": true,
                "max_tokens": 32,
                "messages": [{
                    "role": "user",
                    "content": [{ "type": "text", "text": "hello" }]
                }]
            }),
            &HeaderMap::new(),
            vec![provider_one, provider_two],
            ForwardOptions {
                max_retries: 1,
                request_timeout: Some(Duration::from_millis(50)),
                bypass_circuit_breaker: true,
            },
            RectifierConfig::default(),
        )
        .await
        .expect("transport timeout should fail over to next provider");

    assert_eq!(result.provider.id, "p2");
    assert_eq!(result.response.status(), StatusCode::OK);
    assert_eq!(primary_hits.count.load(Ordering::SeqCst), 1);
    assert_eq!(secondary_hits.count.load(Ordering::SeqCst), 1);
    assert_eq!(primary_bodies.lock().await.len(), 1);
    assert_eq!(secondary_bodies.lock().await.len(), 1);

    primary_server.abort();
    secondary_server.abort();
}

#[tokio::test]
async fn claude_streaming_success_path_does_not_trigger_rectifier_retry() {
    let (base_url, hits, bodies, server) = spawn_scripted_streaming_upstream(vec![(
        StatusCode::OK,
        ScriptedStreamingBody::Sse(
            "data: {\"id\":\"msg_123\",\"type\":\"message_start\"}\n\ndata: [DONE]\n\n",
        ),
    )])
    .await;
    let provider = claude_provider("p1", &base_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    db.save_provider("claude", &provider)
        .expect("save provider for health tracking");

    let body = json!({
        "model": "claude-3-7-sonnet-20250219",
        "stream": true,
        "max_tokens": 32,
        "messages": [{
            "role": "assistant",
            "content": [
                { "type": "thinking", "thinking": "t", "signature": "sig" },
                { "type": "text", "text": "hello", "signature": "text-sig" }
            ]
        }]
    });

    let result = forwarder
        .forward_response(
            &AppType::Claude,
            "/v1/messages",
            body,
            &HeaderMap::new(),
            vec![provider],
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
            RectifierConfig::default(),
        )
        .await
        .expect("streaming success path should not use rectifier retry");

    assert_eq!(result.response.status(), StatusCode::OK);
    assert!(matches!(
        &result.response,
        StreamingResponse::Live(response) if is_sse_response(response)
    ));
    assert_eq!(hits.count.load(Ordering::SeqCst), 1);

    let sent_bodies = bodies.lock().await;
    assert_eq!(sent_bodies.len(), 1);
    assert_eq!(
        sent_bodies[0]["messages"][0]["content"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    server.abort();
}
