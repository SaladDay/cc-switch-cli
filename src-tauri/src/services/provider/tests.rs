use super::*;
use serial_test::serial;
use std::ffi::OsString;
use std::path::Path;
use tempfile::TempDir;

use crate::test_support::{
    lock_test_home_and_settings, set_test_home_override, TestHomeSettingsLock,
};

struct EnvGuard {
    _lock: TestHomeSettingsLock,
    old_home: Option<OsString>,
    old_userprofile: Option<OsString>,
}

impl EnvGuard {
    fn set_home(home: &Path) -> Self {
        let lock = lock_test_home_and_settings();
        let old_home = std::env::var_os("HOME");
        let old_userprofile = std::env::var_os("USERPROFILE");
        std::env::set_var("HOME", home);
        std::env::set_var("USERPROFILE", home);
        set_test_home_override(Some(home));
        crate::settings::reload_test_settings();
        Self {
            _lock: lock,
            old_home,
            old_userprofile,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.old_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        match &self.old_userprofile {
            Some(value) => std::env::set_var("USERPROFILE", value),
            None => std::env::remove_var("USERPROFILE"),
        }
        set_test_home_override(self.old_home.as_deref().map(Path::new));
        crate::settings::reload_test_settings();
    }
}

fn codex_settings(config: &str) -> Value {
    json!({
        "auth": { "OPENAI_API_KEY": "sk-test" },
        "config": config,
    })
}

fn with_common_enabled(mut provider: Provider) -> Provider {
    provider
        .meta
        .get_or_insert_with(crate::provider::ProviderMeta::default)
        .apply_common_config = Some(true);
    provider
}

fn setup_switched_codex_state_with_managed_mcp() -> (TempDir, EnvGuard, AppState) {
    let temp_home = TempDir::new().expect("create temp home");
    let env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
        .expect("create ~/.codex (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            with_common_enabled(Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                codex_settings("model_provider = \"first\"\nmodel = \"gpt-4\"\n\n[model_providers.first]\nbase_url = \"https://api.one.example/v1\"\n"),
                None,
            )),
        );
        manager.providers.insert(
            "p2".to_string(),
            with_common_enabled(Provider::with_id(
                "p2".to_string(),
                "Second".to_string(),
                codex_settings("model_provider = \"second\"\nmodel = \"gpt-4\"\n\n[model_providers.second]\nbase_url = \"https://api.two.example/v1\"\n"),
                None,
            )),
        );
    }
    config.mcp.servers = Some(std::collections::HashMap::new());
    config.mcp.servers.as_mut().expect("mcp servers").insert(
        "my_server".to_string(),
        crate::app_config::McpServer {
            id: "my_server".to_string(),
            name: "My Server".to_string(),
            server: json!({
                "type": "stdio",
                "command": "npx"
            }),
            apps: crate::app_config::McpApps {
                claude: false,
                codex: true,
                gemini: false,
                opencode: false,
                hermes: false,
            },
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        },
    );

    std::fs::write(
        get_codex_config_path(),
        r#"model_provider = "azure"
model = "gpt-4"
disable_response_storage = true

[model_providers.azure]
name = "Azure OpenAI"
base_url = "https://azure.example/v1"
wire_api = "responses"

[mcp_servers.my_server]
command = "npx"
"#,
    )
    .expect("seed live config.toml");

    let state = state_from_config(config);
    ProviderService::switch(&state, AppType::Codex, "p2").expect("switch should succeed");

    (temp_home, env, state)
}

fn setup_codex_state_with_broken_other_snapshot() -> (TempDir, EnvGuard, AppState) {
    let temp_home = TempDir::new().expect("create temp home");
    let env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
        .expect("create ~/.codex (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    config.common_config_snippets.codex = Some("disable_response_storage = true".to_string());
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                codex_settings("model_provider = \"first\"\nmodel = \"gpt-4\"\n\n[model_providers.first]\nbase_url = \"https://api.one.example/v1\"\n"),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Broken legacy".to_string(),
                codex_settings("stale-config"),
                None,
            ),
        );
    }

    std::fs::write(
        get_codex_config_path(),
        "disable_response_storage = true\nmodel_provider = \"first\"\nmodel = \"gpt-4\"\n\n[model_providers.first]\nbase_url = \"https://api.one.example/v1\"\n",
    )
    .expect("seed current live config");

    let state = state_from_config(config);
    state.save().expect("persist config snapshot to db");
    (temp_home, env, state)
}

fn setup_codex_state_with_db_current_and_broken_fallback_other_snapshot(
) -> (TempDir, EnvGuard, AppState) {
    let temp_home = TempDir::new().expect("create temp home");
    let env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
        .expect("create ~/.codex (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    config.common_config_snippets.codex = Some("disable_response_storage = true".to_string());
    let mut current_provider = Provider::with_id(
        "p1".to_string(),
        "First".to_string(),
        codex_settings("model_provider = \"first\"\nmodel = \"gpt-4\"\n\n[model_providers.first]\nbase_url = \"https://api.one.example/v1\"\n"),
        None,
    );
    current_provider.sort_index = Some(10);

    let mut broken_fallback_provider = Provider::with_id(
        "p2".to_string(),
        "Broken legacy".to_string(),
        codex_settings("stale-config"),
        None,
    );
    broken_fallback_provider.sort_index = Some(0);

    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "missing".to_string();
        manager
            .providers
            .insert("p1".to_string(), current_provider.clone());
        manager
            .providers
            .insert("p2".to_string(), broken_fallback_provider.clone());
    }

    std::fs::write(
        get_codex_config_path(),
        "disable_response_storage = true\nmodel_provider = \"first\"\nmodel = \"gpt-4\"\n\n[model_providers.first]\nbase_url = \"https://api.one.example/v1\"\n",
    )
    .expect("seed current live config");

    let state = state_from_config(config);
    state
        .db
        .save_provider(AppType::Codex.as_str(), &current_provider)
        .expect("save current provider to db");
    state
        .db
        .save_provider(AppType::Codex.as_str(), &broken_fallback_provider)
        .expect("save broken fallback provider to db");
    state
        .db
        .set_current_provider(AppType::Codex.as_str(), "p1")
        .expect("set db current provider");
    (temp_home, env, state)
}

#[test]
fn validate_provider_settings_rejects_missing_auth_for_codex() {
    let provider = Provider::with_id(
        "codex".into(),
        "Codex".into(),
        json!({ "config": "base_url = \"https://example.com\"" }),
        None,
    );
    let err = ProviderService::validate_provider_settings(&AppType::Codex, &provider)
        .expect_err("missing auth should be rejected");
    assert!(
        err.to_string().contains("auth"),
        "expected auth error, got {err:?}"
    );
}

#[test]
fn validate_provider_settings_rejects_missing_base_url_for_non_official_codex() {
    let provider = Provider::with_id(
        "codex".into(),
        "Codex".into(),
        json!({
            "auth": {},
            "config": "model_provider = \"custom\"\nmodel = \"gpt-5.4\"\n\n[model_providers.custom]\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
        }),
        None,
    );
    let err = ProviderService::validate_provider_settings(&AppType::Codex, &provider)
        .expect_err("missing base_url should be rejected");
    assert!(
        err.to_string().contains("base_url") || err.to_string().contains("Base URL"),
        "expected base_url error, got {err:?}"
    );
}

#[test]
fn validate_provider_settings_allows_blank_config_for_official_codex() {
    let mut provider = Provider::with_id(
        "openai-official".into(),
        "OpenAI Official".into(),
        json!({
            "auth": {},
            "config": ""
        }),
        Some("https://chatgpt.com/codex".to_string()),
    );
    provider.category = Some("official".to_string());
    provider.meta = Some(crate::provider::ProviderMeta {
        codex_official: Some(true),
        ..Default::default()
    });

    ProviderService::validate_provider_settings(&AppType::Codex, &provider)
        .expect("official Codex provider should not require a base_url");
}

#[test]
fn provider_service_add_rejects_non_official_codex_without_base_url() {
    let state = state_from_config(MultiAppConfig::default());
    let provider = Provider::with_id(
        "codex".into(),
        "Codex".into(),
        json!({
            "auth": {},
            "config": "model_provider = \"custom\"\nmodel = \"gpt-5.4\"\n\n[model_providers.custom]\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
        }),
        None,
    );

    let err = ProviderService::add(&state, AppType::Codex, provider)
        .expect_err("service add should reject missing Codex base_url");
    assert!(
        err.to_string().contains("base_url") || err.to_string().contains("Base URL"),
        "expected base_url error, got {err:?}"
    );
}

#[test]
fn set_common_config_snippet_rejects_non_object_opencode_json() {
    let state = state_from_config(MultiAppConfig::default());

    let err = ProviderService::set_common_config_snippet(
        &state,
        AppType::OpenCode,
        Some("[]".to_string()),
    )
    .expect_err("OpenCode common snippet should require a JSON object");

    assert!(
        err.to_string().contains("JSON object"),
        "unexpected error: {err}"
    );
}

#[test]
#[serial]
fn switch_codex_writes_auth_json_when_live_auth_file_is_missing() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
        .expect("create ~/.codex (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "p2".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "Keyring".to_string(),
                json!({
                    "auth": { "OPENAI_API_KEY": "sk-keyring" },
                    "config": "model_provider = \"keyring\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.keyring]\nrequires_openai_auth = true\n",
                }),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Other".to_string(),
                json!({
                    "auth": { "OPENAI_API_KEY": "sk-other" },
                    "config": "model_provider = \"other\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.other]\nrequires_openai_auth = true\n",
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);

    ProviderService::switch(&state, AppType::Codex, "p1")
        .expect("switch should write auth.json from provider snapshot");

    assert!(
        get_codex_auth_path().exists(),
        "auth.json should be created from provider auth"
    );
    let live_auth: Value =
        crate::config::read_json_file(&get_codex_auth_path()).expect("read auth");
    assert_eq!(live_auth["OPENAI_API_KEY"], json!("sk-keyring"));

    let live_config_text =
        std::fs::read_to_string(get_codex_config_path()).expect("read live config.toml");

    let guard = state.config.read().expect("read config after switch");
    let manager = guard
        .get_manager(&AppType::Codex)
        .expect("codex manager after switch");
    assert_eq!(manager.current, "p1", "current provider should update");
    let provider = manager.providers.get("p1").expect("p1 exists");
    assert_eq!(
        provider
            .settings_config
            .get("auth")
            .and_then(|value| value.get("OPENAI_API_KEY"))
            .and_then(Value::as_str),
        Some("sk-keyring")
    );
    // After the switch, the stored config should match the live config.toml
    let stored_config = provider
        .settings_config
        .get("config")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        !stored_config.is_empty() || !live_config_text.trim().is_empty(),
        "provider snapshot should have config text after switch"
    );
}

#[test]
#[serial]
fn codex_switch_overwrites_existing_auth_json_for_openai_official_provider() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
        .expect("create ~/.codex (initialized)");

    // Seed an existing auth.json (simulates `codex login` or prior configuration).
    let existing_auth = json!({ "OPENAI_API_KEY": "sk-existing" });
    let auth_path = crate::codex_config::get_codex_auth_path();
    crate::config::write_json_file(&auth_path, &existing_auth).expect("write auth.json");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "Third Party".to_string(),
                json!({
                    "auth": { "OPENAI_API_KEY": "sk-third-party" },
                    "config": "model_provider = \"thirdparty\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.thirdparty]\nbase_url = \"https://third-party.example/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n",
                }),
                None,
            ),
        );

        let mut official = Provider::with_id(
            "p2".to_string(),
            "OpenAI Official".to_string(),
            json!({
                "auth": { "OPENAI_API_KEY": "sk-openai-official" },
                "config": "model_provider = \"openai\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.openai]\nbase_url = \"https://api.openai.com/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n",
            }),
            None,
        );
        official.meta = Some(crate::provider::ProviderMeta {
            codex_official: Some(true),
            ..Default::default()
        });
        manager.providers.insert("p2".to_string(), official);
    }

    let state = state_from_config(config);

    ProviderService::switch(&state, AppType::Codex, "p2")
        .expect("switch to official should succeed");

    let live_auth: Value = crate::config::read_json_file(&auth_path).expect("read auth.json");
    assert_eq!(
        live_auth["OPENAI_API_KEY"],
        json!("sk-openai-official"),
        "official provider should write its auth snapshot like upstream"
    );
}

