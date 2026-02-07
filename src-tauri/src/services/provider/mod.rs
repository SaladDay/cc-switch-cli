mod endpoints;
mod gemini_auth;
mod live;
mod usage;

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app_config::{AppType, MultiAppConfig};
use crate::codex_config::{get_codex_auth_path, get_codex_config_path};
use crate::config::{
    copy_file, delete_file, get_claude_settings_path, get_provider_config_path, read_json_file,
    write_json_file,
};
use crate::error::AppError;
use crate::provider::Provider;
use crate::store::AppState;

use gemini_auth::GeminiAuthType;
use live::LiveSnapshot;

/// 供应商相关业务逻辑
pub struct ProviderService;

#[cfg(test)]
fn state_from_config(config: MultiAppConfig) -> AppState {
    AppState {
        db: std::sync::Arc::new(crate::Database::memory().expect("create memory database")),
        config: std::sync::RwLock::new(config),
    }
}

/// 从供应商名称生成 Codex 的 provider ID (lowercase alphanumeric)
/// 示例: "Duck Coding" -> "duckcoding"
fn generate_provider_id_from_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

fn is_codex_official_provider(provider: &Provider) -> bool {
    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.codex_official)
        .unwrap_or(false)
        || provider
            .category
            .as_deref()
            .is_some_and(|category| category.eq_ignore_ascii_case("official"))
        || provider
            .website_url
            .as_deref()
            .is_some_and(|url| url.trim().eq_ignore_ascii_case("https://chatgpt.com/codex"))
        || provider.name.trim().eq_ignore_ascii_case("OpenAI Official")
}

#[derive(Clone)]
struct PostCommitAction {
    app_type: AppType,
    provider: Provider,
    backup: LiveSnapshot,
    sync_mcp: bool,
    refresh_snapshot: bool,
    common_config_snippet: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::ffi::OsString;
    use std::path::Path;
    use tempfile::TempDir;

    struct EnvGuard {
        old_home: Option<OsString>,
        old_userprofile: Option<OsString>,
    }

    impl EnvGuard {
        fn set_home(home: &Path) -> Self {
            let old_home = std::env::var_os("HOME");
            let old_userprofile = std::env::var_os("USERPROFILE");
            std::env::set_var("HOME", home);
            std::env::set_var("USERPROFILE", home);
            Self {
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
        }
    }

    #[test]
    fn validate_provider_settings_allows_missing_auth_for_codex() {
        let mut provider = Provider::with_id(
            "codex".into(),
            "Codex".into(),
            json!({ "config": "base_url = \"https://example.com\"" }),
            None,
        );
        provider.meta = Some(crate::provider::ProviderMeta {
            codex_official: Some(true),
            ..Default::default()
        });
        ProviderService::validate_provider_settings(&AppType::Codex, &provider)
            .expect("Codex auth is optional for official provider");
    }

    #[test]
    fn validate_provider_settings_allows_missing_auth_for_codex_official_by_category() {
        let mut provider = Provider::with_id(
            "codex".into(),
            "Anything".into(),
            json!({ "config": "base_url = \"https://api.openai.com/v1\"\n" }),
            None,
        );
        provider.category = Some("official".to_string());
        ProviderService::validate_provider_settings(&AppType::Codex, &provider)
            .expect("Codex auth is optional for official providers (category=official)");
    }

