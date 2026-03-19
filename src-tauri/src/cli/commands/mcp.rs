use clap::Subcommand;

use crate::app_config::{AppType, McpApps, McpServer};
use crate::cli::ui::{create_table, error, highlight, info, success};
use crate::error::AppError;
use crate::services::McpService;
use crate::store::AppState;

#[derive(Subcommand)]
pub enum McpCommand {
    /// List all MCP servers
    List,
    /// Add a new MCP server (interactive)
    Add,
    /// Edit an MCP server
    Edit {
        /// Server ID to edit
        id: String,
    },
    /// Delete an MCP server
    Delete {
        /// Server ID to delete
        id: String,
    },
    /// Enable an MCP server for specific app(s)
    Enable {
        /// Server ID to enable
        id: String,
    },
    /// Disable an MCP server for specific app(s)
    Disable {
        /// Server ID to disable
        id: String,
    },
    /// Validate a command is in PATH
    Validate {
        /// Command to validate
        command: String,
    },
    /// Sync MCP configuration to live files
    Sync,
    /// Import MCP servers from live configuration
    Import,
}

pub fn execute(cmd: McpCommand, app: Option<AppType>) -> Result<(), AppError> {
    let app_type = app.unwrap_or(AppType::Claude);

    match cmd {
        McpCommand::List => list_servers(app_type),
        McpCommand::Add => add_server(app_type),
        McpCommand::Edit { id } => edit_server(app_type, &id),
        McpCommand::Delete { id } => delete_server(&id),
        McpCommand::Enable { id } => enable_server(app_type, &id),
        McpCommand::Disable { id } => disable_server(app_type, &id),
        McpCommand::Validate { command } => validate_command(&command),
        McpCommand::Sync => sync_servers(),
        McpCommand::Import => import_servers(app_type),
    }
}

fn get_state() -> Result<AppState, AppError> {
    AppState::try_new()
}

fn list_servers(app_type: AppType) -> Result<(), AppError> {
    let state = get_state()?;
    let servers = McpService::get_all_servers(&state)?;

    if servers.is_empty() {
        println!("{}", info("No MCP servers found."));
        println!("Use 'cc-switch mcp add' or 'cc-switch mcp import' to add servers.");
        return Ok(());
    }

    // 创建表格
    let mut table = create_table();
    table.set_header(vec!["ID", "Name", "Claude", "Codex", "Gemini", "Tags"]);

    // 按 ID 排序
    let mut server_list: Vec<_> = servers.into_iter().collect();
    server_list.sort_by(|(a, _), (b, _)| a.cmp(b));

    for (id, server) in server_list {
        let claude_marker = if server.apps.claude { "✓" } else { " " };
        let codex_marker = if server.apps.codex { "✓" } else { " " };
        let gemini_marker = if server.apps.gemini { "✓" } else { " " };
        let tags = server.tags.join(", ");

        let row = vec![
            id.clone(),
            server.name.clone(),
            claude_marker.to_string(),
            codex_marker.to_string(),
            gemini_marker.to_string(),
            tags,
        ];

        table.add_row(row);
    }

    println!("{}", table);
    println!(
        "\n{} Viewing from: {} perspective",
        info("ℹ"),
        app_type.as_str()
    );
    println!("{} ✓ = Enabled for this app", info("→"));

    Ok(())
}

