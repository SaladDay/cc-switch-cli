use inquire::{Confirm, Select, Text};
use std::path::Path;

use crate::app_config::MultiAppConfig;
use crate::cli::i18n::texts;
use crate::cli::ui::{highlight, info, success};
use crate::config::get_app_config_path;
use crate::error::AppError;
use crate::services::ConfigService;

use super::utils::{get_state, pause};

pub fn manage_config_menu() -> Result<(), AppError> {
    loop {
        println!("\n{}", highlight(texts::config_management()));
        println!("{}", "─".repeat(60));

        let choices = vec![
            texts::config_show_path(),
            texts::config_show_full(),
            texts::config_export(),
            texts::config_import(),
            texts::config_backup(),
            texts::config_restore(),
            texts::config_validate(),
            texts::config_reset(),
            texts::back_to_main(),
        ];

        let choice = Select::new(texts::choose_action(), choices)
            .prompt()
            .map_err(|_| AppError::Message("Selection cancelled".to_string()))?;

        if choice == texts::config_show_path() {
            show_config_path_interactive()?;
        } else if choice == texts::config_show_full() {
            show_full_config_interactive()?;
        } else if choice == texts::config_export() {
            let path = Text::new(texts::enter_export_path())
                .with_default("./config-export.json")
                .prompt()
                .map_err(|e| AppError::Message(format!("Input failed: {}", e)))?;
            export_config_interactive(&path)?;
        } else if choice == texts::config_import() {
            let path = Text::new(texts::enter_import_path())
                .prompt()
                .map_err(|e| AppError::Message(format!("Input failed: {}", e)))?;
            import_config_interactive(&path)?;
        } else if choice == texts::config_backup() {
            backup_config_interactive()?;
        } else if choice == texts::config_restore() {
            restore_config_interactive()?;
        } else if choice == texts::config_validate() {
            validate_config_interactive()?;
        } else if choice == texts::config_reset() {
            reset_config_interactive()?;
        } else {
            break;
        }
    }

    Ok(())
}

