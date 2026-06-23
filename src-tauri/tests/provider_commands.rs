#![allow(clippy::await_holding_lock)]

use serde_json::json;
use serial_test::serial;
use std::collections::HashMap;
use std::fs;
use std::net::TcpListener;

use cc_switch_lib::{
    get_claude_settings_path, get_codex_auth_path, get_codex_config_path, read_json_file,
    update_settings, write_codex_live_atomic, AppSettings, AppType, McpApps, McpServer,
    MultiAppConfig, Provider, ProviderMeta, ProviderService, UsageScript,
};

use cc_switch_lib::cli::commands::provider::ProviderCommand;
use cc_switch_lib::cli::commands::provider_usage_query::{
    ProviderUsageQueryCommand, ProviderUsageQuerySetCommand, UsageQueryTemplate,
};

#[path = "support.rs"]
mod support;
use support::{
    enable_codex_official_auth_preservation, ensure_test_home, lock_test_mutex, reset_test_fs,
    state_from_config, CurrentDirGuard,
};

fn find_free_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind free local port");
    listener
        .local_addr()
        .expect("read local listener address")
        .port()
}

fn configure_live_dirs(
    home: &std::path::Path,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let opencode_dir = home.join("opencode-live");
    let hermes_dir = home.join("hermes-live");
    let openclaw_dir = home.join("openclaw-live");
    fs::create_dir_all(&opencode_dir).expect("create opencode live dir");
    fs::create_dir_all(&hermes_dir).expect("create hermes live dir");
    fs::create_dir_all(&openclaw_dir).expect("create openclaw live dir");
    update_settings(AppSettings {
        opencode_config_dir: Some(opencode_dir.display().to_string()),
        hermes_config_dir: Some(hermes_dir.display().to_string()),
        openclaw_config_dir: Some(openclaw_dir.display().to_string()),
        ..Default::default()
    })
    .expect("configure live dirs");
    (opencode_dir, hermes_dir, openclaw_dir)
}

fn provider_command(cmd: ProviderCommand, app: AppType) {
    cc_switch_lib::cli::commands::provider::execute(cmd, Some(app))
        .expect("provider command should succeed");
}

fn provider_command_result(
    cmd: ProviderCommand,
    app: AppType,
) -> Result<(), cc_switch_lib::AppError> {
    cc_switch_lib::cli::commands::provider::execute(cmd, Some(app))
}

fn usage_query_set_command(
    id: &str,
    template: Option<UsageQueryTemplate>,
) -> ProviderUsageQuerySetCommand {
    ProviderUsageQuerySetCommand {
        id: id.to_string(),
        enabled: false,
        disabled: false,
        template,
        code: None,
        timeout: None,
        auto_query_interval: None,
        api_key: None,
        base_url: None,
        access_token: None,
        user_id: None,
    }
}

fn saved_provider(app_type: AppType, id: &str) -> Provider {
    let refreshed = cc_switch_lib::AppState::try_new().expect("reload provider state");
    let config = refreshed.config.read().expect("lock provider state");
    config
        .get_manager(&app_type)
        .expect("provider manager")
        .providers
        .get(id)
        .cloned()
        .expect("saved provider")
}

fn usage_script_fixture() -> UsageScript {
    UsageScript {
        enabled: true,
        language: "javascript".to_string(),
        code: "return { remaining: 1, unit: 'USD' };".to_string(),
        timeout: Some(9),
        api_key: Some("sk-old".to_string()),
        base_url: Some("https://old.example.com".to_string()),
        access_token: None,
        user_id: None,
        template_type: Some("general".to_string()),
        auto_query_interval: Some(30),
        coding_plan_provider: None,
    }
}

