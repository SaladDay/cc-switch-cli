use serde::Serialize;
use serde_json::Value;

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::cli::provider_quota::{
    display_usage_plan_name, provider_display_name, query_quota, quota_target_for_provider,
    usage_value_summary, ProviderUsageQuota, QuotaTarget,
};
use crate::cli::ui::{create_table, error, highlight, info, success, to_json, warning};
use crate::error::AppError;
use crate::provider::{Provider, UsageData, UsageResult};
use crate::services::{
    CodexOAuthService, CredentialStatus, ProviderService, SpeedtestService, StreamCheckService,
};
use crate::store::AppState;

const AUTH_PROVIDER_CODEX_OAUTH: &str = "codex_oauth";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderModelFetchStrategy {
    Bearer,
    Anthropic,
    GoogleApiKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ModelFetchSource {
    Http(ModelFetchTarget),
    CodexOAuth { account_id: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelFetchTarget {
    base_url: String,
    auth_value: Option<String>,
    strategy: ProviderModelFetchStrategy,
}

#[derive(Default)]
struct ClaudeConfig {
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
    haiku_model: Option<String>,
    sonnet_model: Option<String>,
    opus_model: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderQuotaOutput {
    app: AppType,
    provider_id: String,
    provider_name: String,
    target: Option<QuotaTarget>,
    status: String,
    available: bool,
    queried_at: i64,
    result: Option<ProviderUsageQuota>,
    error: Option<String>,
}

fn get_state() -> Result<AppState, AppError> {
    AppState::try_new()
}
pub(crate) fn list_providers(app_type: AppType) -> Result<(), AppError> {
    let state = get_state()?;
    let app_str = app_type.as_str().to_string();
    let providers = ProviderService::list(&state, app_type.clone())?;
    let current_id = ProviderService::current(&state, app_type.clone())?;

    if providers.is_empty() {
        println!("{}", info("No providers found."));
        println!("{}", texts::no_providers_hint());
        return Ok(());
    }

    let mut table = create_table();
    table.set_header(vec!["", "ID", "Name", "API URL"]);

    let mut provider_list: Vec<_> = providers.into_iter().collect();
    provider_list.sort_by(|(_, a), (_, b)| match (a.sort_index, b.sort_index) {
        (Some(idx_a), Some(idx_b)) => idx_a.cmp(&idx_b),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.created_at.cmp(&b.created_at),
    });

    for (id, provider) in provider_list {
        let current_marker = if id == current_id { "✓" } else { " " };
        let api_url = extract_api_url(&provider, &app_type).unwrap_or_else(|| "N/A".to_string());

        table.add_row(vec![current_marker.to_string(), id, provider.name, api_url]);
    }

    println!("{}", table);
    println!("\n{} Application: {}", info("ℹ"), app_str);
    println!("{} Current: {}", info("→"), highlight(&current_id));

    Ok(())
}

pub(crate) fn show_current(app_type: AppType) -> Result<(), AppError> {
    let state = get_state()?;
    let current_id = ProviderService::current(&state, app_type.clone())?;
    let providers = ProviderService::list(&state, app_type.clone())?;

    let provider = providers
        .get(&current_id)
        .ok_or_else(|| AppError::Message(format!("Current provider '{}' not found", current_id)))?;

    println!("{}", highlight("Current Provider"));
    println!("{}", "═".repeat(60));

    println!("\n{}", highlight(texts::basic_info_section_header()));
    println!("  ID:       {}", current_id);
    println!(
        "  {}:     {}",
        texts::name_label_with_colon(),
        provider.name
    );
    println!(
        "  {}:     {}",
        texts::app_label_with_colon(),
        app_type.as_str()
    );

    if matches!(app_type, AppType::Claude) {
        let config = extract_claude_config(&provider.settings_config);

        println!("\n{}", highlight(texts::api_config_section_header()));
        println!(
            "  Base URL: {}",
            config.base_url.unwrap_or_else(|| "N/A".to_string())
        );
        println!(
            "  API Key:  {}",
            config.api_key.unwrap_or_else(|| "N/A".to_string())
        );

        println!("\n{}", highlight(texts::model_config_section_header()));
        println!(
            "  {}:   {}",
            texts::main_model_label_with_colon(),
            config.model.unwrap_or_else(|| "default".to_string())
        );
        println!(
            "  Haiku:    {}",
            config.haiku_model.unwrap_or_else(|| "default".to_string())
        );
        println!(
            "  Sonnet:   {}",
            config.sonnet_model.unwrap_or_else(|| "default".to_string())
        );
        println!(
            "  Opus:     {}",
            config.opus_model.unwrap_or_else(|| "default".to_string())
        );
    } else {
        println!("\n{}", highlight("API 配置 / API Configuration"));
        let api_url = extract_api_url(provider, &app_type).unwrap_or_else(|| "N/A".to_string());
        println!("  API URL:  {}", api_url);
    }

    println!("\n{}", "─".repeat(60));

    Ok(())
}

pub(crate) fn speedtest_provider(app_type: AppType, id: &str) -> Result<(), AppError> {
    let state = get_state()?;
    let providers = ProviderService::list(&state, app_type.clone())?;
    let provider = providers
        .get(id)
        .ok_or_else(|| AppError::Message(format!("Provider '{}' not found", id)))?;

    let api_url = extract_api_url(provider, &app_type)
        .ok_or_else(|| AppError::Message(format!("No API URL configured for provider '{}'", id)))?;

    println!(
        "{}",
        info(&format!("Testing provider '{}'...", provider.name))
    );
    println!("{}", info(&format!("Endpoint: {}", api_url)));
    println!();

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| AppError::Message(format!("Failed to create async runtime: {}", e)))?;

    let results = runtime
        .block_on(async { SpeedtestService::test_endpoints(vec![api_url.clone()], None).await })?;

    if let Some(result) = results.first() {
        let mut table = create_table();
        table.set_header(vec!["Endpoint", "Latency", "Status"]);

        let latency_str = if let Some(latency) = result.latency {
            format!("{} ms", latency)
        } else if result.error.is_some() {
            "Failed".to_string()
        } else {
            "Timeout".to_string()
        };

        let status_str = result
            .status
            .map(|status| status.to_string())
            .unwrap_or_else(|| "N/A".to_string());

        table.add_row(vec![result.url.clone(), latency_str, status_str]);

        println!("{}", table);

        if let Some(err) = &result.error {
            println!("\n{}", error(&format!("Error: {}", err)));
        } else if result.latency.is_some() {
            println!("\n{}", success("✓ Speedtest completed successfully"));
        }
    }

    Ok(())
}

pub(crate) fn stream_check_provider(app_type: AppType, id: &str) -> Result<(), AppError> {
    let state = get_state()?;
    let providers = ProviderService::list(&state, app_type.clone())?;
    let provider = providers
        .get(id)
        .ok_or_else(|| AppError::Message(format!("Provider '{}' not found", id)))?
        .clone();
    let config = state.db.get_stream_check_config()?;

    println!(
        "{}",
        info(&format!("Running stream check for '{}'...", provider.name))
    );

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| AppError::Message(format!("Failed to create async runtime: {}", e)))?;

    let result = runtime.block_on(async {
        StreamCheckService::check_with_retry(&app_type, &provider, &config).await
    })?;

    let _ = state
        .db
        .save_stream_check_log(id, &provider.name, app_type.as_str(), &result);

    println!("{}", highlight("Stream Check"));
    println!("{}", "═".repeat(60));
    for line in crate::cli::tui::build_stream_check_result_lines(&provider.name, &result) {
        println!("{}", line);
    }
    println!();
    if result.success {
        println!("{}", success("✓ Stream check completed successfully"));
    } else {
        println!("{}", warning("Stream check finished with errors."));
    }

    Ok(())
}

pub(crate) fn fetch_models_provider(app_type: AppType, id: &str) -> Result<(), AppError> {
    let state = get_state()?;
    let providers = ProviderService::list(&state, app_type.clone())?;
    let provider = providers
        .get(id)
        .ok_or_else(|| AppError::Message(format!("Provider '{}' not found", id)))?;
    let source = model_fetch_source(provider, &app_type)?;

    println!(
        "{}",
        info(&format!("Fetching models for '{}'...", provider.name))
    );
    print_model_fetch_source(&source);
    println!();

    let models = fetch_models_from_source(&source)?;
    print_fetched_models(&models);

    Ok(())
}

pub(crate) fn fetch_models_once(
    app_type: AppType,
    base_url: Option<&str>,
    api_key: Option<&str>,
    strategy: Option<ProviderModelFetchStrategy>,
) -> Result<(), AppError> {
    let target = one_off_model_fetch_target(&app_type, base_url, api_key, strategy)?;
    let source = ModelFetchSource::Http(target);

    println!("{}", info("Fetching models from one-off config..."));
    print_model_fetch_source(&source);
    println!();

    let models = fetch_models_from_source(&source)?;
    print_fetched_models(&models);

    Ok(())
}

fn print_model_fetch_source(source: &ModelFetchSource) {
    match &source {
        ModelFetchSource::Http(target) => {
            println!("{}", info(&format!("Endpoint: {}", target.base_url)));
        }
        ModelFetchSource::CodexOAuth { account_id } => {
            println!("{}", info("Endpoint: Codex OAuth managed account"));
            if let Some(account_id) = account_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                println!("{}", info(&format!("Account: {}", account_id)));
            }
        }
    }
}

fn fetch_models_from_source(source: &ModelFetchSource) -> Result<Vec<String>, AppError> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| AppError::Message(format!("Failed to create async runtime: {}", e)))?;

    match &source {
        ModelFetchSource::Http(target) => runtime.block_on(async {
            crate::cli::tui::fetch_provider_models_for_tui(
                &target.base_url,
                target.auth_value.as_deref(),
                to_tui_strategy(target.strategy),
            )
            .await
            .map_err(AppError::Message)
        }),
        ModelFetchSource::CodexOAuth { account_id } => runtime.block_on(async {
            CodexOAuthService::get_models(account_id.as_deref())
                .await
                .map(|models| models.into_iter().map(|model| model.id).collect())
                .map_err(AppError::Message)
        }),
    }
}

