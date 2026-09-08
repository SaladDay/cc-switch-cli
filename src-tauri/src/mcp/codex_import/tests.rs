use super::server_spec;
use crate::{
    app_config::{AppType, McpApps, McpServer, MultiAppConfig},
    mcp::{import_from_codex, validate_server_spec},
    test_support::TestEnvGuard,
};
use serde_json::{json, Value};
use std::{collections::HashMap, fs};

fn assert_spec_matches_previous(text: &str) {
    let entry: toml::Table = toml::from_str(text).expect("fixture TOML");
    let expected = previous_server_spec(&entry);
    let actual = server_spec(&entry).expect("Core transport codec");
    assert_eq!(actual, expected, "{text}");
    assert_eq!(
        serde_json::to_string(&actual).unwrap(),
        serde_json::to_string(&expected).unwrap(),
        "field order: {text}"
    );
    assert_eq!(
        validate_server_spec(&actual).map_err(|error| error.to_string()),
        validate_server_spec(&expected).map_err(|error| error.to_string()),
        "{text}"
    );
}

#[test]
fn transport_import_matches_previous_toml_shapes_and_extension_selection() {
    let types = [
        "",
        "type = \"stdio\"\n",
        "type = \"http\"\n",
        "type = \"sse\"\n",
        "type = \"future\"\n",
        "type = false\n",
        "type = 42\n",
        "type = []\n",
        "type = [\"http\", 42, [true]]\n",
        "type = {}\n",
        "type = {x = true}\n",
        "type = {x = \"string\"}\n",
        "type = 1979-05-27T07:32:00Z\n",
        "type = nan\n",
    ];
    let urls = [
        "",
        "url = \"https://example.com/mcp\"\n",
        "url = \"\"\n",
        "url = \" \"\n",
        "url = false\n",
        "url = []\n",
        "url = {x = \"string\"}\n",
        "url = 1979-05-27\n",
    ];
    let headers = [
        "",
        "headers = {KEEP = \"legacy\", DROP = 42}\n",
        "headers = {KEEP = \"legacy\"}\nhttp_headers = {}\n",
        "headers = {KEEP = \"legacy\"}\nhttp_headers = {DROP = false}\n",
        "headers = {KEEP = \"legacy\"}\nhttp_headers = false\n",
        "headers = {KEEP = \"legacy\"}\nhttp_headers = {KEEP = \"native\", DROP = false}\n",
        "http_headers = 1979-05-27\n",
    ];
    for typ in types {
        for url in urls {
            for header in headers {
                assert_spec_matches_previous(&format!(
                    "{typ}{url}{header}{}",
                    r#"
command = "node"
args = ["server", 42, true, [1], {nested = "value"}, 1979-05-27]
cwd = " /work "
env = {KEEP = "value", DROP = false, DATE = 1979-05-27}
enabled = false
description = "host extension"
scalar = 3.5
nonfinite = nan
empty = []
nested = [[1], {drop = "table"}, "keep", 42, true, 1979-05-27]
object = {KEEP = "value", DROP = 42, NESTED = {drop = "yes"}}
date = 1979-05-27T07:32:00Z
"#,
                ));
            }
        }
    }
    for text in [
        "",
        "command = false\nargs = [42]\nenv = {}\ncwd = \"  \"\n",
        "command = 1979-05-27\nargs = []\nenv = false\ncwd = 42\n",
        "command = \"node\"\nargs = [nan, inf, -inf]\nenv = {x = nan}\n",
    ] {
        assert_spec_matches_previous(text);
    }
}

