use std::{collections::HashMap, fs};

use serde_json::json;

use cc_switch_lib::{
    get_claude_mcp_path, get_claude_settings_path, AppError, AppType, McpApps, McpLiveDriftKind,
    McpServer, McpService, MultiAppConfig, ProviderService,
};

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs, state_from_config};

#[test]
fn import_default_config_claude_persists_provider() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let settings_path = get_claude_settings_path();
    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent).expect("create claude settings dir");
    }
    let settings = json!({
        "env": {
            "ANTHROPIC_AUTH_TOKEN": "test-key",
            "ANTHROPIC_BASE_URL": "https://api.test"
        }
    });
    fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings).expect("serialize settings"),
    )
    .expect("seed claude settings.json");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Claude);
    let state = state_from_config(config);

    ProviderService::import_default_config(&state, AppType::Claude)
        .expect("import default config succeeds");

    // 验证内存状态
    let guard = state.config.read().expect("lock config");
    let manager = guard
        .get_manager(&AppType::Claude)
        .expect("claude manager present");
    assert_eq!(manager.current, "default");
    let default_provider = manager.providers.get("default").expect("default provider");
    assert_eq!(
        default_provider.settings_config, settings,
        "default provider should capture live settings"
    );
    drop(guard);

    // 验证配置已持久化到数据库
    let providers = state
        .db
        .get_all_providers("claude")
        .expect("load providers from db");
    assert!(
        providers.contains_key("default"),
        "importing default config should persist provider to db"
    );
}

#[test]
fn import_default_config_without_live_file_returns_error() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let state = state_from_config(MultiAppConfig::default());

    let err = ProviderService::import_default_config(&state, AppType::Claude)
        .expect_err("missing live file should error");
    match err {
        AppError::Localized { zh, .. } => assert!(
            zh.contains("Claude Code 配置文件不存在"),
            "unexpected error message: {zh}"
        ),
        AppError::Message(msg) => assert!(
            msg.contains("Claude Code 配置文件不存在"),
            "unexpected error message: {msg}"
        ),
        other => panic!("unexpected error variant: {other:?}"),
    }

    let providers = state
        .db
        .get_all_providers("claude")
        .expect("load providers from db");
    assert!(
        providers.is_empty(),
        "failed import should not persist providers to db"
    );
}

#[test]
fn import_mcp_from_claude_creates_config_and_enables_servers() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let mcp_path = get_claude_mcp_path();
    let claude_json = json!({
        "mcpServers": {
            "echo": {
                "type": "stdio",
                "command": "echo"
            }
        }
    });
    fs::write(
        &mcp_path,
        serde_json::to_string_pretty(&claude_json).expect("serialize claude mcp"),
    )
    .expect("seed ~/.claude.json");

    let state = state_from_config(MultiAppConfig::default());

    let changed = McpService::import_from_claude(&state).expect("import mcp from claude succeeds");
    assert!(
        changed > 0,
        "import should report inserted or normalized entries"
    );

    let guard = state.config.read().expect("lock config");
    // v3.7.0: 检查统一结构
    let servers = guard
        .mcp
        .servers
        .as_ref()
        .expect("unified servers should exist");
    let entry = servers
        .get("echo")
        .expect("server imported into unified structure");
    assert!(
        entry.apps.claude,
        "imported server should have Claude app enabled"
    );
    drop(guard);

    let servers_db = state
        .db
        .get_all_mcp_servers()
        .expect("load mcp servers from db");
    assert!(
        servers_db.contains_key("echo"),
        "state.save should persist imported server to db"
    );
}

#[test]
fn import_mcp_from_claude_invalid_json_preserves_state() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let mcp_path = get_claude_mcp_path();
    fs::write(&mcp_path, "{\"mcpServers\":") // 不完整 JSON
        .expect("seed invalid ~/.claude.json");

    let state = state_from_config(MultiAppConfig::default());

    let err =
        McpService::import_from_claude(&state).expect_err("invalid json should bubble up error");
    match err {
        AppError::McpValidation(msg) => assert!(
            msg.contains("解析 ~/.claude.json 失败"),
            "unexpected error message: {msg}"
        ),
        other => panic!("unexpected error variant: {other:?}"),
    }

    let servers_db = state
        .db
        .get_all_mcp_servers()
        .expect("load mcp servers from db");
    assert!(
        servers_db.is_empty(),
        "failed import should not persist servers to db"
    );
}

