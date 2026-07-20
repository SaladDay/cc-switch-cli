use clap::{Subcommand, ValueEnum};
use std::{collections::HashSet, path::PathBuf};

use super::{provider_inspect, provider_usage_query};
use crate::app_config::AppType;
use crate::cli::commands::provider_input::{
    build_claude_settings_config_from_prompt, build_codex_settings_config_from_prompt,
    build_gemini_api_key_settings_config, build_gemini_oauth_settings_config,
    build_provider_from_add_template, codex_current_base_url_model,
    common_snippet_has_effective_config, current_timestamp, display_provider_summary,
    generate_provider_id_for_app, prompt_basic_fields, prompt_optional_fields,
    prompt_settings_config, provider_uses_common_config, set_provider_common_config_meta,
    supports_common_config, validate_provider_add_template, validate_provider_id_for_add,
    OptionalFields, ProviderAddTemplate, SettingsConfigPromptResult,
};
use crate::cli::i18n::texts;
use crate::cli::ui::{highlight, info, success, warning};
use crate::error::AppError;
use crate::provider::{AuthBinding, AuthBindingSource, ClaudeApiKeyField, Provider, ProviderMeta};
use crate::services::{AuthService, ManagedAuthAccount, ProviderService};
use crate::store::AppState;
use indexmap::IndexMap;
use inquire::{Confirm, Select};

const AUTH_PROVIDER_CODEX_OAUTH: &str = "codex_oauth";
const CLAUDE_API_FORMAT_ANTHROPIC: &str = "anthropic";
const CLAUDE_API_FORMAT_OPENAI_CHAT: &str = "openai_chat";
const CLAUDE_API_FORMAT_OPENAI_RESPONSES: &str = "openai_responses";
const CLAUDE_API_FORMAT_GEMINI_NATIVE: &str = "gemini_native";
const CLAUDE_API_FORMAT_CHOICES: [&str; 4] = [
    CLAUDE_API_FORMAT_ANTHROPIC,
    CLAUDE_API_FORMAT_OPENAI_CHAT,
    CLAUDE_API_FORMAT_OPENAI_RESPONSES,
    CLAUDE_API_FORMAT_GEMINI_NATIVE,
];
const CODEX_API_FORMAT_CHOICES: [&str; 2] = [
    CLAUDE_API_FORMAT_OPENAI_RESPONSES,
    CLAUDE_API_FORMAT_OPENAI_CHAT,
];

fn is_claude_official_provider(provider: &Provider) -> bool {
    provider
        .category
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("official"))
}

fn is_claude_codex_oauth_provider(provider: &Provider) -> bool {
    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.provider_type.as_deref())
        .is_some_and(|value| value == "codex_oauth")
}

fn normalize_claude_api_format(raw: &str) -> &'static str {
    match raw.trim() {
        CLAUDE_API_FORMAT_OPENAI_CHAT => CLAUDE_API_FORMAT_OPENAI_CHAT,
        CLAUDE_API_FORMAT_OPENAI_RESPONSES => CLAUDE_API_FORMAT_OPENAI_RESPONSES,
        CLAUDE_API_FORMAT_GEMINI_NATIVE => CLAUDE_API_FORMAT_GEMINI_NATIVE,
        _ => CLAUDE_API_FORMAT_ANTHROPIC,
    }
}

fn normalize_codex_api_format(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "chat"
        | "chat_completions"
        | "chat-completions"
        | CLAUDE_API_FORMAT_OPENAI_CHAT
        | "openai-chat"
        | "openai_chat_completions" => CLAUDE_API_FORMAT_OPENAI_CHAT,
        _ => CLAUDE_API_FORMAT_OPENAI_RESPONSES,
    }
}

fn legacy_openrouter_compat_mode_enabled(settings_config: &serde_json::Value) -> bool {
    match settings_config.get("openrouter_compat_mode") {
        Some(serde_json::Value::Bool(value)) => *value,
        Some(serde_json::Value::Number(value)) => value.as_i64().unwrap_or(0) != 0,
        Some(serde_json::Value::String(value)) => {
            let normalized = value.trim().to_ascii_lowercase();
            normalized == "true" || normalized == "1"
        }
        _ => false,
    }
}

fn effective_claude_api_format(provider: &Provider) -> &'static str {
    if is_claude_codex_oauth_provider(provider) {
        return CLAUDE_API_FORMAT_OPENAI_RESPONSES;
    }

    if let Some(api_format) = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.api_format.as_deref())
    {
        return normalize_claude_api_format(api_format);
    }

    if let Some(api_format) = provider
        .settings_config
        .get("api_format")
        .and_then(|value| value.as_str())
    {
        return normalize_claude_api_format(api_format);
    }

    if legacy_openrouter_compat_mode_enabled(&provider.settings_config) {
        CLAUDE_API_FORMAT_OPENAI_CHAT
    } else {
        CLAUDE_API_FORMAT_ANTHROPIC
    }
}

fn effective_codex_api_format(provider: &Provider) -> &'static str {
    if let Some(api_format) = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.api_format.as_deref())
        .or_else(|| {
            provider
                .settings_config
                .get("api_format")
                .and_then(|value| value.as_str())
        })
        .or_else(|| {
            provider
                .settings_config
                .get("apiFormat")
                .and_then(|value| value.as_str())
        })
    {
        return normalize_codex_api_format(api_format);
    }

    if crate::proxy::providers::codex_provider_uses_chat_completions(provider) {
        CLAUDE_API_FORMAT_OPENAI_CHAT
    } else {
        CLAUDE_API_FORMAT_OPENAI_RESPONSES
    }
}

fn provider_meta_is_empty(meta: &ProviderMeta) -> bool {
    serde_json::to_value(meta)
        .ok()
        .and_then(|value| value.as_object().map(|object| object.is_empty()))
        .unwrap_or(false)
}

fn prune_empty_provider_meta(provider: &mut Provider) {
    if provider.meta.as_ref().is_some_and(provider_meta_is_empty) {
        provider.meta = None;
    }
}

fn apply_settings_prompt_result_metadata(
    app_type: &AppType,
    provider: &mut Provider,
    prompt_result: Option<&SettingsConfigPromptResult>,
) {
    if !matches!(app_type, AppType::Claude) {
        return;
    }

    let Some(api_key_field) = prompt_result.and_then(|result| result.claude_api_key_field) else {
        return;
    };

    if is_claude_official_provider(provider) || is_claude_codex_oauth_provider(provider) {
        if let Some(meta) = provider.meta.as_mut() {
            meta.api_key_field = None;
        }
        prune_empty_provider_meta(provider);
        return;
    }

    match api_key_field {
        ClaudeApiKeyField::AuthToken => {
            if let Some(meta) = provider.meta.as_mut() {
                meta.api_key_field = None;
            }
            prune_empty_provider_meta(provider);
        }
        ClaudeApiKeyField::ApiKey => {
            provider
                .meta
                .get_or_insert_with(ProviderMeta::default)
                .api_key_field = Some(api_key_field.as_env_key().to_string());
        }
    }
}

fn strip_claude_api_format_legacy_settings(provider: &mut Provider) {
    let Some(settings_obj) = provider.settings_config.as_object_mut() else {
        return;
    };
    settings_obj.remove("api_format");
    settings_obj.remove("apiFormat");
    settings_obj.remove("openrouter_compat_mode");
}

fn strip_codex_api_format_legacy_settings(provider: &mut Provider) {
    let Some(settings_obj) = provider.settings_config.as_object_mut() else {
        return;
    };
    settings_obj.remove("api_format");
    settings_obj.remove("apiFormat");
}

fn normalize_codex_settings_wire_api(provider: &mut Provider) {
    let Some(config_text) = provider
        .settings_config
        .get("config")
        .and_then(|value| value.as_str())
        .map(str::to_string)
    else {
        return;
    };
    let normalized =
        crate::codex_config::normalize_codex_config_wire_api_to_responses(&config_text);

    if let Some(settings_obj) = provider.settings_config.as_object_mut() {
        settings_obj.insert("config".to_string(), serde_json::Value::String(normalized));
    }
}

fn apply_claude_api_format(provider: &mut Provider, api_format: &str) {
    let api_format = normalize_claude_api_format(api_format);
    if api_format == CLAUDE_API_FORMAT_ANTHROPIC {
        if let Some(meta) = provider.meta.as_mut() {
            meta.api_format = None;
        }
        prune_empty_provider_meta(provider);
    } else {
        provider
            .meta
            .get_or_insert_with(ProviderMeta::default)
            .api_format = Some(api_format.to_string());
    }
    strip_claude_api_format_legacy_settings(provider);
}

fn apply_codex_api_format(provider: &mut Provider, api_format: &str) {
    let api_format = normalize_codex_api_format(api_format);
    provider
        .meta
        .get_or_insert_with(ProviderMeta::default)
        .api_format = Some(api_format.to_string());
    strip_codex_api_format_legacy_settings(provider);
    normalize_codex_settings_wire_api(provider);
}

