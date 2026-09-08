//! Acceptance gates for MCP work inside an ordinary provider switch.

use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use rusqlite::hooks::{AuthAction, AuthContext, Authorization, TransactionOperation};
use serde_json::json;

use super::{consumer_tests::LitePeer, test_fixture::*, *};
use crate::{provider::Provider, services::provider::ProviderService, test_support::TestEnvGuard};

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}

fn state() -> AppState {
    let state = fixture(true);
    for id in ["old", "new"] {
        state
            .db
            .save_provider(
                "gemini",
                &Provider::with_id(
                    id.into(),
                    id.into(),
                    json!({"env":{"GEMINI_API_KEY":format!("{id}-fake")},"config":{"theme":id}}),
                    None,
                ),
            )
            .unwrap();
    }
    state.db.set_current_provider("gemini", "old").unwrap();
    fs::write(
        crate::gemini_config::get_gemini_env_path(),
        "# keep layout\nGEMINI_API_KEY=old-fake\n",
    )
    .unwrap();
    state
}

fn files() -> Vec<(PathBuf, Option<Vec<u8>>)> {
    let mut files = observed_files();
    let env = crate::gemini_config::get_gemini_env_path();
    files.push((env.clone(), fs::read(env).ok()));
    files
}

fn reject_provider_commit(state: &AppState) {
    // The reference remains valid until selection changes, and is checked only
    // at COMMIT. There are no trigger side writes rejected before native work.
    state
        .db
        .conn
        .lock()
        .unwrap()
        .execute_batch(
            "PRAGMA foreign_keys=ON;
         CREATE UNIQUE INDEX fixture_current_reference ON providers(id,app_type,is_current);
         CREATE TABLE fixture_commit_guard(id TEXT,app_type TEXT,selected INTEGER,
           FOREIGN KEY(id,app_type,selected) REFERENCES providers(id,app_type,is_current)
           DEFERRABLE INITIALLY DEFERRED);
         INSERT INTO fixture_commit_guard VALUES('old','gemini',1);",
        )
        .unwrap();
}

fn with_publication_hook<T>(
    hook: impl FnMut(&crate::config::ConfigWriteTarget, Option<&[u8]>) -> Result<(), AppError> + 'static,
    action: impl FnOnce() -> T,
) -> T {
    let hook = std::rc::Rc::new(std::cell::RefCell::new(hook));
    let provider_hook = hook.clone();
    crate::gemini_config::operation::with_hook(
        Box::new(move |resource, bytes| (provider_hook.borrow_mut())(resource, bytes)),
        || {
            native_file::with_exchange_hook(
                Box::new(move |resource, bytes| (hook.borrow_mut())(resource, bytes)),
                action,
            )
        },
    )
}

fn assert_recovered(
    state: &AppState,
    before: &[(PathBuf, Option<Vec<u8>>)],
    cache: &serde_json::Value,
) {
    assert_eq!(
        state.db.get_current_provider("gemini").unwrap().as_deref(),
        Some("old")
    );
    assert_eq!(
        &serde_json::to_value(&*state.config.read().unwrap()).unwrap(),
        cache
    );
    let changed = before
        .iter()
        .filter(|(path, bytes)| &fs::read(path).ok() != bytes)
        .map(|(path, _)| path.display().to_string())
        .collect::<Vec<_>>();
    assert!(
        changed.is_empty(),
        "provider failure left native MCP writes: {changed:?}"
    );
}

#[test]
fn later_mcp_error_recovers_every_app_touched_by_the_provider_switch() {
    let temp = tempdir();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state();
    let mut target = server("target");
    target.apps = all_apps();
    state.db.save_mcp_server(&target).unwrap();
    // The all-App tail must continue after Codex fails, then recover every App
    // when its caller abandons the provider transaction.
    fs::write(crate::codex_config::get_codex_config_path(), "[invalid").unwrap();
    fs::write(crate::hermes_config::get_hermes_config_path(), "[invalid").unwrap();
    let before = files();
    let cache = serde_json::to_value(&*state.config.read().unwrap()).unwrap();
    let error = ProviderService::switch(&state, AppType::Gemini, "new").unwrap_err();
    assert!(error.to_string().contains("codex"), "{error}");
    assert!(error.to_string().contains("hermes"), "{error}");
    assert_recovered(&state, &before, &cache);
}

