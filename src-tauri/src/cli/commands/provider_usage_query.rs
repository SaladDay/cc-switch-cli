use clap::{Args, Subcommand, ValueEnum};
use std::fmt;

use crate::app_config::AppType;
use crate::cli::ui::{info, success};
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta, UsageScript};
use crate::services::ProviderService;
use crate::store::AppState;

const DEFAULT_USAGE_LANGUAGE: &str = "javascript";
const DEFAULT_USAGE_TIMEOUT: u64 = 10;
const DEFAULT_USAGE_AUTO_QUERY_INTERVAL: u64 = 5;
const MAX_USAGE_AUTO_QUERY_INTERVAL: u64 = 1440;
const CODEX_OAUTH_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const DEFAULT_USAGE_GENERAL_PRESET: &str = r#"({
  request: {
    url: "{{baseUrl}}/user/balance",
    method: "GET",
    headers: {
      "Authorization": "Bearer {{apiKey}}",
      "User-Agent": "cc-switch/1.0"
    }
  },
  extractor: function(response) {
    return {
      isValid: response.is_active || true,
      remaining: response.balance,
      unit: "USD"
    };
  }
})"#;
const DEFAULT_USAGE_CUSTOM_PRESET: &str = r#"({
  request: {
    url: "",
    method: "GET",
    headers: {}
  },
  extractor: function(response) {
    return {
      remaining: 0,
      unit: "USD"
    };
  }
})"#;
const DEFAULT_USAGE_NEWAPI_PRESET: &str = r#"({
  request: {
    url: "{{baseUrl}}/api/user/self",
    method: "GET",
    headers: {
      "Content-Type": "application/json",
      "Authorization": "Bearer {{accessToken}}",
      "User-Agent": "cc-switch/1.0",
      "New-Api-User": "{{userId}}"
    },
  },
  extractor: function (response) {
    if (response.success && response.data) {
      return {
        planName: response.data.group || "Default Plan",
        remaining: response.data.quota / 500000,
        used: response.data.used_quota / 500000,
        total: (response.data.quota + response.data.used_quota) / 500000,
        unit: "USD",
      };
    }
    return {
      isValid: false,
      invalidMessage: response.message || "Query failed"
    };
  },
})"#;

#[derive(Subcommand)]
pub enum ProviderUsageQueryCommand {
    /// Show a provider Usage Query configuration
    Show {
        /// Provider ID to inspect
        id: String,
        /// Output raw Usage Query configuration as JSON
        #[arg(long)]
        json: bool,
    },
    /// Set a provider Usage Query configuration
    Set(ProviderUsageQuerySetCommand),
    /// Clear a provider Usage Query configuration
    Clear {
        /// Provider ID to update
        id: String,
    },
}

#[derive(Args)]
pub struct ProviderUsageQuerySetCommand {
    /// Provider ID to update
    pub id: String,
    /// Enable Usage Query
    #[arg(long, conflicts_with = "disabled")]
    pub enabled: bool,
    /// Disable Usage Query while keeping the configuration
    #[arg(long, conflicts_with = "enabled")]
    pub disabled: bool,
    /// Usage Query template exposed by the TUI
    #[arg(long, value_enum)]
    pub template: Option<UsageQueryTemplate>,
    /// JavaScript code for the Usage Query request/extractor
    #[arg(long)]
    pub code: Option<String>,
    /// Script timeout in seconds
    #[arg(long)]
    pub timeout: Option<u64>,
    /// Automatic query interval in minutes; 0 disables automatic querying
    #[arg(long, value_name = "MINUTES")]
    pub auto_query_interval: Option<u64>,
    /// API key used by the general template
    #[arg(long)]
    pub api_key: Option<String>,
    /// Base URL used by the general and newapi templates
    #[arg(long)]
    pub base_url: Option<String>,
    /// Access token used by the newapi template
    #[arg(long)]
    pub access_token: Option<String>,
    /// User ID used by the newapi template
    #[arg(long)]
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum UsageQueryTemplate {
    Custom,
    General,
    Newapi,
    Balance,
}

impl UsageQueryTemplate {
    fn as_str(self) -> &'static str {
        match self {
            Self::Custom => "custom",
            Self::General => "general",
            Self::Newapi => "newapi",
            Self::Balance => "balance",
        }
    }
}

