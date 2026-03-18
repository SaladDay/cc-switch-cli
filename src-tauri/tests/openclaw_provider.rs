use serde_json::json;
use std::str::FromStr;

use cc_switch_lib::{AppType, MultiAppConfig, Provider, ProviderService};

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs, state_from_config};

fn openclaw_settings(provider_id: &str, model_id: &str, api_key: &str) -> serde_json::Value {
    json!({
        "provider": {
            "baseUrl": "https://api.example.com/v1",
            "apiKey": api_key,
            "api": "openai-responses",
            "models": [{
                "id": model_id,
                "name": model_id,
                "reasoning": false,
                "input": ["text"],
                "cost": {
                    "input": 0.0,
                    "output": 0.0,
                    "cacheRead": 0.0,
                    "cacheWrite": 0.0
                },
                "contextWindow": 200000,
                "maxTokens": 8192
            }]
        },
        "primaryModel": format!("{provider_id}/{model_id}")
    })
}

#[test]
fn openclaw_add_syncs_all_providers_without_switching_non_current_primary() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let app_type = AppType::from_str("openclaw").expect("openclaw app type should parse");
    let state = state_from_config(MultiAppConfig::default());

    let first = Provider::with_id(
        "openai".to_string(),
        "OpenAI Compatible".to_string(),
        openclaw_settings("openai", "gpt-4o", "sk-first"),
        None,
    );
    let second = Provider::with_id(
        "moonshot".to_string(),
        "Moonshot".to_string(),
        openclaw_settings("moonshot", "kimi-k2", "sk-second"),
        None,
    );

    ProviderService::add(&state, app_type.clone(), first).expect("first add should succeed");
    ProviderService::add(&state, app_type, second).expect("second add should succeed");

    let openclaw_path = home.join(".openclaw").join("openclaw.json");
    let live: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&openclaw_path).expect("read openclaw live config"),
    )
    .expect("parse openclaw live config");

    let providers = live
        .get("models")
        .and_then(|value| value.get("providers"))
        .and_then(|value| value.as_object())
        .expect("openclaw config should contain provider map");

    assert!(providers.contains_key("openai"));
    assert!(providers.contains_key("moonshot"));
    assert_eq!(
        live.get("agents")
            .and_then(|value| value.get("defaults"))
            .and_then(|value| value.get("model"))
            .and_then(|value| value.get("primary"))
            .and_then(|value| value.as_str()),
        Some("openai/gpt-4o")
    );
}

#[test]
fn openclaw_switch_updates_primary_model() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let app_type = AppType::from_str("openclaw").expect("openclaw app type should parse");
    let state = state_from_config(MultiAppConfig::default());

    let first = Provider::with_id(
        "openai".to_string(),
        "OpenAI Compatible".to_string(),
        openclaw_settings("openai", "gpt-4o", "sk-first"),
        None,
    );
    let second = Provider::with_id(
        "moonshot".to_string(),
        "Moonshot".to_string(),
        openclaw_settings("moonshot", "kimi-k2", "sk-second"),
        None,
    );

    ProviderService::add(&state, app_type.clone(), first).expect("first add should succeed");
    ProviderService::add(&state, app_type.clone(), second).expect("second add should succeed");
    ProviderService::switch(&state, app_type, "moonshot").expect("switch should succeed");

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
        Some("moonshot/kimi-k2")
    );
}