#[test]
fn import_mcp_from_gemini_imports_http_and_sse_servers() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let gemini_dir = home.join(".gemini");
    fs::create_dir_all(&gemini_dir).expect("create gemini dir");
    let settings_path = gemini_dir.join("settings.json");
    let settings = json!({
        "mcpServers": {
            "remote_http": {
                "httpUrl": "http://localhost:1234"
            },
            "remote_sse": {
                "url": "http://localhost:5678"
            },
            "local_stdio": {
                "command": "echo"
            }
        }
    });
    fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings).expect("serialize gemini settings"),
    )
    .expect("seed ~/.gemini/settings.json");

    let state = state_from_config(MultiAppConfig::default());

    McpService::import_from_gemini(&state).expect("import mcp from gemini succeeds");

    let guard = state.config.read().expect("lock config");
    // v3.7.0: 检查统一结构
    let servers = guard
        .mcp
        .servers
        .as_ref()
        .expect("unified servers should exist");

    let remote_http = servers
        .get("remote_http")
        .expect("remote_http server imported into unified structure");
    assert!(
        remote_http.apps.gemini,
        "remote_http should enable Gemini app"
    );
    assert_eq!(
        remote_http.server.get("type").and_then(|v| v.as_str()),
        Some("http"),
        "remote_http should be normalized to type http"
    );
    assert!(
        remote_http
            .server
            .get("url")
            .and_then(|v| v.as_str())
            .is_some_and(|v| v == "http://localhost:1234"),
        "remote_http should have url field"
    );
    assert!(
        remote_http.server.get("httpUrl").is_none(),
        "remote_http should not keep httpUrl field"
    );

    let remote_sse = servers
        .get("remote_sse")
        .expect("remote_sse server imported into unified structure");
    assert!(
        remote_sse.apps.gemini,
        "remote_sse should enable Gemini app"
    );
    assert_eq!(
        remote_sse.server.get("type").and_then(|v| v.as_str()),
        Some("sse"),
        "remote_sse should be normalized to type sse"
    );
    assert!(
        remote_sse
            .server
            .get("url")
            .and_then(|v| v.as_str())
            .is_some_and(|v| v == "http://localhost:5678"),
        "remote_sse should have url field"
    );

    let local_stdio = servers
        .get("local_stdio")
        .expect("local_stdio server imported into unified structure");
    assert!(
        local_stdio.apps.gemini,
        "local_stdio should enable Gemini app"
    );
    assert_eq!(
        local_stdio.server.get("type").and_then(|v| v.as_str()),
        Some("stdio"),
        "local_stdio should be normalized to type stdio"
    );
    assert!(
        local_stdio
            .server
            .get("command")
            .and_then(|v| v.as_str())
            .is_some_and(|v| v == "echo"),
        "local_stdio should have command field"
    );
}

#[test]
fn import_mcp_from_openclaw_imports_registry_servers() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let openclaw_dir = home.join(".openclaw");
    fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    fs::write(
        openclaw_dir.join("openclaw.json"),
        r#"{
  mcp: {
    servers: {
      context7: {
        command: "uvx",
        args: ["context7-mcp"],
      },
      docs: {
        url: "https://mcp.example.com/stream",
        transport: "streamable-http",
      },
    },
  },
}"#,
    )
    .expect("seed openclaw config");

    let state = state_from_config(MultiAppConfig::default());

    let changed =
        McpService::import_from_openclaw(&state).expect("import mcp from openclaw succeeds");
    assert_eq!(changed, 2);

    let guard = state.config.read().expect("lock config");
    let servers = guard.mcp.servers.as_ref().expect("unified servers");
    let context7 = servers.get("context7").expect("context7 imported");
    assert!(context7.apps.openclaw);
    assert_eq!(context7.server["type"], json!("stdio"));
    assert_eq!(context7.server["command"], json!("uvx"));

    let docs = servers.get("docs").expect("docs imported");
    assert!(docs.apps.openclaw);
    assert_eq!(docs.server["type"], json!("http"));
    assert_eq!(docs.server["url"], json!("https://mcp.example.com/stream"));
}