impl fmt::Display for UsageQueryTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub fn execute(cmd: ProviderUsageQueryCommand, app_type: AppType) -> Result<(), AppError> {
    match cmd {
        ProviderUsageQueryCommand::Show { id, json } => show(app_type, &id, json),
        ProviderUsageQueryCommand::Set(command) => set(app_type, command),
        ProviderUsageQueryCommand::Clear { id } => clear(app_type, &id),
    }
}

fn get_state() -> Result<AppState, AppError> {
    AppState::try_new()
}

fn show(app_type: AppType, id: &str, json: bool) -> Result<(), AppError> {
    let state = get_state()?;
    let provider = find_provider(&state, &app_type, id)?;
    let usage_script = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.usage_script.as_ref());

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&usage_script)
                .map_err(|error| AppError::Message(error.to_string()))?
        );
        return Ok(());
    }

    let Some(script) = usage_script else {
        println!("{}", info("Usage Query: not configured"));
        return Ok(());
    };

    println!("Usage Query");
    println!("  Provider: {id}");
    println!("  Enabled: {}", script.enabled);
    println!("  Language: {}", script.language);
    println!("  Template: {}", effective_template_type(script, &provider));
    println!(
        "  Timeout: {}",
        script.timeout.unwrap_or(DEFAULT_USAGE_TIMEOUT)
    );
    println!(
        "  Auto Query Interval: {}",
        script
            .auto_query_interval
            .unwrap_or(DEFAULT_USAGE_AUTO_QUERY_INTERVAL)
    );
    print_optional_masked("API Key", script.api_key.as_deref());
    print_optional("Base URL", script.base_url.as_deref());
    print_optional_masked("Access Token", script.access_token.as_deref());
    print_optional("User ID", script.user_id.as_deref());
    print_optional(
        "Coding Plan Provider",
        script.coding_plan_provider.as_deref(),
    );
    println!(
        "  Code: {}",
        if script.code.is_empty() {
            "<empty>"
        } else {
            "set"
        }
    );

    Ok(())
}

fn set(app_type: AppType, command: ProviderUsageQuerySetCommand) -> Result<(), AppError> {
    let state = get_state()?;
    let mut provider = find_provider(&state, &app_type, &command.id)?;
    let existing = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.usage_script.as_ref())
        .cloned();
    let existing_template = existing
        .as_ref()
        .map(|script| effective_template_type(script, &provider));
    let mut script = existing.unwrap_or_else(|| default_usage_script_for_provider(&provider));

    if command.enabled {
        script.enabled = true;
    } else if command.disabled {
        script.enabled = false;
    }

    let explicit_template = command.template;
    let template = explicit_template
        .map(|template| template.as_str().to_string())
        .or(existing_template)
        .unwrap_or_else(|| {
            default_usage_template_for_provider(&provider)
                .as_str()
                .to_string()
        });

    apply_template_code(
        &mut script,
        explicit_template,
        command.code.as_deref(),
        &app_type,
        &provider,
    );
    if let Some(timeout) = command.timeout {
        script.timeout = Some(timeout);
    } else if script.timeout.is_none() {
        script.timeout = Some(DEFAULT_USAGE_TIMEOUT);
    }
    if let Some(interval) = command.auto_query_interval {
        script.auto_query_interval = Some(normalize_usage_interval(interval));
    } else if script.auto_query_interval.is_none() {
        script.auto_query_interval = Some(DEFAULT_USAGE_AUTO_QUERY_INTERVAL);
    }

    script.language = DEFAULT_USAGE_LANGUAGE.to_string();
    script.template_type = Some(template.clone());
    apply_template_credentials(&mut script, &template, &command);
    validate_usage_script_for_save(&script)?;

    provider
        .meta
        .get_or_insert_with(ProviderMeta::default)
        .usage_script = Some(script);
    ProviderService::update(&state, app_type, provider)?;

    println!("{}", success("✓ Usage Query configuration updated"));
    Ok(())
}

fn clear(app_type: AppType, id: &str) -> Result<(), AppError> {
    let state = get_state()?;
    let mut provider = find_provider(&state, &app_type, id)?;
    if let Some(meta) = provider.meta.as_mut() {
        meta.usage_script = None;
    }
    ProviderService::update(&state, app_type, provider)?;

    println!("{}", success("✓ Usage Query configuration cleared"));
    Ok(())
}

