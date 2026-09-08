//! Opt-in acceptance gates for MCP work inside an ordinary provider switch.

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
#[ignore = "provider-owned MCP adoption gate; expected to fail before migration"]
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
#[ignore = "provider-owned MCP adoption gate; expected to fail before migration"]
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
            // No trigger side writes: those would be rejected before native work.
            // Changing the current provider breaks this reference only at COMMIT.
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
