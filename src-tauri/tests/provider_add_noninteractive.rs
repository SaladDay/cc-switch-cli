#![allow(clippy::await_holding_lock)]

//! Integration tests for the non-interactive `provider add` command.
//!
//! `provider add` is flag-driven (no interactive prompts); the interactive
//! path lives in the TUI. These tests drive `ProviderCommand::Add` through the
//! command dispatcher and assert on the persisted provider.

use cc_switch_lib::cli::commands::provider::{ClaudeApiKeyFieldArg, ProviderCommand};
use cc_switch_lib::cli::commands::provider_input::ProviderAddTemplate;
use cc_switch_lib::{AppType, MultiAppConfig, Provider};

use serial_test::serial;

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs, state_from_config};

/// Optional flags for [`add_command`]; mirrors the CLI `Add` variant so tests
/// only spell out the fields they care about.
#[derive(Default)]
struct AddOpts {
    template: Option<ProviderAddTemplate>,
    id: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
    config: Option<String>,
    config_file: Option<std::path::PathBuf>,
    website_url: Option<String>,
    notes: Option<String>,
    sort_index: Option<usize>,
    api_key_field: Option<ClaudeApiKeyFieldArg>,
    api_format: Option<String>,
    common_config: bool,
    account_id: Option<String>,
    fast_mode: bool,
}

fn add_command(name: Option<&str>, opts: AddOpts) -> ProviderCommand {
    ProviderCommand::Add {
        template: opts.template,
        name: name.map(str::to_string),
        id: opts.id,
        base_url: opts.base_url,
        api_key: opts.api_key,
        model: opts.model,
        config: opts.config,
        config_file: opts.config_file,
        website_url: opts.website_url,
        notes: opts.notes,
        sort_index: opts.sort_index,
        api_key_field: opts.api_key_field,
        api_format: opts.api_format,
        common_config: opts.common_config,
        account_id: opts.account_id,
        fast_mode: opts.fast_mode,
    }
}

fn run_add(name: Option<&str>, app: AppType, opts: AddOpts) -> Result<(), cc_switch_lib::AppError> {
    cc_switch_lib::cli::commands::provider::execute(add_command(name, opts), Some(app))
}

fn prepare_empty_state() {
    reset_test_fs();
    ensure_test_home();
    let state = state_from_config(MultiAppConfig::default());
    state.save().expect("persist empty test config");
    drop(state);
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

fn env_str<'a>(provider: &'a Provider, key: &str) -> Option<&'a str> {
    provider
        .settings_config
        .get("env")
        .and_then(|env| env.get(key))
        .and_then(|value| value.as_str())
}

#[test]
#[serial]
fn add_without_name_errors() {
    let _guard = lock_test_mutex();
    prepare_empty_state();

    let err = run_add(
        None,
        AppType::Claude,
        AddOpts {
            base_url: Some("https://api.example.com".to_string()),
            api_key: Some("sk-test".to_string()),
            ..Default::default()
        },
    )
    .expect_err("provider add without --name should fail");
    assert!(
        err.to_string().to_lowercase().contains("name"),
        "error should mention the missing name flag: {err}"
    );
}

#[test]
#[serial]
fn add_claude_field_mode_defaults_to_auth_token() {
    let _guard = lock_test_mutex();
    prepare_empty_state();

    run_add(
        Some("My Proxy"),
        AppType::Claude,
        AddOpts {
            base_url: Some("https://api.example.com".to_string()),
            api_key: Some("sk-test-123".to_string()),
            model: Some("sonnet".to_string()),
            ..Default::default()
        },
    )
    .expect("claude field-mode add should succeed");

    let provider = saved_provider(AppType::Claude, "my-proxy");
    assert_eq!(
        env_str(&provider, "ANTHROPIC_AUTH_TOKEN"),
        Some("sk-test-123")
    );
    assert_eq!(env_str(&provider, "ANTHROPIC_API_KEY"), None);
    assert_eq!(
        env_str(&provider, "ANTHROPIC_BASE_URL"),
        Some("https://api.example.com")
    );
    assert_eq!(env_str(&provider, "ANTHROPIC_MODEL"), Some("sonnet"));
    // Default auth-token field should not add an apiKeyField override.
    assert!(provider
        .meta
        .as_ref()
        .and_then(|meta| meta.api_key_field.as_ref())
        .is_none());
}

