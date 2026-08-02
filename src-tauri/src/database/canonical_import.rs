//! Canonical reconstruction and publication for untrusted restores.
//!
//! Only `schema.rs` can construct `CanonicalStage`. This module moves rows
//! from `UntrustedScratch` through the exhaustive typed policy manifest, then
//! overlays device-local state while the live database is locked. The source
//! schema is never attached, copied, or published.

use super::restore_policy::{
    copy_fixed_table, count_rows, effective_restore_policy, quote_identifier, table_spec,
    validate_canonical_stage, validate_column_value, RestoreFlavor, RestorePolicy,
    PROXY_CONFIG_LOCAL_COLUMNS, RESTORE_TABLE_SPECS, SYNC_LIVE_OVERLAY_TABLES,
    SYNC_LOCAL_SETTINGS_KEYS, USER_RESTORE_LOCAL_SETTINGS_KEYS,
};
use super::schema::CanonicalStage;
use super::sql_import::UntrustedScratch;
use super::{lock_conn, run_sqlite_backup_to_completion, Database, SCHEMA_VERSION};
use crate::error::AppError;
use rusqlite::backup::Backup;
use rusqlite::types::Value;
use rusqlite::{params, Connection};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct RestoreDiscardReport {
    pub(super) provider_health_rows: u64,
    pub(super) failover_snapshot_rows: u64,
    pub(super) unmatched_proxy_overlay_rows: u64,
}

impl RestoreDiscardReport {
    fn add_table_rows(&mut self, table: &str, rows: u64) {
        match table {
            "provider_health" => {
                self.provider_health_rows = self.provider_health_rows.saturating_add(rows);
            }
            "proxy_failover_live_snapshots" => {
                self.failover_snapshot_rows = self.failover_snapshot_rows.saturating_add(rows);
            }
            _ => {}
        }
    }

    pub(super) fn total_runtime_rows(self) -> u64 {
        self.provider_health_rows
            .saturating_add(self.failover_snapshot_rows)
    }
}

pub(super) struct CanonicalBuild {
    stage: CanonicalStage,
    discarded: RestoreDiscardReport,
}

impl CanonicalBuild {
    pub(super) fn skill_directories(&self) -> Result<BTreeSet<String>, AppError> {
        let mut statement = self
            .stage
            .connection()
            .prepare("SELECT directory FROM skills ORDER BY directory")
            .map_err(database_error)?;
        let directories = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(database_error)?
            .map(|row| row.map_err(database_error))
            .collect();
        directories
    }

    pub(super) fn install_restore_metadata(
        &mut self,
        operation: crate::restore_protocol::RestoreOperationId,
        skills_mode: crate::restore_protocol::RestoreSkillsMode,
    ) -> Result<(), AppError> {
        let transaction = self
            .stage
            .connection_mut()
            .transaction()
            .map_err(database_error)?;
        for key in crate::restore_protocol::RESERVED_RESTORE_SETTING_KEYS {
            transaction
                .execute("DELETE FROM settings WHERE key = ?1", [key])
                .map_err(database_error)?;
        }

        let operation = operation.to_string();
        for (key, value) in [
            (
                crate::restore_protocol::RESTORE_GENERATION_KEY,
                operation.as_str(),
            ),
            (
                crate::restore_protocol::RESTORE_OPERATION_ID_KEY,
                operation.as_str(),
            ),
            (crate::restore_protocol::RESTORE_POSTCOMMIT_KEY, "pending"),
            (
                crate::restore_protocol::RESTORE_SKILLS_MODE_KEY,
                skills_mode.as_str(),
            ),
        ] {
            transaction
                .execute(
                    "INSERT INTO settings (key, value) VALUES (?1, ?2)",
                    params![key, value],
                )
                .map_err(database_error)?;
        }
        if skills_mode.replaces_skills() {
            transaction
                .execute(
                    "INSERT OR REPLACE INTO settings (key, value)
                     VALUES ('skills_ssot_migration_pending', 'false')",
                    [],
                )
                .map_err(database_error)?;
        }
        transaction.commit().map_err(database_error)
    }
}

