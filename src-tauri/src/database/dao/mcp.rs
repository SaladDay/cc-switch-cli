//! MCP 服务器数据访问对象
//!
//! 提供 MCP 服务器的 CRUD 操作。

use crate::app_config::{AppType, McpApps, McpServer};
use crate::database::{lock_conn, shared_store_error, sqlite_write_error, Database};
use crate::error::AppError;
use cc_switch_store::{
    McpServerCatalogValues, McpServerFields, McpServerRow as SharedMcpServerRow,
    McpServerWriteOutcome,
};
use indexmap::IndexMap;

fn require_applied(outcome: McpServerWriteOutcome, action: &str) -> Result<(), AppError> {
    match outcome {
        McpServerWriteOutcome::Applied => Ok(()),
        McpServerWriteOutcome::NotApplied => Err(AppError::Conflict(format!(
            "MCP server {action} was not applied"
        ))),
    }
}

fn apps_from_shared_row(row: &SharedMcpServerRow) -> McpApps {
    let mut apps = McpApps::default();
    for app in AppType::all().filter(AppType::supports_mcp) {
        if let Some(enabled) = row.enabled_for(&app.as_core()) {
            apps.set_enabled_for(&app, enabled);
        }
    }
    apps
}

fn catalog_values<'a>(
    server: &'a McpServer,
    server_config: &'a str,
    tags: &'a str,
    current: Option<&SharedMcpServerRow>,
) -> McpServerCatalogValues<'a> {
    McpServerCatalogValues::new(
        McpServerFields {
            id: &server.id,
            name: &server.name,
            server_config,
            description: server.description.as_deref(),
            homepage: server.homepage.as_deref(),
            docs: server.docs.as_deref(),
            tags,
        },
        |core_app| {
            AppType::all()
                .find(|app| app.supports_mcp() && app.as_core() == *core_app)
                .map(|app| server.apps.is_enabled_for(&app))
                .or_else(|| current.and_then(|row| row.enabled_for(core_app)))
                .unwrap_or(false)
        },
    )
}

impl Database {
    /// 获取所有 MCP 服务器
    pub fn get_all_mcp_servers(&self) -> Result<IndexMap<String, McpServer>, AppError> {
        let conn = lock_conn!(self.conn);
        Self::read_mcp_servers_on(&conn)
    }

    pub(crate) fn read_mcp_servers_on(
        conn: &rusqlite::Connection,
    ) -> Result<IndexMap<String, McpServer>, AppError> {
        let rows = cc_switch_store::read_mcp_server_rows(conn).map_err(shared_store_error)?;
        let mut servers = IndexMap::new();
        for row in rows {
            let server = Self::mcp_server_from_shared_row(row);
            let id = server.id.clone();
            servers.insert(id, server);
        }
        Ok(servers)
    }

    pub(crate) fn mcp_server_from_shared_row(row: SharedMcpServerRow) -> McpServer {
        let apps = apps_from_shared_row(&row);
        McpServer {
            id: row.id,
            name: row.name,
            server: serde_json::from_str(&row.server_config).unwrap_or_default(),
            apps,
            description: row.description,
            homepage: row.homepage,
            docs: row.docs,
            tags: serde_json::from_str(&row.tags).unwrap_or_default(),
        }
    }

    /// 保存 MCP 服务器
    pub fn save_mcp_server(&self, server: &McpServer) -> Result<(), AppError> {
        let server_config = serde_json::to_string(&server.server).map_err(|error| {
            AppError::Database(format!("Failed to serialize server config: {error}"))
        })?;
        let tags = serde_json::to_string(&server.tags)
            .map_err(|error| AppError::Database(format!("Failed to serialize tags: {error}")))?;
        let mut conn = lock_conn!(self.conn);
        let mut transaction =
            cc_switch_store::begin_immediate_transaction(&mut conn).map_err(shared_store_error)?;
        let current = cc_switch_store::read_mcp_server_row(&transaction, &server.id)
            .map_err(shared_store_error)?;
        let values = catalog_values(server, &server_config, &tags, current.as_ref());
        let outcome = match current.as_ref() {
            Some(row) => cc_switch_store::update_mcp_server_catalog_preserving_host_fields(
                &mut transaction,
                row.source_fingerprint(),
                &values,
            ),
            None => cc_switch_store::insert_mcp_server_catalog(&mut transaction, &values),
        }
        .map_err(shared_store_error)?;
        require_applied(outcome, "save")?;
        transaction.commit().map_err(sqlite_write_error)?;
        Ok(())
    }

    /// 删除 MCP 服务器
    pub fn delete_mcp_server(&self, id: &str) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let mut transaction =
            cc_switch_store::begin_immediate_transaction(&mut conn).map_err(shared_store_error)?;
        let current =
            cc_switch_store::read_mcp_server_row(&transaction, id).map_err(shared_store_error)?;
        if let Some(row) = current.as_ref() {
            let outcome =
                cc_switch_store::delete_mcp_server(&mut transaction, id, row.source_fingerprint())
                    .map_err(shared_store_error)?;
            require_applied(outcome, "delete")?;
        }
        transaction.commit().map_err(sqlite_write_error)?;
        Ok(())
    }
}
