use super::{read_mcp_servers_map, set_mcp_servers_map};
use crate::{error::AppError, test_support::TestEnvGuard};
use serde_json::{json, Map, Value};
use std::{collections::HashMap, fs};

fn assert_same_entry(actual: &Value, expected: &Value, id: &str) {
    assert_eq!(actual, expected, "{id}");
    assert_eq!(
        serde_json::to_string(actual).unwrap(),
        serde_json::to_string(expected).unwrap(),
        "serialized field order: {id}"
    );
}

fn set_optional(entry: &mut Map<String, Value>, key: &str, value: &Option<Value>) {
    if let Some(value) = value {
        entry.insert(key.into(), value.clone());
    }
}

#[test]
fn read_document_matches_previous_inference_and_opaque_fields() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    let path = super::user_config_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let types = [
        None,
        Some(Value::Null),
        Some(json!(false)),
        Some(json!("stdio")),
        Some(json!("http")),
        Some(json!("sse")),
        Some(json!("future")),
    ];
    let values = [
        None,
        Some(Value::Null),
        Some(json!(false)),
        Some(json!("")),
        Some(json!("endpoint")),
        Some(json!({"nested":42})),
    ];
    let mut entries = Map::new();
    for typ in &types {
        for command in &values {
            for url in &values {
                for http_url in &values {
                    let mut entry = json!({"future":{"nested":[null,42,{"keep":true}]},
                        "enabled":false, "oauth":{"synthetic":"token"}})
                    .as_object()
                    .unwrap()
                    .clone();
                    for (key, value) in [
                        ("type", typ),
                        ("command", command),
                        ("url", url),
                        ("httpUrl", http_url),
                    ] {
                        set_optional(&mut entry, key, value);
                    }
                    entries.insert(format!("entry-{}", entries.len()), Value::Object(entry));
                }
            }
        }
    }
    for value in [Value::Null, json!(false), json!(42), json!([])] {
        entries.insert(format!("entry-{}", entries.len()), value);
    }
    let text = serde_json::to_string(&json!({"mcpServers":entries,"theme":"keep"})).unwrap();
    fs::write(&path, &text).unwrap();
    let actual = read_mcp_servers_map().unwrap();
    assert_eq!(actual.len(), entries.len());
    for (id, entry) in &entries {
        assert_same_entry(&actual[id], &previous_decode(entry), id);
    }
    assert_eq!(fs::read_to_string(path).unwrap(), text);
}

#[test]
fn write_document_matches_previous_timeouts_wrappers_and_field_order() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    let path = super::user_config_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        r#"{"theme":{"keep":true},"mcpServers":{"old":{"command":"old"}}}"#,
    )
    .unwrap();
    let types = [
        Value::Null,
        json!(false),
        json!("stdio"),
        json!("http"),
        json!("sse"),
        json!(""),
        json!("future"),
    ];
    let values = [
        None,
        Some(Value::Null),
        Some(json!(false)),
        Some(json!({})),
        Some(json!([])),
        Some(json!("120")),
        Some(json!(-1)),
        Some(json!(-0.5)),
        Some(json!(0.0019)),
        Some(json!(0)),
        Some(json!(42)),
        Some(json!(u64::MAX)),
        Some(json!(f64::MAX)),
    ];
    let mut servers = HashMap::new();
    for typ in &types {
        for seconds in &values {
            for milliseconds in &values {
                let mut entry = json!({"type":typ,"url":"https://example.com/mcp","httpUrl":false,
                    "command":"native-extension","env":{"raw":42},"headers":{"Authorization":"synthetic"},
                    "enabled":false,"source":"host","id":"host","name":"host","description":"host",
                    "tags":["host"],"homepage":"host","docs":"host","future":{"keep":[null,true]}})
                    .as_object().unwrap().clone();
                for (key, value) in [
                    ("startup_timeout_sec", seconds),
                    ("startup_timeout_ms", milliseconds),
                    ("tool_timeout_sec", milliseconds),
                    ("tool_timeout_ms", seconds),
                    ("timeout", milliseconds),
                ] {
                    set_optional(&mut entry, key, value);
                }
                let value = Value::Object(entry);
                let value = if servers.len() % 2 == 0 {
                    json!({"server":value,"name":"outer"})
                } else {
                    value
                };
                servers.insert(format!("entry-{}", servers.len()), value);
            }
        }
    }
    for value in [
        json!({}),
        json!({"url":null,"type":"http"}),
        json!({"type":"http","httpUrl":42}),
        json!({"server":{"server":{"unconsumed":true},"type":"http","url":false,"enabled":false}}),
    ] {
        servers.insert(format!("entry-{}", servers.len()), value);
    }
    set_mcp_servers_map(&servers).unwrap();
    let root: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(root["theme"], json!({"keep":true}));
    assert_eq!(root["mcpServers"].as_object().unwrap().len(), servers.len());
    assert!(root["mcpServers"].get("old").is_none());
    for (id, entry) in &servers {
        assert_same_entry(
            &root["mcpServers"][id],
            &previous_encode(id, entry).unwrap(),
            id,
        );
    }
    set_mcp_servers_map(&HashMap::new()).unwrap();
    let root: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(root, json!({"theme":{"keep":true},"mcpServers":{}}));
}

