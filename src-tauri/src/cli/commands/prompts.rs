use clap::{ArgAction, Subcommand, ValueEnum};

use crate::app_config::AppType;
use crate::cli::ui::{create_table, highlight, info, success};
use crate::error::AppError;
use crate::services::pi_prompt_files::{
    PiPromptFileKind, PiPromptFileService, PiPromptTemplateService,
};
use crate::services::PromptService;
use crate::store::AppState;

#[derive(Subcommand)]
pub enum PromptsCommand {
    /// List all prompt presets
    List,
    /// Show current active prompt
    Current,
    /// Show the current live prompt file content
    Live,
    /// Import the current live prompt file as an inactive preset
    Import,
    /// Activate a prompt preset
    Activate {
        /// Prompt preset ID
        id: String,
    },
    /// Deactivate the current active prompt
    Deactivate,
    /// Create a new prompt preset
    Create {
        /// Prompt preset ID
        #[arg(long)]
        id: Option<String>,
        /// Prompt preset name
        #[arg(long = "name", value_name = "NAME")]
        named: Option<String>,
        /// Prompt preset name (legacy positional form)
        name: Option<String>,
        /// Prompt preset description
        #[arg(long)]
        description: Option<String>,
    },
    /// Edit a prompt preset
    Edit {
        /// Prompt preset ID
        id: String,
    },
    /// Rename a prompt preset
    Rename {
        /// Existing prompt preset ID
        id: String,
        /// New prompt preset ID
        #[arg(long = "id")]
        new_id: Option<String>,
        /// New prompt name
        #[arg(long = "name", value_name = "NAME")]
        named: Option<String>,
        /// New prompt description
        #[arg(long, action = ArgAction::Set)]
        description: Option<String>,
        /// New prompt name (legacy positional form)
        name: Option<String>,
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
    /// Manage Pi's native SYSTEM.md or APPEND_SYSTEM.md
    System {
        #[command(subcommand)]
        command: PiSystemPromptCommand,
    },
    /// Manage Pi's native slash-command prompt templates
    Templates {
        #[command(subcommand)]
        command: PiPromptTemplatesCommand,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum PiSystemPromptKind {
    /// APPEND_SYSTEM.md (recommended)
    Append,
    /// SYSTEM.md (replaces Pi's built-in system prompt)
    Override,
}

impl From<PiSystemPromptKind> for PiPromptFileKind {
    fn from(value: PiSystemPromptKind) -> Self {
        match value {
            PiSystemPromptKind::Append => Self::SystemAppend,
            PiSystemPromptKind::Override => Self::SystemOverride,
        }
    }
}

#[derive(Subcommand)]
pub enum PiSystemPromptCommand {
    /// Show a native Pi system prompt file
    Show { kind: PiSystemPromptKind },
    /// Edit a native Pi system prompt file
    Edit { kind: PiSystemPromptKind },
    /// Delete a native Pi system prompt file
    Delete { kind: PiSystemPromptKind },
}

#[derive(Subcommand)]
pub enum PiPromptTemplatesCommand {
    /// List Pi prompt templates
    List,
    /// Show a Pi prompt template
    Show { slug: String },
    /// Create a Pi prompt template
    Create { slug: String },
    /// Edit or rename a Pi prompt template
    Edit {
        slug: String,
        #[arg(long)]
        new_slug: Option<String>,
    },
    /// Delete a Pi prompt template
    Delete { slug: String },
}

pub fn execute(cmd: PromptsCommand, app: Option<AppType>) -> Result<(), AppError> {
    let app_type = app.unwrap_or(AppType::Claude);

    match cmd {
        PromptsCommand::List => list_prompts(app_type),
        PromptsCommand::Current => show_current(app_type),
        PromptsCommand::Live => show_live_prompt(app_type),
        PromptsCommand::Import => import_prompt(app_type),
        PromptsCommand::Activate { id } => activate_prompt(app_type, &id),
        PromptsCommand::Deactivate => deactivate_prompt(app_type),
        PromptsCommand::Create {
            id,
            named,
            name,
            description,
        } => create_prompt(app_type, id, named.or(name), description),
        PromptsCommand::Edit { id } => edit_prompt(app_type, &id),
        PromptsCommand::Rename {
            id,
            new_id,
            named,
            description,
            name,
        } => rename_prompt(app_type, &id, new_id, named.or(name), description),
        PromptsCommand::Delete { id } => delete_prompt(app_type, &id),
        PromptsCommand::Show { id } => show_prompt(app_type, &id),
        PromptsCommand::System { command } => manage_pi_system_prompt(app_type, command),
        PromptsCommand::Templates { command } => manage_pi_prompt_templates(app_type, command),
    }
}

fn require_pi(app_type: &AppType, resource: &str) -> Result<(), AppError> {
    if matches!(app_type, AppType::Pi) {
        Ok(())
    } else {
        Err(AppError::InvalidInput(format!(
            "{resource} is only available with --app pi"
        )))
    }
}

fn manage_pi_system_prompt(
    app_type: AppType,
    command: PiSystemPromptCommand,
) -> Result<(), AppError> {
    require_pi(&app_type, "Pi native system prompts")?;
    let (kind, action) = match command {
        PiSystemPromptCommand::Show { kind } => (kind, "show"),
        PiSystemPromptCommand::Edit { kind } => (kind, "edit"),
        PiSystemPromptCommand::Delete { kind } => (kind, "delete"),
    };
    let filename = match kind {
        PiSystemPromptKind::Append => "APPEND_SYSTEM.md",
        PiSystemPromptKind::Override => "SYSTEM.md",
    };
    let native_kind = kind.into();

    match action {
        "show" => {
            let snapshot = PiPromptFileService::read(native_kind)?;
            if snapshot.exists {
                println!("{}", highlight(filename));
                println!("{}", snapshot.content);
            } else {
                println!("{}", info(&format!("{filename} does not exist.")));
            }
        }
        "edit" => {
            let snapshot = PiPromptFileService::read(native_kind)?;
            if matches!(kind, PiSystemPromptKind::Override) && !snapshot.exists {
                println!(
                    "{}",
                    info("SYSTEM.md replaces Pi's built-in system prompt; APPEND_SYSTEM.md is recommended for normal additions.")
                );
                let confirmed = inquire::Confirm::new("Create SYSTEM.md?")
                    .with_default(false)
                    .prompt()
                    .map_err(|error| AppError::Message(format!("Prompt failed: {error}")))?;
                if !confirmed {
                    println!("{}", info("Cancelled."));
                    return Ok(());
                }
            }
            let initial = if snapshot.exists {
                snapshot.content.as_str()
            } else {
                "# Write the Pi system prompt here\n"
            };
            let edited = crate::cli::editor::open_external_editor(initial)?;
            if snapshot.exists && edited == snapshot.content {
                println!("{}", info("No changes detected."));
                return Ok(());
            }
            PiPromptFileService::replace(native_kind, &snapshot.revision, &edited)?;
            println!("{}", success(&format!("✓ Saved {filename}")));
        }
        "delete" => {
            let snapshot = PiPromptFileService::read(native_kind)?;
            if !snapshot.exists {
                println!("{}", info(&format!("{filename} does not exist.")));
                return Ok(());
            }
            let confirmed = inquire::Confirm::new(&format!("Delete {filename}?"))
                .with_default(false)
                .prompt()
                .map_err(|error| AppError::Message(format!("Prompt failed: {error}")))?;
            if !confirmed {
                println!("{}", info("Cancelled."));
                return Ok(());
            }
            PiPromptFileService::delete(native_kind, &snapshot.revision)?;
            println!("{}", success(&format!("✓ Deleted {filename}")));
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn manage_pi_prompt_templates(
    app_type: AppType,
    command: PiPromptTemplatesCommand,
) -> Result<(), AppError> {
    require_pi(&app_type, "Pi prompt templates")?;
    let templates = PiPromptTemplateService::list()?;
    match command {
        PiPromptTemplatesCommand::List => {
            if templates.is_empty() {
                println!("{}", info("No Pi prompt templates found."));
            } else {
                let mut table = create_table();
                table.set_header(vec!["Slug", "Lines", "Size"]);
                for template in templates {
                    table.add_row(vec![
                        template.slug,
                        template.content.lines().count().to_string(),
                        format!("{} bytes", template.content.len()),
                    ]);
                }
                println!("{table}");
            }
        }
        PiPromptTemplatesCommand::Show { slug } => {
            let template = templates
                .into_iter()
                .find(|template| template.slug == slug)
                .ok_or_else(|| {
                    AppError::InvalidInput(format!("Pi prompt template '{slug}' not found"))
                })?;
            println!("{}", highlight(&template.slug));
            println!("{}", template.content);
        }
        PiPromptTemplatesCommand::Create { slug } => {
            if templates.iter().any(|template| template.slug == slug) {
                return Err(AppError::InvalidInput(format!(
                    "Pi prompt template '{slug}' already exists"
                )));
            }
            let edited =
                crate::cli::editor::open_external_editor("# Write the Pi prompt template here\n")?;
            PiPromptTemplateService::upsert(&slug, None, "missing", &edited)?;
            println!(
                "{}",
                success(&format!("✓ Created Pi prompt template '{slug}'"))
            );
        }
        PiPromptTemplatesCommand::Edit { slug, new_slug } => {
            let template = templates
                .into_iter()
                .find(|template| template.slug == slug)
                .ok_or_else(|| {
                    AppError::InvalidInput(format!("Pi prompt template '{slug}' not found"))
                })?;
            let target_slug = new_slug.as_deref().unwrap_or(&slug);
            let edited = crate::cli::editor::open_external_editor(&template.content)?;
            if target_slug == slug && edited == template.content {
                println!("{}", info("No changes detected."));
                return Ok(());
            }
            PiPromptTemplateService::upsert(target_slug, Some(&slug), &template.revision, &edited)?;
            println!(
                "{}",
                success(&format!("✓ Saved Pi prompt template '{target_slug}'"))
            );
        }
        PiPromptTemplatesCommand::Delete { slug } => {
            let template = templates
                .into_iter()
                .find(|template| template.slug == slug)
                .ok_or_else(|| {
                    AppError::InvalidInput(format!("Pi prompt template '{slug}' not found"))
                })?;
            let confirmed = inquire::Confirm::new(&format!("Delete Pi prompt template '{slug}'?"))
                .with_default(false)
                .prompt()
                .map_err(|error| AppError::Message(format!("Prompt failed: {error}")))?;
            if !confirmed {
                println!("{}", info("Cancelled."));
                return Ok(());
            }
            PiPromptTemplateService::delete(&slug, &template.revision)?;
            println!(
                "{}",
                success(&format!("✓ Deleted Pi prompt template '{slug}'"))
            );
        }
    }
    Ok(())
}

fn get_state() -> Result<AppState, AppError> {
    AppState::try_new()
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

fn show_live_prompt(app_type: AppType) -> Result<(), AppError> {
    let content = PromptService::get_current_file_content(app_type.clone())?;

    match content {
        Some(content) => {
            println!(
                "{}",
                highlight(&format!("Live Prompt File: {}", app_type.as_str()))
            );
            println!("{}", "=".repeat(50));
            if content.is_empty() {
                println!("{}", info("The live prompt file is empty."));
            } else {
                println!("{}", content);
            }
        }
        None => {
            println!(
                "{}",
                info(&format!(
                    "No live prompt file found for {}.",
                    app_type.as_str()
                ))
            );
        }
    }

    Ok(())
}

fn import_prompt(app_type: AppType) -> Result<(), AppError> {
    let state = get_state()?;
    let id = PromptService::import_from_file(&state, app_type.clone())?;
    let prompts = PromptService::get_prompts(&state, app_type.clone())?;
    let name = prompts
        .get(&id)
        .map(|prompt| prompt.name.as_str())
        .unwrap_or(id.as_str());

    println!(
        "{}",
        success(&format!("✓ Imported live prompt file as preset '{}'", id))
    );
    println!("{}", info(&format!("  Name: {}", name)));
    println!("{}", info(&format!("  Application: {}", app_type.as_str())));
    println!(
        "{}",
        info(&format!(
            "Tip: Use 'cc-switch prompts activate {}' to activate it.",
            id
        ))
    );

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

fn create_prompt(
    app_type: AppType,
    id: Option<String>,
    name: Option<String>,
    description: Option<String>,
) -> Result<(), AppError> {
    let state = get_state()?;
    let default_name = format!("Prompt {}", chrono::Local::now().format("%Y-%m-%d %H:%M"));
    let name = match name {
        Some(name) => name,
        None => inquire::Text::new("Prompt name:")
            .with_initial_value(&default_name)
            .prompt()
            .map_err(|e| AppError::Message(format!("Prompt failed: {}", e)))?,
    };
    let trimmed_name = name.trim();
    if trimmed_name.is_empty() {
        return Err(AppError::InvalidInput(
            "Prompt preset name cannot be empty".to_string(),
        ));
    }

    let initial = "# Write your prompt here\n";

    println!("{}", highlight("Create New Prompt Preset"));
    println!("{}", info("Opening external editor..."));

    let edited = crate::cli::editor::open_external_editor(initial)?;
    let prompt = PromptService::create_prompt_with_id(
        &state,
        app_type.clone(),
        id.as_deref(),
        trimmed_name,
        description.as_deref(),
        &edited,
    )?;

    println!(
        "{}",
        success(&format!("✓ Created prompt preset '{}'", prompt.id))
    );
    println!("{}", info(&format!("  Name: {}", prompt.name)));
    println!("{}", info(&format!("  Application: {}", app_type.as_str())));
    println!(
        "{}",
        info("Tip: Use 'cc-switch prompts list' to view all presets.")
    );
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
    let state = get_state()?;
    let prompts = PromptService::get_prompts(&state, _app_type.clone())?;
    let Some(mut prompt) = prompts.get(id).cloned() else {
        return Err(AppError::InvalidInput(format!(
            "Prompt preset '{id}' not found"
        )));
    };

    println!("{}", info(&format!("Editing prompt preset '{}'...", id)));
    println!("{}", info("Opening external editor..."));

    let edited = crate::cli::editor::open_external_editor(&prompt.content)?;

    if edited.trim_end() == prompt.content.trim_end() {
        println!("{}", info("No changes detected."));
        return Ok(());
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    prompt.content = edited.trim_end().to_string();
    prompt.updated_at = Some(timestamp);

    PromptService::upsert_prompt(&state, _app_type.clone(), id, prompt)?;

    println!("{}", success(&format!("✓ Updated prompt preset '{id}'")));
    Ok(())
}

fn rename_prompt(
    app_type: AppType,
    id: &str,
    new_id: Option<String>,
    name: Option<String>,
    description: Option<String>,
) -> Result<(), AppError> {
    let state = get_state()?;
    let prompts = PromptService::get_prompts(&state, app_type.clone())?;
    let Some(prompt) = prompts.get(id) else {
        return Err(AppError::InvalidInput(format!(
            "Prompt preset '{id}' not found"
        )));
    };

    let should_prompt_name = name.is_none() && new_id.is_none() && description.is_none();
    let new_name = match name {
        Some(name) => name,
        None if should_prompt_name => inquire::Text::new("New prompt name:")
            .with_initial_value(&prompt.name)
            .prompt()
            .map_err(|e| AppError::Message(format!("Prompt failed: {}", e)))?,
        None => prompt.name.clone(),
    };

    let trimmed = new_name.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidInput(
            "Prompt preset name cannot be empty".to_string(),
        ));
    }

    let new_id = new_id.unwrap_or_else(|| id.to_string());
    let next_description = description.or_else(|| prompt.description.clone());

    if new_id == id && trimmed == prompt.name && next_description == prompt.description {
        println!("{}", info("No changes detected."));
        return Ok(());
    }

    let prompt = PromptService::update_prompt_metadata(
        &state,
        app_type.clone(),
        id,
        &new_id,
        trimmed,
        next_description,
    )?;

    println!(
        "{}",
        success(&format!("✓ Updated prompt preset '{}'", prompt.id))
    );
    println!("{}", info(&format!("  Name: {}", prompt.name)));
    println!("{}", info(&format!("  Application: {}", app_type.as_str())));
    Ok(())
}
