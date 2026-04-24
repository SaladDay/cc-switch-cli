use serde_json::json;
use std::collections::HashMap;

use cc_switch_lib::{
    get_claude_settings_path, read_json_file, update_settings, write_codex_live_atomic, AppError,
    AppSettings, AppType, McpApps, McpServer, MultiAppConfig, Provider, ProviderMeta,
    ProviderService,
};

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs, state_from_config};

fn sanitize_provider_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            _ => c,
        })
        .collect::<String>()
        .to_lowercase()
}

fn read_openclaw_live_config_json5(path: &std::path::Path) -> serde_json::Value {
    let source = std::fs::read_to_string(path).expect("read openclaw live config source");
    json5::from_str(&source).expect("parse openclaw live config as json5")
}

fn config_with_prompt_entries(entries: &[(&AppType, &str, &str, bool)]) -> MultiAppConfig {
    let mut value =
        serde_json::to_value(MultiAppConfig::default()).expect("serialize default config");

    for (app, id, content, enabled) in entries {
        value["prompts"][app.as_str()]["prompts"][*id] = json!({
            "id": *id,
            "name": format!("Prompt {id}"),
            "content": *content,
            "enabled": *enabled,
            "createdAt": 1,
            "updatedAt": 2
        });
    }

    serde_json::from_value(value).expect("deserialize prompt config")
}

fn codex_provider(
    id: &str,
    name: &str,
    api_key: &str,
    model_provider: &str,
    base_url: &str,
) -> Provider {
    Provider::with_id(
        id.to_string(),
        name.to_string(),
        json!({
            "auth": { "OPENAI_API_KEY": api_key },
            "config": format!(
                "model_provider = \"{model_provider}\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.{model_provider}]\nbase_url = \"{base_url}\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
            )
        }),
        None,
    )
}

fn insert_codex_managed_mcp(config: &mut MultiAppConfig) {
    config.mcp.servers = Some(HashMap::new());
    config.mcp.servers.as_mut().unwrap().insert(
        "echo-server".into(),
        McpServer {
            id: "echo-server".to_string(),
            name: "Echo Server".to_string(),
            server: json!({
                "type": "stdio",
                "command": "echo"
            }),
            apps: McpApps {
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
}

#[test]
fn provider_service_switch_codex_updates_live_and_config() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let legacy_auth = json!({ "OPENAI_API_KEY": "legacy-key" });
    let legacy_config = r#"[mcp_servers.legacy]
type = "stdio"
command = "echo"
"#;
    write_codex_live_atomic(&legacy_auth, Some(legacy_config))
        .expect("seed existing codex live config");

    let mut initial_config = MultiAppConfig::default();
    {
        let manager = initial_config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "old-provider".to_string();
        manager.providers.insert(
            "old-provider".to_string(),
            Provider::with_id(
                "old-provider".to_string(),
                "Legacy".to_string(),
                json!({
                    "auth": {"OPENAI_API_KEY": "stale"},
                    "config": "stale-config"
                }),
                None,
            ),
        );
        manager.providers.insert(
            "new-provider".to_string(),
            Provider::with_id(
                "new-provider".to_string(),
                "Latest".to_string(),
                json!({
                    "auth": {"OPENAI_API_KEY": "fresh-key"},
                    "config": "model_provider = \"latest\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.latest]\nbase_url = \"https://api.example.com/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
                }),
                None,
            ),
        );
    }

    // v3.7.0: unified MCP structure
    initial_config.mcp.servers = Some(HashMap::new());
    initial_config.mcp.servers.as_mut().unwrap().insert(
        "echo-server".into(),
        McpServer {
            id: "echo-server".to_string(),
            name: "Echo Server".to_string(),
            server: json!({
                "type": "stdio",
                "command": "echo"
            }),
            apps: McpApps {
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

    let state = state_from_config(initial_config);

    ProviderService::switch(&state, AppType::Codex, "new-provider")
        .expect("switch provider should succeed");

    let auth_value: serde_json::Value =
        read_json_file(&cc_switch_lib::get_codex_auth_path()).expect("read auth.json");
    assert_eq!(
        auth_value.get("OPENAI_API_KEY").and_then(|v| v.as_str()),
        Some("fresh-key"),
        "live auth.json should reflect new provider"
    );

    let config_text =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    assert!(
        config_text.contains("mcp_servers.echo-server"),
        "config.toml should contain synced MCP servers"
    );

    let guard = state.config.read().expect("read config after switch");
    let manager = guard
        .get_manager(&AppType::Codex)
        .expect("codex manager after switch");
    assert_eq!(manager.current, "new-provider", "current provider updated");

    let new_provider = manager
        .providers
        .get("new-provider")
        .expect("new provider exists");
    let new_config_text = new_provider
        .settings_config
        .get("config")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        new_config_text.contains("model = "),
        "provider config snapshot should contain model snippet"
    );
    assert!(
        !new_config_text.contains("mcp_servers.echo-server"),
        "provider config snapshot should not store synced MCP servers"
    );

    let legacy = manager
        .providers
        .get("old-provider")
        .expect("legacy provider still exists");
    let legacy_auth_value = legacy
        .settings_config
        .get("auth")
        .and_then(|v| v.get("OPENAI_API_KEY"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        legacy_auth_value, "legacy-key",
        "previous provider should be backfilled with live auth"
    );
}

#[test]
fn update_current_codex_provider_preserves_managed_mcp_servers() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let common_snippet = "disable_response_storage = true";
    let live_auth = json!({ "OPENAI_API_KEY": "live-key" });
    let live_config = r#"disable_response_storage = true
model_provider = "current"
model = "gpt-5.2-codex"

[model_providers.current]
base_url = "https://api.before.example/v1"
wire_api = "responses"
requires_openai_auth = true

[mcp_servers.echo-server]
type = "stdio"
command = "echo"
"#;
    write_codex_live_atomic(&live_auth, Some(live_config))
        .expect("seed existing codex live config");

    let mut config = MultiAppConfig::default();
    config.common_config_snippets.codex = Some(common_snippet.to_string());
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "current-provider".to_string();
        manager.providers.insert(
            "current-provider".to_string(),
            codex_provider(
                "current-provider",
                "Current",
                "stored-key",
                "current",
                "https://api.before.example/v1",
            ),
        );
    }
    insert_codex_managed_mcp(&mut config);

    let state = state_from_config(config);

    ProviderService::update(
        &state,
        AppType::Codex,
        codex_provider(
            "current-provider",
            "Current",
            "updated-key",
            "current",
            "https://api.after.example/v1",
        ),
    )
    .expect("update current provider should succeed");

    let config_text =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    assert!(
        config_text.contains("disable_response_storage = true"),
        "common snippet should still be present after update"
    );
    assert!(
        config_text.contains("[mcp_servers.echo-server]"),
        "managed MCP table should remain after updating the current provider"
    );
    assert!(
        config_text.contains("command = \"echo\""),
        "managed MCP payload should remain after updating the current provider"
    );
}

#[test]
fn add_current_codex_provider_preserves_managed_mcp_servers() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let common_snippet = "disable_response_storage = true";
    let live_auth = json!({ "OPENAI_API_KEY": "live-key" });
    let live_config = r#"disable_response_storage = true
model_provider = "legacy"
model = "gpt-5.2-codex"

[model_providers.legacy]
base_url = "https://api.legacy.example/v1"
wire_api = "responses"
requires_openai_auth = true

[mcp_servers.echo-server]
type = "stdio"
command = "echo"
"#;
    write_codex_live_atomic(&live_auth, Some(live_config))
        .expect("seed existing codex live config");

    let mut config = MultiAppConfig::default();
    config.common_config_snippets.codex = Some(common_snippet.to_string());
    insert_codex_managed_mcp(&mut config);

    let state = state_from_config(config);

    ProviderService::add(
        &state,
        AppType::Codex,
        codex_provider(
            "new-provider",
            "New",
            "fresh-key",
            "new",
            "https://api.new.example/v1",
        ),
    )
    .expect("add current provider should succeed");

    let config_text =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    assert!(
        !config_text.contains("disable_response_storage = true"),
        "new missing-meta providers should not apply common config implicitly after migration"
    );
    assert!(
        config_text.contains("[mcp_servers.echo-server]"),
        "managed MCP table should remain after adding the current provider"
    );
    assert!(
        config_text.contains("command = \"echo\""),
        "managed MCP payload should remain after adding the current provider"
    );
}

#[test]
fn update_current_codex_provider_uses_db_current_even_if_config_current_drifted() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let common_snippet = "disable_response_storage = true";
    let live_auth = json!({ "OPENAI_API_KEY": "live-key" });
    let live_config = r#"disable_response_storage = true
model_provider = "stale"
model = "gpt-5.2-codex"

[model_providers.stale]
base_url = "https://api.stale.example/v1"
wire_api = "responses"
requires_openai_auth = true

[mcp_servers.echo-server]
type = "stdio"
command = "echo"
"#;
    write_codex_live_atomic(&live_auth, Some(live_config))
        .expect("seed existing codex live config");

    let mut config = MultiAppConfig::default();
    config.common_config_snippets.codex = Some(common_snippet.to_string());
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "missing-provider".to_string();
        manager.providers.insert(
            "current-provider".to_string(),
            codex_provider(
                "current-provider",
                "Current",
                "stored-key",
                "current",
                "https://api.before.example/v1",
            ),
        );
    }
    insert_codex_managed_mcp(&mut config);

    let state = state_from_config(config);
    state
        .db
        .save_provider(
            AppType::Codex.as_str(),
            &codex_provider(
                "current-provider",
                "Current",
                "stored-key",
                "current",
                "https://api.before.example/v1",
            ),
        )
        .expect("save current provider to db");
    state
        .db
        .set_current_provider(AppType::Codex.as_str(), "current-provider")
        .expect("set db current provider");

    ProviderService::update(
        &state,
        AppType::Codex,
        codex_provider(
            "current-provider",
            "Current",
            "updated-key",
            "current",
            "https://api.after.example/v1",
        ),
    )
    .expect("update should succeed when db current points at the edited provider");

    let config_text =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    assert!(
        config_text.contains("base_url = \"https://api.after.example/v1\""),
        "live config should be rewritten from the healed current provider"
    );
    assert!(
        config_text.contains("[mcp_servers.echo-server]"),
        "managed MCP table should remain after rewriting the healed current provider"
    );

    let guard = state.config.read().expect("read config after update");
    let manager = guard
        .get_manager(&AppType::Codex)
        .expect("codex manager after update");
    assert_eq!(
        manager.current, "current-provider",
        "update should resync the config snapshot to the stored db current provider"
    );
}

#[test]
fn add_first_codex_provider_sets_db_current_and_rewrites_live_when_db_current_missing() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let common_snippet = "disable_response_storage = true";
    let live_auth = json!({ "OPENAI_API_KEY": "live-key" });
    let live_config = r#"disable_response_storage = true
model_provider = "stale"
model = "gpt-5.2-codex"

[model_providers.stale]
base_url = "https://api.stale.example/v1"
wire_api = "responses"
requires_openai_auth = true

[mcp_servers.echo-server]
type = "stdio"
command = "echo"
"#;
    write_codex_live_atomic(&live_auth, Some(live_config))
        .expect("seed existing codex live config");

    let mut config = MultiAppConfig::default();
    config.common_config_snippets.codex = Some(common_snippet.to_string());
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "missing-provider".to_string();
    }
    insert_codex_managed_mcp(&mut config);

    let state = state_from_config(config);

    ProviderService::add(
        &state,
        AppType::Codex,
        codex_provider(
            "new-provider",
            "New",
            "fresh-key",
            "new",
            "https://api.new.example/v1",
        ),
    )
    .expect("add should succeed when db current is missing");

    assert_eq!(
        state
            .db
            .get_current_provider(AppType::Codex.as_str())
            .expect("read db current provider after add")
            .as_deref(),
        Some("new-provider"),
        "first add should promote the new provider in the database when no db current exists"
    );

    let config_text =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    assert!(
        config_text.contains("base_url = \"https://api.new.example/v1\""),
        "live config should be rewritten for the first provider after self-healing dangling current"
    );
    assert!(
        config_text.contains("[mcp_servers.echo-server]"),
        "managed MCP table should remain after adding the healed current provider"
    );

    let guard = state.config.read().expect("read config after add");
    let manager = guard
        .get_manager(&AppType::Codex)
        .expect("codex manager after add");
    assert_eq!(
        manager.current, "new-provider",
        "first add should resync the config snapshot to the promoted db current provider"
    );
}

#[test]
fn update_non_current_codex_provider_does_not_self_heal_or_rewrite_live_from_db_current() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let common_snippet = "disable_response_storage = true";
    let live_auth = json!({ "OPENAI_API_KEY": "current" });
    let live_config = r#"disable_response_storage = true
model_provider = "current"
model = "gpt-5.2-codex"

