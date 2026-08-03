//! Exact source recognition for untrusted v13-v16 restores.
//!
//! This is adapted from the restore trust-boundary patchset pinned at upstream
//! commit `3fa6b1f1`. The CLI intentionally keeps its own v13 compatibility
//! contract, proven by the fixture generated at CLI commit `3c3a7f9`.
//!
//! Recognition happens before any current-schema DDL. A declared version
//! selects an exact table/column inventory; `table_xinfo` keeps generated and
//! hidden columns inside that inventory. Historical rows are decoded as owned
//! Rust `Value`s and validated before a migration may apply SQLite affinity.

use super::restore_policy::{
    quote_identifier, record_skill_directory, table_spec, validate_restore_row, RestoreTableSpec,
};
use super::{Database, SCHEMA_VERSION};
use crate::error::AppError;
use rusqlite::types::Value;
use rusqlite::Connection;
use std::collections::{BTreeMap, BTreeSet};

const EARLIEST_RESTORE_VERSION: i32 = 13;
const LATEST_DECLARED_SOURCE_VERSION: i32 = 16;

/// This explicit inventory is the reviewed CLI restore surface. It must not be
/// inferred from sqlite_schema or the current schema factory.
const SOURCE_TABLE_NAMES: &[&str] = &[
    "mcp_servers",
    "model_pricing",
    "profiles",
    "prompts",
    "provider_endpoints",
    "provider_health",
    "providers",
    "proxy_config",
    "proxy_failover_live_snapshots",
    "proxy_live_backup",
    "proxy_request_logs",
    "session_log_sync",
    "settings",
    "skill_repos",
    "skills",
    "stream_check_logs",
    "usage_daily_rollups",
];

const PRE_V15_EXCLUDED_COLUMNS: &[(&str, &str)] = &[
    ("mcp_servers", "enabled_grokbuild"),
    ("skills", "enabled_grokbuild"),
];

#[derive(Debug)]
struct MigrationSourceTable {
    current: &'static RestoreTableSpec,
    excluded_columns: &'static [&'static str],
}

#[derive(Debug)]
struct MigrationSourceSpec {
    version: i32,
    tables: Vec<MigrationSourceTable>,
}

pub(super) fn require_supported_version(version: i32) -> Result<(), AppError> {
    if SCHEMA_VERSION != LATEST_DECLARED_SOURCE_VERSION {
        return Err(AppError::Config(format!(
            "schema version {SCHEMA_VERSION} has no reviewed migration source specification; \
             latest declared version is {LATEST_DECLARED_SOURCE_VERSION}"
        )));
    }
    if version < EARLIEST_RESTORE_VERSION {
        return Err(AppError::InvalidInput(format!(
            "restore source user_version={version} is older than supported \
             user_version {EARLIEST_RESTORE_VERSION}"
        )));
    }
    if version > LATEST_DECLARED_SOURCE_VERSION {
        return Err(AppError::InvalidInput(format!(
            "restore source user_version={version} is newer than supported \
             user_version {LATEST_DECLARED_SOURCE_VERSION}"
        )));
    }
    Ok(())
}

