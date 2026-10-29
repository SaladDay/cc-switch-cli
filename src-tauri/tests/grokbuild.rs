#![allow(clippy::await_holding_lock)]

#[path = "support.rs"]
mod support;

use cc_switch_lib::{
    AppType, Database, McpApps, McpServer, McpService, MultiAppConfig, Provider, ProviderService,
    ProxyService,
};
use serde_json::{json, Value};
use std::{fs, sync::Arc};
use support::{ensure_test_home, lock_test_mutex, reset_test_fs, state_from_config};

fn provider(id: &str, endpoint: &str) -> Provider {
    Provider::with_id(
        id.into(),
        id.into(),
        json!({"config": format!(
            "[models]\ndefault = \"{id}\"\n[model.\"{id}\"]\nmodel = \"grok-test\"\nname = \"Test\"\nbase_url = \"{endpoint}\"\napi_key = \"test-key\"\napi_backend = \"responses\"\ncontext_window = 500000\n"
        )}),
        None,
    )
}

#[test]
fn grok_aliases_and_switch_family() {
    for name in ["grokbuild", "grok", "grok-build", "grok_build", " GROK "] {
        assert_eq!(name.parse::<AppType>().unwrap(), AppType::GrokBuild);
    }
    assert_eq!(
        serde_json::to_string(&AppType::GrokBuild).unwrap(),
        "\"grokbuild\""
    );
    assert!(!AppType::GrokBuild.is_additive_mode());
    assert!(AppType::GrokBuild.supports_failover());
    assert!(AppType::all().any(|app| app == AppType::GrokBuild));
}

#[test]
fn grok_switch_preserves_globals_mcp_unrelated_models_and_native_auth() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".grok")).unwrap();
    let path = home.join(".grok/config.toml");
    let global = "# user settings\n[ui]\ncompact_mode = true\n[models]\ndefault_reasoning_effort = \"high\"\n[model.other]\nmodel = \"unrelated\"\n[mcp_servers.echo]\ncommand = \"echo\"\n";
    fs::write(&path, global).unwrap();
    let auth = b"native-oauth-must-never-be-touched";
    fs::write(home.join(".grok/auth.json"), auth).unwrap();
    let mut config = MultiAppConfig::default();
    let manager = config.get_manager_mut(&AppType::GrokBuild).unwrap();
    for id in ["a", "b"] {
        manager
            .providers
            .insert(id.into(), provider(id, "https://example.test/v1"));
    }
    let mut official = Provider::with_id(
        "official".into(),
        "Official".into(),
        json!({"config":""}),
        None,
    );
    official.category = Some("official".into());
    manager.providers.insert(official.id.clone(), official);
    let state = state_from_config(config);
    ProviderService::switch(&state, AppType::GrokBuild, "a").unwrap();
    let live = fs::read_to_string(&path).unwrap();
    assert!(live.contains("# user settings"));
    let mut doc: toml::Value = live.parse().unwrap();
    assert_eq!(doc["models"]["default"].as_str(), Some("a"));
    assert_eq!(doc["ui"]["compact_mode"].as_bool(), Some(true));
    assert_eq!(doc["mcp_servers"]["echo"]["command"].as_str(), Some("echo"));
    // Grok can change the selected model itself. Cleanup must still remove our old table.
    doc["models"]["default"] = toml::Value::String("builtin".into());
    fs::write(&path, toml::to_string(&doc).unwrap()).unwrap();
    ProviderService::switch(&state, AppType::GrokBuild, "b").unwrap();
    let doc: toml::Value = fs::read_to_string(&path).unwrap().parse().unwrap();
    assert!(doc["model"].get("a").is_none());
    assert_eq!(doc["models"]["default"].as_str(), Some("b"));
    ProviderService::switch(&state, AppType::GrokBuild, "official").unwrap();
    let doc: toml::Value = fs::read_to_string(&path).unwrap().parse().unwrap();
    assert!(doc["models"].get("default").is_none());
    assert!(doc["model"].get("b").is_none());
    assert!(doc["model"].get("other").is_some());
    assert_eq!(
        doc["models"]["default_reasoning_effort"].as_str(),
        Some("high")
    );
    assert_eq!(fs::read(home.join(".grok/auth.json")).unwrap(), auth);
    assert_eq!(
        cc_switch_lib::AppSettings::load()
            .current_provider_grokbuild
            .as_deref(),
        Some("official")
    );
}