#[test]
fn document_import_preserves_legacy_priority_catalog_metadata_and_enablement() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    let path = crate::codex_config::get_codex_config_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text = r#"
[mcp.servers.duplicate]
command = "legacy-wins"
[mcp_servers.duplicate]
command = "official-does-not-overwrite"
[mcp_servers.existing]
command = "must-not-overwrite-catalog"
[mcp_servers.remote]
url = "https://example.com/mcp"
enabled = false
http_headers = {KEEP = "native", DROP = 42}
headers = {KEEP = "legacy"}
[mcp_servers.invalid]
type = false
command = "invalid-is-skipped"
[mcp_servers.opaque-type]
type = 1979-05-27
command = "historically-accepted"
"#;
    fs::write(&path, text).unwrap();
    let mut config = MultiAppConfig::default();
    let existing = McpServer {
        id: "existing".into(),
        name: "Keep my name".into(),
        server: json!({"command":"catalog", "extra":"keep"}),
        apps: McpApps {
            claude: true,
            ..McpApps::default()
        },
        description: Some("keep".into()),
        homepage: Some("keep".into()),
        docs: None,
        tags: vec!["keep".into()],
    };
    config.mcp.servers = Some(HashMap::from([("existing".into(), existing.clone())]));
    assert_eq!(import_from_codex(&mut config).unwrap(), 4);
    let servers = config.mcp.servers.as_ref().unwrap();
    assert_eq!(servers.len(), 4);
    assert_eq!(servers["duplicate"].server["command"], "legacy-wins");
    let mut expected_existing = existing;
    expected_existing.apps.codex = true;
    assert_eq!(
        serde_json::to_value(&servers["existing"]).unwrap(),
        serde_json::to_value(&expected_existing).unwrap()
    );
    assert_eq!(
        servers["remote"].server,
        json!({"type":"http",
        "url":"https://example.com/mcp", "headers":{"KEEP":"native"}, "enabled":false})
    );
    assert!(!servers["remote"].apps.is_enabled_for(&AppType::Codex));
    assert_eq!(
        servers["opaque-type"].server,
        json!({"command":"historically-accepted"})
    );
    assert!(!servers.contains_key("invalid"));
    assert_eq!(import_from_codex(&mut config).unwrap(), 0);
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
}

#[test]
fn document_import_keeps_existing_bounds_parser_errors_and_collection_tolerance() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    let path = crate::codex_config::get_codex_config_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    for text in [
        "",
        " ",
        "mcp_servers = false\n",
        "mcp = { servers = [42] }\n",
        "mcp_servers = { invalid = 42 }\n",
    ] {
        fs::write(&path, text).unwrap();
        let mut config = MultiAppConfig::default();
        config.mcp.servers = None;
        assert_eq!(import_from_codex(&mut config).unwrap(), 0);
        assert_eq!(config.mcp.servers.is_some(), !text.trim().is_empty());
    }
    for text in ["[broken", "key = \"\\e\"\n", "key = \"\\x41\"\n"] {
        fs::write(&path, text).unwrap();
        let expected = crate::codex_config::read_and_validate_codex_config_text()
            .unwrap_err()
            .to_string();
        let mut config = MultiAppConfig::default();
        config.mcp.servers = None;
        assert_eq!(
            import_from_codex(&mut config).unwrap_err().to_string(),
            expected
        );
        assert!(config.mcp.servers.is_none());
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
    }
    let extension = "x".repeat(cc_switch_core::MAX_OPERATION_CONTENT_BYTES + 1);
    let text = format!("[mcp_servers.large]\ncommand = \"node\"\nextension = \"{extension}\"\n");
    fs::write(&path, &text).unwrap();
    let mut config = MultiAppConfig::default();
    assert_eq!(import_from_codex(&mut config).unwrap(), 1);
    assert_eq!(
        config.mcp.servers.as_ref().unwrap()["large"].server["extension"],
        extension
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
}