#[test]
#[serial]
fn add_claude_api_key_field_sets_meta() {
    let _guard = lock_test_mutex();
    prepare_empty_state();

    run_add(
        Some("KeyMode"),
        AppType::Claude,
        AddOpts {
            base_url: Some("https://k".to_string()),
            api_key: Some("sk-key".to_string()),
            api_key_field: Some(ClaudeApiKeyFieldArg::ApiKey),
            ..Default::default()
        },
    )
    .expect("claude add with api-key field should succeed");

    let provider = saved_provider(AppType::Claude, "keymode");
    assert_eq!(env_str(&provider, "ANTHROPIC_API_KEY"), Some("sk-key"));
    assert_eq!(env_str(&provider, "ANTHROPIC_AUTH_TOKEN"), None);
    assert_eq!(
        provider
            .meta
            .as_ref()
            .and_then(|meta| meta.api_key_field.as_deref()),
        Some("ANTHROPIC_API_KEY")
    );
}

#[test]
#[serial]
fn add_claude_missing_base_url_errors() {
    let _guard = lock_test_mutex();
    prepare_empty_state();

    let err = run_add(
        Some("NoUrl"),
        AppType::Claude,
        AddOpts {
            api_key: Some("sk-x".to_string()),
            ..Default::default()
        },
    )
    .expect_err("claude field-mode add without base url should fail");
    assert!(
        err.to_string().contains("--base-url"),
        "error should mention --base-url: {err}"
    );
}

#[test]
#[serial]
fn add_optional_metadata_is_persisted() {
    let _guard = lock_test_mutex();
    prepare_empty_state();

    run_add(
        Some("Meta"),
        AppType::Claude,
        AddOpts {
            base_url: Some("https://m".to_string()),
            api_key: Some("sk-m".to_string()),
            notes: Some("  team note  ".to_string()),
            website_url: Some("https://site.example".to_string()),
            sort_index: Some(7),
            ..Default::default()
        },
    )
    .expect("claude add with metadata should succeed");

    let provider = saved_provider(AppType::Claude, "meta");
    assert_eq!(provider.notes.as_deref(), Some("team note"));
    assert_eq!(
        provider.website_url.as_deref(),
        Some("https://site.example")
    );
    assert_eq!(provider.sort_index, Some(7));
}

#[test]
#[serial]
fn add_codex_field_mode_builds_config_toml() {
    let _guard = lock_test_mutex();
    prepare_empty_state();

    run_add(
        Some("CodexProxy"),
        AppType::Codex,
        AddOpts {
            base_url: Some("https://api.deepseek.com".to_string()),
            api_key: Some("sk-cx".to_string()),
            model: Some("gpt-5.4".to_string()),
            ..Default::default()
        },
    )
    .expect("codex field-mode add should succeed");

    let provider = saved_provider(AppType::Codex, "codexproxy");
    let config_text = provider
        .settings_config
        .get("config")
        .and_then(|value| value.as_str())
        .expect("codex config toml");
    assert!(config_text.contains("model_provider = \"custom\""));
    assert!(config_text.contains("[model_providers.custom]"));
    assert!(config_text.contains("name = \"CodexProxy\""));
    assert!(!config_text.contains("[model_providers.codexproxy]"));
    assert!(config_text.contains("https://api.deepseek.com"));
    assert!(config_text.contains("gpt-5.4"));
    assert_eq!(
        provider
            .settings_config
            .get("auth")
            .and_then(|auth| auth.get("OPENAI_API_KEY"))
            .and_then(|value| value.as_str()),
        Some("sk-cx")
    );
    assert_eq!(
        provider
            .meta
            .as_ref()
            .and_then(|meta| meta.api_format.as_deref()),
        Some("openai_responses")
    );
}