#[test]
fn set_mcp_enabled_for_codex_writes_live_config() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    // 创建 Codex 配置目录和文件
    let codex_dir = home.join(".codex");
    fs::create_dir_all(&codex_dir).expect("create codex dir");
    fs::write(
        codex_dir.join("auth.json"),
        r#"{"OPENAI_API_KEY":"test-key"}"#,
    )
    .expect("create auth.json");
    fs::write(codex_dir.join("config.toml"), "").expect("create empty config.toml");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);

    // v3.7.0: 使用统一结构
    config.mcp.servers = Some(HashMap::new());
    config.mcp.servers.as_mut().unwrap().insert(
        "codex-server".into(),
        McpServer {
            id: "codex-server".to_string(),
            name: "Codex Server".to_string(),
            server: json!({
                "type": "stdio",
                "command": "echo",
                "env": {
                    "API_KEY": "secret",
                    "PROJECT_ROOT": ""
                }
            }),
            apps: McpApps {
                claude: false,
                codex: false, // 初始未启用
                gemini: false,
                opencode: false,
                openclaw: false,
                hermes: false,
            },
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        },
    );

    let state = state_from_config(config);

    // v3.7.0: 使用 toggle_app 替代 set_enabled
    McpService::toggle_app(&state, "codex-server", AppType::Codex, true)
        .expect("toggle_app should succeed");

    let guard = state.config.read().expect("lock config");
    let entry = guard
        .mcp
        .servers
        .as_ref()
        .unwrap()
        .get("codex-server")
        .expect("codex server exists");
    assert!(
        entry.apps.codex,
        "server should have Codex app enabled after toggle"
    );
    drop(guard);

    let toml_path = cc_switch_lib::get_codex_config_path();
    assert!(
        toml_path.exists(),
        "enabling server should trigger sync to ~/.codex/config.toml"
    );
    let toml_text = fs::read_to_string(&toml_path).expect("read codex config");
    assert!(
        toml_text.contains("codex-server"),
        "codex config should include the enabled server definition"
    );
    assert!(
        toml_text.contains("[mcp_servers.codex-server.env]"),
        "codex config should include env table for enabled server"
    );
    assert!(
        toml_text.contains("API_KEY = \"secret\""),
        "codex config should include API_KEY env entry"
    );
    assert!(
        toml_text.contains("PROJECT_ROOT = \"\""),
        "codex config should preserve empty env values"
    );
}

#[test]
fn set_mcp_enabled_for_openclaw_writes_live_config() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let openclaw_dir = home.join(".openclaw");
    fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
    let openclaw_path = openclaw_dir.join("openclaw.json");
    fs::write(
        &openclaw_path,
        r#"{
  models: {
    mode: "merge",
    providers: {},
  },
}"#,
    )
    .expect("seed openclaw config");

    let mut config = MultiAppConfig::default();
    config.mcp.servers = Some(HashMap::new());
    config.mcp.servers.as_mut().unwrap().insert(
        "docs".into(),
        McpServer {
            id: "docs".to_string(),
            name: "Docs".to_string(),
            server: json!({
                "type": "http",
                "url": "https://mcp.example.com/stream",
                "headers": {
                    "Authorization": "Bearer token"
                }
            }),
            apps: McpApps {
                claude: false,
                codex: false,
                gemini: false,
                opencode: false,
                openclaw: false,
                hermes: false,
            },
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        },
    );

    let state = state_from_config(config);

    McpService::toggle_app(&state, "docs", AppType::OpenClaw, true)
        .expect("toggle openclaw mcp should succeed");

    let raw = fs::read_to_string(&openclaw_path).expect("read openclaw config");
    let parsed: serde_json::Value = json5::from_str(&raw).expect("parse openclaw json5");
    let docs = parsed
        .pointer("/mcp/servers/docs")
        .expect("OpenClaw config should include docs server");

    assert_eq!(docs["url"], json!("https://mcp.example.com/stream"));
    assert_eq!(docs["transport"], json!("streamable-http"));
    assert_eq!(docs["headers"]["Authorization"], json!("Bearer token"));
}