// Compatibility oracle copied from the pre-migration CLI import implementation.
// It is test-only; production transport decoding must use Core.
fn previous_server_spec(entry_tbl: &toml::value::Table) -> Value {
    let toml_to_json = |toml_val: &toml::Value| -> Option<serde_json::Value> {
        match toml_val {
            toml::Value::String(s) => Some(json!(s)),
            toml::Value::Integer(i) => Some(json!(i)),
            toml::Value::Float(f) => Some(json!(f)),
            toml::Value::Boolean(b) => Some(json!(b)),
            toml::Value::Array(arr) => {
                let json_arr: Vec<serde_json::Value> = arr
                    .iter()
                    .filter_map(|item| match item {
                        toml::Value::String(s) => Some(json!(s)),
                        toml::Value::Integer(i) => Some(json!(i)),
                        toml::Value::Float(f) => Some(json!(f)),
                        toml::Value::Boolean(b) => Some(json!(b)),
                        _ => None,
                    })
                    .collect();
                if json_arr.is_empty() {
                    None
                } else {
                    Some(serde_json::Value::Array(json_arr))
                }
            }
            toml::Value::Table(tbl) => {
                let mut json_obj = serde_json::Map::new();
                for (k, v) in tbl.iter() {
                    if let Some(s) = v.as_str() {
                        json_obj.insert(k.clone(), json!(s));
                    }
                }
                if json_obj.is_empty() {
                    None
                } else {
                    Some(serde_json::Value::Object(json_obj))
                }
            }
            toml::Value::Datetime(_) => None,
        }
    };

    // Codex 的远程 MCP 可以只写 `url`，不显式提供 `type`。
    // 仅在 `type` 真正缺失时才推断为 HTTP，避免掩盖显式但非法的配置。
    let typ = if entry_tbl.contains_key("type") {
        entry_tbl.get("type").and_then(|v| v.as_str())
    } else {
        entry_tbl
            .get("url")
            .and_then(|v| v.as_str())
            .filter(|url| !url.trim().is_empty())
            .map(|_| "http")
            .or(Some("stdio"))
    };

    // 构建 JSON 规范
    let mut spec = serde_json::Map::new();
    if let Some(typ) = typ {
        spec.insert("type".into(), json!(typ));
    } else if let Some(type_val) = entry_tbl.get("type").and_then(toml_to_json) {
        spec.insert("type".into(), type_val);
    }

    // 核心字段（需要手动处理的字段）
    let core_fields = match typ {
        Some("stdio") => vec!["type", "command", "args", "env", "cwd"],
        // DB 中的统一规范使用 headers，Codex TOML 使用 http_headers。
        // 两者都必须视为核心字段，避免鉴权值落入通用日志路径。
        Some("http") | Some("sse") => vec!["type", "url", "headers", "http_headers"],
        _ => vec!["type"],
    };

    // 1. 处理核心字段（强类型）
    match typ {
        Some("stdio") => {
            if let Some(cmd) = entry_tbl.get("command").and_then(|v| v.as_str()) {
                spec.insert("command".into(), json!(cmd));
            }
            if let Some(args) = entry_tbl.get("args").and_then(|v| v.as_array()) {
                let arr = args
                    .iter()
                    .filter_map(|x| x.as_str())
                    .map(|s| json!(s))
                    .collect::<Vec<_>>();
                if !arr.is_empty() {
                    spec.insert("args".into(), serde_json::Value::Array(arr));
                }
            }
            if let Some(cwd) = entry_tbl.get("cwd").and_then(|v| v.as_str()) {
                if !cwd.trim().is_empty() {
                    spec.insert("cwd".into(), json!(cwd));
                }
            }
            if let Some(env_tbl) = entry_tbl.get("env").and_then(|v| v.as_table()) {
                let mut env_json = serde_json::Map::new();
                for (k, v) in env_tbl.iter() {
                    if let Some(sv) = v.as_str() {
                        env_json.insert(k.clone(), json!(sv));
                    }
                }
                if !env_json.is_empty() {
                    spec.insert("env".into(), serde_json::Value::Object(env_json));
                }
            }
        }
        Some("http") | Some("sse") => {
            if let Some(url) = entry_tbl.get("url").and_then(|v| v.as_str()) {
                spec.insert("url".into(), json!(url));
            }
            // Read from http_headers (correct Codex format) or headers (legacy) with priority to http_headers
            let headers_tbl = entry_tbl
                .get("http_headers")
                .and_then(|v| v.as_table())
                .or_else(|| entry_tbl.get("headers").and_then(|v| v.as_table()));

            if let Some(headers_tbl) = headers_tbl {
                let mut headers_json = serde_json::Map::new();
                for (k, v) in headers_tbl.iter() {
                    if let Some(sv) = v.as_str() {
                        headers_json.insert(k.clone(), json!(sv));
                    }
                }
                if !headers_json.is_empty() {
                    spec.insert("headers".into(), serde_json::Value::Object(headers_json));
                }
            }
        }
        _ => {}
    }

    // 2. 处理扩展字段和其他未知字段（通用 TOML → JSON 转换）
    for (key, toml_val) in entry_tbl.iter() {
        // 跳过已处理的核心字段
        if core_fields.contains(&key.as_str()) {
            continue;
        }

        // 通用 TOML 值到 JSON 值转换
        let json_val = toml_to_json(toml_val);

        if let Some(val) = json_val {
            spec.insert(key.clone(), val);
            log::debug!("导入扩展字段 '{key}'（值已省略）");
        } else {
            log::debug!("跳过复杂字段 '{key}' (TOML → JSON)");
        }
    }

    serde_json::Value::Object(spec)
}
