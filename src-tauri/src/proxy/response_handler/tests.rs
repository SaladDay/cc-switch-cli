use std::{collections::HashMap, env, sync::Arc, time::Duration};

use axum::{
    body::{to_bytes, Body},
    http::StatusCode,
    response::Response,
};
use bytes::Bytes;
use serde_json::json;
use serial_test::serial;
use tempfile::TempDir;
use tokio::sync::RwLock;

use crate::{
    app_config::AppType,
    database::Database,
    provider::Provider,
    proxy::{provider_router::ProviderRouter, types::ProxyConfig},
};

use super::*;

struct TempHome {
    #[allow(dead_code)]
    dir: TempDir,
    original_home: Option<String>,
    original_userprofile: Option<String>,
    original_config_dir: Option<String>,
}

impl TempHome {
    fn new() -> Self {
        let dir = TempDir::new().expect("create temp home");
        let original_home = env::var("HOME").ok();
        let original_userprofile = env::var("USERPROFILE").ok();
        let original_config_dir = env::var("CC_SWITCH_CONFIG_DIR").ok();

        env::set_var("HOME", dir.path());
        env::set_var("USERPROFILE", dir.path());
        env::set_var("CC_SWITCH_CONFIG_DIR", dir.path().join(".cc-switch"));
        crate::settings::reload_test_settings();

        Self {
            dir,
            original_home,
            original_userprofile,
            original_config_dir,
        }
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        match &self.original_home {
            Some(value) => env::set_var("HOME", value),
            None => env::remove_var("HOME"),
        }

        match &self.original_userprofile {
            Some(value) => env::set_var("USERPROFILE", value),
            None => env::remove_var("USERPROFILE"),
        }

        match &self.original_config_dir {
            Some(value) => env::set_var("CC_SWITCH_CONFIG_DIR", value),
            None => env::remove_var("CC_SWITCH_CONFIG_DIR"),
        }

        crate::settings::reload_test_settings();
    }
}