fn find_provider(state: &AppState, app_type: &AppType, id: &str) -> Result<Provider, AppError> {
    let config = state.config.read().unwrap();
    let manager = config
        .get_manager(app_type)
        .ok_or_else(|| AppError::Message(format!("{} config not found", app_type.as_str())))?;
    manager.providers.get(id).cloned().ok_or_else(|| {
        AppError::localized(
            "provider.not_found",
            format!("供应商不存在: {id}"),
            format!("Provider not found: {id}"),
        )
    })
}

#[cfg(test)]
fn default_usage_script() -> UsageScript {
    usage_script_for_template(UsageQueryTemplate::General)
}

fn default_usage_script_for_provider(provider: &Provider) -> UsageScript {
    usage_script_for_template(default_usage_template_for_provider(provider))
}

fn usage_script_for_template(template: UsageQueryTemplate) -> UsageScript {
    UsageScript {
        enabled: false,
        language: DEFAULT_USAGE_LANGUAGE.to_string(),
        code: default_code_for_template(template).to_string(),
        timeout: Some(DEFAULT_USAGE_TIMEOUT),
        api_key: None,
        base_url: None,
        access_token: None,
        user_id: None,
        template_type: Some(template.as_str().to_string()),
        auto_query_interval: Some(DEFAULT_USAGE_AUTO_QUERY_INTERVAL),
        coding_plan_provider: None,
    }
}

fn default_usage_template_for_provider(provider: &Provider) -> UsageQueryTemplate {
    let base_url = provider_base_url(provider).unwrap_or_default();
    if detect_balance_provider(&base_url) {
        UsageQueryTemplate::Balance
    } else {
        UsageQueryTemplate::General
    }
}

fn effective_template_type(script: &UsageScript, provider: &Provider) -> String {
    if let Some(template) = script
        .template_type
        .as_deref()
        .map(str::trim)
        .filter(|template| !template.is_empty())
    {
        return template.to_string();
    }

    infer_usage_template(script)
        .unwrap_or_else(|| default_usage_template_for_provider(provider))
        .as_str()
        .to_string()
}

fn infer_usage_template(script: &UsageScript) -> Option<UsageQueryTemplate> {
    if script
        .access_token
        .as_ref()
        .is_some_and(|value| !value.is_empty())
        || script
            .user_id
            .as_ref()
            .is_some_and(|value| !value.is_empty())
    {
        Some(UsageQueryTemplate::Newapi)
    } else if script
        .api_key
        .as_ref()
        .is_some_and(|value| !value.is_empty())
        || script
            .base_url
            .as_ref()
            .is_some_and(|value| !value.is_empty())
    {
        Some(UsageQueryTemplate::General)
    } else {
        None
    }
}

fn default_code_for_template(template: UsageQueryTemplate) -> &'static str {
    match template {
        UsageQueryTemplate::Custom => DEFAULT_USAGE_CUSTOM_PRESET,
        UsageQueryTemplate::General => DEFAULT_USAGE_GENERAL_PRESET,
        UsageQueryTemplate::Newapi => DEFAULT_USAGE_NEWAPI_PRESET,
        UsageQueryTemplate::Balance => "",
    }
}

fn default_code_for_template_for_provider(
    template: UsageQueryTemplate,
    app_type: &AppType,
    provider: &Provider,
) -> String {
    match template {
        UsageQueryTemplate::Custom => custom_usage_code_for_provider(app_type, provider),
        _ => default_code_for_template(template).to_string(),
    }
}

fn custom_usage_code_for_provider(app_type: &AppType, provider: &Provider) -> String {
    format!(
        "{}{}",
        usage_query_custom_variable_comment(app_type, provider),
        DEFAULT_USAGE_CUSTOM_PRESET
    )
}

fn usage_query_custom_variable_comment(app_type: &AppType, provider: &Provider) -> String {
    let (base_url, api_key) = usage_query_comment_credentials(app_type, provider);
    format!(
        "// 支持的变量\n// {{{{baseUrl}}}}\n// =\n// {}\n// {{{{apiKey}}}}\n// =\n// {}\n\n",
        base_url, api_key,
    )
}

fn usage_query_comment_value(value: &str) -> String {
    value.trim().replace(['\r', '\n'], " ").trim().to_string()
}