#[test]
fn invalid_documents_entries_and_large_extensions_keep_host_behavior() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    let path = super::user_config_path();
    assert!(read_mcp_servers_map().unwrap().is_empty());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    for text in [
        "null",
        "false",
        "[]",
        r#"{"mcpServers":false}"#,
        r#"{"mcpServers":[]}"#,
    ] {
        fs::write(&path, text).unwrap();
        assert!(read_mcp_servers_map().unwrap().is_empty());
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
    }
    for text in [
        "",
        "{broken",
        "{// JSON5 is not accepted\n}",
        r#"{"mcpServers":{}"#,
    ] {
        fs::write(&path, text).unwrap();
        let expected = super::read_json_value(&path).unwrap_err().to_string();
        assert_eq!(read_mcp_servers_map().unwrap_err().to_string(), expected);
        assert_eq!(
            set_mcp_servers_map(&HashMap::new())
                .unwrap_err()
                .to_string(),
            expected
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
    }
    for spec in [
        Value::Null,
        json!(false),
        json!([]),
        json!({"server":null}),
        json!({"server":false}),
    ] {
        let text = r#"{"unrelated":"keep","mcpServers":{"old":{"command":"old"}}}"#;
        fs::write(&path, text).unwrap();
        let expected = previous_encode("invalid", &spec).unwrap_err().to_string();
        assert_eq!(
            set_mcp_servers_map(&HashMap::from([("invalid".into(), spec)]))
                .unwrap_err()
                .to_string(),
            expected
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
    }
    fs::write(&path, "[]").unwrap();
    let expected = AppError::Config("~/.gemini/settings.json 根必须是对象".into()).to_string();
    assert_eq!(
        set_mcp_servers_map(&HashMap::new())
            .unwrap_err()
            .to_string(),
        expected
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "[]");
    let large = "x".repeat(cc_switch_core::MAX_OPERATION_CONTENT_BYTES + 1);
    let server = json!({"command":"node","extension":large});
    let text =
        serde_json::to_string(&json!({"mcpServers":{"large":server},"theme":"keep"})).unwrap();
    fs::write(&path, &text).unwrap();
    let imported = read_mcp_servers_map().unwrap();
    assert_eq!(imported["large"]["extension"], large);
    set_mcp_servers_map(&imported).unwrap();
    let output: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(output["mcpServers"]["large"]["extension"], large);
    assert_eq!(output["theme"], "keep");
}

#[test]
fn catalog_import_preserves_metadata_and_only_enables_existing_rows() {
    use crate::app_config::{McpApps, McpServer, MultiAppConfig};
    let temp = tempfile::tempdir().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    let path = super::user_config_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text = r#"{"mcpServers":{"existing":{"command":"do-not-overwrite"},
        "remote":{"httpUrl":"https://example.com/mcp","enabled":false,"future":{"keep":true}},
        "inferred":{"type":false,"command":"node"},"invalid":{"httpUrl":false},"scalar":42}}"#;
    fs::write(&path, text).unwrap();
    let existing = McpServer {
        id: "existing".into(),
        name: "keep my name".into(),
        server: json!({"command":"catalog"}),
        apps: McpApps {
            claude: true,
            ..McpApps::default()
        },
        description: Some("keep".into()),
        homepage: Some("keep".into()),
        docs: None,
        tags: vec!["keep".into()],
    };
    let mut config = MultiAppConfig::default();
    config.mcp.servers = Some(HashMap::from([("existing".into(), existing.clone())]));
    assert_eq!(crate::mcp::import_from_gemini(&mut config).unwrap(), 3);
    let servers = config.mcp.servers.as_ref().unwrap();
    let mut expected = existing;
    expected.apps.gemini = true;
    assert_eq!(
        serde_json::to_value(&servers["existing"]).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert_eq!(servers.len(), 3);
    assert_eq!(
        servers["inferred"].server,
        json!({"type":"stdio","command":"node"})
    );
    assert_eq!(servers["remote"].server["future"], json!({"keep":true}));
    assert_eq!(servers["remote"].server["enabled"], false);
    assert!(servers["remote"].apps.gemini);
    assert_eq!(crate::mcp::import_from_gemini(&mut config).unwrap(), 0);
    assert_eq!(fs::read_to_string(path).unwrap(), text);
}

