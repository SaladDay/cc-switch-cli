//! One catalog mutation owns its changes and retained native recovery.

use cc_switch_core::{
    builtin_app_registry,
    fs::{shared_live_config_lock_path, SharedLiveConfigLock, SharedLiveConfigLockError},
    McpNativeSnapshot,
};
use cc_switch_store::McpTransactionGuard;

use super::*;
use crate::database::{shared_store_error, Database};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum NativeAction {
    Enable,
    Disable,
    Sync,
}

pub(super) struct NativeChange<'a> {
    pub id: &'a str,
    pub server: &'a serde_json::Value,
    pub action: NativeAction,
    pub previous_snapshot: Option<&'a McpNativeSnapshot>,
}

impl NativeChange<'_> {
    pub(super) fn enabled(&self) -> bool {
        self.action != NativeAction::Disable
    }
}

pub(super) trait NativeToggle {
    /// Prepare all entries against one observation and publish at most once.
    /// Results correspond to the input order. Recovery remains owned by `self`.
    fn apply_batch(
        &mut self,
        changes: &[NativeChange<'_>],
    ) -> Result<Vec<Option<McpNativeSnapshot>>, AppError>;

    fn apply(
        &mut self,
        id: &str,
        server: &serde_json::Value,
        enabled: bool,
        previous_snapshot: Option<&McpNativeSnapshot>,
    ) -> Result<Option<McpNativeSnapshot>, AppError> {
        self.apply_batch(&[NativeChange {
            id,
            server,
            previous_snapshot,
            action: if enabled {
                NativeAction::Enable
            } else {
                NativeAction::Disable
            },
        }])?
        .pop()
        .ok_or_else(|| AppError::Config("Missing MCP native result".into()))
    }

    /// Refresh an enabled catalog entry without forcing host-native activation.
    fn sync(
        &mut self,
        id: &str,
        server: &serde_json::Value,
        previous_snapshot: Option<&McpNativeSnapshot>,
    ) -> Result<Option<McpNativeSnapshot>, AppError> {
        self.apply_batch(&[NativeChange {
            id,
            server,
            previous_snapshot,
            action: NativeAction::Sync,
        }])?
        .pop()
        .ok_or_else(|| AppError::Config("Missing MCP native result".into()))
    }

    fn rollback(&mut self) -> Result<(), AppError>;
}

enum CatalogMutation {
    Select(Vec<(AppType, bool)>),
    Delete,
    Upsert(McpServer),
}

impl McpService {
    pub(super) fn select_coordinated(
        state: &AppState,
        id: &str,
        selections: impl FnOnce(&McpApps) -> Vec<(AppType, bool)>,
    ) -> Result<bool, AppError> {
        Self::mutate_coordinated(state, id, |apps| CatalogMutation::Select(selections(apps)))
    }

    pub(super) fn delete_coordinated(state: &AppState, id: &str) -> Result<bool, AppError> {
        Self::mutate_coordinated(state, id, |_| CatalogMutation::Delete)
    }

    pub(super) fn upsert_coordinated(state: &AppState, server: McpServer) -> Result<(), AppError> {
        let id = server.id.clone();
        Self::mutate_coordinated(state, &id, |_| CatalogMutation::Upsert(server)).map(|_| ())
    }

