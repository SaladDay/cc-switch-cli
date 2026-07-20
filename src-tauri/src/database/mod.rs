//! 数据库模块 - SQLite 数据持久化
//!
//! 此模块提供应用的核心数据存储功能，包括：
//! - 供应商配置管理
//! - MCP 服务器配置
//! - 提示词管理
//! - Skills 管理
//! - 通用设置存储
//!
//! ## 架构设计
//!
//! ```text
//! database/
//! ├── mod.rs        - Database 结构体 + 初始化
//! ├── schema.rs     - 表结构定义 + Schema 迁移
//! ├── backup.rs     - SQL 导入导出 + 快照备份
//! ├── migration.rs  - JSON → SQLite 数据迁移
//! └── dao/          - 数据访问对象
//!     ├── providers.rs
//!     ├── mcp.rs
//!     ├── prompts.rs
//!     ├── skills.rs
//!     └── settings.rs
//! ```

mod backup;
mod dao;
mod migration;
mod schema;

#[cfg(test)]
mod tests;

// DAO 类型导出供外部使用
pub(crate) use backup::run_sqlite_backup_to_completion;
pub(crate) use dao::model_pricing::ModelPricingUpdate;
pub(crate) use dao::providers_seed::is_official_seed_id;
pub use dao::FailoverQueueItem;

use crate::config::{
    get_app_config_dir, resolve_config_dir_without_following_user_symlinks,
    resolve_existing_or_new_child_path,
};
use crate::error::AppError;
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

// DAO 方法通过 impl Database 提供，无需额外导出

/// 数据库备份保留数量
const DB_BACKUP_RETAIN: usize = 10;
const USAGE_ROLLUP_RETAIN_DAYS: i64 = 30;
const USAGE_MAINTENANCE_INTERVAL_SECS: u64 = 24 * 60 * 60;

static DATABASE_PERMISSION_CHECK: Once = Once::new();

/// 当前 Schema 版本号
/// 每次修改表结构时递增，并在 schema.rs 中添加相应的迁移逻辑
///
/// 注意：本库 schema 与上游项目同步（WebDAV 亦会整库同步），本仓库不得自行
/// 加表/加列或提升版本号；本地新增的持久化需求一律放独立 sidecar 存储
/// （如 session_manager::scan_cache_store）。
pub(crate) const SCHEMA_VERSION: i32 = 13;

fn database_open_flags() -> OpenFlags {
    OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW
}

fn readonly_database_open_flags() -> OpenFlags {
    OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW
}

pub(crate) fn database_path() -> Result<PathBuf, AppError> {
    Ok(
        resolve_config_dir_without_following_user_symlinks(&get_app_config_dir())?
            .join("cc-switch.db"),
    )
}

#[cfg(unix)]
fn reject_hardlinked_database_file(path: &Path, meta: &std::fs::Metadata) -> Result<(), AppError> {
    use std::os::unix::fs::MetadataExt;

    if meta.nlink() > 1 {
        return Err(AppError::InvalidInput(format!(
            "数据库文件不能有多个硬链接: {}",
            path.display()
        )));
    }

    Ok(())
}

#[cfg(unix)]
fn validate_existing_database_file(path: &Path) -> Result<(), AppError> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| AppError::io(path, e))?;
    if meta.file_type().is_symlink() {
        return Err(AppError::InvalidInput(format!(
            "数据库文件不能是符号链接: {}",
            path.display()
        )));
    }
    if !meta.is_file() {
        return Err(AppError::InvalidInput(format!(
            "数据库路径不是普通文件: {}",
            path.display()
        )));
    }

    reject_hardlinked_database_file(path, &meta)
}

#[cfg(unix)]
fn validate_existing_database_init_lock(path: &Path) -> Result<(), AppError> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| AppError::io(path, e))?;
    if meta.file_type().is_symlink() {
        return Err(AppError::InvalidInput(format!(
            "数据库初始化锁不能是符号链接: {}",
            path.display()
        )));
    }
    if !meta.is_file() {
        return Err(AppError::InvalidInput(format!(
            "数据库初始化锁不是普通文件: {}",
            path.display()
        )));
    }

    reject_hardlinked_database_file(path, &meta)
}