fn test_provider_with_settings(
    id: &str,
    name: &str,
    settings_config: serde_json::Value,
) -> Provider {
    Provider {
        id: id.to_string(),
        name: name.to_string(),
        settings_config,
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

fn test_state_with_db(db: Arc<Database>) -> ProxyServerState {
    ProxyServerState {
        db: db.clone(),
        config: Arc::new(RwLock::new(ProxyConfig::default())),
        status: Arc::new(RwLock::new(crate::proxy::types::ProxyStatus::default())),
        start_time: Arc::new(RwLock::new(None)),
        current_providers: Arc::new(RwLock::new(HashMap::new())),
        provider_router: Arc::new(ProviderRouter::new(db)),
    }
}

fn test_state() -> ProxyServerState {
    test_state_with_db(Arc::new(Database::memory().expect("memory db")))
}

async fn set_takeover_enabled(db: &Database, app_type: &str, enabled: bool) {
    let mut config = db
        .get_proxy_config_for_app(app_type)
        .await
        .expect("read app proxy config");
    config.enabled = enabled;
    db.update_proxy_config_for_app(config)
        .await
        .expect("update app proxy config");
}

async fn settle_tasks() {
    tokio::time::sleep(Duration::from_millis(10)).await;
}

#[tokio::test]
async fn buffered_failures_still_accumulate_output_tokens() {
    let state = test_state();
    state.record_request_start().await;

    let response = PreparedResponse {
        response: Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .body(Body::from("upstream failure payload"))
            .expect("response"),
        stream_completion: None,
        estimated_output_tokens: 9,
        upstream_error_summary: None,
        body_bytes: None,
    };

    let _ = ResponseHandler::finish_buffered(
        &state,
        Ok(response),
        reqwest::StatusCode::BAD_GATEWAY,
        None,
        None,
    )
    .await;

    let snapshot = state.snapshot_status().await;
    assert_eq!(snapshot.failed_requests, 1);
    assert_eq!(snapshot.estimated_output_tokens_total, 9);
}

#[tokio::test]
async fn interrupted_streams_keep_partial_output_estimate() {
    let state = test_state();
    state.record_request_start().await;

    let stream = async_stream::stream! {
        yield Ok::<Bytes, std::io::Error>(Bytes::from_static(b"partial output"));
        yield Err::<Bytes, std::io::Error>(std::io::Error::other("boom"));
    };
    let response = PreparedResponse {
        response: Response::builder()
            .status(StatusCode::OK)
            .body(Body::from_stream(stream))
            .expect("response"),
        stream_completion: None,
        estimated_output_tokens: 0,
        upstream_error_summary: None,
        body_bytes: None,
    };

    let response = ResponseHandler::finish_streaming(
        &state,
        Ok(response),
        reqwest::StatusCode::OK,
        None,
        None,
    )
    .await;
    let _ = to_bytes(response.into_body(), usize::MAX).await;
    settle_tasks().await;

    let snapshot = state.snapshot_status().await;
    assert_eq!(snapshot.failed_requests, 1);
    assert!(snapshot.estimated_output_tokens_total > 0);
}

#[tokio::test]
async fn non_success_streams_accumulate_output_tokens_after_body_drains() {
    let state = test_state();
    state.record_request_start().await;

    let response = PreparedResponse {
        response: Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::from("bad request payload"))
            .expect("response"),
        stream_completion: None,
        estimated_output_tokens: 0,
        upstream_error_summary: None,
        body_bytes: None,
    };

    let response = ResponseHandler::finish_streaming(
        &state,
        Ok(response),
        reqwest::StatusCode::BAD_REQUEST,
        None,
        None,
    )
    .await;
    let _ = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    settle_tasks().await;

    let snapshot = state.snapshot_status().await;
    assert_eq!(snapshot.failed_requests, 1);
    assert!(snapshot.estimated_output_tokens_total > 0);
}

#[tokio::test]
async fn buffered_success_streaming_responses_do_not_record_termination_error() {
    let state = test_state();
    state.record_request_start().await;

    let response = PreparedResponse {
        response: Response::builder()
            .status(StatusCode::OK)
            .body(Body::from("transformed buffered fallback"))
            .expect("response"),
        stream_completion: None,
        estimated_output_tokens: 0,
        upstream_error_summary: None,
        body_bytes: Some(Bytes::from_static(b"transformed buffered fallback")),
    };

    let response = ResponseHandler::finish_streaming(
        &state,
        Ok(response),
        reqwest::StatusCode::OK,
        None,
        None,
    )
    .await;
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    assert_eq!(body, Bytes::from_static(b"transformed buffered fallback"));
    settle_tasks().await;

    let snapshot = state.snapshot_status().await;
    assert_eq!(snapshot.success_requests, 1);
    assert_eq!(snapshot.failed_requests, 0);
    assert!(snapshot.last_error.is_none());
    assert!(snapshot.estimated_output_tokens_total > 0);
}

#[tokio::test]
#[serial(home_settings)]
async fn streaming_success_syncs_failover_state_after_body_drains() {
    let _home = TempHome::new();
    let db = Arc::new(Database::memory().expect("memory db"));
    let current = test_provider_with_settings(
        "claude-current",
        "Claude Current",
        json!({"apiKey": "current-key", "base_url": "https://current.example"}),
    );
    let failover = test_provider_with_settings(
        "claude-failover",
        "Claude Failover",
        json!({"apiKey": "failover-key", "base_url": "https://failover.example"}),
    );

    db.save_provider("claude", &current)
        .expect("save current provider");
    db.save_provider("claude", &failover)
        .expect("save failover provider");
    db.set_current_provider("claude", &current.id)
        .expect("set current provider");
    crate::settings::set_current_provider(&AppType::Claude, Some(&current.id))
        .expect("set local current provider");
    db.save_live_backup(
        "claude",
        &serde_json::to_string(&current.settings_config).expect("serialize current backup"),
    )
    .await
    .expect("save current live backup");
    set_takeover_enabled(&db, "claude", true).await;

    let state = test_state_with_db(db.clone());
    state.record_request_start().await;

    let stream_completion = StreamCompletion::default();
    stream_completion.record_success();
    let response = PreparedResponse {
        response: Response::builder()
            .status(StatusCode::OK)
            .body(Body::from_stream(futures::stream::empty::<
                Result<Bytes, std::io::Error>,
            >()))
            .expect("response"),
        stream_completion: Some(stream_completion),
        estimated_output_tokens: 0,
        upstream_error_summary: None,
        body_bytes: None,
    };

    let response = ResponseHandler::finish_streaming(
        &state,
        Ok(response),
        reqwest::StatusCode::OK,
        Some(SuccessSyncInfo {
            app_type: AppType::Claude,
            provider: failover.clone(),
            current_provider_id_at_start: current.id.clone(),
        }),
        None,
    )
    .await;

    let status = state.snapshot_status().await;
    assert_eq!(status.current_provider_id, None);

    let _ = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("drain response body");
    settle_tasks().await;

    let status = state.snapshot_status().await;
    assert_eq!(
        status.current_provider_id.as_deref(),
        Some("claude-failover")
    );
    assert_eq!(
        db.get_current_provider("claude")
            .expect("read current provider")
            .as_deref(),
        Some("claude-failover")
    );
    assert_eq!(
        crate::settings::get_current_provider(&AppType::Claude).as_deref(),
        Some("claude-failover")
    );

    let backup = db
        .get_live_backup("claude")
        .await
        .expect("read live backup")
        .expect("live backup should remain present");
    let backup_snapshot: serde_json::Value =
        serde_json::from_str(&backup.original_config).expect("parse live backup");
    assert_eq!(
        backup_snapshot
            .get("base_url")
            .and_then(serde_json::Value::as_str),
        Some("https://failover.example")
    );
}