fn apply_template_code(
    script: &mut UsageScript,
    explicit_template: Option<UsageQueryTemplate>,
    command_code: Option<&str>,
    app_type: &AppType,
    provider: &Provider,
) {
    if let Some(code) = command_code {
        script.code = code.to_string();
    } else if let Some(template) = explicit_template {
        script.code = default_code_for_template_for_provider(template, app_type, provider);
    }
}

fn normalize_usage_interval(value: u64) -> u64 {
    value.min(MAX_USAGE_AUTO_QUERY_INTERVAL)
}

fn template_requires_script(template: &str) -> bool {
    !matches!(template, "github_copilot" | "token_plan" | "balance")
}

fn validate_usage_script_for_save(script: &UsageScript) -> Result<(), AppError> {
    if !script.enabled {
        return Ok(());
    }

    let template = script.template_type.as_deref().unwrap_or("custom");
    if !template_requires_script(template) {
        return Ok(());
    }

    let code = script.code.trim();
    if code.is_empty() {
        return Err(AppError::InvalidInput(
            "Usage Query script cannot be empty when enabled".to_string(),
        ));
    }
    if !code.contains("return") {
        return Err(AppError::InvalidInput(
            "Usage Query script must contain a return statement when enabled".to_string(),
        ));
    }

    Ok(())
}

fn apply_template_credentials(
    script: &mut UsageScript,
    template: &str,
    command: &ProviderUsageQuerySetCommand,
) {
    match template {
        "general" => {
            set_trimmed_option(&mut script.api_key, command.api_key.as_deref());
            set_trimmed_option(&mut script.base_url, command.base_url.as_deref());
            script.access_token = None;
            script.user_id = None;
            script.coding_plan_provider = None;
        }
        "newapi" => {
            set_trimmed_option(&mut script.base_url, command.base_url.as_deref());
            set_trimmed_option(&mut script.access_token, command.access_token.as_deref());
            set_trimmed_option(&mut script.user_id, command.user_id.as_deref());
            script.api_key = None;
            script.coding_plan_provider = None;
        }
        "custom" | "balance" => {
            script.api_key = None;
            script.base_url = None;
            script.access_token = None;
            script.user_id = None;
            script.coding_plan_provider = None;
        }
        _ => {}
    }
}

fn set_trimmed_option(target: &mut Option<String>, value: Option<&str>) {
    if let Some(value) = value {
        let trimmed = value.trim();
        *target = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
    }
}

fn print_optional(label: &str, value: Option<&str>) {
    if let Some(value) = value {
        if !value.is_empty() {
            println!("  {label}: {value}");
        }
    }
}

fn print_optional_masked(label: &str, value: Option<&str>) {
    if let Some(value) = value {
        if !value.is_empty() {
            println!("  {label}: {}", mask_secret(value));
        }
    }
}

fn mask_secret(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 8 {
        return "***".to_string();
    }
    let prefix: String = chars.iter().take(4).collect();
    let suffix: String = chars[chars.len() - 4..].iter().collect();
    format!("{prefix}...{suffix}")
}

fn detect_balance_provider(base_url: &str) -> bool {
    let url = base_url.to_lowercase();
    url.contains("api.deepseek.com")
        || url.contains("api.stepfun.ai")
        || url.contains("api.stepfun.com")
        || url.contains("api.siliconflow.cn")
        || url.contains("api.siliconflow.com")
        || url.contains("openrouter.ai")
        || url.contains("api.novita.ai")
}

fn provider_base_url(provider: &Provider) -> Option<String> {
    provider
        .settings_config
        .get("env")
        .and_then(|value| value.get("ANTHROPIC_BASE_URL"))
        .and_then(|value| value.as_str())
        .or_else(|| {
            provider
                .settings_config
                .get("env")
                .and_then(|value| value.get("GOOGLE_GEMINI_BASE_URL"))
                .or_else(|| {
                    provider
                        .settings_config
                        .get("env")
                        .and_then(|value| value.get("GEMINI_BASE_URL"))
                })
                .and_then(|value| value.as_str())
        })
        .or_else(|| {
            provider
                .settings_config
                .get("base_url")
                .or_else(|| provider.settings_config.get("baseUrl"))
                .or_else(|| provider.settings_config.get("baseURL"))
                .or_else(|| provider.settings_config.get("endpoint"))
                .and_then(|value| value.as_str())
        })
        .or_else(|| {
            provider
                .settings_config
                .get("options")
                .and_then(|value| value.get("baseURL"))
                .and_then(|value| value.as_str())
        })
        .map(|value| value.to_string())
        .or_else(|| provider_codex_base_url(provider))
}

