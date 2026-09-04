use std::{collections::HashMap, fs};

use serde_json::json;

use cc_switch_lib::{
    get_claude_mcp_path, get_claude_settings_path, AppError, AppType, McpApps, McpServer,
    McpService, MultiAppConfig, ProviderService,
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
                "command": "echo",
                "trust": true
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
    assert_eq!(entry.server["trust"], true);
    drop(guard);

    let servers_db = state
        .db
        .get_all_mcp_servers()
        .expect("load mcp servers from db");
    assert!(
        servers_db.contains_key("echo"),
        "state.save should persist imported server to db"
    );
    assert_eq!(servers_db["echo"].server["trust"], true);
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
#[allow(deprecated)]
fn legacy_sync_wrapper_still_replaces_claude_servers() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".claude")).expect("create Claude dir");
    fs::write(
        get_claude_mcp_path(),
        json!({
            "mcpServers": {
                "bad": 42,
                "stale": {"command":"old"},
                "legacy": {"command":"old","trust":true}
            }
        })
        .to_string(),
    )
    .expect("seed Claude config");

    let mut config = MultiAppConfig::default();
    config.mcp.claude.servers.insert(
        "legacy".to_owned(),
        json!({
            "enabled": true,
            "server": {"type":"stdio","command":"npx"}
        }),
    );
    cc_switch_lib::sync_enabled_to_claude(&config).expect("sync legacy MCP config");

    let live: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(get_claude_mcp_path()).expect("read Claude config"),
    )
    .expect("parse Claude config");
    assert!(live["mcpServers"].get("bad").is_none());
    assert!(live["mcpServers"].get("stale").is_none());
    assert_eq!(live["mcpServers"]["legacy"]["command"], "npx");
    assert_eq!(live["mcpServers"]["legacy"]["trust"], true);
}

#[test]
fn disable_refreshes_native_snapshot_before_restore() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();
    let mcp_path = get_claude_mcp_path();
    fs::write(
        &mcp_path,
        json!({
            "mcpServers": {
                "snapshot": {"command": "npx", "trust": "v1"}
            }
        })
        .to_string(),
    )
    .expect("seed Claude MCP config");
    let state = state_from_config(MultiAppConfig::default());
    McpService::import_from_claude(&state).expect("import Claude MCP server");

    fs::write(
        &mcp_path,
        json!({
            "mcpServers": {
                "snapshot": {"command": 42, "trust": "v2"}
            }
        })
        .to_string(),
    )
    .expect("update native-only field");
    McpService::toggle_app(&state, "snapshot", AppType::Claude, false)
        .expect("disable Claude MCP server");
    McpService::toggle_app(&state, "snapshot", AppType::Claude, true)
        .expect("restore Claude MCP server");

    let live: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(mcp_path).unwrap()).unwrap();
    assert_eq!(live["mcpServers"]["snapshot"]["trust"], "v2");
    assert_eq!(live["mcpServers"]["snapshot"]["command"], "npx");
}

#[test]
fn enabled_edit_restores_a_missing_owned_claude_entry() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();
    let mcp_path = get_claude_mcp_path();
    fs::write(
        &mcp_path,
        json!({"mcpServers":{"snapshot":{"command":"npx","trust":"kept"}}}).to_string(),
    )
    .expect("seed Claude MCP config");
    let state = state_from_config(MultiAppConfig::default());
    McpService::import_from_claude(&state).expect("import Claude MCP server");

    fs::write(&mcp_path, "{}").expect("remove owned live entry externally");
    let mut server = state.config.read().unwrap().mcp.servers.as_ref().unwrap()["snapshot"].clone();
    server.server["command"] = json!("uvx");
    McpService::upsert_server(&state, server).expect("edit enabled owned server");

    let live: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(mcp_path).unwrap()).unwrap();
    assert_eq!(live["mcpServers"]["snapshot"]["command"], "uvx");
    assert_eq!(live["mcpServers"]["snapshot"]["trust"], "kept");
}

