use std::{fs, sync::Arc};

use serde_json::json;

use super::toggle::{NativeAction, NativeChange};
use super::{test_fixture::*, *};
use crate::test_support::TestEnvGuard;
use cc_switch_core::fs::{
    shared_live_config_lock_path, SharedLiveConfigLock, SharedLiveConfigLockError,
};

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}

fn enabled(id: &str) -> McpServer {
    let mut target = server(id);
    target.apps = all_apps();
    target
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

fn fail_at(state: &AppState, failure: &str) {
    let sql = match failure {
        "link" => "CREATE TRIGGER fixture_reject BEFORE INSERT ON mcp_native_links WHEN NEW.app_id='gemini' BEGIN SELECT RAISE(IGNORE); END;",
        "verification" => "CREATE TRIGGER fixture_reject AFTER INSERT ON mcp_native_links WHEN NEW.app_id='gemini' BEGIN UPDATE mcp_servers SET name='changed' WHERE id='peer'; END;",
        "commit" => "PRAGMA foreign_keys=ON; CREATE TABLE fixture_parent(id TEXT PRIMARY KEY); CREATE TABLE fixture_fk(id TEXT REFERENCES fixture_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fixture_reject BEFORE INSERT ON mcp_native_links WHEN NEW.app_id='gemini' BEGIN INSERT INTO fixture_fk VALUES('missing'); END;",
        _ => panic!("unknown fixture failure"),
    };
    state.db.conn.lock().unwrap().execute_batch(sql).unwrap();
    if failure == "commit" {
        // Store updates an existing native link without an INSERT conflict path.
        state.db.conn.lock().unwrap().execute_batch("CREATE TRIGGER fixture_reject_update AFTER UPDATE ON mcp_native_links WHEN NEW.app_id='gemini' BEGIN INSERT INTO fixture_fk VALUES('missing'); END;").unwrap();
    }
}

#[test]
fn full_sync_reads_fresh_catalog_without_saving_stale_cache() {
    let temp = tempdir();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = fixture(true);
    let mut target = enabled("target");
    target.server["command"] = json!("fresh-not-executed");
    state.db.save_mcp_server(&target).unwrap();
    state.db.save_mcp_server(&enabled("fresh-peer")).unwrap();
    let rows = cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap();
    McpService::sync_all_enabled(&state).unwrap();
    assert_eq!(
        cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap(),
        rows
    );
    for (app, path) in paths() {
        let entries = native_entries(&app, &path);
        assert!(entries["fresh-peer"].is_object(), "{app:?}");
        assert_eq!(
            entries["target"]["command"],
            if matches!(app, AppType::OpenCode) {
                json!(["fresh-not-executed"])
            } else {
                json!("fresh-not-executed")
            }
        );
    }
}

#[test]
fn a_later_invalid_entry_does_not_publish_a_partial_app_document() {
    let temp = tempdir();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = fixture(true);
    // Put the invalid entry last in the legacy cache's iteration order, so
    // this baseline deterministically observes publication of a valid prefix.
    {
        let mut config = state.config.write().unwrap();
        let servers = config.mcp.servers.as_mut().unwrap();
        let last = servers.keys().last().unwrap().clone();
        for target in servers.values_mut() {
            target.apps = all_apps();
            if target.id == last {
                target.server = json!(null);
            }
            state.db.save_mcp_server(target).unwrap();
        }
    }
    let before = observed_files();
    assert!(McpService::sync_all_enabled(&state).is_err());
    // Claude rejects null; Codex deliberately accepts it under its existing
    // tolerant conversion policy and may still complete its own App batch.
    let (path, bytes) = &before[0];
    assert_eq!(fs::read(path).ok(), *bytes);
}

#[test]
fn every_native_binding_batches_changes_and_rejects_a_bad_tail_before_publication() {
    for app in McpService::supported_mcp_apps() {
        let temp = tempdir();
        let _env = TestEnvGuard::isolated(temp.path());
        let _state = fixture(true);
        let before = observed_files();
        let spec = json!({"command":"not-executed"});
        let wrong_target = if app == AppType::Gemini {
            cc_switch_core::McpConfigTarget::Claude
        } else {
            cc_switch_core::McpConfigTarget::Gemini
        };
        let snapshot = wrong_target
            .capture_native_entry(r#"{"command":"old-not-executed"}"#)
            .unwrap();
        let trace = std::rc::Rc::new(std::cell::Cell::new(0));
        let calls = trace.clone();
        native_file::with_exchange_hook(
            Box::new(move |_, _| {
                calls.set(calls.get() + 1);
                Ok(())
            }),
            || {
                let mut native = toggle::observe_native(&app).unwrap();
                assert!(native.apply_batch(&[]).unwrap().is_empty());
                assert!(native
                    .apply_batch(&[
                        NativeChange {
                            id: "first",
                            server: &spec,
                            action: NativeAction::Sync,
                            previous_snapshot: None
                        },
                        NativeChange {
                            id: "last",
                            server: &spec,
                            action: NativeAction::Sync,
                            previous_snapshot: Some(&snapshot)
                        },
                    ])
                    .is_err());
                assert_eq!(trace.get(), 0);
                native.rollback().unwrap();
            },
        );
        assert_files(&before);
    }
}

#[test]
fn full_sync_publishes_once_per_app_and_preserves_large_unmanaged_content() {
    let temp = tempdir();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = fixture(true);
    for index in 0..48 {
        state
            .db
            .save_mcp_server(&enabled(&format!("fixture-{index:02}")))
            .unwrap();
    }
    let padding = "x".repeat(cc_switch_core::MAX_OPERATION_CONTENT_BYTES + 1);
    let claude = crate::config::get_claude_mcp_path();
    fs::write(&claude, json!({"fixture":true, "padding":padding, "mcpServers":{"native-only":{"server":null,"source":"keep"}}}).to_string()).unwrap();
    let rows = cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap();
    let cache = serde_json::to_value(&*state.config.read().unwrap()).unwrap();
    let trace = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let writes = trace.clone();
    native_file::with_exchange_hook(
        Box::new(move |resource, _| {
            writes.borrow_mut().push(resource.path().to_owned());
            Ok(())
        }),
        || McpService::sync_all_enabled(&state),
    )
    .unwrap();
    assert_eq!(
        *trace.borrow(),
        paths()
            .into_iter()
            .map(|(_, path)| path)
            .collect::<Vec<_>>()
    );
    for (app, path) in paths() {
        let entries = native_entries(&app, &path);
        for index in 0..48 {
            assert!(entries[format!("fixture-{index:02}")].is_object());
        }
    }
    let native: serde_json::Value = serde_json::from_slice(&fs::read(claude).unwrap()).unwrap();
    assert_eq!(native["padding"], padding);
    assert_eq!(
        native["mcpServers"]["native-only"],
        json!({"server":null,"source":"keep"})
    );
    assert_eq!(
        cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap(),
        rows
    );
    assert_eq!(
        serde_json::to_value(&*state.config.read().unwrap()).unwrap(),
        cache
    );
}

#[test]
fn empty_and_uninitialized_catalogs_do_not_touch_native_files_or_opaque_links() {
    let temp = tempdir();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = fixture(false);
    state.db.save_mcp_server(&enabled("target")).unwrap();
    state.db.conn.lock().unwrap().execute_batch("INSERT INTO mcp_native_links(server_id,app_id,native_snapshot) VALUES('target','gemini','future');").unwrap();
    let before_links = links(&state);
    let lock =
        SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(temp.path())).unwrap();
    McpService::sync_all_enabled(&state).unwrap();
    assert!(paths().iter().all(|(_, path)| !path.exists()));
    assert_eq!(links(&state), before_links);
    state.db.delete_mcp_server("target").unwrap();
    state.db.delete_mcp_server("peer").unwrap();
    for (_, path) in paths() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "invalid native config").unwrap();
    }
    let before = observed_files();
    McpService::sync_all_enabled(&state).unwrap();
    for app in [AppType::OpenClaw, AppType::Pi] {
        McpService::sync_enabled_for_app(&state, &app).unwrap();
    }
    assert_files(&before);
    drop(lock);
}