fn usage_query_comment_credentials(app_type: &AppType, provider: &Provider) -> (String, String) {
    let (base_url, api_key) = provider_comment_credentials(app_type, provider);
    let base_url = base_url.unwrap_or_default();
    (
        usage_query_comment_value(&base_url),
        usage_query_comment_value(api_key.unwrap_or_default()),
    )
}

fn provider_comment_credentials<'a>(
    app_type: &AppType,
    provider: &'a Provider,
) -> (Option<String>, Option<&'a str>) {
    let settings = &provider.settings_config;
    match app_type {
        AppType::Claude if provider.is_codex_oauth() => {
            (Some(CODEX_OAUTH_BASE_URL.to_string()), None)
        }
        AppType::Claude => {
            let env = settings.get("env");
            (
                env.and_then(|value| value.get("ANTHROPIC_BASE_URL"))
                    .and_then(|value| value.as_str())
                    .map(str::to_string),
                env.and_then(|value| value.get("ANTHROPIC_AUTH_TOKEN"))
                    .and_then(|value| value.as_str()),
            )
        }
        AppType::Codex => (
            provider_codex_model_provider_base_url(provider),
            settings
                .get("auth")
                .and_then(|value| value.get("OPENAI_API_KEY"))
                .and_then(|value| value.as_str()),
        ),
        AppType::Gemini => {
            let env = settings.get("env");
            (
                env.and_then(|value| value.get("GOOGLE_GEMINI_BASE_URL"))
                    .or_else(|| env.and_then(|value| value.get("GEMINI_BASE_URL")))
                    .and_then(|value| value.as_str())
                    .map(str::to_string),
                env.and_then(|value| value.get("GEMINI_API_KEY"))
                    .and_then(|value| value.as_str()),
            )
        }
        AppType::OpenCode => {
            let options = settings.get("options");
            (
                options
                    .and_then(|value| value.get("baseURL"))
                    .and_then(|value| value.as_str())
                    .map(str::to_string),
                options
                    .and_then(|value| value.get("apiKey"))
                    .and_then(|value| value.as_str()),
            )
        }
        AppType::Hermes => (
            settings
                .get("base_url")
                .or_else(|| settings.get("baseUrl"))
                .or_else(|| settings.get("baseURL"))
                .or_else(|| settings.get("endpoint"))
                .and_then(|value| value.as_str())
                .map(str::to_string),
            settings
                .get("api_key")
                .or_else(|| settings.get("apiKey"))
                .or_else(|| settings.get("auth_token"))
                .and_then(|value| value.as_str()),
        ),
        AppType::OpenClaw => (
            settings
                .get("baseUrl")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            settings.get("apiKey").and_then(|value| value.as_str()),
        ),
    }
}

fn provider_codex_base_url(provider: &Provider) -> Option<String> {
    let config_toml = provider
        .settings_config
        .get("config")
        .and_then(|value| value.as_str())?;
    let table = toml::from_str::<toml::Table>(config_toml).ok()?;
    table
        .get("base_url")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .or_else(|| {
            let provider_key = table
                .get("model_provider")
                .and_then(|value| value.as_str())?;
            table
                .get("model_providers")
                .and_then(|value| value.as_table())
                .and_then(|providers| providers.get(provider_key))
                .and_then(|value| value.as_table())
                .and_then(|provider_table| provider_table.get("base_url"))
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
        })
}

