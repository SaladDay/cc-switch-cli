//! 数据库备份和恢复
//!
//! 提供 SQL 导出/导入和二进制快照备份功能。

use super::{create_secure_dir_all, lock_conn, Database, DB_BACKUP_RETAIN};
use crate::error::AppError;
use chrono::Utc;
use rusqlite::backup::{Backup, StepResult};
use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use tempfile::{Builder, NamedTempFile};

const CC_SWITCH_SQL_EXPORT_HEADER: &str = "-- CC Switch SQLite 导出";
const INSERT_BATCH_MAX_ROWS: usize = 200;
const INSERT_BATCH_MAX_BYTES: usize = 1024 * 1024;

/// Serialize operations that create or prune database backup files. Always
/// acquire this guard before `Database.conn` so import and normal backup paths
/// share a single lock order.
static BACKUP_FILE_OPERATION_LOCK: Mutex<()> = Mutex::new(());
type BackupFileOperationGuard = MutexGuard<'static, ()>;

fn lock_backup_file_operations() -> Result<BackupFileOperationGuard, AppError> {
    BACKUP_FILE_OPERATION_LOCK
        .lock()
        .map_err(|e| AppError::Database(format!("Backup file operation lock failed: {e}")))
}

const IMPORT_ALLOWED_PRAGMAS: &[&str] = &["foreign_keys", "user_version"];

