//! MCP 服务器数据访问对象
//!
//! 提供 MCP 服务器的 CRUD 操作。

use crate::app_config::AppType;
use crate::app_config::{McpApps, McpServer};
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use cc_switch_store::{McpServerRow, McpServerValues, McpServerWriteOutcome};
use indexmap::IndexMap;
use rusqlite::params;
use serde_json::Value;

pub(crate) struct McpLiveTarget {
    pub app: AppType,
    pub owned: bool,
    pub native_snapshot: Option<cc_switch_core::McpNativeSnapshot>,
}

pub(crate) struct McpNativeLinkUpdate {
    pub server_id: String,
    pub app: AppType,
    pub native_snapshot_json: Option<String>,
}

pub(crate) struct McpLiveChange<T> {
    pub receipt: T,
    pub link_updates: Vec<McpNativeLinkUpdate>,
}

pub(crate) struct McpOwnedServer {
    pub server: McpServer,
    pub native_snapshot: Option<cc_switch_core::McpNativeSnapshot>,
}

fn shared_store_error(error: cc_switch_store::SharedStoreError) -> AppError {
    AppError::Database(error.to_string())
}

fn require_applied(outcome: McpServerWriteOutcome) -> Result<(), AppError> {
    match outcome {
        McpServerWriteOutcome::Applied => Ok(()),
        McpServerWriteOutcome::NotApplied => Err(AppError::Conflict(
            "the shared MCP record changed during the update".to_owned(),
        )),
    }
}