#[test]
fn sync_refreshes_native_snapshot_before_restore() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();
    let mcp_path = get_claude_mcp_path();
    fs::write(
        &mcp_path,
        json!({"mcpServers":{"snapshot":{"command":"npx","trust":"v1"}}}).to_string(),
    )
    .expect("seed Claude MCP config");
    let state = state_from_config(MultiAppConfig::default());
    McpService::import_from_claude(&state).expect("import Claude MCP server");
    state
        .config
        .write()
        .unwrap()
        .mcp
        .servers
        .as_mut()
        .unwrap()
        .get_mut("snapshot")
        .unwrap()
        .apps
        .claude = false;
    state.save().expect("persist disabled catalog state");
    fs::write(
        &mcp_path,
        json!({"mcpServers":{"snapshot":{"command":"npx","trust":"v2"}}}).to_string(),
    )
    .expect("update native-only field");

    McpService::sync_all_enabled(&state).expect("sync disabled managed server");
    McpService::toggle_app(&state, "snapshot", AppType::Claude, true)
        .expect("restore Claude MCP server");

    let live: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(mcp_path).unwrap()).unwrap();
    assert_eq!(live["mcpServers"]["snapshot"]["trust"], "v2");
}

#[test]
fn app_sync_validates_the_whole_batch_before_writing() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".codex")).expect("create Codex dir");
    let path = cc_switch_lib::get_codex_config_path();
    let original = "model = \"keep\"\n[mcp_servers.existing]\ncommand = \"old\"\n";
    fs::write(&path, original).expect("seed Codex config");

    let state = state_from_config(MultiAppConfig::default());
    for id in ["a-valid", "z-invalid"] {
        McpService::upsert_server(
            &state,
            McpServer {
                id: id.to_owned(),
                name: id.to_owned(),
                server: json!({"command":"npx"}),
                apps: McpApps {
                    codex: true,
                    ..McpApps::default()
                },
                description: None,
                homepage: None,
                docs: None,
                tags: Vec::new(),
            },
        )
        .expect("create managed Codex server");
    }
    state
        .config
        .write()
        .unwrap()
        .mcp
        .servers
        .as_mut()
        .unwrap()
        .get_mut("z-invalid")
        .unwrap()
        .server = json!({"command":"npx","future":null});
    state.save().expect("persist invalid test fixture");
    fs::write(&path, original).expect("restore pre-sync Codex config");

    McpService::sync_enabled_for_app(&state, &AppType::Codex)
        .expect_err("invalid second projection must reject the batch");
    assert_eq!(fs::read_to_string(path).unwrap(), original);
}

#[test]
fn app_sync_preserves_an_unowned_unparseable_codex_sibling() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".codex")).expect("create Codex dir");
    let path = cc_switch_lib::get_codex_config_path();
    fs::write(&path, "mcp_servers = { bad = 1 }\n").expect("seed unowned sibling");
    let state = state_from_config(MultiAppConfig::default());
    McpService::upsert_server(
        &state,
        McpServer {
            id: "owned".to_owned(),
            name: "Owned".to_owned(),
            server: json!({"command":"npx"}),
            apps: McpApps {
                codex: true,
                ..McpApps::default()
            },
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        },
    )
    .expect("create managed Codex server");
    let mut server = state.config.read().unwrap().mcp.servers.as_ref().unwrap()["owned"].clone();
    server.server["command"] = json!("uvx");
    state
        .config
        .write()
        .unwrap()
        .mcp
        .servers
        .as_mut()
        .unwrap()
        .insert("owned".to_owned(), server);
    state.save().expect("persist edited catalog row");
    fs::write(
        &path,
        "mcp_servers = { bad = 1, owned = { command = \"old\" } }\n",
    )
    .expect("restore native batch fixture");

    McpService::sync_enabled_for_app(&state, &AppType::Codex).expect("sync only the owned entry");

    let live = fs::read_to_string(path).unwrap();
    assert!(live.contains("bad = 1"));
    assert!(live.contains("command = \"uvx\""));
}

