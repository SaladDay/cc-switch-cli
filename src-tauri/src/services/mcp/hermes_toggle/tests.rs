use std::{fs, path::PathBuf, sync::Arc};

use cc_switch_core::fs::{
    shared_live_config_lock_path, SharedLiveConfigLock, SharedLiveConfigLockError,
};
use serde_json::json;

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
        fs::create_dir_all(get_hermes_config_path().parent().unwrap()).unwrap();
    }
    if let Some(contents) = contents {
        fs::write(get_hermes_config_path(), contents).unwrap();
    }
    let db = Arc::new(Database::init().unwrap());
    db.save_mcp_server(&server("target")).unwrap();
    db.save_mcp_server(&server("peer")).unwrap();
    AppState::new(db)
}

fn toggle(state: &AppState, enabled: bool) -> Result<(), AppError> {
    McpService::toggle_app(state, "target", AppType::Hermes, enabled)
}

fn native() -> Yaml {
    serde_yaml::from_slice(&fs::read(get_hermes_config_path()).unwrap()).unwrap()
}

fn backups() -> Vec<PathBuf> {
    fs::read_dir(crate::config::get_app_config_dir().join("backups/hermes"))
        .into_iter()
        .flatten()
        .map(|entry| entry.unwrap().path())
        .collect()
}

fn unchanged_selection(state: &AppState) {
    assert!(
        !state.db.get_all_mcp_servers().unwrap()["target"]
            .apps
            .hermes
    );
    assert!(
        !state.config.read().unwrap().mcp.servers.as_ref().unwrap()["target"]
            .apps
            .hermes
    );
    assert!(cc_switch_store::read_mcp_native_link(
        &state.db.conn.lock().unwrap(),
        "target",
        "hermes"
    )
    .unwrap()
    .is_none());
}

#[test]
fn fresh_target_and_peer_survive_without_saving_other_catalogs() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(true, Some("fixture: keep\n"));
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
    assert!(target.apps.hermes && target.apps.codex);
    assert_eq!(target.name, "fresh");
    assert_eq!(
        hermes_config::yaml_to_json(&native()["mcp_servers"]["target"]).unwrap(),
        json!({"command":"fresh-not-executed", "enabled":true})
    );
    assert_eq!(native()["fixture"].as_str(), Some("keep"));
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
fn activation_preserves_private_fields_and_yaml_only_siblings() {
    for spec in [
        json!({"command":"not-executed", "args":["a",7], "env":{"N":7}, "cwd":"ignored", "enabled":false}),
        json!({"type":"http", "url":"https://fixture.invalid/mcp", "headers":{"N":7}}),
        json!({"type":"sse", "url":null, "headers":"ignored"}),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let prefix = "# outside\nfixture: .nan\n";
        let suffix = "memory: {keep: true} # outside tail\n";
        let original = format!("{prefix}mcp_servers:\n  target:\n    command: old\n    enabled: false\n    timeout: 30\n    connect_timeout: 10\n    tools: {{include: [read]}}\n    sampling: {{enabled: true}}\n    roots: [keep]\n    auth: oauth\n    unknown: dropped-like-before\n  first: 42\n  second: !Future {{1: value}}\n  last: null\n{suffix}");
        let state = state(true, Some(&original));
        let before = native();
        let mut row = server("target");
        row.server = spec;
        state.db.save_mcp_server(&row).unwrap();
        for enabled in [true, true, false, false] {
            toggle(&state, enabled).unwrap();
            let actual = native();
            assert_eq!(actual["mcp_servers"].get("target").is_some(), enabled);
            if enabled {
                let target = &actual["mcp_servers"]["target"];
                assert_eq!(target["enabled"].as_bool(), Some(true));
                for field in [
                    "timeout",
                    "connect_timeout",
                    "tools",
                    "sampling",
                    "roots",
                    "auth",
                ] {
                    assert_eq!(target[field], before["mcp_servers"]["target"][field]);
                }
                assert!(target.get("unknown").is_none() && target.get("cwd").is_none());
                let mut expected = convert_to_hermes_mcp_spec(&row.server).unwrap();
                for field in [
                    "timeout",
                    "connect_timeout",
                    "tools",
                    "sampling",
                    "roots",
                    "auth",
                ] {
                    expected[field] =
                        hermes_config::yaml_to_json(&before["mcp_servers"]["target"][field])
                            .unwrap();
                }
                assert_eq!(hermes_config::yaml_to_json(target).unwrap(), expected);
            }
            for sibling in ["first", "second", "last"] {
                assert_eq!(
                    actual["mcp_servers"][sibling],
                    before["mcp_servers"][sibling]
                );
            }
            assert_eq!(
                actual["mcp_servers"]
                    .as_mapping()
                    .unwrap()
                    .keys()
                    .filter_map(Yaml::as_str)
                    .filter(|key| *key != "target")
                    .collect::<Vec<_>>(),
                ["first", "second", "last"]
            );
            let text = fs::read_to_string(get_hermes_config_path()).unwrap();
            assert!(text.starts_with(prefix) && text.ends_with(suffix));
        }
    }
}

