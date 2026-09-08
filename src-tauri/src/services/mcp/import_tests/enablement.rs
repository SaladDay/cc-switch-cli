use super::*;
use serde_json::Value;

fn source(app: &AppType, flag: Option<Value>, http: bool) -> (std::path::PathBuf, String) {
    let mut entry = match (app, http) {
        (AppType::OpenCode, true) => json!({"type":"remote","url":"https://example.invalid/mcp"}),
        (AppType::OpenCode, false) => json!({"type":"local","command":["not-executed"]}),
        (_, true) => json!({"url":"https://example.invalid/mcp"}),
        (_, false) => json!({"command":"not-executed"}),
    };
    entry["x_native"] = json!("keep");
    if let Some(flag) = flag {
        entry["enabled"] = flag;
    }
    match app {
        AppType::Codex => (
            crate::codex_config::get_codex_config_path(),
            toml::to_string(&json!({"fixture":"keep","mcp_servers":{"existing":entry}})).unwrap(),
        ),
        AppType::OpenCode => (
            crate::opencode_config::get_opencode_config_path(),
            json!({"fixture":"keep","mcp":{"existing":entry}}).to_string(),
        ),
        AppType::Hermes => (
            crate::hermes_config::get_hermes_config_path(),
            serde_yaml::to_string(&json!({"fixture":"keep","mcp_servers":{"existing":entry}}))
                .unwrap(),
        ),
        _ => panic!("unsupported fixture"),
    }
}

fn import(state: &AppState, app: &AppType) -> Result<usize, AppError> {
    match app {
        AppType::Codex => McpService::import_from_codex(state),
        AppType::OpenCode => McpService::import_from_opencode(state),
        AppType::Hermes => McpService::import_from_hermes(state),
        _ => panic!("unsupported fixture"),
    }
}

#[test]
fn imports_observe_native_state_without_replacing_catalog_connections() {
    for app in [AppType::Codex, AppType::OpenCode, AppType::Hermes] {
        for flag in [
            None,
            Some(json!(false)),
            Some(json!(true)),
            Some(json!("false")),
            Some(json!(0)),
            Some(json!([])),
            Some(json!({})),
        ] {
            for previous in [None, Some(false), Some(true)] {
                for http in [false, true] {
                    let temp = tempfile::tempdir().unwrap();
                    let _env = TestEnvGuard::isolated(temp.path());
                    let (path, native) = source(&app, flag.clone(), http);
                    assert!(path.starts_with(temp.path()));
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    fs::write(&path, &native).unwrap();
                    let state = AppState::new(Arc::new(Database::init().unwrap()));
                    let mut expected = server("existing");
                    if let Some(previous) = previous {
                        expected.apps.claude = true;
                        expected.apps.set_enabled_for(&app, previous);
                        state.db.save_mcp_server(&expected).unwrap();
                    }
                    state.db.save_mcp_server(&server("peer")).unwrap();
                    state.db.conn.lock().unwrap().execute_batch(
                        "ALTER TABLE mcp_servers ADD COLUMN host_note TEXT DEFAULT 'keep';
                         INSERT INTO mcp_native_links(server_id,app_id,native_snapshot) VALUES('peer','codex',NULL);"
                    ).unwrap();
                    let peer = cc_switch_store::read_mcp_server_row(
                        &state.db.conn.lock().unwrap(),
                        "peer",
                    )
                    .unwrap();
                    let enabled = flag != Some(json!(false));
                    let count = usize::from(previous != Some(enabled));
                    assert_eq!(
                        import(&state, &app).unwrap(),
                        count,
                        "{app:?}: {flag:?}, {previous:?}"
                    );
                    assert_eq!(import(&state, &app).unwrap(), 0);
                    assert_eq!(fs::read(&path).unwrap(), native.as_bytes());
                    let rows = state.db.get_all_mcp_servers().unwrap();
                    assert_eq!(rows["existing"].apps.is_enabled_for(&app), enabled);
                    if previous.is_some() {
                        expected.apps.set_enabled_for(&app, enabled);
                        assert_eq!(
                            serde_json::to_value(&rows["existing"]).unwrap(),
                            serde_json::to_value(expected).unwrap()
                        );
                    } else {
                        assert_eq!(
                            rows["existing"].apps.enabled_apps(),
                            if enabled { vec![app.clone()] } else { vec![] }
                        );
                    }
                    assert_eq!(
                        serde_json::to_value(
                            state.config.read().unwrap().mcp.servers.as_ref().unwrap()
                        )
                        .unwrap(),
                        serde_json::to_value(&rows).unwrap()
                    );
                    let conn = state.db.conn.lock().unwrap();
                    assert_eq!(
                        cc_switch_store::read_mcp_server_row(&conn, "peer").unwrap(),
                        peer
                    );
                    assert!(
                        cc_switch_store::read_mcp_native_link(&conn, "peer", "codex")
                            .unwrap()
                            .is_some()
                    );
                    assert_eq!(
                        conn.query_row(
                            "SELECT host_note FROM mcp_servers WHERE id='existing'",
                            [],
                            |row| row.get::<_, String>(0)
                        )
                        .unwrap(),
                        "keep"
                    );
                }
            }
        }
    }
}