fn print_fetched_models(models: &[String]) {
    if models.is_empty() {
        println!("{}", info("No models returned."));
        return;
    }

    let mut table = create_table();
    table.set_header(vec!["#", "Model"]);
    for (index, model) in models.iter().enumerate() {
        table.add_row(vec![(index + 1).to_string(), model.clone()]);
    }

    println!("{}", table);
    println!();
    println!(
        "{}",
        success(&format!("✓ Fetched {} model(s)", models.len()))
    );
}

pub(crate) fn quota_provider(app_type: AppType, id: &str, json: bool) -> Result<(), AppError> {
    let state = get_state()?;
    let providers = ProviderService::list(&state, app_type.clone())?;
    let provider = providers
        .get(id)
        .ok_or_else(|| AppError::Message(format!("Provider '{}' not found", id)))?;
    let provider_name = provider_display_name(&app_type, id, provider);
    let target = quota_target_for_provider(&app_type, id, provider);
    let queried_at = chrono::Utc::now().timestamp_millis();

    let output = if let Some(target) = target {
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|e| AppError::Message(format!("Failed to create async runtime: {}", e)))?;
        match runtime.block_on(query_quota(&target)) {
            Ok(result) => quota_output_from_result(
                app_type,
                id.to_string(),
                provider_name,
                Some(target),
                queried_at,
                result,
            ),
            Err(error) => ProviderQuotaOutput {
                app: app_type,
                provider_id: id.to_string(),
                provider_name,
                target: Some(target),
                status: "query_failed".to_string(),
                available: false,
                queried_at,
                result: None,
                error: Some(error),
            },
        }
    } else {
        ProviderQuotaOutput {
            app: app_type,
            provider_id: id.to_string(),
            provider_name,
            target: None,
            status: "not_available".to_string(),
            available: false,
            queried_at,
            result: None,
            error: None,
        }
    };

    if json {
        println!(
            "{}",
            to_json(&output).map_err(|source| AppError::JsonSerialize { source })?
        );
        return Ok(());
    }

    for line in provider_quota_text_lines(&output) {
        println!("{line}");
    }
    Ok(())
}