#[test]
fn grok_switch_refuses_to_overwrite_user_table_with_same_name() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".grok")).unwrap();
    let path = home.join(".grok/config.toml");
    // 用户自己写的、未被选中的同名表（官方登录态，无 models.default）。
    let user = "[model.a]\nmodel = \"user-own\"\nname = \"Mine\"\nbase_url = \"https://mine.test/v1\"\napi_key = \"mine\"\napi_backend = \"responses\"\ncontext_window = 1000\n";
    fs::write(&path, user).unwrap();
    let mut config = MultiAppConfig::default();
    let manager = config.get_manager_mut(&AppType::GrokBuild).unwrap();
    manager
        .providers
        .insert("a".into(), provider("a", "https://example.test/v1"));
    let state = state_from_config(config);
    let err = ProviderService::switch(&state, AppType::GrokBuild, "a").unwrap_err();
    assert!(err.to_string().contains("\"a\""), "{err}");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        user,
        "user table must stay intact"
    );
}

#[test]
fn grok_import_treats_unselected_user_tables_as_native_official_login() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".grok")).unwrap();
    fs::write(
        home.join(".grok/config.toml"),
        "[ui]\ncompact_mode = true\n[model.\"grok-4.7\"]\nmodel = \"mine\"\n",
    )
    .unwrap();
    let state = state_from_config(MultiAppConfig::default());
    ProviderService::import_default_config(&state, AppType::GrokBuild).unwrap();
    let imported = state
        .db
        .get_provider_by_id("default", "grokbuild")
        .unwrap()
        .unwrap();
    assert_eq!(imported.category.as_deref(), Some("official"));
}

#[tokio::test]
async fn grok_routing_rejects_unresolvable_env_key_before_takeover() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".grok")).unwrap();
    std::env::remove_var("CC_SWITCH_GROK_TEST_UNSET_KEY");
    let mut config = MultiAppConfig::default();
    let manager = config.get_manager_mut(&AppType::GrokBuild).unwrap();
    let mut env_only = provider("a", "https://example.test/v1");
    let text = env_only.settings_config["config"]
        .as_str()
        .unwrap()
        .replace(
            "api_key = \"test-key\"",
            "env_key = \"CC_SWITCH_GROK_TEST_UNSET_KEY\"",
        );
    env_only.settings_config = json!({ "config": text });
    manager.providers.insert("a".into(), env_only);
    let state = state_from_config(config);
    ProviderService::switch(&state, AppType::GrokBuild, "a").unwrap();
    state
        .db
        .set_app_proxy_preferred_port("grokbuild", 15992)
        .unwrap();
    let err = state
        .proxy_service
        .set_takeover_for_app("grokbuild", true)
        .await
        .unwrap_err();
    assert!(err.contains("API key"), "{err}");
    assert!(state
        .db
        .get_live_backup("grokbuild")
        .await
        .unwrap()
        .is_none());
}

#[test]
fn grok_invalid_live_document_never_overwritten() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".grok")).unwrap();
    let path = home.join(".grok/config.toml");
    fs::write(&path, "invalid = [").unwrap();
    let mut config = MultiAppConfig::default();
    config
        .get_manager_mut(&AppType::GrokBuild)
        .unwrap()
        .providers
        .insert("a".into(), provider("a", "https://example.test/v1"));
    let state = state_from_config(config);
    assert!(ProviderService::switch(&state, AppType::GrokBuild, "a").is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "invalid = [");
}