impl Database {
    /// Initialize Core-owned MCP link state without changing the product schema version.
    pub(crate) fn ensure_mcp_native_link_schema(&self) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        cc_switch_store::ensure_mcp_native_link_schema(&mut conn).map_err(shared_store_error)
    }

    /// Merge one observed native MCP document into the shared catalog and
    /// persist its ownership snapshots in the same transaction.
    pub(crate) fn import_native_mcp_servers<T>(
        &self,
        app: &AppType,
        observe: impl FnOnce() -> Result<(Vec<cc_switch_core::McpImport>, T), AppError>,
        live_is_current: impl FnOnce(&T) -> Result<bool, AppError>,
    ) -> Result<usize, AppError> {
        let mut conn = lock_conn!(self.conn);
        let mut transaction =
            cc_switch_store::begin_immediate_transaction(&mut conn).map_err(shared_store_error)?;
        crate::config::with_live_config_update_lock(|| {
            let (imports, observation) = observe()?;
            if imports.is_empty() {
                return Ok(0);
            }
            let mut changed = 0;
            for import in imports {
                let current = cc_switch_store::read_mcp_server_row(&transaction, &import.id)
                    .map_err(shared_store_error)?;
                if let Some(row) = current.as_ref() {
                    let mut server = mcp_server_from_shared_row(row)?;
                    if !cc_switch_core::mcp_servers_equivalent(
                        &app.as_core(),
                        &server.server,
                        &import.server,
                    ) {
                        log::warn!(
                            "Skipping {} MCP server '{}' because its shared connection differs",
                            app.as_str(),
                            import.id
                        );
                        continue;
                    }
                    if !server.apps.is_enabled_for(app) {
                        server.apps.set_enabled_for(app, true);
                        write_mcp_change(&mut transaction, current.as_ref(), Some(&server))?;
                        changed += 1;
                    }
                } else {
                    let mut apps = McpApps::default();
                    apps.set_enabled_for(app, true);
                    let server = McpServer {
                        id: import.id.clone(),
                        name: import.id.clone(),
                        server: import.server,
                        apps,
                        description: None,
                        homepage: None,
                        docs: None,
                        tags: Vec::new(),
                    };
                    write_mcp_change(&mut transaction, None, Some(&server))?;
                    changed += 1;
                }
                let snapshot = import
                    .native_snapshot
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|error| AppError::Database(error.to_string()))?;
                cc_switch_store::upsert_mcp_native_link(
                    &mut transaction,
                    &import.id,
                    app.as_str(),
                    snapshot.as_deref(),
                )
                .map_err(shared_store_error)?;
            }
            if !live_is_current(&observation)? {
                return Err(AppError::Conflict(format!(
                    "{} MCP config changed during import",
                    app.as_str()
                )));
            }
            transaction
                .commit()
                .map_err(|error| AppError::Database(error.to_string()))?;
            Ok(changed)
        })
    }

    /// 获取所有 MCP 服务器
    pub fn get_all_mcp_servers(&self) -> Result<IndexMap<String, McpServer>, AppError> {
        let conn = lock_conn!(self.conn);
        let rows = cc_switch_store::read_mcp_server_rows(&conn)
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut servers = IndexMap::new();
        for row in rows {
            let server = serde_json::from_str(&row.server_config).unwrap_or_default();
            let tags = serde_json::from_str(&row.tags).unwrap_or_default();
            let id = row.id;
            let server = McpServer {
                id: id.clone(),
                name: row.name,
                server,
                apps: McpApps {
                    claude: row.enabled_claude != 0,
                    codex: row.enabled_codex != 0,
                    gemini: row.enabled_gemini != 0,
                    opencode: row.enabled_opencode != 0,
                    hermes: row.enabled_hermes != 0,
                },
                description: row.description,
                homepage: row.homepage,
                docs: row.docs,
                tags,
            };
            servers.insert(id, server);
        }
        Ok(servers)
    }

    pub(crate) fn get_owned_mcp_servers(
        &self,
        app: &AppType,
    ) -> Result<Vec<McpOwnedServer>, AppError> {
        let conn = lock_conn!(self.conn);
        owned_mcp_servers(&conn, app)
    }

    /// 保存 MCP 服务器
    pub fn save_mcp_server(&self, server: &McpServer) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT INTO mcp_servers (
                id, name, server_config, description, homepage, docs, tags,
                enabled_claude, enabled_codex, enabled_gemini, enabled_opencode, enabled_hermes
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                server_config = excluded.server_config,
                description = excluded.description,
                homepage = excluded.homepage,
                docs = excluded.docs,
                tags = excluded.tags,
                enabled_claude = excluded.enabled_claude,
                enabled_codex = excluded.enabled_codex,
                enabled_gemini = excluded.enabled_gemini,
                enabled_opencode = excluded.enabled_opencode,
                enabled_hermes = excluded.enabled_hermes",
            params![
                server.id,
                server.name,
                serde_json::to_string(&server.server).map_err(|e| AppError::Database(format!(
                    "Failed to serialize server config: {e}"
                )))?,
                server.description,
                server.homepage,
                server.docs,
                serde_json::to_string(&server.tags)
                    .map_err(|e| AppError::Database(format!("Failed to serialize tags: {e}")))?,
                server.apps.claude,
                server.apps.codex,
                server.apps.gemini,
                server.apps.opencode,
                server.apps.hermes,
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 删除 MCP 服务器
    pub fn delete_mcp_server(&self, id: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute("DELETE FROM mcp_servers WHERE id = ?1", params![id])
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// Commit one catalog-row change and its live projections as one recoverable operation.
    ///
    /// The database write is transactional. The live callback must undo any partial writes when
    /// it returns an error; if database finalization fails, its exact receipt is rolled back here.
    pub(crate) fn commit_mcp_server_change<T>(
        &self,
        previous: Option<&McpServer>,
        next: Option<&McpServer>,
        affected_apps: &[AppType],
        apply_live: impl FnOnce(&[McpLiveTarget]) -> Result<McpLiveChange<T>, AppError>,
        rollback_live: impl FnOnce(T) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        self.commit_mcp_server_change_inner(
            previous,
            next,
            affected_apps,
            apply_live,
            rollback_live,
        )
    }

    /// Projects every owned server for one app as a validated batch while the
    /// ownership rows are protected by an immediate transaction.
    pub(crate) fn commit_mcp_app_sync<T>(
        &self,
        app: &AppType,
        apply_live: impl FnOnce(&[McpOwnedServer]) -> Result<McpLiveChange<T>, AppError>,
        rollback_live: impl FnOnce(T) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let mut transaction =
            cc_switch_store::begin_immediate_transaction(&mut conn).map_err(shared_store_error)?;
        let targets = owned_mcp_servers(&transaction, app)?;
        let live_change = apply_live(&targets)?;
        let write_result = live_change.link_updates.iter().try_for_each(|update| {
            if update.app != *app
                || !targets
                    .iter()
                    .any(|target| target.server.id == update.server_id)
            {
                return Err(AppError::InvalidInput(
                    "MCP live sync returned an unrelated ownership update".to_owned(),
                ));
            }
            cc_switch_store::upsert_mcp_native_link(
                &mut transaction,
                &update.server_id,
                update.app.as_str(),
                update.native_snapshot_json.as_deref(),
            )
            .map_err(shared_store_error)?;
            Ok(())
        });
        if let Err(error) = write_result {
            let database_recovery = transaction.rollback().err().map(AppError::from);
            let live_recovery = rollback_live(live_change.receipt).err();
            return Err(with_recovery_errors(
                error,
                database_recovery,
                live_recovery,
            ));
        }
        if let Err(error) = transaction.commit() {
            let live_recovery = rollback_live(live_change.receipt).err();
            return Err(with_recovery_errors(
                AppError::Database(error.to_string()),
                None,
                live_recovery,
            ));
        }
        Ok(())
    }

    fn commit_mcp_server_change_inner<T>(
        &self,
        previous: Option<&McpServer>,
        next: Option<&McpServer>,
        affected_apps: &[AppType],
        apply_live: impl FnOnce(&[McpLiveTarget]) -> Result<McpLiveChange<T>, AppError>,
        rollback_live: impl FnOnce(T) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        let id = next
            .or(previous)
            .expect("an MCP change has a server")
            .id
            .as_str();
        let mut conn = lock_conn!(self.conn);
        let mut transaction =
            cc_switch_store::begin_immediate_transaction(&mut conn).map_err(shared_store_error)?;
        let current =
            cc_switch_store::read_mcp_server_row(&transaction, id).map_err(shared_store_error)?;
        if !row_matches_previous(current.as_ref(), previous)? {
            return Err(AppError::Conflict(format!(
                "MCP server '{id}' changed in the shared database"
            )));
        }

        let live_targets = mcp_live_targets(&transaction, previous, next, affected_apps)?;
        let live_change = apply_live(&live_targets)?;
        let write_result =
            write_mcp_change(&mut transaction, current.as_ref(), next).and_then(|()| {
                if next.is_some() {
                    for update in &live_change.link_updates {
                        if update.server_id != id
                            || !live_targets.iter().any(|target| target.app == update.app)
                        {
                            return Err(AppError::InvalidInput(
                                "MCP live change returned an unrelated ownership update".to_owned(),
                            ));
                        }
                        cc_switch_store::upsert_mcp_native_link(
                            &mut transaction,
                            id,
                            update.app.as_str(),
                            update.native_snapshot_json.as_deref(),
                        )
                        .map_err(shared_store_error)?;
                    }
                }
                Ok(())
            });
        if let Err(error) = write_result {
            let database_recovery = transaction.rollback().err().map(AppError::from);
            let live_recovery = rollback_live(live_change.receipt).err();
            return Err(with_recovery_errors(
                error,
                database_recovery,
                live_recovery,
            ));
        }
        if let Err(error) = transaction.commit() {
            let live_recovery = rollback_live(live_change.receipt).err();
            return Err(with_recovery_errors(
                AppError::Database(error.to_string()),
                None,
                live_recovery,
            ));
        }
        Ok(())
    }
}

fn mcp_live_targets(
    connection: &rusqlite::Connection,
    previous: Option<&McpServer>,
    next: Option<&McpServer>,
    affected_apps: &[AppType],
) -> Result<Vec<McpLiveTarget>, AppError> {
    let id = next
        .or(previous)
        .expect("an MCP change has a server")
        .id
        .as_str();
    let connection_changed = match (previous, next) {
        (Some(previous), Some(next)) => previous.server != next.server,
        _ => true,
    };
    let mut targets = Vec::new();
    for app in affected_apps {
        if targets
            .iter()
            .any(|target: &McpLiveTarget| target.app == *app)
        {
            continue;
        }
        let link = cc_switch_store::read_mcp_native_link(connection, id, app.as_str())
            .map_err(shared_store_error)?;
        let owned = link.is_some();
        let preserves_disabled_entry = cc_switch_core::mcp_app_contract(&app.as_core())
            .ok_or_else(|| {
                AppError::InvalidInput(format!("{} does not support MCP", app.as_str()))
            })?
            .preserves_disabled_entry();
        let was_enabled = previous.is_some_and(|server| server.apps.is_enabled_for(app));
        let is_enabled = next.is_some_and(|server| server.apps.is_enabled_for(app));
        let should_project = match (previous, next) {
            (_, Some(_)) if is_enabled => !was_enabled || (owned && connection_changed),
            (Some(_), Some(_)) => {
                owned && (was_enabled || (preserves_disabled_entry && connection_changed))
            }
            (None, Some(_)) => false,
            (Some(_), None) => owned,
            (None, None) => false,
        };
        if !should_project {
            continue;
        }
        let native_snapshot = link
            .and_then(|link| link.native_snapshot)
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|_| AppError::Database("Invalid MCP native snapshot".to_owned()))?;
        targets.push(McpLiveTarget {
            app: app.clone(),
            owned,
            native_snapshot,
        });
    }
    Ok(targets)
}

fn owned_mcp_servers(
    connection: &rusqlite::Connection,
    app: &AppType,
) -> Result<Vec<McpOwnedServer>, AppError> {
    let rows = cc_switch_store::read_mcp_server_rows(connection).map_err(shared_store_error)?;
    let mut servers = Vec::new();
    for row in rows {
        let Some(link) = cc_switch_store::read_mcp_native_link(connection, &row.id, app.as_str())
            .map_err(shared_store_error)?
        else {
            continue;
        };
        let native_snapshot = link
            .native_snapshot
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|_| AppError::Database("Invalid MCP native snapshot".to_owned()))?;
        servers.push(McpOwnedServer {
            server: mcp_server_from_shared_row(&row)?,
            native_snapshot,
        });
    }
    Ok(servers)
}

