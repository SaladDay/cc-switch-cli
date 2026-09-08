use std::{fs, sync::Arc};

use cc_switch_core::fs::{
    shared_live_config_lock_path, SharedLiveConfigLock, SharedLiveConfigLockError,
};

use super::*;
use crate::{
    app_config::{AppType, McpApps, McpServer},
    database::Database,
    services::{mcp::native_file::with_exchange_hook, McpService},
    store::AppState,
    test_support::TestEnvGuard,
};

fn server(id: &str) -> McpServer {
    McpServer {
        id: id.into(),
        name: id.into(),
        server: json!({"command":"not-executed", "enabled":false}),
        apps: McpApps::default(),
        description: None,
        homepage: None,
        docs: None,
        tags: vec![],
    }
}

fn state(initialized: bool, contents: Option<&str>) -> AppState {
    if initialized {
        fs::create_dir_all(get_opencode_config_path().parent().unwrap()).unwrap();
    }
    if let Some(contents) = contents {
        fs::write(get_opencode_config_path(), contents).unwrap();
    }
    let db = Arc::new(Database::init().unwrap());
    db.save_mcp_server(&server("target")).unwrap();
    db.save_mcp_server(&server("peer")).unwrap();
    AppState::new(db)
}

fn toggle(state: &AppState, enabled: bool) -> Result<(), AppError> {
    McpService::toggle_app(state, "target", AppType::OpenCode, enabled)
}

fn native() -> Value {
    serde_json::from_slice(&fs::read(get_opencode_config_path()).unwrap()).unwrap()
}

fn unchanged_selection(state: &AppState) {
    assert!(
        !state.db.get_all_mcp_servers().unwrap()["target"]
            .apps
            .opencode
    );
    assert!(
        !state.config.read().unwrap().mcp.servers.as_ref().unwrap()["target"]
            .apps
            .opencode
    );
    assert!(cc_switch_store::read_mcp_native_link(
        &state.db.conn.lock().unwrap(),
        "target",
        "opencode"
    )
    .unwrap()
    .is_none());
}