#[cfg(unix)]
struct DatabaseInitLock {
    _file: std::fs::File,
}

#[cfg(unix)]
fn acquire_database_init_lock(config_dir: &Path) -> Result<DatabaseInitLock, AppError> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::os::unix::io::AsRawFd;

    let path = config_dir.join("cc-switch.db.init.lock");
    match std::fs::symlink_metadata(&path) {
        Ok(_) => validate_existing_database_init_lock(&path)?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(AppError::io(&path, err)),
    }

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|e| AppError::io(&path, e))?;

    let meta = file.metadata().map_err(|e| AppError::io(&path, e))?;
    if !meta.is_file() {
        return Err(AppError::InvalidInput(format!(
            "数据库初始化锁不是普通文件: {}",
            path.display()
        )));
    }
    reject_hardlinked_database_file(&path, &meta)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|e| AppError::io(&path, e))?;

    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if rc != 0 {
        return Err(AppError::io(&path, std::io::Error::last_os_error()));
    }

    Ok(DatabaseInitLock { _file: file })
}

/// 安全地序列化 JSON，避免 unwrap panic
pub(crate) fn to_json_string<T: Serialize>(value: &T) -> Result<String, AppError> {
    serde_json::to_string(value)
        .map_err(|e| AppError::Config(format!("JSON serialization failed: {e}")))
}

// Create folders with 0o700 permissions.
// Leave existing folders untouched. We fix permissions elsewhere, so this helper
// must not chmod arbitrary existing parents or follow symlinked config paths.
pub(crate) fn create_secure_dir_all(path: &Path) -> Result<bool, AppError> {
    let path = resolve_create_dir_path(path)?;

    #[cfg(unix)]
    {
        create_secure_dir_all_no_symlink(&path)
    }

    #[cfg(not(unix))]
    {
        match std::fs::create_dir_all(&path) {
            Ok(()) => Ok(true),
            Err(err) => Err(AppError::io(&path, err)),
        }
    }
}

fn resolve_create_dir_path(path: &Path) -> Result<PathBuf, AppError> {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        resolve_existing_or_new_child_path(path)?;
        return normalize_path_lexically(path);
    }

    Ok(path.to_path_buf())
}

fn normalize_path_lexically(path: &Path) -> Result<PathBuf, AppError> {
    let base = if path.is_absolute() {
        PathBuf::new()
    } else {
        std::env::current_dir().map_err(|e| AppError::io(".", e))?
    };

    let mut normalized = PathBuf::new();
    for component in base.join(path).components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(AppError::InvalidInput(format!(
                        "路径包含无效的父目录组件: {}",
                        path.display()
                    )));
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }

    Ok(normalized)
}

#[cfg(unix)]
fn create_secure_dir_all_no_symlink(path: &Path) -> Result<bool, AppError> {
    use std::os::unix::fs::DirBuilderExt;

    let mut current = PathBuf::new();
    let mut created_any = false;

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => continue,
            Component::ParentDir => unreachable!("parent components are rejected before creation"),
            Component::Normal(part) => {
                current.push(part);
                match std::fs::symlink_metadata(&current) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        if let Some(resolved) = allowed_platform_symlink_component(&current)? {
                            current = resolved;
                            continue;
                        }
                        return Err(AppError::InvalidInput(format!(
                            "配置目录路径不能包含符号链接: {}",
                            current.display()
                        )));
                    }
                    Ok(meta) if meta.is_dir() => {}
                    Ok(_) => {
                        return Err(AppError::InvalidInput(format!(
                            "配置目录路径组件不是目录: {}",
                            current.display()
                        )));
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                        match std::fs::DirBuilder::new().mode(0o700).create(&current) {
                            Ok(()) => created_any = true,
                            Err(create_err)
                                if create_err.kind() == std::io::ErrorKind::AlreadyExists =>
                            {
                                ensure_existing_secure_dir_component(&current)?;
                            }
                            Err(create_err) => return Err(AppError::io(&current, create_err)),
                        }
                    }
                    Err(err) => return Err(AppError::io(&current, err)),
                }
            }
        }
    }

    Ok(created_any)
}