/// Imported SQL executes in a disposable staging database, but it must not be
/// allowed to attach, create, or mutate files outside that database.
fn import_authorizer(context: rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization {
    use rusqlite::hooks::{AuthAction, Authorization};

    let escapes_staging_db = match context.action {
        AuthAction::Attach { .. } | AuthAction::Detach { .. } => true,
        AuthAction::CreateVtable { .. } | AuthAction::DropVtable { .. } => true,
        AuthAction::Unknown { .. } => true,
        AuthAction::Pragma { pragma_name, .. } => !IMPORT_ALLOWED_PRAGMAS
            .iter()
            .any(|allowed| pragma_name.eq_ignore_ascii_case(allowed)),
        _ => false,
    };

    if escapes_staging_db {
        log::warn!(
            "SQL import rejected an out-of-bound statement: {:?}",
            context.action
        );
        Authorization::Deny
    } else {
        Authorization::Allow
    }
}

// A full-copy step keeps one source snapshot stable for the entire copy. The
// connection's busy handler already performs bounded lock retries, so adding
// another retry loop here would multiply `busy_timeout` and reintroduce long
// startup stalls.
pub(crate) fn run_sqlite_backup_to_completion(backup: &Backup<'_, '_>) -> Result<(), AppError> {
    run_full_backup_step(|pages| {
        backup
            .step(pages)
            .map_err(|e| AppError::Database(e.to_string()))
    })
}

fn run_full_backup_step<Step>(mut step: Step) -> Result<(), AppError>
where
    Step: FnMut(i32) -> Result<StepResult, AppError>,
{
    match step(-1)? {
        StepResult::Done => Ok(()),
        StepResult::Busy => Err(AppError::Database(
            "SQLite backup could not acquire a required lock before busy_timeout elapsed"
                .to_string(),
        )),
        StepResult::Locked => Err(AppError::Database(
            "SQLite backup source connection is locked by an active write".to_string(),
        )),
        StepResult::More => Err(AppError::Database(
            "SQLite backup did not complete a full-copy step".to_string(),
        )),
        _ => Err(AppError::Database(
            "SQLite backup returned an unsupported step result".to_string(),
        )),
    }
}

const SYNC_IMPORT_RESTORE_TABLES: &[&str] = &[
    "proxy_request_logs",
    // This cursor describes files on the current device. Cost projection does
    // not read it, but importing another device's cursor could still skip
    // local usage lines permanently.
    "session_log_sync",
    "stream_check_logs",
    "proxy_live_backup",
    "proxy_failover_live_snapshots",
    "usage_daily_rollups",
    "session_usage_dedup",
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
        column: "auto_failover_enabled",
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
    /// Create the normal SQLite snapshot before a schema migration has
    /// quiesced an older daemon. SQLite's online backup API gives us a
    /// consistent source snapshot while the daemon remains available; if the
    /// backup directory is unwritable or the disk is full, initialization can
    /// fail without taking a working proxy offline.
    pub(crate) fn backup_database_path(database_path: &Path) -> Result<Option<PathBuf>, AppError> {
        if !database_path.exists() {
            return Ok(None);
        }

        let conn =
            Connection::open_with_flags(database_path, super::readonly_database_open_flags())
                .map_err(|error| AppError::Database(error.to_string()))?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|error| AppError::Database(error.to_string()))?;

        let snapshot_source = Self {
            conn: Mutex::new(conn),
            runtime_key: format!("file:{}", database_path.display()),
            db_path: Some(database_path.to_path_buf()),
        };
        snapshot_source.backup_database_file()
    }

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
            create_secure_dir_all(parent)?;
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
        self.import_sql_string_inner_with_hook(sql_raw, policy, || Ok(()))
    }

    fn import_sql_string_inner_with_hook<F>(
        &self,
        sql_raw: &str,
        policy: Option<&SyncPreservationPolicy>,
        on_staging_ready: F,
    ) -> Result<String, AppError>
    where
        F: FnOnce() -> Result<(), AppError>,
    {
        let sql_content = sql_raw.trim_start_matches('\u{feff}');
        Self::validate_cc_switch_sql_export(sql_content)?;

        // 在临时数据库执行导入，确保失败不会污染主库
        let temp_file = NamedTempFile::new().map_err(|e| AppError::IoContext {
            context: "创建临时数据库文件失败".to_string(),
            source: e,
        })?;
        let temp_path = temp_file.path().to_path_buf();
        let temp_conn =
            Connection::open(&temp_path).map_err(|e| AppError::Database(e.to_string()))?;

        // 在建表前把临时库设为增量 auto-vacuum。稍后用 SQLite Backup 把临时库整体
        // 写回主库时会连同数据库头（含 auto_vacuum 模式）一起复制，因此这一步能保证
        // 导入 / WebDAV 下载后主库仍保持 INCREMENTAL——否则临时库默认的 NONE 会被写回
        // 主库，令 issue #327 的膨胀问题在每次同步后复发。
        temp_conn
            .execute("PRAGMA auto_vacuum = INCREMENTAL;", [])
            .map_err(|e| AppError::Database(e.to_string()))?;

        temp_conn.authorizer(Some(import_authorizer));
        let batch_result = temp_conn.execute_batch(sql_content);
        temp_conn.authorizer(
            None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
        );
        batch_result.map_err(|e| AppError::Database(format!("执行 SQL 导入失败: {e}")))?;
        if !temp_conn.is_autocommit() {
            let _ = temp_conn.execute_batch("ROLLBACK;");
            return Err(AppError::localized(
                "backup.sql.incomplete_transaction",
                "SQL 备份事务未完成，文件可能已截断。",
                "The SQL backup transaction is incomplete; the file may be truncated.",
            ));
        }

        // Validate the schema supplied by the backup before migrations can
        // create missing tables and accidentally make a truncated file valid.
        Self::validate_imported_schema(&temp_conn)?;

        // 补齐缺失表/索引并执行迁移
        Self::create_tables_on_conn(&temp_conn)?;
        Self::apply_schema_migrations_on_conn(&temp_conn)?;
        Self::validate_imported_schema_contract(&temp_conn)?;
        on_staging_ready()?;

        let backup_file_guard = lock_backup_file_operations()?;
        // Keep one live-database guard across the safety backup, local overlay,
        // and replacement. This makes the backup an exact rollback point and
        // preserves writes that arrive while the remote SQL is being staged.
        let backup_path = {
            let mut main_conn = lock_conn!(self.conn);
            let backup_path =
                self.backup_database_file_from_conn(&backup_file_guard, &main_conn)?;
            if let Some(policy) = policy {
                Self::restore_sync_local_overlay(&main_conn, &temp_conn, policy)?;
            }
            Self::clear_imported_auto_failover_flags(&temp_conn)?;
            let backup = Backup::new(&temp_conn, &mut main_conn)
                .map_err(|e| AppError::Database(e.to_string()))?;
            run_sqlite_backup_to_completion(&backup)?;
            backup_path
        };

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
            run_sqlite_backup_to_completion(&backup)?;
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
        let tx = target_conn
            .unchecked_transaction()
            .map_err(|e| AppError::Database(format!("开启恢复事务失败: {e}")))?;

        for table in tables {
            if !Self::table_exists(source_conn, table)? || !Self::table_exists(&tx, table)? {
                continue;
            }

            let columns = Self::get_table_columns(source_conn, table)?;
            if columns.is_empty() {
                continue;
            }

            let quoted_table = Self::quote_ident(table);
            let quoted_columns = columns
                .iter()
                .map(|column| Self::quote_ident(column))
                .collect::<Vec<_>>()
                .join(", ");

            tx.execute(&format!("DELETE FROM {quoted_table}"), [])
                .map_err(|e| AppError::Database(format!("清空表 {table} 失败: {e}")))?;

            let placeholders = (1..=columns.len())
                .map(|idx| format!("?{idx}"))
                .collect::<Vec<_>>()
                .join(", ");
            let insert_sql =
                format!("INSERT INTO {quoted_table} ({quoted_columns}) VALUES ({placeholders})");
            let mut insert_stmt = tx
                .prepare(&insert_sql)
                .map_err(|e| AppError::Database(format!("准备表 {table} 插入语句失败: {e}")))?;

            let mut stmt = source_conn
                .prepare(&format!("SELECT {quoted_columns} FROM {quoted_table}"))
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

                insert_stmt
                    .execute(rusqlite::params_from_iter(values.iter()))
                    .map_err(|e| AppError::Database(format!("恢复表 {table} 数据失败: {e}")))?;
            }
        }

        Self::restore_sqlite_sequences(source_conn, &tx, tables)?;

        tx.commit()
            .map_err(|e| AppError::Database(format!("提交恢复事务失败: {e}")))?;
        Ok(())
    }

    fn restore_sqlite_sequences(
        source_conn: &Connection,
        target_conn: &Connection,
        tables: &[&str],
    ) -> Result<(), AppError> {
        if !Self::table_exists(source_conn, "sqlite_sequence")?
            || !Self::table_exists(target_conn, "sqlite_sequence")?
        {
            return Ok(());
        }

        let mut source_stmt = source_conn
            .prepare(
                "SELECT seq FROM sqlite_sequence
                 WHERE name = ?1 ORDER BY rowid DESC LIMIT 1",
            )
            .map_err(|e| AppError::Database(format!("读取 AUTOINCREMENT 序列失败: {e}")))?;
        for table in tables {
            target_conn
                .execute("DELETE FROM sqlite_sequence WHERE name = ?1", [*table])
                .map_err(|e| {
                    AppError::Database(format!("清理表 {table} 的 AUTOINCREMENT 序列失败: {e}"))
                })?;

            let mut rows = source_stmt
                .query([*table])
                .map_err(|e| AppError::Database(format!("查询表 {table} 序列失败: {e}")))?;
            if let Some(row) = rows.next().map_err(|e| AppError::Database(e.to_string()))? {
                let sequence = row
                    .get::<_, Value>(0)
                    .map_err(|e| AppError::Database(format!("解析表 {table} 序列失败: {e}")))?;
                target_conn
                    .execute(
                        "INSERT INTO sqlite_sequence (name, seq) VALUES (?1, ?2)",
                        rusqlite::params![table, sequence],
                    )
                    .map_err(|e| {
                        AppError::Database(format!("恢复表 {table} 的 AUTOINCREMENT 序列失败: {e}"))
                    })?;
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

    fn clear_imported_auto_failover_flags(target_conn: &Connection) -> Result<(), AppError> {
        if !Self::table_exists(target_conn, "proxy_config")? {
            return Ok(());
        }
        let columns = Self::get_table_columns(target_conn, "proxy_config")?;
        if !columns
            .iter()
            .any(|column| column == "auto_failover_enabled")
        {
            return Ok(());
        }

        target_conn
            .execute(
                "UPDATE proxy_config
                 SET auto_failover_enabled = 0
                 WHERE auto_failover_enabled != 0",
                [],
            )
            .map_err(|e| AppError::Database(format!("清理导入的自动故障转移状态失败: {e}")))?;

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
        let backup_file_guard = lock_backup_file_operations()?;
        let conn = lock_conn!(self.conn);
        self.backup_database_file_from_conn(&backup_file_guard, &conn)
    }

    /// Create a safety backup while the caller already owns both the backup
    /// file guard and the live database guard.
    fn backup_database_file_from_conn(
        &self,
        backup_file_guard: &BackupFileOperationGuard,
        source_conn: &Connection,
    ) -> Result<Option<PathBuf>, AppError> {
        self.backup_database_file_from_conn_with_hook(backup_file_guard, source_conn, |_| Ok(()))
    }

    /// Testable publication boundary for the safety snapshot. Production uses
    /// a no-op hook; tests use it to make collision and failure paths
    /// deterministic without weakening the atomic no-clobber publication.
    fn backup_database_file_from_conn_with_hook<F>(
        &self,
        _backup_file_guard: &BackupFileOperationGuard,
        source_conn: &Connection,
        before_first_publish: F,
    ) -> Result<Option<PathBuf>, AppError>
    where
        F: FnOnce(&Path) -> Result<(), AppError>,
    {
        let Some(db_path) = self.db_path.as_deref() else {
            return Ok(None);
        };
        if !db_path.exists() {
            return Ok(None);
        }

        let backup_dir = db_path
            .parent()
            .ok_or_else(|| AppError::Config("无效的数据库路径".to_string()))?
            .join("backups");

        // The migration coordinator can supply a database outside the
        // process-wide config root, so always create the sibling directory.
        create_secure_dir_all(&backup_dir)?;
        // For the normal CC-Switch database, also reject an existing managed
        // backup directory with unsafe permissions.
        if super::database_path()
            .is_ok_and(|managed_database_path| managed_database_path == db_path)
        {
            crate::config::create_managed_config_dir_all(
                &crate::config::get_app_config_dir().join("backups"),
            )?;
        }

        // Build and validate under a non-.db name. Backup discovery and
        // retention only see the final path after atomic publication.
        let mut temp_path = Builder::new()
            .prefix(".cc-switch-backup-")
            .suffix(".tmp")
            .tempfile_in(&backup_dir)
            .map_err(|e| AppError::io(&backup_dir, e))?
            .into_temp_path();
        let temp_db_path: &Path = temp_path.as_ref();
        let open_path = Self::canonicalize_existing_parent(temp_db_path)?;
        let mut dest_conn = Connection::open_with_flags(
            open_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        let backup_result = match Backup::new(source_conn, &mut dest_conn) {
            Ok(backup) => run_sqlite_backup_to_completion(&backup),
            Err(err) => Err(AppError::Database(err.to_string())),
        };
        backup_result?;
        Self::validate_sqlite_integrity(&dest_conn)?;
        dest_conn
            .close()
            .map_err(|(_, e)| AppError::Database(format!("关闭数据库安全备份失败: {e}")))?;

        let mut candidate = backup_dir.join(format!("{}.db", Self::new_db_backup_id()));
        before_first_publish(&candidate)?;
        let backup_path = loop {
            match temp_path.persist_noclobber(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.error.kind() == ErrorKind::AlreadyExists => {
                    temp_path = error.path;
                    candidate = backup_dir.join(format!("{}.db", Self::new_db_backup_id()));
                }
                Err(error) => return Err(AppError::io(&candidate, error.error)),
            }
        };

        Self::cleanup_db_backups(&backup_dir, &[backup_path.as_path()])?;
        Ok(Some(backup_path))
    }

    #[cfg(test)]
    fn remove_incomplete_backup(backup_path: &Path) {
        let mut artifacts = vec![backup_path.to_path_buf()];
        for suffix in ["-journal", "-wal", "-shm"] {
            let mut artifact = backup_path.as_os_str().to_os_string();
            artifact.push(suffix);
            artifacts.push(PathBuf::from(artifact));
        }

        for artifact in artifacts {
            match fs::remove_file(&artifact) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => log::warn!(
                    "Failed to remove incomplete database backup {}: {err}",
                    artifact.display()
                ),
            }
        }
    }

    fn new_db_backup_id() -> String {
        static NEXT_BACKUP_ID: AtomicU64 = AtomicU64::new(0);

        format!(
            "db_backup_{}_{}_{}",
            Utc::now().format("%Y%m%d_%H%M%S_%f"),
            std::process::id(),
            NEXT_BACKUP_ID.fetch_add(1, Ordering::Relaxed)
        )
    }

    #[cfg(test)]
    pub(super) fn create_backup_db_connection(backup_path: &Path) -> Result<Connection, AppError> {
        Self::try_create_backup_db_connection(backup_path)?.ok_or_else(|| AppError::Io {
            path: backup_path.display().to_string(),
            source: std::io::Error::new(
                ErrorKind::AlreadyExists,
                "database backup path already exists",
            ),
        })
    }

    #[cfg(test)]
    fn try_create_backup_db_connection(backup_path: &Path) -> Result<Option<Connection>, AppError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            match std::fs::symlink_metadata(backup_path) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(AppError::InvalidInput(format!(
                        "数据库备份文件不能是符号链接: {}",
                        backup_path.display()
                    )));
                }
                Ok(meta) if meta.is_file() => return Ok(None),
                Ok(_) => {
                    return Err(AppError::InvalidInput(format!(
                        "数据库备份路径不是普通文件: {}",
                        backup_path.display()
                    )));
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    match std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(backup_path)
                    {
                        Ok(_) => {}
                        Err(err) if err.kind() == ErrorKind::AlreadyExists => return Ok(None),
                        Err(err) => return Err(AppError::io(backup_path, err)),
                    }
                }
                Err(err) => return Err(AppError::io(backup_path, err)),
            }

            let open_result = (|| {
                let open_path = Self::canonicalize_existing_parent(backup_path)?;
                let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | OpenFlags::SQLITE_OPEN_NOFOLLOW;
                Connection::open_with_flags(&open_path, flags)
                    .map_err(|e| AppError::Database(e.to_string()))
            })();

            match open_result {
                Ok(conn) => Ok(Some(conn)),
                Err(err) => {
                    Self::remove_incomplete_backup(backup_path);
                    Err(err)
                }
            }
        }

        #[cfg(not(unix))]
        {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(backup_path)
            {
                Ok(_) => {}
                Err(err) if err.kind() == ErrorKind::AlreadyExists => return Ok(None),
                Err(err) => return Err(AppError::io(backup_path, err)),
            }
            let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW;
            match Connection::open_with_flags(backup_path, flags)
                .map_err(|e| AppError::Database(e.to_string()))
            {
                Ok(conn) => Ok(Some(conn)),
                Err(err) => {
                    Self::remove_incomplete_backup(backup_path);
                    Err(err)
                }
            }
        }
    }

    fn canonicalize_existing_parent(path: &Path) -> Result<PathBuf, AppError> {
        let Some(file_name) = path.file_name() else {
            return Err(AppError::InvalidInput(format!(
                "数据库备份路径缺少文件名: {}",
                path.display()
            )));
        };
        let parent = path
            .parent()
            .ok_or_else(|| AppError::InvalidInput(format!("无效路径: {}", path.display())))?;
        let parent = parent.canonicalize().map_err(|e| AppError::io(parent, e))?;
        Ok(parent.join(file_name))
    }

    /// 清理旧的数据库备份，保留最新的 N 个
    fn cleanup_db_backups(dir: &Path, protected_paths: &[&Path]) -> Result<(), AppError> {
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

        let mut removed = 0;
        for entry in sorted {
            if removed >= remove_count {
                break;
            }
            let path = entry.path();
            if protected_paths.iter().any(|protected| {
                fs::canonicalize(&path)
                    .ok()
                    .zip(fs::canonicalize(protected).ok())
                    .is_some_and(|(left, right)| left == right)
                    || path.as_path() == *protected
            }) {
                continue;
            }
            if let Err(err) = fs::remove_file(&path) {
                log::warn!("删除旧数据库备份失败 {}: {}", path.display(), err);
            } else {
                removed += 1;
            }
        }
        Ok(())
    }

    fn validate_sqlite_integrity(conn: &Connection) -> Result<(), AppError> {
        let mut stmt = conn
            .prepare("PRAGMA quick_check;")
            .map_err(|e| AppError::Database(format!("检查数据库完整性失败: {e}")))?;
        let results = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| AppError::Database(format!("检查数据库完整性失败: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Database(format!("检查数据库完整性失败: {e}")))?;

        if results.len() == 1 && results[0].eq_ignore_ascii_case("ok") {
            return Ok(());
        }

        Err(AppError::localized(
            "backup.db.integrity_failed",
            format!("数据库备份完整性检查失败: {}", results.join("; ")),
            format!(
                "Database backup integrity check failed: {}",
                results.join("; ")
            ),
        ))
    }

    /// Validate the schema produced by the external SQL before migrations can
    /// fill missing tables. Empty but genuine backups remain valid.
    fn validate_imported_schema(conn: &Connection) -> Result<(), AppError> {
        const REQUIRED_TABLES: &[&str] = &[
            "providers",
            "provider_endpoints",
            "mcp_servers",
            "prompts",
            "skills",
            "skill_repos",
            "settings",
        ];

        let mut missing = Vec::new();
        for table in REQUIRED_TABLES {
            if !Self::table_exists(conn, table)? {
                missing.push(*table);
            }
        }
        if !missing.is_empty() {
            let names = missing.join(", ");
            return Err(AppError::localized(
                "backup.sql.invalid_schema",
                format!("导入的 SQL 缺少 CC Switch 必需表：{names}"),
                format!("The imported SQL is missing required CC Switch tables: {names}"),
            ));
        }
        Ok(())
    }

    /// `CREATE TABLE IF NOT EXISTS` cannot repair a same-named malformed
    /// table. Check the columns and primary keys that every supported schema
    /// version must expose after migration before replacing the live database.
    fn validate_imported_schema_contract(conn: &Connection) -> Result<(), AppError> {
        const CONTRACTS: &[(&str, &[&str], &[&str])] = &[
            (
                "providers",
                &[
                    "id",
                    "app_type",
                    "name",
                    "settings_config",
                    "website_url",
                    "category",
                    "created_at",
                    "sort_index",
                    "notes",
                    "icon",
                    "icon_color",
                    "meta",
                    "is_current",
                    "in_failover_queue",
                ],
                &["id", "app_type"],
            ),
            (
                "provider_endpoints",
                &["id", "provider_id", "app_type", "url", "added_at"],
                &["id"],
            ),
            (
                "mcp_servers",
                &[
                    "id",
                    "name",
                    "server_config",
                    "description",
                    "homepage",
                    "docs",
                    "tags",
                    "enabled_claude",
                    "enabled_codex",
                    "enabled_gemini",
                    "enabled_grokbuild",
                    "enabled_opencode",
                    "enabled_hermes",
                ],
                &["id"],
            ),
            (
                "prompts",
                &[
                    "id",
                    "app_type",
                    "name",
                    "content",
                    "description",
                    "enabled",
                    "created_at",
                    "updated_at",
                ],
                &["id", "app_type"],
            ),
            (
                "skills",
                &[
                    "id",
                    "name",
                    "description",
                    "directory",
                    "repo_owner",
                    "repo_name",
                    "repo_branch",
                    "readme_url",
                    "enabled_claude",
                    "enabled_codex",
                    "enabled_gemini",
                    "enabled_grokbuild",
                    "enabled_opencode",
                    "enabled_hermes",
                    "installed_at",
                    "content_hash",
                    "updated_at",
                ],
                &["id"],
            ),
            (
                "skill_repos",
                &["owner", "name", "branch", "enabled"],
                &["owner", "name"],
            ),
            ("settings", &["key", "value"], &["key"]),
            (
                "session_usage_dedup",
                &["data_source", "request_id", "semantic_id", "has_entry_id"],
                &["data_source", "request_id"],
            ),
        ];

        let mut violations = Vec::new();
        for (table, required_columns, expected_primary_key) in CONTRACTS {
            let quoted_table = Self::quote_ident(table);
            let mut stmt = conn
                .prepare(&format!("PRAGMA table_info({quoted_table})"))
                .map_err(|e| AppError::Database(format!("Failed to inspect table {table}: {e}")))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
                })
                .map_err(|e| AppError::Database(format!("Failed to inspect table {table}: {e}")))?;
            let columns = rows
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| AppError::Database(format!("Failed to inspect table {table}: {e}")))?;

            for required in *required_columns {
                if !columns
                    .iter()
                    .any(|(actual, _)| actual.eq_ignore_ascii_case(required))
                {
                    violations.push(format!("{table}.{required} is missing"));
                }
            }

            let mut primary_key = columns
                .iter()
                .filter(|(_, ordinal)| *ordinal > 0)
                .map(|(name, ordinal)| (*ordinal, name.as_str()))
                .collect::<Vec<_>>();
            primary_key.sort_by_key(|(ordinal, _)| *ordinal);
            let primary_key = primary_key
                .into_iter()
                .map(|(_, name)| name)
                .collect::<Vec<_>>();
            if primary_key != *expected_primary_key {
                violations.push(format!(
                    "{table} has primary key ({}) instead of ({})",
                    primary_key.join(", "),
                    expected_primary_key.join(", ")
                ));
            }
        }

        let mut index_stmt = conn
            .prepare("PRAGMA index_list(\"session_usage_dedup\")")
            .map_err(|e| AppError::Database(format!("Failed to inspect dedup indexes: {e}")))?;
        let index_rows = index_stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(|e| AppError::Database(format!("Failed to inspect dedup indexes: {e}")))?;
        let indexes = index_rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Database(format!("Failed to inspect dedup indexes: {e}")))?;
        match indexes
            .iter()
            .find(|(name, _, _)| name == "idx_session_usage_dedup_semantic")
        {
            Some((_, unique, partial)) if *unique == 0 && *partial == 0 => {
                let mut columns_stmt = conn
                    .prepare("PRAGMA index_info(\"idx_session_usage_dedup_semantic\")")
                    .map_err(|e| {
                        AppError::Database(format!("Failed to inspect dedup index columns: {e}"))
                    })?;
                let column_rows = columns_stmt
                    .query_map([], |row| row.get::<_, Option<String>>(2))
                    .map_err(|e| {
                        AppError::Database(format!("Failed to inspect dedup index columns: {e}"))
                    })?;
                let columns = column_rows
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| {
                        AppError::Database(format!("Failed to inspect dedup index columns: {e}"))
                    })?
                    .into_iter()
                    .collect::<Option<Vec<_>>>();
                let expected = vec![
                    "data_source".to_string(),
                    "semantic_id".to_string(),
                    "has_entry_id".to_string(),
                ];
                if columns.as_ref() != Some(&expected) {
                    violations.push(format!(
                        "idx_session_usage_dedup_semantic has columns ({}) instead of ({})",
                        columns
                            .unwrap_or_default()
                            .into_iter()
                            .collect::<Vec<_>>()
                            .join(", "),
                        expected.join(", ")
                    ));
                }
            }
            Some(_) => violations.push(
                "idx_session_usage_dedup_semantic must be non-unique and non-partial".to_string(),
            ),
            None => violations.push("idx_session_usage_dedup_semantic is missing".to_string()),
        }

        if violations.is_empty() {
            return Ok(());
        }

        Err(AppError::localized(
            "backup.sql.invalid_schema",
            format!("导入的 SQL 表结构无效：{}", violations.join("; ")),
            format!(
                "The imported SQL has an invalid CC Switch schema: {}",
                violations.join("; ")
            ),
        ))
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
        let mut triggers = Vec::new();
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

            if obj_type == "trigger" {
                triggers.push(sql);
                continue;
            }

            output.push_str(&sql);
            output.push_str(";\n");

            if obj_type == "table" {
                tables.push(name);
            }
        }

        // 导出数据
        for table in tables {
            if Self::should_skip_export_table(&table, policy) {
                continue;
            }

            let columns = Self::get_table_columns(conn, &table)?;
            if columns.is_empty() {
                continue;
            }

            let quoted_table = Self::quote_ident(&table);
            let quoted_columns = columns
                .iter()
                .map(|column| Self::quote_ident(column))
                .collect::<Vec<_>>()
                .join(", ");
            let insert_prefix = format!("INSERT INTO {quoted_table} ({quoted_columns}) VALUES ");

            let mut stmt = conn
                .prepare(&format!("SELECT {quoted_columns} FROM {quoted_table}"))
                .map_err(|e| AppError::Database(e.to_string()))?;
            let mut rows = stmt
                .query([])
                .map_err(|e| AppError::Database(e.to_string()))?;

            let mut pending_rows = 0;
            let mut batch = String::new();
            while let Some(row) = rows.next().map_err(|e| AppError::Database(e.to_string()))? {
                if let Some(policy) = policy {
                    if table == "settings" {
                        if let Some(key_idx) = columns.iter().position(|column| column == "key") {
                            let key = row
                                .get_ref(key_idx)
                                .map_err(|e| AppError::Database(e.to_string()))?;
                            if matches!(key, ValueRef::Text(bytes) if std::str::from_utf8(bytes).is_ok_and(|key| policy.local_settings_keys.contains(&key)))
                            {
                                continue;
                            }
                        }
                    }
                }

                let mut values = Vec::with_capacity(columns.len());
                for (idx, column) in columns.iter().enumerate() {
                    if let Some(default) =
                        policy.and_then(|policy| Self::sync_export_default(&table, column, policy))
                    {
                        values.push(Self::format_owned_sql_value(&default.into_sql_value())?);
                    } else {
                        let value = row
                            .get_ref(idx)
                            .map_err(|e| AppError::Database(e.to_string()))?;
                        values.push(Self::format_sql_value(value)?);
                    }
                }

                let row_sql = format!("({})", values.join(", "));
                let separator_bytes = usize::from(pending_rows > 0);
                if pending_rows > 0
                    && batch.len() + separator_bytes + row_sql.len() + 2 > INSERT_BATCH_MAX_BYTES
                {
                    batch.push_str(";\n");
                    output.push_str(&batch);
                    pending_rows = 0;
                }

                if pending_rows == 0 {
                    batch.clear();
                    batch.push_str(&insert_prefix);
                } else {
                    batch.push(',');
                }
                batch.push_str(&row_sql);
                pending_rows += 1;

                if pending_rows >= INSERT_BATCH_MAX_ROWS {
                    batch.push_str(";\n");
                    output.push_str(&batch);
                    pending_rows = 0;
                }
            }
            if pending_rows > 0 {
                batch.push_str(";\n");
                output.push_str(&batch);
            }
        }

        Self::dump_sqlite_sequences(conn, policy, &mut output)?;

        for sql in triggers {
            output.push_str(&sql);
            output.push_str(";\n");
        }

        output.push_str("COMMIT;\nPRAGMA foreign_keys=ON;\n");
        Ok(output)
    }

    fn should_skip_export_table(table: &str, policy: Option<&SyncPreservationPolicy>) -> bool {
        policy.is_some_and(|policy| {
            policy
                .import_restore_tables
                .iter()
                .chain(policy.export_resettable_tables.iter())
                .any(|skip| *skip == table)
        })
    }

    fn dump_sqlite_sequences(
        conn: &Connection,
        policy: Option<&SyncPreservationPolicy>,
        output: &mut String,
    ) -> Result<(), AppError> {
        if !Self::table_exists(conn, "sqlite_sequence")? {
            return Ok(());
        }

        let mut stmt = conn
            .prepare("SELECT name, seq FROM sqlite_sequence ORDER BY name")
            .map_err(|e| AppError::Database(format!("读取 AUTOINCREMENT 序列失败: {e}")))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| AppError::Database(format!("查询 AUTOINCREMENT 序列失败: {e}")))?;
        let mut values = Vec::new();
        while let Some(row) = rows.next().map_err(|e| AppError::Database(e.to_string()))? {
            let table: String = row
                .get(0)
                .map_err(|e| AppError::Database(format!("解析 AUTOINCREMENT 表名失败: {e}")))?;
            if Self::should_skip_export_table(&table, policy) {
                continue;
            }
            let sequence = row
                .get_ref(1)
                .map_err(|e| AppError::Database(format!("解析表 {table} 序列失败: {e}")))?;
            values.push(format!(
                "({}, {})",
                Self::format_sql_value(ValueRef::Text(table.as_bytes()))?,
                Self::format_sql_value(sequence)?
            ));
        }

        output.push_str("DELETE FROM sqlite_sequence;\n");
        if !values.is_empty() {
            output.push_str("INSERT INTO sqlite_sequence (name, seq) VALUES ");
            output.push_str(&values.join(","));
            output.push_str(";\n");
        }
        Ok(())
    }

    /// 获取表的列名列表
    fn get_table_columns(conn: &Connection, table: &str) -> Result<Vec<String>, AppError> {
        let quoted_table = Self::quote_ident(table);
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({quoted_table})"))
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
            Value::Real(f) => Ok(Self::format_sql_real(*f)),
            Value::Text(text) if !text.contains('\0') => {
                Ok(format!("'{}'", text.replace('\'', "''")))
            }
            Value::Text(text) => Ok(format!(
                "CAST({} AS TEXT)",
                Self::format_sql_blob(text.as_bytes())
            )),
            Value::Blob(bytes) => Ok(Self::format_sql_blob(bytes)),
        }
    }

    fn format_sql_value(value: ValueRef<'_>) -> Result<String, AppError> {
        match value {
            ValueRef::Null => Ok("NULL".to_string()),
            ValueRef::Integer(i) => Ok(i.to_string()),
            ValueRef::Real(f) => Ok(Self::format_sql_real(f)),
            ValueRef::Text(bytes) => match std::str::from_utf8(bytes) {
                Ok(text) if !text.contains('\0') => Ok(format!("'{}'", text.replace('\'', "''"))),
                _ => Ok(format!("CAST({} AS TEXT)", Self::format_sql_blob(bytes))),
            },
            ValueRef::Blob(bytes) => Ok(Self::format_sql_blob(bytes)),
        }
    }

    fn format_sql_real(value: f64) -> String {
        if value.is_nan() {
            return "NULL".to_string();
        }
        if value.is_infinite() {
            return if value.is_sign_negative() {
                "-9.0e999".to_string()
            } else {
                "9.0e999".to_string()
            };
        }
        if value == 0.0 && value.is_sign_negative() {
            return "-0.0".to_string();
        }

        let mut literal = value.to_string();
        if !literal.contains(['.', 'e', 'E']) {
            literal.push_str(".0");
        }
        literal
    }

    fn format_sql_blob(bytes: &[u8]) -> String {
        let mut literal = String::from("X'");
        for byte in bytes {
            use std::fmt::Write;
            let _ = write!(&mut literal, "{byte:02X}");
        }
        literal.push('\'');
        literal
    }

    fn sync_export_default(
        table: &str,
        column: &str,
        policy: &SyncPreservationPolicy,
    ) -> Option<SyncNeutralValue> {
        policy
            .row_keyed_column_groups
            .iter()
            .find(|group| group.table == table)
            .and_then(|group| {
                group
                    .export_defaults
                    .iter()
                    .find(|default| default.column == column)
            })
            .map(|default| default.value)
    }

    fn quote_ident(value: &str) -> String {
        format!("\"{}\"", value.replace('"', "\"\""))
    }
}

