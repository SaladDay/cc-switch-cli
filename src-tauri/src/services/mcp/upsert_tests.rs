use std::{fs, sync::Arc};

use serde_json::json;

use super::{test_fixture::*, *};
use crate::test_support::TestEnvGuard;
use cc_switch_core::fs::{
    shared_live_config_lock_path, SharedLiveConfigLock, SharedLiveConfigLockError,
};

fn tempdir() -> tempfile::TempDir {
    // Native receipts bind canonical parents, including macOS's /var alias.
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}

fn links(state: &AppState) -> Vec<(String, String, Option<String>)> {
    let conn = state.db.conn.lock().unwrap();
    let mut statement = conn.prepare("SELECT server_id,app_id,native_snapshot FROM mcp_native_links ORDER BY server_id,app_id").unwrap();
    statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn updated(id: &str) -> McpServer {
    let mut target = server(id);
    target.name = "updated".into();
    target.server = json!({"command":"updated-not-executed"});
    target.apps = all_apps();
    target.description = Some("description".into());
    target.homepage = Some("https://example.invalid".into());
    target.docs = Some("https://example.invalid/docs".into());
    target.tags = vec!["fixture".into()];
    target
}

fn fail_at(state: &AppState, point: &str, create: bool) {
    let operation = if create { "INSERT" } else { "UPDATE" };
    let sql = match point {
        "catalog" => format!("CREATE TRIGGER fixture_reject BEFORE {operation} ON mcp_servers WHEN NEW.name='updated' BEGIN SELECT RAISE(IGNORE); END;"),
        "verification" => format!("CREATE TRIGGER fixture_reject AFTER {operation} ON mcp_servers WHEN NEW.name='updated' BEGIN UPDATE mcp_servers SET name='changed' WHERE id='peer'; END;"),
        "link" => "CREATE TRIGGER fixture_reject BEFORE INSERT ON mcp_native_links WHEN NEW.app_id='hermes' BEGIN SELECT RAISE(IGNORE); END;".into(),
        "commit" => format!("PRAGMA foreign_keys=ON; CREATE TABLE fixture_parent(name TEXT PRIMARY KEY); CREATE TABLE fixture_fk(name TEXT REFERENCES fixture_parent(name) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fixture_reject AFTER {operation} ON mcp_servers WHEN NEW.name='updated' BEGIN INSERT INTO fixture_fk VALUES('missing'); END;"),
        _ => panic!("unknown fixture failure"),
    };
    state.db.conn.lock().unwrap().execute_batch(&sql).unwrap();
}

#[test]
fn upsert_preserves_fresh_peers_and_unowned_columns() {
    let temp = tempdir();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = fixture(true);
    state.db.save_mcp_server(&server("new-peer")).unwrap();
    state
        .db
        .conn
        .lock()
        .unwrap()
        .execute_batch(
            "ALTER TABLE mcp_servers ADD COLUMN host_note TEXT DEFAULT 'keep';
         UPDATE mcp_servers SET enabled_grokbuild=1 WHERE id='target';
         INSERT INTO mcp_native_links(server_id,app_id,native_snapshot) VALUES('target','grokbuild','future');",
        )
        .unwrap();
    let peer =
        cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "new-peer").unwrap();
    let mut target = server("target");
    target.name = "updated".into();
    target.server = json!({"command":"updated-not-executed"});
    target.apps = all_apps();
    McpService::upsert_server(&state, target.clone()).unwrap();
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
    assert!(links(&state).contains(&("target".into(), "grokbuild".into(), Some("future".into()))));
    assert_eq!(
        state.db.get_all_mcp_servers().unwrap()["target"].name,
        target.name
    );
    for (app, path) in paths() {
        assert_eq!(
            native_entries(&app, &path)["target"]["command"],
            if matches!(app, AppType::OpenCode) {
                json!(["updated-not-executed"])
            } else {
                json!("updated-not-executed")
            }
        );
    }
}