fn apply_fixed_claude_api_format_if_needed(app_type: &AppType, provider: &mut Provider) -> bool {
    if !matches!(app_type, AppType::Claude) {
        return true;
    }

    if is_claude_codex_oauth_provider(provider) {
        apply_claude_api_format(provider, CLAUDE_API_FORMAT_OPENAI_RESPONSES);
        return true;
    }

    if is_claude_official_provider(provider) {
        apply_claude_api_format(provider, CLAUDE_API_FORMAT_ANTHROPIC);
        return true;
    }

    false
}

fn apply_fixed_codex_api_format_if_needed(app_type: &AppType, provider: &mut Provider) -> bool {
    if !matches!(app_type, AppType::Codex) {
        return true;
    }

    if provider.is_codex_official() {
        if let Some(meta) = provider.meta.as_mut() {
            meta.api_format = None;
            meta.codex_chat_reasoning = None;
        }
        if let Some(settings_obj) = provider.settings_config.as_object_mut() {
            settings_obj.remove("modelCatalog");
        }
        prune_empty_provider_meta(provider);
        strip_codex_api_format_legacy_settings(provider);
        normalize_codex_settings_wire_api(provider);
        return true;
    }

    false
}

fn prompt_api_format(
    choices: &'static [&'static str],
    current: &str,
    value_label: fn(&str) -> &'static str,
    fallback: &'static str,
) -> Result<&'static str, AppError> {
    let default_index = choices
        .iter()
        .position(|api_format| *api_format == current)
        .unwrap_or(0);
    let labels = choices
        .iter()
        .map(|api_format| value_label(api_format).to_string())
        .collect::<Vec<_>>();

    let selected = Select::new(texts::tui_label_claude_api_format(), labels.clone())
        .with_starting_cursor(default_index)
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;
    let selected_index = labels
        .iter()
        .position(|label| label == &selected)
        .unwrap_or(default_index);

    Ok(choices.get(selected_index).copied().unwrap_or(fallback))
}

fn prompt_claude_api_format(provider: &Provider) -> Result<&'static str, AppError> {
    prompt_api_format(
        &CLAUDE_API_FORMAT_CHOICES,
        effective_claude_api_format(provider),
        texts::tui_claude_api_format_value,
        CLAUDE_API_FORMAT_ANTHROPIC,
    )
}

fn prompt_codex_api_format(provider: &Provider) -> Result<&'static str, AppError> {
    prompt_api_format(
        &CODEX_API_FORMAT_CHOICES,
        effective_codex_api_format(provider),
        texts::tui_codex_api_format_value,
        CLAUDE_API_FORMAT_OPENAI_RESPONSES,
    )
}

fn prompt_and_apply_claude_api_format(
    app_type: &AppType,
    provider: &mut Provider,
) -> Result<(), AppError> {
    if apply_fixed_claude_api_format_if_needed(app_type, provider) {
        return Ok(());
    }

    let api_format = prompt_claude_api_format(provider)?;
    apply_claude_api_format(provider, api_format);
    Ok(())
}

fn prompt_and_apply_codex_api_format(
    app_type: &AppType,
    provider: &mut Provider,
) -> Result<(), AppError> {
    if apply_fixed_codex_api_format_if_needed(app_type, provider) {
        return Ok(());
    }

    let api_format = prompt_codex_api_format(provider)?;
    apply_codex_api_format(provider, api_format);
    Ok(())
}

fn prompt_and_apply_provider_api_format(
    app_type: &AppType,
    provider: &mut Provider,
) -> Result<(), AppError> {
    match app_type {
        AppType::Claude => prompt_and_apply_claude_api_format(app_type, provider),
        AppType::Codex => prompt_and_apply_codex_api_format(app_type, provider),
        AppType::Gemini | AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => Ok(()),
    }
}

fn normalize_optional_account_id(account_id: Option<String>) -> Option<String> {
    account_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn apply_codex_oauth_provider_options(
    provider: &mut Provider,
    account_id: Option<String>,
    fast_mode: bool,
) {
    if !provider.settings_config.is_object() {
        provider.settings_config = serde_json::json!({});
    }
    if let Some(settings_obj) = provider.settings_config.as_object_mut() {
        let env_value = settings_obj
            .entry("env".to_string())
            .or_insert_with(|| serde_json::json!({}));
        if !env_value.is_object() {
            *env_value = serde_json::json!({});
        }
        if let Some(env_obj) = env_value.as_object_mut() {
            env_obj.remove("ANTHROPIC_AUTH_TOKEN");
            env_obj.remove("ANTHROPIC_API_KEY");
            env_obj.insert(
                "ANTHROPIC_BASE_URL".to_string(),
                serde_json::json!("https://chatgpt.com/backend-api/codex"),
            );
        }
    }

    let account_id = normalize_optional_account_id(account_id);
    let meta = provider.meta.get_or_insert_with(ProviderMeta::default);
    meta.provider_type = Some(AUTH_PROVIDER_CODEX_OAUTH.to_string());
    meta.api_format = Some(CLAUDE_API_FORMAT_OPENAI_RESPONSES.to_string());
    meta.auth_binding = Some(AuthBinding {
        source: AuthBindingSource::ManagedAccount,
        auth_provider: Some(AUTH_PROVIDER_CODEX_OAUTH.to_string()),
        account_id,
    });
    meta.codex_fast_mode = Some(fast_mode);
}

fn codex_oauth_account_id(provider: &Provider) -> Option<String> {
    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.managed_account_id_for(AUTH_PROVIDER_CODEX_OAUTH))
}

fn load_codex_oauth_accounts() -> Result<Vec<ManagedAuthAccount>, AppError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| AppError::Message(format!("failed to create async runtime: {error}")))?;

    runtime
        .block_on(AuthService::list_accounts(AUTH_PROVIDER_CODEX_OAUTH))
        .map_err(AppError::Message)
}

fn codex_oauth_account_label(account: &ManagedAuthAccount) -> String {
    let suffix = if account.is_default {
        format!(", {}", texts::tui_managed_accounts_default())
    } else {
        String::new()
    };
    format!("{} ({}{suffix})", account.login, account.id)
}

fn prompt_codex_oauth_account(
    current_account_id: Option<&str>,
    accounts: &[ManagedAuthAccount],
) -> Result<Option<String>, AppError> {
    let mut choices = Vec::with_capacity(accounts.len() + 1);
    let mut account_ids = Vec::with_capacity(accounts.len() + 1);
    choices.push(texts::tui_managed_accounts_follow_default().to_string());
    account_ids.push(None);

    let mut default_index = 0;
    for account in accounts {
        if current_account_id == Some(account.id.as_str()) {
            default_index = choices.len();
        }
        choices.push(codex_oauth_account_label(account));
        account_ids.push(Some(account.id.clone()));
    }

    if let Some(account_id) = current_account_id {
        if default_index == 0 {
            default_index = choices.len();
            choices.push(account_id.to_string());
            account_ids.push(Some(account_id.to_string()));
        }
    }

    let selected = Select::new(texts::tui_label_chatgpt_account(), choices.clone())
        .with_starting_cursor(default_index)
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;
    let selected_index = choices
        .iter()
        .position(|choice| choice == &selected)
        .unwrap_or(0);

    Ok(account_ids.get(selected_index).cloned().unwrap_or(None))
}

fn prompt_and_apply_codex_oauth_provider_options(
    app_type: &AppType,
    provider: &mut Provider,
) -> Result<(), AppError> {
    if !matches!(app_type, AppType::Claude) || !is_claude_codex_oauth_provider(provider) {
        return Ok(());
    }

    let current_account_id = codex_oauth_account_id(provider);
    let accounts = load_codex_oauth_accounts()?;
    let account_id = prompt_codex_oauth_account(current_account_id.as_deref(), &accounts)?;
    let fast_mode = Confirm::new(texts::tui_label_codex_fast_mode())
        .with_default(provider.codex_fast_mode_enabled())
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    apply_codex_oauth_provider_options(provider, account_id, fast_mode);
    Ok(())
}

fn prompt_common_config_enabled(
    app_type: &AppType,
    common_snippet: Option<&str>,
    current: Option<&Provider>,
) -> Result<Option<bool>, AppError> {
    if !supports_common_config(app_type)
        || !common_snippet_has_effective_config(app_type, common_snippet)
    {
        return Ok(None);
    }

    let default_enabled = current
        .map(|provider| provider_uses_common_config(app_type, provider, common_snippet))
        .unwrap_or(true);
    let enabled = Confirm::new(texts::tui_form_attach_common_config())
        .with_default(default_enabled)
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;
    Ok(Some(enabled))
}

/// Claude API-key environment variable to populate (`provider add`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ClaudeApiKeyFieldArg {
    /// Use ANTHROPIC_AUTH_TOKEN (default)
    AuthToken,
    /// Use ANTHROPIC_API_KEY
    ApiKey,
}

impl From<ClaudeApiKeyFieldArg> for ClaudeApiKeyField {
    fn from(value: ClaudeApiKeyFieldArg) -> Self {
        match value {
            ClaudeApiKeyFieldArg::AuthToken => ClaudeApiKeyField::AuthToken,
            ClaudeApiKeyFieldArg::ApiKey => ClaudeApiKeyField::ApiKey,
        }
    }
}

