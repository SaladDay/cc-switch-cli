use std::{fs, path::PathBuf, sync::Arc};

use serde_json::{json, Value};

use super::*;
use crate::database::Database;

pub(super) fn server(id: &str) -> McpServer {
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

pub(super) fn paths() -> Vec<(AppType, PathBuf)> {
    vec![
        (AppType::Claude, crate::config::get_claude_mcp_path()),
        (AppType::Codex, crate::codex_config::get_codex_config_path()),
        (
            AppType::Gemini,
            crate::gemini_config::get_gemini_settings_path(),
        ),
        (
            AppType::OpenCode,
            crate::opencode_config::get_opencode_config_path(),
        ),
        (
            AppType::Hermes,
            crate::hermes_config::get_hermes_config_path(),
        ),
    ]
}

pub(super) fn fixture(initialized: bool) -> AppState {
    if initialized {
        for (app, path) in paths() {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let contents = match app {
                AppType::Codex => "fixture = true\n",
                AppType::Hermes => "fixture: true\n",
                _ => "{ \"fixture\": true }\n",
            };
            fs::write(&path, contents).unwrap();
        }
    }
    let db = Arc::new(Database::init().unwrap());
    db.save_mcp_server(&server("target")).unwrap();
    db.save_mcp_server(&server("peer")).unwrap();
    fs::write(
        crate::config::get_home_dir().join("coordination-fixture"),
        "cli-lite-v1",
    )
    .unwrap();
    AppState::new(db)
}

pub(super) fn all_apps() -> McpApps {
    let mut apps = McpApps::default();
    for app in McpService::supported_mcp_apps() {
        apps.set_enabled_for(&app, true);
    }
    apps
}

pub(super) fn native_entries(app: &AppType, path: &std::path::Path) -> Value {
    let text = fs::read_to_string(path).unwrap();
    let document: Value = match app {
        AppType::Codex => {
            serde_json::to_value(toml::from_str::<toml::Value>(&text).unwrap()).unwrap()
        }
        AppType::Hermes => serde_yaml::from_str(&text).unwrap(),
        _ => serde_json::from_str(&text).unwrap(),
    };
    assert_eq!(document["fixture"], true);
    document[match app {
        AppType::Claude | AppType::Gemini => "mcpServers",
        AppType::OpenCode => "mcp",
        _ => "mcp_servers",
    }]
    .clone()
}

pub(super) fn observed_files() -> Vec<(PathBuf, Option<Vec<u8>>)> {
    paths()
        .into_iter()
        .map(|(_, path)| {
            let bytes = fs::read(&path).ok();
            (path, bytes)
        })
        .collect()
}

pub(super) fn assert_files(before: &[(PathBuf, Option<Vec<u8>>)]) {
    for (path, bytes) in before {
        assert_eq!(&fs::read(path).ok(), bytes, "{}", path.display());
    }
}