#[test]
#[serial]
fn provider_usage_query_set_writes_upstream_defaults_and_preserves_meta() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "demo".to_string();
        let mut provider = Provider::with_id(
            "demo".to_string(),
            "Demo".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_API_KEY": "sk-provider"
                }
            }),
            None,
        );
        provider.meta = Some(ProviderMeta {
            apply_common_config: Some(true),
            endpoint_auto_select: Some(true),
            ..Default::default()
        });
        manager.providers.insert("demo".to_string(), provider);
    }

    let state = state_from_config(config);
    state.save().expect("persist test providers");
    drop(state);

    let mut command = usage_query_set_command("demo", Some(UsageQueryTemplate::General));
    command.enabled = true;
    command.api_key = Some("  sk-usage  ".to_string());
    command.base_url = Some("  https://usage.example.com  ".to_string());
    command.auto_query_interval = Some(1441);

    provider_command(
        ProviderCommand::UsageQuery(ProviderUsageQueryCommand::Set(command)),
        AppType::Claude,
    );

    let provider = saved_provider(AppType::Claude, "demo");
    let meta = provider.meta.expect("usage query should keep meta");
    assert_eq!(meta.apply_common_config, Some(true));
    assert_eq!(meta.endpoint_auto_select, Some(true));

    let script = meta.usage_script.expect("usage query should be saved");
    assert!(script.enabled);
    assert_eq!(script.language, "javascript");
    assert_eq!(script.template_type.as_deref(), Some("general"));
    assert_eq!(script.timeout, Some(10));
    assert_eq!(script.auto_query_interval, Some(1440));
    assert_eq!(script.api_key.as_deref(), Some("sk-usage"));
    assert_eq!(
        script.base_url.as_deref(),
        Some("https://usage.example.com")
    );
    assert_eq!(script.access_token, None);
    assert_eq!(script.user_id, None);
    assert_eq!(script.coding_plan_provider, None);
    assert!(script.code.contains("{{baseUrl}}/user/balance"));
    assert!(script.code.contains("{{apiKey}}"));
}

#[test]
#[serial]
fn provider_usage_query_set_newapi_clears_general_credentials() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "demo".to_string();
        let mut provider = Provider::with_id(
            "demo".to_string(),
            "Demo".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_API_KEY": "sk-provider"
                }
            }),
            None,
        );
        provider.meta = Some(ProviderMeta {
            apply_common_config: Some(false),
            usage_script: Some(usage_script_fixture()),
            ..Default::default()
        });
        manager.providers.insert("demo".to_string(), provider);
    }

    let state = state_from_config(config);
    state.save().expect("persist test providers");
    drop(state);

    let mut command = usage_query_set_command("demo", Some(UsageQueryTemplate::Newapi));
    command.base_url = Some("https://newapi.example.com".to_string());
    command.access_token = Some("token-demo".to_string());
    command.user_id = Some("user-demo".to_string());

    provider_command(
        ProviderCommand::UsageQuery(ProviderUsageQueryCommand::Set(command)),
        AppType::Claude,
    );

    let provider = saved_provider(AppType::Claude, "demo");
    let meta = provider.meta.expect("usage query should keep meta");
    assert_eq!(meta.apply_common_config, Some(false));

    let script = meta.usage_script.expect("usage query should be saved");
    assert!(script.enabled);
    assert_eq!(script.template_type.as_deref(), Some("newapi"));
    assert_eq!(script.timeout, Some(9));
    assert_eq!(script.auto_query_interval, Some(30));
    assert_eq!(script.api_key, None);
    assert_eq!(
        script.base_url.as_deref(),
        Some("https://newapi.example.com")
    );
    assert_eq!(script.access_token.as_deref(), Some("token-demo"));
    assert_eq!(script.user_id.as_deref(), Some("user-demo"));
    assert!(script.code.contains("{{accessToken}}"));
}

#[test]
#[serial]
fn provider_usage_query_clear_removes_only_usage_script() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "demo".to_string();
        let mut provider = Provider::with_id(
            "demo".to_string(),
            "Demo".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_API_KEY": "sk-provider"
                }
            }),
            None,
        );
        provider.meta = Some(ProviderMeta {
            apply_common_config: Some(true),
            endpoint_auto_select: Some(true),
            usage_script: Some(usage_script_fixture()),
            ..Default::default()
        });
        manager.providers.insert("demo".to_string(), provider);
    }

    let state = state_from_config(config);
    state.save().expect("persist test providers");
    drop(state);

    provider_command(
        ProviderCommand::UsageQuery(ProviderUsageQueryCommand::Clear {
            id: "demo".to_string(),
        }),
        AppType::Claude,
    );

    let provider = saved_provider(AppType::Claude, "demo");
    let meta = provider.meta.expect("meta should remain");
    assert_eq!(meta.apply_common_config, Some(true));
    assert_eq!(meta.endpoint_auto_select, Some(true));
    assert!(meta.usage_script.is_none());
}

#[test]
#[serial]
fn provider_usage_query_set_defaults_to_balance_template_from_provider_url() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "demo".to_string();
        manager.providers.insert(
            "demo".to_string(),
            Provider::with_id(
                "demo".to_string(),
                "Demo".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_API_KEY": "sk-provider",
                        "ANTHROPIC_BASE_URL": "https://openrouter.ai/api/v1"
                    }
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);
    state.save().expect("persist test providers");
    drop(state);

    let mut command = usage_query_set_command("demo", None);
    command.enabled = true;

    provider_command(
        ProviderCommand::UsageQuery(ProviderUsageQueryCommand::Set(command)),
        AppType::Claude,
    );

    let provider = saved_provider(AppType::Claude, "demo");
    let script = provider
        .meta
        .and_then(|meta| meta.usage_script)
        .expect("usage query should be saved");
    assert!(script.enabled);
    assert_eq!(script.template_type.as_deref(), Some("balance"));
    assert_eq!(script.timeout, Some(10));
    assert_eq!(script.auto_query_interval, Some(5));
    assert!(script.code.is_empty());
    assert_eq!(script.api_key, None);
    assert_eq!(script.base_url, None);
    assert_eq!(script.access_token, None);
    assert_eq!(script.user_id, None);
}

