use super::*;
use crate::test_support::TestEnvGuard;
use std::path::Path;

fn documents() -> Vec<Option<String>> {
    [
        None,
        Some(""),
        Some("# comment\n"),
        Some("\u{a0}"),
        Some("invalid = ["),
        Some("mcp_servers = 42\n"),
        Some("mcp_servers = []\n"),
        Some("mcp_servers = {   }\n"),
        Some("mcp_servers = {\t}\n"),
        Some("mcp = 42\n"),
        Some("mcp = { servers = 42, keep = true }\n"),
        Some("mcp_servers = { echo = { command = 'old' }, sibling = { url = 'https://keep.invalid' } }\nmcp = { servers = { echo = { command = 'legacy' }, sibling = { command = 'keep' } }, keep = true }\n"),
        Some("# root\nmodel = 'keep' # unchanged\n[mcp_servers.echo]\ncommand = 'old'\n[mcp_servers.sibling]\nurl = 'https://keep.invalid'\n[model_providers.private]\napi_key = 'synthetic-secret'\n[mcp]\nkeep = true\n[mcp.servers]\n[mcp.servers.echo]\ncommand = 'legacy'\n[mcp.servers.sibling]\ncommand = 'keep'\n"),
        Some("[mcp_servers]\n[mcp.servers]\n"),
    ]
    .into_iter()
    .map(|text| text.map(str::to_owned))
    .collect()
}

fn specs() -> Vec<Value> {
    vec![
        json!({"command":"node", "args":["-y","test"], "env":{"KEY":"synthetic"}}),
        json!({"type":"http", "url":"https://test.invalid/mcp", "headers":{"X-Test":"value"},
            "enabled":false, "startup_timeout_sec":30, "custom":["keep",1,true]}),
        json!({"type":"sse", "url":"https://test.invalid/sse", "cwd":"/synthetic",
            "description":"metadata", "future": {"keep":true}, "timeout":42}),
        json!({"command":"echo", "tiny":1e-200, "large":1e200, "negative":-0.0,
            "max":i64::MAX, "unsigned":u64::MAX, "mixed":[null, {}, [], 1.25, "x"]}),
        json!({"command":"echo", "newline":"first\nsecond", "":true, "a.b":42}),
        json!({"command":false, "args":[1,false], "env":{"KEY":false}}),
        json!({"type":"future", "command":"echo"}),
        json!({}),
        json!([]),
        Value::Null,
    ]
}

fn seed(path: &Path, source: Option<&str>) {
    match source {
        Some(text) => std::fs::write(path, text).unwrap(),
        None if path.exists() => std::fs::remove_file(path).unwrap(),
        None => {}
    }
}

fn outcome(result: Result<(), AppError>, path: &Path) -> (Result<(), String>, Option<Vec<u8>>) {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("unexpected read error: {error}"),
    };
    (result.map_err(|error| error.to_string()), bytes)
}

#[test]
fn single_server_and_removal_match_previous_file_bytes_and_errors() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    let path = crate::codex_config::get_codex_config_path();
    assert!(path.starts_with(temp.path()));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let auth = path.with_file_name("auth.json");
    std::fs::write(&auth, b"synthetic-auth-sentinel").unwrap();
    let config = MultiAppConfig::default();
    for source in documents() {
        for id in ["echo", "spaced id", "a.b\"[]"] {
            for spec in specs() {
                seed(&path, source.as_deref());
                let expected = outcome(
                    previous_sync_single_server_to_codex(&config, id, &spec),
                    &path,
                );
                seed(&path, source.as_deref());
                let actual = outcome(sync_single_server_to_codex(&config, id, &spec), &path);
                assert_eq!(actual, expected, "source={source:?}, id={id}, spec={spec}");
                assert_eq!(std::fs::read(&auth).unwrap(), b"synthetic-auth-sentinel");
            }
            seed(&path, source.as_deref());
            let expected = outcome(previous_remove_server_from_codex(id), &path);
            seed(&path, source.as_deref());
            let actual = outcome(remove_server_from_codex(id), &path);
            assert_eq!(actual, expected, "source={source:?}, id={id}");
        }
    }
}