[model_providers.current]
base_url = "https://api.current.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#;
    write_codex_live_atomic(&live_auth, Some(live_config))
        .expect("seed existing codex live config");

    let mut config = MultiAppConfig::default();
    config.common_config_snippets.codex = Some(common_snippet.to_string());
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "missing-provider".to_string();
        manager.providers.insert(
            "current-provider".to_string(),
            codex_provider(
                "current-provider",
                "Current",
                "current",
                "current",
                "https://api.current.example/v1",
            ),
        );
        manager.providers.insert(
            "other-provider".to_string(),
            codex_provider(
                "other-provider",
                "Other",
                "other",
                "other",
                "https://api.other-before.example/v1",
            ),
        );
    }
    insert_codex_managed_mcp(&mut config);

    let state = state_from_config(config);
    state
        .db
        .save_provider(
            AppType::Codex.as_str(),
            &codex_provider(
                "current-provider",
                "Current",
                "current",
                "current",
                "https://api.current.example/v1",
            ),
        )
        .expect("save current provider to db");
    state
        .db
        .save_provider(
            AppType::Codex.as_str(),
            &codex_provider(
                "other-provider",
                "Other",
                "other",
                "other",
                "https://api.other-before.example/v1",
            ),
        )
        .expect("save other provider to db");
    state
        .db
        .set_current_provider(AppType::Codex.as_str(), "current-provider")
        .expect("set db current provider");

    ProviderService::update(
        &state,
        AppType::Codex,
        codex_provider(
            "other-provider",
            "Other",
            "other-updated",
            "other",
            "https://api.other-after.example/v1",
        ),
    )
    .expect("update non-current provider should succeed without self-healing stale config.current");

    let config_text =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    assert!(
        config_text.contains("base_url = \"https://api.current.example/v1\""),
        "live config should remain pointed at the actual current provider from db"
    );
    assert!(
        !config_text.contains("https://api.other-after.example/v1"),
        "updating a non-current provider should not rewrite live config from the edited provider"
    );
    assert!(
        !config_text.contains("[mcp_servers.echo-server]"),
        "updating a non-current provider should not trigger a live rewrite that injects managed MCP"
    );

    let guard = state.config.read().expect("read config after update");
    let manager = guard
        .get_manager(&AppType::Codex)
        .expect("codex manager after update");
    assert_eq!(
        manager.current, "current-provider",
        "update may resync the local config snapshot to the stored db current provider, but should not rewrite live config"
    );
}

#[test]
fn add_non_current_codex_provider_does_not_self_heal_or_rewrite_live_from_db_current() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let common_snippet = "disable_response_storage = true";
    let live_auth = json!({ "OPENAI_API_KEY": "current" });
    let live_config = r#"disable_response_storage = true
model_provider = "current"
model = "gpt-5.2-codex"

[model_providers.current]
base_url = "https://api.current.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#;
    write_codex_live_atomic(&live_auth, Some(live_config))
        .expect("seed existing codex live config");

    let mut config = MultiAppConfig::default();
    config.common_config_snippets.codex = Some(common_snippet.to_string());
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "missing-provider".to_string();
        manager.providers.insert(
            "current-provider".to_string(),
            codex_provider(
                "current-provider",
                "Current",
                "current",
                "current",
                "https://api.current.example/v1",
            ),
        );
    }
    insert_codex_managed_mcp(&mut config);

    let state = state_from_config(config);
    state
        .db
        .save_provider(
            AppType::Codex.as_str(),
            &codex_provider(
                "current-provider",
                "Current",
                "current",
                "current",
                "https://api.current.example/v1",
            ),
        )
        .expect("save current provider to db");
    state
        .db
        .set_current_provider(AppType::Codex.as_str(), "current-provider")
        .expect("set db current provider");

    ProviderService::add(
        &state,
        AppType::Codex,
        codex_provider(
            "new-provider",
            "New",
            "fresh-key",
            "new",
            "https://api.new.example/v1",
        ),
    )
    .expect("add non-current provider should succeed without self-healing stale config.current");

    let config_text =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    assert!(
        config_text.contains("base_url = \"https://api.current.example/v1\""),
        "live config should remain pointed at the actual current provider from db"
    );
    assert!(
        !config_text.contains("https://api.new.example/v1"),
        "adding a non-current provider should not rewrite live config from the new provider"
    );
    assert!(
        !config_text.contains("[mcp_servers.echo-server]"),
        "adding a non-current provider should not trigger a live rewrite that injects managed MCP"
    );

    let guard = state.config.read().expect("read config after add");
    let manager = guard
        .get_manager(&AppType::Codex)
        .expect("codex manager after add");
    assert_eq!(
        manager.current, "current-provider",
        "add may resync the local config snapshot to the stored db current provider, but should not rewrite live config"
    );
}

#[test]
fn switch_gemini_when_uninitialized_skips_live_sync_and_succeeds() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    assert!(
        !home.join(".gemini").exists(),
        "precondition: ~/.gemini should not exist"
    );

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Gemini)
            .expect("gemini manager");
        manager.current = "old-provider".to_string();
        manager.providers.insert(
            "old-provider".to_string(),
            Provider::with_id(
                "old-provider".to_string(),
                "Old Gemini".to_string(),
                json!({
                    "env": {
                        "GEMINI_API_KEY": "old-key",
                        "GOOGLE_GEMINI_BASE_URL": "https://example.com"
                    },
                    "config": {}
                }),
                Some("https://ai.google.dev".to_string()),
            ),
        );
        manager.providers.insert(
            "new-provider".to_string(),
            Provider::with_id(
                "new-provider".to_string(),
                "New Gemini".to_string(),
                json!({
                    "env": {
                        "GEMINI_API_KEY": "new-key",
                        "GOOGLE_GEMINI_BASE_URL": "https://example.com"
                    },
                    "config": {}
                }),
                Some("https://ai.google.dev".to_string()),
            ),
        );
    }

    let state = state_from_config(config);

    ProviderService::switch(&state, AppType::Gemini, "new-provider")
        .expect("switch should succeed even when Gemini is uninitialized");

    assert!(
        !home.join(".gemini").exists(),
        "should_sync=auto: switching provider should not create ~/.gemini when uninitialized"
    );

    let guard = state.config.read().expect("read config after switch");
    let manager = guard
        .get_manager(&AppType::Gemini)
        .expect("gemini manager after switch");
    assert_eq!(manager.current, "new-provider", "current provider updated");
}

#[test]
fn switch_packycode_gemini_updates_security_selected_type() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Gemini)
            .expect("gemini manager");
        manager.current = "packy-gemini".to_string();
        manager.providers.insert(
            "packy-gemini".to_string(),
            Provider::with_id(
                "packy-gemini".to_string(),
                "PackyCode".to_string(),
                json!({
                    "env": {
                        "GEMINI_API_KEY": "pk-key",
                        "GOOGLE_GEMINI_BASE_URL": "https://www.packyapi.com"
                    }
                }),
                Some("https://www.packyapi.com".to_string()),
            ),
        );
    }

    let state = state_from_config(config);

    ProviderService::switch(&state, AppType::Gemini, "packy-gemini")
        .expect("switching to PackyCode Gemini should succeed");

    let settings_path = home.join(".cc-switch").join("settings.json");
    assert!(
        settings_path.exists(),
        "settings.json should exist at {}",
        settings_path.display()
    );
    let raw = std::fs::read_to_string(&settings_path).expect("read settings.json");
    let value: serde_json::Value =
        serde_json::from_str(&raw).expect("parse settings.json after switch");

    assert_eq!(
        value
            .pointer("/security/auth/selectedType")
            .and_then(|v| v.as_str()),
        Some("gemini-api-key"),
        "PackyCode Gemini should set security.auth.selectedType"
    );
}

#[test]
fn packycode_partner_meta_triggers_security_flag_even_without_keywords() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Gemini)
            .expect("gemini manager");
        manager.current = "packy-meta".to_string();
        let mut provider = Provider::with_id(
            "packy-meta".to_string(),
            "Generic Gemini".to_string(),
            json!({
                "env": {
                    "GEMINI_API_KEY": "pk-meta",
                    "GOOGLE_GEMINI_BASE_URL": "https://generativelanguage.googleapis.com"
                }
            }),
            Some("https://example.com".to_string()),
        );
        provider.meta = Some(ProviderMeta {
            partner_promotion_key: Some("packycode".to_string()),
            ..ProviderMeta::default()
        });
        manager.providers.insert("packy-meta".to_string(), provider);
    }

    let state = state_from_config(config);

    ProviderService::switch(&state, AppType::Gemini, "packy-meta")
        .expect("switching to partner meta provider should succeed");

    let settings_path = home.join(".cc-switch").join("settings.json");
    assert!(
        settings_path.exists(),
        "settings.json should exist at {}",
        settings_path.display()
    );
    let raw = std::fs::read_to_string(&settings_path).expect("read settings.json");
    let value: serde_json::Value =
        serde_json::from_str(&raw).expect("parse settings.json after switch");

    assert_eq!(
        value
            .pointer("/security/auth/selectedType")
            .and_then(|v| v.as_str()),
        Some("gemini-api-key"),
        "Partner meta should set security.auth.selectedType even without packy keywords"
    );
}

#[test]
fn switch_google_official_gemini_sets_oauth_security() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    std::fs::create_dir_all(home.join(".gemini")).expect("create gemini dir (initialized)");

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Gemini)
            .expect("gemini manager");
        manager.current = "google-official".to_string();
        let mut provider = Provider::with_id(
            "google-official".to_string(),
            "Google".to_string(),
            json!({
                "env": {}
            }),
            Some("https://ai.google.dev".to_string()),
        );
        provider.meta = Some(ProviderMeta {
            partner_promotion_key: Some("google-official".to_string()),
            ..ProviderMeta::default()
        });
        manager
            .providers
            .insert("google-official".to_string(), provider);
    }

    let state = state_from_config(config);

    ProviderService::switch(&state, AppType::Gemini, "google-official")
        .expect("switching to Google official Gemini should succeed");

    let settings_path = home.join(".cc-switch").join("settings.json");
    assert!(
        settings_path.exists(),
        "settings.json should exist at {}",
        settings_path.display()
    );

    let raw = std::fs::read_to_string(&settings_path).expect("read settings.json");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("parse settings.json");
    assert_eq!(
        value
            .pointer("/security/auth/selectedType")
            .and_then(|v| v.as_str()),
        Some("oauth-personal"),
        "Google official Gemini should set oauth-personal selectedType in app settings"
    );

    let gemini_settings = home.join(".gemini").join("settings.json");
    assert!(
        gemini_settings.exists(),
        "Gemini settings.json should exist at {}",
        gemini_settings.display()
    );
    let gemini_raw = std::fs::read_to_string(&gemini_settings).expect("read gemini settings");
    let gemini_value: serde_json::Value =
        serde_json::from_str(&gemini_raw).expect("parse gemini settings");

    assert_eq!(
        gemini_value
            .pointer("/security/auth/selectedType")
            .and_then(|v| v.as_str()),
        Some("oauth-personal"),
        "Gemini settings json should also reflect oauth-personal"
    );
}

