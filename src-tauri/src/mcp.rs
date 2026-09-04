use serde_json::{json, Value};
use std::collections::HashMap;

use crate::app_config::{AppType, McpConfig, MultiAppConfig};
use crate::error::AppError;

fn validate_mcp_entry(entry: &Value) -> Result<(), AppError> {
    let object = entry
        .as_object()
        .ok_or_else(|| AppError::McpValidation("MCP 服务器条目必须为 JSON 对象".into()))?;
    let server = object
        .get("server")
        .ok_or_else(|| AppError::McpValidation("MCP 服务器条目缺少 server 字段".into()))?;
    cc_switch_core::validate_mcp_server("legacy", server)
        .map_err(|error| AppError::McpValidation(error.to_string()))?;
    for key in ["name", "description", "homepage", "docs"] {
        if object.get(key).is_some_and(|value| !value.is_string()) {
            return Err(AppError::McpValidation(format!(
                "MCP 服务器 {key} 必须为字符串"
            )));
        }
    }
    if object.get("tags").is_some_and(|value| {
        !value
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string))
    }) {
        return Err(AppError::McpValidation(
            "MCP 服务器 tags 必须为字符串数组".into(),
        ));
    }
    if object
        .get("enabled")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(AppError::McpValidation(
            "MCP 服务器 enabled 必须为布尔值".into(),
        ));
    }
    Ok(())
}

fn normalize_server_keys(map: &mut HashMap<String, Value>) -> usize {
    let mut change_count = 0usize;
    let mut renames: Vec<(String, String)> = Vec::new();

    for (key_ref, value) in map.iter_mut() {
        let key = key_ref.clone();
        let Some(obj) = value.as_object_mut() else {
            continue;
        };

        let id_value = obj.get("id").cloned();

        let target_id: String;

        match id_value {
            Some(id_val) => match id_val.as_str() {
                Some(id_str) => {
                    let trimmed = id_str.trim();
                    if trimmed.is_empty() {
                        obj.insert("id".into(), json!(key.clone()));
                        change_count += 1;
                        target_id = key.clone();
                    } else {
                        if trimmed != id_str {
                            obj.insert("id".into(), json!(trimmed));
                            change_count += 1;
                        }
                        target_id = trimmed.to_string();
                    }
                }
                None => {
                    obj.insert("id".into(), json!(key.clone()));
                    change_count += 1;
                    target_id = key.clone();
                }
            },
            None => {
                obj.insert("id".into(), json!(key.clone()));
                change_count += 1;
                target_id = key.clone();
            }
        }

        if target_id != key {
            renames.push((key, target_id));
        }
    }

    for (old_key, new_key) in renames {
        if old_key == new_key {
            continue;
        }
        if map.contains_key(&new_key) {
            log::warn!("MCP 条目 '{old_key}' 的内部 id '{new_key}' 与现有键冲突，回退为原键");
            if let Some(value) = map.get_mut(&old_key) {
                if let Some(obj) = value.as_object_mut() {
                    if obj
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(|s| s != old_key)
                        .unwrap_or(true)
                    {
                        obj.insert("id".into(), json!(old_key.clone()));
                        change_count += 1;
                    }
                }
            }
            continue;
        }
        if let Some(mut value) = map.remove(&old_key) {
            if let Some(obj) = value.as_object_mut() {
                obj.insert("id".into(), json!(new_key.clone()));
            }
            log::info!("MCP 条目键名已自动修复: '{old_key}' -> '{new_key}'");
            map.insert(new_key, value);
            change_count += 1;
        }
    }

    change_count
}

pub fn normalize_servers_for(config: &mut MultiAppConfig, app: &AppType) -> usize {
    let servers = &mut config.mcp_for_mut(app).servers;
    normalize_server_keys(servers)
}

