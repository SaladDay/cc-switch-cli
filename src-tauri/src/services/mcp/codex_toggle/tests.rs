use std::{fs, sync::Arc};

use cc_switch_core::fs::{
    shared_live_config_lock_path, SharedLiveConfigLock, SharedLiveConfigLockError,
};
use serde_json::json;

use super::*;
use crate::services::mcp::native_file::with_exchange_hook as with_hook;
use crate::{
    app_config::{AppType, McpApps, McpServer},
    database::Database,
    services::McpService,
    store::AppState,
    test_support::TestEnvGuard,
};

fn server(id: &str) -> McpServer {
    McpServer {
        id: id.into(),
        name: id.into(),
        server: json!({"command":"not-executed", "enabled":false, "x_fixture":"keep"}),
        apps: McpApps::default(),
        description: None,
        homepage: None,
        docs: None,
        tags: vec![],
    }
}

fn state(initialized: bool, contents: Option<&str>) -> AppState {
    if initialized {
        fs::create_dir_all(get_codex_config_path().parent().unwrap()).unwrap();
    }
    if let Some(contents) = contents {
        fs::write(get_codex_config_path(), contents).unwrap();
    }
    let db = Arc::new(Database::init().unwrap());
    db.save_mcp_server(&server("target")).unwrap();
    db.save_mcp_server(&server("peer")).unwrap();
    AppState::new(db)
}

fn native() -> toml::Value {
    toml::from_str(&fs::read_to_string(get_codex_config_path()).unwrap()).unwrap()
}

fn toggle(state: &AppState, enabled: bool) -> Result<(), AppError> {
    McpService::toggle_app(state, "target", AppType::Codex, enabled)
}

fn unchanged_selection(state: &AppState) {
    assert!(!state.db.get_all_mcp_servers().unwrap()["target"].apps.codex);
    assert!(
        !state.config.read().unwrap().mcp.servers.as_ref().unwrap()["target"]
            .apps
            .codex
    );
    assert!(cc_switch_store::read_mcp_native_link(
        &state.db.conn.lock().unwrap(),
        "target",
        "codex"
    )
    .unwrap()
    .is_none());
}

#[test]
fn fresh_target_and_new_peer_survive_without_saving_other_catalogs() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(true, Some("model='keep'\n"));
    let mut fresh = server("target");
    fresh.name = "fresh".into();
    fresh.apps.gemini = true;
    fresh.server["command"] = json!("fresh-not-executed");
    state.db.save_mcp_server(&fresh).unwrap();
    state.db.save_mcp_server(&server("later-peer")).unwrap();
    state.db.conn.lock().unwrap().execute_batch("ALTER TABLE mcp_servers ADD COLUMN host_note TEXT DEFAULT 'keep'; INSERT INTO settings(key,value) VALUES('peer-fixture','keep');").unwrap();
    let peer =
        cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "later-peer").unwrap();
    assert!(!state
        .config
        .read()
        .unwrap()
        .mcp
        .servers
        .as_ref()
        .unwrap()
        .contains_key("later-peer"));
    toggle(&state, true).unwrap();
    let rows = state.db.get_all_mcp_servers().unwrap();
    assert!(rows["target"].apps.codex && rows["target"].apps.gemini);
    assert_eq!(rows["target"].name, "fresh");
    assert_eq!(
        native()["mcp_servers"]["target"]["enabled"].as_bool(),
        Some(true)
    );
    assert_eq!(
        native()["mcp_servers"]["target"]["command"].as_str(),
        Some("fresh-not-executed")
    );
    assert_eq!(
        native()["mcp_servers"]["target"]["x_fixture"].as_str(),
        Some("keep")
    );
    assert_eq!(native()["model"].as_str(), Some("keep"));
    assert_eq!(
        state.config.read().unwrap().mcp.servers.as_ref().unwrap()["target"].name,
        "fresh"
    );
    let conn = state.db.conn.lock().unwrap();
    assert_eq!(
        cc_switch_store::read_mcp_server_row(&conn, "later-peer").unwrap(),
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
fn single_toggle_retains_official_and_legacy_siblings_and_native_auth() {
    for source in [
        "# root comment\nmodel='keep'\n[mcp_servers.target]\ncommand='old'\nenabled=false\n[mcp_servers.opaque]\nfuture = 42 # sibling comment\n[mcp.servers.target]\ncommand='old-legacy'\n[mcp.servers.legacy]\nfuture='keep'\n",
        "model='keep'\nmcp_servers = { target = { command='old', enabled=false }, opaque = 42 }\nmcp = { servers = { target = { command='old' }, legacy = { future='keep' } } }\n",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state(true, Some(source));
        let auth = get_codex_config_path().with_file_name("auth.json");
        fs::write(&auth, "not even JSON; must not read or replace").unwrap();
        let before = native();
        for enabled in [true, true, false, false, true] {
            toggle(&state, enabled).unwrap();
            let actual = native();
            assert_eq!(actual["mcp_servers"].get("target").is_some(), enabled);
            if enabled { assert_eq!(actual["mcp_servers"]["target"]["enabled"].as_bool(), Some(true)); }
            assert!(actual["mcp"]["servers"].get("target").is_none());
            assert_eq!(actual["mcp_servers"]["opaque"], before["mcp_servers"]["opaque"]);
            assert_eq!(actual["mcp"]["servers"]["legacy"], before["mcp"]["servers"]["legacy"]);
            assert_eq!(actual["model"], before["model"]);
            assert_eq!(fs::read_to_string(&auth).unwrap(), "not even JSON; must not read or replace");
            if source.starts_with('#') {
                let text = fs::read_to_string(get_codex_config_path()).unwrap();
                assert!(text.contains("# root comment") && text.contains("future = 42 # sibling comment"));
            }
        }
    }
}

#[test]
fn missing_rows_and_uninitialized_apps_do_not_create_native_files() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(false, None);
    for enabled in [true, false] {
        toggle(&state, enabled).unwrap();
        assert_eq!(
            state.db.get_all_mcp_servers().unwrap()["target"].apps.codex,
            enabled
        );
        assert!(!get_codex_config_path().parent().unwrap().exists());
    }
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
    assert!(!get_codex_config_path().parent().unwrap().exists());
}