#[test]
#[allow(deprecated)]
fn targeted_and_legacy_enabled_only_sync_keep_their_distinct_removal_scope() {
    let temp = tempdir();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = fixture(true);
    state.db.save_mcp_server(&enabled("target")).unwrap();
    let gemini = crate::gemini_config::get_gemini_settings_path();
    fs::write(&gemini, json!({"fixture":true, "mcpServers":{"peer":{"command":"disabled-not-executed","trust":"keep"},"native-only":{"command":"unknown","source":"keep"}}}).to_string()).unwrap();
    state.db.conn.lock().unwrap().execute_batch("INSERT INTO mcp_native_links(server_id,app_id,native_snapshot) VALUES('peer','gemini','future');").unwrap();
    let before = observed_files();
    McpService::sync_enabled(&state, AppType::Gemini).unwrap();
    let entries = native_entries(&AppType::Gemini, &gemini);
    assert_eq!(entries["peer"]["trust"], "keep");
    assert!(entries["target"].is_object());
    assert!(McpService::sync_enabled_for_app(&state, &AppType::Gemini).is_err());
    state.db.conn.lock().unwrap().execute_batch("UPDATE mcp_native_links SET native_snapshot=NULL WHERE server_id='peer' AND app_id='gemini';").unwrap();
    McpService::sync_enabled_for_app(&state, &AppType::Gemini).unwrap();
    let entries = native_entries(&AppType::Gemini, &gemini);
    assert!(entries.get("peer").is_none());
    assert_eq!(
        entries["native-only"],
        json!({"command":"unknown","source":"keep"})
    );
    assert!(links(&state)
        .iter()
        .any(|(id, app, snapshot)| id == "peer" && app == "gemini" && snapshot.is_some()));
    for (path, bytes) in before {
        if path != gemini {
            assert_eq!(fs::read(path).ok(), bytes);
        }
    }
}