#[cfg(test)]
mod tests {
    use super::{run_full_backup_step, Database};
    use crate::error::AppError;
    use rusqlite::{backup::StepResult, Connection};
    use std::fs;
    use std::time::{Duration, Instant};

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
    fn full_backup_requests_all_remaining_pages_in_one_step() {
        let mut requested_pages = Vec::new();

        run_full_backup_step(|pages| {
            requested_pages.push(pages);
            Ok(StepResult::Done)
        })
        .expect("a completed full-copy step should succeed");

        assert_eq!(requested_pages, vec![-1]);
    }

    #[test]
    fn full_backup_does_not_multiply_the_connection_busy_timeout() {
        let mut calls = 0usize;
        let error = run_full_backup_step(|pages| {
            assert_eq!(pages, -1);
            calls += 1;
            Ok(StepResult::Busy)
        })
        .expect_err("a busy result must be returned after the connection timeout");

        assert!(error.to_string().contains("busy_timeout"));
        assert_eq!(calls, 1, "the outer layer must not retry a timed-out step");
    }

    #[test]
    fn full_backup_rejects_locked_or_incomplete_steps() {
        let locked = run_full_backup_step(|_| Ok(StepResult::Locked))
            .expect_err("an actively written source connection must fail");
        assert!(locked.to_string().contains("active write"));

        let incomplete = run_full_backup_step(|pages| {
            assert_eq!(pages, -1);
            Ok(StepResult::More)
        })
        .expect_err("an unbounded SQLite backup step should complete atomically");
        assert!(incomplete.to_string().contains("full-copy step"));
    }

