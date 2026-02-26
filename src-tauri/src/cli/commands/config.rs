use clap::Subcommand;
use std::fs;
use std::path::{Path, PathBuf};

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::cli::ui::{error, highlight, info, success, to_json};
use crate::error::AppError;
use crate::services::ConfigService;
use crate::store::AppState;

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Show current configuration
    Show,
    /// Show configuration file path
    Path,
    /// Export configuration to file
    Export {
        /// Output file path
        file: PathBuf,
    },
    /// Import configuration from file
    Import {
        /// Input file path
        file: PathBuf,
    },
    /// Create a backup of current configuration
    Backup {
        /// Optional custom name for the backup
        #[arg(long)]
        name: Option<String>,
    },
    /// Restore from a backup
    Restore {
        /// Backup ID to restore (from list)
        #[arg(long, conflicts_with = "file")]
        backup: Option<String>,

        /// External file path to restore from
        #[arg(long, conflicts_with = "backup")]
        file: Option<PathBuf>,
    },
    /// Validate configuration file
    Validate,
    /// Reset to default configuration
    Reset,

    /// Manage common configuration snippet (per app)
    #[command(subcommand)]
    Common(CommonConfigCommand),
}

#[derive(Subcommand)]
pub enum CommonConfigCommand {
    /// Show current common config snippet
    Show,
    /// Set common config snippet (JSON object)
    Set {
        /// JSON object string (e.g. '{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1}}')
        #[arg(long, conflicts_with = "file")]
        json: Option<String>,

        /// Read JSON object from file
        #[arg(long, conflicts_with = "json")]
        file: Option<PathBuf>,

        /// Apply to current provider immediately
        #[arg(long)]
        apply: bool,
    },
    /// Clear common config snippet
    Clear {
        /// Apply to current provider immediately
        #[arg(long)]
        apply: bool,
    },
}

pub fn execute(cmd: ConfigCommand, app: Option<AppType>) -> Result<(), AppError> {
    match cmd {
        ConfigCommand::Show => show_config(),
        ConfigCommand::Path => show_path(),
        ConfigCommand::Export { file } => export_config(&file),
        ConfigCommand::Import { file } => import_config(&file),
        ConfigCommand::Backup { name } => backup_config(name.as_deref()),
        ConfigCommand::Restore { backup, file } => {
            restore_config(backup.as_deref(), file.as_deref())
        }
        ConfigCommand::Validate => validate_config(),
        ConfigCommand::Reset => reset_config(),
        ConfigCommand::Common(cmd) => execute_common(cmd, app.unwrap_or(AppType::Claude)),
    }
}

fn get_state() -> Result<AppState, AppError> {
    AppState::try_new()
}

fn show_config() -> Result<(), AppError> {
    let state = get_state()?;
    let config = state.config.read()?;

    println!("{}", highlight("Current Configuration"));
    println!("{}", "=".repeat(50));
    println!();

    // Display in pretty JSON format
    let json = to_json(&*config).map_err(|e| AppError::Message(e.to_string()))?;
    println!("{}", json);

    Ok(())
}

fn execute_common(cmd: CommonConfigCommand, app_type: AppType) -> Result<(), AppError> {
    match cmd {
        CommonConfigCommand::Show => show_common(app_type),
        CommonConfigCommand::Set { json, file, apply } => {
            set_common(app_type, json.as_deref(), file.as_deref(), apply)
        }
        CommonConfigCommand::Clear { apply } => clear_common(app_type, apply),
    }
}

fn show_common(app_type: AppType) -> Result<(), AppError> {
    let state = get_state()?;
    let config = state.config.read()?;
    let snippet = config.common_config_snippets.get(&app_type).cloned();

    println!("{}", highlight(texts::config_common_snippet_title()));
    println!("{}", "=".repeat(50));
    println!("App: {}", app_type.as_str());
    println!();

    match snippet {
        Some(s) if !s.trim().is_empty() => {
            println!("{}", s);
        }
        _ => {
            println!("{}", info(texts::config_common_snippet_none_set()));
        }
    }

    Ok(())
}