#[test]
fn set_mcp_enabled_for_codex_writes_remote_headers_once_as_http_headers() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let codex_dir = home.join(".codex");
    fs::create_dir_all(&codex_dir).expect("create codex dir");
    fs::write(
        codex_dir.join("auth.json"),
        r#"{"OPENAI_API_KEY":"test-key"}"#,
    )
    .expect("create auth.json");
    fs::write(codex_dir.join("config.toml"), "").expect("create empty config.toml");

    let mut config = MultiAppConfig::default();
    config.ensure_app(&AppType::Codex);
    config.mcp.servers = Some(HashMap::new());
    config.mcp.servers.as_mut().unwrap().insert(
        "remote-headers".into(),
        McpServer {
            id: "remote-headers".to_string(),
            name: "Remote Headers".to_string(),
            server: json!({
                "type": "http",
                "url": "https://example.com/mcp",
                "headers": {
                    "Authorization": "Bearer token"
                }
            }),
            apps: McpApps {
                claude: false,
                codex: false,
                gemini: false,
                opencode: false,
                openclaw: false,
                hermes: false,
            },
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        },
    );

    let state = state_from_config(config);

    McpService::toggle_app(&state, "remote-headers", AppType::Codex, true)
        .expect("toggle_app should succeed");

    let toml_path = cc_switch_lib::get_codex_config_path();
    let toml_text = fs::read_to_string(&toml_path).expect("read codex config");
    assert!(
        toml_text.contains("[mcp_servers.remote-headers.http_headers]"),
        "codex remote headers should be written as http_headers, got: {toml_text}"
    );
    assert!(
        toml_text.contains("Authorization = \"Bearer token\""),
        "codex remote headers should preserve Authorization value, got: {toml_text}"
    );
    assert!(
        !toml_text.contains("[mcp_servers.remote-headers.headers]"),
        "codex config should not also write legacy headers table, got: {toml_text}"
    );
}

#[test]
fn codex_mcp_live_drift_reports_changed_live_only_db_only_and_in_sync() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let codex_dir = home.join(".codex");
    fs::create_dir_all(&codex_dir).expect("create codex dir");
    fs::write(
        codex_dir.join("config.toml"),
        r#"[mcp_servers.changed]
type = "stdio"
command = "live-command"

[mcp_servers.in_sync]
type = "stdio"
command = "same-command"

[mcp_servers.live_only]
type = "http"
url = "https://live.example.com/mcp"
"#,
    )
    .expect("write codex config");

    let mut config = MultiAppConfig::default();
    config.mcp.servers = Some(HashMap::new());
    let servers = config.mcp.servers.as_mut().unwrap();
    for (id, command) in [
        ("changed", "db-command"),
        ("db_only", "db-only-command"),
        ("in_sync", "same-command"),
    ] {
        servers.insert(
            id.to_string(),
            McpServer {
                id: id.to_string(),
                name: id.to_string(),
                server: json!({
                    "type": "stdio",
                    "command": command
                }),
                apps: McpApps {
                    claude: false,
                    codex: true,
                    gemini: false,
                    opencode: false,
                    openclaw: false,
                    hermes: false,
                },
                description: None,
                homepage: None,
                docs: None,
                tags: Vec::new(),
            },
        );
    }

    let state = state_from_config(config);
    let report = McpService::get_live_drift(&state, AppType::Codex).expect("get live drift");

    assert_eq!(report.app, AppType::Codex);
    let entries = report
        .entries
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<HashMap<_, _>>();

    assert_eq!(entries["changed"].kind, McpLiveDriftKind::Changed);
    assert_eq!(
        entries["changed"].db_spec.as_ref().unwrap()["command"],
        "db-command"
    );
    assert_eq!(
        entries["changed"].live_spec.as_ref().unwrap()["command"],
        "live-command"
    );

    assert_eq!(entries["db_only"].kind, McpLiveDriftKind::DbOnly);
    assert!(entries["db_only"].db_spec.is_some());
    assert!(entries["db_only"].live_spec.is_none());

    assert_eq!(entries["live_only"].kind, McpLiveDriftKind::LiveOnly);
    assert!(entries["live_only"].db_spec.is_none());
    assert!(entries["live_only"].live_spec.is_some());

    assert_eq!(entries["in_sync"].kind, McpLiveDriftKind::InSync);
}