#[test]
fn missing_empty_and_malformed_collection_policies_remain_compatible() {
    for source in [
        None,
        Some(""),
        Some("# only a comment\n"),
        Some("!Config\n"),
        Some("!Config\nfixture: keep\n"),
        Some("mcp_servers: 42\nfixture: keep\n"),
    ] {
        for enabled in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let _env = TestEnvGuard::isolated(temp.path());
            let state = state(true, source);
            toggle(&state, enabled).unwrap();
            assert_eq!(native()["mcp_servers"].get("target").is_some(), enabled);
            assert!(native()["mcp_servers"].is_mapping());
            assert_eq!(
                backups().len(),
                usize::from(source.is_some_and(|source| !source.is_empty()))
            );
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(false, None);
    for enabled in [true, false] {
        toggle(&state, enabled).unwrap();
        assert_eq!(
            state.db.get_all_mcp_servers().unwrap()["target"]
                .apps
                .hermes,
            enabled
        );
        assert!(!get_hermes_config_path().parent().unwrap().exists());
        assert!(backups().is_empty());
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
    assert!(!get_hermes_config_path().parent().unwrap().exists());
}

#[test]
fn invalid_input_or_unsafe_section_replacement_never_publishes_or_backs_up() {
    for source in [
        "bad: [",
        "42\n",
        "null\n",
        "'mcp_servers': {}\n",
        "{mcp_servers: {}}\n",
        "---\n...\n",
        "mcp_servers:\n  target: &target {command: old}\nfixture: *target\n",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state(true, Some(source));
        for enabled in [true, false] {
            assert!(toggle(&state, enabled).is_err(), "{source}: {enabled}");
            unchanged_selection(&state);
            assert_eq!(
                fs::read_to_string(get_hermes_config_path()).unwrap(),
                source
            );
            assert!(backups().is_empty());
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(true, Some("fixture: keep\n"));
    let mut row = server("target");
    row.server = json!({"type":"invalid"});
    state.db.save_mcp_server(&row).unwrap();
    assert!(toggle(&state, true).is_err());
    unchanged_selection(&state);
    assert!(backups().is_empty());
    state.db.save_mcp_server(&server("target")).unwrap();
    fs::write(get_hermes_config_path(), [0xff]).unwrap();
    for enabled in [true, false] {
        assert!(matches!(toggle(&state, enabled), Err(AppError::Io { .. })));
        unchanged_selection(&state);
        assert_eq!(fs::read(get_hermes_config_path()).unwrap(), [0xff]);
    }
}

#[test]
fn backups_keep_exact_preimages_and_noop_does_not_rewrite_or_back_up() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let original = "# keep\nfixture: value\n";
    let state = state(true, Some(original));
    toggle(&state, true).unwrap();
    assert_eq!(backups().len(), 1);
    assert_eq!(fs::read_to_string(&backups()[0]).unwrap(), original);
    // The existing merge policy moves private fields ahead of connection
    // fields on the first merge into a newly created entry.
    toggle(&state, true).unwrap();
    let count = backups().len();
    with_exchange_hook(
        Box::new(|_, _| panic!("unchanged YAML must not be rewritten")),
        || toggle(&state, true),
    )
    .unwrap();
    assert_eq!(backups().len(), count);
    let mut settings = crate::settings::get_settings();
    settings.backup_retain_count = Some(1);
    crate::settings::update_settings(settings).unwrap();
    let before = fs::read(get_hermes_config_path()).unwrap();
    toggle(&state, false).unwrap();
    assert_eq!(backups().len(), 1);
    assert_eq!(fs::read(&backups()[0]).unwrap(), before);
}

fn install_failure(state: &AppState, failure: &str) {
    state.db.conn.lock().unwrap().execute_batch(match failure {
        "link" => "CREATE TRIGGER fixture_reject BEFORE INSERT ON mcp_native_links BEGIN SELECT RAISE(IGNORE); END;",
        "selection" => "CREATE TRIGGER fixture_reject BEFORE UPDATE ON mcp_servers BEGIN SELECT RAISE(IGNORE); END;",
        "verification" => "CREATE TRIGGER fixture_reject AFTER UPDATE ON mcp_servers WHEN NEW.id='target' BEGIN UPDATE mcp_servers SET name='changed' WHERE id='peer'; END;",
        "commit" => "PRAGMA foreign_keys=ON; CREATE UNIQUE INDEX fixture_selection ON mcp_servers(id,enabled_hermes); CREATE TABLE fixture_fk(id TEXT, enabled INTEGER, FOREIGN KEY(id,enabled) REFERENCES mcp_servers(id,enabled_hermes) DEFERRABLE INITIALLY DEFERRED); INSERT INTO fixture_fk VALUES('target',0);",
        _ => panic!("unexpected failure"),
    }).unwrap();
}

#[test]
fn database_failures_recover_missing_and_large_files_under_retained_guards() {
    let large = format!(
        "# {}\nfixture: keep\n",
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
            assert_eq!(count.get(), 2);
            assert_eq!(
                fs::read(get_hermes_config_path()).ok().as_deref(),
                original.map(str::as_bytes)
            );
            unchanged_selection(&state);
            assert_eq!(
                cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap(),
                rows
            );
            assert_eq!(backups().len(), usize::from(original.is_some()));
            if let Some(original) = original {
                assert_eq!(fs::read_to_string(&backups()[0]).unwrap(), original);
            }
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
fn backup_failure_and_unsupported_snapshot_leave_native_and_selection_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let original = "fixture: keep\n";
    let state = state(true, Some(original));
    let lock =
        SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(temp.path())).unwrap();
    assert!(matches!(toggle(&state, true), Err(AppError::Conflict(_))));
    unchanged_selection(&state);
    assert!(backups().is_empty());
    drop(lock);
    let backup_root = crate::config::get_app_config_dir().join("backups");
    fs::create_dir_all(&backup_root).unwrap();
    fs::write(backup_root.join("hermes"), "not a directory").unwrap();
    assert!(toggle(&state, true).is_err());
    unchanged_selection(&state);
    assert_eq!(
        fs::read_to_string(get_hermes_config_path()).unwrap(),
        original
    );
    let snapshot = serde_json::to_string(
        &McpConfigTarget::Gemini
            .capture_native_entry(r#"{"future":true}"#)
            .unwrap(),
    )
    .unwrap();
    let mut conn = state.db.conn.lock().unwrap();
    let mut transaction = cc_switch_store::McpTransactionGuard::begin(&mut conn).unwrap();
    transaction
        .upsert_native_link("target", "hermes", Some(&snapshot))
        .unwrap();
    transaction.commit().unwrap();
    let before = cc_switch_store::read_mcp_native_link(&conn, "target", "hermes").unwrap();
    drop(conn);
    for enabled in [true, false] {
        assert!(matches!(
            toggle(&state, enabled),
            Err(AppError::Database(_))
        ));
        assert_eq!(
            fs::read_to_string(get_hermes_config_path()).unwrap(),
            original
        );
        assert!(
            !state.db.get_all_mcp_servers().unwrap()["target"]
                .apps
                .hermes
        );
        assert!(
            !state.config.read().unwrap().mcp.servers.as_ref().unwrap()["target"]
                .apps
                .hermes
        );
        assert_eq!(
            cc_switch_store::read_mcp_native_link(
                &state.db.conn.lock().unwrap(),
                "target",
                "hermes"
            )
            .unwrap(),
            before
        );
    }
}

#[test]
fn uncertain_native_publication_and_external_changes_preserve_recovery_boundaries() {
    for phase in ["before", "after", "conflict", "recovery"] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let original = "fixture: keep\n";
        let state = state(true, Some(original));
        if phase == "recovery" {
            install_failure(&state, "commit");
        }
        let mut calls = 0;
        let result = with_exchange_hook(
            Box::new(move |resource, replacement| {
                calls += 1;
                if calls == 1 && matches!(phase, "before" | "after") {
                    if phase == "after" {
                        resource.write(replacement.unwrap())?;
                    }
                    return Err(AppError::io(
                        resource.path(),
                        std::io::Error::other("fixture failure"),
                    ));
                }
                if (calls == 1 && phase == "conflict") || (calls == 2 && phase == "recovery") {
                    fs::write(resource.path(), "external: keep this longer content\n").unwrap();
                }
                Ok(())
            }),
            || toggle(&state, true),
        );
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(get_hermes_config_path()).unwrap(),
            if matches!(phase, "before" | "after") {
                original
            } else {
                "external: keep this longer content\n"
            }
        );
        unchanged_selection(&state);
        assert_eq!(backups().len(), 1);
        assert_eq!(fs::read_to_string(&backups()[0]).unwrap(), original);
    }
}

#[test]
fn long_ids_and_failed_disable_preserve_exact_native_state() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(true, None);
    let id = "legacy".repeat(30);
    state.db.save_mcp_server(&server(&id)).unwrap();
    for enabled in [true, false] {
        McpService::toggle_app(&state, &id, AppType::Hermes, enabled).unwrap();
        assert_eq!(native()["mcp_servers"].get(&id).is_some(), enabled);
    }
    toggle(&state, true).unwrap();
    let before = fs::read(get_hermes_config_path()).unwrap();
    install_failure(&state, "selection");
    assert!(toggle(&state, false).is_err());
    assert_eq!(fs::read(get_hermes_config_path()).unwrap(), before);
    assert!(
        state.db.get_all_mcp_servers().unwrap()["target"]
            .apps
            .hermes
    );
    assert!(
        state.config.read().unwrap().mcp.servers.as_ref().unwrap()["target"]
            .apps
            .hermes
    );
}

#[cfg(unix)]
#[test]
fn leaf_links_keep_referent_permissions_and_exact_backup() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let state = state(true, None);
    let source = temp.path().join("source.yaml");
    fs::write(&source, "fixture: keep\n").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
    symlink(&source, get_hermes_config_path()).unwrap();
    toggle(&state, true).unwrap();
    assert!(!get_hermes_config_path().is_symlink());
    assert_eq!(fs::read_to_string(&source).unwrap(), "fixture: keep\n");
    assert_eq!(
        fs::metadata(get_hermes_config_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
    assert_eq!(fs::read(&backups()[0]).unwrap(), fs::read(source).unwrap());
}

#[cfg(unix)]
#[test]
fn database_failures_restore_leaf_links() {
    use std::os::unix::fs::symlink;
    for failure in ["selection", "commit"] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state(true, None);
        let source = temp.path().join("source.yaml");
        fs::write(&source, "fixture: keep\n").unwrap();
        symlink(&source, get_hermes_config_path()).unwrap();
        install_failure(&state, failure);
        assert!(toggle(&state, true).is_err());
        unchanged_selection(&state);
        assert_eq!(fs::read_link(get_hermes_config_path()).unwrap(), source);
        assert_eq!(
            fs::read_to_string(get_hermes_config_path()).unwrap(),
            "fixture: keep\n"
        );
    }
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn real_lite_writer_is_excluded_through_hermes_publication_and_commit_recovery() {
    use crate::services::mcp::consumer_tests::LitePeer;
    for failed in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let state = state(true, Some("fixture: keep\n"));
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