#[test]
fn malformed_native_input_retains_enable_error_and_tolerant_disable_policy() {
    for source in [b"broken = [".as_slice(), &[0xff, 0xfe]] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state(true, None);
        fs::write(get_codex_config_path(), source).unwrap();
        assert!(toggle(&state, true).is_err());
        unchanged_selection(&state);
        assert_eq!(fs::read(get_codex_config_path()).unwrap(), source);
        assert_eq!(
            toggle(&state, false).is_ok(),
            std::str::from_utf8(source).is_ok()
        );
        assert_eq!(fs::read(get_codex_config_path()).unwrap(), source);
    }
}

#[test]
fn unsupported_link_snapshot_is_preserved_without_native_or_catalog_publication() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(true, Some("model='keep'\n"));
    let snapshot = serde_json::to_string(
        &McpConfigTarget::Gemini
            .capture_native_entry(r#"{"future":"keep"}"#)
            .unwrap(),
    )
    .unwrap();
    let mut conn = state.db.conn.lock().unwrap();
    let mut transaction = cc_switch_store::McpTransactionGuard::begin(&mut conn).unwrap();
    transaction
        .upsert_native_link("target", "codex", Some(&snapshot))
        .unwrap();
    transaction.commit().unwrap();
    let before = cc_switch_store::read_mcp_native_link(&conn, "target", "codex").unwrap();
    drop(conn);
    for enabled in [true, false] {
        assert!(toggle(&state, enabled).is_err());
        assert_eq!(
            fs::read_to_string(get_codex_config_path()).unwrap(),
            "model='keep'\n"
        );
        assert!(!state.db.get_all_mcp_servers().unwrap()["target"].apps.codex);
        assert!(
            !state.config.read().unwrap().mcp.servers.as_ref().unwrap()["target"]
                .apps
                .codex
        );
        assert_eq!(
            cc_switch_store::read_mcp_native_link(
                &state.db.conn.lock().unwrap(),
                "target",
                "codex"
            )
            .unwrap(),
            before
        );
    }
}

fn install_failure(state: &AppState, failure: &str) {
    let conn = state.db.conn.lock().unwrap();
    match failure {
        "link" => conn.execute_batch("CREATE TRIGGER fixture_reject BEFORE INSERT ON mcp_native_links BEGIN SELECT RAISE(IGNORE); END;").unwrap(),
        "selection" => conn.execute_batch("CREATE TRIGGER fixture_reject BEFORE UPDATE ON mcp_servers BEGIN SELECT RAISE(IGNORE); END;").unwrap(),
        "verification" => conn.execute_batch("CREATE TRIGGER fixture_reject AFTER UPDATE ON mcp_servers WHEN NEW.id='target' BEGIN UPDATE mcp_servers SET name='changed' WHERE id='peer'; END;").unwrap(),
        "commit" => conn.execute_batch("PRAGMA foreign_keys=ON; CREATE UNIQUE INDEX fixture_selection ON mcp_servers(id,enabled_codex); CREATE TABLE fixture_fk(id TEXT, enabled INTEGER, FOREIGN KEY(id,enabled) REFERENCES mcp_servers(id,enabled_codex) DEFERRABLE INITIALLY DEFERRED); INSERT INTO fixture_fk VALUES('target',0);").unwrap(),
        _ => panic!("unexpected failure"),
    }
}