fn mcp_server_from_shared_row(row: &McpServerRow) -> Result<McpServer, AppError> {
    Ok(McpServer {
        id: row.id.clone(),
        name: row.name.clone(),
        server: serde_json::from_str(&row.server_config)
            .map_err(|error| AppError::Database(format!("Invalid MCP server JSON: {error}")))?,
        apps: McpApps {
            claude: row.enabled_claude != 0,
            codex: row.enabled_codex != 0,
            gemini: row.enabled_gemini != 0,
            opencode: row.enabled_opencode != 0,
            hermes: row.enabled_hermes != 0,
        },
        description: row.description.clone(),
        homepage: row.homepage.clone(),
        docs: row.docs.clone(),
        tags: serde_json::from_str(&row.tags)
            .map_err(|error| AppError::Database(format!("Invalid MCP tags JSON: {error}")))?,
    })
}

fn row_matches_previous(
    row: Option<&McpServerRow>,
    previous: Option<&McpServer>,
) -> Result<bool, AppError> {
    let (Some(row), Some(previous)) = (row, previous) else {
        return Ok(row.is_none() && previous.is_none());
    };
    let server: Value = serde_json::from_str(&row.server_config)
        .map_err(|error| AppError::Database(format!("Invalid MCP server JSON: {error}")))?;
    let tags: Vec<String> = serde_json::from_str(&row.tags)
        .map_err(|error| AppError::Database(format!("Invalid MCP tags JSON: {error}")))?;
    Ok(row.id == previous.id
        && row.name == previous.name
        && server == previous.server
        && row.description == previous.description
        && row.homepage == previous.homepage
        && row.docs == previous.docs
        && tags == previous.tags
        && (row.enabled_claude != 0) == previous.apps.claude
        && (row.enabled_codex != 0) == previous.apps.codex
        && (row.enabled_gemini != 0) == previous.apps.gemini
        && (row.enabled_opencode != 0) == previous.apps.opencode
        && (row.enabled_hermes != 0) == previous.apps.hermes)
}

