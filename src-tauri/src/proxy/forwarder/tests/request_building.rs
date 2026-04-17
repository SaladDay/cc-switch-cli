use std::{env, ffi::OsString, sync::atomic::Ordering, time::Duration};

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use serde_json::json;

use super::{
    bedrock_claude_provider, claude_provider, claude_request_body, spawn_scripted_upstream,
    test_router,
};
use crate::{
    app_config::AppType,
    provider::{AuthBinding, AuthBindingSource, Provider, ProviderMeta},
    proxy::{
        forwarder::{ForwardOptions, RequestForwarder},
        types::{OptimizerConfig, RectifierConfig},
    },
    services::{CodexOAuthService, GitHubCopilotOAuthService},
    test_support::lock_test_home_and_settings,
};

struct ConfigDirEnvGuard {
    original: Option<OsString>,
}

impl ConfigDirEnvGuard {
    fn set(value: Option<&str>) -> Self {
        let original = env::var_os("CC_SWITCH_CONFIG_DIR");
        match value {
            Some(value) => unsafe { env::set_var("CC_SWITCH_CONFIG_DIR", value) },
            None => unsafe { env::remove_var("CC_SWITCH_CONFIG_DIR") },
        }
        Self { original }
    }
}

impl Drop for ConfigDirEnvGuard {
    fn drop(&mut self) {
        match self.original.as_ref() {
            Some(value) => unsafe { env::set_var("CC_SWITCH_CONFIG_DIR", value) },
            None => unsafe { env::remove_var("CC_SWITCH_CONFIG_DIR") },
        }
    }
}

#[tokio::test]
async fn bedrock_claude_prepare_request_injects_optimizer_and_cache_breakpoints() {
    let (base_url, hits, bodies, server) =
        spawn_scripted_upstream(vec![(StatusCode::OK, json!({"ok": true}))]).await;
    let provider = bedrock_claude_provider("p1", &base_url);
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router)
        .expect("create forwarder")
        .with_optimizer_config(OptimizerConfig {
            enabled: true,
            thinking_optimizer: true,
            cache_injection: true,
            cache_ttl: "5m".to_string(),
        });

    let body = json!({
        "model": "anthropic.claude-sonnet-4-5-20250514-v1:0",
        "max_tokens": 32,
        "tools": [{"name": "tool_a"}],
        "system": [{"type": "text", "text": "sys"}],
        "messages": [{
            "role": "assistant",
            "content": [{"type": "text", "text": "hello"}]
        }]
    });

    let response = forwarder
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
        .expect("bedrock claude request should succeed");

    assert_eq!(response.response.status, StatusCode::OK);
    assert_eq!(hits.count.load(Ordering::SeqCst), 1);

    let sent = bodies.lock().await;
    let sent = sent.first().expect("captured upstream request body");
    assert_eq!(sent["thinking"]["type"], "enabled");
    assert_eq!(sent["thinking"]["budget_tokens"], 31);
    assert!(sent["tools"][0].get("cache_control").is_some());
    assert!(sent["system"][0].get("cache_control").is_some());
    assert!(sent["messages"][0]["content"][0]
        .get("cache_control")
        .is_some());

    server.abort();
}

#[tokio::test]
async fn non_bedrock_claude_prepare_request_skips_optimizer_and_cache_injection() {
    let (base_url, hits, bodies, server) =
        spawn_scripted_upstream(vec![(StatusCode::OK, json!({"ok": true}))]).await;
    let provider = claude_provider("p1", &base_url, None);
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router)
        .expect("create forwarder")
        .with_optimizer_config(OptimizerConfig {
            enabled: true,
            thinking_optimizer: true,
            cache_injection: true,
            cache_ttl: "5m".to_string(),
        });

    let body = json!({
        "model": "anthropic.claude-sonnet-4-5-20250514-v1:0",
        "max_tokens": 32,
        "tools": [{"name": "tool_a"}],
        "system": [{"type": "text", "text": "sys"}],
        "messages": [{
            "role": "assistant",
            "content": [{"type": "text", "text": "hello"}]
        }]
    });

    let response = forwarder
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
        .expect("regular claude request should succeed");

    assert_eq!(response.response.status, StatusCode::OK);
    assert_eq!(hits.count.load(Ordering::SeqCst), 1);

    let sent = bodies.lock().await;
    let sent = sent.first().expect("captured upstream request body");
    assert!(sent.get("thinking").is_none());
    assert!(sent["tools"][0].get("cache_control").is_none());
    assert!(sent["system"][0].get("cache_control").is_none());
    assert!(sent["messages"][0]["content"][0]
        .get("cache_control")
        .is_none());

    server.abort();
}

#[tokio::test]
async fn claude_prepare_request_appends_claude_code_beta_to_existing_header() {
    let mut headers = HeaderMap::new();
    headers.insert("anthropic-beta", HeaderValue::from_static("existing-beta"));

    let request = build_request(
        &AppType::Claude,
        &claude_provider("p1", "https://example.com", None),
        headers,
    )
    .await;

    assert_eq!(
        request
            .headers()
            .get("anthropic-beta")
            .and_then(|value| value.to_str().ok()),
        Some("claude-code-20250219,existing-beta")
    );
}