#[test]
fn codex_mcp_import_live_server_overwrites_spec_and_preserves_metadata() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let codex_dir = home.join(".codex");
    fs::create_dir_all(&codex_dir).expect("create codex dir");
    fs::write(
        codex_dir.join("config.toml"),
        r#"[mcp_servers.changed]
type = "stdio"
command = "live-command"

[mcp_servers.live_only]
type = "http"
url = "https://live.example.com/mcp"
"#,
    )
    .expect("write codex config");

    let mut config = MultiAppConfig::default();
    config.mcp.servers = Some(HashMap::new());
    config.mcp.servers.as_mut().unwrap().insert(
        "changed".to_string(),
        McpServer {
            id: "changed".to_string(),
            name: "Existing Name".to_string(),
            server: json!({
                "type": "stdio",
                "command": "db-command"
            }),
            apps: McpApps::default(),
            description: Some("keep description".to_string()),
            homepage: Some("https://homepage.example.com".to_string()),
            docs: Some("https://docs.example.com".to_string()),
            tags: vec!["keep-tag".to_string()],
        },
    );

    let state = state_from_config(config);

    McpService::import_live_server(&state, AppType::Codex, "changed")
        .expect("import changed live server");
    McpService::import_live_server(&state, AppType::Codex, "live_only")
        .expect("import live-only server");

    let guard = state.config.read().expect("lock config");
    let servers = guard.mcp.servers.as_ref().expect("servers");

    let changed = servers.get("changed").expect("changed server");
    assert_eq!(changed.server["command"], "live-command");
    assert!(changed.apps.codex, "Codex app should be enabled");
    assert_eq!(changed.name, "Existing Name");
    assert_eq!(changed.description.as_deref(), Some("keep description"));
    assert_eq!(
        changed.homepage.as_deref(),
        Some("https://homepage.example.com")
    );
    assert_eq!(changed.docs.as_deref(), Some("https://docs.example.com"));
    assert_eq!(changed.tags, vec!["keep-tag".to_string()]);

    let live_only = servers.get("live_only").expect("live-only server");
    assert_eq!(live_only.id, "live_only");
    assert_eq!(live_only.name, "live_only");
    assert_eq!(live_only.server["type"], "http");
    assert_eq!(live_only.server["url"], "https://live.example.com/mcp");
    assert!(live_only.apps.codex);
    assert!(!live_only.apps.claude);
    assert!(!live_only.apps.gemini);
    assert!(!live_only.apps.opencode);
    assert!(!live_only.apps.openclaw);
    assert!(!live_only.apps.hermes);
}

#[test]
fn codex_mcp_push_db_server_to_live_overwrites_live_spec() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let codex_dir = home.join(".codex");
    fs::create_dir_all(&codex_dir).expect("create codex dir");
    fs::write(
        codex_dir.join("config.toml"),
        r#"[mcp_servers.changed]
type = "stdio"
command = "live-command"
"#,
    )
    .expect("write codex config");

    let mut config = MultiAppConfig::default();
    config.mcp.servers = Some(HashMap::new());
    config.mcp.servers.as_mut().unwrap().insert(
        "changed".to_string(),
        McpServer {
            id: "changed".to_string(),
            name: "Changed".to_string(),
            server: json!({
                "type": "stdio",
                "command": "db-command",
                "args": ["from-db"]
            }),
            apps: McpApps {
                claude: false,
                codex: true,
                gemini: false,
                opencode: false,
                openclaw: false,
                hermes: false,
            },
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        },
    );

    let state = state_from_config(config);
    McpService::push_db_server_to_live(&state, AppType::Codex, "changed")
        .expect("push db server to live");

    let toml_text =
        fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read codex config");
    let live: toml::Value = toml::from_str(&toml_text).expect("parse codex config");
    let changed = live
        .get("mcp_servers")
        .and_then(|servers| servers.get("changed"))
        .expect("changed live server");
    assert_eq!(
        changed.get("command").and_then(|value| value.as_str()),
        Some("db-command")
    );
    assert_eq!(
        changed
            .get("args")
            .and_then(|value| value.as_array())
            .and_then(|args| args.first())
            .and_then(|value| value.as_str()),
        Some("from-db")
    );
}

#[test]
fn upsert_server_skips_live_sync_when_gemini_uninitialized() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    assert!(
        !home.join(".gemini").exists(),
        "precondition: ~/.gemini should not exist"
    );

    let mut config = MultiAppConfig::default();
    config.mcp.servers = Some(HashMap::new());

    let state = state_from_config(config);

    let server = McpServer {
        id: "gemini-server".to_string(),
        name: "Gemini Server".to_string(),
        server: json!({
            "type": "http",
            "url": "http://localhost:1234"
        }),
        apps: McpApps {
            claude: false,
            codex: false,
            gemini: true,
            opencode: false,
            openclaw: false,
            hermes: false,
        },
        description: None,
        homepage: None,
        docs: None,
        tags: Vec::new(),
    };

    McpService::upsert_server(&state, server).expect("upsert server should succeed");

    assert!(
        !home.join(".gemini").exists(),
        "should_sync=auto: upsert should not create ~/.gemini when uninitialized"
    );
}

