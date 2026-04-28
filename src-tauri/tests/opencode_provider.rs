use serde_json::json;
use std::str::FromStr;

use cc_switch_lib::{AppError, AppType, MultiAppConfig, Provider, ProviderService};

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs, state_from_config};

fn opencode_provider(id: &str, name: &str, base_url: &str) -> Provider {
    Provider::with_id(
        id.to_string(),
        name.to_string(),
        json!({
            "npm": "@ai-sdk/openai-compatible",
            "options": {
                "baseURL": base_url,
                "apiKey": format!("sk-{id}")
            },
            "models": {
                "main": { "name": "Main" }
            }
        }),
        None,
    )
}

fn opencode_config_path(home: &std::path::Path) -> std::path::PathBuf {
    home.join(".config").join("opencode").join("opencode.json")
}

fn read_opencode_live(path: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("read opencode live config"))
        .expect("parse opencode live config")
}

#[test]
fn opencode_add_syncs_all_providers_to_live_config() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let app_type = AppType::from_str("opencode").expect("opencode app type should parse");
    let state = state_from_config(MultiAppConfig::default());

    let first = Provider::with_id(
        "openai".to_string(),
        "OpenAI Compatible".to_string(),
        json!({
            "npm": "@ai-sdk/openai-compatible",
            "options": {
                "baseURL": "https://api.example.com/v1",
                "apiKey": "sk-first"
            },
            "models": {
                "gpt-4o": { "name": "GPT-4o" }
            }
        }),
        None,
    );

    let second = Provider::with_id(
        "anthropic".to_string(),
        "Anthropic".to_string(),
        json!({
            "npm": "@ai-sdk/anthropic",
            "options": {
                "apiKey": "sk-second"
            },
            "models": {
                "claude-3-7-sonnet": { "name": "Claude 3.7 Sonnet" }
            }
        }),
        None,
    );

    ProviderService::add(&state, app_type.clone(), first).expect("first add should succeed");
    ProviderService::add(&state, app_type, second).expect("second add should succeed");

    let opencode_path = home.join(".config").join("opencode").join("opencode.json");
    let live: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&opencode_path).expect("read opencode live config"),
    )
    .expect("parse opencode live config");

    let providers = live
        .get("provider")
        .and_then(|value| value.as_object())
        .expect("opencode config should contain provider map");

    assert!(providers.contains_key("openai"));
    assert!(providers.contains_key("anthropic"));
}

#[test]
fn opencode_update_live_backed_provider_rewrites_live_config() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenCode)
            .expect("opencode manager");
        manager.providers.insert(
            "live-provider".to_string(),
            opencode_provider(
                "live-provider",
                "Live Provider",
                "https://old.example.com/v1",
            ),
        );
    }

    let opencode_path = opencode_config_path(home);
    std::fs::create_dir_all(opencode_path.parent().expect("opencode config dir"))
        .expect("create opencode dir");
    std::fs::write(
        &opencode_path,
        serde_json::to_string_pretty(&json!({
            "$schema": "https://opencode.ai/config.json",
            "provider": {
                "live-provider": opencode_provider(
                    "live-provider",
                    "Live Provider",
                    "https://old.example.com/v1"
                ).settings_config
            }
        }))
        .expect("serialize opencode live config"),
    )
    .expect("seed opencode live config");

    let state = state_from_config(config);
    ProviderService::update(
        &state,
        AppType::OpenCode,
        opencode_provider(
            "live-provider",
            "Live Provider Updated",
            "https://new.example.com/v1",
        ),
    )
    .expect("updating live-backed opencode provider should rewrite live config");

    let live = read_opencode_live(&opencode_path);
    assert_eq!(
        live["provider"]["live-provider"]["options"]["baseURL"],
        json!("https://new.example.com/v1")
    );
}

#[test]
fn opencode_update_saved_only_provider_does_not_add_to_live_config() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenCode)
            .expect("opencode manager");
        manager.providers.insert(
            "saved-only".to_string(),
            opencode_provider(
                "saved-only",
                "Saved Only",
                "https://saved.old.example.com/v1",
            ),
        );
    }

    let opencode_path = opencode_config_path(home);
    std::fs::create_dir_all(opencode_path.parent().expect("opencode config dir"))
        .expect("create opencode dir");
    std::fs::write(
        &opencode_path,
        serde_json::to_string_pretty(&json!({
            "$schema": "https://opencode.ai/config.json",
            "provider": {
                "keep": opencode_provider("keep", "Keep", "https://keep.example.com/v1")
                    .settings_config
            }
        }))
        .expect("serialize opencode live config"),
    )
    .expect("seed opencode live config");

    let state = state_from_config(config);
    ProviderService::update(
        &state,
        AppType::OpenCode,
        opencode_provider(
            "saved-only",
            "Saved Only Updated",
            "https://saved.new.example.com/v1",
        ),
    )
    .expect("updating saved-only opencode provider should only update stored config");

    let live = read_opencode_live(&opencode_path);
    assert!(live["provider"].get("saved-only").is_none());
    assert_eq!(
        live["provider"]["keep"]["options"]["baseURL"],
        json!("https://keep.example.com/v1")
    );

    let guard = state
        .config
        .read()
        .expect("read config after saved-only update");
    let provider = guard
        .get_manager(&AppType::OpenCode)
        .and_then(|manager| manager.providers.get("saved-only"))
        .expect("saved-only provider should remain saved");
    assert_eq!(provider.name, "Saved Only Updated");
    assert_eq!(
        provider.settings_config["options"]["baseURL"],
        json!("https://saved.new.example.com/v1")
    );
    assert_eq!(
        provider
            .meta
            .as_ref()
            .and_then(|meta| meta.live_config_managed),
        Some(false)
    );
}

