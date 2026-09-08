use std::{fs, sync::Arc};

use cc_switch_core::fs::{
    shared_live_config_lock_path, SharedLiveConfigLock, SharedLiveConfigLockError,
};
use serde_json::{json, Value};

use super::*;
use crate::{
    app_config::{McpApps, McpServer},
    test_support::TestEnvGuard,
};

fn server(id: &str) -> McpServer {
    McpServer {
        id: id.into(),
        name: id.into(),
        server: json!({"command":"not-executed", "timeout":60000}),
        apps: McpApps::default(),
        description: None,
        homepage: None,
        docs: None,
        tags: vec![],
    }
}

fn state(home: &std::path::Path, initialized: bool) -> AppState {
    if initialized {
        fs::create_dir_all(get_gemini_settings_path().parent().unwrap()).unwrap();
        fs::write(
            get_gemini_settings_path(),
            r#"{"fixture":true,"mcpServers":{}}"#,
        )
        .unwrap();
    }
    let db = Arc::new(Database::init().unwrap());
    db.save_mcp_server(&server("target")).unwrap();
    db.save_mcp_server(&server("peer")).unwrap();
    fs::write(home.join("coordination-fixture"), "cli-lite-v1").unwrap();
    AppState::new(db)
}

fn native() -> Value {
    serde_json::from_slice(&fs::read(get_gemini_settings_path()).unwrap()).unwrap()
}

#[test]
fn toggle_reads_fresh_target_and_preserves_peer_fields_and_catalogs() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(temp.path(), true);
    let mut fresh = server("target");
    fresh.name = "Fresh target".into();
    fresh.server = json!({"command":"fresh-not-executed", "timeout":70000});
    fresh.apps.codex = true;
    state.db.save_mcp_server(&fresh).unwrap();
    state
        .db
        .conn
        .lock()
        .unwrap()
        .execute_batch(
            "ALTER TABLE mcp_servers ADD COLUMN host_note TEXT DEFAULT 'keep';
         INSERT INTO settings(key,value) VALUES('peer-fixture','keep');",
        )
        .unwrap();
    let peer = cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "peer")
        .unwrap()
        .unwrap();
    McpService::toggle_app(&state, "target", AppType::Gemini, true).unwrap();
    let rows = state.db.get_all_mcp_servers().unwrap();
    assert!(rows["target"].apps.gemini && rows["target"].apps.codex);
    assert_eq!(rows["target"].name, "Fresh target");
    assert_eq!(native()["mcpServers"]["target"], fresh.server);
    assert_eq!(native()["fixture"], true);
    assert_eq!(
        state.config.read().unwrap().mcp.servers.as_ref().unwrap()["target"].name,
        "Fresh target"
    );
    let conn = state.db.conn.lock().unwrap();
    assert_eq!(
        cc_switch_store::read_mcp_server_row(&conn, "peer")
            .unwrap()
            .unwrap(),
        peer
    );
    assert_eq!(
        conn.query_row(
            "SELECT host_note FROM mcp_servers WHERE id='target'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "keep"
    );
    assert_eq!(
        conn.query_row(
            "SELECT value FROM settings WHERE key='peer-fixture'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "keep"
    );
}

#[test]
fn toggle_initializes_shared_links_without_lite_and_restores_host_native_fields() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(temp.path(), true);
    // Legacy IDs and large host-owned fields are not subjected to document API limits.
    let id = format!(" {} ", "x".repeat(130));
    state.db.save_mcp_server(&server(&id)).unwrap();
    let entry = json!({"command":"not-executed", "timeout":123456,
        "trust":"keep", "server":{"native-only":true}, "name":"native-only",
        "private":"x".repeat(cc_switch_core::MAX_OPERATION_CONTENT_BYTES + 1)});
    fs::write(
        get_gemini_settings_path(),
        json!({"mcpServers":{&id:entry},"fixture":true}).to_string(),
    )
    .unwrap();
    for _ in 0..2 {
        McpService::toggle_app(&state, &id, AppType::Gemini, false).unwrap();
        assert!(native()["mcpServers"].get(&id).is_none());
        let link =
            cc_switch_store::read_mcp_native_link(&state.db.conn.lock().unwrap(), &id, "gemini")
                .unwrap()
                .unwrap();
        assert!(link.native_snapshot.is_some());
    }
    McpService::toggle_app(&state, &id, AppType::Gemini, true).unwrap();
    assert_eq!(native()["mcpServers"][&id], entry);
    assert!(
        cc_switch_store::read_mcp_native_link(&state.db.conn.lock().unwrap(), &id, "gemini")
            .unwrap()
            .unwrap()
            .native_snapshot
            .is_none()
    );
}

