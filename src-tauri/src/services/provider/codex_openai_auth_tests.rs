use super::*;
use serial_test::serial;
use tempfile::TempDir;

use crate::test_support::TestEnvGuard;

#[test]
#[serial]
fn switch_codex_provider_writes_stored_config_directly() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = TestEnvGuard::isolated(temp_home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
        .expect("create ~/.codex (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "OpenAI".to_string(),
                json!({
                    "auth": { "OPENAI_API_KEY": "sk-test" },
                    "config": "model_provider = \"openai\"\nmodel = \"gpt-4o\"\n\n[model_providers.openai]\nbase_url = \"https://api.openai.com/v1\"\nwire_api = \"chat\"\nrequires_openai_auth = true\n"
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);
    ProviderService::switch(&state, AppType::Codex, "p1").expect("switch should succeed");

    let config_text =
        std::fs::read_to_string(get_codex_config_path()).expect("read codex config.toml");
    assert!(
        config_text.contains("requires_openai_auth = true"),
        "config.toml should contain requires_openai_auth from stored config"
    );
    assert!(
        config_text.contains("base_url = \"https://api.openai.com/v1\""),
        "config.toml should contain base_url from stored config"
    );
    assert!(
        config_text.contains("model = \"gpt-4o\""),
        "config.toml should contain model from stored config"
    );
}

#[test]
#[serial]
fn switch_codex_provider_migrates_legacy_flat_config() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = TestEnvGuard::isolated(temp_home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
        .expect("create ~/.codex (initialized)");

    // Start with legacy flat format
    let legacy_config = "base_url = \"https://jp.duckcoding.com/v1\"\nmodel = \"gpt-5.1-codex\"\nwire_api = \"responses\"\nrequires_openai_auth = true";
    let mut provider = Provider::with_id(
        "custom1".to_string(),
        "DuckCoding".to_string(),
        json!({
            "auth": { "OPENAI_API_KEY": "sk-duck" },
            "config": legacy_config
        }),
        None,
    );

    // Simulate startup migration (normally done in AppState::try_new)
    if let Some(migrated) = super::migrate_legacy_codex_config(legacy_config, &provider) {
        provider
            .settings_config
            .as_object_mut()
            .unwrap()
            .insert("config".to_string(), Value::String(migrated));
    }

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    config
        .get_manager_mut(&AppType::Codex)
        .unwrap()
        .providers
        .insert("custom1".to_string(), provider);

    let state = state_from_config(config);
    ProviderService::switch(&state, AppType::Codex, "custom1").expect("switch should succeed");

    let config_text =
        std::fs::read_to_string(get_codex_config_path()).expect("read codex config.toml");
    assert!(
        config_text.contains("model_provider = "),
        "config.toml should have model_provider after migration: {config_text}"
    );
    assert!(
        config_text.contains("[model_providers."),
        "config.toml should have [model_providers.xxx] section after migration: {config_text}"
    );
    assert!(
        config_text.contains("base_url = \"https://jp.duckcoding.com/v1\""),
        "config.toml should preserve base_url after migration: {config_text}"
    );
    assert!(
        config_text.contains("model = \"gpt-5.1-codex\""),
        "config.toml should preserve model after migration: {config_text}"
    );
    assert!(
        config_text.contains("wire_api = \"responses\""),
        "config.toml should preserve wire_api after migration: {config_text}"
    );
}

#[test]
#[serial]
fn switch_codex_overwrites_config_toml_respecting_auth_mode() {
    use crate::settings::{update_settings, AppSettings};

    let temp_home = TempDir::new().expect("create temp home");
    let _env = TestEnvGuard::isolated(temp_home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
        .expect("create ~/.codex (initialized)");

    // With preserve-on-switch enabled, switching to a third-party provider must
    // NOT clobber an existing ChatGPT OAuth auth.json; the API key rides in
    // config.toml instead, while config.toml is still a clean overwrite.
    let previous_settings = crate::settings::get_settings();
    update_settings(AppSettings {
        preserve_codex_official_auth_on_switch: true,
        ..AppSettings::default()
    })
    .expect("enable preserve-on-switch");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.providers.insert(
            "thirdparty".to_string(),
            Provider::with_id(
                "thirdparty".to_string(),
                "Third Party".to_string(),
                json!({
                    "auth": { "OPENAI_API_KEY": "sk-thirdparty" },
                    "config": "model_provider = \"custom\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.custom]\nbase_url = \"https://api.custom.example/v1\"\nwire_api = \"responses\"\n"
                }),
                Some("custom".to_string()),
            ),
        );
    }

    // Seed an existing ChatGPT OAuth login cache in auth.json.
    crate::config::write_json_file(
        &get_codex_auth_path(),
        &json!({
            "tokens": { "access_token": "oauth-access-token" },
            "OPENAI_API_KEY": null
        }),
    )
    .expect("seed live auth.json with OAuth cache");

    let state = state_from_config(config);
    let result = ProviderService::switch(&state, AppType::Codex, "thirdparty");

    // Restore global settings before asserting so other serial tests are clean.
    update_settings(previous_settings).expect("restore settings");
    result.expect("switch should succeed");

    // config.toml is overwritten with the provider's config + the API key as a
    // bearer token (no auth.json write for third-party while preserving).
    let config_text =
        std::fs::read_to_string(get_codex_config_path()).expect("read codex config.toml");
    assert!(
        config_text.contains("base_url = \"https://api.custom.example/v1\""),
        "config.toml should be overwritten with the third-party provider config: {config_text}"
    );
    assert!(
        config_text.contains("experimental_bearer_token"),
        "third-party API key should ride in config.toml as a bearer token: {config_text}"
    );

    // The ChatGPT OAuth login cache in auth.json must be preserved untouched.
    let auth: Value = crate::config::read_json_file(&get_codex_auth_path())
        .expect("auth.json should still exist");
    assert_eq!(
        auth.pointer("/tokens/access_token").and_then(Value::as_str),
        Some("oauth-access-token"),
        "preserve-on-switch must not clobber the OAuth auth.json"
    );
}

#[test]
#[serial]
fn switch_codex_third_party_discards_stray_chatgpt_oauth_after_login() {
    // Regression for issue #328: running `codex login` (ChatGPT OAuth) out-of-band
    // while a third-party provider is active must not leave the ChatGPT login in
    // auth.json when switching back to the third-party provider. A third-party
    // provider authenticates with its API key, so switching to/away from it must
    // never capture or write ChatGPT OAuth material.
    let temp_home = TempDir::new().expect("create temp home");
    let _env = TestEnvGuard::isolated(temp_home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
        .expect("create ~/.codex (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.providers.insert(
            "thirdparty".to_string(),
            Provider::with_id(
                "thirdparty".to_string(),
                "Third Party".to_string(),
                json!({
                    "auth": { "OPENAI_API_KEY": "sk-thirdparty" },
                    "config": "model_provider = \"custom\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.custom]\nbase_url = \"http://localhost:8317/v1\"\nwire_api = \"responses\"\n"
                }),
                Some("custom".to_string()),
            ),
        );
        manager.providers.insert(
            "official".to_string(),
            Provider::with_id(
                "official".to_string(),
                "OpenAI".to_string(),
                json!({
                    "auth": {},
                    "config": "model_provider = \"openai\"\nmodel = \"gpt-5.2-codex\"\n"
                }),
                Some("official".to_string()),
            ),
        );
        manager.current = "thirdparty".to_string();
    }

    let state = state_from_config(config);

    // Start on the third-party provider (clean api-key auth.json).
    ProviderService::switch(&state, AppType::Codex, "thirdparty").expect("switch to thirdparty");

    // Simulate `codex login` (ChatGPT) rewriting live auth.json with OAuth material.
    crate::config::write_json_file(
        &get_codex_auth_path(),
        &json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "tokens": { "access_token": "oauth-access-token", "account_id": "acc-1" },
            "last_refresh": "2026-07-06T00:00:00Z"
        }),
    )
    .expect("simulate codex login (chatgpt)");

    // Switch to official (backfill must not pollute the third-party snapshot),
    // then back to the third-party.
    ProviderService::switch(&state, AppType::Codex, "official").expect("switch to official");

    // The stored third-party snapshot must not have captured the ChatGPT OAuth.
    let stored = state.db.get_all_providers("codex").expect("get providers");
    let tp = stored.get("thirdparty").expect("thirdparty exists");
    let tp_auth = tp
        .settings_config
        .get("auth")
        .cloned()
        .unwrap_or(Value::Null);
    assert!(
        tp_auth.pointer("/tokens/access_token").is_none(),
        "third-party snapshot must not capture ChatGPT OAuth tokens: {tp_auth}"
    );
    assert_eq!(
        tp_auth.get("OPENAI_API_KEY").and_then(Value::as_str),
        Some("sk-thirdparty"),
        "third-party snapshot must keep its API key: {tp_auth}"
    );

    ProviderService::switch(&state, AppType::Codex, "thirdparty")
        .expect("switch back to thirdparty");

    let auth_final: Value =
        crate::config::read_json_file(&get_codex_auth_path()).expect("auth.json final");
    let cfg_final = std::fs::read_to_string(get_codex_config_path()).expect("config.toml final");

    assert!(
        auth_final.pointer("/tokens/access_token").is_none(),
        "live auth.json must not retain ChatGPT OAuth tokens after switching to third-party: {auth_final}"
    );
    assert_eq!(
        auth_final.get("OPENAI_API_KEY").and_then(Value::as_str),
        Some("sk-thirdparty"),
        "live auth.json must carry the third-party API key: {auth_final}"
    );
    assert!(
        cfg_final.contains("base_url = \"http://localhost:8317/v1\""),
        "config.toml should point at the third-party endpoint: {cfg_final}"
    );
}