#[test]
fn unchanged_removal_snapshots_keep_their_raw_future_fields() {
    for app in [AppType::Claude, AppType::Gemini] {
        let temp = tempdir();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = fixture(true);
        let raw = format!(
            r#"{{ "target": "{}", "entry": {{ "command": "old-not-executed", "trust": "keep" }}, "futureMetadata": {{ "keep": true }} }}"#,
            app.as_str()
        );
        state.db.conn.lock().unwrap().execute(
            "INSERT INTO mcp_native_links(server_id,app_id,native_snapshot) VALUES('target',?1,?2)",
            rusqlite::params![app.as_str(), raw],
        ).unwrap();
        for targeted in [false, true, false] {
            if targeted {
                McpService::sync_enabled_for_app(&state, &app).unwrap();
            } else {
                McpService::sync_all_enabled(&state).unwrap();
            }
            assert!(links(&state).contains(&(
                "target".into(),
                app.as_str().into(),
                Some(raw.clone())
            )));
        }
        // Restoring the entry still consumes the saved snapshot.
        state.db.save_mcp_server(&enabled("target")).unwrap();
        McpService::sync_enabled_for_app(&state, &app).unwrap();
        assert!(links(&state).contains(&("target".into(), app.as_str().into(), None)));
        let path = paths()
            .into_iter()
            .find(|(candidate, _)| *candidate == app)
            .unwrap()
            .1;
        let entries = native_entries(&app, &path);
        assert_eq!(entries["target"]["command"], "not-executed");
        assert_eq!(entries["target"]["trust"], "keep");
    }
}