#[test]
#[serial]
fn codex_switch_preserves_base_url_and_wire_api_across_multiple_switches() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
        .expect("create ~/.codex (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({
                    "auth": { "OPENAI_API_KEY": "sk-one" },
                    "config": "model_provider = \"providerone\"\nmodel = \"gpt-4o\"\n\n[model_providers.providerone]\nbase_url = \"https://api.one.example/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n",
                }),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Provider Two".to_string(),
                json!({
                    "auth": { "OPENAI_API_KEY": "sk-two" },
                    "config": "model_provider = \"providertwo\"\nmodel = \"gpt-4o\"\n\n[model_providers.providertwo]\nbase_url = \"https://api.two.example/v1\"\nwire_api = \"chat\"\nrequires_openai_auth = true\n",
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);

    // Seed initial live config for p1, then switch to p2, then back to p1.
    ProviderService::switch(&state, AppType::Codex, "p1").expect("seed p1 live");
    ProviderService::switch(&state, AppType::Codex, "p2").expect("switch to p2");
    ProviderService::switch(&state, AppType::Codex, "p1").expect("switch back to p1");

    let live_text =
        std::fs::read_to_string(get_codex_config_path()).expect("read live config.toml");
    assert!(
        live_text.contains("base_url = \"https://api.one.example/v1\""),
        "live config should retain provider base_url after multiple switches"
    );
    assert!(
        live_text.contains("wire_api = \"responses\""),
        "live config should retain provider wire_api after multiple switches"
    );

    let guard = state.config.read().expect("read config");
    let manager = guard.get_manager(&AppType::Codex).expect("codex manager");
    let provider = manager.providers.get("p1").expect("p1 exists");
    let cfg = provider
        .settings_config
        .get("config")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        cfg.contains("base_url = \"https://api.one.example/v1\""),
        "provider snapshot should retain base_url across switches"
    );
    assert!(
        cfg.contains("wire_api = \"responses\""),
        "provider snapshot should retain wire_api across switches"
    );
}

#[test]
#[serial]
fn codex_switch_backfills_effective_current_and_preserves_runtime_projects() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
        .expect("create ~/.codex (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "p2".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({
                    "auth": { "OPENAI_API_KEY": "sk-one-stale" },
                    "config": "model_provider = \"one\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.one]\nbase_url = \"https://api.one.example/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n",
                }),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Provider Two".to_string(),
                json!({
                    "auth": { "OPENAI_API_KEY": "sk-two" },
                    "config": "model_provider = \"two\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.two]\nbase_url = \"https://api.two.example/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n",
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);
    state
        .db
        .set_current_provider(AppType::Codex.as_str(), "p1")
        .expect("set db current provider to p1");

    crate::config::write_json_file(
        &get_codex_auth_path(),
        &json!({ "OPENAI_API_KEY": "sk-one-live" }),
    )
    .expect("seed live auth.json");
    std::fs::write(
        get_codex_config_path(),
        r#"model_provider = "one"
model = "gpt-5.2-codex"

[model_providers.one]
base_url = "https://api.one-live.example/v1"
wire_api = "responses"
requires_openai_auth = true

[projects."/tmp/codex-project-a"]
trust_level = "trusted"
"#,
    )
    .expect("seed live config.toml with runtime project trust");

    ProviderService::switch(&state, AppType::Codex, "p2").expect("switch to p2");

    let cfg = state.config.read().expect("read config after switch");
    let manager = cfg.get_manager(&AppType::Codex).expect("codex manager");
    let p1_stored = manager
        .providers
        .get("p1")
        .expect("p1 exists")
        .settings_config
        .get("config")
        .and_then(Value::as_str)
        .expect("p1 config should be string");
    assert!(
        p1_stored.contains("[projects.\"/tmp/codex-project-a\"]"),
        "effective current provider should receive runtime project trust"
    );
    assert!(
        p1_stored.contains("base_url = \"https://api.one-live.example/v1\""),
        "effective current provider should receive live provider settings"
    );
    assert!(
        cfg.common_config_snippets
            .codex
            .as_deref()
            .unwrap_or_default()
            .is_empty(),
        "runtime project trust should not be auto-extracted as common config"
    );
    drop(cfg);

    let db_p1 = state
        .db
        .get_provider_by_id("p1", AppType::Codex.as_str())
        .expect("read p1 from db")
        .expect("p1 should exist in db");
    let db_p1_config = db_p1
        .settings_config
        .get("config")
        .and_then(Value::as_str)
        .expect("db p1 config should be string");
    assert!(
        db_p1_config.contains("[projects.\"/tmp/codex-project-a\"]"),
        "state.save should not overwrite the effective-current backfill with stale memory"
    );

    let p2_live = std::fs::read_to_string(get_codex_config_path()).expect("read p2 live config");
    assert!(
        !p2_live.contains("/tmp/codex-project-a"),
        "target provider live config should not absorb source provider runtime project trust"
    );

    ProviderService::switch(&state, AppType::Codex, "p1").expect("switch back to p1");
    let p1_live = std::fs::read_to_string(get_codex_config_path()).expect("read p1 live config");
    assert!(
        p1_live.contains("[projects.\"/tmp/codex-project-a\"]"),
        "runtime project trust should survive switching away and back"
    );
}

#[tokio::test]
#[serial]
async fn switch_updates_running_proxy_takeover_target_without_restart() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token-one",
                        "ANTHROPIC_BASE_URL": "https://api.one.example"
                    }
                }),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Provider Two".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token-two",
                        "ANTHROPIC_BASE_URL": "https://api.two.example"
                    }
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);
    state.save().expect("persist config snapshot to db");

    state
        .proxy_service
        .set_takeover_for_app("claude", true)
        .await
        .expect("enable claude takeover");

    ProviderService::switch(&state, AppType::Claude, "p2").expect("switch should hot-switch");

    let status = state.proxy_service.get_status().await;
    assert_eq!(
        status
            .active_targets
            .iter()
            .find(|target| target.app_type == "claude")
            .map(|target| target.provider_id.as_str()),
        Some("p2"),
        "switching providers while takeover is active should update the running proxy target immediately"
    );

    let backup = state
        .db
        .get_live_backup("claude")
        .await
        .expect("get live backup")
        .expect("backup should exist");
    let stored: Value = serde_json::from_str(&backup.original_config).expect("parse backup");
    assert_eq!(
        stored
            .get("env")
            .and_then(Value::as_object)
            .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
            .and_then(Value::as_str),
        Some("https://api.two.example"),
        "hot-switch should also refresh the restore backup to the newly selected provider"
    );

    state
        .proxy_service
        .stop()
        .await
        .expect("stop proxy runtime");
}

#[test]
#[serial]
fn add_first_provider_sets_current() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    let state = state_from_config(config);

    let provider = with_common_enabled(Provider::with_id(
        "p1".to_string(),
        "First".to_string(),
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token",
                "ANTHROPIC_BASE_URL": "https://claude.example"
            }
        }),
        None,
    ));

    ProviderService::add(&state, AppType::Claude, provider).expect("add should succeed");

    let cfg = state.config.read().expect("read config");
    let manager = cfg.get_manager(&AppType::Claude).expect("claude manager");
    assert_eq!(
        manager.current, "p1",
        "first provider should become current to avoid empty current provider"
    );
}

#[test]
#[serial]
fn current_prefers_effective_current_from_local_settings_without_mutating_config() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token1",
                        "ANTHROPIC_BASE_URL": "https://claude.one"
                    }
                }),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Second".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token2",
                        "ANTHROPIC_BASE_URL": "https://claude.two"
                    }
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);
    state.save().expect("persist config snapshot to db");

    crate::settings::set_current_provider(&AppType::Claude, Some("p2"))
        .expect("set local effective current override");

    let current_id = ProviderService::current(&state, AppType::Claude)
        .expect("resolve current provider from effective local settings");
    assert_eq!(
        current_id, "p2",
        "current() should prefer the effective current provider from local settings"
    );

    let cfg = state.config.read().expect("read config");
    let manager = cfg.get_manager(&AppType::Claude).expect("claude manager");
    assert_eq!(
        manager.current, "p1",
        "current() should not rewrite in-memory config when resolving effective current provider"
    );
}

#[test]
#[serial]
fn current_falls_back_to_db_current_without_self_healing_config() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "missing".to_string();

        let mut p1 = with_common_enabled(Provider::with_id(
            "p1".to_string(),
            "First".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "token1",
                    "ANTHROPIC_BASE_URL": "https://claude.one"
                }
            }),
            None,
        ));
        p1.sort_index = Some(10);

        let mut p2 = with_common_enabled(Provider::with_id(
            "p2".to_string(),
            "Second".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "token2",
                    "ANTHROPIC_BASE_URL": "https://claude.two"
                }
            }),
            None,
        ));
        p2.sort_index = Some(0);

        manager.providers.insert("p1".to_string(), p1);
        manager.providers.insert("p2".to_string(), p2);
    }

    let state = state_from_config(config);
    state
        .db
        .save_provider(
            AppType::Claude.as_str(),
            &with_common_enabled(Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token1",
                        "ANTHROPIC_BASE_URL": "https://claude.one"
                    }
                }),
                None,
            )),
        )
        .expect("save p1 to db");
    state
        .db
        .save_provider(
            AppType::Claude.as_str(),
            &with_common_enabled(Provider::with_id(
                "p2".to_string(),
                "Second".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token2",
                        "ANTHROPIC_BASE_URL": "https://claude.two"
                    }
                }),
                None,
            )),
        )
        .expect("save p2 to db");
    state
        .db
        .set_current_provider(AppType::Claude.as_str(), "p2")
        .expect("set db current provider");

    let current_id =
        ProviderService::current(&state, AppType::Claude).expect("read current provider from db");
    assert_eq!(
        current_id, "p2",
        "current() should fall back to the stored current provider in db"
    );

    let cfg = state.config.read().expect("read config");
    let manager = cfg.get_manager(&AppType::Claude).expect("claude manager");
    assert_eq!(
        manager.current, "missing",
        "current() should not self-heal stale in-memory config while reading effective current provider"
    );
}

#[test]
#[serial]
fn current_clears_invalid_local_override_and_falls_back_to_db_current() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "p2".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token1",
                        "ANTHROPIC_BASE_URL": "https://claude.one"
                    }
                }),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Second".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token2",
                        "ANTHROPIC_BASE_URL": "https://claude.two"
                    }
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);
    state.save().expect("persist config snapshot to db");

    crate::settings::set_current_provider(&AppType::Claude, Some("missing"))
        .expect("set invalid local current override");

    let current_id = ProviderService::current(&state, AppType::Claude)
        .expect("fall back to stored current provider after clearing invalid local override");
    assert_eq!(
        current_id, "p2",
        "current() should fall back to the stored db current provider when local override is invalid"
    );
    assert_eq!(
        crate::settings::get_current_provider(&AppType::Claude),
        None,
        "current() should clear invalid local current override during effective-current fallback"
    );

    let cfg = state.config.read().expect("read config");
    let manager = cfg.get_manager(&AppType::Claude).expect("claude manager");
    assert_eq!(
        manager.current, "p2",
        "current() should not mutate config when the stored current provider is already valid"
    );
}

#[test]
#[serial]
fn sync_current_to_live_prefers_effective_current_from_local_settings() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(
        get_claude_settings_path()
            .parent()
            .expect("claude settings parent dir"),
    )
    .expect("create ~/.claude");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token1",
                        "ANTHROPIC_BASE_URL": "https://claude.one"
                    }
                }),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Second".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token2",
                        "ANTHROPIC_BASE_URL": "https://claude.two"
                    }
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);
    state.save().expect("persist config snapshot to db");

    write_json_file(
        &get_claude_settings_path(),
        &json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token1",
                "ANTHROPIC_BASE_URL": "https://claude.one"
            }
        }),
    )
    .expect("seed live settings with config.current provider");

    crate::settings::set_current_provider(&AppType::Claude, Some("p2"))
        .expect("set local effective current override");

    ProviderService::sync_current_to_live(&state)
        .expect("sync_current_to_live should use effective current provider");

    let live: Value = read_json_file(&get_claude_settings_path()).expect("read live settings");
    let env = live
        .get("env")
        .and_then(Value::as_object)
        .expect("live env should be object");
    assert_eq!(
        env.get("ANTHROPIC_AUTH_TOKEN").and_then(Value::as_str),
        Some("token2"),
        "sync_current_to_live should refresh live settings from the effective current provider"
    );
    assert_eq!(
        env.get("ANTHROPIC_BASE_URL").and_then(Value::as_str),
        Some("https://claude.two"),
        "sync_current_to_live should not keep using stale config.current when local settings override it"
    );

    let cfg = state.config.read().expect("read config after sync");
    let manager = cfg.get_manager(&AppType::Claude).expect("claude manager");
    assert_eq!(
        manager.current, "p1",
        "sync_current_to_live should not rewrite in-memory config while resolving the effective current provider"
    );
}