fn collect_enabled_legacy_servers(config: &McpConfig) -> HashMap<String, Value> {
    config
        .servers
        .iter()
        .filter_map(|(id, entry)| {
            if !entry
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return None;
            }
            entry
                .get("server")
                .filter(|server| server.is_object())
                .cloned()
                .map(|server| (id.clone(), server))
        })
        .collect()
}

fn sync_enabled_compat(config: &MultiAppConfig, app: &AppType) -> Result<(), AppError> {
    crate::services::McpService::replace_servers_for_app(
        &collect_enabled_legacy_servers(config.mcp_for(app)),
        app,
    )
}

#[deprecated(since = "5.10.4", note = "Use McpService APIs instead")]
pub fn sync_enabled_to_claude(config: &MultiAppConfig) -> Result<(), AppError> {
    sync_enabled_compat(config, &AppType::Claude)
}

#[deprecated(since = "5.10.4", note = "Use McpService APIs instead")]
pub fn sync_enabled_to_codex(config: &MultiAppConfig) -> Result<(), AppError> {
    sync_enabled_compat(config, &AppType::Codex)
}

#[deprecated(since = "5.10.4", note = "Use McpService APIs instead")]
pub fn sync_enabled_to_gemini(config: &MultiAppConfig) -> Result<(), AppError> {
    sync_enabled_compat(config, &AppType::Gemini)
}

#[deprecated(since = "5.10.4", note = "Use McpService APIs instead")]
pub fn sync_single_server_to_claude(
    _config: &MultiAppConfig,
    id: &str,
    server_spec: &Value,
) -> Result<(), AppError> {
    crate::services::McpService::project_server_for_app(id, server_spec, &AppType::Claude)
}

#[deprecated(since = "5.10.4", note = "Use McpService APIs instead")]
pub fn sync_single_server_to_codex(
    _config: &MultiAppConfig,
    id: &str,
    server_spec: &Value,
) -> Result<(), AppError> {
    crate::services::McpService::project_server_for_app(id, server_spec, &AppType::Codex)
}

#[deprecated(since = "5.10.4", note = "Use McpService APIs instead")]
pub fn sync_single_server_to_gemini(
    _config: &MultiAppConfig,
    id: &str,
    server_spec: &Value,
) -> Result<(), AppError> {
    crate::services::McpService::project_server_for_app(id, server_spec, &AppType::Gemini)
}

#[deprecated(since = "5.10.4", note = "Use McpService APIs instead")]
pub fn remove_server_from_claude(id: &str) -> Result<(), AppError> {
    crate::services::McpService::remove_server_from_app(id, &AppType::Claude)
}

#[deprecated(since = "5.10.4", note = "Use McpService APIs instead")]
pub fn remove_server_from_codex(id: &str) -> Result<(), AppError> {
    crate::services::McpService::remove_server_from_app(id, &AppType::Codex)
}

#[deprecated(since = "5.10.4", note = "Use McpService APIs instead")]
pub fn remove_server_from_gemini(id: &str) -> Result<(), AppError> {
    crate::services::McpService::remove_server_from_app(id, &AppType::Gemini)
}

#[allow(dead_code)] // v3.7.0: 旧的分应用 API，保留用于未来可能的迁移
pub fn get_servers_snapshot_for(
    config: &mut MultiAppConfig,
    app: &AppType,
) -> (HashMap<String, Value>, usize) {
    let normalized = normalize_servers_for(config, app);
    let mut snapshot = config.mcp_for(app).servers.clone();
    snapshot.retain(|id, value| {
        let Some(obj) = value.as_object_mut() else {
            log::warn!("跳过无效的 MCP 条目 '{id}': 必须为 JSON 对象");
            return false;
        };

        obj.entry(String::from("id")).or_insert(json!(id));

        match validate_mcp_entry(value) {
            Ok(()) => true,
            Err(err) => {
                log::error!("config.json 中存在无效的 MCP 条目 '{id}': {err}");
                false
            }
        }
    });
    (snapshot, normalized)
}

