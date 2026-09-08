use std::{fs, path::Path, sync::Arc};

use cc_switch_core::fs::{
    shared_live_config_lock_path, SharedLiveConfigLock, SharedLiveConfigLockError,
};
use serde_json::{json, Value};

use super::*;
use crate::{
    app_config::{AppType, McpApps, McpServer},
    database::Database,
    services::{mcp::native_file::with_exchange_hook, McpService},
    settings::{update_settings, AppSettings},
    store::AppState,
    test_support::TestEnvGuard,
};

fn server(id: &str) -> McpServer {
    McpServer {
        id: id.into(),
        name: id.into(),
        server: json!({"command":"not-executed"}),
        apps: McpApps::default(),
        description: None,
        homepage: None,
        docs: None,
        tags: vec![],
    }
}

fn state(home: &Path, initialized: bool) -> AppState {
    if initialized {
        fs::create_dir_all(home.join(".claude")).unwrap();
    }
    let db = Arc::new(Database::init().unwrap());
    for id in ["target", "peer"] {
        db.save_mcp_server(&server(id)).unwrap();
    }
    fs::write(home.join("coordination-fixture"), "cli-lite-v1").unwrap();
    AppState::new(db)
}

fn toggle(state: &AppState, enabled: bool) -> Result<(), AppError> {
    McpService::toggle_app(state, "target", AppType::Claude, enabled)
}

fn native() -> Value {
    serde_json::from_slice(&fs::read(get_claude_mcp_path()).unwrap()).unwrap()
}

fn override_dir(home: &Path) {
    update_settings(AppSettings {
        claude_config_dir: Some(home.join("nested/custom").to_string_lossy().into_owned()),
        ..AppSettings::default()
    })
    .unwrap();
    assert_eq!(get_claude_mcp_path(), home.join("nested/custom.json"));
}

#[test]
fn fresh_target_preserves_peer_catalog_fields_and_native_siblings() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(temp.path(), true);
    let mut fresh = server("target");
    fresh.name = "Fresh target".into();
    fresh.apps.codex = true;
    fresh.server = json!({"server":{"command":"fresh", "name":"catalog", "server":{"extension":true}}, "name":"outer"});
    state.db.save_mcp_server(&fresh).unwrap();
    state.db.conn.lock().unwrap().execute_batch("ALTER TABLE mcp_servers ADD COLUMN host_note TEXT DEFAULT 'keep'; INSERT INTO settings(key,value) VALUES('peer-fixture','keep');").unwrap();
    let peer =
        cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "peer").unwrap();
    let fixture = json!({"projects":{"keep":true}, "mcpServers":{"sibling":{"server":42,"enabled":false,"name":"native"}, "scalar":42}});
    fs::write(get_claude_mcp_path(), fixture.to_string()).unwrap();
    for enabled in [true, true, false, false, true] {
        toggle(&state, enabled).unwrap();
        let mut actual = native();
        assert_eq!(
            actual["mcpServers"]
                .as_object_mut()
                .unwrap()
                .shift_remove("target"),
            enabled.then(|| json!({"command":"fresh", "server":{"extension":true}}))
        );
        assert_eq!(actual.to_string(), fixture.to_string());
        let target = state
            .db
            .get_all_mcp_servers()
            .unwrap()
            .shift_remove("target")
            .unwrap();
        assert_eq!(target.apps.claude, enabled);
        assert!(target.apps.codex);
        assert_eq!(target.name, "Fresh target");
    }
    let conn = state.db.conn.lock().unwrap();
    assert_eq!(
        cc_switch_store::read_mcp_server_row(&conn, "peer").unwrap(),
        peer
    );
    assert_eq!(
        conn.query_row(
            "SELECT host_note FROM mcp_servers WHERE id='target'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "keep"
    );
    assert_eq!(
        conn.query_row(
            "SELECT value FROM settings WHERE key='peer-fixture'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "keep"
    );
}

#[test]
fn snapshot_restores_native_extensions_and_current_catalog_connection() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(temp.path(), true);
    let id = format!(" {} ", "x".repeat(130));
    let mut target = server(&id);
    target.server =
        json!({"url":"https://example.invalid", "headers":{"new":"value"}, "name":"catalog"});
    state.db.save_mcp_server(&target).unwrap();
    let entry = json!({"command":"old", "args":["old"], "env":{"old":"value"}, "server":{"native":true}, "name":"native",
        "trust":true, "large":"x".repeat(cc_switch_core::MAX_OPERATION_CONTENT_BYTES + 1)});
    fs::write(
        get_claude_mcp_path(),
        json!({"mcpServers":{&id:entry},"fixture":true}).to_string(),
    )
    .unwrap();
    for _ in 0..2 {
        McpService::toggle_app(&state, &id, AppType::Claude, false).unwrap();
        assert!(native()["mcpServers"].get(&id).is_none());
        assert!(cc_switch_store::read_mcp_native_link(
            &state.db.conn.lock().unwrap(),
            &id,
            "claude"
        )
        .unwrap()
        .unwrap()
        .native_snapshot
        .is_some());
    }
    McpService::toggle_app(&state, &id, AppType::Claude, true).unwrap();
    let restored = native();
    let mut expected = entry;
    for key in ["command", "args", "env"] {
        expected.as_object_mut().unwrap().remove(key);
    }
    expected["url"] = target.server["url"].clone();
    expected["headers"] = target.server["headers"].clone();
    assert_eq!(restored["mcpServers"][&id], expected);
    assert_eq!(restored["fixture"], true);
    assert!(
        cc_switch_store::read_mcp_native_link(&state.db.conn.lock().unwrap(), &id, "claude")
            .unwrap()
            .unwrap()
            .native_snapshot
            .is_none()
    );
}

