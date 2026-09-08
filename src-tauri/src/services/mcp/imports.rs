//! Import into the fresh MCP catalog without saving a whole-product snapshot.

use cc_switch_core::{
    builtin_app_registry,
    fs::{shared_live_config_lock_path, SharedLiveConfigLock, SharedLiveConfigLockError},
};
use cc_switch_store::{McpServerCatalogValues, McpServerFields, McpTransactionGuard};

use super::*;
use crate::database::{shared_store_error, Database};

impl McpService {
    pub(super) fn import_coordinated(
        state: &AppState,
        app: AppType,
        import: fn(&mut MultiAppConfig) -> Result<usize, AppError>,
    ) -> Result<usize, AppError> {
        let mut config = state.config.write()?;
        let mut connection = state.db.conn.lock().map_err(AppError::from)?;
        // Keep the file lock until the database transaction has committed or
        // rolled back, including early returns from a host importer.
        let _shared_lock;
        let mut transaction =
            McpTransactionGuard::begin(&mut connection).map_err(shared_store_error)?;
        let rows = transaction.read_servers().map_err(shared_store_error)?;
        let core_app = app.as_core();
        let column = builtin_app_registry()
            .for_app(&core_app)
            .mcp_contract()
            .expect("each MCP importer has a Registry contract")
            .catalog_column();
        let existing = rows
            .iter()
            .map(|row| {
                (
                    row.id.clone(),
                    (
                        *row.source_fingerprint(),
                        row.enabled_for(&core_app).unwrap_or(false),
                    ),
                )
            })
            .collect::<HashMap<_, _>>();
        let mut staged = MultiAppConfig::default();
        staged.mcp.servers = Some(
            rows.into_iter()
                .map(Database::mcp_server_from_shared_row)
                .map(|server| (server.id.clone(), server))
                .collect(),
        );
        _shared_lock = SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(
            &crate::config::get_home_dir(),
        ))
        .map_err(|error| match error {
            SharedLiveConfigLockError::Unavailable => {
                AppError::Conflict("Live configuration is locked by another operation".into())
            }
            SharedLiveConfigLockError::Io { path, source } => AppError::io(path, source),
        })?;
        let result = (|| {
            // Host parsers retain their existing count, skipped-entry and
            // same-ID rules. Claude may also copy its legacy override file.
            let count = import(&mut staged)?;
            for (id, server) in staged
                .mcp
                .servers
                .as_ref()
                .expect("MCP import retains the initialized catalog")
            {
                if let Some((fingerprint, was_enabled)) = existing.get(id) {
                    let enabled = server.apps.is_enabled_for(&app);
                    if *was_enabled != enabled {
                        transaction
                            .set_server_selection(id, fingerprint, column, enabled)
                            .map_err(shared_store_error)?;
                    }
                } else {
                    let server_config = serde_json::to_string(&server.server)
                        .map_err(|source| AppError::JsonSerialize { source })?;
                    let tags = serde_json::to_string(&server.tags)
                        .map_err(|source| AppError::JsonSerialize { source })?;
                    let values = McpServerCatalogValues::new(
                        McpServerFields {
                            id,
                            name: &server.name,
                            server_config: &server_config,
                            description: server.description.as_deref(),
                            homepage: server.homepage.as_deref(),
                            docs: server.docs.as_deref(),
                            tags: &tags,
                        },
                        |candidate| *candidate == core_app && server.apps.is_enabled_for(&app),
                    );
                    transaction
                        .insert_server_catalog(&values)
                        .map_err(shared_store_error)?;
                }
            }
            Ok(count)
        })();
        let count = match result {
            Ok(count) => count,
            Err(error) => {
                if let Err(rollback) = transaction.rollback() {
                    return Err(AppError::Config(format!(
                        "MCP import failed: {error}; database rollback: {rollback}"
                    )));
                }
                return Err(error);
            }
        };
        transaction.commit().map_err(shared_store_error)?;
        config.mcp.servers = staged.mcp.servers;
        Ok(count)
    }
}