#[test]
fn fresh_target_and_peer_survive_without_saving_other_catalogs() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(true, Some(r#"{"model":"keep"}"#));
    let mut fresh = server("target");
    fresh.name = "fresh".into();
    fresh.apps.codex = true;
    fresh.server["command"] = json!("fresh-not-executed");
    state.db.save_mcp_server(&fresh).unwrap();
    state.db.save_mcp_server(&server("later-peer")).unwrap();
    let conn = state.db.conn.lock().unwrap();
    conn.execute_batch("ALTER TABLE mcp_servers ADD COLUMN host_note TEXT DEFAULT 'keep'; INSERT INTO settings(key,value) VALUES('peer-fixture','keep');").unwrap();
    let peer = cc_switch_store::read_mcp_server_row(&conn, "later-peer").unwrap();
    drop(conn);
    toggle(&state, true).unwrap();
    let target = &state.db.get_all_mcp_servers().unwrap()["target"];
    assert!(target.apps.opencode && target.apps.codex);
    assert_eq!(target.name, "fresh");
    assert_eq!(
        native()["mcp"]["target"],
        json!({"type":"local","command":["fresh-not-executed"],"enabled":true})
    );
    assert_eq!(native()["model"], "keep");
    assert_eq!(
        state.config.read().unwrap().mcp.servers.as_ref().unwrap()["target"].name,
        "fresh"
    );
    let conn = state.db.conn.lock().unwrap();
    assert_eq!(
        cc_switch_store::read_mcp_server_row(&conn, "later-peer").unwrap(),
        peer
    );
    for sql in [
        "SELECT host_note FROM mcp_servers WHERE id='target'",
        "SELECT value FROM settings WHERE key='peer-fixture'",
    ] {
        assert_eq!(
            conn.query_row(sql, [], |row| row.get::<_, String>(0))
                .unwrap(),
            "keep"
        );
    }
}

#[test]
fn portable_conversion_and_repeated_toggles_preserve_siblings_and_order() {
    for (spec, expected) in [
        (
            json!({"command":42}),
            json!({"type":"local", "command":[""], "enabled":true}),
        ),
        (
            json!({"type":"http"}),
            json!({"type":"remote", "enabled":true}),
        ),
        (
            json!({"command":"not-executed", "args":["a",7], "env":{"N":7}, "cwd":"ignored", "enabled":false, "future":"ignored"}),
            json!({"type":"local", "command":["not-executed","a",7], "environment":{"N":7}, "enabled":true}),
        ),
        (
            json!({"type":"http", "url":"https://fixture.invalid/mcp", "headers":{"N":7}, "enabled":false}),
            json!({"type":"remote", "url":"https://fixture.invalid/mcp", "headers":{"N":7}, "enabled":true}),
        ),
        (
            json!({"type":"sse", "url":"https://fixture.invalid/sse", "headers":"ignored"}),
            json!({"type":"remote", "url":"https://fixture.invalid/sse", "enabled":true}),
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let original = r#"{"provider":{"unknown":[1,2]},"mcp":{"target":{"future":"old"},"first":42,"second":{"future":true},"last":null},"future":7}"#;
        let state = state(true, Some(original));
        let mut row = server("target");
        row.server = spec;
        state.db.save_mcp_server(&row).unwrap();
        let before = native();
        for enabled in [true, true, false, false, true] {
            toggle(&state, enabled).unwrap();
            let actual = native();
            assert_eq!(actual["mcp"].get("target"), enabled.then_some(&expected));
            assert_eq!(actual["provider"], before["provider"]);
            assert_eq!(actual["future"], before["future"]);
            for sibling in ["first", "second", "last"] {
                assert_eq!(actual["mcp"][sibling], before["mcp"][sibling]);
            }
            assert_eq!(
                actual["mcp"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .filter(|key| *key != "target")
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                ["first", "second", "last"]
            );
        }
    }
}

#[test]
fn missing_file_initialization_null_root_and_uninitialized_app_policies() {
    for initialized in [false, true] {
        for enabled in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let _env = TestEnvGuard::isolated(temp.path());
            let state = state(initialized, None);
            toggle(&state, enabled).unwrap();
            assert_eq!(
                state.db.get_all_mcp_servers().unwrap()["target"]
                    .apps
                    .opencode,
                enabled
            );
            assert_eq!(get_opencode_config_path().exists(), initialized);
            if initialized {
                assert_eq!(native()["$schema"], "https://opencode.ai/config.json");
                assert_eq!(native().get("mcp").is_some(), enabled);
            } else {
                assert!(!get_opencode_config_path().parent().unwrap().exists());
                assert!(cc_switch_store::read_mcp_native_link(
                    &state.db.conn.lock().unwrap(),
                    "target",
                    "opencode"
                )
                .unwrap()
                .is_none());
                state.db.delete_mcp_server("target").unwrap();
                toggle(&state, true).unwrap();
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
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(true, Some("null"));
    toggle(&state, false).unwrap();
    assert_eq!(native(), Value::Null);
    toggle(&state, true).unwrap();
    assert_eq!(native()["mcp"]["target"]["enabled"], true);
}

#[test]
fn malformed_json_and_containers_do_not_report_false_activation() {
    for source in [
        "{",
        "{ /* comment */ }",
        "42",
        "[]",
        r#"{"mcp":null}"#,
        r#"{"mcp":42}"#,
        r#"{"mcp":[]}"#,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state(true, Some(source));
        assert!(toggle(&state, true).is_err(), "{source}");
        unchanged_selection(&state);
        assert_eq!(
            fs::read_to_string(get_opencode_config_path()).unwrap(),
            source
        );
        let parsed = serde_json::from_str::<Value>(source);
        assert_eq!(toggle(&state, false).is_ok(), parsed.is_ok());
        if let Ok(before) = parsed {
            assert_eq!(native(), before);
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(true, None);
    fs::write(get_opencode_config_path(), [0xff, 0xfe]).unwrap();
    for enabled in [true, false] {
        assert!(matches!(toggle(&state, enabled), Err(AppError::Io { .. })));
        unchanged_selection(&state);
        assert_eq!(fs::read(get_opencode_config_path()).unwrap(), [0xff, 0xfe]);
    }
}

#[test]
fn invalid_specs_and_uninterpretable_snapshots_remain_unpublished() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(true, Some("{}"));
    for spec in [json!(null), json!({"type":"unknown"})] {
        let mut row = server("target");
        row.server = spec;
        state.db.save_mcp_server(&row).unwrap();
        assert!(toggle(&state, true).is_err());
        unchanged_selection(&state);
        assert_eq!(
            fs::read_to_string(get_opencode_config_path()).unwrap(),
            "{}"
        );
    }
    state.db.save_mcp_server(&server("target")).unwrap();
    let snapshot = serde_json::to_string(
        &McpConfigTarget::Gemini
            .capture_native_entry(r#"{"future":true}"#)
            .unwrap(),
    )
    .unwrap();
    let mut conn = state.db.conn.lock().unwrap();
    let mut transaction = cc_switch_store::McpTransactionGuard::begin(&mut conn).unwrap();
    transaction
        .upsert_native_link("target", "opencode", Some(&snapshot))
        .unwrap();
    transaction.commit().unwrap();
    let before = cc_switch_store::read_mcp_native_link(&conn, "target", "opencode").unwrap();
    drop(conn);
    for enabled in [true, false] {
        assert!(toggle(&state, enabled).is_err());
        assert_eq!(
            fs::read_to_string(get_opencode_config_path()).unwrap(),
            "{}"
        );
        assert!(
            !state.db.get_all_mcp_servers().unwrap()["target"]
                .apps
                .opencode
        );
        assert!(
            !state.config.read().unwrap().mcp.servers.as_ref().unwrap()["target"]
                .apps
                .opencode
        );
        assert_eq!(
            cc_switch_store::read_mcp_native_link(
                &state.db.conn.lock().unwrap(),
                "target",
                "opencode"
            )
            .unwrap(),
            before
        );
    }
}

fn install_failure(state: &AppState, failure: &str) {
    let conn = state.db.conn.lock().unwrap();
    conn.execute_batch(match failure {
        "link" => "CREATE TRIGGER fixture_reject BEFORE INSERT ON mcp_native_links BEGIN SELECT RAISE(IGNORE); END;",
        "selection" => "CREATE TRIGGER fixture_reject BEFORE UPDATE ON mcp_servers BEGIN SELECT RAISE(IGNORE); END;",
        "verification" => "CREATE TRIGGER fixture_reject AFTER UPDATE ON mcp_servers WHEN NEW.id='target' BEGIN UPDATE mcp_servers SET name='changed' WHERE id='peer'; END;",
        "commit" => "PRAGMA foreign_keys=ON; CREATE UNIQUE INDEX fixture_selection ON mcp_servers(id,enabled_opencode); CREATE TABLE fixture_fk(id TEXT, enabled INTEGER, FOREIGN KEY(id,enabled) REFERENCES mcp_servers(id,enabled_opencode) DEFERRABLE INITIALLY DEFERRED); INSERT INTO fixture_fk VALUES('target',0);",
        _ => panic!("unexpected failure"),
    }).unwrap();
}

#[test]
fn database_failures_restore_missing_and_large_documents_under_retained_guards() {
    let large = serde_json::to_string(
        &json!({"future":"x".repeat(cc_switch_core::MAX_OPERATION_CONTENT_BYTES + 1)}),
    )
    .unwrap();
    for original in [None, Some(large.as_str())] {
        for failure in ["link", "selection", "verification", "commit"] {
            let temp = tempfile::tempdir().unwrap();
            let _env = TestEnvGuard::isolated(temp.path());
            let state = Arc::new(state(true, original));
            install_failure(&state, failure);
            let rows =
                cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap();
            let home = temp.path().to_owned();
            let observed = state.clone();
            let count = std::rc::Rc::new(std::cell::Cell::new(0));
            let calls = count.clone();
            let result = with_exchange_hook(
                Box::new(move |_, _| {
                    calls.set(calls.get() + 1);
                    assert!(
                        observed.config.try_write().is_err()
                            && observed.db.conn.try_lock().is_err()
                    );
                    assert!(matches!(
                        SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(&home)),
                        Err(SharedLiveConfigLockError::Unavailable)
                    ));
                    let peer =
                        rusqlite::Connection::open(home.join(".cc-switch/cc-switch.db")).unwrap();
                    peer.busy_timeout(std::time::Duration::ZERO).unwrap();
                    assert_eq!(
                        peer.execute_batch("BEGIN IMMEDIATE")
                            .unwrap_err()
                            .sqlite_error_code(),
                        Some(rusqlite::ErrorCode::DatabaseBusy)
                    );
                    Ok(())
                }),
                || toggle(&state, true),
            );
            assert!(result.is_err(), "{failure}");
            assert_eq!(count.get(), 2, "{failure}");
            assert_eq!(
                fs::read(get_opencode_config_path()).ok().as_deref(),
                original.map(str::as_bytes)
            );
            unchanged_selection(&state);
            assert_eq!(
                cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap(),
                rows
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
            toggle(&state, true).unwrap();
        }
    }
}

#[test]
fn shared_lock_contention_and_external_changes_do_not_publish_selection() {
    for external_at in [0, 1, 2] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state(true, Some("{}"));
        if external_at == 0 {
            let _lock =
                SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(temp.path()))
                    .unwrap();
            assert!(matches!(toggle(&state, true), Err(AppError::Conflict(_))));
            assert_eq!(
                fs::read_to_string(get_opencode_config_path()).unwrap(),
                "{}"
            );
        } else {
            if external_at == 2 {
                install_failure(&state, "commit");
            }
            let mut calls = 0;
            let result = with_exchange_hook(
                Box::new(move |resource, _| {
                    calls += 1;
                    if calls == external_at {
                        fs::write(resource.path(), r#"{"external":"keep"}"#).unwrap();
                    }
                    Ok(())
                }),
                || toggle(&state, true),
            );
            assert!(result.is_err());
            if external_at == 2 {
                assert!(result.unwrap_err().to_string().contains("native recovery"));
            }
            assert_eq!(native(), json!({"external":"keep"}));
        }
        unchanged_selection(&state);
    }
}

#[cfg(unix)]
#[test]
fn leaf_link_replacement_retains_referent_and_existing_mode_policy() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(true, None);
    let source = temp.path().join("source.json");
    fs::write(&source, "{}").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
    symlink(&source, get_opencode_config_path()).unwrap();
    toggle(&state, true).unwrap();
    assert!(!get_opencode_config_path().is_symlink());
    assert_eq!(fs::read_to_string(&source).unwrap(), "{}");
    assert_eq!(
        fs::metadata(&source).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(
        fs::metadata(get_opencode_config_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
}

#[test]
fn long_legacy_ids_and_failed_disable_keep_existing_document_policy() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(true, Some("{}"));
    let id = "legacy".repeat(30);
    state.db.save_mcp_server(&server(&id)).unwrap();
    for enabled in [true, false] {
        McpService::toggle_app(&state, &id, AppType::OpenCode, enabled).unwrap();
        assert_eq!(native()["mcp"].get(&id).is_some(), enabled);
    }
    toggle(&state, true).unwrap();
    let before = fs::read(get_opencode_config_path()).unwrap();
    let link =
        cc_switch_store::read_mcp_native_link(&state.db.conn.lock().unwrap(), "target", "opencode")
            .unwrap();
    install_failure(&state, "selection");
    assert!(toggle(&state, false).is_err());
    assert_eq!(fs::read(get_opencode_config_path()).unwrap(), before);
    assert!(
        state.db.get_all_mcp_servers().unwrap()["target"]
            .apps
            .opencode
    );
    assert!(
        state.config.read().unwrap().mcp.servers.as_ref().unwrap()["target"]
            .apps
            .opencode
    );
    assert_eq!(
        cc_switch_store::read_mcp_native_link(&state.db.conn.lock().unwrap(), "target", "opencode")
            .unwrap(),
        link
    );
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn real_lite_writer_is_excluded_through_opencode_publication_and_commit_recovery() {
    use crate::services::mcp::consumer_tests::LitePeer;
    for failed in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state(true, Some("{}"));
        fs::write(temp.path().join("coordination-fixture"), "cli-lite-v1").unwrap();
        if failed {
            install_failure(&state, "commit");
        }
        let home = temp.path().to_owned();
        let count = std::rc::Rc::new(std::cell::Cell::new(0));
        let calls = count.clone();
        let result = with_exchange_hook(
            Box::new(move |_, _| {
                LitePeer::run(
                    &home,
                    "consumer_coordination::native_switch_in_cli_fixture",
                    "probe_locked",
                );
                calls.set(calls.get() + 1);
                Ok(())
            }),
            || toggle(&state, true),
        );
        assert_eq!(result.is_err(), failed);
        assert_eq!(count.get(), if failed { 2 } else { 1 });
        LitePeer::run(
            temp.path(),
            "consumer_coordination::native_switch_in_cli_fixture",
            "probe_released",
        );
    }
}
