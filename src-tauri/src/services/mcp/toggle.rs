//! One selection operation owns its catalog changes and retained native recovery.

use cc_switch_core::{
    builtin_app_registry,
    fs::{shared_live_config_lock_path, SharedLiveConfigLock, SharedLiveConfigLockError},
    McpNativeSnapshot,
};
use cc_switch_store::McpTransactionGuard;

use super::*;
use crate::database::{shared_store_error, Database};

pub(super) trait NativeToggle {
    fn apply(
        &mut self,
        id: &str,
        server: &serde_json::Value,
        enabled: bool,
        previous_snapshot: Option<&McpNativeSnapshot>,
    ) -> Result<Option<McpNativeSnapshot>, AppError>;

    fn rollback(&mut self) -> Result<(), AppError>;
}

impl McpService {
    pub(super) fn select_coordinated(
        state: &AppState,
        id: &str,
        selections: impl FnOnce(&McpApps) -> Vec<(AppType, bool)>,
    ) -> Result<bool, AppError> {
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
            return Ok(false);
        };
        let mut fingerprint = *row.source_fingerprint();
        let mut server = Database::mcp_server_from_shared_row(row);
        // Decide initialization before any write can create another App's directory.
        let selections: Vec<_> = selections(&server.apps)
            .into_iter()
            .map(|(app, enabled)| {
                let live = crate::sync_policy::should_sync_live(&app);
                (app, enabled, live)
            })
            .collect();
        _shared_lock = if selections.iter().any(|(_, _, live)| *live) {
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
        let mut native: Vec<(AppType, Box<dyn NativeToggle>)> = Vec::new();
        let result = (|| {
            for (app, enabled, live) in selections {
                let core_app = app.as_core();
                let column = builtin_app_registry()
                    .for_app(&core_app)
                    .mcp_contract()
                    .ok_or_else(|| AppError::Config("App does not declare MCP support".into()))?
                    .catalog_column();
                let link = transaction
                    .read_native_link(id, core_app.as_str())
                    .map_err(shared_store_error)?;
                let previous_snapshot: Option<McpNativeSnapshot> = link
                    .as_ref()
                    .and_then(|link| link.native_snapshot.as_deref())
                    .map(serde_json::from_str)
                    .transpose()
                    .map_err(|_| AppError::Database("Invalid MCP native snapshot".into()))?;
                if live {
                    // Observe in write order, including earlier publications when
                    // custom paths alias. Retain every receipt and local lock until
                    // commit or reverse-order recovery, including a failed apply.
                    let operation = observe_native(&app)?;
                    native.push((app.clone(), operation));
                    let operation = &mut native.last_mut().expect("inserted operation").1;
                    let snapshot =
                        operation.apply(id, &server.server, enabled, previous_snapshot.as_ref())?;
                    if enabled || link.is_some() || snapshot.is_some() {
                        let snapshot = snapshot
                            .as_ref()
                            .map(serde_json::to_string)
                            .transpose()
                            .map_err(|source| AppError::JsonSerialize { source })?;
                        transaction
                            .upsert_native_link(id, core_app.as_str(), snapshot.as_deref())
                            .map_err(shared_store_error)?;
                    }
                }
                transaction
                    .set_server_selection(id, &fingerprint, column, enabled)
                    .map_err(shared_store_error)?;
                fingerprint = *transaction
                    .read_server(id)
                    .map_err(shared_store_error)?
                    .ok_or_else(|| AppError::Database("MCP selection target disappeared".into()))?
                    .source_fingerprint();
                server.apps.set_enabled_for(&app, enabled);
            }
            Ok(())
        })();
        let failure = match result {
            Ok(()) => match transaction.commit_preserving_on_error() {
                Ok(()) => None,
                Err((transaction, error)) => Some((transaction, shared_store_error(error))),
            },
            Err(error) => Some((transaction, error)),
        };
        if let Some((transaction, error)) = failure {
            let mut failures = Vec::new();
            for (app, operation) in native.iter_mut().rev() {
                if let Err(error) = operation.rollback() {
                    failures.push(format!("native recovery for {}: {error}", app.as_str()));
                }
            }
            let database_error = transaction.rollback().err();
            if let Some(error) = database_error {
                failures.push(format!("database rollback: {error}"));
            }
            if !failures.is_empty() {
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
        Ok(true)
    }
}

/// The host's native bindings are shared by single and multi-App selection.
fn observe_native(app: &AppType) -> Result<Box<dyn NativeToggle>, AppError> {
    Ok(match app {
        AppType::Claude => Box::new(claude_toggle::ClaudeToggle::observe()?),
        AppType::Codex => Box::new(codex_toggle::CodexToggle::observe()?),
        AppType::Gemini => Box::new(gemini_toggle::observe()?),
        AppType::OpenCode => Box::new(opencode_toggle::OpenCodeToggle::observe()?),
        AppType::Hermes => Box::new(hermes_toggle::HermesToggle::observe()?),
        _ => return Err(AppError::Config("App has no MCP selection binding".into())),
    })
}