#[allow(dead_code)] // v3.7.0: 旧的分应用 API，保留用于未来可能的迁移
pub fn upsert_in_config_for(
    config: &mut MultiAppConfig,
    app: &AppType,
    id: &str,
    spec: Value,
) -> Result<bool, AppError> {
    if id.trim().is_empty() {
        return Err(AppError::InvalidInput("MCP 服务器 ID 不能为空".into()));
    }
    normalize_servers_for(config, app);
    validate_mcp_entry(&spec)?;

    let mut entry_obj = spec
        .as_object()
        .cloned()
        .ok_or_else(|| AppError::McpValidation("MCP 服务器条目必须为 JSON 对象".into()))?;
    if let Some(existing_id) = entry_obj.get("id") {
        let Some(existing_id_str) = existing_id.as_str() else {
            return Err(AppError::McpValidation("MCP 服务器 id 必须为字符串".into()));
        };
        if existing_id_str != id {
            return Err(AppError::McpValidation(format!(
                "MCP 服务器条目中的 id '{existing_id_str}' 与参数 id '{id}' 不一致"
            )));
        }
    } else {
        entry_obj.insert(String::from("id"), json!(id));
    }

    let value = Value::Object(entry_obj);

    let servers = &mut config.mcp_for_mut(app).servers;
    let before = servers.get(id).cloned();
    servers.insert(id.to_string(), value);

    Ok(before.is_none())
}

#[allow(dead_code)] // v3.7.0: 旧的分应用 API，保留用于未来可能的迁移
pub fn delete_in_config_for(
    config: &mut MultiAppConfig,
    app: &AppType,
    id: &str,
) -> Result<bool, AppError> {
    if id.trim().is_empty() {
        return Err(AppError::InvalidInput("MCP 服务器 ID 不能为空".into()));
    }
    normalize_servers_for(config, app);
    let existed = config.mcp_for_mut(app).servers.remove(id).is_some();
    Ok(existed)
}

#[allow(dead_code)] // v3.7.0: 旧的分应用 API，保留用于未来可能的迁移
/// 设置启用状态（不执行落盘或文件同步）
pub fn set_enabled_flag_for(
    config: &mut MultiAppConfig,
    app: &AppType,
    id: &str,
    enabled: bool,
) -> Result<bool, AppError> {
    if id.trim().is_empty() {
        return Err(AppError::InvalidInput("MCP 服务器 ID 不能为空".into()));
    }
    normalize_servers_for(config, app);
    if let Some(spec) = config.mcp_for_mut(app).servers.get_mut(id) {
        // 写入 enabled 字段
        let mut obj = spec
            .as_object()
            .cloned()
            .ok_or_else(|| AppError::McpValidation("MCP 服务器定义必须为 JSON 对象".into()))?;
        obj.insert("enabled".into(), json!(enabled));
        *spec = Value::Object(obj);
    } else {
        // 若不存在则直接返回 false
        return Ok(false);
    }

    Ok(true)
}

/// 从 ~/.claude.json 导入 mcpServers 到统一结构（v3.7.0+）
/// 已存在的服务器将启用 Claude 应用，不覆盖其他字段和应用状态
pub fn import_from_claude(config: &mut MultiAppConfig) -> Result<usize, AppError> {
    let contents = crate::claude_mcp::read_mcp_json()?;
    import_core_servers(config, AppType::Claude, contents.as_deref())
}

/// 从 ~/.codex/config.toml 导入 MCP 到统一结构（v3.7.0+）
///
/// 格式支持：
/// - 正确格式：[mcp_servers.*]（Codex 官方标准）
/// - 错误格式：[mcp.servers.*]（容错读取，用于迁移错误写入的配置）
///
/// 已存在的服务器将启用 Codex 应用，不覆盖其他字段和应用状态
pub fn import_from_codex(config: &mut MultiAppConfig) -> Result<usize, AppError> {
    let text = crate::codex_config::read_and_validate_codex_config_text()?;
    let imports = import_mcp_servers_compat(&AppType::Codex, Some(text.as_bytes()))?;
    import_core_records(config, AppType::Codex, imports)
}