#[cfg(unix)]
fn ensure_existing_secure_dir_component(path: &Path) -> Result<(), AppError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(AppError::InvalidInput(format!(
            "配置目录路径不能包含符号链接: {}",
            path.display()
        ))),
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err(AppError::InvalidInput(format!(
            "配置目录路径组件不是目录: {}",
            path.display()
        ))),
        Err(err) => Err(AppError::io(path, err)),
    }
}

#[cfg(unix)]
fn allowed_platform_symlink_component(path: &Path) -> Result<Option<PathBuf>, AppError> {
    #[cfg(target_os = "macos")]
    {
        if matches!(path.to_str(), Some("/tmp") | Some("/var") | Some("/etc")) {
            let resolved = path.canonicalize().map_err(|e| AppError::io(path, e))?;
            let meta = std::fs::metadata(&resolved).map_err(|e| AppError::io(&resolved, e))?;
            if meta.is_dir() {
                return Ok(Some(resolved));
            }
        }
    }

    let _ = path;
    Ok(None)
}

/// 安全地获取 Mutex 锁，避免 unwrap panic
macro_rules! lock_conn {
    ($mutex:expr) => {
        $mutex
            .lock()
            .map_err(|e| AppError::Database(format!("Mutex lock failed: {}", e)))?
    };
}

// 导出宏供子模块使用
pub(crate) use lock_conn;

/// 数据库连接封装
///
/// 使用 Mutex 包装 Connection 以支持在多线程环境（如 Tauri State）中共享。
/// rusqlite::Connection 本身不是 Sync 的，因此需要这层包装。
pub struct Database {
    pub(crate) conn: Mutex<Connection>,
    runtime_key: String,
    db_path: Option<PathBuf>,
}

impl Database {
    fn configure_connection(conn: &Connection) -> Result<(), AppError> {
        conn.execute("PRAGMA foreign_keys = ON;", [])
            .map_err(|e| AppError::Database(e.to_string()))?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 读取当前连接的 `auto_vacuum` 模式（0=NONE, 1=FULL, 2=INCREMENTAL）。
    pub(crate) fn get_auto_vacuum_mode(conn: &Connection) -> Result<i32, AppError> {
        conn.query_row("PRAGMA auto_vacuum;", [], |row| row.get(0))
            .map_err(|e| AppError::Database(format!("读取 auto_vacuum 失败: {e}")))
    }

    /// 判断库中是否已存在用户表（用于区分全新库与存量库）。
    fn has_user_tables(conn: &Connection) -> Result<bool, AppError> {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Database(format!("读取表数量失败: {e}")))?;
        Ok(count > 0)
    }

    /// 在给定连接上确保 `auto_vacuum = INCREMENTAL`。
    ///
    /// 若已是 INCREMENTAL 则直接返回 `Ok(false)`。对已有用户表的存量库，
    /// 切换 `auto_vacuum` 需要整库 `VACUUM` 重建，重建后会一并回收此前累积的
    /// 空闲页（例如被 `rollup_and_prune` 删除但从未归还操作系统的 `proxy_request_logs`
    /// 页），并使后续的 `PRAGMA incremental_vacuum` 真正生效。返回是否发生了重建。
    pub(crate) fn ensure_incremental_auto_vacuum_on_conn(
        conn: &Connection,
    ) -> Result<bool, AppError> {
        let mode = Self::get_auto_vacuum_mode(conn)?;
        if mode == 2 {
            return Ok(false);
        }

        let has_tables = Self::has_user_tables(conn)?;
        conn.execute("PRAGMA auto_vacuum = INCREMENTAL;", [])
            .map_err(|e| AppError::Database(format!("设置 auto_vacuum 失败: {e}")))?;

        // 全新库（尚无用户表）设置 pragma 即可生效，无需 VACUUM。
        if !has_tables {
            return Ok(false);
        }

        conn.execute("VACUUM;", [])
            .map_err(|e| AppError::Database(format!("执行 VACUUM 失败: {e}")))?;
        conn.execute("PRAGMA foreign_keys = ON;", [])
            .map_err(|e| AppError::Database(format!("恢复 foreign_keys 失败: {e}")))?;
        Ok(true)
    }

