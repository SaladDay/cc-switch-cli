use std::process::Command;

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::cli::ui::{create_table, error, highlight, info, success};
use crate::error::AppError;
use crate::services::McpService;
use crate::store::AppState;

use super::utils::{
    clear_screen, get_state, pause, prompt_confirm, prompt_multiselect,
    prompt_multiselect_with_help, prompt_select, prompt_text,
};

pub fn manage_mcp_menu(_app_type: &AppType) -> Result<(), AppError> {
    loop {
        clear_screen();
        println!("\n{}", highlight(texts::mcp_management()));
        println!("{}", "─".repeat(60));

        let state = get_state()?;
        let servers = McpService::get_all_servers(&state)?;

        if servers.is_empty() {
            println!("{}", info(texts::no_mcp_servers()));
        } else {
            let mut table = create_table();
            table.set_header(vec![texts::header_name(), "Claude", "Codex", "Gemini"]);

            let mut server_list: Vec<_> = servers.iter().collect();
            server_list.sort_by_key(|(id, _)| *id);

            for (_, server) in &server_list {
                table.add_row(vec![
                    server.name.clone(),
                    if server.apps.claude { "✓" } else { " " }.to_string(),
                    if server.apps.codex { "✓" } else { " " }.to_string(),
                    if server.apps.gemini { "✓" } else { " " }.to_string(),
                ]);
            }

            println!("{}", table);
        }

        println!();
        let choices = vec![
            texts::sync_all_servers(),
            texts::mcp_enable_server(),
            texts::mcp_disable_server(),
            texts::mcp_delete_server(),
            texts::mcp_import_servers(),
            texts::mcp_validate_command(),
            texts::back_to_main(),
        ];

        let Some(choice) = prompt_select(texts::choose_action(), choices)? else {
            break;
        };

        if choice == texts::sync_all_servers() {
            McpService::sync_all_enabled(&state)?;
            println!("\n{}", success(texts::synced_successfully()));
            pause();
        } else if choice == texts::mcp_enable_server() {
            mcp_enable_server_interactive(&state)?;
        } else if choice == texts::mcp_disable_server() {
            mcp_disable_server_interactive(&state)?;
        } else if choice == texts::mcp_delete_server() {
            mcp_delete_server_interactive(&state)?;
        } else if choice == texts::mcp_import_servers() {
            mcp_import_servers_interactive(&state)?;
        } else if choice == texts::mcp_validate_command() {
            mcp_validate_command_interactive()?;
        } else {
            break;
        }
    }

    Ok(())
}

fn mcp_enable_server_interactive(state: &AppState) -> Result<(), AppError> {
    clear_screen();
    let servers = McpService::get_all_servers(state)?;
    if servers.is_empty() {
        println!("\n{}", info(texts::no_mcp_servers()));
        pause();
        return Ok(());
    }

    let server_choices: Vec<_> = servers
        .iter()
        .map(|(id, s)| format!("{} ({})", s.name, id))
        .collect();

    let Some(selected) = prompt_select(texts::select_server_to_enable(), server_choices)? else {
        return Ok(());
    };

    let server_id = selected
        .split('(')
        .nth(1)
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| AppError::Message("Invalid selection".to_string()))?;

    let app_choices = vec!["Claude", "Codex", "Gemini"];
    let Some(selected_apps) = prompt_multiselect_with_help(
        texts::select_apps_to_enable(),
        app_choices,
        texts::mcp_enable_apps_help(),
    )?
    else {
        return Ok(());
    };

    let apps: Vec<AppType> = selected_apps
        .iter()
        .filter_map(|&s| match s {
            "Claude" => Some(AppType::Claude),
            "Codex" => Some(AppType::Codex),
            "Gemini" => Some(AppType::Gemini),
            _ => None,
        })
        .collect();

    for app in apps {
        McpService::toggle_app(state, server_id, app, true)?;
    }

    println!("\n{}", success(&texts::server_enabled(server_id)));
    pause();
    Ok(())
}

fn mcp_disable_server_interactive(state: &AppState) -> Result<(), AppError> {
    clear_screen();
    let servers = McpService::get_all_servers(state)?;
    if servers.is_empty() {
        println!("\n{}", info(texts::no_mcp_servers()));
        pause();
        return Ok(());
    }

    let server_choices: Vec<_> = servers
        .iter()
        .map(|(id, s)| format!("{} ({})", s.name, id))
        .collect();

    let Some(selected) = prompt_select(texts::select_server_to_disable(), server_choices)? else {
        return Ok(());
    };

    let server_id = selected
        .split('(')
        .nth(1)
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| AppError::Message("Invalid selection".to_string()))?;

    let app_choices = vec!["Claude", "Codex", "Gemini"];
    let Some(selected_apps) = prompt_multiselect_with_help(
        texts::select_apps_to_disable(),
        app_choices,
        texts::mcp_enable_apps_help(),
    )?
    else {
        return Ok(());
    };

    let apps: Vec<AppType> = selected_apps
        .iter()
        .filter_map(|&s| match s {
            "Claude" => Some(AppType::Claude),
            "Codex" => Some(AppType::Codex),
            "Gemini" => Some(AppType::Gemini),
            _ => None,
        })
        .collect();

    for app in apps {
        McpService::toggle_app(state, server_id, app, false)?;
    }

    println!("\n{}", success(&texts::server_disabled(server_id)));
    pause();
    Ok(())
}

fn mcp_delete_server_interactive(state: &AppState) -> Result<(), AppError> {
    clear_screen();
    let servers = McpService::get_all_servers(state)?;
    if servers.is_empty() {
        println!("\n{}", info(texts::no_servers_to_delete()));
        pause();
        return Ok(());
    }

    let server_choices: Vec<_> = servers
        .iter()
        .map(|(id, s)| format!("{} ({})", s.name, id))
        .collect();

    let Some(selected) = prompt_select(texts::select_server_to_delete(), server_choices)? else {
        return Ok(());
    };

    let server_id = selected
        .split('(')
        .nth(1)
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| AppError::Message("Invalid selection".to_string()))?;

    let confirm_prompt = texts::confirm_delete(server_id);
    let Some(confirm) = prompt_confirm(&confirm_prompt, false)? else {
        return Ok(());
    };

    if !confirm {
        println!("\n{}", info(texts::cancelled()));
        pause();
        return Ok(());
    }

    McpService::delete_server(state, server_id)?;
    println!("\n{}", success(&texts::server_deleted(server_id)));
    pause();
    Ok(())
}

fn mcp_import_servers_interactive(state: &AppState) -> Result<(), AppError> {
    clear_screen();
    let mut total = 0;
    total += McpService::import_from_claude(state)?;
    total += McpService::import_from_codex(state)?;
    total += McpService::import_from_gemini(state)?;

    println!("\n{}", success(&texts::servers_imported(total)));
    pause();
    Ok(())
}

fn mcp_validate_command_interactive() -> Result<(), AppError> {
    clear_screen();
    let Some(command) = prompt_text(texts::enter_command_to_validate())? else {
        return Ok(());
    };

    let is_valid = if cfg!(target_os = "windows") {
        Command::new("where")
            .arg(&command)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    } else {
        Command::new("which")
            .arg(&command)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };

    if is_valid {
        println!("\n{}", success(&texts::command_valid(&command)));
    } else {
        println!("\n{}", error(&texts::command_invalid(&command)));
    }

    pause();
    Ok(())
}
