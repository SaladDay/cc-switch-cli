use inquire::{Confirm, Select};

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::cli::ui::{create_table, error, highlight, info, success};
use crate::error::AppError;
use crate::services::{ProviderService, SpeedtestService};
use crate::store::AppState;

use super::utils::{get_state, pause};

pub fn manage_providers_menu(app_type: &AppType) -> Result<(), AppError> {
    loop {
        println!("\n{}", highlight(texts::provider_management()));
        println!("{}", "─".repeat(60));

        let state = get_state()?;
        let providers = ProviderService::list(&state, app_type.clone())?;
        let current_id = ProviderService::current(&state, app_type.clone())?;

        if providers.is_empty() {
            println!("{}", info(texts::no_providers()));
        } else {
            let mut table = create_table();
            table.set_header(vec!["", texts::header_name(), "API URL"]);

            let mut provider_list: Vec<_> = providers.iter().collect();
            provider_list.sort_by(|(_, a), (_, b)| match (a.sort_index, b.sort_index) {
                (Some(idx_a), Some(idx_b)) => idx_a.cmp(&idx_b),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.created_at.cmp(&b.created_at),
            });

            for (id, provider) in &provider_list {
                let marker = if *id == &current_id { "✓" } else { " " };
                let name = if *id == &current_id {
                    format!("* {}", provider.name)
                } else {
                    format!("  {}", provider.name)
                };
                let api_url = extract_api_url(&provider.settings_config, app_type)
                    .unwrap_or_else(|| "N/A".to_string());

                table.add_row(vec![marker.to_string(), name, api_url]);
            }

            println!("{}", table);
        }

        println!();
        let choices = vec![
            texts::view_current_provider(),
            texts::switch_provider(),
            texts::add_provider(),
            texts::edit_provider_menu(),
            texts::delete_provider(),
            texts::back_to_main(),
        ];

        let choice = Select::new(texts::choose_action(), choices)
            .prompt()
            .map_err(|_| AppError::Message("Selection cancelled".to_string()))?;

        if choice == texts::view_current_provider() {
            view_provider_detail(&state, app_type, &current_id)?;
        } else if choice == texts::switch_provider() {
            switch_provider_interactive(&state, app_type, &providers, &current_id)?;
        } else if choice == texts::add_provider() {
            add_provider_interactive(app_type)?;
        } else if choice == texts::edit_provider_menu() {
            edit_provider_interactive(app_type, &providers)?;
        } else if choice == texts::delete_provider() {
            delete_provider_interactive(&state, app_type, &providers, &current_id)?;
        } else {
            break;
        }
    }

    Ok(())
}

