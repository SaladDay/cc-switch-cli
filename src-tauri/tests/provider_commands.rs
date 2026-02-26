use serde_json::json;
use std::collections::HashMap;

use cc_switch_lib::{
    get_codex_auth_path, get_codex_config_path, read_json_file, write_codex_live_atomic, AppType,
    McpApps, McpServer, MultiAppConfig, Provider, ProviderService,
};

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs, state_from_config};

#[test]
fn switch_provider_updates_codex_live_and_state() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let legacy_auth = json!({"OPENAI_API_KEY": "legacy-key"});
    let legacy_config = r#"[mcp_servers.legacy]
type = "stdio"
command = "echo"
"#;
    write_codex_live_atomic(&legacy_auth, Some(legacy_config))
        .expect("seed existing codex live config");

    let mut config = MultiAppConfig::default();
    {
        let manager = config
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
            },
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        },
    );

    let app_state = state_from_config(config);

    ProviderService::switch(&app_state, AppType::Codex, "new-provider")
        .expect("switch provider should succeed");

    let auth_value: serde_json::Value =
        read_json_file(&get_codex_auth_path()).expect("read auth.json");
    assert_eq!(
        auth_value
            .get("OPENAI_API_KEY")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        "fresh-key",
        "live auth.json should reflect new provider"
    );

    let config_text = std::fs::read_to_string(get_codex_config_path()).expect("read config.toml");
    assert!(
        config_text.contains("mcp_servers.echo-server"),
        "config.toml should contain synced MCP servers"
    );

    let locked = app_state.config.read().expect("lock config after switch");
    let manager = locked
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
fn switch_provider_codex_accepts_full_config_toml_and_preserves_base_url() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    // Mark Codex as initialized so live sync is enabled.
    let config_path = get_codex_config_path();
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).expect("create codex dir");
    }

    let full_config = r#"model_provider = "azure"
model = "gpt-5.1-codex"
disable_response_storage = true

[model_providers.azure]
name = "azure"
base_url = "https://old.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#;

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
                "Duck Coding".to_string(),
                json!({
                    "auth": {"OPENAI_API_KEY": "sk-test"},
                    "config": full_config
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);

    ProviderService::switch(&state, AppType::Codex, "p1").expect("switch should succeed");

    let live_text = std::fs::read_to_string(get_codex_config_path()).expect("read config.toml");
    let live_value: toml::Value = toml::from_str(&live_text).expect("parse live config.toml");

    assert_eq!(
        live_value.get("model_provider").and_then(|v| v.as_str()),
        Some("azure"),
        "model_provider should be preserved from stored config"
    );

    let providers = live_value
        .get("model_providers")
        .and_then(|v| v.as_table())
        .expect("model_providers should exist");
    let provider_table = providers
        .get("azure")
        .and_then(|v| v.as_table())
        .expect("azure provider table should exist");
    assert_eq!(
        provider_table.get("base_url").and_then(|v| v.as_str()),
        Some("https://old.example/v1"),
        "base_url should be carried over from stored config"
    );
    assert_eq!(
        provider_table.get("wire_api").and_then(|v| v.as_str()),
        Some("responses"),
        "wire_api should be carried over from stored config"
    );
    assert_eq!(
        provider_table
            .get("requires_openai_auth")
            .and_then(|v| v.as_bool()),
        Some(true),
        "requires_openai_auth should be carried over from stored config"
    );
}

#[test]
fn switch_provider_missing_provider_returns_error() {
    let _guard = lock_test_mutex();
    reset_test_fs();

    let mut config = MultiAppConfig::default();
    config
        .get_manager_mut(&AppType::Claude)
        .expect("claude manager")
        .current = "does-not-exist".to_string();

    let app_state = state_from_config(config);

    let err = ProviderService::switch(&app_state, AppType::Claude, "missing-provider")
        .expect_err("switching to a missing provider should fail");

    assert!(
        err.to_string().contains("供应商不存在"),
        "error message should mention missing provider"
    );
}

#[test]
fn switch_provider_updates_claude_live_and_state() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let settings_path = cc_switch_lib::get_claude_settings_path();
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

    let app_state = state_from_config(config);

    ProviderService::switch(&app_state, AppType::Claude, "new-provider")
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

    let locked = app_state.config.read().expect("lock config after switch");
    let manager = locked
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

    let new_provider = manager
        .providers
        .get("new-provider")
        .expect("new provider exists");
    assert_eq!(
        new_provider
            .settings_config
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_API_KEY"))
            .and_then(|key| key.as_str()),
        Some("fresh-key"),
        "new provider snapshot should retain fresh auth"
    );

    drop(locked);

    let current = app_state
        .db
        .get_current_provider(AppType::Claude.as_str())
        .expect("read current provider from db");
    assert_eq!(
        current.as_deref(),
        Some("new-provider"),
        "db should record the new current provider"
    );
}

#[test]
fn switch_provider_codex_allows_missing_auth_and_writes_config() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();
    if let Some(parent) = get_codex_config_path().parent() {
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
                    "config": "[mcp_servers.test]\ncommand = \"noop\""
                }),
                None,
            ),
        );
    }

    let app_state = state_from_config(config);

    ProviderService::switch(&app_state, AppType::Codex, "invalid")
        .expect("switching should succeed without auth.json for Codex 0.64+");

    let locked = app_state.config.read().expect("lock config after failure");
    let manager = locked.get_manager(&AppType::Codex).expect("codex manager");
    assert!(
        manager.current == "invalid",
        "current provider should update after successful switch"
    );

    let auth_path = get_codex_auth_path();
    assert!(
        !auth_path.exists(),
        "auth.json should not be written when provider has no auth"
    );
    let cfg_path = get_codex_config_path();
    assert!(cfg_path.exists(), "config.toml should be written");
}
