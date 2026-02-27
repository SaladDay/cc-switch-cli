use serde_json::json;
use std::collections::HashMap;

use cc_switch_lib::{
    get_claude_settings_path, read_json_file, write_codex_live_atomic, AppError, AppType, McpApps,
    McpServer, MultiAppConfig, Provider, ProviderMeta, ProviderService,
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
fn provider_service_switch_codex_missing_auth_is_allowed() {
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

    ProviderService::switch(&state, AppType::Codex, "invalid")
        .expect("switching should succeed without auth.json for Codex 0.64+");

    assert!(
        !cc_switch_lib::get_codex_auth_path().exists(),
        "auth.json should not be written when provider has no auth"
    );
    assert!(
        cc_switch_lib::get_codex_config_path().exists(),
        "config.toml should be written"
    );
}

#[test]
fn provider_service_switch_codex_openai_auth_removes_existing_auth_json() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    // Mark Codex as initialized so live sync is enabled.
    std::fs::create_dir_all(home.join(".codex")).expect("create codex dir (initialized)");

    // Seed a legacy auth.json that should not survive an OpenAI-auth (credential store) provider.
    let auth_path = cc_switch_lib::get_codex_auth_path();
    std::fs::write(&auth_path, r#"{"OPENAI_API_KEY":"stale-key"}"#).expect("seed auth.json");
    assert!(auth_path.exists(), "auth.json should exist before switch");

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "p2".to_string();

        // Target provider: OpenAI auth mode, no auth.json required.
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "OpenAI Official".to_string(),
                json!({
                    "config": "model_provider = \"p1\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.p1]\nbase_url = \"https://api.openai.com/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
                }),
                None,
            ),
        );

        // Current provider: uses API key (auth.json).
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
        .expect("switch to OpenAI auth provider should succeed");

    assert!(
        !auth_path.exists(),
        "auth.json should be removed when switching to OpenAI auth mode provider without auth config"
    );
    let backup_exists = std::fs::read_dir(home.join(".codex"))
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
        "auth.json should be backed up when removed in OpenAI auth mode"
    );
}

#[test]
fn provider_service_switch_codex_openai_auth_removes_existing_auth_json_even_if_provider_has_auth()
{
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    // Mark Codex as initialized so live sync is enabled.
    std::fs::create_dir_all(home.join(".codex")).expect("create codex dir (initialized)");

    // Seed a legacy auth.json that should not survive an OpenAI-auth (credential store) provider,
    // even when the provider snapshot still carries a legacy API key.
    let auth_path = cc_switch_lib::get_codex_auth_path();
    std::fs::write(&auth_path, r#"{"OPENAI_API_KEY":"stale-key"}"#).expect("seed auth.json");
    assert!(auth_path.exists(), "auth.json should exist before switch");

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "p2".to_string();

        // Target provider: OpenAI auth mode, but carries a legacy auth object (e.g. user filled it
        // previously). This must NOT result in a written auth.json that overrides OAuth.
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "OpenAI Official".to_string(),
                json!({
                    "auth": { "OPENAI_API_KEY": "sk-should-not-write" },
                    "config": "model_provider = \"p1\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.p1]\nbase_url = \"https://api.openai.com/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
                }),
                None,
            ),
        );

        // Current provider: uses API key (auth.json).
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
        .expect("switch to OpenAI auth provider should succeed");

    assert!(
        !auth_path.exists(),
        "auth.json should be removed when switching to OpenAI auth mode provider, even if provider has legacy auth"
    );

    let backup_exists = std::fs::read_dir(home.join(".codex"))
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
        "auth.json should be backed up when removed in OpenAI auth mode"
    );

    let live_text =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    assert!(
        live_text.contains("requires_openai_auth = true"),
        "config.toml should enable OpenAI auth"
    );
    assert!(
        !live_text.contains("env_key = \"OPENAI_API_KEY\""),
        "config.toml should not force OPENAI_API_KEY env var in OpenAI auth mode"
    );
}

#[test]
fn provider_service_switch_codex_defaults_wire_api_for_openai_official_when_missing() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    // Mark Codex as initialized so live sync is enabled.
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
                    // Intentionally omit wire_api to simulate older/partial configs.
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
    let live_value: toml::Value = toml::from_str(&live_text).expect("parse live config.toml");
    let model_provider = live_value
        .get("model_provider")
        .and_then(|v| v.as_str())
        .expect("model_provider should be set");
    let providers = live_value
        .get("model_providers")
        .and_then(|v| v.as_table())
        .expect("model_providers should exist");
    let openai_official = providers
        .get(model_provider)
        .and_then(|v| v.as_table())
        .expect("active provider table should exist");

    assert_eq!(
        openai_official.get("wire_api").and_then(|v| v.as_str()),
        Some("responses"),
        "wire_api should default to 'responses' for OpenAI official when missing from snippet"
    );
}

#[test]
fn provider_service_switch_codex_defaults_requires_openai_auth_for_openai_official_when_missing() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    // Mark Codex as initialized so live sync is enabled.
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
                    // Intentionally omit requires_openai_auth (and wire_api) to simulate older/partial configs.
                    "config": "model_provider = \"p1\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.p1]\nbase_url = \"https://api.openai.com/v1\"\n"
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
    let live_value: toml::Value = toml::from_str(&live_text).expect("parse live config.toml");

    let model_provider = live_value
        .get("model_provider")
        .and_then(|v| v.as_str())
        .expect("model_provider should be set");
    let provider_table = live_value
        .get("model_providers")
        .and_then(|v| v.as_table())
        .and_then(|providers| providers.get(model_provider))
        .and_then(|v| v.as_table())
        .expect("model_providers.<id> table should exist");

    assert_eq!(
        provider_table
            .get("requires_openai_auth")
            .and_then(|v| v.as_bool()),
        Some(true),
        "requires_openai_auth should default to true for OpenAI official base_url when missing"
    );
    assert_eq!(
        provider_table.get("wire_api").and_then(|v| v.as_str()),
        Some("responses"),
        "wire_api should default to 'responses' for OpenAI official base_url when missing"
    );
    assert!(
        provider_table.get("env_key").is_none(),
        "env_key should be omitted when defaulting to OpenAI auth mode"
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
fn provider_service_delete_current_provider_returns_error() {
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
    }

    let app_state = state_from_config(config);

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
