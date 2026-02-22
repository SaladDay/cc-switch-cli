use base64::prelude::*;
use cc_switch_lib::{import_provider_from_deeplink, parse_deeplink_url, AppType, MultiAppConfig};

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs, state_from_config};

#[test]
fn deeplink_import_claude_provider_persists_to_config() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let url = "ccswitch://v1/import?resource=provider&app=claude&name=DeepLink%20Claude&homepage=https%3A%2F%2Fexample.com&endpoint=https%3A%2F%2Fapi.example.com%2Fv1&apiKey=sk-test-claude-key&model=claude-sonnet-4";
    let request = parse_deeplink_url(url).expect("parse deeplink url");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);

    let state = state_from_config(config);

    let provider_id = import_provider_from_deeplink(&state, request.clone())
        .expect("import provider from deeplink");

    // 验证内存状态
    let guard = state.config.read().expect("read config");
    let manager = guard
        .get_manager(&AppType::Claude)
        .expect("claude manager should exist");
    let provider = manager
        .providers
        .get(&provider_id)
        .expect("provider created via deeplink");
    assert_eq!(
        provider.name,
        request.name.clone().expect("request name"),
        "provider name should match deeplink"
    );
    assert_eq!(provider.website_url.as_deref(), request.homepage.as_deref());
    let auth_token = provider
        .settings_config
        .pointer("/env/ANTHROPIC_API_KEY")
        .and_then(|v| v.as_str());
    let base_url = provider
        .settings_config
        .pointer("/env/ANTHROPIC_BASE_URL")
        .and_then(|v| v.as_str());
    assert_eq!(auth_token, request.api_key.as_deref());
    assert_eq!(base_url, request.endpoint.as_deref());
    drop(guard);

    // 验证配置已持久化
    let persisted = state
        .db
        .get_provider_by_id(&provider_id, AppType::Claude.as_str())
        .expect("read provider from db");
    assert!(persisted.is_some(), "provider should be persisted to db");
}

#[test]
fn deeplink_import_codex_provider_builds_auth_and_config() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let url = "ccswitch://v1/import?resource=provider&app=codex&name=DeepLink%20Codex&homepage=https%3A%2F%2Fopenai.example&endpoint=https%3A%2F%2Fapi.openai.example%2Fv1&apiKey=sk-test-codex-key&model=gpt-4o";
    let request = parse_deeplink_url(url).expect("parse deeplink url");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);

    let state = state_from_config(config);

    let provider_id = import_provider_from_deeplink(&state, request.clone())
        .expect("import provider from deeplink");

    let guard = state.config.read().expect("read config");
    let manager = guard
        .get_manager(&AppType::Codex)
        .expect("codex manager should exist");
    let provider = manager
        .providers
        .get(&provider_id)
        .expect("provider created via deeplink");
    assert_eq!(
        provider.name,
        request.name.clone().expect("request name"),
        "provider name should match deeplink"
    );
    assert_eq!(provider.website_url.as_deref(), request.homepage.as_deref());
    let auth_value = provider
        .settings_config
        .pointer("/auth/OPENAI_API_KEY")
        .and_then(|v| v.as_str());
    let config_text = provider
        .settings_config
        .get("config")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert_eq!(auth_value, request.api_key.as_deref());
    assert!(
        request
            .endpoint
            .as_deref()
            .is_some_and(|endpoint| config_text.contains(endpoint)),
        "config.toml content should contain endpoint"
    );
    assert!(
        config_text.contains("model = \"gpt-4o\""),
        "config.toml content should contain model setting"
    );
    drop(guard);

    let persisted = state
        .db
        .get_provider_by_id(&provider_id, AppType::Codex.as_str())
        .expect("read provider from db");
    assert!(persisted.is_some(), "provider should be persisted to db");
}

#[test]
fn deeplink_import_rejects_non_http_endpoints_from_config() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    let config_json =
        r#"{"env":{"ANTHROPIC_AUTH_TOKEN":"sk-test","ANTHROPIC_BASE_URL":"ftp://example.com/v1"}}"#;
    let config_b64 = BASE64_URL_SAFE_NO_PAD.encode(config_json.as_bytes());

    let url = format!(
        "ccswitch://v1/import?resource=provider&app=claude&name=BadEndpoint&config={config_b64}&configFormat=json"
    );
    let request = parse_deeplink_url(&url).expect("parse deeplink url");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);

    let state = state_from_config(config);

    let err = import_provider_from_deeplink(&state, request)
        .expect_err("non-http endpoints should be rejected");
    assert!(
        err.to_string().contains("Invalid URL scheme"),
        "expected scheme validation error, got {err:?}"
    );
}
