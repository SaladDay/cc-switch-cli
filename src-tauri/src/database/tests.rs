//! 数据库模块测试
//!
//! 包含 Schema 迁移和基本功能的测试。

use super::*;
use crate::app_config::MultiAppConfig;
use crate::prompt::Prompt;
use crate::provider::{Provider, ProviderManager};
use indexmap::IndexMap;
use rusqlite::{params, Connection};
use serde_json::json;
use std::{collections::HashMap, ffi::OsString, path::Path};

struct ConfigDirEnvGuard {
    original: Option<OsString>,
}

impl ConfigDirEnvGuard {
    fn set(path: &Path) -> Self {
        let original = std::env::var_os("CC_SWITCH_CONFIG_DIR");
        unsafe {
            std::env::set_var("CC_SWITCH_CONFIG_DIR", path);
        }
        Self { original }
    }
}

impl Drop for ConfigDirEnvGuard {
    fn drop(&mut self) {
        match self.original.as_ref() {
            Some(value) => unsafe { std::env::set_var("CC_SWITCH_CONFIG_DIR", value) },
            None => unsafe { std::env::remove_var("CC_SWITCH_CONFIG_DIR") },
        }
    }
}

const LEGACY_SCHEMA_SQL: &str = r#"
    CREATE TABLE providers (
        id TEXT NOT NULL,
        app_type TEXT NOT NULL,
        name TEXT NOT NULL,
        settings_config TEXT NOT NULL,
        PRIMARY KEY (id, app_type)
    );
    CREATE TABLE provider_endpoints (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        provider_id TEXT NOT NULL,
        app_type TEXT NOT NULL,
        url TEXT NOT NULL
    );
    CREATE TABLE mcp_servers (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        server_config TEXT NOT NULL
    );
    CREATE TABLE prompts (
        id TEXT NOT NULL,
        app_type TEXT NOT NULL,
        name TEXT NOT NULL,
        content TEXT NOT NULL,
        PRIMARY KEY (id, app_type)
    );
    CREATE TABLE skills (
        key TEXT PRIMARY KEY,
        installed BOOLEAN NOT NULL DEFAULT 0
    );
    CREATE TABLE skill_repos (
        owner TEXT NOT NULL,
        name TEXT NOT NULL,
        PRIMARY KEY (owner, name)
    );
    CREATE TABLE settings (
        key TEXT PRIMARY KEY,
        value TEXT
    );
"#;

// v3.8.x（schema v1）的真实表结构快照：用于验证从 v3.8.* 升级到当前版本的迁移链路
// 参考：tag v3.8.3 的 src-tauri/src/database/schema.rs
const V3_8_SCHEMA_V1_SQL: &str = r#"
    CREATE TABLE providers (
        id TEXT NOT NULL,
        app_type TEXT NOT NULL,
        name TEXT NOT NULL,
        settings_config TEXT NOT NULL,
        website_url TEXT,
        category TEXT,
        created_at INTEGER,
        sort_index INTEGER,
        notes TEXT,
        icon TEXT,
        icon_color TEXT,
        meta TEXT NOT NULL DEFAULT '{}',
        is_current BOOLEAN NOT NULL DEFAULT 0,
        PRIMARY KEY (id, app_type)
    );
    CREATE TABLE provider_endpoints (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        provider_id TEXT NOT NULL,
        app_type TEXT NOT NULL,
        url TEXT NOT NULL,
        added_at INTEGER,
        FOREIGN KEY (provider_id, app_type) REFERENCES providers(id, app_type) ON DELETE CASCADE
    );
    CREATE TABLE mcp_servers (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        server_config TEXT NOT NULL,
        description TEXT,
        homepage TEXT,
        docs TEXT,
        tags TEXT NOT NULL DEFAULT '[]',
        enabled_claude BOOLEAN NOT NULL DEFAULT 0,
        enabled_codex BOOLEAN NOT NULL DEFAULT 0,
        enabled_gemini BOOLEAN NOT NULL DEFAULT 0
    );
    CREATE TABLE prompts (
        id TEXT NOT NULL,
        app_type TEXT NOT NULL,
        name TEXT NOT NULL,
        content TEXT NOT NULL,
        description TEXT,
        enabled BOOLEAN NOT NULL DEFAULT 1,
        created_at INTEGER,
        updated_at INTEGER,
        PRIMARY KEY (id, app_type)
    );
    CREATE TABLE skills (
        key TEXT PRIMARY KEY,
        installed BOOLEAN NOT NULL DEFAULT 0,
        installed_at INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE skill_repos (
        owner TEXT NOT NULL,
        name TEXT NOT NULL,
        branch TEXT NOT NULL DEFAULT 'main',
        enabled BOOLEAN NOT NULL DEFAULT 1,
        PRIMARY KEY (owner, name)
    );
    CREATE TABLE settings (
        key TEXT PRIMARY KEY,
        value TEXT
    );
"#;

#[derive(Debug)]
struct ColumnInfo {
    r#type: String,
    notnull: i64,
    default: Option<String>,
}

fn get_column_info(conn: &Connection, table: &str, column: &str) -> ColumnInfo {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info(\"{table}\");"))
        .expect("prepare pragma");
    let mut rows = stmt.query([]).expect("query pragma");
    while let Some(row) = rows.next().expect("read row") {
        let column_name: String = row.get(1).expect("name");
        if column_name.eq_ignore_ascii_case(column) {
            return ColumnInfo {
                r#type: row.get::<_, String>(2).expect("type"),
                notnull: row.get::<_, i64>(3).expect("notnull"),
                default: row.get::<_, Option<String>>(4).ok().flatten(),
            };
        }
    }
    panic!("column {table}.{column} not found");
}

fn normalize_default(default: &Option<String>) -> Option<String> {
    default
        .as_ref()
        .map(|s| s.trim_matches('\'').trim_matches('"').to_string())
}

fn index_exists(conn: &Connection, index: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1
        )",
        [index],
        |row| row.get::<_, i64>(0),
    )
    .expect("check index")
        != 0
}

#[test]
fn schema_migration_sets_user_version_when_missing() {
    let conn = Connection::open_in_memory().expect("open memory db");

    Database::create_tables_on_conn(&conn).expect("create tables");
    assert_eq!(
        Database::get_user_version(&conn).expect("read version before"),
        0
    );

    Database::apply_schema_migrations_on_conn(&conn).expect("apply migration");

    assert_eq!(
        Database::get_user_version(&conn).expect("read version after"),
        SCHEMA_VERSION
    );
}

#[test]
fn schema_migration_rejects_future_version() {
    let conn = Connection::open_in_memory().expect("open memory db");
    Database::create_tables_on_conn(&conn).expect("create tables");
    Database::set_user_version(&conn, SCHEMA_VERSION + 1).expect("set future version");

    let err =
        Database::apply_schema_migrations_on_conn(&conn).expect_err("should reject higher version");
    let message = err.to_string();
    assert!(message.contains("由较新版本的 CC Switch 创建"));
    assert!(message.contains(&format!("数据库版本: {}", SCHEMA_VERSION + 1)));
    assert!(message.contains(&format!("最高支持数据库版本: {SCHEMA_VERSION}")));
    assert!(message.contains("cc-switch update"));
}

