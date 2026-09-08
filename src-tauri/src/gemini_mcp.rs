use cc_switch_core::{McpConfigTarget, McpEntryDecodePolicy, McpEntryEncodePolicy};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::AppError;
use crate::gemini_config::get_gemini_settings_path;

use crate::gemini_config::operation::GeminiOperation;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct McpStatus {
    pub user_config_path: String,
    pub user_config_exists: bool,
    pub server_count: usize,
}

/// 获取 Gemini MCP 配置文件路径（~/.gemini/settings.json）
fn user_config_path() -> PathBuf {
    get_gemini_settings_path()
}

fn read_json_value(path: &Path) -> Result<Value, AppError> {
    parse_json_value(path, read_json_text(path)?.as_deref())
}

fn read_json_text(path: &Path) -> Result<Option<String>, AppError> {
    if !path.exists() {
        return Ok(None);
    }
    fs::read_to_string(path)
        .map(Some)
        .map_err(|e| AppError::io(path, e))
}

fn parse_json_value(path: &Path, content: Option<&str>) -> Result<Value, AppError> {
    match content {
        Some(content) => serde_json::from_str(content).map_err(|e| AppError::json(path, e)),
        None => Ok(serde_json::json!({})),
    }
}

/// 读取 Gemini MCP 配置文件的完整 JSON 文本
#[allow(dead_code)]
pub fn read_mcp_json() -> Result<Option<String>, AppError> {
    read_json_text(&user_config_path())
}

/// 读取 Gemini settings.json 中的 mcpServers 映射
pub fn read_mcp_servers_map() -> Result<std::collections::HashMap<String, Value>, AppError> {
    let path = user_config_path();
    let root = read_json_value(&path)?;
    decode_servers(&root)
}

fn decode_servers(root: &Value) -> Result<std::collections::HashMap<String, Value>, AppError> {
    let mut servers: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    let Some(obj) = root.get("mcpServers").and_then(|v| v.as_object()) else {
        return Ok(servers);
    };

    for (id, raw_spec) in obj {
        let spec = if raw_spec.is_object() {
            McpConfigTarget::Gemini
                .decode_server_with_policy(raw_spec, McpEntryDecodePolicy::InferFromStringFields)
                .map_err(|error| AppError::McpValidation(error.to_string()))?
        } else {
            raw_spec.clone()
        };

        servers.insert(id.clone(), spec);
    }

    Ok(servers)
}

/// 将给定的启用 MCP 服务器映射写入到 Gemini settings.json 的 mcpServers 字段
/// 仅覆盖 mcpServers，其他字段保持不变
pub fn set_mcp_servers_map(
    servers: &std::collections::HashMap<String, Value>,
) -> Result<(), AppError> {
    let path = user_config_path();
    let mut operation = GeminiOperation::observe_settings(&path)?;
    let root = parse_json_value(&path, operation.contents())?;
    publish_servers(&mut operation, root, servers)
}

/// Read-modify-write callers must derive their map from the same observation
/// used by publication. Reading a map and later calling the replacement setter
/// would authorize stale server data against a newer document.
pub(crate) fn update_mcp_servers_map(
    update: impl FnOnce(&mut std::collections::HashMap<String, Value>),
) -> Result<(), AppError> {
    let path = user_config_path();
    let mut operation = GeminiOperation::observe_settings(&path)?;
    update_with_operation(&mut operation, update)
}

pub(crate) fn update_with_operation(
    operation: &mut GeminiOperation,
    update: impl FnOnce(&mut std::collections::HashMap<String, Value>),
) -> Result<(), AppError> {
    let root = parse_json_value(operation.settings_path(), operation.contents())?;
    let mut servers = decode_servers(&root)?;
    update(&mut servers);
    publish_servers(operation, root, &servers)
}

