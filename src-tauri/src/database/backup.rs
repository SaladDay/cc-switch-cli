//! 数据库备份和恢复
//!
//! 提供 SQL 导出/导入和二进制快照备份功能。

use super::{lock_conn, Database, DB_BACKUP_RETAIN};
use crate::config::get_app_config_dir;
use crate::error::AppError;
use chrono::Utc;
use rusqlite::backup::Backup;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

const CC_SWITCH_SQL_EXPORT_HEADER: &str = "-- CC Switch SQLite 导出";

const SYNC_IMPORT_RESTORE_TABLES: &[&str] = &[
    "proxy_request_logs",
    "stream_check_logs",
    "proxy_live_backup",
    "usage_daily_rollups",
];

const SYNC_EXPORT_RESETTABLE_TABLES: &[&str] = &["provider_health"];

const SYNC_LOCAL_SETTINGS_KEYS: &[&str] = &["proxy_runtime_session"];
const PROXY_CONFIG_LOCAL_COLUMNS: &[&str] =
    &["proxy_enabled", "listen_address", "listen_port", "enabled"];

#[derive(Clone, Copy)]
enum SyncNeutralValue {
    Integer(i64),
    Text(&'static str),
}

impl SyncNeutralValue {
    fn into_sql_value(self) -> Value {
        match self {
            Self::Integer(value) => Value::Integer(value),
            Self::Text(value) => Value::Text(value.to_string()),
        }
    }
}

#[derive(Clone, Copy)]
struct SyncNeutralizedColumn {
    column: &'static str,
    value: SyncNeutralValue,
}

#[derive(Clone, Copy)]
struct SyncRowKeyedColumnGroup {
    table: &'static str,
    key_column: &'static str,
    preserved_columns: &'static [&'static str],
    export_defaults: &'static [SyncNeutralizedColumn],
}

#[derive(Clone, Copy)]
struct SyncPreservationPolicy {
    import_restore_tables: &'static [&'static str],
    export_resettable_tables: &'static [&'static str],
    local_settings_keys: &'static [&'static str],
    row_keyed_column_groups: &'static [SyncRowKeyedColumnGroup],
}

const PROXY_CONFIG_EXPORT_DEFAULTS: &[SyncNeutralizedColumn] = &[
    SyncNeutralizedColumn {
        column: "proxy_enabled",
        value: SyncNeutralValue::Integer(0),
    },
    SyncNeutralizedColumn {
        column: "listen_address",
        value: SyncNeutralValue::Text("127.0.0.1"),
    },
    SyncNeutralizedColumn {
        column: "listen_port",
        value: SyncNeutralValue::Integer(15721),
    },
    SyncNeutralizedColumn {
        column: "enabled",
        value: SyncNeutralValue::Integer(0),
    },
    SyncNeutralizedColumn {
        column: "live_takeover_active",
        value: SyncNeutralValue::Integer(0),
    },
];

const SYNC_ROW_KEYED_COLUMN_GROUPS: &[SyncRowKeyedColumnGroup] = &[SyncRowKeyedColumnGroup {
    table: "proxy_config",
    key_column: "app_type",
    preserved_columns: PROXY_CONFIG_LOCAL_COLUMNS,
    export_defaults: PROXY_CONFIG_EXPORT_DEFAULTS,
}];

const SYNC_PRESERVATION_POLICY: SyncPreservationPolicy = SyncPreservationPolicy {
    import_restore_tables: SYNC_IMPORT_RESTORE_TABLES,
    export_resettable_tables: SYNC_EXPORT_RESETTABLE_TABLES,
    local_settings_keys: SYNC_LOCAL_SETTINGS_KEYS,
    row_keyed_column_groups: SYNC_ROW_KEYED_COLUMN_GROUPS,
};

impl Database {
    /// 导出为 SQL 字符串（内存操作，不写文件）
    pub fn export_sql_string(&self) -> Result<String, AppError> {
        let snapshot = self.snapshot_to_memory()?;
        Self::dump_sql(&snapshot, None)
    }

    pub fn export_sql_string_for_sync(&self) -> Result<String, AppError> {
        let snapshot = self.snapshot_to_memory()?;
        Self::dump_sql(&snapshot, Some(&SYNC_PRESERVATION_POLICY))
    }

