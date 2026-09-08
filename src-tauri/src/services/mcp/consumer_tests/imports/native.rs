use super::*;

mod cli_toggle;

fn map_key(app: &AppType) -> &'static str {
    match app {
        AppType::Claude | AppType::Gemini => "mcpServers",
        AppType::OpenCode => "mcp",
        AppType::Codex | AppType::Hermes => "mcp_servers",
        _ => panic!("unsupported fixture App"),
    }
}

fn document(app: &AppType, bytes: &[u8]) -> Value {
    match app {
        AppType::Codex => serde_json::to_value(
            toml::from_str::<toml::Value>(std::str::from_utf8(bytes).unwrap()).unwrap(),
        )
        .unwrap(),
        AppType::Hermes => serde_yaml::from_slice(bytes).unwrap(),
        _ => serde_json::from_slice(bytes).unwrap(),
    }
}

fn fixture(app: &AppType, disabled: bool) -> (std::path::PathBuf, String) {
    let (path, native) = native_fixture(app, false);
    let mut root = document(app, native.as_bytes());
    let entry = root[map_key(app)]["cli-import"].as_object_mut().unwrap();
    entry.insert("x_native".into(), json!("preserve native extension"));
    if matches!(app, AppType::Codex | AppType::OpenCode | AppType::Hermes) {
        entry.insert("enabled".into(), json!(!disabled));
    } else {
        assert!(!disabled);
    }
    let mut sibling = Value::Object(entry.clone());
    sibling["x_native"] = json!("preserve native sibling");
    root[map_key(app)]
        .as_object_mut()
        .unwrap()
        .insert("native-sibling".into(), sibling);
    let text = match app {
        AppType::Codex => toml::to_string(&root).unwrap(),
        AppType::Hermes => serde_yaml::to_string(&root).unwrap(),
        _ => serde_json::to_string_pretty(&root).unwrap(),
    };
    (path, text)
}

fn assert_unrelated_native(app: &AppType, actual: &Value, native: &str) {
    let before = document(app, native.as_bytes());
    assert_eq!(actual["fixture"], before["fixture"]);
    assert_eq!(
        actual[map_key(app)]["native-sibling"],
        before[map_key(app)]["native-sibling"]
    );
}

fn peer(home: &Path, app: &AppType, action: &str) {
    LitePeer::run(
        home,
        "consumer_coordination::mcp_lifecycle::mcp_lifecycle_in_cli_fixture",
        &format!("{}:{action}", app.as_core().as_str()),
    );
}

fn lifecycle(app: AppType, import: fn(&AppState) -> Result<usize, AppError>) {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let (path, native) = fixture(&app, false);
    assert!(path.starts_with(temp.path()));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &native).unwrap();
    let state = AppState::new(Arc::new(Database::init().unwrap()));
    fs::write(temp.path().join("coordination-fixture"), "cli-lite-v1").unwrap();
    LitePeer::create_mcp(temp.path());
    let peer_row =
        cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "lite-peer")
            .unwrap()
            .expect("Lite created its peer row");
    assert_eq!(import(&state).unwrap(), 2);
    assert_eq!(fs::read(&path).unwrap(), native.as_bytes());
    peer(temp.path(), &app, "disable");
    assert!(!state.db.get_all_mcp_servers().unwrap()["cli-import"]
        .apps
        .is_enabled_for(&app));
    let disabled = document(&app, &fs::read(&path).unwrap());
    let target = &disabled[map_key(&app)]["cli-import"];
    if matches!(app, AppType::Claude | AppType::Gemini) {
        assert!(target.is_null());
    } else {
        assert_eq!(target["enabled"], false);
    }
    assert_unrelated_native(&app, &disabled, &native);
    peer(temp.path(), &app, "enable");
    assert!(state.db.get_all_mcp_servers().unwrap()["cli-import"]
        .apps
        .is_enabled_for(&app));
    let restored = document(&app, &fs::read(&path).unwrap());
    let mut expected = document(&app, native.as_bytes())[map_key(&app)]["cli-import"].clone();
    // Existing projection policies normalize known fields. Keep the comparison
    // exact for everything else, including the native-only extension.
    match app {
        AppType::Codex => {
            expected.as_object_mut().unwrap().remove("enabled");
            expected["type"] = json!("stdio");
        }
        AppType::Gemini => expected["timeout"] = json!(60_000),
        _ => {}
    }
    assert_eq!(restored[map_key(&app)]["cli-import"], expected);
    assert_unrelated_native(&app, &restored, &native);
    assert_eq!(import(&state).unwrap(), 0);
    peer(temp.path(), &app, "delete");
    assert!(!state
        .db
        .get_all_mcp_servers()
        .unwrap()
        .contains_key("cli-import"));
    let removed = document(&app, &fs::read(&path).unwrap());
    assert!(removed[map_key(&app)]["cli-import"].is_null());
    assert_unrelated_native(&app, &removed, &native);
    assert_eq!(import(&state).unwrap(), 0);
    assert!(!state
        .config
        .read()
        .unwrap()
        .mcp
        .servers
        .as_ref()
        .unwrap()
        .contains_key("cli-import"));
    assert_eq!(
        cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "lite-peer")
            .unwrap()
            .expect("the peer survives the complete lifecycle"),
        peer_row
    );
}