#[test]
#[serial]
fn provider_usage_query_set_rejects_enabled_script_without_return() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "demo".to_string();
        manager.providers.insert(
            "demo".to_string(),
            Provider::with_id(
                "demo".to_string(),
                "Demo".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_API_KEY": "sk-provider"
                    }
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);
    state.save().expect("persist test providers");
    drop(state);

    let mut command = usage_query_set_command("demo", Some(UsageQueryTemplate::Custom));
    command.enabled = true;
    command.code = Some("({ request: { url: 'https://usage.example.com' } })".to_string());

    let err = provider_command_result(
        ProviderCommand::UsageQuery(ProviderUsageQueryCommand::Set(command)),
        AppType::Claude,
    )
    .expect_err("enabled script without return should be rejected");

    assert!(
        err.to_string().contains("return"),
        "error should explain the missing return statement: {err}"
    );

    let provider = saved_provider(AppType::Claude, "demo");
    assert!(
        provider.meta.and_then(|meta| meta.usage_script).is_none(),
        "invalid Usage Query config must not be persisted"
    );
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
        let mut provider = Provider::with_id(
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
        );
        provider.meta = Some(cc_switch_lib::ProviderMeta {
            apply_common_config: Some(true),
            ..Default::default()
        });
        manager.providers.insert("demo".to_string(), provider);
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
fn provider_export_requires_explicit_common_config_opt_in() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let project_dir = home.join("project-no-common");
    let _cwd_guard = CurrentDirGuard::change_to(&project_dir);

    let mut config = MultiAppConfig::default();
    config.common_config_snippets.claude =
        Some(r#"{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1}}"#.to_string());
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
    assert!(
        exported
            .get("env")
            .and_then(|v| v.get("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"))
            .is_none(),
        "export must not apply common config when provider did not opt in"
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
#[serial]
fn provider_duplicate_persists_distinct_copy_and_skips_transient_state() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "provider-one".to_string();

        let mut original = Provider::with_id(
            "provider-one".to_string(),
            "Provider One".to_string(),
            json!({"env": {"ANTHROPIC_AUTH_TOKEN": "sk-original"}}),
            Some("https://example.com".to_string()),
        );
        original.created_at = Some(123);
        original.sort_index = Some(7);
        original.notes = Some("Retain this note".to_string());
        original.in_failover_queue = true;
        original.meta = Some(ProviderMeta {
            endpoint_auto_select: Some(true),
            ..Default::default()
        });

        manager.providers.insert(original.id.clone(), original);
        manager.providers.insert("provider-one-copy".to_string(), {
            let mut provider = Provider::with_id(
                "provider-one-copy".to_string(),
                "Existing Copy".to_string(),
                json!({}),
                None,
            );
            provider.sort_index = Some(8);
            provider
        });
        manager.providers.insert("later-provider".to_string(), {
            let mut provider = Provider::with_id(
                "later-provider".to_string(),
                "Later Provider".to_string(),
                json!({}),
                None,
            );
            provider.sort_index = Some(9);
            provider
        });
    }

    let state = state_from_config(config);
    state.save().expect("persist test providers");
    drop(state);

    cc_switch_lib::cli::commands::provider::execute(
        cc_switch_lib::cli::commands::provider::ProviderCommand::Duplicate {
            id: "provider-one".to_string(),
            edit: false,
        },
        Some(AppType::Claude),
    )
    .expect("duplicate command should succeed");

    let refreshed = cc_switch_lib::AppState::try_new().expect("reload provider state");
    let config = refreshed.config.read().expect("lock provider state");
    let manager = config
        .get_manager(&AppType::Claude)
        .expect("claude manager after duplicate");
    let original = manager
        .providers
        .get("provider-one")
        .expect("original provider remains");
    let copied = manager
        .providers
        .get("provider-one-copy-2")
        .expect("copy uses a collision-free id");

    assert_eq!(original.name, "Provider One");
    assert_eq!(copied.name, "Provider One copy");
    assert_eq!(copied.settings_config, original.settings_config);
    assert_eq!(copied.notes.as_deref(), Some("Retain this note"));
    assert_eq!(
        copied
            .meta
            .as_ref()
            .and_then(|meta| meta.endpoint_auto_select),
        Some(true)
    );
    assert!(copied.created_at.is_some());
    assert_eq!(copied.sort_index, Some(8));
    assert!(!copied.in_failover_queue);
    assert_eq!(
        manager
            .providers
            .get("provider-one-copy")
            .and_then(|provider| provider.sort_index),
        Some(9)
    );
    assert_eq!(
        manager
            .providers
            .get("later-provider")
            .and_then(|provider| provider.sort_index),
        Some(10)
    );
}