#[test]
fn invalid_documents_and_catalog_entries_do_not_publish() {
    for bytes in [
        b"".as_slice(),
        b"{invalid",
        b"[]",
        b"null",
        b"\xff",
        b"{/*comment*/}",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state(temp.path(), true);
        fs::write(get_claude_mcp_path(), bytes).unwrap();
        for enabled in [true, false] {
            assert!(toggle(&state, enabled).is_err());
            assert_eq!(fs::read(get_claude_mcp_path()).unwrap(), bytes);
            assert!(
                !state.db.get_all_mcp_servers().unwrap()["target"]
                    .apps
                    .claude
            );
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(temp.path(), true);
    for spec in [json!(null), json!({"server":null})] {
        let mut target = server("target");
        target.server = spec;
        state.db.save_mcp_server(&target).unwrap();
        assert!(toggle(&state, true).is_err());
        assert!(!get_claude_mcp_path().exists());
    }
}

#[test]
fn missing_and_uninitialized_targets_and_malformed_collections_keep_host_policy() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(temp.path(), false);
    for enabled in [true, false] {
        toggle(&state, enabled).unwrap();
        assert!(!get_claude_mcp_path().exists());
        assert!(!temp.path().join(".claude").exists());
        assert_eq!(
            state.db.get_all_mcp_servers().unwrap()["target"]
                .apps
                .claude,
            enabled
        );
    }
    fs::create_dir_all(temp.path().join(".claude")).unwrap();
    for section in [json!(null), json!([]), json!(42)] {
        fs::write(
            get_claude_mcp_path(),
            json!({"fixture":true, "mcpServers":section}).to_string(),
        )
        .unwrap();
        toggle(&state, true).unwrap();
        assert_eq!(
            native(),
            json!({"fixture":true, "mcpServers":{"target":{"command":"not-executed"}}})
        );
    }
    state.db.delete_mcp_server("target").unwrap();
    let before = fs::read(get_claude_mcp_path()).unwrap();
    toggle(&state, true).unwrap();
    assert_eq!(fs::read(get_claude_mcp_path()).unwrap(), before);
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

fn fail_at(state: &AppState, point: &str) {
    let sql = match point {
        "link" => "CREATE TRIGGER fixture_reject BEFORE INSERT ON mcp_native_links BEGIN SELECT RAISE(IGNORE); END;",
        "selection" => "CREATE TRIGGER fixture_reject BEFORE UPDATE ON mcp_servers BEGIN SELECT RAISE(IGNORE); END;",
        "commit" => "PRAGMA foreign_keys=ON; CREATE UNIQUE INDEX fixture_selection ON mcp_servers(id,enabled_claude); CREATE TABLE fixture_fk(id TEXT, enabled INTEGER, FOREIGN KEY(id,enabled) REFERENCES mcp_servers(id,enabled_claude) DEFERRABLE INITIALLY DEFERRED); INSERT INTO fixture_fk VALUES('target',0);",
        _ => panic!("unknown failure"),
    };
    state.db.conn.lock().unwrap().execute_batch(sql).unwrap();
}

#[test]
fn migration_publishes_once_preserves_permissions_and_recovers_absent_destination() {
    for failure in [None, Some("link"), Some("selection"), Some("commit")] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state(temp.path(), true);
        override_dir(temp.path());
        let source = r#"{ "fixture": true, "mcpServers": {"sibling":{"server":42}} }"#;
        fs::write(get_default_claude_mcp_path(), source).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                get_default_claude_mcp_path(),
                fs::Permissions::from_mode(0o640),
            )
            .unwrap();
        }
        if let Some(failure) = failure {
            fail_at(&state, failure);
        }
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let count = calls.clone();
        let home = temp.path().to_owned();
        let result = with_exchange_hook(
            Box::new(move |resource, _| {
                if count.get() == 0 {
                    assert!(
                        !resource.path().exists(),
                        "migration must not pre-publish the source"
                    );
                }
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
                count.set(count.get() + 1);
                Ok(())
            }),
            || toggle(&state, true),
        );
        assert_eq!(result.is_err(), failure.is_some());
        assert_eq!(calls.get(), if failure.is_some() { 2 } else { 1 });
        assert_eq!(
            fs::read_to_string(get_default_claude_mcp_path()).unwrap(),
            source
        );
        assert_eq!(
            state.db.get_all_mcp_servers().unwrap()["target"]
                .apps
                .claude,
            failure.is_none()
        );
        assert_eq!(
            state.config.read().unwrap().mcp.servers.as_ref().unwrap()["target"]
                .apps
                .claude,
            failure.is_none()
        );
        if failure.is_some() {
            assert!(!get_claude_mcp_path().exists());
            assert!(cc_switch_store::read_mcp_native_link(
                &state.db.conn.lock().unwrap(),
                "target",
                "claude"
            )
            .unwrap()
            .is_none());
        } else {
            assert_eq!(native()["fixture"], true);
            assert_eq!(native()["mcpServers"]["sibling"]["server"], 42);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(get_claude_mcp_path())
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o640
                );
            }
        }
        let _released =
            SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(temp.path())).unwrap();
    }
}