    #[test]
    #[serial]
    fn switch_codex_succeeds_without_auth_json() {
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
                        "config": "requires_openai_auth = true\n",
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
                        "config": "requires_openai_auth = true\n",
                    }),
                    None,
                ),
            );
        }

        let state = state_from_config(config);

        ProviderService::switch(&state, AppType::Codex, "p1")
            .expect("switch should succeed without auth.json when using credential store");

        assert!(
            !get_codex_auth_path().exists(),
            "auth.json should remain absent when provider has no auth config"
        );

        let live_config_text =
            std::fs::read_to_string(get_codex_config_path()).expect("read live config.toml");
        let live_config_snippet =
            ProviderService::codex_config_snippet_from_live_config(&live_config_text)
                .expect("extract provider config snippet");

        let guard = state.config.read().expect("read config after switch");
        let manager = guard
            .get_manager(&AppType::Codex)
            .expect("codex manager after switch");
        assert_eq!(manager.current, "p1", "current provider should update");
        let provider = manager.providers.get("p1").expect("p1 exists");
        assert!(
            provider.settings_config.get("auth").is_none(),
            "snapshot should not inject auth when auth.json is absent"
        );
        assert_eq!(
            provider
                .settings_config
                .get("config")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            live_config_snippet,
            "provider snapshot should match extracted snippet even without auth.json"
        );
    }

    #[test]
    #[serial]
    fn codex_switch_removes_existing_auth_json_for_openai_official_provider() {
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
                        "config": "base_url = \"https://third-party.example/v1\"\nmodel = \"gpt-5.2-codex\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n",
                    }),
                    None,
                ),
            );

            let mut official = Provider::with_id(
                "p2".to_string(),
                "OpenAI Official".to_string(),
                json!({
                    "config": "base_url = \"https://api.openai.com/v1\"\nmodel = \"gpt-5.2-codex\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n",
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

        assert!(
            !auth_path.exists(),
            "auth.json should be removed when switching to OpenAI official provider"
        );

        let backup_exists = std::fs::read_dir(crate::codex_config::get_codex_config_dir())
            .expect("read codex dir")
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("auth.json.cc-switch.bak.")
            });
        assert!(
            backup_exists,
            "auth.json should be backed up when removed for OpenAI official provider"
        );
    }

    #[test]
    fn codex_config_snippet_defaults_wire_api_to_responses_for_openai_auth_when_missing() {
        let config_text = r#"
model_provider = "openai"
model = "gpt-5.2-codex"

[model_providers.openai]
base_url = "https://api.openai.com/v1"
requires_openai_auth = true
"#;

        let snippet = ProviderService::codex_config_snippet_from_live_config(config_text)
            .expect("extract provider config snippet");
        assert!(
            snippet.contains("wire_api = \"responses\""),
            "wire_api should default to responses for OpenAI auth when missing from config"
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
                        "config": "base_url = \"https://api.one.example/v1\"\nmodel = \"gpt-4o\"\nwire_api = \"responses\"\nenv_key = \"OPENAI_API_KEY\"\nrequires_openai_auth = false\n",
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
                        "config": "base_url = \"https://api.two.example/v1\"\nmodel = \"gpt-4o\"\nwire_api = \"chat\"\nenv_key = \"OPENAI_API_KEY\"\nrequires_openai_auth = false\n",
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
        let live_snippet =
            ProviderService::codex_config_snippet_from_live_config(&live_text).expect("snippet");
        assert!(
            live_snippet.contains("base_url = \"https://api.one.example/v1\""),
            "live snippet should retain provider base_url after multiple switches"
        );
        assert!(
            live_snippet.contains("wire_api = \"responses\""),
            "live snippet should retain provider wire_api after multiple switches"
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
    fn add_first_provider_sets_current() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let mut config = MultiAppConfig::default();
        config.ensure_app(&AppType::Claude);
        let state = state_from_config(config);

        let provider = Provider::with_id(
            "p1".to_string(),
            "First".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "token",
                    "ANTHROPIC_BASE_URL": "https://claude.example"
                }
            }),
            None,
        );

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
    fn current_self_heals_when_current_provider_missing() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let mut config = MultiAppConfig::default();
        config.ensure_app(&AppType::Claude);
        {
            let manager = config
                .get_manager_mut(&AppType::Claude)
                .expect("claude manager");
            manager.current = "missing".to_string();

            let mut p1 = Provider::with_id(
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
            p1.sort_index = Some(10);

            let mut p2 = Provider::with_id(
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
            p2.sort_index = Some(0);

            manager.providers.insert("p1".to_string(), p1);
            manager.providers.insert("p2".to_string(), p2);
        }

        let state = state_from_config(config);

        let current_id =
            ProviderService::current(&state, AppType::Claude).expect("self-heal current provider");
        assert_eq!(
            current_id, "p2",
            "should pick provider with smaller sort_index"
        );

        let cfg = state.config.read().expect("read config");
        let manager = cfg.get_manager(&AppType::Claude).expect("claude manager");
        assert_eq!(
            manager.current, "p2",
            "current should be updated in config after self-heal"
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

        let provider = Provider::with_id(
            "p1".to_string(),
            "First".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "token",
                    "ANTHROPIC_BASE_URL": "https://claude.example"
                }
            }),
            None,
        );

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
    fn common_config_snippet_is_merged_into_codex_config_on_write() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        std::fs::create_dir_all(crate::codex_config::get_codex_config_dir())
            .expect("create ~/.codex (initialized)");

        let mut config = MultiAppConfig::default();
        config.ensure_app(&AppType::Codex);
        config.common_config_snippets.codex = Some("disable_response_storage = true".to_string());

        let state = state_from_config(config);

        let provider = Provider::with_id(
            "p1".to_string(),
            "First".to_string(),
            json!({
                "auth": { "OPENAI_API_KEY": "sk-test" },
                "config": "base_url = \"https://api.example/v1\"\nmodel = \"gpt-5.2-codex\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
            }),
            None,
        );

        ProviderService::add(&state, AppType::Codex, provider).expect("add should succeed");

        let live_text = std::fs::read_to_string(get_codex_config_path()).expect("read config.toml");
        assert!(
            live_text.contains("disable_response_storage = true"),
            "common snippet should be merged into config.toml"
        );
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
                    json!({ "config": "base_url = \"https://api.one.example/v1\"\n" }),
                    None,
                ),
            );
            manager.providers.insert(
                "p2".to_string(),
                Provider::with_id(
                    "p2".to_string(),
                    "Second".to_string(),
                    json!({ "config": "base_url = \"https://api.two.example/v1\"\n" }),
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
                    json!({ "config": "base_url = \"https://api.one.example/v1\"\n" }),
                    None,
                ),
            );
            manager.providers.insert(
                "p2".to_string(),
                serde_json::from_value(json!({
                    "id": "p2",
                    "name": "Second",
                    "settingsConfig": { "config": "base_url = \"https://api.two.example/v1\"\n" },
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
            "common snippet should be removed when applyCommonConfig=false"
        );
        assert!(
            live_text.contains("network_access = \"restricted\""),
            "unrelated root config should be preserved"
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
        config.common_config_snippets.gemini =
            Some(r#"{"env":{"CC_SWITCH_GEMINI_COMMON":"1"}}"#.to_string());

        let state = state_from_config(config);

        let provider = Provider::with_id(
            "p1".to_string(),
            "First".to_string(),
            json!({
                "env": {
                    "GEMINI_API_KEY": "token"
                }
            }),
            None,
        );

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
    fn common_config_snippet_is_not_persisted_into_gemini_provider_snapshot_on_switch() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let mut config = MultiAppConfig::default();
        config.ensure_app(&AppType::Gemini);
        config.common_config_snippets.gemini =
            Some(r#"{"env":{"CC_SWITCH_GEMINI_COMMON":"1"}}"#.to_string());

        let state = state_from_config(config);

        let p1 = Provider::with_id(
            "p1".to_string(),
            "First".to_string(),
            json!({
                "env": {
                    "GEMINI_API_KEY": "token1"
                }
            }),
            None,
        );
        let p2 = Provider::with_id(
            "p2".to_string(),
            "Second".to_string(),
            json!({
                "env": {
                    "GEMINI_API_KEY": "token2"
                }
            }),
            None,
        );

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
}

fn merge_json_values(base: &mut Value, overlay: &Value) {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            for (key, overlay_value) in overlay_map {
                match base_map.get_mut(key) {
                    Some(base_value) => merge_json_values(base_value, overlay_value),
                    None => {
                        base_map.insert(key.clone(), overlay_value.clone());
                    }
                }
            }
        }
        (base_value, overlay_value) => {
            *base_value = overlay_value.clone();
        }
    }
}

fn strip_common_values(target: &mut Value, common: &Value) {
    match (target, common) {
        (Value::Object(target_map), Value::Object(common_map)) => {
            for (key, common_value) in common_map {
                let should_remove = match target_map.get_mut(key) {
                    Some(target_value) => match target_value {
                        Value::Object(_) if matches!(common_value, Value::Object(_)) => {
                            strip_common_values(target_value, common_value);
                            target_value.as_object().is_some_and(|m| m.is_empty())
                        }
                        _ => target_value == common_value,
                    },
                    None => false,
                };

                if should_remove {
                    target_map.remove(key);
                }
            }
        }
        (target_value, common_value) => {
            if target_value == common_value {
                *target_value = Value::Null;
            }
        }
    }
}

impl ProviderService {
    fn parse_common_claude_config_snippet(snippet: &str) -> Result<Value, AppError> {
        let value: Value = serde_json::from_str(snippet).map_err(|e| {
            AppError::localized(
                "common_config.claude.invalid_json",
                format!("Claude 通用配置片段不是有效的 JSON：{e}"),
                format!("Claude common config snippet is not valid JSON: {e}"),
            )
        })?;
        if !value.is_object() {
            return Err(AppError::localized(
                "common_config.claude.not_object",
                "Claude 通用配置片段必须是 JSON 对象",
                "Claude common config snippet must be a JSON object",
            ));
        }
        Ok(value)
    }

    fn parse_common_gemini_config_snippet(snippet: &str) -> Result<Value, AppError> {
        let value: Value = serde_json::from_str(snippet).map_err(|e| {
            AppError::localized(
                "common_config.gemini.invalid_json",
                format!("Gemini 通用配置片段不是有效的 JSON：{e}"),
                format!("Gemini common config snippet is not valid JSON: {e}"),
            )
        })?;
        if !value.is_object() {
            return Err(AppError::localized(
                "common_config.gemini.not_object",
                "Gemini 通用配置片段必须是 JSON 对象",
                "Gemini common config snippet must be a JSON object",
            ));
        }
        Ok(value)
    }

    fn extract_codex_common_config_from_config_toml(config_toml: &str) -> Result<String, AppError> {
        let config_toml = config_toml.trim();
        if config_toml.is_empty() {
            return Ok(String::new());
        }

        let mut doc = config_toml
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| AppError::Message(format!("TOML parse error: {e}")))?;

        // Remove provider-specific fields.
        let root = doc.as_table_mut();
        root.remove("model");
        root.remove("model_provider");
        // Legacy/alt formats might use a top-level base_url.
        root.remove("base_url");
        // Remove entire model_providers table (provider-specific configuration)
        root.remove("model_providers");

        // Clean up multiple empty lines (keep at most one blank line).
        let mut cleaned = String::new();
        let mut blank_run = 0usize;
        for line in doc.to_string().lines() {
            if line.trim().is_empty() {
                blank_run += 1;
                if blank_run <= 1 {
                    cleaned.push('\n');
                }
                continue;
            }
            blank_run = 0;
            cleaned.push_str(line);
            cleaned.push('\n');
        }

        Ok(cleaned.trim().to_string())
    }

    fn maybe_update_codex_common_config_snippet(
        config: &mut MultiAppConfig,
        config_toml: &str,
    ) -> Result<(), AppError> {
        let existing = config
            .common_config_snippets
            .codex
            .as_deref()
            .unwrap_or_default()
            .trim();
        if !existing.is_empty() {
            return Ok(());
        }

        let extracted = Self::extract_codex_common_config_from_config_toml(config_toml)?;
        if extracted.trim().is_empty() {
            return Ok(());
        }

        config.common_config_snippets.codex = Some(extracted);
        Ok(())
    }

    fn merge_toml_tables(dst: &mut toml_edit::Table, src: &toml_edit::Table) {
        for (key, src_item) in src.iter() {
            match (dst.get_mut(key), src_item.as_table()) {
                (Some(dst_item), Some(src_table)) => {
                    if let Some(dst_table) = dst_item.as_table_mut() {
                        Self::merge_toml_tables(dst_table, src_table);
                    } else {
                        *dst_item = toml_edit::Item::Table(src_table.clone());
                    }
                }
                (Some(dst_item), None) => {
                    *dst_item = src_item.clone();
                }
                (None, _) => {
                    dst.insert(key, src_item.clone());
                }
            }
        }
    }

    fn strip_toml_tables(dst: &mut toml_edit::Table, src: &toml_edit::Table) {
        for (key, src_item) in src.iter() {
            let should_remove = if let Some(src_table) = src_item.as_table() {
                match dst.get_mut(key) {
                    Some(dst_item) => {
                        if let Some(dst_table) = dst_item.as_table_mut() {
                            Self::strip_toml_tables(dst_table, src_table);
                            dst_table.is_empty()
                        } else {
                            true
                        }
                    }
                    None => false,
                }
            } else {
                dst.contains_key(key)
            };

            if should_remove {
                dst.remove(key);
            }
        }
    }

    /// 归一化 Claude 模型键：读旧键(ANTHROPIC_SMALL_FAST_MODEL)，写新键(DEFAULT_*), 并删除旧键
    fn normalize_claude_models_in_value(settings: &mut Value) -> bool {
        let mut changed = false;
        let env = match settings.get_mut("env") {
            Some(v) if v.is_object() => v.as_object_mut().unwrap(),
            _ => return changed,
        };

        let model = env
            .get("ANTHROPIC_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let small_fast = env
            .get("ANTHROPIC_SMALL_FAST_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let current_haiku = env
            .get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let current_sonnet = env
            .get("ANTHROPIC_DEFAULT_SONNET_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let current_opus = env
            .get("ANTHROPIC_DEFAULT_OPUS_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let target_haiku = current_haiku
            .or_else(|| small_fast.clone())
            .or_else(|| model.clone());
        let target_sonnet = current_sonnet
            .or_else(|| model.clone())
            .or_else(|| small_fast.clone());
        let target_opus = current_opus
            .or_else(|| model.clone())
            .or_else(|| small_fast.clone());

        if env.get("ANTHROPIC_DEFAULT_HAIKU_MODEL").is_none() {
            if let Some(v) = target_haiku {
                env.insert(
                    "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(),
                    Value::String(v),
                );
                changed = true;
            }
        }
        if env.get("ANTHROPIC_DEFAULT_SONNET_MODEL").is_none() {
            if let Some(v) = target_sonnet {
                env.insert(
                    "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
                    Value::String(v),
                );
                changed = true;
            }
        }
        if env.get("ANTHROPIC_DEFAULT_OPUS_MODEL").is_none() {
            if let Some(v) = target_opus {
                env.insert("ANTHROPIC_DEFAULT_OPUS_MODEL".to_string(), Value::String(v));
                changed = true;
            }
        }

        if env.remove("ANTHROPIC_SMALL_FAST_MODEL").is_some() {
            changed = true;
        }

        changed
    }

    fn normalize_provider_if_claude(app_type: &AppType, provider: &mut Provider) {
        if matches!(app_type, AppType::Claude) {
            let mut v = provider.settings_config.clone();
            if Self::normalize_claude_models_in_value(&mut v) {
                provider.settings_config = v;
            }
        }
    }
    fn run_transaction<R, F>(state: &AppState, f: F) -> Result<R, AppError>
    where
        F: FnOnce(&mut MultiAppConfig) -> Result<(R, Option<PostCommitAction>), AppError>,
    {
        let mut guard = state.config.write().map_err(AppError::from)?;
        let original = guard.clone();
        let (result, action) = match f(&mut guard) {
            Ok(value) => value,
            Err(err) => {
                *guard = original;
                return Err(err);
            }
        };
        drop(guard);

        if let Err(save_err) = state.save() {
            if let Err(rollback_err) = Self::restore_config_only(state, original.clone()) {
                return Err(AppError::localized(
                    "config.save.rollback_failed",
                    format!("保存配置失败: {save_err}；回滚失败: {rollback_err}"),
                    format!("Failed to save config: {save_err}; rollback failed: {rollback_err}"),
                ));
            }
            return Err(save_err);
        }

        if let Some(action) = action {
            if let Err(err) = Self::apply_post_commit(state, &action) {
                if let Err(rollback_err) =
                    Self::rollback_after_failure(state, original.clone(), action.backup.clone())
                {
                    return Err(AppError::localized(
                        "post_commit.rollback_failed",
                        format!("后置操作失败: {err}；回滚失败: {rollback_err}"),
                        format!("Post-commit step failed: {err}; rollback failed: {rollback_err}"),
                    ));
                }
                return Err(err);
            }
        }

        Ok(result)
    }

    fn restore_config_only(state: &AppState, snapshot: MultiAppConfig) -> Result<(), AppError> {
        {
            let mut guard = state.config.write().map_err(AppError::from)?;
            *guard = snapshot;
        }
        state.save()
    }

    fn rollback_after_failure(
        state: &AppState,
        snapshot: MultiAppConfig,
        backup: LiveSnapshot,
    ) -> Result<(), AppError> {
        Self::restore_config_only(state, snapshot)?;
        backup.restore()
    }

    fn apply_post_commit(state: &AppState, action: &PostCommitAction) -> Result<(), AppError> {
        let apply_common_config = action
            .provider
            .meta
            .as_ref()
            .and_then(|meta| meta.apply_common_config)
            .unwrap_or(true);
        Self::write_live_snapshot(
            &action.app_type,
            &action.provider,
            action.common_config_snippet.as_deref(),
            apply_common_config,
        )?;
        if action.sync_mcp {
            // 使用 v3.7.0 统一的 MCP 同步机制，支持所有应用
            use crate::services::mcp::McpService;
            McpService::sync_all_enabled(state)?;
        }
        if action.refresh_snapshot && crate::sync_policy::should_sync_live(&action.app_type) {
            Self::refresh_provider_snapshot(state, &action.app_type, &action.provider.id)?;
        }

        // D6: Align upstream live flows - also sync skills (best effort, should not block provider ops).
        if let Err(e) = crate::services::skill::SkillService::sync_all_enabled_best_effort() {
            log::warn!("同步 Skills 失败: {e}");
        }
        Ok(())
    }

    fn refresh_provider_snapshot(
        state: &AppState,
        app_type: &AppType,
        provider_id: &str,
    ) -> Result<(), AppError> {
        match app_type {
            AppType::Claude => {
                let settings_path = get_claude_settings_path();
                if !settings_path.exists() {
                    return Err(AppError::localized(
                        "claude.live.missing",
                        "Claude 设置文件不存在，无法刷新快照",
                        "Claude settings file missing; cannot refresh snapshot",
                    ));
                }
                let mut live_after = read_json_file::<Value>(&settings_path)?;
                let _ = Self::normalize_claude_models_in_value(&mut live_after);

                let common_snippet = {
                    let guard = state.config.read().map_err(AppError::from)?;
                    guard.common_config_snippets.claude.clone()
                };
                if let Some(snippet) = common_snippet.as_deref() {
                    let snippet = snippet.trim();
                    if !snippet.is_empty() {
                        let common = Self::parse_common_claude_config_snippet(snippet)?;
                        strip_common_values(&mut live_after, &common);
                    }
                }
                {
                    let mut guard = state.config.write().map_err(AppError::from)?;
                    if let Some(manager) = guard.get_manager_mut(app_type) {
                        if let Some(target) = manager.providers.get_mut(provider_id) {
                            target.settings_config = live_after;
                        }
                    }
                }
                state.save()?;
            }
            AppType::Codex => {
                let auth_path = get_codex_auth_path();
                let auth = if auth_path.exists() {
                    Some(read_json_file::<Value>(&auth_path)?)
                } else {
                    None
                };
                let cfg_text = crate::codex_config::read_and_validate_codex_config_text()?;
                let cfg_snippet = Self::codex_config_snippet_from_live_config(&cfg_text)?;
                let common_snippet = Self::extract_codex_common_config_from_config_toml(&cfg_text)?;

                {
                    let mut guard = state.config.write().map_err(AppError::from)?;
                    if !common_snippet.trim().is_empty()
                        && guard
                            .common_config_snippets
                            .codex
                            .as_deref()
                            .unwrap_or_default()
                            .trim()
                            .is_empty()
                    {
                        guard.common_config_snippets.codex = Some(common_snippet.clone());
                    }
                    if let Some(manager) = guard.get_manager_mut(app_type) {
                        if let Some(target) = manager.providers.get_mut(provider_id) {
                            let obj = target.settings_config.as_object_mut().ok_or_else(|| {
                                AppError::Config(format!(
                                    "供应商 {provider_id} 的 Codex 配置必须是 JSON 对象"
                                ))
                            })?;
                            if let Some(auth) = auth {
                                obj.insert("auth".to_string(), auth);
                            } else {
                                obj.remove("auth");
                            }
                            obj.insert("config".to_string(), Value::String(cfg_snippet.clone()));
                        }
                    }
                }
                state.save()?;
            }
            AppType::Gemini => {
                use crate::gemini_config::{
                    env_to_json, get_gemini_env_path, get_gemini_settings_path, read_gemini_env,
                };

                let env_path = get_gemini_env_path();
                if !env_path.exists() {
                    return Err(AppError::localized(
                        "gemini.live.missing",
                        "Gemini .env 文件不存在，无法刷新快照",
                        "Gemini .env file missing; cannot refresh snapshot",
                    ));
                }
                let env_map = read_gemini_env()?;
                let mut live_after = env_to_json(&env_map);

                let settings_path = get_gemini_settings_path();
                let config_value = if settings_path.exists() {
                    read_json_file(&settings_path)?
                } else {
                    json!({})
                };

                if let Some(obj) = live_after.as_object_mut() {
                    obj.insert("config".to_string(), config_value);
                }

                let common_snippet = {
                    let guard = state.config.read().map_err(AppError::from)?;
                    guard.common_config_snippets.gemini.clone()
                };
                if let Some(snippet) = common_snippet.as_deref() {
                    let snippet = snippet.trim();
                    if !snippet.is_empty() {
                        let common = Self::parse_common_gemini_config_snippet(snippet)?;
                        strip_common_values(&mut live_after, &common);
                    }
                }

                {
                    let mut guard = state.config.write().map_err(AppError::from)?;
                    if let Some(manager) = guard.get_manager_mut(app_type) {
                        if let Some(target) = manager.providers.get_mut(provider_id) {
                            target.settings_config = live_after;
                        }
                    }
                }
                state.save()?;
            }
        }
        Ok(())
    }

    fn capture_live_snapshot(app_type: &AppType) -> Result<LiveSnapshot, AppError> {
        live::capture_live_snapshot(app_type)
    }

    /// 列出指定应用下的所有供应商
    pub fn list(
        state: &AppState,
        app_type: AppType,
    ) -> Result<IndexMap<String, Provider>, AppError> {
        let config = state.config.read().map_err(AppError::from)?;
        let manager = config
            .get_manager(&app_type)
            .ok_or_else(|| Self::app_not_found(&app_type))?;
        Ok(manager.get_all_providers().clone())
    }

    /// 获取当前供应商 ID
    pub fn current(state: &AppState, app_type: AppType) -> Result<String, AppError> {
        {
            let config = state.config.read().map_err(AppError::from)?;
            let manager = config
                .get_manager(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;

            if manager.current.is_empty() || manager.providers.contains_key(&manager.current) {
                return Ok(manager.current.clone());
            }
        }

        let app_type_clone = app_type.clone();
        Self::run_transaction(state, move |config| {
            let manager = config
                .get_manager_mut(&app_type_clone)
                .ok_or_else(|| Self::app_not_found(&app_type_clone))?;

            if manager.current.is_empty() || manager.providers.contains_key(&manager.current) {
                return Ok((manager.current.clone(), None));
            }

            let mut provider_list: Vec<_> = manager.providers.iter().collect();
            provider_list.sort_by(|(_, a), (_, b)| match (a.sort_index, b.sort_index) {
                (Some(idx_a), Some(idx_b)) => idx_a.cmp(&idx_b),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.created_at.cmp(&b.created_at),
            });

            manager.current = provider_list
                .first()
                .map(|(id, _)| (*id).clone())
                .unwrap_or_default();

            Ok((manager.current.clone(), None))
        })
    }

    /// 新增供应商
    pub fn add(state: &AppState, app_type: AppType, provider: Provider) -> Result<bool, AppError> {
        let mut provider = provider;
        // 归一化 Claude 模型键
        Self::normalize_provider_if_claude(&app_type, &mut provider);
        Self::validate_provider_settings(&app_type, &provider)?;

        let app_type_clone = app_type.clone();
        let provider_clone = provider.clone();

        Self::run_transaction(state, move |config| {
            config.ensure_app(&app_type_clone);
            let manager = config
                .get_manager_mut(&app_type_clone)
                .ok_or_else(|| Self::app_not_found(&app_type_clone))?;

            let was_empty = manager.providers.is_empty();
            manager
                .providers
                .insert(provider_clone.id.clone(), provider_clone.clone());

            if was_empty && manager.current.is_empty() {
                manager.current = provider_clone.id.clone();
            }

            let is_current = manager.current == provider_clone.id;
            let action = if is_current {
                let backup = Self::capture_live_snapshot(&app_type_clone)?;
                let common_config_snippet =
                    config.common_config_snippets.get(&app_type_clone).cloned();
                Some(PostCommitAction {
                    app_type: app_type_clone.clone(),
                    provider: provider_clone.clone(),
                    backup,
                    sync_mcp: false,
                    refresh_snapshot: false,
                    common_config_snippet,
                })
            } else {
                None
            };

            Ok((true, action))
        })
    }

    /// 更新供应商
    pub fn update(
        state: &AppState,
        app_type: AppType,
        provider: Provider,
    ) -> Result<bool, AppError> {
        let mut provider = provider;
        // 归一化 Claude 模型键
        Self::normalize_provider_if_claude(&app_type, &mut provider);
        Self::validate_provider_settings(&app_type, &provider)?;
        let provider_id = provider.id.clone();
        let app_type_clone = app_type.clone();
        let provider_clone = provider.clone();

        Self::run_transaction(state, move |config| {
            let manager = config
                .get_manager_mut(&app_type_clone)
                .ok_or_else(|| Self::app_not_found(&app_type_clone))?;

            if !manager.providers.contains_key(&provider_id) {
                return Err(AppError::localized(
                    "provider.not_found",
                    format!("供应商不存在: {provider_id}"),
                    format!("Provider not found: {provider_id}"),
                ));
            }

            let is_current = manager.current == provider_id;
            let merged = if let Some(existing) = manager.providers.get(&provider_id) {
                let mut updated = provider_clone.clone();
                match (existing.meta.as_ref(), updated.meta.take()) {
                    // 前端未提供 meta，表示不修改，沿用旧值
                    (Some(old_meta), None) => {
                        updated.meta = Some(old_meta.clone());
                    }
                    (None, None) => {
                        updated.meta = None;
                    }
                    // 前端提供的 meta 视为权威，直接覆盖（其中 custom_endpoints 允许是空，表示删除所有自定义端点）
                    (_old, Some(new_meta)) => {
                        updated.meta = Some(new_meta);
                    }
                }
                updated
            } else {
                provider_clone.clone()
            };

            manager.providers.insert(provider_id.clone(), merged);

            let action = if is_current {
                let backup = Self::capture_live_snapshot(&app_type_clone)?;
                let common_config_snippet =
                    config.common_config_snippets.get(&app_type_clone).cloned();
                Some(PostCommitAction {
                    app_type: app_type_clone.clone(),
                    provider: provider_clone.clone(),
                    backup,
                    sync_mcp: false,
                    refresh_snapshot: false,
                    common_config_snippet,
                })
            } else {
                None
            };

            Ok((true, action))
        })
    }

    /// 导入当前 live 配置为默认供应商
    pub fn import_default_config(state: &AppState, app_type: AppType) -> Result<(), AppError> {
        {
            let config = state.config.read().map_err(AppError::from)?;
            if let Some(manager) = config.get_manager(&app_type) {
                if !manager.get_all_providers().is_empty() {
                    return Ok(());
                }
            }
        }

        let settings_config = match app_type {
            AppType::Codex => {
                let auth_path = get_codex_auth_path();
                if !auth_path.exists() {
                    return Err(AppError::localized(
                        "codex.live.missing",
                        "Codex 配置文件不存在",
                        "Codex configuration file is missing",
                    ));
                }
                let auth: Value = read_json_file(&auth_path)?;
                let config_str = crate::codex_config::read_and_validate_codex_config_text()?;
                json!({ "auth": auth, "config": config_str })
            }
            AppType::Claude => {
                let settings_path = get_claude_settings_path();
                if !settings_path.exists() {
                    return Err(AppError::localized(
                        "claude.live.missing",
                        "Claude Code 配置文件不存在",
                        "Claude settings file is missing",
                    ));
                }
                let mut v = read_json_file::<Value>(&settings_path)?;
                let _ = Self::normalize_claude_models_in_value(&mut v);
                v
            }
            AppType::Gemini => {
                use crate::gemini_config::{
                    env_to_json, get_gemini_env_path, get_gemini_settings_path, read_gemini_env,
                };

                // 读取 .env 文件（环境变量）
                let env_path = get_gemini_env_path();
                if !env_path.exists() {
                    return Err(AppError::localized(
                        "gemini.live.missing",
                        "Gemini 配置文件不存在",
                        "Gemini configuration file is missing",
                    ));
                }

                let env_map = read_gemini_env()?;
                let env_json = env_to_json(&env_map);
                let env_obj = env_json.get("env").cloned().unwrap_or_else(|| json!({}));

                // 读取 settings.json 文件（MCP 配置等）
                let settings_path = get_gemini_settings_path();
                let config_obj = if settings_path.exists() {
                    read_json_file(&settings_path)?
                } else {
                    json!({})
                };

                // 返回完整结构：{ "env": {...}, "config": {...} }
                json!({
                    "env": env_obj,
                    "config": config_obj
                })
            }
        };

        let mut provider = Provider::with_id(
            "default".to_string(),
            "default".to_string(),
            settings_config,
            None,
        );
        provider.category = Some("custom".to_string());

        {
            let mut config = state.config.write().map_err(AppError::from)?;
            let manager = config
                .get_manager_mut(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;
            manager
                .providers
                .insert(provider.id.clone(), provider.clone());
            manager.current = provider.id.clone();
        }

        state.save()?;
        Ok(())
    }

    /// 读取当前 live 配置
    pub fn read_live_settings(app_type: AppType) -> Result<Value, AppError> {
        match app_type {
            AppType::Codex => {
                let auth_path = get_codex_auth_path();
                if !auth_path.exists() {
                    return Err(AppError::localized(
                        "codex.auth.missing",
                        "Codex 配置文件不存在：缺少 auth.json",
                        "Codex configuration missing: auth.json not found",
                    ));
                }
                let auth: Value = read_json_file(&auth_path)?;
                let cfg_text = crate::codex_config::read_and_validate_codex_config_text()?;
                Ok(json!({ "auth": auth, "config": cfg_text }))
            }
            AppType::Claude => {
                let path = get_claude_settings_path();
                if !path.exists() {
                    return Err(AppError::localized(
                        "claude.live.missing",
                        "Claude Code 配置文件不存在",
                        "Claude settings file is missing",
                    ));
                }
                read_json_file(&path)
            }
            AppType::Gemini => {
                use crate::gemini_config::{
                    env_to_json, get_gemini_env_path, get_gemini_settings_path, read_gemini_env,
                };

                // 读取 .env 文件（环境变量）
                let env_path = get_gemini_env_path();
                if !env_path.exists() {
                    return Err(AppError::localized(
                        "gemini.env.missing",
                        "Gemini .env 文件不存在",
                        "Gemini .env file not found",
                    ));
                }

                let env_map = read_gemini_env()?;
                let env_json = env_to_json(&env_map);
                let env_obj = env_json.get("env").cloned().unwrap_or_else(|| json!({}));

                // 读取 settings.json 文件（MCP 配置等）
                let settings_path = get_gemini_settings_path();
                let config_obj = if settings_path.exists() {
                    read_json_file(&settings_path)?
                } else {
                    json!({})
                };

                // 返回完整结构：{ "env": {...}, "config": {...} }
                Ok(json!({
                    "env": env_obj,
                    "config": config_obj
                }))
            }
        }
    }

    /// 更新供应商排序
    pub fn update_sort_order(
        state: &AppState,
        app_type: AppType,
        updates: Vec<ProviderSortUpdate>,
    ) -> Result<bool, AppError> {
        {
            let mut cfg = state.config.write().map_err(AppError::from)?;
            let manager = cfg
                .get_manager_mut(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;

            for update in updates {
                if let Some(provider) = manager.providers.get_mut(&update.id) {
                    provider.sort_index = Some(update.sort_index);
                }
            }
        }

        state.save()?;
        Ok(true)
    }

    /// 切换指定应用的供应商
    pub fn switch(state: &AppState, app_type: AppType, provider_id: &str) -> Result<(), AppError> {
        let app_type_clone = app_type.clone();
        let provider_id_owned = provider_id.to_string();

        Self::run_transaction(state, move |config| {
            let backup = Self::capture_live_snapshot(&app_type_clone)?;
            let provider = match app_type_clone {
                AppType::Codex => Self::prepare_switch_codex(config, &provider_id_owned)?,
                AppType::Claude => Self::prepare_switch_claude(config, &provider_id_owned)?,
                AppType::Gemini => Self::prepare_switch_gemini(config, &provider_id_owned)?,
            };

            let action = PostCommitAction {
                app_type: app_type_clone.clone(),
                provider,
                backup,
                sync_mcp: true, // v3.7.0: 所有应用切换时都同步 MCP，防止配置丢失
                refresh_snapshot: true,
                common_config_snippet: config.common_config_snippets.get(&app_type_clone).cloned(),
            };

            Ok(((), Some(action)))
        })
    }

    /// 从 Codex 的 `config.toml` 中提取当前 provider 的“供应商片段配置”（用于写入到 CC-Switch 的 provider.settings_config.config）。
    ///
    /// CC-Switch 约定：Codex provider 的 `settings_config.config` 只存与该 provider 相关的字段（如 base_url / model / wire_api / env_key 等），
    /// **不**保存整份 `~/.codex/config.toml`，否则二次切换会因字段位置不同而丢失 base_url / wire_api。
    fn codex_config_snippet_from_live_config(config_text: &str) -> Result<String, AppError> {
        if config_text.trim().is_empty() {
            return Ok(String::new());
        }

        let root: toml::Table = toml::from_str(config_text).map_err(|e| {
            AppError::Config(format!(
                "解析 {} 失败: {e}",
                get_codex_config_path().display()
            ))
        })?;

        let model_provider = root
            .get("model_provider")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let model = root
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("gpt-5.2-codex");

        let provider_table = root
            .get("model_providers")
            .and_then(|v| v.as_table())
            .and_then(|table| {
                model_provider
                    .as_deref()
                    .and_then(|provider_id| table.get(provider_id))
            })
            .and_then(|v| v.as_table());

        // 优先从 model_providers.<id> 读取；否则尝试读取旧版的根级字段（向后兼容）
        let base_url = provider_table
            .and_then(|t| t.get("base_url"))
            .and_then(|v| v.as_str())
            .or_else(|| root.get("base_url").and_then(|v| v.as_str()))
            .unwrap_or("");

        let requires_openai_auth = provider_table
            .and_then(|t| t.get("requires_openai_auth"))
            .and_then(|v| v.as_bool())
            .or_else(|| root.get("requires_openai_auth").and_then(|v| v.as_bool()));
        let env_key = provider_table
            .and_then(|t| t.get("env_key"))
            .and_then(|v| v.as_str())
            .or_else(|| root.get("env_key").and_then(|v| v.as_str()));
        let wire_api = provider_table
            .and_then(|t| t.get("wire_api"))
            .and_then(|v| v.as_str())
            .or_else(|| root.get("wire_api").and_then(|v| v.as_str()))
            .unwrap_or_else(|| {
                if requires_openai_auth == Some(true)
                    || base_url.trim_start().starts_with("https://api.openai.com")
                {
                    "responses"
                } else {
                    "chat"
                }
            });

        let mut lines = Vec::new();
        if !base_url.trim().is_empty() {
            lines.push(format!("base_url = \"{}\"", base_url.trim()));
        }
        lines.push(format!("model = \"{}\"", model.trim()));
        lines.push(format!("wire_api = \"{}\"", wire_api.trim()));

        match requires_openai_auth {
            Some(true) => {
                lines.push("requires_openai_auth = true".to_string());
            }
            Some(false) => {
                if let Some(env_key) = env_key {
                    let env_key = env_key.trim();
                    if !env_key.is_empty() {
                        lines.push(format!("env_key = \"{}\"", env_key));
                    }
                }
                lines.push("requires_openai_auth = false".to_string());
            }
            None => {
                if let Some(env_key) = env_key {
                    let env_key = env_key.trim();
                    if !env_key.is_empty() {
                        lines.push(format!("env_key = \"{}\"", env_key));
                        // 防止 write_codex_live 推断错误的 OpenAI auth 模式
                        lines.push("requires_openai_auth = false".to_string());
                    }
                }
            }
        }

        Ok(lines.join("\n"))
    }

    fn prepare_switch_codex(
        config: &mut MultiAppConfig,
        provider_id: &str,
    ) -> Result<Provider, AppError> {
        let provider = config
            .get_manager(&AppType::Codex)
            .ok_or_else(|| Self::app_not_found(&AppType::Codex))?
            .providers
            .get(provider_id)
            .cloned()
            .ok_or_else(|| {
                AppError::localized(
                    "provider.not_found",
                    format!("供应商不存在: {provider_id}"),
                    format!("Provider not found: {provider_id}"),
                )
            })?;

        Self::backfill_codex_current(config, provider_id)?;

        if let Some(manager) = config.get_manager_mut(&AppType::Codex) {
            manager.current = provider_id.to_string();
        }

        Ok(provider)
    }

    fn backfill_codex_current(
        config: &mut MultiAppConfig,
        next_provider: &str,
    ) -> Result<(), AppError> {
        let current_id = config
            .get_manager(&AppType::Codex)
            .map(|m| m.current.clone())
            .unwrap_or_default();

        if current_id.is_empty() || current_id == next_provider {
            return Ok(());
        }

        let auth_path = get_codex_auth_path();
        let config_path = get_codex_config_path();
        if !auth_path.exists() && !config_path.exists() {
            return Ok(());
        }

        let auth = if auth_path.exists() {
            Some(read_json_file::<Value>(&auth_path)?)
        } else {
            None
        };
        let config_snippet = if config_path.exists() {
            let config_text =
                std::fs::read_to_string(&config_path).map_err(|e| AppError::io(&config_path, e))?;
            Self::maybe_update_codex_common_config_snippet(config, &config_text)?;
            Some(Self::codex_config_snippet_from_live_config(&config_text)?)
        } else {
            None
        };

        if let Some(manager) = config.get_manager_mut(&AppType::Codex) {
            if let Some(current) = manager.providers.get_mut(&current_id) {
                if !current.settings_config.is_object() {
                    current.settings_config = json!({});
                }

                let obj = current.settings_config.as_object_mut().unwrap();
                if let Some(auth) = auth {
                    obj.insert("auth".to_string(), auth);
                }
                if let Some(config_snippet) = config_snippet {
                    obj.insert("config".to_string(), Value::String(config_snippet));
                }
            }
        }

        Ok(())
    }

    fn write_codex_live(
        provider: &Provider,
        common_config_snippet: Option<&str>,
        apply_common_config: bool,
    ) -> Result<(), AppError> {
        use toml_edit::{value, Item, Table};

        if !crate::sync_policy::should_sync_live(&AppType::Codex) {
            return Ok(());
        }

        let settings = provider
            .settings_config
            .as_object()
            .ok_or_else(|| AppError::Config("Codex 配置必须是 JSON 对象".into()))?;

        // auth 字段现在是可选的（Codex 0.64+ 使用环境变量）
        let auth = settings.get("auth");
        let auth_is_empty = auth
            .map(|a| a.as_object().map(|o| o.is_empty()).unwrap_or(true))
            .unwrap_or(true);

        // 获取存储的 config TOML 文本
        let cfg_text = settings.get("config").and_then(Value::as_str).unwrap_or("");

        // 解析存储的 TOML 以提取字段（如果为空则使用默认值）
        let stored_config: toml::Table = if cfg_text.trim().is_empty() {
            toml::Table::new()
        } else {
            toml::from_str(cfg_text).map_err(|e| {
                AppError::Config(format!(
                    "解析供应商 {} 的 config TOML 失败: {}",
                    provider.id, e
                ))
            })?
        };

        // 兼容：部分旧版本/外部导入可能把“整份 config.toml”写进 provider.config。
        // 对于这种情况，base_url / wire_api / requires_openai_auth / env_key 等应从
        // model_provider + model_providers.<id> 中提取，而不是只看根级字段。
        let model_provider_in_config = stored_config
            .get("model_provider")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let provider_table_from_full_config = stored_config
            .get("model_providers")
            .and_then(|v| v.as_table())
            .and_then(|providers| {
                model_provider_in_config
                    .as_deref()
                    .and_then(|id| providers.get(id))
            })
            .and_then(|v| v.as_table());

        // 提取必要字段
        let base_url = provider_table_from_full_config
            .and_then(|t| t.get("base_url"))
            .and_then(|v| v.as_str())
            .or_else(|| stored_config.get("base_url").and_then(|v| v.as_str()))
            .unwrap_or("");
        let model = stored_config
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("gpt-5.2-codex");
        let env_key = provider_table_from_full_config
            .and_then(|t| t.get("env_key"))
            .and_then(|v| v.as_str())
            .or_else(|| stored_config.get("env_key").and_then(|v| v.as_str()))
            .map(str::trim)
            .filter(|s| !s.is_empty());
        // Back-compat: older/partial snippets may omit `requires_openai_auth`. For the official
        // OpenAI endpoint, default to OpenAI auth mode to avoid forcing users to export
        // `OPENAI_API_KEY` unless they explicitly opted into env var mode.
        let inferred_requires_openai_auth =
            base_url.trim_start().starts_with("https://api.openai.com")
                && (env_key.is_none() || (env_key == Some("OPENAI_API_KEY") && !auth_is_empty));
        let requires_openai_auth = provider_table_from_full_config
            .and_then(|t| t.get("requires_openai_auth"))
            .and_then(|v| v.as_bool())
            .or_else(|| {
                stored_config
                    .get("requires_openai_auth")
                    .and_then(|v| v.as_bool())
            })
            .unwrap_or(inferred_requires_openai_auth);
        let wire_api = provider_table_from_full_config
            .and_then(|t| t.get("wire_api"))
            .and_then(|v| v.as_str())
            .or_else(|| stored_config.get("wire_api").and_then(|v| v.as_str()))
            .unwrap_or_else(|| {
                // Heuristic defaults:
                // - OpenAI official base_url (or OpenAI auth mode) → responses
                // - Otherwise → chat
                if requires_openai_auth
                    || base_url.trim_start().starts_with("https://api.openai.com")
                {
                    "responses"
                } else {
                    "chat"
                }
            });

        // 从供应商名称生成 provider ID
        let provider_id = generate_provider_id_from_name(&provider.name);

        // 读取现有 config.toml（保留 MCP 服务器等其他配置）
        let base_text = crate::codex_config::read_and_validate_codex_config_text()?;
        let mut doc = if base_text.trim().is_empty() {
            toml_edit::DocumentMut::default()
        } else {
            base_text
                .parse::<toml_edit::DocumentMut>()
                .map_err(|e| AppError::Config(format!("解析 config.toml 失败: {}", e)))?
        };

        if let Some(snippet) = common_config_snippet {
            let snippet = snippet.trim();
            if !snippet.is_empty() {
                let common_doc = snippet.parse::<toml_edit::DocumentMut>().map_err(|e| {
                    AppError::localized(
                        "common_config.codex.invalid_toml",
                        format!("Codex 通用配置片段不是有效的 TOML：{e}"),
                        format!("Codex common config snippet is not valid TOML: {e}"),
                    )
                })?;
                if apply_common_config {
                    Self::merge_toml_tables(doc.as_table_mut(), common_doc.as_table());
                } else {
                    Self::strip_toml_tables(doc.as_table_mut(), common_doc.as_table());
                }
            }
        }

        // 移除不应该在根级别的字段
        doc.as_table_mut().remove("base_url");
        doc.as_table_mut().remove("wire_api");
        doc.as_table_mut().remove("env_key");
        doc.as_table_mut().remove("requires_openai_auth");

        // 设置根级别字段
        doc["model_provider"] = value(&provider_id);
        doc["model"] = value(model);

        // 从 stored_config 复制其他根级别字段（如果有）
        for (key, val) in stored_config.iter() {
            match key.as_str() {
                // 跳过已处理的字段和应该在 provider section 的字段
                "base_url"
                | "wire_api"
                | "env_key"
                | "requires_openai_auth"
                | "name"
                | "model_provider"
                | "model_providers"
                | "mcp_servers" => continue,
                "model" => continue, // 已在上面设置
                // 复制其他根级别字段（如 model_reasoning_effort, network_access 等）
                _ => {
                    if let Some(toml_val) = Self::toml_value_to_toml_edit_value(val) {
                        doc[key] = Item::Value(toml_val);
                    }
                }
            }
        }

        // 构建 [model_providers.<id>] 表
        let mut provider_table = Table::new();
        provider_table["name"] = value(&provider_id);
        if !base_url.is_empty() {
            provider_table["base_url"] = value(base_url);
        }
        provider_table["wire_api"] = value(wire_api);
        if requires_openai_auth {
            provider_table["requires_openai_auth"] = value(true);
        } else if let Some(env_key) = env_key {
            provider_table["env_key"] = value(env_key);
        }

        // 确保 model_providers 表存在
        if !doc.contains_key("model_providers") {
            doc["model_providers"] = Item::Table(Table::new());
        }

        // 设置 provider
        if let Some(providers_item) = doc.get_mut("model_providers") {
            if let Some(providers_table) = providers_item.as_table_like_mut() {
                providers_table.insert(&provider_id, Item::Table(provider_table));
            }
        }

        // 写回 config.toml
        let new_text = doc.to_string();
        let config_path = get_codex_config_path();
        crate::config::write_text_file(&config_path, &new_text)?;

        // auth.json handling:
        //
        // Codex has two auth modes:
        // - API Key mode (auth.json): third-party/custom providers that explicitly carry auth.
        // - Credential store / OpenAI official mode: auth.json must be absent, otherwise it
        //   overrides the credential store.
        //
        // Align with upstream UI behavior:
        // - If provider has no auth (or is explicitly marked as official), remove existing auth.json.
        // - Otherwise, write auth.json from provider.auth.
        let auth_path = get_codex_auth_path();
        let should_remove_auth_json = auth_is_empty || is_codex_official_provider(provider);
        if should_remove_auth_json {
            if auth_path.exists() {
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                let backup_path = auth_path.with_file_name(format!("auth.json.cc-switch.bak.{ts}"));
                copy_file(&auth_path, &backup_path)?;
                delete_file(&auth_path)?;
            }
        } else if let Some(auth_value) = auth {
            write_json_file(&auth_path, auth_value)?;
        }

        Ok(())
    }

    /// 将 toml::Value 转换为 toml_edit::Value
    fn toml_value_to_toml_edit_value(val: &toml::Value) -> Option<toml_edit::Value> {
        use toml_edit::Value as EditValue;
        match val {
            toml::Value::String(s) => Some(EditValue::from(s.as_str())),
            toml::Value::Integer(i) => Some(EditValue::from(*i)),
            toml::Value::Float(f) => Some(EditValue::from(*f)),
            toml::Value::Boolean(b) => Some(EditValue::from(*b)),
            _ => None, // 暂不处理数组和表
        }
    }

    fn prepare_switch_claude(
        config: &mut MultiAppConfig,
        provider_id: &str,
    ) -> Result<Provider, AppError> {
        let provider = config
            .get_manager(&AppType::Claude)
            .ok_or_else(|| Self::app_not_found(&AppType::Claude))?
            .providers
            .get(provider_id)
            .cloned()
            .ok_or_else(|| {
                AppError::localized(
                    "provider.not_found",
                    format!("供应商不存在: {provider_id}"),
                    format!("Provider not found: {provider_id}"),
                )
            })?;

        Self::backfill_claude_current(config, provider_id)?;

        if let Some(manager) = config.get_manager_mut(&AppType::Claude) {
            manager.current = provider_id.to_string();
        }

        Ok(provider)
    }

    fn prepare_switch_gemini(
        config: &mut MultiAppConfig,
        provider_id: &str,
    ) -> Result<Provider, AppError> {
        let provider = config
            .get_manager(&AppType::Gemini)
            .ok_or_else(|| Self::app_not_found(&AppType::Gemini))?
            .providers
            .get(provider_id)
            .cloned()
            .ok_or_else(|| {
                AppError::localized(
                    "provider.not_found",
                    format!("供应商不存在: {provider_id}"),
                    format!("Provider not found: {provider_id}"),
                )
            })?;

        Self::backfill_gemini_current(config, provider_id)?;

        if let Some(manager) = config.get_manager_mut(&AppType::Gemini) {
            manager.current = provider_id.to_string();
        }

        Ok(provider)
    }

    fn backfill_claude_current(
        config: &mut MultiAppConfig,
        next_provider: &str,
    ) -> Result<(), AppError> {
        let settings_path = get_claude_settings_path();
        if !settings_path.exists() {
            return Ok(());
        }

        let current_id = config
            .get_manager(&AppType::Claude)
            .map(|m| m.current.clone())
            .unwrap_or_default();
        if current_id.is_empty() || current_id == next_provider {
            return Ok(());
        }

        let mut live = read_json_file::<Value>(&settings_path)?;
        let _ = Self::normalize_claude_models_in_value(&mut live);
        if let Some(snippet) = config.common_config_snippets.claude.as_deref() {
            let snippet = snippet.trim();
            if !snippet.is_empty() {
                let common = Self::parse_common_claude_config_snippet(snippet)?;
                strip_common_values(&mut live, &common);
            }
        }
        if let Some(manager) = config.get_manager_mut(&AppType::Claude) {
            if let Some(current) = manager.providers.get_mut(&current_id) {
                current.settings_config = live;
            }
        }

        Ok(())
    }

    fn backfill_gemini_current(
        config: &mut MultiAppConfig,
        next_provider: &str,
    ) -> Result<(), AppError> {
        use crate::gemini_config::{
            env_to_json, get_gemini_env_path, get_gemini_settings_path, read_gemini_env,
        };

        let env_path = get_gemini_env_path();
        if !env_path.exists() {
            return Ok(());
        }

        let current_id = config
            .get_manager(&AppType::Gemini)
            .map(|m| m.current.clone())
            .unwrap_or_default();
        if current_id.is_empty() || current_id == next_provider {
            return Ok(());
        }

        let env_map = read_gemini_env()?;
        let mut live = env_to_json(&env_map);

        let settings_path = get_gemini_settings_path();
        let config_value = if settings_path.exists() {
            read_json_file(&settings_path)?
        } else {
            json!({})
        };
        if let Some(obj) = live.as_object_mut() {
            obj.insert("config".to_string(), config_value);
        }

        if let Some(snippet) = config.common_config_snippets.gemini.as_deref() {
            let snippet = snippet.trim();
            if !snippet.is_empty() {
                let common = Self::parse_common_gemini_config_snippet(snippet)?;
                strip_common_values(&mut live, &common);
            }
        }

        if let Some(manager) = config.get_manager_mut(&AppType::Gemini) {
            if let Some(current) = manager.providers.get_mut(&current_id) {
                current.settings_config = live;
            }
        }

        Ok(())
    }

    fn write_claude_live(
        provider: &Provider,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        if !crate::sync_policy::should_sync_live(&AppType::Claude) {
            return Ok(());
        }

        let settings_path = get_claude_settings_path();
        let mut provider_content = provider.settings_config.clone();
        let _ = Self::normalize_claude_models_in_value(&mut provider_content);

        let content_to_write = if let Some(snippet) = common_config_snippet {
            let snippet = snippet.trim();
            if snippet.is_empty() {
                provider_content
            } else {
                let common = Self::parse_common_claude_config_snippet(snippet)?;
                let mut merged = common;
                merge_json_values(&mut merged, &provider_content);
                let _ = Self::normalize_claude_models_in_value(&mut merged);
                merged
            }
        } else {
            provider_content
        };

        write_json_file(&settings_path, &content_to_write)?;
        Ok(())
    }

    pub(crate) fn write_gemini_live(
        provider: &Provider,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        Self::write_gemini_live_impl(provider, common_config_snippet, false)
    }

    pub(crate) fn write_gemini_live_force(
        provider: &Provider,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        Self::write_gemini_live_impl(provider, common_config_snippet, true)
    }

    fn write_gemini_live_impl(
        provider: &Provider,
        common_config_snippet: Option<&str>,
        force_sync: bool,
    ) -> Result<(), AppError> {
        use crate::gemini_config::{
            get_gemini_settings_path, json_to_env, validate_gemini_settings_strict,
            write_gemini_env_atomic,
        };

        // 一次性检测认证类型，避免重复检测
        let auth_type = Self::detect_gemini_auth_type(provider);

        if !force_sync && !crate::sync_policy::should_sync_live(&AppType::Gemini) {
            // still update CC-Switch app-level settings, but do not create any ~/.gemini files
            match auth_type {
                GeminiAuthType::GoogleOfficial => {
                    Self::ensure_google_oauth_security_flag(provider)?
                }
                GeminiAuthType::ApiKey => Self::ensure_api_key_security_flag(provider)?,
            }
            return Ok(());
        }

        let provider_content = provider.settings_config.clone();
        let content_to_write = if let Some(snippet) = common_config_snippet {
            let snippet = snippet.trim();
            if snippet.is_empty() {
                provider_content
            } else {
                let common = Self::parse_common_gemini_config_snippet(snippet)?;
                let mut merged = common;
                merge_json_values(&mut merged, &provider_content);
                merged
            }
        } else {
            provider_content
        };

        let mut env_map = json_to_env(&content_to_write)?;

        // 准备要写入 ~/.gemini/settings.json 的配置（缺省时保留现有文件内容）
        let settings_path = get_gemini_settings_path();
        let mut config_to_write = if let Some(config_value) = content_to_write.get("config") {
            if config_value.is_null() {
                None // null → 保留现有文件
            } else if let Some(provider_config) = config_value.as_object() {
                if provider_config.is_empty() {
                    None // 空对象 {} → 保留现有文件
                } else {
                    // 有内容 → 合并到现有 settings.json（保留现有 key，如 mcpServers），供应商优先
                    let mut merged = if settings_path.exists() {
                        read_json_file(&settings_path)?
                    } else {
                        json!({})
                    };

                    if !merged.is_object() {
                        merged = json!({});
                    }

                    let merged_map = merged.as_object_mut().ok_or_else(|| {
                        AppError::localized(
                            "gemini.validation.invalid_settings",
                            "Gemini 现有 settings.json 格式错误: 必须是对象",
                            "Gemini existing settings.json invalid: must be a JSON object",
                        )
                    })?;
                    for (key, value) in provider_config {
                        merged_map.insert(key.clone(), value.clone());
                    }

                    Some(merged)
                }
            } else {
                return Err(AppError::localized(
                    "gemini.validation.invalid_config",
                    "Gemini 配置格式错误: config 必须是对象或 null",
                    "Gemini config invalid: config must be an object or null",
                ));
            }
        } else {
            None
        };

        if config_to_write.is_none() {
            if settings_path.exists() {
                config_to_write = Some(read_json_file(&settings_path)?);
            } else {
                config_to_write = Some(json!({})); // 新建空配置
            }
        }

        match auth_type {
            GeminiAuthType::GoogleOfficial => {
                // Google 官方使用 OAuth，清空 env
                env_map.clear();
                write_gemini_env_atomic(&env_map)?;
            }
            GeminiAuthType::ApiKey => {
                // API Key 供应商（所有第三方服务）
                // 统一处理：验证配置 + 写入 .env 文件
                validate_gemini_settings_strict(&content_to_write)?;
                write_gemini_env_atomic(&env_map)?;
            }
        }

        if let Some(config_value) = config_to_write {
            write_json_file(&settings_path, &config_value)?;
        }

        match auth_type {
            GeminiAuthType::GoogleOfficial => Self::ensure_google_oauth_security_flag(provider)?,
            GeminiAuthType::ApiKey => Self::ensure_api_key_security_flag(provider)?,
        }

        Ok(())
    }

    fn write_live_snapshot(
        app_type: &AppType,
        provider: &Provider,
        common_config_snippet: Option<&str>,
        apply_common_config: bool,
    ) -> Result<(), AppError> {
        match app_type {
            AppType::Codex => {
                Self::write_codex_live(provider, common_config_snippet, apply_common_config)
            }
            AppType::Claude => Self::write_claude_live(
                provider,
                if apply_common_config {
                    common_config_snippet
                } else {
                    None
                },
            ),
            AppType::Gemini => Self::write_gemini_live(
                provider,
                if apply_common_config {
                    common_config_snippet
                } else {
                    None
                },
            ), // 新增
        }
    }

    fn validate_provider_settings(app_type: &AppType, provider: &Provider) -> Result<(), AppError> {
        match app_type {
            AppType::Claude => {
                if !provider.settings_config.is_object() {
                    return Err(AppError::localized(
                        "provider.claude.settings.not_object",
                        "Claude 配置必须是 JSON 对象",
                        "Claude configuration must be a JSON object",
                    ));
                }
            }
            AppType::Codex => {
                let settings = provider.settings_config.as_object().ok_or_else(|| {
                    AppError::localized(
                        "provider.codex.settings.not_object",
                        "Codex 配置必须是 JSON 对象",
                        "Codex configuration must be a JSON object",
                    )
                })?;

                let is_official = is_codex_official_provider(provider);

                // config 字段必须存在且是字符串
                let config_value = settings.get("config").ok_or_else(|| {
                    AppError::localized(
                        "provider.codex.config.missing",
                        format!("供应商 {} 缺少 config 配置", provider.id),
                        format!("Provider {} is missing config configuration", provider.id),
                    )
                })?;
                if !(config_value.is_string() || config_value.is_null()) {
                    return Err(AppError::localized(
                        "provider.codex.config.invalid_type",
                        "Codex config 字段必须是字符串",
                        "Codex config field must be a string",
                    ));
                }
                if let Some(cfg_text) = config_value.as_str() {
                    crate::codex_config::validate_config_toml(cfg_text)?;
                }

                // auth 规则：
                // - 官方供应商：auth 可选（使用 codex login 保存的凭证）
                // - 第三方/自定义：必须提供 auth.OPENAI_API_KEY
                match settings.get("auth") {
                    Some(auth) => {
                        let auth_obj = auth.as_object().ok_or_else(|| {
                            AppError::localized(
                                "provider.codex.auth.not_object",
                                format!("供应商 {} 的 auth 配置必须是 JSON 对象", provider.id),
                                format!(
                                    "Provider {} auth configuration must be a JSON object",
                                    provider.id
                                ),
                            )
                        })?;
                        if !is_official {
                            let api_key = auth_obj
                                .get("OPENAI_API_KEY")
                                .and_then(|v| v.as_str())
                                .map(str::trim)
                                .unwrap_or("");
                            if api_key.is_empty() {
                                return Err(AppError::localized(
                                    "provider.codex.api_key.missing",
                                    format!("供应商 {} 缺少 OPENAI_API_KEY", provider.id),
                                    format!("Provider {} is missing OPENAI_API_KEY", provider.id),
                                ));
                            }
                        }
                    }
                    None => {
                        if !is_official {
                            return Err(AppError::localized(
                                "provider.codex.auth.missing",
                                format!("供应商 {} 缺少 auth 配置", provider.id),
                                format!("Provider {} is missing auth configuration", provider.id),
                            ));
                        }
                    }
                }
            }
            AppType::Gemini => {
                // 新增
                use crate::gemini_config::validate_gemini_settings;
                validate_gemini_settings(&provider.settings_config)?
            }
        }

        // 🔧 验证并清理 UsageScript 配置（所有应用类型通用）
        if let Some(meta) = &provider.meta {
            if let Some(usage_script) = &meta.usage_script {
                Self::validate_usage_script(usage_script)?;
            }
        }

        Ok(())
    }

    fn app_not_found(app_type: &AppType) -> AppError {
        AppError::localized(
            "provider.app_not_found",
            format!("应用类型不存在: {app_type:?}"),
            format!("App type not found: {app_type:?}"),
        )
    }

    pub fn delete(state: &AppState, app_type: AppType, provider_id: &str) -> Result<(), AppError> {
        let provider_snapshot = {
            let config = state.config.read().map_err(AppError::from)?;
            let manager = config
                .get_manager(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;

            if manager.current == provider_id {
                return Err(AppError::localized(
                    "provider.delete.current",
                    "不能删除当前正在使用的供应商",
                    "Cannot delete the provider currently in use",
                ));
            }

            manager.providers.get(provider_id).cloned().ok_or_else(|| {
                AppError::localized(
                    "provider.not_found",
                    format!("供应商不存在: {provider_id}"),
                    format!("Provider not found: {provider_id}"),
                )
            })?
        };

        match app_type {
            AppType::Codex => {
                crate::codex_config::delete_codex_provider_config(
                    provider_id,
                    &provider_snapshot.name,
                )?;
            }
            AppType::Claude => {
                // 兼容旧版本：历史上会在 Claude 目录内为每个供应商生成 settings-*.json 副本
                // 这里继续清理这些遗留文件，避免堆积过期配置。
                let by_name = get_provider_config_path(provider_id, Some(&provider_snapshot.name));
                let by_id = get_provider_config_path(provider_id, None);
                delete_file(&by_name)?;
                delete_file(&by_id)?;
            }
            AppType::Gemini => {
                // Gemini 使用单一的 .env 文件，不需要删除单独的供应商配置文件
            }
        }

        {
            let mut config = state.config.write().map_err(AppError::from)?;
            let manager = config
                .get_manager_mut(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;

            if manager.current == provider_id {
                return Err(AppError::localized(
                    "provider.delete.current",
                    "不能删除当前正在使用的供应商",
                    "Cannot delete the provider currently in use",
                ));
            }

            manager.providers.shift_remove(provider_id);
        }

        state.save()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderSortUpdate {
    pub id: String,
    #[serde(rename = "sortIndex")]
    pub sort_index: usize,
}

#[cfg(test)]
mod codex_openai_auth_tests {
    use super::*;
    use serial_test::serial;
    use std::ffi::OsString;
    use std::path::Path;
    use tempfile::TempDir;

    struct EnvGuard {
        old_home: Option<OsString>,
        old_userprofile: Option<OsString>,
    }

    impl EnvGuard {
        fn set_home(home: &Path) -> Self {
            let old_home = std::env::var_os("HOME");
            let old_userprofile = std::env::var_os("USERPROFILE");
            std::env::set_var("HOME", home);
            std::env::set_var("USERPROFILE", home);
            Self {
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
        }
    }

    #[test]
    #[serial]
    fn switch_codex_provider_uses_openai_auth_instead_of_env_key() {
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
            manager.providers.insert(
                "p1".to_string(),
                Provider::with_id(
                    "p1".to_string(),
                    "OpenAI".to_string(),
                    json!({
                        "auth": { "OPENAI_API_KEY": "sk-test" },
                        "config": "base_url = \"https://api.openai.com/v1\"\nmodel = \"gpt-4o\"\nenv_key = \"OPENAI_API_KEY\"\nwire_api = \"chat\""
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
            "config.toml should enable OpenAI auth for Codex model provider"
        );
        assert!(
            !config_text.contains("env_key = \"OPENAI_API_KEY\""),
            "config.toml should not force OPENAI_API_KEY env var by default"
        );
    }
}
