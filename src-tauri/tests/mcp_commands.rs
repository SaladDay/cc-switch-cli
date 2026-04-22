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