fn view_provider_detail(
    state: &AppState,
    app_type: &AppType,
    current_id: &str,
) -> Result<(), AppError> {
    loop {
        let providers = ProviderService::list(state, app_type.clone())?;
        if let Some(provider) = providers.get(current_id) {
            println!("\n{}", highlight(texts::current_provider_details()));
            println!("{}", "═".repeat(60));

            // 基本信息
            println!("\n{}", highlight(texts::basic_info_section_header()));
            println!("  ID:       {}", current_id);
            println!("  {}:     {}", texts::name_label_with_colon(), provider.name);
            println!("  {}:     {}", texts::app_label_with_colon(), app_type.as_str());

            // 仅 Claude 应用显示详细配置
            if matches!(app_type, AppType::Claude) {
                let config = extract_claude_config(&provider.settings_config);

                // API 配置
                println!("\n{}", highlight(texts::api_config_section_header()));
                println!(
                    "  Base URL: {}",
                    config.base_url.unwrap_or_else(|| "N/A".to_string())
                );
                println!(
                    "  API Key:  {}",
                    config.api_key.unwrap_or_else(|| "N/A".to_string())
                );

                // 模型配置
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
                // Codex/Gemini 应用只显示 API URL
                println!("\n{}", highlight(texts::api_config_section_header()));
                let api_url = extract_api_url(&provider.settings_config, app_type)
                    .unwrap_or_else(|| "N/A".to_string());
                println!("  API URL:  {}", api_url);
            }

            println!("\n{}", "─".repeat(60));

            // Show action menu
            println!();
            let choices = vec![texts::speedtest_endpoint(), texts::back()];
            let choice = Select::new(texts::choose_action(), choices)
                .prompt()
                .map_err(|_| AppError::Message("Selection cancelled".to_string()))?;

            if choice == texts::speedtest_endpoint() {
                speedtest_provider_interactive(state, app_type, current_id, provider)?;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    Ok(())
}

fn speedtest_provider_interactive(
    _state: &AppState,
    app_type: &AppType,
    _provider_id: &str,
    provider: &crate::provider::Provider,
) -> Result<(), AppError> {
    // Extract API URL
    let api_url = extract_api_url(&provider.settings_config, app_type);

    if api_url.is_none() {
        println!("\n{}", error("No API URL configured for this provider"));
        pause();
        return Ok(());
    }

    let api_url = api_url.unwrap();

    println!("\n{}", info(&format!("Testing provider '{}'...", provider.name)));
    println!("{}", info(&format!("Endpoint: {}", api_url)));
    println!();

    // Run speedtest asynchronously
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| AppError::Message(format!("Failed to create async runtime: {}", e)))?;

    let results = runtime.block_on(async {
        SpeedtestService::test_endpoints(vec![api_url.clone()], None).await
    })?;

    // Display results
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

        let status_str = result.status
            .map(|s| s.to_string())
            .unwrap_or_else(|| "N/A".to_string());

        table.add_row(vec![
            result.url.clone(),
            latency_str,
            status_str,
        ]);

        println!("{}", table);

        // Show error details if any
        if let Some(err) = &result.error {
            println!("\n{}", error(&format!("Error: {}", err)));
        } else if result.latency.is_some() {
            println!("\n{}", success("✓ Speedtest completed successfully"));
        }
    }

    pause();
    Ok(())
}

pub fn extract_api_url(settings_config: &serde_json::Value, app_type: &AppType) -> Option<String> {
    match app_type {
        AppType::Claude => settings_config
            .get("env")?
            .get("ANTHROPIC_BASE_URL")?
            .as_str()
            .map(|s| s.to_string()),
        AppType::Codex => {
            if let Some(config_str) = settings_config.get("config")?.as_str() {
                for line in config_str.lines() {
                    let line = line.trim();
                    if line.starts_with("base_url") {
                        if let Some(url_part) = line.split('=').nth(1) {
                            let url = url_part.trim().trim_matches('"').trim_matches('\'');
                            return Some(url.to_string());
                        }
                    }
                }
            }
            None
        }
        AppType::Gemini => settings_config
            .get("env")?
            .get("GEMINI_BASE_URL")
            .or_else(|| settings_config.get("env")?.get("BASE_URL"))?
            .as_str()
            .map(|s| s.to_string()),
    }
}

fn switch_provider_interactive(
    state: &AppState,
    app_type: &AppType,
    providers: &std::collections::HashMap<String, crate::provider::Provider>,
    current_id: &str,
) -> Result<(), AppError> {
    if providers.len() <= 1 {
        println!("\n{}", info(texts::only_one_provider()));
        pause();
        return Ok(());
    }

    let mut provider_choices: Vec<_> = providers
        .iter()
        .filter(|(id, _)| *id != current_id)
        .map(|(id, p)| format!("{} ({})", p.name, id))
        .collect();
    provider_choices.sort();

    if provider_choices.is_empty() {
        println!("\n{}", info(texts::no_other_providers()));
        pause();
        return Ok(());
    }

    let choice = Select::new(texts::select_provider_to_switch(), provider_choices)
        .prompt()
        .map_err(|_| AppError::Message("Selection cancelled".to_string()))?;

    let id = choice
        .split('(')
        .nth(1)
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| AppError::Message("Invalid choice".to_string()))?;

    ProviderService::switch(state, app_type.clone(), id)?;

    println!("\n{}", success(&texts::switched_to_provider(id)));
    println!("{}", info(texts::restart_note()));
    pause();

    Ok(())
}

fn delete_provider_interactive(
    state: &AppState,
    app_type: &AppType,
    providers: &std::collections::HashMap<String, crate::provider::Provider>,
    current_id: &str,
) -> Result<(), AppError> {
    let deletable: Vec<_> = providers
        .iter()
        .filter(|(id, _)| *id != current_id)
        .map(|(id, p)| format!("{} ({})", p.name, id))
        .collect();

    if deletable.is_empty() {
        println!("\n{}", info(texts::no_deletable_providers()));
        pause();
        return Ok(());
    }

    let choice = Select::new(texts::select_provider_to_delete(), deletable)
        .prompt()
        .map_err(|_| AppError::Message("Selection cancelled".to_string()))?;

    let id = choice
        .split('(')
        .nth(1)
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| AppError::Message("Invalid choice".to_string()))?;

    let confirm = Confirm::new(&texts::confirm_delete(id))
        .with_default(false)
        .prompt()
        .map_err(|_| AppError::Message("Confirmation failed".to_string()))?;

    if !confirm {
        println!("\n{}", info(texts::cancelled()));
        pause();
        return Ok(());
    }

    ProviderService::delete(state, app_type.clone(), id)?;
    println!("\n{}", success(&texts::deleted_provider(id)));
    pause();

    Ok(())
}

