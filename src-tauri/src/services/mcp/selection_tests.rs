use std::{fs, sync::Arc};

use serde_json::{json, Value};

use super::{test_fixture::*, *};
use crate::test_support::TestEnvGuard;

#[test]
fn matrix_reads_fresh_catalog_and_preserves_unowned_fields() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = fixture(true);
    let mut fresh = server("target");
    fresh.name = "fresh".into();
    fresh.server = json!({"command":"fresh-not-executed"});
    state.db.save_mcp_server(&fresh).unwrap();
    state.db.save_mcp_server(&server("new-peer")).unwrap();
    state.db.conn.lock().unwrap().execute_batch("ALTER TABLE mcp_servers ADD COLUMN host_note TEXT DEFAULT 'keep'; UPDATE mcp_servers SET enabled_grokbuild=1 WHERE id='target';").unwrap();
    let peer =
        cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "new-peer").unwrap();
    assert!(McpService::set_apps(&state, "target", all_apps()).unwrap());
    let rows = state.db.get_all_mcp_servers().unwrap();
    assert_eq!(rows["target"].name, "fresh");
    assert_eq!(rows["target"].apps, all_apps());
    assert_eq!(
        state.config.read().unwrap().mcp.servers.as_ref().unwrap()["target"].name,
        "fresh"
    );
    let conn = state.db.conn.lock().unwrap();
    assert_eq!(
        cc_switch_store::read_mcp_server_row(&conn, "new-peer").unwrap(),
        peer
    );
    assert_eq!(
        conn.query_row(
            "SELECT host_note,enabled_grokbuild FROM mcp_servers WHERE id='target'",
            [],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        )
        .unwrap(),
        ("keep".into(), 1)
    );
    drop(conn);
    for (app, path) in paths() {
        assert_eq!(
            native_entries(&app, &path)["target"]["command"],
            if matches!(app, AppType::OpenCode) {
                json!(["fresh-not-executed"])
            } else {
                json!("fresh-not-executed")
            }
        );
    }
}

#[test]
fn every_supported_selection_matrix_and_missing_target_keep_their_contract() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = fixture(true);
    let apps: Vec<_> = McpService::supported_mcp_apps().collect();
    assert_eq!(
        apps,
        paths().into_iter().map(|(app, _)| app).collect::<Vec<_>>()
    );
    for mask in 0..(1 << apps.len()) {
        let mut desired = McpApps::default();
        for (index, app) in apps.iter().enumerate() {
            desired.set_enabled_for(app, mask & (1 << index) != 0);
        }
        assert!(McpService::set_apps(&state, "target", desired.clone()).unwrap());
        assert_eq!(
            state.db.get_all_mcp_servers().unwrap()["target"].apps,
            desired
        );
        for (app, path) in paths() {
            let entries = native_entries(&app, &path);
            if desired.is_enabled_for(&app) {
                assert!(entries["target"].is_object(), "{app:?}, mask={mask}");
                assert_ne!(entries["target"].get("enabled"), Some(&json!(false)));
            } else {
                assert!(entries.get("target").is_none());
            }
        }
    }
    state.db.delete_mcp_server("target").unwrap();
    let before = observed_files();
    assert!(!McpService::set_apps(&state, "target", all_apps()).unwrap());
    assert_files(&before);
    assert!(!state
        .config
        .read()
        .unwrap()
        .mcp
        .servers
        .as_ref()
        .unwrap()
        .contains_key("target"));
}