#[test]
fn provider_commit_failure_recovers_all_mcp_publications() {
    let mut published = Vec::new();
    for fail_commit in [false, true] {
        let temp = tempdir();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state();
        let mut target = server("target");
        target.apps = all_apps();
        state.db.save_mcp_server(&target).unwrap();
        if fail_commit {
            reject_provider_commit(&state);
        }
        let before = files();
        let cache = serde_json::to_value(&*state.config.read().unwrap()).unwrap();
        let commit_files = Arc::new(Mutex::new(None));
        if fail_commit {
            let observed = commit_files.clone();
            let paths = before
                .iter()
                .map(|(path, _)| path.clone())
                .collect::<Vec<_>>();
            // Observe the last COMMIT, after the entry point's read transaction,
            // before SQLite checks the constraint and the caller compensates.
            // rusqlite 0.31 maps SQLite's COMMIT authorization to Unknown.
            state
                .db
                .conn
                .lock()
                .unwrap()
                .authorizer(Some(move |context: AuthContext<'_>| {
                    if matches!(
                        context.action,
                        AuthAction::Transaction {
                            operation: TransactionOperation::Unknown
                        }
                    ) {
                        *observed.lock().unwrap() = Some(
                            paths
                                .iter()
                                .map(|path| fs::read(path).ok())
                                .collect::<Vec<_>>(),
                        );
                    }
                    Authorization::Allow
                }));
        }
        let result = ProviderService::switch(&state, AppType::Gemini, "new");
        state
            .db
            .conn
            .lock()
            .unwrap()
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        if fail_commit {
            let error = result.unwrap_err();
            assert!(
                matches!(&error, AppError::Conflict(message)
                    if message.contains("FOREIGN KEY constraint failed")),
                "{error}"
            );
            assert_eq!(
                commit_files.lock().unwrap().as_ref(),
                Some(&published),
                "failed COMMIT must observe the same complete publication as the successful control"
            );
            assert_recovered(&state, &before, &cache);
        } else {
            result.unwrap();
            for (app, path) in paths() {
                assert!(native_entries(&app, &path)["target"].is_object(), "{app:?}");
            }
            published = before.iter().map(|(path, _)| fs::read(path).ok()).collect();
        }
    }
}

#[test]
#[cfg(unix)]
fn recovery_preserves_links_to_gemini_from_earlier_and_later_apps() {
    use std::os::unix::fs::symlink;

    for linked_app in [AppType::Claude, AppType::OpenCode] {
        let temp = tempdir();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state();
        let mut target = server("target");
        target.apps = all_apps();
        state.db.save_mcp_server(&target).unwrap();
        let settings = crate::gemini_config::get_gemini_settings_path();
        let link = paths()
            .into_iter()
            .find(|(app, _)| *app == linked_app)
            .unwrap()
            .1;
        fs::remove_file(&link).unwrap();
        symlink(&settings, &link).unwrap();
        reject_provider_commit(&state);
        let before = files();
        let cache = serde_json::to_value(&*state.config.read().unwrap()).unwrap();
        let error = ProviderService::switch(&state, AppType::Gemini, "new").unwrap_err();
        assert!(
            matches!(&error, AppError::Conflict(message)
            if message.contains("FOREIGN KEY constraint failed")),
            "{linked_app:?}: {error}"
        );
        assert_recovered(&state, &before, &cache);
        assert_eq!(fs::read_link(link).unwrap(), settings);
    }
}