#[test]
fn openclaw_update_non_current_provider_preserves_primary_model() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let app_type = AppType::from_str("openclaw").expect("openclaw app type should parse");
    let state = state_from_config(MultiAppConfig::default());

    let first = Provider::with_id(
        "openai".to_string(),
        "OpenAI Compatible".to_string(),
        openclaw_settings("openai", "gpt-4o", "sk-first"),
        None,
    );
    let second = Provider::with_id(
        "moonshot".to_string(),
        "Moonshot".to_string(),
        openclaw_settings("moonshot", "kimi-k2", "sk-second"),
        None,
    );

    ProviderService::add(&state, app_type.clone(), first).expect("first add should succeed");
    ProviderService::add(&state, app_type.clone(), second).expect("second add should succeed");

    let updated = Provider::with_id(
        "moonshot".to_string(),
        "Moonshot".to_string(),
        openclaw_settings("moonshot", "kimi-k2-turbo", "sk-updated"),
        None,
    );
    ProviderService::update(&state, app_type.clone(), updated).expect("update should succeed");

    let manager = {
        let config = state.config.read().expect("read state config");
        config
            .get_manager(&app_type)
            .expect("openclaw manager should exist")
            .clone()
    };
    let stored = manager
        .providers
        .get("moonshot")
        .expect("updated provider should exist in store");
    assert_eq!(
        stored
            .settings_config
            .get("primaryModel")
            .and_then(|value| value.as_str()),
        Some("moonshot/kimi-k2-turbo")
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
        Some("openai/gpt-4o")
    );
    assert_eq!(
        live.get("models")
            .and_then(|value| value.get("providers"))
            .and_then(|value| value.get("moonshot"))
            .and_then(|value| value.get("apiKey"))
            .and_then(|value| value.as_str()),
        Some("sk-updated")
    );
    assert_eq!(
        live.get("models")
            .and_then(|value| value.get("providers"))
            .and_then(|value| value.get("moonshot"))
            .and_then(|value| value.get("models"))
            .and_then(|value| value.as_array())
            .and_then(|models| models.first())
            .and_then(|value| value.get("id"))
            .and_then(|value| value.as_str()),
        Some("kimi-k2-turbo")
    );
}

#[test]
fn openclaw_import_reads_live_config_and_tracks_current_provider() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    std::fs::write(
        openclaw_dir.join("openclaw.json"),
        serde_json::to_string_pretty(&json!({
            "models": {
                "providers": {
                    "openai": {
                        "name": "OpenAI Compatible",
                        "baseUrl": "https://api.openai.example/v1",
                        "apiKey": "sk-openai",
                        "api": "openai-responses",
                        "models": [{
                            "id": "gpt-4o",
                            "name": "gpt-4o",
                            "reasoning": false,
                            "input": ["text"],
                            "cost": {
                                "input": 0.0,
                                "output": 0.0,
                                "cacheRead": 0.0,
                                "cacheWrite": 0.0
                            },
                            "contextWindow": 200000,
                            "maxTokens": 8192
                        }]
                    },
                    "moonshot": {
                        "name": "Moonshot",
                        "baseUrl": "https://api.moonshot.example/v1",
                        "apiKey": "sk-moonshot",
                        "api": "openai-responses",
                        "models": [{
                            "id": "kimi-k2",
                            "name": "kimi-k2",
                            "reasoning": false,
                            "input": ["text"],
                            "cost": {
                                "input": 0.0,
                                "output": 0.0,
                                "cacheRead": 0.0,
                                "cacheWrite": 0.0
                            },
                            "contextWindow": 128000,
                            "maxTokens": 8192
                        }]
                    }
                }
            },
            "agents": {
                "defaults": {
                    "model": {
                        "primary": "moonshot/kimi-k2"
                    }
                }
            }
        }))
        .expect("serialize live config"),
    )
    .expect("write openclaw live config");

    let app_type = AppType::from_str("openclaw").expect("openclaw app type should parse");
    let state = state_from_config(MultiAppConfig::default());

    ProviderService::import_default_config(&state, app_type.clone())
        .expect("import should succeed");

    let manager = {
        let config = state.config.read().expect("read state config");
        config
            .get_manager(&app_type)
            .expect("openclaw manager should exist")
            .clone()
    };

    assert_eq!(manager.current, "moonshot");
    assert_eq!(manager.providers.len(), 2);
    assert_eq!(
        manager
            .providers
            .get("openai")
            .and_then(|provider| provider.settings_config.get("primaryModel"))
            .and_then(|value| value.as_str()),
        Some("openai/gpt-4o")
    );
    assert_eq!(
        manager
            .providers
            .get("moonshot")
            .and_then(|provider| provider.settings_config.get("primaryModel"))
            .and_then(|value| value.as_str()),
        Some("moonshot/kimi-k2")
    );
}