#[test]
#[serial]
fn provider_duplicate_opencode_skips_live_write_and_avoids_live_only_id() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let live_path = home.join(".config").join("opencode").join("opencode.json");
    std::fs::create_dir_all(live_path.parent().expect("live config parent"))
        .expect("create opencode config dir");
    std::fs::write(
        &live_path,
        serde_json::to_string_pretty(&json!({
            "provider": {
                "provider-one-copy": {
                    "npm": "@ai-sdk/openai-compatible",
                    "options": {
                        "baseURL": "https://live.example",
                        "apiKey": "sk-live"
                    },
                    "models": {
                        "live-model": { "name": "Live Model" }
                    }
                }
            }
        }))
        .expect("serialize live opencode config"),
    )
    .expect("seed live opencode config");

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenCode)
            .expect("opencode manager");
        manager.providers.insert(
            "provider-one".to_string(),
            Provider::with_id(
                "provider-one".to_string(),
                "Provider One".to_string(),
                json!({
                    "npm": "@ai-sdk/openai-compatible",
                    "options": {
                        "baseURL": "https://one.example",
                        "apiKey": "sk-one"
                    },
                    "models": {
                        "model-one": { "name": "Model One" }
                    }
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);
    state.save().expect("persist test providers");
    drop(state);

    cc_switch_lib::cli::commands::provider::execute(
        cc_switch_lib::cli::commands::provider::ProviderCommand::Duplicate {
            id: "provider-one".to_string(),
            edit: false,
        },
        Some(AppType::OpenCode),
    )
    .expect("duplicate command should succeed");

    let refreshed = cc_switch_lib::AppState::try_new().expect("reload provider state");
    let config = refreshed.config.read().expect("lock provider state");
    let manager = config
        .get_manager(&AppType::OpenCode)
        .expect("opencode manager after duplicate");
    let copied = manager
        .providers
        .get("provider-one-copy-2")
        .expect("copy should avoid live-only id");

    assert_eq!(
        copied
            .meta
            .as_ref()
            .and_then(|meta| meta.live_config_managed),
        Some(false)
    );

    let live_config: serde_json::Value =
        read_json_file(&live_path).expect("read opencode live config");
    let live_providers = live_config
        .get("provider")
        .and_then(|value| value.as_object())
        .expect("live provider map");
    assert!(live_providers.contains_key("provider-one-copy"));
    assert!(!live_providers.contains_key("provider-one-copy-2"));
}

#[test]
#[serial]
fn provider_duplicate_hermes_clears_read_only_source_marker() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Hermes)
            .expect("hermes manager");
        manager.providers.insert(
            "remote-provider".to_string(),
            Provider::with_id(
                "remote-provider".to_string(),
                "Remote Provider".to_string(),
                json!({
                    "api_mode": "chat_completions",
                    "base_url": "https://remote.example/v1",
                    "api_key": "sk-remote",
                    cc_switch_lib::hermes_config::PROVIDER_SOURCE_FIELD:
                        cc_switch_lib::hermes_config::PROVIDER_SOURCE_DICT,
                    "provider_key": "remote-provider",
                }),
                None,
            ),
        );
    }

    let state = state_from_config(config);
    state.save().expect("persist test providers");
    drop(state);

    cc_switch_lib::cli::commands::provider::execute(
        cc_switch_lib::cli::commands::provider::ProviderCommand::Duplicate {
            id: "remote-provider".to_string(),
            edit: false,
        },
        Some(AppType::Hermes),
    )
    .expect("duplicate command should succeed");

    let refreshed = cc_switch_lib::AppState::try_new().expect("reload provider state");
    let config = refreshed.config.read().expect("lock provider state");
    let manager = config
        .get_manager(&AppType::Hermes)
        .expect("hermes manager after duplicate");
    let copied = manager
        .providers
        .get("remote-provider-copy")
        .expect("copy should use source id copy suffix");

    assert!(copied
        .settings_config
        .get(cc_switch_lib::hermes_config::PROVIDER_SOURCE_FIELD)
        .is_none());
    assert!(copied.settings_config.get("provider_key").is_none());
    assert_eq!(
        copied
            .meta
            .as_ref()
            .and_then(|meta| meta.live_config_managed),
        Some(false)
    );
}