#[test]
fn database_failures_recover_in_publication_order_without_releasing_protection() {
    use cc_switch_core::fs::{
        shared_live_config_lock_path, SharedLiveConfigLock, SharedLiveConfigLockError,
    };

    for failure in ["suppressed", "provider-drift", "commit", "aborted"] {
        let temp = tempdir();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = Arc::new(state());
        let mut target = server("target");
        target.apps = all_apps();
        state.db.save_mcp_server(&target).unwrap();
        state.db.conn.lock().unwrap().execute_batch(
            "ALTER TABLE providers ADD COLUMN fixture_private BLOB DEFAULT X'00ff';
             ALTER TABLE mcp_servers ADD COLUMN fixture_private BLOB DEFAULT X'01fe';
             UPDATE mcp_servers SET enabled_grokbuild=1 WHERE id='peer';
             INSERT INTO mcp_native_links(server_id,app_id,native_snapshot) VALUES('peer','grokbuild','future opaque snapshot');",
        ).unwrap();
        if failure == "commit" {
            reject_provider_commit(&state);
        } else {
            let (timing, action) = match failure {
                "suppressed" => ("BEFORE", "SELECT RAISE(IGNORE);"),
                "provider-drift" => (
                    "AFTER",
                    "UPDATE providers SET fixture_private=X'0203' WHERE id='old';",
                ),
                "aborted" => ("AFTER", "SELECT RAISE(ROLLBACK, 'fixture abort');"),
                _ => unreachable!(),
            };
            state.db.conn.lock().unwrap().execute_batch(&format!(
                "CREATE TRIGGER fixture_link {timing} INSERT ON mcp_native_links WHEN NEW.app_id='hermes' BEGIN {action} END;"
            )).unwrap();
        }
        let providers =
            cc_switch_store::read_provider_rows(&state.db.conn.lock().unwrap(), None).unwrap();
        let servers =
            cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap();
        let link = cc_switch_store::read_mcp_native_link(
            &state.db.conn.lock().unwrap(),
            "peer",
            "grokbuild",
        )
        .unwrap();
        let before = files();
        let cache = serde_json::to_value(&*state.config.read().unwrap()).unwrap();
        let trace = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let observed = trace.clone();
        let held = state.clone();
        let home = temp.path().to_owned();
        let error = with_publication_hook(
            move |resource, _| {
                assert!(held.config.try_write().is_err());
                assert!(held.db.conn.try_lock().is_err());
                assert!(matches!(
                    SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(&home)),
                    Err(SharedLiveConfigLockError::Unavailable)
                ));
                let conn =
                    rusqlite::Connection::open(home.join(".cc-switch/cc-switch.db")).unwrap();
                conn.busy_timeout(std::time::Duration::ZERO).unwrap();
                let attempt = conn.execute_batch("BEGIN IMMEDIATE");
                if failure == "aborted" && observed.borrow().len() >= 7 {
                    // SQLite itself releases this protection on RAISE(ROLLBACK).
                    attempt.unwrap();
                    conn.execute_batch("ROLLBACK").unwrap();
                } else {
                    assert_eq!(
                        attempt.unwrap_err().sqlite_error_code(),
                        Some(rusqlite::ErrorCode::DatabaseBusy)
                    );
                }
                observed.borrow_mut().push(resource.path().to_owned());
                Ok(())
            },
            || ProviderService::switch(&state, AppType::Gemini, "new"),
        )
        .unwrap_err();
        assert!(
            !error.to_string().contains("回滚失败"),
            "{failure}: {error}"
        );
        let mut expected = vec![
            crate::gemini_config::get_gemini_env_path(),
            crate::gemini_config::get_gemini_settings_path(),
        ];
        expected.extend(paths().into_iter().map(|(_, path)| path));
        expected.extend(expected.clone().into_iter().rev());
        assert_eq!(*trace.borrow(), expected, "{failure}");
        assert_recovered(&state, &before, &cache);
        let conn = state.db.conn.lock().unwrap();
        assert_eq!(
            cc_switch_store::read_provider_rows(&conn, None).unwrap(),
            providers
        );
        assert_eq!(
            cc_switch_store::read_mcp_server_rows(&conn).unwrap(),
            servers
        );
        assert_eq!(
            cc_switch_store::read_mcp_native_link(&conn, "peer", "grokbuild").unwrap(),
            link
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM mcp_native_links", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        conn.execute_batch("BEGIN IMMEDIATE; ROLLBACK").unwrap();
        drop(
            SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(temp.path())).unwrap(),
        );
    }
}

