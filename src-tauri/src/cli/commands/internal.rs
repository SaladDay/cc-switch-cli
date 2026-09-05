use std::path::PathBuf;

use clap::Subcommand;

use crate::error::AppError;
use crate::services::ProviderService;
use crate::store::AppState;

#[derive(Subcommand)]
pub enum InternalCommand {
    /// Release an inherited launch lock after Codex and credential capture finish.
    #[cfg(unix)]
    ReleaseCodexLock { lock_fd: i32 },
    /// Persist Codex files written during `cc-switch start codex`.
    CaptureCodexTemp {
        provider_id: String,
        codex_home: PathBuf,
        /// Capture credentials without persisting launch-only configuration
        #[arg(long)]
        auth_only: bool,
    },
}

pub fn execute(cmd: InternalCommand) -> Result<(), AppError> {
    match cmd {
        #[cfg(unix)]
        InternalCommand::ReleaseCodexLock { lock_fd } => {
            if unsafe { libc::flock(lock_fd, libc::LOCK_UN) } != 0 {
                return Err(AppError::Config(format!(
                    "Failed to release Codex launch lock: {}",
                    std::io::Error::last_os_error()
                )));
            }
            Ok(())
        }
        InternalCommand::CaptureCodexTemp {
            provider_id,
            codex_home,
            auth_only,
        } => {
            if auth_only {
                return ProviderService::capture_codex_launch_auth(
                    &crate::Database::init()?,
                    &provider_id,
                    &codex_home,
                );
            }
            let state = AppState::try_new()?;
            ProviderService::capture_codex_temp_launch_snapshot(&state, &provider_id, &codex_home)
        }
    }
}