#[test]
#[serial]
fn updating_common_snippet_uses_db_current_without_fallback_healing_config() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::config::get_claude_config_dir())
        .expect("create ~/.claude (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "missing".to_string();

        let mut p1 = with_common_enabled(Provider::with_id(
            "p1".to_string(),
            "First".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "token1",
                    "ANTHROPIC_BASE_URL": "https://claude.one"
                }
            }),
            None,
        ));
        p1.sort_index = Some(10);

        let mut p2 = with_common_enabled(Provider::with_id(
            "p2".to_string(),
            "Second".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "token2",
                    "ANTHROPIC_BASE_URL": "https://claude.two"
                }
            }),
            None,
        ));
        p2.sort_index = Some(0);

        manager.providers.insert("p1".to_string(), p1);
        manager.providers.insert("p2".to_string(), p2);
    }

    write_json_file(
        &get_claude_settings_path(),
        &json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "stale-token",
                "ANTHROPIC_BASE_URL": "https://stale.example"
            }
        }),
    )
    .expect("seed stale live settings");

    let state = state_from_config(config);
    state
        .db
        .save_provider(
            AppType::Claude.as_str(),
            &with_common_enabled(Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token1",
                        "ANTHROPIC_BASE_URL": "https://claude.one"
                    }
                }),
                None,
            )),
        )
        .expect("save first provider to db");
    state
        .db
        .save_provider(
            AppType::Claude.as_str(),
            &with_common_enabled(Provider::with_id(
                "p2".to_string(),
                "Second".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token2",
                        "ANTHROPIC_BASE_URL": "https://claude.two"
                    }
                }),
                None,
            )),
        )
        .expect("save second provider to db");
    state
        .db
        .set_current_provider(AppType::Claude.as_str(), "p1")
        .expect("set db current provider");

    ProviderService::set_common_config_snippet(
        &state,
        AppType::Claude,
        Some(r#"{"includeCoAuthoredBy":false}"#.to_string()),
    )
    .expect("update common snippet");

    let cfg = state.config.read().expect("read config");
    let manager = cfg.get_manager(&AppType::Claude).expect("claude manager");
    assert_eq!(
        manager.current, "missing",
        "updating common snippet should not rewrite stale config.current while syncing live from db current"
    );
    drop(cfg);

    let live: Value = read_json_file(&get_claude_settings_path()).expect("read live settings");
    assert_eq!(
        live.get("includeCoAuthoredBy").and_then(Value::as_bool),
        Some(false),
        "new common snippet should be applied to the healed current live settings"
    );
    let env = live
        .get("env")
        .and_then(Value::as_object)
        .expect("live env should be object");
    assert_eq!(
        env.get("ANTHROPIC_AUTH_TOKEN").and_then(Value::as_str),
        Some("token1"),
        "live settings should refresh from the effective current provider instead of fallback-healing config.current"
    );
}

#[test]
#[serial]
fn updating_common_snippet_uses_db_current_when_config_snapshot_is_missing_current_provider() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::config::get_claude_config_dir())
        .expect("create ~/.claude (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "missing".to_string();
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Second".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token2",
                        "ANTHROPIC_BASE_URL": "https://claude.two"
                    }
                }),
                None,
            ),
        );
    }

    write_json_file(
        &get_claude_settings_path(),
        &json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "stale-token",
                "ANTHROPIC_BASE_URL": "https://stale.example"
            }
        }),
    )
    .expect("seed stale live settings");

    let state = state_from_config(config);
    state
        .db
        .save_provider(
            AppType::Claude.as_str(),
            &with_common_enabled(Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token1",
                        "ANTHROPIC_BASE_URL": "https://claude.one"
                    }
                }),
                None,
            )),
        )
        .expect("save current provider to db");
    state
        .db
        .save_provider(
            AppType::Claude.as_str(),
            &with_common_enabled(Provider::with_id(
                "p2".to_string(),
                "Second".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token2",
                        "ANTHROPIC_BASE_URL": "https://claude.two"
                    }
                }),
                None,
            )),
        )
        .expect("save non-current provider to db");
    state
        .db
        .set_current_provider(AppType::Claude.as_str(), "p1")
        .expect("set db current provider");

    ProviderService::set_common_config_snippet(
        &state,
        AppType::Claude,
        Some(r#"{"includeCoAuthoredBy":false}"#.to_string()),
    )
    .expect("update common snippet should use db current even when config snapshot is missing it");

    let live: Value = read_json_file(&get_claude_settings_path()).expect("read live settings");
    assert_eq!(
        live.get("includeCoAuthoredBy").and_then(Value::as_bool),
        Some(false),
        "new common snippet should be applied to live settings"
    );
    let env = live
        .get("env")
        .and_then(Value::as_object)
        .expect("live env should be object");
    assert_eq!(
        env.get("ANTHROPIC_AUTH_TOKEN").and_then(Value::as_str),
        Some("token1"),
        "live settings should be refreshed from the db current provider even when config snapshot lacks it"
    );

    let cfg = state.config.read().expect("read config after update");
    let manager = cfg.get_manager(&AppType::Claude).expect("claude manager");
    assert_eq!(
        manager.current, "missing",
        "updating common snippet should not rewrite stale config.current even when hydrating the current provider snapshot from db"
    );
    assert!(
        manager.providers.contains_key("p1"),
        "missing current provider snapshot should be hydrated from db before the common snippet update is persisted"
    );

    let db_providers = state
        .db
        .get_all_providers(AppType::Claude.as_str())
        .expect("read db providers after update");
    assert!(
        db_providers.contains_key("p1"),
        "db current provider should remain persisted after updating the common snippet"
    );
}

#[test]
#[serial]
fn common_config_snippet_is_merged_into_claude_settings_on_write() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::config::get_claude_config_dir())
        .expect("create ~/.claude (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    config.common_config_snippets.claude = Some(
        r#"{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1},"includeCoAuthoredBy":false}"#
            .to_string(),
    );

    let state = state_from_config(config);

    let provider = with_common_enabled(Provider::with_id(
        "p1".to_string(),
        "First".to_string(),
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token",
                "ANTHROPIC_BASE_URL": "https://claude.example"
            }
        }),
        None,
    ));

    ProviderService::add(&state, AppType::Claude, provider).expect("add should succeed");

    let settings_path = get_claude_settings_path();
    let live: Value = read_json_file(&settings_path).expect("read live settings");

    assert_eq!(
        live.get("includeCoAuthoredBy").and_then(Value::as_bool),
        Some(false),
        "common snippet should be merged into settings.json"
    );

    let env = live
        .get("env")
        .and_then(Value::as_object)
        .expect("settings.env should be object");

    assert_eq!(
        env.get("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC")
            .and_then(Value::as_i64),
        Some(1),
        "common env key should be present in settings.env"
    );
    assert_eq!(
        env.get("ANTHROPIC_AUTH_TOKEN").and_then(Value::as_str),
        Some("token"),
        "provider env key should remain in settings.env"
    );
}

#[test]
fn build_effective_live_snapshot_merges_claude_common_config_with_upstream_precedence() {
    let provider = with_common_enabled(Provider::with_id(
        "p1".to_string(),
        "First".to_string(),
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token",
                "ANTHROPIC_BASE_URL": "https://provider.example"
            },
            "includeCoAuthoredBy": true,
            "permissions": {
                "allow": ["Bash(git status)"]
            }
        }),
        None,
    ));

    let effective = ProviderService::build_effective_live_snapshot(
        &AppType::Claude,
        &provider,
        Some(
            r#"{"env":{"ANTHROPIC_BASE_URL":"https://common.example","CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1},"includeCoAuthoredBy":false,"permissions":{"allow":["Bash(ls)"]}}"#,
        ),
        true,
    )
    .expect("build effective snapshot");

    assert_eq!(
        effective["env"]["ANTHROPIC_AUTH_TOKEN"],
        json!("token"),
        "provider auth token should be preserved"
    );
    assert_eq!(
        effective["env"]["ANTHROPIC_BASE_URL"],
        json!("https://common.example"),
        "common config should follow upstream merge precedence"
    );
    assert_eq!(
        effective["env"]["CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"],
        json!(1),
        "common env values should still be merged"
    );
    assert_eq!(
        effective["includeCoAuthoredBy"],
        json!(false),
        "common top-level settings should follow upstream merge precedence"
    );
    assert_eq!(
        effective["permissions"]["allow"],
        json!(["Bash(ls)"]),
        "common nested settings should follow upstream merge precedence"
    );
}

#[test]
fn missing_common_config_meta_uses_subset_detection() {
    let provider_with_subset = Provider::with_id(
        "p1".to_string(),
        "First".to_string(),
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token",
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": 1
            }
        }),
        None,
    );
    let provider_without_subset = Provider::with_id(
        "p2".to_string(),
        "Second".to_string(),
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token"
            }
        }),
        None,
    );
    let snippet = r#"{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1}}"#;

    assert!(
        common_config::provider_uses_common_config(
            &AppType::Claude,
            &provider_with_subset,
            Some(snippet),
        ),
        "missing meta should use common config when the provider snapshot already contains it as a subset"
    );
    assert!(
        !common_config::provider_uses_common_config(
            &AppType::Claude,
            &provider_without_subset,
            Some(snippet),
        ),
        "missing meta should not behave like default-enabled when the subset is absent"
    );
}

#[test]
fn json_common_config_array_subset_removal_preserves_extra_items() {
    let settings = json!({
        "permissions": {
            "allow": [
                { "tool": "Bash", "pattern": "git status" },
                { "tool": "Read", "pattern": "src/**" }
            ]
        }
    });
    let snippet = r#"{"permissions":{"allow":[{"tool":"Bash"}]}}"#;

    let stripped =
        common_config::test_support::remove(&AppType::Claude, &settings, snippet).expect("strip");

    assert_eq!(
        stripped["permissions"]["allow"],
        json!([{ "tool": "Read", "pattern": "src/**" }]),
        "array subset removal should remove only the matching common item"
    );
}

#[test]
fn toml_common_config_array_subset_removal_preserves_extra_items_and_identity_keys() {
    let settings = codex_settings(
        "model = \"gpt-5\"\ndisable_response_storage = true\ntools = [{ name = \"common\", command = \"npx\" }, { name = \"provider\", command = \"uvx\" }]\n",
    );
    let snippet =
        "model = \"gpt-5\"\ndisable_response_storage = true\ntools = [{ name = \"common\" }]\n";

    let stripped =
        common_config::test_support::remove(&AppType::Codex, &settings, snippet).expect("strip");
    let stored = stripped
        .get("config")
        .and_then(Value::as_str)
        .expect("config should remain string");

    assert!(
        stored.contains("model = \"gpt-5\""),
        "Codex identity keys should not be stripped by common config removal"
    );
    assert!(
        !stored.contains("disable_response_storage = true"),
        "matching common scalar should be stripped"
    );
    assert!(
        !stored.contains("name = \"common\""),
        "matching common array item should be stripped"
    );
    assert!(
        stored.contains("name = \"provider\""),
        "provider-specific array item should remain"
    );
}

#[test]
#[serial]
fn set_codex_common_config_snippet_rejects_runtime_local_keys() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    let state = state_from_config(MultiAppConfig::default());

    let err = ProviderService::set_common_config_snippet(
        &state,
        AppType::Codex,
        Some("[projects.\"/tmp/demo\"]\ntrust_level = \"trusted\"".to_string()),
    )
    .expect_err("runtime-local Codex tables should be rejected");

    assert!(
        err.to_string().contains("runtime-local key") || err.to_string().contains("运行时本地配置"),
        "error should clearly explain that runtime-local Codex keys are not valid common config"
    );
}

#[test]
fn historical_codex_runtime_keys_are_sanitized_before_effective_apply() {
    let provider = with_common_enabled(Provider::with_id(
        "p1".to_string(),
        "First".to_string(),
        codex_settings(
            "model_provider = \"first\"\nmodel = \"gpt-5\"\n\n[model_providers.first]\nbase_url = \"https://api.example/v1\"\n",
        ),
        None,
    ));
    let effective = ProviderService::build_effective_live_snapshot(
        &AppType::Codex,
        &provider,
        Some(
            "disable_response_storage = true\n\n[projects.\"/tmp/demo\"]\ntrust_level = \"trusted\"\n",
        ),
        true,
    )
    .expect("build effective snapshot");
    let config = effective
        .get("config")
        .and_then(Value::as_str)
        .expect("effective Codex config");

    assert!(
        config.contains("disable_response_storage = true"),
        "safe historical common keys should still apply"
    );
    assert!(
        !config.contains("[projects"),
        "runtime-local historical keys should be sanitized before live apply"
    );
}

#[test]
fn build_effective_live_snapshot_skips_claude_common_config_when_disabled() {
    let mut provider = Provider::with_id(
        "p1".to_string(),
        "First".to_string(),
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token",
                "ANTHROPIC_BASE_URL": "https://provider.example"
            }
        }),
        None,
    );
    provider.meta = Some(crate::provider::ProviderMeta {
        apply_common_config: Some(false),
        ..Default::default()
    });

    let effective = ProviderService::build_effective_live_snapshot(
        &AppType::Claude,
        &provider,
        Some(
            r#"{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1},"includeCoAuthoredBy":false}"#,
        ),
        true,
    )
    .expect("build effective snapshot");

    assert!(
        effective.get("includeCoAuthoredBy").is_none(),
        "common top-level settings should be skipped when disabled"
    );
    assert!(
        effective["env"]
            .get("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC")
            .is_none(),
        "common env settings should be skipped when disabled"
    );
    assert_eq!(
        effective["env"]["ANTHROPIC_BASE_URL"],
        json!("https://provider.example"),
        "provider settings should remain untouched"
    );
}

