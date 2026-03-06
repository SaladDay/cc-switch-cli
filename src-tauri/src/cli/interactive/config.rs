use inquire::{Confirm, Text};
use std::path::Path;

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::cli::ui::{error, highlight, info, success};
use crate::config::get_app_config_path;
use crate::error::AppError;
use crate::services::ConfigService;
use crate::services::ProviderService;

use super::utils::{
    clear_screen, get_state, handle_inquire, pause, prompt_confirm, prompt_select, prompt_text,
    prompt_text_with_default,
};

pub fn manage_config_menu(app_type: &AppType) -> Result<(), AppError> {
    loop {
        clear_screen();
        println!("\n{}", highlight(texts::config_management()));
        println!("{}", texts::tui_rule(60));

        let choices = vec![
            texts::config_show_path(),
            texts::config_show_full(),
            texts::config_export(),
            texts::config_import(),
            texts::config_backup(),
            texts::config_restore(),
            texts::config_validate(),
            texts::config_common_snippet(),
            texts::config_reset(),
            texts::back_to_main(),
        ];

        let Some(choice) = prompt_select(texts::choose_action(), choices)? else {
            break;
        };

        if choice == texts::config_show_path() {
            show_config_path_interactive()?;
        } else if choice == texts::config_show_full() {
            show_full_config_interactive()?;
        } else if choice == texts::config_export() {
            let Some(path) = prompt_text_with_default(
                texts::enter_export_path(),
                texts::tui_default_config_export_path(),
            )?
            else {
                continue;
            };
            export_config_interactive(&path)?;
        } else if choice == texts::config_import() {
            let Some(path) = prompt_text(texts::enter_import_path())? else {
                continue;
            };
            import_config_interactive(&path)?;
        } else if choice == texts::config_backup() {
            backup_config_interactive()?;
        } else if choice == texts::config_restore() {
            restore_config_interactive()?;
        } else if choice == texts::config_validate() {
            validate_config_interactive()?;
        } else if choice == texts::config_common_snippet() {
            edit_common_config_snippet_interactive(app_type)?;
        } else if choice == texts::config_reset() {
            reset_config_interactive()?;
        } else {
            break;
        }
    }

    Ok(())
}

fn edit_common_config_snippet_interactive(app_type: &AppType) -> Result<(), AppError> {
    clear_screen();
    println!(
        "\n{}",
        highlight(
            texts::config_common_snippet()
                .trim_start_matches("🧩 ")
                .trim()
        )
    );
    println!("{}", texts::tui_rule(60));

    let state = get_state()?;
    let current = {
        let cfg = state.config.read()?;
        cfg.common_config_snippets.get(app_type).cloned()
    }
    .unwrap_or_default();

    let initial = if current.trim().is_empty() {
        texts::tui_default_common_snippet_for_app(app_type.as_str()).to_string()
    } else {
        current
    };

    let field_name = format!("common_config_snippet.{}", app_type.as_str());

    loop {
        println!(
            "\n{}",
            info(&format!(
                "{} ({})",
                texts::opening_external_editor(),
                field_name
            ))
        );

        let edited = match open_external_editor(&initial) {
            Ok(content) => content,
            Err(e) => {
                println!("\n{}", error(&format!("{}", e)));
                return Ok(());
            }
        };

        // Check if content was changed
        if edited.trim() == initial.trim() {
            println!("\n{}", info(texts::no_changes_detected()));
            return Ok(());
        }

        let edited = edited.trim().to_string();
        let (next_snippet, action_label) = if edited.is_empty() {
            (None, texts::common_config_snippet_cleared())
        } else if matches!(*app_type, AppType::Codex) {
            let doc: toml_edit::DocumentMut = match edited.parse() {
                Ok(v) => v,
                Err(e) => {
                    println!(
                        "\n{}",
                        error(&texts::common_config_snippet_invalid_toml(&e.to_string()))
                    );
                    if !retry_prompt()? {
                        return Ok(());
                    }
                    continue;
                }
            };

            let canonical = doc.to_string().trim().to_string();

            println!("\n{}", highlight(texts::config_common_snippet()));
            println!("{}", texts::tui_rule(60));
            println!("{}", canonical);

            let Some(confirm) = prompt_confirm(texts::confirm_save_changes(), false)? else {
                return Ok(());
            };

            if !confirm {
                println!("\n{}", info(texts::cancelled()));
                return Ok(());
            }

            (Some(canonical), texts::common_config_snippet_saved())
        } else {
            let value: serde_json::Value = match serde_json::from_str(&edited) {
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
            };

            if !value.is_object() {
                println!(
                    "\n{}",
                    error(&texts::common_config_snippet_not_object().to_string())
                );
                if !retry_prompt()? {
                    return Ok(());
                }
                continue;
            }

            let pretty = serde_json::to_string_pretty(&value)
                .map_err(|e| AppError::Message(texts::failed_to_serialize_json(&e.to_string())))?;

            println!("\n{}", highlight(texts::config_common_snippet()));
            println!("{}", texts::tui_rule(60));
            println!("{}", pretty);

            let Some(confirm) = prompt_confirm(texts::confirm_save_changes(), false)? else {
                return Ok(());
            };

            if !confirm {
                println!("\n{}", info(texts::cancelled()));
                return Ok(());
            }

            (Some(pretty), texts::common_config_snippet_saved())
        };

        {
            let mut cfg = state.config.write()?;
            cfg.common_config_snippets.set(app_type, next_snippet);
        }
        state.save()?;

        println!("\n{}", success(action_label));

        break;
    }

    let Some(apply) = prompt_confirm(texts::common_config_snippet_apply_now(), true)? else {
        return Ok(());
    };

    if apply {
        let current_id = ProviderService::current(&state, app_type.clone())?;
        if current_id.trim().is_empty() {
            println!(
                "{}",
                info(texts::common_config_snippet_no_current_provider())
            );
        } else {
            ProviderService::switch(&state, app_type.clone(), &current_id)?;
            println!("{}", success(texts::common_config_snippet_applied()));
        }
    } else {
        println!("{}", info(texts::common_config_snippet_apply_hint()));
    }

    pause();
    Ok(())
}