#[test]
fn bulk_sync_matches_previous_selection_order_file_bytes_and_errors() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    let path = crate::codex_config::get_codex_config_path();
    assert!(path.starts_with(temp.path()));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let auth = path.with_file_name("auth.json");
    std::fs::write(&auth, b"synthetic-auth-sentinel").unwrap();
    let mut mixed = MultiAppConfig::default();
    for (index, spec) in specs().into_iter().enumerate() {
        mixed.mcp.codex.servers.insert(
            format!("entry.{index}"),
            json!({"enabled":true,"server":spec}),
        );
    }
    mixed.mcp.codex.servers.insert(
        "disabled".into(),
        json!({"enabled":false,"server":{"command":"skip"}}),
    );
    let mut invalid = MultiAppConfig::default();
    invalid.mcp.codex.servers.insert(
        "invalid".into(),
        json!({"enabled":true,"server":{"type":"future"}}),
    );
    for config in [MultiAppConfig::default(), mixed, invalid] {
        for source in documents() {
            seed(&path, source.as_deref());
            let expected = outcome(previous_sync_enabled_to_codex(&config), &path);
            seed(&path, source.as_deref());
            let actual = outcome(sync_enabled_to_codex(&config), &path);
            assert_eq!(actual, expected, "source={source:?}");
            assert_eq!(std::fs::read(&auth).unwrap(), b"synthetic-auth-sentinel");
        }
    }
}

#[test]
fn uninitialized_apps_and_large_native_files_keep_host_policy() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    let path = crate::codex_config::get_codex_config_path();
    assert!(path.starts_with(temp.path()));
    let config = MultiAppConfig::default();
    let spec = json!({"command":"echo"});
    assert!(!path.parent().unwrap().exists());
    previous_sync_single_server_to_codex(&config, "echo", &spec).unwrap();
    sync_single_server_to_codex(&config, "echo", &spec).unwrap();
    previous_sync_enabled_to_codex(&config).unwrap();
    sync_enabled_to_codex(&config).unwrap();
    previous_remove_server_from_codex("echo").unwrap();
    remove_server_from_codex("echo").unwrap();
    assert!(!path.parent().unwrap().exists());

    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let source = format!(
        "# {}\nmodel = 'keep'\n",
        "x".repeat(cc_switch_core::MAX_OPERATION_CONTENT_BYTES)
    );
    seed(&path, Some(&source));
    let expected = outcome(
        previous_sync_single_server_to_codex(&config, "echo", &spec),
        &path,
    );
    assert!(expected.0.is_ok());
    seed(&path, Some(&source));
    assert_eq!(
        outcome(sync_single_server_to_codex(&config, "echo", &spec), &path),
        expected
    );
}

// Test-only baseline from 0c80c5bc. Host I/O and field selection stay unchanged;
// these retain the previous native representations and document operations.

fn previous_sync_enabled_to_codex(config: &MultiAppConfig) -> Result<(), AppError> {
    if !crate::sync_policy::should_sync_live(&AppType::Codex) {
        return Ok(());
    }
    use toml_edit::{Item, Table};

    let enabled = collect_enabled_servers(&config.mcp.codex);

    let base_text = crate::codex_config::read_and_validate_codex_config_text()?;

    let mut doc = if base_text.trim().is_empty() {
        toml_edit::DocumentMut::default()
    } else {
        base_text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| AppError::McpValidation(format!("解析 config.toml 失败: {e}")))?
    };

    if let Some(mcp_item) = doc.get_mut("mcp") {
        if let Some(tbl) = mcp_item.as_table_like_mut() {
            if tbl.contains_key("servers") {
                log::warn!("检测到错误的 MCP 格式 [mcp.servers]，正在清理并迁移到 [mcp_servers]");
                tbl.remove("servers");
            }
        }
    }

    if enabled.is_empty() {
        doc.as_table_mut().remove("mcp_servers");
    } else {
        let mut servers_tbl = Table::new();
        let mut ids: Vec<_> = enabled.keys().cloned().collect();
        ids.sort();
        for id in ids {
            let spec = enabled.get(&id).expect("spec must exist");
            match previous_json_server_to_toml_table(spec) {
                Ok(table) => {
                    servers_tbl[&id[..]] = Item::Table(table);
                }
                Err(err) => {
                    log::error!("跳过无效的 MCP 服务器 '{id}': {err}");
                }
            }
        }
        doc["mcp_servers"] = Item::Table(servers_tbl);
    }

    let new_text = doc.to_string();
    let path = crate::codex_config::get_codex_config_path();
    crate::config::write_text_file(&path, &new_text)?;
    Ok(())
}