#[test]
fn opencode_and_hermes_imports_keep_loose_container_and_path_errors() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let state = state_from_config(MultiAppConfig::default());

    let opencode_path = home.join(".config/opencode/opencode.json");
    fs::create_dir_all(opencode_path.parent().unwrap()).expect("create OpenCode dir");
    fs::write(&opencode_path, r#"{"mcp":[]}"#).expect("write loose OpenCode container");
    assert_eq!(
        McpService::import_from_opencode(&state).expect("ignore loose OpenCode container"),
        0
    );
    fs::write(&opencode_path, "{").expect("write invalid OpenCode JSON");
    assert!(matches!(
        McpService::import_from_opencode(&state),
        Err(AppError::Json { .. })
    ));

    let hermes_path = home.join(".hermes/config.yaml");
    fs::create_dir_all(hermes_path.parent().unwrap()).expect("create Hermes dir");
    fs::write(&hermes_path, "mcp_servers: []\n").expect("write loose Hermes container");
    assert_eq!(
        McpService::import_from_hermes(&state).expect("ignore loose Hermes container"),
        0
    );
    fs::write(&hermes_path, "mcp_servers: [\n").expect("write invalid Hermes YAML");
    assert!(matches!(
        McpService::import_from_hermes(&state),
        Err(AppError::Config(_))
    ));
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
fn import_codex_legacy_headers_canonicalizes_to_headers() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let codex_dir = home.join(".codex");
    fs::create_dir_all(&codex_dir).expect("create codex dir");
    fs::write(
        codex_dir.join("config.toml"),
        r#"
[mcp_servers.remote]
type = "http"
url = "https://example.com/mcp"

[mcp_servers.remote.headers]
Authorization = "Bearer legacy-token"
"#,
    )
    .expect("seed Codex config with legacy headers");

    let state = state_from_config(MultiAppConfig::default());
    McpService::import_from_codex(&state).expect("import Codex MCP");

    let guard = state.config.read().expect("lock config");
    let remote = guard
        .mcp
        .servers
        .as_ref()
        .and_then(|servers| servers.get("remote"))
        .expect("remote MCP imported");
    assert_eq!(
        remote.server["headers"]["Authorization"],
        "Bearer legacy-token"
    );
    assert!(
        remote.server.get("http_headers").is_none(),
        "unified config must keep only canonical headers"
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

    let gemini_dir = home.join(".gemini");
    fs::create_dir_all(&gemini_dir).expect("initialize Gemini after the skipped write");
    let settings_path = gemini_dir.join("settings.json");
    fs::write(
        &settings_path,
        json!({"mcpServers":{"gemini-server":{"command":"user-owned"}}}).to_string(),
    )
    .expect("seed an unmanaged same-id server");

    McpService::toggle_app(&state, "gemini-server", AppType::Gemini, false)
        .expect("disable catalog link");
    McpService::delete_server(&state, "gemini-server").expect("delete catalog server");

    let live: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(settings_path).unwrap()).unwrap();
    assert_eq!(live["mcpServers"]["gemini-server"]["command"], "user-owned");
}

#[test]
fn first_enable_refuses_an_unmanaged_same_id_server() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();
    let mcp_path = get_claude_mcp_path();
    let original = json!({
        "mcpServers": {
            "collision": {"command": "user-owned", "trust": true}
        }
    })
    .to_string();
    fs::write(&mcp_path, &original).expect("seed unmanaged Claude server");

    let mut config = MultiAppConfig::default();
    config.mcp.servers.as_mut().unwrap().insert(
        "collision".into(),
        McpServer {
            id: "collision".into(),
            name: "Collision".into(),
            server: json!({"command":"managed"}),
            apps: McpApps::default(),
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        },
    );
    let state = state_from_config(config);

    let error = McpService::toggle_app(&state, "collision", AppType::Claude, true)
        .expect_err("same-id unmanaged server must not be claimed");
    assert!(error.to_string().contains("unmanaged MCP server"));
    assert_eq!(fs::read_to_string(&mcp_path).unwrap(), original);
    assert!(
        !state.config.read().unwrap().mcp.servers.as_ref().unwrap()["collision"]
            .apps
            .claude
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
                hermes: false,
            },
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        },
    );

    let state = state_from_config(config);
    McpService::import_from_gemini(&state).expect("claim the existing Gemini server");

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
fn sync_all_enabled_preserves_an_unowned_gemini_server() {
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
        remove_me_present,
        "unowned native entry changed: {settings_text}"
    );
}

#[test]
fn sync_all_enabled_removes_an_owned_disabled_gemini_server() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let gemini_dir = home.join(".gemini");
    fs::create_dir_all(&gemini_dir).expect("create Gemini dir");
    let settings_path = gemini_dir.join("settings.json");
    fs::write(&settings_path, "{}").expect("initialize Gemini settings");
    let state = state_from_config(MultiAppConfig::default());
    let server = McpServer {
        id: "owned".into(),
        name: "Owned".into(),
        server: json!({"type":"http","url":"http://localhost:1234"}),
        apps: McpApps {
            gemini: true,
            ..McpApps::default()
        },
        description: None,
        homepage: None,
        docs: None,
        tags: Vec::new(),
    };
    McpService::upsert_server(&state, server).expect("create managed Gemini server");
    McpService::toggle_app(&state, "owned", AppType::Gemini, false)
        .expect("disable managed Gemini server");
    fs::write(
        &settings_path,
        json!({"mcpServers":{"owned":{"command":"stale"}}}).to_string(),
    )
    .expect("restore a stale managed entry");

    McpService::sync_all_enabled(&state).expect("sync managed servers");

    let live: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(settings_path).unwrap()).unwrap();
    assert!(live["mcpServers"].get("owned").is_none());
}

