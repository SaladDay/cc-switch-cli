use std::sync::Arc;

use axum::{routing::post, Router};
use cc_switch_lib::{Database, Provider, ProviderMeta, ProxyService};
use serde_json::{json, Value};

use crate::helpers::{
    bind_test_listener, handle_chat_completions_error,
    handle_chat_completions_non_standard_json_error, handle_chat_completions_plain_error,
    handle_invalid_chat_completions, parse_insert_values, set_claude_proxy_port_to_ephemeral,
    wait_for_request_log_lines,
};

#[tokio::test]
async fn proxy_claude_openai_chat_non_success_error_is_transformed_to_anthropic_shape() {
    let upstream_router =
        Router::new().route("/v1/chat/completions", post(handle_chat_completions_error));

    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("read upstream address");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let provider = Provider {
        id: "claude-openai-chat-error".to_string(),
        name: "Claude OpenAI Chat Error".to_string(),
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
            api_format: Some("openai_chat".to_string()),
            ..ProviderMeta::default()
        }),
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };
    db.save_provider("claude", &provider)
        .expect("save test provider");
    db.set_current_provider("claude", &provider.id)
        .expect("set current provider");

    set_claude_proxy_port_to_ephemeral(&db).await;
    let service = ProxyService::new(db.clone());

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
            "metadata": {
                "session_id": "claude-session-non-success"
            },
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }))
        .send()
        .await
        .expect("send request to proxy");

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

    let body: Value = response.json().await.expect("parse error response");
    assert_eq!(body.get("type").and_then(|v| v.as_str()), Some("error"));
    assert_eq!(
        body.pointer("/error/type").and_then(|v| v.as_str()),
        Some("invalid_request_error")
    );
    assert_eq!(
        body.pointer("/error/message").and_then(|v| v.as_str()),
        Some("upstream rejected the request")
    );

    let status = service.get_status().await;
    assert_eq!(
        status.last_error,
        Some("upstream returned 400: upstream rejected the request".to_string())
    );

    let log_lines = wait_for_request_log_lines(&db, 1).await;
    assert_eq!(log_lines.len(), 1);
    let log_values = parse_insert_values(&log_lines[0]);
    assert_eq!(log_values[1], "claude-openai-chat-error");
    assert_eq!(log_values[3], "claude-3-7-sonnet");
    assert_eq!(log_values[4], "claude-3-7-sonnet");
    assert_eq!(log_values[5], "");
    assert_eq!(log_values[10], "2");
    assert_eq!(log_values[19], "400");
    assert_eq!(log_values[21], "claude-session-non-success");
    assert!(log_values[20].contains("upstream rejected the request"));

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
async fn proxy_claude_openai_chat_non_standard_json_error_preserves_upstream_status_and_body() {
    let upstream_router = Router::new().route(
        "/v1/chat/completions",
        post(handle_chat_completions_non_standard_json_error),
    );

    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("read upstream address");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let provider = Provider {
        id: "claude-openai-chat-non-standard-error".to_string(),
        name: "Claude OpenAI Chat Non Standard Error".to_string(),
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
            api_format: Some("openai_chat".to_string()),
            ..ProviderMeta::default()
        }),
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };
    db.save_provider("claude", &provider)
        .expect("save test provider");
    db.set_current_provider("claude", &provider.id)
        .expect("set current provider");

    set_claude_proxy_port_to_ephemeral(&db).await;
    let service = ProxyService::new(db.clone());

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

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let body: Value = response.json().await.expect("parse passthrough error body");
    assert_eq!(
        body,
        json!({
            "message": "upstream rejected the request"
        })
    );

    let status = service.get_status().await;
    assert_eq!(
        status.last_error,
        Some("upstream returned 400: upstream rejected the request".to_string())
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
async fn proxy_claude_openai_chat_non_json_error_body_passthrough_preserves_status_and_body() {
    let upstream_router = Router::new().route(
        "/v1/chat/completions",
        post(handle_chat_completions_plain_error),
    );

    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("read upstream address");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let provider = Provider {
        id: "claude-openai-chat-plain-error".to_string(),
        name: "Claude OpenAI Chat Plain Error".to_string(),
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
            api_format: Some("openai_chat".to_string()),
            ..ProviderMeta::default()
        }),
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };
    db.save_provider("claude", &provider)
        .expect("save test provider");
    db.set_current_provider("claude", &provider.id)
        .expect("set current provider");

    set_claude_proxy_port_to_ephemeral(&db).await;
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
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(
        response.text().await.expect("read passthrough error body"),
        "upstream rate limit"
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

#[tokio::test]
async fn proxy_claude_buffered_transform_error_does_not_sync_failover_state() {
    let upstream_router = Router::new().route(
        "/v1/chat/completions",
        post(handle_invalid_chat_completions),
    );

    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("read upstream address");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let original_provider = Provider {
        id: "claude-openai-chat-current".to_string(),
        name: "Claude OpenAI Chat Current".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("http://{}", upstream_addr),
                "ANTHROPIC_API_KEY": "sk-test-current"
            }
        }),
        website_url: None,
        category: Some("claude".to_string()),
        created_at: None,
        sort_index: Some(1),
        notes: None,
        meta: Some(ProviderMeta {
            api_format: Some("openai_chat".to_string()),
            ..ProviderMeta::default()
        }),
        icon: None,
        icon_color: None,
        in_failover_queue: true,
    };
    let failover_provider = Provider {
        id: "claude-openai-chat-failover".to_string(),
        name: "Claude OpenAI Chat Failover".to_string(),
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
        meta: Some(ProviderMeta {
            api_format: Some("openai_chat".to_string()),
            ..ProviderMeta::default()
        }),
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

    let mut claude_proxy = db
        .get_proxy_config_for_app("claude")
        .await
        .expect("get claude proxy config");
    claude_proxy.auto_failover_enabled = true;
    claude_proxy.max_retries = 0;
    db.set_app_proxy_preferred_port("claude", 0)
        .expect("update claude app proxy port");
    db.update_proxy_config_for_app(claude_proxy)
        .await
        .expect("update claude proxy config");

    let service = ProxyService::new(db.clone());

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

    assert!(!response.status().is_success());
    assert_eq!(
        db.get_current_provider("claude")
            .expect("read current provider after transform error")
            .as_deref(),
        Some("claude-openai-chat-current")
    );

    let status = service.get_status().await;
    assert_eq!(status.current_provider_id, None);
    assert_eq!(status.current_provider, None);
    assert_eq!(status.failover_count, 0);
    assert!(status.active_targets.is_empty());

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}
