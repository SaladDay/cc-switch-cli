use indexmap::IndexMap;

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::cli::ui::{create_table, error, highlight, info, success, warning};
use crate::error::AppError;
use crate::services::{ProviderService, SpeedtestService};
use crate::store::AppState;

use super::utils::{clear_screen, get_state, pause, prompt_confirm, prompt_select};

pub fn manage_providers_menu(app_type: &AppType) -> Result<(), AppError> {
    loop {
        clear_screen();
        println!("\n{}", highlight(texts::provider_management()));
        println!("{}", texts::tui_rule(60));

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
                let marker = if *id == &current_id {
                    texts::tui_marker_active()
                } else {
                    texts::tui_marker_inactive()
                };
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

        let Some(choice) = prompt_select(texts::choose_action(), choices)? else {
            break;
        };

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
        clear_screen();
        let providers = ProviderService::list(state, app_type.clone())?;
        if let Some(provider) = providers.get(current_id) {
            println!("\n{}", highlight(texts::current_provider_details()));
            println!("{}", texts::tui_rule_heavy(60));

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
                println!("\n{}", highlight(texts::api_config_section_header()));
                let api_url = extract_api_url(&provider.settings_config, app_type)
                    .unwrap_or_else(|| "N/A".to_string());
                println!("  API URL:  {}", api_url);
            }

            println!("\n{}", texts::tui_rule(60));

            // Show action menu
            println!();
            let choices = vec![texts::speedtest_endpoint(), texts::back()];
            let Some(choice) = prompt_select(texts::choose_action(), choices)? else {
                break;
            };

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
    clear_screen();
    // Extract API URL
    let api_url = extract_api_url(&provider.settings_config, app_type);

    if api_url.is_none() {
        println!("\n{}", error("No API URL configured for this provider"));
        pause();
        return Ok(());
    }

    let api_url = api_url.unwrap();

    println!(
        "\n{}",
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
    providers: &IndexMap<String, crate::provider::Provider>,
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

    let Some(choice) = prompt_select(texts::select_provider_to_switch(), provider_choices)? else {
        return Ok(());
    };

    let id = choice
        .split('(')
        .nth(1)
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| AppError::Message(texts::invalid_selection_format().to_string()))?;

    let skip_live_sync = !crate::sync_policy::should_sync_live(app_type);
    ProviderService::switch(state, app_type.clone(), id)?;

    println!("\n{}", success(&texts::switched_to_provider(id)));
    if skip_live_sync {
        println!(
            "{}",
            warning(&texts::live_sync_skipped_uninitialized_warning(
                app_type.as_str()
            ))
        );
    }
    println!("{}", info(texts::restart_note()));
    pause();

    Ok(())
}

fn delete_provider_interactive(
    state: &AppState,
    app_type: &AppType,
    providers: &IndexMap<String, crate::provider::Provider>,
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

    let Some(choice) = prompt_select(texts::select_provider_to_delete(), deletable)? else {
        return Ok(());
    };

    let id = choice
        .split('(')
        .nth(1)
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| AppError::Message(texts::invalid_selection_format().to_string()))?;

    let confirm_prompt = texts::confirm_delete(id);
    let Some(confirm) = prompt_confirm(&confirm_prompt, false)? else {
        return Ok(());
    };

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

/// Edit mode choices for provider editing
#[derive(Debug, Clone)]
enum EditMode {
    Interactive,
    JsonEditor,
    Cancel,
}

impl std::fmt::Display for EditMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Interactive => write!(f, "{}", texts::edit_mode_interactive()),
            Self::JsonEditor => write!(f, "{}", texts::edit_mode_json_editor()),
            Self::Cancel => write!(f, "{}", texts::cancel()),
        }
    }
}

/// Codex config file choices for JSON editing
#[derive(Debug, Clone)]
enum CodexConfigFile {
    Auth,   // auth.json
    Config, // config.toml
}

impl std::fmt::Display for CodexConfigFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auth => write!(f, "auth.json"),
            Self::Config => write!(f, "config.toml"),
        }
    }
}

