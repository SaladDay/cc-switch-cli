use std::{fs, sync::Arc};

use super::{test_fixture::*, *};
use crate::test_support::TestEnvGuard;
use cc_switch_core::fs::{
    shared_live_config_lock_path, SharedLiveConfigLock, SharedLiveConfigLockError,
};

fn links(state: &AppState) -> Vec<cc_switch_store::McpNativeLinkRow> {
    let conn = state.db.conn.lock().unwrap();
    let mut statement = conn
        .prepare("SELECT server_id,app_id FROM mcp_native_links ORDER BY server_id,app_id")
        .unwrap();
    let keys = statement
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap();
    keys.map(|key| {
        let (id, app) = key.unwrap();
        cc_switch_store::read_mcp_native_link(&conn, &id, &app)
            .unwrap()
            .unwrap()
    })
    .collect()
}

fn fail_at(state: &AppState, point: &str) {
    let sql = match point {
        "delete" => "CREATE TRIGGER fixture_reject BEFORE DELETE ON mcp_servers WHEN OLD.id='target' BEGIN SELECT RAISE(IGNORE); END;",
        "cascade" => "CREATE TRIGGER fixture_reject BEFORE DELETE ON mcp_native_links WHEN OLD.server_id='target' BEGIN SELECT RAISE(IGNORE); END;",
        "verification" => "CREATE TRIGGER fixture_reject AFTER DELETE ON mcp_servers WHEN OLD.id='target' BEGIN UPDATE mcp_servers SET name='changed' WHERE id='peer'; END;",
        "commit" => "PRAGMA foreign_keys=ON; CREATE TABLE fixture_fk(id TEXT REFERENCES mcp_servers(id) DEFERRABLE INITIALLY DEFERRED); INSERT INTO fixture_fk VALUES('target');",
        _ => panic!("unknown failure point"),
    };
    state.db.conn.lock().unwrap().execute_batch(sql).unwrap();
}

#[test]
fn deletion_covers_all_selection_matrices_without_touching_disabled_apps() {
    let apps: Vec<_> = McpService::supported_mcp_apps().collect();
    assert_eq!(apps.len(), paths().len());
    for mask in 0..(1 << apps.len()) {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = fixture(true);
        McpService::set_apps(&state, "target", all_apps()).unwrap();
        McpService::set_apps(&state, "peer", all_apps()).unwrap();
        let mut fresh = server("target");
        for (bit, app) in apps.iter().enumerate() {
            fresh.apps.set_enabled_for(app, mask & (1 << bit) != 0);
        }
        state.db.save_mcp_server(&fresh).unwrap();
        // Removed links need not contain a snapshot this host understands.
        state.db.conn.lock().unwrap().execute_batch(
            "UPDATE mcp_native_links SET native_snapshot='future-or-invalid' WHERE server_id='target';
             INSERT INTO mcp_native_links(server_id,app_id,native_snapshot) VALUES('target','grokbuild','future');"
        ).unwrap();
        let peers: Vec<_> = links(&state)
            .into_iter()
            .filter(|row| row.server_id == "peer")
            .collect();
        let before = observed_files();
        let native_peers: Vec<_> = paths()
            .iter()
            .map(|(app, path)| native_entries(app, path)["peer"].clone())
            .collect();
        assert!(McpService::delete_server(&state, "target").unwrap());
        for (index, (app, path)) in paths().iter().enumerate() {
            let entries = native_entries(app, path);
            assert_eq!(entries["peer"], native_peers[index]);
            assert_eq!(
                entries.get("target").is_none(),
                fresh.apps.is_enabled_for(app)
            );
            if !fresh.apps.is_enabled_for(app) {
                assert_eq!(fs::read(path).ok(), before[index].1);
            }
        }
        assert_eq!(links(&state), peers);
        assert!(!state
            .db
            .get_all_mcp_servers()
            .unwrap()
            .contains_key("target"));
        assert!(!McpService::delete_server(&state, "target").unwrap());
    }
}