/// Uses the same document observation for the shared snapshot and publication.
/// Existing entry codecs, catalog-wrapper handling and metadata filtering stay
/// host-owned. A removed entry may restore its Core snapshot on activation.
pub(crate) fn prepare_toggles<'a>(
    path: &Path,
    original: Option<&str>,
    changes: impl IntoIterator<
        Item = (
            &'a str,
            &'a Value,
            bool,
            Option<&'a cc_switch_core::McpNativeSnapshot>,
        ),
    >,
) -> Result<(String, Vec<Option<cc_switch_core::McpNativeSnapshot>>), AppError> {
    let mut root = parse_json_value(path, original)?;
    let mut snapshots = Vec::new();
    for (id, server, enabled, previous_snapshot) in changes {
        let entry = root.get("mcpServers").and_then(|servers| servers.get(id));
        let restore = enabled && entry.is_none();
        let snapshot = if enabled {
            None
        } else {
            match entry.filter(|entry| entry.is_object()) {
                Some(entry) => Some(
                    McpConfigTarget::Gemini
                        .capture_native_entry(&entry.to_string())
                        .map_err(|error| AppError::McpValidation(error.to_string()))?,
                ),
                None => previous_snapshot.cloned(),
            }
        };
        let desired = enabled
            .then(|| project_server(id, server, previous_snapshot.filter(|_| restore)))
            .transpose()?;
        let object = root
            .as_object_mut()
            .ok_or_else(|| AppError::Config("~/.gemini/settings.json 根必须是对象".into()))?;
        let collection = object
            .entry("mcpServers")
            .or_insert_with(|| Value::Object(Map::new()));
        // Retain the host's malformed-collection policy, but do not decode or
        // re-encode siblings: a single toggle owns only its selected native entry.
        if !collection.is_object() {
            *collection = Value::Object(Map::new());
        }
        let servers = collection
            .as_object_mut()
            .expect("initialized MCP collection");
        match desired {
            Some(native) => {
                servers.insert(id.to_owned(), native);
            }
            None => {
                servers.shift_remove(id);
            }
        }
        snapshots.push(snapshot);
    }
    let contents =
        serde_json::to_string_pretty(&root).map_err(|source| AppError::JsonSerialize { source })?;
    Ok((contents, snapshots))
}

fn publish_servers(
    operation: &mut GeminiOperation,
    root: Value,
    servers: &std::collections::HashMap<String, Value>,
) -> Result<(), AppError> {
    publish_document(operation, &project_servers(root, servers)?)
}

fn project_servers(
    mut root: Value,
    servers: &std::collections::HashMap<String, Value>,
) -> Result<Value, AppError> {
    // 构建 mcpServers 对象：移除 UI 辅助字段（enabled/source），仅保留实际 MCP 规范
    let mut out: Map<String, Value> = Map::new();
    for (id, spec) in servers.iter() {
        out.insert(id.clone(), project_server(id, spec, None)?);
    }

    {
        let obj = root
            .as_object_mut()
            .ok_or_else(|| AppError::Config("~/.gemini/settings.json 根必须是对象".into()))?;
        obj.insert("mcpServers".into(), Value::Object(out));
    }

    Ok(root)
}

fn project_server(
    id: &str,
    spec: &Value,
    restore: Option<&cc_switch_core::McpNativeSnapshot>,
) -> Result<Value, AppError> {
    let obj = spec
        .as_object()
        .ok_or_else(|| AppError::McpValidation(format!("MCP 服务器 '{id}' 不是对象")))?;
    let server = if let Some(server) = obj.get("server") {
        if !server.is_object() {
            return Err(AppError::McpValidation(format!(
                "MCP 服务器 '{id}' server 字段不是对象"
            )));
        }
        server
    } else {
        spec
    };
    let native = if let Some(snapshot) = restore {
        let mut catalog = server
            .as_object()
            .expect("validated catalog MCP entry")
            .clone();
        // Filter catalog metadata before restoration. Native snapshot
        // extensions are not catalog wrappers or UI metadata.
        remove_catalog_metadata(&mut catalog);
        let restored = McpConfigTarget::Gemini
            .restore_native_entry_with_policy(
                snapshot,
                &Value::Object(catalog),
                McpEntryEncodePolicy::PreserveFields,
            )
            .map_err(|error| AppError::McpValidation(error.to_string()))?;
        serde_json::from_str(&restored)
            .map_err(|error| AppError::json(user_config_path(), error))?
    } else {
        let native = McpConfigTarget::Gemini
            .encode_server_with_policy(server, McpEntryEncodePolicy::PreserveFields)
            .map_err(|error| AppError::McpValidation(error.to_string()))?;
        let Value::Object(mut obj) = native else {
            return Err(AppError::McpValidation(
                "Gemini MCP entry codec did not return an object".into(),
            ));
        };
        remove_catalog_metadata(&mut obj);
        Value::Object(obj)
    };
    Ok(native)
}

fn remove_catalog_metadata(obj: &mut Map<String, Value>) {
    obj.remove("enabled");
    obj.remove("source");
    obj.remove("id");
    obj.remove("name");
    obj.remove("description");
    obj.remove("tags");
    obj.remove("homepage");
    obj.remove("docs");
}

fn publish_document(operation: &mut GeminiOperation, root: &Value) -> Result<(), AppError> {
    let json =
        serde_json::to_string_pretty(root).map_err(|e| AppError::JsonSerialize { source: e })?;
    operation.write_settings(json)
}