    /// 确保本库启用增量 auto-vacuum；存量库首次迁移前会先做一次全量备份。
    pub(crate) fn ensure_incremental_auto_vacuum(&self) -> Result<bool, AppError> {
        let mode = {
            let conn = lock_conn!(self.conn);
            Self::get_auto_vacuum_mode(&conn)?
        };
        if mode == 2 {
            return Ok(false);
        }

        let has_tables = {
            let conn = lock_conn!(self.conn);
            Self::has_user_tables(&conn)?
        };
        if has_tables {
            log::info!(
                "Detected auto_vacuum={mode}, rebuilding database to enable incremental vacuum"
            );
            self.backup_database_file()?;
        }

        let rebuilt = {
            let conn = lock_conn!(self.conn);
            Self::ensure_incremental_auto_vacuum_on_conn(&conn)?
        };

        if rebuilt {
            log::info!("Incremental auto-vacuum enabled after database rebuild");
        } else {
            log::info!("Incremental auto-vacuum configured for new database");
        }

        Ok(rebuilt)
    }

    /// 初始化数据库连接并创建表
    ///
    /// 数据库文件位于 `~/.cc-switch/cc-switch.db`
    pub fn init() -> Result<Self, AppError> {
        if let Err(err) = crate::config::validate_config_dir() {
            log::warn!("拒绝初始化数据库：配置目录校验失败: {err}");
            return Err(err);
        }
        warn_insecure_permissions_once();

        let db_path = database_path()?;

        // 确保父目录存在
        if let Some(parent) = db_path.parent() {
            create_secure_dir_all(parent)?;
        }

        #[cfg(unix)]
        let _init_lock = db_path
            .parent()
            .map(acquire_database_init_lock)
            .transpose()?;

        // 新建数据库文件时以 0o600 原子创建，已有文件的权限由 prompt_fix_permissions 处理
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            match std::fs::symlink_metadata(&db_path) {
                Ok(_) => validate_existing_database_file(&db_path)?,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    match std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&db_path)
                    {
                        Ok(_) => {}
                        Err(create_err)
                            if create_err.kind() == std::io::ErrorKind::AlreadyExists =>
                        {
                            validate_existing_database_file(&db_path)?;
                        }
                        Err(create_err) => return Err(AppError::io(&db_path, create_err)),
                    }
                }
                Err(err) => return Err(AppError::io(&db_path, err)),
            }
        }
        #[cfg(not(unix))]
        {
            if !db_path.exists() {
                std::fs::File::create(&db_path).map_err(|e| AppError::io(&db_path, e))?;
            }
        }

        let conn = Connection::open_with_flags(&db_path, database_open_flags())
            .map_err(|e| AppError::Database(e.to_string()))?;

        Self::configure_connection(&conn)?;

        // 全新库：在建表、且在切到 WAL 之前启用增量 auto-vacuum。
        // 顺序很重要——`journal_mode=WAL` 会写入 page 1，之后再设 `auto_vacuum`
        // 对空库将静默失效（模式仍为 NONE，需整库 VACUUM 才能切换）。
        // unix 下文件已被预创建为空，因此以「是否已存在用户表」判断是否为全新库。
        if !Self::has_user_tables(&conn)? {
            conn.execute("PRAGMA auto_vacuum = INCREMENTAL;", [])
                .map_err(|e| AppError::Database(e.to_string()))?;
        }

        // 多进程并发：daemon 与 worker 都会打开这个文件，WAL + busy_timeout 让
        // 短暂的 SQLITE_BUSY 自动重试而不是直接失败。
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| AppError::Database(e.to_string()))?;

        // synchronous 保持 SQLite 默认（FULL）：本库除可重建的 usage 行外还存
        // provider/settings 等权威配置，不应全局降低耐久性。批量导入期间由
        // `bulk_import_durability_guard()` 在本连接上临时降为 NORMAL 并在结束后恢复。

        let db = Self {
            conn: Mutex::new(conn),
            runtime_key: format!("file:{}", db_path.display()),
            db_path: Some(db_path.clone()),
        };

        {
            let conn = lock_conn!(db.conn);
            let version = Self::get_user_version(&conn)?;
            drop(conn);

            if version > SCHEMA_VERSION {
                return Err(Self::future_schema_error(version));
            }

            if version > 0 && version < SCHEMA_VERSION {
                log::info!(
                    "Creating pre-migration database backup (v{version} -> v{SCHEMA_VERSION})"
                );
                db.backup_database_file().map_err(|err| {
                    AppError::Database(format!(
                        "Pre-migration backup failed; database migration was not started: {err}"
                    ))
                })?;
            }
        }

        db.create_tables()?;
        db.apply_schema_migrations()?;
        // 存量库若仍是 auto_vacuum=NONE（老版本从未启用增量回收），在此切换到
        // INCREMENTAL 并整库 VACUUM 一次，回收历史累积的空闲页（issue #327：
        // proxy_request_logs 等本地表被 prune 删除后文件从不收缩，导致 WebDAV
        // 下载/上传对超大库反复全量拷贝而卡死）。失败不致命，仅记录告警。
        if let Err(err) = db.ensure_incremental_auto_vacuum() {
            log::warn!("Failed to ensure incremental auto-vacuum: {err}");
        }
        db.ensure_model_pricing_seeded()?;
        db.run_usage_maintenance("startup");

        Ok(db)
    }

    /// 打开当前 schema 的只读快照连接。
    ///
    /// 用于 TUI 后台热刷新等只读路径；不会创建目录、建表、迁移、seed 或执行启动维护。
    pub fn open_readonly_current_schema() -> Result<Self, AppError> {
        let db_path = database_path()?;
        if !db_path.exists() {
            return Err(AppError::Database(format!(
                "database is not initialized: {}",
                db_path.display()
            )));
        }
        #[cfg(unix)]
        validate_existing_database_file(&db_path)?;

        let conn = Connection::open_with_flags(&db_path, readonly_database_open_flags())
            .map_err(|e| AppError::Database(e.to_string()))?;
        Self::configure_connection(&conn)?;

        let version = Self::get_user_version(&conn)?;
        if version > SCHEMA_VERSION {
            return Err(Self::future_schema_error(version));
        }
        if version != SCHEMA_VERSION {
            return Err(AppError::Database(format!(
                "database schema version {version} requires initialization before snapshot reads; current schema version is {SCHEMA_VERSION}"
            )));
        }

        Ok(Self {
            conn: Mutex::new(conn),
            runtime_key: format!("file:{}", db_path.display()),
            db_path: Some(db_path),
        })
    }

    /// 创建内存数据库（用于测试）
    pub fn memory() -> Result<Self, AppError> {
        static NEXT_MEMORY_DB_ID: AtomicU64 = AtomicU64::new(1);

        let conn = Connection::open_in_memory().map_err(|e| AppError::Database(e.to_string()))?;

        Self::configure_connection(&conn)?;
        // 与文件库保持一致：建表前启用增量 auto-vacuum。
        conn.execute("PRAGMA auto_vacuum = INCREMENTAL;", [])
            .map_err(|e| AppError::Database(e.to_string()))?;

        let db = Self {
            conn: Mutex::new(conn),
            runtime_key: format!(
                "memory:{}",
                NEXT_MEMORY_DB_ID.fetch_add(1, Ordering::Relaxed)
            ),
            db_path: None,
        };
        db.create_tables()?;
        db.ensure_model_pricing_seeded()?;

        Ok(db)
    }

    /// 检查 MCP 服务器表是否为空
    pub fn is_mcp_table_empty(&self) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM mcp_servers", [], |row| row.get(0))
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(count == 0)
    }

    /// 检查提示词表是否为空
    pub fn is_prompts_table_empty(&self) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM prompts", [], |row| row.get(0))
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(count == 0)
    }

    pub(crate) fn runtime_key(&self) -> &str {
        &self.runtime_key
    }

    pub(crate) fn spawn_periodic_usage_maintenance(
        db: Arc<Self>,
        context: &'static str,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_secs(USAGE_MAINTENANCE_INTERVAL_SECS));
            interval.tick().await;

            loop {
                interval.tick().await;
                let db = db.clone();
                let task_context = context.to_string();
                let log_context = task_context.clone();
                match tokio::task::spawn_blocking(move || {
                    db.run_usage_maintenance(&task_context);
                })
                .await
                {
                    Ok(()) => {}
                    Err(error) => {
                        log::warn!(
                            "Periodic usage maintenance task failed ({log_context}): {error}"
                        )
                    }
                }
            }
        })
    }

    fn run_usage_maintenance(&self, context: &str) {
        match self.backfill_missing_usage_costs() {
            Ok(updated) if updated > 0 => {
                log::info!("Usage maintenance backfilled costs ({context}): updated={updated}");
            }
            Ok(_) => {}
            Err(error) => {
                log::warn!("Usage maintenance cost backfill failed ({context}): {error}");
                return;
            }
        }

        match self.rollup_and_prune(USAGE_ROLLUP_RETAIN_DAYS) {
            Ok(deleted) if deleted > 0 => match self.conn.lock() {
                Ok(conn) => {
                    if let Err(error) = conn.execute_batch("PRAGMA incremental_vacuum;") {
                        log::warn!(
                            "Usage maintenance incremental vacuum failed ({context}): {error}"
                        );
                    }
                }
                Err(error) => {
                    log::warn!(
                        "Usage maintenance incremental vacuum lock failed ({context}): {error}"
                    )
                }
            },
            Ok(_) => {}
            Err(error) => {
                log::warn!("Usage maintenance rollup_and_prune failed ({context}): {error}")
            }
        }
    }
}