fn quota_output_from_result(
    app: AppType,
    provider_id: String,
    provider_name: String,
    target: Option<QuotaTarget>,
    queried_at: i64,
    result: ProviderUsageQuota,
) -> ProviderQuotaOutput {
    let (status, available, error) = quota_result_summary(&result);
    ProviderQuotaOutput {
        app,
        provider_id,
        provider_name,
        target,
        status,
        available,
        queried_at,
        result: Some(result),
        error,
    }
}

fn quota_result_summary(result: &ProviderUsageQuota) -> (String, bool, Option<String>) {
    match result {
        ProviderUsageQuota::Subscription(quota) => subscription_quota_summary(quota),
        ProviderUsageQuota::Script(result) => script_usage_summary(result),
    }
}

fn subscription_quota_summary(
    quota: &crate::services::SubscriptionQuota,
) -> (String, bool, Option<String>) {
    match quota.credential_status {
        CredentialStatus::NotFound => return ("not_available".to_string(), false, None),
        CredentialStatus::ParseError => {
            return (
                "credential_parse_failed".to_string(),
                false,
                quota
                    .credential_message
                    .clone()
                    .or_else(|| quota.error.clone()),
            );
        }
        CredentialStatus::Expired if !quota.success => {
            return (
                "login_expired".to_string(),
                false,
                quota
                    .credential_message
                    .clone()
                    .or_else(|| quota.error.clone()),
            );
        }
        _ => {}
    }

    if !quota.success {
        return ("query_failed".to_string(), false, quota.error.clone());
    }

    if quota.tiers.is_empty() {
        return ("not_available".to_string(), false, None);
    }

    ("ok".to_string(), true, None)
}

fn script_usage_summary(result: &UsageResult) -> (String, bool, Option<String>) {
    if !result.success {
        return ("query_failed".to_string(), false, result.error.clone());
    }

    if result.data.as_ref().is_none_or(|items| items.is_empty()) {
        return ("not_available".to_string(), false, None);
    }

    ("ok".to_string(), true, None)
}

fn provider_quota_text_lines(output: &ProviderQuotaOutput) -> Vec<String> {
    let mut lines = vec![
        format!(
            "Provider: {} ({})",
            output.provider_name, output.provider_id
        ),
        format!("Application: {}", output.app.as_str()),
    ];

    match &output.result {
        Some(ProviderUsageQuota::Subscription(quota)) => {
            push_subscription_quota_lines(&mut lines, quota);
        }
        Some(ProviderUsageQuota::Script(result)) => {
            push_script_usage_lines(&mut lines, result);
        }
        None if output.status == "query_failed" => {
            lines.push(format!(
                "{}: {}",
                texts::tui_label_quota(),
                texts::tui_quota_query_failed()
            ));
            if let Some(error) = output
                .error
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                lines.push(format!("Error: {error}"));
            }
        }
        None => {
            lines.push(format!(
                "{}: {}",
                texts::tui_label_quota(),
                texts::tui_quota_not_available()
            ));
        }
    }

    lines
}

