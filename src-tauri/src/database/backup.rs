//! 数据库备份和恢复
//!
//! 提供 SQL 导出/导入和二进制快照备份功能。

use super::canonical_import::CanonicalBuild;
use super::restore_policy::{should_skip_sync_export, RestoreFlavor, SYNC_LOCAL_SETTINGS_KEYS};
use super::sql_import::{SqlImportBatch, UntrustedScratch};
use super::{create_secure_dir_all, lock_conn, Database, DB_BACKUP_RETAIN};
use crate::error::AppError;
use crate::restore_protocol::{RestoreOperationId, RestoreSkillsMode};
use chrono::Utc;
use rusqlite::backup::{Backup, StepResult};
use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

const CC_SWITCH_SQL_EXPORT_HEADER: &str = "-- CC Switch SQLite 导出";

/// A fully decoded and canonicalized restore candidate. It cannot be
/// published until the coordinator arms it with a locally generated
/// operation id.
pub(crate) struct PreparedDatabaseRestore {
    build: CanonicalBuild,
    flavor: RestoreFlavor,
}

/// The type-level publication capability: only candidates carrying locally
/// generated restore metadata can cross the SQLite Backup boundary.
pub(crate) struct ArmedDatabaseRestore {
    build: CanonicalBuild,
    flavor: RestoreFlavor,
    operation: RestoreOperationId,
}

pub(crate) struct PublishedDatabaseRestore {
    pub(crate) backup_id: String,
    pub(crate) operation: RestoreOperationId,
}

impl PreparedDatabaseRestore {
    pub(crate) fn skill_directories(&self) -> Result<std::collections::BTreeSet<String>, AppError> {
        self.build.skill_directories()
    }

    pub(crate) fn arm(
        mut self,
        operation: RestoreOperationId,
        skills_mode: RestoreSkillsMode,
    ) -> Result<ArmedDatabaseRestore, AppError> {
        self.build
            .install_restore_metadata(operation, skills_mode)?;
        Ok(ArmedDatabaseRestore {
            build: self.build,
            flavor: self.flavor,
            operation,
        })
    }
}

const BACKUP_PAGES_PER_STEP: i32 = 256;
const MAX_BACKUP_TRANSIENT_RETRIES: u32 = 50;
const MAX_BACKUP_STEPS: u32 = 100_000;
const BACKUP_RETRY_DELAY: Duration = Duration::from_millis(10);

pub(crate) fn run_sqlite_backup_to_completion(backup: &Backup<'_, '_>) -> Result<(), AppError> {
    run_bounded_backup_steps(|pages| {
        backup
            .step(pages)
            .map_err(|e| AppError::Database(e.to_string()))
    })
}

fn run_bounded_backup_steps<Step>(mut step: Step) -> Result<(), AppError>
where
    Step: FnMut(i32) -> Result<StepResult, AppError>,
{
    let mut transient_retries = 0_u32;
    for _ in 0..MAX_BACKUP_STEPS {
        match step(BACKUP_PAGES_PER_STEP)? {
            StepResult::Done => return Ok(()),
            StepResult::More => transient_retries = 0,
            StepResult::Busy | StepResult::Locked => {
                if transient_retries >= MAX_BACKUP_TRANSIENT_RETRIES {
                    return Err(AppError::Database(format!(
                        "SQLite backup could not acquire a required lock before \
                         busy_timeout elapsed after \
                         {MAX_BACKUP_TRANSIENT_RETRIES} bounded retries"
                    )));
                }
                transient_retries += 1;
                std::thread::sleep(BACKUP_RETRY_DELAY);
            }
            _ => {
                return Err(AppError::Database(
                    "SQLite backup returned an unsupported step result".to_string(),
                ));
            }
        }
    }
    Err(AppError::Database(format!(
        "SQLite backup exceeded {MAX_BACKUP_STEPS} bounded steps"
    )))
}

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
    export_defaults: &'static [SyncNeutralizedColumn],
}

#[derive(Clone, Copy)]
struct SyncPreservationPolicy {
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
    export_defaults: PROXY_CONFIG_EXPORT_DEFAULTS,
}];