fn delete_server(id: &str) -> Result<(), AppError> {
    let state = get_state()?;

    // 检查服务器是否存在
    let servers = McpService::get_all_servers(&state)?;
    let server = servers
        .get(id)
        .ok_or_else(|| AppError::Message(format!("MCP server '{}' not found", id)))?;

    // 显示将要删除的服务器信息
    println!("{}", highlight("Server to be deleted:"));
    println!("ID:   {}", id);
    println!("Name: {}", server.name);

    let enabled_apps: Vec<&str> = vec![
        if server.apps.claude {
            Some("Claude")
        } else {
            None
        },
        if server.apps.codex {
            Some("Codex")
        } else {
            None
        },
        if server.apps.gemini {
            Some("Gemini")
        } else {
            None
        },
    ]
    .into_iter()
    .flatten()
    .collect();

    if !enabled_apps.is_empty() {
        println!("Enabled for: {}", enabled_apps.join(", "));
    }
    println!();

    // 确认删除
    let confirm = inquire::Confirm::new(&format!(
        "Are you sure you want to delete MCP server '{}'?",
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
    let deleted = McpService::delete_server(&state, id)?;

    if deleted {
        println!("{}", success(&format!("✓ Deleted MCP server '{}'", id)));
        if !enabled_apps.is_empty() {
            println!(
                "{}",
                info(&format!("  Removed from: {}", enabled_apps.join(", ")))
            );
        }
    } else {
        println!("{}", error(&format!("Failed to delete server '{}'", id)));
    }

    Ok(())
}

fn enable_server(app_type: AppType, id: &str) -> Result<(), AppError> {
    let state = get_state()?;
    let app_str = app_type.as_str().to_string();

    // 检查服务器是否存在
    let servers = McpService::get_all_servers(&state)?;
    if !servers.contains_key(id) {
        return Err(AppError::Message(format!("MCP server '{}' not found", id)));
    }

    // 执行启用
    McpService::toggle_app(&state, id, app_type, true)?;

    println!(
        "{}",
        success(&format!("✓ Enabled MCP server '{}' for {}", id, app_str))
    );
    println!(
        "{}",
        info("Note: Configuration has been synced to live file.")
    );

    Ok(())
}

fn disable_server(app_type: AppType, id: &str) -> Result<(), AppError> {
    let state = get_state()?;
    let app_str = app_type.as_str().to_string();

    // 检查服务器是否存在
    let servers = McpService::get_all_servers(&state)?;
    if !servers.contains_key(id) {
        return Err(AppError::Message(format!("MCP server '{}' not found", id)));
    }

    // 执行禁用
    McpService::toggle_app(&state, id, app_type, false)?;

    println!(
        "{}",
        success(&format!("✓ Disabled MCP server '{}' for {}", id, app_str))
    );
    println!(
        "{}",
        info("Note: Configuration has been removed from live file.")
    );

    Ok(())
}

fn sync_servers() -> Result<(), AppError> {
    let state = get_state()?;

    println!("{}", info("Syncing all enabled MCP servers..."));

    McpService::sync_all_enabled(&state)?;

    println!("{}", success("✓ All MCP servers synced successfully"));
    println!(
        "{}",
        info("Note: Live configuration files have been updated.")
    );

    Ok(())
}

fn import_servers(app_type: AppType) -> Result<(), AppError> {
    let state = get_state()?;
    let app_str = app_type.as_str().to_string();

    println!(
        "{}",
        info(&format!(
            "Importing MCP servers from {} live config...",
            app_str
        ))
    );

    let count = match app_type {
        AppType::Claude => McpService::import_from_claude(&state)?,
        AppType::Codex => McpService::import_from_codex(&state)?,
        AppType::Gemini => McpService::import_from_gemini(&state)?,
        AppType::OpenCode => 0,
        AppType::OpenClaw => 0,
    };

    if count > 0 {
        println!(
            "{}",
            success(&format!(
                "✓ Imported {} MCP server(s) from {}",
                count, app_str
            ))
        );
        println!(
            "{}",
            info("Note: Servers have been added to unified configuration.")
        );
    } else {
        println!(
            "{}",
            info(&format!("No new MCP servers found in {} config.", app_str))
        );
    }

    Ok(())
}

fn add_server(_app_type: AppType) -> Result<(), AppError> {
    let state = get_state()?;

    let mut apps = McpApps::default();
    apps.set_enabled_for(&_app_type, true);

    let template = McpServer {
        id: String::new(),
        name: String::new(),
        server: serde_json::json!({
            "command": "",
            "args": [],
        }),
        apps,
        description: None,
        homepage: None,
        docs: None,
        tags: vec![],
    };
    let initial = serde_json::to_string_pretty(&template)
        .map_err(|e| AppError::Message(format!("failed to serialize template: {e}")))?;

    println!("{}", highlight("Add New MCP Server"));
    println!("{}", info("Opening external editor..."));
    let edited = crate::cli::editor::open_external_editor(&initial)?;

    let server: McpServer = serde_json::from_str(&edited)
        .map_err(|e| AppError::Message(format!("invalid JSON: {e}")))?;
    if server.id.trim().is_empty() || server.name.trim().is_empty() {
        return Err(AppError::InvalidInput(
            "missing required fields: id, name".to_string(),
        ));
    }

    McpService::upsert_server(&state, server)?;

    println!("{}", success("✓ MCP server saved"));
    println!(
        "{}",
        info("Tip: Use 'cc-switch mcp list' to view all servers.")
    );
    Ok(())
}

fn edit_server(_app_type: AppType, id: &str) -> Result<(), AppError> {
    let state = get_state()?;
    let servers = McpService::get_all_servers(&state)?;
    let Some(existing) = servers.get(id).cloned() else {
        return Err(AppError::InvalidInput(format!(
            "MCP server '{id}' not found"
        )));
    };

    let initial = serde_json::to_string_pretty(&existing)
        .map_err(|e| AppError::Message(format!("failed to serialize server: {e}")))?;

    println!("{}", info(&format!("Editing MCP server '{}'...", id)));
    println!("{}", info("Opening external editor..."));
    let edited = crate::cli::editor::open_external_editor(&initial)?;

    if edited.trim_end() == initial.trim_end() {
        println!("{}", info("No changes detected."));
        return Ok(());
    }

    let mut server: McpServer = serde_json::from_str(&edited)
        .map_err(|e| AppError::Message(format!("invalid JSON: {e}")))?;
    server.id = id.to_string();
    if server.name.trim().is_empty() {
        return Err(AppError::InvalidInput(
            "missing required field: name".to_string(),
        ));
    }

    McpService::upsert_server(&state, server)?;

    println!("{}", success("✓ MCP server updated"));
    Ok(())
}

fn validate_command(command: &str) -> Result<(), AppError> {
    println!("{}", info(&format!("Validating command '{}'...", command)));

    // 检查命令是否在 PATH 中
    if which::which(command).is_ok() {
        println!(
            "{}",
            success(&format!("✓ Command '{}' is available in PATH", command))
        );
    } else {
        println!(
            "{}",
            error(&format!("✗ Command '{}' not found in PATH", command))
        );
        println!(
            "{}",
            info("Make sure the command is installed and accessible.")
        );
    }

    Ok(())
}
