use std::path::PathBuf;

use clap::Subcommand;

use crate::error::AppError;
use crate::services::ProviderService;
use crate::store::AppState;

#[derive(Subcommand)]
pub enum InternalCommand {
    /// Persist Codex files written during `cc-switch start codex`.
    CaptureCodexTemp {
        provider_id: String,
        codex_home: PathBuf,
    },
}

pub fn execute(cmd: InternalCommand) -> Result<(), AppError> {
    match cmd {
        InternalCommand::CaptureCodexTemp {
            provider_id,
            codex_home,
        } => {
            let state = AppState::try_new()?;
            ProviderService::capture_codex_temp_launch_snapshot(&state, &provider_id, &codex_home)
        }
    }
}