fn set_common(
    app_type: AppType,
    json_text: Option<&str>,
    file: Option<&Path>,
    apply: bool,
) -> Result<(), AppError> {
    let raw = if let Some(text) = json_text {
        text.to_string()
    } else if let Some(path) = file {
        fs::read_to_string(path).map_err(|e| AppError::io(path, e))?
    } else {
        return Err(AppError::InvalidInput(
            texts::config_common_snippet_require_json_or_file().to_string(),
        ));
    };

    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| AppError::InvalidInput(texts::tui_toast_invalid_json(&e.to_string())))?;
    if !value.is_object() {
        return Err(AppError::InvalidInput(
            texts::common_config_snippet_not_object().to_string(),
        ));
    }

    let pretty = serde_json::to_string_pretty(&value)
        .map_err(|e| AppError::Message(texts::failed_to_serialize_json(&e.to_string())))?;

    let state = get_state()?;
    {
        let mut config = state.config.write()?;
        config.common_config_snippets.set(&app_type, Some(pretty));
    }
    state.save()?;

    println!(
        "{}",
        success(&texts::config_common_snippet_set_for_app(app_type.as_str()))
    );

    if apply {
        apply_common_to_current(&state, app_type)?;
    } else {
        println!(
            "{}",
            info("Tip: run `cc-switch provider switch <id>` to re-apply settings to the live config.")
        );
    }

    Ok(())
}

fn clear_common(app_type: AppType, apply: bool) -> Result<(), AppError> {
    let state = get_state()?;
    {
        let mut config = state.config.write()?;
        config.common_config_snippets.set(&app_type, None);
    }
    state.save()?;

    println!(
        "{}",
        success(&format!(
            "✓ Common config snippet cleared for app '{}'",
            app_type.as_str()
        ))
    );

    if apply {
        apply_common_to_current(&state, app_type)?;
    } else {
        println!(
            "{}",
            info("Tip: run `cc-switch provider switch <id>` to re-apply settings to the live config.")
        );
    }

    Ok(())
}

fn apply_common_to_current(state: &AppState, app_type: AppType) -> Result<(), AppError> {
    use crate::services::ProviderService;

    let current_id = ProviderService::current(state, app_type.clone())?;
    if current_id.trim().is_empty() {
        println!("{}", info("No current provider; nothing to apply."));
        return Ok(());
    }

    ProviderService::switch(state, app_type, &current_id)?;
    println!("{}", success("✓ Applied to live config."));
    Ok(())
}

fn show_path() -> Result<(), AppError> {
    let config_dir = crate::config::get_app_config_dir();
    let db_path = config_dir.join("cc-switch.db");
    let legacy_config_path = config_dir.join("config.json");

    println!("{}", highlight("Configuration Paths"));
    println!("{}", "=".repeat(50));
    println!("DB file:      {}", db_path.display());
    println!("Legacy JSON:  {}", legacy_config_path.display());
    println!("Config dir:   {}", config_dir.display());

    // Check if DB file exists
    if db_path.exists() {
        println!("\n{} Database exists", success("✓"));

        // Show file size
        if let Ok(metadata) = fs::metadata(&db_path) {
            println!("File size:    {} bytes", metadata.len());
        }
    } else {
        println!("\n{} Database file does not exist", error("✗"));
        println!("{}", info("Run cc-switch once to create the database."));
    }

    // Show backup directory
    let backup_dir = config_dir.join("backups");
    if backup_dir.exists() {
        if let Ok(entries) = fs::read_dir(&backup_dir) {
            let count = entries.filter_map(|e| e.ok()).count();
            println!("\nBackups dir:  {}", backup_dir.display());
            println!("Backups:      {} backup(s) found", count);
        }
    }

    Ok(())
}

fn export_config(file: &PathBuf) -> Result<(), AppError> {
    println!(
        "{}",
        info(&format!("Exporting configuration to {}...", file.display()))
    );

    // Check if target file already exists
    if file.exists() {
        let confirm = inquire::Confirm::new(&format!(
            "File '{}' already exists. Overwrite?",
            file.display()
        ))
        .with_default(false)
        .prompt()
        .map_err(|e| AppError::Message(format!("Prompt failed: {}", e)))?;

        if !confirm {
            println!("{}", info("Cancelled."));
            return Ok(());
        }
    }

    // Ensure parent directory exists
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    // Export configuration
    ConfigService::export_config_to_path(file)?;

    println!(
        "{}",
        success(&format!("✓ Configuration exported to {}", file.display()))
    );

    Ok(())
}

fn import_config(file: &PathBuf) -> Result<(), AppError> {
    println!(
        "{}",
        info(&format!(
            "Importing configuration from {}...",
            file.display()
        ))
    );

    // Check if source file exists
    if !file.exists() {
        return Err(AppError::Message(format!(
            "File '{}' not found",
            file.display()
        )));
    }

    // Confirm import
    println!();
    println!("{}", highlight("Warning:"));
    println!("This will replace your current database with the imported SQL backup.");
    println!("A backup will be created automatically.");
    println!();

    let confirm = inquire::Confirm::new("Continue with import?")
        .with_default(false)
        .prompt()
        .map_err(|e| AppError::Message(format!("Prompt failed: {}", e)))?;

    if !confirm {
        println!("{}", info("Cancelled."));
        return Ok(());
    }

    // Perform import
    let state = get_state()?;
    let backup_id = ConfigService::import_config_from_path(file, &state)?;

    // 导入后同步 live 配置
    if let Err(e) = crate::services::provider::ProviderService::sync_current_to_live(&state) {
        log::warn!("配置导入后同步 live 配置失败: {e}");
    }

    println!(
        "{}",
        success(&format!("✓ Configuration imported from {}", file.display()))
    );
    if !backup_id.is_empty() {
        println!("{}", info(&format!("  Backup created: {}", backup_id)));
    }
    println!();
    println!(
        "{}",
        info("Note: Restart your CLI clients to apply the changes.")
    );

    Ok(())
}

