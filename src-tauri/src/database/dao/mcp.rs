//! MCP 服务器数据访问对象
//!
//! 提供 MCP 服务器的 CRUD 操作。

use crate::app_config::{McpApps, McpServer};
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use indexmap::IndexMap;
use rusqlite::params;

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
}
