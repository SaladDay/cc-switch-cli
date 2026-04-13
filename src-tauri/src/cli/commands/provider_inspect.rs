use serde_json::Value;

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::cli::ui::{create_table, error, highlight, info, success, warning};
use crate::error::AppError;
use crate::provider::Provider;
use crate::services::{ProviderService, SpeedtestService, StreamCheckService};
use crate::store::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderModelFetchStrategy {
    Bearer,
    Anthropic,
    GoogleApiKey,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelFetchTarget {
    base_url: String,
    auth_value: String,
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

pub(crate) fn usage_provider(app_type: AppType, id: &str) -> Result<(), AppError> {
    let state = get_state()?;
    let providers = ProviderService::list(&state, app_type.clone())?;
    let provider = providers
        .get(id)
        .ok_or_else(|| AppError::Message(format!("Provider '{}' not found", id)))?;

    println!(
        "{}",
        info(&format!(
            "Querying usage for '{}' (usage script from CC-Switch / provider meta)...",
            provider.name
        ))
    );
    println!();

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| AppError::Message(format!("Failed to create async runtime: {}", e)))?;

    let result = runtime.block_on(async { ProviderService::query_usage(&state, app_type, id).await })?;

    println!("{}", highlight("Usage"));
    println!("{}", "═".repeat(60));

    if !result.success {
        if let Some(msg) = result.error {
            println!("\n{}", error(&msg));
        } else {
            println!("\n{}", error("Usage query failed."));
        }
        return Ok(());
    }

    let rows = match result.data {
        Some(v) if !v.is_empty() => v,
        _ => {
            println!("\n{}", info("No usage rows returned."));
            return Ok(());
        }
    };

    let mut table = create_table();
    table.set_header(vec![
        "Plan",
        "Extra",
        "Valid",
        "Total",
        "Used",
        "Remaining",
        "Unit",
        "Note",
    ]);

    fn fmt_num(v: Option<f64>) -> String {
        v.map(|x| format!("{x}"))
            .unwrap_or_else(|| "—".to_string())
    }

    fn fmt_valid(v: Option<bool>) -> String {
        match v {
            Some(true) => "Y".to_string(),
            Some(false) => "N".to_string(),
            None => "—".to_string(),
        }
    }

    for row in rows {
        let note = row
            .invalid_message
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("—");
        table.add_row(vec![
            row.plan_name.unwrap_or_else(|| "—".to_string()),
            row.extra.unwrap_or_else(|| "—".to_string()),
            fmt_valid(row.is_valid),
            fmt_num(row.total),
            fmt_num(row.used),
            fmt_num(row.remaining),
            row.unit.unwrap_or_else(|| "—".to_string()),
            note.to_string(),
        ]);
    }

    println!("\n{}", table);
    println!();
    println!("{}", success("Usage query completed."));

    Ok(())
}

pub(crate) fn fetch_models_provider(app_type: AppType, id: &str) -> Result<(), AppError> {
    let state = get_state()?;
    let providers = ProviderService::list(&state, app_type.clone())?;
    let provider = providers
        .get(id)
        .ok_or_else(|| AppError::Message(format!("Provider '{}' not found", id)))?;
    let target = model_fetch_target(provider, &app_type)?;

    println!(
        "{}",
        info(&format!("Fetching models for '{}'...", provider.name))
    );
    println!("{}", info(&format!("Endpoint: {}", target.base_url)));
    println!();

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| AppError::Message(format!("Failed to create async runtime: {}", e)))?;

    let models = runtime.block_on(async {
        crate::cli::tui::fetch_provider_models_for_tui(
            &target.base_url,
            Some(target.auth_value.as_str()),
            to_tui_strategy(target.strategy),
        )
        .await
        .map_err(AppError::Message)
    })?;

    if models.is_empty() {
        println!("{}", info("No models returned."));
        return Ok(());
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

    Ok(())
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
                auth_value,
                strategy,
            })
        }
        AppType::Codex => Ok(ModelFetchTarget {
            base_url,
            auth_value: StreamCheckService::extract_codex_key(provider).ok_or_else(|| {
                AppError::Message(format!("Missing API key for provider '{}'", provider.id))
            })?,
            strategy: ProviderModelFetchStrategy::Bearer,
        }),
        AppType::Gemini => {
            let (auth_value, strategy) = extract_gemini_model_fetch_auth(provider)?;
            Ok(ModelFetchTarget {
                base_url,
                auth_value,
                strategy,
            })
        }
        AppType::OpenCode => Ok(ModelFetchTarget {
            base_url,
            auth_value: provider
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
            strategy: ProviderModelFetchStrategy::Bearer,
        }),
        AppType::OpenClaw => Ok(ModelFetchTarget {
            base_url,
            auth_value: provider
                .settings_config
                .get("apiKey")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .ok_or_else(|| {
                    AppError::Message(format!("Missing API key for provider '{}'", provider.id))
                })?,
            strategy: ProviderModelFetchStrategy::Bearer,
        }),
    }
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
    use serde_json::json;

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
        assert_eq!(target.auth_value, "sk-claude");
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
        assert_eq!(target.auth_value, "sk-openrouter");
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
        assert_eq!(target.auth_value, "sk-codex-env");
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

        assert_eq!(target.auth_value, "ya29.token");
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