#[test]
fn switch_gemini_merges_existing_settings_preserving_mcp_servers() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let gemini_dir = home.join(".gemini");
    std::fs::create_dir_all(&gemini_dir).expect("create gemini dir");
    let gemini_settings_path = gemini_dir.join("settings.json");
    let existing_settings = json!({
        "mcpServers": {
            "keep": { "command": "echo" }
        }
    });
    std::fs::write(
        &gemini_settings_path,
        serde_json::to_string_pretty(&existing_settings).expect("serialize existing settings"),
    )
    .expect("seed existing gemini settings.json");

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Gemini)
            .expect("gemini manager");
        manager.current = "old".to_string();
        manager.providers.insert(
            "old".to_string(),
            Provider::with_id(
                "old".to_string(),
                "Old Gemini".to_string(),
                json!({
                    "env": {
                        "GEMINI_API_KEY": "old-key"
                    }
                }),
                None,
            ),
        );
        manager.providers.insert(
            "new".to_string(),
            Provider::with_id(
                "new".to_string(),
                "New Gemini".to_string(),
                json!({
                    "env": {
                        "GEMINI_API_KEY": "new-key"
                    },
                    "config": {
                        "ccSwitchTestKey": "new",
                        "security": {
                            "auth": {
                                "selectedType": "gemini-api-key"
                            }
                        }
                    }
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);

    ProviderService::switch(&state, AppType::Gemini, "new")
        .expect("switching to new gemini provider should succeed");

    let raw = std::fs::read_to_string(&gemini_settings_path).expect("read gemini settings.json");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("parse gemini settings.json");

    assert_eq!(
        value
            .pointer("/mcpServers/keep/command")
            .and_then(|v| v.as_str()),
        Some("echo"),
        "switch should preserve existing mcpServers entries in Gemini settings.json, got: {raw}"
    );
    assert_eq!(
        value.pointer("/ccSwitchTestKey").and_then(|v| v.as_str()),
        Some("new"),
        "switch should merge provider config into existing Gemini settings.json, got: {raw}"
    );
}

#[test]
fn provider_service_switch_claude_updates_live_and_state() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let settings_path = get_claude_settings_path();
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent).expect("create claude settings dir");
    }
    let legacy_live = json!({
        "env": {
            "ANTHROPIC_API_KEY": "legacy-key"
        },
        "workspace": {
            "path": "/tmp/workspace"
        }
    });
    std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&legacy_live).expect("serialize legacy live"),
    )
    .expect("seed claude live config");

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "old-provider".to_string();
        manager.providers.insert(
            "old-provider".to_string(),
            Provider::with_id(
                "old-provider".to_string(),
                "Legacy Claude".to_string(),
                json!({
                    "env": { "ANTHROPIC_API_KEY": "stale-key" }
                }),
                None,
            ),
        );
        manager.providers.insert(
            "new-provider".to_string(),
            Provider::with_id(
                "new-provider".to_string(),
                "Fresh Claude".to_string(),
                json!({
                    "env": { "ANTHROPIC_API_KEY": "fresh-key" },
                    "workspace": { "path": "/tmp/new-workspace" }
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);

    ProviderService::switch(&state, AppType::Claude, "new-provider")
        .expect("switch provider should succeed");

    let live_after: serde_json::Value =
        read_json_file(&settings_path).expect("read claude live settings");
    assert_eq!(
        live_after
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_API_KEY"))
            .and_then(|key| key.as_str()),
        Some("fresh-key"),
        "live settings.json should reflect new provider auth"
    );

    let guard = state
        .config
        .read()
        .expect("read claude config after switch");
    let manager = guard
        .get_manager(&AppType::Claude)
        .expect("claude manager after switch");
    assert_eq!(manager.current, "new-provider", "current provider updated");

    let legacy_provider = manager
        .providers
        .get("old-provider")
        .expect("legacy provider still exists");
    assert_eq!(
        legacy_provider.settings_config, legacy_live,
        "previous provider should receive backfilled live config"
    );
}

#[test]
fn provider_service_switch_missing_provider_returns_error() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    let state = state_from_config(MultiAppConfig::default());

    let err = ProviderService::switch(&state, AppType::Claude, "missing")
        .expect_err("switching missing provider should fail");
    match err {
        AppError::Localized { key, .. } => assert_eq!(key, "provider.not_found"),
        other => panic!("expected Localized error for provider not found, got {other:?}"),
    }
}

#[test]
fn provider_service_switch_openclaw_syncs_only_target_entry() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenClaw)
            .expect("openclaw manager");
        manager.providers.insert(
            "keep".to_string(),
            Provider::with_id(
                "keep".to_string(),
                "Keep".to_string(),
                json!({
                    "apiKey": "sk-keep",
                    "baseUrl": "https://keep.example/v1",
                    "models": [{ "id": "keep-model" }]
                }),
                None,
            ),
        );
        manager.providers.insert(
            "target".to_string(),
            Provider::with_id(
                "target".to_string(),
                "Target".to_string(),
                json!({
                    "apiKey": "sk-target",
                    "baseUrl": "https://target.example/v1",
                    "models": [{ "id": "target-model" }]
                }),
                None,
            ),
        );
    }

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    std::fs::write(
        &openclaw_path,
        serde_json::to_string_pretty(&json!({
            "models": {
                "mode": "merge",
                "providers": {
                    "keep": {
                        "apiKey": "sk-keep",
                        "baseUrl": "https://keep.example/v1",
                        "models": [{ "id": "keep-model" }]
                    }
                }
            },
            "agents": {
                "defaults": {
                    "model": {
                        "primary": "keep/keep-model"
                    }
                }
            }
        }))
        .expect("serialize openclaw live config"),
    )
    .expect("seed openclaw live config");

    let state = state_from_config(config);

    ProviderService::switch(&state, AppType::OpenClaw, "target")
        .expect("switch openclaw provider should succeed");

    let live_after: serde_json::Value =
        read_json_file(&openclaw_path).expect("read openclaw live config after switch");
    let providers = live_after["models"]["providers"]
        .as_object()
        .expect("openclaw config should contain providers map");
    assert!(providers.contains_key("keep"));
    assert!(providers.contains_key("target"));
    assert_eq!(
        providers["target"]["baseUrl"], "https://target.example/v1",
        "switch should sync the selected provider into live config"
    );

    let guard = state
        .config
        .read()
        .expect("read openclaw config after switch");
    let manager = guard
        .get_manager(&AppType::OpenClaw)
        .expect("openclaw manager after switch");
    assert!(
        manager.current.is_empty(),
        "additive-mode switch should not set current provider"
    );
}

#[test]
fn provider_service_list_openclaw_does_not_sync_live_into_local_manager() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    std::fs::write(
        &openclaw_path,
        serde_json::to_string_pretty(&json!({
            "models": {
                "mode": "merge",
                "providers": {
                    "live-only": {
                        "apiKey": "sk-live-only",
                        "baseUrl": "https://live.only.example/v1",
                        "models": [{ "id": "live-only-model", "name": "Live Only Model" }]
                    }
                }
            }
        }))
        .expect("serialize openclaw live config"),
    )
    .expect("seed openclaw live config");

    let state = state_from_config(MultiAppConfig::default());

    let providers = ProviderService::list(&state, AppType::OpenClaw)
        .expect("listing openclaw providers should succeed");
    assert!(
        providers.is_empty(),
        "read-only list should reflect the current local snapshot without importing live providers"
    );

    let guard = state.config.read().expect("read config after list");
    let manager = guard
        .get_manager(&AppType::OpenClaw)
        .expect("openclaw manager after list");
    assert!(
        manager.providers.is_empty(),
        "read-only list should not mutate the OpenClaw manager or rely on state.save()"
    );
}

#[test]
fn provider_service_sync_current_to_live_openclaw_skips_saved_only_snapshot_providers_missing_from_live(
) {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenClaw)
            .expect("openclaw manager");
        manager.providers.insert(
            "keep".to_string(),
            Provider::with_id(
                "keep".to_string(),
                "Keep Updated".to_string(),
                json!({
                    "apiKey": "sk-keep-new",
                    "baseUrl": "https://keep.new.example/v1",
                    "models": [{ "id": "keep-model-old" }]
                }),
                None,
            ),
        );
        manager.providers.insert(
            "saved-only".to_string(),
            Provider::with_id(
                "saved-only".to_string(),
                "Saved Only".to_string(),
                json!({
                    "apiKey": "sk-saved-only",
                    "baseUrl": "https://saved.only.example/v1",
                    "models": [{ "id": "saved-only-model" }]
                }),
                None,
            ),
        );
    }

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    std::fs::write(
        &openclaw_path,
        serde_json::to_string_pretty(&json!({
            "models": {
                "mode": "merge",
                "providers": {
                    "keep": {
                        "apiKey": "sk-keep-old",
                        "baseUrl": "https://keep.old.example/v1",
                        "models": [{ "id": "keep-model-old" }]
                    }
                }
            },
            "agents": {
                "defaults": {
                    "model": {
                        "primary": "keep/keep-model-old"
                    }
                }
            }
        }))
        .expect("serialize openclaw live config"),
    )
    .expect("seed openclaw live config");

    let state = state_from_config(config);

    ProviderService::sync_current_to_live(&state)
        .expect("sync_current_to_live should tolerate additive-mode snapshots");

    let live_after = read_openclaw_live_config_json5(&openclaw_path);
    let providers = live_after["models"]["providers"]
        .as_object()
        .expect("openclaw live config should contain providers map");
    assert_eq!(
        providers
            .get("keep")
            .and_then(|provider| provider.get("baseUrl"))
            .and_then(|value| value.as_str()),
        Some("https://keep.new.example/v1"),
        "sync_current_to_live should still refresh providers that already exist in live openclaw.json"
    );
    assert!(
        !providers.contains_key("saved-only"),
        "sync_current_to_live should not repopulate saved-only OpenClaw providers into live openclaw.json"
    );
    assert_eq!(
        live_after["agents"]["defaults"]["model"]["primary"],
        json!("keep/keep-model-old"),
        "sync_current_to_live should leave unrelated OpenClaw sections untouched"
    );
}

#[test]
fn provider_service_sync_current_to_live_openclaw_skips_when_live_file_is_missing() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenClaw)
            .expect("openclaw manager");
        manager.providers.insert(
            "saved-only".to_string(),
            Provider::with_id(
                "saved-only".to_string(),
                "Saved Only".to_string(),
                json!({
                    "apiKey": "sk-saved-only",
                    "baseUrl": "https://saved.only.example/v1",
                    "models": [{ "id": "saved-only-model" }]
                }),
                None,
            ),
        );
    }

    let openclaw_path = home.join(".openclaw").join("openclaw.json");
    assert!(
        !openclaw_path.exists(),
        "precondition: openclaw.json should be absent before sync_current_to_live"
    );

    let state = state_from_config(config);

    ProviderService::sync_current_to_live(&state)
        .expect("sync_current_to_live should skip OpenClaw when live file is missing");

    assert!(
        !openclaw_path.exists(),
        "sync_current_to_live should not recreate a missing openclaw.json file"
    );
}

#[test]
fn provider_service_sync_current_to_live_openclaw_ignores_malformed_live_membership() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenClaw)
            .expect("openclaw manager");
        manager.providers.insert(
            "keep".to_string(),
            Provider::with_id(
                "keep".to_string(),
                "Keep Updated".to_string(),
                json!({
                    "apiKey": "sk-keep-new",
                    "baseUrl": "https://keep.new.example/v1",
                    "models": [{ "id": "keep-model-old" }]
                }),
                None,
            ),
        );
        manager.providers.insert(
            "model-less".to_string(),
            Provider::with_id(
                "model-less".to_string(),
                "Model Less".to_string(),
                json!({
                    "apiKey": "sk-model-less-new",
                    "baseUrl": "https://model.less.new.example/v1",
                    "models": [{ "id": "model-less-model" }]
                }),
                None,
            ),
        );
    }

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    std::fs::write(
        &openclaw_path,
        serde_json::to_string_pretty(&json!({
            "models": {
                "mode": "merge",
                "providers": {
                    "keep": {
                        "apiKey": "sk-keep-old",
                        "baseUrl": "https://keep.old.example/v1",
                        "models": [{ "id": "keep-model-old" }]
                    },
                    "model-less": {
                        "apiKey": "sk-model-less-old",
                        "baseUrl": "https://model.less.old.example/v1",
                        "models": [{ "name": "Missing Id" }]
                    }
                }
            }
        }))
        .expect("serialize openclaw live config"),
    )
    .expect("seed openclaw live config");

    let state = state_from_config(config);

    ProviderService::sync_current_to_live(&state)
        .expect("sync_current_to_live should tolerate invalid live OpenClaw members");

    let live_after = read_openclaw_live_config_json5(&openclaw_path);
    let providers = live_after["models"]["providers"]
        .as_object()
        .expect("openclaw live config should contain providers map");
    assert_eq!(
        providers
            .get("keep")
            .and_then(|provider| provider.get("baseUrl"))
            .and_then(|value| value.as_str()),
        Some("https://keep.new.example/v1"),
        "sync_current_to_live should still refresh valid providers already present in live openclaw.json"
    );
    assert_eq!(
        providers
            .get("model-less")
            .and_then(|provider| provider.get("baseUrl"))
            .and_then(|value| value.as_str()),
        Some("https://model.less.old.example/v1"),
        "malformed live providers should not count as valid membership for sync_current_to_live"
    );
    assert_eq!(
        providers["model-less"]["models"][0]["name"],
        json!("Missing Id"),
        "sync_current_to_live should leave invalid live OpenClaw entries untouched instead of resurrecting them"
    );
}

#[test]
fn provider_service_sync_current_to_live_openclaw_ignores_blank_model_ids_in_live_membership() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenClaw)
            .expect("openclaw manager");
        manager.providers.insert(
            "keep".to_string(),
            Provider::with_id(
                "keep".to_string(),
                "Keep Updated".to_string(),
                json!({
                    "apiKey": "sk-keep-new",
                    "baseUrl": "https://keep.new.example/v1",
                    "models": [{ "id": "keep-model" }]
                }),
                None,
            ),
        );
        manager.providers.insert(
            "blank-model-id".to_string(),
            Provider::with_id(
                "blank-model-id".to_string(),
                "Blank Model Id Updated".to_string(),
                json!({
                    "apiKey": "sk-blank-new",
                    "baseUrl": "https://blank.new.example/v1",
                    "models": [{ "id": "real-model" }]
                }),
                None,
            ),
        );
    }

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    std::fs::write(
        &openclaw_path,
        serde_json::to_string_pretty(&json!({
            "models": {
                "mode": "merge",
                "providers": {
                    "keep": {
                        "apiKey": "sk-keep-old",
                        "baseUrl": "https://keep.old.example/v1",
                        "models": [{ "id": "keep-model" }]
                    },
                    "blank-model-id": {
                        "apiKey": "sk-blank-old",
                        "baseUrl": "https://blank.old.example/v1",
                        "models": [{ "id": "   ", "name": "Blank Id" }]
                    }
                }
            }
        }))
        .expect("serialize openclaw live config"),
    )
    .expect("seed openclaw live config");

    let state = state_from_config(config);

    ProviderService::sync_current_to_live(&state)
        .expect("sync_current_to_live should tolerate blank-id OpenClaw members");

    let live_after = read_openclaw_live_config_json5(&openclaw_path);
    let providers = live_after["models"]["providers"]
        .as_object()
        .expect("openclaw live config should contain providers map");
    assert_eq!(
        providers
            .get("keep")
            .and_then(|provider| provider.get("baseUrl"))
            .and_then(|value| value.as_str()),
        Some("https://keep.new.example/v1"),
        "sync_current_to_live should still refresh valid providers already present in live openclaw.json"
    );
    assert_eq!(
        providers
            .get("blank-model-id")
            .and_then(|provider| provider.get("baseUrl"))
            .and_then(|value| value.as_str()),
        Some("https://blank.old.example/v1"),
        "blank OpenClaw model ids should not count as valid membership for sync_current_to_live"
    );
    assert_eq!(
        providers["blank-model-id"]["models"][0]["id"],
        json!("   "),
        "sync_current_to_live should leave blank-id live OpenClaw entries untouched instead of resurrecting them"
    );
}