#[tokio::test]
async fn claude_prepare_request_sets_defaults_and_filters_blocked_caller_headers() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_static("Bearer caller-token"),
    );
    headers.insert("x-api-key", HeaderValue::from_static("caller-api-key"));
    headers.insert(
        "x-goog-api-key",
        HeaderValue::from_static("caller-goog-key"),
    );
    headers.insert("accept-encoding", HeaderValue::from_static("gzip"));
    headers.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.10"));
    headers.insert("x-real-ip", HeaderValue::from_static("203.0.113.11"));

    let request = build_request(
        &AppType::Claude,
        &claude_provider("p1", "https://example.com", None),
        headers,
    )
    .await;

    assert_eq!(
        header_value(&request, "anthropic-beta"),
        Some("claude-code-20250219")
    );
    assert_eq!(
        header_value(&request, "anthropic-version"),
        Some("2023-06-01")
    );
    assert_eq!(header_value(&request, "accept-encoding"), Some("identity"));
    assert_eq!(
        header_value(&request, "authorization"),
        Some("Bearer key-p1")
    );
    assert_eq!(header_value(&request, "x-api-key"), Some("key-p1"));
    assert_eq!(header_value(&request, "x-goog-api-key"), None);
    assert_eq!(
        header_value(&request, "x-forwarded-for"),
        Some("203.0.113.10")
    );
    assert_eq!(header_value(&request, "x-real-ip"), Some("203.0.113.11"));
}

#[tokio::test]
async fn non_claude_prepare_request_skips_claude_specific_headers() {
    let request = build_request(
        &AppType::Codex,
        &codex_provider("https://example.com"),
        HeaderMap::new(),
    )
    .await;

    assert_eq!(header_value(&request, "anthropic-beta"), None);
    assert_eq!(header_value(&request, "anthropic-version"), None);
    assert_eq!(
        header_value(&request, "authorization"),
        Some("Bearer codex-key")
    );
}

#[tokio::test]
async fn codex_oauth_prepare_request_injects_bound_account_headers() {
    let _lock = lock_test_home_and_settings();
    let temp = tempfile::tempdir().expect("create temp dir");
    let _guard = ConfigDirEnvGuard::set(Some(temp.path().to_string_lossy().as_ref()));
    CodexOAuthService::reset_for_tests();
    CodexOAuthService::seed_account_for_tests(
        "acc-bound",
        "rt-bound",
        Some("bound@example.com"),
        Some("at-bound"),
        None,
    )
    .await
    .expect("seed bound account");

    let provider = codex_oauth_provider(Some("acc-bound"));
    let request = build_request(&AppType::Claude, &provider, HeaderMap::new()).await;

    assert_eq!(
        request.url().as_str(),
        "https://chatgpt.com/backend-api/codex/responses"
    );
    assert_eq!(
        header_value(&request, "authorization"),
        Some("Bearer at-bound")
    );
    assert_eq!(
        header_value(&request, "chatgpt-account-id"),
        Some("acc-bound")
    );
    assert_eq!(header_value(&request, "originator"), Some("cc-switch"));
}

#[tokio::test]
async fn codex_oauth_prepare_request_falls_back_to_default_account() {
    let _lock = lock_test_home_and_settings();
    let temp = tempfile::tempdir().expect("create temp dir");
    let _guard = ConfigDirEnvGuard::set(Some(temp.path().to_string_lossy().as_ref()));
    CodexOAuthService::reset_for_tests();
    CodexOAuthService::seed_account_for_tests(
        "acc-default",
        "rt-default",
        Some("default@example.com"),
        Some("at-default"),
        None,
    )
    .await
    .expect("seed default account");

    let provider = codex_oauth_provider(None);
    let request = build_request(&AppType::Claude, &provider, HeaderMap::new()).await;

    assert_eq!(
        header_value(&request, "authorization"),
        Some("Bearer at-default")
    );
    assert_eq!(
        header_value(&request, "chatgpt-account-id"),
        Some("acc-default")
    );
}