const SYNC_PRESERVATION_POLICY: SyncPreservationPolicy = SyncPreservationPolicy {
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
    pub(crate) fn import_sql_string(&self, sql_raw: &str) -> Result<String, AppError> {
        let prepared = Self::prepare_sql_batch(
            SqlImportBatch::from_borrowed(sql_raw)?,
            RestoreFlavor::UserRestore,
        )?;
        self.publish_armed_database_restore(
            prepared.arm(RestoreOperationId::fresh(), RestoreSkillsMode::Preserve)?,
        )
        .map(|outcome| outcome.backup_id)
    }

    pub(crate) fn prepare_sql_string_for_sync(
        sql_raw: &str,
    ) -> Result<PreparedDatabaseRestore, AppError> {
        Self::prepare_sql_batch(SqlImportBatch::from_borrowed(sql_raw)?, RestoreFlavor::Sync)
    }

    #[cfg(test)]
    pub(crate) fn import_sql_string_for_sync(&self, sql_raw: &str) -> Result<String, AppError> {
        let prepared = Self::prepare_sql_string_for_sync(sql_raw)?;
        self.publish_armed_database_restore(
            prepared.arm(RestoreOperationId::fresh(), RestoreSkillsMode::Replace)?,
        )
        .map(|outcome| outcome.backup_id)
    }

    fn prepare_sql_batch(
        batch: SqlImportBatch,
        flavor: RestoreFlavor,
    ) -> Result<PreparedDatabaseRestore, AppError> {
        Self::validate_cc_switch_sql_export(batch.sql())?;
        let scratch = UntrustedScratch::from_batch(&batch)?;
        // The SQL text is no longer needed once SQLite has materialized the
        // bounded scratch database. Release large restore buffers before the
        // canonical stage is built so peak memory is not the sum of all three.
        drop(batch);
        Self::prepare_untrusted_scratch(scratch, flavor)
    }

    fn prepare_untrusted_scratch(
        scratch: UntrustedScratch,
        flavor: RestoreFlavor,
    ) -> Result<PreparedDatabaseRestore, AppError> {
        let stage = Self::build_canonical_stage(&scratch, flavor)?;
        // CanonicalBuild owns every validated row needed for publication.
        // Closing scratch here also makes the private type boundary explicit.
        drop(scratch);
        Ok(PreparedDatabaseRestore {
            build: stage,
            flavor,
        })
    }

    pub(crate) fn publish_armed_database_restore(
        &self,
        armed: ArmedDatabaseRestore,
    ) -> Result<PublishedDatabaseRestore, AppError> {
        let publication = self.publish_canonical_stage(armed.build, armed.flavor)?;
        let discarded_runtime_rows = publication.discarded.total_runtime_rows();
        if discarded_runtime_rows > 0 {
            log::info!("restore discarded {discarded_runtime_rows} runtime-derived rows");
        }
        let backup_id = publication
            .safety_backup
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_default();
        Ok(PublishedDatabaseRestore {
            backup_id,
            operation: armed.operation,
        })
    }

    /// 从 SQL 文件导入，返回生成的备份 ID（若无备份则为空字符串）
    pub(crate) fn prepare_sql_restore(
        source_path: &Path,
    ) -> Result<PreparedDatabaseRestore, AppError> {
        Self::prepare_sql_batch(
            SqlImportBatch::read_from_path(source_path)?,
            RestoreFlavor::UserRestore,
        )
    }

    pub(crate) fn prepare_binary_restore(
        source_path: &Path,
    ) -> Result<PreparedDatabaseRestore, AppError> {
        Self::prepare_untrusted_scratch(
            UntrustedScratch::from_binary(source_path)?,
            RestoreFlavor::UserRestore,
        )
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

    /// 生成一致性快照备份，返回备份文件路径（不存在主库时返回 None）
    pub(crate) fn backup_database_file(&self) -> Result<Option<PathBuf>, AppError> {
        let conn = lock_conn!(self.conn);
        self.backup_database_file_on_locked_connection(&conn)
    }

    pub(super) fn backup_database_file_on_locked_connection(
        &self,
        conn: &Connection,
    ) -> Result<Option<PathBuf>, AppError> {
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

        let (backup_path, mut dest_conn) = Self::create_unique_backup_db_connection(&backup_dir)?;
        let backup_result = match Backup::new(conn, &mut dest_conn) {
            Ok(backup) => run_sqlite_backup_to_completion(&backup),
            Err(err) => Err(AppError::Database(err.to_string())),
        };
        drop(dest_conn);
        if let Err(err) = backup_result {
            Self::remove_incomplete_backup(&backup_path);
            return Err(err);
        }

        Self::cleanup_db_backups(&backup_dir)?;
        Ok(Some(backup_path))
    }

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

    fn create_unique_backup_db_connection(
        backup_dir: &Path,
    ) -> Result<(PathBuf, Connection), AppError> {
        for _ in 0..100 {
            let backup_path = backup_dir.join(format!("{}.db", Self::new_db_backup_id()));
            match Self::try_create_backup_db_connection(&backup_path)? {
                Some(conn) => return Ok((backup_path, conn)),
                None => continue,
            }
        }

        Err(AppError::Io {
            path: backup_dir.display().to_string(),
            source: std::io::Error::new(
                ErrorKind::AlreadyExists,
                "failed to allocate a unique database backup path",
            ),
        })
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
            if policy.is_some() && should_skip_sync_export(&table) {
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

        Ok(!policy.local_settings_keys.contains(&key))
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
}

#[cfg(test)]
mod tests {
    use super::{
        run_bounded_backup_steps, run_sqlite_backup_to_completion, Database, BACKUP_PAGES_PER_STEP,
        CC_SWITCH_SQL_EXPORT_HEADER,
    };
    use crate::error::AppError;
    use rusqlite::{
        backup::{Backup, StepResult},
        Connection,
    };
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

    fn inject_after_export_header(export: &str, statement: &str) -> String {
        let header_end = export
            .find('\n')
            .expect("CC Switch exports always contain a header line");
        format!(
            "{}\n{statement}\n{}",
            &export[..header_end],
            &export[header_end + 1..]
        )
    }

    fn remove_create_table_statement(export: &str, table: &str) -> String {
        let prefix = format!("CREATE TABLE {table} ");
        let start = export
            .find(&prefix)
            .unwrap_or_else(|| panic!("export has no {table} CREATE TABLE statement"));
        let end = export[start..]
            .find(";\n")
            .map(|offset| start + offset + 2)
            .unwrap_or_else(|| panic!("{table} CREATE TABLE statement has no terminator"));
        format!("{}{}", &export[..start], &export[end..])
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
    fn backup_uses_bounded_page_steps() {
        let mut requested_pages = Vec::new();

        run_bounded_backup_steps(|pages| {
            requested_pages.push(pages);
            Ok(StepResult::Done)
        })
        .expect("a completed bounded step should succeed");

        assert_eq!(requested_pages, vec![BACKUP_PAGES_PER_STEP]);
    }

    #[test]
    fn backup_retries_transient_busy_results_with_a_bound() {
        let mut calls = 0usize;
        run_bounded_backup_steps(|pages| {
            assert_eq!(pages, BACKUP_PAGES_PER_STEP);
            calls += 1;
            if calls == 1 {
                Ok(StepResult::Busy)
            } else {
                Ok(StepResult::Done)
            }
        })
        .expect("a transient busy result should be retried");

        assert_eq!(calls, 2);
    }

    #[test]
    fn backup_continues_after_more_pages() {
        let mut calls = 0;
        run_bounded_backup_steps(|pages| {
            assert_eq!(pages, BACKUP_PAGES_PER_STEP);
            calls += 1;
            Ok(if calls == 1 {
                StepResult::More
            } else {
                StepResult::Done
            })
        })
        .expect("a multi-step backup should complete");
        assert_eq!(calls, 2);
    }

    #[test]
    fn imported_restore_metadata_is_replaced_by_a_local_operation() -> Result<(), AppError> {
        let remote = Database::memory()?;
        {
            let connection = crate::database::lock_conn!(remote.conn);
            seed_provider(&connection, "remote-provider")?;
            for key in crate::restore_protocol::RESERVED_RESTORE_SETTING_KEYS {
                connection.execute(
                    "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, 'attacker')",
                    [key],
                )?;
            }
        }
        let export = remote.export_sql_string()?;

        let local = Database::memory()?;
        local.import_sql_string(&export)?;

        let generation = local
            .get_setting(crate::restore_protocol::RESTORE_GENERATION_KEY)?
            .expect("published candidate has a generation");
        let operation = local
            .get_setting(crate::restore_protocol::RESTORE_OPERATION_ID_KEY)?
            .expect("published candidate has an operation id");
        assert_eq!(generation, operation);
        assert_ne!(generation, "attacker");
        crate::restore_protocol::RestoreOperationId::parse(&generation)?;
        assert_eq!(
            local
                .get_setting(crate::restore_protocol::RESTORE_POSTCOMMIT_KEY)?
                .as_deref(),
            Some("pending")
        );
        assert_eq!(
            local.get_setting(crate::restore_protocol::RESTORE_INTENT_KEY)?,
            None,
            "an incoming or old-live intent must not cross publication"
        );
        Ok(())
    }

    #[test]
    fn sql_import_rejects_embedded_nul_without_replacing_main_database() -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
        }
        let mut sql = remote_db.export_sql_string()?;
        sql.push('\0');
        sql.push_str("DELETE FROM providers;");

        let local_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(local_db.conn);
            seed_provider(&conn, "local-provider")?;
        }

        let error = local_db
            .import_sql_string(&sql)
            .expect_err("an embedded NUL must not silently truncate an import");
        assert!(
            error.to_string().to_ascii_lowercase().contains("nul"),
            "the error should identify the unsupported NUL byte: {error}"
        );

        let conn = crate::database::lock_conn!(local_db.conn);
        let local_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM providers WHERE id = 'local-provider'",
            [],
            |row| row.get(0),
        )?;
        let remote_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM providers WHERE id = 'remote-provider'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(local_count, 1, "a rejected import must preserve local data");
        assert_eq!(
            remote_count, 0,
            "a rejected import must not partially replace the main database"
        );

        Ok(())
    }

    #[test]
    fn sql_import_blocks_external_file_actions() -> Result<(), AppError> {
        let source_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(source_db.conn);
            seed_provider(&conn, "remote-provider")?;
        }
        let export = source_db.export_sql_string()?;
        let temp = tempfile::tempdir().expect("create temp dir");

        for (name, statement) in [
            (
                "attached.db",
                "ATTACH DATABASE '{path}' AS imported_side_effect;",
            ),
            ("vacuumed.db", "VACUUM INTO '{path}';"),
        ] {
            let target = temp.path().join(name);
            let quoted_path = target.to_string_lossy().replace('\'', "''");
            let statement = statement.replace("{path}", &quoted_path);
            let sql = inject_after_export_header(&export, &statement);
            let local_db = Database::memory()?;

            local_db
                .import_sql_string(&sql)
                .expect_err("external file actions in imported SQL must be denied");
            assert!(
                !target.exists(),
                "a rejected import must not create {}",
                target.display()
            );
        }

        Ok(())
    }

    #[test]
    fn sql_import_rejects_unreadable_application_values_before_replacing_main_database(
    ) -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
        }
        let export = remote_db.export_sql_string()?;
        let malformed = export.replacen(
            "COMMIT;",
            "UPDATE providers SET name = X'00' WHERE id = 'remote-provider';\nCOMMIT;",
            1,
        );

        let local_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(local_db.conn);
            seed_provider(&conn, "local-provider")?;
        }

        local_db
            .import_sql_string(&malformed)
            .expect_err("values that application DAOs cannot read must be rejected");

        let conn = crate::database::lock_conn!(local_db.conn);
        let local_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM providers WHERE id = 'local-provider'",
            [],
            |row| row.get(0),
        )?;
        let remote_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM providers WHERE id = 'remote-provider'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(local_count, 1, "a rejected import must preserve local data");
        assert_eq!(
            remote_count, 0,
            "an unreadable candidate must not replace the main database"
        );

        Ok(())
    }

    #[test]
    fn sql_import_preserves_storage_extensible_provider_app_namespaces() -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            conn.execute(
                "INSERT INTO providers (id, app_type, name, settings_config, meta)
                 VALUES ('desktop-provider', 'claude-desktop', 'Desktop', '{}', '{}')",
                [],
            )?;
            conn.execute(
                "INSERT INTO provider_endpoints (provider_id, app_type, url)
                 VALUES (
                    'desktop-provider',
                    'claude-desktop',
                    'https://desktop.example'
                 )",
                [],
            )?;
        }
        let export = remote_db.export_sql_string()?;
        let local_db = Database::memory()?;

        local_db.import_sql_string(&export)?;

        let conn = crate::database::lock_conn!(local_db.conn);
        let provider_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM providers
             WHERE id = 'desktop-provider' AND app_type = 'claude-desktop'",
            [],
            |row| row.get(0),
        )?;
        let endpoint_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM provider_endpoints
             WHERE provider_id = 'desktop-provider'
               AND app_type = 'claude-desktop'
               AND url = 'https://desktop.example'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(
            (provider_count, endpoint_count),
            (1, 1),
            "restore must preserve forward-compatible string namespaces and their FK children"
        );
        Ok(())
    }

    #[test]
    fn sql_import_rebuilds_canonical_schema_without_candidate_triggers() -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
        }
        let export = remote_db.export_sql_string()?;
        let export_with_trigger = export.replacen(
            "COMMIT;",
            "CREATE TRIGGER imported_tripwire
             AFTER INSERT ON settings
             WHEN NEW.key = 'tripwire'
             BEGIN
                 DELETE FROM providers;
             END;
             COMMIT;",
            1,
        );
        let local_db = Database::memory()?;

        local_db.import_sql_string(&export_with_trigger)?;

        let conn = crate::database::lock_conn!(local_db.conn);
        let imported_trigger_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'trigger' AND name = 'imported_tripwire'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(
            imported_trigger_count, 0,
            "candidate schema objects must never be published"
        );

        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('tripwire', 'armed')",
            [],
        )?;
        let provider_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM providers", [], |row| row.get(0))?;
        assert_eq!(
            provider_count, 1,
            "future writes must not execute schema supplied by an import"
        );

        Ok(())
    }

    #[test]
    fn binary_restore_rebuilds_canonical_schema_without_candidate_triggers() -> Result<(), AppError>
    {
        let temp = tempfile::tempdir().map_err(|error| AppError::IoContext {
            context: "create binary restore fixture directory".to_string(),
            source: error,
        })?;
        let source_path = temp.path().join("candidate.db");
        let source = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(source.conn);
            seed_provider(&conn, "binary-provider")?;
            conn.execute_batch(
                "CREATE TRIGGER imported_binary_tripwire
                 AFTER INSERT ON settings
                 BEGIN
                     DELETE FROM providers;
                 END;",
            )?;
            let mut destination = Connection::open(&source_path)?;
            let backup = Backup::new(&conn, &mut destination)?;
            run_sqlite_backup_to_completion(&backup)?;
        }
        drop(source);

        let prepared = Database::prepare_binary_restore(&source_path)?;
        let target = Database::memory()?;
        let armed = prepared.arm(
            crate::restore_protocol::RestoreOperationId::fresh(),
            crate::restore_protocol::RestoreSkillsMode::Preserve,
        )?;
        target.publish_armed_database_restore(armed)?;

        let conn = crate::database::lock_conn!(target.conn);
        let trigger_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'trigger' AND name = 'imported_binary_tripwire'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(trigger_count, 0);
        let provider_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM providers WHERE id = 'binary-provider'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(provider_count, 1);
        Ok(())
    }

    #[test]
    fn canonical_publication_replaces_the_entire_live_schema() -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
        }
        let export = remote_db.export_sql_string()?;
        let local_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(local_db.conn);
            seed_provider(&conn, "local-provider")?;
            conn.execute_batch(
                "CREATE TABLE historical_weak_schema (value TEXT);
                 CREATE VIEW historical_view AS
                    SELECT value FROM historical_weak_schema;
                 CREATE INDEX historical_index
                    ON historical_weak_schema(value);
                 CREATE TRIGGER historical_trigger
                    AFTER INSERT ON historical_weak_schema
                    BEGIN
                        SELECT 1;
                    END;",
            )?;
        }

        local_db.import_sql_string(&export)?;

        let conn = crate::database::lock_conn!(local_db.conn);
        let stale_objects: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE name IN (
                'historical_weak_schema',
                'historical_view',
                'historical_index',
                'historical_trigger'
             )",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(
            stale_objects, 0,
            "publication must replace pages from CanonicalStage, not retain the live schema"
        );
        Ok(())
    }

    #[test]
    fn local_sql_restore_preserves_live_skill_metadata() -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
            conn.execute(
                "INSERT INTO skills (id, name, directory, installed_at, updated_at)
                 VALUES ('remote-skill', 'Remote Skill', 'remote-skill', 1, 1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO skill_repos (owner, name, branch, enabled)
                 VALUES ('remote', 'repo', 'main', 1)",
                [],
            )?;
        }
        let export = remote_db.export_sql_string()?;
        let local_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(local_db.conn);
            seed_provider(&conn, "local-provider")?;
            conn.execute(
                "INSERT INTO skills (id, name, directory, installed_at, updated_at)
                 VALUES ('local-skill', 'Local Skill', 'local-skill', 1, 1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO skill_repos (owner, name, branch, enabled)
                 VALUES ('local', 'repo', 'main', 1)",
                [],
            )?;
        }

        local_db.import_sql_string(&export)?;

        let conn = crate::database::lock_conn!(local_db.conn);
        let skill_ids = conn
            .prepare("SELECT id FROM skills ORDER BY id")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let repos = conn
            .prepare("SELECT owner FROM skill_repos ORDER BY owner")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(skill_ids, vec!["local-skill"]);
        assert_eq!(repos, vec!["local"]);
        Ok(())
    }

    #[test]
    fn sql_import_disables_supplied_triggers_while_the_untrusted_batch_runs() -> Result<(), AppError>
    {
        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
        }
        let export = remote_db.export_sql_string()?;
        let armed_export = export.replacen(
            "COMMIT;",
            "CREATE TRIGGER imported_execution_tripwire
             AFTER INSERT ON settings
             BEGIN
                 DELETE FROM providers;
             END;
             INSERT INTO settings (key, value) VALUES ('tripwire', 'armed');
             COMMIT;",
            1,
        );

        let local_db = Database::memory()?;
        local_db.import_sql_string(&armed_export)?;

        let conn = crate::database::lock_conn!(local_db.conn);
        let provider_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM providers WHERE id = 'remote-provider'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(
            provider_count, 1,
            "untrusted triggers must never execute while the dump is assembled"
        );
        Ok(())
    }

    #[test]
    fn sql_import_does_not_trust_a_current_user_version_stamp() -> Result<(), AppError> {
        let forged = format!(
            "{CC_SWITCH_SQL_EXPORT_HEADER}
PRAGMA foreign_keys=OFF;
PRAGMA user_version={};
BEGIN TRANSACTION;
CREATE TABLE providers (id TEXT PRIMARY KEY);
INSERT INTO providers (id) VALUES ('forged-provider');
COMMIT;
PRAGMA foreign_keys=ON;",
            crate::database::SCHEMA_VERSION
        );
        let local_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(local_db.conn);
            seed_provider(&conn, "local-provider")?;
        }

        local_db
            .import_sql_string(&forged)
            .expect_err("user_version must select migrations, not authenticate a schema");

        let conn = crate::database::lock_conn!(local_db.conn);
        let local_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM providers WHERE id = 'local-provider'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(local_count, 1, "a forged version stamp must not publish");
        Ok(())
    }

    #[test]
    fn migration_source_recognition_rejects_missing_tables_before_schema_creation(
    ) -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
        }
        let export = remote_db.export_sql_string()?;
        let missing_profiles = remove_create_table_statement(&export, "profiles");
        let local_db = Database::memory()?;

        local_db
            .import_sql_string(&missing_profiles)
            .expect_err("current-schema DDL must not fill a missing source table");
        Ok(())
    }

    #[test]
    fn migration_source_recognition_rejects_hidden_or_generated_columns() -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
        }
        let export = remote_db.export_sql_string()?;
        let original = "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);";
        let hostile = "CREATE TABLE settings (
            key TEXT PRIMARY KEY,
            value TEXT,
            concealed TEXT GENERATED ALWAYS AS ('hidden') VIRTUAL
        );";
        assert!(
            export.contains(original),
            "unexpected settings DDL in export"
        );
        let generated = export.replacen(original, hostile, 1);
        let local_db = Database::memory()?;

        local_db
            .import_sql_string(&generated)
            .expect_err("table_xinfo must reject generated source columns");
        Ok(())
    }

    #[test]
    fn sql_import_rejects_out_of_range_proxy_port_before_publish() -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
            set_proxy_row(
                &conn,
                "claude",
                false,
                "127.0.0.1",
                i64::MAX,
                false,
                false,
                3,
            )?;
        }
        let export = remote_db.export_sql_string()?;

        let local_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(local_db.conn);
            seed_provider(&conn, "local-provider")?;
        }

        local_db
            .import_sql_string(&export)
            .expect_err("a listen port outside the runtime u16 domain must be rejected");

        let conn = crate::database::lock_conn!(local_db.conn);
        let local_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM providers WHERE id = 'local-provider'",
            [],
            |row| row.get(0),
        )?;
        let remote_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM providers WHERE id = 'remote-provider'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(local_count, 1, "a rejected import must preserve local data");
        assert_eq!(remote_count, 0, "invalid data must not be published");

        Ok(())
    }

    #[test]
    fn sync_restore_decoder_rejects_unsafe_and_colliding_skill_directories() -> Result<(), AppError>
    {
        for directories in [vec!["../escape"], vec!["Résumé", "Re\u{301}sume\u{301}"]] {
            let remote_db = Database::memory()?;
            {
                let conn = crate::database::lock_conn!(remote_db.conn);
                seed_provider(&conn, "remote-provider")?;
                for (index, directory) in directories.iter().enumerate() {
                    conn.execute(
                        "INSERT INTO skills (
                            id, name, directory, installed_at, updated_at
                         ) VALUES (?1, ?2, ?3, 1, 1)",
                        rusqlite::params![
                            format!("skill-{index}"),
                            format!("Skill {index}"),
                            directory
                        ],
                    )?;
                }
            }
            let export = remote_db.export_sql_string()?;
            let local_db = Database::memory()?;
            local_db
                .import_sql_string_for_sync(&export)
                .expect_err("unsafe Skill directories must fail before publication");
        }
        Ok(())
    }

    #[test]
    fn sql_import_rejects_proxy_bind_values_outside_the_runtime_contract() -> Result<(), AppError> {
        for (listen_address, listen_port) in [("example.com", 15721), ("127.0.0.1", 1)] {
            let remote_db = Database::memory()?;
            {
                let conn = crate::database::lock_conn!(remote_db.conn);
                seed_provider(&conn, "remote-provider")?;
                set_proxy_row(
                    &conn,
                    "claude",
                    false,
                    listen_address,
                    listen_port,
                    false,
                    false,
                    3,
                )?;
            }
            let export = remote_db.export_sql_string()?;

            let local_db = Database::memory()?;
            {
                let conn = crate::database::lock_conn!(local_db.conn);
                seed_provider(&conn, "local-provider")?;
            }

            local_db.import_sql_string(&export).expect_err(
                "restore must enforce the same listen address and port contract as runtime input",
            );

            let conn = crate::database::lock_conn!(local_db.conn);
            let local_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM providers WHERE id = 'local-provider'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(local_count, 1, "a rejected import must preserve local data");
        }

        Ok(())
    }

    #[test]
    fn sql_import_rejects_sort_indices_that_cannot_be_safely_incremented() -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
            conn.execute(
                "UPDATE providers SET sort_index = ?1 WHERE id = 'remote-provider'",
                [i64::MAX],
            )?;
        }
        let export = remote_db.export_sql_string()?;

        let local_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(local_db.conn);
            seed_provider(&conn, "local-provider")?;
        }

        local_db
            .import_sql_string(&export)
            .expect_err("restore must reject sort indices that poison the next provider insert");

        let conn = crate::database::lock_conn!(local_db.conn);
        let local_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM providers WHERE id = 'local-provider'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(local_count, 1);
        Ok(())
    }

    #[test]
    fn sql_import_rekeys_surrogate_ids_before_future_inserts() -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
            conn.execute(
                "INSERT INTO provider_endpoints
                 (id, provider_id, app_type, url, added_at)
                 VALUES (?1, 'remote-provider', 'claude', 'https://remote.invalid', 1)",
                [i64::MAX],
            )?;
            conn.execute(
                "INSERT INTO stream_check_logs
                 (id, provider_id, provider_name, app_type, status, success, message,
                  response_time_ms, http_status, model_used, retry_count, tested_at)
                 VALUES (?1, 'remote-provider', 'Remote', 'claude', 'success', 1, 'ok',
                         1, 200, 'model', 0, 1)",
                [i64::MAX],
            )?;
        }
        let export = remote_db.export_sql_string()?;

        let local_db = Database::memory()?;
        local_db.import_sql_string(&export)?;

        let conn = crate::database::lock_conn!(local_db.conn);
        conn.execute(
            "INSERT INTO provider_endpoints
             (provider_id, app_type, url, added_at)
             VALUES ('remote-provider', 'claude', 'https://next.invalid', 2)",
            [],
        )
        .expect("a restored endpoint ID must not exhaust AUTOINCREMENT");
        conn.execute(
            "INSERT INTO stream_check_logs
             (provider_id, provider_name, app_type, status, success, message,
              response_time_ms, http_status, model_used, retry_count, tested_at)
             VALUES ('remote-provider', 'Remote', 'claude', 'success', 1, 'next',
                     2, 200, 'model', 0, 2)",
            [],
        )
        .expect("a restored stream-check ID must not exhaust AUTOINCREMENT");

        let max_endpoint_id: i64 =
            conn.query_row("SELECT MAX(id) FROM provider_endpoints", [], |row| {
                row.get(0)
            })?;
        let max_stream_id: i64 =
            conn.query_row("SELECT MAX(id) FROM stream_check_logs", [], |row| {
                row.get(0)
            })?;
        assert!(max_endpoint_id < i64::MAX);
        assert!(max_stream_id < i64::MAX);
        Ok(())
    }

    #[test]
    fn sql_import_rejects_foreign_key_orphans_before_publish() -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
            conn.pragma_update(None, "foreign_keys", "OFF")?;
            conn.execute(
                "INSERT INTO provider_endpoints (provider_id, app_type, url)
                 VALUES ('missing-provider', 'claude', 'https://orphan.invalid')",
                [],
            )?;
        }
        let export = remote_db.export_sql_string()?;

        let local_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(local_db.conn);
            seed_provider(&conn, "local-provider")?;
        }

        local_db
            .import_sql_string(&export)
            .expect_err("foreign-key orphans must fail closed");

        let conn = crate::database::lock_conn!(local_db.conn);
        let local_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM providers WHERE id = 'local-provider'",
            [],
            |row| row.get(0),
        )?;
        let orphan_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM provider_endpoints
             WHERE provider_id = 'missing-provider'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(local_count, 1, "a rejected import must preserve local data");
        assert_eq!(orphan_count, 0, "orphaned rows must not be published");

        Ok(())
    }

    #[test]
    fn low_level_sql_import_does_not_write_gemini_live_config() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create temp dir");
        let _environment = crate::test_support::TestEnvGuard::isolated(temp.path());
        let gemini_dir = temp.path().join(".gemini");
        fs::create_dir_all(&gemini_dir).expect("create Gemini sandbox");
        let gemini_env = gemini_dir.join(".env");
        let original_env = "GOOGLE_API_KEY=candidate-secret\nKEEP=unchanged\n";
        fs::write(&gemini_env, original_env).expect("write Gemini sandbox env");

        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
        }
        remote_db.set_config_snippet(
            "gemini",
            Some(r#"{"GOOGLE_API_KEY":"candidate-secret"}"#.to_string()),
        )?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            conn.execute(
                "DELETE FROM settings
                 WHERE key = 'gemini_common_config_credentials_scrubbed_v1'",
                [],
            )?;
        }
        let export = remote_db.export_sql_string()?;
        let local_db = Database::memory()?;

        local_db.import_sql_string(&export)?;

        assert_eq!(
            fs::read_to_string(&gemini_env).expect("read Gemini sandbox env"),
            original_env,
            "candidate hydration and low-level publication must be side-effect free"
        );

        Ok(())
    }

    #[test]
    fn sql_import_accepts_fractional_rollup_latency_read_as_numeric() -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
            conn.execute(
                "INSERT INTO usage_daily_rollups (
                    date, app_type, provider_id, model, request_count, success_count,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    input_token_semantics, total_cost_usd, avg_latency_ms
                ) VALUES (
                    '2026-08-01', 'claude', 'remote-provider', 'claude-test',
                    2, 2, 20, 10, 0, 0, 2, '0.1', 1.5
                )",
                [],
            )?;
        }
        let export = remote_db.export_sql_string()?;
        let local_db = Database::memory()?;

        local_db.import_sql_string(&export)?;

        let conn = crate::database::lock_conn!(local_db.conn);
        let latency: f64 = conn.query_row(
            "SELECT avg_latency_ms FROM usage_daily_rollups
             WHERE date = '2026-08-01' AND provider_id = 'remote-provider'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(latency, 1.5);

        Ok(())
    }

    #[test]
    fn real_v13_export_from_3c3a7f9_migrates_without_losing_cli_semantics() -> Result<(), AppError>
    {
        let temp = tempfile::tempdir().expect("create isolated v13 restore home");
        let _environment = crate::test_support::TestEnvGuard::isolated(temp.path());
        let fixture = include_str!("../../tests/fixtures/restore/v13-from-3c3a7f9.sql");
        let local_db = Database::memory()?;

        local_db.import_sql_string(fixture)?;

        let conn = crate::database::lock_conn!(local_db.conn);
        assert_eq!(
            Database::get_user_version(&conn)?,
            crate::database::SCHEMA_VERSION
        );
        for (id, app_type) in [
            ("v13-claude", "claude"),
            ("v13-hermes", "hermes"),
            ("v13-openclaw", "openclaw"),
        ] {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM providers WHERE id = ?1 AND app_type = ?2",
                rusqlite::params![id, app_type],
                |row| row.get(0),
            )?;
            assert_eq!(count, 1, "missing migrated {app_type} provider {id}");
        }

        let pricing: (String, String) = conn.query_row(
            "SELECT display_name, input_cost_per_million
             FROM model_pricing
             WHERE model_id = 'claude-3-5-haiku-20241022'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(
            pricing,
            ("V13 User Price".to_string(), "13.13".to_string()),
            "v13 model_pricing rows are user data"
        );

        let latency: (String, f64) = conn.query_row(
            "SELECT typeof(avg_latency_ms), avg_latency_ms
             FROM usage_daily_rollups
             WHERE date = '2026-07-13' AND provider_id = 'v13-claude'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(latency, ("real".to_string(), 1.5));
        Ok(())
    }

    #[test]
    fn sql_import_preserves_user_model_pricing_without_reseeding() -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        let model_id;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
            model_id = conn.query_row(
                "SELECT model_id FROM model_pricing ORDER BY model_id LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )?;
            let changed = conn.execute(
                "UPDATE model_pricing
                 SET display_name = 'User Override',
                     input_cost_per_million = '123.456',
                     output_cost_per_million = '654.321'
                 WHERE model_id = ?1",
                [&model_id],
            )?;
            assert_eq!(changed, 1, "fixture pricing row must exist");
        }
        let export = remote_db.export_sql_string()?;
        let local_db = Database::memory()?;

        local_db.import_sql_string(&export)?;

        let conn = crate::database::lock_conn!(local_db.conn);
        let restored: (String, String, String) = conn.query_row(
            "SELECT display_name, input_cost_per_million, output_cost_per_million
             FROM model_pricing WHERE model_id = ?1",
            [&model_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(
            restored,
            (
                "User Override".to_string(),
                "123.456".to_string(),
                "654.321".to_string()
            ),
            "model_pricing is portable user data, not a seed overlay"
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
    fn sync_import_rebuilds_failover_snapshots_when_the_local_parent_is_absent(
    ) -> Result<(), AppError> {
        let remote_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            seed_provider(&conn, "remote-provider")?;
        }
        let remote_sql = remote_db.export_sql_string_for_sync()?;

        let local_db = Database::memory()?;
        {
            let conn = crate::database::lock_conn!(local_db.conn);
            seed_provider(&conn, "local-only-provider")?;
            conn.execute(
                "INSERT INTO proxy_failover_live_snapshots
                    (app_type, provider_id, config_json, generated_at)
                 VALUES ('claude', 'local-only-provider', '{}', '2026-08-01T00:00:00Z')",
                [],
            )?;
        }

        local_db.import_sql_string_for_sync(&remote_sql)?;

        let conn = crate::database::lock_conn!(local_db.conn);
        let remote_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM providers WHERE id = 'remote-provider'",
            [],
            |row| row.get(0),
        )?;
        let snapshot_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM proxy_failover_live_snapshots",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(remote_count, 1);
        assert_eq!(
            snapshot_count, 0,
            "failover live snapshots are runtime-derived and must be rebuilt"
        );
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