#[test]
#[serial]
fn add_codex_api_format_override_is_applied() {
    let _guard = lock_test_mutex();
    prepare_empty_state();

    run_add(
        Some("CodexChat"),
        AppType::Codex,
        AddOpts {
            base_url: Some("https://chat".to_string()),
            api_key: Some("sk-chat".to_string()),
            api_format: Some("chat".to_string()),
            ..Default::default()
        },
    )
    .expect("codex add with api-format override should succeed");

    let provider = saved_provider(AppType::Codex, "codexchat");
    assert_eq!(
        provider
            .meta
            .as_ref()
            .and_then(|meta| meta.api_format.as_deref()),
        Some("openai_chat")
    );
}

#[test]
#[serial]
fn add_sponsor_template_inherits_base_url() {
    let _guard = lock_test_mutex();
    prepare_empty_state();

    run_add(
        Some("Packy"),
        AppType::Claude,
        AddOpts {
            template: Some(ProviderAddTemplate::Packycode),
            api_key: Some("sk-packy".to_string()),
            ..Default::default()
        },
    )
    .expect("sponsor template add should succeed");

    let provider = saved_provider(AppType::Claude, "packy");
    assert_eq!(
        env_str(&provider, "ANTHROPIC_BASE_URL"),
        Some("https://www.packyapi.com")
    );
    assert_eq!(env_str(&provider, "ANTHROPIC_AUTH_TOKEN"), Some("sk-packy"));
    assert_eq!(
        provider.meta.as_ref().and_then(|meta| meta.is_partner),
        Some(true)
    );
}

#[test]
#[serial]
fn add_gemini_uses_oauth_without_key_and_api_key_with_key() {
    let _guard = lock_test_mutex();
    prepare_empty_state();

    run_add(Some("GemOAuth"), AppType::Gemini, AddOpts::default())
        .expect("gemini oauth add should succeed");
    let oauth = saved_provider(AppType::Gemini, "gemoauth");
    assert_eq!(env_str(&oauth, "GEMINI_API_KEY"), None);

    run_add(
        Some("GemKey"),
        AppType::Gemini,
        AddOpts {
            api_key: Some("AIza-xyz".to_string()),
            model: Some("gemini-3-pro-preview".to_string()),
            ..Default::default()
        },
    )
    .expect("gemini api-key add should succeed");
    let keyed = saved_provider(AppType::Gemini, "gemkey");
    assert_eq!(env_str(&keyed, "GEMINI_API_KEY"), Some("AIza-xyz"));
    assert_eq!(
        env_str(&keyed, "GEMINI_MODEL"),
        Some("gemini-3-pro-preview")
    );
}

#[test]
#[serial]
fn add_additive_app_requires_raw_config() {
    let _guard = lock_test_mutex();
    prepare_empty_state();

    let err = run_add(
        Some("OcNoConfig"),
        AppType::OpenCode,
        AddOpts {
            base_url: Some("https://x.y".to_string()),
            ..Default::default()
        },
    )
    .expect_err("opencode field-mode add should fail");
    assert!(
        err.to_string().contains("--config"),
        "error should point to --config: {err}"
    );

    run_add(
        Some("OcOk"),
        AppType::OpenCode,
        AddOpts {
            config: Some(
                r#"{"provider":{"ocok":{"npm":"@ai-sdk/openai-compatible","options":{"baseURL":"https://x.y/v1","apiKey":"sk-oc"},"models":{"gpt-4o":{}}}}}"#
                    .to_string(),
            ),
            ..Default::default()
        },
    )
    .expect("opencode raw-config add should succeed");
    let provider = saved_provider(AppType::OpenCode, "ocok");
    assert!(provider.settings_config.get("provider").is_some());
}

#[test]
#[serial]
fn add_config_file_is_read() {
    let _guard = lock_test_mutex();
    let home = ensure_test_home();
    prepare_empty_state();

    let cfg_path = home.join("claude-cfg.json");
    std::fs::write(
        &cfg_path,
        r#"{"env":{"ANTHROPIC_BASE_URL":"https://fromfile","ANTHROPIC_API_KEY":"sk-file"}}"#,
    )
    .expect("write config file");

    run_add(
        Some("FromFile"),
        AppType::Claude,
        AddOpts {
            config_file: Some(cfg_path),
            api_key_field: Some(ClaudeApiKeyFieldArg::ApiKey),
            ..Default::default()
        },
    )
    .expect("config-file add should succeed");

    let provider = saved_provider(AppType::Claude, "fromfile");
    assert_eq!(
        env_str(&provider, "ANTHROPIC_BASE_URL"),
        Some("https://fromfile")
    );
    assert_eq!(env_str(&provider, "ANTHROPIC_API_KEY"), Some("sk-file"));
    assert_eq!(
        provider
            .meta
            .as_ref()
            .and_then(|meta| meta.api_key_field.as_deref()),
        Some("ANTHROPIC_API_KEY")
    );
}