fn show_config_path_interactive() -> Result<(), AppError> {
    let config_path = get_app_config_path();
    let config_dir = config_path.parent().unwrap_or(&config_path);

    println!(
        "\n{}",
        highlight(texts::config_show_path().trim_start_matches("📍 "))
    );
    println!("{}", "─".repeat(60));
    println!("Config file: {}", config_path.display());
    println!("Config dir:  {}", config_dir.display());

    if config_path.exists() {
        if let Ok(metadata) = std::fs::metadata(&config_path) {
            println!("File size:   {} bytes", metadata.len());
        }
    } else {
        println!("Status:      File does not exist");
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
    let config = MultiAppConfig::load()?;
    let json = serde_json::to_string_pretty(&config)
        .map_err(|e| AppError::Message(format!("Failed to serialize config: {}", e)))?;

    println!(
        "\n{}",
        highlight(texts::config_show_full().trim_start_matches("👁️  "))
    );
    println!("{}", "─".repeat(60));
    println!("{}", json);

    pause();
    Ok(())
}

fn export_config_interactive(path: &str) -> Result<(), AppError> {
    let target_path = Path::new(path);

    if target_path.exists() {
        let confirm = Confirm::new(&texts::file_overwrite_confirm(path))
            .with_default(false)
            .prompt()
            .map_err(|_| AppError::Message("Confirmation failed".to_string()))?;

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
    let file_path = Path::new(path);

    if !file_path.exists() {
        return Err(AppError::Message(format!("File not found: {}", path)));
    }

    let confirm = Confirm::new(texts::confirm_import())
        .with_default(false)
        .prompt()
        .map_err(|_| AppError::Message("Confirmation failed".to_string()))?;

    if !confirm {
        println!("\n{}", info(texts::cancelled()));
        pause();
        return Ok(());
    }

    let state = get_state()?;
    let backup_id = ConfigService::import_config_from_path(file_path, &state)?;

    println!("\n{}", success(&texts::imported_from(path)));
    println!("{}", info(&format!("Backup created: {}", backup_id)));
    pause();
    Ok(())
}

fn backup_config_interactive() -> Result<(), AppError> {
    println!(
        "\n{}",
        highlight(texts::config_backup().trim_start_matches("💾 "))
    );
    println!("{}", "─".repeat(60));

    // 询问是否使用自定义名称
    let use_custom_name = Confirm::new("是否使用自定义备份名称？")
        .with_default(false)
        .with_help_message("自定义名称可以帮助您识别备份用途，如 'before-update'")
        .prompt()
        .map_err(|_| AppError::Message("Confirmation failed".to_string()))?;

    let custom_name = if use_custom_name {
        Some(
            Text::new("请输入备份名称：")
                .with_help_message("仅支持字母、数字、短横线和下划线")
                .prompt()
                .map_err(|e| AppError::Message(format!("Input failed: {}", e)))?
                .trim()
                .to_string(),
        )
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
    println!(
        "\n{}",
        highlight(texts::config_restore().trim_start_matches("♻️  "))
    );
    println!("{}", "─".repeat(60));

    // 获取备份列表
    let config_path = get_app_config_path();
    let backups = ConfigService::list_backups(&config_path)?;

    if backups.is_empty() {
        println!("\n{}", info("暂无可用备份"));
        println!("{}", info("提示：使用 '💾 备份配置' 创建备份"));
        pause();
        return Ok(());
    }

    // 显示备份列表供选择
    println!("\n找到 {} 个备份：", backups.len());
    println!();

    let choices: Vec<String> = backups
        .iter()
        .map(|b| format!("{} - {}", b.display_name, b.id))
        .collect();

    let selection = Select::new("选择要恢复的备份：", choices)
        .prompt()
        .map_err(|_| AppError::Message("Selection cancelled".to_string()))?;

    // 从选择中提取备份 ID
    let selected_backup = backups
        .iter()
        .find(|b| selection.contains(&b.id))
        .ok_or_else(|| AppError::Message("无效的选择".to_string()))?;

    println!();
    println!("{}", highlight("警告："));
    println!("这将使用所选备份替换当前配置");
    println!("当前配置会先自动备份");
    println!();

    let confirm = Confirm::new("确认恢复？")
        .with_default(false)
        .prompt()
        .map_err(|_| AppError::Message("Confirmation failed".to_string()))?;

    if !confirm {
        println!("\n{}", info(texts::cancelled()));
        pause();
        return Ok(());
    }

    let state = get_state()?;
    let pre_restore_backup = ConfigService::restore_from_backup_id(&selected_backup.id, &state)?;

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
    let config_path = get_app_config_path();

    println!(
        "\n{}",
        highlight(texts::config_validate().trim_start_matches("✓ "))
    );
    println!("{}", "─".repeat(60));

    if !config_path.exists() {
        return Err(AppError::Message("Config file does not exist".to_string()));
    }

    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| AppError::Message(format!("Failed to read config: {}", e)))?;

    let _: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| AppError::Message(format!("Invalid JSON: {}", e)))?;

    let config: MultiAppConfig = serde_json::from_str(&content)
        .map_err(|e| AppError::Message(format!("Invalid config structure: {}", e)))?;

    println!("{}", success(texts::config_valid()));
    println!();

    let claude_count = config
        .apps
        .get("claude")
        .map(|m| m.providers.len())
        .unwrap_or(0);
    let codex_count = config
        .apps
        .get("codex")
        .map(|m| m.providers.len())
        .unwrap_or(0);
    let gemini_count = config
        .apps
        .get("gemini")
        .map(|m| m.providers.len())
        .unwrap_or(0);
    let mcp_count = config.mcp.servers.as_ref().map(|s| s.len()).unwrap_or(0);

    println!("Claude providers: {}", claude_count);
    println!("Codex providers:  {}", codex_count);
    println!("Gemini providers: {}", gemini_count);
    println!("MCP servers:      {}", mcp_count);

    pause();
    Ok(())
}

fn reset_config_interactive() -> Result<(), AppError> {
    let confirm = Confirm::new(texts::confirm_reset())
        .with_default(false)
        .prompt()
        .map_err(|_| AppError::Message("Confirmation failed".to_string()))?;

    if !confirm {
        println!("\n{}", info(texts::cancelled()));
        pause();
        return Ok(());
    }

    let config_path = get_app_config_path();

    let backup_id = ConfigService::create_backup(&config_path, None)?;

    if config_path.exists() {
        std::fs::remove_file(&config_path)
            .map_err(|e| AppError::Message(format!("Failed to delete config: {}", e)))?;
    }

    let _ = MultiAppConfig::load()?;

    println!("\n{}", success(texts::config_reset_done()));
    println!(
        "{}",
        info(&format!("Previous config backed up: {}", backup_id))
    );
    pause();
    Ok(())
}