#[test]
#[serial_test::serial]
fn init_rejects_future_schema_before_creating_tables() {
    let _lock = crate::test_support::lock_test_home_and_settings();
    let temp = tempfile::tempdir().expect("create temp dir");
    let _guard = ConfigDirEnvGuard::set(temp.path());
    let db_path = temp.path().join("cc-switch.db");
    let conn = Connection::open(&db_path).expect("open db");
    Database::set_user_version(&conn, SCHEMA_VERSION + 1).expect("set future version");
    drop(conn);

    let err = match Database::init() {
        Ok(_) => panic!("future schema should fail init"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("由较新版本的 CC Switch 创建"),
        "unexpected error: {err}"
    );

    let conn = Connection::open(&db_path).expect("reopen db");
    assert!(
        !Database::table_exists(&conn, "providers").expect("check providers table"),
        "future schema init should not create tables"
    );
}

#[test]
#[serial_test::serial]
fn init_aborts_migration_when_pre_migration_backup_fails() {
    let _lock = crate::test_support::lock_test_home_and_settings();
    let temp = tempfile::tempdir().expect("create temp dir");
    let _guard = ConfigDirEnvGuard::set(temp.path());
    let db_path = temp.path().join("cc-switch.db");

    let db = Database::init().expect("initialize current database");
    Database::set_user_version(
        &db.conn.lock().expect("lock database connection"),
        SCHEMA_VERSION - 1,
    )
    .expect("downgrade schema marker");
    drop(db);

    // A regular file at the backup-directory path deterministically makes the
    // safety backup fail before any schema migration can begin.
    std::fs::write(temp.path().join("backups"), b"not-a-directory")
        .expect("block backup directory creation");

    let err = match Database::init() {
        Ok(_) => panic!("migration must not proceed without its safety backup"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("Pre-migration backup failed"),
        "unexpected error: {err}"
    );

    let conn = Connection::open(&db_path).expect("reopen database");
    assert_eq!(
        Database::get_user_version(&conn).expect("read schema version"),
        SCHEMA_VERSION - 1,
        "failed backup must leave the schema version unchanged"
    );
}

#[test]
#[serial_test::serial]
fn init_rejects_unsafe_config_dir() {
    let _lock = crate::test_support::lock_test_home_and_settings();
    let _guard = ConfigDirEnvGuard::set(Path::new("/tmp"));

    let err = match Database::init() {
        Ok(_) => panic!("unsafe config dir should fail init"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("CC_SWITCH_CONFIG_DIR"),
        "unexpected error: {err}"
    );
}
#[test]
#[serial_test::serial]
fn readonly_snapshot_rejects_missing_database_without_creating_file() {
    let _lock = crate::test_support::lock_test_home_and_settings();
    let temp = tempfile::tempdir().expect("create temp dir");
    let _guard = ConfigDirEnvGuard::set(temp.path());
    let db_path = temp.path().join("cc-switch.db");

    let err = match Database::open_readonly_current_schema() {
        Ok(_) => panic!("missing database should not open as a snapshot"),
        Err(err) => err,
    };

    assert!(
        err.to_string().contains("database is not initialized"),
        "unexpected error: {err}"
    );
    assert!(
        !db_path.exists(),
        "readonly snapshot open should not create the database file"
    );
}

#[test]
#[serial_test::serial]
fn readonly_snapshot_rejects_old_schema_without_migrating() {
    let _lock = crate::test_support::lock_test_home_and_settings();
    let temp = tempfile::tempdir().expect("create temp dir");
    let _guard = ConfigDirEnvGuard::set(temp.path());
    let db_path = temp.path().join("cc-switch.db");
    let conn = Connection::open(&db_path).expect("open db");
    Database::set_user_version(&conn, SCHEMA_VERSION - 1).expect("set old version");
    drop(conn);

    let err = match Database::open_readonly_current_schema() {
        Ok(_) => panic!("old schema should require initialization first"),
        Err(err) => err,
    };

    assert!(
        err.to_string().contains("requires initialization"),
        "unexpected error: {err}"
    );
    let conn = Connection::open(&db_path).expect("reopen db");
    assert!(
        !Database::table_exists(&conn, "providers").expect("check providers table"),
        "readonly snapshot open should not create or migrate tables"
    );
}

#[test]
#[serial_test::serial]
fn readonly_snapshot_opens_current_schema_without_allowing_writes() {
    let _lock = crate::test_support::lock_test_home_and_settings();
    let temp = tempfile::tempdir().expect("create temp dir");
    let _guard = ConfigDirEnvGuard::set(temp.path());

    let db = Database::init().expect("initialize database");
    assert_eq!(
        Database::get_user_version(&db.conn.lock().expect("lock db conn")).expect("read version"),
        SCHEMA_VERSION
    );
    drop(db);

    let snapshot =
        Database::open_readonly_current_schema().expect("open readonly current schema snapshot");
    assert!(
        snapshot.get_all_providers("claude").is_ok(),
        "readonly snapshot should support normal reads"
    );
    let err = snapshot
        .set_setting("snapshot_write_probe", "1")
        .expect_err("readonly snapshot should reject writes");
    assert!(
        err.to_string().contains("readonly") || err.to_string().contains("read-only"),
        "unexpected write error: {err}"
    );
}

#[test]
fn schema_migration_adds_missing_columns_for_providers() {
    let conn = Connection::open_in_memory().expect("open memory db");

    // 创建旧版 providers 表，缺少新增列
    conn.execute_batch(LEGACY_SCHEMA_SQL)
        .expect("seed old schema");

    Database::apply_schema_migrations_on_conn(&conn).expect("apply migrations");

    // 验证关键新增列已补齐
    for (table, column) in [
        ("providers", "meta"),
        ("providers", "is_current"),
        ("provider_endpoints", "added_at"),
        ("mcp_servers", "enabled_gemini"),
        ("prompts", "updated_at"),
        ("skills", "installed_at"),
        ("skill_repos", "enabled"),
    ] {
        assert!(
            Database::has_column(&conn, table, column).expect("check column"),
            "{table}.{column} should exist after migration"
        );
    }

    // 验证 meta 列约束保持一致
    let meta = get_column_info(&conn, "providers", "meta");
    assert_eq!(meta.notnull, 1, "meta should be NOT NULL");
    assert_eq!(
        normalize_default(&meta.default).as_deref(),
        Some("{}"),
        "meta default should be '{{}}'"
    );

    assert_eq!(
        Database::get_user_version(&conn).expect("version after migration"),
        SCHEMA_VERSION
    );
}

#[test]
fn schema_migration_aligns_column_defaults_and_types() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute_batch(LEGACY_SCHEMA_SQL)
        .expect("seed old schema");

    Database::apply_schema_migrations_on_conn(&conn).expect("apply migrations");

    let is_current = get_column_info(&conn, "providers", "is_current");
    assert_eq!(is_current.r#type, "BOOLEAN");
    assert_eq!(is_current.notnull, 1);
    assert_eq!(normalize_default(&is_current.default).as_deref(), Some("0"));

    let tags = get_column_info(&conn, "mcp_servers", "tags");
    assert_eq!(tags.r#type, "TEXT");
    assert_eq!(tags.notnull, 1);
    assert_eq!(normalize_default(&tags.default).as_deref(), Some("[]"));

    let enabled = get_column_info(&conn, "prompts", "enabled");
    assert_eq!(enabled.r#type, "BOOLEAN");
    assert_eq!(enabled.notnull, 1);
    assert_eq!(normalize_default(&enabled.default).as_deref(), Some("1"));

    let installed_at = get_column_info(&conn, "skills", "installed_at");
    assert_eq!(installed_at.r#type, "INTEGER");
    assert_eq!(installed_at.notnull, 1);
    assert_eq!(
        normalize_default(&installed_at.default).as_deref(),
        Some("0")
    );

    let branch = get_column_info(&conn, "skill_repos", "branch");
    assert_eq!(branch.r#type, "TEXT");
    assert_eq!(normalize_default(&branch.default).as_deref(), Some("main"));

    let skill_repo_enabled = get_column_info(&conn, "skill_repos", "enabled");
    assert_eq!(skill_repo_enabled.r#type, "BOOLEAN");
    assert_eq!(skill_repo_enabled.notnull, 1);
    assert_eq!(
        normalize_default(&skill_repo_enabled.default).as_deref(),
        Some("1")
    );
}

#[test]
fn schema_create_tables_include_pricing_model_columns() {
    let conn = Connection::open_in_memory().expect("open memory db");
    Database::create_tables_on_conn(&conn).expect("create tables");

    let multiplier = get_column_info(&conn, "proxy_config", "default_cost_multiplier");
    assert_eq!(multiplier.r#type, "TEXT");
    assert_eq!(multiplier.notnull, 1);
    assert_eq!(normalize_default(&multiplier.default).as_deref(), Some("1"));

    let pricing_source = get_column_info(&conn, "proxy_config", "pricing_model_source");
    assert_eq!(pricing_source.r#type, "TEXT");
    assert_eq!(pricing_source.notnull, 1);
    assert_eq!(
        normalize_default(&pricing_source.default).as_deref(),
        Some("response")
    );

    let request_model = get_column_info(&conn, "proxy_request_logs", "request_model");
    assert_eq!(request_model.r#type, "TEXT");
    assert_eq!(request_model.notnull, 0);
}

#[test]
fn schema_migration_v4_adds_pricing_model_columns() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute_batch(
        r#"
        CREATE TABLE providers (
            id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            name TEXT NOT NULL,
            settings_config TEXT NOT NULL DEFAULT '{}',
            meta TEXT NOT NULL DEFAULT '{}',
            PRIMARY KEY (id, app_type)
        );
        CREATE TABLE proxy_config (app_type TEXT PRIMARY KEY);
        CREATE TABLE proxy_request_logs (request_id TEXT PRIMARY KEY, model TEXT NOT NULL);
        CREATE TABLE mcp_servers (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            server_config TEXT NOT NULL,
            enabled_claude INTEGER NOT NULL DEFAULT 0,
            enabled_codex INTEGER NOT NULL DEFAULT 0,
            enabled_gemini INTEGER NOT NULL DEFAULT 0,
            enabled_opencode INTEGER NOT NULL DEFAULT 0
        );
        "#,
    )
    .expect("seed v4 schema");

    Database::set_user_version(&conn, 4).expect("set user_version=4");
    Database::apply_schema_migrations_on_conn(&conn).expect("apply migrations");

    let multiplier = get_column_info(&conn, "proxy_config", "default_cost_multiplier");
    assert_eq!(multiplier.r#type, "TEXT");
    assert_eq!(multiplier.notnull, 1);
    assert_eq!(normalize_default(&multiplier.default).as_deref(), Some("1"));

    let pricing_source = get_column_info(&conn, "proxy_config", "pricing_model_source");
    assert_eq!(pricing_source.r#type, "TEXT");
    assert_eq!(pricing_source.notnull, 1);
    assert_eq!(
        normalize_default(&pricing_source.default).as_deref(),
        Some("response")
    );

    let request_model = get_column_info(&conn, "proxy_request_logs", "request_model");
    assert_eq!(request_model.r#type, "TEXT");
    assert_eq!(request_model.notnull, 0);

    assert_eq!(
        Database::get_user_version(&conn).expect("version after migration"),
        SCHEMA_VERSION
    );
}

#[test]
fn startup_migration_repairs_legacy_request_logs_before_session_index() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute_batch(
        r#"
        CREATE TABLE proxy_request_logs (
            request_id TEXT PRIMARY KEY,
            model TEXT NOT NULL
        );
        "#,
    )
    .expect("seed legacy request logs table");
    Database::set_user_version(&conn, 4).expect("set user_version=4");

    Database::create_tables_on_conn(&conn).expect("create tables should tolerate legacy logs");
    assert!(
        !Database::has_column(&conn, "proxy_request_logs", "session_id")
            .expect("check session_id before migration"),
        "create_tables should not pretend IF NOT EXISTS upgraded an existing table"
    );
    assert!(
        !index_exists(&conn, "idx_request_logs_session"),
        "session index should wait until the session_id column exists"
    );

    Database::apply_schema_migrations_on_conn(&conn).expect("apply migrations");

    for column in [
        "provider_id",
        "app_type",
        "request_model",
        "input_tokens",
        "output_tokens",
        "status_code",
        "session_id",
        "created_at",
        "data_source",
    ] {
        assert!(
            Database::has_column(&conn, "proxy_request_logs", column)
                .expect("check repaired column"),
            "proxy_request_logs.{column} should be repaired before creating indexes"
        );
    }
    assert!(index_exists(&conn, "idx_request_logs_provider"));
    assert!(index_exists(&conn, "idx_request_logs_created_at"));
    assert!(index_exists(&conn, "idx_request_logs_model"));
    assert!(index_exists(&conn, "idx_request_logs_session"));
    assert!(index_exists(&conn, "idx_request_logs_status"));
    assert!(index_exists(&conn, "idx_request_logs_app_created_at"));
    assert!(index_exists(&conn, "idx_request_logs_dedup_lookup_expr"));
}

#[test]
fn schema_create_tables_include_usage_daily_rollups() {
    let conn = Connection::open_in_memory().expect("open memory db");
    Database::create_tables_on_conn(&conn).expect("create tables");

    assert!(
        Database::table_exists(&conn, "usage_daily_rollups").expect("check table"),
        "usage_daily_rollups should exist after create_tables"
    );

    let avg_latency = get_column_info(&conn, "usage_daily_rollups", "avg_latency_ms");
    assert_eq!(avg_latency.r#type, "INTEGER");
    assert_eq!(avg_latency.notnull, 1);
    assert_eq!(
        normalize_default(&avg_latency.default).as_deref(),
        Some("0")
    );

    let request_model = get_column_info(&conn, "usage_daily_rollups", "request_model");
    assert_eq!(request_model.r#type, "TEXT");
    assert_eq!(request_model.notnull, 1);
    assert_eq!(
        normalize_default(&request_model.default).as_deref(),
        Some("")
    );

    let pricing_model = get_column_info(&conn, "usage_daily_rollups", "pricing_model");
    assert_eq!(pricing_model.r#type, "TEXT");
    assert_eq!(pricing_model.notnull, 1);
    assert_eq!(
        normalize_default(&pricing_model.default).as_deref(),
        Some("")
    );

    for table in ["proxy_request_logs", "usage_daily_rollups"] {
        let semantics = get_column_info(&conn, table, "input_token_semantics");
        assert_eq!(semantics.r#type, "INTEGER");
        assert_eq!(semantics.notnull, 1);
        assert_eq!(normalize_default(&semantics.default).as_deref(), Some("0"));
    }

    assert!(
        Database::table_exists(&conn, "profiles").expect("check profiles table"),
        "profiles should exist after create_tables"
    );
    let mut stmt = conn
        .prepare("PRAGMA table_info('profiles')")
        .expect("prepare profiles table info");
    let profile_columns = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .expect("query profiles table info")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect profiles table info");
    assert_eq!(
        profile_columns,
        vec![
            ("id".to_string(), "TEXT".to_string(), 0, 1),
            ("name".to_string(), "TEXT".to_string(), 1, 0),
            ("payload".to_string(), "TEXT".to_string(), 1, 0),
            ("sort_order".to_string(), "INTEGER".to_string(), 0, 0),
            ("created_at".to_string(), "INTEGER".to_string(), 0, 0),
            ("updated_at".to_string(), "INTEGER".to_string(), 0, 0),
        ]
    );

    assert!(
        Database::table_exists(&conn, "session_log_sync").expect("check session_log_sync table"),
        "session_log_sync should exist after create_tables"
    );

    let content_hash = get_column_info(&conn, "skills", "content_hash");
    assert_eq!(content_hash.r#type, "TEXT");
    assert_eq!(content_hash.notnull, 0);

    let updated_at = get_column_info(&conn, "skills", "updated_at");
    assert_eq!(updated_at.r#type, "INTEGER");
    assert_eq!(updated_at.notnull, 1);
    assert_eq!(normalize_default(&updated_at.default).as_deref(), Some("0"));

    let data_source = get_column_info(&conn, "proxy_request_logs", "data_source");
    assert_eq!(data_source.r#type, "TEXT");
    assert_eq!(data_source.notnull, 1);
    assert_eq!(
        normalize_default(&data_source.default).as_deref(),
        Some("proxy")
    );

    let request_pricing_model = get_column_info(&conn, "proxy_request_logs", "pricing_model");
    assert_eq!(request_pricing_model.r#type, "TEXT");
    assert_eq!(request_pricing_model.notnull, 0);

    let mcp_enabled_hermes = get_column_info(&conn, "mcp_servers", "enabled_hermes");
    assert_eq!(mcp_enabled_hermes.r#type, "BOOLEAN");
    assert_eq!(mcp_enabled_hermes.notnull, 1);
    assert_eq!(
        normalize_default(&mcp_enabled_hermes.default).as_deref(),
        Some("0")
    );

    let skill_enabled_hermes = get_column_info(&conn, "skills", "enabled_hermes");
    assert_eq!(skill_enabled_hermes.r#type, "BOOLEAN");
    assert_eq!(skill_enabled_hermes.notnull, 1);
    assert_eq!(
        normalize_default(&skill_enabled_hermes.default).as_deref(),
        Some("0")
    );
}

#[test]
#[serial_test::serial]
fn init_runs_startup_usage_rollup() {
    let _lock = crate::test_support::lock_test_home_and_settings();
    let temp = tempfile::tempdir().expect("create temp dir");
    let _guard = ConfigDirEnvGuard::set(temp.path());

    {
        let db = Database::init().expect("init db");
        let old_ts = chrono::Utc::now().timestamp() - 40 * 86_400;
        let conn = db.conn.lock().expect("lock db");
        conn.execute(
            "INSERT INTO proxy_request_logs (
                request_id, provider_id, app_type, model, request_model,
                input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                total_cost_usd, latency_ms, status_code, created_at, data_source
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                "startup-rollup-old",
                "anthropic",
                "claude",
                "claude-sonnet-4",
                "claude-sonnet-4",
                100,
                50,
                7,
                3,
                "0.25",
                123,
                200,
                old_ts,
                "proxy",
            ],
        )
        .expect("seed old usage log");
        conn.execute(
            "INSERT INTO proxy_request_logs (
                request_id, provider_id, app_type, model, request_model,
                input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd,
                total_cost_usd, latency_ms, status_code, created_at, data_source
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5,
                ?6, ?7, ?8, ?9,
                '0', '0', '0', '0',
                '0', ?10, ?11, ?12, ?13
            )",
            params![
                "startup-rollup-zero-cost",
                "_codex_session",
                "codex",
                "gpt-5.5",
                "gpt-5.5",
                1_000_000,
                0,
                0,
                0,
                0,
                200,
                old_ts,
                "codex_session",
            ],
        )
        .expect("seed old zero-cost usage log");
    }

    let db = Database::init().expect("reinit db");
    let conn = db.conn.lock().expect("lock db");
    let remaining: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM proxy_request_logs WHERE request_id = 'startup-rollup-old'",
            [],
            |row| row.get(0),
        )
        .expect("count old request log");
    assert_eq!(
        remaining, 0,
        "startup maintenance should prune rolled-up detail logs"
    );

    let rollup: (i64, i64, i64, i64, i64, i64, String, i64) = conn
        .query_row(
            "SELECT request_count, success_count, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms
             FROM usage_daily_rollups
             WHERE app_type = 'claude'
               AND provider_id = 'anthropic'
               AND model = 'claude-sonnet-4'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .expect("read startup rollup");
    assert_eq!(rollup, (1, 1, 100, 50, 7, 3, "0.25".to_string(), 123));

    let zero_cost_remaining: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM proxy_request_logs WHERE request_id = 'startup-rollup-zero-cost'",
            [],
            |row| row.get(0),
        )
        .expect("count old zero-cost request log");
    assert_eq!(
        zero_cost_remaining, 0,
        "startup maintenance should prune old zero-cost details only after backfill"
    );

    let (zero_cost_requests, zero_cost_total): (i64, f64) = conn
        .query_row(
            "SELECT request_count, CAST(total_cost_usd AS REAL)
             FROM usage_daily_rollups
             WHERE app_type = 'codex'
               AND provider_id = '_codex_session'
               AND model = 'gpt-5.5'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read zero-cost startup rollup");
    assert_eq!(zero_cost_requests, 1);
    assert_eq!(
        zero_cost_total, 5.0,
        "startup maintenance should backfill costs before rolling up old details"
    );
}

#[test]
fn schema_migration_v5_adds_usage_daily_rollups() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute_batch(
        r#"
        CREATE TABLE providers (
            id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            name TEXT NOT NULL,
            settings_config TEXT NOT NULL,
            meta TEXT NOT NULL DEFAULT '{}',
            is_current BOOLEAN NOT NULL DEFAULT 0,
            in_failover_queue BOOLEAN NOT NULL DEFAULT 0,
            PRIMARY KEY (id, app_type)
        );
        CREATE TABLE mcp_servers (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            server_config TEXT NOT NULL,
            enabled_claude INTEGER NOT NULL DEFAULT 0,
            enabled_codex INTEGER NOT NULL DEFAULT 0,
            enabled_gemini INTEGER NOT NULL DEFAULT 0,
            enabled_opencode INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE prompts (
            id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            name TEXT NOT NULL,
            content TEXT NOT NULL,
            PRIMARY KEY (id, app_type)
        );
        CREATE TABLE skills (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            directory TEXT NOT NULL,
            enabled_claude BOOLEAN NOT NULL DEFAULT 0,
            enabled_codex BOOLEAN NOT NULL DEFAULT 0,
            enabled_gemini BOOLEAN NOT NULL DEFAULT 0,
            enabled_opencode BOOLEAN NOT NULL DEFAULT 0,
            installed_at INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE skill_repos (
            owner TEXT NOT NULL,
            name TEXT NOT NULL,
            branch TEXT NOT NULL DEFAULT 'main',
            enabled BOOLEAN NOT NULL DEFAULT 1,
            PRIMARY KEY (owner, name)
        );
        CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
        CREATE TABLE proxy_config (
            app_type TEXT PRIMARY KEY,
            default_cost_multiplier TEXT NOT NULL DEFAULT '1',
            pricing_model_source TEXT NOT NULL DEFAULT 'response'
        );
        CREATE TABLE proxy_request_logs (
            request_id TEXT PRIMARY KEY,
            provider_id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            model TEXT NOT NULL,
            request_model TEXT
        );
        CREATE TABLE stream_check_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            provider_id TEXT NOT NULL,
            provider_name TEXT NOT NULL,
            app_type TEXT NOT NULL,
            status TEXT NOT NULL,
            success INTEGER NOT NULL,
            message TEXT NOT NULL,
            tested_at INTEGER NOT NULL
        );
        CREATE TABLE model_pricing (
            model_id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            input_cost_per_million TEXT NOT NULL,
            output_cost_per_million TEXT NOT NULL,
            cache_read_cost_per_million TEXT NOT NULL DEFAULT '0',
            cache_creation_cost_per_million TEXT NOT NULL DEFAULT '0'
        );
        CREATE TABLE proxy_live_backup (
            app_type TEXT PRIMARY KEY,
            original_config TEXT NOT NULL,
            backed_up_at TEXT NOT NULL
        );
        "#,
    )
    .expect("seed v5 schema");
    Database::set_user_version(&conn, 5).expect("set user_version=5");

    Database::apply_schema_migrations_on_conn(&conn).expect("apply migrations");

    assert!(
        Database::table_exists(&conn, "usage_daily_rollups").expect("check table"),
        "usage_daily_rollups should exist after v5 migration"
    );
    assert_eq!(
        Database::get_user_version(&conn).expect("version after migration"),
        SCHEMA_VERSION
    );
}

#[test]
fn schema_migration_v6_adds_skill_update_columns() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute_batch(
        r#"
        CREATE TABLE skills (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            directory TEXT NOT NULL,
            enabled_claude BOOLEAN NOT NULL DEFAULT 0,
            enabled_codex BOOLEAN NOT NULL DEFAULT 0,
            enabled_gemini BOOLEAN NOT NULL DEFAULT 0,
            enabled_opencode BOOLEAN NOT NULL DEFAULT 0,
            installed_at INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE providers (
            id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            name TEXT NOT NULL,
            settings_config TEXT NOT NULL,
            meta TEXT NOT NULL DEFAULT '{}',
            PRIMARY KEY (id, app_type)
        );
        CREATE TABLE mcp_servers (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            server_config TEXT NOT NULL,
            enabled_claude INTEGER NOT NULL DEFAULT 0,
            enabled_codex INTEGER NOT NULL DEFAULT 0,
            enabled_gemini INTEGER NOT NULL DEFAULT 0,
            enabled_opencode INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE prompts (
            id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            name TEXT NOT NULL,
            content TEXT NOT NULL,
            PRIMARY KEY (id, app_type)
        );
        CREATE TABLE skill_repos (
            owner TEXT NOT NULL,
            name TEXT NOT NULL,
            branch TEXT NOT NULL DEFAULT 'main',
            enabled BOOLEAN NOT NULL DEFAULT 1,
            PRIMARY KEY (owner, name)
        );
        CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
        CREATE TABLE proxy_config (app_type TEXT PRIMARY KEY);
        CREATE TABLE proxy_request_logs (request_id TEXT PRIMARY KEY, model TEXT NOT NULL);
        CREATE TABLE stream_check_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            provider_id TEXT NOT NULL,
            provider_name TEXT NOT NULL,
            app_type TEXT NOT NULL,
            status TEXT NOT NULL,
            success INTEGER NOT NULL,
            message TEXT NOT NULL,
            tested_at INTEGER NOT NULL
        );
        CREATE TABLE model_pricing (
            model_id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            input_cost_per_million TEXT NOT NULL,
            output_cost_per_million TEXT NOT NULL,
            cache_read_cost_per_million TEXT NOT NULL DEFAULT '0',
            cache_creation_cost_per_million TEXT NOT NULL DEFAULT '0'
        );
        CREATE TABLE proxy_live_backup (
            app_type TEXT PRIMARY KEY,
            original_config TEXT NOT NULL,
            backed_up_at TEXT NOT NULL
        );
        CREATE TABLE usage_daily_rollups (
            date TEXT NOT NULL,
            app_type TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            model TEXT NOT NULL,
            request_count INTEGER NOT NULL DEFAULT 0,
            success_count INTEGER NOT NULL DEFAULT 0,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens INTEGER NOT NULL DEFAULT 0,
            cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
            total_cost_usd TEXT NOT NULL DEFAULT '0',
            avg_latency_ms INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (date, app_type, provider_id, model)
        );
        "#,
    )
    .expect("seed v6 schema");

    Database::set_user_version(&conn, 6).expect("set user_version=6");
    Database::apply_schema_migrations_on_conn(&conn).expect("apply migrations");

    assert!(
        Database::has_column(&conn, "skills", "content_hash").expect("check content_hash"),
        "skills.content_hash should exist after v6 -> current migration"
    );
    assert!(
        Database::has_column(&conn, "skills", "updated_at").expect("check updated_at"),
        "skills.updated_at should exist after v6 -> current migration"
    );
}

#[test]
fn schema_migration_v7_adds_session_log_tracking_and_corrects_pricing() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute_batch(
        r#"
        CREATE TABLE providers (
            id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            name TEXT NOT NULL,
            settings_config TEXT NOT NULL,
            meta TEXT NOT NULL DEFAULT '{}',
            PRIMARY KEY (id, app_type)
        );
        CREATE TABLE skills (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            directory TEXT NOT NULL,
            enabled_claude BOOLEAN NOT NULL DEFAULT 0,
            enabled_codex BOOLEAN NOT NULL DEFAULT 0,
            enabled_gemini BOOLEAN NOT NULL DEFAULT 0,
            enabled_opencode BOOLEAN NOT NULL DEFAULT 0,
            installed_at INTEGER NOT NULL DEFAULT 0,
            content_hash TEXT,
            updated_at INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE mcp_servers (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            server_config TEXT NOT NULL,
            enabled_claude INTEGER NOT NULL DEFAULT 0,
            enabled_codex INTEGER NOT NULL DEFAULT 0,
            enabled_gemini INTEGER NOT NULL DEFAULT 0,
            enabled_opencode INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE prompts (
            id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            name TEXT NOT NULL,
            content TEXT NOT NULL,
            PRIMARY KEY (id, app_type)
        );
        CREATE TABLE skill_repos (
            owner TEXT NOT NULL,
            name TEXT NOT NULL,
            branch TEXT NOT NULL DEFAULT 'main',
            enabled BOOLEAN NOT NULL DEFAULT 1,
            PRIMARY KEY (owner, name)
        );
        CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
        CREATE TABLE proxy_config (app_type TEXT PRIMARY KEY);
        CREATE TABLE proxy_request_logs (
            request_id TEXT PRIMARY KEY,
            provider_id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            model TEXT NOT NULL
        );
        CREATE TABLE stream_check_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            provider_id TEXT NOT NULL,
            provider_name TEXT NOT NULL,
            app_type TEXT NOT NULL,
            status TEXT NOT NULL,
            success INTEGER NOT NULL,
            message TEXT NOT NULL,
            tested_at INTEGER NOT NULL
        );
        CREATE TABLE model_pricing (
            model_id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            input_cost_per_million TEXT NOT NULL,
            output_cost_per_million TEXT NOT NULL,
            cache_read_cost_per_million TEXT NOT NULL DEFAULT '0',
            cache_creation_cost_per_million TEXT NOT NULL DEFAULT '0'
        );
        INSERT INTO model_pricing (
            model_id, display_name, input_cost_per_million, output_cost_per_million,
            cache_read_cost_per_million, cache_creation_cost_per_million
        ) VALUES ('deepseek-v3', 'DeepSeek V3', '2.00', '8.00', '0.40', '0');
        CREATE TABLE proxy_live_backup (
            app_type TEXT PRIMARY KEY,
            original_config TEXT NOT NULL,
            backed_up_at TEXT NOT NULL
        );
        CREATE TABLE usage_daily_rollups (
            date TEXT NOT NULL,
            app_type TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            model TEXT NOT NULL,
            request_count INTEGER NOT NULL DEFAULT 0,
            success_count INTEGER NOT NULL DEFAULT 0,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens INTEGER NOT NULL DEFAULT 0,
            cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
            total_cost_usd TEXT NOT NULL DEFAULT '0',
            avg_latency_ms INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (date, app_type, provider_id, model)
        );
        "#,
    )
    .expect("seed v7 schema");

    Database::set_user_version(&conn, 7).expect("set user_version=7");
    Database::apply_schema_migrations_on_conn(&conn).expect("apply migrations");

    assert!(
        Database::has_column(&conn, "proxy_request_logs", "data_source")
            .expect("check data_source"),
        "proxy_request_logs.data_source should exist after v7 -> current migration"
    );
    assert!(
        Database::table_exists(&conn, "session_log_sync").expect("check session_log_sync"),
        "session_log_sync should exist after v7 -> current migration"
    );

    let pricing: (String, String, String) = conn
        .query_row(
            "SELECT input_cost_per_million, output_cost_per_million, cache_read_cost_per_million
             FROM model_pricing WHERE model_id = 'deepseek-v3'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read deepseek-v3 pricing");
    assert_eq!(
        pricing,
        ("0.28".to_string(), "1.11".to_string(), "0.028".to_string()),
        "v7 -> v8 migration should normalize DeepSeek pricing values"
    );
}

#[test]
fn schema_migration_v8_refreshes_model_pricing_and_reaches_current_schema() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute_batch(
        r#"
        CREATE TABLE providers (
            id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            name TEXT NOT NULL,
            settings_config TEXT NOT NULL,
            meta TEXT NOT NULL DEFAULT '{}',
            PRIMARY KEY (id, app_type)
        );
        CREATE TABLE skills (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            directory TEXT NOT NULL,
            enabled_claude BOOLEAN NOT NULL DEFAULT 0,
            enabled_codex BOOLEAN NOT NULL DEFAULT 0,
            enabled_gemini BOOLEAN NOT NULL DEFAULT 0,
            enabled_opencode BOOLEAN NOT NULL DEFAULT 0,
            installed_at INTEGER NOT NULL DEFAULT 0,
            content_hash TEXT,
            updated_at INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE mcp_servers (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            server_config TEXT NOT NULL,
            enabled_claude INTEGER NOT NULL DEFAULT 0,
            enabled_codex INTEGER NOT NULL DEFAULT 0,
            enabled_gemini INTEGER NOT NULL DEFAULT 0,
            enabled_opencode INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE prompts (
            id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            name TEXT NOT NULL,
            content TEXT NOT NULL,
            PRIMARY KEY (id, app_type)
        );
        CREATE TABLE skill_repos (
            owner TEXT NOT NULL,
            name TEXT NOT NULL,
            branch TEXT NOT NULL DEFAULT 'main',
            enabled BOOLEAN NOT NULL DEFAULT 1,
            PRIMARY KEY (owner, name)
        );
        CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
        CREATE TABLE proxy_config (app_type TEXT PRIMARY KEY);
        CREATE TABLE proxy_request_logs (
            request_id TEXT PRIMARY KEY,
            provider_id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            model TEXT NOT NULL,
            data_source TEXT NOT NULL DEFAULT 'proxy'
        );
        CREATE TABLE stream_check_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            provider_id TEXT NOT NULL,
            provider_name TEXT NOT NULL,
            app_type TEXT NOT NULL,
            status TEXT NOT NULL,
            success INTEGER NOT NULL,
            message TEXT NOT NULL,
            tested_at INTEGER NOT NULL
        );
        CREATE TABLE model_pricing (
            model_id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            input_cost_per_million TEXT NOT NULL,
            output_cost_per_million TEXT NOT NULL,
            cache_read_cost_per_million TEXT NOT NULL DEFAULT '0',
            cache_creation_cost_per_million TEXT NOT NULL DEFAULT '0'
        );
        INSERT INTO model_pricing (
            model_id, display_name, input_cost_per_million, output_cost_per_million,
            cache_read_cost_per_million, cache_creation_cost_per_million
        ) VALUES ('deepseek-v3', 'DeepSeek V3', '9.99', '9.99', '9.99', '0');
        CREATE TABLE proxy_live_backup (
            app_type TEXT PRIMARY KEY,
            original_config TEXT NOT NULL,
            backed_up_at TEXT NOT NULL
        );
        CREATE TABLE usage_daily_rollups (
            date TEXT NOT NULL,
            app_type TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            model TEXT NOT NULL,
            request_count INTEGER NOT NULL DEFAULT 0,
            success_count INTEGER NOT NULL DEFAULT 0,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens INTEGER NOT NULL DEFAULT 0,
            cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
            total_cost_usd TEXT NOT NULL DEFAULT '0',
            avg_latency_ms INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (date, app_type, provider_id, model)
        );
        CREATE TABLE session_log_sync (
            file_path TEXT PRIMARY KEY,
            last_modified INTEGER NOT NULL,
            last_line_offset INTEGER NOT NULL DEFAULT 0,
            last_synced_at INTEGER NOT NULL
        );
        "#,
    )
    .expect("seed v8 schema");

    Database::set_user_version(&conn, 8).expect("set user_version=8");
    Database::apply_schema_migrations_on_conn(&conn).expect("apply migrations");

    assert_eq!(
        Database::get_user_version(&conn).expect("version after migration"),
        SCHEMA_VERSION
    );

    let deepseek_v3: (String, String, String) = conn
        .query_row(
            "SELECT input_cost_per_million, output_cost_per_million, cache_read_cost_per_million
             FROM model_pricing WHERE model_id = 'deepseek-v3'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read deepseek-v3 pricing");
    assert_eq!(
        deepseek_v3,
        ("0.28".to_string(), "1.11".to_string(), "0.028".to_string()),
        "v8 -> v9 migration should fully refresh pricing data"
    );

    let latest_model_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM model_pricing WHERE model_id = 'gpt-5.4'",
            [],
            |row| row.get(0),
        )
        .expect("count gpt-5.4");
    assert_eq!(
        latest_model_count, 1,
        "latest pricing catalog should be seeded"
    );

    assert!(
        Database::has_column(&conn, "mcp_servers", "enabled_hermes")
            .expect("check mcp enabled_hermes"),
        "v9 -> v10 should add enabled_hermes to mcp_servers"
    );
    assert!(
        Database::has_column(&conn, "skills", "enabled_hermes")
            .expect("check skills enabled_hermes"),
        "v9 -> v10 should add enabled_hermes to skills"
    );
    assert!(
        Database::has_column(&conn, "proxy_request_logs", "pricing_model")
            .expect("check proxy_request_logs pricing_model"),
        "v10 -> v11 should add pricing_model to proxy_request_logs"
    );
    assert!(
        Database::has_column(&conn, "usage_daily_rollups", "request_model")
            .expect("check rollup request_model"),
        "v10 -> v11 should add request_model to usage_daily_rollups"
    );
    assert!(
        Database::has_column(&conn, "usage_daily_rollups", "pricing_model")
            .expect("check rollup pricing_model"),
        "v10 -> v11 should add pricing_model to usage_daily_rollups"
    );
}

#[test]
fn schema_migration_v10_to_v11_preserves_rollup_rows_with_empty_new_dimensions() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute_batch(
        r#"
        CREATE TABLE proxy_request_logs (
            request_id TEXT PRIMARY KEY,
            provider_id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            model TEXT NOT NULL,
            request_model TEXT,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens INTEGER NOT NULL DEFAULT 0,
            cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
            input_cost_usd TEXT NOT NULL DEFAULT '0',
            output_cost_usd TEXT NOT NULL DEFAULT '0',
            cache_read_cost_usd TEXT NOT NULL DEFAULT '0',
            cache_creation_cost_usd TEXT NOT NULL DEFAULT '0',
            total_cost_usd TEXT NOT NULL DEFAULT '0',
            latency_ms INTEGER NOT NULL,
            status_code INTEGER NOT NULL,
            cost_multiplier TEXT NOT NULL DEFAULT '1.0',
            created_at INTEGER NOT NULL,
            data_source TEXT NOT NULL DEFAULT 'proxy'
        );
        INSERT INTO proxy_request_logs (
            request_id, provider_id, app_type, model, request_model,
            input_tokens, output_tokens, total_cost_usd, latency_ms, status_code, created_at
        ) VALUES ('log-1', 'provider-a', 'claude', 'kimi-k2', 'claude-sonnet-4', 10, 20, '0.03', 123, 200, 1);
        CREATE TABLE usage_daily_rollups (
            date TEXT NOT NULL,
            app_type TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            model TEXT NOT NULL,
            request_count INTEGER NOT NULL DEFAULT 0,
            success_count INTEGER NOT NULL DEFAULT 0,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens INTEGER NOT NULL DEFAULT 0,
            cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
            total_cost_usd TEXT NOT NULL DEFAULT '0',
            avg_latency_ms INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (date, app_type, provider_id, model)
        );
        INSERT INTO usage_daily_rollups (
            date, app_type, provider_id, model, request_count, success_count,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            total_cost_usd, avg_latency_ms
        ) VALUES ('2026-06-01', 'claude', 'provider-a', 'kimi-k2', 2, 2, 100, 50, 7, 3, '0.42', 222);
        "#,
    )
    .expect("seed v10 schema");

    Database::set_user_version(&conn, 10).expect("set user_version=10");
    Database::apply_schema_migrations_on_conn(&conn).expect("apply v11 migration");

    assert_eq!(
        Database::get_user_version(&conn).expect("version after migration"),
        SCHEMA_VERSION
    );
    assert!(
        Database::has_column(&conn, "proxy_request_logs", "pricing_model")
            .expect("check request pricing_model"),
        "v11 migration should add proxy_request_logs.pricing_model"
    );

    let rollup: (String, String, i64, i64, String) = conn
        .query_row(
            "SELECT request_model, pricing_model, request_count, input_tokens, total_cost_usd
             FROM usage_daily_rollups
             WHERE date = '2026-06-01' AND app_type = 'claude'
               AND provider_id = 'provider-a' AND model = 'kimi-k2'",
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
        )
        .expect("read migrated rollup");
    assert_eq!(
        rollup,
        ("".to_string(), "".to_string(), 2, 100, "0.42".to_string())
    );
}

#[test]
fn schema_migration_v11_to_v13_preserves_data_and_adds_upstream_schema() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute_batch(
        r#"
        CREATE TABLE proxy_request_logs (
            request_id TEXT PRIMARY KEY,
            provider_id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            model TEXT NOT NULL,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens INTEGER NOT NULL DEFAULT 0,
            cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
            latency_ms INTEGER NOT NULL DEFAULT 0,
            status_code INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO proxy_request_logs (
            request_id, provider_id, app_type, model, input_tokens,
            output_tokens, cache_read_tokens, cache_creation_tokens,
            latency_ms, status_code, created_at
        ) VALUES ('legacy-log', 'p1', 'codex', 'gpt-5.5', 800, 10, 300, 200, 50, 200, 1);

        CREATE TABLE usage_daily_rollups (
            date TEXT NOT NULL,
            app_type TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            model TEXT NOT NULL,
            request_model TEXT NOT NULL DEFAULT '',
            pricing_model TEXT NOT NULL DEFAULT '',
            request_count INTEGER NOT NULL DEFAULT 0,
            success_count INTEGER NOT NULL DEFAULT 0,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens INTEGER NOT NULL DEFAULT 0,
            cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
            total_cost_usd TEXT NOT NULL DEFAULT '0',
            avg_latency_ms INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (date, app_type, provider_id, model, request_model, pricing_model)
        );
        INSERT INTO usage_daily_rollups (
            date, app_type, provider_id, model, request_count,
            input_tokens, cache_read_tokens, cache_creation_tokens
        ) VALUES ('2026-07-01', 'codex', 'p1', 'gpt-5.5', 1, 500, 300, 200);

        CREATE TABLE profiles (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            payload TEXT NOT NULL,
            sort_order INTEGER,
            created_at INTEGER,
            updated_at INTEGER
        );
        INSERT INTO profiles (id, name, payload, sort_order, created_at, updated_at)
        VALUES ('profile-1', 'Existing', '{"providers":{}}', 7, 100, 200);
        "#,
    )
    .expect("seed schema v11");
    Database::set_user_version(&conn, 11).expect("set user_version=11");

    Database::apply_schema_migrations_on_conn(&conn).expect("migrate v11 to v13");

    assert_eq!(
        Database::get_user_version(&conn).expect("read migrated version"),
        SCHEMA_VERSION
    );
    for table in ["proxy_request_logs", "usage_daily_rollups"] {
        let semantics = get_column_info(&conn, table, "input_token_semantics");
        assert_eq!(semantics.r#type, "INTEGER");
        assert_eq!(semantics.notnull, 1);
        assert_eq!(normalize_default(&semantics.default).as_deref(), Some("0"));
    }
    let legacy_semantics: (i64, i64) = conn
        .query_row(
            "SELECT
                (SELECT input_token_semantics FROM proxy_request_logs WHERE request_id = 'legacy-log'),
                (SELECT input_token_semantics FROM usage_daily_rollups WHERE date = '2026-07-01')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read migrated legacy semantics");
    assert_eq!(legacy_semantics, (0, 0));

    let profile: (String, String, i64, i64, i64) = conn
        .query_row(
            "SELECT name, payload, sort_order, created_at, updated_at
             FROM profiles WHERE id = 'profile-1'",
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
        )
        .expect("read preserved profile");
    assert_eq!(
        profile,
        (
            "Existing".to_string(),
            "{\"providers\":{}}".to_string(),
            7,
            100,
            200,
        )
    );

    Database::apply_schema_migrations_on_conn(&conn).expect("migration should be idempotent");
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM profiles", [], |row| row
            .get::<_, i64>(0))
            .expect("count profiles"),
        1
    );
}

#[test]
fn schema_current_v13_repairs_missing_usage_semantics_columns() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute_batch(
        r#"
        CREATE TABLE proxy_request_logs (request_id TEXT PRIMARY KEY);
        CREATE TABLE usage_daily_rollups (date TEXT PRIMARY KEY);
        "#,
    )
    .expect("seed partial v13 schema");
    Database::set_user_version(&conn, 13).expect("set user_version=13");

    Database::apply_schema_migrations_on_conn(&conn).expect("repair current schema");

    assert!(
        Database::has_column(&conn, "proxy_request_logs", "input_token_semantics")
            .expect("check log semantics")
    );
    assert!(
        Database::has_column(&conn, "usage_daily_rollups", "input_token_semantics")
            .expect("check rollup semantics")
    );
}

#[test]
fn schema_migration_v11_to_v13_rolls_back_both_steps_on_v13_failure() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute_batch(
        r#"
        CREATE TABLE proxy_request_logs (
            request_id TEXT PRIMARY KEY,
            model TEXT NOT NULL
        );
        INSERT INTO proxy_request_logs (request_id, model) VALUES ('keep-me', 'gpt-5.5');
        CREATE TABLE usage_daily_rollups (date TEXT PRIMARY KEY);
        "#,
    )
    .expect("seed v11 tables");
    Database::set_user_version(&conn, 11).expect("set user_version=11");

    conn.authorizer(Some(|context: AuthContext<'_>| match context.action {
        AuthAction::AlterTable {
            table_name: "usage_daily_rollups",
            ..
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }));
    let error = Database::apply_schema_migrations_on_conn(&conn)
        .expect_err("denied v13 ALTER should fail migration");
    conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);

    assert!(
        error.to_string().contains("input_token_semantics")
            || error.to_string().contains("not authorized"),
        "unexpected migration error: {error}"
    );
    assert_eq!(
        Database::get_user_version(&conn).expect("read rolled-back version"),
        11
    );
    assert!(!Database::table_exists(&conn, "profiles").expect("check rolled-back profiles table"));
    assert!(
        !Database::has_column(&conn, "proxy_request_logs", "input_token_semantics")
            .expect("check rolled-back log column")
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM proxy_request_logs WHERE request_id = 'keep-me'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("count preserved row"),
        1
    );
}

#[test]
fn create_tables_migrates_legacy_global_profile_marker_once() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute(
        "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT)",
        [],
    )
    .expect("create settings");
    conn.execute_batch(
        "INSERT INTO settings (key, value) VALUES ('current_profile_id', 'legacy');
         INSERT INTO settings (key, value) VALUES ('current_profile_id_claude', 'newer');",
    )
    .expect("seed profile markers");

    Database::create_tables_on_conn(&conn).expect("create canonical schema");
    Database::create_tables_on_conn(&conn).expect("repeat canonical schema");

    let scoped: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'current_profile_id_claude'",
            [],
            |row| row.get(0),
        )
        .expect("read scoped marker");
    assert_eq!(scoped, "legacy");
    let old_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM settings WHERE key = 'current_profile_id'",
            [],
            |row| row.get(0),
        )
        .expect("count old marker");
    assert_eq!(old_count, 0);
}

#[test]
fn schema_migration_v9_adds_hermes_columns() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute_batch(
        r#"
        CREATE TABLE mcp_servers (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            server_config TEXT NOT NULL,
            enabled_claude INTEGER NOT NULL DEFAULT 0,
            enabled_codex INTEGER NOT NULL DEFAULT 0,
            enabled_gemini INTEGER NOT NULL DEFAULT 0,
            enabled_opencode INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE skills (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            directory TEXT NOT NULL,
            enabled_claude BOOLEAN NOT NULL DEFAULT 0,
            enabled_codex BOOLEAN NOT NULL DEFAULT 0,
            enabled_gemini BOOLEAN NOT NULL DEFAULT 0,
            enabled_opencode BOOLEAN NOT NULL DEFAULT 0,
            installed_at INTEGER NOT NULL DEFAULT 0,
            content_hash TEXT,
            updated_at INTEGER NOT NULL DEFAULT 0
        );
        "#,
    )
    .expect("seed v9 schema");

    Database::set_user_version(&conn, 9).expect("set user_version=9");
    Database::apply_schema_migrations_on_conn(&conn).expect("apply migrations");

    assert_eq!(
        Database::get_user_version(&conn).expect("version after migration"),
        SCHEMA_VERSION
    );
    assert!(
        Database::has_column(&conn, "mcp_servers", "enabled_hermes")
            .expect("check mcp enabled_hermes"),
        "mcp_servers.enabled_hermes should exist after v9 -> v10 migration"
    );
    assert!(
        Database::has_column(&conn, "skills", "enabled_hermes")
            .expect("check skills enabled_hermes"),
        "skills.enabled_hermes should exist after v9 -> v10 migration"
    );
}

#[test]
fn mcp_dao_roundtrip_preserves_hermes_enablement() {
    let db = Database::memory().expect("create memory db");

    {
        let conn = db.conn.lock().expect("lock conn");
        conn.execute(
            "INSERT INTO mcp_servers (
                id, name, server_config, tags,
                enabled_claude, enabled_codex, enabled_gemini, enabled_opencode, enabled_hermes
            ) VALUES (?1, ?2, ?3, ?4, 0, 0, 0, 0, 1)",
            params![
                "remote-hermes",
                "Remote Hermes",
                serde_json::to_string(&json!({
                    "type": "stdio",
                    "command": "echo"
                }))
                .expect("serialize server config"),
                "[]",
            ],
        )
        .expect("seed hermes-enabled mcp row");
    }

    let mut servers = db.get_all_mcp_servers().expect("load mcp servers");
    let mut server = servers
        .shift_remove("remote-hermes")
        .expect("find seeded mcp server");
    assert!(
        server.apps.hermes,
        "DAO load should preserve enabled_hermes in memory"
    );

    server.description = Some("updated".to_string());
    db.save_mcp_server(&server).expect("save mcp server");

    let enabled_hermes: i64 = {
        let conn = db.conn.lock().expect("lock conn");
        conn.query_row(
            "SELECT enabled_hermes FROM mcp_servers WHERE id = 'remote-hermes'",
            [],
            |row| row.get(0),
        )
        .expect("read enabled_hermes after save")
    };
    assert_eq!(enabled_hermes, 1, "save should not clear enabled_hermes");
}

#[test]
fn schema_create_tables_repairs_legacy_proxy_config_singleton_to_per_app() {
    let conn = Connection::open_in_memory().expect("open memory db");

    // 模拟测试版 v2：user_version=2，但 proxy_config 仍是单例结构（无 app_type）
    Database::set_user_version(&conn, 2).expect("set user_version");
    conn.execute_batch(
        r#"
        CREATE TABLE proxy_config (
            id INTEGER PRIMARY KEY,
            enabled INTEGER NOT NULL DEFAULT 0,
            listen_address TEXT NOT NULL DEFAULT '127.0.0.1',
            listen_port INTEGER NOT NULL DEFAULT 5000,
            max_retries INTEGER NOT NULL DEFAULT 3,
            request_timeout INTEGER NOT NULL DEFAULT 300,
            enable_logging INTEGER NOT NULL DEFAULT 1,
            target_app TEXT NOT NULL DEFAULT 'claude',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT INTO proxy_config (id, enabled) VALUES (1, 1);
        "#,
    )
    .expect("seed legacy proxy_config");

    Database::create_tables_on_conn(&conn).expect("create tables should repair proxy_config");

    assert!(
        Database::has_column(&conn, "proxy_config", "app_type").expect("check app_type"),
        "proxy_config should be migrated to per-app structure"
    );

    let count: i32 = conn
        .query_row("SELECT COUNT(*) FROM proxy_config", [], |r| r.get(0))
        .expect("count rows");
    assert_eq!(count, 3, "per-app proxy_config should have 3 rows");

    // 新结构下应能按 app_type 查询
    let _: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM proxy_config WHERE app_type = 'claude'",
            [],
            |r| r.get(0),
        )
        .expect("query by app_type");
}

#[test]
fn schema_migration_masks_legacy_failover_without_takeover() {
    let conn = Connection::open_in_memory().expect("open memory db");

    Database::set_user_version(&conn, 2).expect("set user_version");
    conn.execute_batch(
        r#"
        CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
        INSERT INTO settings (key, value) VALUES
            ('proxy_takeover_claude', 'false'),
            ('auto_failover_enabled_claude', 'true'),
            ('proxy_takeover_codex', 'true'),
            ('auto_failover_enabled_codex', 'true');

        CREATE TABLE proxy_config (
            id INTEGER PRIMARY KEY,
            enabled INTEGER NOT NULL DEFAULT 0,
            listen_address TEXT NOT NULL DEFAULT '127.0.0.1',
            listen_port INTEGER NOT NULL DEFAULT 5000,
            max_retries INTEGER NOT NULL DEFAULT 3,
            request_timeout INTEGER NOT NULL DEFAULT 300,
            enable_logging INTEGER NOT NULL DEFAULT 1,
            target_app TEXT NOT NULL DEFAULT 'claude',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT INTO proxy_config (id, enabled) VALUES (1, 1);
        "#,
    )
    .expect("seed legacy proxy state");

    Database::create_tables_on_conn(&conn).expect("create tables");
    Database::apply_schema_migrations_on_conn(&conn).expect("apply migrations");

    let claude: (i64, i64) = conn
        .query_row(
            "SELECT enabled, auto_failover_enabled FROM proxy_config WHERE app_type = 'claude'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read claude proxy config");
    assert_eq!(claude, (0, 0));

    let codex: (i64, i64) = conn
        .query_row(
            "SELECT enabled, auto_failover_enabled FROM proxy_config WHERE app_type = 'codex'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read codex proxy config");
    assert_eq!(codex, (1, 0));
}

#[test]
fn schema_migration_clears_current_failover_without_takeover() {
    let conn = Connection::open_in_memory().expect("open memory db");

    Database::create_tables_on_conn(&conn).expect("create tables");
    Database::set_user_version(&conn, SCHEMA_VERSION).expect("set current user_version");
    conn.execute(
        "UPDATE proxy_config
         SET enabled = 0, auto_failover_enabled = 1
         WHERE app_type = 'claude'",
        [],
    )
    .expect("seed invalid current proxy config");

    Database::apply_schema_migrations_on_conn(&conn).expect("apply migrations");

    let auto_failover_enabled: i64 = conn
        .query_row(
            "SELECT auto_failover_enabled FROM proxy_config WHERE app_type = 'claude'",
            [],
            |row| row.get(0),
        )
        .expect("read auto_failover_enabled");
    assert_eq!(auto_failover_enabled, 0);
}

#[test]
fn migration_from_v3_8_schema_v1_to_current_schema_v3() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute("PRAGMA foreign_keys = ON;", [])
        .expect("enable foreign keys");

    // 模拟 v3.8.* 用户的数据库（schema v1）
    conn.execute_batch(V3_8_SCHEMA_V1_SQL)
        .expect("seed v3.8 schema v1");
    Database::set_user_version(&conn, 1).expect("set user_version=1");

    // 插入一条旧版 Provider + Skill（用于验证迁移不会破坏既有数据）
    conn.execute(
        "INSERT INTO providers (
            id, app_type, name, settings_config, website_url, category,
            created_at, sort_index, notes, icon, icon_color, meta, is_current
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            "p1",
            "claude",
            "Test Provider",
            serde_json::to_string(&json!({ "anthropicApiKey": "sk-test" })).unwrap(),
            Option::<String>::None,
            Option::<String>::None,
            Option::<i64>::None,
            Option::<usize>::None,
            Option::<String>::None,
            Option::<String>::None,
            Option::<String>::None,
            "{}",
            1,
        ],
    )
    .expect("seed provider");

    conn.execute(
        "INSERT INTO skills (key, installed, installed_at) VALUES (?1, ?2, ?3)",
        params!["claude:demo-skill", 1, 1700000000i64],
    )
    .expect("seed legacy skill");

    // 按应用启动流程：先 create_tables（补齐新增表），再 apply_schema_migrations（按 user_version 迁移）
    Database::create_tables_on_conn(&conn).expect("create tables");
    Database::apply_schema_migrations_on_conn(&conn).expect("apply migrations");

    assert_eq!(
        Database::get_user_version(&conn).expect("user_version after migration"),
        SCHEMA_VERSION
    );

    // v1 -> v2：providers 新增字段必须补齐
    for column in [
        "cost_multiplier",
        "limit_daily_usd",
        "limit_monthly_usd",
        "provider_type",
        "in_failover_queue",
    ] {
        assert!(
            Database::has_column(&conn, "providers", column).expect("check column"),
            "providers.{column} should exist after migration"
        );
    }

    // 旧 provider 不应丢失，且新增字段应有默认值
    let provider_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM providers WHERE id = 'p1' AND app_type = 'claude'",
            [],
            |r| r.get(0),
        )
        .expect("count providers");
    assert_eq!(provider_count, 1);

    let cost_multiplier: String = conn
        .query_row(
            "SELECT cost_multiplier FROM providers WHERE id = 'p1' AND app_type = 'claude'",
            [],
            |r| r.get(0),
        )
        .expect("read cost_multiplier");
    assert_eq!(cost_multiplier, "1.0");

    // v2 -> v3：skills 表重建为统一结构，并设置 pending 标记（后续由启动时扫描文件系统重建数据）
    assert!(
        Database::has_column(&conn, "skills", "enabled_claude").expect("check skills v3 column"),
        "skills table should be migrated to v3 structure"
    );
    let skills_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM skills", [], |r| r.get(0))
        .expect("count skills");
    assert_eq!(skills_count, 0, "skills table should be rebuilt empty");

    let pending: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'skills_ssot_migration_pending'",
            [],
            |r| r.get(0),
        )
        .ok();
    assert!(
        matches!(pending.as_deref(), Some("true") | Some("1")),
        "skills_ssot_migration_pending should be set after v2->v3 migration"
    );

    // v3.9+ 新增：proxy_config 三行 seed 必须存在（否则 UI 会查不到默认值）
    let proxy_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM proxy_config", [], |r| r.get(0))
        .expect("count proxy_config rows");
    assert_eq!(proxy_rows, 3);

    // model_pricing 应具备默认数据（迁移时会 seed）
    let pricing_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM model_pricing", [], |r| r.get(0))
        .expect("count model_pricing rows");
    assert!(pricing_rows > 0, "model_pricing should be seeded");
}

#[test]
fn schema_migration_v10_adds_failover_live_snapshots() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute_batch(
        r#"
        CREATE TABLE providers (
            id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            name TEXT NOT NULL,
            settings_config TEXT NOT NULL,
            meta TEXT NOT NULL DEFAULT '{}',
            PRIMARY KEY (id, app_type)
        );
        CREATE TABLE mcp_servers (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            server_config TEXT NOT NULL,
            enabled_claude BOOLEAN NOT NULL DEFAULT 0,
            enabled_codex BOOLEAN NOT NULL DEFAULT 0,
            enabled_gemini BOOLEAN NOT NULL DEFAULT 0,
            enabled_opencode BOOLEAN NOT NULL DEFAULT 0,
            enabled_hermes BOOLEAN NOT NULL DEFAULT 0
        );
        CREATE TABLE skills (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            directory TEXT NOT NULL,
            enabled_claude BOOLEAN NOT NULL DEFAULT 0,
            enabled_codex BOOLEAN NOT NULL DEFAULT 0,
            enabled_gemini BOOLEAN NOT NULL DEFAULT 0,
            enabled_opencode BOOLEAN NOT NULL DEFAULT 0,
            enabled_hermes BOOLEAN NOT NULL DEFAULT 0,
            installed_at INTEGER NOT NULL DEFAULT 0,
            content_hash TEXT,
            updated_at INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
        CREATE TABLE proxy_config (app_type TEXT PRIMARY KEY);
        CREATE TABLE proxy_live_backup (
            app_type TEXT PRIMARY KEY,
            original_config TEXT NOT NULL,
            backed_up_at TEXT NOT NULL
        );
        "#,
    )
    .expect("seed v10 schema");
    Database::set_user_version(&conn, 10).expect("set user_version=10");

    Database::apply_schema_migrations_on_conn(&conn).expect("apply migrations");

    assert!(
        Database::table_exists(&conn, "proxy_failover_live_snapshots").expect("check table"),
        "proxy_failover_live_snapshots should exist after v10 migration"
    );
    assert_eq!(
        Database::get_user_version(&conn).expect("version after migration"),
        SCHEMA_VERSION
    );
}

#[test]
fn schema_dry_run_does_not_write_to_disk() {
    // Create minimal valid config for migration
    let mut apps = HashMap::new();
    apps.insert("claude".to_string(), ProviderManager::default());

    let config = MultiAppConfig {
        version: 2,
        apps,
        mcp: Default::default(),
        prompts: Default::default(),
        skills: Default::default(),
        common_config_snippets: Default::default(),
        claude_common_config_snippet: None,
    };

    // Dry-run should succeed without any file I/O errors
    let result = Database::migrate_from_json_dry_run(&config);
    assert!(
        result.is_ok(),
        "Dry-run should succeed with valid config: {result:?}"
    );
}

#[test]
fn dry_run_validates_schema_compatibility() {
    // Create config with actual provider data
    let mut providers = IndexMap::new();
    providers.insert(
        "test-provider".to_string(),
        Provider {
            id: "test-provider".to_string(),
            name: "Test Provider".to_string(),
            settings_config: json!({
                "anthropicApiKey": "sk-test-123",
            }),
            website_url: None,
            category: None,
            created_at: Some(1234567890),
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        },
    );

    let manager = ProviderManager {
        providers,
        current: "test-provider".to_string(),
    };

    let mut apps = HashMap::new();
    apps.insert("claude".to_string(), manager);

    let config = MultiAppConfig {
        version: 2,
        apps,
        mcp: Default::default(),
        prompts: Default::default(),
        skills: Default::default(),
        common_config_snippets: Default::default(),
        claude_common_config_snippet: None,
    };

    // Dry-run should validate the full migration path
    let result = Database::migrate_from_json_dry_run(&config);
    assert!(
        result.is_ok(),
        "Dry-run should succeed with provider data: {result:?}"
    );
}

#[test]
fn json_migration_imports_opencode_and_openclaw_prompts() {
    let mut config = MultiAppConfig::default();
    config.prompts.opencode.prompts.insert(
        "oc-prompt".to_string(),
        Prompt {
            id: "oc-prompt".to_string(),
            name: "OpenCode Prompt".to_string(),
            content: "opencode content".to_string(),
            description: None,
            enabled: true,
            created_at: Some(1),
            updated_at: Some(2),
        },
    );
    config.prompts.openclaw.prompts.insert(
        "claw-prompt".to_string(),
        Prompt {
            id: "claw-prompt".to_string(),
            name: "OpenClaw Prompt".to_string(),
            content: "openclaw content".to_string(),
            description: None,
            enabled: false,
            created_at: Some(3),
            updated_at: Some(4),
        },
    );

    let db = Database::memory().expect("create memory db");
    db.migrate_from_json(&config).expect("migrate prompts");

    let opencode = db.get_prompts("opencode").expect("load opencode prompts");
    let openclaw = db.get_prompts("openclaw").expect("load openclaw prompts");

    assert_eq!(
        opencode.get("oc-prompt").expect("opencode prompt").content,
        "opencode content"
    );
    assert_eq!(
        openclaw
            .get("claw-prompt")
            .expect("openclaw prompt")
            .content,
        "openclaw content"
    );
}

#[test]
fn schema_model_pricing_is_seeded_on_init() {
    let db = Database::memory().expect("create memory db");

    let conn = db.conn.lock().expect("lock conn");

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM model_pricing", [], |row| row.get(0))
        .expect("count pricing");

    assert!(
        count > 0,
        "模型定价数据应该在初始化时自动填充，实际数量: {}",
        count
    );

    // 验证包含 Claude 模型
    let claude_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM model_pricing WHERE model_id LIKE 'claude-%'",
            [],
            |row| row.get(0),
        )
        .expect("check claude");
    assert!(
        claude_count > 0,
        "应该包含 Claude 模型定价，实际数量: {}",
        claude_count
    );

    // 验证包含 GPT 模型
    let gpt_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM model_pricing WHERE model_id LIKE 'gpt-%'",
            [],
            |row| row.get(0),
        )
        .expect("check gpt");
    assert!(
        gpt_count > 0,
        "应该包含 GPT 模型定价，实际数量: {}",
        gpt_count
    );

    // 验证包含 Gemini 模型
    let gemini_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM model_pricing WHERE model_id LIKE 'gemini-%'",
            [],
            |row| row.get(0),
        )
        .expect("check gemini");
    assert!(
        gemini_count > 0,
        "应该包含 Gemini 模型定价，实际数量: {}",
        gemini_count
    );

    let deepseek_v3: (String, String, String) = conn
        .query_row(
            "SELECT input_cost_per_million, output_cost_per_million, cache_read_cost_per_million
             FROM model_pricing WHERE model_id = 'deepseek-v3'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("check deepseek-v3 pricing");
    assert_eq!(
        deepseek_v3,
        ("0.28".to_string(), "1.11".to_string(), "0.028".to_string()),
        "新建数据库也应使用修正后的 DeepSeek 定价"
    );
}

#[test]
#[serial_test::serial]
#[cfg(unix)]
fn init_creates_db_file_with_restrictive_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let _lock = crate::test_support::lock_test_home_and_settings();
    let temp = tempfile::tempdir().expect("create temp dir");
    let _guard = ConfigDirEnvGuard::set(temp.path());

    let _db = Database::init().expect("init db");

    let db_perms = std::fs::metadata(temp.path().join("cc-switch.db"))
        .expect("metadata db")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(db_perms, 0o600, "new db file should be created with 0o600");
}

#[test]
#[serial_test::serial]
#[cfg(unix)]
fn init_creates_config_dir_with_restrictive_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let _lock = crate::test_support::lock_test_home_and_settings();
    let temp = tempfile::tempdir().expect("create temp dir");
    let config_dir = temp.path().join("new-config-dir");
    let _guard = ConfigDirEnvGuard::set(&config_dir);

    let _db = Database::init().expect("init db");

    let dir_perms = std::fs::metadata(&config_dir)
        .expect("metadata dir")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_perms, 0o700, "new config dir should be 0o700");
}

#[test]
#[serial_test::serial]
#[cfg(unix)]
fn concurrent_init_on_fresh_config_dir_all_succeeds() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Barrier};
    use std::thread;

    let _lock = crate::test_support::lock_test_home_and_settings();
    let temp = tempfile::tempdir().expect("create temp dir");
    let config_dir = temp.path().join("fresh-config-dir");
    let _guard = ConfigDirEnvGuard::set(&config_dir);

    let thread_count = 8;
    let barrier = Arc::new(Barrier::new(thread_count));
    let mut handles = Vec::with_capacity(thread_count);

    for _ in 0..thread_count {
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            Database::init().map(|_| ())
        }));
    }

    for handle in handles {
        handle
            .join()
            .expect("init thread should not panic")
            .expect("concurrent init should succeed");
    }

    let conn =
        Connection::open(config_dir.join("cc-switch.db")).expect("open initialized database");
    let version = Database::get_user_version(&conn).expect("read schema version");
    assert_eq!(version, SCHEMA_VERSION);

    let lock_path = config_dir.join("cc-switch.db.init.lock");
    let lock_mode = std::fs::metadata(&lock_path)
        .expect("metadata init lock")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(lock_mode, 0o600, "init lock should be owner-only");
}