#[test]
fn unchanged_and_uninitialized_apps_are_not_written_but_fresh_changes_are() {
    use cc_switch_core::fs::{shared_live_config_lock_path, SharedLiveConfigLock};
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = fixture(false);
    assert!(McpService::set_apps(&state, "target", all_apps()).unwrap());
    assert!(paths().iter().all(|(_, path)| !path.exists()));
    for (_, path) in paths() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "invalid native config").unwrap();
    }
    let before = observed_files();
    let lock =
        SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(temp.path())).unwrap();
    assert!(McpService::set_apps(&state, "target", all_apps()).unwrap());
    assert_files(&before);
    drop(lock);
    let mut fresh = state
        .db
        .get_all_mcp_servers()
        .unwrap()
        .shift_remove("target")
        .unwrap();
    fresh.apps.claude = false;
    state.db.save_mcp_server(&fresh).unwrap();
    // Requested matrix matches the stale cache, but not the current database.
    assert!(McpService::set_apps(&state, "target", all_apps()).is_err());
    assert_files(&before);
    assert!(
        !state.db.get_all_mcp_servers().unwrap()["target"]
            .apps
            .claude
    );
}

fn fail_at(state: &AppState, point: &str) {
    let sql = match point {
        "link" => "CREATE TRIGGER fixture_reject BEFORE INSERT ON mcp_native_links WHEN NEW.app_id='gemini' BEGIN SELECT RAISE(IGNORE); END;",
        "selection" => "CREATE TRIGGER fixture_reject BEFORE UPDATE ON mcp_servers WHEN NEW.enabled_gemini=1 BEGIN SELECT RAISE(IGNORE); END;",
        "verification" => "CREATE TRIGGER fixture_reject AFTER UPDATE ON mcp_servers WHEN NEW.enabled_gemini=1 BEGIN UPDATE mcp_servers SET name='changed' WHERE id='peer'; END;",
        "commit" => "PRAGMA foreign_keys=ON; CREATE UNIQUE INDEX fixture_selection ON mcp_servers(id,enabled_claude); CREATE TABLE fixture_fk(id TEXT, enabled INTEGER, FOREIGN KEY(id,enabled) REFERENCES mcp_servers(id,enabled_claude) DEFERRABLE INITIALLY DEFERRED); INSERT INTO fixture_fk VALUES('target',0);",
        _ => panic!("unknown failure point"),
    };
    state.db.conn.lock().unwrap().execute_batch(sql).unwrap();
}

#[test]
fn database_failures_restore_all_files_under_retained_guards_and_allow_retry() {
    use cc_switch_core::fs::{
        shared_live_config_lock_path, SharedLiveConfigLock, SharedLiveConfigLockError,
    };
    for failure in ["link", "selection", "verification", "commit"] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = Arc::new(fixture(true));
        let before = observed_files();
        let rows = cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap();
        fail_at(&state, failure);
        let observed = state.clone();
        let home = temp.path().to_owned();
        let trace = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let writes = trace.clone();
        let result = native_file::with_exchange_hook(
            Box::new(move |resource, _| {
                assert!(observed.config.try_write().is_err());
                assert!(observed.db.conn.try_lock().is_err());
                assert!(matches!(
                    SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(&home)),
                    Err(SharedLiveConfigLockError::Unavailable)
                ));
                let conn =
                    rusqlite::Connection::open(home.join(".cc-switch/cc-switch.db")).unwrap();
                conn.busy_timeout(std::time::Duration::ZERO).unwrap();
                assert_eq!(
                    conn.execute_batch("BEGIN IMMEDIATE")
                        .unwrap_err()
                        .sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy)
                );
                writes.borrow_mut().push(resource.path().to_owned());
                Ok(())
            }),
            || McpService::set_apps(&state, "target", all_apps()),
        );
        assert!(result.is_err(), "{failure}");
        let trace = trace.borrow();
        assert!(trace.len() >= 4 && trace.len() % 2 == 0);
        let (published, restored) = trace.split_at(trace.len() / 2);
        assert_eq!(
            published.iter().rev().collect::<Vec<_>>(),
            restored.iter().collect::<Vec<_>>()
        );
        assert_files(&before);
        assert_eq!(
            state.config.read().unwrap().mcp.servers.as_ref().unwrap()["target"].apps,
            McpApps::default()
        );
        let conn = state.db.conn.lock().unwrap();
        assert_eq!(cc_switch_store::read_mcp_server_rows(&conn).unwrap(), rows);
        for app in McpService::supported_mcp_apps() {
            assert!(
                cc_switch_store::read_mcp_native_link(&conn, "target", app.as_str())
                    .unwrap()
                    .is_none()
            );
        }
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS fixture_reject; DROP TABLE IF EXISTS fixture_fk;",
        )
        .unwrap();
        drop(conn);
        assert!(McpService::set_apps(&state, "target", all_apps()).unwrap());
        let _released =
            SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(temp.path())).unwrap();
    }
}