fn push_subscription_quota_lines(
    lines: &mut Vec<String>,
    quota: &crate::services::SubscriptionQuota,
) {
    match quota.credential_status {
        CredentialStatus::NotFound => {
            lines.push(format!(
                "{}: {}",
                texts::tui_label_quota(),
                texts::tui_quota_not_available()
            ));
            return;
        }
        CredentialStatus::ParseError => {
            lines.push(format!(
                "{}: {}",
                texts::tui_label_quota(),
                texts::tui_quota_parse_error()
            ));
            push_optional_error(
                lines,
                quota.credential_message.as_ref().or(quota.error.as_ref()),
            );
            return;
        }
        CredentialStatus::Expired if !quota.success => {
            lines.push(format!(
                "{}: {}",
                texts::tui_label_quota(),
                texts::tui_quota_expired()
            ));
            push_optional_error(
                lines,
                quota.credential_message.as_ref().or(quota.error.as_ref()),
            );
            return;
        }
        _ => {}
    }

    if !quota.success {
        lines.push(format!(
            "{}: {}",
            texts::tui_label_quota(),
            texts::tui_quota_query_failed()
        ));
        push_optional_error(lines, quota.error.as_ref());
        return;
    }

    if quota.tiers.is_empty() {
        lines.push(format!(
            "{}: {}",
            texts::tui_label_quota(),
            texts::tui_quota_not_available()
        ));
        return;
    }

    lines.push(format!(
        "{}: {}",
        texts::tui_label_quota(),
        texts::tui_quota_ok()
    ));
    if let Some(queried_at) = quota.queried_at {
        lines.push(format!(
            "{}: {}",
            texts::tui_quota_last_checked(),
            queried_at
        ));
    }
    for tier in &quota.tiers {
        lines.push(format!(
            "{}: {}",
            quota_tier_label(&tier.name),
            quota_percent_text(tier.utilization)
        ));
    }
    if let Some(extra) = quota.extra_usage.as_ref().filter(|extra| extra.is_enabled) {
        let mut parts = Vec::new();
        if let Some(used) = extra.used_credits {
            parts.push(format!("{used:.1}"));
        }
        if let Some(limit) = extra.monthly_limit {
            parts.push(format!("/ {limit:.1}"));
        }
        if let Some(currency) = extra.currency.as_deref() {
            parts.push(currency.to_string());
        }
        if let Some(utilization) = extra.utilization {
            parts.push(format!("({})", quota_percent_text(utilization)));
        }
        if !parts.is_empty() {
            lines.push(format!(
                "{}: {}",
                texts::tui_quota_extra_usage(),
                parts.join(" ")
            ));
        }
    }
}

fn push_script_usage_lines(lines: &mut Vec<String>, result: &UsageResult) {
    if !result.success {
        lines.push(format!(
            "{}: {}",
            texts::tui_label_quota(),
            texts::tui_quota_query_failed()
        ));
        push_optional_error(lines, result.error.as_ref());
        return;
    }

    let Some(items) = result.data.as_ref().filter(|items| !items.is_empty()) else {
        lines.push(format!(
            "{}: {}",
            texts::tui_label_quota(),
            texts::tui_quota_not_available()
        ));
        return;
    };

    if items.len() == 1 && display_usage_plan_name(&items[0]).is_none() {
        lines.push(format!(
            "{}: {}",
            texts::tui_label_quota(),
            script_usage_item_text(&items[0])
        ));
        return;
    }

    lines.push(format!(
        "{}: {}",
        texts::tui_label_quota(),
        texts::tui_quota_ok()
    ));
    for (idx, item) in items.iter().enumerate() {
        let label = display_usage_plan_name(item)
            .map_or_else(|| format!("Usage {}", idx + 1), str::to_string);
        lines.push(format!("{label}: {}", script_usage_item_text(item)));
    }
}

fn script_usage_item_text(item: &UsageData) -> String {
    let mut parts = Vec::new();
    parts.push(usage_value_summary(item).unwrap_or_else(|| texts::tui_quota_ok().to_string()));
    if item.is_valid == Some(false) {
        parts.push(
            item.invalid_message
                .clone()
                .unwrap_or_else(|| texts::tui_quota_query_failed().to_string()),
        );
    }
    if let Some(extra) = item
        .extra
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(extra.to_string());
    }
    parts.join("  ")
}

fn push_optional_error(lines: &mut Vec<String>, message: Option<&String>) {
    if let Some(message) = message
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("Error: {message}"));
    }
}