#[test]
fn opencode_remove_from_live_config_marks_db_only_until_readded() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let state = state_from_config(MultiAppConfig::default());
    ProviderService::add(
        &state,
        AppType::OpenCode,
        opencode_provider("toggle", "Toggle", "https://toggle.old.example.com/v1"),
    )
    .expect("add opencode provider");

    let opencode_path = opencode_config_path(home);
    assert!(read_opencode_live(&opencode_path)["provider"]
        .get("toggle")
        .is_some());

    ProviderService::remove_from_live_config(&state, AppType::OpenCode, "toggle")
        .expect("remove opencode provider from live config");

    let live_after_remove = read_opencode_live(&opencode_path);
    assert!(live_after_remove["provider"].get("toggle").is_none());
    {
        let guard = state.config.read().expect("read config after remove");
        let provider = guard
            .get_manager(&AppType::OpenCode)
            .and_then(|manager| manager.providers.get("toggle"))
            .expect("toggle provider should remain saved");
        assert_eq!(
            provider
                .meta
                .as_ref()
                .and_then(|meta| meta.live_config_managed),
            Some(false)
        );
    }

    ProviderService::sync_current_to_live(&state)
        .expect("sync_current_to_live should skip db-only opencode provider");
    assert!(read_opencode_live(&opencode_path)["provider"]
        .get("toggle")
        .is_none());

    ProviderService::switch(&state, AppType::OpenCode, "toggle")
        .expect("switch should re-add opencode provider to live config");
    assert!(read_opencode_live(&opencode_path)["provider"]
        .get("toggle")
        .is_some());
    {
        let guard = state.config.read().expect("read config after re-add");
        let provider = guard
            .get_manager(&AppType::OpenCode)
            .and_then(|manager| manager.providers.get("toggle"))
            .expect("toggle provider should remain saved");
        assert_eq!(
            provider
                .meta
                .as_ref()
                .and_then(|meta| meta.live_config_managed),
            Some(true)
        );
    }

    ProviderService::update(
        &state,
        AppType::OpenCode,
        opencode_provider(
            "toggle",
            "Toggle Updated",
            "https://toggle.new.example.com/v1",
        ),
    )
    .expect("re-added opencode provider edit should update live config");
    assert_eq!(
        read_opencode_live(&opencode_path)["provider"]["toggle"]["options"]["baseURL"],
        json!("https://toggle.new.example.com/v1")
    );
}

#[test]
fn openclaw_add_syncs_all_providers_to_live_config() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let app_type = AppType::from_str("openclaw").expect("openclaw app type should parse");
    let state = state_from_config(MultiAppConfig::default());

    let first = Provider::with_id(
        "openai".to_string(),
        "OpenAI Compatible".to_string(),
        json!({
            "api": "openai-responses",
            "apiKey": "sk-first",
            "baseUrl": "https://api.example.com/v1",
            "models": [
                { "id": "gpt-4.1", "name": "GPT-4.1", "contextWindow": 128000 }
            ]
        }),
        None,
    );

    let second = Provider::with_id(
        "anthropic".to_string(),
        "Anthropic".to_string(),
        json!({
            "apiKey": "sk-second",
            "baseUrl": "https://anthropic.example/v1",
            "models": [
                { "id": "claude-sonnet-4", "name": "Claude Sonnet 4", "contextWindow": 200000 }
            ]
        }),
        None,
    );

    ProviderService::add(&state, app_type.clone(), first).expect("first add should succeed");
    ProviderService::add(&state, app_type, second).expect("second add should succeed");

    let openclaw_path = home.join(".openclaw").join("openclaw.json");
    let live: serde_json::Value = json5::from_str(
        &std::fs::read_to_string(&openclaw_path).expect("read openclaw live config"),
    )
    .expect("parse openclaw live config");

    assert_eq!(live["models"]["mode"], "merge");
    let providers = live["models"]["providers"]
        .as_object()
        .expect("openclaw config should contain models.providers map");

    assert!(providers.contains_key("openai"));
    assert!(providers.contains_key("anthropic"));
}

#[test]
fn openclaw_add_rejects_non_provider_like_object_before_syncing_live_config() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let app_type = AppType::from_str("openclaw").expect("openclaw app type should parse");
    let state = state_from_config(MultiAppConfig::default());

    let invalid = Provider::with_id(
        "broken".to_string(),
        "Broken".to_string(),
        json!({
            "foo": "bar"
        }),
        None,
    );

    let err = ProviderService::add(&state, app_type, invalid)
        .expect_err("invalid OpenClaw provider should be rejected before live sync");

    match err {
        AppError::Localized { key, .. } => {
            assert_eq!(key, "provider.openclaw.models.missing");
        }
        other => panic!("expected localized missing-openclaw-models error, got {other:?}"),
    }

    let guard = state.config.read().expect("read config after rejected add");
    let manager = guard
        .get_manager(&AppType::OpenClaw)
        .expect("openclaw manager after rejected add");
    assert!(
        !manager.providers.contains_key("broken"),
        "rejected OpenClaw add should not persist the invalid provider"
    );

    let openclaw_path = home.join(".openclaw").join("openclaw.json");
    assert!(
        !openclaw_path.exists(),
        "rejected OpenClaw add should not create or update live openclaw.json"
    );
}