#[test]
fn grok_mcp_flags_roundtrip_and_projection_uses_native_headers() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".grok")).unwrap();
    let state = state_from_config(MultiAppConfig::default());
    let server = McpServer {
        id: "remote".into(),
        name: "Remote".into(),
        server: json!({"type":"http", "url":"https://example.test/mcp", "headers":{"Authorization":"test"}}),
        apps: McpApps {
            grokbuild: true,
            ..Default::default()
        },
        description: None,
        homepage: None,
        docs: None,
        tags: vec![],
    };
    McpService::upsert_server(&state, server).unwrap();
    assert!(
        state.db.get_all_mcp_servers().unwrap()["remote"]
            .apps
            .grokbuild
    );
    let path = home.join(".grok/config.toml");
    let doc: toml::Value = fs::read_to_string(&path).unwrap().parse().unwrap();
    assert_eq!(
        doc["mcp_servers"]["remote"]["headers"]["Authorization"].as_str(),
        Some("test")
    );
    assert!(doc["mcp_servers"]["remote"].get("type").is_none());
    McpService::toggle_app(&state, "remote", AppType::GrokBuild, false).unwrap();
    let doc: toml::Value = fs::read_to_string(&path).unwrap().parse().unwrap();
    assert!(doc
        .get("mcp_servers")
        .and_then(|v| v.get("remote"))
        .is_none());
}

#[tokio::test]
async fn grok_responses_route_uses_grok_provider_and_credentials() {
    use axum::{http::HeaderMap, routing::post, Json, Router};
    let _guard = lock_test_mutex();
    reset_test_fs();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    let app = Router::new().route("/v1/responses", post(|headers: HeaderMap, Json(body): Json<Value>| async move {
        assert_eq!(headers["authorization"], "Bearer test-key");
        assert_eq!(body["model"], "grok-test");
        Json(json!({"id":"resp_grok", "object":"response", "status":"completed", "output":[{"type":"message", "role":"assistant", "content":[{"type":"output_text", "text":"grok ok"}]}]}))
    }));
    let upstream = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let db = Arc::new(Database::memory().unwrap());
    db.save_provider("grokbuild", &provider("a", &endpoint))
        .unwrap();
    db.set_current_provider("grokbuild", "a").unwrap();
    let service = ProxyService::new(db);
    let mut runtime = service.get_config().await.unwrap();
    runtime.listen_address = "127.0.0.1".into();
    runtime.listen_port = 0;
    let info = service.start_with_runtime_config(runtime).await.unwrap();
    let response = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/grokbuild/v1/responses",
            info.port
        ))
        .json(&json!({"model":"grok-test", "input":"Hello", "stream":false}))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    service.stop().await.unwrap();
    upstream.abort();
    assert!(status.is_success(), "{status}: {body}");
    assert!(body.contains("grok ok"), "{body}");
}

#[tokio::test]
async fn grok_takeover_hot_switch_restore_preserves_live_preferences() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".grok")).unwrap();
    let path = home.join(".grok/config.toml");
    let mut config = MultiAppConfig::default();
    let manager = config.get_manager_mut(&AppType::GrokBuild).unwrap();
    for id in ["a", "b"] {
        manager
            .providers
            .insert(id.into(), provider(id, "https://example.test/v1"));
    }
    let state = state_from_config(config);
    ProviderService::switch(&state, AppType::GrokBuild, "a").unwrap();
    let mut live = fs::read_to_string(&path).unwrap();
    live.push_str("\n[ui]\ncompact_mode = true\n");
    fs::write(&path, live).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    state
        .db
        .set_app_proxy_preferred_port("grokbuild", port)
        .unwrap();
    state
        .proxy_service
        .set_takeover_for_app("grokbuild", true)
        .await
        .unwrap();
    let doc: toml::Value = fs::read_to_string(&path).unwrap().parse().unwrap();
    assert_eq!(doc["model"]["a"]["api_key"].as_str(), Some("PROXY_MANAGED"));
    assert!(doc["model"]["a"]["base_url"]
        .as_str()
        .unwrap()
        .ends_with("/grokbuild/v1"));
    ProviderService::switch(&state, AppType::GrokBuild, "b").unwrap();
    state
        .proxy_service
        .set_takeover_for_app("grokbuild", false)
        .await
        .unwrap();
    let doc: toml::Value = fs::read_to_string(&path).unwrap().parse().unwrap();
    assert_eq!(doc["models"]["default"].as_str(), Some("b"));
    assert_eq!(doc["model"]["b"]["api_key"].as_str(), Some("test-key"));
    assert_eq!(doc["ui"]["compact_mode"].as_bool(), Some(true));
    assert!(doc["model"].get("a").is_none());
}

