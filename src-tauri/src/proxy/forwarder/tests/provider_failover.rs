use std::{sync::atomic::Ordering, time::Duration};

use axum::http::{HeaderMap, StatusCode};
use futures::StreamExt;
use serde_json::{json, Value};

use super::{
    claude_provider, claude_request_body, codex_provider,
    spawn_delayed_scripted_streaming_upstream, spawn_mock_upstream,
    spawn_scripted_streaming_upstream, spawn_scripted_upstream, test_router, ScriptedStreamingBody,
};
use crate::{
    app_config::AppType,
    provider::{LocalProxyRequestOverrides, ProviderMeta},
    proxy::{
        error::ProxyError,
        forwarder::{ForwardOptions, RequestForwarder},
        types::RectifierConfig,
    },
};

#[tokio::test]
async fn single_provider_bypasses_open_breaker() {
    let (base_url, hits, server) = spawn_mock_upstream(StatusCode::OK, json!({"ok": true})).await;
    let provider = claude_provider("p1", &base_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router.clone()).expect("create forwarder");

    db.save_provider("claude", &provider)
        .expect("save provider for health tracking");
    let mut config = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("load proxy config");
    config.circuit_timeout_seconds = 3600;
    db.update_proxy_config_for_app(config)
        .await
        .expect("update proxy timeout");

    router
        .record_result(
            "p1",
            "claude",
            false,
            false,
            Some("open breaker".to_string()),
        )
        .await
        .expect("open breaker");
    assert!(!router.allow_provider_request("p1", "claude").await.allowed);

    let result = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            claude_request_body(),
            &HeaderMap::new(),
            vec![provider.clone()],
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
            RectifierConfig::default(),
        )
        .await
        .expect("single provider request should succeed");

    assert_eq!(result.provider.id, provider.id);
    assert_eq!(hits.count.load(Ordering::SeqCst), 1);

    server.abort();
}

#[tokio::test]
async fn single_provider_respects_open_breaker_without_explicit_bypass_option() {
    let (base_url, hits, server) = spawn_mock_upstream(StatusCode::OK, json!({"ok": true})).await;
    let provider = claude_provider("p1", &base_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router.clone()).expect("create forwarder");

    db.save_provider("claude", &provider)
        .expect("save provider for health tracking");
    let mut config = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("load proxy config");
    config.circuit_timeout_seconds = 3600;
    db.update_proxy_config_for_app(config)
        .await
        .expect("update proxy timeout");

    router
        .record_result(
            "p1",
            "claude",
            false,
            false,
            Some("open breaker".to_string()),
        )
        .await
        .expect("open breaker");
    assert!(!router.allow_provider_request("p1", "claude").await.allowed);

    let error = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            claude_request_body(),
            &HeaderMap::new(),
            vec![provider.clone()],
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect_err("single provider request should respect an open breaker");

    assert!(matches!(error, ProxyError::NoAvailableProvider));
    assert_eq!(hits.count.load(Ordering::SeqCst), 0);

    server.abort();
}