fn add_provider_interactive(app_type: &AppType) -> Result<(), AppError> {
    // 调用命令层的实现
    crate::cli::commands::provider::execute(
        crate::cli::commands::provider::ProviderCommand::Add,
        Some(app_type.clone()),
    )?;

    pause();
    Ok(())
}

fn edit_provider_interactive(
    app_type: &AppType,
    providers: &std::collections::HashMap<String, crate::provider::Provider>,
) -> Result<(), AppError> {
    if providers.is_empty() {
        println!("{}", error(texts::no_editable_providers()));
        pause();
        return Ok(());
    }

    // 1. 显示供应商列表让用户选择
    let mut provider_list: Vec<_> = providers.iter().collect();
    provider_list.sort_by(|(_, a), (_, b)| match (a.sort_index, b.sort_index) {
        (Some(idx_a), Some(idx_b)) => idx_a.cmp(&idx_b),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.created_at.cmp(&b.created_at),
    });

    // 2. 使用 ID 列表配对，避免字符串匹配的潜在 bug
    let choices: Vec<String> = provider_list
        .iter()
        .map(|(id, provider)| format!("{} ({})", provider.name, id))
        .collect();

    let selection = Select::new(texts::select_provider_to_edit(), choices)
        .prompt()
        .map_err(|_| AppError::Message("Selection cancelled".to_string()))?;

    // 从 "Name (id)" 格式中提取 ID
    let selected_id = selection
        .rsplit_once('(')  // 从右边分割，找到最后一个 '('
        .and_then(|(_, id_part)| id_part.strip_suffix(')'))  // 移除末尾的 ')'
        .ok_or_else(|| AppError::Message(texts::invalid_selection_format().to_string()))?
        .to_string();

    // 3. 调用命令层的实现
    crate::cli::commands::provider::execute(
        crate::cli::commands::provider::ProviderCommand::Edit {
            id: selected_id,
        },
        Some(app_type.clone()),
    )?;

    pause();
    Ok(())
}

/// Claude 配置信息
#[derive(Default)]
struct ClaudeConfig {
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
    haiku_model: Option<String>,
    sonnet_model: Option<String>,
    opus_model: Option<String>,
}

/// 提取 Claude 配置信息
fn extract_claude_config(settings_config: &serde_json::Value) -> ClaudeConfig {
    let env = settings_config.get("env").and_then(|v| v.as_object());

    if let Some(env) = env {
        ClaudeConfig {
            api_key: env
                .get("ANTHROPIC_AUTH_TOKEN")
                .or_else(|| env.get("ANTHROPIC_API_KEY"))
                .and_then(|v| v.as_str())
                .map(mask_api_key),
            base_url: env
                .get("ANTHROPIC_BASE_URL")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            model: env
                .get("ANTHROPIC_MODEL")
                .and_then(|v| v.as_str())
                .map(simplify_model_name),
            haiku_model: env
                .get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
                .and_then(|v| v.as_str())
                .map(simplify_model_name),
            sonnet_model: env
                .get("ANTHROPIC_DEFAULT_SONNET_MODEL")
                .and_then(|v| v.as_str())
                .map(simplify_model_name),
            opus_model: env
                .get("ANTHROPIC_DEFAULT_OPUS_MODEL")
                .and_then(|v| v.as_str())
                .map(simplify_model_name),
        }
    } else {
        ClaudeConfig::default()
    }
}

/// 将 API Key 脱敏显示（显示前8位 + ...）
fn mask_api_key(key: &str) -> String {
    if key.len() > 8 {
        format!("{}...", &key[..8])
    } else {
        key.to_string()
    }
}

/// 简化模型名称（去掉日期后缀）
/// 例如：claude-3-5-sonnet-20241022 -> claude-3-5-sonnet
fn simplify_model_name(name: &str) -> String {
    // 移除末尾的日期格式（8位数字）
    if let Some(pos) = name.rfind('-') {
        let suffix = &name[pos + 1..];
        if suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_digit()) {
            return name[..pos].to_string();
        }
    }
    name.to_string()
}
