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

use crate::config::get_app_config_dir;
use crate::error::AppError;
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// DAO 方法通过 impl Database 提供，无需额外导出

/// 数据库备份保留数量
const DB_BACKUP_RETAIN: usize = 10;
const USAGE_ROLLUP_RETAIN_DAYS: i64 = 30;
const USAGE_MAINTENANCE_INTERVAL_SECS: u64 = 24 * 60 * 60;

/// 当前 Schema 版本号
/// 每次修改表结构时递增，并在 schema.rs 中添加相应的迁移逻辑
pub(crate) const SCHEMA_VERSION: i32 = 11;

/// 安全地序列化 JSON，避免 unwrap panic
pub(crate) fn to_json_string<T: Serialize>(value: &T) -> Result<String, AppError> {
    serde_json::to_string(value)
        .map_err(|e| AppError::Config(format!("JSON serialization failed: {e}")))
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
        let db_path = get_app_config_dir().join("cc-switch.db");

        // 确保父目录存在
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }

        let conn = Connection::open(&db_path).map_err(|e| AppError::Database(e.to_string()))?;

        Self::configure_connection(&conn)?;
        // 多进程并发：daemon 与 worker 都会打开这个文件，WAL + busy_timeout 让
        // 短暂的 SQLITE_BUSY 自动重试而不是直接失败。
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| AppError::Database(e.to_string()))?;

        let db = Self {
            conn: Mutex::new(conn),
            runtime_key: format!("file:{}", db_path.display()),
        };

        {
            let conn = lock_conn!(db.conn);
            let version = Self::get_user_version(&conn)?;
            drop(conn);

            if version > SCHEMA_VERSION {
                // 上游 cc-switch 可能已升级 DB 版本（如 v12）。若核心 schema 兼容则允许继续运行。
                if version == 12 {
                    log::warn!(
                        "数据库版本 {version} 高于当前支持的最高版本 {SCHEMA_VERSION}，将尝试以兼容模式运行"
                    );
                } else {
                    return Err(Self::future_schema_error(version));
                }
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
        let db_path = get_app_config_dir().join("cc-switch.db");
        if !db_path.exists() {
            return Err(AppError::Database(format!(
                "database is not initialized: {}",
                db_path.display()
            )));
        }

        let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| AppError::Database(e.to_string()))?;
        Self::configure_connection(&conn)?;

        let version = Self::get_user_version(&conn)?;
        if version > SCHEMA_VERSION {
            if version == 12 {
                log::warn!(
                    "数据库版本 {version} 高于当前支持的最高版本 {SCHEMA_VERSION}，将尝试以兼容模式运行（只读）"
                );
            } else {
                return Err(Self::future_schema_error(version));
            }
        }
        if version != SCHEMA_VERSION && version != 12 {
            return Err(AppError::Database(format!(
                "database schema version {version} requires initialization before snapshot reads; current schema version is {SCHEMA_VERSION}"
            )));
        }

        Ok(Self {
            conn: Mutex::new(conn),
            runtime_key: format!("file:{}", db_path.display()),
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