#[tokio::test]
async fn single_streaming_provider_respects_open_breaker_without_explicit_bypass_option() {
    let (base_url, hits, bodies, server) = spawn_scripted_streaming_upstream(vec![(
        StatusCode::OK,
        ScriptedStreamingBody::Sse(
            "data: {\"id\":\"msg_123\",\"type\":\"message_start\"}\n\ndata: [DONE]\n\n",
        ),
    )])
    .await;
    let provider = claude_provider("p1", &base_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router.clone()).expect("create forwarder");

    db.save_provider("claude", &provider)
        .expect("save provider for health tracking");
    let mut config = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("load proxy config");
    config.circuit_timeout_seconds = 3600;
    db.update_proxy_config_for_app(config)
        .await
        .expect("update proxy timeout");

    router
        .record_result(
            "p1",
            "claude",
            false,
            false,
            Some("open breaker".to_string()),
        )
        .await
        .expect("open breaker");
    assert!(!router.allow_provider_request("p1", "claude").await.allowed);

    let body = json!({
        "model": "claude-3-7-sonnet-20250219",
        "stream": true,
        "max_tokens": 32,
        "messages": [{
            "role": "user",
            "content": [{ "type": "text", "text": "hello" }]
        }]
    });

    let error = forwarder
        .forward_response(
            &AppType::Claude,
            "/v1/messages",
            body,
            &HeaderMap::new(),
            vec![provider.clone()],
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect_err("single provider streaming request should respect an open breaker");

    assert!(matches!(error, ProxyError::NoAvailableProvider));
    assert_eq!(hits.count.load(Ordering::SeqCst), 0);
    assert_eq!(bodies.lock().await.len(), 0);

    server.abort();
}

#[tokio::test]
async fn claude_buffered_failover_uses_second_provider_and_per_provider_endpoint() {
    let (primary_url, primary_hits, primary_server) = spawn_mock_upstream(
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({"error": {"message": "primary down"}}),
    )
    .await;
    let (secondary_url, secondary_hits, secondary_server) =
        spawn_mock_upstream(StatusCode::OK, json!({"id": "resp_123", "ok": true})).await;
    let provider_one = claude_provider("p1", &primary_url, Some("openai_chat"));
    let provider_two = claude_provider("p2", &secondary_url, Some("openai_chat"));
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router.clone()).expect("create forwarder");

    db.save_provider("claude", &provider_one)
        .expect("save primary provider for health tracking");
    db.save_provider("claude", &provider_two)
        .expect("save secondary provider for health tracking");

    let result = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            claude_request_body(),
            &HeaderMap::new(),
            vec![provider_one, provider_two.clone()],
            ForwardOptions {
                max_retries: 1,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect("second provider should succeed");

    assert_eq!(result.provider.id, provider_two.id);
    assert_eq!(primary_hits.count.load(Ordering::SeqCst), 1);
    assert_eq!(secondary_hits.count.load(Ordering::SeqCst), 1);
    assert_eq!(
        primary_hits.paths.lock().await.as_slice(),
        ["/v1/chat/completions"]
    );
    assert_eq!(
        secondary_hits.paths.lock().await.as_slice(),
        ["/v1/chat/completions"]
    );

    primary_server.abort();
    secondary_server.abort();
}

#[tokio::test]
async fn max_retries_zero_attempts_only_the_first_available_provider() {
    let (primary_url, primary_hits, primary_server) = spawn_mock_upstream(
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({"error": {"message": "primary down"}}),
    )
    .await;
    let (secondary_url, secondary_hits, secondary_server) =
        spawn_mock_upstream(StatusCode::OK, json!({"ok": true})).await;
    let primary = claude_provider("p1", &primary_url, None);
    let secondary = claude_provider("p2", &secondary_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    db.save_provider("claude", &primary)
        .expect("save primary provider");
    db.save_provider("claude", &secondary)
        .expect("save secondary provider");

    let error = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            claude_request_body(),
            &HeaderMap::new(),
            vec![primary, secondary],
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect_err("zero retries should not attempt the fallback provider");

    assert!(matches!(
        error,
        ProxyError::UpstreamError { status: 500, .. }
    ));
    assert_eq!(primary_hits.count.load(Ordering::SeqCst), 1);
    assert_eq!(secondary_hits.count.load(Ordering::SeqCst), 0);

    primary_server.abort();
    secondary_server.abort();
}

#[tokio::test]
async fn max_retries_caps_attempts_before_the_remaining_queue() {
    let (first_url, first_hits, first_server) = spawn_mock_upstream(
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({"error": {"message": "first down"}}),
    )
    .await;
    let (second_url, second_hits, second_server) = spawn_mock_upstream(
        StatusCode::BAD_GATEWAY,
        json!({"error": {"message": "second down"}}),
    )
    .await;
    let (third_url, third_hits, third_server) =
        spawn_mock_upstream(StatusCode::OK, json!({"ok": true})).await;
    let first = claude_provider("p1", &first_url, None);
    let second = claude_provider("p2", &second_url, None);
    let third = claude_provider("p3", &third_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    for provider in [&first, &second, &third] {
        db.save_provider("claude", provider).expect("save provider");
    }

    let error = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            claude_request_body(),
            &HeaderMap::new(),
            vec![first, second, third],
            ForwardOptions {
                max_retries: 1,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect_err("one retry should cap the request at two providers");

    assert!(matches!(
        error,
        ProxyError::UpstreamError { status: 502, .. }
    ));
    assert_eq!(first_hits.count.load(Ordering::SeqCst), 1);
    assert_eq!(second_hits.count.load(Ordering::SeqCst), 1);
    assert_eq!(third_hits.count.load(Ordering::SeqCst), 0);

    first_server.abort();
    second_server.abort();
    third_server.abort();
}

#[tokio::test]
async fn open_breaker_candidates_do_not_consume_the_attempt_limit() {
    let (open_url, open_hits, open_server) =
        spawn_mock_upstream(StatusCode::OK, json!({"unexpected": true})).await;
    let (healthy_url, healthy_hits, healthy_server) =
        spawn_mock_upstream(StatusCode::OK, json!({"ok": true})).await;
    let open_provider = claude_provider("open", &open_url, None);
    let healthy_provider = claude_provider("healthy", &healthy_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router.clone()).expect("create forwarder");

    db.save_provider("claude", &open_provider)
        .expect("save open provider");
    db.save_provider("claude", &healthy_provider)
        .expect("save healthy provider");
    let mut config = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("load proxy config");
    config.circuit_timeout_seconds = 3600;
    db.update_proxy_config_for_app(config)
        .await
        .expect("update circuit timeout");
    router
        .record_result(
            &open_provider.id,
            "claude",
            false,
            false,
            Some("open breaker".to_string()),
        )
        .await
        .expect("open first provider breaker");

    let result = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            claude_request_body(),
            &HeaderMap::new(),
            vec![open_provider, healthy_provider],
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect("skipped open breaker should not consume the only attempt");

    assert_eq!(result.provider.id, "healthy");
    assert_eq!(open_hits.count.load(Ordering::SeqCst), 0);
    assert_eq!(healthy_hits.count.load(Ordering::SeqCst), 1);

    open_server.abort();
    healthy_server.abort();
}

#[tokio::test]
async fn buffered_failover_applies_each_providers_own_request_overrides() {
    let (primary_url, primary_hits, primary_bodies, primary_server) =
        spawn_scripted_upstream(vec![(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error": {"message": "primary down"}}),
        )])
        .await;
    let (secondary_url, secondary_hits, secondary_bodies, secondary_server) =
        spawn_scripted_upstream(vec![(StatusCode::OK, json!({"ok": true}))]).await;
    let mut primary_provider = claude_provider("primary", &primary_url, None);
    primary_provider.meta = Some(ProviderMeta {
        custom_user_agent: Some("primary-agent/1.0".to_string()),
        local_proxy_request_overrides: Some(LocalProxyRequestOverrides {
            headers: [("x-provider-route".to_string(), "primary".to_string())]
                .into_iter()
                .collect(),
            body: Some(json!({"metadata": {"provider_route": "primary"}})),
        }),
        ..Default::default()
    });
    let mut secondary_provider = claude_provider("secondary", &secondary_url, None);
    secondary_provider.meta = Some(ProviderMeta {
        custom_user_agent: Some("secondary-agent/2.0".to_string()),
        local_proxy_request_overrides: Some(LocalProxyRequestOverrides {
            headers: [("x-provider-route".to_string(), "secondary".to_string())]
                .into_iter()
                .collect(),
            body: Some(json!({"metadata": {"provider_route": "secondary"}})),
        }),
        ..Default::default()
    });
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    db.save_provider("claude", &primary_provider)
        .expect("save primary provider for health tracking");
    db.save_provider("claude", &secondary_provider)
        .expect("save secondary provider for health tracking");

    let result = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            claude_request_body(),
            &HeaderMap::new(),
            vec![primary_provider, secondary_provider],
            ForwardOptions {
                max_retries: 1,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect("secondary provider should succeed after primary failure");

    assert_eq!(result.provider.id, "secondary");
    assert_eq!(primary_hits.count.load(Ordering::SeqCst), 1);
    assert_eq!(secondary_hits.count.load(Ordering::SeqCst), 1);

    let primary_headers = primary_hits.headers.lock().await;
    assert_eq!(primary_headers.len(), 1);
    assert_eq!(primary_headers[0]["user-agent"], "primary-agent/1.0");
    assert_eq!(primary_headers[0]["x-provider-route"], "primary");
    drop(primary_headers);

    let secondary_headers = secondary_hits.headers.lock().await;
    assert_eq!(secondary_headers.len(), 1);
    assert_eq!(secondary_headers[0]["user-agent"], "secondary-agent/2.0");
    assert_eq!(secondary_headers[0]["x-provider-route"], "secondary");
    drop(secondary_headers);

    let primary_bodies = primary_bodies.lock().await;
    assert_eq!(primary_bodies.len(), 1);
    assert_eq!(primary_bodies[0]["metadata"]["provider_route"], "primary");
    drop(primary_bodies);

    let secondary_bodies = secondary_bodies.lock().await;
    assert_eq!(secondary_bodies.len(), 1);
    assert_eq!(
        secondary_bodies[0]["metadata"]["provider_route"],
        "secondary"
    );

    primary_server.abort();
    secondary_server.abort();
}

#[tokio::test]
async fn failover_enabled_single_queued_negative_provider_does_not_use_non_queued_healthy_provider()
{
    let (queued_url, queued_hits, queued_server) = spawn_mock_upstream(
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({"error": {"message": "queued down"}}),
    )
    .await;
    let (healthy_url, healthy_hits, healthy_server) =
        spawn_mock_upstream(StatusCode::OK, json!({"id": "resp_healthy", "ok": true})).await;
    let queued_provider = claude_provider("queued", &queued_url, None);
    let healthy_provider = claude_provider("healthy", &healthy_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router.clone()).expect("create forwarder");

    db.save_provider("claude", &queued_provider)
        .expect("save queued provider");
    db.save_provider("claude", &healthy_provider)
        .expect("save healthy provider");
    db.set_current_provider("claude", &healthy_provider.id)
        .expect("set non-queued current provider");
    db.add_to_failover_queue("claude", &queued_provider.id)
        .expect("queue negative provider");
    let mut config = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("load proxy config");
    config.enabled = true;
    config.auto_failover_enabled = true;
    db.update_proxy_config_for_app(config)
        .await
        .expect("enable failover");

    let selected = router
        .select_providers("claude")
        .await
        .expect("select queued providers");
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].id, queued_provider.id);

    let error = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            claude_request_body(),
            &HeaderMap::new(),
            selected,
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect_err(
            "single queued negative provider should fail without using non-queued healthy provider",
        );

    assert!(matches!(
        error,
        ProxyError::UpstreamError { status: 500, .. }
    ));
    assert_eq!(queued_hits.count.load(Ordering::SeqCst), 1);
    assert_eq!(healthy_hits.count.load(Ordering::SeqCst), 0);

    queued_server.abort();
    healthy_server.abort();
}

#[tokio::test]
async fn failover_enabled_multiple_queued_providers_transfer_by_queue_priority() {
    let (primary_url, primary_hits, primary_server) = spawn_mock_upstream(
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({"error": {"message": "primary down"}}),
    )
    .await;
    let (secondary_url, secondary_hits, secondary_server) =
        spawn_mock_upstream(StatusCode::OK, json!({"id": "resp_secondary", "ok": true})).await;
    let primary_provider = claude_provider("primary", &primary_url, None);
    let secondary_provider = claude_provider("secondary", &secondary_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router.clone()).expect("create forwarder");

    db.save_provider("claude", &primary_provider)
        .expect("save primary provider");
    db.save_provider("claude", &secondary_provider)
        .expect("save secondary provider");
    db.add_to_failover_queue("claude", &primary_provider.id)
        .expect("queue primary provider");
    db.add_to_failover_queue("claude", &secondary_provider.id)
        .expect("queue secondary provider");
    let mut config = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("load proxy config");
    config.enabled = true;
    config.auto_failover_enabled = true;
    db.update_proxy_config_for_app(config)
        .await
        .expect("enable failover");

    let selected = router
        .select_providers("claude")
        .await
        .expect("select queued providers");
    assert_eq!(selected[0].id, primary_provider.id);
    assert_eq!(selected[1].id, secondary_provider.id);

    let result = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            claude_request_body(),
            &HeaderMap::new(),
            selected,
            ForwardOptions {
                max_retries: 1,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect("secondary queued provider should succeed after primary failure");

    assert_eq!(result.provider.id, secondary_provider.id);
    assert_eq!(primary_hits.count.load(Ordering::SeqCst), 1);
    assert_eq!(secondary_hits.count.load(Ordering::SeqCst), 1);

    primary_server.abort();
    secondary_server.abort();
}

#[tokio::test]
async fn failover_enabled_all_queued_providers_unavailable_fails_after_attempting_queue() {
    let (primary_url, primary_hits, primary_server) = spawn_mock_upstream(
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({"error": {"message": "primary down"}}),
    )
    .await;
    let (secondary_url, secondary_hits, secondary_server) = spawn_mock_upstream(
        StatusCode::BAD_GATEWAY,
        json!({"error": {"message": "secondary down"}}),
    )
    .await;
    let primary_provider = claude_provider("primary", &primary_url, None);
    let secondary_provider = claude_provider("secondary", &secondary_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router.clone()).expect("create forwarder");

    db.save_provider("claude", &primary_provider)
        .expect("save primary provider");
    db.save_provider("claude", &secondary_provider)
        .expect("save secondary provider");
    db.add_to_failover_queue("claude", &primary_provider.id)
        .expect("queue primary provider");
    db.add_to_failover_queue("claude", &secondary_provider.id)
        .expect("queue secondary provider");
    let mut config = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("load proxy config");
    config.enabled = true;
    config.auto_failover_enabled = true;
    db.update_proxy_config_for_app(config)
        .await
        .expect("enable failover");

    let selected = router
        .select_providers("claude")
        .await
        .expect("select queued providers");

    let error = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            claude_request_body(),
            &HeaderMap::new(),
            selected,
            ForwardOptions {
                max_retries: 1,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect_err("all queued negative providers should fail");

    assert!(matches!(
        error,
        ProxyError::UpstreamError { status: 502, .. }
    ));
    assert_eq!(primary_hits.count.load(Ordering::SeqCst), 1);
    assert_eq!(secondary_hits.count.load(Ordering::SeqCst), 1);

    primary_server.abort();
    secondary_server.abort();
}
#[tokio::test]
async fn plain_buffered_400_stops_without_polluting_provider_health() {
    let (primary_url, primary_hits, primary_server) = spawn_mock_upstream(
        StatusCode::BAD_REQUEST,
        json!({"error": {"message": "bad request"}}),
    )
    .await;
    let (secondary_url, secondary_hits, secondary_server) =
        spawn_mock_upstream(StatusCode::OK, json!({"ok": true})).await;
    let provider_one = claude_provider("p1", &primary_url, None);
    let provider_two = claude_provider("p2", &secondary_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router.clone()).expect("create forwarder");

    db.save_provider("claude", &provider_one)
        .expect("save primary provider for health tracking");
    db.save_provider("claude", &provider_two)
        .expect("save secondary provider for health tracking");

    router
        .record_result(
            "p1",
            "claude",
            false,
            false,
            Some("open breaker".to_string()),
        )
        .await
        .expect("open breaker");

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
        .expect_err("plain 400 is a non-retryable client error");

    assert!(matches!(
        error,
        ProxyError::UpstreamError { status: 400, .. }
    ));
    assert_eq!(primary_hits.count.load(Ordering::SeqCst), 1);
    assert_eq!(secondary_hits.count.load(Ordering::SeqCst), 0);

    let permit = router.allow_provider_request("p1", "claude").await;
    assert!(permit.allowed);
    assert!(permit.used_half_open_permit);

    primary_server.abort();
    secondary_server.abort();
}

#[tokio::test]
async fn nonretryable_http_status_family_stops_and_keeps_health_neutral() {
    let statuses = [400, 405, 406, 413, 414, 415, 422, 501];
    let primary_responses = statuses
        .iter()
        .map(|status| {
            (
                StatusCode::from_u16(*status).expect("valid status"),
                json!({"error": {"message": format!("client error {status}")}}),
            )
        })
        .collect();
    let (primary_url, primary_hits, _bodies, primary_server) =
        spawn_scripted_upstream(primary_responses).await;
    let (secondary_url, secondary_hits, secondary_server) =
        spawn_mock_upstream(StatusCode::OK, json!({"ok": true})).await;
    let primary = claude_provider("p1", &primary_url, None);
    let secondary = claude_provider("p2", &secondary_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    db.save_provider("claude", &primary)
        .expect("save primary provider");
    db.save_provider("claude", &secondary)
        .expect("save secondary provider");

    for status in statuses {
        let error = forwarder
            .forward_buffered_response(
                &AppType::Claude,
                "/v1/messages",
                claude_request_body(),
                &HeaderMap::new(),
                vec![primary.clone(), secondary.clone()],
                ForwardOptions {
                    max_retries: 1,
                    request_timeout: Some(Duration::from_secs(2)),
                    bypass_circuit_breaker: false,
                },
                RectifierConfig::default(),
            )
            .await
            .expect_err("client status should not fail over");
        assert!(matches!(
            error,
            ProxyError::UpstreamError {
                status: actual,
                ..
            } if actual == status
        ));
    }

    let health = db
        .get_provider_health(&primary.id, "claude")
        .await
        .expect("load primary health");
    assert_eq!(health.consecutive_failures, 0);
    assert!(health.last_failure_at.is_none());
    assert_eq!(primary_hits.count.load(Ordering::SeqCst), statuses.len());
    assert_eq!(secondary_hits.count.load(Ordering::SeqCst), 0);

    primary_server.abort();
    secondary_server.abort();
}

#[tokio::test]
async fn official_codex_401_and_403_do_not_fail_over_or_pollute_health() {
    let (official_url, official_hits, _bodies, official_server) = spawn_scripted_upstream(vec![
        (
            StatusCode::UNAUTHORIZED,
            json!({"error": {"message": "login expired"}}),
        ),
        (
            StatusCode::FORBIDDEN,
            json!({"error": {"message": "account forbidden"}}),
        ),
    ])
    .await;
    let (fallback_url, fallback_hits, fallback_server) =
        spawn_mock_upstream(StatusCode::OK, json!({"status": "completed"})).await;
    let official = codex_provider("official", &official_url, true);
    let fallback = codex_provider("fallback", &fallback_url, false);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    db.save_provider("codex", &official)
        .expect("save official Codex provider");
    db.save_provider("codex", &fallback)
        .expect("save fallback Codex provider");

    for expected_status in [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN] {
        let result = forwarder
            .forward_buffered_response(
                &AppType::Codex,
                "/responses",
                json!({"model": "gpt-5.4", "input": "hello"}),
                &HeaderMap::new(),
                vec![official.clone(), fallback.clone()],
                ForwardOptions {
                    max_retries: 1,
                    request_timeout: Some(Duration::from_secs(2)),
                    bypass_circuit_breaker: false,
                },
                RectifierConfig::default(),
            )
            .await
            .expect("official Codex auth error should be returned without failover");
        assert_eq!(result.provider.id, official.id);
        assert_eq!(result.response.status, expected_status);
    }

    let health = db
        .get_provider_health(&official.id, "codex")
        .await
        .expect("load official provider health");
    assert_eq!(health.consecutive_failures, 0);
    assert!(health.last_failure_at.is_none());
    assert_eq!(official_hits.count.load(Ordering::SeqCst), 2);
    assert_eq!(fallback_hits.count.load(Ordering::SeqCst), 0);

    official_server.abort();
    fallback_server.abort();
}

#[tokio::test]
async fn responses_json_2xx_failure_fails_over_before_provider_commit() {
    let (primary_url, primary_hits, primary_server) = spawn_mock_upstream(
        StatusCode::OK,
        json!({
            "id": "resp_failed",
            "status": "failed",
            "error": {"type": "server_error", "message": "primary failed"}
        }),
    )
    .await;
    let (secondary_url, secondary_hits, secondary_server) = spawn_mock_upstream(
        StatusCode::OK,
        json!({"id": "resp_ok", "status": "completed", "output": []}),
    )
    .await;
    let primary = claude_provider("p1", &primary_url, Some("openai_responses"));
    let secondary = claude_provider("p2", &secondary_url, Some("openai_responses"));
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    db.save_provider("claude", &primary)
        .expect("save primary provider");
    db.save_provider("claude", &secondary)
        .expect("save secondary provider");

    let result = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            claude_request_body(),
            &HeaderMap::new(),
            vec![primary, secondary],
            ForwardOptions {
                max_retries: 1,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect("2xx semantic failure should fail over");

    assert_eq!(result.provider.id, "p2");
    assert_eq!(primary_hits.count.load(Ordering::SeqCst), 1);
    assert_eq!(secondary_hits.count.load(Ordering::SeqCst), 1);

    primary_server.abort();
    secondary_server.abort();
}

#[tokio::test]
async fn responses_sse_failure_before_output_fails_over_and_replays_fallback() {
    let primary_sse = concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"status\":\"in_progress\"}}\n\n",
        "event: response.failed\n",
        "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"type\":\"server_error\",\"message\":\"boom\"}}}\n\n"
    );
    let fallback_sse = concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"status\":\"in_progress\"}}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n"
    );
    let (primary_url, primary_hits, _primary_bodies, primary_server) =
        spawn_scripted_streaming_upstream(vec![(
            StatusCode::OK,
            ScriptedStreamingBody::Sse(primary_sse),
        )])
        .await;
    let (secondary_url, secondary_hits, _secondary_bodies, secondary_server) =
        spawn_scripted_streaming_upstream(vec![(
            StatusCode::OK,
            ScriptedStreamingBody::Sse(fallback_sse),
        )])
        .await;
    let primary = claude_provider("p1", &primary_url, Some("openai_responses"));
    let secondary = claude_provider("p2", &secondary_url, Some("openai_responses"));
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    db.save_provider("claude", &primary)
        .expect("save primary provider");
    db.save_provider("claude", &secondary)
        .expect("save secondary provider");

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
            vec![primary, secondary],
            ForwardOptions {
                max_retries: 1,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect("pre-output response.failed should fail over");

    assert_eq!(result.provider.id, "p2");
    let super::super::StreamingResponse::Live(response) = result.response else {
        panic!("fallback Responses body should remain streaming");
    };
    let chunks = response
        .bytes_stream()
        .map(|chunk| chunk.expect("read replayed Responses chunk"))
        .collect::<Vec<_>>()
        .await;
    assert_eq!(chunks.concat().as_slice(), fallback_sse.as_bytes());
    assert_eq!(primary_hits.count.load(Ordering::SeqCst), 1);
    assert_eq!(secondary_hits.count.load(Ordering::SeqCst), 1);

    primary_server.abort();
    secondary_server.abort();
}

#[tokio::test]
async fn claude_buffered_rectifier_owned_400_stops_before_next_provider() {
    let (primary_url, primary_hits, primary_bodies, primary_server) = spawn_scripted_upstream(vec![
        (
            StatusCode::BAD_REQUEST,
            json!({"error": {"message": "messages.1.content.0: Invalid `signature` in `thinking` block"}}),
        ),
        (
            StatusCode::BAD_REQUEST,
            json!({"error": {"message": "messages.1.content.0: Invalid `signature` in `thinking` block"}}),
        ),
    ])
    .await;
    let (secondary_url, secondary_hits, secondary_server) =
        spawn_mock_upstream(StatusCode::OK, json!({"ok": true})).await;
    let provider_one = claude_provider("p1", &primary_url, None);
    let provider_two = claude_provider("p2", &secondary_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router.clone()).expect("create forwarder");

    db.save_provider("claude", &provider_one)
        .expect("save primary provider for health tracking");
    db.save_provider("claude", &provider_two)
        .expect("save secondary provider for health tracking");
    router
        .record_result(
            "p1",
            "claude",
            false,
            false,
            Some("open breaker".to_string()),
        )
        .await
        .expect("open primary breaker");

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
            vec![provider_one, provider_two],
            ForwardOptions {
                max_retries: 1,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect_err("rectifier-owned 400 should surface as UpstreamError");

    match error {
        ProxyError::UpstreamError { status, body } => {
            assert_eq!(status, 400);
            assert!(body
                .as_deref()
                .expect("rectifier-owned 400 should preserve body")
                .contains("Invalid `signature`"));
        }
        other => panic!("expected UpstreamError, got {other:?}"),
    }
    assert_eq!(primary_hits.count.load(Ordering::SeqCst), 2);
    assert_eq!(secondary_hits.count.load(Ordering::SeqCst), 0);

    let sent_bodies = primary_bodies.lock().await;
    assert_eq!(sent_bodies.len(), 2);

    let permit = router.allow_provider_request("p1", "claude").await;
    assert!(permit.allowed);
    assert!(permit.used_half_open_permit);

    primary_server.abort();
    secondary_server.abort();
}

#[tokio::test]
async fn plain_streaming_422_stops_before_next_provider() {
    let (primary_url, primary_hits, primary_bodies, primary_server) =
        spawn_scripted_streaming_upstream(vec![(
            StatusCode::UNPROCESSABLE_ENTITY,
            ScriptedStreamingBody::Json(json!({"error": {"message": "unprocessable request"}})),
        )])
        .await;
    let (secondary_url, secondary_hits, secondary_bodies, secondary_server) =
        spawn_scripted_streaming_upstream(vec![(
            StatusCode::OK,
            ScriptedStreamingBody::Sse(
                "data: {\"id\":\"msg_123\",\"type\":\"message_start\"}\n\ndata: [DONE]\n\n",
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

    let body = json!({
        "model": "claude-3-7-sonnet-20250219",
        "stream": true,
        "max_tokens": 32,
        "messages": [{
            "role": "user",
            "content": [{ "type": "text", "text": "hello" }]
        }]
    });

    let error = forwarder
        .forward_response(
            &AppType::Claude,
            "/v1/messages",
            body,
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
        .expect_err("plain streaming 422 is a non-retryable client error");

    assert!(matches!(
        error,
        ProxyError::UpstreamError { status: 422, .. }
    ));
    assert_eq!(primary_hits.count.load(Ordering::SeqCst), 1);
    assert_eq!(secondary_hits.count.load(Ordering::SeqCst), 0);
    assert_eq!(primary_bodies.lock().await.len(), 1);
    assert_eq!(secondary_bodies.lock().await.len(), 0);

    primary_server.abort();
    secondary_server.abort();
}

#[tokio::test]
async fn single_candidate_with_failover_enabled_respects_open_breaker() {
    let (base_url, hits, server) = spawn_mock_upstream(StatusCode::OK, json!({"ok": true})).await;
    let provider = claude_provider("p1", &base_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router.clone()).expect("create forwarder");

    db.save_provider("claude", &provider)
        .expect("save provider for health tracking");
    let mut config = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("load proxy config");
    config.circuit_timeout_seconds = 3600;
    db.update_proxy_config_for_app(config)
        .await
        .expect("update proxy timeout");

    router
        .record_result(
            "p1",
            "claude",
            false,
            false,
            Some("open breaker".to_string()),
        )
        .await
        .expect("open breaker");

    let error = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            claude_request_body(),
            &HeaderMap::new(),
            vec![provider.clone()],
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect_err("single failover candidate should respect an open breaker");

    assert!(matches!(error, ProxyError::NoAvailableProvider));
    assert_eq!(hits.count.load(Ordering::SeqCst), 0);

    server.abort();
}

#[tokio::test]
async fn skipped_candidates_preserve_last_attempted_upstream_response() {
    let (primary_url, primary_hits, primary_server) = spawn_mock_upstream(
        StatusCode::TOO_MANY_REQUESTS,
        json!({"error": {"message": "rate limited"}}),
    )
    .await;
    let (skipped_url, skipped_hits, skipped_server) =
        spawn_mock_upstream(StatusCode::OK, json!({"ok": true})).await;
    let primary_provider = claude_provider("p1", &primary_url, None);
    let skipped_provider = claude_provider("p2", &skipped_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router.clone()).expect("create forwarder");

    db.save_provider("claude", &primary_provider)
        .expect("save primary provider for health tracking");
    db.save_provider("claude", &skipped_provider)
        .expect("save skipped provider for health tracking");

    let mut config = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("load proxy config");
    config.circuit_timeout_seconds = 3600;
    db.update_proxy_config_for_app(config)
        .await
        .expect("update proxy timeout");

    router
        .record_result(
            "p2",
            "claude",
            false,
            false,
            Some("open breaker".to_string()),
        )
        .await
        .expect("open skipped provider breaker");
    assert!(!router.allow_provider_request("p2", "claude").await.allowed);

    let error = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            claude_request_body(),
            &HeaderMap::new(),
            vec![primary_provider, skipped_provider],
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect_err("skipped candidates should surface the last attempted upstream error");

    match error {
        ProxyError::UpstreamError { status, body } => {
            assert_eq!(status, 429);
            let parsed: Value = serde_json::from_str(
                body.as_deref()
                    .expect("skipped candidates should preserve upstream body"),
            )
            .expect("parse body");
            assert_eq!(parsed, json!({"error": {"message": "rate limited"}}));
        }
        other => panic!("expected UpstreamError, got {other:?}"),
    }

    assert_eq!(primary_hits.count.load(Ordering::SeqCst), 1);
    assert_eq!(skipped_hits.count.load(Ordering::SeqCst), 0);

    primary_server.abort();
    skipped_server.abort();
}

#[tokio::test]
async fn later_half_open_provider_permit_is_not_preclaimed_when_earlier_success_stops() {
    let (primary_url, primary_hits, primary_server) =
        spawn_mock_upstream(StatusCode::OK, json!({"ok": true})).await;
    let (half_open_url, half_open_hits, half_open_server) =
        spawn_mock_upstream(StatusCode::OK, json!({"ok": true})).await;
    let primary_provider = claude_provider("p1", &primary_url, None);
    let half_open_provider = claude_provider("p2", &half_open_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router.clone()).expect("create forwarder");

    db.save_provider("claude", &primary_provider)
        .expect("save primary provider for health tracking");
    db.save_provider("claude", &half_open_provider)
        .expect("save half-open provider for health tracking");

    router
        .record_result(
            "p2",
            "claude",
            false,
            false,
            Some("open breaker".to_string()),
        )
        .await
        .expect("move provider into half-open state");

    let result = forwarder
        .forward_buffered_response(
            &AppType::Claude,
            "/v1/messages",
            claude_request_body(),
            &HeaderMap::new(),
            vec![primary_provider, half_open_provider],
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: false,
            },
            RectifierConfig::default(),
        )
        .await
        .expect("earlier success should stop before later half-open provider");

    assert_eq!(result.provider.id, "p1");
    assert_eq!(primary_hits.count.load(Ordering::SeqCst), 1);
    assert_eq!(half_open_hits.count.load(Ordering::SeqCst), 0);

    let permit = router.allow_provider_request("p2", "claude").await;
    assert!(permit.allowed);
    assert!(permit.used_half_open_permit);

    primary_server.abort();
    half_open_server.abort();
}

#[tokio::test]
async fn claude_buffered_rectifier_retries_same_provider_on_invalid_signature() {
    let (base_url, hits, bodies, server) = spawn_scripted_upstream(vec![
        (
            StatusCode::BAD_REQUEST,
            json!({"error": {"message": "messages.1.content.0: Invalid `signature` in `thinking` block"}}),
        ),
        (StatusCode::OK, json!({"id": "msg_123", "content": []})),
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

    let result = forwarder
        .forward_buffered_response(
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
        .expect("signature rectifier should retry same provider once");

    assert_eq!(result.provider.id, "p1");
    assert_eq!(result.response.status, StatusCode::OK);
    assert_eq!(hits.count.load(Ordering::SeqCst), 2);

    let sent_bodies = bodies.lock().await;
    assert_eq!(sent_bodies.len(), 2);
    assert_eq!(
        sent_bodies[0]["messages"][0]["content"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let retried_content = sent_bodies[1]["messages"][0]["content"].as_array().unwrap();
    assert_eq!(retried_content.len(), 1);
    assert_eq!(retried_content[0]["type"], "text");
    assert!(retried_content[0].get("signature").is_none());

    server.abort();
}

#[tokio::test]
async fn claude_openai_chat_budget_rectifier_retries_same_provider_with_transformed_body() {
    let (base_url, hits, bodies, server) = spawn_scripted_upstream(vec![
        (
            StatusCode::BAD_REQUEST,
            json!({"error": {"message": "thinking.budget_tokens: Input should be greater than or equal to 1024"}}),
        ),
        (
            StatusCode::OK,
            json!({"id": "resp_123", "choices": [{"message": {"role": "assistant", "content": "ok"}}]}),
        ),
    ])
    .await;
    let provider = claude_provider("p1", &base_url, Some("openai_chat"));
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    db.save_provider("claude", &provider)
        .expect("save provider for health tracking");

    let body = json!({
        "model": "claude-3-7-sonnet-20250219",
        "max_tokens": 1024,
        "thinking": { "type": "enabled", "budget_tokens": 512 },
        "messages": [{
            "role": "user",
            "content": [{ "type": "text", "text": "hello" }]
        }]
    });

    let result = forwarder
        .forward_buffered_response(
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
        .expect("budget rectifier should retry same provider once");

    assert_eq!(result.provider.id, "p1");
    assert_eq!(result.response.status, StatusCode::OK);
    assert_eq!(hits.count.load(Ordering::SeqCst), 2);
    assert_eq!(
        hits.paths.lock().await.as_slice(),
        ["/v1/chat/completions", "/v1/chat/completions"]
    );

    let sent_bodies = bodies.lock().await;
    assert_eq!(sent_bodies.len(), 2);
    assert_eq!(sent_bodies[0]["max_tokens"], 1024);
    assert_eq!(sent_bodies[1]["max_tokens"], 64000);
    assert!(sent_bodies[1].get("messages").is_some());

    server.abort();
}

#[tokio::test]
async fn claude_streaming_rectifier_retries_same_provider_on_invalid_signature_error() {
    let (base_url, hits, bodies, server) = spawn_scripted_streaming_upstream(vec![
        (
            StatusCode::BAD_REQUEST,
            ScriptedStreamingBody::Json(
                json!({"error": {"message": "messages.1.content.0: Invalid `signature` in `thinking` block"}}),
            ),
        ),
        (
            StatusCode::OK,
            ScriptedStreamingBody::Sse(
                "data: {\"id\":\"msg_123\",\"type\":\"message_start\"}\n\ndata: [DONE]\n\n",
            ),
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
        .expect("streaming signature rectifier should retry same provider once");

    assert_eq!(result.provider.id, "p1");
    assert_eq!(result.response.status(), StatusCode::OK);
    assert_eq!(hits.count.load(Ordering::SeqCst), 2);

    let sent_bodies = bodies.lock().await;
    assert_eq!(sent_bodies.len(), 2);
    let retried_content = sent_bodies[1]["messages"][0]["content"].as_array().unwrap();
    assert_eq!(retried_content.len(), 1);
    assert_eq!(retried_content[0]["type"], "text");
    assert!(retried_content[0].get("signature").is_none());

    server.abort();
}

#[tokio::test]
async fn claude_streaming_rectifier_owned_400_stops_before_next_provider() {
    let (primary_url, primary_hits, primary_bodies, primary_server) =
        spawn_delayed_scripted_streaming_upstream(vec![
            (
                Duration::from_millis(0),
                StatusCode::BAD_REQUEST,
                ScriptedStreamingBody::Json(
                    json!({"error": {"message": "messages.1.content.0: Invalid `signature` in `thinking` block"}}),
                ),
            ),
            (
                Duration::from_millis(0),
                StatusCode::BAD_REQUEST,
                ScriptedStreamingBody::Json(
                    json!({"error": {"message": "messages.1.content.0: Invalid `signature` in `thinking` block"}}),
                ),
            ),
        ])
        .await;
    let (secondary_url, secondary_hits, secondary_bodies, secondary_server) =
        spawn_delayed_scripted_streaming_upstream(vec![(
            Duration::from_millis(0),
            StatusCode::OK,
            ScriptedStreamingBody::Sse(
                "data: {\"id\":\"msg_123\",\"type\":\"message_start\"}\n\ndata: [DONE]\n\n",
            ),
        )])
        .await;
    let provider_one = claude_provider("p1", &primary_url, None);
    let provider_two = claude_provider("p2", &secondary_url, None);
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router.clone()).expect("create forwarder");

    db.save_provider("claude", &provider_one)
        .expect("save primary provider for health tracking");
    db.save_provider("claude", &provider_two)
        .expect("save secondary provider for health tracking");
    router
        .record_result(
            "p1",
            "claude",
            false,
            false,
            Some("open breaker".to_string()),
        )
        .await
        .expect("open primary breaker");

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

    let error = forwarder
        .forward_response(
            &AppType::Claude,
            "/v1/messages",
            body,
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
        .expect_err("rectifier-owned streaming 400 should surface as UpstreamError");

    match error {
        ProxyError::UpstreamError { status, body } => {
            assert_eq!(status, 400);
            assert!(body
                .as_deref()
                .expect("rectifier-owned streaming 400 should preserve body")
                .contains("Invalid `signature`"));
        }
        other => panic!("expected UpstreamError, got {other:?}"),
    }
    assert_eq!(primary_hits.count.load(Ordering::SeqCst), 2);
    assert_eq!(secondary_hits.count.load(Ordering::SeqCst), 0);
    assert_eq!(primary_bodies.lock().await.len(), 2);
    assert_eq!(secondary_bodies.lock().await.len(), 0);

    let permit = router.allow_provider_request("p1", "claude").await;
    assert!(permit.allowed);
    assert!(permit.used_half_open_permit);

    primary_server.abort();
    secondary_server.abort();
}

#[tokio::test]
async fn claude_streaming_openai_chat_budget_rectifier_retries_same_provider() {
    let (base_url, hits, bodies, server) = spawn_scripted_streaming_upstream(vec![
        (
            StatusCode::BAD_REQUEST,
            ScriptedStreamingBody::Json(
                json!({"error": {"message": "thinking.budget_tokens: Input should be greater than or equal to 1024"}}),
            ),
        ),
        (
            StatusCode::OK,
            ScriptedStreamingBody::Sse(
                "data: {\"id\":\"chatcmpl_123\",\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n",
            ),
        ),
    ])
    .await;
    let provider = claude_provider("p1", &base_url, Some("openai_chat"));
    let (db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    db.save_provider("claude", &provider)
        .expect("save provider for health tracking");

    let body = json!({
        "model": "claude-3-7-sonnet-20250219",
        "stream": true,
        "max_tokens": 1024,
        "thinking": { "type": "enabled", "budget_tokens": 512 },
        "messages": [{
            "role": "user",
            "content": [{ "type": "text", "text": "hello" }]
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
        .expect("streaming budget rectifier should retry same provider once");

    assert_eq!(result.provider.id, "p1");
    assert_eq!(result.response.status(), StatusCode::OK);
    assert_eq!(hits.count.load(Ordering::SeqCst), 2);
    assert_eq!(
        hits.paths.lock().await.as_slice(),
        ["/v1/chat/completions", "/v1/chat/completions"]
    );

    let sent_bodies = bodies.lock().await;
    assert_eq!(sent_bodies.len(), 2);
    assert_eq!(sent_bodies[0]["max_tokens"], 1024);
    assert_eq!(sent_bodies[1]["max_tokens"], 64000);

    server.abort();
}
