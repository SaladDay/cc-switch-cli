use clap::Subcommand;

use crate::app_config::AppType;
use crate::cli::commands::provider_input::{
    current_timestamp, display_provider_summary, generate_provider_id, prompt_basic_fields,
    prompt_optional_fields, prompt_settings_config, prompt_settings_config_for_add, OptionalFields,
    ProviderAddMode,
};
use crate::cli::i18n::texts;
use crate::cli::ui::{create_table, error, highlight, info, success, warning};
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta};
use crate::services::{ProviderService, SpeedtestService};
use crate::store::AppState;
use inquire::{Confirm, Select, Text};

fn supports_official_provider(app_type: &AppType) -> bool {
    matches!(app_type, AppType::Codex)
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
    AppState::try_new()
}

fn list_providers(app_type: AppType) -> Result<(), AppError> {
    let state = get_state()?;
    let app_str = app_type.as_str().to_string();
    let providers = ProviderService::list(&state, app_type.clone())?;
    let current_id = ProviderService::current(&state, app_type.clone())?;

    if providers.is_empty() {
        println!("{}", info("No providers found."));
        println!("{}", texts::no_providers_hint());
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
    let skip_live_sync = !crate::sync_policy::should_sync_live(&app_type);

    // 检查 provider 是否存在
    let providers = ProviderService::list(&state, app_type.clone())?;
    if !providers.contains_key(id) {
        return Err(AppError::Message(format!("Provider '{}' not found", id)));
    }

    // 执行切换
    ProviderService::switch(&state, app_type, id)?;

    println!("{}", success(&format!("✓ Switched to provider '{}'", id)));
    println!("{}", info(&format!("  Application: {}", app_str)));
    if skip_live_sync {
        println!(
            "{}",
            warning(&texts::live_sync_skipped_uninitialized_warning(&app_str))
        );
    }
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

fn add_provider(app_type: AppType) -> Result<(), AppError> {
    // Disable bracketed paste mode to work around inquire dropping paste events
    crate::cli::terminal::disable_bracketed_paste_mode_best_effort();

    println!("{}", highlight("Add New Provider"));
    println!("{}", "=".repeat(50));

    let add_mode = if supports_official_provider(&app_type) {
        let choices = vec![
            texts::add_official_provider(),
            texts::add_third_party_provider(),
        ];
        match Select::new(texts::select_provider_add_mode(), choices.clone()).prompt() {
            Ok(selected) if selected == texts::add_official_provider() => ProviderAddMode::Official,
            Ok(_selected) => ProviderAddMode::ThirdParty,
            Err(inquire::error::InquireError::OperationCanceled)
            | Err(inquire::error::InquireError::OperationInterrupted) => {
                println!("{}", info(texts::cancelled()));
                return Ok(());
            }
            Err(e) => {
                return Err(AppError::Message(texts::input_failed_error(&e.to_string())));
            }
        }
    } else {
        ProviderAddMode::ThirdParty
    };

    // 1. 加载配置和状态
    let state = AppState::try_new()?;
    let config = state.config.read().unwrap();
    let manager = config
        .get_manager(&app_type)
        .ok_or_else(|| AppError::Message(texts::app_config_not_found(app_type.as_str())))?;
    let existing_ids: Vec<String> = manager.providers.keys().cloned().collect();
    drop(config);

    // 2. 收集基本字段
    let is_codex_official = matches!(
        (app_type.clone(), add_mode),
        (AppType::Codex, ProviderAddMode::Official)
    );
    let (name, website_url) = match (app_type.clone(), add_mode) {
        (AppType::Codex, ProviderAddMode::Official) => {
            let name = Text::new(texts::provider_name_label())
                .with_placeholder("OpenAI")
                .with_help_message(texts::provider_name_help())
                .prompt()
                .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err(AppError::InvalidInput(
                    texts::provider_name_empty_error().to_string(),
                ));
            }
            (name, Some("https://chatgpt.com/codex".to_string()))
        }
        _ => prompt_basic_fields(None)?,
    };
    let id = generate_provider_id(&name, &existing_ids);
    println!("{}", info(&texts::generated_id_message(&id)));

    // 3. 收集配置
    let settings_config = prompt_settings_config_for_add(&app_type, add_mode)?;

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

    // 5. 构建 Provider 对象
    let provider = Provider {
        id: id.clone(),
        name,
        settings_config,
        website_url,
        category: None,
        created_at: Some(current_timestamp()),
        sort_index: optional.sort_index,
        notes: optional.notes,
        icon: None,
        icon_color: None,
        meta: if is_codex_official {
            Some(ProviderMeta {
                codex_official: Some(true),
                ..Default::default()
            })
        } else {
            None
        },
        in_failover_queue: false,
    };

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
    ProviderService::add(&state, app_type.clone(), provider)?;

    // 8. 成功消息
    println!(
        "\n{}",
        success(&texts::entity_added_success(texts::entity_provider(), &id))
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_official_provider_is_codex_only() {
        assert!(supports_official_provider(&AppType::Codex));
        assert!(!supports_official_provider(&AppType::Claude));
        assert!(!supports_official_provider(&AppType::Gemini));
    }
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
    let settings_config = if Confirm::new(texts::modify_provider_config_prompt())
        .with_default(false)
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    {
        prompt_settings_config(&app_type, Some(&original.settings_config))?
    } else {
        original.settings_config.clone()
    };

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
    let updated = Provider {
        id: id.to_string(),
        name: name.trim().to_string(),
        settings_config,
        website_url,
        category: None,
        created_at: original.created_at,
        sort_index: optional.sort_index,
        notes: optional.notes,
        icon: None,
        icon_color: None,
        meta: original.meta,                           // 保留元数据
        in_failover_queue: original.in_failover_queue, // 保留故障转移状态
    };

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

fn duplicate_provider(_app_type: AppType, id: &str) -> Result<(), AppError> {
    println!("{}", info(&format!("Duplicating provider '{}'...", id)));
    println!("{}", error("Provider duplication is not yet implemented."));
    Ok(())
}

fn speedtest_provider(app_type: AppType, id: &str) -> Result<(), AppError> {
    let state = get_state()?;

    // Get provider by ID
    let providers = ProviderService::list(&state, app_type.clone())?;
    let provider = providers
        .get(id)
        .ok_or_else(|| AppError::Message(format!("Provider '{}' not found", id)))?;

    // Extract API URL
    let api_url = extract_api_url(&provider.settings_config, &app_type)
        .ok_or_else(|| AppError::Message(format!("No API URL configured for provider '{}'", id)))?;

    println!(
        "{}",
        info(&format!("Testing provider '{}'...", provider.name))
    );
    println!("{}", info(&format!("Endpoint: {}", api_url)));
    println!();

    // Run speedtest asynchronously
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| AppError::Message(format!("Failed to create async runtime: {}", e)))?;

    let results = runtime
        .block_on(async { SpeedtestService::test_endpoints(vec![api_url.clone()], None).await })?;

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

        let status_str = result
            .status
            .map(|s| s.to_string())
            .unwrap_or_else(|| "N/A".to_string());

        table.add_row(vec![result.url.clone(), latency_str, status_str]);

        println!("{}", table);

        // Show error details if any
        if let Some(err) = &result.error {
            println!("\n{}", error(&format!("Error: {}", err)));
        } else if result.latency.is_some() {
            println!("\n{}", success("✓ Speedtest completed successfully"));
        }
    }

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
