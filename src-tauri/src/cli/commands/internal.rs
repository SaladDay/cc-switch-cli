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
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| AppError::Message(e.to_string()))?;
            // Capture's DB write, accepted-result read and managed import must be
            // serialized together with activation and managed credential refresh.
            let _mutation_guard = runtime
                .block_on(crate::services::state_coordination::acquire_restore_mutation_guard())
                .map_err(AppError::Message)?;
            let db = crate::Database::init()?;
            if auth_only {
                ProviderService::capture_codex_launch_auth(&db, &provider_id, &codex_home)?;
            } else {
                let state = AppState::try_new()?;
                ProviderService::capture_codex_temp_launch_snapshot(
                    &state,
                    &provider_id,
                    &codex_home,
                )?;
            }
            // Use the accepted DB snapshot, not a launch file rejected by optimistic concurrency.
            let providers = db.get_all_providers("codex")?;
            if let Some(provider) = providers
                .get(&provider_id)
                .filter(|p| ProviderService::codex_live_write_category(p) == Some("official"))
            {
                if let Some(auth) = provider.settings_config.get("auth") {
                    runtime
                        .block_on(crate::services::codex_account::capture_native_auth_locked(
                            auth,
                        ))
                        .map_err(AppError::Message)?;
                }
            }
            Ok(())
        }
    }
}
