use inquire::{Confirm, Select};

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::cli::ui::{create_table, highlight, info, success};
use crate::error::AppError;
use crate::services::PromptService;
use crate::store::AppState;

use super::utils::{get_state, pause};

pub fn manage_prompts_menu(app_type: &AppType) -> Result<(), AppError> {
    loop {
        println!("\n{}", highlight(texts::prompts_management()));
        println!("{}", "─".repeat(60));

        let state = get_state()?;
        let prompts = PromptService::get_prompts(&state, app_type.clone())?;

        if prompts.is_empty() {
            println!("{}", info(texts::no_prompts()));
        } else {
            let mut table = create_table();
            table.set_header(vec!["", texts::header_name(), texts::header_description()]);

            let mut prompt_list: Vec<_> = prompts.iter().collect();
            prompt_list.sort_by(|(_, a), (_, b)| {
                b.updated_at.unwrap_or(0).cmp(&a.updated_at.unwrap_or(0))
            });

            for (_, prompt) in &prompt_list {
                let marker = if prompt.enabled { "✓" } else { " " };
                let name = if prompt.enabled {
                    format!("* {}", prompt.name)
                } else {
                    format!("  {}", prompt.name)
                };
                let desc = prompt
                    .description
                    .as_deref()
                    .unwrap_or("")
                    .chars()
                    .take(40)
                    .collect::<String>();

                table.add_row(vec![marker.to_string(), name, desc]);
            }

            println!("{}", table);
        }

        println!();
        let choices = vec![
            texts::prompts_view_current(),
            texts::switch_active_prompt(),
            texts::prompts_show_content(),
            texts::prompts_delete(),
            texts::back_to_main(),
        ];

        let choice = Select::new(texts::choose_action(), choices)
            .prompt()
            .map_err(|_| AppError::Message("Selection cancelled".to_string()))?;

        if choice == texts::prompts_view_current() {
            view_current_prompt_interactive(&state, app_type, &prompts)?;
        } else if choice == texts::switch_active_prompt() {
            switch_prompt_interactive(&state, app_type, &prompts)?;
        } else if choice == texts::prompts_show_content() {
            show_prompt_content_interactive(&prompts)?;
        } else if choice == texts::prompts_delete() {
            delete_prompt_interactive(&state, app_type, &prompts)?;
        } else {
            break;
        }
    }

    Ok(())
}

fn view_current_prompt_interactive(
    _state: &AppState,
    _app_type: &AppType,
    prompts: &std::collections::HashMap<String, crate::prompt::Prompt>,
) -> Result<(), AppError> {
    let active = prompts.iter().find(|(_, p)| p.enabled);

    if let Some((id, prompt)) = active {
        println!(
            "\n{}",
            highlight(texts::prompts_view_current().trim_start_matches("📋 "))
        );
        println!("{}", "─".repeat(60));
        println!("ID:          {}", id);
        println!("Name:        {}", prompt.name);
        if let Some(desc) = &prompt.description {
            println!("Description: {}", desc);
        }
        println!();
        println!("Content:");
        println!("{}", "─".repeat(60));
        println!("{}", prompt.content);
    } else {
        println!("\n{}", info(texts::no_active_prompt()));
    }

    pause();
    Ok(())
}

fn show_prompt_content_interactive(
    prompts: &std::collections::HashMap<String, crate::prompt::Prompt>,
) -> Result<(), AppError> {
    if prompts.is_empty() {
        println!("\n{}", info(texts::no_prompts_available()));
        pause();
        return Ok(());
    }

    let prompt_choices: Vec<_> = prompts
        .iter()
        .map(|(id, p)| format!("{} ({})", p.name, id))
        .collect();

    let selected = Select::new(texts::select_prompt_to_view(), prompt_choices)
        .prompt()
        .map_err(|_| AppError::Message("Selection cancelled".to_string()))?;

    let prompt_id = selected
        .split('(')
        .nth(1)
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| AppError::Message("Invalid selection".to_string()))?;

    if let Some(prompt) = prompts.get(prompt_id) {
        println!("\n{}", highlight(&prompt.name));
        println!("{}", "─".repeat(60));
        if let Some(desc) = &prompt.description {
            println!("Description: {}", desc);
            println!();
        }
        println!("{}", prompt.content);
    }

    pause();
    Ok(())
}

fn delete_prompt_interactive(
    state: &AppState,
    app_type: &AppType,
    prompts: &std::collections::HashMap<String, crate::prompt::Prompt>,
) -> Result<(), AppError> {
    let deletable: Vec<_> = prompts
        .iter()
        .filter(|(_, p)| !p.enabled)
        .map(|(id, p)| format!("{} ({})", p.name, id))
        .collect();

    if deletable.is_empty() {
        println!("\n{}", info(texts::no_prompts_to_delete()));
        if prompts.iter().any(|(_, p)| p.enabled) {
            println!("{}", info(texts::cannot_delete_active()));
        }
        pause();
        return Ok(());
    }

    let selected = Select::new(texts::select_prompt_to_delete(), deletable)
        .prompt()
        .map_err(|_| AppError::Message("Selection cancelled".to_string()))?;

    let prompt_id = selected
        .split('(')
        .nth(1)
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| AppError::Message("Invalid selection".to_string()))?;

    let confirm = Confirm::new(&texts::confirm_delete(prompt_id))
        .with_default(false)
        .prompt()
        .map_err(|_| AppError::Message("Confirmation failed".to_string()))?;

    if !confirm {
        println!("\n{}", info(texts::cancelled()));
        pause();
        return Ok(());
    }

    PromptService::delete_prompt(state, app_type.clone(), prompt_id)?;
    println!("\n{}", success(&texts::prompt_deleted(prompt_id)));
    pause();
    Ok(())
}

fn switch_prompt_interactive(
    state: &AppState,
    app_type: &AppType,
    prompts: &std::collections::HashMap<String, crate::prompt::Prompt>,
) -> Result<(), AppError> {
    if prompts.is_empty() {
        println!("\n{}", info(texts::no_prompts_available()));
        pause();
        return Ok(());
    }

    let prompt_choices: Vec<_> = prompts
        .iter()
        .map(|(id, p)| format!("{} ({})", p.name, id))
        .collect();

    let choice = Select::new(texts::select_prompt_to_activate(), prompt_choices)
        .prompt()
        .map_err(|_| AppError::Message("Selection cancelled".to_string()))?;

    let id = choice
        .split('(')
        .nth(1)
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| AppError::Message("Invalid choice".to_string()))?;

    // 检查是否选择了已激活的提示词（Toggle 功能）
    if let Some(prompt) = prompts.get(id) {
        if prompt.enabled {
            // 取消激活
            PromptService::disable_prompt(state, app_type.clone(), id)?;
            println!("\n{}", success(&texts::deactivated_prompt(id)));
            println!("{}", info(texts::prompt_cleared_note()));
            pause();
            return Ok(());
        }
    }

    // 激活提示词
    PromptService::enable_prompt(state, app_type.clone(), id)?;

    println!("\n{}", success(&texts::activated_prompt(id)));
    println!("{}", info(texts::prompt_synced_note()));
    pause();

    Ok(())
}
