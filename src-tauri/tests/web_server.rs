// The serial test mutex is intentionally held across awaits (mirrors the other
// integration tests) to serialize filesystem-isolated cases.
#![allow(clippy::await_holding_lock)]

//! Integration tests for the `cc-switch web` HTTP bridge.
//!
//! Boots the real axum router against an isolated, seeded [`AppState`] and
//! exercises the invoke bridge, the session-token gate, and graceful
//! degradation for unwired commands. Mirrors the curl-level smoke test.

use std::sync::Arc;

use cc_switch_lib::web::{build_router, WebState};
use cc_switch_lib::AppState;
use serde_json::json;
use serial_test::serial;

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs};

const TOKEN: &str = "test-token";

/// Boot the router on an ephemeral loopback port and return its base URL plus
/// the join handle of the serving task.
async fn spawn_server() -> (String, tokio::task::JoinHandle<()>) {
    // Mirror the real `web serve` precondition: main runs startup recovery
    // (which seeds default providers) before dispatch; the test must do the
    // same since there is no main here.
    let state = AppState::try_new_with_startup_recovery().expect("seed app state");

    // build_router needs an assets dir with index.html; the API tests never
    // hit it, but the fallback service requires it to exist.
    let assets = std::env::temp_dir().join(format!("cc-switch-web-test-{}", std::process::id()));
    std::fs::create_dir_all(&assets).expect("create assets dir");
    std::fs::write(assets.join("index.html"), "<!doctype html><html></html>")
        .expect("write index.html");

    let web_state = WebState::new(Arc::new(state), TOKEN.to_string());
    let router = build_router(web_state, assets);

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    (format!("http://{addr}"), handle)
}

#[tokio::test]
#[serial]
async fn web_bridge_serves_providers_and_enforces_token() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let (base, server) = spawn_server().await;
    let client = reqwest::Client::new();

    // /api/health is public.
    let res = client
        .get(format!("{base}/api/health"))
        .send()
        .await
        .expect("health request");
    assert_eq!(res.status(), 200, "health should be public");

    // get_providers with a valid token returns the seeded provider in the
    // exact camelCase shape the frontend's TS `Provider` type expects.
    let res = client
        .post(format!("{base}/api/invoke/get_providers"))
        .header("x-cc-switch-token", TOKEN)
        .json(&json!({ "app": "claude" }))
        .send()
        .await
        .expect("get_providers request");
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.expect("json body");
    let provider = body
        .get("claude-official")
        .expect("seeded claude-official provider");
    assert!(
        provider.get("settingsConfig").map(|v| v.is_object()) == Some(true),
        "expected camelCase settingsConfig, got {provider}"
    );

    // Missing token -> 401.
    let res = client
        .post(format!("{base}/api/invoke/get_providers"))
        .json(&json!({ "app": "claude" }))
        .send()
        .await
        .expect("unauth request");
    assert_eq!(res.status(), 401, "missing token must be rejected");

    // Unwired command -> 501 NotImplemented (graceful degradation).
    let res = client
        .post(format!("{base}/api/invoke/not_a_real_command"))
        .header("x-cc-switch-token", TOKEN)
        .json(&json!({}))
        .send()
        .await
        .expect("unknown command request");
    assert_eq!(res.status(), 501, "unwired commands degrade to 501");

    server.abort();
}

#[tokio::test]
#[serial]
async fn web_bridge_switch_provider_round_trips() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let (base, server) = spawn_server().await;
    let client = reqwest::Client::new();

    // Switch to the seeded provider; expect the SwitchResult shape.
    let res = client
        .post(format!("{base}/api/invoke/switch_provider"))
        .header("x-cc-switch-token", TOKEN)
        .json(&json!({ "app": "claude", "id": "claude-official" }))
        .send()
        .await
        .expect("switch request");
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.expect("switch json");
    assert!(
        body.get("warnings").map(|v| v.is_array()) == Some(true),
        "expected {{ warnings: [] }}, got {body}"
    );

    // The current provider should now reflect the switch.
    let res = client
        .post(format!("{base}/api/invoke/get_current_provider"))
        .header("x-cc-switch-token", TOKEN)
        .json(&json!({ "app": "claude" }))
        .send()
        .await
        .expect("current request");
    assert_eq!(res.status(), 200);
    let current: String = res.json().await.expect("current json");
    assert_eq!(current, "claude-official");

    server.abort();
}

/// Exercises the async dispatch path: `get_proxy_status` calls an async
/// `ProxyService` fn via `common::block_on` (block_in_place), which requires a
/// multi-threaded runtime — hence the `flavor = "multi_thread"`. Also spot-checks
/// a few read-only commands across modules resolve through the dispatch chain.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn web_bridge_handles_async_and_cross_module_commands() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let (base, server) = spawn_server().await;
    let client = reqwest::Client::new();

    for cmd in [
        "get_proxy_status",      // async (block_on) — proxy module
        "get_settings",          // sync — meta module
        "get_usage_summary",     // sync — usage module
        "get_claude_mcp_status", // sync — mcp_config module
    ] {
        let res = client
            .post(format!("{base}/api/invoke/{cmd}"))
            .header("x-cc-switch-token", TOKEN)
            .json(&json!({}))
            .send()
            .await
            .unwrap_or_else(|e| panic!("{cmd} request failed: {e}"));
        assert_eq!(res.status(), 200, "{cmd} should return 200");
    }

    server.abort();
}

#[tokio::test]
#[serial]
async fn web_bridge_serves_provider_presets() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let (base, server) = spawn_server().await;
    let client = reqwest::Client::new();

    // Claude preset catalog: the CLI's template set (Custom is dropped), in the
    // frontend preset envelope. First entry is the official preset.
    let res = client
        .post(format!("{base}/api/invoke/get_provider_presets"))
        .header("x-cc-switch-token", TOKEN)
        .json(&json!({ "app": "claude" }))
        .send()
        .await
        .expect("get_provider_presets request");
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.expect("presets json");
    let presets = body.as_array().expect("presets is an array");
    assert_eq!(
        presets.len(),
        8,
        "claude presets = 9 templates minus Custom"
    );
    assert_eq!(presets[0]["name"], "Claude Official");
    assert_eq!(presets[0]["isOfficial"], json!(true));
    assert!(
        presets[0]["settingsConfig"]["env"].is_object(),
        "claude preset carries settingsConfig.env"
    );

    // Codex presets expose top-level auth/config (TOML), not settingsConfig.
    let res = client
        .post(format!("{base}/api/invoke/get_provider_presets"))
        .header("x-cc-switch-token", TOKEN)
        .json(&json!({ "app": "codex" }))
        .send()
        .await
        .expect("get_provider_presets codex request");
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.expect("codex presets json");
    let presets = body.as_array().expect("codex presets array");
    let deepseek = presets
        .iter()
        .find(|p| p["name"] == "DeepSeek")
        .expect("codex deepseek preset");
    assert!(
        deepseek.get("settingsConfig").is_none(),
        "codex presets must not nest settingsConfig"
    );
    assert!(deepseek["config"]
        .as_str()
        .is_some_and(|c| c.contains("deepseek")));
    assert!(deepseek["modelCatalog"].is_array());

    server.abort();
}