#[test]
fn a_recovery_conflict_does_not_prevent_recovery_of_other_apps() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = fixture(true);
    let before = observed_files();
    fail_at(&state, "commit");
    let mut calls = 0;
    let error = native_file::with_exchange_hook(
        Box::new(move |resource, _| {
            if resource.path() == crate::gemini_config::get_gemini_settings_path() {
                calls += 1;
                if calls == 2 {
                    fs::write(resource.path(), r#"{"external":true}"#).unwrap();
                }
            }
            Ok(())
        }),
        || McpService::set_apps(&state, "target", all_apps()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("native recovery"));
    let gemini = crate::gemini_config::get_gemini_settings_path();
    for (path, bytes) in before {
        assert_eq!(
            fs::read(&path).ok(),
            if path == gemini {
                Some(br#"{"external":true}"#.to_vec())
            } else {
                bytes
            }
        );
    }
    assert_eq!(
        state.db.get_all_mcp_servers().unwrap()["target"].apps,
        McpApps::default()
    );
}

#[test]
fn shared_custom_destination_recovers_in_reverse_write_order() {
    use crate::settings::{update_settings, AppSettings};
    for failure in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = fixture(true);
        update_settings(AppSettings {
            claude_config_dir: Some(
                temp.path()
                    .join(".gemini/settings")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ..AppSettings::default()
        })
        .unwrap();
        assert_eq!(
            crate::config::get_claude_mcp_path(),
            crate::gemini_config::get_gemini_settings_path()
        );
        let before = observed_files();
        if failure {
            fail_at(&state, "commit");
        }
        let result = McpService::set_apps(&state, "target", all_apps());
        assert_eq!(result.is_err(), failure);
        if failure {
            assert_files(&before);
        } else {
            for (app, path) in paths() {
                assert!(native_entries(&app, &path)["target"].is_object());
            }
        }
    }
}

#[test]
fn uncertain_later_publication_recovers_the_entire_prefix() {
    for published in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = fixture(true);
        let before = observed_files();
        let mut attempted = false;
        let result = native_file::with_exchange_hook(
            Box::new(move |resource, replacement| {
                if resource.path() == crate::opencode_config::get_opencode_config_path()
                    && !attempted
                {
                    attempted = true;
                    if published {
                        resource.write(replacement.unwrap())?;
                    }
                    return Err(AppError::io(
                        resource.path(),
                        std::io::Error::other("fixture publication failure"),
                    ));
                }
                Ok(())
            }),
            || McpService::set_apps(&state, "target", all_apps()),
        );
        assert!(result.is_err());
        assert_files(&before);
        assert_eq!(
            state.db.get_all_mcp_servers().unwrap()["target"].apps,
            McpApps::default()
        );
    }
}

#[cfg(unix)]
#[test]
fn cross_app_leaf_links_and_missing_files_are_restored_after_failed_commit() {
    use std::os::unix::fs::symlink;
    for claude_link in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = fixture(true);
        let claude = crate::config::get_claude_mcp_path();
        let gemini = crate::gemini_config::get_gemini_settings_path();
        let (link, referent) = if claude_link {
            (&claude, &gemini)
        } else {
            (&gemini, &claude)
        };
        fs::remove_file(link).unwrap();
        symlink(referent, link).unwrap();
        for (app, path) in paths() {
            if matches!(app, AppType::Codex | AppType::OpenCode | AppType::Hermes) {
                fs::remove_file(path).unwrap();
            }
        }
        let before = observed_files();
        fail_at(&state, "commit");
        assert!(McpService::set_apps(&state, "target", all_apps()).is_err());
        assert_files(&before);
        assert_eq!(fs::read_link(link).unwrap(), *referent);
    }
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn real_lite_writer_is_excluded_through_all_app_publication_and_recovery() {
    for failure in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = fixture(true);
        if failure {
            fail_at(&state, "commit");
        }
        let home = temp.path().to_owned();
        let result = native_file::with_exchange_hook(
            Box::new(move |_, _| {
                consumer_tests::LitePeer::run(
                    &home,
                    "consumer_coordination::native_switch_in_cli_fixture",
                    "probe_locked",
                );
                Ok(())
            }),
            || McpService::set_apps(&state, "target", all_apps()),
        );
        assert_eq!(result.is_err(), failure);
        consumer_tests::LitePeer::run(
            temp.path(),
            "consumer_coordination::native_switch_in_cli_fixture",
            "probe_released",
        );
    }
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn matrix_snapshots_round_trip_between_real_cli_and_lite_services() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = fixture(true);
    state.db.save_mcp_server(&server("cli-import")).unwrap();
    McpService::set_apps(&state, "cli-import", all_apps()).unwrap();
    for (app, path) in paths() {
        if matches!(app, AppType::Claude | AppType::Gemini) {
            let mut document: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            document["mcpServers"]["cli-import"]["x_native"] = json!({"keep":true});
            fs::write(&path, document.to_string()).unwrap();
        }
    }
    let peer = |app: &AppType, action| {
        consumer_tests::LitePeer::run(
            temp.path(),
            "consumer_coordination::mcp_lifecycle::mcp_lifecycle_in_cli_fixture",
            &format!("{}:{action}", app.as_str()),
        )
    };
    McpService::set_apps(&state, "cli-import", McpApps::default()).unwrap();
    for app in McpService::supported_mcp_apps() {
        peer(&app, "enable");
    }
    for app in McpService::supported_mcp_apps() {
        peer(&app, "disable");
    }
    McpService::set_apps(&state, "cli-import", all_apps()).unwrap();
    for (app, path) in paths() {
        let entries = native_entries(&app, &path);
        assert!(entries["cli-import"].is_object());
        if matches!(app, AppType::Claude | AppType::Gemini) {
            assert_eq!(entries["cli-import"]["x_native"], json!({"keep":true}));
        }
        assert_ne!(entries["cli-import"].get("enabled"), Some(&json!(false)));
    }
}

