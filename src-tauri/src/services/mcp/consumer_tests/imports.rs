use super::*;

mod native;
mod scale;

#[derive(Clone, Copy)]
enum ImportCase {
    New,
    Existing,
    AlreadyEnabled,
    Empty,
}

fn native_fixture(app: &AppType, empty: bool) -> (std::path::PathBuf, String) {
    let command = "not-executed-native-import";
    let entries = if empty {
        json!({})
    } else {
        json!({"cli-import":{"command":command}})
    };
    match app {
        AppType::Claude => (
            crate::config::get_claude_mcp_path(),
            json!({"fixture":"keep","mcpServers":entries}).to_string(),
        ),
        AppType::Codex => (
            crate::codex_config::get_codex_config_path(),
            if empty {
                "fixture = 'keep'\n[mcp_servers]\n".into()
            } else {
                format!("fixture = 'keep'\n[mcp_servers.cli-import]\ncommand = '{command}'\n")
            },
        ),
        AppType::Gemini => (
            crate::gemini_config::get_gemini_settings_path(),
            json!({"fixture":"keep","mcpServers":entries}).to_string(),
        ),
        AppType::OpenCode => (
            crate::opencode_config::get_opencode_config_path(),
            if empty {
                json!({"fixture":"keep","mcp":{}})
            } else {
                json!({"fixture":"keep","mcp":{"cli-import":{"type":"local","command":[command]}}})
            }
            .to_string(),
        ),
        AppType::Hermes => (
            crate::hermes_config::get_hermes_config_path(),
            serde_yaml::to_string(&json!({"fixture":"keep","mcp_servers":entries})).unwrap(),
        ),
        AppType::OpenClaw | AppType::Pi => panic!("this App has no CLI MCP importer"),
    }
}

fn assert_shared_import(
    app: AppType,
    import: fn(&AppState) -> Result<usize, AppError>,
    case: ImportCase,
) {
    let temp = tempfile::tempdir().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    let (path, native) = native_fixture(&app, matches!(case, ImportCase::Empty));
    assert!(path.starts_with(temp.path()), "fixture must stay isolated");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &native).unwrap();
    let db = Arc::new(Database::init().unwrap());
    let mut existing = McpServer {
        id: "cli-import".into(),
        name: "Existing catalog metadata".into(),
        server: json!({"command":"not-executed-catalog-import"}),
        apps: McpApps::default(),
        description: Some("Keep the existing connection even when import differs".into()),
        homepage: None,
        docs: None,
        tags: vec!["keep".into()],
    };
    let has_existing = matches!(case, ImportCase::Existing | ImportCase::AlreadyEnabled);
    if has_existing {
        let other = if app == AppType::Claude {
            AppType::Codex
        } else {
            AppType::Claude
        };
        existing.apps.set_enabled_for(&other, true);
        existing
            .apps
            .set_enabled_for(&app, matches!(case, ImportCase::AlreadyEnabled));
        db.save_mcp_server(&existing).unwrap();
    }
    db.conn
        .lock()
        .unwrap()
        .execute_batch("ALTER TABLE mcp_servers ADD COLUMN host_note TEXT DEFAULT 'keep';")
        .unwrap();
    if has_existing {
        db.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE mcp_servers SET host_note='host-owned-cli-import' WHERE id='cli-import'",
                [],
            )
            .unwrap();
    }
    fs::write(temp.path().join("coordination-fixture"), "cli-lite-v1").unwrap();
    let state = AppState::new(db);
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
    let peer = cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "lite-peer")
        .unwrap()
        .expect("real Lite worker must commit the peer before import");

    let expected_count = usize::from(matches!(case, ImportCase::New | ImportCase::Existing));
    assert_eq!(import(&state).unwrap(), expected_count);
    assert_eq!(fs::read(&path).unwrap(), native.as_bytes());
    let after = state.db.get_all_mcp_servers().unwrap();
    if has_existing {
        existing.apps.set_enabled_for(&app, true);
        assert_eq!(
            serde_json::to_value(&after["cli-import"]).unwrap(),
            serde_json::to_value(&existing).unwrap()
        );
        let note: String = state
            .db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT host_note FROM mcp_servers WHERE id='cli-import'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(note, "host-owned-cli-import");
    } else if matches!(case, ImportCase::New) {
        assert_eq!(
            after["cli-import"].server["command"],
            "not-executed-native-import"
        );
        assert_eq!(after["cli-import"].apps.enabled_apps(), vec![app]);
    } else {
        assert!(!after.contains_key("cli-import"));
    }
    assert_eq!(
        cc_switch_store::read_mcp_server_row(&state.db.conn.lock().unwrap(), "lite-peer").unwrap(),
        Some(peer),
        "MCP import must preserve Lite's independently committed record, including zero-change imports"
    );
}

macro_rules! import_cases {
    ($module:ident, $app:path, $import:path) => {
        mod $module {
            use super::*;
            #[test]
            #[ignore = "requires the independently built Lite library test binary"]
            fn new_record_keeps_peer() {
                assert_shared_import($app, $import, ImportCase::New);
            }
            #[test]
            #[ignore = "requires the independently built Lite library test binary"]
            fn existing_record_keeps_connection_and_peer() {
                assert_shared_import($app, $import, ImportCase::Existing);
            }
            #[test]
            #[ignore = "requires the independently built Lite library test binary"]
            fn already_enabled_record_keeps_peer() {
                assert_shared_import($app, $import, ImportCase::AlreadyEnabled);
            }
            #[test]
            #[ignore = "requires the independently built Lite library test binary"]
            fn empty_import_keeps_peer() {
                assert_shared_import($app, $import, ImportCase::Empty);
            }
        }
    };
}

import_cases!(claude, AppType::Claude, McpService::import_from_claude);
import_cases!(codex, AppType::Codex, McpService::import_from_codex);
import_cases!(gemini, AppType::Gemini, McpService::import_from_gemini);
import_cases!(
    opencode,
    AppType::OpenCode,
    McpService::import_from_opencode
);
import_cases!(hermes, AppType::Hermes, McpService::import_from_hermes);