#[test]
#[serial_test::serial]
#[cfg(unix)]
fn init_rejects_symlinked_config_dir_without_writing_target() {
    use std::os::unix::fs::symlink;

    let _lock = crate::test_support::lock_test_home_and_settings();
    let temp = tempfile::tempdir().expect("create temp dir");
    let external_dir = temp.path().join("external");
    let config_link = temp.path().join("cc-switch-link");
    std::fs::create_dir(&external_dir).expect("create external dir");
    symlink(&external_dir, &config_link).expect("create config symlink");

    let _guard = ConfigDirEnvGuard::set(&config_link);
    let err = match Database::init() {
        Ok(_) => panic!("symlinked config dir should be rejected"),
        Err(err) => err,
    };

    assert!(
        err.to_string().contains("符号链接") || err.to_string().contains("symlink"),
        "unexpected error: {err}"
    );
    assert!(
        !external_dir.join("cc-switch.db").exists(),
        "init should not follow the config-dir symlink and create the database outside cc-switch"
    );
}

#[test]
#[serial_test::serial]
#[cfg(unix)]
fn init_rejects_symlinked_db_file_without_opening_target() {
    use std::os::unix::fs::symlink;

    let _lock = crate::test_support::lock_test_home_and_settings();
    let temp = tempfile::tempdir().expect("create temp dir");
    let config_dir = temp.path().join("cc-switch");
    let external_db = temp.path().join("external.db");
    std::fs::create_dir(&config_dir).expect("create config dir");
    std::fs::write(&external_db, b"not sqlite").expect("write external db");
    symlink(&external_db, config_dir.join("cc-switch.db")).expect("create db symlink");

    let _guard = ConfigDirEnvGuard::set(&config_dir);
    let err = match Database::init() {
        Ok(_) => panic!("symlinked db file should be rejected"),
        Err(err) => err,
    };

    assert!(
        err.to_string().contains("符号链接") || err.to_string().contains("symlink"),
        "unexpected error: {err}"
    );
    assert_eq!(
        std::fs::read(&external_db).expect("read external db"),
        b"not sqlite",
        "init should not open or modify the symlink target"
    );
}

