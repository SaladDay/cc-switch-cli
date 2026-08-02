//! Durable database state for the cross-resource restore protocol.

use super::{database_path, lock_conn, Database};
use crate::error::AppError;
use crate::restore_protocol::{
    RestoreIntent, RestoreOperationId, RestoreSkillsMode, RESTORE_GENERATION_KEY,
    RESTORE_INTENT_KEY, RESTORE_OPERATION_ID_KEY, RESTORE_PENDING_RETRY_KEY,
    RESTORE_POSTCOMMIT_KEY, RESTORE_SKILLS_MODE_KEY,
};
use rusqlite::{params, Connection, OpenFlags};
use std::sync::Mutex;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RestorePostcommitState {
    Pending,
    Applied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PublishedRestoreState {
    pub(crate) operation: RestoreOperationId,
    pub(crate) skills_mode: RestoreSkillsMode,
    pub(crate) postcommit: RestorePostcommitState,
}

impl Database {
    pub(crate) fn persist_restore_intent(&self, intent: RestoreIntent) -> Result<(), AppError> {
        let encoded = intent.encode()?;
        let mut connection = lock_conn!(self.conn);
        let transaction = connection
            .transaction()
            .map_err(|error| AppError::Database(error.to_string()))?;
        transaction
            .execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                params![RESTORE_INTENT_KEY, encoded],
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        transaction
            .commit()
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub(crate) fn read_restore_intent(&self) -> Result<Option<RestoreIntent>, AppError> {
        self.get_setting(RESTORE_INTENT_KEY)?
            .map(|value| RestoreIntent::decode(&value))
            .transpose()
    }

    pub(crate) fn clear_restore_intent(
        &self,
        operation: RestoreOperationId,
    ) -> Result<(), AppError> {
        let current = self.read_restore_intent()?;
        match current {
            None => Ok(()),
            Some(intent) if intent.operation_id == operation => {
                self.delete_setting(RESTORE_INTENT_KEY)
            }
            Some(intent) => Err(AppError::InvalidInput(format!(
                "restore intent changed from {operation} to {}",
                intent.operation_id
            ))),
        }
    }

    pub(crate) fn published_restore_state(
        &self,
    ) -> Result<Option<PublishedRestoreState>, AppError> {
        load_published_restore_state(|key| self.get_setting(key))
    }

    pub(crate) fn mark_restore_postcommit_applied(
        &self,
        operation: RestoreOperationId,
    ) -> Result<(), AppError> {
        let mut connection = lock_conn!(self.conn);
        let transaction = connection
            .transaction()
            .map_err(|error| AppError::Database(error.to_string()))?;
        require_current_operation(&transaction, operation)?;
        transaction
            .execute(
                "UPDATE settings SET value = 'applied' WHERE key = ?1",
                [RESTORE_POSTCOMMIT_KEY],
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        transaction
            .execute(
                "DELETE FROM settings WHERE key = ?1",
                [RESTORE_PENDING_RETRY_KEY],
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        transaction
            .commit()
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub(crate) fn persist_restore_retry_marker(
        &self,
        operation: RestoreOperationId,
        marker: &str,
    ) -> Result<(), AppError> {
        let mut connection = lock_conn!(self.conn);
        let transaction = connection
            .transaction()
            .map_err(|error| AppError::Database(error.to_string()))?;
        require_current_operation(&transaction, operation)?;
        transaction
            .execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                params![RESTORE_PENDING_RETRY_KEY, marker],
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        transaction
            .commit()
            .map_err(|error| AppError::Database(error.to_string()))
    }

    /// Open an existing database for restore recovery without creating,
    /// migrating, seeding, or running startup maintenance.
    pub(crate) fn open_for_restore_recovery() -> Result<Option<Self>, AppError> {
        let path = database_path()?;
        if !path.exists() {
            return Ok(None);
        }
        #[cfg(unix)]
        super::validate_existing_database_file(&path)?;

        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let connection = Connection::open_with_flags(&path, flags)
            .map_err(|error| AppError::Database(error.to_string()))?;
        Self::configure_connection(&connection)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|error| AppError::Database(error.to_string()))?;

        if !Self::has_user_tables(&connection)? {
            return Ok(None);
        }
        let has_settings: bool = connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_schema
                    WHERE type = 'table' AND name = 'settings'
                 )",
                [],
                |row| row.get(0),
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        if !has_settings {
            return Ok(None);
        }
        let restore_rows: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM settings
                 WHERE key IN (?1, ?2, ?3, ?4, ?5)",
                params![
                    RESTORE_INTENT_KEY,
                    RESTORE_GENERATION_KEY,
                    RESTORE_OPERATION_ID_KEY,
                    RESTORE_SKILLS_MODE_KEY,
                    RESTORE_POSTCOMMIT_KEY,
                ],
                |row| row.get(0),
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        if restore_rows == 0 {
            return Ok(None);
        }

        Ok(Some(Self {
            conn: Mutex::new(connection),
            runtime_key: format!("restore-recovery:{}", path.display()),
            db_path: Some(path),
        }))
    }

    pub(crate) fn validate_published_restore_schema(&self) -> Result<(), AppError> {
        let connection = lock_conn!(self.conn);
        let version = Self::get_user_version(&connection)?;
        super::migration_source::require_supported_version(version)?;
        Self::validate_migration_source_version(&connection, version)
    }
}

fn require_current_operation(
    connection: &Connection,
    operation: RestoreOperationId,
) -> Result<(), AppError> {
    let current: Option<String> = connection
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [RESTORE_OPERATION_ID_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| AppError::Database(error.to_string()))?;
    let operation_text = operation.to_string();
    if current.as_deref() != Some(operation_text.as_str()) {
        return Err(AppError::InvalidInput(format!(
            "restore operation {operation} is no longer current"
        )));
    }
    Ok(())
}

fn load_published_restore_state(
    mut get: impl FnMut(&str) -> Result<Option<String>, AppError>,
) -> Result<Option<PublishedRestoreState>, AppError> {
    let generation = get(RESTORE_GENERATION_KEY)?;
    let operation = get(RESTORE_OPERATION_ID_KEY)?;
    let skills_mode = get(RESTORE_SKILLS_MODE_KEY)?;
    let postcommit = get(RESTORE_POSTCOMMIT_KEY)?;
    if generation.is_none() && operation.is_none() && skills_mode.is_none() && postcommit.is_none()
    {
        return Ok(None);
    }
    let generation = generation.ok_or_else(|| incomplete_metadata(RESTORE_GENERATION_KEY))?;
    let operation = operation.ok_or_else(|| incomplete_metadata(RESTORE_OPERATION_ID_KEY))?;
    let skills_mode = skills_mode.ok_or_else(|| incomplete_metadata(RESTORE_SKILLS_MODE_KEY))?;
    let postcommit = postcommit.ok_or_else(|| incomplete_metadata(RESTORE_POSTCOMMIT_KEY))?;
    if generation != operation {
        return Err(AppError::InvalidInput(
            "restore generation and operation id do not match".to_string(),
        ));
    }
    let operation = RestoreOperationId::parse(&operation)?;
    let skills_mode = RestoreSkillsMode::parse(&skills_mode)?;
    let postcommit = match postcommit.as_str() {
        "pending" => RestorePostcommitState::Pending,
        "applied" => RestorePostcommitState::Applied,
        value => {
            return Err(AppError::InvalidInput(format!(
                "invalid restore postcommit state {value:?}"
            )));
        }
    };
    Ok(Some(PublishedRestoreState {
        operation,
        skills_mode,
        postcommit,
    }))
}

fn incomplete_metadata(key: &str) -> AppError {
    AppError::InvalidInput(format!("restore metadata is incomplete: missing {key}"))
}

use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use super::{PublishedRestoreState, RestorePostcommitState};
    use crate::database::Database;
    use crate::restore_protocol::{
        RestoreOperationId, RestoreSkillsMode, RESTORE_GENERATION_KEY, RESTORE_OPERATION_ID_KEY,
        RESTORE_POSTCOMMIT_KEY, RESTORE_SKILLS_MODE_KEY,
    };

    const OPERATION: &str = "00112233-4455-4677-8899-aabbccddeeff";

    #[test]
    fn published_state_requires_a_complete_matching_token_set() {
        let database = Database::memory().expect("create database");
        database
            .set_setting(RESTORE_GENERATION_KEY, OPERATION)
            .expect("set partial metadata");
        assert!(database.published_restore_state().is_err());

        database
            .set_setting(RESTORE_OPERATION_ID_KEY, OPERATION)
            .expect("set operation");
        database
            .set_setting(RESTORE_SKILLS_MODE_KEY, "replace")
            .expect("set Skills mode");
        database
            .set_setting(RESTORE_POSTCOMMIT_KEY, "pending")
            .expect("set postcommit");
        assert_eq!(
            database.published_restore_state().expect("read state"),
            Some(PublishedRestoreState {
                operation: RestoreOperationId::for_test(OPERATION),
                skills_mode: RestoreSkillsMode::Replace,
                postcommit: RestorePostcommitState::Pending,
            })
        );
    }
}