fn backup_config(custom_name: Option<&str>) -> Result<(), AppError> {
    let config_path = crate::config::get_app_config_path();

    if let Some(name) = custom_name {
        println!(
            "{}",
            info(&format!("Creating backup with name '{}'...", name))
        );
    } else {
        println!("{}", info("Creating backup of current configuration..."));
    }

    let backup_id = ConfigService::create_backup(&config_path, custom_name.map(|s| s.to_string()))?;

    if backup_id.is_empty() {
        println!("{}", error("Failed to create backup."));
    } else {
        let backup_dir = config_path.parent().unwrap().join("backups");
        let backup_file = backup_dir.join(format!("{}.sql", backup_id));

        println!("{}", success(&format!("✓ Backup created: {}", backup_id)));
        println!("Location: {}", backup_file.display());
    }

    Ok(())
}

fn restore_config(backup_id: Option<&str>, file_path: Option<&Path>) -> Result<(), AppError> {
    let config_path = crate::config::get_app_config_path();

    // 情况1：指定了备份 ID
    if let Some(id) = backup_id {
        println!("{}", info(&format!("Restoring from backup '{}'...", id)));

        let confirm =
            inquire::Confirm::new("This will replace your current configuration. Continue?")
                .with_default(false)
                .prompt()
                .map_err(|e| AppError::Message(format!("Prompt failed: {}", e)))?;

        if !confirm {
            println!("{}", info("Cancelled."));
            return Ok(());
        }

        let state = get_state()?;
        let pre_restore_backup = ConfigService::restore_from_backup_id(id, &state)?;

        // 恢复后同步 live 配置
        if let Err(e) = crate::services::provider::ProviderService::sync_current_to_live(&state) {
            log::warn!("备份恢复后同步 live 配置失败: {e}");
        }

        println!(
            "{}",
            success(&format!("✓ Configuration restored from backup '{}'", id))
        );
        if !pre_restore_backup.is_empty() {
            println!(
                "{}",
                info(&format!("  Pre-restore backup: {}", pre_restore_backup))
            );
        }
        println!();
        println!(
            "{}",
            info("Note: Restart your CLI clients to apply the changes.")
        );

        return Ok(());
    }

    // 情况2：指定了文件路径
    if let Some(file) = file_path {
        println!(
            "{}",
            info(&format!(
                "Restoring configuration from {}...",
                file.display()
            ))
        );

        if !file.exists() {
            return Err(AppError::Message(format!(
                "File '{}' not found",
                file.display()
            )));
        }

        println!();
        println!("{}", highlight("Warning:"));
        println!("This will replace your current database with the SQL backup file.");
        println!("A backup of the current state will be created first.");
        println!();

        let confirm = inquire::Confirm::new(texts::config_restore_confirm_prompt())
            .with_default(false)
            .prompt()
            .map_err(|e| AppError::Message(format!("Prompt failed: {}", e)))?;

        if !confirm {
            println!("{}", info("Cancelled."));
            return Ok(());
        }

        let state = get_state()?;
        let pre_restore_backup = ConfigService::import_config_from_path(file, &state)?;

        // 恢复后同步 live 配置
        if let Err(e) = crate::services::provider::ProviderService::sync_current_to_live(&state) {
            log::warn!("配置恢复后同步 live 配置失败: {e}");
        }

        println!(
            "{}",
            success(&format!("✓ Configuration restored from {}", file.display()))
        );
        if !pre_restore_backup.is_empty() {
            println!(
                "{}",
                info(&format!("  Pre-restore backup: {}", pre_restore_backup))
            );
        }
        println!();
        println!(
            "{}",
            info("Note: Restart your CLI clients to apply the changes.")
        );

        return Ok(());
    }

    // 情况3：无参数，显示交互式列表
    println!("{}", highlight(texts::available_backups()));
    println!("{}", "=".repeat(50));

    let backups = ConfigService::list_backups(&config_path)?;

    if backups.is_empty() {
        println!();
        println!("{}", info(texts::no_backups_found()));
        println!("{}", info(texts::create_backup_first_hint()));
        return Ok(());
    }

    println!();
    println!("{}", texts::found_backups(backups.len()));
    println!();

    let choices: Vec<String> = backups
        .iter()
        .map(|b| format!("{} - {}", b.display_name, b.id))
        .collect();

    let selection = inquire::Select::new(texts::select_backup_to_restore(), choices)
        .prompt()
        .map_err(|_| AppError::Message(texts::selection_cancelled().to_string()))?;

    let selected_backup = backups
        .iter()
        .find(|b| selection.contains(&b.id))
        .ok_or_else(|| AppError::Message(texts::invalid_selection().to_string()))?;

    println!();
    println!("{}", highlight(texts::warning_title()));
    println!("{}", texts::config_restore_warning_replace());
    println!("{}", texts::config_restore_warning_pre_backup());
    println!();

    let confirm = inquire::Confirm::new(texts::config_restore_confirm_prompt())
        .with_default(false)
        .prompt()
        .map_err(|e| AppError::Message(format!("Prompt failed: {}", e)))?;

    if !confirm {
        println!("{}", info(texts::cancelled()));
        return Ok(());
    }

    let state = get_state()?;
    let pre_restore_backup = ConfigService::restore_from_backup_id(&selected_backup.id, &state)?;

    // 恢复后同步 live 配置
    if let Err(e) = crate::services::provider::ProviderService::sync_current_to_live(&state) {
        log::warn!("备份恢复后同步 live 配置失败: {e}");
    }

    println!(
        "{}",
        success(&format!(
            "✓ Configuration restored from: {}",
            selected_backup.display_name
        ))
    );
    if !pre_restore_backup.is_empty() {
        println!(
            "{}",
            info(&format!("  Pre-restore backup: {}", pre_restore_backup))
        );
    }
    println!();
    println!(
        "{}",
        info("Note: Restart your CLI clients to apply the changes.")
    );

    Ok(())
}