#[test]
fn grok_deeplink_is_accepted_at_public_parser() {
    let request = cc_switch_lib::parse_deeplink_url("ccswitch://v1/import?resource=provider&app=grokbuild&name=Test&endpoint=https%3A%2F%2Fexample.test%2Fv1&apiKey=test-key&model=grok-test").unwrap();
    assert_eq!(request.app.as_deref(), Some("grokbuild"));
}

#[tokio::test]
async fn grok_official_takeover_is_refused_without_leaving_state() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".grok")).unwrap();
    let mut config = MultiAppConfig::default();
    let manager = config.get_manager_mut(&AppType::GrokBuild).unwrap();
    let mut official = Provider::with_id(
        "official".into(),
        "Grok Official".into(),
        json!({"config":""}),
        None,
    );
    official.category = Some("official".into());
    manager.providers.insert(official.id.clone(), official);
    let state = state_from_config(config);
    ProviderService::switch(&state, AppType::GrokBuild, "official").unwrap();
    state
        .db
        .set_app_proxy_preferred_port("grokbuild", 15991)
        .unwrap();

    let err = state
        .proxy_service
        .set_takeover_for_app("grokbuild", true)
        .await
        .unwrap_err();
    assert!(err.contains("official") || err.contains("Grok"), "{err}");
    assert!(
        state
            .db
            .get_live_backup("grokbuild")
            .await
            .unwrap()
            .is_none(),
        "refused takeover must not persist a live backup"
    );
    assert!(
        !state
            .db
            .get_proxy_config_for_app("grokbuild")
            .await
            .unwrap()
            .enabled
    );
}

#[tokio::test]
async fn grok_hot_switch_to_official_under_takeover_is_atomic() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".grok")).unwrap();
    let path = home.join(".grok/config.toml");
    let mut config = MultiAppConfig::default();
    let manager = config.get_manager_mut(&AppType::GrokBuild).unwrap();
    for id in ["a", "b"] {
        manager
            .providers
            .insert(id.into(), provider(id, "https://example.test/v1"));
    }
    let mut official = Provider::with_id(
        "official".into(),
        "Grok Official".into(),
        json!({"config":""}),
        None,
    );
    official.category = Some("official".into());
    manager.providers.insert(official.id.clone(), official);
    let state = state_from_config(config);
    ProviderService::switch(&state, AppType::GrokBuild, "a").unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    state
        .db
        .set_app_proxy_preferred_port("grokbuild", port)
        .unwrap();
    state
        .proxy_service
        .set_takeover_for_app("grokbuild", true)
        .await
        .unwrap();

    let err = ProviderService::switch(&state, AppType::GrokBuild, "official").unwrap_err();
    assert!(
        err.to_string().contains("official") || err.to_string().contains("Grok"),
        "{err}"
    );
    assert_eq!(
        state
            .db
            .get_current_provider("grokbuild")
            .unwrap()
            .as_deref(),
        Some("a"),
        "pointer must not move when the hot switch is refused"
    );
    let doc: toml::Value = fs::read_to_string(&path).unwrap().parse().unwrap();
    assert_eq!(doc["models"]["default"].as_str(), Some("a"));
    assert_eq!(doc["model"]["a"]["api_key"].as_str(), Some("PROXY_MANAGED"));
    state
        .proxy_service
        .set_takeover_for_app("grokbuild", false)
        .await
        .unwrap();
}

