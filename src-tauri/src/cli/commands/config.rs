use clap::Subcommand;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::app_config::MultiAppConfig;
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
}

pub fn execute(cmd: ConfigCommand) -> Result<(), AppError> {
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
    }
}

fn get_state() -> Result<AppState, AppError> {
    let config = MultiAppConfig::load()?;
    Ok(AppState {
        config: RwLock::new(config),
    })
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

fn show_path() -> Result<(), AppError> {
    let config_path = crate::config::get_app_config_path();
    let config_dir = crate::config::get_app_config_dir();

    println!("{}", highlight("Configuration Paths"));
    println!("{}", "=".repeat(50));
    println!("Config file:  {}", config_path.display());
    println!("Config dir:   {}", config_dir.display());

    // Check if config file exists
    if config_path.exists() {
        println!("\n{} Configuration file exists", success("✓"));

        // Show file size
        if let Ok(metadata) = fs::metadata(&config_path) {
            println!("File size:    {} bytes", metadata.len());
        }
    } else {
        println!("\n{} Configuration file does not exist", error("✗"));
        println!("{}", info("Run cc-switch to create default configuration."));
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

    // Validate the file is valid JSON
    let content = fs::read_to_string(file).map_err(|e| AppError::io(file, e))?;
    let _: MultiAppConfig = serde_json::from_str(&content)
        .map_err(|e| AppError::Message(format!("Invalid configuration file: {}", e)))?;

    // Confirm import
    println!();
    println!("{}", highlight("Warning:"));
    println!("This will replace your current configuration with the imported one.");
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

    if !config_path.exists() {
        println!("{}", error("Configuration file does not exist."));
        println!("{}", info("Nothing to backup."));
        return Ok(());
    }

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
        let backup_file = backup_dir.join(format!("{}.json", backup_id));

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

        let content = fs::read_to_string(file).map_err(|e| AppError::io(file, e))?;
        let _: MultiAppConfig = serde_json::from_str(&content)
            .map_err(|e| AppError::Message(format!("Invalid configuration file: {}", e)))?;

        println!();
        println!("{}", highlight("Warning:"));
        println!("This will replace your current configuration with the file.");
        println!("A backup of the current state will be created first.");
        println!();

        let confirm = inquire::Confirm::new("Continue with restore?")
            .with_default(false)
            .prompt()
            .map_err(|e| AppError::Message(format!("Prompt failed: {}", e)))?;

        if !confirm {
            println!("{}", info("Cancelled."));
            return Ok(());
        }

        let pre_restore_backup = ConfigService::create_backup(&config_path, None)?;
        let state = get_state()?;
        let _ = ConfigService::import_config_from_path(file, &state)?;

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
    println!("{}", highlight("Available Backups"));
    println!("{}", "=".repeat(50));

    let backups = ConfigService::list_backups(&config_path)?;

    if backups.is_empty() {
        println!();
        println!("{}", info("No backups found."));
        println!("{}", info("Create a backup first: cc-switch config backup"));
        return Ok(());
    }

    println!();
    println!("Found {} backup(s):", backups.len());
    println!();

    let choices: Vec<String> = backups
        .iter()
        .map(|b| format!("{} - {}", b.display_name, b.id))
        .collect();

    let selection = inquire::Select::new("Select backup to restore:", choices)
        .prompt()
        .map_err(|_| AppError::Message("Selection cancelled".to_string()))?;

    let selected_backup = backups
        .iter()
        .find(|b| selection.contains(&b.id))
        .ok_or_else(|| AppError::Message("Invalid selection".to_string()))?;

    println!();
    println!("{}", highlight("Warning:"));
    println!("This will replace your current configuration with the selected backup.");
    println!("A backup of the current state will be created first.");
    println!();

    let confirm = inquire::Confirm::new("Continue with restore?")
        .with_default(false)
        .prompt()
        .map_err(|e| AppError::Message(format!("Prompt failed: {}", e)))?;

    if !confirm {
        println!("{}", info("Cancelled."));
        return Ok(());
    }

    let state = get_state()?;
    let pre_restore_backup = ConfigService::restore_from_backup_id(&selected_backup.id, &state)?;

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
    let config_path = crate::config::get_app_config_path();

    println!("{}", info("Validating configuration..."));
    println!();

    // Check if file exists
    if !config_path.exists() {
        println!("{}", error("✗ Configuration file does not exist"));
        println!("Path: {}", config_path.display());
        return Ok(());
    }

    println!("{} Configuration file exists", success("✓"));
    println!("Path: {}", config_path.display());

    // Try to load and parse the configuration
    match MultiAppConfig::load() {
        Ok(config) => {
            println!("{} Configuration is valid JSON", success("✓"));

            // Show some stats
            let claude_count = config
                .get_manager(&crate::app_config::AppType::Claude)
                .map(|m| m.providers.len())
                .unwrap_or(0);
            let codex_count = config
                .get_manager(&crate::app_config::AppType::Codex)
                .map(|m| m.providers.len())
                .unwrap_or(0);
            let gemini_count = config
                .get_manager(&crate::app_config::AppType::Gemini)
                .map(|m| m.providers.len())
                .unwrap_or(0);
            let mcp_count = config.mcp.servers.as_ref().map(|s| s.len()).unwrap_or(0);

            println!();
            println!("{}", highlight("Configuration Summary:"));
            println!("Claude providers:  {}", claude_count);
            println!("Codex providers:   {}", codex_count);
            println!("Gemini providers:  {}", gemini_count);
            println!("MCP servers:       {}", mcp_count);

            println!();
            println!("{}", success("✓ Configuration validation passed"));
        }
        Err(e) => {
            println!("{} Configuration parsing failed", error("✗"));
            println!("Error: {}", e);
            return Err(e);
        }
    }

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

    // Create a backup before reset
    let config_path = crate::config::get_app_config_path();
    let backup_id = ConfigService::create_backup(&config_path, None)?;

    // Delete the current config file
    if config_path.exists() {
        fs::remove_file(&config_path).map_err(|e| AppError::io(&config_path, e))?;
    }

    // Force reload will create a new default config
    let _ = MultiAppConfig::load()?;

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
