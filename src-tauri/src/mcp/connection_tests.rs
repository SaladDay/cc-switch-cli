use super::*;
use crate::app_config::{McpApps, McpServer};
use crate::test_support::TestEnvGuard;
use serde_json::Map;

fn fixtures() -> Vec<Value> {
    let types = [
        None,
        Some(json!(null)),
        Some(json!(false)),
        Some(json!(42)),
        Some(json!([])),
        Some(json!({})),
        Some(json!("")),
        Some(json!("stdio")),
        Some(json!("http")),
        Some(json!("sse")),
        Some(json!("remote")),
        Some(json!("HTTP")),
    ];
    let fields = [
        None,
        Some(json!(null)),
        Some(json!(false)),
        Some(json!(42)),
        Some(json!([])),
        Some(json!({})),
        Some(json!("")),
        Some(json!(" \t\n\u{2003}")),
        Some(json!(" value ")),
        Some(json!("\u{200b}")),
    ];
    let mut fixtures = vec![
        Value::Null,
        json!(false),
        json!(42),
        json!("node"),
        json!([]),
    ];
    for transport in &types {
        for command in &fields {
            for url in &fields {
                let mut input = json!({"args":[42], "env":{"opaque":false}, "headers":false,
                    "cwd":null, "timeout":-1, "enabled":false, "http_headers":{"native":true},
                    "httpUrl":"not decoded", "extension":{"keep":[null,42]}});
                for (key, field) in [("type", transport), ("command", command), ("url", url)] {
                    if let Some(value) = field {
                        input[key] = value.clone();
                    }
                }
                fixtures.push(input);
            }
        }
    }
    fixtures
}

fn validation_result(result: Result<(), AppError>) -> Result<(), String> {
    result.map_err(|error| match error {
        AppError::McpValidation(message) => message,
        other => panic!("unexpected error variant: {other:?}"),
    })
}

#[test]
fn validation_matches_the_previous_result_and_localized_error_for_every_fixture() {
    let mut inputs = fixtures();
    inputs.push(
        json!({"command":"node", "opaque":"x".repeat(cc_switch_core::MAX_OPERATION_CONTENT_BYTES)}),
    );
    assert_eq!(inputs.len(), 1206);
    for input in inputs {
        let original = serde_json::to_string(&input).unwrap();
        assert_eq!(
            validation_result(validate_server_spec(&input)),
            validation_result(previous_validate_server_spec(&input)),
            "{input}"
        );
        assert_eq!(serde_json::to_string(&input).unwrap(), original);
    }
}

#[test]
fn claude_import_keeps_tolerant_selection_and_existing_catalog_data() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    let path = crate::config::get_claude_mcp_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut entries: Map<String, Value> = fixtures()
        .into_iter()
        .enumerate()
        .map(|(index, value)| (format!("server-{index}"), value))
        .collect();
    entries.insert("existing".into(), json!({"command":"native"}));
    entries.insert(
        " spaced id ".into(),
        json!({"command":"node",
        "opaque":"x".repeat(cc_switch_core::MAX_OPERATION_CONTENT_BYTES)}),
    );
    let accepted: HashMap<_, _> = entries
        .iter()
        .filter(|(_, value)| previous_validate_server_spec(value).is_ok())
        .map(|(id, value)| (id.clone(), value.clone()))
        .collect();
    let text = serde_json::to_string(&json!({"theme":"keep", "mcpServers":entries})).unwrap();
    std::fs::write(&path, &text).unwrap();
    let mut existing = McpServer {
        id: "existing".into(),
        name: "catalog name".into(),
        server: json!({"command":"catalog"}),
        apps: McpApps {
            codex: true,
            ..McpApps::default()
        },
        description: Some("keep".into()),
        homepage: Some("keep".into()),
        docs: Some("keep".into()),
        tags: vec!["keep".into()],
    };
    let mut config = MultiAppConfig::default();
    config.mcp.servers = Some(HashMap::from([("existing".into(), existing.clone())]));
    assert_eq!(import_from_claude(&mut config).unwrap(), accepted.len());
    existing.apps.claude = true;
    let servers = config.mcp.servers.as_ref().unwrap();
    assert_eq!(servers.len(), accepted.len());
    assert_eq!(
        serde_json::to_value(&servers["existing"]).unwrap(),
        serde_json::to_value(existing).unwrap()
    );
    for (id, input) in accepted.iter().filter(|(id, _)| id.as_str() != "existing") {
        let imported = &servers[id];
        assert_eq!(
            serde_json::to_string(&imported.server).unwrap(),
            serde_json::to_string(input).unwrap(),
            "{id}"
        );
        assert_eq!(imported.name, *id);
        assert!(imported.apps.claude);
        assert!(!imported.apps.codex);
        assert!(!imported.apps.gemini);
        assert!(!imported.apps.opencode);
        assert!(!imported.apps.hermes);
    }
    assert_eq!(import_from_claude(&mut config).unwrap(), 0);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}

// Kept only as a compatibility oracle for the replaced production validator.
fn previous_validate_server_spec(spec: &Value) -> Result<(), AppError> {
    if !spec.is_object() {
        return Err(AppError::McpValidation(
            "MCP 服务器连接定义必须为 JSON 对象".into(),
        ));
    }
    if let Some(type_val) = spec.get("type") {
        if !type_val.is_string() {
            return Err(AppError::McpValidation(
                "MCP 服务器 type 必须是字符串".into(),
            ));
        }
    }
    let t_opt = spec.get("type").and_then(|x| x.as_str());
    // 支持三种：stdio/http/sse；若缺省 type 则按 stdio 处理（与社区常见 .mcp.json 一致）
    let is_stdio = t_opt.map(|t| t == "stdio").unwrap_or(true);
    let is_http = t_opt.map(|t| t == "http").unwrap_or(false);
    let is_sse = t_opt.map(|t| t == "sse").unwrap_or(false);

    if !(is_stdio || is_http || is_sse) {
        return Err(AppError::McpValidation(
            "MCP 服务器 type 必须是 'stdio'、'http' 或 'sse'（或省略表示 stdio）".into(),
        ));
    }

    if is_stdio {
        let cmd = spec.get("command").and_then(|x| x.as_str()).unwrap_or("");
        if cmd.trim().is_empty() {
            return Err(AppError::McpValidation(
                "stdio 类型的 MCP 服务器缺少 command 字段".into(),
            ));
        }
    }
    if is_http {
        let url = spec.get("url").and_then(|x| x.as_str()).unwrap_or("");
        if url.trim().is_empty() {
            return Err(AppError::McpValidation(
                "http 类型的 MCP 服务器缺少 url 字段".into(),
            ));
        }
    }
    if is_sse {
        let url = spec.get("url").and_then(|x| x.as_str()).unwrap_or("");
        if url.trim().is_empty() {
            return Err(AppError::McpValidation(
                "sse 类型的 MCP 服务器缺少 url 字段".into(),
            ));
        }
    }
    Ok(())
}