fn validate_config() -> Result<(), AppError> {
    let config_dir = crate::config::get_app_config_dir();
    let db_path = config_dir.join("cc-switch.db");

    println!("{}", info("Validating database..."));
    println!();

    if !db_path.exists() {
        println!("{}", error("✗ Database file does not exist"));
        println!("Path: {}", db_path.display());
        return Ok(());
    }

    println!("{} Database file exists", success("✓"));
    println!("Path: {}", db_path.display());

    let db = crate::Database::init()?;
    println!("{} Database schema is readable", success("✓"));

    // Show some stats
    let claude_count = db.get_all_providers("claude")?.len();
    let codex_count = db.get_all_providers("codex")?.len();
    let gemini_count = db.get_all_providers("gemini")?.len();
    let mcp_count = db.get_all_mcp_servers()?.len();
    let skills_count = db.get_all_installed_skills()?.len();

    println!();
    println!("{}", highlight("Database Summary:"));
    println!("Claude providers:  {}", claude_count);
    println!("Codex providers:   {}", codex_count);
    println!("Gemini providers:  {}", gemini_count);
    println!("MCP servers:       {}", mcp_count);
    println!("Skills installed:  {}", skills_count);

    println!();
    println!("{}", success("✓ Database validation passed"));

    Ok(())
}

fn reset_config() -> Result<(), AppError> {
    println!("{}", highlight("Reset Configuration"));
    println!("{}", "=".repeat(50));
    println!();
    println!("{}", highlight("Warning:"));
    println!("This will delete your current configuration and create a fresh default one.");
    println!("All your providers, MCP servers, and settings will be lost.");
    println!();
    println!("{}", info("Consider creating a backup first:"));
    println!("  cc-switch config backup");
    println!();

    let confirm = inquire::Confirm::new("Are you sure you want to reset to default configuration?")
        .with_default(false)
        .prompt()
        .map_err(|e| AppError::Message(format!("Prompt failed: {}", e)))?;

    if !confirm {
        println!("{}", info("Cancelled."));
        return Ok(());
    }

    // Create a backup before reset (SQL)
    let config_path = crate::config::get_app_config_path();
    let backup_id = ConfigService::create_backup(&config_path, None)?;

    // Delete the database file
    let db_path = crate::config::get_app_config_dir().join("cc-switch.db");
    if db_path.exists() {
        fs::remove_file(&db_path).map_err(|e| AppError::io(&db_path, e))?;
    }

    // Recreate empty DB
    let _ = crate::Database::init()?;

    println!("{}", success("✓ Configuration reset to defaults"));
    if !backup_id.is_empty() {
        println!("{}", info(&format!("  Backup created: {}", backup_id)));
        println!(
            "{}",
            info("  You can restore it later using: cc-switch config restore")
        );
    }

    Ok(())
}