/// 从 ~/.gemini/settings.json 导入 mcpServers 到统一结构（v3.7.0+）
/// 已存在的服务器将启用 Gemini 应用，不覆盖其他字段和应用状态
pub fn import_from_gemini(config: &mut MultiAppConfig) -> Result<usize, AppError> {
    let contents = crate::gemini_mcp::read_mcp_json()?;
    import_core_servers(config, AppType::Gemini, contents.as_deref())
}

fn import_core_servers(
    config: &mut MultiAppConfig,
    app: AppType,
    contents: Option<&str>,
) -> Result<usize, AppError> {
    let imports = import_mcp_servers_compat(&app, contents.map(str::as_bytes))?;
    import_core_records(config, app, imports)
}

pub(crate) fn import_mcp_servers_compat(
    app: &AppType,
    contents: Option<&[u8]>,
) -> Result<Vec<cc_switch_core::McpImport>, AppError> {
    if let Some(contents) = contents {
        match app {
            AppType::Claude | AppType::Gemini | AppType::OpenCode => {
                let root: Value = serde_json::from_slice(contents).map_err(|error| match app {
                    AppType::Claude => {
                        AppError::McpValidation(format!("解析 ~/.claude.json 失败: {error}"))
                    }
                    AppType::Gemini => {
                        AppError::json(&crate::gemini_config::get_gemini_settings_path(), error)
                    }
                    AppType::OpenCode => {
                        AppError::json(&crate::opencode_config::get_opencode_config_path(), error)
                    }
                    _ => unreachable!("JSON compatibility is limited to JSON MCP applications"),
                })?;
                let section = if *app == AppType::OpenCode {
                    "mcp"
                } else {
                    "mcpServers"
                };
                if !root.get(section).is_some_and(Value::is_object) {
                    return Ok(Vec::new());
                }
            }
            AppType::Hermes => {
                let root: serde_yaml::Value =
                    serde_yaml::from_slice(contents).map_err(|error| {
                        AppError::Config(format!("Failed to parse Hermes config as YAML: {error}"))
                    })?;
                if !root
                    .get("mcp_servers")
                    .is_some_and(serde_yaml::Value::is_mapping)
                {
                    return Ok(Vec::new());
                }
            }
            _ => {}
        }
    }
    let mut imports = cc_switch_core::import_mcp_servers(&app.as_core(), contents)
        .map_err(|error| AppError::McpValidation(error.to_string()))?;
    match app {
        AppType::Claude | AppType::Gemini => {
            let text = contents
                .map(std::str::from_utf8)
                .transpose()
                .map_err(|error| AppError::McpValidation(error.to_string()))?;
            preserve_legacy_json_records(app, text, &mut imports)?;
        }
        AppType::Codex => {
            if let Some(contents) = contents {
                let text = std::str::from_utf8(contents)
                    .map_err(|error| AppError::McpValidation(error.to_string()))?;
                preserve_legacy_codex_extensions(text, &mut imports)?;
            }
        }
        _ => {}
    }
    Ok(imports)
}

fn preserve_legacy_json_records(
    app: &AppType,
    contents: Option<&str>,
    imports: &mut [cc_switch_core::McpImport],
) -> Result<(), AppError> {
    let Some((contents, path)) = (match (app, contents) {
        (AppType::Claude, Some(contents)) => Some((contents, "~/.claude.json")),
        (AppType::Gemini, Some(contents)) => Some((contents, "~/.gemini/settings.json")),
        _ => None,
    }) else {
        return Ok(());
    };
    let root: Value = serde_json::from_str(contents)
        .map_err(|error| AppError::McpValidation(format!("解析 {path} 失败: {error}")))?;
    let Some(entries) = root.get("mcpServers").and_then(Value::as_object) else {
        return Ok(());
    };
    const MANAGED_FIELDS: &[&str] = &[
        "type",
        "command",
        "args",
        "env",
        "cwd",
        "url",
        "httpUrl",
        "headers",
        "http_headers",
    ];
    for import in imports {
        let Some(entry) = entries.get(&import.id).and_then(Value::as_object) else {
            continue;
        };
        let Some(server) = import.server.as_object_mut() else {
            continue;
        };
        for (key, value) in entry {
            if !MANAGED_FIELDS.contains(&key.as_str()) {
                server.insert(key.clone(), value.clone());
            }
        }
    }
    Ok(())
}

