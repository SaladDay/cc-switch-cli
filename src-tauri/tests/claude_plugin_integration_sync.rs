use serde_json::json;

use cc_switch_lib::{
    set_enable_claude_plugin_integration, sync_claude_plugin_on_provider_switch,
    sync_claude_plugin_on_settings_toggle, AppType, Provider,
};

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs};

#[test]
fn settings_toggle_sync_writes_and_clears_primary_api_key() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let path = home.join(".claude").join("config.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create ~/.claude");
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&json!({ "foo": "bar" })).expect("serialize seed"),
    )
    .expect("seed config");

    set_enable_claude_plugin_integration(true).expect("enable integration");
    sync_claude_plugin_on_settings_toggle(true).expect("sync on enable");

    let enabled: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read after enable"))
            .expect("parse after enable");
    assert_eq!(enabled["primaryApiKey"], json!("any"));
    assert_eq!(enabled["foo"], json!("bar"));

    set_enable_claude_plugin_integration(false).expect("disable integration");
    sync_claude_plugin_on_settings_toggle(false).expect("sync on disable");

    let disabled: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read after disable"))
            .expect("parse after disable");
    assert!(disabled.get("primaryApiKey").is_none());
    assert_eq!(disabled["foo"], json!("bar"));
}

#[test]
fn provider_switch_sync_respects_integration_toggle() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let path = home.join(".claude").join("config.json");
    let provider = Provider::with_id(
        "third-party".to_string(),
        "Third Party".to_string(),
        json!({"env": {"ANTHROPIC_API_KEY": "test"}}),
        None,
    );

    set_enable_claude_plugin_integration(false).expect("disable integration");
    sync_claude_plugin_on_provider_switch(&AppType::Claude, &provider)
        .expect("sync should be no-op when disabled");
    assert!(!path.exists(), "config should not be created when disabled");

    set_enable_claude_plugin_integration(true).expect("enable integration");
    sync_claude_plugin_on_provider_switch(&AppType::Claude, &provider)
        .expect("sync should apply when enabled");

    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read config"))
            .expect("parse config");
    assert_eq!(value["primaryApiKey"], json!("any"));
}

#[test]
fn provider_switch_sync_clears_for_official_provider() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let path = home.join(".claude").join("config.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create ~/.claude");
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&json!({
            "primaryApiKey": "any",
            "foo": "bar"
        }))
        .expect("serialize seed"),
    )
    .expect("seed config");

    let mut official = Provider::with_id(
        "official".to_string(),
        "Official".to_string(),
        json!({}),
        None,
    );
    official.category = Some("official".to_string());

    set_enable_claude_plugin_integration(true).expect("enable integration");
    sync_claude_plugin_on_provider_switch(&AppType::Claude, &official)
        .expect("sync official provider");

    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read config"))
            .expect("parse config");
    assert!(value.get("primaryApiKey").is_none());
    assert_eq!(value["foo"], json!("bar"));
}