#[test]
fn upsert_later_native_failure_restores_catalog_cache_and_files() {
    for create in [false, true] {
        let temp = tempdir();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = fixture(true);
        fs::write(crate::hermes_config::get_hermes_config_path(), "[invalid").unwrap();
        let files = observed_files();
        let rows = cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap();
        let cache = serde_json::to_value(&*state.config.read().unwrap()).unwrap();
        let mut target = server(if create { "new-target" } else { "target" });
        target.name = "updated".into();
        target.apps = all_apps();
        assert!(McpService::upsert_server(&state, target).is_err());
        assert_eq!(
            cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap(),
            rows
        );
        assert_eq!(
            serde_json::to_value(&*state.config.read().unwrap()).unwrap(),
            cache
        );
        assert_files(&files);
    }
}

#[test]
fn create_and_update_cover_all_matrices_and_refresh_unchanged_enabled_apps() {
    for create in [false, true] {
        for mask in 0..(1 << McpService::supported_mcp_apps().count()) {
            let temp = tempdir();
            let _env = TestEnvGuard::isolated(temp.path());
            let state = fixture(true);
            McpService::set_apps(&state, "peer", all_apps()).unwrap();
            let peer_links = links(&state);
            let id = if create { "created" } else { "target" };
            if !create {
                // A peer enables the row after this caller cached disabled flags.
                let fresh = AppState::new(state.db.clone());
                McpService::set_apps(&fresh, id, all_apps()).unwrap();
            }
            let mut target = updated(id);
            for (bit, app) in McpService::supported_mcp_apps().enumerate() {
                target.apps.set_enabled_for(&app, mask & (1 << bit) != 0);
            }
            for command in ["first-not-executed", "second-not-executed"] {
                target.server["command"] = json!(command);
                McpService::upsert_server(&state, target.clone()).unwrap();
                assert_eq!(
                    links(&state)
                        .into_iter()
                        .filter(|(id, _, _)| id == "peer")
                        .collect::<Vec<_>>(),
                    peer_links
                );
                assert_eq!(
                    serde_json::to_value(&state.db.get_all_mcp_servers().unwrap()[id]).unwrap(),
                    serde_json::to_value(&target).unwrap()
                );
                assert_eq!(
                    serde_json::to_value(
                        &state.config.read().unwrap().mcp.servers.as_ref().unwrap()[id]
                    )
                    .unwrap(),
                    serde_json::to_value(&target).unwrap()
                );
                for (app, path) in paths() {
                    let entries = native_entries(&app, &path);
                    assert_eq!(
                        entries.get(id).is_some(),
                        target.apps.is_enabled_for(&app),
                        "{app:?} {mask}"
                    );
                    let expected_command = |command| {
                        if matches!(app, AppType::OpenCode) {
                            json!([command])
                        } else {
                            json!(command)
                        }
                    };
                    assert_eq!(entries["peer"]["command"], expected_command("not-executed"));
                    if target.apps.is_enabled_for(&app) {
                        assert_eq!(entries[id]["command"], expected_command(command));
                    }
                }
            }
        }
    }
}

#[test]
fn catalog_only_upserts_leave_uninitialized_and_unchanged_disabled_apps_alone() {
    let temp = tempdir();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = fixture(false);
    let lock =
        SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(temp.path())).unwrap();
    state.config.write().unwrap().mcp.servers = None;
    McpService::upsert_server(&state, updated("created")).unwrap();
    assert!(paths().iter().all(|(_, path)| !path.exists()));
    for (_, path) in paths() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "invalid native config").unwrap();
    }
    let before = observed_files();
    let mut target = updated("target");
    target.apps = McpApps::default();
    McpService::upsert_server(&state, target).unwrap();
    assert_files(&before);
    assert!(matches!(
        McpService::upsert_server(&state, updated("target")),
        Err(AppError::Conflict(_))
    ));
    assert_files(&before);
    drop(lock);
}