#[test]
#[serial]
fn common_config_snippet_can_be_disabled_per_provider_for_claude() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::config::get_claude_config_dir())
        .expect("create ~/.claude (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    config.common_config_snippets.claude = Some(
        r#"{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1},"includeCoAuthoredBy":false}"#
            .to_string(),
    );

    let state = state_from_config(config);

    let provider: Provider = serde_json::from_value(json!({
        "id": "p1",
        "name": "First",
        "settingsConfig": {
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token",
                "ANTHROPIC_BASE_URL": "https://claude.example"
            }
        },
        "meta": { "applyCommonConfig": false }
    }))
    .expect("parse provider");

    ProviderService::add(&state, AppType::Claude, provider).expect("add should succeed");

    let settings_path = get_claude_settings_path();
    let live: Value = read_json_file(&settings_path).expect("read live settings");

    assert!(
        live.get("includeCoAuthoredBy").is_none(),
        "common snippet should not be merged when applyCommonConfig=false"
    );
    assert!(
        !live
            .get("env")
            .and_then(Value::as_object)
            .map(|env| env.contains_key("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"))
            .unwrap_or(false),
        "common env keys should not be merged when applyCommonConfig=false"
    );
    assert_eq!(
        live.get("env")
            .and_then(Value::as_object)
            .and_then(|env| env.get("ANTHROPIC_AUTH_TOKEN"))
            .and_then(Value::as_str),
        Some("token"),
        "provider env should still be written"
    );
}

#[test]
#[serial]
fn provider_add_strips_common_snippet_before_claude_snapshot_persist() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::config::get_claude_config_dir())
        .expect("create ~/.claude (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    config.common_config_snippets.claude = Some(
        r#"{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1},"includeCoAuthoredBy":false}"#
            .to_string(),
    );

    let state = state_from_config(config);

    let provider = with_common_enabled(Provider::with_id(
        "p1".to_string(),
        "First".to_string(),
        json!({
            "includeCoAuthoredBy": false,
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token",
                "ANTHROPIC_BASE_URL": "https://claude.example",
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": 1
            }
        }),
        None,
    ));

    ProviderService::add(&state, AppType::Claude, provider).expect("add should succeed");

    let cfg = state.config.read().expect("read config after add");
    let provider = cfg
        .get_manager(&AppType::Claude)
        .expect("claude manager")
        .providers
        .get("p1")
        .expect("p1 exists");
    assert!(
        provider
            .settings_config
            .get("includeCoAuthoredBy")
            .is_none(),
        "common top-level keys should be stripped before persisting Claude snapshot"
    );
    let env = provider
        .settings_config
        .get("env")
        .and_then(Value::as_object)
        .expect("provider env should be object");
    assert!(
        !env.contains_key("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"),
        "common env keys should be stripped before persisting Claude snapshot"
    );
    assert_eq!(
        env.get("ANTHROPIC_AUTH_TOKEN").and_then(Value::as_str),
        Some("token"),
        "provider-specific env keys should remain in the stored snapshot"
    );
}

#[test]
#[serial]
fn provider_add_strips_legacy_claude_model_keys_from_common_snippet() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::config::get_claude_config_dir())
        .expect("create ~/.claude (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    config.common_config_snippets.claude =
        Some(r#"{"env":{"ANTHROPIC_SMALL_FAST_MODEL":"claude-3-5-haiku-20241022"}}"#.to_string());

    let state = state_from_config(config);

    let provider = with_common_enabled(Provider::with_id(
        "p1".to_string(),
        "First".to_string(),
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token",
                "ANTHROPIC_BASE_URL": "https://claude.example",
                "ANTHROPIC_SMALL_FAST_MODEL": "claude-3-5-haiku-20241022"
            }
        }),
        None,
    ));

    ProviderService::add(&state, AppType::Claude, provider).expect("add should succeed");

    let cfg = state.config.read().expect("read config after add");
    let provider = cfg
        .get_manager(&AppType::Claude)
        .expect("claude manager")
        .providers
        .get("p1")
        .expect("p1 exists");
    let env = provider
        .settings_config
        .get("env")
        .and_then(Value::as_object)
        .expect("provider env should be object");

    assert!(
        !env.contains_key("ANTHROPIC_SMALL_FAST_MODEL"),
        "legacy Claude common keys should not remain after provider normalization"
    );
    assert!(
        !env.contains_key("ANTHROPIC_DEFAULT_HAIKU_MODEL"),
        "normalized Claude common keys should be stripped before persisting the provider snapshot"
    );
    assert_eq!(
        env.get("ANTHROPIC_AUTH_TOKEN").and_then(Value::as_str),
        Some("token"),
        "provider-specific env keys should remain in the stored snapshot"
    );
}

#[test]
#[serial]
fn provider_update_strips_common_snippet_before_claude_snapshot_persist() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::config::get_claude_config_dir())
        .expect("create ~/.claude (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    config.common_config_snippets.claude = Some(
        r#"{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1},"includeCoAuthoredBy":false}"#
            .to_string(),
    );
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token",
                        "ANTHROPIC_BASE_URL": "https://claude.example"
                    }
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);

    let provider = Provider::with_id(
        "p1".to_string(),
        "First Updated".to_string(),
        json!({
            "includeCoAuthoredBy": false,
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token-updated",
                "ANTHROPIC_BASE_URL": "https://claude.updated",
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": 1
            }
        }),
        None,
    );

    ProviderService::update(&state, AppType::Claude, provider).expect("update should succeed");

    let cfg = state.config.read().expect("read config after update");
    let provider = cfg
        .get_manager(&AppType::Claude)
        .expect("claude manager")
        .providers
        .get("p1")
        .expect("p1 exists");
    assert!(
        provider
            .settings_config
            .get("includeCoAuthoredBy")
            .is_none(),
        "common top-level keys should be stripped before persisting updated Claude snapshot"
    );
    let env = provider
        .settings_config
        .get("env")
        .and_then(Value::as_object)
        .expect("provider env should be object");
    assert!(
        !env.contains_key("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"),
        "common env keys should be stripped before persisting updated Claude snapshot"
    );
    assert_eq!(
        env.get("ANTHROPIC_AUTH_TOKEN").and_then(Value::as_str),
        Some("token-updated"),
        "provider-specific env keys should remain in the updated stored snapshot"
    );
}

#[test]
#[serial]
fn provider_update_treats_settings_effective_current_as_current_for_live_write() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::config::get_claude_config_dir())
        .expect("create ~/.claude (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token1",
                        "ANTHROPIC_BASE_URL": "https://claude.one"
                    }
                }),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Second".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token2",
                        "ANTHROPIC_BASE_URL": "https://claude.two"
                    }
                }),
                None,
            ),
        );
    }
    let state = state_from_config(config);
    state.save().expect("persist config snapshot to db");

    write_json_file(
        &get_claude_settings_path(),
        &json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token1",
                "ANTHROPIC_BASE_URL": "https://claude.one"
            }
        }),
    )
    .expect("seed current live settings as p1");

    crate::settings::set_current_provider(&AppType::Claude, Some("p2"))
        .expect("set local effective current override to p2");

    let provider = Provider::with_id(
        "p2".to_string(),
        "Second Updated".to_string(),
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token2-updated",
                "ANTHROPIC_BASE_URL": "https://claude.two.updated"
            }
        }),
        None,
    );

    ProviderService::update(&state, AppType::Claude, provider).expect("update should succeed");

    let live: Value = read_json_file(&get_claude_settings_path()).expect("read live settings");
    let live_env = live
        .get("env")
        .and_then(Value::as_object)
        .expect("live env should be object");
    assert_eq!(
        live_env.get("ANTHROPIC_AUTH_TOKEN").and_then(Value::as_str),
        Some("token2-updated"),
        "update should treat settings effective current (p2) as current and rewrite live settings"
    );
    assert_eq!(
        live_env.get("ANTHROPIC_BASE_URL").and_then(Value::as_str),
        Some("https://claude.two.updated"),
        "live settings should reflect updated effective current provider"
    );
}

#[test]
#[serial]
fn provider_update_clears_invalid_local_current_override_and_falls_back_to_stored_current() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::config::get_claude_config_dir())
        .expect("create ~/.claude (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token1",
                        "ANTHROPIC_BASE_URL": "https://claude.one"
                    }
                }),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Second".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token2",
                        "ANTHROPIC_BASE_URL": "https://claude.two"
                    }
                }),
                None,
            ),
        );
    }
    let state = state_from_config(config);
    state.save().expect("persist config snapshot to db");

    write_json_file(
        &get_claude_settings_path(),
        &json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token2",
                "ANTHROPIC_BASE_URL": "https://claude.two"
            }
        }),
    )
    .expect("seed current live settings as p2");

    crate::settings::set_current_provider(&AppType::Claude, Some("missing"))
        .expect("set invalid local current override");

    let provider = Provider::with_id(
        "p1".to_string(),
        "First Updated".to_string(),
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token1-updated",
                "ANTHROPIC_BASE_URL": "https://claude.one.updated"
            }
        }),
        None,
    );

    ProviderService::update(&state, AppType::Claude, provider).expect("update should succeed");

    assert_eq!(
        crate::settings::get_current_provider(&AppType::Claude),
        None,
        "invalid local current override should be cleared during effective-current fallback"
    );

    let live: Value = read_json_file(&get_claude_settings_path()).expect("read live settings");
    let live_env = live
        .get("env")
        .and_then(Value::as_object)
        .expect("live env should be object");
    assert_eq!(
        live_env.get("ANTHROPIC_AUTH_TOKEN").and_then(Value::as_str),
        Some("token1-updated"),
        "update should fall back to stored current provider when local override is invalid"
    );
    assert_eq!(
        live_env.get("ANTHROPIC_BASE_URL").and_then(Value::as_str),
        Some("https://claude.one.updated"),
        "live settings should reflect stored current provider fallback"
    );
}

#[test]
#[serial]
fn common_config_snippet_is_not_persisted_into_provider_snapshot_on_switch() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    config.common_config_snippets.claude = Some(
        r#"{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1},"includeCoAuthoredBy":false}"#
            .to_string(),
    );

    let state = state_from_config(config);

    let p1 = Provider::with_id(
        "p1".to_string(),
        "First".to_string(),
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token1",
                "ANTHROPIC_BASE_URL": "https://claude.one"
            }
        }),
        None,
    );
    let p2 = Provider::with_id(
        "p2".to_string(),
        "Second".to_string(),
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token2",
                "ANTHROPIC_BASE_URL": "https://claude.two"
            }
        }),
        None,
    );

    ProviderService::add(&state, AppType::Claude, p1).expect("add p1");
    ProviderService::add(&state, AppType::Claude, p2).expect("add p2");

    ProviderService::switch(&state, AppType::Claude, "p2").expect("switch to p2");

    let cfg = state.config.read().expect("read config");
    let manager = cfg.get_manager(&AppType::Claude).expect("claude manager");
    let p1_after = manager.providers.get("p1").expect("p1 exists");

    assert!(
        p1_after
            .settings_config
            .get("includeCoAuthoredBy")
            .is_none(),
        "common top-level keys should not be persisted into provider snapshot"
    );

    let env = p1_after
        .settings_config
        .get("env")
        .and_then(Value::as_object)
        .expect("provider env should be object");
    assert!(
        !env.contains_key("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"),
        "common env keys should not be persisted into provider snapshot"
    );
    assert_eq!(
        env.get("ANTHROPIC_AUTH_TOKEN").and_then(Value::as_str),
        Some("token1"),
        "provider-specific env should remain in snapshot"
    );
}