#[test]
#[serial_test::serial]
#[cfg(unix)]
fn init_rejects_hardlinked_db_file_without_opening_target() {
    use std::os::unix::fs::MetadataExt;

    let _lock = crate::test_support::lock_test_home_and_settings();
    let temp = tempfile::tempdir().expect("create temp dir");
    let config_dir = temp.path().join("cc-switch");
    let external_db = temp.path().join("external.db");
    let linked_db = config_dir.join("cc-switch.db");
    std::fs::create_dir(&config_dir).expect("create config dir");
    std::fs::write(&external_db, b"not sqlite").expect("write external db");
    std::fs::hard_link(&external_db, &linked_db).expect("create db hardlink");
    assert_eq!(
        std::fs::metadata(&external_db)
            .expect("metadata external db")
            .nlink(),
        2,
        "test setup should create a multi-link inode"
    );

    let _guard = ConfigDirEnvGuard::set(&config_dir);
    let err = match Database::init() {
        Ok(_) => panic!("hardlinked db file should be rejected"),
        Err(err) => err,
    };

    assert!(
        err.to_string().contains("硬链接") || err.to_string().contains("hardlink"),
        "unexpected error: {err}"
    );
    assert_eq!(
        std::fs::read(&external_db).expect("read external db"),
        b"not sqlite",
        "init should not open or modify a hardlinked database target"
    );
}