#[test]
fn one_app_database_failure_recovers_its_whole_batch_and_keeps_other_apps_committed() {
    for failure in ["link", "verification", "commit"] {
        let temp = tempdir();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = Arc::new(fixture(true));
        for id in ["target", "peer"] {
            state.db.save_mcp_server(&enabled(id)).unwrap();
        }
        state.db.conn.lock().unwrap().execute_batch("ALTER TABLE mcp_servers ADD COLUMN host_note TEXT DEFAULT 'keep'; UPDATE mcp_servers SET enabled_grokbuild=1; INSERT INTO mcp_native_links(server_id,app_id,native_snapshot) VALUES('target','grokbuild','future');").unwrap();
        let rows = cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap();
        let cache = serde_json::to_value(&*state.config.read().unwrap()).unwrap();
        let before = observed_files();
        fail_at(&state, failure);
        let observed = state.clone();
        let home = temp.path().to_owned();
        let trace = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let writes = trace.clone();
        let error = native_file::with_exchange_hook(
            Box::new(move |resource, _| {
                assert!(observed.config.try_write().is_ok());
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
            || McpService::sync_all_enabled(&state),
        )
        .unwrap_err();
        assert!(error.to_string().contains("gemini"));
        let expected = paths()
            .into_iter()
            .flat_map(|(app, path)| {
                if app == AppType::Gemini {
                    vec![path.clone(), path]
                } else {
                    vec![path]
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(*trace.borrow(), expected);
        for ((app, path), (_, bytes)) in paths().into_iter().zip(before) {
            if app == AppType::Gemini {
                assert_eq!(fs::read(path).ok(), bytes);
            } else {
                let entries = native_entries(&app, &path);
                assert!(entries["target"].is_object());
                assert!(entries["peer"].is_object());
            }
        }
        assert_eq!(
            cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap(),
            rows
        );
        assert_eq!(
            serde_json::to_value(&*state.config.read().unwrap()).unwrap(),
            cache
        );
        let links = links(&state);
        assert!(links.iter().all(|(_, app, _)| app != "gemini"));
        assert!(links.contains(&("target".into(), "grokbuild".into(), Some("future".into()))));
        state
            .db
            .conn
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER fixture_reject; DROP TRIGGER IF EXISTS fixture_reject_update; DROP TABLE IF EXISTS fixture_fk;")
            .unwrap();
        McpService::sync_all_enabled(&state).unwrap();
        let _released =
            SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(temp.path())).unwrap();
    }
}

#[test]
fn bulk_recovers_uncertain_publication_without_overwriting_external_recovery_edits() {
    for failure in ["before", "after", "external"] {
        let temp = tempdir();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = fixture(true);
        for id in ["target", "peer"] {
            state.db.save_mcp_server(&enabled(id)).unwrap();
        }
        let before = fs::read(crate::gemini_config::get_gemini_settings_path()).unwrap();
        if failure == "external" {
            fail_at(&state, "commit");
        }
        let mut calls = 0;
        let error = native_file::with_exchange_hook(
            Box::new(move |resource, replacement| {
                if resource.path() == crate::gemini_config::get_gemini_settings_path() {
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
            || McpService::sync_all_enabled(&state),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string().contains("native recovery"),
            failure == "external"
        );
        assert_eq!(
            fs::read(crate::gemini_config::get_gemini_settings_path()).unwrap(),
            if failure == "external" {
                br#"{"external":true}"#.to_vec()
            } else {
                before
            }
        );
        assert!(links(&state).iter().all(|(_, app, _)| app != "gemini"));
        for (app, path) in paths() {
            if app != AppType::Gemini {
                assert!(native_entries(&app, &path)["target"].is_object());
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn later_app_failure_restores_its_observation_including_an_earlier_committed_alias() {
    use std::os::unix::fs::symlink;
    for layout in ["same", "claude-link", "gemini-link"] {
        let temp = tempdir();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = fixture(true);
        let mut target = enabled("target");
        target.apps.gemini = false;
        state.db.save_mcp_server(&target).unwrap();
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
        // Ensure Gemini writes a link even when this layout has no target entry.
        state
            .db
            .conn
            .lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO mcp_native_links(server_id,app_id) VALUES('target','gemini');",
            )
            .unwrap();
        fail_at(&state, "commit");
        assert!(McpService::sync_all_enabled(&state).is_err());
        assert!(
            native_entries(&AppType::Claude, &crate::config::get_claude_mcp_path())["target"]
                .is_object()
        );
        if layout == "gemini-link" {
            let (path, referent) = link.unwrap();
            assert_eq!(fs::read_link(path).unwrap(), referent);
        }
        assert!(links(&state)
            .iter()
            .any(|(id, app, _)| id == "target" && app == "claude"));
    }
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn bulk_exchanges_native_snapshots_and_fresh_catalog_rows_with_real_lite() {
    let temp = tempdir();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = fixture(true);
    state.db.save_mcp_server(&enabled("cli-import")).unwrap();
    McpService::sync_all_enabled(&state).unwrap();
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
        fs::write(path, document.to_string()).unwrap();
        consumer_tests::LitePeer::run(temp.path(), peer_test, &format!("{}:disable", app.as_str()));
    }
    consumer_tests::LitePeer::run(
        temp.path(),
        "consumer_coordination::create_mcp_in_cli_fixture",
        "unused",
    );
    let peer =
        cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "lite-peer").unwrap();
    McpService::sync_all_enabled(&state).unwrap();
    let mut target = enabled("cli-import");
    target.server["command"] = json!("updated-not-executed");
    state.db.save_mcp_server(&target).unwrap();
    McpService::sync_all_enabled(&state).unwrap();
    target.apps.claude = false;
    target.apps.gemini = false;
    state.db.save_mcp_server(&target).unwrap();
    McpService::sync_all_enabled(&state).unwrap();
    for app in [AppType::Claude, AppType::Gemini] {
        consumer_tests::LitePeer::run(temp.path(), peer_test, &format!("{}:enable", app.as_str()));
        let path = paths()
            .into_iter()
            .find(|(candidate, _)| *candidate == app)
            .unwrap()
            .1;
        let entries = native_entries(&app, &path);
        assert_eq!(entries["cli-import"]["trust"], "native-only");
        assert_eq!(entries["cli-import"]["command"], "updated-not-executed");
    }
    assert_eq!(
        cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "lite-peer").unwrap(),
        peer
    );
    // Lite's create fixture leaves the peer disabled; sync must not activate it.
    assert!(native_entries(
        &AppType::Gemini,
        &crate::gemini_config::get_gemini_settings_path()
    )
    .get("lite-peer")
    .is_none());
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn bulk_excludes_real_lite_during_each_app_commit_and_recovery() {
    for failure in [false, true] {
        let temp = tempdir();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = fixture(true);
        state.db.save_mcp_server(&enabled("target")).unwrap();
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
            || McpService::sync_all_enabled(&state),
        );
        assert_eq!(result.is_err(), failure);
        consumer_tests::LitePeer::run(
            temp.path(),
            "consumer_coordination::native_switch_in_cli_fixture",
            "probe_released",
        );
    }
}