#[test]
fn restored_entry_keeps_catalog_timeout_policy_without_reencoding() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(temp.path(), true);
    for wrapped in [false, true] {
        let mut target = server("target");
        let spec = json!({"command":"not-executed", "startup_timeout_sec":1,
            "tool_timeout_sec":1, "name":"catalog-only", "enabled":false});
        target.server = if wrapped {
            json!({"server":spec,"name":"outer-catalog-only"})
        } else {
            spec
        };
        state.db.save_mcp_server(&target).unwrap();
        fs::write(get_gemini_settings_path(), "{}").unwrap();
        McpService::toggle_app(&state, "target", AppType::Gemini, true).unwrap();
        let ordinary = native()["mcpServers"]["target"].clone();
        assert_eq!(ordinary, json!({"command":"not-executed", "timeout":1000}));
        fs::write(
            get_gemini_settings_path(),
            json!({"fixture":true,
            "mcpServers":{"target":{"command":"old", "trust":true,
                "name":"native-only", "server":{"native-only":true}}}})
            .to_string(),
        )
        .unwrap();
        McpService::toggle_app(&state, "target", AppType::Gemini, false).unwrap();
        McpService::toggle_app(&state, "target", AppType::Gemini, true).unwrap();
        let mut expected = ordinary;
        expected["trust"] = json!(true);
        expected["name"] = json!("native-only");
        expected["server"] = json!({"native-only":true});
        assert_eq!(
            native()["mcpServers"]["target"],
            expected,
            "wrapped={wrapped}"
        );
        assert_eq!(native()["fixture"], true);
    }
}

#[test]
fn toggle_preserves_missing_target_and_uninitialized_app_policy() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(temp.path(), false);
    for enabled in [true, false, true] {
        McpService::toggle_app(&state, "missing", AppType::Gemini, enabled).unwrap();
        McpService::toggle_app(&state, "target", AppType::Gemini, enabled).unwrap();
        assert_eq!(
            state.db.get_all_mcp_servers().unwrap()["target"]
                .apps
                .gemini,
            enabled
        );
        assert!(!get_gemini_settings_path().parent().unwrap().exists());
        assert!(cc_switch_store::read_mcp_native_link(
            &state.db.conn.lock().unwrap(),
            "target",
            "gemini"
        )
        .unwrap()
        .is_none());
    }
    state.db.delete_mcp_server("target").unwrap();
    McpService::toggle_app(&state, "target", AppType::Gemini, true).unwrap();
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
fn repeated_toggle_still_repairs_native_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(temp.path(), true);
    for _ in 0..2 {
        fs::write(get_gemini_settings_path(), "{}").unwrap();
        McpService::toggle_app(&state, "target", AppType::Gemini, true).unwrap();
        assert_eq!(native()["mcpServers"]["target"], server("target").server);
    }
}

#[test]
fn single_toggle_preserves_raw_siblings_for_all_transitions() {
    for (connection, expected) in [
        (
            json!({"command":"not-executed", "startup_timeout_sec":1, "tool_timeout_sec":2}),
            json!({"command":"not-executed", "timeout":2000}),
        ),
        (
            json!({"type":"http", "url":"https://example.invalid/mcp", "headers":{"X-Test":"fixture"}}),
            json!({"httpUrl":"https://example.invalid/mcp", "headers":{"X-Test":"fixture"}, "timeout":60000}),
        ),
    ] {
        for wrapped in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let _env = TestEnvGuard::isolated(temp.path());
            let state = state(temp.path(), true);
            let mut target = server("target");
            target.server = if wrapped {
                json!({"server":connection,"name":"catalog-only"})
            } else {
                connection.clone()
            };
            state.db.save_mcp_server(&target).unwrap();
            let siblings = json!({
                "plain":{"command":"not-executed-sibling"},
                "target":{"command":"old-not-executed"},
                "opaque":{"httpUrl":"native-only", "type":"future", "enabled":false,
                    "name":"native-name", "server":{"native":true}, "timeout":"leave-as-is"},
                "null":null, "scalar":42, "list":["future-format"]
            });
            let mut fixture = json!({"security":{"keep":true}, "mcpServers":siblings});
            fs::write(get_gemini_settings_path(), fixture.to_string()).unwrap();
            fixture["mcpServers"]
                .as_object_mut()
                .unwrap()
                .shift_remove("target");
            for enabled in [true, true, false, false, true] {
                McpService::toggle_app(&state, "target", AppType::Gemini, enabled).unwrap();
                let mut actual = native();
                let target = actual["mcpServers"]
                    .as_object_mut()
                    .unwrap()
                    .shift_remove("target");
                assert_eq!(target, enabled.then(|| expected.clone()));
                assert_eq!(actual, fixture, "enabled={enabled}, wrapped={wrapped}");
                // Unchanged entries retain field order as well as values.
                assert_eq!(actual.to_string(), fixture.to_string());
                assert_eq!(
                    state.db.get_all_mcp_servers().unwrap()["target"]
                        .apps
                        .gemini,
                    enabled
                );
            }
        }
    }
}

