//! The standalone Gemini toggle owns one selection and its native link.

use cc_switch_core::{
    builtin_app_registry,
    fs::{shared_live_config_lock_path, SharedLiveConfigLock, SharedLiveConfigLockError},
    McpNativeSnapshot,
};
use cc_switch_store::McpTransactionGuard;

#[cfg(test)]
mod tests;

use super::*;
use crate::{
    database::{shared_store_error, Database},
    gemini_config::{get_gemini_settings_path, operation::GeminiOperation},
};

impl McpService {
    pub(super) fn toggle_gemini_coordinated(
        state: &AppState,
        id: &str,
        enabled: bool,
    ) -> Result<(), AppError> {
        let mut config = state.config.write()?;
        let mut connection = state.db.conn.lock().map_err(AppError::from)?;
        // Acquire after the DB transaction, but release after its rollback on
        // early returns. Native recovery must finish before either is released.
        let _shared_lock;
        let mut transaction =
            McpTransactionGuard::begin(&mut connection).map_err(shared_store_error)?;
        let Some(row) = transaction.read_server(id).map_err(shared_store_error)? else {
            transaction.commit().map_err(shared_store_error)?;
            if let Some(servers) = config.mcp.servers.as_mut() {
                servers.remove(id);
            }
            return Ok(());
        };
        let fingerprint = *row.source_fingerprint();
        let mut server = Database::mcp_server_from_shared_row(row);
        server.apps.gemini = enabled;
        let app = AppType::Gemini.as_core();
        let column = builtin_app_registry()
            .for_app(&app)
            .mcp_contract()
            .expect("Gemini declares MCP support")
            .catalog_column();
        let link = transaction
            .read_native_link(id, app.as_str())
            .map_err(shared_store_error)?;
        let previous_snapshot: Option<McpNativeSnapshot> = link
            .as_ref()
            .and_then(|link| link.native_snapshot.as_deref())
            .map(serde_json::from_str)
            .transpose()
            .map_err(|_| AppError::Database("Invalid MCP native snapshot".into()))?;
        let sync_live = crate::sync_policy::should_sync_live(&AppType::Gemini);
        _shared_lock = if sync_live {
            Some(
                SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(
                    &crate::config::get_home_dir(),
                ))
                .map_err(|error| match error {
                    SharedLiveConfigLockError::Unavailable => AppError::Conflict(
                        "Live configuration is locked by another operation".into(),
                    ),
                    SharedLiveConfigLockError::Io { path, source } => AppError::io(path, source),
                })?,
            )
        } else {
            None
        };
        let mut native = if sync_live {
            Some(GeminiOperation::observe_settings(
                &get_gemini_settings_path(),
            )?)
        } else {
            None
        };
        let result = (|| {
            if let Some(native) = native.as_mut() {
                let snapshot = crate::gemini_mcp::toggle_with_operation(
                    native,
                    id,
                    &server.server,
                    enabled,
                    previous_snapshot.as_ref(),
                )?;
                if enabled || link.is_some() || snapshot.is_some() {
                    let snapshot = snapshot
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()
                        .map_err(|source| AppError::JsonSerialize { source })?;
                    transaction
                        .upsert_native_link(id, app.as_str(), snapshot.as_deref())
                        .map_err(shared_store_error)?;
                }
            }
            transaction
                .set_server_selection(id, &fingerprint, column, enabled)
                .map_err(shared_store_error)
        })();
        let failure = match result {
            Ok(()) => match transaction.commit_preserving_on_error() {
                Ok(()) => None,
                Err((transaction, error)) => Some((transaction, shared_store_error(error))),
            },
            Err(error) => Some((transaction, error)),
        };
        if let Some((transaction, error)) = failure {
            let native_error = native.as_mut().and_then(|native| native.rollback().err());
            let database_error = transaction.rollback().err();
            if native_error.is_some() || database_error.is_some() {
                let mut failures = Vec::new();
                if let Some(error) = native_error {
                    failures.push(format!("native recovery: {error}"));
                }
                if let Some(error) = database_error {
                    failures.push(format!("database rollback: {error}"));
                }
                return Err(AppError::Config(format!(
                    "MCP toggle failed: {error}; {}",
                    failures.join("; ")
                )));
            }
            return Err(error);
        }
        // Publish only the target cache row after commit. Other host workflows
        // still own their own cache refresh and persistence boundaries.
        config
            .mcp
            .servers
            .get_or_insert_with(HashMap::new)
            .insert(id.to_owned(), server);
        Ok(())
    }
}