#[test]
fn migrate_legacy_codex_config_noop_for_new_format() {
    let new_format = "model_provider = \"openai\"\nmodel = \"gpt-4o\"\n\n[model_providers.openai]\nbase_url = \"https://api.openai.com/v1\"\nwire_api = \"chat\"\n";
    let provider = Provider::with_id("p1".to_string(), "OpenAI".to_string(), json!({}), None);
    let result = super::migrate_legacy_codex_config(new_format, &provider);
    assert!(result.is_none(), "new format should not trigger migration");
}

#[test]
fn migrate_legacy_codex_config_converts_flat_format() {
    let legacy = "base_url = \"https://custom.com/v1\"\nmodel = \"gpt-5.1-codex\"\nwire_api = \"responses\"\nrequires_openai_auth = true";
    let provider = Provider::with_id(
        "my_provider".to_string(),
        "My Provider".to_string(),
        json!({}),
        None,
    );
    let result = super::migrate_legacy_codex_config(legacy, &provider)
        .expect("legacy format should trigger migration");
    assert!(
        result.contains("model_provider = \"custom\""),
        "should use the stable custom provider bucket: {result}"
    );
    assert!(
        result.contains("[model_providers.custom]"),
        "should create the stable model_providers section: {result}"
    );
    assert!(
        result.contains("name = \"My Provider\""),
        "should keep the provider display name: {result}"
    );
    assert!(
        result.contains("base_url = \"https://custom.com/v1\""),
        "should preserve base_url: {result}"
    );
    assert!(
        result.contains("wire_api = \"responses\""),
        "should preserve wire_api: {result}"
    );
}

#[test]
fn migrate_legacy_codex_config_preserves_extra_keys() {
    let legacy = "base_url = \"https://custom.com/v1\"\nmodel = \"gpt-5.1-codex\"\nwire_api = \"responses\"\nrequires_openai_auth = true\nmodel_reasoning_effort = \"high\"\ndisable_response_storage = true";
    let provider = Provider::with_id("test".to_string(), "Test".to_string(), json!({}), None);
    let result = super::migrate_legacy_codex_config(legacy, &provider)
        .expect("legacy format should trigger migration");
    assert!(
        result.contains("model_reasoning_effort = \"high\""),
        "should preserve model_reasoning_effort: {result}"
    );
    assert!(
        result.contains("disable_response_storage = true"),
        "should preserve disable_response_storage: {result}"
    );
}