fn write_mcp_change(
    transaction: &mut rusqlite::Transaction<'_>,
    current: Option<&McpServerRow>,
    next: Option<&McpServer>,
) -> Result<(), AppError> {
    match next {
        Some(server) => {
            let server_config = serde_json::to_string(&server.server)
                .map_err(|error| AppError::Database(error.to_string()))?;
            let tags = serde_json::to_string(&server.tags)
                .map_err(|error| AppError::Database(error.to_string()))?;
            let values = McpServerValues {
                id: &server.id,
                name: &server.name,
                server_config: &server_config,
                description: server.description.as_deref(),
                homepage: server.homepage.as_deref(),
                docs: server.docs.as_deref(),
                tags: &tags,
                enabled_claude: server.apps.claude,
                enabled_codex: server.apps.codex,
                enabled_gemini: server.apps.gemini,
                enabled_grokbuild: current.is_some_and(|row| row.enabled_grokbuild != 0),
                enabled_opencode: server.apps.opencode,
                enabled_hermes: server.apps.hermes,
            };
            let outcome = match current {
                Some(row) => cc_switch_store::update_mcp_server(
                    transaction,
                    row.source_fingerprint(),
                    &values,
                ),
                None => cc_switch_store::insert_mcp_server(transaction, &values),
            }
            .map_err(shared_store_error)?;
            require_applied(outcome)
        }
        None => {
            let current = current.ok_or_else(|| {
                AppError::Conflict("the shared MCP record no longer exists".to_owned())
            })?;
            require_applied(
                cc_switch_store::delete_mcp_server(
                    transaction,
                    &current.id,
                    current.source_fingerprint(),
                )
                .map_err(shared_store_error)?,
            )
        }
    }
}