#[test]
fn sync_all_enabled_continues_after_another_app_fails() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    fs::create_dir_all(home.join(".claude")).expect("create Claude dir");
    fs::write(get_claude_mcp_path(), "{}").expect("initialize Claude MCP config");

    let gemini_dir = home.join(".gemini");
    fs::create_dir_all(&gemini_dir).expect("create Gemini dir");
    let gemini_path = gemini_dir.join("settings.json");
    fs::write(&gemini_path, "{}").expect("initialize Gemini settings");

    let state = state_from_config(MultiAppConfig::default());
    McpService::upsert_server(
        &state,
        McpServer {
            id: "remove_me".to_string(),
            name: "Remove Me".to_string(),
            server: json!({
                "type": "http",
                "url": "http://localhost:1234"
            }),
            apps: McpApps {
                claude: true,
                gemini: true,
                ..McpApps::default()
            },
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        },
    )
    .expect("create managed server");
    McpService::set_apps(&state, "remove_me", McpApps::default()).expect("disable managed server");

    fs::write(get_claude_mcp_path(), "{\"mcpServers\":").expect("seed invalid Claude MCP config");
    fs::write(
        &gemini_path,
        json!({
            "mcpServers": {
                "remove_me": {
                    "httpUrl": "http://localhost:1234"
                }
            }
        })
        .to_string(),
    )
    .expect("seed Gemini settings");

    let error = McpService::sync_all_enabled(&state)
        .expect_err("the invalid Claude file should be reported");
    assert!(
        error.to_string().contains("claude"),
        "aggregate error should identify the failed app: {error}"
    );

    let gemini: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&gemini_path).expect("read Gemini settings after sync"),
    )
    .expect("parse Gemini settings after sync");
    assert!(
        gemini
            .get("mcpServers")
            .and_then(serde_json::Value::as_object)
            .is_none_or(|servers| !servers.contains_key("remove_me")),
        "Gemini projection must still run after Claude fails: {gemini}"
    );
}

#[test]
fn upsert_refreshes_a_preserved_disabled_opencode_entry() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let opencode_dir = home.join(".config").join("opencode");
    fs::create_dir_all(&opencode_dir).expect("create OpenCode dir");
    let path = opencode_dir.join("opencode.json");
    fs::write(
        &path,
        json!({
            "mcp": {
                "disabled": {
                    "type":"local",
                    "command":["old"],
                    "enabled":false,
                    "timeout":30
                }
            }
        })
        .to_string(),
    )
    .expect("seed OpenCode config");

    let previous = McpServer {
        id: "disabled".to_owned(),
        name: "Disabled".to_owned(),
        server: json!({"command":"old"}),
        apps: McpApps::default(),
        description: None,
        homepage: None,
        docs: None,
        tags: Vec::new(),
    };
    let mut config = MultiAppConfig::default();
    config
        .mcp
        .servers
        .as_mut()
        .unwrap()
        .insert(previous.id.clone(), previous.clone());
    let state = state_from_config(config);
    McpService::import_from_opencode(&state).expect("claim the disabled OpenCode server");

    McpService::upsert_server(
        &state,
        McpServer {
            server: json!({"command":"new"}),
            ..previous
        },
    )
    .expect("update disabled server");

    let live: serde_json::Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(live["mcp"]["disabled"]["command"], json!(["new"]));
    assert_eq!(live["mcp"]["disabled"]["enabled"], false);
    assert_eq!(live["mcp"]["disabled"]["timeout"], 30);
}

#[test]
fn upsert_new_server_does_not_touch_disabled_apps() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".claude")).expect("create Claude dir");
    let opencode_dir = home.join(".config").join("opencode");
    fs::create_dir_all(&opencode_dir).expect("create OpenCode dir");
    let opencode_path = opencode_dir.join("opencode.json");
    fs::write(&opencode_path, "{").expect("seed invalid unrelated OpenCode config");
    let state = state_from_config(MultiAppConfig::default());

    McpService::upsert_server(
        &state,
        McpServer {
            id: "claude-only".to_owned(),
            name: "Claude only".to_owned(),
            server: json!({"command":"npx"}),
            apps: McpApps {
                claude: true,
                ..McpApps::default()
            },
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        },
    )
    .expect("unrelated disabled app must not block a new server");

    assert_eq!(fs::read_to_string(opencode_path).unwrap(), "{");
    let claude: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(get_claude_mcp_path()).expect("read Claude config"),
    )
    .expect("parse Claude config");
    assert_eq!(claude["mcpServers"]["claude-only"]["command"], "npx");
}