fn edit_provider_interactive(
    app_type: &AppType,
    providers: &IndexMap<String, crate::provider::Provider>,
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

    let Some(selection) = prompt_select(texts::select_provider_to_edit(), choices)? else {
        return Ok(());
    };

    // 从 "Name (id)" 格式中提取 ID
    let selected_id = selection
        .rsplit_once('(') // 从右边分割，找到最后一个 '('
        .and_then(|(_, id_part)| id_part.strip_suffix(')')) // 移除末尾的 ')'
        .ok_or_else(|| AppError::Message(texts::invalid_selection_format().to_string()))?
        .to_string();

    // 3. 选择编辑模式
    let edit_mode_choices = vec![
        EditMode::Interactive,
        EditMode::JsonEditor,
        EditMode::Cancel,
    ];

    let Some(edit_mode) = prompt_select(texts::choose_edit_mode(), edit_mode_choices)? else {
        return Ok(());
    };

    match edit_mode {
        EditMode::Interactive => {
            // 调用命令层的交互式编辑实现
            crate::cli::commands::provider::execute(
                crate::cli::commands::provider::ProviderCommand::Edit { id: selected_id },
                Some(app_type.clone()),
            )?;
        }
        EditMode::JsonEditor => {
            // 获取当前供应商数据
            let original = providers
                .get(&selected_id)
                .ok_or_else(|| AppError::Message(texts::provider_not_found(&selected_id)))?;

            // 调用 JSON 编辑器
            edit_provider_with_json_editor(app_type, &selected_id, original)?;
        }
        EditMode::Cancel => {
            println!("\n{}", info(texts::cancelled()));
        }
    }

    pause();
    Ok(())
}