    #[test]
    fn dump_sql_batches_rows_into_multi_row_inserts() -> Result<(), AppError> {
        let source = Connection::open_in_memory()?;
        source.execute_batch(
            "CREATE TABLE batch_rows (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
             WITH RECURSIVE rows(id) AS (
                 VALUES(1) UNION ALL SELECT id + 1 FROM rows WHERE id < 450
             )
             INSERT INTO batch_rows SELECT id, printf('value-%d', id) FROM rows;",
        )?;

        let sql = Database::dump_sql(&source, None)?;
        assert_eq!(sql.matches("INSERT INTO \"batch_rows\"").count(), 3);

        let target = Connection::open_in_memory()?;
        target.execute_batch(&sql)?;
        let count: i64 =
            target.query_row("SELECT COUNT(*) FROM batch_rows", [], |row| row.get(0))?;
        assert_eq!(count, 450);
        Ok(())
    }

    #[test]
    fn dump_sql_round_trips_special_values_before_creating_triggers() -> Result<(), AppError> {
        let source = Connection::open_in_memory()?;
        source.execute_batch(
            r#"CREATE TABLE "special""rows" (
                 id INTEGER PRIMARY KEY,
                 "text""value" TEXT NOT NULL,
                 blob_value BLOB NOT NULL,
                 nullable TEXT,
                 generated TEXT GENERATED ALWAYS AS ("text""value" || '-generated') STORED
             );
             INSERT INTO "special""rows" (id, "text""value", blob_value, nullable) VALUES
                 (1, 'O''Brien,
第二行 😀', X'00FF10', NULL),
                 (2, 'kept', X'', 'value');
             CREATE TRIGGER ignore_second_row
             BEFORE INSERT ON "special""rows" WHEN NEW.id = 2
             BEGIN SELECT RAISE(IGNORE); END;"#,
        )?;

        let sql = Database::dump_sql(&source, None)?;
        assert!(
            sql.find("INSERT INTO \"special\"\"rows\"").unwrap()
                < sql.find("CREATE TRIGGER ignore_second_row").unwrap()
        );

        let target = Connection::open_in_memory()?;
        target.execute_batch(&sql)?;
        target.execute(
            r#"INSERT INTO "special""rows" (id, "text""value", blob_value)
               VALUES (2, 'ignored', X'01')"#,
            [],
        )?;
        let values: (String, String, Option<String>, String, i64) = target.query_row(
            r#"SELECT "text""value", hex(blob_value), nullable, generated,
                      (SELECT COUNT(*) FROM "special""rows")
               FROM "special""rows" WHERE id = 1"#,
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )?;
        assert_eq!(
            values,
            (
                "O'Brien,\n第二行 😀".into(),
                "00FF10".into(),
                None,
                "O'Brien,\n第二行 😀-generated".into(),
                2
            )
        );
        Ok(())
    }

