use clap::Subcommand;
use std::sync::RwLock;

use crate::app_config::{AppType, MultiAppConfig};
use crate::cli::ui::{create_table, error, highlight, info, success};
use crate::error::AppError;
use crate::services::ProviderService;
use crate::store::AppState;

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
    /// Add a new provider (interactive)
    Add,
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
    },
    /// Test provider endpoint speed
    Speedtest {
        /// Provider ID to test
        id: String,
    },
}

pub fn execute(cmd: ProviderCommand, app: Option<AppType>) -> Result<(), AppError> {
    let app_type = app.unwrap_or(AppType::Claude);

    match cmd {
        ProviderCommand::List => list_providers(app_type),
        ProviderCommand::Current => show_current(app_type),
        ProviderCommand::Switch { id } => switch_provider(app_type, &id),
        ProviderCommand::Add => add_provider(app_type),
        ProviderCommand::Edit { id } => edit_provider(app_type, &id),
        ProviderCommand::Delete { id } => delete_provider(app_type, &id),
        ProviderCommand::Duplicate { id } => duplicate_provider(app_type, &id),
        ProviderCommand::Speedtest { id } => speedtest_provider(app_type, &id),
    }
}

fn get_state() -> Result<AppState, AppError> {
    let config = MultiAppConfig::load()?;
    Ok(AppState {
        config: RwLock::new(config),
    })
}

fn list_providers(app_type: AppType) -> Result<(), AppError> {
    let state = get_state()?;
    let app_str = app_type.as_str().to_string();
    let providers = ProviderService::list(&state, app_type.clone())?;
    let current_id = ProviderService::current(&state, app_type.clone())?;

    if providers.is_empty() {
        println!("{}", info("No providers found."));
        println!("Use 'cc-switch provider add' to create a new provider.");
        return Ok(());
    }

    // 创建表格
    let mut table = create_table();
    table.set_header(vec!["", "ID", "Name", "API URL"]);

    // 按创建时间排序
    let mut provider_list: Vec<_> = providers.into_iter().collect();
    provider_list.sort_by(|(_, a), (_, b)| {
        // 先按 sort_index，再按创建时间
        match (a.sort_index, b.sort_index) {
            (Some(idx_a), Some(idx_b)) => idx_a.cmp(&idx_b),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.created_at.cmp(&b.created_at),
        }
    });

    for (id, provider) in provider_list {
        let current_marker = if id == current_id { "✓" } else { " " };
        let api_url = extract_api_url(&provider.settings_config, &app_type)
            .unwrap_or_else(|| "N/A".to_string());

        table.add_row(vec![
            current_marker.to_string(),
            id.clone(),
            provider.name.clone(),
            api_url,
        ]);
    }

    println!("{}", table);
    println!("\n{} Application: {}", info("ℹ"), app_str);
    println!("{} Current: {}", info("→"), highlight(&current_id));

    Ok(())
}

fn show_current(app_type: AppType) -> Result<(), AppError> {
    let state = get_state()?;
    let current_id = ProviderService::current(&state, app_type.clone())?;
    let providers = ProviderService::list(&state, app_type.clone())?;

    let provider = providers
        .get(&current_id)
        .ok_or_else(|| AppError::Message(format!("Current provider '{}' not found", current_id)))?;

    println!("{}", highlight("Current Provider"));
    println!("{}", "═".repeat(60));

    // 基本信息
    println!("\n{}", highlight("基本信息 / Basic Info"));
    println!("  ID:       {}", current_id);
    println!("  名称:     {}", provider.name);
    println!("  应用:     {}", app_type.as_str());

    // 仅 Claude 应用显示详细配置
    if matches!(app_type, AppType::Claude) {
        let config = extract_claude_config(&provider.settings_config);

        // API 配置
        println!("\n{}", highlight("API 配置 / API Configuration"));
        println!(
            "  Base URL: {}",
            config.base_url.unwrap_or_else(|| "N/A".to_string())
        );
        println!(
            "  API Key:  {}",
            config.api_key.unwrap_or_else(|| "N/A".to_string())
        );

        // 模型配置
        println!("\n{}", highlight("模型配置 / Model Configuration"));
        println!(
            "  主模型:   {}",
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
        println!("\n{}", highlight("API 配置 / API Configuration"));
        let api_url = extract_api_url(&provider.settings_config, &app_type)
            .unwrap_or_else(|| "N/A".to_string());
        println!("  API URL:  {}", api_url);
    }

    println!("\n{}", "─".repeat(60));

    Ok(())
}

fn switch_provider(app_type: AppType, id: &str) -> Result<(), AppError> {
    let state = get_state()?;
    let app_str = app_type.as_str().to_string();

    // 检查 provider 是否存在
    let providers = ProviderService::list(&state, app_type.clone())?;
    if !providers.contains_key(id) {
        return Err(AppError::Message(format!("Provider '{}' not found", id)));
    }

    // 执行切换
    ProviderService::switch(&state, app_type, id)?;

    println!("{}", success(&format!("✓ Switched to provider '{}'", id)));
    println!("{}", info(&format!("  Application: {}", app_str)));
    println!(
        "\n{}",
        info("Note: Restart your CLI client to apply the changes.")
    );

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

fn add_provider(_app_type: AppType) -> Result<(), AppError> {
    println!("{}", highlight("Add New Provider"));
    println!("{}", "=".repeat(50));

    // 交互式输入
    let _name = inquire::Text::new("Provider name:")
        .prompt()
        .map_err(|e| AppError::Message(format!("Prompt failed: {}", e)))?;

    println!(
        "\n{}",
        info("Note: Provider configuration is complex and varies by app type.")
    );
    println!(
        "{}",
        info("For now, please use the config file directly to add detailed settings.")
    );
    println!(
        "\n{}",
        error("Interactive provider creation is not yet fully implemented.")
    );
    println!("{}", info("Coming soon in the next update!"));

    Ok(())
}

fn edit_provider(_app_type: AppType, id: &str) -> Result<(), AppError> {
    println!("{}", info(&format!("Editing provider '{}'...", id)));
    println!("{}", error("Provider editing is not yet implemented."));
    println!(
        "{}",
        info("Please edit ~/.cc-switch/config.json directly for now.")
    );
    Ok(())
}

fn duplicate_provider(_app_type: AppType, id: &str) -> Result<(), AppError> {
    println!("{}", info(&format!("Duplicating provider '{}'...", id)));
    println!("{}", error("Provider duplication is not yet implemented."));
    Ok(())
}

fn speedtest_provider(_app_type: AppType, id: &str) -> Result<(), AppError> {
    println!("{}", info(&format!("Testing provider '{}'...", id)));
    println!("{}", error("Speedtest is not yet implemented."));
    Ok(())
}

fn extract_api_url(settings_config: &serde_json::Value, app_type: &AppType) -> Option<String> {
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
                .map(|s| mask_api_key(s)),
            base_url: env
                .get("ANTHROPIC_BASE_URL")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            model: env
                .get("ANTHROPIC_MODEL")
                .and_then(|v| v.as_str())
                .map(|s| simplify_model_name(s)),
            haiku_model: env
                .get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
                .and_then(|v| v.as_str())
                .map(|s| simplify_model_name(s)),
            sonnet_model: env
                .get("ANTHROPIC_DEFAULT_SONNET_MODEL")
                .and_then(|v| v.as_str())
                .map(|s| simplify_model_name(s)),
            opus_model: env
                .get("ANTHROPIC_DEFAULT_OPUS_MODEL")
                .and_then(|v| v.as_str())
                .map(|s| simplify_model_name(s)),
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