fn with_recovery_errors(
    error: AppError,
    database_recovery: Option<AppError>,
    live_recovery: Option<AppError>,
) -> AppError {
    let mut failures = Vec::new();
    if let Some(error) = database_recovery {
        failures.push(format!("database rollback: {error}"));
    }
    if let Some(error) = live_recovery {
        failures.push(format!("live rollback: {error}"));
    }
    if failures.is_empty() {
        error
    } else {
        AppError::Message(format!(
            "{error}; MCP recovery failed: {}",
            failures.join("; ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use serde_json::json;

    use super::*;

    fn server(name: &str) -> McpServer {
        McpServer {
            id: "server".to_owned(),
            name: name.to_owned(),
            server: json!({"type":"stdio","command":"npx"}),
            apps: McpApps::default(),
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        }
    }

    #[test]
    fn database_write_failure_rolls_back_the_live_receipt() {
        let database = Database::memory().expect("create database");
        let previous = server("Previous");
        let next = server("Next");
        database
            .save_mcp_server(&previous)
            .expect("seed MCP server");
        database
            .conn
            .lock()
            .expect("lock database")
            .execute_batch(
                "CREATE TRIGGER reject_mcp_update
                 BEFORE UPDATE ON mcp_servers
                 BEGIN
                   SELECT RAISE(ABORT, 'blocked');
                 END;",
            )
            .expect("install rejecting trigger");

        let live_applied = Cell::new(false);
        let live_rolled_back = Cell::new(false);
        database
            .commit_mcp_server_change(
                Some(&previous),
                Some(&next),
                &[],
                |_| {
                    live_applied.set(true);
                    Ok(McpLiveChange {
                        receipt: (),
                        link_updates: Vec::new(),
                    })
                },
                |()| {
                    live_rolled_back.set(true);
                    Ok(())
                },
            )
            .expect_err("trigger must reject the database update");

        assert!(live_applied.get());
        assert!(live_rolled_back.get());
        assert_eq!(
            database.get_all_mcp_servers().unwrap()["server"].name,
            "Previous"
        );
    }

    #[test]
    fn app_sync_link_failure_rolls_back_the_live_receipt() {
        let database = Database::memory().expect("create database");
        let imports = cc_switch_core::import_mcp_servers(
            &cc_switch_core::AppType::Claude,
            Some(br#"{"mcpServers":{"server":{"command":"npx","trust":true}}}"#),
        )
        .expect("parse native MCP");
        database
            .import_native_mcp_servers(&AppType::Claude, || Ok((imports, ())), |()| Ok(true))
            .expect("import MCP server");
        database
            .conn
            .lock()
            .expect("lock database")
            .execute_batch(
                "CREATE TRIGGER reject_mcp_link_update
                 BEFORE UPDATE ON mcp_native_links
                 BEGIN
                   SELECT RAISE(ABORT, 'blocked');
                 END;",
            )
            .expect("install rejecting trigger");

        let live_rolled_back = Cell::new(false);
        database
            .commit_mcp_app_sync(
                &AppType::Claude,
                |targets| {
                    assert_eq!(targets.len(), 1);
                    Ok(McpLiveChange {
                        receipt: (),
                        link_updates: vec![McpNativeLinkUpdate {
                            server_id: "server".to_owned(),
                            app: AppType::Claude,
                            native_snapshot_json: Some("updated".to_owned()),
                        }],
                    })
                },
                |()| {
                    live_rolled_back.set(true);
                    Ok(())
                },
            )
            .expect_err("trigger must reject the ownership update");

        assert!(live_rolled_back.get());
        let conn = database.conn.lock().expect("lock database");
        let link = cc_switch_store::read_mcp_native_link(&conn, "server", "claude")
            .unwrap()
            .unwrap();
        assert_ne!(link.native_snapshot.as_deref(), Some("updated"));
    }

    #[test]
    fn native_import_commits_catalog_and_snapshot_together() {
        let database = Database::memory().expect("create database");
        let imports = cc_switch_core::import_mcp_servers(
            &cc_switch_core::AppType::Claude,
            Some(br#"{"mcpServers":{"server":{"command":"npx","timeout":30}}}"#),
        )
        .expect("parse native MCP");

        let changed = database
            .import_native_mcp_servers(&AppType::Claude, || Ok((imports, ())), |()| Ok(true))
            .expect("import MCP server");

        assert_eq!(changed, 1);
        let server = &database.get_all_mcp_servers().unwrap()["server"];
        assert!(server.apps.claude);
        assert!(server.server.get("timeout").is_none());
        let conn = database.conn.lock().expect("lock database");
        let link = cc_switch_store::read_mcp_native_link(&conn, "server", "claude")
            .expect("read native link")
            .expect("native link exists");
        assert!(link.native_snapshot.is_some());
    }

    #[test]
    fn explicit_native_import_enables_the_selected_app() {
        let database = Database::memory().expect("create database");
        let imports = cc_switch_core::import_mcp_servers(
            &cc_switch_core::AppType::OpenCode,
            Some(br#"{"mcp":{"server":{"type":"local","command":["npx"],"enabled":false}}}"#),
        )
        .expect("parse disabled native MCP");

        database
            .import_native_mcp_servers(&AppType::OpenCode, || Ok((imports, ())), |()| Ok(true))
            .expect("import native MCP");

        assert!(
            database.get_all_mcp_servers().unwrap()["server"]
                .apps
                .opencode
        );
    }

    #[test]
    fn unmanaged_live_target_does_not_claim_ownership() {
        let database = Database::memory().expect("create database");
        let mut next = server("New");
        next.apps.claude = true;

        database
            .commit_mcp_server_change(
                None,
                Some(&next),
                &[AppType::Claude],
                |targets| {
                    assert_eq!(targets.len(), 1);
                    Ok(McpLiveChange {
                        receipt: (),
                        link_updates: Vec::new(),
                    })
                },
                |()| Ok(()),
            )
            .expect("commit catalog-only server");

        let conn = database.conn.lock().expect("lock database");
        assert!(
            cc_switch_store::read_mcp_native_link(&conn, "server", "claude")
                .expect("read native link")
                .is_none()
        );
    }

    #[test]
    fn conflicting_native_import_does_not_claim_ownership() {
        let database = Database::memory().expect("create database");
        let existing = server("Existing");
        database
            .save_mcp_server(&existing)
            .expect("seed MCP server");
        let imports = cc_switch_core::import_mcp_servers(
            &cc_switch_core::AppType::Claude,
            Some(br#"{"mcpServers":{"server":{"command":"uvx","trust":true}}}"#),
        )
        .expect("parse native MCP");

        let changed = database
            .import_native_mcp_servers(&AppType::Claude, || Ok((imports, ())), |()| Ok(true))
            .expect("skip conflicting native MCP server");

        assert_eq!(changed, 0);
        let stored = &database.get_all_mcp_servers().unwrap()["server"];
        assert_eq!(stored.server["command"], "npx");
        assert!(!stored.apps.claude);
        let conn = database.conn.lock().expect("lock database");
        assert!(
            cc_switch_store::read_mcp_native_link(&conn, "server", "claude")
                .expect("read native link")
                .is_none()
        );
    }

    #[test]
    fn changed_native_document_rolls_back_catalog_and_snapshot() {
        let database = Database::memory().expect("create database");
        let imports = cc_switch_core::import_mcp_servers(
            &cc_switch_core::AppType::Claude,
            Some(br#"{"mcpServers":{"server":{"command":"npx"}}}"#),
        )
        .expect("parse native MCP");

        database
            .import_native_mcp_servers(&AppType::Claude, || Ok((imports, ())), |()| Ok(false))
            .expect_err("detect changed native document");

        assert!(database.get_all_mcp_servers().unwrap().is_empty());
        let conn = database.conn.lock().expect("lock database");
        assert!(
            cc_switch_store::read_mcp_native_link(&conn, "server", "claude")
                .expect("read rolled-back link")
                .is_none()
        );
    }

    #[test]
    fn imported_snapshot_survives_disable_and_is_offered_on_restore() {
        let database = Database::memory().expect("create database");
        let imports = cc_switch_core::import_mcp_servers(
            &cc_switch_core::AppType::Claude,
            Some(br#"{"mcpServers":{"server":{"command":"npx","trust":true}}}"#),
        )
        .expect("parse native MCP");
        database
            .import_native_mcp_servers(&AppType::Claude, || Ok((imports, ())), |()| Ok(true))
            .expect("import MCP server");

        let enabled = database.get_all_mcp_servers().unwrap()["server"].clone();
        let mut disabled = enabled.clone();
        disabled.apps.claude = false;
        database
            .commit_mcp_server_change(
                Some(&enabled),
                Some(&disabled),
                &[AppType::Claude],
                |targets| {
                    assert_eq!(targets.len(), 1);
                    assert!(targets[0].native_snapshot.is_some());
                    Ok(McpLiveChange {
                        receipt: (),
                        link_updates: vec![McpNativeLinkUpdate {
                            server_id: "server".to_owned(),
                            app: AppType::Claude,
                            native_snapshot_json: targets[0]
                                .native_snapshot
                                .as_ref()
                                .map(serde_json::to_string)
                                .transpose()
                                .unwrap(),
                        }],
                    })
                },
                |()| Ok(()),
            )
            .expect("disable imported server");

        let stored_disabled = database.get_all_mcp_servers().unwrap()["server"].clone();
        let mut restored = stored_disabled.clone();
        restored.apps.claude = true;
        let snapshot_offered = Cell::new(false);
        database
            .commit_mcp_server_change(
                Some(&stored_disabled),
                Some(&restored),
                &[AppType::Claude],
                |targets| {
                    snapshot_offered
                        .set(targets.len() == 1 && targets[0].native_snapshot.is_some());
                    Ok(McpLiveChange {
                        receipt: (),
                        link_updates: vec![McpNativeLinkUpdate {
                            server_id: "server".to_owned(),
                            app: AppType::Claude,
                            native_snapshot_json: targets[0]
                                .native_snapshot
                                .as_ref()
                                .map(serde_json::to_string)
                                .transpose()
                                .unwrap(),
                        }],
                    })
                },
                |()| Ok(()),
            )
            .expect("restore imported server");
        assert!(snapshot_offered.get());
    }
}