fn import_core_records(
    config: &mut MultiAppConfig,
    app: AppType,
    imports: Vec<cc_switch_core::McpImport>,
) -> Result<usize, AppError> {
    use crate::app_config::{McpApps, McpServer};

    let servers = config.mcp.servers.get_or_insert_with(HashMap::new);
    let mut changed = 0;
    for import in imports {
        if let Some(existing) = servers.get_mut(&import.id) {
            if !existing.apps.is_enabled_for(&app) {
                existing.apps.set_enabled_for(&app, true);
                changed += 1;
            }
        } else {
            let mut apps = McpApps::default();
            apps.set_enabled_for(&app, true);
            servers.insert(
                import.id.clone(),
                McpServer {
                    id: import.id.clone(),
                    name: import.id,
                    server: import.server,
                    apps,
                    description: None,
                    homepage: None,
                    docs: None,
                    tags: Vec::new(),
                },
            );
            changed += 1;
        }
    }
    Ok(changed)
}

fn preserve_legacy_codex_extensions(
    text: &str,
    imports: &mut [cc_switch_core::McpImport],
) -> Result<(), AppError> {
    const MANAGED_FIELDS: &[&str] = &[
        "type",
        "command",
        "args",
        "env",
        "cwd",
        "url",
        "headers",
        "http_headers",
    ];
    let root: toml::Table = toml::from_str(text).map_err(|error| {
        AppError::McpValidation(format!("解析 ~/.codex/config.toml 失败: {error}"))
    })?;
    for import in imports {
        let entry = root
            .get("mcp_servers")
            .and_then(toml::Value::as_table)
            .and_then(|servers| servers.get(&import.id))
            .or_else(|| {
                root.get("mcp")
                    .and_then(toml::Value::as_table)
                    .and_then(|mcp| mcp.get("servers"))
                    .and_then(toml::Value::as_table)
                    .and_then(|servers| servers.get(&import.id))
            })
            .and_then(toml::Value::as_table);
        let (Some(entry), Some(server)) = (entry, import.server.as_object_mut()) else {
            continue;
        };
        for (key, value) in entry {
            if MANAGED_FIELDS.contains(&key.as_str()) {
                continue;
            }
            if let Some(value) = legacy_toml_extension(value) {
                server.insert(key.clone(), value);
            }
        }
    }
    Ok(())
}

fn legacy_toml_extension(value: &toml::Value) -> Option<Value> {
    match value {
        toml::Value::String(value) => Some(json!(value)),
        toml::Value::Integer(value) => Some(json!(value)),
        toml::Value::Float(value) => Some(json!(value)),
        toml::Value::Boolean(value) => Some(json!(value)),
        toml::Value::Array(values) => {
            let values = values
                .iter()
                .filter_map(|value| match value {
                    toml::Value::String(value) => Some(json!(value)),
                    toml::Value::Integer(value) => Some(json!(value)),
                    toml::Value::Float(value) => Some(json!(value)),
                    toml::Value::Boolean(value) => Some(json!(value)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            (!values.is_empty()).then_some(Value::Array(values))
        }
        toml::Value::Table(values) => {
            let values = values
                .iter()
                .filter_map(|(key, value)| {
                    value
                        .as_str()
                        .map(|value| (key.clone(), Value::String(value.to_owned())))
                })
                .collect::<serde_json::Map<_, _>>();
            (!values.is_empty()).then_some(Value::Object(values))
        }
        toml::Value::Datetime(_) => None,
    }
}
