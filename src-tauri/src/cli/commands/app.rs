use crate::app_config::AppType;
use crate::error::AppError;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum AppCommand {
    /// Show current application selection
    Current,
    /// Switch to a specific application
    Use {
        /// Application to use
        #[arg(value_enum)]
        app: AppType,
    },
    /// List all supported applications
    List,
}

pub fn execute(cmd: AppCommand) -> Result<(), AppError> {
    match cmd {
        AppCommand::Current => show_current(),
        AppCommand::Use { app } => use_app(app),
        AppCommand::List => list_apps(),
    }
}

fn show_current() -> Result<(), AppError> {
    println!("Showing current app...");
    Ok(())
}

fn use_app(_app: AppType) -> Result<(), AppError> {
    println!("Switching app...");
    Ok(())
}

fn list_apps() -> Result<(), AppError> {
    println!("Listing supported apps...");
    Ok(())
}