fn warn_insecure_permissions_once() {
    DATABASE_PERMISSION_CHECK.call_once(|| {
        let issues = crate::config::check_permissions();
        if issues.is_empty() {
            return;
        }

        log::warn!("检测到不安全的 cc-switch 配置权限，请收紧后再继续使用");
        for (path, current, expected) in issues {
            log::warn!(
                "不安全权限: path={} current={:03o} expected={:03o}",
                path.display(),
                current,
                expected
            );
        }
    });
}

/// 批量导入耐久性守卫：持有期间本连接 `synchronous=NORMAL`（WAL 下 COMMIT
/// 不再逐次 fsync，HDD/macOS 上是首次导入的主要开销），Drop 时恢复**进入守卫
/// 之前**读到的 synchronous 值（数值 0=OFF/1=NORMAL/2=FULL/3=EXTRA），而非硬
/// 编码 FULL——即便调用方或未来的连接初始化改用了非 FULL 的默认值，恢复也忠实
/// 还原原值。进入时读 pragma 失败则退回旧行为（Drop 恢复 FULL）并记 debug 日志。
///
/// # 独占连接契约
///
/// 本守卫降级的是**整条连接**的 synchronous，因此调用方必须持有一条仅用于本次
/// 导入的**独占连接**；绝不能是与 proxy 日志、failover 等权威写入共享的连接
/// （否则那些权威写入会在窗口内一并被降级到 NORMAL）。现有三个调用方都满足：
///
/// - TUI 后台同步 worker：`Database::init()` 新开的专用连接
///   （`cli/tui/runtime_systems/workers.rs` 的 `session_usage_sync_worker_loop`）。
/// - CLI `sessions sync` 命令：命令独占的连接
///   （`cli/commands/sessions.rs` 的 `sync_usage_for_provider`）。
/// - 周期同步：`reopen_for_import()` 重开的、指向同一文件的独立导入连接
///   （`services/session_usage.rs` 的阻塞线程路径）。
///
/// 只应包裹**可从源文件重建的数据**的批量写入（会话用量导入），窗口应尽量短。
pub(crate) struct BulkImportDurabilityGuard<'a> {
    db: &'a Database,
    /// 进入守卫前读到的 `synchronous` 数值（0=OFF/1=NORMAL/2=FULL/3=EXTRA）。
    /// Drop 用它以数值形式恢复；读取失败为 `None`，Drop 回退恢复 FULL。
    restore: Option<i64>,
}