#[tokio::test]
async fn codex_oauth_prepare_request_errors_without_available_account() {
    let _lock = lock_test_home_and_settings();
    let temp = tempfile::tempdir().expect("create temp dir");
    let _guard = ConfigDirEnvGuard::set(Some(temp.path().to_string_lossy().as_ref()));
    CodexOAuthService::reset_for_tests();

    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");
    let provider = codex_oauth_provider(None);

    let error = forwarder
        .prepare_request(
            &AppType::Claude,
            &provider,
            "/v1/messages",
            &claude_request_body(),
            &HeaderMap::new(),
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect_err("prepare request should fail without codex oauth account");

    assert!(
        error.to_string().contains("Codex OAuth 认证失败"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn github_copilot_prepare_request_uses_managed_account_session_token() {
    let _lock = lock_test_home_and_settings();
    let temp = tempfile::tempdir().expect("create temp dir");
    let _guard = ConfigDirEnvGuard::set(Some(temp.path().to_string_lossy().as_ref()));
    GitHubCopilotOAuthService::reset_for_tests();

    let auth_path = temp.path().join("copilot_auth.json");
    std::fs::write(
        &auth_path,
        serde_json::to_vec_pretty(&json!({
            "version": 3,
            "accounts": {
                "acc-gh": {
                    "github_token": "ghu-test-token",
                    "authenticated_at": 123
                }
            },
            "default_account_id": "acc-gh"
        }))
        .expect("serialize copilot auth store"),
    )
    .expect("write copilot auth store");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind copilot token endpoint");
    let address = listener.local_addr().expect("copilot token endpoint addr");
    let server = tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/copilot_internal/v2/token",
            axum::routing::get(|| async {
                axum::Json(json!({
                    "token": "copilot-session-token",
                    "expires_at": 4102444800i64
                }))
            }),
        );
        let _ = axum::serve(listener, app).await;
    });
    let token_url = format!("http://{address}/copilot_internal/v2/token");
    let _token_guard = CopilotTokenUrlEnvGuard::set(Some(token_url.as_str()));

    let request = build_request(&AppType::Claude, &github_copilot_provider(None), HeaderMap::new())
        .await;

    assert_eq!(
        header_value(&request, "authorization"),
        Some("Bearer copilot-session-token")
    );
    assert_eq!(header_value(&request, "editor-version"), Some("vscode/1.85.0"));
    assert_eq!(
        header_value(&request, "editor-plugin-version"),
        Some("copilot/1.150.0")
    );
    assert_eq!(
        header_value(&request, "copilot-integration-id"),
        Some("vscode-chat")
    );

    server.abort();
}

async fn build_request(
    app_type: &AppType,
    provider: &Provider,
    headers: HeaderMap,
) -> reqwest::Request {
    let (_db, router) = test_router().await;
    let forwarder = RequestForwarder::new(router).expect("create forwarder");

    forwarder
        .prepare_request(
            app_type,
            provider,
            "/v1/messages",
            &claude_request_body(),
            &headers,
            ForwardOptions {
                max_retries: 0,
                request_timeout: Some(Duration::from_secs(2)),
                bypass_circuit_breaker: true,
            },
        )
        .await
        .expect("prepare request")
        .build()
        .expect("build request")
}

fn codex_provider(base_url: &str) -> Provider {
    Provider::with_id(
        "codex".to_string(),
        "Codex Provider".to_string(),
        json!({
            "base_url": base_url,
            "apiKey": "codex-key",
        }),
        None,
    )
}

fn codex_oauth_provider(account_id: Option<&str>) -> Provider {
    Provider {
        id: "codex-oauth".to_string(),
        name: "Codex OAuth".to_string(),
        settings_config: json!({
            "base_url": "https://ignored.example.com",
            "apiKey": "ignored-placeholder"
        }),
        website_url: None,
        category: None,
        created_at: None,
        sort_index: None,
        notes: None,
        meta: Some(ProviderMeta {
            provider_type: Some("codex_oauth".to_string()),
            auth_binding: Some(AuthBinding {
                source: AuthBindingSource::ManagedAccount,
                auth_provider: Some("codex_oauth".to_string()),
                account_id: account_id.map(str::to_string),
            }),
            ..Default::default()
        }),
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    }
}

struct CopilotTokenUrlEnvGuard {
    original: Option<OsString>,
}

impl CopilotTokenUrlEnvGuard {
    fn set(value: Option<&str>) -> Self {
        let original = env::var_os("CC_SWITCH_GITHUB_COPILOT_TOKEN_URL");
        match value {
            Some(value) => unsafe { env::set_var("CC_SWITCH_GITHUB_COPILOT_TOKEN_URL", value) },
            None => unsafe { env::remove_var("CC_SWITCH_GITHUB_COPILOT_TOKEN_URL") },
        }
        Self { original }
    }
}

impl Drop for CopilotTokenUrlEnvGuard {
    fn drop(&mut self) {
        match self.original.as_ref() {
            Some(value) => unsafe { env::set_var("CC_SWITCH_GITHUB_COPILOT_TOKEN_URL", value) },
            None => unsafe { env::remove_var("CC_SWITCH_GITHUB_COPILOT_TOKEN_URL") },
        }
    }
}

fn github_copilot_provider(account_id: Option<&str>) -> Provider {
    Provider {
        id: "github-copilot".to_string(),
        name: "GitHub Copilot".to_string(),
        settings_config: json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://api.githubcopilot.com"
            }
        }),
        website_url: None,
        category: None,
        created_at: None,
        sort_index: None,
        notes: None,
        meta: Some(ProviderMeta {
            provider_type: Some("github_copilot".to_string()),
            auth_binding: Some(AuthBinding {
                source: AuthBindingSource::ManagedAccount,
                auth_provider: Some("github_copilot".to_string()),
                account_id: account_id.map(str::to_string),
            }),
            ..Default::default()
        }),
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    }
}

fn header_value<'a>(request: &'a reqwest::Request, name: &str) -> Option<&'a str> {
    request
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
}