#[test]
fn later_app_failure_restores_earlier_files_and_catalog() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = fixture(true);
    fs::write(crate::gemini_config::get_gemini_settings_path(), "{invalid").unwrap();
    let before: Vec<_> = paths()
        .into_iter()
        .map(|(_, path)| {
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    let rows = cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap();
    assert!(McpService::set_apps(&state, "target", all_apps()).is_err());
    for (path, bytes) in before {
        assert_eq!(fs::read(&path).unwrap(), bytes, "{}", path.display());
    }
    assert_eq!(
        cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap(),
        rows
    );
    assert_eq!(
        state.config.read().unwrap().mcp.servers.as_ref().unwrap()["target"].apps,
        McpApps::default()
    );
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn real_lite_peer_survives_matrix_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = fixture(true);
    consumer_tests::LitePeer::run(
        temp.path(),
        "consumer_coordination::create_mcp_in_cli_fixture",
        "unused",
    );
    let peer = cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "lite-peer")
        .unwrap()
        .unwrap();
    for apps in [all_apps(), McpApps::default()] {
        assert!(McpService::set_apps(&state, "target", apps.clone()).unwrap());
        assert_eq!(
            cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "lite-peer")
                .unwrap(),
            Some(peer.clone())
        );
        assert_eq!(state.db.get_all_mcp_servers().unwrap()["target"].apps, apps);
        for (app, path) in paths() {
            let entries = native_entries(&app, &path);
            if apps.is_enabled_for(&app) {
                assert!(entries["target"].is_object());
            } else {
                assert!(entries.get("target").is_none());
            }
        }
    }
}