#[test]
#[serial]
fn provider_duplicate_missing_source_returns_error_without_creating_provider() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    let state = state_from_config(MultiAppConfig::default());
    state.save().expect("persist empty test state");
    drop(state);

    let err = cc_switch_lib::cli::commands::provider::execute(
        cc_switch_lib::cli::commands::provider::ProviderCommand::Duplicate {
            id: "missing-provider".to_string(),
            edit: false,
        },
        Some(AppType::Claude),
    )
    .expect_err("duplicating a missing provider should fail");
    assert!(err.to_string().contains("missing-provider"), "{err}");

    let refreshed = cc_switch_lib::AppState::try_new().expect("reload provider state");
    let config = refreshed.config.read().expect("lock provider state");
    assert!(config
        .get_manager(&AppType::Claude)
        .expect("claude manager after failed duplicate")
        .providers
        .is_empty());
}

#[test]
#[serial]
fn provider_live_config_cli_import_live_imports_additive_app_providers() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let (opencode_dir, hermes_dir, openclaw_dir) = configure_live_dirs(home);

    fs::write(
        opencode_dir.join("opencode.json"),
        serde_json::to_string_pretty(&json!({
            "$schema": "https://opencode.ai/config.json",
            "provider": {
                "open-live": {
                    "npm": "@ai-sdk/openai-compatible",
                    "name": "Open Live",
                    "options": {
                        "baseURL": "https://open.example/v1",
                        "apiKey": "sk-open"
                    },
                    "models": {
                        "open-model": { "name": "Open Model" }
                    }
                }
            }
        }))
        .expect("serialize opencode live config"),
    )
    .expect("write opencode live config");
    fs::write(
        hermes_dir.join("config.yaml"),
        r#"
custom_providers:
  - name: hermes-live
    base_url: https://hermes.example/v1
    api_key: sk-hermes
    models:
      hermes-model:
        context_length: 200000
model: {}
"#,
    )
    .expect("write hermes live config");
    fs::write(
        openclaw_dir.join("openclaw.json"),
        r#"
{
  models: {
    mode: 'merge',
    providers: {
      'claw-live': {
        api: 'openai-completions',
        models: [{ id: 'claw-model', name: 'Claw Model' }],
      },
    },
  },
}
"#,
    )
    .expect("write openclaw live config");

    let state = state_from_config(MultiAppConfig::default());
    state.save().expect("persist empty provider state");
    drop(state);

    provider_command(
        cc_switch_lib::cli::commands::provider::ProviderCommand::ImportLive,
        AppType::OpenCode,
    );
    provider_command(
        cc_switch_lib::cli::commands::provider::ProviderCommand::ImportLive,
        AppType::Hermes,
    );
    provider_command(
        cc_switch_lib::cli::commands::provider::ProviderCommand::ImportLive,
        AppType::OpenClaw,
    );

    let refreshed = cc_switch_lib::AppState::try_new().expect("reload provider state");
    let config = refreshed.config.read().expect("lock provider state");
    for (app_type, id) in [
        (AppType::OpenCode, "open-live"),
        (AppType::Hermes, "hermes-live"),
        (AppType::OpenClaw, "claw-live"),
    ] {
        let provider = config
            .get_manager(&app_type)
            .and_then(|manager| manager.providers.get(id))
            .unwrap_or_else(|| panic!("expected imported {id} provider"));
        assert_eq!(
            provider
                .meta
                .as_ref()
                .and_then(|meta| meta.live_config_managed),
            Some(true),
            "{id} should be marked live-config managed"
        );
    }
}