#[test]
fn missing_cached_disabled_and_uninitialized_targets_keep_their_boundaries() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = fixture(false);
    let mut target = server("target");
    target.apps = all_apps();
    state.db.save_mcp_server(&target).unwrap();
    let lock =
        SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(temp.path())).unwrap();
    assert!(McpService::delete_server(&state, "target").unwrap());
    assert!(paths().iter().all(|(_, path)| !path.exists()));
    assert!(McpService::supported_mcp_apps().all(|app| !crate::sync_policy::should_sync_live(&app)));
    drop(lock);

    state.db.save_mcp_server(&server("target")).unwrap();
    state.config.write().unwrap().mcp.servers = None;
    for (_, path) in paths() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "invalid native config").unwrap();
    }
    let before = observed_files();
    let lock =
        SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(temp.path())).unwrap();
    assert!(McpService::delete_server(&state, "target").unwrap());
    assert_files(&before);
    assert!(state.config.read().unwrap().mcp.servers.is_none());

    state.config.write().unwrap().mcp.servers = Some(HashMap::from([("target".into(), target)]));
    assert!(!McpService::delete_server(&state, "target").unwrap());
    assert!(state
        .config
        .read()
        .unwrap()
        .mcp
        .servers
        .as_ref()
        .unwrap()
        .is_empty());
    assert_files(&before);
    assert!(state.db.get_all_mcp_servers().unwrap().contains_key("peer"));
    drop(lock);
}

#[test]
fn deletion_retains_native_policies_and_rejects_legacy_invalid_yaml() {
    for app in McpService::supported_mcp_apps() {
        for original in [None, Some("{invalid"), Some("null")] {
            let temp = tempfile::tempdir().unwrap();
            let _env = TestEnvGuard::isolated(temp.path());
            let state = fixture(true);
            fs::create_dir_all(crate::config::get_claude_config_dir()).unwrap();
            let path = paths()
                .into_iter()
                .find(|(candidate, _)| candidate == &app)
                .unwrap()
                .1;
            let reset = || match original {
                Some(text) => fs::write(&path, text).unwrap(),
                None => match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => panic!("fixture reset: {error}"),
                },
            };
            reset();
            let old_result = McpService::remove_server_from_native("target", &app);
            let expected = fs::read(&path).ok();
            reset();
            let mut target = server("target");
            target.apps.set_enabled_for(&app, true);
            state.db.save_mcp_server(&target).unwrap();
            let result = McpService::delete_server(&state, "target");
            if matches!(app, AppType::Hermes) && original == Some("null") {
                // The old text writer reports success but appends a mapping to
                // a scalar document. Keep the shared binding's safety check.
                assert!(old_result.is_ok());
                assert!(
                    serde_yaml::from_slice::<serde_yaml::Value>(expected.as_ref().unwrap())
                        .is_err()
                );
                assert!(result.is_err());
                assert_eq!(fs::read_to_string(&path).unwrap(), "null");
                assert!(state
                    .db
                    .get_all_mcp_servers()
                    .unwrap()
                    .contains_key("target"));
                continue;
            }
            assert_eq!(
                result.is_err(),
                old_result.is_err(),
                "{app:?} {original:?}: {result:?}"
            );
            assert_eq!(fs::read(&path).ok(), expected, "{app:?} {original:?}");
            assert_eq!(
                state
                    .db
                    .get_all_mcp_servers()
                    .unwrap()
                    .contains_key("target"),
                result.is_err()
            );
        }
    }
}