#[test]
#[cfg(unix)]
fn create_secure_dir_all_rejects_unresolved_parent_dir_components_without_chmodding_parent() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("create temp dir");
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o755))
        .expect("set parent dir perms");

    let path = temp.path().join("child").join("..");
    let err = create_secure_dir_all(&path).expect_err("unresolved parent should be rejected");
    let message = err.to_string();

    assert!(
        message.contains("父目录组件") || message.contains("parent"),
        "unexpected error: {message}"
    );
    assert!(
        !temp.path().join("child").exists(),
        "rejected path should not create intermediate directories"
    );

    let parent_perms = std::fs::metadata(temp.path())
        .expect("metadata parent")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        parent_perms, 0o755,
        "rejected path should not chmod the parent directory"
    );
}

#[test]
#[serial_test::serial]
#[cfg(unix)]
fn init_rejects_parent_dir_config_path_even_when_child_exists() {
    let _lock = crate::test_support::lock_test_home_and_settings();
    let temp = tempfile::tempdir().expect("create temp dir");
    let parent = temp.path().join("parent");
    let child = parent.join("child");
    std::fs::create_dir_all(&child).expect("create child dir");

    let _guard = ConfigDirEnvGuard::set(&child.join(".."));
    if Database::init().is_ok() {
        panic!("parent-dir config path should be rejected");
    }

    assert!(
        !parent.join("cc-switch.db").exists(),
        "init must not normalize child/.. and create the database in the parent"
    );
}