#[test]
fn openclaw_import_preserves_non_first_live_primary_model() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    std::fs::write(
        openclaw_dir.join("openclaw.json"),
        serde_json::to_string_pretty(&json!({
            "models": {
                "providers": {
                    "openai": {
                        "name": "OpenAI Compatible",
                        "baseUrl": "https://api.openai.example/v1",
                        "apiKey": "sk-openai",
                        "api": "openai-responses",
                        "models": [{
                            "id": "gpt-4o-mini",
                            "name": "gpt-4o-mini",
                            "reasoning": false,
                            "input": ["text"],
                            "cost": {
                                "input": 0.0,
                                "output": 0.0,
                                "cacheRead": 0.0,
                                "cacheWrite": 0.0
                            },
                            "contextWindow": 200000,
                            "maxTokens": 8192
                        }, {
                            "id": "gpt-4o",
                            "name": "gpt-4o",
                            "reasoning": false,
                            "input": ["text"],
                            "cost": {
                                "input": 0.0,
                                "output": 0.0,
                                "cacheRead": 0.0,
                                "cacheWrite": 0.0
                            },
                            "contextWindow": 200000,
                            "maxTokens": 8192
                        }]
                    }
                }
            },
            "agents": {
                "defaults": {
                    "model": {
                        "primary": "openai/gpt-4o"
                    }
                }
            }
        }))
        .expect("serialize live config"),
    )
    .expect("write openclaw live config");

    let app_type = AppType::from_str("openclaw").expect("openclaw app type should parse");
    let state = state_from_config(MultiAppConfig::default());

    ProviderService::import_default_config(&state, app_type.clone())
        .expect("import should succeed");

    let manager = {
        let config = state.config.read().expect("read state config");
        config
            .get_manager(&app_type)
            .expect("openclaw manager should exist")
            .clone()
    };

    assert_eq!(manager.current, "openai");
    assert_eq!(
        manager
            .providers
            .get("openai")
            .and_then(|provider| provider.settings_config.get("primaryModel"))
            .and_then(|value| value.as_str()),
        Some("openai/gpt-4o")
    );
}

#[test]
fn openclaw_add_rejects_missing_api_key() {
    let _guard = lock_test_mutex();
    reset_test_fs();

    let app_type = AppType::from_str("openclaw").expect("openclaw app type should parse");
    let state = state_from_config(MultiAppConfig::default());
    let invalid = Provider::with_id(
        "openai".to_string(),
        "OpenAI Compatible".to_string(),
        json!({
            "provider": {
                "baseUrl": "https://api.example.com/v1",
                "api": "openai-responses",
                "models": [{
                    "id": "gpt-4o",
                    "name": "gpt-4o",
                    "reasoning": false,
                    "input": ["text"],
                    "cost": {
                        "input": 0.0,
                        "output": 0.0,
                        "cacheRead": 0.0,
                        "cacheWrite": 0.0
                    },
                    "contextWindow": 200000,
                    "maxTokens": 8192
                }]
            },
            "primaryModel": "openai/gpt-4o"
        }),
        None,
    );

    let err = ProviderService::add(&state, app_type, invalid).expect_err("add should fail");
    assert!(
        err.to_string().contains("apiKey"),
        "unexpected error: {err}"
    );
}

#[test]
fn openclaw_switch_initializes_live_config_with_all_providers() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    let app_type = AppType::from_str("openclaw").expect("openclaw app type should parse");
    let manager = config
        .get_manager_mut(&app_type)
        .expect("openclaw manager should exist");
    manager.current = "openai".to_string();
    manager.providers.insert(
        "openai".to_string(),
        Provider::with_id(
            "openai".to_string(),
            "OpenAI Compatible".to_string(),
            openclaw_settings("openai", "gpt-4o", "sk-openai"),
            None,
        ),
    );
    manager.providers.insert(
        "moonshot".to_string(),
        Provider::with_id(
            "moonshot".to_string(),
            "Moonshot".to_string(),
            openclaw_settings("moonshot", "kimi-k2", "sk-moonshot"),
            None,
        ),
    );

    let state = state_from_config(config);
    ProviderService::switch(&state, app_type, "moonshot").expect("switch should succeed");

    let openclaw_path = home.join(".openclaw").join("openclaw.json");
    let live: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&openclaw_path).expect("read openclaw live config"),
    )
    .expect("parse openclaw live config");

    let providers = live
        .get("models")
        .and_then(|value| value.get("providers"))
        .and_then(|value| value.as_object())
        .expect("openclaw config should contain provider map");

    assert!(providers.contains_key("openai"));
    assert!(providers.contains_key("moonshot"));
    assert_eq!(
        live.get("agents")
            .and_then(|value| value.get("defaults"))
            .and_then(|value| value.get("model"))
            .and_then(|value| value.get("primary"))
            .and_then(|value| value.as_str()),
        Some("moonshot/kimi-k2")
    );
}