impl Drop for BulkImportDurabilityGuard<'_> {
    fn drop(&mut self) {
        if let Ok(conn) = self.db.conn.lock() {
            match self.restore {
                Some(value) => match conn.pragma_update(None, "synchronous", value) {
                    Ok(()) => log::debug!("[BULK-IMPORT] 本连接恢复 synchronous={value}"),
                    Err(e) => log::warn!("[BULK-IMPORT] 恢复 synchronous={value} 失败: {e}"),
                },
                None => match conn.pragma_update(None, "synchronous", "FULL") {
                    Ok(()) => {
                        log::debug!(
                            "[BULK-IMPORT] 本连接恢复 synchronous=FULL（读取原值失败的回退）"
                        )
                    }
                    Err(e) => log::warn!("[BULK-IMPORT] 恢复 synchronous=FULL 失败: {e}"),
                },
            }
        }
    }
}

impl Database {
    /// 见 [`BulkImportDurabilityGuard`]。设置失败只降速不影响正确性。
    pub(crate) fn bulk_import_durability_guard(&self) -> BulkImportDurabilityGuard<'_> {
        let mut restore = None;
        if let Ok(conn) = self.conn.lock() {
            // 先记下进入守卫前的实际 synchronous 值，供 Drop 忠实恢复。
            restore = match conn.query_row("PRAGMA synchronous", [], |row| row.get::<_, i64>(0)) {
                Ok(value) => Some(value),
                Err(e) => {
                    log::debug!("[BULK-IMPORT] 读取当前 synchronous 失败，Drop 将回退 FULL: {e}");
                    None
                }
            };
            match conn.pragma_update(None, "synchronous", "NORMAL") {
                Ok(()) => log::debug!("[BULK-IMPORT] 本连接 synchronous=NORMAL（导入期间）"),
                Err(e) => log::debug!("[BULK-IMPORT] 设置 synchronous=NORMAL 失败: {e}"),
            }
        }
        BulkImportDurabilityGuard { db: self, restore }
    }

    /// 为批量导入重开一个指向同一数据库文件的独立连接。
    ///
    /// 周期同步运行在与 daemon/proxy 共享 `Database` 的进程里：导入走独立
    /// 连接，耐久性守卫就只降级导入侧（共享连接上的 proxy 日志、failover
    /// 等权威写入保持 FULL），批量事务也不会经进程内 mutex 阻塞共享连接。
    /// 内存库（测试）没有文件路径，返回 Err，调用方回退共享连接。
    pub(crate) fn reopen_for_import(&self) -> Result<Self, AppError> {
        let Some(db_path) = self.db_path.clone() else {
            return Err(AppError::Database(
                "内存数据库无法重开独立导入连接".to_string(),
            ));
        };
        let conn = Connection::open_with_flags(&db_path, database_open_flags())
            .map_err(|e| AppError::Database(format!("重开数据库连接失败: {e}")))?;
        Self::configure_connection(&conn)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(Self {
            conn: Mutex::new(conn),
            runtime_key: self.runtime_key.clone(),
            db_path: Some(db_path),
        })
    }
}