#[test]
#[serial]
fn updating_common_snippet_removes_stale_fields_from_other_claude_provider_snapshots() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::config::get_claude_config_dir())
        .expect("create ~/.claude (initialized)");

    let old_snippet =
        r#"{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1},"includeCoAuthoredBy":false}"#;
    let new_snippet = r#"{"env":{"CLAUDE_CODE_USE_BEDROCK":1},"includeCoAuthoredBy":true}"#;

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    config.common_config_snippets.claude = Some(old_snippet.to_string());
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token1",
                        "ANTHROPIC_BASE_URL": "https://claude.one"
                    }
                }),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Second".to_string(),
                json!({
                    "includeCoAuthoredBy": false,
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token2",
                        "ANTHROPIC_BASE_URL": "https://claude.two",
                        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": 1
                    }
                }),
                None,
            ),
        );
    }

    write_json_file(
        &get_claude_settings_path(),
        &json!({
            "includeCoAuthoredBy": false,
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token1",
                "ANTHROPIC_BASE_URL": "https://claude.one",
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": 1
            }
        }),
    )
    .expect("seed current live settings");

    let state = state_from_config(config);
    state.save().expect("persist config snapshot to db");

    ProviderService::set_common_config_snippet(
        &state,
        AppType::Claude,
        Some(new_snippet.to_string()),
    )
    .expect("update common snippet");

    let cfg = state.config.read().expect("read config after update");
    assert_eq!(
        cfg.common_config_snippets.claude.as_deref(),
        Some(new_snippet),
        "new snippet should be persisted into app config"
    );

    let p2_after = cfg
        .get_manager(&AppType::Claude)
        .expect("claude manager")
        .providers
        .get("p2")
        .expect("p2 exists");
    assert!(
        p2_after
            .settings_config
            .get("includeCoAuthoredBy")
            .is_none(),
        "old top-level common keys should be stripped from other provider snapshots"
    );
    let p2_env = p2_after
        .settings_config
        .get("env")
        .and_then(Value::as_object)
        .expect("p2 env should be object");
    assert!(
        !p2_env.contains_key("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"),
        "old common env keys should be stripped from other provider snapshots"
    );
    assert_eq!(
        p2_env.get("ANTHROPIC_AUTH_TOKEN").and_then(Value::as_str),
        Some("token2"),
        "provider-specific env keys should remain after migration"
    );
    drop(cfg);

    let live: Value = read_json_file(&get_claude_settings_path()).expect("read live settings");
    assert_eq!(
        live.get("includeCoAuthoredBy").and_then(Value::as_bool),
        Some(true),
        "current live settings should reflect the new common snippet"
    );
    let live_env = live
        .get("env")
        .and_then(Value::as_object)
        .expect("live env should be object");
    assert_eq!(
        live_env
            .get("CLAUDE_CODE_USE_BEDROCK")
            .and_then(Value::as_i64),
        Some(1),
        "new common env key should be merged into current live settings"
    );
    assert!(
        !live_env.contains_key("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"),
        "old common env key should be removed from current live settings"
    );
    assert_eq!(
        live_env.get("ANTHROPIC_AUTH_TOKEN").and_then(Value::as_str),
        Some("token1"),
        "current provider env should remain in live settings"
    );
}

#[test]
#[serial]
fn updating_common_snippet_migrates_legacy_claude_model_keys_from_provider_snapshots() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::config::get_claude_config_dir())
        .expect("create ~/.claude (initialized)");

    let old_snippet = r#"{"env":{"ANTHROPIC_SMALL_FAST_MODEL":"claude-3-5-haiku-20241022"}}"#;
    let new_snippet = r#"{"env":{"CLAUDE_CODE_USE_BEDROCK":1}}"#;

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    config.common_config_snippets.claude = Some(old_snippet.to_string());
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token1",
                        "ANTHROPIC_BASE_URL": "https://claude.one"
                    }
                }),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Second".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token2",
                        "ANTHROPIC_BASE_URL": "https://claude.two",
                        "ANTHROPIC_DEFAULT_HAIKU_MODEL": "claude-3-5-haiku-20241022",
                        "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-3-5-haiku-20241022",
                        "ANTHROPIC_DEFAULT_OPUS_MODEL": "claude-3-5-haiku-20241022"
                    }
                }),
                None,
            ),
        );
    }

    write_json_file(
        &get_claude_settings_path(),
        &json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token1",
                "ANTHROPIC_BASE_URL": "https://claude.one"
            }
        }),
    )
    .expect("seed current live settings");

    let state = state_from_config(config);

    ProviderService::set_common_config_snippet(
        &state,
        AppType::Claude,
        Some(new_snippet.to_string()),
    )
    .expect("update common snippet");

    let cfg = state.config.read().expect("read config after update");
    let p2_after = cfg
        .get_manager(&AppType::Claude)
        .expect("claude manager")
        .providers
        .get("p2")
        .expect("p2 exists");
    let p2_env = p2_after
        .settings_config
        .get("env")
        .and_then(Value::as_object)
        .expect("p2 env should be object");

    assert!(
        !p2_env.contains_key("ANTHROPIC_DEFAULT_HAIKU_MODEL"),
        "legacy Claude common model keys should be stripped even when the stored snapshot was normalized"
    );
    assert!(
        !p2_env.contains_key("ANTHROPIC_DEFAULT_SONNET_MODEL"),
        "normalized Sonnet key derived from the legacy snippet should also be stripped"
    );
    assert!(
        !p2_env.contains_key("ANTHROPIC_DEFAULT_OPUS_MODEL"),
        "normalized Opus key derived from the legacy snippet should also be stripped"
    );
    assert_eq!(
        p2_env.get("ANTHROPIC_AUTH_TOKEN").and_then(Value::as_str),
        Some("token2"),
        "provider-specific env keys should remain after migration"
    );
}

#[test]
#[serial]
fn updating_common_snippet_skips_providers_with_apply_common_config_disabled() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::config::get_claude_config_dir())
        .expect("create ~/.claude (initialized)");

    let old_snippet =
        r#"{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1},"includeCoAuthoredBy":false}"#;
    let new_snippet = r#"{"env":{"CLAUDE_CODE_USE_BEDROCK":1},"includeCoAuthoredBy":true}"#;

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    config.common_config_snippets.claude = Some(old_snippet.to_string());
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token1",
                        "ANTHROPIC_BASE_URL": "https://claude.one"
                    }
                }),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            serde_json::from_value(json!({
                "id": "p2",
                "name": "Second",
                "settingsConfig": {
                    "includeCoAuthoredBy": false,
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token2",
                        "ANTHROPIC_BASE_URL": "https://claude.two",
                        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": 1
                    }
                },
                "meta": { "applyCommonConfig": false }
            }))
            .expect("parse provider p2"),
        );
    }

    write_json_file(
        &get_claude_settings_path(),
        &json!({
            "includeCoAuthoredBy": false,
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token1",
                "ANTHROPIC_BASE_URL": "https://claude.one",
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": 1
            }
        }),
    )
    .expect("seed current live settings");

    let state = state_from_config(config);

    ProviderService::set_common_config_snippet(
        &state,
        AppType::Claude,
        Some(new_snippet.to_string()),
    )
    .expect("update common snippet");

    let cfg = state.config.read().expect("read config after update");
    let p2_after = cfg
        .get_manager(&AppType::Claude)
        .expect("claude manager")
        .providers
        .get("p2")
        .expect("p2 exists");
    assert_eq!(
        p2_after
            .settings_config
            .get("includeCoAuthoredBy")
            .and_then(Value::as_bool),
        Some(false),
        "applyCommonConfig=false provider should keep its stored top-level fields during migration"
    );
    let p2_env = p2_after
        .settings_config
        .get("env")
        .and_then(Value::as_object)
        .expect("p2 env should be object");
    assert_eq!(
        p2_env
            .get("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC")
            .and_then(Value::as_i64),
        Some(1),
        "applyCommonConfig=false provider should keep its stored common env keys during migration"
    );
    assert_eq!(
        p2_env.get("ANTHROPIC_AUTH_TOKEN").and_then(Value::as_str),
        Some("token2"),
        "provider-specific env keys should remain untouched"
    );
}

#[test]
#[serial]
fn setting_claude_common_snippet_normalizes_existing_provider_snapshot() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::config::get_claude_config_dir())
        .expect("create ~/.claude (initialized)");

    let new_snippet =
        r#"{"includeCoAuthoredBy":false,"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1}}"#;

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                json!({
                    "includeCoAuthoredBy": false,
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token1",
                        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": 1
                    }
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);

    ProviderService::set_common_config_snippet(
        &state,
        AppType::Claude,
        Some(new_snippet.to_string()),
    )
    .expect("set common snippet");

    let cfg = state.config.read().expect("read config after update");
    let provider = cfg
        .get_manager(&AppType::Claude)
        .expect("claude manager")
        .providers
        .get("p1")
        .expect("p1 exists");

    assert!(
        provider
            .settings_config
            .get("includeCoAuthoredBy")
            .is_none(),
        "new Claude common top-level fields should be stripped from existing provider snapshots immediately"
    );
    let env = provider
        .settings_config
        .get("env")
        .and_then(Value::as_object)
        .expect("stored claude env should be object");
    assert!(
        !env.contains_key("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"),
        "new Claude common env fields should be stripped from existing provider snapshots immediately"
    );
    assert_eq!(
        env.get("ANTHROPIC_AUTH_TOKEN").and_then(Value::as_str),
        Some("token1"),
        "provider-specific Claude env should remain after normalization"
    );
}

#[test]
#[serial]
fn clearing_claude_common_snippet_tolerates_invalid_stored_snippet() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::config::get_claude_config_dir())
        .expect("create ~/.claude (initialized)");

    let invalid_old_snippet = r#"{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1}"#;

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    config.common_config_snippets.claude = Some(invalid_old_snippet.to_string());
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token1",
                        "ANTHROPIC_BASE_URL": "https://claude.one"
                    }
                }),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Second".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": "token2",
                        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": 1
                    }
                }),
                None,
            ),
        );
    }

    write_json_file(
        &get_claude_settings_path(),
        &json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token1",
                "ANTHROPIC_BASE_URL": "https://claude.one",
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": 1
            }
        }),
    )
    .expect("seed current live settings");

    let state = state_from_config(config);
    state.save().expect("persist config snapshot to db");

    ProviderService::clear_common_config_snippet(&state, AppType::Claude)
        .expect("clear should recover from invalid stored snippet");

    let cfg = state.config.read().expect("read config after clear");
    assert_eq!(
        cfg.common_config_snippets.claude, None,
        "invalid stored snippet should not block clearing the saved common snippet"
    );
    drop(cfg);

    let live: Value = read_json_file(&get_claude_settings_path()).expect("read live settings");
    let env = live
        .get("env")
        .and_then(Value::as_object)
        .expect("live env should be object");
    assert!(
        !env.contains_key("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"),
        "clearing should rewrite live settings from the provider snapshot even when the old snippet is invalid"
    );
    assert_eq!(
        env.get("ANTHROPIC_AUTH_TOKEN").and_then(Value::as_str),
        Some("token1"),
        "provider-specific Claude env should remain after recovery"
    );
}

#[test]
#[serial]
fn common_config_snippet_is_merged_into_codex_config_on_write() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
        .expect("create ~/.codex (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    config.common_config_snippets.codex = Some("disable_response_storage = true".to_string());

    let state = state_from_config(config);

    let provider = with_common_enabled(Provider::with_id(
        "p1".to_string(),
        "First".to_string(),
        json!({
            "auth": { "OPENAI_API_KEY": "sk-test" },
            "config": "model_provider = \"first\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.first]\nbase_url = \"https://api.example/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
        }),
        None,
    ));

    ProviderService::add(&state, AppType::Codex, provider).expect("add should succeed");

    let live_text = std::fs::read_to_string(get_codex_config_path()).expect("read config.toml");
    assert!(
        live_text.contains("disable_response_storage = true"),
        "common snippet should be merged into config.toml"
    );
}

#[test]
#[serial]
fn provider_add_strips_common_snippet_before_codex_snapshot_persist() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
        .expect("create ~/.codex (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    config.common_config_snippets.codex = Some("disable_response_storage = true".to_string());

    let state = state_from_config(config);

    let provider = with_common_enabled(Provider::with_id(
        "p1".to_string(),
        "First".to_string(),
        json!({
            "auth": { "OPENAI_API_KEY": "sk-test" },
            "config": "disable_response_storage = true\nmodel_provider = \"first\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.first]\nbase_url = \"https://api.example/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
        }),
        None,
    ));

    ProviderService::add(&state, AppType::Codex, provider).expect("add should succeed");

    let cfg = state.config.read().expect("read config after add");
    let provider = cfg
        .get_manager(&AppType::Codex)
        .expect("codex manager")
        .providers
        .get("p1")
        .expect("p1 exists");
    let stored_config = provider
        .settings_config
        .get("config")
        .and_then(Value::as_str)
        .expect("stored codex config should be string");

    assert!(
        !stored_config.contains("disable_response_storage = true"),
        "common Codex keys should be stripped before persisting provider snapshot"
    );
    assert!(
        stored_config.contains("base_url = \"https://api.example/v1\""),
        "provider-specific Codex config should remain in the stored snapshot"
    );
}

#[test]
fn strip_codex_common_config_keeps_unmatched_nested_table_siblings() {
    let stored_config = r#"disable_response_storage = true
model_provider = "first"
model = "gpt-5"

[mcp_servers.shared]
command = "npx"

[mcp_servers.provider_only]
command = "uvx"

[model_providers.first]
base_url = "https://api.example/v1"
"#;
    let common_snippet = r#"disable_response_storage = true

[mcp_servers.shared]
command = "npx"
"#;

    let stripped =
        strip_codex_common_config_from_full_text(stored_config, common_snippet).expect("strip");

    assert!(
        !stripped.contains("[mcp_servers.shared]"),
        "matched nested common table should be removed"
    );
    assert!(
        stripped.contains("[mcp_servers.provider_only]"),
        "unmatched nested siblings should remain in the stored snapshot"
    );
    assert!(
        stripped.contains("command = \"uvx\""),
        "provider-specific nested table contents should remain"
    );
}

