use cc_switch_lib::{
    import_provider_from_deeplink, AppType, DeepLinkImportRequest, MultiAppConfig,
};

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs, state_from_config};

#[test]
fn openclaw_deeplink_import_adds_and_switches_provider_with_default_model() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let state = state_from_config(MultiAppConfig::default());
    let request = DeepLinkImportRequest {
        version: "v1".to_string(),
        resource: "provider".to_string(),
        app: Some("openclaw".to_string()),
        name: Some("OpenClaw Demo".to_string()),
        enabled: Some(true),
        homepage: Some("https://provider.example".to_string()),
        endpoint: Some("https://api.example.com/v1".to_string()),
        api_key: Some("sk-openclaw".to_string()),
        icon: None,
        model: None,
        notes: Some("imported from deeplink".to_string()),
        haiku_model: None,
        sonnet_model: None,
        opus_model: None,
        content: None,
        description: None,
        apps: None,
        repo: None,
        directory: None,
        branch: None,
        config: None,
        config_format: None,
        config_url: None,
        usage_enabled: None,
        usage_script: None,
        usage_api_key: None,
        usage_base_url: None,
        usage_access_token: None,
        usage_user_id: None,
        usage_auto_interval: None,
    };

    let provider_id =
        import_provider_from_deeplink(&state, request).expect("deeplink import should succeed");

    let manager = {
        let config = state.config.read().expect("read state config");
        config
            .get_manager(&AppType::OpenClaw)
            .expect("openclaw manager should exist")
            .clone()
    };
    let expected_primary = format!("{provider_id}/gpt-5.2-codex");

    assert_eq!(manager.current, provider_id);
    let provider = manager
        .providers
        .get(&provider_id)
        .expect("imported provider should exist in store");
    assert_eq!(
        provider
            .settings_config
            .get("primaryModel")
            .and_then(|value| value.as_str()),
        Some(expected_primary.as_str())
    );
    assert_eq!(
        provider
            .settings_config
            .get("provider")
            .and_then(|value| value.get("apiKey"))
            .and_then(|value| value.as_str()),
        Some("sk-openclaw")
    );

    let openclaw_path = home.join(".openclaw").join("openclaw.json");
    let live: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&openclaw_path).expect("read openclaw live config"),
    )
    .expect("parse openclaw live config");

    assert_eq!(
        live.get("agents")
            .and_then(|value| value.get("defaults"))
            .and_then(|value| value.get("model"))
            .and_then(|value| value.get("primary"))
            .and_then(|value| value.as_str()),
        Some(expected_primary.as_str())
    );
    assert_eq!(
        live.get("models")
            .and_then(|value| value.get("providers"))
            .and_then(|value| value.get(&provider_id))
            .and_then(|value| value.get("baseUrl"))
            .and_then(|value| value.as_str()),
        Some("https://api.example.com/v1")
    );
}