#[test]
fn database_failures_restore_catalog_links_and_native_files_before_unlocking() {
    for failure in ["delete", "cascade", "verification", "commit"] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = Arc::new(fixture(true));
        McpService::set_apps(&state, "target", all_apps()).unwrap();
        let files = observed_files();
        let rows = cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap();
        let original_links = links(&state);
        let cache = serde_json::to_value(&*state.config.read().unwrap()).unwrap();
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
            || McpService::delete_server(&state, "target"),
        );
        assert!(result.is_err(), "{failure}");
        let trace = trace.borrow();
        assert_eq!(trace.len(), 2 * paths().len());
        let (published, restored) = trace.split_at(paths().len());
        assert_eq!(
            published.iter().rev().collect::<Vec<_>>(),
            restored.iter().collect::<Vec<_>>()
        );
        assert_files(&files);
        assert_eq!(
            cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap(),
            rows
        );
        assert_eq!(links(&state), original_links);
        assert_eq!(
            serde_json::to_value(&*state.config.read().unwrap()).unwrap(),
            cache
        );
        state
            .db
            .conn
            .lock()
            .unwrap()
            .execute_batch(
                "DROP TRIGGER IF EXISTS fixture_reject; DROP TABLE IF EXISTS fixture_fk;",
            )
            .unwrap();
        let lock =
            SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(temp.path())).unwrap();
        assert!(matches!(
            McpService::delete_server(&state, "target"),
            Err(AppError::Conflict(_))
        ));
        assert_files(&files);
        drop(lock);
        assert!(McpService::delete_server(&state, "target").unwrap());
        assert!(links(&state).is_empty());
        let _released =
            SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(temp.path())).unwrap();
    }
}