/// Edit provider using external JSON editor (per-file editing)
fn edit_provider_with_json_editor(
    app_type: &AppType,
    id: &str,
    original: &crate::provider::Provider,
) -> Result<(), AppError> {
    // 1. Determine which field to edit based on app type
    let (field_name, content_to_edit, is_toml) = match app_type {
        AppType::Claude => {
            // Claude: edit entire settings_config (JSON, including env and permissions)
            let json_str = serde_json::to_string_pretty(&original.settings_config)
                .map_err(|e| AppError::JsonSerialize { source: e })?;

            ("settings_config", json_str, false)
        }
        AppType::Codex => {
            // Codex: ask user which file to edit
            let Some(file_choice) = prompt_select(
                texts::select_config_file_to_edit(),
                vec![CodexConfigFile::Auth, CodexConfigFile::Config],
            )?
            else {
                return Ok(());
            };

            match file_choice {
                CodexConfigFile::Auth => {
                    // Edit auth.json (JSON format)
                    let auth_value = original.settings_config.get("auth").ok_or_else(|| {
                        AppError::Message(texts::provider_missing_auth_field().to_string())
                    })?;

                    let json_str = serde_json::to_string_pretty(auth_value)
                        .map_err(|e| AppError::JsonSerialize { source: e })?;

                    ("settings_config.auth", json_str, false)
                }
                CodexConfigFile::Config => {
                    // Edit config.toml (TOML format)
                    let config_str = original
                        .settings_config
                        .get("config")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            AppError::Message(
                                texts::provider_missing_or_invalid_config_field().to_string(),
                            )
                        })?;

                    ("settings_config.config", config_str.to_string(), true)
                }
            }
        }
        AppType::Gemini => {
            // Gemini: edit entire settings_config (JSON)
            let json_str = serde_json::to_string_pretty(&original.settings_config)
                .map_err(|e| AppError::JsonSerialize { source: e })?;

            ("settings_config", json_str, false)
        }
    };

    // 2. Edit loop with validation
    loop {
        // Open external editor
        println!(
            "\n{}",
            info(&format!(
                "{} ({})",
                texts::opening_external_editor(),
                field_name
            ))
        );
        let edited_content = match open_external_editor(&content_to_edit) {
            Ok(content) => content,
            Err(e) => {
                println!("\n{}", error(&format!("{}", e)));
                return Ok(());
            }
        };

        // Check if content was changed
        if edited_content.trim() == content_to_edit.trim() {
            println!("\n{}", info(texts::no_changes_detected()));
            return Ok(());
        }

        // 3. Validate syntax based on format
        let validated_value = if is_toml {
            // TOML validation for Codex config
            match toml::from_str::<toml::Value>(&edited_content) {
                Ok(_) => {
                    // Store as string in JSON Value
                    serde_json::Value::String(edited_content.clone())
                }
                Err(e) => {
                    println!("\n{}", error(&format!("Invalid TOML syntax: {}", e)));

                    if !retry_prompt()? {
                        return Ok(());
                    }
                    continue;
                }
            }
        } else {
            // JSON validation
            match serde_json::from_str::<serde_json::Value>(&edited_content) {
                Ok(v) => v,
                Err(e) => {
                    println!(
                        "\n{}",
                        error(&format!("{}: {}", texts::invalid_json_syntax(), e))
                    );

                    if !retry_prompt()? {
                        return Ok(());
                    }
                    continue;
                }
            }
        };

        // 4. Merge edited field back into Provider
        let mut updated_provider = original.clone();

        match app_type {
            AppType::Claude => {
                // Replace entire settings_config
                updated_provider.settings_config = validated_value;
            }
            AppType::Codex => {
                // Update auth or config field based on what was edited
                if let Some(settings_obj) = updated_provider.settings_config.as_object_mut() {
                    if field_name == "settings_config.auth" {
                        settings_obj.insert("auth".to_string(), validated_value);
                    } else {
                        settings_obj.insert("config".to_string(), validated_value);
                    }
                }
            }
            AppType::Gemini => {
                // Replace entire settings_config
                updated_provider.settings_config = validated_value;
            }
        }

        // 5. Display summary
        println!("\n{}", highlight(texts::provider_summary()));
        println!("{}", texts::tui_rule(60));
        display_provider_summary(&updated_provider, app_type);

        // 6. Confirm save
        let Some(confirm) = prompt_confirm(texts::confirm_save_changes(), false)? else {
            return Ok(());
        };

        if !confirm {
            println!("\n{}", info(texts::cancelled()));
            return Ok(());
        }

        // 7. Save to config.json
        let state = get_state()?;
        ProviderService::update(&state, app_type.clone(), updated_provider)?;

        println!(
            "\n{}",
            success(&texts::entity_updated_success(texts::entity_provider(), id))
        );

        // 8. Immediately sync to live config files
        println!("\n{}", info("Syncing to live config files..."));
        ProviderService::switch(&state, app_type.clone(), id)?;

        println!("{}", success("✓ Changes synced to live config files"));
        println!("{}", info(texts::restart_note()));

        break;
    }

    Ok(())
}

/// Helper function to prompt for retry
fn retry_prompt() -> Result<bool, AppError> {
    Ok(prompt_confirm(texts::retry_editing(), true)?.unwrap_or(false))
}

/// Open external editor for content editing
fn open_external_editor(initial_content: &str) -> Result<String, AppError> {
    edit::edit(initial_content)
        .map_err(|e| AppError::Message(format!("{}: {}", texts::editor_failed(), e)))
}

/// Display provider summary (used by JSON editor)
fn display_provider_summary(provider: &crate::provider::Provider, app_type: &AppType) {
    println!("  {}:       {}", texts::id_label_colon(), provider.id);
    println!(
        "  {}:     {}",
        texts::name_label_with_colon(),
        provider.name
    );

    if let Some(url) = &provider.website_url {
        println!("  {}:      {}", texts::url_label_colon(), url);
    }

    if let Some(sort_index) = provider.sort_index {
        println!("  {}:     {}", texts::sort_index_label_colon(), sort_index);
    }

    // Show API URL if available
    if let Some(api_url) = extract_api_url(&provider.settings_config, app_type) {
        println!("  {}:  {}", texts::api_url_label_colon(), api_url);
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