    /// 导出为 SQLite 兼容的 SQL 文本文件
    pub fn export_sql(&self, target_path: &Path) -> Result<(), AppError> {
        let dump = self.export_sql_string()?;

        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }

        crate::config::atomic_write(target_path, dump.as_bytes())
    }

    /// 从 SQL 字符串导入，返回生成的备份 ID（若无备份则为空字符串）
    pub fn import_sql_string(&self, sql_raw: &str) -> Result<String, AppError> {
        self.import_sql_string_inner(sql_raw, None)
    }

    pub(crate) fn import_sql_string_for_sync(&self, sql_raw: &str) -> Result<String, AppError> {
        self.import_sql_string_inner(sql_raw, Some(&SYNC_PRESERVATION_POLICY))
    }

    fn import_sql_string_inner(
        &self,
        sql_raw: &str,
        policy: Option<&SyncPreservationPolicy>,
    ) -> Result<String, AppError> {
        let sql_content = sql_raw.trim_start_matches('\u{feff}');
        Self::validate_cc_switch_sql_export(sql_content)?;

        // 导入前备份现有数据库
        let backup_path = self.backup_database_file()?;

        let local_snapshot = policy.map(|_| self.snapshot_to_memory()).transpose()?;

        // 在临时数据库执行导入，确保失败不会污染主库
        let temp_file = NamedTempFile::new().map_err(|e| AppError::IoContext {
            context: "创建临时数据库文件失败".to_string(),
            source: e,
        })?;
        let temp_path = temp_file.path().to_path_buf();
        let temp_conn =
            Connection::open(&temp_path).map_err(|e| AppError::Database(e.to_string()))?;

        temp_conn
            .execute_batch(sql_content)
            .map_err(|e| AppError::Database(format!("执行 SQL 导入失败: {e}")))?;

        // 补齐缺失表/索引并进行基础校验
        Self::create_tables_on_conn(&temp_conn)?;
        Self::apply_schema_migrations_on_conn(&temp_conn)?;
        Self::validate_basic_state(&temp_conn)?;
        if let (Some(local_snapshot), Some(policy)) = (local_snapshot.as_ref(), policy) {
            Self::restore_sync_local_overlay(local_snapshot, &temp_conn, policy)?;
        }

        // 使用 Backup 将临时库原子写回主库
        {
            let mut main_conn = lock_conn!(self.conn);
            let backup = Backup::new(&temp_conn, &mut main_conn)
                .map_err(|e| AppError::Database(e.to_string()))?;
            backup
                .step(-1)
                .map_err(|e| AppError::Database(e.to_string()))?;
        }

        let backup_id = backup_path
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_default();

        Ok(backup_id)
    }

    /// 从 SQL 文件导入，返回生成的备份 ID（若无备份则为空字符串）
    pub fn import_sql(&self, source_path: &Path) -> Result<String, AppError> {
        if !source_path.exists() {
            return Err(AppError::InvalidInput(format!(
                "SQL 文件不存在: {}",
                source_path.display()
            )));
        }

        let sql_raw = fs::read_to_string(source_path).map_err(|e| AppError::io(source_path, e))?;
        self.import_sql_string(&sql_raw)
    }

    /// 创建内存快照以避免长时间持有数据库锁
    pub(crate) fn snapshot_to_memory(&self) -> Result<Connection, AppError> {
        let conn = lock_conn!(self.conn);
        let mut snapshot =
            Connection::open_in_memory().map_err(|e| AppError::Database(e.to_string()))?;

        {
            let backup =
                Backup::new(&conn, &mut snapshot).map_err(|e| AppError::Database(e.to_string()))?;
            backup
                .step(-1)
                .map_err(|e| AppError::Database(e.to_string()))?;
        }

        Ok(snapshot)
    }

    fn validate_cc_switch_sql_export(sql: &str) -> Result<(), AppError> {
        let trimmed = sql.trim_start();
        if trimmed.starts_with(CC_SWITCH_SQL_EXPORT_HEADER) {
            return Ok(());
        }

        Err(AppError::localized(
            "backup.sql.invalid_format",
            "仅支持导入由 CC Switch 导出的 SQL 备份文件。",
            "Only SQL backups exported by CC Switch are supported.",
        ))
    }

    fn restore_tables(
        source_conn: &Connection,
        target_conn: &Connection,
        tables: &[&str],
    ) -> Result<(), AppError> {
        for table in tables {
            if !Self::table_exists(source_conn, table)? || !Self::table_exists(target_conn, table)?
            {
                continue;
            }

            let columns = Self::get_table_columns(source_conn, table)?;
            if columns.is_empty() {
                continue;
            }

            target_conn
                .execute(&format!("DELETE FROM \"{table}\""), [])
                .map_err(|e| AppError::Database(format!("清空表 {table} 失败: {e}")))?;

            let placeholders = (1..=columns.len())
                .map(|idx| format!("?{idx}"))
                .collect::<Vec<_>>()
                .join(", ");
            let cols = columns
                .iter()
                .map(|column| format!("\"{column}\""))
                .collect::<Vec<_>>()
                .join(", ");
            let insert_sql = format!("INSERT INTO \"{table}\" ({cols}) VALUES ({placeholders})");

            let mut stmt = source_conn
                .prepare(&format!("SELECT * FROM \"{table}\""))
                .map_err(|e| AppError::Database(format!("读取表 {table} 失败: {e}")))?;
            let mut rows = stmt
                .query([])
                .map_err(|e| AppError::Database(format!("查询表 {table} 数据失败: {e}")))?;

            while let Some(row) = rows.next().map_err(|e| AppError::Database(e.to_string()))? {
                let mut values = Vec::with_capacity(columns.len());
                for idx in 0..columns.len() {
                    values.push(
                        row.get::<_, rusqlite::types::Value>(idx)
                            .map_err(|e| AppError::Database(e.to_string()))?,
                    );
                }

                target_conn
                    .execute(&insert_sql, rusqlite::params_from_iter(values.iter()))
                    .map_err(|e| AppError::Database(format!("恢复表 {table} 数据失败: {e}")))?;
            }
        }

        Ok(())
    }

    fn restore_sync_local_overlay(
        source_conn: &Connection,
        target_conn: &Connection,
        policy: &SyncPreservationPolicy,
    ) -> Result<(), AppError> {
        Self::restore_tables(source_conn, target_conn, policy.import_restore_tables)?;
        Self::clear_tables(target_conn, policy.export_resettable_tables)?;
        Self::restore_settings_keys(source_conn, target_conn, policy.local_settings_keys)?;
        for group in policy.row_keyed_column_groups {
            Self::restore_row_keyed_column_group(source_conn, target_conn, group)?;
        }
        Ok(())
    }

    fn clear_tables(target_conn: &Connection, tables: &[&str]) -> Result<(), AppError> {
        for table in tables {
            if !Self::table_exists(target_conn, table)? {
                continue;
            }

            target_conn
                .execute(&format!("DELETE FROM {}", Self::quote_ident(table)), [])
                .map_err(|e| AppError::Database(format!("清空表 {table} 失败: {e}")))?;
        }

        Ok(())
    }

    fn restore_settings_keys(
        source_conn: &Connection,
        target_conn: &Connection,
        keys: &[&str],
    ) -> Result<(), AppError> {
        if keys.is_empty()
            || !Self::table_exists(source_conn, "settings")?
            || !Self::table_exists(target_conn, "settings")?
        {
            return Ok(());
        }

        for key in keys {
            let local_value: Option<String> = source_conn
                .query_row("SELECT value FROM settings WHERE key = ?1", [*key], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(|e| AppError::Database(format!("读取本地 settings 键 {key} 失败: {e}")))?;

            target_conn
                .execute("DELETE FROM settings WHERE key = ?1", [*key])
                .map_err(|e| AppError::Database(format!("清理远端 settings 键 {key} 失败: {e}")))?;

            if let Some(value) = local_value {
                target_conn
                    .execute(
                        "INSERT INTO settings (key, value) VALUES (?1, ?2)",
                        rusqlite::params![*key, value],
                    )
                    .map_err(|e| {
                        AppError::Database(format!("恢复本地 settings 键 {key} 失败: {e}"))
                    })?;
            }
        }

        Ok(())
    }

    fn restore_row_keyed_column_group(
        source_conn: &Connection,
        target_conn: &Connection,
        group: &SyncRowKeyedColumnGroup,
    ) -> Result<(), AppError> {
        if !Self::table_exists(source_conn, group.table)?
            || !Self::table_exists(target_conn, group.table)?
        {
            return Ok(());
        }

        let source_columns = Self::get_table_columns(source_conn, group.table)?;
        let target_columns = Self::get_table_columns(target_conn, group.table)?;
        if !source_columns
            .iter()
            .any(|column| column == group.key_column)
            || !target_columns
                .iter()
                .any(|column| column == group.key_column)
        {
            return Ok(());
        }

        let preserved_columns = group
            .preserved_columns
            .iter()
            .copied()
            .filter(|column| {
                source_columns.iter().any(|existing| existing == column)
                    && target_columns.iter().any(|existing| existing == column)
            })
            .collect::<Vec<_>>();
        if preserved_columns.is_empty() {
            return Ok(());
        }

        let select_columns = std::iter::once(group.key_column)
            .chain(preserved_columns.iter().copied())
            .map(Self::quote_ident)
            .collect::<Vec<_>>()
            .join(", ");
        let select_sql = format!(
            "SELECT {select_columns} FROM {}",
            Self::quote_ident(group.table)
        );
        let assignments = preserved_columns
            .iter()
            .enumerate()
            .map(|(idx, column)| format!("{} = ?{}", Self::quote_ident(column), idx + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let update_sql = format!(
            "UPDATE {} SET {assignments} WHERE {} = ?{}",
            Self::quote_ident(group.table),
            Self::quote_ident(group.key_column),
            preserved_columns.len() + 1
        );

        let mut stmt = source_conn.prepare(&select_sql).map_err(|e| {
            AppError::Database(format!("读取本地表 {} 的列组失败: {e}", group.table))
        })?;
        let mut rows = stmt.query([]).map_err(|e| {
            AppError::Database(format!("查询本地表 {} 的列组数据失败: {e}", group.table))
        })?;

        while let Some(row) = rows.next().map_err(|e| AppError::Database(e.to_string()))? {
            let mut values = Vec::with_capacity(preserved_columns.len() + 1);
            for idx in 1..=preserved_columns.len() {
                values.push(
                    row.get::<_, Value>(idx)
                        .map_err(|e| AppError::Database(e.to_string()))?,
                );
            }
            values.push(
                row.get::<_, Value>(0)
                    .map_err(|e| AppError::Database(e.to_string()))?,
            );

            target_conn
                .execute(&update_sql, rusqlite::params_from_iter(values.iter()))
                .map_err(|e| {
                    AppError::Database(format!("恢复本地表 {} 的列组失败: {e}", group.table))
                })?;
        }

        Ok(())
    }

    /// 生成一致性快照备份，返回备份文件路径（不存在主库时返回 None）
    pub(crate) fn backup_database_file(&self) -> Result<Option<PathBuf>, AppError> {
        let db_path = get_app_config_dir().join("cc-switch.db");
        if !db_path.exists() {
            return Ok(None);
        }

        let backup_dir = db_path
            .parent()
            .ok_or_else(|| AppError::Config("无效的数据库路径".to_string()))?
            .join("backups");

        fs::create_dir_all(&backup_dir).map_err(|e| AppError::io(&backup_dir, e))?;

        let base_id = format!("db_backup_{}", Utc::now().format("%Y%m%d_%H%M%S"));
        let mut backup_id = base_id.clone();
        let mut backup_path = backup_dir.join(format!("{backup_id}.db"));
        let mut counter = 1;
        while backup_path.exists() {
            backup_id = format!("{base_id}_{counter}");
            backup_path = backup_dir.join(format!("{backup_id}.db"));
            counter += 1;
        }

        {
            let conn = lock_conn!(self.conn);
            let mut dest_conn =
                Connection::open(&backup_path).map_err(|e| AppError::Database(e.to_string()))?;
            let backup = Backup::new(&conn, &mut dest_conn)
                .map_err(|e| AppError::Database(e.to_string()))?;
            backup
                .step(-1)
                .map_err(|e| AppError::Database(e.to_string()))?;
        }

        Self::cleanup_db_backups(&backup_dir)?;
        Ok(Some(backup_path))
    }

    /// 清理旧的数据库备份，保留最新的 N 个
    fn cleanup_db_backups(dir: &Path) -> Result<(), AppError> {
        let entries = match fs::read_dir(dir) {
            Ok(iter) => iter
                .filter_map(|entry| entry.ok())
                .filter(|entry| {
                    entry
                        .path()
                        .extension()
                        .map(|ext| ext == "db")
                        .unwrap_or(false)
                })
                .collect::<Vec<_>>(),
            Err(_) => return Ok(()),
        };

        if entries.len() <= DB_BACKUP_RETAIN {
            return Ok(());
        }

        let remove_count = entries.len().saturating_sub(DB_BACKUP_RETAIN);
        let mut sorted = entries;
        sorted.sort_by_key(|entry| entry.metadata().and_then(|m| m.modified()).ok());

        for entry in sorted.into_iter().take(remove_count) {
            if let Err(err) = fs::remove_file(entry.path()) {
                log::warn!("删除旧数据库备份失败 {}: {}", entry.path().display(), err);
            }
        }
        Ok(())
    }

    /// 基础状态校验
    fn validate_basic_state(conn: &Connection) -> Result<(), AppError> {
        let provider_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM providers", [], |row| row.get(0))
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mcp_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM mcp_servers", [], |row| row.get(0))
            .map_err(|e| AppError::Database(e.to_string()))?;

        if provider_count == 0 && mcp_count == 0 {
            return Err(AppError::Config(
                "导入的 SQL 未包含有效的供应商或 MCP 数据".to_string(),
            ));
        }
        Ok(())
    }

    /// 导出数据库为 SQL 文本
    fn dump_sql(
        conn: &Connection,
        policy: Option<&SyncPreservationPolicy>,
    ) -> Result<String, AppError> {
        let mut output = String::new();
        let timestamp = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let user_version: i64 = conn
            .query_row("PRAGMA user_version;", [], |row| row.get(0))
            .unwrap_or(0);

        output.push_str(&format!(
            "-- CC Switch SQLite 导出\n-- 生成时间: {timestamp}\n-- user_version: {user_version}\n"
        ));
        output.push_str("PRAGMA foreign_keys=OFF;\n");
        output.push_str(&format!("PRAGMA user_version={user_version};\n"));
        output.push_str("BEGIN TRANSACTION;\n");

        // 导出 schema
        let mut stmt = conn
            .prepare(
                "SELECT type, name, tbl_name, sql
                 FROM sqlite_master
                 WHERE sql NOT NULL AND type IN ('table','index','trigger','view')
                 ORDER BY type='table' DESC, name",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut tables = Vec::new();
        let mut rows = stmt
            .query([])
            .map_err(|e| AppError::Database(e.to_string()))?;
        while let Some(row) = rows.next().map_err(|e| AppError::Database(e.to_string()))? {
            let obj_type: String = row.get(0).map_err(|e| AppError::Database(e.to_string()))?;
            let name: String = row.get(1).map_err(|e| AppError::Database(e.to_string()))?;
            let sql: String = row.get(3).map_err(|e| AppError::Database(e.to_string()))?;

            // 跳过 SQLite 内部对象（如 sqlite_sequence）
            if name.starts_with("sqlite_") {
                continue;
            }

            output.push_str(&sql);
            output.push_str(";\n");

            if obj_type == "table" && !name.starts_with("sqlite_") {
                tables.push(name);
            }
        }

        // 导出数据
        for table in tables {
            if policy.is_some_and(|policy| {
                policy
                    .import_restore_tables
                    .iter()
                    .any(|skip| *skip == table)
                    || policy
                        .export_resettable_tables
                        .iter()
                        .any(|skip| *skip == table)
            }) {
                continue;
            }

            let columns = Self::get_table_columns(conn, &table)?;
            if columns.is_empty() {
                continue;
            }

            let mut stmt = conn
                .prepare(&format!("SELECT * FROM \"{table}\""))
                .map_err(|e| AppError::Database(e.to_string()))?;
            let mut rows = stmt
                .query([])
                .map_err(|e| AppError::Database(e.to_string()))?;

            while let Some(row) = rows.next().map_err(|e| AppError::Database(e.to_string()))? {
                let mut values = Vec::with_capacity(columns.len());
                for idx in 0..columns.len() {
                    values.push(
                        row.get::<_, Value>(idx)
                            .map_err(|e| AppError::Database(e.to_string()))?,
                    );
                }

                if let Some(policy) = policy {
                    if !Self::should_export_row(&table, &columns, &values, policy)? {
                        continue;
                    }
                    Self::neutralize_export_row(&table, &columns, &mut values, policy);
                }

                let cols = columns
                    .iter()
                    .map(|c| format!("\"{c}\""))
                    .collect::<Vec<_>>()
                    .join(", ");
                output.push_str(&format!(
                    "INSERT INTO \"{table}\" ({cols}) VALUES ({});\n",
                    values
                        .iter()
                        .map(Self::format_owned_sql_value)
                        .collect::<Result<Vec<_>, _>>()?
                        .join(", ")
                ));
            }
        }

        output.push_str("COMMIT;\nPRAGMA foreign_keys=ON;\n");
        Ok(output)
    }

    /// 获取表的列名列表
    fn get_table_columns(conn: &Connection, table: &str) -> Result<Vec<String>, AppError> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info(\"{table}\")"))
            .map_err(|e| AppError::Database(e.to_string()))?;
        let iter = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut columns = Vec::new();
        for col in iter {
            columns.push(col.map_err(|e| AppError::Database(e.to_string()))?);
        }
        Ok(columns)
    }

    fn format_owned_sql_value(value: &Value) -> Result<String, AppError> {
        match value {
            Value::Null => Ok("NULL".to_string()),
            Value::Integer(i) => Ok(i.to_string()),
            Value::Real(f) => Ok(f.to_string()),
            Value::Text(text) => Ok(format!("'{}'", text.replace('\'', "''"))),
            Value::Blob(bytes) => {
                let mut s = String::from("X'");
                for b in bytes {
                    use std::fmt::Write;
                    let _ = write!(&mut s, "{b:02X}");
                }
                s.push('\'');
                Ok(s)
            }
        }
    }

    fn should_export_row(
        table: &str,
        columns: &[String],
        values: &[Value],
        policy: &SyncPreservationPolicy,
    ) -> Result<bool, AppError> {
        if table != "settings" {
            return Ok(true);
        }

        let Some(key_idx) = columns.iter().position(|column| column == "key") else {
            return Ok(true);
        };
        let Some(key) = Self::value_as_str(&values[key_idx]) else {
            return Ok(true);
        };

        Ok(!policy
            .local_settings_keys
            .iter()
            .any(|local_key| *local_key == key))
    }

    fn neutralize_export_row(
        table: &str,
        columns: &[String],
        values: &mut [Value],
        policy: &SyncPreservationPolicy,
    ) {
        let Some(group) = policy
            .row_keyed_column_groups
            .iter()
            .find(|group| group.table == table)
        else {
            return;
        };

        for default in group.export_defaults {
            if let Some(idx) = columns.iter().position(|column| column == default.column) {
                values[idx] = default.value.into_sql_value();
            }
        }
    }

    fn value_as_str(value: &Value) -> Option<&str> {
        match value {
            Value::Text(text) => Some(text.as_str()),
            _ => None,
        }
    }

    fn quote_ident(value: &str) -> String {
        format!("\"{}\"", value.replace('"', "\"\""))
    }
}

#[cfg(test)]
mod tests {
    use super::Database;
    use crate::error::AppError;
    use rusqlite::Connection;

    fn seed_provider(conn: &Connection, id: &str) -> Result<(), AppError> {
        conn.execute(
            "INSERT INTO providers (id, app_type, name, settings_config, meta)
             VALUES (?1, 'claude', ?2, '{}', '{}')",
            rusqlite::params![id, format!("Provider {id}")],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn set_proxy_row(
        conn: &Connection,
        app_type: &str,
        proxy_enabled: bool,
        listen_address: &str,
        listen_port: i64,
        enabled: bool,
        auto_failover_enabled: bool,
        max_retries: i64,
    ) -> Result<(), AppError> {
        conn.execute(
            "UPDATE proxy_config
             SET proxy_enabled = ?2,
                 listen_address = ?3,
                 listen_port = ?4,
                 enabled = ?5,
                 auto_failover_enabled = ?6,
                 max_retries = ?7
             WHERE app_type = ?1",
            rusqlite::params![
                app_type,
                if proxy_enabled { 1 } else { 0 },
                listen_address,
                listen_port,
                if enabled { 1 } else { 0 },
                if auto_failover_enabled { 1 } else { 0 },
                max_retries,
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    fn read_proxy_row(
        conn: &Connection,
        app_type: &str,
    ) -> Result<(bool, String, i64, bool, bool, i64), AppError> {
        conn.query_row(
            "SELECT proxy_enabled, listen_address, listen_port, enabled, auto_failover_enabled, max_retries
             FROM proxy_config WHERE app_type = ?1",
            [app_type],
            |row| {
                Ok((
                    row.get::<_, i64>(0)? != 0,
                    row.get(1)?,
                    row.get(2)?,
                    row.get::<_, i64>(3)? != 0,
                    row.get::<_, i64>(4)? != 0,
                    row.get(5)?,
                ))
            },
        )
        .map_err(|e| AppError::Database(e.to_string()))
    }

    #[test]
    fn sync_import_preserves_local_only_tables() -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            conn.execute(
                "INSERT INTO providers (id, app_type, name, settings_config, meta)
                 VALUES ('remote-provider', 'claude', 'Remote Provider', '{}', '{}')",
                [],
            )?;
        }
        let remote_sql = remote_db.export_sql_string_for_sync()?;

        let local_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(local_db.conn);
            conn.execute(
                "INSERT INTO providers (id, app_type, name, settings_config, meta)
                 VALUES ('local-provider', 'claude', 'Local Provider', '{}', '{}')",
                [],
            )?;
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model,
                    input_tokens, output_tokens, total_cost_usd,
                    latency_ms, status_code, created_at
                ) VALUES ('req-1', 'local-provider', 'claude', 'claude-3', 100, 50, '0.01', 120, 200, 1000)",
                [],
            )?;
            conn.execute(
                "INSERT INTO usage_daily_rollups (
                    date, app_type, provider_id, model, request_count, success_count,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    total_cost_usd, avg_latency_ms
                ) VALUES ('2026-03-01', 'claude', 'local-provider', 'claude-3', 7, 7, 700, 350, 0, 0, '0.07', 120)",
                [],
            )?;
            conn.execute(
                "INSERT INTO stream_check_logs (
                    provider_id, provider_name, app_type, status, success, message,
                    response_time_ms, http_status, model_used, retry_count, tested_at
                ) VALUES ('local-provider', 'Local Provider', 'claude', 'operational', 1, 'ok', 42, 200, 'claude-3', 0, 1000)",
                [],
            )?;
        }

        local_db.import_sql_string_for_sync(&remote_sql)?;

        let remote_provider_exists: i64 = {
            let conn = crate::database::lock_conn!(local_db.conn);
            conn.query_row(
                "SELECT COUNT(*) FROM providers WHERE id = 'remote-provider' AND app_type = 'claude'",
                [],
                |row| row.get(0),
            )?
        };
        assert_eq!(
            remote_provider_exists, 1,
            "remote config should be imported"
        );

        let (request_logs, rollups, stream_logs): (i64, i64, i64) = {
            let conn = crate::database::lock_conn!(local_db.conn);
            let request_logs =
                conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |row| {
                    row.get(0)
                })?;
            let rollups =
                conn.query_row("SELECT COUNT(*) FROM usage_daily_rollups", [], |row| {
                    row.get(0)
                })?;
            let stream_logs =
                conn.query_row("SELECT COUNT(*) FROM stream_check_logs", [], |row| {
                    row.get(0)
                })?;
            (request_logs, rollups, stream_logs)
        };
        assert_eq!(request_logs, 1, "local request logs should be preserved");
        assert_eq!(rollups, 1, "local rollups should be preserved");
        assert_eq!(
            stream_logs, 1,
            "local stream check logs should be preserved"
        );

        Ok(())
    }

    #[test]
    fn sync_import_preserves_local_settings_keys() -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
            conn.execute(
                "INSERT INTO settings (key, value) VALUES ('proxy_runtime_session', '{\"pid\":999}')",
                [],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        }
        let remote_sql = remote_db.export_sql_string()?;

        let local_db = Database::memory()?;
        local_db
            .set_setting("proxy_runtime_session", "{\"pid\":123}")
            .expect("persist local runtime session");
        {
            let conn = crate::database::lock_conn!(local_db.conn);
            seed_provider(&conn, "local-provider")?;
        }

        local_db.import_sql_string_for_sync(&remote_sql)?;

        assert_eq!(
            local_db
                .get_setting("proxy_runtime_session")
                .expect("read local runtime session after import")
                .as_deref(),
            Some("{\"pid\":123}")
        );

        Ok(())
    }

    #[test]
    fn sync_import_preserves_local_proxy_state_and_keeps_remote_strategy() -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
            set_proxy_row(
                &conn,
                "claude",
                false,
                "192.168.10.10",
                31001,
                false,
                true,
                9,
            )?;
            set_proxy_row(&conn, "codex", true, "192.168.10.11", 31002, true, false, 8)?;
            set_proxy_row(
                &conn,
                "gemini",
                false,
                "192.168.10.12",
                31003,
                true,
                true,
                7,
            )?;
            conn.execute(
                "INSERT INTO settings (key, value) VALUES ('proxy_runtime_session', '{\"pid\":999}')",
                [],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        }
        let remote_sql = remote_db.export_sql_string()?;

        let local_db = Database::memory()?;
        local_db
            .set_setting("proxy_runtime_session", "{\"pid\":123}")
            .expect("persist local runtime session");
        {
            let conn = crate::database::lock_conn!(local_db.conn);
            seed_provider(&conn, "local-provider")?;
            set_proxy_row(&conn, "claude", true, "10.0.0.1", 21001, true, false, 1)?;
            set_proxy_row(&conn, "codex", false, "10.0.0.2", 21002, false, true, 2)?;
            set_proxy_row(&conn, "gemini", true, "10.0.0.3", 21003, false, false, 3)?;
        }

        local_db.import_sql_string_for_sync(&remote_sql)?;

        let conn = crate::database::lock_conn!(local_db.conn);
        assert_eq!(
            read_proxy_row(&conn, "claude")?,
            (true, "10.0.0.1".to_string(), 21001, true, true, 9)
        );
        assert_eq!(
            read_proxy_row(&conn, "codex")?,
            (false, "10.0.0.2".to_string(), 21002, false, false, 8)
        );
        assert_eq!(
            read_proxy_row(&conn, "gemini")?,
            (true, "10.0.0.3".to_string(), 21003, false, true, 7)
        );

        drop(conn);
        assert_eq!(
            local_db
                .get_setting("proxy_runtime_session")
                .expect("read local runtime session after overlay")
                .as_deref(),
            Some("{\"pid\":123}")
        );

        Ok(())
    }

    #[test]
    fn sync_export_scrubbed_snapshot_old_client_behavior_is_neutral_not_poisoned(
    ) -> Result<(), AppError> {
        let db = Database::memory()?;
        db.set_setting("proxy_runtime_session", "{\"pid\":456}")
            .expect("persist runtime session");
        {
            let conn = crate::database::lock_conn!(db.conn);
            seed_provider(&conn, "portable-provider")?;
            set_proxy_row(&conn, "claude", true, "10.1.0.1", 41001, true, true, 6)?;
            set_proxy_row(&conn, "codex", true, "10.1.0.2", 41002, true, false, 5)?;
            set_proxy_row(&conn, "gemini", true, "10.1.0.3", 41003, true, true, 4)?;
        }

        let sync_sql = db.export_sql_string_for_sync()?;
        assert!(
            !sync_sql.contains("proxy_runtime_session"),
            "sync export should omit runtime session key:\n{sync_sql}"
        );

        let old_client_db = Database::memory()?;
        old_client_db.import_sql_string(&sync_sql)?;
        let conn = crate::database::lock_conn!(old_client_db.conn);
        assert_eq!(
            read_proxy_row(&conn, "claude")?,
            (false, "127.0.0.1".to_string(), 15721, false, true, 6)
        );
        assert_eq!(
            read_proxy_row(&conn, "codex")?,
            (false, "127.0.0.1".to_string(), 15721, false, false, 5)
        );
        assert_eq!(
            read_proxy_row(&conn, "gemini")?,
            (false, "127.0.0.1".to_string(), 15721, false, true, 4)
        );
        drop(conn);
        assert!(
            old_client_db
                .get_setting("proxy_runtime_session")
                .expect("read runtime session from old client import")
                .is_none(),
            "old client import should not receive runtime session marker"
        );

        Ok(())
    }
}