fn retry_prompt() -> Result<bool, AppError> {
    Ok(prompt_confirm(texts::retry_editing(), true)?.unwrap_or(false))
}

fn open_external_editor(initial_content: &str) -> Result<String, AppError> {
    crate::cli::editor::open_external_editor(initial_content)
}

fn show_config_path_interactive() -> Result<(), AppError> {
    clear_screen();
    let config_dir = crate::config::get_app_config_dir();
    let db_path = config_dir.join("cc-switch.db");
    let legacy_config_path = get_app_config_path();

    println!(
        "\n{}",
        highlight(texts::config_show_path().trim_start_matches("📍 "))
    );
    println!("{}", texts::tui_rule(60));
    println!("DB file:      {}", db_path.display());
    println!("Legacy JSON:  {}", legacy_config_path.display());
    println!("Config dir:   {}", config_dir.display());

    if db_path.exists() {
        if let Ok(metadata) = std::fs::metadata(&db_path) {
            println!("File size:   {} bytes", metadata.len());
        }
    } else {
        println!("Status:      Database file does not exist");
    }

    let backup_dir = config_dir.join("backups");
    if backup_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&backup_dir) {
            let count = entries.filter(|e| e.is_ok()).count();
            println!("Backups:     {} files in {}", count, backup_dir.display());
        }
    }

    pause();
    Ok(())
}

fn show_full_config_interactive() -> Result<(), AppError> {
    clear_screen();
    let state = get_state()?;
    let config = state.config.read()?;
    let json = serde_json::to_string_pretty(&*config)
        .map_err(|e| AppError::Message(format!("Failed to serialize config: {}", e)))?;

    println!(
        "\n{}",
        highlight(texts::config_show_full().trim_start_matches("👁️  "))
    );
    println!("{}", texts::tui_rule(60));
    println!("{}", json);

    pause();
    Ok(())
}

fn export_config_interactive(path: &str) -> Result<(), AppError> {
    clear_screen();
    let target_path = Path::new(path);

    if target_path.exists() {
        let overwrite_prompt = texts::file_overwrite_confirm(path);
        let Some(confirm) = prompt_confirm(&overwrite_prompt, false)? else {
            return Ok(());
        };

        if !confirm {
            println!("\n{}", info(texts::cancelled()));
            pause();
            return Ok(());
        }
    }

    ConfigService::export_config_to_path(target_path)?;

    println!("\n{}", success(&texts::exported_to(path)));
    pause();
    Ok(())
}

fn import_config_interactive(path: &str) -> Result<(), AppError> {
    clear_screen();
    let file_path = Path::new(path);

    if !file_path.exists() {
        return Err(AppError::Message(format!("File not found: {}", path)));
    }

    let Some(confirm) = prompt_confirm(texts::confirm_import(), false)? else {
        return Ok(());
    };

    if !confirm {
        println!("\n{}", info(texts::cancelled()));
        pause();
        return Ok(());
    }

    let state = get_state()?;
    let backup_id = ConfigService::import_config_from_path(file_path, &state)?;

    // 导入后同步 live 配置
    if let Err(e) = crate::services::provider::ProviderService::sync_current_to_live(&state) {
        log::warn!("配置导入后同步 live 配置失败: {e}");
    }

    println!("\n{}", success(&texts::imported_from(path)));
    println!("{}", info(&format!("Backup created: {}", backup_id)));
    pause();
    Ok(())
}

