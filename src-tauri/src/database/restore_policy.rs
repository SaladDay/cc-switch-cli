//! Typed, exhaustive restore policy for the CLI database.
//!
//! The untrusted database is never attached to, or published as, the live
//! database. Every portable row crosses this manifest as owned Rust values,
//! is validated against the storage class and runtime domain used by the
//! application, and is then inserted into a schema-owned canonical stage.

use crate::error::AppError;
use crate::provider::ProviderMeta;
use rusqlite::types::Value;
use rusqlite::Connection;
use rust_decimal::Decimal;
use serde::de::DeserializeOwned;
use std::collections::BTreeSet;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RestorePolicy {
    PortableIncoming,
    PreserveLive,
    RebuildRuntime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RestoreFlavor {
    UserRestore,
    Sync,
}

pub(super) fn effective_restore_policy(
    spec: &RestoreTableSpec,
    flavor: RestoreFlavor,
) -> RestorePolicy {
    if flavor == RestoreFlavor::UserRestore && matches!(spec.name, "skills" | "skill_repos") {
        RestorePolicy::PreserveLive
    } else {
        spec.policy
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StorageKind {
    Text,
    Integer,
    Real,
    /// A runtime numeric field which intentionally accepts both SQLite
    /// INTEGER and REAL storage classes.
    Number,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntegerDomain {
    Unrestricted,
    Boolean,
    NonNegative,
    SortIndex,
    Unsigned8,
    Unsigned16,
    NonNegativeI32,
    Unsigned32,
    InputTokenSemantics,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RealDomain {
    NotReal,
    FiniteNonNegative,
    FiniteUnitInterval,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RestoreColumnSpec {
    pub(super) name: &'static str,
    storage: StorageKind,
    nullable: bool,
    integer_domain: IntegerDomain,
    real_domain: RealDomain,
}

#[derive(Debug, Clone, Copy)]
enum RestoreRowValidator {
    OpaqueStorage,
    Skill,
    Provider,
    Mcp,
    ProxyConfig,
    JsonColumns(&'static [usize]),
    NonNegativeDecimalColumns(&'static [usize]),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RestoreTableSpec {
    pub(super) name: &'static str,
    pub(super) policy: RestorePolicy,
    pub(super) columns: &'static [RestoreColumnSpec],
    validator: RestoreRowValidator,
    /// Column index for a surrogate INTEGER PRIMARY KEY that has no incoming
    /// references and should be regenerated at each canonical boundary.
    regenerate_integer_primary_key: Option<usize>,
    parents: &'static [&'static str],
}

/// Portable user rows that historical schema migrations intentionally
/// regenerated. Restore captures them as validated Rust values before running
/// the old migration chain, then reapplies them afterward so migrations update
/// shape without silently replacing user choices.
pub(super) struct MigrationSensitiveUserData {
    model_pricing: Option<Vec<Vec<Value>>>,
}

macro_rules! text_col {
    ($name:literal) => {
        RestoreColumnSpec {
            name: $name,
            storage: StorageKind::Text,
            nullable: false,
            integer_domain: IntegerDomain::Unrestricted,
            real_domain: RealDomain::NotReal,
        }
    };
}

macro_rules! nullable_text_col {
    ($name:literal) => {
        RestoreColumnSpec {
            name: $name,
            storage: StorageKind::Text,
            nullable: true,
            integer_domain: IntegerDomain::Unrestricted,
            real_domain: RealDomain::NotReal,
        }
    };
}

macro_rules! integer_col {
    ($name:literal, $domain:ident) => {
        RestoreColumnSpec {
            name: $name,
            storage: StorageKind::Integer,
            nullable: false,
            integer_domain: IntegerDomain::$domain,
            real_domain: RealDomain::NotReal,
        }
    };
}

macro_rules! nullable_integer_col {
    ($name:literal, $domain:ident) => {
        RestoreColumnSpec {
            name: $name,
            storage: StorageKind::Integer,
            nullable: true,
            integer_domain: IntegerDomain::$domain,
            real_domain: RealDomain::NotReal,
        }
    };
}

macro_rules! real_col {
    ($name:literal, $domain:ident) => {
        RestoreColumnSpec {
            name: $name,
            storage: StorageKind::Real,
            nullable: false,
            integer_domain: IntegerDomain::Unrestricted,
            real_domain: RealDomain::$domain,
        }
    };
}

macro_rules! number_col {
    ($name:literal, $integer_domain:ident, $real_domain:ident) => {
        RestoreColumnSpec {
            name: $name,
            storage: StorageKind::Number,
            nullable: false,
            integer_domain: IntegerDomain::$integer_domain,
            real_domain: RealDomain::$real_domain,
        }
    };
}

const PROVIDERS_COLUMNS: &[RestoreColumnSpec] = &[
    text_col!("id"),
    text_col!("app_type"),
    text_col!("name"),
    text_col!("settings_config"),
    nullable_text_col!("website_url"),
    nullable_text_col!("category"),
    nullable_integer_col!("created_at", Unrestricted),
    nullable_integer_col!("sort_index", SortIndex),
    nullable_text_col!("notes"),
    nullable_text_col!("icon"),
    nullable_text_col!("icon_color"),
    text_col!("meta"),
    integer_col!("is_current", Boolean),
    integer_col!("in_failover_queue", Boolean),
    text_col!("cost_multiplier"),
    nullable_text_col!("limit_daily_usd"),
    nullable_text_col!("limit_monthly_usd"),
    nullable_text_col!("provider_type"),
];

const PROVIDER_ENDPOINTS_COLUMNS: &[RestoreColumnSpec] = &[
    integer_col!("id", Unrestricted),
    text_col!("provider_id"),
    text_col!("app_type"),
    text_col!("url"),
    nullable_integer_col!("added_at", Unrestricted),
];

const MCP_COLUMNS: &[RestoreColumnSpec] = &[
    text_col!("id"),
    text_col!("name"),
    text_col!("server_config"),
    nullable_text_col!("description"),
    nullable_text_col!("homepage"),
    nullable_text_col!("docs"),
    text_col!("tags"),
    integer_col!("enabled_claude", Boolean),
    integer_col!("enabled_codex", Boolean),
    integer_col!("enabled_gemini", Boolean),
    integer_col!("enabled_grokbuild", Boolean),
    integer_col!("enabled_opencode", Boolean),
    integer_col!("enabled_hermes", Boolean),
];

const PROMPTS_COLUMNS: &[RestoreColumnSpec] = &[
    text_col!("id"),
    text_col!("app_type"),
    text_col!("name"),
    text_col!("content"),
    nullable_text_col!("description"),
    integer_col!("enabled", Boolean),
    nullable_integer_col!("created_at", Unrestricted),
    nullable_integer_col!("updated_at", Unrestricted),
];

const SKILLS_COLUMNS: &[RestoreColumnSpec] = &[
    text_col!("id"),
    text_col!("name"),
    nullable_text_col!("description"),
    text_col!("directory"),
    nullable_text_col!("repo_owner"),
    nullable_text_col!("repo_name"),
    nullable_text_col!("repo_branch"),
    nullable_text_col!("readme_url"),
    integer_col!("enabled_claude", Boolean),
    integer_col!("enabled_codex", Boolean),
    integer_col!("enabled_gemini", Boolean),
    integer_col!("enabled_grokbuild", Boolean),
    integer_col!("enabled_opencode", Boolean),
    integer_col!("enabled_hermes", Boolean),
    integer_col!("installed_at", Unrestricted),
    nullable_text_col!("content_hash"),
    integer_col!("updated_at", Unrestricted),
];

const SKILL_REPOS_COLUMNS: &[RestoreColumnSpec] = &[
    text_col!("owner"),
    text_col!("name"),
    text_col!("branch"),
    integer_col!("enabled", Boolean),
];

const SETTINGS_COLUMNS: &[RestoreColumnSpec] = &[text_col!("key"), nullable_text_col!("value")];

const PROXY_CONFIG_COLUMNS: &[RestoreColumnSpec] = &[
    text_col!("app_type"),
    integer_col!("proxy_enabled", Boolean),
    text_col!("listen_address"),
    integer_col!("listen_port", Unsigned16),
    integer_col!("enable_logging", Boolean),
    integer_col!("enabled", Boolean),
    integer_col!("auto_failover_enabled", Boolean),
    integer_col!("max_retries", Unsigned8),
    integer_col!("streaming_first_byte_timeout", NonNegativeI32),
    integer_col!("streaming_idle_timeout", NonNegativeI32),
    integer_col!("non_streaming_timeout", NonNegativeI32),
    integer_col!("circuit_failure_threshold", NonNegativeI32),
    integer_col!("circuit_success_threshold", NonNegativeI32),
    integer_col!("circuit_timeout_seconds", NonNegativeI32),
    real_col!("circuit_error_rate_threshold", FiniteUnitInterval),
    integer_col!("circuit_min_requests", NonNegativeI32),
    text_col!("default_cost_multiplier"),
    text_col!("pricing_model_source"),
    integer_col!("live_takeover_active", Boolean),
    text_col!("created_at"),
    text_col!("updated_at"),
];

const PROVIDER_HEALTH_COLUMNS: &[RestoreColumnSpec] = &[
    text_col!("provider_id"),
    text_col!("app_type"),
    integer_col!("is_healthy", Boolean),
    integer_col!("consecutive_failures", Unsigned32),
    nullable_text_col!("last_success_at"),
    nullable_text_col!("last_failure_at"),
    nullable_text_col!("last_error"),
    text_col!("updated_at"),
];

const PROXY_REQUEST_LOG_COLUMNS: &[RestoreColumnSpec] = &[
    text_col!("request_id"),
    text_col!("provider_id"),
    text_col!("app_type"),
    text_col!("model"),
    integer_col!("input_tokens", Unsigned32),
    integer_col!("output_tokens", Unsigned32),
    integer_col!("cache_read_tokens", Unsigned32),
    integer_col!("cache_creation_tokens", Unsigned32),
    text_col!("input_cost_usd"),
    text_col!("output_cost_usd"),
    text_col!("cache_read_cost_usd"),
    text_col!("cache_creation_cost_usd"),
    text_col!("total_cost_usd"),
    integer_col!("latency_ms", NonNegative),
    nullable_integer_col!("first_token_ms", NonNegative),
    nullable_integer_col!("duration_ms", NonNegative),
    integer_col!("status_code", Unsigned16),
    nullable_text_col!("error_message"),
    nullable_text_col!("session_id"),
    nullable_text_col!("provider_type"),
    integer_col!("is_streaming", Boolean),
    text_col!("cost_multiplier"),
    integer_col!("created_at", Unrestricted),
    nullable_text_col!("request_model"),
    text_col!("data_source"),
    nullable_text_col!("pricing_model"),
    integer_col!("input_token_semantics", InputTokenSemantics),
];

const MODEL_PRICING_COLUMNS: &[RestoreColumnSpec] = &[
    text_col!("model_id"),
    text_col!("display_name"),
    text_col!("input_cost_per_million"),
    text_col!("output_cost_per_million"),
    text_col!("cache_read_cost_per_million"),
    text_col!("cache_creation_cost_per_million"),
];

const STREAM_CHECK_LOG_COLUMNS: &[RestoreColumnSpec] = &[
    integer_col!("id", Unrestricted),
    text_col!("provider_id"),
    text_col!("provider_name"),
    text_col!("app_type"),
    text_col!("status"),
    integer_col!("success", Boolean),
    text_col!("message"),
    nullable_integer_col!("response_time_ms", NonNegative),
    nullable_integer_col!("http_status", Unsigned16),
    nullable_text_col!("model_used"),
    nullable_integer_col!("retry_count", Unsigned32),
    integer_col!("tested_at", Unrestricted),
];

const PROXY_LIVE_BACKUP_COLUMNS: &[RestoreColumnSpec] = &[
    text_col!("app_type"),
    text_col!("original_config"),
    text_col!("backed_up_at"),
];

const PROXY_FAILOVER_SNAPSHOT_COLUMNS: &[RestoreColumnSpec] = &[
    text_col!("app_type"),
    text_col!("provider_id"),
    text_col!("config_json"),
    text_col!("generated_at"),
];

const USAGE_DAILY_ROLLUP_COLUMNS: &[RestoreColumnSpec] = &[
    text_col!("date"),
    text_col!("app_type"),
    text_col!("provider_id"),
    text_col!("model"),
    text_col!("request_model"),
    text_col!("pricing_model"),
    integer_col!("request_count", NonNegative),
    integer_col!("success_count", NonNegative),
    integer_col!("input_tokens", NonNegative),
    integer_col!("output_tokens", NonNegative),
    integer_col!("cache_read_tokens", NonNegative),
    integer_col!("cache_creation_tokens", NonNegative),
    integer_col!("input_token_semantics", InputTokenSemantics),
    text_col!("total_cost_usd"),
    // SQLite AVG intentionally persists fractional values such as 1.5 even
    // though the historical declaration has INTEGER affinity.
    number_col!("avg_latency_ms", NonNegative, FiniteNonNegative),
];

const PROFILES_COLUMNS: &[RestoreColumnSpec] = &[
    text_col!("id"),
    text_col!("name"),
    text_col!("payload"),
    nullable_integer_col!("sort_order", Unrestricted),
    nullable_integer_col!("created_at", Unrestricted),
    nullable_integer_col!("updated_at", Unrestricted),
];

const SESSION_LOG_SYNC_COLUMNS: &[RestoreColumnSpec] = &[
    text_col!("file_path"),
    integer_col!("last_modified", Unrestricted),
    integer_col!("last_line_offset", NonNegative),
    integer_col!("last_synced_at", Unrestricted),
];

/// Parent-before-child order is also the canonical insert order. This is the
/// complete CLI manifest; the coverage assertion fails if schema.rs gains or
/// loses a user table without an explicit policy decision here.
pub(super) const RESTORE_TABLE_SPECS: &[RestoreTableSpec] = &[
    RestoreTableSpec {
        name: "providers",
        policy: RestorePolicy::PortableIncoming,
        columns: PROVIDERS_COLUMNS,
        validator: RestoreRowValidator::Provider,
        regenerate_integer_primary_key: None,
        parents: &[],
    },
    RestoreTableSpec {
        name: "provider_endpoints",
        policy: RestorePolicy::PortableIncoming,
        columns: PROVIDER_ENDPOINTS_COLUMNS,
        validator: RestoreRowValidator::OpaqueStorage,
        regenerate_integer_primary_key: Some(0),
        parents: &["providers"],
    },
    RestoreTableSpec {
        name: "mcp_servers",
        policy: RestorePolicy::PortableIncoming,
        columns: MCP_COLUMNS,
        validator: RestoreRowValidator::Mcp,
        regenerate_integer_primary_key: None,
        parents: &[],
    },
    RestoreTableSpec {
        name: "prompts",
        policy: RestorePolicy::PortableIncoming,
        columns: PROMPTS_COLUMNS,
        validator: RestoreRowValidator::OpaqueStorage,
        regenerate_integer_primary_key: None,
        parents: &[],
    },
    RestoreTableSpec {
        name: "skills",
        policy: RestorePolicy::PortableIncoming,
        columns: SKILLS_COLUMNS,
        validator: RestoreRowValidator::Skill,
        regenerate_integer_primary_key: None,
        parents: &[],
    },
    RestoreTableSpec {
        name: "skill_repos",
        policy: RestorePolicy::PortableIncoming,
        columns: SKILL_REPOS_COLUMNS,
        validator: RestoreRowValidator::OpaqueStorage,
        regenerate_integer_primary_key: None,
        parents: &[],
    },
    RestoreTableSpec {
        name: "settings",
        policy: RestorePolicy::PortableIncoming,
        columns: SETTINGS_COLUMNS,
        validator: RestoreRowValidator::OpaqueStorage,
        regenerate_integer_primary_key: None,
        parents: &[],
    },
    RestoreTableSpec {
        name: "proxy_config",
        policy: RestorePolicy::PortableIncoming,
        columns: PROXY_CONFIG_COLUMNS,
        validator: RestoreRowValidator::ProxyConfig,
        regenerate_integer_primary_key: None,
        parents: &[],
    },
    RestoreTableSpec {
        name: "provider_health",
        policy: RestorePolicy::RebuildRuntime,
        columns: PROVIDER_HEALTH_COLUMNS,
        validator: RestoreRowValidator::OpaqueStorage,
        regenerate_integer_primary_key: None,
        parents: &["providers"],
    },
    RestoreTableSpec {
        name: "proxy_request_logs",
        policy: RestorePolicy::PortableIncoming,
        columns: PROXY_REQUEST_LOG_COLUMNS,
        validator: RestoreRowValidator::NonNegativeDecimalColumns(&[8, 9, 10, 11, 12, 21]),
        regenerate_integer_primary_key: None,
        parents: &[],
    },
    RestoreTableSpec {
        name: "model_pricing",
        policy: RestorePolicy::PortableIncoming,
        columns: MODEL_PRICING_COLUMNS,
        validator: RestoreRowValidator::NonNegativeDecimalColumns(&[2, 3, 4, 5]),
        regenerate_integer_primary_key: None,
        parents: &[],
    },
    RestoreTableSpec {
        name: "stream_check_logs",
        policy: RestorePolicy::PortableIncoming,
        columns: STREAM_CHECK_LOG_COLUMNS,
        validator: RestoreRowValidator::OpaqueStorage,
        regenerate_integer_primary_key: Some(0),
        parents: &[],
    },
    RestoreTableSpec {
        name: "proxy_live_backup",
        policy: RestorePolicy::PortableIncoming,
        columns: PROXY_LIVE_BACKUP_COLUMNS,
        validator: RestoreRowValidator::JsonColumns(&[1]),
        regenerate_integer_primary_key: None,
        parents: &[],
    },
    RestoreTableSpec {
        name: "proxy_failover_live_snapshots",
        policy: RestorePolicy::RebuildRuntime,
        columns: PROXY_FAILOVER_SNAPSHOT_COLUMNS,
        validator: RestoreRowValidator::JsonColumns(&[2]),
        regenerate_integer_primary_key: None,
        parents: &["providers"],
    },
    RestoreTableSpec {
        name: "usage_daily_rollups",
        policy: RestorePolicy::PortableIncoming,
        columns: USAGE_DAILY_ROLLUP_COLUMNS,
        validator: RestoreRowValidator::NonNegativeDecimalColumns(&[13]),
        regenerate_integer_primary_key: None,
        parents: &[],
    },
    RestoreTableSpec {
        name: "profiles",
        policy: RestorePolicy::PortableIncoming,
        // The CLI deliberately treats the upstream profile payload as opaque
        // user data; validating it against a GUI-only contract would create a
        // false compatibility boundary.
        columns: PROFILES_COLUMNS,
        validator: RestoreRowValidator::OpaqueStorage,
        regenerate_integer_primary_key: None,
        parents: &[],
    },
    RestoreTableSpec {
        name: "session_log_sync",
        policy: RestorePolicy::PreserveLive,
        columns: SESSION_LOG_SYNC_COLUMNS,
        validator: RestoreRowValidator::OpaqueStorage,
        regenerate_integer_primary_key: None,
        parents: &[],
    },
];

pub(super) const SYNC_LIVE_OVERLAY_TABLES: &[&str] = &[
    "proxy_request_logs",
    "stream_check_logs",
    "proxy_live_backup",
    "usage_daily_rollups",
];

pub(super) const SYNC_LOCAL_SETTINGS_KEYS: &[&str] = &["proxy_runtime_session"];
pub(super) const USER_RESTORE_LOCAL_SETTINGS_KEYS: &[&str] = &["skills_ssot_migration_pending"];

pub(super) const PROXY_CONFIG_LOCAL_COLUMNS: &[&str] =
    &["proxy_enabled", "listen_address", "listen_port", "enabled"];

pub(super) fn should_skip_sync_export(table: &str) -> bool {
    let policy = RESTORE_TABLE_SPECS
        .iter()
        .find(|spec| spec.name == table)
        .map(|spec| spec.policy);
    matches!(
        policy,
        Some(RestorePolicy::PreserveLive | RestorePolicy::RebuildRuntime)
    ) || SYNC_LIVE_OVERLAY_TABLES.contains(&table)
}

pub(super) fn table_spec(name: &str) -> Result<&'static RestoreTableSpec, AppError> {
    RESTORE_TABLE_SPECS
        .iter()
        .find(|spec| spec.name == name)
        .ok_or_else(|| AppError::Database(format!("restore policy has no table '{name}'")))
}

pub(super) fn capture_migration_sensitive_user_data(
    connection: &Connection,
) -> Result<MigrationSensitiveUserData, AppError> {
    let spec = table_spec("model_pricing")?;
    let model_pricing = if table_has_all_columns(connection, spec)? {
        Some(read_fixed_rows(connection, spec)?)
    } else {
        None
    };
    Ok(MigrationSensitiveUserData { model_pricing })
}

pub(super) fn restore_migration_sensitive_user_data(
    connection: &Connection,
    snapshot: MigrationSensitiveUserData,
) -> Result<(), AppError> {
    if let Some(rows) = snapshot.model_pricing {
        replace_fixed_rows(connection, table_spec("model_pricing")?, &rows)?;
    }
    Ok(())
}

pub(super) fn quoted_columns(spec: &RestoreTableSpec) -> String {
    spec.columns
        .iter()
        .map(|column| quote_identifier(column.name))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

pub(super) fn read_and_validate_row(
    row: &rusqlite::Row<'_>,
    spec: &RestoreTableSpec,
) -> Result<Vec<Value>, AppError> {
    let values = (0..spec.columns.len())
        .map(|index| row.get::<_, Value>(index))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| invalid_candidate(error.to_string()))?;
    validate_restore_row(spec, &values)?;
    Ok(values)
}

pub(super) fn validate_column_value(
    table: &str,
    column: &RestoreColumnSpec,
    value: &Value,
) -> Result<(), AppError> {
    validate_storage(table, column, value)?;
    validate_integer_domain(table, column, value)?;
    validate_real_domain(table, column, value)
}

pub(super) fn copy_fixed_table(
    source: &Connection,
    target: &Connection,
    spec: &RestoreTableSpec,
    clear_target: bool,
) -> Result<u64, AppError> {
    if clear_target {
        target
            .execute(&format!("DELETE FROM {}", quote_identifier(spec.name)), [])
            .map_err(database_error)?;
    }

    let columns = quoted_columns(spec);
    let select = format!("SELECT {columns} FROM {}", quote_identifier(spec.name));
    let placeholders = (1..=spec.columns.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let insert = format!(
        "INSERT INTO {} ({columns}) VALUES ({placeholders})",
        quote_identifier(spec.name)
    );

    let mut statement = source.prepare(&select).map_err(|error| {
        invalid_candidate(format!(
            "required column is missing from table {:?}: {error}",
            spec.name
        ))
    })?;
    let mut rows = statement.query([]).map_err(database_error)?;
    let mut copied = 0_u64;
    let mut skill_directories = std::collections::BTreeMap::new();
    while let Some(row) = rows.next().map_err(database_error)? {
        let mut values = read_and_validate_row(row, spec)?;
        record_skill_directory(spec, &values, &mut skill_directories)?;
        if let Some(index) = spec.regenerate_integer_primary_key {
            values[index] = Value::Null;
        }
        target
            .execute(&insert, rusqlite::params_from_iter(values.iter()))
            .map_err(|error| {
                invalid_candidate(format!(
                    "canonical insert into {:?} failed: {error}",
                    spec.name
                ))
            })?;
        copied = copied.saturating_add(1);
    }
    Ok(copied)
}

fn read_fixed_rows(
    source: &Connection,
    spec: &RestoreTableSpec,
) -> Result<Vec<Vec<Value>>, AppError> {
    let columns = quoted_columns(spec);
    let mut statement = source
        .prepare(&format!(
            "SELECT {columns} FROM {}",
            quote_identifier(spec.name)
        ))
        .map_err(database_error)?;
    let mut rows = statement.query([]).map_err(database_error)?;
    let mut captured = Vec::new();
    while let Some(row) = rows.next().map_err(database_error)? {
        captured.push(read_and_validate_row(row, spec)?);
    }
    Ok(captured)
}

fn replace_fixed_rows(
    target: &Connection,
    spec: &RestoreTableSpec,
    rows: &[Vec<Value>],
) -> Result<(), AppError> {
    let columns = quoted_columns(spec);
    let placeholders = (1..=spec.columns.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let insert = format!(
        "INSERT INTO {} ({columns}) VALUES ({placeholders})",
        quote_identifier(spec.name)
    );
    target
        .execute(&format!("DELETE FROM {}", quote_identifier(spec.name)), [])
        .map_err(database_error)?;
    for values in rows {
        validate_restore_row(spec, values)?;
        target
            .execute(&insert, rusqlite::params_from_iter(values.iter()))
            .map_err(database_error)?;
    }
    Ok(())
}

fn table_has_all_columns(
    connection: &Connection,
    spec: &RestoreTableSpec,
) -> Result<bool, AppError> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_schema
                WHERE type = 'table' AND name = ?1
             )",
            [spec.name],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if !exists {
        return Ok(false);
    }
    let mut statement = connection
        .prepare(&format!(
            "PRAGMA table_info({})",
            quote_identifier(spec.name)
        ))
        .map_err(database_error)?;
    let actual = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(database_error)?
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(database_error)?;
    Ok(spec
        .columns
        .iter()
        .all(|column| actual.contains(column.name)))
}

pub(super) fn count_rows(
    connection: &Connection,
    spec: &RestoreTableSpec,
) -> Result<u64, AppError> {
    let count: i64 = connection
        .query_row(
            &format!("SELECT COUNT(*) FROM {}", quote_identifier(spec.name)),
            [],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    u64::try_from(count)
        .map_err(|_| AppError::Database(format!("negative row count for {:?}", spec.name)))
}

pub(super) fn validate_canonical_stage(connection: &Connection) -> Result<(), AppError> {
    assert_restore_policy_topology()?;
    let policy_tables = RESTORE_TABLE_SPECS
        .iter()
        .map(|spec| spec.name.to_string())
        .collect::<BTreeSet<_>>();
    assert_restore_policy_coverage(&canonical_user_tables(connection)?, &policy_tables)?;
    validate_exact_column_coverage(connection)?;
    validate_stage_rows(connection)?;

    let integrity: String = connection
        .query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))
        .map_err(database_error)?;
    if integrity != "ok" {
        return Err(AppError::Database(format!(
            "canonical integrity_check failed: {integrity}"
        )));
    }

    let mut foreign_keys = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(database_error)?;
    if foreign_keys
        .query([])
        .map_err(database_error)?
        .next()
        .map_err(database_error)?
        .is_some()
    {
        return Err(invalid_candidate(
            "canonical foreign_key_check reported an orphan row".to_string(),
        ));
    }
    Ok(())
}

fn validate_storage(
    table: &str,
    column: &RestoreColumnSpec,
    value: &Value,
) -> Result<(), AppError> {
    let valid = match value {
        Value::Null => column.nullable,
        Value::Text(_) => column.storage == StorageKind::Text,
        Value::Integer(_) => matches!(column.storage, StorageKind::Integer | StorageKind::Number),
        Value::Real(_) => matches!(column.storage, StorageKind::Real | StorageKind::Number),
        Value::Blob(_) => false,
    };
    if valid {
        Ok(())
    } else {
        Err(invalid_candidate(format!(
            "invalid storage class for {table}.{}",
            column.name
        )))
    }
}

fn validate_integer_domain(
    table: &str,
    column: &RestoreColumnSpec,
    value: &Value,
) -> Result<(), AppError> {
    let Value::Integer(value) = value else {
        return Ok(());
    };
    let valid = match column.integer_domain {
        IntegerDomain::Unrestricted => true,
        IntegerDomain::Boolean => matches!(*value, 0 | 1),
        IntegerDomain::NonNegative => *value >= 0,
        // Provider order is operational metadata, not a sparse identifier.
        // Keep enough headroom for every realistic list while guaranteeing
        // duplicate/append paths can increment without i64/usize overflow.
        IntegerDomain::SortIndex => (0..=i32::MAX as i64).contains(value),
        IntegerDomain::Unsigned8 => (0..=u8::MAX as i64).contains(value),
        IntegerDomain::Unsigned16 => (0..=u16::MAX as i64).contains(value),
        IntegerDomain::NonNegativeI32 => (0..=i32::MAX as i64).contains(value),
        IntegerDomain::Unsigned32 => (0..=u32::MAX as i64).contains(value),
        IntegerDomain::InputTokenSemantics => (0..=2).contains(value),
    };
    if valid {
        Ok(())
    } else {
        Err(invalid_candidate(format!(
            "out-of-domain integer {value} at {table}.{} ({:?})",
            column.name, column.integer_domain
        )))
    }
}

fn validate_real_domain(
    table: &str,
    column: &RestoreColumnSpec,
    value: &Value,
) -> Result<(), AppError> {
    let Value::Real(value) = value else {
        return Ok(());
    };
    let valid = match column.real_domain {
        RealDomain::NotReal => false,
        RealDomain::FiniteNonNegative => value.is_finite() && *value >= 0.0,
        RealDomain::FiniteUnitInterval => value.is_finite() && (0.0..=1.0).contains(value),
    };
    if valid {
        Ok(())
    } else {
        Err(invalid_candidate(format!(
            "out-of-domain real {value} at {table}.{} ({:?})",
            column.name, column.real_domain
        )))
    }
}

pub(super) fn validate_restore_row(
    spec: &RestoreTableSpec,
    values: &[Value],
) -> Result<(), AppError> {
    if values.len() != spec.columns.len() {
        return Err(AppError::Database(format!(
            "restore decoder width mismatch for {:?}",
            spec.name
        )));
    }
    for (column, value) in spec.columns.iter().zip(values) {
        validate_column_value(spec.name, column, value)?;
    }

    match spec.validator {
        RestoreRowValidator::OpaqueStorage => {}
        RestoreRowValidator::Skill => {
            let directory = text_value(spec.name, "directory", &values[3])?;
            crate::skill_directory::SkillDirectory::parse(directory).map_err(|error| {
                invalid_candidate(format!(
                    "invalid Skill directory component {directory:?}: {}",
                    error.reason()
                ))
            })?;
        }
        RestoreRowValidator::Provider => {
            validate_json::<serde_json::Value>(spec.name, "settings_config", &values[3])?;
            validate_json::<ProviderMeta>(spec.name, "meta", &values[11])?;
            validate_non_negative_decimal(spec.name, "cost_multiplier", &values[14])?;
            for index in [15_usize, 16] {
                if !matches!(values[index], Value::Null) {
                    validate_non_negative_decimal(
                        spec.name,
                        spec.columns[index].name,
                        &values[index],
                    )?;
                }
            }
        }
        RestoreRowValidator::Mcp => {
            validate_json::<serde_json::Value>(spec.name, "server_config", &values[2])?;
            validate_json::<Vec<String>>(spec.name, "tags", &values[6])?;
        }
        RestoreRowValidator::ProxyConfig => {
            let listen_address = text_value(spec.name, "listen_address", &values[2])?;
            crate::cli::proxy_settings::validate_proxy_listen_address(listen_address).map_err(
                |error| {
                    invalid_candidate(format!(
                        "invalid proxy listen address {listen_address:?}: {error}"
                    ))
                },
            )?;
            let Value::Integer(listen_port) = &values[3] else {
                return Err(invalid_candidate(
                    "integer required at proxy_config.listen_port".to_string(),
                ));
            };
            let listen_port = u16::try_from(*listen_port).map_err(|error| {
                invalid_candidate(format!("invalid proxy listen port {listen_port}: {error}"))
            })?;
            crate::cli::proxy_settings::validate_proxy_listen_port(listen_port).map_err(
                |error| {
                    invalid_candidate(format!("invalid proxy listen port {listen_port}: {error}"))
                },
            )?;
            validate_non_negative_decimal(spec.name, "default_cost_multiplier", &values[16])?;
            let pricing_source = text_value(spec.name, "pricing_model_source", &values[17])?;
            if !matches!(pricing_source, "request" | "response") {
                return Err(invalid_candidate(format!(
                    "unsupported pricing model source {pricing_source:?}"
                )));
            }
        }
        RestoreRowValidator::JsonColumns(indices) => {
            for index in indices {
                validate_json::<serde_json::Value>(
                    spec.name,
                    spec.columns[*index].name,
                    &values[*index],
                )?;
            }
        }
        RestoreRowValidator::NonNegativeDecimalColumns(indices) => {
            for index in indices {
                validate_non_negative_decimal(
                    spec.name,
                    spec.columns[*index].name,
                    &values[*index],
                )?;
            }
        }
    }
    Ok(())
}

fn text_value<'a>(table: &str, column: &str, value: &'a Value) -> Result<&'a str, AppError> {
    match value {
        Value::Text(value) => Ok(value),
        _ => Err(invalid_candidate(format!(
            "text required at {table}.{column}"
        ))),
    }
}

fn validate_json<T: DeserializeOwned>(
    table: &str,
    column: &str,
    value: &Value,
) -> Result<(), AppError> {
    let text = text_value(table, column, value)?;
    serde_json::from_str::<T>(text)
        .map(|_| ())
        .map_err(|error| invalid_candidate(format!("invalid JSON at {table}.{column}: {error}")))
}

fn validate_non_negative_decimal(table: &str, column: &str, value: &Value) -> Result<(), AppError> {
    let value = text_value(table, column, value)?;
    let parsed = Decimal::from_str(value).map_err(|error| {
        invalid_candidate(format!("invalid decimal at {table}.{column}: {error}"))
    })?;
    if parsed < Decimal::ZERO {
        return Err(invalid_candidate(format!(
            "negative decimal at {table}.{column}: {value}"
        )));
    }
    Ok(())
}

fn validate_stage_rows(connection: &Connection) -> Result<(), AppError> {
    for spec in RESTORE_TABLE_SPECS {
        let columns = quoted_columns(spec);
        let mut statement = connection
            .prepare(&format!(
                "SELECT {columns} FROM {}",
                quote_identifier(spec.name)
            ))
            .map_err(database_error)?;
        let mut rows = statement.query([]).map_err(database_error)?;
        let mut skill_directories = std::collections::BTreeMap::new();
        while let Some(row) = rows.next().map_err(database_error)? {
            let values = read_and_validate_row(row, spec)?;
            record_skill_directory(spec, &values, &mut skill_directories)?;
        }
    }
    Ok(())
}

pub(super) fn record_skill_directory(
    spec: &RestoreTableSpec,
    values: &[Value],
    observed: &mut std::collections::BTreeMap<String, String>,
) -> Result<(), AppError> {
    if !matches!(spec.validator, RestoreRowValidator::Skill) {
        return Ok(());
    }
    let id = text_value(spec.name, "id", &values[0])?;
    let directory = text_value(spec.name, "directory", &values[3])?;
    let directory = crate::skill_directory::SkillDirectory::parse(directory).map_err(|error| {
        invalid_candidate(format!(
            "invalid Skill directory component {directory:?}: {}",
            error.reason()
        ))
    })?;
    let key = directory.collision_key();
    if let Some(previous) = observed.insert(key, id.to_string()) {
        return Err(invalid_candidate(format!(
            "Skills {previous:?} and {id:?} have directories that collide across supported filesystems"
        )));
    }
    Ok(())
}

fn assert_restore_policy_topology() -> Result<(), AppError> {
    let mut seen = BTreeSet::new();
    for spec in RESTORE_TABLE_SPECS {
        if !seen.insert(spec.name) {
            return Err(AppError::Database(format!(
                "duplicate restore policy for {:?}",
                spec.name
            )));
        }
        for column in spec.columns {
            if !matches!(column.storage, StorageKind::Integer | StorageKind::Number)
                && column.integer_domain != IntegerDomain::Unrestricted
            {
                return Err(AppError::Database(format!(
                    "non-integer restore column {:?}.{:?} declares an integer domain",
                    spec.name, column.name
                )));
            }
            if matches!(column.storage, StorageKind::Real | StorageKind::Number)
                && column.real_domain == RealDomain::NotReal
            {
                return Err(AppError::Database(format!(
                    "real restore column {:?}.{:?} has no finite domain",
                    spec.name, column.name
                )));
            }
            if !matches!(column.storage, StorageKind::Real | StorageKind::Number)
                && column.real_domain != RealDomain::NotReal
            {
                return Err(AppError::Database(format!(
                    "non-real restore column {:?}.{:?} declares a real domain",
                    spec.name, column.name
                )));
            }
        }
        for parent in spec.parents {
            if !seen.contains(parent) {
                return Err(AppError::Database(format!(
                    "restore policy {:?} precedes parent {parent:?}",
                    spec.name
                )));
            }
        }
    }
    Ok(())
}

fn canonical_user_tables(connection: &Connection) -> Result<BTreeSet<String>, AppError> {
    let mut statement = connection
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )
        .map_err(database_error)?;
    let tables = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(database_error)?
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(database_error)?;
    Ok(tables)
}

fn assert_restore_policy_coverage(
    canonical_tables: &BTreeSet<String>,
    policy_tables: &BTreeSet<String>,
) -> Result<(), AppError> {
    if canonical_tables == policy_tables {
        return Ok(());
    }
    let missing_policy = canonical_tables
        .difference(policy_tables)
        .cloned()
        .collect::<Vec<_>>();
    let stale_policy = policy_tables
        .difference(canonical_tables)
        .cloned()
        .collect::<Vec<_>>();
    Err(AppError::Database(format!(
        "canonical user tables and restore policy manifest differ; \
         missing policy={missing_policy:?}, stale policy={stale_policy:?}"
    )))
}

fn validate_exact_column_coverage(connection: &Connection) -> Result<(), AppError> {
    for spec in RESTORE_TABLE_SPECS {
        let mut statement = connection
            .prepare(&format!(
                "PRAGMA table_info({})",
                quote_identifier(spec.name)
            ))
            .map_err(database_error)?;
        let actual = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(database_error)?
            .collect::<Result<BTreeSet<_>, _>>()
            .map_err(database_error)?;
        let expected = spec
            .columns
            .iter()
            .map(|column| column.name.to_string())
            .collect::<BTreeSet<_>>();
        if actual != expected {
            return Err(AppError::Database(format!(
                "restore columns for {:?} differ from schema.rs; expected={expected:?}, actual={actual:?}",
                spec.name
            )));
        }
    }
    Ok(())
}

fn database_error(error: rusqlite::Error) -> AppError {
    AppError::Database(error.to_string())
}

fn invalid_candidate(reason: String) -> AppError {
    AppError::localized(
        "backup.sql.invalid_data",
        format!("SQL 备份包含 CC Switch 无法安全读取的数据：{reason}"),
        format!("The SQL backup contains data CC Switch cannot safely read: {reason}"),
    )
}

#[cfg(test)]
mod tests {
    use super::{validate_canonical_stage, RESTORE_TABLE_SPECS};
    use crate::database::Database;
    use crate::error::AppError;

    #[test]
    fn restore_policy_manifest_covers_every_cli_table() -> Result<(), AppError> {
        assert_eq!(
            RESTORE_TABLE_SPECS.len(),
            17,
            "the CLI restore manifest must explicitly classify all 17 tables"
        );
        let database = Database::memory()?;
        let connection = crate::database::lock_conn!(database.conn);
        validate_canonical_stage(&connection)
    }
}