#[test]
fn provider_service_update_saved_only_openclaw_writes_live_config_additively() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenClaw)
            .expect("openclaw manager");
        manager.providers.insert(
            "saved-only".to_string(),
            Provider::with_id(
                "saved-only".to_string(),
                "Saved Only".to_string(),
                json!({
                    "apiKey": "sk-saved-old",
                    "baseUrl": "https://saved.old.example/v1",
                    "models": [{ "id": "saved-model" }]
                }),
                None,
            ),
        );
    }

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    let original_text = r#"// keep-live-comment
{
  models: {
    mode: 'merge',
    providers: {
      keep: {
        apiKey: 'sk-keep',
        baseUrl: 'https://keep.example/v1',
        models: [{ id: 'keep-model' }],
      },
    },
  },
  agents: {
    defaults: {
      model: {
        primary: 'keep/keep-model',
      },
    },
  },
}
"#;
    std::fs::write(&openclaw_path, original_text).expect("seed openclaw live config");

    let state = state_from_config(config);

    cc_switch_lib::ProviderService::update(
        &state,
        AppType::OpenClaw,
        Provider::with_id(
            "saved-only".to_string(),
            "Saved Only Updated".to_string(),
            json!({
                "apiKey": "sk-saved-new",
                "baseUrl": "https://saved.new.example/v1",
                "models": [{ "id": "saved-model-updated" }]
            }),
            None,
        ),
    )
    .expect("updating saved-only openclaw provider should write back into live config");

    let guard = state
        .config
        .read()
        .expect("read config after saved-only update");
    let provider = guard
        .get_manager(&AppType::OpenClaw)
        .and_then(|manager| manager.providers.get("saved-only"))
        .expect("saved-only provider should remain mirrored in local state");
    assert_eq!(provider.name, "Saved Only Updated");
    assert!(
        provider.created_at.unwrap_or_default() >= 1_000_000_000_000,
        "OpenClaw user-touched markers should use millisecond timestamps"
    );
    assert_eq!(
        provider.settings_config["baseUrl"],
        json!("https://saved.new.example/v1")
    );
    assert_eq!(
        provider.settings_config["models"][0]["id"],
        json!("saved-model-updated")
    );

    let live_after_text =
        std::fs::read_to_string(&openclaw_path).expect("read openclaw live config");
    let live_after = read_openclaw_live_config_json5(&openclaw_path);
    assert!(
        live_after_text.contains("// keep-live-comment"),
        "additive OpenClaw writes should preserve unrelated round-trippable source when possible"
    );
    assert_eq!(
        live_after["models"]["providers"]["saved-only"]["baseUrl"],
        json!("https://saved.new.example/v1")
    );
    assert_eq!(
        live_after["models"]["providers"]["saved-only"]["models"][0]["id"],
        json!("saved-model-updated")
    );
    assert_eq!(
        live_after["agents"]["defaults"]["model"]["primary"],
        json!("keep/keep-model"),
        "provider updates should still leave unrelated agents.defaults untouched"
    );
}

#[test]
fn provider_service_update_saved_only_openclaw_rejects_broken_live_file() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenClaw)
            .expect("openclaw manager");
        manager.providers.insert(
            "saved-only".to_string(),
            Provider::with_id(
                "saved-only".to_string(),
                "Saved Only".to_string(),
                json!({
                    "apiKey": "sk-saved-old",
                    "baseUrl": "https://saved.old.example/v1",
                    "models": [{ "id": "saved-model" }]
                }),
                None,
            ),
        );
    }

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    let broken_live_text = "{ broken: [ }\n";
    std::fs::write(&openclaw_path, broken_live_text).expect("seed broken openclaw live config");

    let state = state_from_config(config);

    let err = cc_switch_lib::ProviderService::update(
        &state,
        AppType::OpenClaw,
        Provider::with_id(
            "saved-only".to_string(),
            "Saved Only Updated".to_string(),
            json!({
                "apiKey": "sk-saved-new",
                "baseUrl": "https://saved.new.example/v1",
                "models": [{ "id": "saved-model-updated" }]
            }),
            None,
        ),
    )
    .expect_err("saved-only update should fail when openclaw.json cannot be rewritten");

    assert!(
        err.to_string().contains("round-trip JSON5"),
        "expected rewrite failure to mention round-trip JSON5 parsing, got: {err}"
    );

    let guard = state
        .config
        .read()
        .expect("read config after saved-only update with broken live file");
    let provider = guard
        .get_manager(&AppType::OpenClaw)
        .and_then(|manager| manager.providers.get("saved-only"))
        .expect("saved-only provider should remain in snapshot state");
    assert_eq!(provider.name, "Saved Only");
    assert_eq!(
        provider.settings_config["baseUrl"],
        json!("https://saved.old.example/v1")
    );

    let live_after_text =
        std::fs::read_to_string(&openclaw_path).expect("read broken live config after update");
    assert_eq!(
        live_after_text, broken_live_text,
        "failed additive update should not touch broken openclaw.json text"
    );
}

#[test]
fn provider_service_update_openclaw_allows_model_catalog_refs_to_dangle() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenClaw)
            .expect("openclaw manager");
        manager.providers.insert(
            "keep".to_string(),
            Provider::with_id(
                "keep".to_string(),
                "Keep".to_string(),
                json!({
                    "apiKey": "sk-keep",
                    "baseUrl": "https://keep.example/v1",
                    "models": [
                        { "id": "primary-model" },
                        { "id": "fallback-model" }
                    ]
                }),
                None,
            ),
        );
    }

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    let original_text = r#"{
  models: {
    mode: 'merge',
    providers: {
      keep: {
        apiKey: 'sk-keep',
        baseUrl: 'https://keep.example/v1',
        models: [
          { id: 'primary-model' },
          { id: 'fallback-model' },
        ],
      },
    },
  },
  agents: {
    defaults: {
      models: {
        'keep/fallback-model': {
          alias: 'Fallback',
        },
      },
    },
  },
}
"#;
    std::fs::write(&openclaw_path, original_text).expect("seed openclaw live config");

    let state = state_from_config(config);

    cc_switch_lib::ProviderService::update(
        &state,
        AppType::OpenClaw,
        Provider::with_id(
            "keep".to_string(),
            "Keep".to_string(),
            json!({
                "apiKey": "sk-keep",
                "baseUrl": "https://keep.example/v2",
                "models": [{ "id": "primary-model" }]
            }),
            None,
        ),
    )
    .expect("OpenClaw update should allow agents.defaults.models refs to become dangling");

    let guard = state.config.read().expect("read config after update");
    let provider = guard
        .get_manager(&AppType::OpenClaw)
        .and_then(|manager| manager.providers.get("keep"))
        .expect("provider should remain in saved state after update");
    assert_eq!(
        provider.settings_config["baseUrl"],
        json!("https://keep.example/v2")
    );
    assert_eq!(
        provider.settings_config["models"],
        json!([{ "id": "primary-model" }])
    );

    let live_after = read_openclaw_live_config_json5(&openclaw_path);
    assert_eq!(
        live_after["models"]["providers"]["keep"]["baseUrl"],
        json!("https://keep.example/v2")
    );
    assert_eq!(
        live_after["agents"]["defaults"]["models"]["keep/fallback-model"]["alias"],
        json!("Fallback")
    );
}

#[test]
fn provider_service_add_openclaw_rejects_noncanonical_settings_shape() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let state = state_from_config(MultiAppConfig::default());

    let err = ProviderService::add(
        &state,
        AppType::OpenClaw,
        Provider::with_id(
            "invalid-shape".to_string(),
            "Invalid Shape".to_string(),
            json!({
                "apiKey": "sk-invalid",
                "baseUrl": "https://invalid.example/v1",
                "models": {
                    "id": "not-an-array"
                }
            }),
            None,
        ),
    )
    .expect_err("OpenClaw add should reject noncanonical provider schema");

    match err {
        AppError::Localized { key, .. } => {
            assert_eq!(key, "provider.openclaw.settings.invalid");
        }
        other => panic!("expected localized invalid-openclaw-settings error, got {other:?}"),
    }
}

#[test]
fn provider_service_add_openclaw_rejects_model_less_provider() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let state = state_from_config(MultiAppConfig::default());

    let err = ProviderService::add(
        &state,
        AppType::OpenClaw,
        Provider::with_id(
            "model-less".to_string(),
            "Model Less".to_string(),
            json!({
                "apiKey": "sk-model-less",
                "baseUrl": "https://model.less.example/v1",
                "models": []
            }),
            None,
        ),
    )
    .expect_err("OpenClaw add should reject providers without models");

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
        !manager.providers.contains_key("model-less"),
        "rejected OpenClaw add should not persist a model-less provider"
    );
}

#[test]
fn provider_service_add_openclaw_rejects_legacy_alias_settings_shape() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let state = state_from_config(MultiAppConfig::default());

    let err = ProviderService::add(
        &state,
        AppType::OpenClaw,
        Provider::with_id(
            "legacy-shape".to_string(),
            "Legacy Shape".to_string(),
            json!({
                "api_key": "sk-legacy",
                "base_url": "https://legacy.example/v1",
                "npm": "@ai-sdk/openai-compatible",
                "options": {
                    "apiKey": "sk-options",
                    "baseURL": "https://options.example/v1"
                },
                "models": [
                    {
                        "id": "legacy-model",
                        "context_window": 128000
                    }
                ]
            }),
            None,
        ),
    )
    .expect_err("OpenClaw add should reject legacy alias settings");

    match err {
        AppError::Localized { key, zh, en } => {
            assert_eq!(key, "provider.openclaw.settings.invalid");
            let message = format!("{zh} {en}");
            assert!(message.contains("api_key"));
            assert!(message.contains("base_url"));
            assert!(message.contains("npm"));
            assert!(message.contains("options"));
            assert!(message.contains("context_window"));
        }
        other => panic!("expected localized invalid-openclaw-settings error, got {other:?}"),
    }
}

#[test]
fn provider_service_update_in_config_openclaw_updates_live_config() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenClaw)
            .expect("openclaw manager");
        manager.providers.insert(
            "keep".to_string(),
            Provider::with_id(
                "keep".to_string(),
                "Keep".to_string(),
                json!({
                    "apiKey": "sk-keep-old",
                    "baseUrl": "https://keep.old.example/v1",
                    "models": [{ "id": "keep-model" }]
                }),
                None,
            ),
        );
    }

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    std::fs::write(
        &openclaw_path,
        r#"{
            models: {
                mode: 'merge',
                providers: {
                    keep: {
                        apiKey: 'sk-keep-old',
                        baseUrl: 'https://keep.old.example/v1',
                        models: [{ id: 'keep-model' }],
                    },
                },
            },
        }"#,
    )
    .expect("seed openclaw live config");

    let state = state_from_config(config);

    cc_switch_lib::ProviderService::update(
        &state,
        AppType::OpenClaw,
        Provider::with_id(
            "keep".to_string(),
            "Keep Updated".to_string(),
            json!({
                "apiKey": "sk-keep-new",
                "baseUrl": "https://keep.new.example/v1",
                "models": [{ "id": "keep-model-updated" }]
            }),
            None,
        ),
    )
    .expect("updating in-config openclaw provider should still sync live config");

    let live_after = read_openclaw_live_config_json5(&openclaw_path);
    assert_eq!(
        live_after["models"]["providers"]["keep"]["baseUrl"],
        json!("https://keep.new.example/v1")
    );
    assert_eq!(
        live_after["models"]["providers"]["keep"]["models"][0]["id"],
        json!("keep-model-updated")
    );
}