fn backup_config_interactive() -> Result<(), AppError> {
    clear_screen();
    println!(
        "\n{}",
        highlight(texts::config_backup().trim_start_matches("💾 "))
    );
    println!("{}", texts::tui_rule(60));

    // 询问是否使用自定义名称
    let Some(use_custom_name) = handle_inquire(
        Confirm::new("是否使用自定义备份名称？")
            .with_default(false)
            .with_help_message("自定义名称可以帮助您识别备份用途，如 'before-update'")
            .prompt(),
    )?
    else {
        return Ok(());
    };

    let custom_name = if use_custom_name {
        let Some(input) = handle_inquire(
            Text::new("请输入备份名称：")
                .with_help_message("仅支持字母、数字、短横线和下划线")
                .prompt(),
        )?
        else {
            return Ok(());
        };

        let trimmed = input.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    } else {
        None
    };

    let config_path = get_app_config_path();
    let backup_id = ConfigService::create_backup(&config_path, custom_name)?;

    println!("\n{}", success(&texts::backup_created(&backup_id)));

    // 显示备份文件完整路径
    let backup_dir = config_path.parent().unwrap().join("backups");
    let backup_file = backup_dir.join(format!("{}.json", backup_id));
    println!("{}", info(&format!("位置: {}", backup_file.display())));

    pause();
    Ok(())
}

fn restore_config_interactive() -> Result<(), AppError> {
    clear_screen();
    println!(
        "\n{}",
        highlight(texts::config_restore().trim_start_matches("♻️  "))
    );
    println!("{}", texts::tui_rule(60));

    // 获取备份列表
    let config_path = get_app_config_path();
    let backups = ConfigService::list_backups(&config_path)?;

    if backups.is_empty() {
        println!("\n{}", info(texts::no_backups_found()));
        println!("{}", info(texts::create_backup_first_hint()));
        pause();
        return Ok(());
    }

    // 显示备份列表供选择
    println!("\n{}", texts::found_backups(backups.len()));
    println!();

    let choices: Vec<String> = backups
        .iter()
        .map(|b| format!("{} - {}", b.display_name, b.id))
        .collect();

    let Some(selection) = prompt_select(texts::select_backup_to_restore(), choices)? else {
        return Ok(());
    };

    // 从选择中提取备份 ID
    let selected_backup = backups
        .iter()
        .find(|b| selection.contains(&b.id))
        .ok_or_else(|| AppError::Message(texts::invalid_selection().to_string()))?;

    println!();
    println!("{}", highlight(texts::warning_title()));
    println!("{}", texts::config_restore_warning_replace());
    println!("{}", texts::config_restore_warning_pre_backup());
    println!();

    let Some(confirm) = prompt_confirm(texts::config_restore_confirm_prompt(), false)? else {
        return Ok(());
    };

    if !confirm {
        println!("\n{}", info(texts::cancelled()));
        pause();
        return Ok(());
    }

    let state = get_state()?;
    let pre_restore_backup = ConfigService::restore_from_backup_id(&selected_backup.id, &state)?;

    // 恢复后同步 live 配置
    if let Err(e) = crate::services::provider::ProviderService::sync_current_to_live(&state) {
        log::warn!("备份恢复后同步 live 配置失败: {e}");
    }

    println!(
        "\n{}",
        success(&format!("✓ 已从备份恢复: {}", selected_backup.display_name))
    );
    if !pre_restore_backup.is_empty() {
        println!(
            "{}",
            info(&format!("  恢复前配置已备份: {}", pre_restore_backup))
        );
    }
    println!("\n{}", info("注意：重启 CLI 客户端以应用更改"));

    pause();
    Ok(())
}

fn validate_config_interactive() -> Result<(), AppError> {
    clear_screen();
    let config_dir = crate::config::get_app_config_dir();
    let db_path = config_dir.join("cc-switch.db");

    println!(
        "\n{}",
        highlight(texts::config_validate().trim_start_matches("✓ "))
    );
    println!("{}", texts::tui_rule(60));

    if !db_path.exists() {
        return Err(AppError::Message(
            texts::tui_toast_config_file_does_not_exist().to_string(),
        ));
    }

    let db = crate::Database::init()?;
    let claude_count = db.get_all_providers("claude")?.len();
    let codex_count = db.get_all_providers("codex")?.len();
    let gemini_count = db.get_all_providers("gemini")?.len();
    let mcp_count = db.get_all_mcp_servers()?.len();
    let skills_count = db.get_all_installed_skills()?.len();

    println!("{}", success(texts::config_valid()));
    println!();

    println!("Claude providers: {}", claude_count);
    println!("Codex providers:  {}", codex_count);
    println!("Gemini providers: {}", gemini_count);
    println!("MCP servers:      {}", mcp_count);
    println!("Skills installed: {}", skills_count);

    pause();
    Ok(())
}

fn reset_config_interactive() -> Result<(), AppError> {
    clear_screen();
    let Some(confirm) = prompt_confirm(texts::confirm_reset(), false)? else {
        return Ok(());
    };

    if !confirm {
        println!("\n{}", info(texts::cancelled()));
        pause();
        return Ok(());
    }

    let config_dir = crate::config::get_app_config_dir();
    let db_path = config_dir.join("cc-switch.db");

    let backup_id = ConfigService::create_backup(&db_path, None)?;

    if db_path.exists() {
        std::fs::remove_file(&db_path).map_err(|e| AppError::io(&db_path, e))?;
    }

    let _ = crate::Database::init()?;

    println!("\n{}", success(texts::config_reset_done()));
    if !backup_id.is_empty() {
        println!(
            "{}",
            info(&format!("Previous config backed up: {}", backup_id))
        );
    }
    pause();
    Ok(())
}