#[test]
#[cfg(unix)]
fn create_secure_dir_all_rejects_symlink_component_without_writing_target() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().expect("create temp dir");
    let external_dir = temp.path().join("external");
    let link_dir = temp.path().join("link");
    std::fs::create_dir(&external_dir).expect("create external dir");
    symlink(&external_dir, &link_dir).expect("create symlink");

    let err = create_secure_dir_all(&link_dir.join("nested"))
        .expect_err("symlink components should be rejected");

    assert!(
        err.to_string().contains("符号链接") || err.to_string().contains("symlink"),
        "unexpected error: {err}"
    );
    assert!(
        !external_dir.join("nested").exists(),
        "rejected path should not create directories through the symlink target"
    );
}

#[test]
#[cfg(unix)]
fn create_secure_dir_all_accepts_existing_directory_after_create_race() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let dir = temp.path().join("existing");
    std::fs::create_dir(&dir).expect("create existing dir");

    assert!(
        !create_secure_dir_all(&dir).expect("existing directory should be accepted"),
        "existing directory should not be reported as newly created"
    );
}

#[test]
#[cfg(unix)]
fn backup_database_connection_rejects_symlink_path_without_writing_target() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().expect("create temp dir");
    let backup_path = temp.path().join("backup.db");
    let external_target = temp.path().join("external.db");
    symlink(&external_target, &backup_path).expect("create dangling backup symlink");

    let err = Database::create_backup_db_connection(&backup_path)
        .expect_err("backup connection should reject symlink path");

    assert!(
        err.to_string().contains("符号链接") || err.to_string().contains("symlink"),
        "unexpected error: {err}"
    );
    assert!(
        !external_target.exists(),
        "backup creation must not follow symlink target"
    );
}