// Baseline converters are test-only compatibility oracles.
fn previous_decode(raw_spec: &Value) -> Value {
    let mut spec = raw_spec.clone();

    // Reverse conversion (align upstream):
    // - httpUrl -> url + type:"http"
    // - if no type: command => "stdio", url => "sse"
    if let Some(spec_obj) = spec.as_object_mut() {
        if let Some(http_url_value) = spec_obj.remove("httpUrl") {
            spec_obj.insert("url".to_string(), http_url_value);
            spec_obj.insert("type".to_string(), Value::String("http".to_string()));
        }

        let has_type = spec_obj.get("type").and_then(|v| v.as_str()).is_some();
        if !has_type {
            if spec_obj.get("command").and_then(|v| v.as_str()).is_some() {
                spec_obj.insert("type".to_string(), Value::String("stdio".to_string()));
            } else if spec_obj.get("url").and_then(|v| v.as_str()).is_some() {
                spec_obj.insert("type".to_string(), Value::String("sse".to_string()));
            }
        }
    }
    spec
}

fn previous_encode(id: &str, spec: &Value) -> Result<Value, AppError> {
    let mut obj = if let Some(map) = spec.as_object() {
        map.clone()
    } else {
        return Err(AppError::McpValidation(format!(
            "MCP 服务器 '{id}' 不是对象"
        )));
    };

    // 提取 server 字段（如果存在）
    if let Some(server_val) = obj.remove("server") {
        let server_obj = server_val.as_object().cloned().ok_or_else(|| {
            AppError::McpValidation(format!("MCP 服务器 '{id}' server 字段不是对象"))
        })?;
        obj = server_obj;
    }

    // Gemini CLI 格式转换：
    // - Gemini 不使用 "type" 字段（从字段名推断传输类型）
    // - HTTP 使用 "httpUrl" 字段，SSE 使用 "url" 字段
    let transport_type = obj.get("type").and_then(|v| v.as_str());
    if transport_type == Some("http") {
        // HTTP streaming: 将 "url" 重命名为 "httpUrl"
        if let Some(url_value) = obj.remove("url") {
            obj.insert("httpUrl".to_string(), url_value);
        }
    }
    // SSE 保持 "url" 字段不变

    // Timeout conversion:
    // - CC-Switch/Codex/Claude may use startup_timeout_* / tool_timeout_*.
    // - Gemini CLI uses a single timeout field (ms).
    // Derive Gemini timeout by taking the maximum of:
    //   - existing `timeout` (if any)
    //   - startup timeout (default 10s)
    //   - tool timeout (default 60s)
    const DEFAULT_STARTUP_MS: u64 = 10_000;
    const DEFAULT_TOOL_MS: u64 = 60_000;

    let existing_timeout_ms = obj
        .get("timeout")
        .and_then(|val| val.as_u64().or_else(|| val.as_f64().map(|f| f as u64)));

    let extract_timeout =
        |obj: &mut Map<String, Value>, key: &str, multiplier: u64| -> Option<u64> {
            obj.remove(key).and_then(|val| {
                val.as_u64()
                    .map(|n| n.saturating_mul(multiplier))
                    .or_else(|| val.as_f64().map(|f| (f * multiplier as f64) as u64))
            })
        };

    let startup_ms = extract_timeout(&mut obj, "startup_timeout_sec", 1000)
        .or_else(|| extract_timeout(&mut obj, "startup_timeout_ms", 1))
        .unwrap_or(DEFAULT_STARTUP_MS);
    let tool_ms = extract_timeout(&mut obj, "tool_timeout_sec", 1000)
        .or_else(|| extract_timeout(&mut obj, "tool_timeout_ms", 1))
        .unwrap_or(DEFAULT_TOOL_MS);

    let derived_timeout_ms = startup_ms.max(tool_ms);
    let final_timeout_ms = existing_timeout_ms.unwrap_or(0).max(derived_timeout_ms);
    obj.insert(
        "timeout".to_string(),
        Value::Number(final_timeout_ms.into()),
    );

    // 移除 UI 辅助字段和 type 字段（Gemini 不需要）
    obj.remove("type");
    obj.remove("enabled");
    obj.remove("source");
    obj.remove("id");
    obj.remove("name");
    obj.remove("description");
    obj.remove("tags");
    obj.remove("homepage");
    obj.remove("docs");
    Ok(Value::Object(obj))
}