#[test]
#[serial]
fn add_explicit_id_and_duplicate_guard() {
    let _guard = lock_test_mutex();
    prepare_empty_state();

    run_add(
        Some("Explicit"),
        AppType::Claude,
        AddOpts {
            id: Some("custom-id".to_string()),
            base_url: Some("https://e".to_string()),
            api_key: Some("sk-e".to_string()),
            ..Default::default()
        },
    )
    .expect("explicit-id add should succeed");
    assert_eq!(
        saved_provider(AppType::Claude, "custom-id").name,
        "Explicit"
    );

    let err = run_add(
        Some("Again"),
        AppType::Claude,
        AddOpts {
            id: Some("custom-id".to_string()),
            base_url: Some("https://e2".to_string()),
            api_key: Some("sk-e2".to_string()),
            ..Default::default()
        },
    )
    .expect_err("duplicate id should fail");
    assert!(
        err.to_string().to_lowercase().contains("custom-id"),
        "duplicate error should mention the id: {err}"
    );
}

#[test]
#[serial]
fn add_codex_oauth_template_requires_account() {
    let _guard = lock_test_mutex();
    prepare_empty_state();

    let err = run_add(
        Some("CxOauth"),
        AppType::Claude,
        AddOpts {
            template: Some(ProviderAddTemplate::CodexOauth),
            ..Default::default()
        },
    )
    .expect_err("codex-oauth without --account-id should fail");
    assert!(
        err.to_string().contains("--account-id"),
        "error should require --account-id: {err}"
    );
}

#[test]
#[serial]
fn add_deepseek_template_preserves_api_format_without_override() {
    let _guard = lock_test_mutex();
    prepare_empty_state();

    run_add(
        Some("DeepSeek"),
        AppType::Codex,
        AddOpts {
            template: Some(ProviderAddTemplate::Deepseek),
            api_key: Some("sk-ds".to_string()),
            ..Default::default()
        },
    )
    .expect("deepseek template add should succeed");

    // The template seeds openai_chat; omitting --api-format must not reset it
    // to the openai_responses default.
    let provider = saved_provider(AppType::Codex, "deepseek");
    assert_eq!(
        provider
            .meta
            .as_ref()
            .and_then(|meta| meta.api_format.as_deref()),
        Some("openai_chat")
    );
}

#[test]
#[serial]
fn add_invalid_api_format_errors() {
    let _guard = lock_test_mutex();
    prepare_empty_state();

    let err = run_add(
        Some("BadFmt"),
        AppType::Claude,
        AddOpts {
            base_url: Some("https://b".to_string()),
            api_key: Some("sk-b".to_string()),
            api_format: Some("openai-response".to_string()),
            ..Default::default()
        },
    )
    .expect_err("invalid --api-format should fail");
    assert!(
        err.to_string().to_lowercase().contains("api format"),
        "error should reject the invalid format: {err}"
    );
}

#[test]
#[serial]
fn add_official_template_rejects_field_overrides() {
    let _guard = lock_test_mutex();
    prepare_empty_state();

    let err = run_add(
        Some("OfficialOverride"),
        AppType::Codex,
        AddOpts {
            template: Some(ProviderAddTemplate::OpenaiOfficial),
            base_url: Some("https://x".to_string()),
            api_key: Some("sk-x".to_string()),
            ..Default::default()
        },
    )
    .expect_err("official template with field overrides should fail");
    assert!(
        err.to_string().contains("does not accept") || err.to_string().contains("不接受"),
        "error should reject overrides on official templates: {err}"
    );
}