#[test]
fn provider_service_update_openclaw_rejects_model_less_provider() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenClaw)
            .expect("openclaw manager");
        manager.providers.insert(
            "keep".to_string(),
            Provider::with_id(
                "keep".to_string(),
                "Keep".to_string(),
                json!({
                    "apiKey": "sk-keep",
                    "baseUrl": "https://keep.example/v1",
                    "models": [{ "id": "keep-model" }]
                }),
                None,
            ),
        );
    }

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    let original_text = r#"{
  models: {
    mode: 'merge',
    providers: {
      keep: {
        apiKey: 'sk-keep',
        baseUrl: 'https://keep.example/v1',
        models: [{ id: 'keep-model' }],
      },
    },
  },
}
"#;
    std::fs::write(&openclaw_path, original_text).expect("seed openclaw live config");

    let state = state_from_config(config);

    let err = ProviderService::update(
        &state,
        AppType::OpenClaw,
        Provider::with_id(
            "keep".to_string(),
            "Keep".to_string(),
            json!({
                "apiKey": "sk-keep",
                "baseUrl": "https://keep.example/v2",
                "models": []
            }),
            None,
        ),
    )
    .expect_err("OpenClaw update should reject providers without models");

    match err {
        AppError::Localized { key, .. } => {
            assert_eq!(key, "provider.openclaw.models.missing");
        }
        other => panic!("expected localized missing-openclaw-models error, got {other:?}"),
    }

    let guard = state
        .config
        .read()
        .expect("read config after rejected update");
    let provider = guard
        .get_manager(&AppType::OpenClaw)
        .and_then(|manager| manager.providers.get("keep"))
        .expect("provider should remain in saved state after rejected update");
    assert_eq!(
        provider.settings_config["baseUrl"],
        json!("https://keep.example/v1")
    );
    assert_eq!(
        provider.settings_config["models"][0]["id"],
        json!("keep-model")
    );

    let live_after_text =
        std::fs::read_to_string(&openclaw_path).expect("read openclaw live config after reject");
    assert_eq!(
        live_after_text, original_text,
        "rejecting a model-less OpenClaw update should leave openclaw.json text untouched"
    );
}

#[test]
fn provider_service_update_openclaw_rejects_noncanonical_settings_shape_without_touching_live() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenClaw)
            .expect("openclaw manager");
        manager.providers.insert(
            "keep".to_string(),
            Provider::with_id(
                "keep".to_string(),
                "Keep".to_string(),
                json!({
                    "apiKey": "sk-keep-old",
                    "baseUrl": "https://keep.old.example/v1",
                    "models": [{ "id": "keep-model" }]
                }),
                None,
            ),
        );
    }

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    let original_text = r#"{
  models: {
    mode: 'merge',
    providers: {
      keep: {
        apiKey: 'sk-keep-old',
        baseUrl: 'https://keep.old.example/v1',
        models: [{ id: 'keep-model' }],
      },
    },
  },
}
"#;
    std::fs::write(&openclaw_path, original_text).expect("seed openclaw live config");

    let state = state_from_config(config);

    let err = cc_switch_lib::ProviderService::update(
        &state,
        AppType::OpenClaw,
        Provider::with_id(
            "keep".to_string(),
            "Keep Updated".to_string(),
            json!({
                "apiKey": "sk-keep-new",
                "baseUrl": "https://keep.new.example/v1",
                "models": {
                    "id": "not-an-array"
                }
            }),
            None,
        ),
    )
    .expect_err("OpenClaw update should reject noncanonical provider schema");

    match err {
        AppError::Localized { key, .. } => {
            assert_eq!(key, "provider.openclaw.settings.invalid");
        }
        other => panic!("expected localized invalid-openclaw-settings error, got {other:?}"),
    }

    let guard = state
        .config
        .read()
        .expect("read config after rejected update");
    let provider = guard
        .get_manager(&AppType::OpenClaw)
        .and_then(|manager| manager.providers.get("keep"))
        .expect("provider should remain in saved state after rejected update");
    assert_eq!(provider.name, "Keep");
    assert_eq!(
        provider.settings_config["baseUrl"],
        json!("https://keep.old.example/v1")
    );

    let live_after_text =
        std::fs::read_to_string(&openclaw_path).expect("read openclaw live config after reject");
    assert_eq!(
        live_after_text, original_text,
        "rejecting an invalid-schema update should leave openclaw.json text untouched"
    );
}

#[test]
fn provider_service_update_openclaw_rejects_legacy_alias_settings_shape_without_touching_live() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenClaw)
            .expect("openclaw manager");
        manager.providers.insert(
            "keep".to_string(),
            Provider::with_id(
                "keep".to_string(),
                "Keep".to_string(),
                json!({
                    "apiKey": "sk-keep-old",
                    "baseUrl": "https://keep.old.example/v1",
                    "models": [{ "id": "keep-model" }]
                }),
                None,
            ),
        );
    }

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    let original_text = r#"{
  models: {
    mode: 'merge',
    providers: {
      keep: {
        apiKey: 'sk-keep-old',
        baseUrl: 'https://keep.old.example/v1',
        models: [{ id: 'keep-model' }],
      },
    },
  },
}
"#;
    std::fs::write(&openclaw_path, original_text).expect("seed openclaw live config");

    let state = state_from_config(config);

    let err = cc_switch_lib::ProviderService::update(
        &state,
        AppType::OpenClaw,
        Provider::with_id(
            "keep".to_string(),
            "Keep Updated".to_string(),
            json!({
                "api_key": "sk-legacy-new",
                "base_url": "https://legacy.new.example/v1",
                "models": [
                    {
                        "id": "keep-model",
                        "context_window": 256000
                    }
                ]
            }),
            None,
        ),
    )
    .expect_err("OpenClaw update should reject legacy alias settings");

    match err {
        AppError::Localized { key, zh, en } => {
            assert_eq!(key, "provider.openclaw.settings.invalid");
            let message = format!("{zh} {en}");
            assert!(message.contains("api_key"));
            assert!(message.contains("base_url"));
            assert!(message.contains("context_window"));
        }
        other => panic!("expected localized invalid-openclaw-settings error, got {other:?}"),
    }

    let guard = state
        .config
        .read()
        .expect("read config after rejected update");
    let provider = guard
        .get_manager(&AppType::OpenClaw)
        .and_then(|manager| manager.providers.get("keep"))
        .expect("provider should remain in saved state after rejected update");
    assert_eq!(
        provider.settings_config["baseUrl"],
        json!("https://keep.old.example/v1")
    );

    let live_after_text =
        std::fs::read_to_string(&openclaw_path).expect("read openclaw live config after reject");
    assert_eq!(
        live_after_text, original_text,
        "rejecting a legacy-alias OpenClaw update should leave openclaw.json text untouched"
    );
}

#[test]
fn provider_service_update_openclaw_allows_default_model_refs_to_dangle() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenClaw)
            .expect("openclaw manager");
        manager.providers.insert(
            "keep".to_string(),
            Provider::with_id(
                "keep".to_string(),
                "Keep".to_string(),
                json!({
                    "apiKey": "sk-keep",
                    "baseUrl": "https://keep.example/v1",
                    "models": [
                        { "id": "primary-model" },
                        { "id": "fallback-model" }
                    ]
                }),
                None,
            ),
        );
    }

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    let original_text = r#"{
  models: {
    mode: 'merge',
    providers: {
      keep: {
        apiKey: 'sk-keep',
        baseUrl: 'https://keep.example/v1',
        models: [
          { id: 'primary-model' },
          { id: 'fallback-model' },
        ],
      },
    },
  },
  agents: {
    defaults: {
      model: {
        primary: 'keep/fallback-model',
        fallbacks: ['keep/primary-model'],
      },
    },
  },
}
"#;
    std::fs::write(&openclaw_path, original_text).expect("seed openclaw live config");

    let state = state_from_config(config);

    cc_switch_lib::ProviderService::update(
        &state,
        AppType::OpenClaw,
        Provider::with_id(
            "keep".to_string(),
            "Keep".to_string(),
            json!({
                "apiKey": "sk-keep",
                "baseUrl": "https://keep.example/v2",
                "models": [{ "id": "primary-model" }]
            }),
            None,
        ),
    )
    .expect("OpenClaw live update should allow default model refs to become dangling");

    let guard = state.config.read().expect("read config after update");
    let provider = guard
        .get_manager(&AppType::OpenClaw)
        .and_then(|manager| manager.providers.get("keep"))
        .expect("provider should remain in saved state after update");
    assert_eq!(
        provider.settings_config["baseUrl"],
        json!("https://keep.example/v2")
    );
    assert_eq!(
        provider.settings_config["models"][0]["id"],
        json!("primary-model")
    );
    assert_eq!(
        provider.settings_config["models"].as_array().map(Vec::len),
        Some(1)
    );

    let live_after = read_openclaw_live_config_json5(&openclaw_path);
    assert_eq!(
        live_after["models"]["providers"]["keep"]["baseUrl"],
        json!("https://keep.example/v2")
    );
    assert_eq!(
        live_after["models"]["providers"]["keep"]["models"],
        json!([{ "id": "primary-model" }])
    );
    assert_eq!(
        live_after["agents"]["defaults"]["model"]["primary"],
        json!("keep/fallback-model"),
        "provider updates should not rewrite unrelated agents.defaults state"
    );
}

#[test]
fn provider_service_import_default_openclaw_skips_additive_mode() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    std::fs::write(
        &openclaw_path,
        r#"{
            models: {
                mode: 'merge',
                providers: {
                    openai: {
                        apiKey: 'sk-openai',
                        baseUrl: 'https://api.example.com/v1',
                        models: [{ id: 'gpt-4.1', name: 'GPT-4.1' }],
                    },
                },
            },
        }"#,
    )
    .expect("seed openclaw json5 live config");

    let state = state_from_config(MultiAppConfig::default());

    ProviderService::import_default_config(&state, AppType::OpenClaw)
        .expect("generic import should skip additive mode apps");

    let guard = state
        .config
        .read()
        .expect("read openclaw config after generic import");
    let manager = guard
        .get_manager(&AppType::OpenClaw)
        .expect("openclaw manager after generic import");
    assert!(
        manager.providers.is_empty(),
        "generic import_default_config should stay aligned with upstream and skip OpenClaw"
    );
}

#[test]
fn provider_service_import_openclaw_providers_from_live_imports_valid_live_providers_from_json5_and_skips_invalid_rows(
) {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    let source = r#"{
            models: {
                mode: 'merge',
                providers: {
                    openai: {
                        name: 'Top Level Name Should Be Ignored',
                        apiKey: 'sk-openai',
                        baseUrl: 'https://api.example.com/v1',
                        models: [
                            {
                                id: 'gpt-4.1',
                                name: 'GPT-4.1',
                            },
                        ],
                    },
                    anthropic: {
                        name: 'Anthropic',
                        api_key: 'sk-anthropic',
                        base_url: 'https://anthropic.example/v1',
                    },
                    modeless: {
                        apiKey: 'sk-modeless',
                        baseUrl: 'https://modeless.example/v1',
                        models: [],
                    },
                    malformed: {
                        apiKey: 'sk-malformed',
                        baseUrl: 'https://malformed.example/v1',
                        models: [{ name: 'Missing Id' }],
                    },
                },
            },
        }"#;
    std::fs::write(&openclaw_path, source).expect("seed openclaw json5 live config");

    let state = state_from_config(MultiAppConfig::default());

    let imported = ProviderService::import_openclaw_providers_from_live(&state)
        .expect("import should skip invalid OpenClaw live providers and keep valid ones");

    let guard = state
        .config
        .read()
        .expect("read openclaw config after import");
    let manager = guard
        .get_manager(&AppType::OpenClaw)
        .expect("openclaw manager after import");
    assert_eq!(
        imported, 1,
        "only valid OpenClaw live providers should import"
    );
    assert_eq!(manager.providers.len(), 1);
    assert!(manager.providers.contains_key("openai"));
    assert!(
        !manager.providers.contains_key("anthropic"),
        "legacy-alias live entries should be skipped"
    );
    assert!(
        !manager.providers.contains_key("modeless"),
        "model-less live entries should be skipped"
    );
    assert!(
        !manager.providers.contains_key("malformed"),
        "malformed live entries should be skipped"
    );
    assert_eq!(
        manager
            .providers
            .get("openai")
            .expect("valid provider should be imported")
            .settings_config["baseUrl"],
        json!("https://api.example.com/v1")
    );
    assert!(
        manager.current.is_empty(),
        "additive-mode import should not set current"
    );

    let after = std::fs::read_to_string(&openclaw_path).expect("read openclaw file after import");
    assert_eq!(after, source);
}

