use clap::Subcommand;

use crate::cli::ui::{highlight, info, success};
use crate::error::AppError;
use crate::AppState;

/// Global upstream proxy management commands
#[derive(Subcommand, Debug, Clone)]
pub enum UpstreamCommand {
    /// Show current upstream proxy configuration and status
    Show,

    /// Enable upstream proxy (requires URL to be already configured)
    Enable,

    /// Disable upstream proxy (clear proxy setting)
    Disable,
}

pub fn execute(cmd: UpstreamCommand) -> Result<(), AppError> {
    match cmd {
        UpstreamCommand::Show => show_upstream_proxy(),
        UpstreamCommand::Enable => enable_upstream_proxy(),
        UpstreamCommand::Disable => disable_upstream_proxy(),
    }
}

fn get_state() -> Result<AppState, AppError> {
    AppState::try_new()
}

fn create_runtime() -> Result<tokio::runtime::Runtime, AppError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("failed to create async runtime: {e}")))
}

fn show_upstream_proxy() -> Result<(), AppError> {
    let state = get_state()?;
    let enabled = state.db.get_global_proxy_enabled()?;
    let url = state.db.get_global_proxy_url()?;
    let http_client_url = crate::proxy::http_client::get_current_proxy_url();

    println!("{}", highlight("Upstream Proxy Configuration"));
    println!("{}", "=".repeat(40));

    println!("Status: {}", if enabled { info("Enabled") } else { info("Disabled") });

    if let Some(url) = url {
        println!("URL: {}", url);
    } else {
        println!("URL: {}", info("Not configured"));
    }

    println!("HTTP client active: {}", http_client_url.as_deref().unwrap_or("(direct connection)"));

    Ok(())
}

fn enable_upstream_proxy() -> Result<(), AppError> {
    let state = get_state()?;

    // Set enabled to true
    state.db.set_global_proxy_enabled(true)?;

    // Get URL from database
    let url = state.db.get_global_proxy_url()?;

    // Update HTTP client based on URL
    let effective_url = url.as_deref().filter(|u| !u.trim().is_empty());
    crate::proxy::http_client::update_proxy(effective_url)
        .map_err(|e| AppError::Message(format!("Failed to update HTTP client: {}", e)))?;

    if let Some(url) = effective_url {
        println!("{}", success(&format!("Upstream proxy enabled: {}", url)));
    } else {
        println!("{}", success("Upstream proxy enabled (no URL configured)"));
    }
    Ok(())
}


fn disable_upstream_proxy() -> Result<(), AppError> {
    let state = get_state()?;

    // Set enabled to false
    state.db.set_global_proxy_enabled(false)?;

    // Update HTTP client (direct connection)
    crate::proxy::http_client::update_proxy(None)
        .map_err(|e| AppError::Message(format!("Failed to update HTTP client: {}", e)))?;

    println!("{}", success("Upstream proxy disabled (direct connection)"));
    Ok(())
}