pub(super) struct CanonicalPublication {
    pub(super) safety_backup: Option<PathBuf>,
    pub(super) discarded: RestoreDiscardReport,
}

impl Database {
    pub(super) fn build_canonical_stage(
        scratch: &UntrustedScratch,
        flavor: RestoreFlavor,
    ) -> Result<CanonicalBuild, AppError> {
        let mut stage = Self::current_canonical_stage()?;
        stage
            .connection()
            .execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(database_error)?;

        let mut discarded = RestoreDiscardReport::default();
        {
            let transaction = stage
                .connection_mut()
                .transaction()
                .map_err(database_error)?;

            // The schema factory may create seed/default rows. Clear every
            // table child-first, including model_pricing, because pricing is
            // user data and must not be silently replaced by canonical seeds.
            for spec in RESTORE_TABLE_SPECS.iter().rev() {
                transaction
                    .execute(&format!("DELETE FROM {}", quote_identifier(spec.name)), [])
                    .map_err(database_error)?;
            }

            // Parent-first, plain INSERT, with foreign keys enabled for the
            // entire canonical stage lifetime.
            for spec in RESTORE_TABLE_SPECS {
                match effective_restore_policy(spec, flavor) {
                    RestorePolicy::PortableIncoming => {
                        copy_fixed_table(scratch.connection(), &transaction, spec, false)?;
                    }
                    RestorePolicy::RebuildRuntime => {
                        discarded
                            .add_table_rows(spec.name, count_rows(scratch.connection(), spec)?);
                    }
                    RestorePolicy::PreserveLive => {}
                }
            }
            transaction.commit().map_err(database_error)?;
        }

        validate_stage(&stage)?;
        validate_pure_hydration(&stage)?;
        Ok(CanonicalBuild { stage, discarded })
    }

    /// Sole publication boundary. The destination is replaced page-for-page
    /// from a schema-factory-owned CanonicalStage using SQLite's atomic Backup
    /// transaction. No live schema object survives this boundary.
    pub(super) fn publish_canonical_stage(
        &self,
        mut build: CanonicalBuild,
        flavor: RestoreFlavor,
    ) -> Result<CanonicalPublication, AppError> {
        let mut main = lock_conn!(self.conn);
        let safety_backup = self.backup_database_file_on_locked_connection(&main)?;

        {
            let transaction = build
                .stage
                .connection_mut()
                .transaction()
                .map_err(database_error)?;

            for spec in RESTORE_TABLE_SPECS {
                let policy = effective_restore_policy(spec, flavor);
                if policy == RestorePolicy::RebuildRuntime {
                    build
                        .discarded
                        .add_table_rows(spec.name, count_rows(&main, spec)?);
                    continue;
                }

                let preserve = policy == RestorePolicy::PreserveLive
                    || (flavor == RestoreFlavor::Sync
                        && SYNC_LIVE_OVERLAY_TABLES.contains(&spec.name));
                if preserve {
                    copy_fixed_table(&main, &transaction, spec, true)?;
                }
            }

            match flavor {
                RestoreFlavor::UserRestore => {
                    copy_live_settings_keys(&main, &transaction, USER_RESTORE_LOCAL_SETTINGS_KEYS)?;
                }
                RestoreFlavor::Sync => {
                    copy_live_settings_keys(&main, &transaction, SYNC_LOCAL_SETTINGS_KEYS)?;
                    build.discarded.unmatched_proxy_overlay_rows = copy_proxy_config_local_columns(
                        &main,
                        &transaction,
                        PROXY_CONFIG_LOCAL_COLUMNS,
                    )?;
                }
            }

            // Runtime routing state is rebuilt after restore; never arm a
            // failover loop merely because an export carried a stale flag.
            transaction
                .execute(
                    "UPDATE proxy_config
                     SET auto_failover_enabled = 0
                     WHERE auto_failover_enabled != 0",
                    [],
                )
                .map_err(database_error)?;
            transaction.commit().map_err(database_error)?;
        }
        // Overlay values crossed the same typed decoder and stage FKs, but the
        // complete stage is certified again immediately before the one atomic
        // publication operation.
        validate_stage(&build.stage)?;
        fail_canonical_publication_for_test()?;
        let backup = Backup::new(build.stage.connection(), &mut main)
            .map_err(|error| AppError::Database(format!("open canonical publication: {error}")))?;
        run_sqlite_backup_to_completion(&backup)?;

        if build.discarded.total_runtime_rows() > 0
            || build.discarded.unmatched_proxy_overlay_rows > 0
        {
            log::info!(
                "restore rebuilt runtime state: provider_health={}, \
                 failover_snapshots={}, unmatched_proxy_overlays={}",
                build.discarded.provider_health_rows,
                build.discarded.failover_snapshot_rows,
                build.discarded.unmatched_proxy_overlay_rows
            );
        }

        Ok(CanonicalPublication {
            safety_backup,
            discarded: build.discarded,
        })
    }
}