#[test]
fn other_app_publication_failures_recover_without_overwriting_external_edits() {
    // Gemini's provider/MCP uncertain-write and dependent-env cases remain in
    // provider::gemini::execution_tests. Here each other binding fails in turn.
    for app in McpService::supported_mcp_apps().filter(|app| *app != AppType::Gemini) {
        for failure in ["before", "after", "external"] {
            let temp = tempdir();
            let _env = TestEnvGuard::isolated(temp.path());
            let state = state();
            let mut target = server("target");
            target.apps = all_apps();
            state.db.save_mcp_server(&target).unwrap();
            if failure == "external" {
                reject_provider_commit(&state);
            }
            let mut before = files();
            let cache = serde_json::to_value(&*state.config.read().unwrap()).unwrap();
            let path = paths()
                .into_iter()
                .find(|(target, _)| *target == app)
                .unwrap()
                .1;
            let observed_path = path.clone();
            let mut calls = 0;
            let error = with_publication_hook(
                move |resource, replacement| {
                    if resource.path() == observed_path {
                        calls += 1;
                        if failure == "external" && calls == 2 {
                            fs::write(resource.path(), b"external fixture").unwrap();
                        } else if failure != "external" && calls == 1 {
                            if failure == "after" {
                                resource.write(replacement.unwrap())?;
                            }
                            return Err(AppError::io(
                                resource.path(),
                                std::io::Error::other("fixture native failure"),
                            ));
                        }
                    }
                    Ok(())
                },
                || ProviderService::switch(&state, AppType::Gemini, "new"),
            )
            .unwrap_err();
            if failure == "external" {
                assert!(
                    error.to_string().contains("native recovery"),
                    "{app:?}: {error}"
                );
                before
                    .iter_mut()
                    .find(|(target, _)| *target == path)
                    .unwrap()
                    .1 = Some(b"external fixture".to_vec());
            } else {
                assert!(
                    error.to_string().contains("fixture native failure"),
                    "{app:?}: {error}"
                );
            }
            assert_recovered(&state, &before, &cache);
            assert_eq!(
                state
                    .db
                    .conn
                    .lock()
                    .unwrap()
                    .query_row("SELECT COUNT(*) FROM mcp_native_links", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
    }
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn provider_mcp_excludes_real_lite_through_commit_and_recovery() {
    for failure in [false, true] {
        let temp = tempdir();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state();
        let mut target = server("target");
        target.apps = all_apps();
        state.db.save_mcp_server(&target).unwrap();
        if failure {
            reject_provider_commit(&state);
        }
        let home = temp.path().to_owned();
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let observed = calls.clone();
        let result = with_publication_hook(
            move |_, _| {
                LitePeer::run(
                    &home,
                    "consumer_coordination::native_switch_in_cli_fixture",
                    "probe_locked",
                );
                observed.set(observed.get() + 1);
                Ok(())
            },
            || ProviderService::switch(&state, AppType::Gemini, "new"),
        );
        assert_eq!(result.is_err(), failure, "{result:?}");
        assert_eq!(calls.get(), if failure { 14 } else { 7 });
        LitePeer::run(
            temp.path(),
            "consumer_coordination::native_switch_in_cli_fixture",
            "probe_released",
        );
    }
}

#[test]
#[ignore = "provider-owned MCP adoption gate; requires the independently built Lite test binary"]
fn lite_restores_native_fields_after_provider_owned_mcp_removal() {
    // The public sync is a positive control with the same profile and peer.
    // The second case must meet that shared contract through a provider switch.
    for provider_owned in [false, true] {
        let temp = tempdir();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state();
        state.db.save_mcp_server(&server("cli-import")).unwrap();
        let path = crate::gemini_config::get_gemini_settings_path();
        fs::write(
            &path,
            json!({"fixture":true,"mcpServers":{
                "cli-import":{"command":"old-not-executed","trust":"keep-native"}
            }})
            .to_string(),
        )
        .unwrap();
        LitePeer::run(
            temp.path(),
            "consumer_coordination::create_mcp_in_cli_fixture",
            "unused",
        );
        let peer =
            cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "lite-peer")
                .unwrap()
                .expect("Lite must commit its peer record");
        if provider_owned {
            ProviderService::switch(&state, AppType::Gemini, "new").unwrap();
        } else {
            McpService::sync_enabled_for_app(&state, &AppType::Gemini).unwrap();
        }
        assert!(native_entries(&AppType::Gemini, &path)
            .get("cli-import")
            .is_none());
        assert_eq!(
            cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "lite-peer")
                .unwrap()
                .as_ref(),
            Some(&peer)
        );
        LitePeer::run(
            temp.path(),
            "consumer_coordination::mcp_lifecycle::mcp_lifecycle_in_cli_fixture",
            "gemini:enable",
        );
        assert_eq!(
            cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "lite-peer")
                .unwrap(),
            Some(peer),
            "the complete CLI/Lite round trip must retain the peer record"
        );
        let entries = native_entries(&AppType::Gemini, &path);
        assert_eq!(entries["cli-import"]["command"], "not-executed");
        assert_eq!(
            entries["cli-import"]["trust"], "keep-native",
            "provider_owned={provider_owned}"
        );
    }
}