#[test]
#[serial]
fn provider_live_config_cli_remove_from_config_keeps_provider_saved() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let (opencode_dir, _, _) = configure_live_dirs(home);
    let live_path = opencode_dir.join("opencode.json");
    fs::write(
        &live_path,
        serde_json::to_string_pretty(&json!({
            "$schema": "https://opencode.ai/config.json",
            "provider": {
                "toggle": {
                    "npm": "@ai-sdk/openai-compatible",
                    "options": {
                        "baseURL": "https://toggle.example/v1",
                        "apiKey": "sk-toggle"
                    },
                    "models": {
                        "toggle-model": { "name": "Toggle Model" }
                    }
                }
            }
        }))
        .expect("serialize opencode live config"),
    )
    .expect("write opencode live config");

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenCode)
            .expect("opencode manager");
        let mut provider = Provider::with_id(
            "toggle".to_string(),
            "Toggle".to_string(),
            json!({
                "npm": "@ai-sdk/openai-compatible",
                "options": {
                    "baseURL": "https://toggle.example/v1",
                    "apiKey": "sk-toggle"
                },
                "models": {
                    "toggle-model": { "name": "Toggle Model" }
                }
            }),
            None,
        );
        provider.meta = Some(ProviderMeta {
            live_config_managed: Some(true),
            ..Default::default()
        });
        manager.providers.insert("toggle".to_string(), provider);
    }
    let state = state_from_config(config);
    state.save().expect("persist opencode provider");
    drop(state);

    provider_command(
        cc_switch_lib::cli::commands::provider::ProviderCommand::RemoveFromConfig {
            id: "toggle".to_string(),
        },
        AppType::OpenCode,
    );

    let live: serde_json::Value = read_json_file(&live_path).expect("read opencode live config");
    assert!(!live
        .get("provider")
        .and_then(|value| value.as_object())
        .is_some_and(|providers| providers.contains_key("toggle")));

    let refreshed = cc_switch_lib::AppState::try_new().expect("reload provider state");
    let config = refreshed.config.read().expect("lock provider state");
    let provider = config
        .get_manager(&AppType::OpenCode)
        .and_then(|manager| manager.providers.get("toggle"))
        .expect("provider should remain saved after remove-from-config");
    assert_eq!(
        provider
            .meta
            .as_ref()
            .and_then(|meta| meta.live_config_managed),
        Some(false)
    );
}

#[test]
#[serial]
fn provider_live_config_cli_remove_from_config_rejects_non_additive_apps() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    let state = state_from_config(MultiAppConfig::default());
    state.save().expect("persist empty provider state");
    drop(state);

    let err = cc_switch_lib::cli::commands::provider::execute(
        cc_switch_lib::cli::commands::provider::ProviderCommand::RemoveFromConfig {
            id: "demo".to_string(),
        },
        Some(AppType::Claude),
    )
    .expect_err("non-additive remove-from-config should fail");
    assert!(
        err.to_string().contains("additive") || err.to_string().contains("累加"),
        "{err}"
    );
}

#[test]
#[serial]
fn provider_live_config_cli_openclaw_remove_rejects_default_provider() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let (_, _, openclaw_dir) = configure_live_dirs(home);
    let live_path = openclaw_dir.join("openclaw.json");
    fs::write(
        &live_path,
        r#"
{
  models: {
    mode: 'merge',
    providers: {
      p1: {
        api: 'openai-completions',
        models: [{ id: 'primary-model', name: 'Primary' }],
      },
    },
  },
  agents: {
    defaults: {
      model: {
        primary: 'p1/primary-model',
      },
    },
  },
}
"#,
    )
    .expect("write openclaw live config");

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::OpenClaw)
            .expect("openclaw manager");
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({
                    "api": "openai-completions",
                    "models": [{ "id": "primary-model", "name": "Primary" }]
                }),
                None,
            ),
        );
    }
    let state = state_from_config(config);
    state.save().expect("persist openclaw provider");
    drop(state);

    let err = cc_switch_lib::cli::commands::provider::execute(
        cc_switch_lib::cli::commands::provider::ProviderCommand::RemoveFromConfig {
            id: "p1".to_string(),
        },
        Some(AppType::OpenClaw),
    )
    .expect_err("default OpenClaw provider should not be removable");
    assert!(err.to_string().contains("default") || err.to_string().contains("默认"));

    let live_source = fs::read_to_string(&live_path).expect("read openclaw live config");
    let live: serde_json::Value =
        json5::from_str(&live_source).expect("parse openclaw live config");
    assert!(live
        .get("models")
        .and_then(|models| models.get("providers"))
        .and_then(|providers| providers.as_object())
        .is_some_and(|providers| providers.contains_key("p1")));
}