    fn mutate_coordinated(
        state: &AppState,
        id: &str,
        mutation: impl FnOnce(&McpApps) -> CatalogMutation,
    ) -> Result<bool, AppError> {
        let mut config = state.config.write()?;
        let mut connection = state.db.conn.lock().map_err(AppError::from)?;
        // Acquire after the DB transaction, but release after its rollback on
        // early returns. Native recovery must finish before either is released.
        let _shared_lock;
        let mut transaction =
            McpTransactionGuard::begin(&mut connection).map_err(shared_store_error)?;
        let row = transaction.read_server(id).map_err(shared_store_error)?;
        let current = row.clone().map(Database::mcp_server_from_shared_row);
        let before = current
            .as_ref()
            .map(|server| &server.apps)
            .cloned()
            .unwrap_or_default();
        let mutation = mutation(&before);
        if row.is_none() && !matches!(mutation, CatalogMutation::Upsert(_)) {
            transaction.commit().map_err(shared_store_error)?;
            if let Some(servers) = config.mcp.servers.as_mut() {
                servers.remove(id);
            }
            return Ok(false);
        }
        let mut fingerprint = row.as_ref().map(|row| *row.source_fingerprint());
        let delete = matches!(mutation, CatalogMutation::Delete);
        let upsert = matches!(mutation, CatalogMutation::Upsert(_));
        let (mut server, selections) = match mutation {
            CatalogMutation::Upsert(server) => {
                // Preserve removal-before-sync order. Unchanged enabled Apps
                // still receive edits; unchanged disabled Apps are untouched.
                let removals = Self::supported_mcp_apps()
                    .filter(|app| before.is_enabled_for(app) && !server.apps.is_enabled_for(app))
                    .map(|app| (app, false));
                let updates = Self::supported_mcp_apps()
                    .filter(|app| server.apps.is_enabled_for(app))
                    .map(|app| (app, true));
                let selections = removals.chain(updates).collect::<Vec<_>>();
                (server, selections)
            }
            CatalogMutation::Select(selections) => {
                (current.expect("existing selection target"), selections)
            }
            CatalogMutation::Delete => {
                let selections = Self::supported_mcp_apps()
                    .filter(|app| before.is_enabled_for(app))
                    .map(|app| (app, false))
                    .collect();
                (current.expect("existing deletion target"), selections)
            }
        };
        // Decide initialization before any write can create another App's directory.
        let selections: Vec<_> = selections
            .into_iter()
            .map(|(app, enabled)| {
                let live = crate::sync_policy::should_sync_live(&app);
                (app, enabled, live)
            })
            .collect();
        _shared_lock = if selections.iter().any(|(_, _, live)| *live) {
            Some(lock_live_config()?)
        } else {
            None
        };
        let mut native: Vec<(AppType, Box<dyn NativeToggle>)> = Vec::new();
        let result = (|| {
            if upsert {
                let config = serde_json::to_string(&server.server)
                    .map_err(|source| AppError::JsonSerialize { source })?;
                let tags = serde_json::to_string(&server.tags)
                    .map_err(|source| AppError::JsonSerialize { source })?;
                let values = Database::mcp_catalog_values(&server, &config, &tags, row.as_ref());
                match fingerprint.as_ref() {
                    Some(fingerprint) => transaction.update_server_catalog(fingerprint, &values),
                    None => transaction.insert_server_catalog(&values),
                }
                .map_err(shared_store_error)?;
            }
            for (app, enabled, live) in selections {
                let core_app = app.as_core();
                let column = builtin_app_registry()
                    .for_app(&core_app)
                    .mcp_contract()
                    .ok_or_else(|| AppError::Config("App does not declare MCP support".into()))?
                    .catalog_column();
                // Deletion owns the row and its cascading links. A stale or
                // future snapshot is not input to native entry removal.
                // Catalog-only edits also leave opaque snapshots untouched.
                let link = if delete || (upsert && !live) {
                    None
                } else {
                    transaction
                        .read_native_link(id, core_app.as_str())
                        .map_err(shared_store_error)?
                };
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
                    let snapshot = if upsert && enabled {
                        operation.sync(id, &server.server, previous_snapshot.as_ref())?
                    } else {
                        operation.apply(id, &server.server, enabled, previous_snapshot.as_ref())?
                    };
                    if !delete && (enabled || link.is_some() || snapshot.is_some()) {
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
                if !delete && !upsert {
                    transaction
                        .set_server_selection(
                            id,
                            fingerprint.as_ref().expect("existing selection target"),
                            column,
                            enabled,
                        )
                        .map_err(shared_store_error)?;
                    fingerprint = Some(
                        *transaction
                            .read_server(id)
                            .map_err(shared_store_error)?
                            .ok_or_else(|| {
                                AppError::Database("MCP selection target disappeared".into())
                            })?
                            .source_fingerprint(),
                    );
                    server.apps.set_enabled_for(&app, enabled);
                }
            }
            if delete {
                transaction
                    .delete_server(id, fingerprint.as_ref().expect("existing deletion target"))
                    .map_err(shared_store_error)?;
            }
            Ok(())
        })();
        let action = if delete {
            "deletion"
        } else if upsert {
            "upsert"
        } else {
            "toggle"
        };
        finish_operation(transaction, &mut native, result, action)?;
        // Publish only the target cache row after commit. Other host workflows
        // still own their own cache refresh and persistence boundaries.
        if delete {
            if let Some(servers) = config.mcp.servers.as_mut() {
                servers.remove(id);
            }
        } else {
            config
                .mcp
                .servers
                .get_or_insert_with(HashMap::new)
                .insert(id.to_owned(), server);
        }
        Ok(true)
    }
}

pub(super) fn lock_live_config() -> Result<SharedLiveConfigLock, AppError> {
    SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(&crate::config::get_home_dir()))
        .map_err(|error| match error {
            SharedLiveConfigLockError::Unavailable => {
                AppError::Conflict("Live configuration is locked by another operation".into())
            }
            SharedLiveConfigLockError::Io { path, source } => AppError::io(path, source),
        })
}

/// Callers retain their native bindings and file lock through commit or recovery.
pub(super) fn finish_operation(
    transaction: McpTransactionGuard<'_>,
    native: &mut [(AppType, Box<dyn NativeToggle>)],
    result: Result<(), AppError>,
    action: &str,
) -> Result<(), AppError> {
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
        if let Err(error) = transaction.rollback() {
            failures.push(format!("database rollback: {error}"));
        }
        if !failures.is_empty() {
            return Err(AppError::Config(format!(
                "MCP {action} failed: {error}; {}",
                failures.join("; ")
            )));
        }
        return Err(error);
    }
    Ok(())
}

/// One factory serves standalone catalog mutations and whole-catalog sync.
pub(super) fn observe_native(app: &AppType) -> Result<Box<dyn NativeToggle>, AppError> {
    Ok(match app {
        AppType::Claude => Box::new(claude_toggle::ClaudeToggle::observe()?),
        AppType::Codex => Box::new(codex_toggle::CodexToggle::observe()?),
        AppType::Gemini => Box::new(gemini_toggle::observe()?),
        AppType::OpenCode => Box::new(opencode_toggle::OpenCodeToggle::observe()?),
        AppType::Hermes => Box::new(hermes_toggle::HermesToggle::observe()?),
        _ => return Err(AppError::Config("App has no MCP selection binding".into())),
    })
}
