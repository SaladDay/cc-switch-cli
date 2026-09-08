use super::*;

fn write_fixture(app: &AppType, home: &Path, disabled: bool) -> (std::path::PathBuf, String) {
    let (path, native) = fixture(app, disabled);
    assert!(path.starts_with(home));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &native).unwrap();
    fs::write(home.join("coordination-fixture"), "cli-lite-v1").unwrap();
    (path, native)
}

fn assert_native_enabled(app: &AppType, path: &Path, original: &str) {
    let actual = document(app, &fs::read(path).unwrap());
    assert_unrelated_native(app, &actual, original);
    let entry = &actual[map_key(app)]["cli-import"];
    assert_eq!(
        entry["command"],
        document(app, original.as_bytes())[map_key(app)]["cli-import"]["command"]
    );
    let enabled = match entry.get("enabled") {
        // The command assertion above proves the entry exists. These formats
        // allow an absent flag to mean enabled.
        None => true,
        Some(value) => value == true,
    };
    assert!(
        enabled,
        "successful CLI enable must activate the native entry for {app:?}"
    );
}

fn activation(app: AppType, import: fn(&AppState) -> Result<usize, AppError>, cli_first: bool) {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let (path, native) = write_fixture(&app, temp.path(), true);
    let db = Arc::new(Database::init().unwrap());
    LitePeer::create_mcp(temp.path());
    if !cli_first {
        peer(temp.path(), &app, "import");
    }
    // Load after Lite's writes so this case tests activation independently of
    // the separate stale-catalog writer check below.
    let state = AppState::new(db);
    if cli_first {
        assert_eq!(import(&state).unwrap(), 2);
    }
    assert!(
        !state.config.read().unwrap().mcp.servers.as_ref().unwrap()["cli-import"]
            .apps
            .is_enabled_for(&app)
    );
    assert_eq!(fs::read(&path).unwrap(), native.as_bytes());
    for _ in 0..2 {
        McpService::toggle_app(&state, "cli-import", app.clone(), true).unwrap();
        assert!(state.db.get_all_mcp_servers().unwrap()["cli-import"]
            .apps
            .is_enabled_for(&app));
        assert_native_enabled(&app, &path, &native);
    }
}

fn retains_lite_peer(app: AppType, import: fn(&AppState) -> Result<usize, AppError>) {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let (path, native) = write_fixture(&app, temp.path(), false);
    let state = AppState::new(Arc::new(Database::init().unwrap()));
    assert_eq!(import(&state).unwrap(), 2);
    LitePeer::create_mcp(temp.path());
    assert!(!state
        .config
        .read()
        .unwrap()
        .mcp
        .servers
        .as_ref()
        .unwrap()
        .contains_key("lite-peer"));
    let before = cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "lite-peer")
        .unwrap()
        .expect("real Lite peer committed its row");
    McpService::toggle_app(&state, "cli-import", app.clone(), true).unwrap();
    assert_eq!(
        cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "lite-peer").unwrap(),
        Some(before),
        "CLI toggle must preserve a newer Lite record for {app:?}"
    );
    assert_native_enabled(&app, &path, &native);
}

macro_rules! acceptance_case {
    ($name:ident, $body:expr) => {
        #[test]
        #[ignore = "requires the independently built Lite library test binary"]
        fn $name() {
            $body
        }
    };
}

acceptance_case!(
    codex_after_cli_import,
    activation(AppType::Codex, McpService::import_from_codex, true)
);
acceptance_case!(
    codex_after_lite_import,
    activation(AppType::Codex, McpService::import_from_codex, false)
);
acceptance_case!(
    opencode_after_cli_import,
    activation(AppType::OpenCode, McpService::import_from_opencode, true)
);
acceptance_case!(
    opencode_after_lite_import,
    activation(AppType::OpenCode, McpService::import_from_opencode, false)
);
acceptance_case!(
    hermes_after_cli_import,
    activation(AppType::Hermes, McpService::import_from_hermes, true)
);
acceptance_case!(
    hermes_after_lite_import,
    activation(AppType::Hermes, McpService::import_from_hermes, false)
);
acceptance_case!(
    claude_keeps_lite_peer,
    retains_lite_peer(AppType::Claude, McpService::import_from_claude)
);
acceptance_case!(
    codex_keeps_lite_peer,
    retains_lite_peer(AppType::Codex, McpService::import_from_codex)
);
acceptance_case!(
    gemini_keeps_lite_peer,
    retains_lite_peer(AppType::Gemini, McpService::import_from_gemini)
);
acceptance_case!(
    opencode_keeps_lite_peer,
    retains_lite_peer(AppType::OpenCode, McpService::import_from_opencode)
);
acceptance_case!(
    hermes_keeps_lite_peer,
    retains_lite_peer(AppType::Hermes, McpService::import_from_hermes)
);