#[test]
fn provider_service_import_openclaw_providers_from_live_imports_missing_live_providers_incrementally(
) {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    std::fs::write(
        &openclaw_path,
        r#"{
            models: {
                mode: 'merge',
                providers: {
                    openai: {
                        apiKey: 'sk-openai',
                        baseUrl: 'https://api.example.com/v1',
                        models: [{ id: 'gpt-4.1', name: 'GPT-4.1' }],
                    },
                    groq: {
                        apiKey: 'sk-groq',
                        baseUrl: 'https://groq.example/v1',
                        models: [{ id: 'llama-4', name: 'Llama 4' }],
                    },
                },
            },
        }"#,
    )
    .expect("seed openclaw json5 live config");

    let mut config = MultiAppConfig::default();
    config
        .get_manager_mut(&AppType::OpenClaw)
        .expect("openclaw manager")
        .providers
        .insert(
            "openai".to_string(),
            Provider::with_id(
                "openai".to_string(),
                "Already Imported".to_string(),
                json!({
                    "apiKey": "sk-existing",
                    "baseUrl": "https://existing.example/v1",
                    "models": [{ "id": "gpt-4.1", "name": "Existing GPT-4.1" }]
                }),
                None,
            ),
        );

    let state = state_from_config(config);

    let imported = ProviderService::import_openclaw_providers_from_live(&state)
        .expect("import openclaw live config should succeed");

    let guard = state
        .config
        .read()
        .expect("read openclaw config after incremental import");
    let manager = guard
        .get_manager(&AppType::OpenClaw)
        .expect("openclaw manager after incremental import");

    assert_eq!(
        imported, 2,
        "sync should refresh existing bare rows and import missing live providers"
    );
    assert_eq!(manager.providers.len(), 2);
    assert!(manager.providers.contains_key("openai"));
    assert!(manager.providers.contains_key("groq"));
    assert_eq!(
        manager
            .providers
            .get("openai")
            .expect("existing provider should be refreshed from live config")
            .name,
        "openai"
    );
    assert_eq!(
        manager
            .providers
            .get("openai")
            .expect("existing provider should be refreshed from live config")
            .settings_config["baseUrl"],
        json!("https://api.example.com/v1")
    );
    assert_eq!(
        manager
            .providers
            .get("groq")
            .expect("missing provider should be imported")
            .name,
        "groq"
    );
    assert_eq!(
        manager
            .providers
            .get("groq")
            .expect("missing provider should be imported")
            .settings_config["baseUrl"],
        json!("https://groq.example/v1")
    );
}

#[test]
fn provider_service_import_openclaw_providers_from_live_skips_legacy_alias_provider_shape() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    let source = r#"{
            models: {
                mode: 'merge',
                providers: {
                    openai: {
                        apiKey: 'sk-openai',
                        baseUrl: 'https://api.example.com/v1',
                        models: [{ id: 'gpt-4.1', name: 'GPT-4.1' }],
                    },
                    legacy: {
                        api_key: 'sk-legacy',
                        base_url: 'https://legacy.example/v1',
                        models: [{ id: 'legacy-model', context_window: 128000 }],
                    },
                },
            },
        }"#;
    std::fs::write(&openclaw_path, source).expect("seed openclaw json5 live config");

    let state = state_from_config(MultiAppConfig::default());

    let imported = ProviderService::import_openclaw_providers_from_live(&state)
        .expect("import should skip malformed OpenClaw live providers");

    let guard = state
        .config
        .read()
        .expect("read config after rejected import");
    let manager = guard
        .get_manager(&AppType::OpenClaw)
        .expect("openclaw manager after import");
    assert_eq!(imported, 1, "only valid live providers should be mirrored");
    assert!(manager.providers.contains_key("openai"));
    assert!(
        !manager.providers.contains_key("legacy"),
        "legacy-alias OpenClaw providers should be skipped instead of blocking the whole mirror"
    );

    let after = std::fs::read_to_string(&openclaw_path).expect("read openclaw file after import");
    assert_eq!(after, source);
}

#[test]
fn provider_service_import_openclaw_live_skips_blank_ids_and_existing_entries() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    std::fs::write(
        &openclaw_path,
        r#"{
            models: {
                mode: 'merge',
                providers: {
                    '': {
                        apiKey: 'sk-blank',
                        baseUrl: 'https://blank.example/v1',
                        models: [{ id: 'ignored', name: 'Ignored Blank Entry' }],
                    },
                    existing: {
                        apiKey: 'sk-existing-live',
                        baseUrl: 'https://existing-live.example/v1',
                        models: [{ id: 'existing-model', name: 'Existing Live Name' }],
                    },
                    newcomer: {
                        apiKey: 'sk-newcomer',
                        baseUrl: 'https://newcomer.example/v1',
                        models: [{ id: 'new-model', name: 'New Model' }],
                    },
                },
            },
        }"#,
    )
    .expect("seed openclaw json5 live config");

    let mut config = MultiAppConfig::default();
    config
        .get_manager_mut(&AppType::OpenClaw)
        .expect("openclaw manager")
        .providers
        .insert(
            "existing".to_string(),
            Provider::with_id(
                "existing".to_string(),
                "Already Imported".to_string(),
                json!({
                    "apiKey": "sk-existing-db",
                    "baseUrl": "https://existing-db.example/v1",
                    "models": [{ "id": "existing-model", "name": "Existing DB Name" }]
                }),
                None,
            ),
        );

    let state = state_from_config(config);

    let imported = ProviderService::import_openclaw_providers_from_live(&state)
        .expect("import openclaw live config should succeed");

    assert_eq!(
        imported, 2,
        "sync should skip blank ids, refresh existing bare rows, and import newcomers"
    );

    let guard = state
        .config
        .read()
        .expect("read openclaw config after blank-id import");
    let manager = guard
        .get_manager(&AppType::OpenClaw)
        .expect("openclaw manager after blank-id import");

    assert_eq!(manager.providers.len(), 2);
    assert!(!manager.providers.contains_key(""));
    assert!(manager.providers.contains_key("existing"));
    assert!(manager.providers.contains_key("newcomer"));
    assert!(
        manager.current.is_empty(),
        "additive-mode import should keep current provider empty"
    );
    assert_eq!(
        manager
            .providers
            .get("existing")
            .expect("existing provider should be refreshed from live config")
            .name,
        "existing"
    );
    assert_eq!(
        manager
            .providers
            .get("existing")
            .expect("existing provider should be refreshed from live config")
            .settings_config["baseUrl"],
        json!("https://existing-live.example/v1")
    );
    assert_eq!(
        manager
            .providers
            .get("newcomer")
            .expect("new provider should be imported")
            .name,
        "newcomer"
    );
    assert_eq!(
        manager
            .providers
            .get("newcomer")
            .expect("new provider should be imported")
            .settings_config["baseUrl"],
        json!("https://newcomer.example/v1")
    );
}

#[test]
fn provider_service_import_openclaw_providers_from_live_skips_modeless_provider_even_if_default_references_it(
) {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    std::fs::write(
        &openclaw_path,
        r#"{
            models: {
                mode: 'merge',
                providers: {
                    openai: {
                        apiKey: 'sk-openai',
                        baseUrl: 'https://api.example.com/v1',
                        models: [],
                    },
                },
            },
            agents: {
                defaults: {
                    model: {
                        primary: 'openai/gpt-4.1',
                    },
                },
            },
        }"#,
    )
    .expect("seed openclaw json5 live config");

    let state = state_from_config(MultiAppConfig::default());

    ProviderService::import_openclaw_providers_from_live(&state)
        .expect("import openclaw live config should succeed");

    let guard = state
        .config
        .read()
        .expect("read openclaw config after import");
    let manager = guard
        .get_manager(&AppType::OpenClaw)
        .expect("openclaw manager after import");

    assert!(
        !manager.providers.contains_key("openai"),
        "OpenClaw import should stay aligned with upstream and skip providers without models"
    );
}

#[test]
fn provider_service_import_openclaw_providers_from_live_uses_provider_id_when_primary_model_has_no_name(
) {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    std::fs::write(
        &openclaw_path,
        r#"{
            models: {
                mode: 'merge',
                providers: {
                    openai: {
                        name: 'Top Level Name Should Stay Ignored',
                        apiKey: 'sk-openai',
                        baseUrl: 'https://api.example.com/v1',
                        models: [{ id: 'gpt-4.1' }],
                    },
                },
            },
        }"#,
    )
    .expect("seed openclaw json5 live config");

    let state = state_from_config(MultiAppConfig::default());

    ProviderService::import_openclaw_providers_from_live(&state)
        .expect("import openclaw live config should succeed");

    let guard = state
        .config
        .read()
        .expect("read openclaw config after import");
    let manager = guard
        .get_manager(&AppType::OpenClaw)
        .expect("openclaw manager after import");
    let openai = manager
        .providers
        .get("openai")
        .expect("openai provider should be imported");

    assert_eq!(
        openai.name, "openai",
        "OpenClaw import should fall back to provider id when the first model has no name"
    );
}

#[test]
fn provider_service_import_openclaw_providers_from_live_ignores_later_model_name_when_first_model_has_no_name(
) {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    std::fs::write(
        &openclaw_path,
        r#"{
            models: {
                mode: 'merge',
                providers: {
                    openai: {
                        apiKey: 'sk-openai',
                        baseUrl: 'https://api.example.com/v1',
                        models: [
                            { id: 'gpt-4.1' },
                            {
                                id: 'gpt-4.1-mini',
                                name: 'Later Name Should Stay Ignored',
                            },
                        ],
                    },
                },
            },
        }"#,
    )
    .expect("seed openclaw json5 live config");

    let state = state_from_config(MultiAppConfig::default());

    ProviderService::import_openclaw_providers_from_live(&state)
        .expect("import openclaw live config should succeed");

    let guard = state
        .config
        .read()
        .expect("read openclaw config after import");
    let manager = guard
        .get_manager(&AppType::OpenClaw)
        .expect("openclaw manager after import");
    let openai = manager
        .providers
        .get("openai")
        .expect("openai provider should be imported");

    assert_eq!(
        openai.name, "openai",
        "OpenClaw import should only consider the first model name before falling back to provider id"
    );
}

#[test]
fn provider_service_import_openclaw_providers_from_live_preserves_saved_name_for_existing_provider()
{
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    std::fs::write(
        &openclaw_path,
        r#"{
            models: {
                mode: 'merge',
                providers: {
                    openai: {
                        apiKey: 'sk-openai-live',
                        baseUrl: 'https://live.example/v1',
                        models: [{ id: 'gpt-4.1', name: 'Live Model Name' }],
                    },
                },
            },
        }"#,
    )
    .expect("seed openclaw json5 live config");

    let mut config = MultiAppConfig::default();
    let mut saved_provider = Provider::with_id(
        "openai".to_string(),
        "Saved Provider Name".to_string(),
        json!({
            "apiKey": "sk-openai-saved",
            "baseUrl": "https://saved.example/v1",
            "models": [{ "id": "gpt-4.1", "name": "Saved Model Name" }]
        }),
        None,
    );
    saved_provider.notes = Some("customized row".to_string());
    config
        .get_manager_mut(&AppType::OpenClaw)
        .expect("openclaw manager")
        .providers
        .insert("openai".to_string(), saved_provider);

    let state = state_from_config(config);

    let imported = ProviderService::import_openclaw_providers_from_live(&state)
        .expect("import openclaw live config should succeed");
    assert_eq!(imported, 1, "sync should update the existing row in place");

    let guard = state
        .config
        .read()
        .expect("read openclaw config after import");
    let manager = guard
        .get_manager(&AppType::OpenClaw)
        .expect("openclaw manager after import");
    let openai = manager
        .providers
        .get("openai")
        .expect("openai provider should still exist");

    assert_eq!(
        openai.name, "Saved Provider Name",
        "OpenClaw live sync should preserve the saved provider name for existing rows"
    );
    assert_eq!(
        openai.settings_config["baseUrl"],
        json!("https://live.example/v1"),
        "existing OpenClaw rows should still refresh settings from live config"
    );
}

#[test]
fn provider_service_import_openclaw_providers_from_live_preserves_existing_row_when_live_entry_is_invalid(
) {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    std::fs::write(
        &openclaw_path,
        r#"{
            models: {
                mode: 'merge',
                providers: {
                    preserved: {
                        apiKey: 'sk-live-invalid',
                        baseUrl: 'https://live.invalid.example/v1',
                        models: [],
                    },
                },
            },
        }"#,
    )
    .expect("seed invalid openclaw live config");

    let mut config = MultiAppConfig::default();
    let mut saved_provider = Provider::with_id(
        "preserved".to_string(),
        "Saved Provider Name".to_string(),
        json!({
            "apiKey": "sk-saved",
            "baseUrl": "https://saved.example/v1",
            "models": [{ "id": "saved-model", "name": "Saved Model" }]
        }),
        None,
    );
    saved_provider.notes = Some("keep this metadata".to_string());
    config
        .get_manager_mut(&AppType::OpenClaw)
        .expect("openclaw manager")
        .providers
        .insert("preserved".to_string(), saved_provider);

    let state = state_from_config(config);

    let imported = ProviderService::import_openclaw_providers_from_live(&state)
        .expect("import should tolerate invalid live OpenClaw entries");
    assert_eq!(
        imported, 0,
        "invalid-but-present OpenClaw rows should not be pruned or re-imported"
    );

    let guard = state
        .config
        .read()
        .expect("read openclaw config after import");
    let manager = guard
        .get_manager(&AppType::OpenClaw)
        .expect("openclaw manager after import");
    let preserved = manager
        .providers
        .get("preserved")
        .expect("existing provider row should stay mirrored locally");

    assert_eq!(preserved.name, "Saved Provider Name");
    assert_eq!(preserved.notes.as_deref(), Some("keep this metadata"));
    assert_eq!(
        preserved.settings_config["baseUrl"],
        json!("https://saved.example/v1"),
        "invalid live membership should not overwrite the last good local snapshot"
    );
}