#[test]
fn strip_codex_common_config_keeps_provider_specific_value_in_shared_nested_table() {
    let stored_config = r#"disable_response_storage = true
model_provider = "first"
model = "gpt-5"

[mcp_servers.shared]
command = "uvx"

[model_providers.first]
base_url = "https://api.example/v1"
"#;
    let common_snippet = r#"disable_response_storage = true

[mcp_servers.shared]
command = "npx"
"#;

    let stripped =
        strip_codex_common_config_from_full_text(stored_config, common_snippet).expect("strip");

    assert!(
        stripped.contains("[mcp_servers.shared]"),
        "shared nested table should remain when provider value differs from common snippet"
    );
    assert!(
        stripped.contains("command = \"uvx\""),
        "provider-specific value in the same nested table should not be stripped"
    );
}

#[test]
#[serial]
fn provider_add_tolerates_invalid_codex_common_snippet_during_storage_normalization() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    config.common_config_snippets.codex = Some("disable_response_storage = [".to_string());

    let state = state_from_config(config);

    let provider = with_common_enabled(Provider::with_id(
        "p1".to_string(),
        "First".to_string(),
        json!({
            "auth": { "OPENAI_API_KEY": "sk-test" },
            "config": "model_provider = \"first\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.first]\nbase_url = \"https://api.example/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
        }),
        None,
    ));

    ProviderService::add(&state, AppType::Codex, provider)
        .expect("historical invalid common snippet should not block provider add");
}

#[test]
#[serial]
fn codex_switch_extracts_common_snippet_preserving_mcp_servers() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                codex_settings("model_provider = \"first\"\nmodel = \"gpt-4\"\n\n[model_providers.first]\nbase_url = \"https://api.one.example/v1\"\n"),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Second".to_string(),
                codex_settings("model_provider = \"second\"\nmodel = \"gpt-4\"\n\n[model_providers.second]\nbase_url = \"https://api.two.example/v1\"\n"),
                None,
            ),
        );
    }

    let state = state_from_config(config);

    let config_toml = r#"model_provider = "azure"
model = "gpt-4"
disable_response_storage = true

[model_providers.azure]
name = "Azure OpenAI"
base_url = "https://azure.example/v1"
wire_api = "responses"

[mcp_servers.my_server]
base_url = "http://localhost:8080"
"#;

    let config_path = get_codex_config_path();
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).expect("create codex dir");
    }
    std::fs::write(&config_path, config_toml).expect("seed config.toml");

    ProviderService::switch(&state, AppType::Codex, "p2").expect("switch should succeed");

    let cfg = state.config.read().expect("read config after switch");
    let extracted = cfg
        .common_config_snippets
        .codex
        .as_deref()
        .unwrap_or_default();

    assert!(
        extracted.contains("disable_response_storage = true"),
        "should keep top-level common config"
    );
    assert!(
        extracted.contains("[mcp_servers.my_server]"),
        "should keep mcp_servers table"
    );
    assert!(
        extracted.contains("base_url = \"http://localhost:8080\""),
        "should keep mcp_servers.* base_url"
    );
    assert!(
        !extracted
            .lines()
            .any(|line| line.trim_start().starts_with("model_provider")),
        "should remove top-level model_provider"
    );
    assert!(
        !extracted
            .lines()
            .any(|line| line.trim_start().starts_with("model =")),
        "should remove top-level model"
    );
    assert!(
        !extracted.contains("[model_providers"),
        "should remove entire model_providers table"
    );
}

#[test]
#[serial]
fn setting_codex_common_snippet_after_switch_preserves_mcp_servers() {
    let (_temp_home, _env, state) = setup_switched_codex_state_with_managed_mcp();

    ProviderService::set_common_config_snippet(
        &state,
        AppType::Codex,
        Some("network_access = \"restricted\"".to_string()),
    )
    .expect("set common snippet");

    let live_text = std::fs::read_to_string(get_codex_config_path()).expect("read config.toml");

    assert!(
        live_text.contains("network_access = \"restricted\""),
        "new common snippet should be written to live config"
    );
    assert!(
        live_text.contains("[mcp_servers.my_server]"),
        "managed MCP table should remain after rewriting live config"
    );
    assert!(
        live_text.contains("command = \"npx\""),
        "managed MCP contents should remain after rewriting live config"
    );
}

#[test]
#[serial]
fn clearing_codex_common_snippet_after_switch_preserves_mcp_servers() {
    let (_temp_home, _env, state) = setup_switched_codex_state_with_managed_mcp();

    ProviderService::clear_common_config_snippet(&state, AppType::Codex)
        .expect("clear common snippet");

    let live_text = std::fs::read_to_string(get_codex_config_path()).expect("read config.toml");

    assert!(
        !live_text.contains("disable_response_storage = true"),
        "clearing should remove the extracted common snippet from live config"
    );
    assert!(
        live_text.contains("[mcp_servers.my_server]"),
        "managed MCP table should remain after clearing the common snippet"
    );
    assert!(
        live_text.contains("command = \"npx\""),
        "managed MCP contents should remain after clearing the common snippet"
    );
}

#[test]
#[serial]
fn setting_codex_common_snippet_skips_broken_other_provider_snapshot() {
    let (_temp_home, _env, state) = setup_codex_state_with_broken_other_snapshot();

    ProviderService::set_common_config_snippet(
        &state,
        AppType::Codex,
        Some("network_access = \"restricted\"".to_string()),
    )
    .expect("set should tolerate broken non-current snapshot");

    let cfg = state.config.read().expect("read config after set");
    assert_eq!(
        cfg.common_config_snippets.codex.as_deref(),
        Some("network_access = \"restricted\""),
        "new common snippet should still be persisted"
    );
    let broken = cfg
        .get_manager(&AppType::Codex)
        .expect("codex manager")
        .providers
        .get("p2")
        .expect("broken snapshot should remain");
    assert_eq!(
        broken.settings_config.get("config").and_then(Value::as_str),
        Some("stale-config"),
        "broken legacy snapshot should be left untouched instead of aborting the transaction"
    );
    drop(cfg);

    let live_text = std::fs::read_to_string(get_codex_config_path()).expect("read config.toml");
    assert!(
        live_text.contains("network_access = \"restricted\""),
        "current live config should still refresh to the new common snippet"
    );
    assert!(
        !live_text.contains("disable_response_storage = true"),
        "old common snippet should be removed from the live config"
    );
}

#[test]
#[serial]
fn clearing_codex_common_snippet_skips_broken_other_provider_snapshot() {
    let (_temp_home, _env, state) = setup_codex_state_with_broken_other_snapshot();

    ProviderService::clear_common_config_snippet(&state, AppType::Codex)
        .expect("clear should tolerate broken non-current snapshot");

    let cfg = state.config.read().expect("read config after clear");
    assert!(
        cfg.common_config_snippets.codex.is_none(),
        "clearing should still remove the saved common snippet"
    );
    let broken = cfg
        .get_manager(&AppType::Codex)
        .expect("codex manager")
        .providers
        .get("p2")
        .expect("broken snapshot should remain");
    assert_eq!(
        broken.settings_config.get("config").and_then(Value::as_str),
        Some("stale-config"),
        "broken legacy snapshot should be left untouched instead of aborting the clear path"
    );
    drop(cfg);

    let live_text = std::fs::read_to_string(get_codex_config_path()).expect("read config.toml");
    assert!(
        !live_text.contains("disable_response_storage = true"),
        "clearing should still remove the old common snippet from the live config"
    );
    assert!(
        live_text.contains("base_url = \"https://api.one.example/v1\""),
        "current provider config should remain after clearing the common snippet"
    );
}

#[test]
#[serial]
fn setting_codex_common_snippet_uses_db_current_before_skipping_broken_other_snapshot() {
    let (_temp_home, _env, state) =
        setup_codex_state_with_db_current_and_broken_fallback_other_snapshot();

    ProviderService::set_common_config_snippet(
        &state,
        AppType::Codex,
        Some("network_access = \"restricted\"".to_string()),
    )
    .expect("set should use the db current provider before normalizing snapshots");

    let cfg = state.config.read().expect("read config after set");
    assert_eq!(
        cfg.common_config_snippets.codex.as_deref(),
        Some("network_access = \"restricted\""),
        "new common snippet should still be persisted"
    );
    let manager = cfg.get_manager(&AppType::Codex).expect("codex manager");
    assert_eq!(
        manager.current, "missing",
        "setting a common snippet should not rewrite stale config.current while syncing live from db current"
    );
    let broken = manager
        .providers
        .get("p2")
        .expect("broken snapshot should remain");
    assert_eq!(
        broken.settings_config.get("config").and_then(Value::as_str),
        Some("stale-config"),
        "broken legacy snapshot should still be left untouched"
    );
    drop(cfg);

    let live_text = std::fs::read_to_string(get_codex_config_path()).expect("read config.toml");
    assert!(
        live_text.contains("network_access = \"restricted\""),
        "db current provider should still refresh the live config with the new common snippet"
    );
    assert!(
        !live_text.contains("disable_response_storage = true"),
        "old common snippet should be removed from the live config"
    );
    assert!(
        live_text.contains("base_url = \"https://api.one.example/v1\""),
        "live config should be rebuilt from the db current provider"
    );
}

#[test]
#[serial]
fn clearing_codex_common_snippet_uses_db_current_before_skipping_broken_other_snapshot() {
    let (_temp_home, _env, state) =
        setup_codex_state_with_db_current_and_broken_fallback_other_snapshot();

    ProviderService::clear_common_config_snippet(&state, AppType::Codex)
        .expect("clear should use the db current provider before normalizing snapshots");

    let cfg = state.config.read().expect("read config after clear");
    assert!(
        cfg.common_config_snippets.codex.is_none(),
        "clearing should still remove the saved common snippet"
    );
    let manager = cfg.get_manager(&AppType::Codex).expect("codex manager");
    assert_eq!(
        manager.current, "missing",
        "clearing a common snippet should not rewrite stale config.current while syncing live from db current"
    );
    let broken = manager
        .providers
        .get("p2")
        .expect("broken snapshot should remain");
    assert_eq!(
        broken.settings_config.get("config").and_then(Value::as_str),
        Some("stale-config"),
        "broken legacy snapshot should still be left untouched during clear"
    );
    drop(cfg);

    let live_text = std::fs::read_to_string(get_codex_config_path()).expect("read config.toml");
    assert!(
        !live_text.contains("disable_response_storage = true"),
        "clearing should still remove the old common snippet from the live config"
    );
    assert!(
        live_text.contains("base_url = \"https://api.one.example/v1\""),
        "live config should be rebuilt from the db current provider during clear"
    );
}

#[test]
#[serial]
fn codex_switch_auto_extracted_common_normalizes_other_existing_provider_snapshots() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                codex_settings("disable_response_storage = true\nmodel_provider = \"first\"\nmodel = \"gpt-4\"\n\n[model_providers.first]\nbase_url = \"https://api.one.example/v1\"\n"),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Second".to_string(),
                codex_settings("disable_response_storage = true\nmodel_provider = \"second\"\nmodel = \"gpt-4\"\n\n[model_providers.second]\nbase_url = \"https://api.two.example/v1\"\n"),
                None,
            ),
        );
        manager.providers.insert(
            "p3".to_string(),
            Provider::with_id(
                "p3".to_string(),
                "Third".to_string(),
                codex_settings("disable_response_storage = true\nmodel_provider = \"third\"\nmodel = \"gpt-4\"\n\n[model_providers.third]\nbase_url = \"https://api.three.example/v1\"\n"),
                None,
            ),
        );
    }

    let state = state_from_config(config);

    let config_path = get_codex_config_path();
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).expect("create codex dir");
    }
    std::fs::write(
        &config_path,
        "disable_response_storage = true\nmodel_provider = \"first\"\nmodel = \"gpt-4\"\n\n[model_providers.first]\nbase_url = \"https://api.one.example/v1\"\n",
    )
    .expect("seed config.toml");

    ProviderService::switch(&state, AppType::Codex, "p2").expect("switch should succeed");

    let cfg = state.config.read().expect("read config after switch");
    assert_eq!(
        cfg.common_config_snippets.codex.as_deref(),
        Some("disable_response_storage = true"),
        "switch should persist the auto-extracted common snippet"
    );

    let p3_stored = cfg
        .get_manager(&AppType::Codex)
        .expect("codex manager")
        .providers
        .get("p3")
        .expect("p3 exists")
        .settings_config
        .get("config")
        .and_then(Value::as_str)
        .expect("stored codex config should be string");

    assert!(
        !p3_stored.contains("disable_response_storage = true"),
        "other existing provider snapshots should also be normalized after common snippet is auto-extracted"
    );
    assert!(
        p3_stored.contains("base_url = \"https://api.three.example/v1\""),
        "provider-specific config should remain after auto-normalization"
    );
}

