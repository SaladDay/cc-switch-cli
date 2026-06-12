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
pub(crate) const SCHEMA_VERSION: i32 = 11;

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
        // 多进程并发：daemon 与 worker 都会打开这个文件，WAL + busy_timeout 让
        // 短暂的 SQLITE_BUSY 自动重试而不是直接失败。
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| AppError::Database(e.to_string()))?;

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
                if let Err(err) = db.backup_database_file() {
                    log::warn!("Pre-migration backup failed, continuing migration: {err}");
                }
            }
        }

        db.create_tables()?;
        db.apply_schema_migrations()?;
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