#[test]
fn migration_rejects_invalid_seed_and_does_not_replace_existing_destination() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(temp.path(), true);
    override_dir(temp.path());
    fs::write(get_default_claude_mcp_path(), "{invalid").unwrap();
    assert!(toggle(&state, true).is_err());
    assert!(!get_claude_mcp_path().exists());
    fs::write(get_claude_mcp_path(), r#"{"fixture":"destination"}"#).unwrap();
    toggle(&state, true).unwrap();
    assert_eq!(native()["fixture"], "destination");
    assert_eq!(
        fs::read_to_string(get_default_claude_mcp_path()).unwrap(),
        "{invalid"
    );
}

#[test]
fn invalid_or_wrong_app_snapshot_is_not_discarded() {
    for snapshot in [
        "{}".to_owned(),
        serde_json::to_string(
            &McpConfigTarget::Gemini
                .capture_native_entry(r#"{"command":"not-executed"}"#)
                .unwrap(),
        )
        .unwrap(),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state(temp.path(), true);
        let mut conn = state.db.conn.lock().unwrap();
        let mut transaction = cc_switch_store::McpTransactionGuard::begin(&mut conn).unwrap();
        transaction
            .upsert_native_link("target", "claude", Some(&snapshot))
            .unwrap();
        transaction.commit().unwrap();
        drop(conn);
        assert!(toggle(&state, true).is_err());
        assert!(!get_claude_mcp_path().exists());
        assert!(
            !state.db.get_all_mcp_servers().unwrap()["target"]
                .apps
                .claude
        );
        assert_eq!(
            cc_switch_store::read_mcp_native_link(
                &state.db.conn.lock().unwrap(),
                "target",
                "claude"
            )
            .unwrap()
            .unwrap()
            .native_snapshot
            .as_deref(),
            Some(snapshot.as_str())
        );
    }
}

#[test]
fn publication_conflicts_and_uncertain_failures_preserve_or_restore_the_observed_file() {
    for change in ["before", "uncertain", "recovery"] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state(temp.path(), true);
        let original = "{ \"fixture\": true }\n";
        fs::write(get_claude_mcp_path(), original).unwrap();
        if change == "recovery" {
            fail_at(&state, "commit");
        }
        let mut calls = 0;
        let result = with_exchange_hook(
            Box::new(move |resource, replacement| {
                calls += 1;
                if (change == "before" && calls == 1) || (change == "recovery" && calls == 2) {
                    fs::write(resource.path(), r#"{"external":true}"#).unwrap();
                }
                if change == "uncertain" && calls == 1 {
                    resource.write(replacement.unwrap())?;
                    return Err(AppError::io(
                        resource.path(),
                        std::io::Error::other("fixture uncertain write"),
                    ));
                }
                Ok(())
            }),
            || toggle(&state, true),
        );
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(get_claude_mcp_path()).unwrap(),
            if change == "uncertain" {
                original
            } else {
                r#"{"external":true}"#
            }
        );
        assert!(
            !state.db.get_all_mcp_servers().unwrap()["target"]
                .apps
                .claude
        );
    }
}