#[test]
#[serial]
fn provider_live_config_cli_openclaw_set_default_uses_live_order_and_preserves_extra() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let (_, _, openclaw_dir) = configure_live_dirs(home);
    let live_path = openclaw_dir.join("openclaw.json");
    fs::write(
        &live_path,
        r#"
{
  models: {
    mode: 'merge',
    providers: {
      p1: {
        api: 'openai-completions',
        models: [
          { id: 'live-primary', name: 'Live Primary' },
          { id: 'snapshot-primary', name: 'Snapshot Primary' },
          { id: 'fallback-two', name: 'Fallback Two' },
        ],
      },
    },
  },
  agents: {
    defaults: {
      model: {
        primary: 'p1/snapshot-primary',
        fallbacks: ['p1/live-primary'],
        reasoningEffort: 'high',
      },
    },
  },
}
"#,
    )
    .expect("write openclaw live config");
    let state = state_from_config(MultiAppConfig::default());
    state.save().expect("persist empty provider state");
    drop(state);

    provider_command(
        cc_switch_lib::cli::commands::provider::ProviderCommand::SetDefault {
            id: "p1".to_string(),
            model: None,
        },
        AppType::OpenClaw,
    );

    let live_source = fs::read_to_string(&live_path).expect("read openclaw live config");
    let live: serde_json::Value =
        json5::from_str(&live_source).expect("parse openclaw live config");
    let model = live
        .get("agents")
        .and_then(|agents| agents.get("defaults"))
        .and_then(|defaults| defaults.get("model"))
        .expect("default model should exist");
    assert_eq!(
        model.get("primary").and_then(|value| value.as_str()),
        Some("p1/live-primary")
    );
    assert_eq!(
        model
            .get("fallbacks")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str())
                    .collect::<Vec<_>>()
            }),
        Some(vec!["p1/snapshot-primary", "p1/fallback-two"])
    );
    assert_eq!(
        model
            .get("reasoningEffort")
            .and_then(|value| value.as_str()),
        Some("high")
    );
}

#[test]
#[serial]
fn provider_live_config_cli_hermes_set_default_uses_switch_semantics() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let (_, hermes_dir, _) = configure_live_dirs(home);
    let live_path = hermes_dir.join("config.yaml");
    fs::write(&live_path, "custom_providers: []\nmodel: {}\n").expect("write hermes live config");

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Hermes)
            .expect("hermes manager");
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "Hermes Provider".to_string(),
                json!({
                    "base_url": "https://hermes.example/v1",
                    "api_key": "sk-hermes",
                    "models": [{ "id": "hermes-model", "name": "Hermes Model" }]
                }),
                None,
            ),
        );
    }
    let state = state_from_config(config);
    state.save().expect("persist hermes provider");
    drop(state);

    provider_command(
        cc_switch_lib::cli::commands::provider::ProviderCommand::SetDefault {
            id: "p1".to_string(),
            model: None,
        },
        AppType::Hermes,
    );

    let live_source = fs::read_to_string(&live_path).expect("read hermes live config");
    let live: serde_yaml::Value = serde_yaml::from_str(&live_source).expect("parse hermes yaml");
    assert_eq!(
        live.get("model")
            .and_then(|model| model.get("provider"))
            .and_then(|value| value.as_str()),
        Some("p1")
    );
    assert!(live
        .get("custom_providers")
        .and_then(|providers| providers.as_sequence())
        .is_some_and(|providers| {
            providers.iter().any(|provider| {
                provider
                    .get("name")
                    .and_then(|value| value.as_str())
                    .is_some_and(|name| name == "p1")
            })
        }));

    let refreshed = cc_switch_lib::AppState::try_new().expect("reload provider state");
    assert_eq!(
        ProviderService::current(&refreshed, AppType::Hermes)
            .expect("read hermes current provider"),
        "p1"
    );
}