#[test]
fn provider_service_switch_codex_missing_auth_is_rejected() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();
    if let Some(parent) = cc_switch_lib::get_codex_config_path().parent() {
        std::fs::create_dir_all(parent).expect("create codex dir (initialized)");
    }

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.providers.insert(
            "invalid".to_string(),
            Provider::with_id(
                "invalid".to_string(),
                "Broken Codex".to_string(),
                json!({
                    "config": "model_provider = \"invalid\"\nmodel = \"gpt-4o\"\n\n[model_providers.invalid]\nbase_url = \"https://api.example.com/v1\"\nwire_api = \"chat\"\nrequires_openai_auth = false\nenv_key = \"OPENAI_API_KEY\"\n"
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);

    let err = ProviderService::switch(&state, AppType::Codex, "invalid")
        .expect_err("switching should fail when provider snapshot has no auth");

    assert!(
        err.to_string().contains("auth"),
        "expected auth-related error, got {err}"
    );
    assert!(
        !cc_switch_lib::get_codex_config_path().exists(),
        "config.toml should not be written on failed switch"
    );
}

#[test]
fn provider_service_switch_codex_openai_official_writes_auth_json_from_provider_snapshot() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    std::fs::create_dir_all(home.join(".codex")).expect("create codex dir (initialized)");

    let auth_path = cc_switch_lib::get_codex_auth_path();
    std::fs::write(&auth_path, r#"{"OPENAI_API_KEY":"stale-key"}"#).expect("seed auth.json");
    assert!(auth_path.exists(), "auth.json should exist before switch");

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "p2".to_string();

        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "OpenAI Official".to_string(),
                json!({
                    "auth": { "OPENAI_API_KEY": "sk-official" },
                    "config": "model_provider = \"p1\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.p1]\nbase_url = \"https://api.openai.com/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
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
                    "config": "model_provider = \"p2\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.p2]\nbase_url = \"https://api.other.example/v1\"\nwire_api = \"chat\"\nrequires_openai_auth = false\nenv_key = \"OPENAI_API_KEY\"\n"
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);

    ProviderService::switch(&state, AppType::Codex, "p1")
        .expect("switch to OpenAI official provider should succeed");

    let auth_value: serde_json::Value =
        read_json_file(&auth_path).expect("read auth.json after switch");
    assert_eq!(
        auth_value.get("OPENAI_API_KEY").and_then(|v| v.as_str()),
        Some("sk-official"),
        "auth.json should be overwritten from the provider snapshot"
    );
}

#[test]
fn provider_service_switch_codex_preserves_missing_wire_api_for_openai_official() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    std::fs::create_dir_all(home.join(".codex")).expect("create codex dir (initialized)");

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "OpenAI Official".to_string(),
                json!({
                    "auth": { "OPENAI_API_KEY": "sk-official" },
                    "config": "model_provider = \"p1\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.p1]\nbase_url = \"https://api.openai.com/v1\"\nrequires_openai_auth = true\n"
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);

    ProviderService::switch(&state, AppType::Codex, "p1")
        .expect("switch to OpenAI official provider should succeed");

    let live_text =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    assert!(
        !live_text.contains("wire_api = \"responses\""),
        "live config should preserve the stored snippet instead of defaulting wire_api"
    );
}

#[test]
fn provider_service_switch_codex_preserves_missing_requires_openai_auth_for_openai_official() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    std::fs::create_dir_all(home.join(".codex")).expect("create codex dir (initialized)");

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "p1".to_string();
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "OpenAI Official".to_string(),
                json!({
                    "auth": { "OPENAI_API_KEY": "sk-official" },
                    "config": "model_provider = \"p1\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.p1]\nbase_url = \"https://api.openai.com/v1\"\nwire_api = \"responses\"\n"
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);

    ProviderService::switch(&state, AppType::Codex, "p1")
        .expect("switch to OpenAI official provider should succeed");

    let live_text =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    assert!(
        !live_text.contains("requires_openai_auth = true"),
        "live config should preserve the stored snippet instead of defaulting requires_openai_auth"
    );
}

#[test]
fn provider_service_switch_codex_openai_official_preserves_oauth_auth_and_common_only_config() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    std::fs::create_dir_all(home.join(".codex")).expect("create codex dir (initialized)");

    let auth_path = cc_switch_lib::get_codex_auth_path();
    std::fs::write(&auth_path, r#"{"OPENAI_API_KEY":"stale-key"}"#).expect("seed auth.json");

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "p2".to_string();

        let mut official = Provider::with_id(
            "p1".to_string(),
            "OpenAI Official".to_string(),
            json!({
                "auth": {
                    "access_token": "oauth-token",
                    "refresh_token": "refresh-token"
                },
                "config": "model_reasoning_effort = \"high\"\ndisable_response_storage = true"
            }),
            Some("https://chatgpt.com/codex".to_string()),
        );
        official.meta = Some(ProviderMeta {
            codex_official: Some(true),
            ..Default::default()
        });
        manager.providers.insert("p1".to_string(), official);

        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Other".to_string(),
                json!({
                    "auth": { "OPENAI_API_KEY": "sk-other" },
                    "config": "model_provider = \"p2\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.p2]\nbase_url = \"https://api.other.example/v1\"\nwire_api = \"chat\"\nrequires_openai_auth = false\nenv_key = \"OPENAI_API_KEY\"\n"
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);

    ProviderService::switch(&state, AppType::Codex, "p1")
        .expect("switch to stripped OpenAI official provider should succeed");

    let auth_value: serde_json::Value =
        read_json_file(&auth_path).expect("read auth.json after switch");
    assert_eq!(
        auth_value["access_token"],
        json!("oauth-token"),
        "official provider should restore the stored OAuth auth snapshot"
    );

    let live_text =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    assert!(
        !live_text.contains("base_url = "),
        "official provider should not reintroduce a provider-specific base_url into live config"
    );
    assert!(
        live_text.contains("model_reasoning_effort = \"high\""),
        "official provider should still keep shared Codex settings"
    );
}

#[test]
fn provider_service_delete_codex_removes_provider_and_files() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "keep".to_string();
        manager.providers.insert(
            "keep".to_string(),
            Provider::with_id(
                "keep".to_string(),
                "Keep".to_string(),
                json!({
                    "auth": {"OPENAI_API_KEY": "keep-key"},
                    "config": ""
                }),
                None,
            ),
        );
        manager.providers.insert(
            "to-delete".to_string(),
            Provider::with_id(
                "to-delete".to_string(),
                "DeleteCodex".to_string(),
                json!({
                    "auth": {"OPENAI_API_KEY": "delete-key"},
                    "config": ""
                }),
                None,
            ),
        );
    }

    let sanitized = sanitize_provider_name("DeleteCodex");
    let codex_dir = home.join(".codex");
    std::fs::create_dir_all(&codex_dir).expect("create codex dir");
    let auth_path = codex_dir.join(format!("auth-{sanitized}.json"));
    let cfg_path = codex_dir.join(format!("config-{sanitized}.toml"));
    std::fs::write(&auth_path, "{}").expect("seed auth file");
    std::fs::write(&cfg_path, "base_url = \"https://example\"").expect("seed config file");

    let app_state = state_from_config(config);

    ProviderService::delete(&app_state, AppType::Codex, "to-delete")
        .expect("delete provider should succeed");

    let locked = app_state.config.read().expect("lock config after delete");
    let manager = locked.get_manager(&AppType::Codex).expect("codex manager");
    assert!(
        !manager.providers.contains_key("to-delete"),
        "provider entry should be removed"
    );
    assert!(
        !auth_path.exists() && !cfg_path.exists(),
        "provider-specific files should be deleted"
    );
}

#[test]
fn provider_service_delete_claude_removes_provider_files() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "keep".to_string();
        manager.providers.insert(
            "keep".to_string(),
            Provider::with_id(
                "keep".to_string(),
                "Keep".to_string(),
                json!({
                    "env": { "ANTHROPIC_API_KEY": "keep-key" }
                }),
                None,
            ),
        );
        manager.providers.insert(
            "delete".to_string(),
            Provider::with_id(
                "delete".to_string(),
                "DeleteClaude".to_string(),
                json!({
                    "env": { "ANTHROPIC_API_KEY": "delete-key" }
                }),
                None,
            ),
        );
    }

    let sanitized = sanitize_provider_name("DeleteClaude");
    let claude_dir = home.join(".claude");
    std::fs::create_dir_all(&claude_dir).expect("create claude dir");
    let by_name = claude_dir.join(format!("settings-{sanitized}.json"));
    let by_id = claude_dir.join("settings-delete.json");
    std::fs::write(&by_name, "{}").expect("seed settings by name");
    std::fs::write(&by_id, "{}").expect("seed settings by id");

    let app_state = state_from_config(config);

    ProviderService::delete(&app_state, AppType::Claude, "delete").expect("delete claude provider");

    let locked = app_state.config.read().expect("lock config after delete");
    let manager = locked
        .get_manager(&AppType::Claude)
        .expect("claude manager");
    assert!(
        !manager.providers.contains_key("delete"),
        "claude provider should be removed"
    );
    assert!(
        !by_name.exists() && !by_id.exists(),
        "provider config files should be deleted"
    );
}

#[test]
fn provider_service_delete_openclaw_removes_provider_from_live_and_state() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenClaw)
            .expect("openclaw manager");
        manager.providers.insert(
            "keep".to_string(),
            Provider::with_id(
                "keep".to_string(),
                "Keep".to_string(),
                json!({
                    "apiKey": "sk-keep",
                    "baseUrl": "https://keep.example/v1",
                    "models": [{ "id": "keep-model" }]
                }),
                None,
            ),
        );
        manager.providers.insert(
            "to-delete".to_string(),
            Provider::with_id(
                "to-delete".to_string(),
                "DeleteOpenClaw".to_string(),
                json!({
                    "apiKey": "sk-delete",
                    "baseUrl": "https://delete.example/v1",
                    "models": [{ "id": "delete-model" }]
                }),
                None,
            ),
        );
    }

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    std::fs::write(
        &openclaw_path,
        serde_json::to_string_pretty(&json!({
            "models": {
                "mode": "merge",
                "providers": {
                    "keep": {
                        "apiKey": "sk-keep",
                        "baseUrl": "https://keep.example/v1",
                        "models": [{ "id": "keep-model" }]
                    },
                    "to-delete": {
                        "apiKey": "sk-delete",
                        "baseUrl": "https://delete.example/v1",
                        "models": [{ "id": "delete-model" }]
                    }
                }
            }
        }))
        .expect("serialize openclaw live config"),
    )
    .expect("seed openclaw live config");

    let app_state = state_from_config(config);

    ProviderService::delete(&app_state, AppType::OpenClaw, "to-delete")
        .expect("delete openclaw provider should succeed");

    let locked = app_state.config.read().expect("lock config after delete");
    let manager = locked
        .get_manager(&AppType::OpenClaw)
        .expect("openclaw manager after delete");
    assert!(
        !manager.providers.contains_key("to-delete"),
        "openclaw provider should be removed from state"
    );
    assert!(manager.providers.contains_key("keep"));

    let live_after: serde_json::Value =
        read_json_file(&openclaw_path).expect("read openclaw live config after delete");
    assert_eq!(live_after["models"]["mode"], "merge");
    let providers = live_after["models"]["providers"]
        .as_object()
        .expect("openclaw config should contain providers map");
    assert!(providers.contains_key("keep"));
    assert!(
        !providers.contains_key("to-delete"),
        "deleted openclaw provider should be removed from live config"
    );
}

#[test]
fn provider_service_delete_openclaw_default_provider_allows_dangling_default_model() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenClaw)
            .expect("openclaw manager");
        manager.providers.insert(
            "keep".to_string(),
            Provider::with_id(
                "keep".to_string(),
                "Keep".to_string(),
                json!({
                    "apiKey": "sk-keep",
                    "baseUrl": "https://keep.example/v1",
                    "models": [
                        { "id": "primary-model" },
                        { "id": "fallback-model" }
                    ]
                }),
                None,
            ),
        );
    }

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    std::fs::write(
        &openclaw_path,
        serde_json::to_string_pretty(&json!({
            "models": {
                "mode": "merge",
                "providers": {
                    "keep": {
                        "apiKey": "sk-keep",
                        "baseUrl": "https://keep.example/v1",
                        "models": [
                            { "id": "primary-model" },
                            { "id": "fallback-model" }
                        ]
                    }
                }
            },
            "agents": {
                "defaults": {
                    "model": {
                        "primary": "keep/fallback-model",
                        "fallbacks": ["keep/primary-model"]
                    }
                }
            }
        }))
        .expect("serialize openclaw live config"),
    )
    .expect("seed openclaw live config");

    let app_state = state_from_config(config);

    ProviderService::delete(&app_state, AppType::OpenClaw, "keep")
        .expect("deleting a default-referenced OpenClaw provider should succeed");

    let locked = app_state.config.read().expect("lock config after delete");
    let manager = locked
        .get_manager(&AppType::OpenClaw)
        .expect("openclaw manager after delete");
    assert!(!manager.providers.contains_key("keep"));

    let live_after = read_openclaw_live_config_json5(&openclaw_path);
    assert_eq!(
        live_after["agents"]["defaults"]["model"]["primary"],
        "keep/fallback-model"
    );
    assert!(live_after["models"]["providers"].get("keep").is_none());
}

