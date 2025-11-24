use clap::Subcommand;
use std::sync::RwLock;

use crate::app_config::{AppType, MultiAppConfig};
use crate::cli::ui::{create_table, error, highlight, info, success};
use crate::error::AppError;
use crate::services::PromptService;
use crate::store::AppState;

#[derive(Subcommand)]
pub enum PromptsCommand {
    /// List all prompt presets
    List,
    /// Show current active prompt
    Current,
    /// Activate a prompt preset
    Activate {
        /// Prompt preset ID
        id: String,
    },
    /// Deactivate the current active prompt
    Deactivate,
    /// Create a new prompt preset
    Create,
    /// Edit a prompt preset
    Edit {
        /// Prompt preset ID
        id: String,
    },
    /// Delete a prompt preset
    Delete {
        /// Prompt preset ID
        id: String,
    },
    /// Show prompt content
    Show {
        /// Prompt preset ID
        id: String,
    },
}

pub fn execute(cmd: PromptsCommand, app: Option<AppType>) -> Result<(), AppError> {
    let app_type = app.unwrap_or(AppType::Claude);

    match cmd {
        PromptsCommand::List => list_prompts(app_type),
        PromptsCommand::Current => show_current(app_type),
        PromptsCommand::Activate { id } => activate_prompt(app_type, &id),
        PromptsCommand::Deactivate => deactivate_prompt(app_type),
        PromptsCommand::Create => create_prompt(app_type),
        PromptsCommand::Edit { id } => edit_prompt(app_type, &id),
        PromptsCommand::Delete { id } => delete_prompt(app_type, &id),
        PromptsCommand::Show { id } => show_prompt(app_type, &id),
    }
}

fn get_state() -> Result<AppState, AppError> {
    let config = MultiAppConfig::load()?;
    Ok(AppState {
        config: RwLock::new(config),
    })
}

fn list_prompts(app_type: AppType) -> Result<(), AppError> {
    let state = get_state()?;
    let prompts = PromptService::get_prompts(&state, app_type.clone())?;

    if prompts.is_empty() {
        println!("{}", info("No prompt presets found."));
        println!("Use 'cc-switch prompts create' to create a new prompt preset.");
        return Ok(());
    }

    // 创建表格
    let mut table = create_table();
    table.set_header(vec!["", "ID", "Name", "Description", "Updated"]);

    // 按更新时间排序
    let mut prompt_list: Vec<_> = prompts.into_iter().collect();
    prompt_list.sort_by(|(_, a), (_, b)| b.updated_at.unwrap_or(0).cmp(&a.updated_at.unwrap_or(0)));

    for (id, prompt) in prompt_list {
        let enabled_marker = if prompt.enabled { "✓" } else { " " };
        let updated = prompt
            .updated_at
            .map(|ts| {
                use chrono::{DateTime, Utc};
                DateTime::<Utc>::from_timestamp(ts, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "Unknown".to_string())
            })
            .unwrap_or_else(|| "Unknown".to_string());

        let description = prompt
            .description
            .as_deref()
            .unwrap_or("")
            .chars()
            .take(40)
            .collect::<String>();
        let description = if prompt.description.as_ref().map(|d| d.len()).unwrap_or(0) > 40 {
            format!("{}...", description)
        } else {
            description
        };

        let row = vec![
            enabled_marker.to_string(),
            id.clone(),
            prompt.name.clone(),
            description,
            updated,
        ];

        table.add_row(row);
    }

    println!("{}", table);
    println!("\n{} Application: {}", info("ℹ"), app_type.as_str());
    println!("{} ✓ = Currently active", info("→"));

    Ok(())
}

fn show_current(app_type: AppType) -> Result<(), AppError> {
    let state = get_state()?;
    let prompts = PromptService::get_prompts(&state, app_type.clone())?;

    // 找到当前激活的 prompt
    let active = prompts
        .iter()
        .find(|(_, p)| p.enabled)
        .map(|(id, p)| (id.clone(), p.clone()));

    match active {
        Some((id, prompt)) => {
            let updated = prompt
                .updated_at
                .and_then(|ts| {
                    use chrono::{DateTime, Utc};
                    DateTime::<Utc>::from_timestamp(ts, 0)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                })
                .unwrap_or_else(|| "Unknown".to_string());

            println!("{}", highlight("Current Active Prompt"));
            println!("{}", "=".repeat(50));
            println!("ID:          {}", id);
            println!("Name:        {}", prompt.name);
            if let Some(desc) = &prompt.description {
                println!("Description: {}", desc);
            }
            println!("Updated:     {}", updated);
            println!("App:         {}", app_type.as_str());
            println!();
            println!("{}", highlight("Content Preview:"));
            println!("{}", "-".repeat(50));

            // 显示内容预览（前 10 行）
            let lines: Vec<&str> = prompt.content.lines().collect();
            let preview_lines = lines.iter().take(10);
            for line in preview_lines {
                println!("{}", line);
            }

            if lines.len() > 10 {
                println!("...");
                println!("{}", info(&format!("({} more lines)", lines.len() - 10)));
            }
        }
        None => {
            println!("{}", info("No active prompt preset."));
            println!("Use 'cc-switch prompts activate <id>' to activate a prompt.");
        }
    }

    Ok(())
}