#[tokio::test]
async fn grok_stale_takeover_recovery_removes_managed_profile_without_credentials() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".grok")).unwrap();
    let path = home.join(".grok/config.toml");
    let mut config = MultiAppConfig::default();
    config
        .get_manager_mut(&AppType::GrokBuild)
        .unwrap()
        .providers
        .insert("a".into(), provider("a", "https://example.test/v1"));
    let state = state_from_config(config);
    ProviderService::switch(&state, AppType::GrokBuild, "a").unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    state
        .db
        .set_app_proxy_preferred_port("grokbuild", port)
        .unwrap();
    state
        .proxy_service
        .set_takeover_for_app("grokbuild", true)
        .await
        .unwrap();
    // Simulate the stale case: provider row and backup are gone, live still points at the proxy.
    state.db.delete_provider("grokbuild", "a").unwrap();
    state.db.delete_live_backup("grokbuild").await.unwrap();

    state
        .proxy_service
        .set_takeover_for_app("grokbuild", false)
        .await
        .unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("PROXY_MANAGED"), "{text}");
    let doc: toml::Value = text.parse().unwrap();
    assert!(doc.get("models").and_then(|m| m.get("default")).is_none());
    assert!(doc.get("ui").is_none(), "user sections must survive: {doc}");
}

#[tokio::test]
async fn grok_deeplink_import_preserves_profile_protocol_and_options() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let state = state_from_config(MultiAppConfig::default());
    let config_toml = "[models]\ndefault = \"my-profile\"\n\n[model.\"my-profile\"]\nmodel = \"grok-4.5\"\nname = \"Linked\"\nbase_url = \"https://example.test/v1\"\napi_key = \"linked-key\"\napi_backend = \"messages\"\ncontext_window = 128000\n";
    let url = format!(
        "ccswitch://v1/import?resource=provider&app=grokbuild&name=Linked&homepage=https%3A%2F%2Fexample.test&endpoint=https%3A%2F%2Fexample.test%2Fv1&apiKey=linked-key&model=grok-4.5&configFormat=toml&config={}",
        base64_url_safe(config_toml)
    );
    let request = cc_switch_lib::parse_deeplink_url(&url).unwrap();
    let provider_id = cc_switch_lib::import_provider_from_deeplink(&state, request).unwrap();
    let saved = state
        .db
        .get_provider_by_id(&provider_id, "grokbuild")
        .unwrap()
        .unwrap();
    let text = saved.settings_config["config"].as_str().unwrap();
    assert!(text.contains("api_backend = \"messages\""), "{text}");
    assert!(text.contains("context_window = 128000"), "{text}");
    assert!(text.contains("default = \"my-profile\""), "{text}");
}

#[tokio::test]
async fn grok_deeplink_url_overrides_replace_untrusted_env_key_reference() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let state = state_from_config(MultiAppConfig::default());
    let config_toml = "[models]\ndefault = \"p\"\n\n[model.p]\nmodel = \"from-link\"\nname = \"Linked\"\nbase_url = \"https://attacker.test/v1\"\nenv_key = \"SOME_SECRET_VAR\"\napi_backend = \"responses\"\ncontext_window = 500000\n";
    let url = format!(
        "ccswitch://v1/import?resource=provider&app=grokbuild&name=Linked&endpoint=https%3A%2F%2Fapi.example.test%2Fv1&apiKey=url-key&model=grok-4.7&configFormat=toml&config={}",
        base64_url_safe(config_toml)
    );
    let request = cc_switch_lib::parse_deeplink_url(&url).unwrap();
    let id = cc_switch_lib::import_provider_from_deeplink(&state, request).unwrap();
    let saved = state
        .db
        .get_provider_by_id(&id, "grokbuild")
        .unwrap()
        .unwrap();
    let text = saved.settings_config["config"].as_str().unwrap();
    let doc: toml::Value = text.parse().unwrap();
    let profile = &doc["model"]["p"];
    assert!(profile.get("env_key").is_none(), "{text}");
    assert_eq!(profile["api_key"].as_str(), Some("url-key"));
    assert_eq!(
        profile["base_url"].as_str(),
        Some("https://api.example.test/v1")
    );
    assert_eq!(profile["model"].as_str(), Some("grok-4.7"));
    assert_eq!(profile["api_backend"].as_str(), Some("responses"));
}

