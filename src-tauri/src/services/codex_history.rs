use crate::error::AppError;

#[derive(Debug, Clone, Default)]
pub struct CodexHistoryToggleOutcome {
    pub changed: bool,
    pub migration:
        Option<crate::codex_history_migration::CodexHistoryProviderBucketMigrationOutcome>,
    pub restore: Option<crate::codex_history_migration::CodexOfficialHistoryRestoreOutcome>,
}

pub fn set_unified_session_history_enabled(
    enabled: bool,
    migrate_existing: bool,
    restore: bool,
) -> Result<CodexHistoryToggleOutcome, AppError> {
    let existing = crate::settings::get_settings();
    let changed = existing.unify_codex_session_history != enabled;
    if !changed {
        return Ok(CodexHistoryToggleOutcome {
            changed: false,
            ..Default::default()
        });
    }

    let mut next = existing.clone();
    next.unify_codex_session_history = enabled;
    next.unify_codex_migrate_existing = if enabled && migrate_existing {
        Some(true)
    } else {
        None
    };

    crate::settings::update_settings(next)?;
    let state = match crate::store::AppState::try_new() {
        Ok(state) => state,
        Err(err) => {
            rollback_codex_history_settings(&existing);
            return Err(AppError::Message(format!(
                "Unified Codex session history setting was rolled back because app state initialization failed: {err}"
            )));
        }
    };
    if let Err(err) = futures::executor::block_on(
        state
            .db
            .delete_failover_live_snapshots_for_app(crate::app_config::AppType::Codex.as_str()),
    ) {
        rollback_codex_history_settings(&existing);
        return Err(AppError::Message(format!(
            "Unified Codex session history setting was rolled back because failover snapshots could not be invalidated: {err}"
        )));
    }
    if let Err(err) = crate::services::provider::reapply_current_codex_official_live(&state) {
        rollback_codex_history_settings(&existing);
        return Err(AppError::Message(format!(
            "Unified Codex session history setting was rolled back because live config rewrite failed: {err}"
        )));
    }

    if enabled {
        let migration = if migrate_existing {
            Some(
                crate::codex_history_migration::maybe_migrate_codex_official_history_to_unified_bucket(
                )?,
            )
        } else {
            None
        };
        Ok(CodexHistoryToggleOutcome {
            changed: true,
            migration,
            restore: None,
        })
    } else {
        crate::settings::clear_codex_official_history_unify_migration()?;
        crate::settings::clear_codex_unify_migrate_existing()?;
        let restore = if restore {
            Some(crate::codex_history_migration::restore_codex_official_history_from_backups()?)
        } else {
            None
        };
        Ok(CodexHistoryToggleOutcome {
            changed: true,
            migration: None,
            restore,
        })
    }
}

fn rollback_codex_history_settings(existing: &crate::settings::AppSettings) {
    if let Err(err) = crate::settings::update_settings(existing.clone()) {
        log::error!("Failed to roll back unified Codex session history setting: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider;
    use serde_json::json;
    use serial_test::serial;
    use tempfile::TempDir;

    use crate::test_support::TestEnvGuard;

    #[test]
    #[serial(home_settings)]
    fn toggling_unified_history_invalidates_codex_failover_snapshots() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());
        let mut settings = crate::settings::get_settings();
        settings.unify_codex_session_history = true;
        crate::settings::update_settings(settings).expect("seed enabled setting");
        let state = crate::store::AppState::try_new().expect("create app state");
        let mut provider = Provider::with_id(
            "official".to_string(),
            "OpenAI Official".to_string(),
            json!({ "auth": {}, "config": "model = \"gpt-5.4\"\n" }),
            None,
        );
        provider.category = Some("official".to_string());
        state
            .db
            .save_provider(crate::app_config::AppType::Codex.as_str(), &provider)
            .expect("seed provider");
        futures::executor::block_on(state.db.save_failover_live_snapshot(
            crate::app_config::AppType::Codex.as_str(),
            &provider.id,
            r#"{"auth":{},"config":"model_provider = \"custom\""}"#,
        ))
        .expect("seed failover snapshot");

        set_unified_session_history_enabled(false, false, false)
            .expect("disable unified session history");

        let snapshot =
            futures::executor::block_on(state.db.get_failover_live_snapshot(
                crate::app_config::AppType::Codex.as_str(),
                &provider.id,
            ))
            .expect("read failover snapshot");
        assert!(snapshot.is_none(), "toggle must invalidate stale snapshots");
    }
}
