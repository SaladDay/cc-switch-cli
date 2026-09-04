//! MCP 服务器数据访问对象
//!
//! 提供 MCP 服务器的 CRUD 操作。

use crate::app_config::{McpApps, McpServer};
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use cc_switch_store::{McpServerRow, McpServerValues, McpServerWriteOutcome};
use indexmap::IndexMap;
use rusqlite::params;
use serde_json::Value;

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
        apply_live: impl FnOnce() -> Result<T, AppError>,
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

        let receipt = apply_live()?;
        let write_result = write_mcp_change(&mut transaction, current.as_ref(), next);
        if let Err(error) = write_result {
            let database_recovery = transaction.rollback().err().map(AppError::from);
            let live_recovery = rollback_live(receipt).err();
            return Err(with_recovery_errors(
                error,
                database_recovery,
                live_recovery,
            ));
        }
        if let Err(error) = transaction.commit() {
            let live_recovery = rollback_live(receipt).err();
            return Err(with_recovery_errors(
                AppError::Database(error.to_string()),
                None,
                live_recovery,
            ));
        }
        Ok(())
    }
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
                || {
                    live_applied.set(true);
                    Ok(())
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
}