fn activate_prompt(app_type: AppType, id: &str) -> Result<(), AppError> {
    let state = get_state()?;
    let app_str = app_type.as_str().to_string();

    // 检查 prompt 是否存在
    let prompts = PromptService::get_prompts(&state, app_type.clone())?;
    if !prompts.contains_key(id) {
        return Err(AppError::Message(format!(
            "Prompt preset '{}' not found",
            id
        )));
    }

    // 执行激活
    PromptService::enable_prompt(&state, app_type, id)?;

    println!(
        "{}",
        success(&format!("✓ Activated prompt preset '{}'", id))
    );
    println!("{}", info(&format!("  Application: {}", app_str)));
    println!();
    println!(
        "{}",
        info("Note: The prompt has been synced to the live configuration file.")
    );

    Ok(())
}

fn delete_prompt(app_type: AppType, id: &str) -> Result<(), AppError> {
    let state = get_state()?;

    // 检查 prompt 是否存在
    let prompts = PromptService::get_prompts(&state, app_type.clone())?;
    let prompt = prompts
        .get(id)
        .ok_or_else(|| AppError::Message(format!("Prompt preset '{}' not found", id)))?;

    // 检查是否是当前激活的 prompt
    if prompt.enabled {
        return Err(AppError::Message(
            "Cannot delete the currently active prompt. Please activate another prompt first."
                .to_string(),
        ));
    }

    // 显示将要删除的 prompt 信息
    println!("{}", highlight("Prompt to be deleted:"));
    println!("ID:   {}", id);
    println!("Name: {}", prompt.name);
    if let Some(desc) = &prompt.description {
        println!("Desc: {}", desc);
    }
    println!();

    // 确认删除
    let confirm = inquire::Confirm::new(&format!(
        "Are you sure you want to delete prompt preset '{}'?",
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
    PromptService::delete_prompt(&state, app_type, id)?;

    println!("{}", success(&format!("✓ Deleted prompt preset '{}'", id)));

    Ok(())
}

fn show_prompt(app_type: AppType, id: &str) -> Result<(), AppError> {
    let state = get_state()?;
    let prompts = PromptService::get_prompts(&state, app_type)?;

    let prompt = prompts
        .get(id)
        .ok_or_else(|| AppError::Message(format!("Prompt preset '{}' not found", id)))?;

    let updated = prompt
        .updated_at
        .and_then(|ts| {
            use chrono::{DateTime, Utc};
            DateTime::<Utc>::from_timestamp(ts, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        })
        .unwrap_or_else(|| "Unknown".to_string());

    println!("{}", highlight(&format!("Prompt Preset: {}", prompt.name)));
    println!("{}", "=".repeat(50));
    println!("ID:          {}", id);
    println!("Name:        {}", prompt.name);
    if let Some(desc) = &prompt.description {
        println!("Description: {}", desc);
    }
    println!(
        "Status:      {}",
        if prompt.enabled {
            highlight("Active")
        } else {
            "Inactive".to_string()
        }
    );
    println!("Updated:     {}", updated);
    println!();
    println!("{}", highlight("Content:"));
    println!("{}", "-".repeat(50));
    println!("{}", prompt.content);
    println!("{}", "-".repeat(50));
    println!("Lines: {}", prompt.content.lines().count());
    println!("Size:  {} bytes", prompt.content.len());

    Ok(())
}

fn create_prompt(_app_type: AppType) -> Result<(), AppError> {
    println!("{}", highlight("Create New Prompt Preset"));
    println!("{}", "=".repeat(50));
    println!();
    println!("{}", info("Note: Prompt creation requires text editing."));
    println!("{}", info("For now, you can:"));
    println!();
    println!("1. Edit the configuration file directly:");
    println!("   ~/.cc-switch/config.json");
    println!();
    println!("2. Or add a prompt preset manually to the prompts section.");
    println!();
    println!(
        "{}",
        error("Interactive prompt creation is not yet fully implemented.")
    );
    println!("{}", info("Coming soon in the next update!"));

    Ok(())
}

fn deactivate_prompt(app_type: AppType) -> Result<(), AppError> {
    let state = get_state()?;
    let prompts = PromptService::get_prompts(&state, app_type.clone())?;

    // Find currently enabled prompt
    let active = prompts
        .iter()
        .find(|(_, p)| p.enabled)
        .map(|(id, _)| id.clone());

    match active {
        Some(id) => {
            // Deactivate the current prompt
            PromptService::disable_prompt(&state, app_type.clone(), &id)?;

            println!(
                "{}",
                success(&format!("✓ Deactivated prompt preset '{}'", id))
            );
            println!("{}", info(&format!("  Application: {}", app_type.as_str())));
            println!();
            println!(
                "{}",
                info("Note: The live configuration file has been cleared.")
            );
        }
        None => {
            println!("{}", info("No active prompt to deactivate."));
            println!("Use 'cc-switch prompts activate <id>' to activate a prompt preset.");
        }
    }

    Ok(())
}

fn edit_prompt(_app_type: AppType, id: &str) -> Result<(), AppError> {
    println!("{}", info(&format!("Editing prompt preset '{}'...", id)));
    println!("{}", error("Prompt editing is not yet implemented."));
    println!(
        "{}",
        info("Please edit ~/.cc-switch/config.json directly for now.")
    );
    println!();
    println!(
        "{}",
        info("Tip: Use 'cc-switch prompts show <id>' to view the current content.")
    );
    Ok(())
}