fn source_spec(version: i32) -> Result<MigrationSourceSpec, AppError> {
    require_supported_version(version)?;
    let tables = SOURCE_TABLE_NAMES
        .iter()
        .map(|name| {
            let current = table_spec(name)?;
            let excluded_columns = if version < 15 {
                match *name {
                    "mcp_servers" | "skills" => &["enabled_grokbuild"][..],
                    _ => &[][..],
                }
            } else {
                &[][..]
            };
            Ok(MigrationSourceTable {
                current,
                excluded_columns,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(MigrationSourceSpec { version, tables })
}

impl MigrationSourceTable {
    fn includes_column(&self, name: &str) -> bool {
        !self.excluded_columns.contains(&name)
    }

    fn expected_columns(&self) -> BTreeSet<String> {
        self.current
            .columns
            .iter()
            .filter(|column| self.includes_column(column.name))
            .map(|column| column.name.to_string())
            .collect()
    }
}

impl Database {
    pub(crate) fn validate_untrusted_migration_source(
        connection: &Connection,
    ) -> Result<i32, AppError> {
        let version = Self::get_user_version(connection)?;
        let spec = source_spec(version)?;
        validate_source_shape(connection, &spec)?;
        if version < SCHEMA_VERSION {
            // Migrations may rebuild tables with INSERT...SELECT. Decode all
            // historical values first so affinity cannot sanitize hostile
            // storage classes or domains before Rust sees them.
            validate_source_values(connection, &spec)?;
        }
        Ok(version)
    }

    pub(crate) fn validate_migration_source_version(
        connection: &Connection,
        version: i32,
    ) -> Result<(), AppError> {
        let spec = source_spec(version)?;
        validate_source_shape(connection, &spec)
    }
}

fn validate_source_shape(
    connection: &Connection,
    spec: &MigrationSourceSpec,
) -> Result<(), AppError> {
    let observed_tables = connection
        .prepare(
            "SELECT name FROM main.sqlite_schema
             WHERE type = 'table'
             ORDER BY name",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|error| invalid_source(spec.version, format!("inspect table set: {error}")))?
        .into_iter()
        .filter(|name| !is_sqlite_internal_table(name))
        .collect::<Vec<_>>();
    let expected_tables = spec
        .tables
        .iter()
        .map(|table| table.current.name.to_string())
        .collect::<Vec<_>>();
    if observed_tables != expected_tables {
        let observed = observed_tables.into_iter().collect::<BTreeSet<_>>();
        let expected = expected_tables.into_iter().collect::<BTreeSet<_>>();
        return Err(invalid_source(
            spec.version,
            format!(
                "table set mismatch; missing={:?}, extra={:?}",
                expected.difference(&observed).collect::<Vec<_>>(),
                observed.difference(&expected).collect::<Vec<_>>()
            ),
        ));
    }

    for table in &spec.tables {
        let pragma = format!(
            "PRAGMA main.table_xinfo({})",
            quote_identifier(table.current.name)
        );
        let observed_columns = connection
            .prepare(&pragma)
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(1)?, row.get::<_, i64>(6)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(|error| {
                invalid_source(
                    spec.version,
                    format!("inspect {} columns: {error}", table.current.name),
                )
            })?;
        let observed_names = observed_columns
            .iter()
            .map(|(name, _hidden)| name.clone())
            .collect::<BTreeSet<_>>();
        let expected_names = table.expected_columns();
        let hidden = observed_columns
            .iter()
            .filter(|(_name, hidden)| *hidden != 0)
            .map(|(name, hidden)| format!("{name}:{hidden}"))
            .collect::<Vec<_>>();
        if observed_names.len() != observed_columns.len()
            || observed_names != expected_names
            || !hidden.is_empty()
        {
            return Err(invalid_source(
                spec.version,
                format!(
                    "{} column set mismatch; missing={:?}, extra={:?}, generated_or_hidden={hidden:?}",
                    table.current.name,
                    expected_names.difference(&observed_names).collect::<Vec<_>>(),
                    observed_names.difference(&expected_names).collect::<Vec<_>>()
                ),
            ));
        }
    }
    Ok(())
}

fn validate_source_values(
    connection: &Connection,
    spec: &MigrationSourceSpec,
) -> Result<(), AppError> {
    for table in &spec.tables {
        let source_columns = table
            .current
            .columns
            .iter()
            .filter(|column| table.includes_column(column.name))
            .collect::<Vec<_>>();
        let projection = source_columns
            .iter()
            .map(|column| quote_identifier(column.name))
            .collect::<Vec<_>>()
            .join(", ");
        let mut statement = connection
            .prepare(&format!(
                "SELECT {projection} FROM {}",
                quote_identifier(table.current.name)
            ))
            .map_err(|error| {
                invalid_source(
                    spec.version,
                    format!("prepare {} value decoder: {error}", table.current.name),
                )
            })?;
        let mut rows = statement.query([]).map_err(|error| {
            invalid_source(
                spec.version,
                format!("read {} values: {error}", table.current.name),
            )
        })?;
        let mut skill_directories = BTreeMap::new();
        while let Some(row) = rows.next().map_err(|error| {
            invalid_source(
                spec.version,
                format!("read {} row: {error}", table.current.name),
            )
        })? {
            let source_values = (0..source_columns.len())
                .map(|index| row.get::<_, Value>(index))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    invalid_source(
                        spec.version,
                        format!("decode {} row: {error}", table.current.name),
                    )
                })?;
            let mut source_values = source_values.into_iter();
            let current_values = table
                .current
                .columns
                .iter()
                .map(|column| {
                    if table.includes_column(column.name) {
                        source_values.next().ok_or_else(|| {
                            AppError::Database(format!(
                                "migration source decoder width mismatch for {}",
                                table.current.name
                            ))
                        })
                    } else {
                        // v15 owns these two BOOLEAN columns and initializes
                        // both to false.
                        Ok(Value::Integer(0))
                    }
                })
                .collect::<Result<Vec<_>, AppError>>()?;
            if source_values.next().is_some() {
                return Err(AppError::Database(format!(
                    "migration source decoder overflow for {}",
                    table.current.name
                )));
            }
            validate_restore_row(table.current, &current_values)?;
            record_skill_directory(table.current, &current_values, &mut skill_directories)?;
        }
    }
    Ok(())
}

fn is_sqlite_internal_table(name: &str) -> bool {
    matches!(name, "sqlite_sequence" | "sqlite_stat1" | "sqlite_stat4")
}

fn invalid_source(version: i32, detail: String) -> AppError {
    AppError::InvalidInput(format!(
        "untrusted v{version} migration source is not canonical: {detail}"
    ))
}

#[cfg(test)]
mod tests {
    use super::{source_spec, PRE_V15_EXCLUDED_COLUMNS, SOURCE_TABLE_NAMES};
    use crate::database::SCHEMA_VERSION;

    #[test]
    fn every_supported_version_has_the_reviewed_cli_table_inventory() {
        for version in 13..=SCHEMA_VERSION {
            let spec = source_spec(version).expect("declared source spec");
            let names = spec
                .tables
                .iter()
                .map(|table| table.current.name)
                .collect::<Vec<_>>();
            assert_eq!(names, SOURCE_TABLE_NAMES);
        }
    }

    #[test]
    fn historical_column_deltas_are_explicit_and_exhaustive() {
        assert_eq!(
            PRE_V15_EXCLUDED_COLUMNS,
            &[
                ("mcp_servers", "enabled_grokbuild"),
                ("skills", "enabled_grokbuild")
            ]
        );
        for version in [13, 14] {
            let spec = source_spec(version).expect("historical source spec");
            for (table_name, column_name) in PRE_V15_EXCLUDED_COLUMNS {
                let table = spec
                    .tables
                    .iter()
                    .find(|table| table.current.name == *table_name)
                    .expect("declared source table");
                assert!(!table.includes_column(column_name));
            }
        }
    }
}