#[test]
#[serial_test::serial]
#[cfg(unix)]
fn init_does_not_silently_fix_existing_dir_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let _lock = crate::test_support::lock_test_home_and_settings();
    let temp = tempfile::tempdir().expect("create temp dir");
    let _guard = ConfigDirEnvGuard::set(temp.path());

    // Set dir to a permissive mode before init
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o755))
        .expect("set dir perms");

    let _db = Database::init().expect("init db");

    let dir_perms = std::fs::metadata(temp.path())
        .expect("metadata dir")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        dir_perms, 0o755,
        "init should not silently change existing dir permissions"
    );
}
#[test]
fn model_pricing_delete_survives_reseed_until_user_upserts() {
    let db = Database::memory().expect("create memory db");

    assert!(db
        .delete_model_pricing("gpt-5.4")
        .expect("delete seeded pricing"));
    db.ensure_model_pricing_seeded()
        .expect("reseed after delete");

    {
        let conn = db.conn.lock().expect("lock conn");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM model_pricing WHERE model_id = 'gpt-5.4'",
                [],
                |row| row.get(0),
            )
            .expect("count deleted pricing");
        assert_eq!(count, 0, "deleted built-in pricing should stay hidden");
    }

    let restored = ModelPricingUpdate::new("gpt-5.4", "GPT 5.4", "2", "8", "0.2", "0")
        .expect("valid restored pricing");
    db.upsert_model_pricing(&restored).expect("restore pricing");
    db.ensure_model_pricing_seeded()
        .expect("reseed after restore");

    let conn = db.conn.lock().expect("lock conn");
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM model_pricing WHERE model_id = 'gpt-5.4'",
            [],
            |row| row.get(0),
        )
        .expect("count restored pricing");
    assert_eq!(count, 1, "manual upsert should clear delete marker");
}