#[test]
fn invalid_target_keeps_raw_document_catalog_and_links() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(temp.path(), true);
    let mut target = server("target");
    target.server = json!({"server":null});
    state.db.save_mcp_server(&target).unwrap();
    let before = "{ \"mcpServers\": {\"sibling\": null}, \"fixture\": true }\n";
    fs::write(get_gemini_settings_path(), before).unwrap();
    let rows = cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap();
    assert!(McpService::toggle_app(&state, "target", AppType::Gemini, true).is_err());
    assert_eq!(
        fs::read(get_gemini_settings_path()).unwrap(),
        before.as_bytes()
    );
    assert_eq!(
        cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap(),
        rows
    );
    assert!(cc_switch_store::read_mcp_native_link(
        &state.db.conn.lock().unwrap(),
        "target",
        "gemini"
    )
    .unwrap()
    .is_none());
    assert!(
        !state.config.read().unwrap().mcp.servers.as_ref().unwrap()["target"]
            .apps
            .gemini
    );
}

#[test]
fn toggle_rejects_shared_lock_contention_without_mutating_state() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(temp.path(), true);
    let before = fs::read(get_gemini_settings_path()).unwrap();
    let rows = cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap();
    let guard =
        SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(temp.path())).unwrap();
    assert!(matches!(
        McpService::toggle_app(&state, "target", AppType::Gemini, true),
        Err(AppError::Conflict(_))
    ));
    assert_eq!(fs::read(get_gemini_settings_path()).unwrap(), before);
    assert_eq!(
        cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap(),
        rows
    );
    assert!(
        !state.config.read().unwrap().mcp.servers.as_ref().unwrap()["target"]
            .apps
            .gemini
    );
    drop(guard);
    McpService::toggle_app(&state, "target", AppType::Gemini, true).unwrap();
}

#[test]
fn toggle_recovers_sqlite_abort_and_preserves_external_changes_during_recovery() {
    for external_change in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state(temp.path(), true);
        state.db.conn.lock().unwrap().execute_batch(
            "CREATE TRIGGER fixture_abort BEFORE UPDATE ON mcp_servers BEGIN SELECT RAISE(ROLLBACK, 'fixture abort'); END;"
        ).unwrap();
        let before = fs::read(get_gemini_settings_path()).unwrap();
        let mut calls = 0;
        let result = crate::gemini_config::operation::with_hook(
            Box::new(move |_, _| {
                calls += 1;
                if external_change && calls == 2 {
                    fs::write(get_gemini_settings_path(), r#"{"external":"keep"}"#).unwrap();
                }
                Ok(())
            }),
            || McpService::toggle_app(&state, "target", AppType::Gemini, true),
        );
        let error = result.unwrap_err();
        if external_change {
            assert!(error.to_string().contains("native recovery"));
            assert_eq!(native(), json!({"external":"keep"}));
        } else {
            assert_eq!(fs::read(get_gemini_settings_path()).unwrap(), before);
        }
        assert!(
            !state.db.get_all_mcp_servers().unwrap()["target"]
                .apps
                .gemini
        );
        assert!(
            !state.config.read().unwrap().mcp.servers.as_ref().unwrap()["target"]
                .apps
                .gemini
        );
        assert!(cc_switch_store::read_mcp_native_link(
            &state.db.conn.lock().unwrap(),
            "target",
            "gemini"
        )
        .unwrap()
        .is_none());
        let _released =
            SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(temp.path())).unwrap();
    }
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn real_lite_native_writer_is_excluded_during_mcp_publication_and_recovery() {
    use crate::services::mcp::consumer_tests::LitePeer;
    for failed_commit in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state(temp.path(), true);
        if failed_commit {
            state.db.conn.lock().unwrap().execute_batch("PRAGMA foreign_keys=ON;
                CREATE UNIQUE INDEX fixture_selection ON mcp_servers(id,enabled_gemini);
                CREATE TABLE fixture_fk(id TEXT, enabled INTEGER, FOREIGN KEY(id,enabled) REFERENCES mcp_servers(id,enabled_gemini) DEFERRABLE INITIALLY DEFERRED);
                INSERT INTO fixture_fk VALUES('target',0);").unwrap();
        }
        let home = temp.path().to_owned();
        let count = std::rc::Rc::new(std::cell::Cell::new(0));
        let calls = count.clone();
        let result = crate::gemini_config::operation::with_hook(
            Box::new(move |_, _| {
                let before = fs::read(get_gemini_settings_path()).unwrap();
                LitePeer::run(
                    &home,
                    "consumer_coordination::native_switch_in_cli_fixture",
                    "probe_locked",
                );
                assert_eq!(fs::read(get_gemini_settings_path()).unwrap(), before);
                calls.set(calls.get() + 1);
                Ok(())
            }),
            || McpService::toggle_app(&state, "target", AppType::Gemini, true),
        );
        assert_eq!(result.is_err(), failed_commit);
        assert_eq!(count.get(), if failed_commit { 2 } else { 1 });
        let before = fs::read(get_gemini_settings_path()).unwrap();
        LitePeer::run(
            temp.path(),
            "consumer_coordination::native_switch_in_cli_fixture",
            "probe_released",
        );
        assert_eq!(fs::read(get_gemini_settings_path()).unwrap(), before);
    }
}

