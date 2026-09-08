//! Whole-catalog synchronization owns one App's publication and native links.

use cc_switch_core::McpNativeSnapshot;
use cc_switch_store::{McpNativeLinkRow, McpTransactionGuard};

use super::{
    toggle::{finish_operation, lock_live_config, observe_native, NativeAction, NativeChange},
    *,
};
use crate::database::{shared_store_error, Database};

impl McpService {
    pub(super) fn sync_catalog_for_app(
        state: &AppState,
        app: &AppType,
        include_disabled: bool,
    ) -> Result<(), AppError> {
        if !app.supports_mcp() || !crate::sync_policy::should_sync_live(app) {
            return Ok(());
        }
        // Sync does not mutate the host cache. No state lock is acquired while
        // holding the database or native locks, including during recovery.
        let mut connection = state.db.conn.lock().map_err(AppError::from)?;
        let _shared_lock;
        let mut transaction =
            McpTransactionGuard::begin(&mut connection).map_err(shared_store_error)?;
        let servers = transaction
            .read_servers()
            .map_err(shared_store_error)?
            .into_iter()
            .map(Database::mcp_server_from_shared_row)
            .filter(|server| include_disabled || server.apps.is_enabled_for(app))
            .collect::<Vec<_>>();
        if servers.is_empty() {
            return transaction.commit().map_err(shared_store_error);
        }
        let batch = CatalogBatch::prepare(&mut transaction, app, &servers)?;
        _shared_lock = lock_live_config()?;
        let mut native = [(app.clone(), observe_native(app)?)];
        let result = batch.apply(&mut transaction, |changes| native[0].1.apply_batch(changes));
        finish_operation(transaction, &mut native, result, "sync")
    }
}

/// The public command and provider-owned tail share projection/link ownership,
/// while their callers retain different commit and recovery boundaries.
pub(super) struct CatalogBatch<'a> {
    app: &'a AppType,
    servers: &'a [McpServer],
    links: Vec<Option<McpNativeLinkRow>>,
    previous: Vec<Option<McpNativeSnapshot>>,
}

impl<'a> CatalogBatch<'a> {
    pub(super) fn prepare(
        transaction: &mut McpTransactionGuard<'_>,
        app: &'a AppType,
        servers: &'a [McpServer],
    ) -> Result<Self, AppError> {
        let links = servers
            .iter()
            .map(|server| {
                transaction
                    .read_native_link(&server.id, app.as_str())
                    .map_err(shared_store_error)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let previous = links
            .iter()
            .map(|link| {
                link.as_ref()
                    .and_then(|link| link.native_snapshot.as_deref())
                    .map(serde_json::from_str::<McpNativeSnapshot>)
                    .transpose()
                    .map_err(|_| AppError::Database("Invalid MCP native snapshot".into()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            app,
            servers,
            links,
            previous,
        })
    }

    pub(super) fn apply(
        &self,
        transaction: &mut McpTransactionGuard<'_>,
        publish: impl FnOnce(&[NativeChange<'_>]) -> Result<Vec<Option<McpNativeSnapshot>>, AppError>,
    ) -> Result<(), AppError> {
        let changes = self
            .servers
            .iter()
            .zip(&self.previous)
            .map(|(server, snapshot)| NativeChange {
                id: &server.id,
                server: &server.server,
                previous_snapshot: snapshot.as_ref(),
                action: if server.apps.is_enabled_for(self.app) {
                    NativeAction::Sync
                } else {
                    NativeAction::Disable
                },
            })
            .collect::<Vec<_>>();
        let snapshots = publish(&changes)?;
        if snapshots.len() != changes.len() {
            return Err(AppError::Config(
                "Incomplete MCP native batch result".into(),
            ));
        }
        for ((change, link), snapshot) in changes.iter().zip(&self.links).zip(snapshots) {
            if change.enabled() || link.is_some() || snapshot.is_some() {
                let snapshot = if snapshot.as_ref() == change.previous_snapshot {
                    // An unchanged removal snapshot remains opaque, including
                    // envelope fields added by a newer consumer.
                    link.as_ref().and_then(|link| link.native_snapshot.clone())
                } else {
                    snapshot
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()
                        .map_err(|source| AppError::JsonSerialize { source })?
                };
                transaction
                    .upsert_native_link(change.id, self.app.as_str(), snapshot.as_deref())
                    .map_err(shared_store_error)?;
            }
        }
        Ok(())
    }
}