fn fail_canonical_publication_for_test() -> Result<(), AppError> {
    #[cfg(test)]
    if TEST_CANONICAL_PUBLICATION_FAILURE.with(std::cell::Cell::get) {
        return Err(AppError::Database(
            "forced canonical publication failure".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    static TEST_CANONICAL_PUBLICATION_FAILURE: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) struct CanonicalPublicationFailureGuard;

#[cfg(test)]
impl CanonicalPublicationFailureGuard {
    pub(crate) fn activate() -> Self {
        TEST_CANONICAL_PUBLICATION_FAILURE.with(|flag| {
            assert!(
                !flag.replace(true),
                "canonical publication failure seam is nested"
            );
        });
        Self
    }
}

#[cfg(test)]
impl Drop for CanonicalPublicationFailureGuard {
    fn drop(&mut self) {
        TEST_CANONICAL_PUBLICATION_FAILURE.with(|flag| flag.set(false));
    }
}

fn validate_stage(stage: &CanonicalStage) -> Result<(), AppError> {
    if Database::get_user_version(stage.connection())? != SCHEMA_VERSION {
        return Err(AppError::Database(
            "canonical stage has an unexpected schema version".to_string(),
        ));
    }
    validate_canonical_stage(stage.connection())
}

/// Exercise the same read-only configuration hydration used after publication
/// without invoking semantic migrations or touching live application files.
fn validate_pure_hydration(stage: &CanonicalStage) -> Result<(), AppError> {
    let provider_count: i64 = stage
        .connection()
        .query_row("SELECT COUNT(*) FROM providers", [], |row| row.get(0))
        .map_err(database_error)?;
    let mcp_count: i64 = stage
        .connection()
        .query_row("SELECT COUNT(*) FROM mcp_servers", [], |row| row.get(0))
        .map_err(database_error)?;
    if provider_count == 0 && mcp_count == 0 {
        return Err(AppError::Config(
            "导入的 SQL 未包含有效的供应商或 MCP 数据".to_string(),
        ));
    }

    let snapshot_database = stage.open_readonly_database()?;
    crate::store::load_config_snapshot_from_db_pure(&snapshot_database)?;
    Ok(())
}

fn copy_live_settings_keys(
    source: &Connection,
    target: &Connection,
    keys: &[&str],
) -> Result<(), AppError> {
    let spec = table_spec("settings")?;
    let columns = super::restore_policy::quoted_columns(spec);
    let insert = format!("INSERT INTO settings ({columns}) VALUES (?1, ?2)");

    for key in keys {
        target
            .execute("DELETE FROM settings WHERE key = ?1", [key])
            .map_err(database_error)?;

        let mut statement = source
            .prepare(&format!("SELECT {columns} FROM settings WHERE key = ?1"))
            .map_err(database_error)?;
        let mut rows = statement.query([key]).map_err(database_error)?;
        if let Some(row) = rows.next().map_err(database_error)? {
            let values = super::restore_policy::read_and_validate_row(row, spec)?;
            target
                .execute(&insert, rusqlite::params_from_iter(values.iter()))
                .map_err(database_error)?;
        }
    }
    Ok(())
}

/// Copy only the installation-local proxy fields for app rows that exist in
/// both the locked live database and the incoming canonical stage.
fn copy_proxy_config_local_columns(
    source: &Connection,
    target: &Connection,
    columns: &[&str],
) -> Result<u64, AppError> {
    let spec = table_spec("proxy_config")?;
    let app_type_spec = spec
        .columns
        .iter()
        .find(|column| column.name == "app_type")
        .ok_or_else(|| {
            AppError::Database(
                "proxy_config.app_type is absent from the restore policy".to_string(),
            )
        })?;
    let selected_specs = columns
        .iter()
        .map(|name| {
            spec.columns
                .iter()
                .find(|column| column.name == *name)
                .ok_or_else(|| {
                    AppError::Database(format!(
                        "proxy overlay column {name:?} is absent from restore policy"
                    ))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let select_columns = std::iter::once("app_type")
        .chain(columns.iter().copied())
        .map(quote_identifier)
        .collect::<Vec<_>>()
        .join(", ");
    let assignments = columns
        .iter()
        .enumerate()
        .map(|(index, column)| format!("{} = ?{}", quote_identifier(column), index + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let update = format!(
        "UPDATE proxy_config SET {assignments} WHERE app_type = ?{}",
        columns.len() + 1
    );

    let mut statement = source
        .prepare(&format!("SELECT {select_columns} FROM proxy_config"))
        .map_err(database_error)?;
    let mut rows = statement.query([]).map_err(database_error)?;
    let mut unmatched = 0_u64;
    while let Some(row) = rows.next().map_err(database_error)? {
        let app_type = row.get::<_, Value>(0).map_err(database_error)?;
        validate_column_value(spec.name, app_type_spec, &app_type)?;

        let mut values = Vec::with_capacity(columns.len() + 1);
        for (offset, column) in selected_specs.iter().enumerate() {
            let value = row.get::<_, Value>(offset + 1).map_err(database_error)?;
            validate_column_value(spec.name, column, &value)?;
            values.push(value);
        }
        values.push(app_type);
        let changed = target
            .execute(&update, rusqlite::params_from_iter(values.iter()))
            .map_err(database_error)?;
        if changed == 0 {
            unmatched = unmatched.saturating_add(1);
        }
    }
    Ok(unmatched)
}

fn database_error(error: rusqlite::Error) -> AppError {
    AppError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{copy_proxy_config_local_columns, RestoreFlavor};
    use crate::database::sql_import::{SqlImportBatch, UntrustedScratch};
    use crate::database::Database;
    use crate::error::AppError;
    use rusqlite::Connection;

    #[test]
    fn proxy_overlay_intersects_rows_instead_of_creating_partial_rows() -> Result<(), AppError> {
        let live = Database::memory()?;
        let incoming =
            Connection::open_in_memory().map_err(|error| AppError::Database(error.to_string()))?;
        Database::create_tables_on_conn(
            &incoming,
            crate::database::MigrationRunContext::LocalUpgrade,
        )?;
        incoming
            .execute("DELETE FROM proxy_config WHERE app_type = 'codex'", [])
            .map_err(|error| AppError::Database(error.to_string()))?;

        let live = crate::database::lock_conn!(live.conn);
        let unmatched =
            copy_proxy_config_local_columns(&live, &incoming, super::PROXY_CONFIG_LOCAL_COLUMNS)?;
        assert_eq!(unmatched, 1);
        let codex_rows: i64 = incoming
            .query_row(
                "SELECT COUNT(*) FROM proxy_config WHERE app_type = 'codex'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        assert_eq!(codex_rows, 0);
        Ok(())
    }

    #[test]
    #[serial_test::serial(home_settings)]
    fn whole_database_publication_keeps_long_lived_wal_connections_on_the_live_file(
    ) -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create isolated restore home");
        let _environment = crate::test_support::TestEnvGuard::isolated(temp.path());

        let remote = Database::memory()?;
        {
            let connection = crate::database::lock_conn!(remote.conn);
            connection.execute(
                "INSERT INTO providers (id, app_type, name, settings_config, meta)
                 VALUES ('remote-provider', 'claude', 'Remote', '{}', '{}')",
                [],
            )?;
        }
        let sql = remote.export_sql_string()?;
        let batch = SqlImportBatch::from_borrowed(&sql)?;
        let scratch = UntrustedScratch::from_batch(&batch)?;
        let build = Database::build_canonical_stage(&scratch, RestoreFlavor::UserRestore)?;

        let live = Database::init()?;
        let db_path = crate::database::database_path()?;
        live.save_provider(
            "claude",
            &crate::provider::Provider::with_id(
                "old-provider".to_string(),
                "Old".to_string(),
                serde_json::json!({}),
                None,
            ),
        )?;
        {
            let connection = crate::database::lock_conn!(live.conn);
            connection
                .execute_batch(
                    "PRAGMA journal_mode = WAL;
                     CREATE TABLE historical_extra(value TEXT);
                     CREATE VIEW historical_view AS SELECT value FROM historical_extra;
                     CREATE TRIGGER historical_trigger
                     AFTER INSERT ON historical_extra
                     BEGIN
                         INSERT INTO historical_extra(value) VALUES ('triggered');
                     END;",
                )
                .map_err(|error| AppError::Database(error.to_string()))?;
        }

        #[cfg(unix)]
        let inode_before = {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(&db_path)
                .map_err(|error| AppError::io(&db_path, error))?
                .ino()
        };

        let long_lived =
            Connection::open(&db_path).map_err(|error| AppError::Database(error.to_string()))?;
        long_lived
            .execute_batch("PRAGMA journal_mode = WAL; BEGIN")
            .map_err(|error| AppError::Database(error.to_string()))?;
        let old_rows: i64 = long_lived
            .query_row(
                "SELECT COUNT(*) FROM providers WHERE id = 'old-provider'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        assert_eq!(old_rows, 1);

        live.publish_canonical_stage(build, RestoreFlavor::UserRestore)?;

        // An already-open WAL reader keeps its coherent pre-publication
        // snapshot until it ends that transaction.
        let snapshot_rows: i64 = long_lived
            .query_row(
                "SELECT COUNT(*) FROM providers WHERE id = 'old-provider'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        assert_eq!(snapshot_rows, 1);
        long_lived
            .execute_batch("COMMIT")
            .map_err(|error| AppError::Database(error.to_string()))?;

        let new_rows: i64 = long_lived
            .query_row(
                "SELECT COUNT(*) FROM providers WHERE id = 'remote-provider'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        assert_eq!(new_rows, 1);
        let old_rows: i64 = long_lived
            .query_row(
                "SELECT COUNT(*) FROM providers WHERE id = 'old-provider'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        assert_eq!(old_rows, 0);
        let historical_objects: i64 = long_lived
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE name IN ('historical_extra', 'historical_view', 'historical_trigger')",
                [],
                |row| row.get(0),
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        assert_eq!(historical_objects, 0);

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let inode_after = std::fs::metadata(&db_path)
                .map_err(|error| AppError::io(&db_path, error))?
                .ino();
            assert_eq!(
                inode_before, inode_after,
                "publication must update the live database, not rename over its inode"
            );
        }
        Ok(())
    }
}