#[test]
fn set_apps_replaces_matrix_and_syncs_opencode_live_config() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let opencode_dir = home.join(".config").join("opencode");
    fs::create_dir_all(&opencode_dir).expect("create opencode dir");
    let opencode_path = opencode_dir.join("opencode.json");
    fs::write(
        &opencode_path,
        json!({
            "theme": "system",
            "mcp": {
                "matrix-server": {
                    "type": "remote",
                    "url": "https://example.com/mcp",
                    "enabled": false,
                    "timeout": 30
                }
            }
        })
        .to_string(),
    )
    .expect("seed opencode config");

    let mut config = MultiAppConfig::default();
    config.mcp.servers = Some(HashMap::new());
    config.mcp.servers.as_mut().unwrap().insert(
        "matrix-server".into(),
        McpServer {
            id: "matrix-server".to_string(),
            name: "Matrix Server".to_string(),
            server: json!({
                "type": "http",
                "url": "https://example.com/mcp"
            }),
            apps: McpApps::default(),
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        },
    );

    let state = state_from_config(config);
    McpService::import_from_opencode(&state).expect("claim the existing OpenCode server");
    McpService::set_apps(&state, "matrix-server", McpApps::default())
        .expect("clear imported app state");

    let apps = McpApps {
        opencode: true,
        ..Default::default()
    };
    assert!(
        McpService::set_apps(&state, "matrix-server", apps).expect("set apps succeeds"),
        "existing server should be updated"
    );

    {
        let guard = state.config.read().expect("lock config");
        let server = guard
            .mcp
            .servers
            .as_ref()
            .expect("unified servers")
            .get("matrix-server")
            .expect("matrix server exists");
        assert!(
            server.apps.opencode,
            "OpenCode matrix bit should be enabled"
        );
        assert!(
            !server.apps.claude && !server.apps.codex && !server.apps.gemini && !server.apps.hermes,
            "set_apps should replace the full supported-app matrix"
        );
    }

    let opencode_text = fs::read_to_string(&opencode_path).expect("read opencode config");
    let opencode_json: serde_json::Value =
        serde_json::from_str(&opencode_text).expect("parse opencode config");
    assert!(
        opencode_json
            .get("mcp")
            .and_then(|mcp| mcp.as_object())
            .is_some_and(|mcp| mcp.contains_key("matrix-server")),
        "enabling OpenCode should write the live MCP config, got: {opencode_text}"
    );
    let projected_server = &opencode_json["mcp"]["matrix-server"];
    assert_eq!(projected_server["type"], "remote");
    assert_eq!(projected_server["enabled"], true);
    assert_eq!(projected_server["timeout"], 30);
    assert_eq!(opencode_json["theme"], "system");

    assert!(
        McpService::set_apps(&state, "matrix-server", McpApps::default())
            .expect("clear apps succeeds"),
        "existing server should be updated"
    );

    let opencode_text = fs::read_to_string(&opencode_path).expect("read opencode config");
    let opencode_json: serde_json::Value =
        serde_json::from_str(&opencode_text).expect("parse opencode config");
    assert_eq!(opencode_json["mcp"]["matrix-server"]["enabled"], false);
    assert_eq!(opencode_json["mcp"]["matrix-server"]["timeout"], 30);
    assert_eq!(opencode_json["theme"], "system");

    assert!(McpService::delete_server(&state, "matrix-server").expect("delete server"));
    let opencode_text = fs::read_to_string(&opencode_path).expect("read deleted config");
    let opencode_json: serde_json::Value =
        serde_json::from_str(&opencode_text).expect("parse deleted config");
    assert!(
        opencode_json
            .get("mcp")
            .and_then(|mcp| mcp.as_object())
            .is_none_or(|mcp| !mcp.contains_key("matrix-server")),
        "deleting the shared server should remove the native entry, got: {opencode_text}"
    );
}