#[test]
fn model_pricing_seeds_gpt_5_6_family_and_aliases() {
    let db = Database::memory().expect("create memory db");
    let conn = db.conn.lock().expect("lock conn");

    let expected = [
        ("gpt-5.6-sol", "5", "30", "0.50", "6.25"),
        ("gpt-5.6-terra", "2.50", "15", "0.25", "3.125"),
        ("gpt-5.6-luna", "1", "6", "0.10", "1.25"),
        ("gpt-5.6", "5", "30", "0.50", "6.25"),
        ("gpt-5.6-high", "5", "30", "0.50", "6.25"),
    ];

    for (model_id, input, output, cache_read, cache_write) in expected {
        let pricing: (String, String, String, String) = conn
            .query_row(
                "SELECT input_cost_per_million, output_cost_per_million,
                        cache_read_cost_per_million, cache_creation_cost_per_million
                 FROM model_pricing WHERE model_id = ?1",
                [model_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap_or_else(|error| panic!("query {model_id} pricing: {error}"));
        assert_eq!(
            pricing,
            (
                input.to_string(),
                output.to_string(),
                cache_read.to_string(),
                cache_write.to_string(),
            ),
            "unexpected pricing for {model_id}"
        );
    }
}

#[test]
fn model_pricing_repairs_only_untouched_upstream_gpt_5_6_seeds() {
    let db = Database::memory().expect("create memory db");
    {
        let conn = db.conn.lock().expect("lock conn");
        conn.execute(
            "UPDATE model_pricing SET cache_creation_cost_per_million = '0'
             WHERE model_id = 'gpt-5.6-sol'",
            [],
        )
        .expect("restore upstream zero cache-write seed");
        conn.execute(
            "UPDATE model_pricing
             SET input_cost_per_million = '9', cache_creation_cost_per_million = '0'
             WHERE model_id = 'gpt-5.6-terra'",
            [],
        )
        .expect("set custom Terra pricing");
    }

    db.ensure_model_pricing_seeded()
        .expect("ensure pricing seeded");

    let conn = db.conn.lock().expect("lock conn");
    let sol_cache_write: String = conn
        .query_row(
            "SELECT cache_creation_cost_per_million FROM model_pricing
             WHERE model_id = 'gpt-5.6-sol'",
            [],
            |row| row.get(0),
        )
        .expect("query repaired Sol pricing");
    assert_eq!(sol_cache_write, "6.25");

    let terra_custom: (String, String) = conn
        .query_row(
            "SELECT input_cost_per_million, cache_creation_cost_per_million
             FROM model_pricing WHERE model_id = 'gpt-5.6-terra'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("query custom Terra pricing");
    assert_eq!(terra_custom, ("9".to_string(), "0".to_string()));
}

#[test]
fn model_pricing_upsert_rejects_invalid_values() {
    let db = Database::memory().expect("create memory db");
    let invalid = ModelPricingUpdate {
        model_id: "bad-model".to_string(),
        display_name: "Bad Model".to_string(),
        input_cost_per_million: "not-a-number".to_string(),
        output_cost_per_million: "1".to_string(),
        cache_read_cost_per_million: "0".to_string(),
        cache_creation_cost_per_million: "0".to_string(),
    };

    assert!(db.upsert_model_pricing(&invalid).is_err());

    let conn = db.conn.lock().expect("lock conn");
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM model_pricing WHERE model_id = 'bad-model'",
            [],
            |row| row.get(0),
        )
        .expect("count invalid pricing");
    assert_eq!(count, 0, "invalid pricing should not be persisted");

    assert!(ModelPricingUpdate::new("bad-negative", "Bad Negative", "-1", "1", "0", "0").is_err());
    assert!(ModelPricingUpdate::new("", "Blank Model", "1", "1", "0", "0").is_err());
}

#[test]
fn should_auto_extract_config_snippet_respects_snippet_and_cleared_flag() {
    let db = Database::memory().expect("create memory db");

    // 全新状态：无片段、未清空 → 允许自动播种
    assert!(db
        .should_auto_extract_config_snippet("claude")
        .expect("gate"));
    assert!(!db.is_config_snippet_cleared("claude").expect("cleared"));

    // 有片段 → 不再自动播种
    db.set_config_snippet("claude", Some("{\"a\":1}".to_string()))
        .expect("set snippet");
    assert!(!db
        .should_auto_extract_config_snippet("claude")
        .expect("gate after set"));

    // 删除片段并标记为已清空 → 仍不自动播种
    db.set_config_snippet("claude", None)
        .expect("clear snippet");
    db.set_config_snippet_cleared("claude", true)
        .expect("set cleared");
    assert!(db
        .is_config_snippet_cleared("claude")
        .expect("cleared true"));
    assert!(!db
        .should_auto_extract_config_snippet("claude")
        .expect("gate after cleared"));

    // 取消清空标记 → 重新允许自动播种
    db.set_config_snippet_cleared("claude", false)
        .expect("unset cleared");
    assert!(!db
        .is_config_snippet_cleared("claude")
        .expect("cleared false"));
    assert!(db
        .should_auto_extract_config_snippet("claude")
        .expect("gate after unset"));
}

#[test]
fn memory_database_uses_incremental_auto_vacuum() {
    let db = Database::memory().expect("create memory db");
    let conn = db.conn.lock().expect("lock conn");
    assert_eq!(
        Database::get_auto_vacuum_mode(&conn).expect("read auto_vacuum"),
        2,
        "in-memory database should be configured with INCREMENTAL auto_vacuum"
    );
}

#[test]
fn ensure_incremental_auto_vacuum_rebuilds_existing_file_db() {
    let temp = tempfile::NamedTempFile::new().expect("create temp db file");
    let path = temp.path().to_path_buf();

    let conn = Connection::open(&path).expect("open temp db");
    conn.execute("PRAGMA auto_vacuum = NONE;", [])
        .expect("set none auto_vacuum");
    Database::create_tables_on_conn(&conn).expect("create tables");

    assert_eq!(
        Database::get_auto_vacuum_mode(&conn).expect("auto_vacuum before rebuild"),
        0,
        "existing file db should start with NONE auto_vacuum"
    );

    let rebuilt =
        Database::ensure_incremental_auto_vacuum_on_conn(&conn).expect("enable incremental mode");
    assert!(
        rebuilt,
        "existing db with tables should require a VACUUM rebuild"
    );
    drop(conn);

    let reopened = Connection::open(&path).expect("reopen temp db");
    assert_eq!(
        Database::get_auto_vacuum_mode(&reopened).expect("auto_vacuum after rebuild"),
        2,
        "file db should persist INCREMENTAL auto_vacuum after VACUUM rebuild"
    );
}

/// issue #327 回归：存量库被 prune 删行后不会自动收缩（auto_vacuum=NONE 下
/// `incremental_vacuum` 是空操作）；迁移到 INCREMENTAL 时的整库 VACUUM 应回收
/// 这些历史空闲页，避免 WebDAV 同步对超大库反复全量拷贝。
#[test]
fn ensure_incremental_auto_vacuum_reclaims_bloated_free_pages() {
    let temp = tempfile::NamedTempFile::new().expect("create temp db file");
    let path = temp.path().to_path_buf();

    let conn = Connection::open(&path).expect("open temp db");
    conn.execute("PRAGMA auto_vacuum = NONE;", [])
        .expect("set none auto_vacuum");
    conn.execute("CREATE TABLE logs(id INTEGER PRIMARY KEY, blob TEXT);", [])
        .expect("create logs table");

    conn.execute_batch("BEGIN;").expect("begin");
    {
        let mut stmt = conn
            .prepare("INSERT INTO logs(blob) VALUES (?1)")
            .expect("prepare insert");
        let payload = "x".repeat(2048);
        for _ in 0..4000 {
            stmt.execute([&payload]).expect("insert row");
        }
    }
    conn.execute_batch("COMMIT;").expect("commit");

    // 模拟 rollup_and_prune 删除历史明细行：空闲页仍留在文件里。
    conn.execute("DELETE FROM logs WHERE id > 100;", [])
        .expect("prune rows");
    // 旧路径：auto_vacuum=NONE 时 incremental_vacuum 是空操作，文件不收缩。
    conn.execute_batch("PRAGMA incremental_vacuum;")
        .expect("noop incremental vacuum");
    drop(conn);
    let bloated = std::fs::metadata(&path).expect("stat bloated").len();

    let conn = Connection::open(&path).expect("reopen temp db");
    let rebuilt = Database::ensure_incremental_auto_vacuum_on_conn(&conn)
        .expect("migrate to incremental auto_vacuum");
    assert!(rebuilt, "bloated existing db should be rebuilt via VACUUM");
    drop(conn);
    let reclaimed = std::fs::metadata(&path).expect("stat reclaimed").len();

    assert!(
        reclaimed < bloated / 2,
        "VACUUM migration should reclaim the majority of free pages \
         (bloated={bloated} bytes, reclaimed={reclaimed} bytes)"
    );
}

/// 耐久性守卫 Drop 恢复的是**进入守卫前**的 synchronous 值，而非硬编码 FULL：
/// 先把连接设为非默认的 OFF(0)，进入守卫期间应变为 NORMAL(1)，Drop 后必须
/// 精确恢复到 OFF(0)。
#[test]
fn bulk_import_guard_restores_prior_synchronous() {
    let db = Database::memory().expect("memory db");

    let read_sync = |db: &Database| -> i64 {
        let conn = db.conn.lock().expect("lock conn");
        conn.query_row("PRAGMA synchronous", [], |row| row.get::<_, i64>(0))
            .expect("read synchronous")
    };

    // 先把 synchronous 设为非默认值 OFF(0)。
    {
        let conn = db.conn.lock().expect("lock conn");
        conn.pragma_update(None, "synchronous", "OFF")
            .expect("set synchronous=OFF");
    }
    assert_eq!(read_sync(&db), 0, "预置为 OFF(0)");

    {
        let _guard = db.bulk_import_durability_guard();
        // 守卫期间降级为 NORMAL(1)。
        assert_eq!(read_sync(&db), 1, "守卫期间应为 NORMAL(1)");
    }

    // Drop 后恢复到进入前的原值 OFF(0)，而非硬编码 FULL(2)。
    assert_eq!(read_sync(&db), 0, "Drop 后应恢复原值 OFF(0)");
}