fn quota_tier_label(name: &str) -> String {
    match name {
        "five_hour" => texts::tui_quota_tier_five_hour().to_string(),
        "seven_day" => texts::tui_quota_tier_seven_day().to_string(),
        "seven_day_opus" => texts::tui_quota_tier_seven_day_opus().to_string(),
        "seven_day_sonnet" => texts::tui_quota_tier_seven_day_sonnet().to_string(),
        "weekly_limit" => texts::tui_quota_tier_weekly_limit().to_string(),
        "premium" => texts::tui_quota_tier_premium().to_string(),
        "gemini_pro" => texts::tui_quota_tier_gemini_pro().to_string(),
        "gemini_flash" => texts::tui_quota_tier_gemini_flash().to_string(),
        "gemini_flash_lite" => texts::tui_quota_tier_gemini_flash_lite().to_string(),
        other => other.replace('_', " "),
    }
}

fn quota_percent_text(utilization: f64) -> String {
    format!("{:.0}%", utilization.clamp(0.0, 100.0))
}

fn model_fetch_target(
    provider: &Provider,
    app_type: &AppType,
) -> Result<ModelFetchTarget, AppError> {
    let base_url = StreamCheckService::extract_base_url(provider, app_type)?;
    let base_url = base_url.trim().trim_end_matches('/').to_string();
    if base_url.is_empty() {
        return Err(AppError::Message(format!(
            "No API URL configured for provider '{}'",
            provider.id
        )));
    }

    match app_type {
        AppType::Claude => {
            let auth_value = StreamCheckService::extract_claude_key(provider).ok_or_else(|| {
                AppError::Message(format!("Missing API key for provider '{}'", provider.id))
            })?;
            let strategy = if claude_uses_bearer_auth(provider, &base_url) {
                ProviderModelFetchStrategy::Bearer
            } else {
                ProviderModelFetchStrategy::Anthropic
            };

            Ok(ModelFetchTarget {
                base_url,
                auth_value: Some(auth_value),
                strategy,
            })
        }
        AppType::Codex => {
            Ok(ModelFetchTarget {
                base_url,
                auth_value: Some(StreamCheckService::extract_codex_key(provider).ok_or_else(
                    || AppError::Message(format!("Missing API key for provider '{}'", provider.id)),
                )?),
                strategy: ProviderModelFetchStrategy::Bearer,
            })
        }
        AppType::Gemini => {
            let (auth_value, strategy) = extract_gemini_model_fetch_auth(provider)?;
            Ok(ModelFetchTarget {
                base_url,
                auth_value: Some(auth_value),
                strategy,
            })
        }
        AppType::OpenCode => Ok(ModelFetchTarget {
            base_url,
            auth_value: Some(
                provider
                    .settings_config
                    .get("options")
                    .and_then(|options| options.get("apiKey"))
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| {
                        AppError::Message(format!("Missing API key for provider '{}'", provider.id))
                    })?,
            ),
            strategy: ProviderModelFetchStrategy::Bearer,
        }),
        AppType::Hermes => Ok(ModelFetchTarget {
            base_url,
            auth_value: Some(
                provider
                    .settings_config
                    .get("apiKey")
                    .or_else(|| provider.settings_config.get("api_key"))
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| {
                        AppError::Message(format!("Missing API key for provider '{}'", provider.id))
                    })?,
            ),
            strategy: ProviderModelFetchStrategy::Bearer,
        }),
        AppType::OpenClaw => Ok(ModelFetchTarget {
            base_url,
            auth_value: Some(
                provider
                    .settings_config
                    .get("apiKey")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| {
                        AppError::Message(format!("Missing API key for provider '{}'", provider.id))
                    })?,
            ),
            strategy: ProviderModelFetchStrategy::Bearer,
        }),
    }
}

fn one_off_model_fetch_target(
    app_type: &AppType,
    base_url: Option<&str>,
    api_key: Option<&str>,
    strategy: Option<ProviderModelFetchStrategy>,
) -> Result<ModelFetchTarget, AppError> {
    let base_url = base_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::Message("No API URL configured for one-off model fetch".into()))?
        .trim_end_matches('/')
        .to_string();
    let auth_value = api_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let strategy = strategy.unwrap_or_else(|| default_one_off_model_fetch_strategy(app_type));

    Ok(ModelFetchTarget {
        base_url,
        auth_value,
        strategy,
    })
}

fn default_one_off_model_fetch_strategy(app_type: &AppType) -> ProviderModelFetchStrategy {
    match app_type {
        AppType::Claude => ProviderModelFetchStrategy::Anthropic,
        AppType::Gemini => ProviderModelFetchStrategy::GoogleApiKey,
        AppType::Codex | AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => {
            ProviderModelFetchStrategy::Bearer
        }
    }
}

fn model_fetch_source(
    provider: &Provider,
    app_type: &AppType,
) -> Result<ModelFetchSource, AppError> {
    if matches!(app_type, AppType::Claude) && provider.is_codex_oauth() {
        return Ok(ModelFetchSource::CodexOAuth {
            account_id: codex_oauth_account_id(provider),
        });
    }

    model_fetch_target(provider, app_type).map(ModelFetchSource::Http)
}

fn codex_oauth_account_id(provider: &Provider) -> Option<String> {
    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.managed_account_id_for(AUTH_PROVIDER_CODEX_OAUTH))
}