#[test]
fn upsert_retains_native_conversion_and_activation_policies() {
    for app in McpService::supported_mcp_apps() {
        for native_disabled in [false, true] {
            let temp = tempdir();
            let _env = TestEnvGuard::isolated(temp.path());
            let state = fixture(true);
            let path = paths()
                .into_iter()
                .find(|(candidate, _)| *candidate == app)
                .unwrap()
                .1;
            let original = if native_disabled && matches!(app, AppType::Hermes) {
                "fixture: true\nmcp_servers:\n  target:\n    command: old\n    enabled: false\n    timeout: 123\n".to_owned()
            } else {
                fs::read_to_string(&path).unwrap()
            };
            fs::write(&path, &original).unwrap();
            let mut target = updated("target");
            target.apps = McpApps::default();
            target.apps.set_enabled_for(&app, true);
            target.server = json!({"command":"updated-not-executed", "args":["arg"], "env":{"KEY":"fixture"}, "enabled":false, "timeout":3000});
            McpService::sync_server_to_app_internal(&MultiAppConfig::default(), &target, &app)
                .unwrap();
            let expected = native_entries(&app, &path)["target"].clone();
            fs::write(&path, original).unwrap();
            McpService::upsert_server(&state, target).unwrap();
            assert_eq!(
                native_entries(&app, &path)["target"],
                expected,
                "{app:?} {native_disabled}"
            );
            if matches!(app, AppType::Codex) || (native_disabled && matches!(app, AppType::Hermes))
            {
                assert_eq!(expected["enabled"], false);
                McpService::toggle_app(&state, "target", app.clone(), true).unwrap();
                assert_eq!(native_entries(&app, &path)["target"]["enabled"], true);
            }
        }
    }
}

#[test]
fn upsert_only_interprets_snapshots_for_native_apps_it_writes() {
    for app in McpService::supported_mcp_apps() {
        for initialized in [false, true] {
            let temp = tempdir();
            let _env = TestEnvGuard::isolated(temp.path());
            let state = fixture(initialized);
            state.db.conn.lock().unwrap().execute(
                "INSERT INTO mcp_native_links(server_id,app_id,native_snapshot) VALUES('target',?1,'future-or-invalid')",
                [app.as_str()],
            ).unwrap();
            let before = observed_files();
            let before_links = links(&state);
            let rows =
                cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap();
            let mut target = updated("target");
            target.apps = McpApps::default();
            target.apps.set_enabled_for(&app, true);
            let result = McpService::upsert_server(&state, target);
            assert_eq!(result.is_err(), initialized, "{app:?}");
            assert_files(&before);
            assert_eq!(links(&state), before_links);
            if initialized {
                assert_eq!(
                    cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap(),
                    rows
                );
            }
        }
    }
}

#[test]
fn upsert_database_failures_restore_files_links_and_cache_under_retained_guards() {
    for create in [false, true] {
        for failure in ["catalog", "verification", "link", "commit"] {
            let temp = tempdir();
            let _env = TestEnvGuard::isolated(temp.path());
            let state = Arc::new(fixture(true));
            let files = observed_files();
            let rows =
                cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap();
            let before_links = links(&state);
            let cache = serde_json::to_value(&*state.config.read().unwrap()).unwrap();
            fail_at(&state, failure, create);
            let trace = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let writes = trace.clone();
            let observed = state.clone();
            let home = temp.path().to_owned();
            let target = updated(if create { "created" } else { "target" });
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
                || McpService::upsert_server(&state, target.clone()),
            );
            assert!(result.is_err(), "{failure} create={create}");
            let trace = trace.borrow();
            let expected_writes = match failure {
                "catalog" => 0,
                // The next guarded write detects the peer changed by a trigger.
                "verification" => 1,
                _ => paths().len(),
            };
            assert_eq!(trace.len(), 2 * expected_writes);
            let (published, restored) = trace.split_at(expected_writes);
            assert_eq!(
                published.iter().rev().collect::<Vec<_>>(),
                restored.iter().collect::<Vec<_>>()
            );
            assert_files(&files);
            assert_eq!(
                cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap(),
                rows
            );
            assert_eq!(links(&state), before_links);
            assert_eq!(
                serde_json::to_value(&*state.config.read().unwrap()).unwrap(),
                cache
            );
            state
                .db
                .conn
                .lock()
                .unwrap()
                .execute_batch("DROP TRIGGER fixture_reject; DROP TABLE IF EXISTS fixture_fk;")
                .unwrap();
            McpService::upsert_server(&state, target).unwrap();
            let _released =
                SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(temp.path()))
                    .unwrap();
        }
    }
}