#[test]
#[serial]
fn codex_switch_auto_extracted_common_skips_unparseable_other_provider_snapshots() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                codex_settings("disable_response_storage = true\nmodel_provider = \"first\"\nmodel = \"gpt-4\"\n\n[model_providers.first]\nbase_url = \"https://api.one.example/v1\"\n"),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Second".to_string(),
                codex_settings("disable_response_storage = true\nmodel_provider = \"second\"\nmodel = \"gpt-4\"\n\n[model_providers.second]\nbase_url = \"https://api.two.example/v1\"\n"),
                None,
            ),
        );
        manager.providers.insert(
            "p3".to_string(),
            Provider::with_id(
                "p3".to_string(),
                "Broken legacy".to_string(),
                codex_settings("stale-config"),
                None,
            ),
        );
    }

    let state = state_from_config(config);

    let config_path = get_codex_config_path();
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).expect("create codex dir");
    }
    std::fs::write(
        &config_path,
        "disable_response_storage = true\nmodel_provider = \"first\"\nmodel = \"gpt-4\"\n\n[model_providers.first]\nbase_url = \"https://api.one.example/v1\"\n",
    )
    .expect("seed config.toml");

    ProviderService::switch(&state, AppType::Codex, "p2")
        .expect("switch should skip broken legacy snapshots");

    let cfg = state.config.read().expect("read config after switch");
    assert_eq!(
        cfg.common_config_snippets.codex.as_deref(),
        Some("disable_response_storage = true"),
        "switch should still persist the auto-extracted common snippet"
    );

    let manager = cfg.get_manager(&AppType::Codex).expect("codex manager");
    assert_eq!(
        manager.current, "p2",
        "current provider should still update"
    );

    let p3_stored = manager
        .providers
        .get("p3")
        .expect("p3 exists")
        .settings_config
        .get("config")
        .and_then(Value::as_str)
        .expect("stored codex config should be string");
    assert_eq!(
        p3_stored, "stale-config",
        "broken legacy snapshot should be left untouched instead of blocking the switch"
    );
}

#[test]
#[serial]
fn common_config_snippet_can_be_disabled_per_provider_for_codex() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
        .expect("create ~/.codex (initialized)");

    let config_path = get_codex_config_path();
    std::fs::write(
        &config_path,
        "disable_response_storage = true\nnetwork_access = \"restricted\"\n",
    )
    .expect("seed config.toml");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    config.common_config_snippets.codex = Some("disable_response_storage = true".to_string());
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                codex_settings("model_provider = \"first\"\nmodel = \"gpt-4\"\n\n[model_providers.first]\nbase_url = \"https://api.one.example/v1\"\n"),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            serde_json::from_value(json!({
                "id": "p2",
                "name": "Second",
                "settingsConfig": {
                    "auth": { "OPENAI_API_KEY": "sk-test" },
                    "config": "model_provider = \"second\"\nmodel = \"gpt-4\"\n\n[model_providers.second]\nbase_url = \"https://api.two.example/v1\"\n"
                },
                "meta": { "applyCommonConfig": false }
            }))
            .expect("parse provider p2"),
        );
    }

    let state = state_from_config(config);

    ProviderService::switch(&state, AppType::Codex, "p2").expect("switch should succeed");

    let live_text = std::fs::read_to_string(get_codex_config_path()).expect("read config.toml");
    assert!(
        !live_text.contains("disable_response_storage = true"),
        "common snippet should not be merged when applyCommonConfig=false"
    );
    assert!(
        live_text.contains("base_url = \"https://api.two.example/v1\""),
        "provider-specific config should be written"
    );
}

#[test]
#[serial]
fn updating_common_snippet_removes_stale_fields_from_other_codex_provider_snapshots() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
        .expect("create ~/.codex (initialized)");

    let old_snippet = "disable_response_storage = true";
    let new_snippet = "network_access = \"restricted\"";

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    config.common_config_snippets.codex = Some(old_snippet.to_string());
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                codex_settings("model_provider = \"first\"\nmodel = \"gpt-4\"\n\n[model_providers.first]\nbase_url = \"https://api.one.example/v1\"\n"),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Second".to_string(),
                codex_settings("disable_response_storage = true\nmodel_provider = \"second\"\nmodel = \"gpt-4\"\n\n[model_providers.second]\nbase_url = \"https://api.two.example/v1\"\n"),
                None,
            ),
        );
    }

    std::fs::write(
        get_codex_config_path(),
        "disable_response_storage = true\nmodel_provider = \"first\"\nmodel = \"gpt-4\"\n\n[model_providers.first]\nbase_url = \"https://api.one.example/v1\"\n",
    )
    .expect("seed current live config");

    let state = state_from_config(config);
    state.save().expect("persist config snapshot to db");

    ProviderService::set_common_config_snippet(
        &state,
        AppType::Codex,
        Some(new_snippet.to_string()),
    )
    .expect("update common snippet");

    let cfg = state.config.read().expect("read config after update");
    let p2_after = cfg
        .get_manager(&AppType::Codex)
        .expect("codex manager")
        .providers
        .get("p2")
        .expect("p2 exists");
    let stored_config = p2_after
        .settings_config
        .get("config")
        .and_then(Value::as_str)
        .expect("stored codex config should be string");

    assert!(
        !stored_config.contains("disable_response_storage = true"),
        "old common Codex keys should be stripped from other provider snapshots"
    );
    assert!(
        stored_config.contains("base_url = \"https://api.two.example/v1\""),
        "provider-specific Codex config should remain after migration"
    );
    drop(cfg);

    let live_text = std::fs::read_to_string(get_codex_config_path()).expect("read config.toml");
    assert!(
        live_text.contains("network_access = \"restricted\""),
        "current live config should reflect the new common snippet"
    );
    assert!(
        !live_text.contains("disable_response_storage = true"),
        "current live config should no longer carry the old common snippet"
    );
}

#[test]
#[serial]
fn setting_codex_common_snippet_normalizes_existing_provider_snapshot() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
        .expect("create ~/.codex (initialized)");

    let new_snippet = "disable_response_storage = true";

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                codex_settings("disable_response_storage = true\nmodel_provider = \"first\"\nmodel = \"gpt-4\"\n\n[model_providers.first]\nbase_url = \"https://api.one.example/v1\"\n"),
                None,
            ),
        );
    }

    let state = state_from_config(config);
    state.save().expect("persist config snapshot to db");

    ProviderService::set_common_config_snippet(
        &state,
        AppType::Codex,
        Some(new_snippet.to_string()),
    )
    .expect("set common snippet");

    let cfg = state.config.read().expect("read config after update");
    let stored_config = cfg
        .get_manager(&AppType::Codex)
        .expect("codex manager")
        .providers
        .get("p1")
        .expect("p1 exists")
        .settings_config
        .get("config")
        .and_then(Value::as_str)
        .expect("stored codex config should be string");

    assert!(
        !stored_config.contains("disable_response_storage = true"),
        "new Codex common fields should be stripped from existing provider snapshots immediately"
    );
    assert!(
        stored_config.contains("base_url = \"https://api.one.example/v1\""),
        "provider-specific Codex config should remain after normalization"
    );
}

#[test]
#[serial]
fn replacing_codex_common_snippet_tolerates_invalid_stored_snippet() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
        .expect("create ~/.codex (initialized)");

    let invalid_old_snippet = "disable_response_storage = true\n[";
    let new_snippet = "network_access = \"restricted\"";

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    config.common_config_snippets.codex = Some(invalid_old_snippet.to_string());
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                codex_settings("model_provider = \"first\"\nmodel = \"gpt-4\"\n\n[model_providers.first]\nbase_url = \"https://api.one.example/v1\"\n"),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Second".to_string(),
                codex_settings("disable_response_storage = true\nmodel_provider = \"second\"\nmodel = \"gpt-4\"\n\n[model_providers.second]\nbase_url = \"https://api.two.example/v1\"\n"),
                None,
            ),
        );
    }

    std::fs::write(
        get_codex_config_path(),
        "disable_response_storage = true\nmodel_provider = \"first\"\nmodel = \"gpt-4\"\n\n[model_providers.first]\nbase_url = \"https://api.one.example/v1\"\n",
    )
    .expect("seed current live config");

    let state = state_from_config(config);
    state.save().expect("persist config snapshot to db");

    ProviderService::set_common_config_snippet(
        &state,
        AppType::Codex,
        Some(new_snippet.to_string()),
    )
    .expect("replace should recover from invalid stored snippet");

    let cfg = state.config.read().expect("read config after replace");
    assert_eq!(
        cfg.common_config_snippets.codex.as_deref(),
        Some(new_snippet),
        "invalid stored snippet should not block replacing the saved common snippet"
    );
    drop(cfg);

    let live_text = std::fs::read_to_string(get_codex_config_path()).expect("read config.toml");
    assert!(
        live_text.contains("network_access = \"restricted\""),
        "replacing should write the new common snippet into the live Codex config"
    );
    assert!(
        !live_text.contains("disable_response_storage = true"),
        "replacing should rewrite live Codex config from the provider snapshot even when the old snippet is invalid"
    );
}

#[test]
#[serial]
fn import_default_config_strips_codex_common_snippet_before_persisting_snapshot() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
        .expect("create ~/.codex (initialized)");

    write_json_file(
        &get_codex_auth_path(),
        &json!({ "OPENAI_API_KEY": "sk-test" }),
    )
    .expect("write auth.json");
    std::fs::write(
        get_codex_config_path(),
        "disable_response_storage = true\nnetwork_access = \"restricted\"\nmodel_provider = \"default\"\nmodel = \"gpt-4\"\n\n[model_providers.default]\nbase_url = \"https://api.example/v1\"\n",
    )
    .expect("write config.toml");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    config.common_config_snippets.codex =
        Some("disable_response_storage = true\nnetwork_access = \"restricted\"".to_string());
    let state = state_from_config(config);

    ProviderService::import_default_config(&state, AppType::Codex)
        .expect("import default codex config");

    let cfg = state.config.read().expect("read config after import");
    let provider = cfg
        .get_manager(&AppType::Codex)
        .expect("codex manager")
        .providers
        .get("default")
        .expect("default provider exists");
    let stored_config = provider
        .settings_config
        .get("config")
        .and_then(Value::as_str)
        .expect("stored codex config should be string");

    assert!(
        stored_config.contains("disable_response_storage = true"),
        "missing-meta Codex import should keep common top-level keys for upstream subset detection"
    );
    assert!(
        stored_config.contains("network_access = \"restricted\""),
        "missing-meta Codex import should not strip common fields unless explicitly enabled"
    );
    assert!(
        stored_config.contains("base_url = \"https://api.example/v1\""),
        "provider-specific Codex config should remain after import"
    );
}

#[test]
fn extract_credentials_returns_expected_values() {
    let provider = Provider::with_id(
        "claude".into(),
        "Claude".into(),
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token",
                "ANTHROPIC_BASE_URL": "https://claude.example"
            }
        }),
        None,
    );
    let (api_key, base_url) =
        ProviderService::extract_credentials(&provider, &AppType::Claude).unwrap();
    assert_eq!(api_key, "token");
    assert_eq!(base_url, "https://claude.example");
}

#[test]
fn resolve_usage_script_credentials_falls_back_to_provider_values() {
    let provider = Provider::with_id(
        "claude".into(),
        "Claude".into(),
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token",
                "ANTHROPIC_BASE_URL": "https://claude.example"
            }
        }),
        None,
    );
    let usage_script = crate::provider::UsageScript {
        enabled: true,
        language: "javascript".to_string(),
        code: String::new(),
        timeout: None,
        api_key: None,
        base_url: None,
        access_token: None,
        user_id: None,
        template_type: None,
        auto_query_interval: None,
    };

    let (api_key, base_url) = ProviderService::resolve_usage_script_credentials(
        &provider,
        &AppType::Claude,
        &usage_script,
    )
    .expect("should resolve via provider values");
    assert_eq!(api_key, "token");
    assert_eq!(base_url, "https://claude.example");
}

#[test]
fn resolve_usage_script_credentials_does_not_require_provider_api_key_when_script_has_one() {
    let provider = Provider::with_id(
        "claude".into(),
        "Claude".into(),
        json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://claude.example"
            }
        }),
        None,
    );
    let usage_script = crate::provider::UsageScript {
        enabled: true,
        language: "javascript".to_string(),
        code: String::new(),
        timeout: None,
        api_key: Some("override".to_string()),
        base_url: None,
        access_token: None,
        user_id: None,
        template_type: None,
        auto_query_interval: None,
    };

    let (api_key, base_url) = ProviderService::resolve_usage_script_credentials(
        &provider,
        &AppType::Claude,
        &usage_script,
    )
    .expect("should resolve base_url from provider without needing provider api key");
    assert_eq!(api_key, "override");
    assert_eq!(base_url, "https://claude.example");
}