#[derive(Subcommand)]
pub enum ProviderCommand {
    /// List all providers
    List,
    /// Show current provider
    Current,
    /// Switch to a provider
    Switch {
        /// Provider ID to switch to
        id: String,
    },
    /// Add a new provider (non-interactive; use the TUI for interactive add)
    Add {
        /// Provider template to apply before creation
        #[arg(long, value_enum)]
        template: Option<ProviderAddTemplate>,
        /// Provider display name (required)
        #[arg(long)]
        name: Option<String>,
        /// Explicit provider ID (default: generated from the name)
        #[arg(long)]
        id: Option<String>,
        /// API endpoint base URL (Claude/Codex/Gemini field mode)
        #[arg(long, conflicts_with_all = ["config", "config_file"])]
        base_url: Option<String>,
        /// API key or token (Claude/Codex/Gemini field mode)
        #[arg(long, conflicts_with_all = ["config", "config_file"])]
        api_key: Option<String>,
        /// Default model (Claude/Codex/Gemini field mode, optional)
        #[arg(long, conflicts_with_all = ["config", "config_file"])]
        model: Option<String>,
        /// Raw settings_config as a JSON string (any app, advanced use)
        #[arg(long, conflicts_with = "config_file")]
        config: Option<String>,
        /// Raw settings_config read from a JSON file (any app, advanced use)
        #[arg(long)]
        config_file: Option<PathBuf>,
        /// Website URL (optional)
        #[arg(long)]
        website_url: Option<String>,
        /// Notes (optional)
        #[arg(long)]
        notes: Option<String>,
        /// Sort index (optional)
        #[arg(long)]
        sort_index: Option<usize>,
        /// Claude API-key field to populate (default: auth-token)
        #[arg(long, value_enum)]
        api_key_field: Option<ClaudeApiKeyFieldArg>,
        /// Provider API format (Claude: anthropic|openai_chat|openai_responses|gemini_native; Codex: responses|chat)
        #[arg(long)]
        api_format: Option<String>,
        /// Attach the app-level common config snippet (Claude/Codex/Gemini)
        #[arg(long)]
        common_config: bool,
        /// Codex OAuth managed account ID (required for the codex-oauth template)
        #[arg(long)]
        account_id: Option<String>,
        /// Enable Codex fast mode (codex-oauth template)
        #[arg(long)]
        fast_mode: bool,
    },
    /// Edit a provider
    Edit {
        /// Provider ID to edit
        id: String,
    },
    /// Delete a provider
    Delete {
        /// Provider ID to delete
        id: String,
    },
    /// Duplicate a provider
    Duplicate {
        /// Provider ID to duplicate
        id: String,
        /// Edit copied provider fields before saving
        #[arg(long)]
        edit: bool,
    },
    /// Import providers from the current live app config
    ImportLive,
    /// Remove a provider from additive live app config without deleting it
    RemoveFromConfig {
        /// Provider ID to remove from live config
        id: String,
    },
    /// Set the default provider/model for apps that support it
    SetDefault {
        /// Provider ID to set as default
        id: String,
        /// OpenClaw model ID to set as primary; defaults to the first live model
        #[arg(long)]
        model: Option<String>,
    },
    /// Test provider endpoint speed
    Speedtest {
        /// Provider ID to test
        id: String,
    },
    /// Run stream health check for a provider
    StreamCheck {
        /// Provider ID to check
        id: String,
    },
    /// Fetch remote model list for a provider
    FetchModels {
        /// Provider ID to query
        #[arg(required_unless_present = "base_url")]
        id: Option<String>,
        /// Base URL to query without saving a provider
        #[arg(long, conflicts_with = "id", required_unless_present = "id")]
        base_url: Option<String>,
        /// API key for the one-off request
        #[arg(long, requires = "base_url")]
        api_key: Option<String>,
        /// Authentication/header strategy for the one-off request
        #[arg(long, value_enum, requires = "base_url")]
        auth: Option<ModelFetchAuthArg>,
    },
    /// Query provider quota or usage
    Quota {
        /// Provider ID to query
        id: String,
        /// Output raw quota result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Configure provider Usage Query
    #[command(subcommand)]
    UsageQuery(provider_usage_query::ProviderUsageQueryCommand),
    /// Export a Claude provider to a standalone settings file
    Export {
        /// Provider ID to export
        id: String,
        /// Output path (default: {cwd}/.claude/settings.local.json)
        /// If path is a directory, appends settings-{provider-name}.json
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

pub fn execute(cmd: ProviderCommand, app: Option<AppType>) -> Result<(), AppError> {
    let app_type = app.unwrap_or(AppType::Claude);

    match cmd {
        ProviderCommand::List => provider_inspect::list_providers(app_type),
        ProviderCommand::Current => provider_inspect::show_current(app_type),
        ProviderCommand::Switch { id } => switch_provider(app_type, &id),
        ProviderCommand::Add {
            template,
            name,
            id,
            base_url,
            api_key,
            model,
            config,
            config_file,
            website_url,
            notes,
            sort_index,
            api_key_field,
            api_format,
            common_config,
            account_id,
            fast_mode,
        } => add_provider(
            app_type,
            AddProviderArgs {
                template,
                name,
                id,
                base_url,
                api_key,
                model,
                config,
                config_file,
                website_url,
                notes,
                sort_index,
                api_key_field: api_key_field.map(ClaudeApiKeyField::from),
                api_format,
                common_config,
                account_id,
                fast_mode,
            },
        ),
        ProviderCommand::Edit { id } => edit_provider(app_type, &id),
        ProviderCommand::Delete { id } => delete_provider(app_type, &id),
        ProviderCommand::Duplicate { id, edit } => duplicate_provider(app_type, &id, edit),
        ProviderCommand::ImportLive => import_live_config(app_type),
        ProviderCommand::RemoveFromConfig { id } => remove_from_config(app_type, &id),
        ProviderCommand::SetDefault { id, model } => {
            set_default_provider(app_type, &id, model.as_deref())
        }
        ProviderCommand::Speedtest { id } => provider_inspect::speedtest_provider(app_type, &id),
        ProviderCommand::StreamCheck { id } => {
            provider_inspect::stream_check_provider(app_type, &id)
        }
        ProviderCommand::FetchModels {
            id,
            base_url,
            api_key,
            auth,
        } => {
            if let Some(id) = id {
                provider_inspect::fetch_models_provider(app_type, &id)
            } else {
                provider_inspect::fetch_models_once(
                    app_type,
                    base_url.as_deref(),
                    api_key.as_deref(),
                    auth.map(Into::into),
                )
            }
        }
        ProviderCommand::Quota { id, json } => {
            provider_inspect::quota_provider(app_type, &id, json)
        }
        ProviderCommand::UsageQuery(cmd) => provider_usage_query::execute(cmd, app_type),
        ProviderCommand::Export { id, output } => export_provider(app_type, &id, output),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ModelFetchAuthArg {
    Bearer,
    Anthropic,
    GoogleApiKey,
}

impl From<ModelFetchAuthArg> for provider_inspect::ProviderModelFetchStrategy {
    fn from(value: ModelFetchAuthArg) -> Self {
        match value {
            ModelFetchAuthArg::Bearer => Self::Bearer,
            ModelFetchAuthArg::Anthropic => Self::Anthropic,
            ModelFetchAuthArg::GoogleApiKey => Self::GoogleApiKey,
        }
    }
}

fn get_state() -> Result<AppState, AppError> {
    AppState::try_new()
}

/// Resolve a switch argument to a real provider id and its provider record.
///
/// An exact id match always wins, so a provider whose name happens to collide
/// with another provider's id never shadows the id lookup. When the id lookup
/// misses, fall back to a case- and whitespace-insensitive `name` match. A
/// single name match resolves to that provider's real id; multiple matches
/// return an ambiguity error listing the candidate ids so the user can pick one.
fn resolve_provider_for_switch(
    providers: &IndexMap<String, Provider>,
    arg: &str,
) -> Result<(String, Provider), AppError> {
    if let Some(provider) = providers.get(arg) {
        return Ok((arg.to_string(), provider.clone()));
    }

    let needle = arg.trim();
    let matches: Vec<(&String, &Provider)> = providers
        .iter()
        .filter(|(_, provider)| provider.name.trim().eq_ignore_ascii_case(needle))
        .collect();

    match matches.as_slice() {
        [] => Err(AppError::Message(format!("Provider '{}' not found", arg))),
        [(id, provider)] => Ok(((*id).clone(), (*provider).clone())),
        multiple => {
            let candidate_ids = multiple
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Err(AppError::Message(format!(
                "Multiple providers named '{}'. Specify one by id: {}",
                arg, candidate_ids
            )))
        }
    }
}

fn switch_provider(app_type: AppType, id: &str) -> Result<(), AppError> {
    let state = get_state()?;
    let app_str = app_type.as_str().to_string();
    let skip_live_sync = !crate::sync_policy::should_sync_live(&app_type);

    // 检查 provider 是否存在（支持按 id 或名称解析）
    let providers = ProviderService::list(&state, app_type.clone())?;
    let (resolved_id, provider) = resolve_provider_for_switch(&providers, id)?;
    let id = resolved_id.as_str();

    // 执行切换（upstream parity：干净写入，无冲突提示）
    ProviderService::switch(&state, app_type.clone(), id)?;
    if let Err(err) =
        crate::claude_plugin::sync_claude_plugin_on_provider_switch(&app_type, &provider)
    {
        println!(
            "{}",
            warning(&texts::claude_plugin_sync_failed_warning(&err.to_string()))
        );
    }

    if app_type.is_additive_mode() {
        println!(
            "{}",
            success(&texts::provider_added_to_app_config(id, &app_str))
        );
    } else {
        println!("{}", success(&texts::switched_to_provider(id)));
    }
    println!("{}", info(&format!("  Application: {}", app_str)));
    if skip_live_sync {
        println!(
            "{}",
            warning(&texts::live_sync_skipped_uninitialized_warning(&app_str))
        );
    }
    println!("\n{}", info(texts::restart_note()));

    Ok(())
}

fn delete_provider(app_type: AppType, id: &str) -> Result<(), AppError> {
    let state = get_state()?;

    // 检查是否是当前 provider
    let current_id = ProviderService::current(&state, app_type.clone())?;
    if id == current_id {
        return Err(AppError::Message(
            "Cannot delete the current active provider. Please switch to another provider first."
                .to_string(),
        ));
    }

    // 确认删除
    let confirm = inquire::Confirm::new(&format!(
        "Are you sure you want to delete provider '{}'?",
        id
    ))
    .with_default(false)
    .prompt()
    .map_err(|e| AppError::Message(format!("Prompt failed: {}", e)))?;

    if !confirm {
        println!("{}", info("Cancelled."));
        return Ok(());
    }

    // 执行删除
    ProviderService::delete(&state, app_type, id)?;

    println!("{}", success(&format!("✓ Deleted provider '{}'", id)));

    Ok(())
}

/// Parsed flags for the non-interactive `provider add`.
struct AddProviderArgs {
    template: Option<ProviderAddTemplate>,
    name: Option<String>,
    id: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
    config: Option<String>,
    config_file: Option<PathBuf>,
    website_url: Option<String>,
    notes: Option<String>,
    sort_index: Option<usize>,
    api_key_field: Option<ClaudeApiKeyField>,
    api_format: Option<String>,
    common_config: bool,
    account_id: Option<String>,
    fast_mode: bool,
}

/// Trim a flag value and drop it when empty.
fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn add_requires_name_error() -> AppError {
    AppError::InvalidInput(if crate::cli::i18n::is_chinese() {
        "provider add 需要 --name 参数；如需交互式添加，请运行 TUI（cc-switch）".to_string()
    } else {
        "provider add requires --name; run the TUI (cc-switch) for interactive add".to_string()
    })
}

fn add_missing_field_error(flag: &str) -> AppError {
    AppError::InvalidInput(if crate::cli::i18n::is_chinese() {
        format!("非交互式 provider add 缺少必填参数 {flag}")
    } else {
        format!("non-interactive provider add is missing required flag {flag}")
    })
}

fn add_additive_requires_config_error(app_type: &AppType) -> AppError {
    AppError::InvalidInput(if crate::cli::i18n::is_chinese() {
        format!(
            "应用 {} 的 provider add 需要通过 --config 或 --config-file 提供原始配置（或使用 TUI）",
            app_type.as_str()
        )
    } else {
        format!(
            "provider add for {} requires raw config via --config or --config-file (or use the TUI)",
            app_type.as_str()
        )
    })
}

fn add_template_rejects_config_error(template: ProviderAddTemplate) -> AppError {
    AppError::InvalidInput(if crate::cli::i18n::is_chinese() {
        format!(
            "模板 '{}' 不接受 --base-url/--api-key/--model/--config 等配置参数",
            template.cli_name()
        )
    } else {
        format!(
            "template '{}' does not accept --base-url/--api-key/--model/--config overrides",
            template.cli_name()
        )
    })
}

fn codex_oauth_requires_account_error() -> AppError {
    AppError::InvalidInput(if crate::cli::i18n::is_chinese() {
        "codex-oauth 模板需要 --account-id（请先运行 `cc-switch auth login`）".to_string()
    } else {
        "the codex-oauth template requires --account-id (run `cc-switch auth login` first)"
            .to_string()
    })
}

fn codex_oauth_account_not_found_error(
    account_id: &str,
    accounts: &[ManagedAuthAccount],
) -> AppError {
    let available = if accounts.is_empty() {
        "(none)".to_string()
    } else {
        accounts
            .iter()
            .map(codex_oauth_account_label)
            .collect::<Vec<_>>()
            .join(", ")
    };
    AppError::InvalidInput(if crate::cli::i18n::is_chinese() {
        format!("未找到 codex-oauth 账号 '{account_id}'。可用账号：{available}")
    } else {
        format!("codex-oauth account '{account_id}' not found. Available accounts: {available}")
    })
}

fn common_config_unavailable_message(app_type: &AppType) -> String {
    if crate::cli::i18n::is_chinese() {
        format!(
            "应用 {} 未配置公共配置片段，已忽略 --common-config",
            app_type.as_str()
        )
    } else {
        format!(
            "no common config snippet configured for {}; --common-config ignored",
            app_type.as_str()
        )
    }
}

/// Read the raw `settings_config` JSON from `--config` or `--config-file`.
fn load_raw_settings_config(args: &AddProviderArgs) -> Result<Option<serde_json::Value>, AppError> {
    let raw = if let Some(text) = args.config.as_deref() {
        text.to_string()
    } else if let Some(path) = args.config_file.as_deref() {
        std::fs::read_to_string(path).map_err(|e| {
            AppError::InvalidInput(if crate::cli::i18n::is_chinese() {
                format!("无法读取配置文件 {}: {e}", path.display())
            } else {
                format!("failed to read config file {}: {e}", path.display())
            })
        })?
    } else {
        return Ok(None);
    };

    let value: serde_json::Value = serde_json::from_str(raw.trim()).map_err(|e| {
        AppError::InvalidInput(if crate::cli::i18n::is_chinese() {
            format!("配置 JSON 解析失败: {e}")
        } else {
            format!("failed to parse config JSON: {e}")
        })
    })?;
    Ok(Some(value))
}

fn resolve_add_provider_id(
    app_type: &AppType,
    explicit: Option<&str>,
    name: &str,
    existing_ids: &[String],
) -> Result<String, AppError> {
    match explicit.map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => {
            validate_provider_id_for_add(app_type, id, existing_ids)?;
            Ok(id.to_string())
        }
        None => Ok(generate_provider_id_for_app(app_type, name, existing_ids)),
    }
}

fn claude_current_base_url(current: Option<&serde_json::Value>) -> Option<String> {
    current
        .and_then(|v| v.get("env"))
        .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
        .and_then(|u| u.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn gemini_current_env(current: Option<&serde_json::Value>, keys: &[&str]) -> Option<String> {
    let env = current.and_then(|v| v.get("env"))?;
    keys.iter().find_map(|key| {
        env.get(*key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    })
}

/// Build a provider's `settings_config` from CLI flags (raw or field mode).
fn build_add_settings_config(
    app_type: &AppType,
    args: &AddProviderArgs,
    raw_config: Option<&serde_json::Value>,
    current: Option<&serde_json::Value>,
    provider_name: &str,
    prompt_result: &mut Option<SettingsConfigPromptResult>,
) -> Result<serde_json::Value, AppError> {
    // Raw config wins for every app type.
    if let Some(raw) = raw_config {
        if matches!(app_type, AppType::Claude) {
            let field = args
                .api_key_field
                .unwrap_or_else(|| ClaudeApiKeyField::from_meta_and_settings(None, raw));
            *prompt_result = Some(SettingsConfigPromptResult {
                settings_config: raw.clone(),
                claude_api_key_field: Some(field),
            });
        }
        return Ok(raw.clone());
    }

    match app_type {
        AppType::Claude => {
            let base_url = non_empty(args.base_url.clone())
                .or_else(|| claude_current_base_url(current))
                .ok_or_else(|| add_missing_field_error("--base-url"))?;
            let api_key = non_empty(args.api_key.clone())
                .ok_or_else(|| add_missing_field_error("--api-key"))?;
            let field = args.api_key_field.unwrap_or(ClaudeApiKeyField::AuthToken);
            let model_fields = vec![("ANTHROPIC_MODEL", non_empty(args.model.clone()))];
            let settings = build_claude_settings_config_from_prompt(
                current,
                field,
                &api_key,
                &base_url,
                model_fields,
                false,
            );
            *prompt_result = Some(SettingsConfigPromptResult {
                settings_config: settings.clone(),
                claude_api_key_field: Some(field),
            });
            Ok(settings)
        }
        AppType::Codex => {
            let (cur_base, cur_model) = codex_current_base_url_model(current);
            let base_url = non_empty(args.base_url.clone())
                .or(cur_base)
                .ok_or_else(|| add_missing_field_error("--base-url"))?;
            let api_key = non_empty(args.api_key.clone())
                .ok_or_else(|| add_missing_field_error("--api-key"))?;
            let model = non_empty(args.model.clone())
                .or(cur_model)
                .unwrap_or_default();
            Ok(build_codex_settings_config_from_prompt(
                current,
                &api_key,
                &base_url,
                model.trim(),
                provider_name,
            ))
        }
        AppType::Gemini => match non_empty(args.api_key.clone()) {
            Some(api_key) => {
                let base_url = non_empty(args.base_url.clone())
                    .or_else(|| {
                        gemini_current_env(current, &["GOOGLE_GEMINI_BASE_URL", "GEMINI_BASE_URL"])
                    })
                    .unwrap_or_default();
                let model = non_empty(args.model.clone())
                    .or_else(|| gemini_current_env(current, &["GEMINI_MODEL"]))
                    .unwrap_or_default();
                Ok(build_gemini_api_key_settings_config(
                    current, &api_key, &base_url, &model,
                ))
            }
            None => Ok(build_gemini_oauth_settings_config(current)),
        },
        AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => {
            Err(add_additive_requires_config_error(app_type))
        }
    }
}

fn add_invalid_api_format_error(raw: &str, choices: &str) -> AppError {
    AppError::InvalidInput(if crate::cli::i18n::is_chinese() {
        format!("无效的 API 格式 '{raw}'。可用值：{choices}")
    } else {
        format!("invalid API format '{raw}'. Valid values: {choices}")
    })
}

fn validate_claude_api_format(raw: &str) -> Result<&'static str, AppError> {
    match raw.trim() {
        CLAUDE_API_FORMAT_ANTHROPIC => Ok(CLAUDE_API_FORMAT_ANTHROPIC),
        CLAUDE_API_FORMAT_OPENAI_CHAT => Ok(CLAUDE_API_FORMAT_OPENAI_CHAT),
        CLAUDE_API_FORMAT_OPENAI_RESPONSES => Ok(CLAUDE_API_FORMAT_OPENAI_RESPONSES),
        CLAUDE_API_FORMAT_GEMINI_NATIVE => Ok(CLAUDE_API_FORMAT_GEMINI_NATIVE),
        other => Err(add_invalid_api_format_error(
            other,
            "anthropic, openai_chat, openai_responses, gemini_native",
        )),
    }
}

fn validate_codex_api_format(raw: &str) -> Result<&'static str, AppError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "chat"
        | "chat_completions"
        | "chat-completions"
        | "openai_chat"
        | "openai-chat"
        | "openai_chat_completions" => Ok(CLAUDE_API_FORMAT_OPENAI_CHAT),
        "responses" | "openai_responses" | "openai-responses" => {
            Ok(CLAUDE_API_FORMAT_OPENAI_RESPONSES)
        }
        other => Err(add_invalid_api_format_error(other, "responses, chat")),
    }
}

/// Apply the provider API format for `provider add`. When `--api-format` is
/// omitted the provider's effective/existing format is preserved (template
/// seeds, raw-config meta) rather than being reset to a hard-coded default.
fn apply_add_provider_api_format(
    app_type: &AppType,
    provider: &mut Provider,
    api_format: Option<&str>,
) -> Result<(), AppError> {
    match app_type {
        AppType::Claude => {
            if apply_fixed_claude_api_format_if_needed(app_type, provider) {
                return Ok(());
            }
            let format = match api_format {
                Some(raw) => validate_claude_api_format(raw)?,
                None => effective_claude_api_format(provider),
            };
            apply_claude_api_format(provider, format);
        }
        AppType::Codex => {
            if apply_fixed_codex_api_format_if_needed(app_type, provider) {
                return Ok(());
            }
            let format = match api_format {
                Some(raw) => validate_codex_api_format(raw)?,
                None => effective_codex_api_format(provider),
            };
            apply_codex_api_format(provider, format);
        }
        AppType::Gemini | AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => {}
    }
    Ok(())
}

/// Apply codex-oauth options non-interactively. Unlike the interactive flow,
/// the managed account must be specified explicitly via `--account-id`.
fn apply_add_codex_oauth_options(
    app_type: &AppType,
    provider: &mut Provider,
    account_id: Option<&str>,
    fast_mode: bool,
) -> Result<(), AppError> {
    if !matches!(app_type, AppType::Claude) || !is_claude_codex_oauth_provider(provider) {
        return Ok(());
    }

    let account_id = account_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(codex_oauth_requires_account_error)?;
    let accounts = load_codex_oauth_accounts()?;
    if !accounts.iter().any(|account| account.id == account_id) {
        return Err(codex_oauth_account_not_found_error(account_id, &accounts));
    }
    apply_codex_oauth_provider_options(provider, Some(account_id.to_string()), fast_mode);
    Ok(())
}

fn add_provider(app_type: AppType, args: AddProviderArgs) -> Result<(), AppError> {
    let name = non_empty(args.name.clone()).ok_or_else(add_requires_name_error)?;

    // 1. 加载配置和状态
    let state = AppState::try_new()?;
    let config = state.config.read().unwrap();
    let manager = config
        .get_manager(&app_type)
        .ok_or_else(|| AppError::Message(texts::app_config_not_found(app_type.as_str())))?;
    let existing_ids: Vec<String> = manager.providers.keys().cloned().collect();
    let common_snippet = config.common_config_snippets.get(&app_type).cloned();
    drop(config);

    let template = args.template.unwrap_or(ProviderAddTemplate::Custom);
    validate_provider_add_template(&app_type, template)?;

    let raw_config = load_raw_settings_config(&args)?;
    let mut settings_prompt_result: Option<SettingsConfigPromptResult> = None;

    // 2. 构造供应商（自定义或基于模板）
    let mut provider = if template.is_custom() {
        let id = resolve_add_provider_id(&app_type, args.id.as_deref(), &name, &existing_ids)?;
        let settings_config = build_add_settings_config(
            &app_type,
            &args,
            raw_config.as_ref(),
            None,
            &name,
            &mut settings_prompt_result,
        )?;
        Provider {
            id,
            name,
            settings_config,
            website_url: non_empty(args.website_url.clone()),
            category: None,
            created_at: Some(current_timestamp()),
            sort_index: None,
            notes: None,
            icon: None,
            icon_color: None,
            meta: None,
            in_failover_queue: false,
        }
    } else {
        let mut provider = build_provider_from_add_template(&app_type, template, &existing_ids)?;
        provider.name = name.clone();
        provider.id = resolve_add_provider_id(&app_type, args.id.as_deref(), &name, &existing_ids)?;
        if let Some(url) = non_empty(args.website_url.clone()) {
            provider.website_url = Some(url);
        }

        let has_field_input = raw_config.is_some()
            || args.base_url.is_some()
            || args.api_key.is_some()
            || args.model.is_some();
        if template.requires_settings_prompt() {
            // Sponsor / third-party templates: fold the CLI field values into the
            // template's prefilled settings_config (base_url is inherited when
            // --base-url is omitted).
            let current = provider.settings_config.clone();
            provider.settings_config = build_add_settings_config(
                &app_type,
                &args,
                raw_config.as_ref(),
                Some(&current),
                &name,
                &mut settings_prompt_result,
            )?;
        } else if has_field_input {
            // Official / OAuth templates are self-contained; rebuilding their
            // settings from field flags would leave inconsistent official
            // metadata behind, so reject the overrides outright.
            return Err(add_template_rejects_config_error(template));
        }
        provider
    };

    // 3. 应用可选字段与共享元数据
    provider.sort_index = args.sort_index;
    provider.notes = non_empty(args.notes.clone());
    apply_settings_prompt_result_metadata(
        &app_type,
        &mut provider,
        settings_prompt_result.as_ref(),
    );
    apply_add_provider_api_format(
        &app_type,
        &mut provider,
        non_empty(args.api_format.clone()).as_deref(),
    )?;
    apply_add_codex_oauth_options(
        &app_type,
        &mut provider,
        args.account_id.as_deref(),
        args.fast_mode,
    )?;

    if supports_common_config(&app_type)
        && common_snippet_has_effective_config(&app_type, common_snippet.as_deref())
    {
        set_provider_common_config_meta(&mut provider, args.common_config);
    } else if args.common_config {
        println!("{}", warning(&common_config_unavailable_message(&app_type)));
    }

    // 4. 显示摘要并写入
    display_provider_summary(&provider, &app_type);
    let provider_id = provider.id.clone();
    ProviderService::add(&state, app_type.clone(), provider)?;

    println!(
        "\n{}",
        success(&texts::entity_added_success(
            texts::entity_provider(),
            &provider_id
        ))
    );

    Ok(())
}

fn edit_provider(app_type: AppType, id: &str) -> Result<(), AppError> {
    // Disable bracketed paste mode to work around inquire dropping paste events
    crate::cli::terminal::disable_bracketed_paste_mode_best_effort();

    println!("{}", highlight(&format!("Edit Provider: {}", id)));
    println!("{}", "=".repeat(50));

    // 1. 加载并验证供应商存在
    let state = AppState::try_new()?;
    let config = state.config.read().unwrap();
    let manager = config
        .get_manager(&app_type)
        .ok_or_else(|| AppError::Message(texts::app_config_not_found(app_type.as_str())))?;
    let original = manager
        .providers
        .get(id)
        .ok_or_else(|| {
            let msg = texts::entity_not_found(texts::entity_provider(), id);
            AppError::localized("provider.not_found", msg.clone(), msg)
        })?
        .clone();
    let is_current = manager.current == id;
    let common_snippet = config.common_config_snippets.get(&app_type).cloned();
    drop(config);

    // 2. 显示当前配置
    println!("\n{}", highlight(texts::current_config_header()));
    display_provider_summary(&original, &app_type);
    println!();

    // 3. 全量编辑各字段（使用当前值作为默认）
    println!("{}", info(texts::edit_fields_instruction()));

    // 调用 prompt_basic_fields 来处理基本字段输入（自动使用 initial_value）
    let (name, website_url) = prompt_basic_fields(Some(&original))?;

    // 4. 询问是否修改配置
    let settings_prompt_result = if Confirm::new(texts::modify_provider_config_prompt())
        .with_default(false)
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    {
        Some(prompt_settings_config(
            &app_type,
            Some(&original.settings_config),
            original.meta.as_ref(),
            matches!(app_type, AppType::Codex) && original.is_codex_official(),
            &name,
        )?)
    } else {
        None
    };
    let settings_config = settings_prompt_result
        .as_ref()
        .map(|result| result.settings_config.clone())
        .unwrap_or_else(|| original.settings_config.clone());

    // 5. 询问是否修改可选字段
    let optional = if Confirm::new(texts::modify_optional_fields_prompt())
        .with_default(false)
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    {
        prompt_optional_fields(Some(&original))?
    } else {
        OptionalFields::from_provider(&original)
    };

    // 6. 构建更新后的 Provider（保留 meta 和 created_at）
    let mut updated = Provider {
        id: id.to_string(),
        name: name.trim().to_string(),
        settings_config,
        website_url,
        category: original.category.clone(),
        created_at: original.created_at,
        sort_index: optional.sort_index,
        notes: optional.notes,
        icon: original.icon.clone(),
        icon_color: original.icon_color.clone(),
        meta: original.meta,                           // 保留元数据
        in_failover_queue: original.in_failover_queue, // 保留故障转移状态
    };
    apply_settings_prompt_result_metadata(&app_type, &mut updated, settings_prompt_result.as_ref());
    prompt_and_apply_provider_api_format(&app_type, &mut updated)?;
    prompt_and_apply_codex_oauth_provider_options(&app_type, &mut updated)?;
    if let Some(enabled) =
        prompt_common_config_enabled(&app_type, common_snippet.as_deref(), Some(&updated))?
    {
        set_provider_common_config_meta(&mut updated, enabled);
    }

    // 7. 显示修改摘要并确认
    println!("\n{}", highlight(texts::updated_config_header()));
    display_provider_summary(&updated, &app_type);
    if !Confirm::new(&texts::confirm_update_entity(texts::entity_provider()))
        .with_default(false)
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    {
        println!("{}", info(texts::cancelled()));
        return Ok(());
    }

    // 8. 调用 Service 层（upstream parity：干净写入，无冲突提示）
    ProviderService::update(&state, app_type.clone(), updated)?;

    // 9. 成功消息
    println!(
        "\n{}",
        success(&texts::entity_updated_success(texts::entity_provider(), id))
    );
    if is_current {
        println!("{}", warning(texts::current_provider_synced_warning()));
    }

    Ok(())
}

fn provider_copy_id(original_id: &str, existing_ids: &[String]) -> String {
    let base_id = format!("{}-copy", original_id.trim());
    if !existing_ids.iter().any(|id| id == &base_id) {
        return base_id;
    }

    let mut counter = 2;
    loop {
        let candidate = format!("{base_id}-{counter}");
        if !existing_ids.iter().any(|id| id == &candidate) {
            return candidate;
        }
        counter += 1;
    }
}

fn existing_provider_ids_for_duplicate(
    app_type: &AppType,
    manager_ids: impl IntoIterator<Item = String>,
) -> Result<Vec<String>, AppError> {
    let mut ids = manager_ids.into_iter().collect::<HashSet<_>>();
    if app_type.is_additive_mode() {
        let live_ids = match app_type {
            AppType::OpenCode => crate::opencode_config::get_providers()?
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>(),
            AppType::Hermes => crate::hermes_config::get_providers()?
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>(),
            AppType::OpenClaw => crate::openclaw_config::get_providers()?
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        ids.extend(live_ids);
    }
    Ok(ids.into_iter().collect())
}

fn provider_duplicate_draft(source: &Provider, existing_ids: &[String]) -> Provider {
    let mut draft = source.clone();
    draft.id = provider_copy_id(&source.id, existing_ids);
    draft.name = format!("{} copy", source.name.trim());
    draft.created_at = None;
    draft.in_failover_queue = false;
    draft
}

fn duplicate_provider(app_type: AppType, id: &str, edit: bool) -> Result<(), AppError> {
    if edit {
        return duplicate_provider_interactive(app_type, id);
    }

    let state = AppState::try_new()?;
    let duplicate = ProviderService::duplicate(&state, app_type, id, None)?;

    println!(
        "{}",
        success(&texts::provider_duplicated_success(id, &duplicate.id))
    );
    Ok(())
}

fn duplicate_provider_interactive(app_type: AppType, id: &str) -> Result<(), AppError> {
    crate::cli::terminal::disable_bracketed_paste_mode_best_effort();

    println!("{}", highlight(&format!("Duplicate Provider: {}", id)));
    println!("{}", "=".repeat(50));

    let state = AppState::try_new()?;
    let config = state.config.read().unwrap();
    let manager = config
        .get_manager(&app_type)
        .ok_or_else(|| AppError::Message(texts::app_config_not_found(app_type.as_str())))?;
    let source = manager
        .providers
        .get(id)
        .ok_or_else(|| {
            let msg = texts::entity_not_found(texts::entity_provider(), id);
            AppError::localized("provider.not_found", msg.clone(), msg)
        })?
        .clone();
    let existing_ids =
        existing_provider_ids_for_duplicate(&app_type, manager.providers.keys().cloned())?;
    let common_snippet = config.common_config_snippets.get(&app_type).cloned();
    drop(config);

    let draft = provider_duplicate_draft(&source, &existing_ids);

    println!("\n{}", highlight(texts::current_config_header()));
    display_provider_summary(&draft, &app_type);
    println!();
    println!("{}", info(texts::edit_fields_instruction()));

    let (name, website_url) = prompt_basic_fields(Some(&draft))?;
    let settings_prompt_result = if Confirm::new(texts::modify_provider_config_prompt())
        .with_default(false)
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    {
        Some(prompt_settings_config(
            &app_type,
            Some(&draft.settings_config),
            draft.meta.as_ref(),
            matches!(app_type, AppType::Codex) && source.is_codex_official(),
            &name,
        )?)
    } else {
        None
    };
    let settings_config = settings_prompt_result
        .as_ref()
        .map(|result| result.settings_config.clone())
        .unwrap_or_else(|| draft.settings_config.clone());

    let optional = if Confirm::new(texts::modify_optional_fields_prompt())
        .with_default(false)
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    {
        prompt_optional_fields(Some(&draft))?
    } else {
        OptionalFields::from_provider(&draft)
    };

    let mut copied = Provider {
        id: draft.id.clone(),
        name: name.trim().to_string(),
        settings_config,
        website_url,
        category: source.category.clone(),
        created_at: None,
        sort_index: optional.sort_index,
        notes: optional.notes,
        icon: source.icon.clone(),
        icon_color: source.icon_color.clone(),
        meta: source.meta.clone(),
        in_failover_queue: false,
    };
    apply_settings_prompt_result_metadata(&app_type, &mut copied, settings_prompt_result.as_ref());
    prompt_and_apply_provider_api_format(&app_type, &mut copied)?;
    prompt_and_apply_codex_oauth_provider_options(&app_type, &mut copied)?;
    if let Some(enabled) =
        prompt_common_config_enabled(&app_type, common_snippet.as_deref(), Some(&copied))?
    {
        set_provider_common_config_meta(&mut copied, enabled);
    }

    println!("\n{}", highlight(texts::updated_config_header()));
    display_provider_summary(&copied, &app_type);
    if !Confirm::new(&texts::confirm_create_entity(texts::entity_provider()))
        .with_default(false)
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    {
        println!("{}", info(texts::cancelled()));
        return Ok(());
    }

    let duplicate = ProviderService::duplicate(&state, app_type, id, Some(copied))?;
    println!(
        "{}",
        success(&texts::provider_duplicated_success(id, &duplicate.id))
    );
    Ok(())
}

fn import_live_config(app_type: AppType) -> Result<(), AppError> {
    let state = get_state()?;
    let imported = ProviderService::import_live_config(&state, app_type.clone())?;
    if imported > 0 {
        println!(
            "{}",
            success(&format!(
                "✓ Imported {imported} provider(s) from {} live config",
                app_type.as_str()
            ))
        );
    } else {
        println!(
            "{}",
            info(&format!(
                "No providers imported from {} live config.",
                app_type.as_str()
            ))
        );
    }
    Ok(())
}

fn remove_from_config(app_type: AppType, id: &str) -> Result<(), AppError> {
    let state = get_state()?;
    ProviderService::remove_from_live_config(&state, app_type.clone(), id)?;
    println!(
        "{}",
        success(&format!(
            "✓ Removed provider '{}' from {} live config",
            id,
            app_type.as_str()
        ))
    );
    Ok(())
}

fn set_default_provider(app_type: AppType, id: &str, model: Option<&str>) -> Result<(), AppError> {
    let state = get_state()?;
    let default = ProviderService::set_default_model(&state, app_type.clone(), id, model)?;
    println!(
        "{}",
        success(&format!(
            "✓ Set '{}' as default for {}",
            default,
            app_type.as_str()
        ))
    );
    Ok(())
}

fn export_provider(app_type: AppType, id: &str, output: Option<PathBuf>) -> Result<(), AppError> {
    if !matches!(app_type, AppType::Claude) {
        return Err(AppError::Message(format!(
            "Provider export currently supports only Claude standalone settings files. Use --app claude (current app: {}).",
            app_type.as_str()
        )));
    }

    let state = get_state()?;

    // Single lock scope: get provider AND common_config_snippet together
    let (provider, common_config_snippet) = {
        let config = state.config.read().map_err(AppError::from)?;
        let manager = config
            .get_manager(&app_type)
            .ok_or_else(|| AppError::Message(texts::app_config_not_found(app_type.as_str())))?;

        let provider = manager
            .providers
            .get(id)
            .ok_or_else(|| {
                let msg = texts::provider_not_found(id);
                AppError::localized("provider.not_found", msg.clone(), msg)
            })?
            .clone();

        (
            provider,
            config.common_config_snippets.get(&app_type).cloned(),
        )
    };

    let apply_common_config = ProviderService::provider_uses_common_config_for_app(
        &app_type,
        &provider,
        common_config_snippet.as_deref(),
    );

    let output_path = match output {
        None => {
            // Default: {cwd}/.claude/settings.local.json (auto-loaded by Claude CLI)
            std::env::current_dir()
                .map_err(|e| AppError::Message(format!("无法获取当前工作目录: {}", e)))?
                .join(".claude")
                .join("settings.local.json")
        }
        Some(path) => {
            // If path looks like a directory (no .json extension), append settings-{name}.json
            let path_str = path.to_string_lossy();
            if path_str.ends_with('/') || path_str.ends_with('\\') || !path_str.ends_with(".json") {
                path.join(format!(
                    "settings-{}.json",
                    crate::config::sanitize_provider_name(&provider.name)
                ))
            } else {
                path
            }
        }
    };

    if output_path.exists() {
        let confirm = Confirm::new(&format!(
            "File '{}' already exists. Overwrite?",
            output_path.display()
        ))
        .with_default(false)
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

        if !confirm {
            println!("{}", info(texts::cancelled()));
            return Ok(());
        }
    }

    let settings_content = ProviderService::build_live_backup_snapshot(
        &app_type,
        &provider,
        common_config_snippet.as_deref(),
        apply_common_config,
    )?;

    crate::config::write_json_file(&output_path, &settings_content)?;

    println!(
        "{}",
        success(&format!(
            "✓ Exported provider '{}' to {}",
            id,
            output_path.display()
        ))
    );

    // If output is settings.local.json, Claude CLI will auto-load it
    if output_path
        .file_name()
        .map(|n| n.to_string_lossy() == "settings.local.json")
        .unwrap_or(false)
    {
        println!(
            "{}",
            info("Claude CLI will auto-load this config. Just run: claude")
        );
    } else {
        println!(
            "{}",
            info(&format!(
                "Use it with: claude --settings {}",
                output_path.display()
            ))
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn claude_provider(settings_config: serde_json::Value) -> Provider {
        Provider::with_id(
            "provider-1".to_string(),
            "Provider One".to_string(),
            settings_config,
            None,
        )
    }

    fn codex_provider(settings_config: serde_json::Value) -> Provider {
        Provider::with_id(
            "codex-provider".to_string(),
            "Codex Provider".to_string(),
            settings_config,
            None,
        )
    }

    #[test]
    fn claude_api_format_effective_value_prefers_meta_over_legacy_settings() {
        let mut provider = claude_provider(json!({
            "api_format": "openai_chat",
            "openrouter_compat_mode": true
        }));
        provider.meta = Some(ProviderMeta {
            api_format: Some("openai_responses".to_string()),
            ..Default::default()
        });

        assert_eq!(
            effective_claude_api_format(&provider),
            CLAUDE_API_FORMAT_OPENAI_RESPONSES
        );
    }

    #[test]
    fn claude_api_format_effective_value_preserves_gemini_native_meta() {
        let mut provider = claude_provider(json!({
            "api_format": "openai_chat"
        }));
        provider.meta = Some(ProviderMeta {
            api_format: Some(CLAUDE_API_FORMAT_GEMINI_NATIVE.to_string()),
            ..Default::default()
        });

        assert_eq!(
            effective_claude_api_format(&provider),
            CLAUDE_API_FORMAT_GEMINI_NATIVE
        );
    }

    #[test]
    fn claude_api_format_effective_value_reads_legacy_openrouter_flag() {
        let provider = claude_provider(json!({
            "openrouter_compat_mode": "true"
        }));

        assert_eq!(
            effective_claude_api_format(&provider),
            CLAUDE_API_FORMAT_OPENAI_CHAT
        );
    }

    #[test]
    fn claude_api_format_apply_writes_canonical_meta_and_removes_legacy_settings() {
        let mut provider = claude_provider(json!({
            "api_format": "anthropic",
            "apiFormat": "openai_chat",
            "openrouter_compat_mode": true,
            "env": {
                "ANTHROPIC_BASE_URL": "https://example.com"
            }
        }));

        apply_claude_api_format(&mut provider, CLAUDE_API_FORMAT_OPENAI_CHAT);

        assert_eq!(
            provider
                .meta
                .as_ref()
                .and_then(|meta| meta.api_format.as_deref()),
            Some(CLAUDE_API_FORMAT_OPENAI_CHAT)
        );
        assert!(provider.settings_config.get("api_format").is_none());
        assert!(provider.settings_config.get("apiFormat").is_none());
        assert!(provider
            .settings_config
            .get("openrouter_compat_mode")
            .is_none());
        assert_eq!(
            provider.settings_config["env"]["ANTHROPIC_BASE_URL"],
            "https://example.com"
        );
    }

    #[test]
    fn claude_api_key_field_prompt_metadata_writes_non_default_only() {
        let mut provider = claude_provider(json!({
            "env": {
                "ANTHROPIC_API_KEY": "sk-api-key"
            }
        }));
        provider.meta = Some(ProviderMeta {
            apply_common_config: Some(false),
            ..Default::default()
        });
        let prompt_result = SettingsConfigPromptResult {
            settings_config: provider.settings_config.clone(),
            claude_api_key_field: Some(ClaudeApiKeyField::ApiKey),
        };

        apply_settings_prompt_result_metadata(
            &AppType::Claude,
            &mut provider,
            Some(&prompt_result),
        );

        let meta = provider.meta.expect("metadata should be preserved");
        assert_eq!(meta.apply_common_config, Some(false));
        assert_eq!(meta.api_key_field.as_deref(), Some("ANTHROPIC_API_KEY"));

        let mut provider = claude_provider(json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "sk-auth-token"
            }
        }));
        provider.meta = Some(ProviderMeta {
            apply_common_config: Some(true),
            api_key_field: Some("ANTHROPIC_API_KEY".to_string()),
            ..Default::default()
        });
        let prompt_result = SettingsConfigPromptResult {
            settings_config: provider.settings_config.clone(),
            claude_api_key_field: Some(ClaudeApiKeyField::AuthToken),
        };

        apply_settings_prompt_result_metadata(
            &AppType::Claude,
            &mut provider,
            Some(&prompt_result),
        );

        let meta = provider.meta.expect("non-auth metadata should remain");
        assert_eq!(meta.apply_common_config, Some(true));
        assert_eq!(
            meta.api_key_field, None,
            "upstream omits meta.apiKeyField for the default auth-token field"
        );
    }

    #[test]
    fn claude_api_format_apply_writes_gemini_native_meta() {
        let mut provider = claude_provider(json!({
            "api_format": "openai_chat",
            "apiFormat": "openai_chat",
            "openrouter_compat_mode": true,
        }));

        apply_claude_api_format(&mut provider, CLAUDE_API_FORMAT_GEMINI_NATIVE);

        assert_eq!(
            provider
                .meta
                .as_ref()
                .and_then(|meta| meta.api_format.as_deref()),
            Some(CLAUDE_API_FORMAT_GEMINI_NATIVE)
        );
        assert!(provider.settings_config.get("api_format").is_none());
        assert!(provider.settings_config.get("apiFormat").is_none());
        assert!(provider
            .settings_config
            .get("openrouter_compat_mode")
            .is_none());
    }

    #[test]
    fn claude_api_format_apply_anthropic_removes_only_api_format_meta() {
        let mut provider = claude_provider(json!({}));
        provider.meta = Some(ProviderMeta {
            apply_common_config: Some(false),
            api_format: Some(CLAUDE_API_FORMAT_OPENAI_RESPONSES.to_string()),
            ..Default::default()
        });

        apply_claude_api_format(&mut provider, CLAUDE_API_FORMAT_ANTHROPIC);

        let meta = provider.meta.expect("other metadata should be preserved");
        assert_eq!(meta.apply_common_config, Some(false));
        assert_eq!(meta.api_format, None);
    }

    #[test]
    fn claude_api_format_apply_anthropic_prunes_empty_meta() {
        let mut provider = claude_provider(json!({}));
        provider.meta = Some(ProviderMeta {
            api_format: Some(CLAUDE_API_FORMAT_OPENAI_CHAT.to_string()),
            ..Default::default()
        });

        apply_claude_api_format(&mut provider, CLAUDE_API_FORMAT_ANTHROPIC);

        assert!(provider.meta.is_none());
    }

    #[test]
    fn claude_api_format_fixed_provider_handling_skips_official_and_clears_meta() {
        let mut provider = claude_provider(json!({
            "api_format": "openai_chat"
        }));
        provider.category = Some("official".to_string());
        provider.meta = Some(ProviderMeta {
            api_format: Some(CLAUDE_API_FORMAT_OPENAI_CHAT.to_string()),
            ..Default::default()
        });

        assert!(apply_fixed_claude_api_format_if_needed(
            &AppType::Claude,
            &mut provider
        ));
        assert!(provider.meta.is_none());
        assert!(provider.settings_config.get("api_format").is_none());
    }

    #[test]
    fn claude_api_format_fixed_provider_handling_forces_codex_oauth_responses() {
        let mut provider = claude_provider(json!({
            "openrouter_compat_mode": true
        }));
        provider.meta = Some(ProviderMeta {
            provider_type: Some("codex_oauth".to_string()),
            ..Default::default()
        });

        assert!(apply_fixed_claude_api_format_if_needed(
            &AppType::Claude,
            &mut provider
        ));
        assert_eq!(
            provider
                .meta
                .as_ref()
                .and_then(|meta| meta.api_format.as_deref()),
            Some(CLAUDE_API_FORMAT_OPENAI_RESPONSES)
        );
        assert!(provider
            .settings_config
            .get("openrouter_compat_mode")
            .is_none());
    }

    #[test]
    fn codex_api_format_effective_value_prefers_meta_over_legacy_settings() {
        let mut provider = codex_provider(json!({
            "api_format": "openai_chat",
            "apiFormat": "chat"
        }));
        provider.meta = Some(ProviderMeta {
            api_format: Some(CLAUDE_API_FORMAT_OPENAI_RESPONSES.to_string()),
            ..Default::default()
        });

        assert_eq!(
            effective_codex_api_format(&provider),
            CLAUDE_API_FORMAT_OPENAI_RESPONSES
        );
    }

    #[test]
    fn codex_api_format_effective_value_reads_legacy_chat_wire_api() {
        let provider = codex_provider(json!({
            "config": r#"model_provider = "vendor"

[model_providers.vendor]
base_url = "https://vendor.example/v1"
wire_api = "chat"
"#
        }));

        assert_eq!(
            effective_codex_api_format(&provider),
            CLAUDE_API_FORMAT_OPENAI_CHAT
        );
    }

    #[test]
    fn codex_api_format_apply_writes_meta_and_normalizes_legacy_config() {
        let mut provider = codex_provider(json!({
            "api_format": "openai_responses",
            "apiFormat": "openai_responses",
            "config": r#"model_provider = "vendor"
wire_api = "chat"

[model_providers.vendor]
base_url = "https://vendor.example/v1"
wire_api = "chat"
"#
        }));

        apply_codex_api_format(&mut provider, CLAUDE_API_FORMAT_OPENAI_CHAT);

        assert_eq!(
            provider
                .meta
                .as_ref()
                .and_then(|meta| meta.api_format.as_deref()),
            Some(CLAUDE_API_FORMAT_OPENAI_CHAT)
        );
        assert!(provider.settings_config.get("api_format").is_none());
        assert!(provider.settings_config.get("apiFormat").is_none());
        let config_text = provider
            .settings_config
            .get("config")
            .and_then(serde_json::Value::as_str)
            .expect("config should remain a string");
        assert!(config_text.contains("wire_api = \"responses\""));
        assert!(
            !config_text.contains("wire_api = \"chat\""),
            "CLI should persist upstream Codex wire_api semantics"
        );
    }

    #[test]
    fn codex_api_format_fixed_provider_clears_overrides_and_normalizes_config() {
        let mut provider = codex_provider(json!({
            "api_format": "openai_chat",
            "apiFormat": "chat",
            "config": r#"model_provider = "openai"

[model_providers.openai]
base_url = "https://api.openai.com/v1"
wire_api = "chat"
"#,
            "modelCatalog": {
                "models": [
                    { "model": "stale-chat-model" }
                ]
            }
        }));
        provider.category = Some("official".to_string());
        provider.meta = Some(ProviderMeta {
            api_format: Some(CLAUDE_API_FORMAT_OPENAI_CHAT.to_string()),
            codex_chat_reasoning: Some(crate::provider::CodexChatReasoningConfig {
                supports_thinking: Some(true),
                supports_effort: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        });

        assert!(apply_fixed_codex_api_format_if_needed(
            &AppType::Codex,
            &mut provider
        ));
        assert!(provider.meta.is_none());
        assert!(provider.settings_config.get("api_format").is_none());
        assert!(provider.settings_config.get("apiFormat").is_none());
        assert!(provider.settings_config.get("modelCatalog").is_none());
        let config_text = provider
            .settings_config
            .get("config")
            .and_then(serde_json::Value::as_str)
            .expect("config should remain a string");
        assert!(config_text.contains("wire_api = \"responses\""));
        assert!(!config_text.contains("wire_api = \"chat\""));
    }

    #[test]
    fn codex_oauth_provider_options_write_upstream_managed_account_shape() {
        let mut provider = claude_provider(json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "stale-token",
                "ANTHROPIC_API_KEY": "stale-key",
                "ANTHROPIC_BASE_URL": "https://stale.example",
                "ANTHROPIC_MODEL": "gpt-5.4"
            }
        }));
        provider.meta = Some(ProviderMeta {
            provider_type: Some("codex_oauth".to_string()),
            apply_common_config: Some(true),
            ..Default::default()
        });

        apply_codex_oauth_provider_options(&mut provider, Some(" acc-123 ".to_string()), true);

        let meta = provider.meta.expect("metadata should be present");
        assert_eq!(meta.apply_common_config, Some(true));
        assert_eq!(meta.provider_type.as_deref(), Some("codex_oauth"));
        assert_eq!(
            meta.api_format.as_deref(),
            Some(CLAUDE_API_FORMAT_OPENAI_RESPONSES)
        );
        assert_eq!(meta.codex_fast_mode, Some(true));
        let binding = meta.auth_binding.expect("auth binding should be present");
        assert_eq!(binding.source, AuthBindingSource::ManagedAccount);
        assert_eq!(binding.auth_provider.as_deref(), Some("codex_oauth"));
        assert_eq!(binding.account_id.as_deref(), Some("acc-123"));

        let env = provider
            .settings_config
            .get("env")
            .and_then(serde_json::Value::as_object)
            .expect("env should remain an object");
        assert!(env.get("ANTHROPIC_AUTH_TOKEN").is_none());
        assert!(env.get("ANTHROPIC_API_KEY").is_none());
        assert_eq!(
            env.get("ANTHROPIC_BASE_URL")
                .and_then(serde_json::Value::as_str),
            Some("https://chatgpt.com/backend-api/codex")
        );
        assert_eq!(
            env.get("ANTHROPIC_MODEL")
                .and_then(serde_json::Value::as_str),
            Some("gpt-5.4")
        );
    }

    #[test]
    fn codex_oauth_provider_options_blank_account_follows_default() {
        let mut provider = claude_provider(json!({}));
        provider.meta = Some(ProviderMeta {
            provider_type: Some("codex_oauth".to_string()),
            auth_binding: Some(AuthBinding {
                source: AuthBindingSource::ManagedAccount,
                auth_provider: Some("codex_oauth".to_string()),
                account_id: Some("old-account".to_string()),
            }),
            codex_fast_mode: Some(true),
            ..Default::default()
        });

        apply_codex_oauth_provider_options(&mut provider, Some(" \n ".to_string()), false);

        let meta = provider.meta.expect("metadata should be present");
        assert_eq!(meta.provider_type.as_deref(), Some("codex_oauth"));
        assert_eq!(
            meta.api_format.as_deref(),
            Some(CLAUDE_API_FORMAT_OPENAI_RESPONSES)
        );
        assert_eq!(meta.codex_fast_mode, Some(false));
        let binding = meta.auth_binding.expect("auth binding should be present");
        assert_eq!(binding.source, AuthBindingSource::ManagedAccount);
        assert_eq!(binding.auth_provider.as_deref(), Some("codex_oauth"));
        assert!(
            binding.account_id.is_none(),
            "default-account binding should omit accountId"
        );
    }

    #[test]
    fn duplicate_draft_matches_tui_copy_identity_defaults() {
        let mut provider = claude_provider(json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "sk-demo"
            }
        }));
        provider.created_at = Some(123);
        provider.in_failover_queue = true;
        provider.sort_index = Some(7);

        let draft = provider_duplicate_draft(
            &provider,
            &["provider-1".to_string(), "provider-1-copy".to_string()],
        );

        assert_eq!(draft.id, "provider-1-copy-2");
        assert_eq!(draft.name, "Provider One copy");
        assert_eq!(draft.created_at, None);
        assert!(!draft.in_failover_queue);
        assert_eq!(draft.sort_index, Some(7));
        assert_eq!(
            draft.settings_config["env"]["ANTHROPIC_AUTH_TOKEN"],
            "sk-demo"
        );
    }
}