fn disabled_import(app: AppType, import: fn(&AppState) -> Result<usize, AppError>) {
    for lite_first in [false, true] {
        disabled_import_order(&app, import, lite_first);
    }
}

fn disabled_import_order(
    app: &AppType,
    import: fn(&AppState) -> Result<usize, AppError>,
    lite_first: bool,
) {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let (path, native) = fixture(app, true);
    assert!(path.starts_with(temp.path()));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &native).unwrap();
    let state = AppState::new(Arc::new(Database::init().unwrap()));
    fs::write(temp.path().join("coordination-fixture"), "cli-lite-v1").unwrap();
    if lite_first {
        peer(temp.path(), app, "import");
        let report: Value = serde_json::from_slice(
            &fs::read(temp.path().join("lite-mcp-import-report.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(report["newServers"], 2);
        assert_eq!(report["failedApps"], json!([]));
        assert!(!state.db.get_all_mcp_servers().unwrap()["cli-import"]
            .apps
            .is_enabled_for(app));
    }
    assert_eq!(import(&state).unwrap(), if lite_first { 0 } else { 2 });
    assert_eq!(import(&state).unwrap(), 0);
    assert!(!state.db.get_all_mcp_servers().unwrap()["cli-import"]
        .apps
        .is_enabled_for(app));
    assert_eq!(fs::read(&path).unwrap(), native.as_bytes());
    peer(temp.path(), app, "enable");
    assert!(state.db.get_all_mcp_servers().unwrap()["cli-import"]
        .apps
        .is_enabled_for(app));
    let enabled = document(app, &fs::read(&path).unwrap());
    let entry = &enabled[map_key(app)]["cli-import"];
    assert_eq!(
        entry["command"],
        document(app, native.as_bytes())[map_key(app)]["cli-import"]["command"]
    );
    assert_eq!(entry["x_native"], "preserve native extension");
    assert_unrelated_native(app, &enabled, &native);
    let active = match entry.get("enabled") {
        None => matches!(app, AppType::Codex),
        Some(value) => value == true,
    };
    assert!(
        active,
        "explicit Lite enable after CLI import must activate the native entry"
    );
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn config_path_overrides_do_not_escape_the_fixture() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let (inside, native) = fixture(&AppType::Hermes, false);
    assert!(inside.starts_with(temp.path()));
    fs::create_dir_all(inside.parent().unwrap()).unwrap();
    fs::write(&inside, &native).unwrap();
    let state = AppState::new(Arc::new(Database::init().unwrap()));
    assert_eq!(McpService::import_from_hermes(&state).unwrap(), 2);
    fs::write(temp.path().join("coordination-fixture"), "cli-lite-v1").unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("config.yaml");
    fs::write(&path, &native).unwrap();
    LitePeer::run_with_environment(
        temp.path(),
        "consumer_coordination::mcp_lifecycle::mcp_lifecycle_in_cli_fixture",
        "hermes:disable",
        &[
            ("CLAUDE_CONFIG_DIR", outside.path()),
            ("CODEX_HOME", outside.path()),
            ("HERMES_HOME", outside.path()),
            ("PI_CODING_AGENT_DIR", outside.path()),
            ("LOCALAPPDATA", outside.path()),
        ],
    );
    let disabled = document(&AppType::Hermes, &fs::read(inside).unwrap());
    assert_eq!(disabled["mcp_servers"]["cli-import"]["enabled"], false);
    assert_unrelated_native(&AppType::Hermes, &disabled, &native);
    assert!(
        !state.db.get_all_mcp_servers().unwrap()["cli-import"]
            .apps
            .hermes
    );
    assert_eq!(fs::read(path).unwrap(), native.as_bytes());
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 1);
}

macro_rules! lifecycle_case {
    ($test:ident, $app:path, $import:path) => {
        #[test]
        #[ignore = "requires the independently built Lite library test binary"]
        fn $test() {
            lifecycle($app, $import);
        }
    };
}
macro_rules! disabled_case {
    ($test:ident, $app:path, $import:path) => {
        #[test]
        #[ignore = "requires the independently built Lite library test binary"]
        fn $test() {
            disabled_import($app, $import);
        }
    };
}
lifecycle_case!(
    claude_lifecycle,
    AppType::Claude,
    McpService::import_from_claude
);
lifecycle_case!(
    codex_lifecycle,
    AppType::Codex,
    McpService::import_from_codex
);
lifecycle_case!(
    gemini_lifecycle,
    AppType::Gemini,
    McpService::import_from_gemini
);
lifecycle_case!(
    opencode_lifecycle,
    AppType::OpenCode,
    McpService::import_from_opencode
);
lifecycle_case!(
    hermes_lifecycle,
    AppType::Hermes,
    McpService::import_from_hermes
);
disabled_case!(
    codex_disabled_import,
    AppType::Codex,
    McpService::import_from_codex
);
disabled_case!(
    opencode_disabled_import,
    AppType::OpenCode,
    McpService::import_from_opencode
);
disabled_case!(
    hermes_disabled_import,
    AppType::Hermes,
    McpService::import_from_hermes
);