fn claude_uses_bearer_auth(provider: &Provider, base_url: &str) -> bool {
    if base_url.contains("openrouter.ai") {
        return true;
    }

    provider
        .settings_config
        .get("auth_mode")
        .and_then(|value| value.as_str())
        .or_else(|| {
            provider
                .settings_config
                .get("env")
                .and_then(|env| env.get("AUTH_MODE"))
                .and_then(|value| value.as_str())
        })
        .is_some_and(|value| value == "bearer_only")
}

fn extract_gemini_model_fetch_auth(
    provider: &Provider,
) -> Result<(String, ProviderModelFetchStrategy), AppError> {
    let env_map = crate::gemini_config::json_to_env(&provider.settings_config)?;

    if let Some(token) = env_map
        .get("GOOGLE_ACCESS_TOKEN")
        .or_else(|| env_map.get("GEMINI_ACCESS_TOKEN"))
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok((token.to_string(), ProviderModelFetchStrategy::Bearer));
    }

    let key = env_map
        .get("GEMINI_API_KEY")
        .or_else(|| env_map.get("GOOGLE_API_KEY"))
        .or_else(|| env_map.get("API_KEY"))
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::Message(format!("Missing API key for provider '{}'", provider.id))
        })?;

    if key.starts_with("ya29.") {
        return Ok((key.to_string(), ProviderModelFetchStrategy::Bearer));
    }

    if let Some(access_token) = parse_access_token_blob(key) {
        return Ok((access_token, ProviderModelFetchStrategy::Bearer));
    }

    Ok((key.to_string(), ProviderModelFetchStrategy::GoogleApiKey))
}

fn parse_access_token_blob(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw.trim()).ok()?;
    value
        .get("access_token")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn to_tui_strategy(strategy: ProviderModelFetchStrategy) -> crate::cli::tui::ModelFetchStrategy {
    match strategy {
        ProviderModelFetchStrategy::Bearer => crate::cli::tui::ModelFetchStrategy::Bearer,
        ProviderModelFetchStrategy::Anthropic => crate::cli::tui::ModelFetchStrategy::Anthropic,
        ProviderModelFetchStrategy::GoogleApiKey => {
            crate::cli::tui::ModelFetchStrategy::GoogleApiKey
        }
    }
}

