use super::*;
use serial_test::serial;
use tempfile::TempDir;

use crate::test_support::TestEnvGuard;

fn third_party_codex_provider(api_key: &str) -> Provider {
    Provider::with_id(
        "thirdparty".to_string(),
        "Third Party".to_string(),
        json!({
            "auth": { "OPENAI_API_KEY": api_key },
            "config": "model_provider = \"custom\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.custom]\nbase_url = \"https://api.custom.example/v1\"\nwire_api = \"responses\"\n"
        }),
        Some("custom".to_string()),
    )
}

#[test]
#[serial]
fn switch_codex_provider_projects_route_with_provider_scoped_credentials() {
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
        config_text.contains("requires_openai_auth = false"),
        "a third-party route without an official login must not require one"
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
fn force_sync_codex_third_party_refreshes_auth_when_preserve_is_disabled() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = TestEnvGuard::isolated(temp_home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
        .expect("create ~/.codex (initialized)");
    crate::settings::set_preserve_codex_official_auth_on_switch(false)
        .expect("disable preserve-on-switch");

    crate::config::write_json_file(
        &get_codex_auth_path(),
        &json!({ "OPENAI_API_KEY": "sk-stale-provider" }),
    )
    .expect("seed stale auth.json");

    let provider = third_party_codex_provider("sk-current-provider");
    ProviderService::write_codex_live_force(&provider, None, false, &[])
        .expect("force sync should succeed");

    assert!(!get_codex_auth_path().exists());
    let config_text = std::fs::read_to_string(get_codex_config_path()).expect("read config.toml");
    assert_eq!(
        crate::codex_config::extract_codex_experimental_bearer_token(&config_text).as_deref(),
        Some("sk-current-provider")
    );
}

#[test]
#[serial]
fn force_sync_codex_third_party_preserves_oauth_when_preserve_is_enabled() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = TestEnvGuard::isolated(temp_home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
        .expect("create ~/.codex (initialized)");
    crate::settings::set_preserve_codex_official_auth_on_switch(true)
        .expect("enable preserve-on-switch");

    let preserved_auth = json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": null,
        "tokens": {
            "access_token": "oauth-access-token",
            "account_id": "account-1"
        }
    });
    crate::config::write_json_file(&get_codex_auth_path(), &preserved_auth)
        .expect("seed OAuth auth.json");

    let provider = third_party_codex_provider("sk-current-provider");
    ProviderService::write_codex_live_force(&provider, None, false, &[])
        .expect("force sync should succeed");

    let auth: Value =
        crate::config::read_json_file(&get_codex_auth_path()).expect("read auth.json");
    assert_eq!(
        auth, preserved_auth,
        "force sync must preserve the official OAuth login"
    );

    let config_text = std::fs::read_to_string(get_codex_config_path()).expect("read config.toml");
    assert_eq!(
        crate::codex_config::extract_codex_experimental_bearer_token(&config_text).as_deref(),
        Some("sk-current-provider"),
        "preserve enabled must carry the provider API key in config.toml"
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
        manager.providers.get_mut("official").unwrap().category = Some("official".into());
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

    assert!(!get_codex_auth_path().exists());
    let cfg_final = std::fs::read_to_string(get_codex_config_path()).expect("config.toml final");
    assert_eq!(
        crate::codex_config::extract_codex_experimental_bearer_token(&cfg_final).as_deref(),
        Some("sk-thirdparty")
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

fn auth_switch_state() -> AppState {
    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    let manager = config.get_manager_mut(&AppType::Codex).unwrap();
    manager.providers.insert(
        "thirdparty".into(),
        third_party_codex_provider("sk-thirdparty"),
    );
    manager.providers.insert(
        "official".into(),
        Provider::with_id(
            "official".into(),
            "OpenAI Official".into(),
            json!({"auth": {}, "config": "model = \"gpt-5.2\"\n"}),
            Some("official".into()),
        ),
    );
    manager.providers.get_mut("official").unwrap().category = Some("official".into());
    manager.current = "official".into();
    state_from_config(config)
}

#[test]
#[serial]
fn official_login_round_trip_uses_local_stash_without_backfilling_tokens() {
    for preserve in [false, true] {
        let home = TempDir::new().unwrap();
        let _env = TestEnvGuard::isolated(home.path());
        std::fs::create_dir_all(crate::codex_config::get_codex_config_dir()).unwrap();
        crate::settings::set_preserve_codex_official_auth_on_switch(preserve).unwrap();
        let state = auth_switch_state();
        let login = json!({"auth_mode":"chatgpt", "tokens": {"access_token":"fresh", "refresh_token":"rotated", "account_id":"a"}});
        write_json_file(&get_codex_auth_path(), &login).unwrap();
        crate::config::write_text_file(
            &get_codex_config_path(),
            "model = \"gpt-5.2\"\n# user setting\napproval_policy = \"never\"\n",
        )
        .unwrap();
        ProviderService::switch(&state, AppType::Codex, "thirdparty").unwrap();
        assert_eq!(get_codex_auth_path().exists(), preserve);
        let text = std::fs::read_to_string(get_codex_config_path()).unwrap();
        assert_eq!(
            crate::codex_config::extract_codex_experimental_bearer_token(&text).as_deref(),
            Some("sk-thirdparty")
        );
        assert!(text.contains("# user setting"));
        assert!(text.contains("approval_policy = \"never\""));
        let providers = state.db.get_all_providers("codex").unwrap();
        assert_eq!(providers["official"].settings_config["auth"], json!({}));
        ProviderService::switch(&state, AppType::Codex, "official").unwrap();
        assert_eq!(
            read_json_file::<Value>(&get_codex_auth_path()).unwrap(),
            login
        );
        assert_eq!(
            state.db.get_all_providers("codex").unwrap()["official"].settings_config["auth"],
            json!({})
        );
        let text = std::fs::read_to_string(get_codex_config_path()).unwrap();
        assert!(!text.contains("sk-thirdparty"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(codex_live::stash_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}

#[test]
#[serial]
fn official_switch_removes_known_legacy_key_but_preserves_independent_api_login() {
    for (key, remains) in [("sk-thirdparty", false), ("sk-my-openai-login", true)] {
        let home = TempDir::new().unwrap();
        let _env = TestEnvGuard::isolated(home.path());
        std::fs::create_dir_all(crate::codex_config::get_codex_config_dir()).unwrap();
        let state = auth_switch_state();
        write_json_file(
            &get_codex_auth_path(),
            &json!({"OPENAI_API_KEY":key, "last_refresh":"metadata"}),
        )
        .unwrap();
        ProviderService::switch(&state, AppType::Codex, "official").unwrap();
        assert_eq!(get_codex_auth_path().exists(), remains);
    }
}

#[test]
#[serial]
fn unreadable_login_stash_blocks_switch_before_any_credentials_change() {
    let home = TempDir::new().unwrap();
    let _env = TestEnvGuard::isolated(home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir()).unwrap();
    crate::settings::set_preserve_codex_official_auth_on_switch(false).unwrap();
    let state = auth_switch_state();
    let login = json!({"tokens":{"access_token":"fresh"}});
    write_json_file(&get_codex_auth_path(), &login).unwrap();
    crate::config::atomic_write_private(&codex_live::stash_path(), b"broken").unwrap();
    assert!(ProviderService::switch(&state, AppType::Codex, "thirdparty").is_err());
    assert_eq!(
        read_json_file::<Value>(&get_codex_auth_path()).unwrap(),
        login
    );
    assert_eq!(std::fs::read(codex_live::stash_path()).unwrap(), b"broken");
}

#[test]
#[serial]
fn interrupted_switch_recovers_credentials_and_pointer_before_switching_again() {
    let home = TempDir::new().unwrap();
    let _env = TestEnvGuard::isolated(home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir()).unwrap();
    crate::settings::set_preserve_codex_official_auth_on_switch(false).unwrap();
    let state = auth_switch_state();
    let login = json!({"tokens":{"access_token":"fresh", "account_id":"a"}});
    write_json_file(&get_codex_auth_path(), &login).unwrap();
    crate::mode::operation::failpoint::crash_at(Some("published:0"));
    let result = ProviderService::switch(&state, AppType::Codex, "thirdparty");
    crate::mode::operation::failpoint::crash_at(None);
    assert!(result.is_err());
    assert!(
        crate::mode::state::pending(&crate::live::engine::DeviceStore::for_device(), "codex")
            .unwrap()
            .is_some()
    );
    ProviderService::switch(&state, AppType::Codex, "official").unwrap();
    assert_eq!(
        read_json_file::<Value>(&get_codex_auth_path()).unwrap(),
        login
    );
    assert_eq!(
        ProviderService::current(&state, AppType::Codex).unwrap(),
        "official"
    );
    assert!(
        crate::mode::state::pending(&crate::live::engine::DeviceStore::for_device(), "codex")
            .unwrap()
            .is_none()
    );
}

#[test]
#[serial]
fn reselecting_official_after_logout_does_not_restore_legacy_row_tokens() {
    let home = TempDir::new().unwrap();
    let _env = TestEnvGuard::isolated(home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir()).unwrap();
    let state = auth_switch_state();
    {
        let mut config = state.config.write().unwrap();
        config
            .get_manager_mut(&AppType::Codex)
            .unwrap()
            .providers
            .get_mut("official")
            .unwrap()
            .settings_config["auth"] = json!({"tokens":{"access_token":"stale", "account_id":"a"}});
    }
    state.save().unwrap();
    ProviderService::switch(&state, AppType::Codex, "official").unwrap();
    assert!(!get_codex_auth_path().exists());
}

#[test]
#[serial]
fn unpublished_conflict_keeps_external_token_rotation_and_config_edit() {
    let home = TempDir::new().unwrap();
    let _env = TestEnvGuard::isolated(home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir()).unwrap();
    let state = auth_switch_state();
    write_json_file(
        &get_codex_auth_path(),
        &json!({"tokens":{"access_token":"old"}}),
    )
    .unwrap();
    crate::mode::operation::failpoint::on_before_publish(Some(Box::new(|index, _path| {
        if index == 0 {
            std::fs::write(
                get_codex_auth_path(),
                br#"{"tokens":{"access_token":"rotated"}}"#,
            )
            .unwrap();
            std::fs::write(get_codex_config_path(), b"user_edit = [").unwrap();
        }
    })));
    let result = ProviderService::switch(&state, AppType::Codex, "thirdparty");
    crate::mode::operation::failpoint::on_before_publish(None);
    assert!(result.is_err());
    assert_eq!(
        read_json_file::<Value>(&get_codex_auth_path()).unwrap()["tokens"]["access_token"],
        "rotated"
    );
    assert_eq!(
        std::fs::read_to_string(get_codex_config_path()).unwrap(),
        "user_edit = ["
    );
    assert_eq!(
        ProviderService::current(&state, AppType::Codex).unwrap(),
        "official"
    );
}

#[cfg(unix)]
#[test]
#[serial]
fn codex_switch_with_group_writable_umask_creates_compatible_managed_directory() {
    use std::os::unix::{fs::PermissionsExt, process::CommandExt};
    const CHILD: &str = "CC_SWITCH_TEST_480_UMASK_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "services::provider::codex_openai_auth_tests::codex_switch_with_group_writable_umask_creates_compatible_managed_directory", "--nocapture"])
            .env(CHILD, "1");
        // Change umask only in the child; other tests in this process are unaffected.
        unsafe {
            command.pre_exec(|| {
                libc::umask(0o002);
                Ok(())
            });
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let home = TempDir::new().unwrap();
    let _env = TestEnvGuard::isolated(home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir()).unwrap();
    let state = auth_switch_state();
    ProviderService::switch(&state, AppType::Codex, "thirdparty").unwrap();
    assert_eq!(
        std::fs::metadata(crate::config::get_app_config_dir())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(codex_live::stash_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
#[serial]
fn ordinary_switch_preserves_live_edits_after_common_snippet_was_saved() {
    let home = TempDir::new().unwrap();
    let _env = TestEnvGuard::isolated(home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir()).unwrap();
    let state = auth_switch_state();
    {
        let mut config = state.config.write().unwrap();
        config.common_config_snippets.codex = Some("approval_policy = \"on-request\"".into());
        for provider in config
            .get_manager_mut(&AppType::Codex)
            .unwrap()
            .providers
            .values_mut()
        {
            provider
                .meta
                .get_or_insert_with(Default::default)
                .apply_common_config = Some(true);
        }
    }
    state.save().unwrap();
    crate::config::write_text_file(&get_codex_config_path(), "approval_policy = \"never\"\n")
        .unwrap();
    for id in ["thirdparty", "official"] {
        ProviderService::switch(&state, AppType::Codex, id).unwrap();
        let live = std::fs::read_to_string(get_codex_config_path()).unwrap();
        assert!(live.contains("approval_policy = \"never\""), "{live}");
    }
}

#[test]
#[serial]
fn token_rotation_rejects_switch_before_route_or_stash_publication() {
    let home = TempDir::new().unwrap();
    let _env = TestEnvGuard::isolated(home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir()).unwrap();
    crate::settings::set_preserve_codex_official_auth_on_switch(false).unwrap();
    let state = auth_switch_state();
    let config = "model = \"gpt-5.2\"\napproval_policy = \"never\"\n";
    crate::config::write_text_file(&get_codex_config_path(), config).unwrap();
    write_json_file(
        &get_codex_auth_path(),
        &json!({"tokens":{"access_token":"old"}}),
    )
    .unwrap();
    crate::mode::operation::failpoint::on_before_publish(Some(Box::new(|index, _| {
        if index == 0 {
            std::fs::write(
                get_codex_auth_path(),
                br#"{"tokens":{"access_token":"rotated"}}"#,
            )
            .unwrap();
        }
    })));
    let result = ProviderService::switch(&state, AppType::Codex, "thirdparty");
    crate::mode::operation::failpoint::on_before_publish(None);
    assert!(result.is_err());
    assert_eq!(
        std::fs::read_to_string(get_codex_config_path()).unwrap(),
        config
    );
    assert_eq!(
        read_json_file::<Value>(&get_codex_auth_path()).unwrap()["tokens"]["access_token"],
        "rotated"
    );
    assert!(!super::codex_live::stash_path().exists());
    assert_eq!(
        ProviderService::current(&state, AppType::Codex).unwrap(),
        "official"
    );
    assert!(crate::mode::operation::settle(&state.db, "codex")
        .unwrap()
        .is_none());
}

#[test]
#[serial]
fn malformed_or_unreadable_auth_does_not_block_upstream_route_switching() {
    for unreadable in [false, true] {
        let home = TempDir::new().unwrap();
        let _env = TestEnvGuard::isolated(home.path());
        std::fs::create_dir_all(crate::codex_config::get_codex_config_dir()).unwrap();
        crate::settings::set_preserve_codex_official_auth_on_switch(false).unwrap();
        let auth_path = get_codex_auth_path();
        if unreadable {
            std::fs::create_dir(&auth_path).unwrap();
        } else {
            std::fs::write(&auth_path, "{").unwrap();
        }
        let state = auth_switch_state();
        for id in ["thirdparty", "official"] {
            ProviderService::switch(&state, AppType::Codex, id).unwrap();
            assert_eq!(
                ProviderService::current(&state, AppType::Codex).unwrap(),
                id
            );
            if unreadable {
                assert!(auth_path.is_dir());
            } else {
                assert!(
                    !auth_path.exists(),
                    "upstream clears malformed auth when preservation is off"
                );
            }
        }
    }
}
