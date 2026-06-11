use clap::{Subcommand, ValueEnum};
use std::{collections::HashSet, path::PathBuf};

use super::{provider_inspect, provider_usage_query};
use crate::app_config::AppType;
use crate::cli::commands::provider_input::{
    build_provider_from_add_template, common_snippet_has_effective_config, current_timestamp,
    display_provider_summary, prompt_basic_fields, prompt_optional_fields,
    prompt_provider_id_for_add, prompt_settings_config, prompt_settings_config_for_add,
    provider_add_template_choices, provider_uses_common_config, set_provider_common_config_meta,
    supports_common_config, validate_provider_add_template, OptionalFields, ProviderAddTemplate,
    SettingsConfigPromptResult,
};
use crate::cli::i18n::texts;
use crate::cli::ui::{highlight, info, success, warning};
use crate::error::AppError;
use crate::provider::{AuthBinding, AuthBindingSource, ClaudeApiKeyField, Provider, ProviderMeta};
use crate::services::{AuthService, ManagedAuthAccount, ProviderService};
use crate::store::AppState;
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
    /// Add a new provider
    Add {
        /// Provider template to apply before creation
        #[arg(long, value_enum)]
        template: Option<ProviderAddTemplate>,
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
        ProviderCommand::Add { template } => add_provider(app_type, template),
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

fn switch_provider(app_type: AppType, id: &str) -> Result<(), AppError> {
    let state = get_state()?;
    let app_str = app_type.as_str().to_string();
    let skip_live_sync = !crate::sync_policy::should_sync_live(&app_type);

    // 检查 provider 是否存在
    let providers = ProviderService::list(&state, app_type.clone())?;
    let Some(provider) = providers.get(id).cloned() else {
        return Err(AppError::Message(format!("Provider '{}' not found", id)));
    };

    // 执行切换
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

fn prompt_provider_add_template(
    app_type: &AppType,
) -> Result<Option<ProviderAddTemplate>, AppError> {
    let choices = provider_add_template_choices(app_type);
    if choices.is_empty() {
        return Ok(Some(ProviderAddTemplate::Custom));
    }

    let labels = choices
        .iter()
        .map(|choice| choice.label.to_string())
        .collect::<Vec<_>>();
    match Select::new(texts::select_provider_add_mode(), labels.clone()).prompt() {
        Ok(selected) => {
            let template = choices
                .iter()
                .find(|choice| choice.label == selected)
                .map(|choice| choice.template)
                .unwrap_or(ProviderAddTemplate::Custom);
            Ok(Some(template))
        }
        Err(inquire::error::InquireError::OperationCanceled)
        | Err(inquire::error::InquireError::OperationInterrupted) => {
            println!("{}", info(texts::cancelled()));
            Ok(None)
        }
        Err(e) => Err(AppError::Message(texts::input_failed_error(&e.to_string()))),
    }
}

fn add_provider(app_type: AppType, template: Option<ProviderAddTemplate>) -> Result<(), AppError> {
    // Disable bracketed paste mode to work around inquire dropping paste events
    crate::cli::terminal::disable_bracketed_paste_mode_best_effort();

    println!("{}", highlight("Add New Provider"));
    println!("{}", "=".repeat(50));

    // 1. 加载配置和状态
    let state = AppState::try_new()?;
    let config = state.config.read().unwrap();
    let manager = config
        .get_manager(&app_type)
        .ok_or_else(|| AppError::Message(texts::app_config_not_found(app_type.as_str())))?;
    let existing_ids: Vec<String> = manager.providers.keys().cloned().collect();
    let common_snippet = config.common_config_snippets.get(&app_type).cloned();
    drop(config);

    let template = match template {
        Some(template) => {
            validate_provider_add_template(&app_type, template)?;
            template
        }
        None => match prompt_provider_add_template(&app_type)? {
            Some(template) => template,
            None => return Ok(()),
        },
    };

    // 2. 收集基本字段
    let mut settings_prompt_result = None;
    let mut provider = if template.is_custom() {
        let (name, website_url) = prompt_basic_fields(None)?;
        let id = prompt_provider_id_for_add(&app_type, &name, &existing_ids)?;
        println!("{}", info(&texts::generated_id_message(&id)));

        let prompt_result = prompt_settings_config_for_add(&app_type)?;
        let settings_config = prompt_result.settings_config.clone();
        settings_prompt_result = Some(prompt_result);
        Provider {
            id,
            name,
            settings_config,
            website_url,
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
        if matches!(app_type, AppType::Hermes | AppType::OpenClaw) {
            provider.id = prompt_provider_id_for_add(&app_type, &provider.name, &existing_ids)?;
        }
        if template.requires_settings_prompt() {
            let prompt_result = prompt_settings_config(
                &app_type,
                Some(&provider.settings_config),
                provider.meta.as_ref(),
                provider.is_codex_official(),
            )?;
            provider.settings_config = prompt_result.settings_config.clone();
            settings_prompt_result = Some(prompt_result);
        }
        println!("{}", info(&texts::generated_id_message(&provider.id)));
        provider
    };

    // 4. 询问是否配置可选字段
    let optional = if Confirm::new(texts::configure_optional_fields_prompt())
        .with_default(false)
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    {
        prompt_optional_fields(None)?
    } else {
        OptionalFields::default()
    };

    // 5. 应用可选字段与共享元数据
    provider.sort_index = optional.sort_index;
    provider.notes = optional.notes;
    apply_settings_prompt_result_metadata(
        &app_type,
        &mut provider,
        settings_prompt_result.as_ref(),
    );
    prompt_and_apply_provider_api_format(&app_type, &mut provider)?;
    prompt_and_apply_codex_oauth_provider_options(&app_type, &mut provider)?;
    if let Some(enabled) = prompt_common_config_enabled(&app_type, common_snippet.as_deref(), None)?
    {
        set_provider_common_config_meta(&mut provider, enabled);
    }

    // 6. 显示摘要并确认
    display_provider_summary(&provider, &app_type);
    if !Confirm::new(&texts::confirm_create_entity(texts::entity_provider()))
        .with_default(false)
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    {
        println!("{}", info(texts::cancelled()));
        return Ok(());
    }

    // 7. 调用 Service 层
    let provider_id = provider.id.clone();
    ProviderService::add(&state, app_type.clone(), provider)?;

    // 8. 成功消息
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

    // 8. 调用 Service 层
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
