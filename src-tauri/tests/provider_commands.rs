use serde_json::json;
use serial_test::serial;
use std::collections::HashMap;
use std::net::TcpListener;

use cc_switch_lib::{
    get_claude_settings_path, get_codex_auth_path, get_codex_config_path, read_json_file,
    write_codex_live_atomic, AppType, McpApps, McpServer, MultiAppConfig, Provider,
    ProviderService,
};

#[path = "support.rs"]
mod support;
use support::{
    ensure_test_home, lock_test_mutex, reset_test_fs, state_from_config, CurrentDirGuard,
};

fn find_free_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind free local port");
    listener
        .local_addr()
        .expect("read local listener address")
        .port()
}

#[test]
#[serial]
fn provider_export_writes_merged_claude_settings_to_default_path() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let project_dir = home.join("project-a");
    let _cwd_guard = CurrentDirGuard::change_to(&project_dir);

    let mut config = MultiAppConfig::default();
    config.common_config_snippets.claude = Some(
        r#"{
  "env": {
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": 1
  },
  "includeCoAuthoredBy": false
}"#
        .to_string(),
    );
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "demo".to_string();
        manager.providers.insert(
            "demo".to_string(),
            Provider::with_id(
                "demo".to_string(),
                "demo".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_API_KEY": "sk-demo"
                    },
                    "permissions": {
                        "allow": ["Bash"]
                    }
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);
    state.save().expect("persist test config");

    cc_switch_lib::cli::commands::provider::execute(
        cc_switch_lib::cli::commands::provider::ProviderCommand::Export {
            id: "demo".to_string(),
            output: None,
        },
        Some(AppType::Claude),
    )
    .expect("export command should succeed");

    let export_path = project_dir.join(".claude").join("settings.local.json");
    let exported: serde_json::Value = read_json_file(&export_path).expect("read exported file");

    assert_eq!(
        exported
            .get("env")
            .and_then(|v| v.get("ANTHROPIC_API_KEY"))
            .and_then(|v| v.as_str()),
        Some("sk-demo"),
        "provider env should be preserved"
    );
    assert_eq!(
        exported
            .get("env")
            .and_then(|v| v.get("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"))
            .and_then(|v| v.as_i64()),
        Some(1),
        "common config snippet should be merged into export"
    );
    assert_eq!(
        exported
            .get("includeCoAuthoredBy")
            .and_then(|v| v.as_bool()),
        Some(false),
        "top-level common config should be present in export"
    );
    assert_eq!(
        exported
            .get("permissions")
            .and_then(|v| v.get("allow"))
            .and_then(|v| v.as_array())
            .map(|values| values.len()),
        Some(1),
        "provider-specific settings should remain in export"
    );
}

#[test]
#[serial]
fn provider_export_respects_apply_common_config_flag_and_custom_output() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    config.common_config_snippets.claude =
        Some(r#"{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1}}"#.to_string());
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "demo".to_string();
        let mut provider = Provider::with_id(
            "demo".to_string(),
            "demo".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_API_KEY": "sk-demo"
                }
            }),
            None,
        );
        provider.meta = Some(cc_switch_lib::ProviderMeta {
            apply_common_config: Some(false),
            ..Default::default()
        });
        manager.providers.insert("demo".to_string(), provider);
    }

    let state = state_from_config(config);
    state.save().expect("persist test config");

    let output_path = home.join("exports").join("custom-settings.json");
    cc_switch_lib::cli::commands::provider::execute(
        cc_switch_lib::cli::commands::provider::ProviderCommand::Export {
            id: "demo".to_string(),
            output: Some(output_path.clone()),
        },
        Some(AppType::Claude),
    )
    .expect("export command should succeed");

    let exported: serde_json::Value = read_json_file(&output_path).expect("read exported file");
    assert!(
        exported
            .get("env")
            .and_then(|v| v.get("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"))
            .is_none(),
        "common config should be skipped when applyCommonConfig=false"
    );
}

