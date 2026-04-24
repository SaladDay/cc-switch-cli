//! 数据库模块测试
//!
//! 包含 Schema 迁移和基本功能的测试。

use super::*;
use crate::app_config::MultiAppConfig;
use crate::provider::{Provider, ProviderManager};
use indexmap::IndexMap;
use rusqlite::{params, Connection};
use serde_json::json;
use std::collections::HashMap;

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
    assert!(
        err.to_string().contains("数据库版本过新"),
        "unexpected error: {err}"
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
        ("skill_repos", "token_env"),
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

    let token_env = get_column_info(&conn, "skill_repos", "token_env");
    assert_eq!(token_env.r#type, "TEXT");
    assert_eq!(token_env.notnull, 0);
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
fn schema_migration_v8_refreshes_model_pricing_and_reaches_v10() {
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