#[test]
fn deletion_recovers_uncertain_publication_and_keeps_external_recovery_edits() {
    for failure in ["before", "after", "external"] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = fixture(true);
        McpService::set_apps(&state, "target", all_apps()).unwrap();
        let before = observed_files();
        let original_links = links(&state);
        if failure == "external" {
            fail_at(&state, "commit");
        }
        let mut calls = 0;
        let error = native_file::with_exchange_hook(
            Box::new(move |resource, replacement| {
                if resource.path() == crate::opencode_config::get_opencode_config_path() {
                    calls += 1;
                    if failure == "external" && calls == 2 {
                        fs::write(resource.path(), r#"{"external":true}"#).unwrap();
                    } else if failure != "external" && calls == 1 {
                        if failure == "after" {
                            resource.write(replacement.unwrap())?;
                        }
                        return Err(AppError::io(
                            resource.path(),
                            std::io::Error::other("fixture"),
                        ));
                    }
                }
                Ok(())
            }),
            || McpService::delete_server(&state, "target"),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string().contains("native recovery"),
            failure == "external"
        );
        for (path, bytes) in before {
            let expected = if failure == "external"
                && path == crate::opencode_config::get_opencode_config_path()
            {
                Some(br#"{"external":true}"#.to_vec())
            } else {
                bytes
            };
            assert_eq!(fs::read(&path).ok(), expected, "{}", path.display());
        }
        assert_eq!(
            state.db.get_all_mcp_servers().unwrap()["target"].apps,
            all_apps()
        );
        assert_eq!(links(&state), original_links);
    }
}

#[cfg(unix)]
#[test]
fn shared_destinations_and_cross_app_links_survive_failed_deletion() {
    use std::os::unix::fs::symlink;
    for layout in ["same", "claude-link", "gemini-link"] {
        for failure in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let _env = TestEnvGuard::isolated(temp.path());
            let state = fixture(true);
            McpService::set_apps(&state, "target", all_apps()).unwrap();
            let claude = crate::config::get_claude_mcp_path();
            let gemini = crate::gemini_config::get_gemini_settings_path();
            let link = match layout {
                "same" => {
                    crate::settings::update_settings(crate::settings::AppSettings {
                        claude_config_dir: Some(
                            temp.path()
                                .join(".gemini/settings")
                                .to_string_lossy()
                                .into_owned(),
                        ),
                        ..Default::default()
                    })
                    .unwrap();
                    None
                }
                "claude-link" => Some((claude, gemini)),
                _ => Some((gemini, claude)),
            };
            if let Some((path, referent)) = &link {
                fs::remove_file(path).unwrap();
                symlink(referent, path).unwrap();
            }
            let before = observed_files();
            if failure {
                fail_at(&state, "commit");
            }
            let result = McpService::delete_server(&state, "target");
            assert_eq!(result.is_err(), failure, "{layout}: {result:?}");
            if failure {
                assert_files(&before);
                if let Some((path, referent)) = &link {
                    assert_eq!(fs::read_link(path).unwrap(), *referent);
                }
            } else {
                for (app, path) in paths() {
                    assert!(native_entries(&app, &path).get("target").is_none());
                }
            }
        }
    }
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn deletion_round_trips_real_consumer_state_without_removing_a_lite_peer() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = fixture(true);
    state.db.save_mcp_server(&server("cli-import")).unwrap();
    let peer_test = "consumer_coordination::mcp_lifecycle::mcp_lifecycle_in_cli_fixture";
    for app in McpService::supported_mcp_apps() {
        consumer_tests::LitePeer::run(temp.path(), peer_test, &format!("{}:enable", app.as_str()));
    }
    consumer_tests::LitePeer::run(
        temp.path(),
        "consumer_coordination::create_mcp_in_cli_fixture",
        "unused",
    );
    let peer =
        cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "lite-peer").unwrap();
    assert!(McpService::delete_server(&state, "cli-import").unwrap());
    assert_eq!(
        cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "lite-peer").unwrap(),
        peer
    );
    for (app, path) in paths() {
        assert!(native_entries(&app, &path).get("cli-import").is_none());
    }
    state.db.save_mcp_server(&server("cli-import")).unwrap();
    McpService::set_apps(&state, "cli-import", all_apps()).unwrap();
    consumer_tests::LitePeer::run(temp.path(), peer_test, "gemini:delete");
    assert!(!McpService::delete_server(&state, "cli-import").unwrap());
    for (app, path) in paths() {
        assert!(native_entries(&app, &path).get("cli-import").is_none());
    }
    assert_eq!(
        cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "lite-peer").unwrap(),
        peer
    );
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn deletion_excludes_a_real_lite_writer_until_commit_or_recovery_finishes() {
    for failure in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = fixture(true);
        McpService::set_apps(&state, "target", all_apps()).unwrap();
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
            || McpService::delete_server(&state, "target"),
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
fn deletion_uses_fresh_selections_and_preserves_a_new_peer() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = fixture(true);
    // A second state changes selections after the caller has cached the row.
    let fresh = AppState::new(state.db.clone());
    McpService::set_apps(&fresh, "target", all_apps()).unwrap();
    state.db.save_mcp_server(&server("new-peer")).unwrap();
    state
        .db
        .conn
        .lock()
        .unwrap()
        .execute_batch(
            "ALTER TABLE mcp_servers ADD COLUMN host_note TEXT DEFAULT 'keep';
         UPDATE mcp_servers SET enabled_grokbuild=1 WHERE id='new-peer';",
        )
        .unwrap();
    let peer = cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "new-peer")
        .unwrap()
        .unwrap();
    assert!(McpService::delete_server(&state, "target").unwrap());
    assert_eq!(
        cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "new-peer").unwrap(),
        Some(peer)
    );
    for (app, path) in paths() {
        assert!(
            native_entries(&app, &path).get("target").is_none(),
            "{app:?}"
        );
    }
    assert!(!state
        .db
        .get_all_mcp_servers()
        .unwrap()
        .contains_key("target"));
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
fn deletion_recovers_earlier_files_and_catalog_after_a_later_app_fails() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = fixture(true);
    McpService::set_apps(&state, "target", all_apps()).unwrap();
    fs::write(crate::gemini_config::get_gemini_settings_path(), "{invalid").unwrap();
    let before: Vec<_> = paths()
        .into_iter()
        .map(|(_, path)| {
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    let rows = cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap();
    let cache = serde_json::to_value(&*state.config.read().unwrap()).unwrap();
    assert!(McpService::delete_server(&state, "target").is_err());
    assert_eq!(
        cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap(),
        rows
    );
    assert_eq!(
        serde_json::to_value(&*state.config.read().unwrap()).unwrap(),
        cache
    );
    for (path, bytes) in before {
        assert_eq!(fs::read(&path).unwrap(), bytes, "{}", path.display());
    }
}