#[test]
fn toggle_retains_both_guards_during_failure_recovery_and_releases_for_retry() {
    for failure in ["native", "link", "selection", "verification", "commit"] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = Arc::new(state(temp.path(), true));
        let conn = state.db.conn.lock().unwrap();
        match failure {
            "link" => conn.execute_batch("CREATE TRIGGER fixture_reject BEFORE INSERT ON mcp_native_links BEGIN SELECT RAISE(IGNORE); END;").unwrap(),
            "selection" => conn.execute_batch("CREATE TRIGGER fixture_reject BEFORE UPDATE ON mcp_servers BEGIN SELECT RAISE(IGNORE); END;").unwrap(),
            "verification" => conn.execute_batch("CREATE TRIGGER fixture_reject AFTER UPDATE ON mcp_servers WHEN NEW.id='target' BEGIN UPDATE mcp_servers SET name='changed' WHERE id='peer'; END;").unwrap(),
            "commit" => conn.execute_batch("PRAGMA foreign_keys=ON;
                CREATE UNIQUE INDEX fixture_selection ON mcp_servers(id,enabled_gemini);
                CREATE TABLE fixture_fk(id TEXT, enabled INTEGER, FOREIGN KEY(id,enabled) REFERENCES mcp_servers(id,enabled_gemini) DEFERRABLE INITIALLY DEFERRED);
                INSERT INTO fixture_fk VALUES('target',0);").unwrap(),
            _ => {}
        }
        let rows = cc_switch_store::read_mcp_server_rows(&conn).unwrap();
        drop(conn);
        let before = fs::read(get_gemini_settings_path()).unwrap();
        let home = temp.path().to_owned();
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let count = calls.clone();
        let observed = state.clone();
        let result = crate::gemini_config::operation::with_hook(
            Box::new(move |_, _| {
                assert!(observed.config.try_write().is_err());
                assert!(observed.db.conn.try_lock().is_err());
                assert!(matches!(
                    SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(&home)),
                    Err(SharedLiveConfigLockError::Unavailable)
                ));
                let peer =
                    rusqlite::Connection::open(home.join(".cc-switch/cc-switch.db")).unwrap();
                peer.busy_timeout(std::time::Duration::ZERO).unwrap();
                let error = peer.execute_batch("BEGIN IMMEDIATE").unwrap_err();
                assert_eq!(
                    error.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy)
                );
                count.set(count.get() + 1);
                if failure == "native" && count.get() == 1 {
                    return Err(AppError::io(
                        get_gemini_settings_path(),
                        std::io::Error::other("fixture publication failure"),
                    ));
                }
                Ok(())
            }),
            || McpService::toggle_app(&state, "target", AppType::Gemini, true),
        );
        assert!(result.is_err(), "{failure}");
        assert!(
            calls.get() >= if failure == "native" { 1 } else { 2 },
            "{failure}"
        );
        assert_eq!(fs::read(get_gemini_settings_path()).unwrap(), before);
        assert!(
            !state.config.read().unwrap().mcp.servers.as_ref().unwrap()["target"]
                .apps
                .gemini
        );
        let conn = state.db.conn.lock().unwrap();
        assert_eq!(cc_switch_store::read_mcp_server_rows(&conn).unwrap(), rows);
        assert!(
            cc_switch_store::read_mcp_native_link(&conn, "target", "gemini")
                .unwrap()
                .is_none()
        );
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS fixture_reject; DROP TABLE IF EXISTS fixture_fk;",
        )
        .unwrap();
        drop(conn);
        let guard =
            SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(temp.path())).unwrap();
        drop(guard);
        McpService::toggle_app(&state, "target", AppType::Gemini, true).unwrap();
    }
}