#[test]
fn delete_server_preserves_an_unowned_gemini_entry_when_disabled() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let gemini_dir = home.join(".gemini");
    fs::create_dir_all(&gemini_dir).expect("create Gemini dir");
    let settings_path = gemini_dir.join("settings.json");
    fs::write(
        &settings_path,
        json!({
            "theme": "system",
            "mcpServers": {
                "stale-server": {
                    "command": "old"
                }
            }
        })
        .to_string(),
    )
    .expect("seed Gemini settings");

    let mut config = MultiAppConfig::default();
    config.mcp.servers.as_mut().unwrap().insert(
        "stale-server".into(),
        McpServer {
            id: "stale-server".into(),
            name: "Stale server".into(),
            server: json!({"type": "stdio", "command": "new"}),
            apps: McpApps::default(),
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        },
    );
    let state = state_from_config(config);

    assert!(McpService::delete_server(&state, "stale-server").expect("delete server"));

    let settings: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings_path).expect("read Gemini settings"))
            .expect("parse Gemini settings");
    assert_eq!(settings["mcpServers"]["stale-server"]["command"], "old");
    assert_eq!(settings["theme"], "system");
}

#[test]
fn failed_delete_keeps_the_shared_record_retryable() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".claude")).expect("create Claude dir");
    fs::write(get_claude_mcp_path(), "{}").expect("initialize Claude config");

    let gemini_dir = home.join(".gemini");
    fs::create_dir_all(&gemini_dir).expect("create Gemini dir");
    let gemini_path = gemini_dir.join("settings.json");
    fs::write(&gemini_path, "{}").expect("initialize Gemini config");
    let original_gemini =
        json!({"mcpServers":{"retryable":{"command":"old","timeout":30,"future":"keep"}}})
            .to_string();
    let state = state_from_config(MultiAppConfig::default());
    McpService::upsert_server(
        &state,
        McpServer {
            id: "retryable".into(),
            name: "Retryable".into(),
            server: json!({"type":"stdio","command":"new"}),
            apps: McpApps {
                claude: true,
                gemini: true,
                ..McpApps::default()
            },
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        },
    )
    .expect("create managed server");
    fs::write(get_claude_mcp_path(), "{\"mcpServers\":").expect("seed invalid Claude config");
    fs::write(&gemini_path, &original_gemini).expect("seed Gemini config");

    McpService::delete_server(&state, "retryable")
        .expect_err("invalid Claude config should stop the delete");
    assert!(
        state
            .config
            .read()
            .unwrap()
            .mcp
            .servers
            .as_ref()
            .unwrap()
            .contains_key("retryable"),
        "a failed delete must remain retryable in memory"
    );
    assert!(
        state
            .db
            .get_all_mcp_servers()
            .unwrap()
            .contains_key("retryable"),
        "a failed delete must remain retryable in the shared database"
    );
    assert_eq!(
        fs::read_to_string(&gemini_path).expect("read rolled-back Gemini config"),
        original_gemini,
        "a failed multi-app delete must restore the exact native bytes"
    );

    fs::write(get_claude_mcp_path(), "{}").expect("repair Claude config");
    assert!(McpService::delete_server(&state, "retryable").expect("retry delete"));
    let gemini: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(gemini_path).unwrap()).unwrap();
    assert!(gemini["mcpServers"].get("retryable").is_none());
}

#[test]
fn failed_toggle_does_not_commit_the_enabled_state() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let opencode_dir = home.join(".config").join("opencode");
    fs::create_dir_all(&opencode_dir).expect("create OpenCode dir");
    fs::write(opencode_dir.join("opencode.json"), "{}").expect("seed OpenCode config");

    let mut config = MultiAppConfig::default();
    config.mcp.servers.as_mut().unwrap().insert(
        "unsupported-cwd".into(),
        McpServer {
            id: "unsupported-cwd".into(),
            name: "Unsupported cwd".into(),
            server: json!({"type":"stdio","command":"echo","cwd":"/tmp"}),
            apps: McpApps::default(),
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        },
    );
    let state = state_from_config(config);

    McpService::toggle_app(&state, "unsupported-cwd", AppType::OpenCode, true)
        .expect_err("OpenCode must reject cwd");
    let memory = state.config.read().unwrap();
    assert!(
        !memory.mcp.servers.as_ref().unwrap()["unsupported-cwd"]
            .apps
            .opencode
    );
    drop(memory);
    assert!(
        !state.db.get_all_mcp_servers().unwrap()["unsupported-cwd"]
            .apps
            .opencode
    );
}