#[test]
fn upsert_server_disables_app_removes_from_gemini_live() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let url = "http://localhost:1234";

    // 预先写入 Gemini live 配置，包含待删除的 MCP server
    let gemini_dir = home.join(".gemini");
    fs::create_dir_all(&gemini_dir).expect("create gemini dir");
    let settings_path = gemini_dir.join("settings.json");
    let settings = json!({
        "mcpServers": {
            "remove_me": {
                "httpUrl": url
            }
        }
    });
    fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings).expect("serialize gemini settings"),
    )
    .expect("seed ~/.gemini/settings.json");

    let seeded_text = fs::read_to_string(&settings_path).expect("read gemini settings after seed");
    let seeded_json: serde_json::Value =
        serde_json::from_str(&seeded_text).expect("parse gemini settings after seed");
    let seeded_present = seeded_json
        .get("mcpServers")
        .and_then(|v| v.as_object())
        .is_some_and(|mcp_servers| mcp_servers.contains_key("remove_me"));
    assert!(
        seeded_present,
        "seeded ~/.gemini/settings.json should include remove_me"
    );

    // 初始化统一结构：旧值 Gemini = true
    let mut config = MultiAppConfig::default();
    config.mcp.servers = Some(HashMap::new());
    config.mcp.servers.as_mut().unwrap().insert(
        "remove_me".into(),
        McpServer {
            id: "remove_me".to_string(),
            name: "Remove Me".to_string(),
            server: json!({
                "type": "http",
                "url": url
            }),
            apps: McpApps {
                claude: false,
                codex: false,
                gemini: true,
                opencode: false,
                openclaw: false,
                hermes: false,
            },
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        },
    );

    let state = state_from_config(config);

    // 模拟“取消勾选 Gemini”
    let server = McpServer {
        id: "remove_me".to_string(),
        name: "Remove Me".to_string(),
        server: json!({
            "type": "http",
            "url": url
        }),
        apps: McpApps {
            claude: false,
            codex: false,
            gemini: false,
            opencode: false,
            openclaw: false,
            hermes: false,
        },
        description: None,
        homepage: None,
        docs: None,
        tags: Vec::new(),
    };

    McpService::upsert_server(&state, server).expect("upsert server succeeds");

    // 断言：Gemini live 中应移除该 server
    let settings_text = fs::read_to_string(&settings_path).expect("read gemini settings");
    let settings_json: serde_json::Value =
        serde_json::from_str(&settings_text).expect("parse gemini settings");
    let remove_me_present = settings_json
        .get("mcpServers")
        .and_then(|v| v.as_object())
        .is_some_and(|mcp_servers| mcp_servers.contains_key("remove_me"));
    assert!(
        !remove_me_present,
        "upsert with Gemini disabled should remove it from ~/.gemini/settings.json, got: {settings_text}"
    );
}

#[test]
fn sync_all_enabled_removes_disabled_gemini_server_from_live_config() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let url = "http://localhost:1234";

    let gemini_dir = home.join(".gemini");
    fs::create_dir_all(&gemini_dir).expect("create gemini dir");
    let settings_path = gemini_dir.join("settings.json");
    let settings = json!({
        "mcpServers": {
            "remove_me": {
                "httpUrl": url
            }
        }
    });
    fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings).expect("serialize gemini settings"),
    )
    .expect("seed ~/.gemini/settings.json");

    let mut config = MultiAppConfig::default();
    config.mcp.servers = Some(HashMap::new());
    config.mcp.servers.as_mut().unwrap().insert(
        "remove_me".into(),
        McpServer {
            id: "remove_me".to_string(),
            name: "Remove Me".to_string(),
            server: json!({
                "type": "http",
                "url": url
            }),
            apps: McpApps {
                claude: false,
                codex: false,
                gemini: false,
                opencode: false,
                openclaw: false,
                hermes: false,
            },
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        },
    );

    let state = state_from_config(config);
    state.save().expect("persist config to db");

    McpService::sync_all_enabled(&state).expect("sync_all_enabled succeeds");

    let settings_text = fs::read_to_string(&settings_path).expect("read gemini settings");
    let settings_json: serde_json::Value =
        serde_json::from_str(&settings_text).expect("parse gemini settings");
    let remove_me_present = settings_json
        .get("mcpServers")
        .and_then(|v| v.as_object())
        .is_some_and(|mcp_servers| mcp_servers.contains_key("remove_me"));
    assert!(
        !remove_me_present,
        "sync_all_enabled should remove disabled Gemini binding from live config, got: {settings_text}"
    );
}