    #[test]
    fn dump_sql_splits_batches_by_statement_size() -> Result<(), AppError> {
        let source = Connection::open_in_memory()?;
        source.execute(
            "CREATE TABLE large_rows (id INTEGER PRIMARY KEY, payload TEXT NOT NULL)",
            [],
        )?;
        let payload = "x".repeat(super::INSERT_BATCH_MAX_BYTES / 2 + 1024);
        for id in 1..=3 {
            source.execute(
                "INSERT INTO large_rows VALUES (?1, ?2)",
                rusqlite::params![id, payload],
            )?;
        }

        let sql = Database::dump_sql(&source, None)?;
        let inserts = sql
            .lines()
            .filter(|line| line.starts_with("INSERT INTO \"large_rows\""))
            .collect::<Vec<_>>();
        assert_eq!(inserts.len(), 3);
        assert!(inserts
            .iter()
            .all(|statement| statement.len() <= super::INSERT_BATCH_MAX_BYTES));
        Ok(())
    }

    #[test]
    fn restore_tables_rolls_back_all_tables_on_failure() -> Result<(), AppError> {
        let source = Connection::open_in_memory()?;
        source.execute_batch(
            r#"CREATE TABLE first_table (
                 value TEXT NOT NULL,
                 generated TEXT GENERATED ALWAYS AS (value || '-generated') STORED
             );
             CREATE TABLE second_table (value INTEGER NOT NULL);
             INSERT INTO first_table (value) VALUES ('replacement');
             INSERT INTO second_table VALUES (-1);"#,
        )?;
        let target = Connection::open_in_memory()?;
        target.execute_batch(
            r#"CREATE TABLE first_table (
                 value TEXT NOT NULL,
                 generated TEXT GENERATED ALWAYS AS (value || '-generated') STORED
             );
             CREATE TABLE second_table (value INTEGER NOT NULL CHECK (value >= 0));
             INSERT INTO first_table (value) VALUES ('sentinel');
             INSERT INTO second_table VALUES (7);"#,
        )?;

        let error = Database::restore_tables(&source, &target, &["first_table", "second_table"])
            .expect_err("the second table must fail its constraint");
        assert!(error.to_string().contains("second_table"), "{error}");

        let first: (String, String) =
            target.query_row("SELECT value, generated FROM first_table", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
        let second: i64 =
            target.query_row("SELECT value FROM second_table", [], |row| row.get(0))?;
        assert_eq!(
            (first.0.as_str(), first.1.as_str(), second),
            ("sentinel", "sentinel-generated", 7)
        );
        Ok(())
    }

    #[test]
    fn file_backup_does_not_retry_after_a_real_busy_timeout() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create temp dir");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());

        let db = Database::init()?;
        let db_path = crate::database::database_path()?;
        {
            let conn = crate::database::lock_conn!(db.conn);
            conn.pragma_update(None, "journal_mode", "DELETE")
                .map_err(|e| AppError::Database(e.to_string()))?;
            conn.busy_timeout(Duration::from_millis(50))
                .map_err(|e| AppError::Database(e.to_string()))?;
        }

        let locker = Connection::open(&db_path).map_err(|e| AppError::Database(e.to_string()))?;
        locker
            .execute_batch("BEGIN EXCLUSIVE;")
            .map_err(|e| AppError::Database(e.to_string()))?;

        let started = Instant::now();
        let error = db
            .backup_database_file()
            .expect_err("an exclusive source lock should make the backup fail");
        let elapsed = started.elapsed();
        locker
            .execute_batch("ROLLBACK;")
            .map_err(|e| AppError::Database(e.to_string()))?;

        assert!(
            elapsed < Duration::from_secs(1),
            "the outer backup layer must not multiply the 50ms busy timeout: {elapsed:?}"
        );
        assert!(error.to_string().contains("busy_timeout"));

        let backup_dir = db_path.parent().expect("database parent").join("backups");
        let artifacts = fs::read_dir(&backup_dir)
            .map_err(|e| AppError::io(&backup_dir, e))?
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        assert!(
            artifacts.is_empty(),
            "failed backups must not leave database or journal artifacts: {artifacts:?}"
        );

        Ok(())
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
            conn.execute(
                "INSERT INTO profiles (id, name, payload, sort_order, created_at, updated_at)
                 VALUES ('remote-profile', 'Remote Project', ?1, 1, 100, 200)",
                [r#"{"providers":{"claude-desktop":"desktop-provider"}}"#],
            )?;
            conn.execute(
                "INSERT INTO settings (key, value)
                 VALUES ('current_profile_id_claude-desktop', 'remote-profile')",
                [],
            )?;
            conn.execute(
                "INSERT INTO session_log_sync
                    (file_path, last_modified, last_line_offset, last_synced_at)
                 VALUES ('/shared/session.jsonl', 999, 999, 999)",
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
                    input_tokens, output_tokens, input_token_semantics, total_cost_usd,
                    latency_ms, status_code, created_at
                ) VALUES ('req-1', 'local-provider', 'claude', 'claude-3', 100, 50, 2, '0.01', 120, 200, 1000)",
                [],
            )?;
            conn.execute(
                "INSERT INTO usage_daily_rollups (
                    date, app_type, provider_id, model, request_count, success_count,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    input_token_semantics, total_cost_usd, avg_latency_ms
                ) VALUES ('2026-03-01', 'claude', 'local-provider', 'claude-3', 7, 7, 700, 350, 0, 0, 2, '0.07', 120)",
                [],
            )?;
            conn.execute(
                "INSERT INTO stream_check_logs (
                    provider_id, provider_name, app_type, status, success, message,
                    response_time_ms, http_status, model_used, retry_count, tested_at
                ) VALUES ('local-provider', 'Local Provider', 'claude', 'operational', 1, 'ok', 42, 200, 'claude-3', 0, 1000)",
                [],
            )?;
            conn.execute(
                "INSERT INTO session_log_sync
                    (file_path, last_modified, last_line_offset, last_synced_at)
                 VALUES ('/shared/session.jsonl', 123, 12, 1000)",
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

        let (profile_payload, current_profile): (String, String) = {
            let conn = crate::database::lock_conn!(local_db.conn);
            let payload = conn.query_row(
                "SELECT payload FROM profiles WHERE id = 'remote-profile'",
                [],
                |row| row.get(0),
            )?;
            let current = conn.query_row(
                "SELECT value FROM settings WHERE key = 'current_profile_id_claude-desktop'",
                [],
                |row| row.get(0),
            )?;
            (payload, current)
        };
        assert_eq!(
            profile_payload,
            r#"{"providers":{"claude-desktop":"desktop-provider"}}"#
        );
        assert_eq!(current_profile, "remote-profile");

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
        let local_sync: (i64, i64, i64) = {
            let conn = crate::database::lock_conn!(local_db.conn);
            conn.query_row(
                "SELECT last_modified, last_line_offset, last_synced_at
                 FROM session_log_sync
                 WHERE file_path = '/shared/session.jsonl'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?
        };
        assert_eq!(
            local_sync,
            (123, 12, 1000),
            "WebDAV restore must not replace local file progress with a remote device's cursor"
        );
        let semantics: (i64, i64) = {
            let conn = crate::database::lock_conn!(local_db.conn);
            conn.query_row(
                "SELECT
                    (SELECT input_token_semantics FROM proxy_request_logs WHERE request_id = 'req-1'),
                    (SELECT input_token_semantics FROM usage_daily_rollups WHERE date = '2026-03-01')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?
        };
        assert_eq!(semantics, (2, 2));

        Ok(())
    }

    #[test]
    fn memory_import_does_not_create_global_database_backup() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create temp dir");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());

        let global_db = Database::init()?;
        {
            let conn = crate::database::lock_conn!(global_db.conn);
            seed_provider(&conn, "global-provider")?;
        }

        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
        }
        let remote_sql = remote_db.export_sql_string_for_sync()?;

        let local_db = Database::memory()?;
        local_db.import_sql_string_for_sync(&remote_sql)?;

        assert!(
            !temp.path().join(".cc-switch").join("backups").exists(),
            "importing into an in-memory database must not back up the process-global database"
        );

        Ok(())
    }

    /// issue #327 回归：SQL 导入 / WebDAV 下载通过 SQLite Backup 把临时库整体写回
    /// 主库，会连数据库头一起复制。若临时库是默认的 auto_vacuum=NONE，主库就会被
    /// 重置回 NONE，令膨胀问题在每次同步后复发。修复后主库应始终保持 INCREMENTAL。
    #[test]
    fn sync_import_keeps_main_database_incremental_auto_vacuum() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create temp dir");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());

        let local_db = Database::init()?;
        {
            let conn = crate::database::lock_conn!(local_db.conn);
            assert_eq!(
                Database::get_auto_vacuum_mode(&conn)?,
                2,
                "freshly initialized db should already be INCREMENTAL"
            );
        }

        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
        }
        let remote_sql = remote_db.export_sql_string_for_sync()?;
        local_db.import_sql_string_for_sync(&remote_sql)?;

        // 写回主库后（内存连接视角）仍应为 INCREMENTAL。
        {
            let conn = crate::database::lock_conn!(local_db.conn);
            assert_eq!(
                Database::get_auto_vacuum_mode(&conn)?,
                2,
                "auto_vacuum must remain INCREMENTAL after sync import"
            );
        }

        // 以原始连接直接读磁盘（不经 Database::init 的迁移），确认已持久化。
        let db_path = crate::database::database_path()?;
        let raw = Connection::open(&db_path).expect("reopen db file");
        assert_eq!(
            Database::get_auto_vacuum_mode(&raw)?,
            2,
            "auto_vacuum must persist as INCREMENTAL on disk after import"
        );

        Ok(())
    }

    #[test]
    fn file_database_backups_use_unique_paths() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create temp dir");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());

        let db = Database::init()?;
        {
            let conn = crate::database::lock_conn!(db.conn);
            seed_provider(&conn, "local-provider")?;
        }

        let first = db
            .backup_database_file()?
            .expect("first backup should be created");
        let second = db
            .backup_database_file()?
            .expect("second backup should be created");

        assert_ne!(first, second, "backup paths should not collide");
        assert!(first.exists(), "first backup should exist");
        assert!(second.exists(), "second backup should exist");

        Ok(())
    }

    #[test]
    fn file_database_backup_retries_atomic_publication_collision() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create temp dir");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        let db = Database::init()?;
        let guard = super::lock_backup_file_operations()?;
        let conn = crate::database::lock_conn!(db.conn);
        let mut collided_path = None;

        let backup_path = db
            .backup_database_file_from_conn_with_hook(&guard, &conn, |candidate| {
                fs::write(candidate, b"existing backup sentinel")
                    .map_err(|e| AppError::io(candidate, e))?;
                collided_path = Some(candidate.to_path_buf());
                Ok(())
            })?
            .expect("file database should produce a backup");
        let collided_path = collided_path.expect("hook should record collision path");

        assert_ne!(backup_path, collided_path);
        assert_eq!(
            fs::read(&collided_path).map_err(|e| AppError::io(&collided_path, e))?,
            b"existing backup sentinel",
            "atomic publication must never overwrite an existing backup"
        );
        let backup =
            Connection::open_with_flags(&backup_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Database::validate_sqlite_integrity(&backup)?;
        Ok(())
    }

    #[test]
    fn file_database_backup_cleans_staging_file_when_publication_aborts() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create temp dir");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        let db = Database::init()?;
        let db_path = crate::database::database_path()?;
        let guard = super::lock_backup_file_operations()?;
        let conn = crate::database::lock_conn!(db.conn);

        let error = db
            .backup_database_file_from_conn_with_hook(&guard, &conn, |_| {
                Err(AppError::Database(
                    "injected publication failure".to_string(),
                ))
            })
            .expect_err("injected publication failure must abort the backup");
        assert!(error.to_string().contains("injected publication failure"));

        let backup_dir = db_path.parent().expect("database parent").join("backups");
        let artifacts = fs::read_dir(&backup_dir)
            .map_err(|e| AppError::io(&backup_dir, e))?
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        assert!(
            artifacts.is_empty(),
            "aborted publication must clean temporary backup artifacts: {artifacts:?}"
        );
        Ok(())
    }

    #[test]
    fn backup_database_path_creates_backup_beside_supplied_database() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create temp dir");
        let canonical_temp =
            std::fs::canonicalize(temp.path()).expect("canonicalize temp directory");
        let db_path = canonical_temp.join("custom.db");
        let conn = Connection::open(&db_path).expect("create source database");
        conn.execute("CREATE TABLE sample (id INTEGER PRIMARY KEY)", [])
            .expect("create source table");
        drop(conn);

        let backup_path = Database::backup_database_path(&db_path)?
            .expect("backup should be created for supplied database");
        let expected_dir = canonical_temp.join("backups");

        assert_eq!(backup_path.parent(), Some(expected_dir.as_path()));
        assert!(backup_path.exists(), "backup file should exist");

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn file_database_backup_rejects_other_user_writable_backup_dir() -> Result<(), AppError> {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("create temp dir");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        let db = Database::init()?;
        let backup_dir = crate::config::get_app_config_dir().join("backups");
        std::fs::create_dir(&backup_dir).expect("create backup dir");
        std::fs::set_permissions(&backup_dir, std::fs::Permissions::from_mode(0o777))
            .expect("set backup dir permissions");

        let err = db
            .backup_database_file()
            .expect_err("other-user-writable backup dir must be rejected");

        assert!(err.to_string().contains("不能允许组或其他用户写入"));
        assert!(
            std::fs::read_dir(&backup_dir)
                .expect("read backup dir")
                .next()
                .is_none(),
            "rejected backup must not create artifacts"
        );
        let mode = std::fs::metadata(&backup_dir)
            .expect("metadata backup dir")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o777, "validation must not chmod the directory");

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
    fn sync_import_preserves_local_proxy_state_and_clears_runtime_failover() -> Result<(), AppError>
    {
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
            (true, "10.0.0.1".to_string(), 21001, true, false, 9)
        );
        assert_eq!(
            read_proxy_row(&conn, "codex")?,
            (false, "10.0.0.2".to_string(), 21002, false, false, 8)
        );
        assert_eq!(
            read_proxy_row(&conn, "gemini")?,
            (true, "10.0.0.3".to_string(), 21003, false, false, 7)
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
    fn plain_sql_import_clears_runtime_failover_state() -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
            set_proxy_row(&conn, "claude", true, "127.0.0.1", 15721, true, true, 9)?;
        }
        let remote_sql = remote_db.export_sql_string()?;

        let local_db = Database::memory()?;
        local_db.import_sql_string(&remote_sql)?;

        let conn = crate::database::lock_conn!(local_db.conn);
        assert_eq!(
            read_proxy_row(&conn, "claude")?,
            (true, "127.0.0.1".to_string(), 15721, true, false, 9)
        );

        Ok(())
    }

    #[test]
    fn import_accepts_a_genuine_empty_export() -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        let sql = remote_db.export_sql_string()?;

        let local_db = Database::memory()?;
        local_db.import_sql_string(&sql)?;

        let conn = crate::database::lock_conn!(local_db.conn);
        assert!(Database::table_exists(&conn, "providers")?);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM providers", [], |row| row
                .get::<_, i64>(0))?,
            0
        );
        Ok(())
    }

    #[test]
    fn import_rejects_header_only_and_incomplete_exports_without_touching_live_data(
    ) -> Result<(), AppError> {
        let local_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(local_db.conn);
            seed_provider(&conn, "local-provider")?;
        }

        let header_only = "-- CC Switch SQLite 导出\nBEGIN TRANSACTION;\nCOMMIT;\n";
        let error = local_db
            .import_sql_string(header_only)
            .expect_err("header-only SQL must fail schema validation");
        assert!(error.to_string().contains("required CC Switch tables"));

        let remote_db = Database::memory()?;
        let complete = remote_db.export_sql_string()?;
        let incomplete = complete
            .strip_suffix("COMMIT;\nPRAGMA foreign_keys=ON;\n")
            .expect("known export trailer");
        let error = local_db
            .import_sql_string(incomplete)
            .expect_err("an open transaction must be treated as truncation");
        assert!(error.to_string().contains("transaction is incomplete"));

        let conn = crate::database::lock_conn!(local_db.conn);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM providers WHERE id = 'local-provider'",
                [],
                |row| row.get::<_, i64>(0),
            )?,
            1
        );
        Ok(())
    }

    #[test]
    fn import_rejects_malformed_same_name_tables_without_touching_live_data() -> Result<(), AppError>
    {
        let remote_db = Database::memory()?;
        let valid_sql = remote_db.export_sql_string()?;
        let malformed_sql = valid_sql.replacen("settings_config TEXT NOT NULL,", "", 1);
        assert_ne!(
            malformed_sql, valid_sql,
            "test must remove the target column"
        );

        let local_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(local_db.conn);
            seed_provider(&conn, "local-provider")?;
        }

        let error = local_db
            .import_sql_string(&malformed_sql)
            .expect_err("a v17 table missing a canonical column must be rejected");
        assert!(error.to_string().contains("providers.settings_config"));

        let conn = crate::database::lock_conn!(local_db.conn);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM providers WHERE id = 'local-provider'",
                [],
                |row| row.get::<_, i64>(0),
            )?,
            1,
            "schema validation failure must leave the live database untouched"
        );
        Ok(())
    }

    #[test]
    fn import_rejects_malformed_v17_dedup_contract_without_touching_live_data(
    ) -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        let valid_sql = remote_db.export_sql_string()?;
        let malformed_pk = valid_sql.replacen(
            "PRIMARY KEY (data_source, request_id)",
            "PRIMARY KEY (request_id)",
            1,
        );
        assert_ne!(
            malformed_pk, valid_sql,
            "test must alter the v17 primary key"
        );

        let local_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(local_db.conn);
            seed_provider(&conn, "local-provider")?;
        }

        let error = local_db
            .import_sql_string(&malformed_pk)
            .expect_err("a malformed v17 dedup primary key must be rejected");
        assert!(error.to_string().contains("session_usage_dedup"), "{error}");

        let malformed_index = valid_sql.replacen(
            "session_usage_dedup(data_source, semantic_id, has_entry_id)",
            "session_usage_dedup(data_source, request_id, has_entry_id)",
            1,
        );
        assert_ne!(
            malformed_index, valid_sql,
            "test must alter the v17 semantic index"
        );
        let error = local_db
            .import_sql_string(&malformed_index)
            .expect_err("a malformed v17 dedup index must be rejected");
        assert!(
            error
                .to_string()
                .contains("idx_session_usage_dedup_semantic"),
            "{error}"
        );

        let conn = crate::database::lock_conn!(local_db.conn);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM providers WHERE id = 'local-provider'",
                [],
                |row| row.get::<_, i64>(0),
            )?,
            1,
            "v17 contract failures must leave the live database untouched"
        );
        Ok(())
    }

    #[test]
    fn import_authorizer_rejects_out_of_bound_statements() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create temp dir");
        let escaped_path = temp.path().join("escaped.db");
        let escaped_dir = temp.path().join("escaped-temp");
        fs::create_dir(&escaped_dir).expect("create escaped temp directory");
        let escaped_path_sql = escaped_path.to_string_lossy().replace('\'', "''");
        let escaped_dir_sql = escaped_dir.to_string_lossy().replace('\'', "''");
        let cases = [
            (
                "attach",
                format!("ATTACH DATABASE '{escaped_path_sql}' AS escaped;"),
            ),
            ("vacuum", "VACUUM;".to_string()),
            ("vacuum into", format!("VACUUM INTO '{escaped_path_sql}';")),
            (
                "virtual table",
                "CREATE VIRTUAL TABLE escaped_fts USING fts5(content);".to_string(),
            ),
            ("writable_schema", "PRAGMA writable_schema=ON;".to_string()),
            (
                "temp_store_directory",
                format!("PRAGMA temp_store_directory='{escaped_dir_sql}';"),
            ),
        ];

        for (label, statement) in cases {
            let db = Database::memory()?;
            let sql = format!("-- CC Switch SQLite 导出\n{statement}\n");
            let error = match db.import_sql_string(&sql) {
                Ok(_) => panic!("{label} must be denied by the SQLite authorizer"),
                Err(error) => error,
            };
            assert!(
                error.to_string().to_ascii_lowercase().contains("authoriz"),
                "unexpected {label} error: {error}"
            );
        }
        assert!(
            !escaped_path.exists(),
            "denied statements must not create a file"
        );
        Ok(())
    }

    #[test]
    fn dump_sql_preserves_text_bytes_real_storage_and_autoincrement_high_water_mark(
    ) -> Result<(), AppError> {
        let source = Connection::open_in_memory()?;
        source.execute_batch(
            "CREATE TABLE fidelity (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                text_value TEXT NOT NULL,
                real_value ANY NOT NULL
             ) STRICT;
             INSERT INTO fidelity (id, text_value, real_value)
             VALUES (1, CAST(X'41004280' AS TEXT), 1.0);
             INSERT INTO fidelity (id, text_value, real_value)
             VALUES (50, 'deleted', 2.0);
             DELETE FROM fidelity WHERE id = 50;",
        )?;

        let sql = Database::dump_sql(&source, None)?;
        let target = Connection::open_in_memory()?;
        target.execute_batch(&sql)?;

        let values: (String, String) = target.query_row(
            "SELECT hex(text_value), typeof(real_value) FROM fidelity WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(values, ("41004280".to_string(), "real".to_string()));
        let sequence: i64 = target.query_row(
            "SELECT seq FROM sqlite_sequence WHERE name = 'fidelity'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(sequence, 50);
        Ok(())
    }

    #[test]
    fn sync_import_preserves_local_writes_that_arrive_after_staging() -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
        }
        let remote_sql = remote_db.export_sql_string_for_sync()?;

        let local_db = Database::memory()?;
        local_db.import_sql_string_inner_with_hook(
            &remote_sql,
            Some(&super::SYNC_PRESERVATION_POLICY),
            || {
                let conn = crate::database::lock_conn!(local_db.conn);
                conn.execute(
                    "INSERT INTO session_usage_dedup
                     (data_source, request_id, semantic_id, has_entry_id)
                     VALUES ('pi_session', 'late-request', 'late-semantic', 1)",
                    [],
                )
                .map_err(|e| AppError::Database(e.to_string()))?;
                Ok(())
            },
        )?;

        let conn = crate::database::lock_conn!(local_db.conn);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM session_usage_dedup
                 WHERE data_source = 'pi_session' AND request_id = 'late-request'",
                [],
                |row| row.get::<_, i64>(0),
            )?,
            1
        );
        Ok(())
    }

    #[test]
    fn file_sync_import_safety_backup_and_live_db_include_post_staging_write(
    ) -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create temp dir");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());

        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
        }
        let remote_sql = remote_db.export_sql_string_for_sync()?;

        let local_db = Database::init()?;
        let backup_id = local_db.import_sql_string_inner_with_hook(
            &remote_sql,
            Some(&super::SYNC_PRESERVATION_POLICY),
            || {
                let conn = crate::database::lock_conn!(local_db.conn);
                conn.execute(
                    "INSERT INTO session_usage_dedup
                     (data_source, request_id, semantic_id, has_entry_id)
                     VALUES ('pi_session', 'late-file-request', 'late-file-semantic', 1)",
                    [],
                )
                .map_err(|e| AppError::Database(e.to_string()))?;
                Ok(())
            },
        )?;
        assert!(
            !backup_id.is_empty(),
            "file import must create a safety backup"
        );

        let live_count = {
            let conn = crate::database::lock_conn!(local_db.conn);
            conn.query_row(
                "SELECT COUNT(*) FROM session_usage_dedup
                 WHERE request_id = 'late-file-request'",
                [],
                |row| row.get::<_, i64>(0),
            )?
        };
        assert_eq!(live_count, 1, "local overlay must preserve the late write");

        let backup_path = crate::database::database_path()?
            .parent()
            .expect("database parent")
            .join("backups")
            .join(format!("{backup_id}.db"));
        let backup =
            Connection::open_with_flags(&backup_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let backup_count = backup.query_row(
            "SELECT COUNT(*) FROM session_usage_dedup
             WHERE request_id = 'late-file-request'",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        assert_eq!(
            backup_count, 1,
            "the returned safety backup must be the exact pre-replacement live snapshot"
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
            (false, "127.0.0.1".to_string(), 15721, false, false, 6)
        );
        assert_eq!(
            read_proxy_row(&conn, "codex")?,
            (false, "127.0.0.1".to_string(), 15721, false, false, 5)
        );
        assert_eq!(
            read_proxy_row(&conn, "gemini")?,
            (false, "127.0.0.1".to_string(), 15721, false, false, 4)
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