#[test]
fn provider_service_delete_openclaw_provider_referenced_only_by_fallback_allows_dangling_default_model(
) {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenClaw)
            .expect("openclaw manager");
        manager.providers.insert(
            "keep".to_string(),
            Provider::with_id(
                "keep".to_string(),
                "Keep".to_string(),
                json!({
                    "apiKey": "sk-keep",
                    "baseUrl": "https://keep.example/v1",
                    "models": [
                        { "id": "primary-model" },
                        { "id": "fallback-model" }
                    ]
                }),
                None,
            ),
        );
    }

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    std::fs::write(
        &openclaw_path,
        serde_json::to_string_pretty(&json!({
            "models": {
                "mode": "merge",
                "providers": {
                    "keep": {
                        "apiKey": "sk-keep",
                        "baseUrl": "https://keep.example/v1",
                        "models": [
                            { "id": "primary-model" },
                            { "id": "fallback-model" }
                        ]
                    }
                }
            },
            "agents": {
                "defaults": {
                    "model": {
                        "primary": "other-provider/other-model",
                        "fallbacks": ["keep/fallback-model"]
                    }
                }
            }
        }))
        .expect("serialize openclaw live config"),
    )
    .expect("seed openclaw live config");

    let app_state = state_from_config(config);

    ProviderService::delete(&app_state, AppType::OpenClaw, "keep")
        .expect("deleting a fallback-referenced OpenClaw provider should succeed");

    let locked = app_state.config.read().expect("lock config after delete");
    let manager = locked
        .get_manager(&AppType::OpenClaw)
        .expect("openclaw manager after delete");
    assert!(!manager.providers.contains_key("keep"));

    let live_after = read_openclaw_live_config_json5(&openclaw_path);
    assert_eq!(
        live_after["agents"]["defaults"]["model"]["fallbacks"],
        json!(["keep/fallback-model"])
    );
    assert!(live_after["models"]["providers"].get("keep").is_none());
}

#[test]
fn provider_service_switch_openclaw_ignores_unrelated_mcp_sync_failures() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenClaw)
            .expect("openclaw manager");
        manager.providers.insert(
            "keep".to_string(),
            Provider::with_id(
                "keep".to_string(),
                "Keep".to_string(),
                json!({
                    "apiKey": "sk-keep",
                    "baseUrl": "https://keep.example/v1",
                    "models": [{ "id": "keep-model" }]
                }),
                None,
            ),
        );
        manager.providers.insert(
            "target".to_string(),
            Provider::with_id(
                "target".to_string(),
                "Target".to_string(),
                json!({
                    "apiKey": "sk-target",
                    "baseUrl": "https://target.example/v1",
                    "models": [{ "id": "target-model" }]
                }),
                None,
            ),
        );
    }
    config.mcp.servers = Some(HashMap::new());
    config.mcp.servers.as_mut().unwrap().insert(
        "broken-opencode".into(),
        McpServer {
            id: "broken-opencode".to_string(),
            name: "Broken OpenCode".to_string(),
            server: json!({
                "type": "wat"
            }),
            apps: McpApps {
                claude: false,
                codex: false,
                gemini: false,
                opencode: true,
                hermes: false,
            },
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        },
    );

    let openclaw_dir = home.join(".openclaw");
    std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    std::fs::create_dir_all(home.join(".config").join("opencode"))
        .expect("create opencode dir so MCP sync runs");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    let original_text = r#"// keep-this-comment
{
  models: {
    mode: 'merge',
    providers: {
      keep: {
        apiKey: 'sk-keep',
        baseUrl: 'https://keep.example/v1',
        models: [{ id: 'keep-model' }],
      },
    },
  },
  agents: {
    defaults: {
      model: {
        primary: 'keep/keep-model',
      },
    },
  },
  tools: {
    profile: 'coding',
  },
}
"#;
    std::fs::write(&openclaw_path, original_text).expect("seed openclaw json5 live config");

    let state = state_from_config(config);

    ProviderService::switch(&state, AppType::OpenClaw, "target")
        .expect("OpenClaw switch should ignore unrelated MCP sync failures");

    let live_after = read_openclaw_live_config_json5(&openclaw_path);
    assert_eq!(
        live_after
            .pointer("/models/providers/target/baseUrl")
            .and_then(|value| value.as_str()),
        Some("https://target.example/v1"),
        "switch should still update OpenClaw live config"
    );

    let opencode_path = home.join(".config").join("opencode").join("opencode.json");
    assert!(
        !opencode_path.exists(),
        "OpenClaw switch should not trigger OpenCode MCP sync side effects"
    );
}

#[test]
fn provider_service_delete_current_provider_returns_error() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    let keep = Provider::with_id(
        "keep".to_string(),
        "Keep".to_string(),
        json!({
            "env": { "ANTHROPIC_API_KEY": "keep-key" }
        }),
        None,
    );

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "keep".to_string();
        manager.providers.insert("keep".to_string(), keep.clone());
    }

    let app_state = state_from_config(config);
    app_state
        .db
        .save_provider(AppType::Claude.as_str(), &keep)
        .expect("save keep provider to db");
    app_state
        .db
        .set_current_provider(AppType::Claude.as_str(), "keep")
        .expect("set db current provider");

    let err = ProviderService::delete(&app_state, AppType::Claude, "keep")
        .expect_err("deleting current provider should fail");
    match err {
        AppError::Localized { zh, .. } => assert!(
            zh.contains("不能删除当前正在使用的供应商"),
            "unexpected message: {zh}"
        ),
        AppError::Config(msg) => assert!(
            msg.contains("不能删除当前正在使用的供应商"),
            "unexpected message: {msg}"
        ),
        other => panic!("expected Config error, got {other:?}"),
    }
}

#[test]
fn provider_service_delete_provider_selected_in_local_settings_returns_error() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "keep".to_string();
        manager.providers.insert(
            "keep".to_string(),
            Provider::with_id(
                "keep".to_string(),
                "Keep".to_string(),
                json!({
                    "env": { "ANTHROPIC_API_KEY": "keep-key" }
                }),
                None,
            ),
        );
        manager.providers.insert(
            "delete".to_string(),
            Provider::with_id(
                "delete".to_string(),
                "Delete".to_string(),
                json!({
                    "env": { "ANTHROPIC_API_KEY": "delete-key" }
                }),
                None,
            ),
        );
    }

    let mut settings = AppSettings::default();
    settings.current_provider_claude = Some("delete".to_string());
    update_settings(settings).expect("set local current provider override");

    let app_state = state_from_config(config);

    let err = ProviderService::delete(&app_state, AppType::Claude, "delete")
        .expect_err("deleting local-settings current provider should fail");
    match err {
        AppError::Localized { zh, .. } => assert!(
            zh.contains("不能删除当前正在使用的供应商"),
            "unexpected message: {zh}"
        ),
        AppError::Config(msg) => assert!(
            msg.contains("不能删除当前正在使用的供应商"),
            "unexpected message: {msg}"
        ),
        other => panic!("expected Config error, got {other:?}"),
    }
}

#[test]
fn provider_service_delete_db_current_provider_returns_error_even_if_config_current_drifted() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    let keep = Provider::with_id(
        "keep".to_string(),
        "Keep".to_string(),
        json!({
            "env": { "ANTHROPIC_API_KEY": "keep-key" }
        }),
        None,
    );
    let delete = Provider::with_id(
        "delete".to_string(),
        "Delete".to_string(),
        json!({
            "env": { "ANTHROPIC_API_KEY": "delete-key" }
        }),
        None,
    );

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "keep".to_string();
        manager.providers.insert("keep".to_string(), keep.clone());
        manager
            .providers
            .insert("delete".to_string(), delete.clone());
    }

    let app_state = state_from_config(config);
    app_state
        .db
        .save_provider(AppType::Claude.as_str(), &keep)
        .expect("save keep provider to db");
    app_state
        .db
        .save_provider(AppType::Claude.as_str(), &delete)
        .expect("save delete provider to db");
    app_state
        .db
        .set_current_provider(AppType::Claude.as_str(), "delete")
        .expect("set db current provider");

    let err = ProviderService::delete(&app_state, AppType::Claude, "delete")
        .expect_err("deleting db current provider should fail");
    match err {
        AppError::Localized { zh, .. } => assert!(
            zh.contains("不能删除当前正在使用的供应商"),
            "unexpected message: {zh}"
        ),
        AppError::Config(msg) => assert!(
            msg.contains("不能删除当前正在使用的供应商"),
            "unexpected message: {msg}"
        ),
        other => panic!("expected Config error, got {other:?}"),
    }
}

#[test]
fn provider_service_delete_non_current_provider_succeeds_when_only_config_current_drifted() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    let keep = Provider::with_id(
        "keep".to_string(),
        "Keep".to_string(),
        json!({
            "env": { "ANTHROPIC_API_KEY": "keep-key" }
        }),
        None,
    );
    let delete = Provider::with_id(
        "delete".to_string(),
        "Delete".to_string(),
        json!({
            "env": { "ANTHROPIC_API_KEY": "delete-key" }
        }),
        None,
    );

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "delete".to_string();
        manager.providers.insert("keep".to_string(), keep.clone());
        manager
            .providers
            .insert("delete".to_string(), delete.clone());
    }

    let app_state = state_from_config(config);
    app_state
        .db
        .save_provider(AppType::Claude.as_str(), &keep)
        .expect("save keep provider to db");
    app_state
        .db
        .save_provider(AppType::Claude.as_str(), &delete)
        .expect("save delete provider to db");
    app_state
        .db
        .set_current_provider(AppType::Claude.as_str(), "keep")
        .expect("set db current provider");

    ProviderService::delete(&app_state, AppType::Claude, "delete")
        .expect("deleting non-current provider should succeed when only config.current drifted");

    let stored_current = app_state
        .db
        .get_current_provider(AppType::Claude.as_str())
        .expect("read db current provider after delete");
    assert_eq!(
        stored_current.as_deref(),
        Some("keep"),
        "delete should preserve the actual current provider stored in db"
    );

    let cfg = app_state.config.read().expect("read config after delete");
    let manager = cfg.get_manager(&AppType::Claude).expect("claude manager");
    assert_eq!(
        manager.current, "keep",
        "delete should resync stale config.current to the stored db current provider"
    );
    assert!(
        !manager.providers.contains_key("delete"),
        "deleted provider should be removed from config snapshot"
    );
}

#[test]
fn provider_service_sync_current_to_live_writes_active_codex_prompt_file() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    std::fs::create_dir_all(home.join(".codex")).expect("create codex dir");

    let config = config_with_prompt_entries(&[(
        &AppType::Codex,
        "codex-active",
        "# codex prompt\nUse the repo conventions.",
        true,
    )]);
    let state = state_from_config(config);

    ProviderService::sync_current_to_live(&state).expect("sync current to live");

    let prompt_path = home.join(".codex").join("AGENTS.md");
    assert_eq!(
        std::fs::read_to_string(&prompt_path).expect("read codex prompt"),
        "# codex prompt\nUse the repo conventions."
    );
}

#[test]
fn provider_service_sync_current_to_live_writes_active_claude_and_gemini_prompt_files() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    std::fs::create_dir_all(home.join(".claude")).expect("create claude dir");
    std::fs::create_dir_all(home.join(".gemini")).expect("create gemini dir");

    let config = config_with_prompt_entries(&[
        (&AppType::Claude, "claude-active", "# claude prompt", true),
        (&AppType::Gemini, "gemini-active", "# gemini prompt", true),
    ]);
    let state = state_from_config(config);

    ProviderService::sync_current_to_live(&state).expect("sync current to live");

    assert_eq!(
        std::fs::read_to_string(home.join(".claude").join("CLAUDE.md"))
            .expect("read claude prompt"),
        "# claude prompt"
    );
    assert_eq!(
        std::fs::read_to_string(home.join(".gemini").join("GEMINI.md"))
            .expect("read gemini prompt"),
        "# gemini prompt"
    );
}

#[test]
fn provider_service_sync_current_to_live_skips_prompt_file_for_uninitialized_apps() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let config =
        config_with_prompt_entries(&[(&AppType::Codex, "codex-active", "# codex prompt", true)]);
    let state = state_from_config(config);

    ProviderService::sync_current_to_live(&state).expect("sync current to live");

    assert!(
        !home.join(".codex").join("AGENTS.md").exists(),
        "prompt sync should skip uninitialized Codex home"
    );
}

#[test]
fn provider_service_sync_current_to_live_keeps_existing_prompt_file_without_active_prompt() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let codex_dir = home.join(".codex");
    std::fs::create_dir_all(&codex_dir).expect("create codex dir");
    let prompt_path = codex_dir.join("AGENTS.md");
    std::fs::write(&prompt_path, "unmanaged prompt").expect("seed unmanaged prompt");

    let config =
        config_with_prompt_entries(&[(&AppType::Codex, "codex-inactive", "# codex prompt", false)]);
    let state = state_from_config(config);

    ProviderService::sync_current_to_live(&state).expect("sync current to live");

    assert_eq!(
        std::fs::read_to_string(&prompt_path).expect("read prompt after sync"),
        "unmanaged prompt"
    );
}
