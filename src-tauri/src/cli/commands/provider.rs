use clap::Subcommand;
use std::path::PathBuf;

use super::provider_inspect;
use crate::app_config::AppType;
use crate::cli::commands::provider_input::{
    current_timestamp, display_provider_summary, generate_provider_id, prompt_basic_fields,
    prompt_optional_fields, prompt_settings_config, prompt_settings_config_for_add, OptionalFields,
    ProviderAddMode,
};
use crate::cli::i18n::texts;
use crate::cli::ui::{error, highlight, info, success, warning};
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta};
use crate::services::ProviderService;
use crate::store::AppState;
use inquire::{Confirm, Select, Text};

fn supports_official_provider(app_type: &AppType) -> bool {
    matches!(app_type, AppType::Codex)
}

fn is_codex_official_provider(provider: &Provider) -> bool {
    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.codex_official)
        .unwrap_or(false)
        || provider.category.as_deref() == Some("official")
        || provider.website_url.as_deref() == Some("https://chatgpt.com/codex")
        || provider.name.trim().eq_ignore_ascii_case("OpenAI Official")
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
    /// Run stream health check for a provider
    StreamCheck {
        /// Provider ID to check
        id: String,
    },
    /// Fetch remote model list for a provider
    FetchModels {
        /// Provider ID to query
        id: String,
    },
    /// Query usage for a provider (requires usage script configured in provider meta)
    Usage {
        /// Provider ID to query
        id: String,
    },
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
        ProviderCommand::Add => add_provider(app_type),
        ProviderCommand::Edit { id } => edit_provider(app_type, &id),
        ProviderCommand::Delete { id } => delete_provider(app_type, &id),
        ProviderCommand::Duplicate { id } => duplicate_provider(app_type, &id),
        ProviderCommand::Speedtest { id } => provider_inspect::speedtest_provider(app_type, &id),
        ProviderCommand::StreamCheck { id } => {
            provider_inspect::stream_check_provider(app_type, &id)
        }
        ProviderCommand::FetchModels { id } => {
            provider_inspect::fetch_models_provider(app_type, &id)
        }
        ProviderCommand::Usage { id } => provider_inspect::usage_provider(app_type, &id),
        ProviderCommand::Export { id, output } => export_provider(app_type, &id, output),
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
        prompt_settings_config(
            &app_type,
            Some(&original.settings_config),
            matches!(app_type, AppType::Codex) && is_codex_official_provider(&original),
        )?
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

    let apply_common_config = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.apply_common_config)
        .unwrap_or(true);

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