#[tokio::test]
async fn grok_chat_completions_profile_is_converted_while_routed() {
    use axum::{routing::post, Json, Router};
    let _guard = lock_test_mutex();
    reset_test_fs();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    let app = Router::new().route("/v1/chat/completions", post(|Json(body): Json<Value>| async move {
        assert!(body.get("messages").is_some(), "expected Chat body: {body}");
        assert!(body.get("input").is_none(), "Responses body leaked upstream: {body}");
        Json(json!({"id":"chat_1","object":"chat.completion","created":1,"model":"grok-test",
            "choices":[{"index":0,"message":{"role":"assistant","content":"chat ok"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}))
    }));
    let upstream = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let db = Arc::new(Database::memory().unwrap());
    let mut chat = provider("a", &endpoint);
    let text = chat.settings_config["config"].as_str().unwrap().replace(
        "api_backend = \"responses\"",
        "api_backend = \"chat_completions\"",
    );
    chat.settings_config = json!({ "config": text });
    db.save_provider("grokbuild", &chat).unwrap();
    db.set_current_provider("grokbuild", "a").unwrap();
    let service = ProxyService::new(db);
    let mut runtime = service.get_config().await.unwrap();
    runtime.listen_address = "127.0.0.1".into();
    runtime.listen_port = 0;
    let info = service.start_with_runtime_config(runtime).await.unwrap();
    let response = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/grokbuild/v1/responses",
            info.port
        ))
        .json(&json!({"model":"grok-test", "input":"Hello", "stream":false}))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    service.stop().await.unwrap();
    upstream.abort();
    assert!(status.is_success(), "{status}: {body}");
    assert!(body.contains("chat ok"), "{body}");
}

#[test]
fn grok_first_switch_keeps_unowned_selected_table() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".grok")).unwrap();
    let path = home.join(".grok/config.toml");
    // Selected custom table lacks name/api_backend/context_window on purpose:
    // profile discovery must not depend on optional fields.
    fs::write(&path, "[models]\ndefault = \"legacy\"\n\n[model.legacy]\nmodel = \"old\"\nbase_url = \"https://old.test/v1\"\n").unwrap();
    let mut config = MultiAppConfig::default();
    let mut official = Provider::with_id(
        "official".into(),
        "Grok Official".into(),
        json!({"config":""}),
        None,
    );
    official.category = Some("official".into());
    config
        .get_manager_mut(&AppType::GrokBuild)
        .unwrap()
        .providers
        .insert(official.id.clone(), official);
    let state = state_from_config(config);
    ProviderService::switch(&state, AppType::GrokBuild, "official").unwrap();
    let doc: toml::Value = fs::read_to_string(&path).unwrap().parse().unwrap();
    // 不是 CC-Switch 写的表、数据库里也没有副本：只取消选中，绝不删除。
    assert_eq!(
        doc["model"]["legacy"]["model"].as_str(),
        Some("old"),
        "{doc}"
    );
    assert!(doc.get("models").and_then(|m| m.get("default")).is_none());
}

fn base64_url_safe(input: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE.encode(input)
}