#[cfg(unix)]
#[test]
fn commit_failure_restores_regular_and_dangling_leaf_links_without_writing_the_referent() {
    use std::os::unix::fs::symlink;
    for dangling in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state(temp.path(), true);
        override_dir(temp.path());
        fs::write(get_default_claude_mcp_path(), r#"{"fixture":"seed"}"#).unwrap();
        let path = get_claude_mcp_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let referent = path.parent().unwrap().join("referent");
        if !dangling {
            fs::write(&referent, r#"{"fixture":"referent"}"#).unwrap();
        }
        symlink("referent", &path).unwrap();
        fail_at(&state, "commit");
        assert!(toggle(&state, true).is_err());
        assert_eq!(fs::read_link(&path).unwrap(), Path::new("referent"));
        assert_eq!(
            fs::read_to_string(&referent).ok().as_deref(),
            (!dangling).then_some(r#"{"fixture":"referent"}"#)
        );
    }
}

#[cfg(windows)]
#[test]
fn readonly_migration_source_fails_without_creating_the_destination() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(temp.path(), true);
    override_dir(temp.path());
    let source = get_default_claude_mcp_path();
    fs::write(&source, "{}").unwrap();
    let permissions = fs::metadata(&source).unwrap().permissions();
    let mut readonly = permissions.clone();
    readonly.set_readonly(true);
    fs::set_permissions(&source, readonly).unwrap();
    let result = toggle(&state, true);
    fs::set_permissions(&source, permissions).unwrap();
    assert!(result.is_err());
    assert!(!get_claude_mcp_path().exists());
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn real_lite_writer_is_excluded_through_publication_and_commit_recovery() {
    use crate::services::mcp::consumer_tests::LitePeer;
    for failure in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state(temp.path(), true);
        fs::write(get_claude_mcp_path(), "{}").unwrap();
        if failure {
            fail_at(&state, "commit");
        }
        let home = temp.path().to_owned();
        let result = with_exchange_hook(
            Box::new(move |_, _| {
                LitePeer::run(
                    &home,
                    "consumer_coordination::native_switch_in_cli_fixture",
                    "probe_locked",
                );
                Ok(())
            }),
            || toggle(&state, true),
        );
        assert_eq!(result.is_err(), failure);
        LitePeer::run(
            temp.path(),
            "consumer_coordination::native_switch_in_cli_fixture",
            "probe_released",
        );
    }
}