fn provider_codex_model_provider_base_url(provider: &Provider) -> Option<String> {
    let config_toml = provider
        .settings_config
        .get("config")
        .and_then(|value| value.as_str())?;
    let table = toml::from_str::<toml::Table>(config_toml).ok()?;
    let provider_key = table
        .get("model_provider")
        .and_then(|value| value.as_str())?;
    table
        .get("model_providers")
        .and_then(|value| value.as_table())
        .and_then(|providers| providers.get(provider_key))
        .and_then(|value| value.as_table())
        .and_then(|provider_table| provider_table.get("base_url"))
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_command(template: Option<UsageQueryTemplate>) -> ProviderUsageQuerySetCommand {
        ProviderUsageQuerySetCommand {
            id: "demo".to_string(),
            enabled: true,
            disabled: false,
            template,
            code: Some("return {};".to_string()),
            timeout: None,
            auto_query_interval: None,
            api_key: Some("sk-demo".to_string()),
            base_url: Some("https://usage.example.com".to_string()),
            access_token: Some("token-demo".to_string()),
            user_id: Some("user-demo".to_string()),
        }
    }

    #[test]
    fn default_usage_script_matches_upstream_defaults() {
        let script = default_usage_script();

        assert!(!script.enabled);
        assert_eq!(script.language, "javascript");
        assert!(script.code.contains("{{baseUrl}}/user/balance"));
        assert_eq!(script.timeout, Some(10));
        assert_eq!(script.auto_query_interval, Some(5));
        assert_eq!(script.template_type.as_deref(), Some("general"));
    }

    #[test]
    fn normalizes_auto_query_interval_like_tui() {
        assert_eq!(normalize_usage_interval(0), 0);
        assert_eq!(normalize_usage_interval(60), 60);
        assert_eq!(normalize_usage_interval(1441), 1440);
    }

    #[test]
    fn default_template_detects_balance_providers() {
        let provider = Provider::with_id(
            "demo".to_string(),
            "Demo".to_string(),
            serde_json::json!({"env": {"ANTHROPIC_BASE_URL": "https://openrouter.ai/api/v1"}}),
            None,
        );

        assert_eq!(
            default_usage_template_for_provider(&provider),
            UsageQueryTemplate::Balance
        );
    }

    #[test]
    fn default_usage_script_for_balance_provider_uses_balance_template() {
        let provider = Provider::with_id(
            "demo".to_string(),
            "Demo".to_string(),
            serde_json::json!({"env": {"ANTHROPIC_BASE_URL": "https://api.deepseek.com"}}),
            None,
        );

        let script = default_usage_script_for_provider(&provider);

        assert_eq!(script.template_type.as_deref(), Some("balance"));
        assert!(script.code.is_empty());
    }

    #[test]
    fn infers_legacy_usage_template_from_credentials() {
        let mut script = default_usage_script();
        script.template_type = None;
        script.access_token = Some("token-demo".to_string());
        assert_eq!(
            infer_usage_template(&script),
            Some(UsageQueryTemplate::Newapi)
        );

        script.access_token = None;
        script.api_key = Some("sk-demo".to_string());
        assert_eq!(
            infer_usage_template(&script),
            Some(UsageQueryTemplate::General)
        );
    }

    #[test]
    fn effective_template_type_infers_legacy_configs_like_tui() {
        let provider = Provider::with_id(
            "demo".to_string(),
            "Demo".to_string(),
            serde_json::json!({"env": {"ANTHROPIC_BASE_URL": "https://openrouter.ai/api/v1"}}),
            None,
        );
        let mut script = UsageScript {
            template_type: None,
            access_token: Some("token-demo".to_string()),
            user_id: None,
            ..default_usage_script()
        };

        assert_eq!(effective_template_type(&script, &provider), "newapi");

        script.access_token = None;
        script.api_key = Some("sk-demo".to_string());
        assert_eq!(effective_template_type(&script, &provider), "general");

        script.api_key = None;
        script.base_url = None;
        assert_eq!(effective_template_type(&script, &provider), "balance");
    }

    #[test]
    fn validates_enabled_script_like_upstream_modal() {
        let mut script = default_usage_script();
        script.enabled = true;

        assert!(validate_usage_script_for_save(&script).is_ok());

        script.code = "   ".to_string();
        assert!(validate_usage_script_for_save(&script).is_err());

        script.code = "({ request: { url: 'https://usage.example.com' } })".to_string();
        assert!(validate_usage_script_for_save(&script).is_err());

        script.template_type = Some("balance".to_string());
        script.code.clear();
        assert!(validate_usage_script_for_save(&script).is_ok());
    }

    #[test]
    fn general_template_keeps_only_general_credentials() {
        let mut script = default_usage_script();
        let command = set_command(Some(UsageQueryTemplate::General));
        apply_template_credentials(&mut script, "general", &command);

        assert_eq!(script.api_key.as_deref(), Some("sk-demo"));
        assert_eq!(
            script.base_url.as_deref(),
            Some("https://usage.example.com")
        );
        assert_eq!(script.access_token, None);
        assert_eq!(script.user_id, None);
        assert_eq!(script.coding_plan_provider, None);
    }

    #[test]
    fn newapi_template_keeps_only_newapi_credentials() {
        let mut script = default_usage_script();
        let command = set_command(Some(UsageQueryTemplate::Newapi));
        apply_template_credentials(&mut script, "newapi", &command);

        assert_eq!(script.api_key, None);
        assert_eq!(
            script.base_url.as_deref(),
            Some("https://usage.example.com")
        );
        assert_eq!(script.access_token.as_deref(), Some("token-demo"));
        assert_eq!(script.user_id.as_deref(), Some("user-demo"));
        assert_eq!(script.coding_plan_provider, None);
    }

    #[test]
    fn custom_template_removes_template_credentials() {
        let mut script = UsageScript {
            api_key: Some("sk-old".to_string()),
            base_url: Some("https://old.example.com".to_string()),
            access_token: Some("old-token".to_string()),
            user_id: Some("old-user".to_string()),
            coding_plan_provider: Some("kimi".to_string()),
            ..default_usage_script()
        };

        let command = set_command(Some(UsageQueryTemplate::Custom));
        apply_template_credentials(&mut script, "custom", &command);

        assert_eq!(script.api_key, None);
        assert_eq!(script.base_url, None);
        assert_eq!(script.access_token, None);
        assert_eq!(script.user_id, None);
        assert_eq!(script.coding_plan_provider, None);
    }

    #[test]
    fn default_code_tracks_template_selection() {
        assert!(default_code_for_template(UsageQueryTemplate::General).contains("{{apiKey}}"));
        assert!(default_code_for_template(UsageQueryTemplate::Newapi).contains("{{accessToken}}"));
        assert!(default_code_for_template(UsageQueryTemplate::Custom).contains("remaining: 0"));
        assert_eq!(default_code_for_template(UsageQueryTemplate::Balance), "");
    }

    #[test]
    fn provider_usage_query_custom_template_defaults_include_provider_variables() {
        let provider = Provider::with_id(
            "demo".to_string(),
            "Demo".to_string(),
            serde_json::json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": " sk-demo\nsecret ",
                    "ANTHROPIC_BASE_URL": " https://usage.example.com/v1 "
                }
            }),
            None,
        );
        let mut script = default_usage_script();

        apply_template_code(
            &mut script,
            Some(UsageQueryTemplate::Custom),
            None,
            &AppType::Claude,
            &provider,
        );

        assert!(script.code.starts_with(
            "// 支持的变量\n// {{baseUrl}}\n// =\n// https://usage.example.com/v1\n// {{apiKey}}\n// =\n// sk-demo secret\n\n"
        ));
        assert!(script.code.contains(DEFAULT_USAGE_CUSTOM_PRESET));
    }

    #[test]
    fn provider_usage_query_custom_template_uses_tui_provider_credentials_by_app() {
        let cases = vec![
            (
                AppType::Claude,
                Provider::with_id(
                    "claude".to_string(),
                    "Claude".to_string(),
                    serde_json::json!({
                        "baseUrl": "https://wrong.example/v1",
                        "env": {
                            "ANTHROPIC_BASE_URL": "https://claude.example/v1",
                            "ANTHROPIC_AUTH_TOKEN": "sk-claude"
                        }
                    }),
                    None,
                ),
                "https://claude.example/v1",
                "sk-claude",
            ),
            (
                AppType::Codex,
                Provider::with_id(
                    "codex".to_string(),
                    "Codex".to_string(),
                    serde_json::json!({
                        "config": "base_url = \"https://wrong.example/v1\"\nmodel_provider = \"real\"\n\n[model_providers.real]\nbase_url = \"https://codex.example/v1\"\n",
                        "auth": {
                            "OPENAI_API_KEY": "sk-codex"
                        }
                    }),
                    None,
                ),
                "https://codex.example/v1",
                "sk-codex",
            ),
            (
                AppType::Gemini,
                Provider::with_id(
                    "gemini".to_string(),
                    "Gemini".to_string(),
                    serde_json::json!({
                        "baseUrl": "https://wrong.example/v1",
                        "env": {
                            "GOOGLE_GEMINI_BASE_URL": "https://gemini.example/v1",
                            "GEMINI_API_KEY": "sk-gemini"
                        }
                    }),
                    None,
                ),
                "https://gemini.example/v1",
                "sk-gemini",
            ),
            (
                AppType::OpenCode,
                Provider::with_id(
                    "opencode".to_string(),
                    "OpenCode".to_string(),
                    serde_json::json!({
                        "baseUrl": "https://wrong.example/v1",
                        "apiKey": "wrong-key",
                        "options": {
                            "baseURL": "https://opencode.example/v1",
                            "apiKey": "sk-opencode"
                        }
                    }),
                    None,
                ),
                "https://opencode.example/v1",
                "sk-opencode",
            ),
            (
                AppType::Hermes,
                Provider::with_id(
                    "hermes".to_string(),
                    "Hermes".to_string(),
                    serde_json::json!({
                        "endpoint": "https://wrong.example/v1",
                        "base_url": "https://hermes.example/v1",
                        "api_key": "sk-hermes"
                    }),
                    None,
                ),
                "https://hermes.example/v1",
                "sk-hermes",
            ),
            (
                AppType::OpenClaw,
                Provider::with_id(
                    "openclaw".to_string(),
                    "OpenClaw".to_string(),
                    serde_json::json!({
                        "base_url": "https://wrong.example/v1",
                        "baseUrl": "https://openclaw.example/v1",
                        "apiKey": "sk-openclaw"
                    }),
                    None,
                ),
                "https://openclaw.example/v1",
                "sk-openclaw",
            ),
        ];

        for (app_type, provider, expected_base_url, expected_api_key) in cases {
            let mut script = default_usage_script();

            apply_template_code(
                &mut script,
                Some(UsageQueryTemplate::Custom),
                None,
                &app_type,
                &provider,
            );

            let expected_prefix = format!(
                "// 支持的变量\n// {{{{baseUrl}}}}\n// =\n// {expected_base_url}\n// {{{{apiKey}}}}\n// =\n// {expected_api_key}\n\n"
            );
            assert!(
                script.code.starts_with(&expected_prefix),
                "{} custom variable comment did not match TUI-loaded credentials:\n{}",
                app_type.as_str(),
                script.code
            );
        }
    }

    #[test]
    fn provider_usage_query_custom_template_uses_codex_oauth_provider_variables() {
        let mut provider = Provider::with_id(
            "codex-oauth".to_string(),
            "Codex OAuth".to_string(),
            serde_json::json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://wrong.example/v1",
                    "ANTHROPIC_AUTH_TOKEN": "sk-wrong"
                }
            }),
            None,
        );
        provider.meta = Some(ProviderMeta {
            provider_type: Some("codex_oauth".to_string()),
            ..ProviderMeta::default()
        });
        let mut script = default_usage_script();

        apply_template_code(
            &mut script,
            Some(UsageQueryTemplate::Custom),
            None,
            &AppType::Claude,
            &provider,
        );

        assert!(script.code.starts_with(
            "// 支持的变量\n// {{baseUrl}}\n// =\n// https://chatgpt.com/backend-api/codex\n// {{apiKey}}\n// =\n// \n\n"
        ));
    }

    #[test]
    fn provider_usage_query_custom_template_explicit_code_is_not_rewritten() {
        let provider = Provider::with_id(
            "demo".to_string(),
            "Demo".to_string(),
            serde_json::json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "sk-demo",
                    "ANTHROPIC_BASE_URL": "https://usage.example.com/v1"
                }
            }),
            None,
        );
        let mut script = default_usage_script();

        apply_template_code(
            &mut script,
            Some(UsageQueryTemplate::Custom),
            Some("return { remaining: 42 };"),
            &AppType::Claude,
            &provider,
        );

        assert_eq!(script.code, "return { remaining: 42 };");
    }

    #[test]
    fn masks_usage_query_secrets_for_text_output() {
        assert_eq!(mask_secret("sk-1234567890"), "sk-1...7890");
        assert_eq!(mask_secret("密钥一二三四五六七八九"), "密钥一二...六七八九");
        assert_eq!(mask_secret("short"), "***");
    }

    #[test]
    fn empty_credential_arguments_clear_existing_values() {
        let mut script = UsageScript {
            api_key: Some("sk-old".to_string()),
            base_url: Some("https://old.example.com".to_string()),
            ..default_usage_script()
        };
        let mut command = set_command(Some(UsageQueryTemplate::General));
        command.api_key = Some("   ".to_string());
        command.base_url = None;

        apply_template_credentials(&mut script, "general", &command);

        assert_eq!(script.api_key, None);
        assert_eq!(script.base_url.as_deref(), Some("https://old.example.com"));
    }
}