fn previous_sync_single_server_to_codex(
    _config: &MultiAppConfig,
    id: &str,
    server_spec: &Value,
) -> Result<(), AppError> {
    if !crate::sync_policy::should_sync_live(&AppType::Codex) {
        return Ok(());
    }
    let config_path = crate::codex_config::get_codex_config_path();

    let mut doc = if config_path.exists() {
        let content =
            std::fs::read_to_string(&config_path).map_err(|e| AppError::io(&config_path, e))?;
        content
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| AppError::McpValidation(format!("解析 config.toml 失败: {e}")))?
    } else {
        toml_edit::DocumentMut::new()
    };

    if let Some(mcp_item) = doc.get_mut("mcp") {
        if let Some(tbl) = mcp_item.as_table_like_mut() {
            if tbl.contains_key("servers") {
                log::warn!("检测到错误的 MCP 格式 [mcp.servers]，正在清理并迁移到 [mcp_servers]");
                tbl.remove("servers");
            }
        }
    }

    let toml_table = previous_json_server_to_toml_table(server_spec)?;
    previous_upsert_mcp_server_table(&mut doc, id, toml_table)?;

    let new_text = doc.to_string();
    crate::config::write_text_file(&config_path, &new_text)?;

    Ok(())
}

fn previous_remove_server_from_codex(id: &str) -> Result<(), AppError> {
    if !crate::sync_policy::should_sync_live(&AppType::Codex) {
        return Ok(());
    }
    let config_path = crate::codex_config::get_codex_config_path();

    if !config_path.exists() {
        return Ok(()); // 文件不存在，无需删除
    }

    let content =
        std::fs::read_to_string(&config_path).map_err(|e| AppError::io(&config_path, e))?;

    let mut doc = match content.parse::<toml_edit::DocumentMut>() {
        Ok(doc) => doc,
        Err(e) => {
            log::warn!("解析 Codex config.toml 失败: {e}，跳过删除操作");
            return Ok(());
        }
    };

    previous_remove_mcp_server_from_doc(&mut doc, id);

    let new_text = doc.to_string();
    crate::config::write_text_file(&config_path, &new_text)?;

    Ok(())
}

fn previous_upsert_mcp_server_table(
    doc: &mut toml_edit::DocumentMut,
    id: &str,
    table: toml_edit::Table,
) -> Result<(), AppError> {
    if doc
        .get_mut("mcp_servers")
        .and_then(toml_edit::Item::as_table_like_mut)
        .is_none()
    {
        if doc.get("mcp_servers").is_some_and(|item| !item.is_none()) {
            log::warn!("config.toml 的 mcp_servers 不是表，已重置为空表");
        }
        doc["mcp_servers"] = toml_edit::table();
    }

    let servers = doc
        .get_mut("mcp_servers")
        .and_then(toml_edit::Item::as_table_like_mut)
        .ok_or_else(|| AppError::McpValidation("config.toml 的 mcp_servers 不是表".to_string()))?;
    servers.insert(id, toml_edit::Item::Table(table));
    Ok(())
}

