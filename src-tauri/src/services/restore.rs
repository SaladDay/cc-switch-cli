//! Restore publication coordination shared by local, backup, and cloud paths.

use crate::database::{Database, PreparedDatabaseRestore, RestorePostcommitState};
use crate::error::AppError;
use crate::restore_protocol::{
    RestoreIntent, RestoreOperationId, RestoreSkillsMode, RESTORE_GENERATION_KEY,
};
use crate::store::AppState;

use super::skills_restore::{
    finalize_published_skills, operation_staging_exists, rollback_unpublished_skills,
    InstalledSkillsRestore, PreparedSkillsRestore,
};
use super::state_coordination::{
    acquire_restore_exclusive_permit, acquire_restore_exclusive_permit_blocking,
    RestoreExclusivePermit,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RestorePublicationToken(String);

impl RestorePublicationToken {
    fn from_operation(operation: RestoreOperationId) -> Self {
        Self(operation.to_string())
    }

    pub(crate) fn is_current(&self) -> Result<bool, AppError> {
        let database = Database::open_readonly_current_schema_with_busy_timeout(
            std::time::Duration::from_millis(50),
        )?;
        self.matches_database(&database)
    }

    pub(crate) fn matches_database(&self, database: &Database) -> Result<bool, AppError> {
        Ok(database.get_setting(RESTORE_GENERATION_KEY)?.as_deref() == Some(self.0.as_str()))
    }

    #[cfg(test)]
    pub(crate) fn for_test(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RestorePendingPhase {
    SkillsFinalize,
    SemanticMigration,
    PureHydration,
    MemoryInstall,
    LiveProjection,
    Finalize,
}

impl RestorePendingPhase {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::SkillsFinalize => "skills-finalize",
            Self::SemanticMigration => "semantic-migration",
            Self::PureHydration => "pure-hydration",
            Self::MemoryInstall => "memory-install",
            Self::LiveProjection => "live-projection",
            Self::Finalize => "finalize",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RestorePendingRetry {
    pub(crate) phase: RestorePendingPhase,
    pub(crate) detail: String,
}

impl RestorePendingRetry {
    pub(crate) fn message(&self) -> String {
        format!(
            "Restore committed; {} is pending retry: {}",
            self.phase.as_str(),
            self.detail
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RestorePostCommitStatus {
    Applied,
    PendingRetry(RestorePendingRetry),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RestorePublication<Snapshot = ()> {
    pub(crate) token: RestorePublicationToken,
    pub(crate) status: RestorePostCommitStatus,
    pub(crate) snapshot: Option<Snapshot>,
}

impl RestorePostCommitStatus {
    pub(crate) fn pending_retry(&self) -> Option<&RestorePendingRetry> {
        match self {
            Self::Applied => None,
            Self::PendingRetry(pending) => Some(pending),
        }
    }
}

pub(crate) struct RestoreCompletion<Snapshot> {
    pub(crate) publication: RestorePublicationToken,
    pub(crate) pre_restore_backup_id: String,
    pub(crate) status: RestorePostCommitStatus,
    pub(crate) snapshot: Option<Snapshot>,
}

pub(crate) struct RestoreCoordinator;

impl RestoreCoordinator {
    pub(crate) async fn acquire() -> Result<RestoreSession, AppError> {
        let permit = acquire_restore_exclusive_permit()
            .await
            .map_err(AppError::Message)?;
        Ok(RestoreSession {
            permit,
            operation: RestoreOperationId::fresh(),
        })
    }

    pub(crate) fn acquire_blocking() -> Result<RestoreSession, AppError> {
        let permit = acquire_restore_exclusive_permit_blocking().map_err(AppError::Message)?;
        Ok(RestoreSession {
            permit,
            operation: RestoreOperationId::fresh(),
        })
    }

    /// Read-only probe performed while the caller holds a shared permit. A
    /// false result lets ordinary state loading proceed without an exclusive
    /// lock round-trip.
    pub(crate) fn recovery_needed() -> Result<bool, AppError> {
        let Some(database) = Database::open_for_restore_recovery()? else {
            return Ok(false);
        };
        if database.read_restore_intent()?.is_some() {
            return Ok(true);
        }
        let Some(published) = database.published_restore_state()? else {
            return Ok(false);
        };
        Ok(published.postcommit == RestorePostcommitState::Pending
            || (published.skills_mode.replaces_skills()
                && operation_staging_exists(published.operation)?))
    }

    pub(crate) fn recover_before_state_load_blocking() -> Result<(), AppError> {
        let permit = acquire_restore_exclusive_permit_blocking().map_err(AppError::Message)?;
        recover_pending_under_permit(&permit)
    }
}

/// One exclusive restore operation. The capability spans state loading,
/// candidate publication, memory installation, and every live projection.
pub(crate) struct RestoreSession {
    permit: RestoreExclusivePermit,
    operation: RestoreOperationId,
}

impl RestoreSession {
    pub(crate) fn operation(&self) -> RestoreOperationId {
        self.operation
    }

    pub(crate) fn load_state(&self) -> Result<AppState, AppError> {
        recover_pending_under_permit(&self.permit)?;
        AppState::try_new_for_restore(&self.permit)
    }

    pub(crate) fn publish<Snapshot>(
        &self,
        state: &AppState,
        prepared_database: PreparedDatabaseRestore,
        prepared_skills: Option<PreparedSkillsRestore>,
        snapshot: impl FnOnce(&AppState) -> Snapshot,
    ) -> Result<RestoreCompletion<Snapshot>, AppError> {
        ensure_restore_allowed(state)?;
        let skills_mode = if prepared_skills.is_some() {
            RestoreSkillsMode::Replace
        } else {
            RestoreSkillsMode::Preserve
        };
        let armed = prepared_database.arm(self.operation, skills_mode)?;
        let intent = RestoreIntent {
            operation_id: self.operation,
            skills_mode,
        };
        state.db.persist_restore_intent(intent)?;

        let installed_skills = match prepared_skills {
            Some(prepared) => match prepared.install() {
                Ok(installed) => Some(installed),
                Err(error) => {
                    return Err(rollback_prepublication_failure(
                        &state.db,
                        self.operation,
                        error,
                    ));
                }
            },
            None => None,
        };

        let published = match state.db.publish_armed_database_restore(armed) {
            Ok(published) => published,
            Err(error) => {
                return Err(rollback_publication_failure(
                    &state.db,
                    self.operation,
                    installed_skills,
                    error,
                ));
            }
        };
        debug_assert_eq!(published.operation, self.operation);
        let publication = RestorePublicationToken::from_operation(self.operation);

        if let Some(installed) = installed_skills {
            if let Err(error) = installed.finalize() {
                return Ok(pending_completion(
                    state,
                    publication,
                    published.backup_id,
                    RestorePendingPhase::SkillsFinalize,
                    error,
                ));
            }
        }

        finish_postcommit(
            &self.permit,
            state,
            publication,
            published.backup_id,
            snapshot,
        )
    }
}

fn rollback_prepublication_failure(
    database: &Database,
    operation: RestoreOperationId,
    original: AppError,
) -> AppError {
    match rollback_unpublished_skills(operation)
        .and_then(|()| database.clear_restore_intent(operation))
    {
        Ok(()) => original,
        Err(rollback) => AppError::Message(format!(
            "restore preparation failed: {original}; rollback remains pending: {rollback}"
        )),
    }
}

fn rollback_publication_failure(
    database: &Database,
    operation: RestoreOperationId,
    installed_skills: Option<InstalledSkillsRestore>,
    original: AppError,
) -> AppError {
    let rollback = match installed_skills {
        Some(installed) => installed.rollback(),
        None => Ok(()),
    }
    .and_then(|()| database.clear_restore_intent(operation));
    match rollback {
        Ok(()) => original,
        Err(rollback) => AppError::Message(format!(
            "database publication failed: {original}; Skills/intent rollback remains pending: {rollback}"
        )),
    }
}

fn recover_pending_under_permit(permit: &RestoreExclusivePermit) -> Result<(), AppError> {
    let Some(recovery_database) = Database::open_for_restore_recovery()? else {
        return Ok(());
    };

    if let Some(intent) = recovery_database.read_restore_intent()? {
        if intent.skills_mode.replaces_skills() {
            rollback_unpublished_skills(intent.operation_id)?;
        }
        recovery_database.clear_restore_intent(intent.operation_id)?;
    }

    let published = recovery_database.published_restore_state()?;
    if published.is_some() {
        recovery_database.validate_published_restore_schema()?;
    }
    drop(recovery_database);
    let Some(published) = published else {
        return Ok(());
    };

    if published.skills_mode.replaces_skills() && operation_staging_exists(published.operation)? {
        finalize_published_skills(published.operation)?;
    }
    if published.postcommit == RestorePostcommitState::Applied {
        return Ok(());
    }

    let state = AppState::try_new_for_restore(permit)?;
    let publication = RestorePublicationToken::from_operation(published.operation);
    let completion = finish_postcommit(permit, &state, publication, String::new(), |_| ())?;
    match completion.status {
        RestorePostCommitStatus::Applied => Ok(()),
        RestorePostCommitStatus::PendingRetry(pending) => Err(AppError::Message(pending.message())),
    }
}

fn finish_postcommit<Snapshot>(
    permit: &RestoreExclusivePermit,
    state: &AppState,
    publication: RestorePublicationToken,
    backup_id: String,
    snapshot: impl FnOnce(&AppState) -> Snapshot,
) -> Result<RestoreCompletion<Snapshot>, AppError> {
    let operation = RestoreOperationId::parse(&publication.0)?;
    if let Err(error) = run_post_restore_semantic_migrations(&state.db) {
        return Ok(pending_completion(
            state,
            publication,
            backup_id,
            RestorePendingPhase::SemanticMigration,
            error,
        ));
    }

    let config = match crate::store::load_config_snapshot_from_db_pure(&state.db) {
        Ok(config) => config,
        Err(error) => {
            return Ok(pending_completion(
                state,
                publication,
                backup_id,
                RestorePendingPhase::PureHydration,
                error,
            ));
        }
    };
    if let Err(error) = state.replace_config_snapshot(config) {
        return Ok(pending_completion(
            state,
            publication,
            backup_id,
            RestorePendingPhase::MemoryInstall,
            error,
        ));
    }

    if let Err(error) =
        crate::services::provider::ProviderService::sync_current_to_live_for_restore(state, permit)
    {
        return Ok(pending_completion(
            state,
            publication,
            backup_id,
            RestorePendingPhase::LiveProjection,
            error,
        ));
    }

    if let Err(error) = state.db.mark_restore_postcommit_applied(operation) {
        return Ok(pending_completion(
            state,
            publication,
            backup_id,
            RestorePendingPhase::Finalize,
            error,
        ));
    }

    let restored_snapshot = snapshot(state);
    Ok(RestoreCompletion {
        publication,
        pre_restore_backup_id: backup_id,
        status: RestorePostCommitStatus::Applied,
        snapshot: Some(restored_snapshot),
    })
}

fn ensure_restore_allowed(state: &AppState) -> Result<(), AppError> {
    if state
        .proxy_service
        .is_running_snapshot_blocking()
        .map_err(AppError::Message)?
    {
        return Err(AppError::localized(
            "restore.proxy_running",
            "本地代理正在运行，请先停止代理后再恢复配置",
            "The local proxy is running. Stop it before restoring configuration.",
        ));
    }

    for app_type in [
        crate::AppType::Claude,
        crate::AppType::Codex,
        crate::AppType::Gemini,
    ] {
        if state
            .proxy_service
            .is_app_takeover_active_blocking(&app_type)
            .map_err(AppError::Message)?
        {
            return Err(AppError::localized(
                "restore.takeover_active",
                "当前仍有应用处于代理接管状态，请先关闭接管后再恢复配置",
                "An app takeover is still active. Disable takeover before restoring configuration.",
            ));
        }
    }
    Ok(())
}

fn pending_completion<Snapshot>(
    state: &AppState,
    publication: RestorePublicationToken,
    backup_id: String,
    phase: RestorePendingPhase,
    error: AppError,
) -> RestoreCompletion<Snapshot> {
    let pending = RestorePendingRetry {
        phase,
        detail: error.to_string(),
    };
    let marker = serde_json::json!({
        "phase": pending.phase.as_str(),
        "detail": &pending.detail,
        "publication": &publication.0,
    })
    .to_string();
    let operation = RestoreOperationId::parse(&publication.0);
    let marker_result =
        operation.and_then(|operation| state.db.persist_restore_retry_marker(operation, &marker));
    if let Err(marker_error) = marker_result {
        log::warn!(
            "{}; additionally failed to persist retry marker: {}",
            pending.message(),
            marker_error
        );
    } else {
        log::warn!("{}", pending.message());
    }

    RestoreCompletion {
        publication,
        pre_restore_backup_id: backup_id,
        status: RestorePostCommitStatus::PendingRetry(pending),
        snapshot: None,
    }
}

fn run_post_restore_semantic_migrations(database: &Database) -> Result<(), AppError> {
    #[cfg(test)]
    if TEST_SEMANTIC_FAILURE.with(std::cell::Cell::get) {
        return Err(AppError::Message(
            "forced post-restore semantic migration failure".to_string(),
        ));
    }
    crate::store::run_post_restore_semantic_migrations(database)
}

#[cfg(test)]
thread_local! {
    static TEST_SEMANTIC_FAILURE: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) struct SemanticFailureGuard;

#[cfg(test)]
impl SemanticFailureGuard {
    pub(crate) fn activate() -> Self {
        TEST_SEMANTIC_FAILURE.with(|flag| {
            assert!(!flag.replace(true), "semantic failure seam is nested");
        });
        Self
    }
}

#[cfg(test)]
impl Drop for SemanticFailureGuard {
    fn drop(&mut self) {
        TEST_SEMANTIC_FAILURE.with(|flag| flag.set(false));
    }
}

#[cfg(test)]
mod tests {
    use super::{RestoreCoordinator, RestorePostCommitStatus, SemanticFailureGuard};
    use crate::app_config::AppType;
    use crate::database::{CanonicalPublicationFailureGuard, Database};
    use crate::error::AppError;
    use crate::provider::Provider;
    use crate::restore_protocol::{RestoreIntent, RestoreSkillsMode};
    use crate::services::provider::GeminiScrubLiveFailureGuard;
    use crate::services::skills_restore::PreparedSkillsRestore;
    use crate::store::AppState;
    use serde_json::json;
    use std::io::Write;

    fn prepared_provider_restore(
        id: &str,
    ) -> Result<crate::database::PreparedDatabaseRestore, AppError> {
        let remote = Database::memory()?;
        remote.save_provider(
            AppType::Claude.as_str(),
            &Provider::with_id(
                id.to_string(),
                id.to_string(),
                json!({"env": {"ANTHROPIC_AUTH_TOKEN": "sandbox"}}),
                None,
            ),
        )?;
        Database::prepare_sql_string_for_sync(&remote.export_sql_string_for_sync()?)
    }

    fn one_skill_zip(directory: &str, content: &[u8]) -> Vec<u8> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        writer
            .start_file(
                format!("{directory}/SKILL.md"),
                crate::services::webdav_sync::archive::zip_file_options(),
            )
            .expect("start Skill file");
        writer.write_all(content).expect("write Skill file");
        writer.finish().expect("finish Skills ZIP").into_inner()
    }

    fn insert_skill(database: &Database, id: &str, directory: &str) -> Result<(), AppError> {
        let connection = crate::database::lock_conn!(database.conn);
        connection
            .execute(
                "INSERT INTO skills (
                     id, name, directory, enabled_claude, enabled_codex,
                     enabled_gemini, enabled_opencode, enabled_hermes, installed_at
                 ) VALUES (?1, ?2, ?3, 0, 0, 0, 0, 0, 1)",
                rusqlite::params![id, directory, directory],
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(())
    }

    fn prepared_provider_and_skill_restore(
        provider_id: &str,
        skill_directory: &str,
    ) -> Result<(crate::database::PreparedDatabaseRestore, Vec<u8>), AppError> {
        let remote = Database::memory()?;
        remote.save_provider(
            AppType::Claude.as_str(),
            &Provider::with_id(
                provider_id.to_string(),
                provider_id.to_string(),
                json!({"env": {"ANTHROPIC_AUTH_TOKEN": "sandbox"}}),
                None,
            ),
        )?;
        insert_skill(
            &remote,
            &format!("remote:{skill_directory}"),
            skill_directory,
        )?;
        let database =
            Database::prepare_sql_string_for_sync(&remote.export_sql_string_for_sync()?)?;
        Ok((
            database,
            one_skill_zip(skill_directory, provider_id.as_bytes()),
        ))
    }

    fn seed_old_live_state(database: &Database) -> Result<std::path::PathBuf, AppError> {
        database.save_provider(
            AppType::Claude.as_str(),
            &Provider::with_id(
                "old-provider".to_string(),
                "Old".to_string(),
                json!({}),
                None,
            ),
        )?;
        insert_skill(database, "local:old-skill", "old-skill")?;
        let old_file = crate::config::get_app_config_dir().join("skills/old-skill/SKILL.md");
        std::fs::create_dir_all(old_file.parent().expect("old Skill parent"))
            .map_err(|error| AppError::io(&old_file, error))?;
        std::fs::write(&old_file, b"old").map_err(|error| AppError::io(&old_file, error))?;
        Ok(old_file)
    }

    #[test]
    #[serial_test::serial(home_settings)]
    fn publication_tokens_advance_across_serialized_restores() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create isolated restore home");
        let _environment = crate::test_support::TestEnvGuard::isolated(temp.path());
        let initial = Database::init()?;
        initial.save_provider(
            AppType::Claude.as_str(),
            &Provider::with_id("initial".into(), "Initial".into(), json!({}), None),
        )?;
        drop(initial);

        let first = {
            let session = RestoreCoordinator::acquire_blocking()?;
            let state = session.load_state()?;
            session
                .publish(&state, prepared_provider_restore("first")?, None, |_| ())?
                .publication
        };
        assert!(first.is_current()?);

        let second = {
            let session = RestoreCoordinator::acquire_blocking()?;
            let state = session.load_state()?;
            session
                .publish(&state, prepared_provider_restore("second")?, None, |_| ())?
                .publication
        };
        assert_ne!(first, second);
        assert!(!first.is_current()?);
        assert!(second.is_current()?);
        Ok(())
    }

    #[test]
    #[serial_test::serial(home_settings)]
    fn semantic_failure_reports_committed_restore_pending_retry() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create isolated restore home");
        let _environment = crate::test_support::TestEnvGuard::isolated(temp.path());
        let initial = Database::init()?;
        initial.save_provider(
            AppType::Claude.as_str(),
            &Provider::with_id("initial".into(), "Initial".into(), json!({}), None),
        )?;
        drop(initial);

        let session = RestoreCoordinator::acquire_blocking()?;
        let state = session.load_state()?;
        let _failure = SemanticFailureGuard::activate();
        let completion = session.publish(
            &state,
            prepared_provider_restore("committed")?,
            None,
            |_| (),
        )?;
        assert!(matches!(
            completion.status,
            RestorePostCommitStatus::PendingRetry(_)
        ));
        assert!(completion.snapshot.is_none());
        assert!(completion.publication.matches_database(&state.db)?);
        assert!(state
            .db
            .get_provider_by_id("committed", AppType::Claude.as_str())?
            .is_some());
        Ok(())
    }

    #[test]
    #[serial_test::serial(home_settings)]
    fn database_publication_failure_rolls_back_installed_skills_and_intent() -> Result<(), AppError>
    {
        let temp = tempfile::tempdir().expect("create isolated restore home");
        let _environment = crate::test_support::TestEnvGuard::isolated(temp.path());
        let initial = Database::init()?;
        let old_file = seed_old_live_state(&initial)?;
        drop(initial);

        let session = RestoreCoordinator::acquire_blocking()?;
        let state = session.load_state()?;
        let (prepared_database, skills_zip) =
            prepared_provider_and_skill_restore("new-provider", "new-skill")?;
        let expected = prepared_database.skill_directories()?;
        let prepared_skills =
            PreparedSkillsRestore::prepare(session.operation(), &skills_zip, &expected)?;

        let _failure = CanonicalPublicationFailureGuard::activate();
        let error = match session.publish(&state, prepared_database, Some(prepared_skills), |_| ())
        {
            Ok(_) => panic!("database publication seam must fail before the final commit"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("forced canonical publication"));

        assert!(state
            .db
            .get_provider_by_id("old-provider", AppType::Claude.as_str())?
            .is_some());
        assert!(state
            .db
            .get_provider_by_id("new-provider", AppType::Claude.as_str())?
            .is_none());
        assert!(state.db.get_installed_skill("local:old-skill")?.is_some());
        assert!(state.db.get_installed_skill("remote:new-skill")?.is_none());
        assert_eq!(std::fs::read(&old_file).expect("read old Skill"), b"old");
        assert!(!crate::config::get_app_config_dir()
            .join("skills/new-skill")
            .exists());
        assert!(state.db.read_restore_intent()?.is_none());
        Ok(())
    }

    #[test]
    #[serial_test::serial(home_settings)]
    fn recovery_rolls_back_skills_when_old_live_intent_survives() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create isolated restore home");
        let _environment = crate::test_support::TestEnvGuard::isolated(temp.path());
        let initial = Database::init()?;
        let old_file = seed_old_live_state(&initial)?;
        drop(initial);

        {
            let session = RestoreCoordinator::acquire_blocking()?;
            let state = session.load_state()?;
            let operation = session.operation();
            let (prepared_database, skills_zip) =
                prepared_provider_and_skill_restore("new-provider", "new-skill")?;
            let expected = prepared_database.skill_directories()?;
            let prepared_skills =
                PreparedSkillsRestore::prepare(operation, &skills_zip, &expected)?;
            let _armed = prepared_database.arm(operation, RestoreSkillsMode::Replace)?;
            state.db.persist_restore_intent(RestoreIntent {
                operation_id: operation,
                skills_mode: RestoreSkillsMode::Replace,
            })?;
            let installed = prepared_skills.install()?;
            std::mem::forget(installed);
            // Simulate process death before the SQLite Backup publication.
        }

        RestoreCoordinator::recover_before_state_load_blocking()?;
        let recovered = Database::init()?;
        assert!(recovered
            .get_provider_by_id("old-provider", AppType::Claude.as_str())?
            .is_some());
        assert!(recovered
            .get_provider_by_id("new-provider", AppType::Claude.as_str())?
            .is_none());
        assert_eq!(std::fs::read(&old_file).expect("read old Skill"), b"old");
        assert!(!crate::config::get_app_config_dir()
            .join("skills/new-skill")
            .exists());
        assert!(recovered.read_restore_intent()?.is_none());
        Ok(())
    }

    #[test]
    #[serial_test::serial(home_settings)]
    fn recovery_finalizes_skills_when_new_database_generation_is_live() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create isolated restore home");
        let _environment = crate::test_support::TestEnvGuard::isolated(temp.path());
        let initial = Database::init()?;
        let old_file = seed_old_live_state(&initial)?;
        drop(initial);
        let operation;

        {
            let session = RestoreCoordinator::acquire_blocking()?;
            let state = session.load_state()?;
            operation = session.operation();
            let (prepared_database, skills_zip) =
                prepared_provider_and_skill_restore("new-provider", "new-skill")?;
            let expected = prepared_database.skill_directories()?;
            let prepared_skills =
                PreparedSkillsRestore::prepare(operation, &skills_zip, &expected)?;
            let armed = prepared_database.arm(operation, RestoreSkillsMode::Replace)?;
            state.db.persist_restore_intent(RestoreIntent {
                operation_id: operation,
                skills_mode: RestoreSkillsMode::Replace,
            })?;
            let installed = prepared_skills.install()?;
            state.db.publish_armed_database_restore(armed)?;
            std::mem::forget(installed);
            // Simulate process death after Backup committed but before Skills
            // finalization and all post-commit work.
        }

        RestoreCoordinator::recover_before_state_load_blocking()?;
        let recovered = Database::init()?;
        assert!(recovered
            .get_provider_by_id("new-provider", AppType::Claude.as_str())?
            .is_some());
        assert!(recovered
            .get_provider_by_id("old-provider", AppType::Claude.as_str())?
            .is_none());
        assert!(!old_file.exists());
        assert_eq!(
            std::fs::read(crate::config::get_app_config_dir().join("skills/new-skill/SKILL.md"))
                .expect("read new Skill"),
            b"new-provider"
        );
        let published = recovered
            .published_restore_state()?
            .expect("published restore metadata");
        assert_eq!(published.operation, operation);
        assert_eq!(published.skills_mode, RestoreSkillsMode::Replace);
        assert_eq!(
            published.postcommit,
            crate::database::RestorePostcommitState::Applied
        );
        assert!(!crate::config::get_app_config_dir()
            .join(".restore")
            .join(operation.to_string())
            .exists());
        Ok(())
    }

    #[test]
    #[serial_test::serial(home_settings)]
    fn gemini_scrub_failure_is_committed_then_retried_before_normal_state_load(
    ) -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create isolated restore home");
        let _environment = crate::test_support::TestEnvGuard::isolated(temp.path());
        Database::init()?;
        let env_path = crate::gemini_config::get_gemini_env_path();
        std::fs::create_dir_all(env_path.parent().expect("Gemini env parent"))
            .map_err(|error| AppError::io(&env_path, error))?;
        std::fs::write(
            &env_path,
            "# keep\nGEMINI_API_KEY=leaked-key\nUNRELATED=value\n",
        )
        .map_err(|error| AppError::io(&env_path, error))?;

        let remote = Database::memory()?;
        remote.save_provider(
            AppType::Gemini.as_str(),
            &Provider::with_id(
                "gemini-restored".to_string(),
                "Gemini Restored".to_string(),
                json!({"env": {"GEMINI_API_KEY": "leaked-key"}}),
                None,
            ),
        )?;
        remote.set_config_snippet(
            AppType::Gemini.as_str(),
            Some(json!({"GEMINI_API_KEY": "leaked-key"}).to_string()),
        )?;
        let prepared =
            Database::prepare_sql_string_for_sync(&remote.export_sql_string_for_sync()?)?;

        let publication = {
            let session = RestoreCoordinator::acquire_blocking()?;
            let state = session.load_state()?;
            let _failure = GeminiScrubLiveFailureGuard::activate();
            let completion = session.publish(&state, prepared, None, |_| ())?;
            assert!(matches!(
                completion.status,
                RestorePostCommitStatus::PendingRetry(_)
            ));
            assert!(completion.publication.matches_database(&state.db)?);
            assert_eq!(
                state
                    .db
                    .published_restore_state()?
                    .expect("published state")
                    .postcommit,
                crate::database::RestorePostcommitState::Pending
            );
            completion.publication
        };

        assert!(
            std::fs::read_to_string(&env_path)
                .expect("read failed-scrub env")
                .contains("GEMINI_API_KEY=leaked-key"),
            "the injected failure must happen before the live env mutation"
        );

        // AppState's ordinary startup path must notice and finish the pending
        // restore before any normal startup migration or live import runs.
        let recovered = AppState::try_new()?;
        assert!(publication.matches_database(&recovered.db)?);
        assert_eq!(
            recovered
                .db
                .published_restore_state()?
                .expect("published state")
                .postcommit,
            crate::database::RestorePostcommitState::Applied
        );
        assert_eq!(
            std::fs::read_to_string(&env_path).expect("read scrubbed env"),
            "# keep\nUNRELATED=value\n"
        );
        Ok(())
    }
}