#[test]
#[serial]
fn provider_export_rejects_non_claude_apps() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    let err = cc_switch_lib::cli::commands::provider::execute(
        cc_switch_lib::cli::commands::provider::ProviderCommand::Export {
            id: "demo".to_string(),
            output: None,
        },
        Some(AppType::Codex),
    )
    .expect_err("non-claude export should fail");

    assert!(
        err.to_string().contains("supports only Claude"),
        "error should explain Claude-only support"
    );
}

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
                hermes: false,
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
fn switch_provider_codex_rejects_missing_auth() {
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

    let err = ProviderService::switch(&app_state, AppType::Codex, "invalid")
        .expect_err("switching should fail when provider snapshot has no auth");

    let locked = app_state.config.read().expect("lock config after failure");
    let manager = locked.get_manager(&AppType::Codex).expect("codex manager");
    assert!(
        manager.current != "invalid",
        "current provider should not update after a failed switch"
    );
    assert!(
        err.to_string().contains("auth"),
        "expected auth-related error, got {err}"
    );

    let auth_path = get_codex_auth_path();
    assert!(
        !auth_path.exists(),
        "auth.json should remain absent on failed switch"
    );
    let cfg_path = get_codex_config_path();
    assert!(!cfg_path.exists(), "config.toml should not be written");
}

#[tokio::test]
#[serial]
async fn switch_provider_under_takeover_keeps_claude_live_pointing_to_proxy_and_updates_restore_backup(
) {
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
    config.common_config_snippets.claude = Some(
        serde_json::json!({
            "env": {
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1"
            }
        })
        .to_string(),
    );

    let state = state_from_config(config);
    state.save().expect("persist provider state to db");

    let mut proxy_config = state
        .proxy_service
        .get_config()
        .await
        .expect("read proxy config");
    proxy_config.listen_port = find_free_port();
    state
        .proxy_service
        .update_config(&proxy_config)
        .await
        .expect("update proxy config");

    state
        .proxy_service
        .set_takeover_for_app("claude", true)
        .await
        .expect("enable claude takeover");

    ProviderService::switch(&state, AppType::Claude, "new-provider")
        .expect("switch provider should succeed under takeover");

    let live_during_takeover: serde_json::Value =
        read_json_file(&settings_path).expect("read claude live settings under takeover");
    assert_eq!(
        live_during_takeover
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
            .and_then(|value| value.as_str()),
        Some(format!("http://127.0.0.1:{}", proxy_config.listen_port).as_str()),
        "provider switch under takeover should keep Claude live config pointed at the local proxy"
    );
    assert_eq!(
        live_during_takeover
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_API_KEY"))
            .and_then(|value| value.as_str()),
        Some("PROXY_MANAGED"),
        "provider switch under takeover should keep the managed placeholder in Claude live config"
    );

    let backup_after_switch = state
        .db
        .get_live_backup("claude")
        .await
        .expect("read claude live backup after takeover-time switch")
        .expect("claude takeover backup should exist after switch");
    let backup_after_switch: serde_json::Value =
        serde_json::from_str(&backup_after_switch.original_config)
            .expect("parse claude live backup after takeover-time switch");
    assert_eq!(
        backup_after_switch
            .get("env")
            .and_then(|env| env.get("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"))
            .and_then(|value| value.as_str()),
        Some("1"),
        "takeover-time switch should refresh the stored backup with the same Claude common snippet semantics"
    );

    state
        .proxy_service
        .set_takeover_for_app("claude", false)
        .await
        .expect("disable claude takeover");

    let restored_live: serde_json::Value =
        read_json_file(&settings_path).expect("read restored claude live settings");
    assert_eq!(
        restored_live
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_API_KEY"))
            .and_then(|value| value.as_str()),
        Some("fresh-key"),
        "restore after a takeover-time switch should recover the new provider config, not the pre-switch one"
    );
    assert_eq!(
        restored_live
            .get("env")
            .and_then(|env| env.get("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"))
            .and_then(|value| value.as_str()),
        Some("1"),
        "restore after a takeover-time switch should keep the normal Claude common snippet semantics"
    );
}
