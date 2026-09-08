use std::{fs, sync::Arc};

use cc_switch_core::fs::{shared_live_config_lock_path, SharedLiveConfigLock};
use serde_json::json;

use super::*;
use crate::{
    database::Database,
    settings::{update_settings, AppSettings},
    test_support::TestEnvGuard,
};

#[test]
fn claude_import_preserves_legacy_override_copy_policy() {
    for case in ["copy", "existing", "invalid", "missing", "blocked"] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let custom = temp.path().join("nested/custom");
        update_settings(AppSettings {
            claude_config_dir: Some(custom.to_string_lossy().into_owned()),
            ..AppSettings::default()
        })
        .unwrap();
        let legacy = crate::config::get_default_claude_mcp_path();
        let target = crate::config::get_claude_mcp_path();
        assert!(legacy.starts_with(temp.path()) && target.starts_with(temp.path()));
        assert_eq!(target, temp.path().join("nested/custom.json"));
        let source = if case == "invalid" {
            "{invalid"
        } else {
            r#"{"mcpServers":{"legacy":{"command":"not-executed-legacy"}}}"#
        };
        if case != "missing" {
            fs::write(&legacy, source).unwrap();
        }
        let existing = r#"{"mcpServers":{"existing":{"command":"not-executed-existing"}}}"#;
        if case == "existing" {
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(&target, existing).unwrap();
        } else if case == "blocked" {
            fs::write(temp.path().join("nested"), "not a directory").unwrap();
        }
        let state = AppState::new(Arc::new(Database::init().unwrap()));
        let result = McpService::import_from_claude(&state);
        if case == "invalid" {
            assert!(matches!(result, Err(AppError::McpValidation(_))));
            assert_eq!(fs::read_to_string(&target).unwrap(), source);
            assert!(state.db.get_all_mcp_servers().unwrap().is_empty());
        } else if matches!(case, "missing" | "blocked") {
            assert_eq!(result.unwrap(), 0);
            assert!(!target.exists());
        } else {
            assert_eq!(result.unwrap(), 1);
            let (expected, id) = if case == "existing" {
                (existing, "existing")
            } else {
                (source, "legacy")
            };
            assert_eq!(fs::read_to_string(&target).unwrap(), expected);
            let rows = state.db.get_all_mcp_servers().unwrap();
            assert_eq!(rows.len(), 1);
            assert!(rows[id].apps.claude);
        }
        if case != "missing" {
            assert_eq!(fs::read_to_string(&legacy).unwrap(), source);
        }
    }
}

fn server(id: &str) -> McpServer {
    McpServer {
        id: id.into(),
        name: format!("Catalog {id}"),
        server: json!({"command":"not-executed-catalog"}),
        apps: McpApps::default(),
        description: Some("catalog metadata".into()),
        homepage: None,
        docs: None,
        tags: vec!["catalog-tag".into()],
    }
}

fn gemini_fixture() -> (AppState, std::path::PathBuf, String) {
    let path = crate::gemini_config::get_gemini_settings_path();
    let native = json!({"fixture":"keep","mcpServers":{
        "existing":{"command":"not-executed-native-existing"},
        "new":{"command":"not-executed-native-new"},
        "invalid":{"command":42}
    }})
    .to_string();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &native).unwrap();
    let db = Arc::new(Database::init().unwrap());
    db.save_mcp_server(&server("existing")).unwrap();
    db.save_mcp_server(&server("peer")).unwrap();
    db.conn.lock().unwrap().execute_batch(
        "INSERT INTO mcp_native_links(server_id,app_id,native_snapshot) VALUES('existing','codex',NULL);"
    ).unwrap();
    (AppState::new(db), path, native)
}

#[test]
fn import_uses_fresh_catalog_and_preserves_other_fields_on_repeated_calls() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let (state, path, native) = gemini_fixture();
    let mut fresh = server("existing");
    fresh.name = "Newer peer metadata".into();
    fresh.apps.gemini = true;
    fresh.apps.codex = true;
    state.db.save_mcp_server(&fresh).unwrap();
    state
        .db
        .conn
        .lock()
        .unwrap()
        .execute_batch(
            "ALTER TABLE mcp_servers ADD COLUMN host_note TEXT DEFAULT 'default';
         UPDATE mcp_servers SET host_note='host-owned', enabled_grokbuild=1 WHERE id='existing';
         INSERT INTO settings(key,value) VALUES('import-peer-fixture','keep');",
        )
        .unwrap();
    state.config.write().unwrap().claude_common_config_snippet = Some("cache-only".into());
    let before = cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap();
    // The latest existing row is already enabled; only the new row counts.
    assert_eq!(McpService::import_from_gemini(&state).unwrap(), 1);
    assert_eq!(McpService::import_from_gemini(&state).unwrap(), 0);
    assert_eq!(fs::read_to_string(path).unwrap(), native);
    let conn = state.db.conn.lock().unwrap();
    for row in before {
        assert_eq!(
            cc_switch_store::read_mcp_server_row(&conn, &row.id).unwrap(),
            Some(row)
        );
    }
    assert!(cc_switch_store::read_mcp_server_row(&conn, "invalid")
        .unwrap()
        .is_none());
    assert!(
        cc_switch_store::read_mcp_native_link(&conn, "existing", "codex")
            .unwrap()
            .is_some()
    );
    assert_eq!(
        conn.query_row(
            "SELECT value FROM settings WHERE key='import-peer-fixture'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "keep"
    );
    drop(conn);
    let cache = state.config.read().unwrap();
    assert_eq!(
        cache.claude_common_config_snippet.as_deref(),
        Some("cache-only")
    );
    assert_eq!(
        serde_json::to_value(&cache.mcp.servers.as_ref().unwrap()["existing"]).unwrap(),
        serde_json::to_value(fresh).unwrap()
    );
}