#[test]
fn switch_provider_updates_codex_live_and_state() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    enable_codex_official_auth_preservation();
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
        "legacy-key",
        "third-party Codex switches should preserve the user's auth.json login cache"
    );

    let config_text = std::fs::read_to_string(get_codex_config_path()).expect("read config.toml");
    assert!(
        config_text.contains("mcp_servers.echo-server"),
        "config.toml should contain synced MCP servers"
    );
    let parsed_config: toml::Value = toml::from_str(&config_text).expect("parse config.toml");
    assert_eq!(
        parsed_config
            .get("model_providers")
            .and_then(|value| value.get("latest"))
            .and_then(|value| value.get("experimental_bearer_token"))
            .and_then(|value| value.as_str()),
        Some("fresh-key"),
        "third-party provider token should be written into the active model provider table"
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
            "ANTHROPIC_API_KEY": "fresh-key",
            "LOCAL_ONLY": "preserve-me"
        },
        "workspace": {
            "path": "/tmp/new-workspace",
            "localOnly": true
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
        let mut new_provider = Provider::with_id(
            "new-provider".to_string(),
            "Fresh Claude".to_string(),
            json!({
                "env": { "ANTHROPIC_API_KEY": "fresh-key" },
                "workspace": { "path": "/tmp/new-workspace" }
            }),
            None,
        );
        new_provider.meta = Some(cc_switch_lib::ProviderMeta {
            apply_common_config: Some(true),
            ..Default::default()
        });
        manager
            .providers
            .insert("new-provider".to_string(), new_provider);
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
async fn switch_provider_under_takeover_refreshes_claude_restore_backup_to_selected_provider() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let settings_path = get_claude_settings_path();
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent).expect("create claude settings dir");
    }

    let provider_a_live = json!({
        "env": {
            "ANTHROPIC_API_KEY": "stale-key",
            "ANTHROPIC_BASE_URL": "https://a.example",
            "LOCAL_ONLY": "preserve-me"
        },
        "workspace": {
            "path": "/tmp/old-workspace",
            "localOnly": true
        }
    });
    std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&provider_a_live).expect("serialize provider A live"),
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
                    "env": {
                        "ANTHROPIC_API_KEY": "stale-key",
                        "ANTHROPIC_BASE_URL": "https://a.example"
                    },
                    "workspace": { "path": "/tmp/old-workspace" }
                }),
                None,
            ),
        );
        let mut new_provider = Provider::with_id(
            "new-provider".to_string(),
            "Fresh Claude".to_string(),
            json!({
                "env": { "ANTHROPIC_API_KEY": "fresh-key" },
                "workspace": { "path": "/tmp/new-workspace" }
            }),
            None,
        );
        new_provider.meta = Some(cc_switch_lib::ProviderMeta {
            apply_common_config: Some(true),
            ..Default::default()
        });
        manager
            .providers
            .insert("new-provider".to_string(), new_provider);
        manager.providers.insert(
            "future-provider".to_string(),
            Provider::with_id(
                "future-provider".to_string(),
                "Future Claude".to_string(),
                json!({
                    "env": { "ANTHROPIC_API_KEY": "future-key" },
                    "workspace": { "path": "/tmp/future-workspace" }
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

    let proxy_port = find_free_port();
    state
        .db
        .set_app_proxy_preferred_port("claude", proxy_port)
        .expect("update claude proxy port");

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
        Some(format!("http://127.0.0.1:{proxy_port}").as_str()),
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
            .and_then(|env| env.get("ANTHROPIC_API_KEY"))
            .and_then(|value| value.as_str()),
        Some("fresh-key"),
        "takeover-time switch should refresh the restore backup to the selected provider"
    );
    assert_eq!(
        backup_after_switch
            .get("env")
            .and_then(|env| env.get("LOCAL_ONLY"))
            .and_then(|value| value.as_str()),
        Some("preserve-me"),
        "takeover-time switch should keep local-only live config values in the restore backup"
    );
    assert!(
        backup_after_switch
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
            .is_none(),
        "takeover-time switch should remove provider-owned keys that are absent from the selected provider"
    );
    assert_eq!(
        backup_after_switch
            .get("env")
            .and_then(|env| env.get("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"))
            .and_then(|value| value.as_str()),
        Some("1"),
        "selected-provider restore backup should use normal Claude common snippet semantics"
    );

    let failover_snapshot = state
        .db
        .get_failover_live_snapshot("claude", "new-provider")
        .await
        .expect("read generated failover snapshot after takeover-time switch")
        .expect("new provider failover snapshot should exist after switch");
    let failover_snapshot: serde_json::Value = serde_json::from_str(&failover_snapshot.config_json)
        .expect("parse generated failover snapshot after takeover-time switch");
    assert_eq!(
        failover_snapshot
            .get("env")
            .and_then(|env| env.get("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"))
            .and_then(|value| value.as_str()),
        Some("1"),
        "takeover-time switch should generate a provider snapshot with normal Claude common snippet semantics"
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
        "restore after a takeover-time switch should recover the selected provider"
    );
    assert_eq!(
        restored_live
            .get("env")
            .and_then(|env| env.get("LOCAL_ONLY"))
            .and_then(|value| value.as_str()),
        Some("preserve-me"),
        "restore after a takeover-time switch should keep local-only live config values"
    );
    assert!(
        restored_live
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
            .is_none(),
        "restore after a takeover-time switch should not restore stale provider-owned keys"
    );
    assert_ne!(
        restored_live
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_API_KEY"))
            .and_then(|value| value.as_str()),
        Some("stale-key"),
        "disable takeover must not restore the provider that was current when takeover started"
    );

    ProviderService::switch(&state, AppType::Claude, "future-provider")
        .expect("switching away after takeover restore should succeed");

    let saved_new_provider = saved_provider(AppType::Claude, "new-provider");
    assert_eq!(
        saved_new_provider
            .settings_config
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_API_KEY"))
            .and_then(|value| value.as_str()),
        Some("fresh-key"),
        "switching after takeover restore should not backfill stale provider A into provider B"
    );

    let live_after_future_switch: serde_json::Value =
        read_json_file(&settings_path).expect("read claude live settings after future switch");
    assert_eq!(
        live_after_future_switch
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_API_KEY"))
            .and_then(|value| value.as_str()),
        Some("future-key"),
        "switching after takeover restore should write provider C instead of stale provider A"
    );
}