fn extract_api_url(provider: &Provider, app_type: &AppType) -> Option<String> {
    StreamCheckService::extract_base_url(provider, app_type)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

fn extract_claude_config(settings_config: &Value) -> ClaudeConfig {
    let env = settings_config
        .get("env")
        .and_then(|value| value.as_object());

    if let Some(env) = env {
        ClaudeConfig {
            api_key: env
                .get("ANTHROPIC_AUTH_TOKEN")
                .or_else(|| env.get("ANTHROPIC_API_KEY"))
                .and_then(|value| value.as_str())
                .map(mask_api_key),
            base_url: env
                .get("ANTHROPIC_BASE_URL")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            model: env
                .get("ANTHROPIC_MODEL")
                .and_then(|value| value.as_str())
                .map(simplify_model_name),
            haiku_model: env
                .get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
                .and_then(|value| value.as_str())
                .map(simplify_model_name),
            sonnet_model: env
                .get("ANTHROPIC_DEFAULT_SONNET_MODEL")
                .and_then(|value| value.as_str())
                .map(simplify_model_name),
            opus_model: env
                .get("ANTHROPIC_DEFAULT_OPUS_MODEL")
                .and_then(|value| value.as_str())
                .map(simplify_model_name),
        }
    } else {
        ClaudeConfig::default()
    }
}

fn mask_api_key(key: &str) -> String {
    if key.len() > 8 {
        format!("{}...", &key[..8])
    } else {
        key.to_string()
    }
}

fn simplify_model_name(name: &str) -> String {
    if let Some(pos) = name.rfind('-') {
        let suffix = &name[pos + 1..];
        if suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_digit()) {
            return name[..pos].to_string();
        }
    }
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{AuthBinding, AuthBindingSource, ProviderMeta};
    use crate::services::{ExtraUsage, QuotaTier, SubscriptionQuota};
    use serde_json::json;

    fn quota_output(result: ProviderUsageQuota) -> ProviderQuotaOutput {
        quota_output_from_result(
            AppType::Claude,
            "demo".to_string(),
            "Demo".to_string(),
            None,
            1_700_000_000_000,
            result,
        )
    }

    fn usage_item(plan_name: Option<&str>, remaining: Option<f64>) -> UsageData {
        UsageData {
            plan_name: plan_name.map(str::to_string),
            extra: None,
            is_valid: Some(true),
            invalid_message: None,
            total: None,
            used: None,
            remaining,
            unit: Some("USD".to_string()),
        }
    }

    fn subscription_quota(
        status: CredentialStatus,
        success: bool,
        tiers: Vec<QuotaTier>,
    ) -> SubscriptionQuota {
        SubscriptionQuota {
            tool: "claude".to_string(),
            credential_status: status,
            credential_message: None,
            success,
            tiers,
            extra_usage: None,
            error: None,
            queried_at: Some(1_700_000_000_000),
        }
    }

    #[test]
    fn provider_quota_text_shows_script_empty_success_as_not_available_only() {
        let output = quota_output(ProviderUsageQuota::Script(UsageResult {
            success: true,
            data: Some(vec![]),
            error: None,
        }));

        let lines = provider_quota_text_lines(&output);
        let joined = lines.join("\n");

        assert!(joined.contains(texts::tui_quota_not_available()));
        assert!(!joined.contains(texts::tui_quota_ok()));
        assert!(!joined.contains(texts::tui_quota_last_checked()));
    }

    #[test]
    fn provider_quota_text_hides_single_default_usage_plan_title() {
        let output = quota_output(ProviderUsageQuota::Script(UsageResult {
            success: true,
            data: Some(vec![usage_item(Some("default"), Some(2.0))]),
            error: None,
        }));

        let lines = provider_quota_text_lines(&output);
        let joined = lines.join("\n");

        assert!(joined.contains(&format!("{}: 2 USD", texts::tui_label_quota())));
        assert!(!joined.contains("Usage 1"));
        assert!(!joined.contains("default"));
    }

    #[test]
    fn provider_quota_text_labels_multiple_unnamed_usage_items() {
        let output = quota_output(ProviderUsageQuota::Script(UsageResult {
            success: true,
            data: Some(vec![
                usage_item(None, Some(2.0)),
                usage_item(None, Some(3.0)),
            ]),
            error: None,
        }));

        let lines = provider_quota_text_lines(&output);
        let joined = lines.join("\n");

        assert!(joined.contains(&format!(
            "{}: {}",
            texts::tui_label_quota(),
            texts::tui_quota_ok()
        )));
        assert!(joined.contains("Usage 1: 2 USD"));
        assert!(joined.contains("Usage 2: 3 USD"));
    }

    #[test]
    fn provider_quota_text_shows_script_failure_error() {
        let output = quota_output(ProviderUsageQuota::Script(UsageResult {
            success: false,
            data: None,
            error: Some("boom".to_string()),
        }));

        let lines = provider_quota_text_lines(&output);
        let joined = lines.join("\n");

        assert!(joined.contains(texts::tui_quota_query_failed()));
        assert!(joined.contains("Error: boom"));
    }

    #[test]
    fn provider_quota_text_maps_subscription_credential_states() {
        let not_found = provider_quota_text_lines(&quota_output(ProviderUsageQuota::Subscription(
            subscription_quota(CredentialStatus::NotFound, false, vec![]),
        )))
        .join("\n");
        assert!(not_found.contains(texts::tui_quota_not_available()));

        let parse_error = provider_quota_text_lines(&quota_output(
            ProviderUsageQuota::Subscription(SubscriptionQuota {
                credential_message: Some("bad file".to_string()),
                ..subscription_quota(CredentialStatus::ParseError, false, vec![])
            }),
        ))
        .join("\n");
        assert!(parse_error.contains(texts::tui_quota_parse_error()));
        assert!(parse_error.contains("Error: bad file"));

        let expired = provider_quota_text_lines(&quota_output(ProviderUsageQuota::Subscription(
            SubscriptionQuota {
                credential_message: Some("expired".to_string()),
                ..subscription_quota(CredentialStatus::Expired, false, vec![])
            },
        )))
        .join("\n");
        assert!(expired.contains(texts::tui_quota_expired()));
        assert!(expired.contains("Error: expired"));
    }

    #[test]
    fn provider_quota_text_prints_subscription_tiers_and_extra_usage() {
        let output = quota_output(ProviderUsageQuota::Subscription(SubscriptionQuota {
            extra_usage: Some(ExtraUsage {
                is_enabled: true,
                monthly_limit: Some(100.0),
                used_credits: Some(25.0),
                utilization: Some(25.0),
                currency: Some("USD".to_string()),
            }),
            ..subscription_quota(
                CredentialStatus::Valid,
                true,
                vec![QuotaTier {
                    name: "five_hour".to_string(),
                    utilization: 42.4,
                    resets_at: None,
                }],
            )
        }));

        let joined = provider_quota_text_lines(&output).join("\n");

        assert!(joined.contains(&format!(
            "{}: {}",
            texts::tui_label_quota(),
            texts::tui_quota_ok()
        )));
        assert!(joined.contains(&format!("{}: 42%", texts::tui_quota_tier_five_hour())));
        assert!(joined.contains(&format!(
            "{}: 25.0 / 100.0 USD (25%)",
            texts::tui_quota_extra_usage()
        )));
    }

    #[test]
    fn model_fetch_target_for_claude_uses_base_url_and_api_key() {
        let provider = Provider::with_id(
            "demo".to_string(),
            "Demo".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://claude.example.com",
                    "ANTHROPIC_API_KEY": "sk-claude"
                }
            }),
            None,
        );

        let target = model_fetch_target(&provider, &AppType::Claude)
            .expect("claude provider should resolve fetch target");

        assert_eq!(target.base_url, "https://claude.example.com");
        assert_eq!(target.auth_value.as_deref(), Some("sk-claude"));
        assert_eq!(target.strategy, ProviderModelFetchStrategy::Anthropic);
    }

    #[test]
    fn model_fetch_target_for_claude_supports_openrouter_bearer_mode() {
        let provider = Provider::with_id(
            "demo".to_string(),
            "Demo".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://openrouter.ai/api/v1",
                    "OPENROUTER_API_KEY": "sk-openrouter"
                }
            }),
            None,
        );

        let target = model_fetch_target(&provider, &AppType::Claude)
            .expect("openrouter provider should resolve fetch target");

        assert_eq!(target.strategy, ProviderModelFetchStrategy::Bearer);
        assert_eq!(target.auth_value.as_deref(), Some("sk-openrouter"));
    }

    #[test]
    fn model_fetch_source_for_claude_codex_oauth_uses_managed_auth_without_config_key() {
        let mut provider = Provider::with_id(
            "codex".to_string(),
            "Codex OAuth".to_string(),
            json!({ "env": {} }),
            None,
        );
        provider.meta = Some(ProviderMeta {
            provider_type: Some("codex_oauth".to_string()),
            ..Default::default()
        });

        let source = model_fetch_source(&provider, &AppType::Claude)
            .expect("codex oauth provider should use managed auth");

        assert_eq!(source, ModelFetchSource::CodexOAuth { account_id: None });
    }

    #[test]
    fn model_fetch_source_for_claude_codex_oauth_keeps_bound_account_id() {
        let mut provider = Provider::with_id(
            "codex".to_string(),
            "Codex OAuth".to_string(),
            json!({ "env": {} }),
            None,
        );
        provider.meta = Some(ProviderMeta {
            provider_type: Some("codex_oauth".to_string()),
            auth_binding: Some(AuthBinding {
                source: AuthBindingSource::ManagedAccount,
                auth_provider: Some("codex_oauth".to_string()),
                account_id: Some("acc-123".to_string()),
            }),
            ..Default::default()
        });

        let source = model_fetch_source(&provider, &AppType::Claude)
            .expect("codex oauth provider should use managed auth");

        assert_eq!(
            source,
            ModelFetchSource::CodexOAuth {
                account_id: Some("acc-123".to_string())
            }
        );
    }

    #[test]
    fn model_fetch_target_for_codex_supports_env_openai_key() {
        let provider = Provider::with_id(
            "demo".to_string(),
            "Demo".to_string(),
            json!({
                "env": {
                    "OPENAI_API_KEY": "sk-codex-env"
                },
                "config": "model_provider = \"demo\"\n\n[model_providers.demo]\nbase_url = \"https://codex.example.com/v1\"\n"
            }),
            None,
        );

        let target = model_fetch_target(&provider, &AppType::Codex)
            .expect("codex provider should resolve fetch target");

        assert_eq!(target.base_url, "https://codex.example.com/v1");
        assert_eq!(target.auth_value.as_deref(), Some("sk-codex-env"));
        assert_eq!(target.strategy, ProviderModelFetchStrategy::Bearer);
    }

    #[test]
    fn model_fetch_target_for_gemini_supports_access_token() {
        let provider = Provider::with_id(
            "demo".to_string(),
            "Demo".to_string(),
            json!({
                "env": {
                    "GOOGLE_GEMINI_BASE_URL": "https://generativelanguage.googleapis.com",
                    "GOOGLE_ACCESS_TOKEN": "ya29.token"
                }
            }),
            None,
        );

        let target = model_fetch_target(&provider, &AppType::Gemini)
            .expect("gemini provider should resolve oauth fetch target");

        assert_eq!(target.auth_value.as_deref(), Some("ya29.token"));
        assert_eq!(target.strategy, ProviderModelFetchStrategy::Bearer);
    }

    #[test]
    fn one_off_model_fetch_target_defaults_strategy_by_app_and_trims_input() {
        let target = one_off_model_fetch_target(
            &AppType::Gemini,
            Some(" https://gemini.example.com/ "),
            Some(" sk-gemini "),
            None,
        )
        .expect("one-off target should be built");

        assert_eq!(target.base_url, "https://gemini.example.com");
        assert_eq!(target.auth_value.as_deref(), Some("sk-gemini"));
        assert_eq!(target.strategy, ProviderModelFetchStrategy::GoogleApiKey);
    }

    #[test]
    fn one_off_model_fetch_target_allows_auth_override_and_optional_key() {
        let target = one_off_model_fetch_target(
            &AppType::Claude,
            Some("https://openrouter.ai/api/v1"),
            None,
            Some(ProviderModelFetchStrategy::Bearer),
        )
        .expect("one-off target should be built");

        assert_eq!(target.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(target.auth_value, None);
        assert_eq!(target.strategy, ProviderModelFetchStrategy::Bearer);
    }

    #[test]
    fn model_fetch_target_rejects_empty_base_url() {
        let provider = Provider::with_id(
            "demo".to_string(),
            "Demo".to_string(),
            json!({
                "options": {
                    "baseURL": "",
                    "apiKey": "sk-opencode"
                }
            }),
            None,
        );

        let err = model_fetch_target(&provider, &AppType::OpenCode)
            .expect_err("empty base url should be rejected");

        assert!(err.to_string().contains("No API URL configured"));
    }
}
