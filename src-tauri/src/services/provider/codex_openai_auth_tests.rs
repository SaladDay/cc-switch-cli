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
        result.contains("model_provider = \"my_provider\""),
        "should set model_provider from provider id: {result}"
    );
    assert!(
        result.contains("[model_providers.my_provider]"),
        "should create model_providers section: {result}"
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