#[test]
fn recovery_retains_guards_and_exact_bytes_for_missing_and_large_files() {
    let large = format!(
        "# {}\nmodel='keep'\n",
        "x".repeat(cc_switch_core::MAX_OPERATION_CONTENT_BYTES + 1)
    );
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
            let result = with_hook(
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
            assert_eq!(
                count.get(),
                2,
                "publication and retained recovery: {failure}"
            );
            assert_eq!(
                fs::read(get_codex_config_path()).ok().as_deref(),
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
fn uncertain_native_failure_recovers_before_and_after_publication() {
    for original in [None, Some("model='keep'\n")] {
        for published in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let _env = TestEnvGuard::isolated(temp.path());
            let state = state(true, original);
            let mut calls = 0;
            let result = with_hook(
                Box::new(move |resource, replacement| {
                    calls += 1;
                    if calls == 1 {
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
                || toggle(&state, true),
            );
            assert!(result.is_err());
            assert_eq!(
                fs::read(get_codex_config_path()).ok().as_deref(),
                original.map(str::as_bytes)
            );
            unchanged_selection(&state);
        }
    }
}

#[test]
fn conflicts_preserve_external_changes_during_publication_and_recovery() {
    for at in [1, 2] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state(true, Some("model='keep'\n"));
        if at == 2 {
            install_failure(&state, "commit");
        }
        let mut calls = 0;
        let result = with_hook(
            Box::new(move |resource, _| {
                calls += 1;
                if calls == at {
                    fs::write(resource.path(), "external='keep this longer content'\n").unwrap();
                }
                Ok(())
            }),
            || toggle(&state, true),
        );
        assert!(result.is_err());
        assert_eq!(
            native()["external"].as_str(),
            Some("keep this longer content")
        );
        if at == 2 {
            assert!(result.unwrap_err().to_string().contains("native recovery"));
        }
        unchanged_selection(&state);
    }
}

#[test]
fn shared_lock_contention_rejects_before_native_publication() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(true, Some("model='keep'\n"));
    let guard =
        SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(temp.path())).unwrap();
    assert!(matches!(toggle(&state, true), Err(AppError::Conflict(_))));
    unchanged_selection(&state);
    assert_eq!(
        fs::read_to_string(get_codex_config_path()).unwrap(),
        "model='keep'\n"
    );
    drop(guard);
    toggle(&state, true).unwrap();
}

#[cfg(unix)]
#[test]
fn leaf_link_replacement_preserves_referent_and_observes_external_replacements() {
    use std::os::unix::{fs::symlink, fs::PermissionsExt};
    for external in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state(true, None);
        let source = temp.path().join("source.toml");
        fs::write(&source, "model='source'\n").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
        symlink(&source, get_codex_config_path()).unwrap();
        let other = source.clone();
        let result = with_hook(
            Box::new(move |_, _| {
                if external {
                    let replacement = other.with_extension("replacement");
                    fs::write(&replacement, "external='keep'\n").unwrap();
                    fs::rename(replacement, &other).unwrap();
                }
                Ok(())
            }),
            || toggle(&state, true),
        );
        assert_eq!(result.is_err(), external);
        if external {
            unchanged_selection(&state);
            assert_eq!(fs::read_to_string(&source).unwrap(), "external='keep'\n");
        } else {
            assert_eq!(fs::read_to_string(&source).unwrap(), "model='source'\n");
            assert!(!get_codex_config_path().is_symlink());
            assert_eq!(
                fs::metadata(get_codex_config_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o640
            );
        }
    }
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn real_lite_writer_is_excluded_through_codex_publication_and_commit_recovery() {
    use crate::services::mcp::consumer_tests::LitePeer;
    for failed in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state(true, Some("model='keep'\n"));
        fs::write(temp.path().join("coordination-fixture"), "cli-lite-v1").unwrap();
        if failed {
            install_failure(&state, "commit");
        }
        let home = temp.path().to_owned();
        let count = std::rc::Rc::new(std::cell::Cell::new(0));
        let calls = count.clone();
        let result = with_hook(
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