#[test]
fn upsert_recovers_uncertain_publication_and_preserves_external_recovery_changes() {
    for failure in ["before", "after", "external"] {
        let temp = tempdir();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = fixture(true);
        let before = observed_files();
        let rows = cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap();
        if failure == "external" {
            fail_at(&state, "commit", false);
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
            || McpService::upsert_server(&state, updated("target")),
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
            assert_eq!(fs::read(&path).ok(), expected);
        }
        assert_eq!(
            cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap(),
            rows
        );
        assert!(links(&state).is_empty());
    }
}

#[cfg(unix)]
#[test]
fn upsert_preserves_removal_before_sync_order_and_recovers_shared_paths_and_links() {
    use std::os::unix::fs::symlink;
    for layout in ["same", "claude-link", "gemini-link"] {
        for failure in [false, true] {
            let temp = tempdir();
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
                fail_at(&state, "commit", false);
            }
            let mut target = updated("target");
            target.apps.gemini = false;
            let trace = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let writes = trace.clone();
            let result = native_file::with_exchange_hook(
                Box::new(move |resource, _| {
                    writes.borrow_mut().push(resource.path().to_owned());
                    Ok(())
                }),
                || McpService::upsert_server(&state, target),
            );
            assert_eq!(result.is_err(), failure, "{layout}: {result:?}");
            if failure {
                assert_files(&before);
                if let Some((path, referent)) = &link {
                    assert_eq!(fs::read_link(path).unwrap(), *referent);
                }
            } else {
                // Gemini removal runs before the still-enabled Claude refresh.
                assert_eq!(
                    trace.borrow()[0],
                    crate::gemini_config::get_gemini_settings_path()
                );
                assert_eq!(
                    native_entries(&AppType::Claude, &crate::config::get_claude_mcp_path())
                        ["target"]["command"],
                    "updated-not-executed"
                );
            }
        }
    }
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn upsert_exchanges_snapshots_and_preserves_records_from_real_lite() {
    let temp = tempdir();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = fixture(true);
    McpService::upsert_server(&state, updated("cli-import")).unwrap();
    let peer_test = "consumer_coordination::mcp_lifecycle::mcp_lifecycle_in_cli_fixture";
    for app in [AppType::Claude, AppType::Gemini] {
        let path = paths()
            .into_iter()
            .find(|(candidate, _)| *candidate == app)
            .unwrap()
            .1;
        let mut document: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        document["mcpServers"]["cli-import"]["trust"] = json!("native-only");
        fs::write(&path, document.to_string()).unwrap();
        consumer_tests::LitePeer::run(temp.path(), peer_test, &format!("{}:disable", app.as_str()));
    }
    consumer_tests::LitePeer::run(
        temp.path(),
        "consumer_coordination::create_mcp_in_cli_fixture",
        "unused",
    );
    let peer =
        cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "lite-peer").unwrap();
    let mut target = updated("cli-import");
    target.server["command"] = json!("new-not-executed");
    McpService::upsert_server(&state, target.clone()).unwrap();
    target.apps.claude = false;
    target.apps.gemini = false;
    McpService::upsert_server(&state, target).unwrap();
    for app in [AppType::Claude, AppType::Gemini] {
        consumer_tests::LitePeer::run(temp.path(), peer_test, &format!("{}:enable", app.as_str()));
        let path = paths()
            .into_iter()
            .find(|(candidate, _)| *candidate == app)
            .unwrap()
            .1;
        let entries = native_entries(&app, &path);
        assert_eq!(entries["cli-import"]["trust"], "native-only");
        assert_eq!(entries["cli-import"]["command"], "new-not-executed");
    }
    assert_eq!(
        cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "lite-peer").unwrap(),
        peer
    );
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn upsert_excludes_real_lite_until_commit_or_recovery_finishes() {
    for create in [false, true] {
        for failure in [false, true] {
            let temp = tempdir();
            let _env = TestEnvGuard::isolated(temp.path());
            let state = fixture(true);
            if failure {
                fail_at(&state, "commit", create);
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
                || {
                    McpService::upsert_server(
                        &state,
                        updated(if create { "created" } else { "target" }),
                    )
                },
            );
            assert_eq!(result.is_err(), failure);
            consumer_tests::LitePeer::run(
                temp.path(),
                "consumer_coordination::native_switch_in_cli_fixture",
                "probe_released",
            );
        }
    }
}