#[test]
fn disabling_import_failure_preserves_database_cache_links_and_source() {
    for app in [AppType::Codex, AppType::OpenCode, AppType::Hermes] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let (path, native) = source(&app, Some(json!(false)), false);
        assert!(path.starts_with(temp.path()));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &native).unwrap();
        let db = Arc::new(Database::init().unwrap());
        let mut existing = server("existing");
        existing.apps.set_enabled_for(&app, true);
        db.save_mcp_server(&existing).unwrap();
        db.conn.lock().unwrap().execute_batch(
            "INSERT INTO mcp_native_links(server_id,app_id,native_snapshot) VALUES('existing','codex',NULL);
             CREATE TRIGGER fixture_reject BEFORE UPDATE ON mcp_servers BEGIN SELECT RAISE(IGNORE); END;"
        ).unwrap();
        let state = AppState::new(db);
        let cache = serde_json::to_value(&*state.config.read().unwrap()).unwrap();
        let rows = cc_switch_store::read_mcp_server_rows(&state.db.conn.lock().unwrap()).unwrap();
        let link = cc_switch_store::read_mcp_native_link(
            &state.db.conn.lock().unwrap(),
            "existing",
            "codex",
        )
        .unwrap();
        assert!(import(&state, &app).is_err());
        assert_eq!(fs::read(&path).unwrap(), native.as_bytes());
        assert_eq!(
            serde_json::to_value(&*state.config.read().unwrap()).unwrap(),
            cache
        );
        let conn = state.db.conn.lock().unwrap();
        assert_eq!(cc_switch_store::read_mcp_server_rows(&conn).unwrap(), rows);
        assert_eq!(
            cc_switch_store::read_mcp_native_link(&conn, "existing", "codex").unwrap(),
            link
        );
        conn.execute_batch("DROP TRIGGER fixture_reject;").unwrap();
        drop(conn);
        assert_eq!(import(&state, &app).unwrap(), 1);
        assert!(!state.db.get_all_mcp_servers().unwrap()["existing"]
            .apps
            .is_enabled_for(&app));
    }
}

#[test]
fn codex_first_valid_entry_controls_state_without_repeated_count_churn() {
    for legacy in ["enabled = false", "enabled = true", "command = 42"] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let path = crate::codex_config::get_codex_config_path();
        let fields = if legacy.starts_with("enabled") {
            format!("command = 'legacy'\n{legacy}")
        } else {
            legacy.into()
        };
        let canonical_enabled = legacy == "enabled = false";
        let native = format!("[mcp.servers.existing]\n{fields}\n[mcp_servers.existing]\ncommand = 'canonical'\nenabled = {canonical_enabled}\n");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &native).unwrap();
        let state = AppState::new(Arc::new(Database::init().unwrap()));
        assert_eq!(import(&state, &AppType::Codex).unwrap(), 1);
        assert_eq!(import(&state, &AppType::Codex).unwrap(), 0);
        let rows = state.db.get_all_mcp_servers().unwrap();
        assert_eq!(rows["existing"].apps.codex, legacy == "enabled = true");
        assert_eq!(
            rows["existing"].server["command"],
            if legacy.starts_with("enabled") {
                "legacy"
            } else {
                "canonical"
            }
        );
        assert_eq!(fs::read(path).unwrap(), native.as_bytes());
    }
}