#[test]
#[serial]
fn common_config_snippet_is_merged_into_gemini_env_on_write() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::gemini_config::get_gemini_dir())
        .expect("create ~/.gemini (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Gemini);
    config.common_config_snippets.gemini = Some(r#"{"CC_SWITCH_GEMINI_COMMON":"1"}"#.to_string());

    let state = state_from_config(config);

    let provider = with_common_enabled(Provider::with_id(
        "p1".to_string(),
        "First".to_string(),
        json!({
            "env": {
                "GEMINI_API_KEY": "token"
            }
        }),
        None,
    ));

    ProviderService::add(&state, AppType::Gemini, provider).expect("add should succeed");

    let env = crate::gemini_config::read_gemini_env().expect("read gemini env");
    assert_eq!(
        env.get("CC_SWITCH_GEMINI_COMMON").map(String::as_str),
        Some("1"),
        "common snippet env key should be present in ~/.gemini/.env"
    );
    assert_eq!(
        env.get("GEMINI_API_KEY").map(String::as_str),
        Some("token"),
        "provider env key should remain in ~/.gemini/.env"
    );
}

#[test]
#[serial]
fn provider_add_strips_common_snippet_before_gemini_snapshot_persist() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::gemini_config::get_gemini_dir())
        .expect("create ~/.gemini (initialized)");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Gemini);
    config.common_config_snippets.gemini = Some(r#"{"CC_SWITCH_GEMINI_COMMON":"1"}"#.to_string());

    let state = state_from_config(config);

    let provider = with_common_enabled(Provider::with_id(
        "p1".to_string(),
        "First".to_string(),
        json!({
            "env": {
                "GEMINI_API_KEY": "token",
                "CC_SWITCH_GEMINI_COMMON": "1"
            }
        }),
        None,
    ));

    ProviderService::add(&state, AppType::Gemini, provider).expect("add should succeed");

    let cfg = state.config.read().expect("read config after add");
    let provider = cfg
        .get_manager(&AppType::Gemini)
        .expect("gemini manager")
        .providers
        .get("p1")
        .expect("p1 exists");
    let env = provider
        .settings_config
        .get("env")
        .and_then(Value::as_object)
        .expect("provider env should be object");

    assert!(
        !env.contains_key("CC_SWITCH_GEMINI_COMMON"),
        "common Gemini env keys should be stripped before persisting provider snapshot"
    );
    assert_eq!(
        env.get("GEMINI_API_KEY").and_then(Value::as_str),
        Some("token"),
        "provider-specific Gemini env keys should remain in the stored snapshot"
    );
}

#[test]
#[serial]
fn common_config_snippet_is_not_persisted_into_gemini_provider_snapshot_on_switch() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Gemini);
    config.common_config_snippets.gemini = Some(r#"{"CC_SWITCH_GEMINI_COMMON":"1"}"#.to_string());

    let state = state_from_config(config);

    let p1 = with_common_enabled(Provider::with_id(
        "p1".to_string(),
        "First".to_string(),
        json!({
            "env": {
                "GEMINI_API_KEY": "token1"
            }
        }),
        None,
    ));
    let p2 = with_common_enabled(Provider::with_id(
        "p2".to_string(),
        "Second".to_string(),
        json!({
            "env": {
                "GEMINI_API_KEY": "token2"
            }
        }),
        None,
    ));

    ProviderService::add(&state, AppType::Gemini, p1).expect("add p1");
    ProviderService::add(&state, AppType::Gemini, p2).expect("add p2");

    ProviderService::switch(&state, AppType::Gemini, "p2").expect("switch to p2");

    let cfg = state.config.read().expect("read config");
    let manager = cfg.get_manager(&AppType::Gemini).expect("gemini manager");
    let p1_after = manager.providers.get("p1").expect("p1 exists");

    let env = p1_after
        .settings_config
        .get("env")
        .and_then(Value::as_object)
        .expect("provider env should be object");

    assert!(
        !env.contains_key("CC_SWITCH_GEMINI_COMMON"),
        "common env keys should not be persisted into provider snapshot"
    );
    assert_eq!(
        env.get("GEMINI_API_KEY").and_then(Value::as_str),
        Some("token1"),
        "provider-specific env should remain in snapshot"
    );
}

#[test]
#[serial]
fn updating_common_snippet_removes_stale_fields_from_other_gemini_provider_snapshots() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::gemini_config::get_gemini_dir())
        .expect("create ~/.gemini (initialized)");

    let old_snippet = r#"{"CC_SWITCH_GEMINI_COMMON":"1"}"#;
    let new_snippet = r#"{"CC_SWITCH_GEMINI_REPLACED":"1"}"#;

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Gemini);
    config.common_config_snippets.gemini = Some(old_snippet.to_string());
    {
        let manager = config
            .get_manager_mut(&AppType::Gemini)
            .expect("gemini manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                json!({
                    "env": {
                        "GEMINI_API_KEY": "token1"
                    }
                }),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Second".to_string(),
                json!({
                    "env": {
                        "GEMINI_API_KEY": "token2",
                        "CC_SWITCH_GEMINI_COMMON": "1"
                    }
                }),
                None,
            ),
        );
    }

    crate::gemini_config::write_gemini_env_atomic(&std::collections::HashMap::from([
        ("GEMINI_API_KEY".to_string(), "token1".to_string()),
        ("CC_SWITCH_GEMINI_COMMON".to_string(), "1".to_string()),
    ]))
    .expect("seed current gemini env");

    let state = state_from_config(config);
    state.save().expect("persist config snapshot to db");

    ProviderService::set_common_config_snippet(
        &state,
        AppType::Gemini,
        Some(new_snippet.to_string()),
    )
    .expect("update common snippet");

    let cfg = state.config.read().expect("read config after update");
    let p2_after = cfg
        .get_manager(&AppType::Gemini)
        .expect("gemini manager")
        .providers
        .get("p2")
        .expect("p2 exists");
    let env = p2_after
        .settings_config
        .get("env")
        .and_then(Value::as_object)
        .expect("provider env should be object");

    assert!(
        !env.contains_key("CC_SWITCH_GEMINI_COMMON"),
        "old common Gemini env keys should be stripped from other provider snapshots"
    );
    assert_eq!(
        env.get("GEMINI_API_KEY").and_then(Value::as_str),
        Some("token2"),
        "provider-specific Gemini env keys should remain after migration"
    );
    drop(cfg);

    let live_env = crate::gemini_config::read_gemini_env().expect("read gemini env");
    assert_eq!(
        live_env
            .get("CC_SWITCH_GEMINI_REPLACED")
            .map(String::as_str),
        Some("1"),
        "current live Gemini env should reflect the new common snippet"
    );
    assert!(
        !live_env.contains_key("CC_SWITCH_GEMINI_COMMON"),
        "current live Gemini env should no longer carry the old common snippet"
    );
}

#[test]
#[serial]
fn setting_gemini_common_snippet_normalizes_existing_provider_snapshot() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::gemini_config::get_gemini_dir())
        .expect("create ~/.gemini (initialized)");

    let new_snippet = r#"{"CC_SWITCH_GEMINI_COMMON":"1"}"#;

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Gemini);
    {
        let manager = config
            .get_manager_mut(&AppType::Gemini)
            .expect("gemini manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            with_common_enabled(Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                json!({
                    "env": {
                        "GEMINI_API_KEY": "token1",
                        "CC_SWITCH_GEMINI_COMMON": "1"
                    }
                }),
                None,
            )),
        );
    }

    let state = state_from_config(config);
    state.save().expect("persist config snapshot to db");

    ProviderService::set_common_config_snippet(
        &state,
        AppType::Gemini,
        Some(new_snippet.to_string()),
    )
    .expect("set common snippet");

    let cfg = state.config.read().expect("read config after update");
    let env = cfg
        .get_manager(&AppType::Gemini)
        .expect("gemini manager")
        .providers
        .get("p1")
        .expect("p1 exists")
        .settings_config
        .get("env")
        .and_then(Value::as_object)
        .expect("stored gemini env should be object");

    assert!(
        !env.contains_key("CC_SWITCH_GEMINI_COMMON"),
        "new Gemini common fields should be stripped from existing provider snapshots immediately"
    );
    assert_eq!(
        env.get("GEMINI_API_KEY").and_then(Value::as_str),
        Some("token1"),
        "provider-specific Gemini env should remain after normalization"
    );
}

#[test]
#[serial]
fn replacing_gemini_common_snippet_tolerates_invalid_stored_snippet() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::gemini_config::get_gemini_dir())
        .expect("create ~/.gemini (initialized)");

    let invalid_old_snippet = r#"{"CC_SWITCH_GEMINI_COMMON":"1""#;
    let new_snippet = r#"{"CC_SWITCH_GEMINI_REPLACED":"1"}"#;

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Gemini);
    config.common_config_snippets.gemini = Some(invalid_old_snippet.to_string());
    {
        let manager = config
            .get_manager_mut(&AppType::Gemini)
            .expect("gemini manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "First".to_string(),
                json!({
                    "env": {
                        "GEMINI_API_KEY": "token1"
                    }
                }),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Second".to_string(),
                json!({
                    "env": {
                        "GEMINI_API_KEY": "token2",
                        "CC_SWITCH_GEMINI_COMMON": "1"
                    }
                }),
                None,
            ),
        );
    }

    crate::gemini_config::write_gemini_env_atomic(&std::collections::HashMap::from([
        ("GEMINI_API_KEY".to_string(), "token1".to_string()),
        ("CC_SWITCH_GEMINI_COMMON".to_string(), "1".to_string()),
    ]))
    .expect("seed current gemini env");

    let state = state_from_config(config);
    state.save().expect("persist config snapshot to db");

    ProviderService::set_common_config_snippet(
        &state,
        AppType::Gemini,
        Some(new_snippet.to_string()),
    )
    .expect("replace should recover from invalid stored snippet");

    let cfg = state.config.read().expect("read config after replace");
    assert_eq!(
        cfg.common_config_snippets.gemini.as_deref(),
        Some(new_snippet),
        "invalid stored snippet should not block replacing the saved common snippet"
    );
    drop(cfg);

    let live_env = crate::gemini_config::read_gemini_env().expect("read gemini env");
    assert_eq!(
        live_env
            .get("CC_SWITCH_GEMINI_REPLACED")
            .map(String::as_str),
        Some("1"),
        "replacing should write the new common snippet into the live Gemini env"
    );
    assert!(
        !live_env.contains_key("CC_SWITCH_GEMINI_COMMON"),
        "replacing should rewrite live Gemini env from the provider snapshot even when the old snippet is invalid"
    );
}

#[test]
#[serial]
fn import_default_config_strips_gemini_common_snippet_before_persisting_snapshot() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    std::fs::create_dir_all(crate::gemini_config::get_gemini_dir())
        .expect("create ~/.gemini (initialized)");

    crate::gemini_config::write_gemini_env_atomic(&std::collections::HashMap::from([
        ("GEMINI_API_KEY".to_string(), "token".to_string()),
        ("CC_SWITCH_GEMINI_COMMON".to_string(), "1".to_string()),
    ]))
    .expect("write gemini env");
    write_json_file(
        &crate::gemini_config::get_gemini_settings_path(),
        &json!({
            "theme": "light",
            "providerOnly": true
        }),
    )
    .expect("write gemini settings.json");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Gemini);
    config.common_config_snippets.gemini = Some(r#"{"CC_SWITCH_GEMINI_COMMON":"1"}"#.to_string());
    let state = state_from_config(config);

    ProviderService::import_default_config(&state, AppType::Gemini)
        .expect("import default gemini config");

    let cfg = state.config.read().expect("read config after import");
    let provider = cfg
        .get_manager(&AppType::Gemini)
        .expect("gemini manager")
        .providers
        .get("default")
        .expect("default provider exists");
    let env = provider
        .settings_config
        .get("env")
        .and_then(Value::as_object)
        .expect("stored gemini env should be object");
    let config_obj = provider
        .settings_config
        .get("config")
        .and_then(Value::as_object)
        .expect("stored gemini config should be object");

    assert!(
        env.contains_key("CC_SWITCH_GEMINI_COMMON"),
        "missing-meta Gemini import should keep common env keys for upstream subset detection"
    );
    assert_eq!(
        env.get("GEMINI_API_KEY").and_then(Value::as_str),
        Some("token"),
        "provider-specific Gemini env should remain after import"
    );
    assert_eq!(
        config_obj.get("theme").and_then(Value::as_str),
        Some("light"),
        "Gemini common snippets are env-scoped and should not strip settings.json keys"
    );
    assert_eq!(
        config_obj.get("providerOnly").and_then(Value::as_bool),
        Some(true),
        "provider-specific Gemini config should remain after import"
    );
}