#[test]
fn failed_import_keeps_catalog_cache_and_native_bytes_and_releases_locks() {
    for failure in ["parse", "insert", "selection", "drift", "commit", "lock"] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let (state, path, native) = gemini_fixture();
        let conn = state.db.conn.lock().unwrap();
        match failure {
            "insert" => conn.execute_batch("CREATE TRIGGER fixture_reject BEFORE INSERT ON mcp_servers BEGIN SELECT RAISE(IGNORE); END;").unwrap(),
            "selection" => conn.execute_batch("CREATE TRIGGER fixture_reject BEFORE UPDATE ON mcp_servers BEGIN SELECT RAISE(IGNORE); END;").unwrap(),
            "drift" => conn.execute_batch("CREATE TRIGGER fixture_reject AFTER UPDATE ON mcp_servers WHEN NEW.id='existing' BEGIN UPDATE mcp_servers SET name='drift' WHERE id='peer'; END;").unwrap(),
            "commit" => conn.execute_batch("PRAGMA foreign_keys=ON;
                CREATE UNIQUE INDEX fixture_selection ON mcp_servers(id,enabled_gemini);
                CREATE TABLE fixture_fk(id TEXT, enabled INTEGER, FOREIGN KEY(id,enabled) REFERENCES mcp_servers(id,enabled_gemini) DEFERRABLE INITIALLY DEFERRED);
                INSERT INTO fixture_fk VALUES('existing',0);").unwrap(),
            _ => {}
        }
        let rows = cc_switch_store::read_mcp_server_rows(&conn).unwrap();
        let link = cc_switch_store::read_mcp_native_link(&conn, "existing", "codex").unwrap();
        drop(conn);
        if failure == "parse" {
            fs::write(&path, "{invalid").unwrap();
        }
        let before_native = fs::read(&path).unwrap();
        let cache = serde_json::to_value(&*state.config.read().unwrap()).unwrap();
        let lock_path = shared_live_config_lock_path(temp.path());
        let held =
            (failure == "lock").then(|| SharedLiveConfigLock::try_acquire(&lock_path).unwrap());
        assert!(McpService::import_from_gemini(&state).is_err(), "{failure}");
        assert_eq!(
            serde_json::to_value(&*state.config.read().unwrap()).unwrap(),
            cache,
            "{failure}"
        );
        assert_eq!(fs::read(&path).unwrap(), before_native, "{failure}");
        let conn = state.db.conn.lock().unwrap();
        assert_eq!(
            cc_switch_store::read_mcp_server_rows(&conn).unwrap(),
            rows,
            "{failure}"
        );
        assert_eq!(
            cc_switch_store::read_mcp_native_link(&conn, "existing", "codex").unwrap(),
            link,
            "{failure}"
        );
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS fixture_reject; DROP TABLE IF EXISTS fixture_fk;",
        )
        .unwrap();
        drop(conn);
        drop(held);
        drop(SharedLiveConfigLock::try_acquire(&lock_path).unwrap());
        fs::write(&path, native).unwrap();
        assert_eq!(
            McpService::import_from_gemini(&state).unwrap(),
            2,
            "retry {failure}"
        );
    }
}

#[test]
fn import_all_retains_earlier_app_commit_and_stops_at_first_parse_error() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let (state, gemini, native) = gemini_fixture();
    fs::write(
        crate::config::get_claude_mcp_path(),
        r#"{"mcpServers":{"first":{"command":"not-executed-first"}}}"#,
    )
    .unwrap();
    let codex = crate::codex_config::get_codex_config_path();
    fs::create_dir_all(codex.parent().unwrap()).unwrap();
    fs::write(codex, "[invalid").unwrap();
    assert!(McpService::import_from_supported_apps(&state).is_err());
    let rows = state.db.get_all_mcp_servers().unwrap();
    assert!(rows["first"].apps.claude);
    assert!(!rows.contains_key("new"));
    assert!(!rows["existing"].apps.gemini);
    assert!(state
        .config
        .read()
        .unwrap()
        .mcp
        .servers
        .as_ref()
        .unwrap()
        .contains_key("first"));
    assert_eq!(fs::read_to_string(gemini).unwrap(), native);
}