fn previous_remove_mcp_server_from_doc(doc: &mut toml_edit::DocumentMut, id: &str) {
    if let Some(item) = doc.get_mut("mcp_servers") {
        let user_authored = !item.is_none();
        match item.as_table_like_mut() {
            Some(mcp_servers) => {
                mcp_servers.remove(id);
            }
            None if user_authored => {
                log::warn!("config.toml 的 mcp_servers 不是表，无法删除服务器 '{id}'");
            }
            None => {}
        }
    }

    if let Some(mcp_table) = doc.get_mut("mcp").and_then(|item| item.as_table_like_mut()) {
        if let Some(servers) = mcp_table
            .get_mut("servers")
            .and_then(|item| item.as_table_like_mut())
        {
            if servers.remove(id).is_some() {
                log::warn!("从错误的 MCP 格式 [mcp.servers] 中清理了服务器 '{id}'");
            }
        }
    }
}

fn previous_json_server_to_toml_table(spec: &Value) -> Result<toml_edit::Table, AppError> {
    let typ = spec.get("type").and_then(|v| v.as_str()).unwrap_or("stdio");

    // 定义核心字段（已在下方处理，跳过通用转换）
    let core_fields: &[&str] = match typ {
        "stdio" => &["type", "command", "args", "env", "cwd"],
        "http" | "sse" => &["type", "url", "headers", "http_headers"],
        _ => &["type"],
    };

    // 定义扩展字段白名单（Codex 常见可选字段）
    let extended_fields = [
        // 通用字段
        "timeout",
        "timeout_ms",
        "startup_timeout_ms",
        "startup_timeout_sec",
        "connection_timeout",
        "read_timeout",
        "debug",
        "log_level",
        "disabled",
        // stdio 特有
        "shell",
        "encoding",
        "working_dir",
        "restart_on_exit",
        "max_restart_count",
        // http/sse 特有
        "retry_count",
        "max_retry_attempts",
        "retry_delay",
        "cache_tools_list",
        "verify_ssl",
        "insecure",
        "proxy",
    ];

    // Preserve the CLI's tolerant field filtering before native encoding.
    let mut connection = serde_json::Map::new();
    connection.insert("type".into(), json!(typ));
    for &field in core_fields {
        let value = match field {
            "command" | "url" => Some(json!(spec.get(field).and_then(Value::as_str).unwrap_or(""))),
            "cwd" => spec
                .get(field)
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(|value| json!(value)),
            "args" => spec
                .get(field)
                .and_then(Value::as_array)
                .map(|values| {
                    Value::Array(
                        values
                            .iter()
                            .filter(|value| value.is_string())
                            .cloned()
                            .collect(),
                    )
                })
                .filter(|value| value.as_array().is_some_and(|values| !values.is_empty())),
            "env" | "headers" => spec
                .get(field)
                .and_then(Value::as_object)
                .map(|values| {
                    Value::Object(
                        values
                            .iter()
                            .filter(|(_, value)| value.is_string())
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect(),
                    )
                })
                .filter(|value| value.as_object().is_some_and(|values| !values.is_empty())),
            _ => None,
        };
        if let Some(value) = value {
            connection.insert(field.into(), value);
        }
    }
    let native = cc_switch_core::McpConfigTarget::Codex
        .encode_server(&Value::Object(connection))
        .map_err(|error| AppError::McpValidation(error.to_string()))?;
    let text =
        toml::to_string(&native).map_err(|error| AppError::McpValidation(error.to_string()))?;
    let mut t = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| AppError::McpValidation(error.to_string()))?
        .into_table();

    // 2. 处理扩展字段和其他未知字段
    if let Some(obj) = spec.as_object() {
        for (key, value) in obj {
            // 跳过已处理的核心字段
            if core_fields.contains(&key.as_str()) {
                continue;
            }

            // 尝试使用通用转换器
            if let Some(toml_item) = json_value_to_toml_item(value, key) {
                t[&key[..]] = toml_item;

                // 未知扩展字段同样可能携带 token / secret，只记录字段名。
                if extended_fields.contains(&key.as_str()) {
                    log::debug!("已转换扩展字段 '{key}'（值已省略）");
                } else {
                    log::debug!("已转换自定义字段 '{key}'（值已省略）");
                }
            }
        }
    }

    Ok(t)
}
