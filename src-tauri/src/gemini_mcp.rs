use cc_switch_core::{McpConfigTarget, McpEntryDecodePolicy, McpEntryEncodePolicy};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::atomic_write;
use crate::error::AppError;
use crate::gemini_config::get_gemini_settings_path;

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
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let content = fs::read_to_string(path).map_err(|e| AppError::io(path, e))?;
    let value: Value = serde_json::from_str(&content).map_err(|e| AppError::json(path, e))?;
    Ok(value)
}

fn write_json_value(path: &Path, value: &Value) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }
    let json =
        serde_json::to_string_pretty(value).map_err(|e| AppError::JsonSerialize { source: e })?;
    atomic_write(path, json.as_bytes())
}

/// 读取 Gemini MCP 配置文件的完整 JSON 文本
#[allow(dead_code)]
pub fn read_mcp_json() -> Result<Option<String>, AppError> {
    let path = user_config_path();
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))?;
    Ok(Some(content))
}

/// 读取 Gemini settings.json 中的 mcpServers 映射
pub fn read_mcp_servers_map() -> Result<std::collections::HashMap<String, Value>, AppError> {
    let path = user_config_path();
    if !path.exists() {
        return Ok(std::collections::HashMap::new());
    }

    let root = read_json_value(&path)?;
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
    let mut root = if path.exists() {
        read_json_value(&path)?
    } else {
        serde_json::json!({})
    };

    // 构建 mcpServers 对象：移除 UI 辅助字段（enabled/source），仅保留实际 MCP 规范
    let mut out: Map<String, Value> = Map::new();
    for (id, spec) in servers.iter() {
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
        let native = McpConfigTarget::Gemini
            .encode_server_with_policy(server, McpEntryEncodePolicy::PreserveFields)
            .map_err(|error| AppError::McpValidation(error.to_string()))?;
        let Value::Object(mut obj) = native else {
            return Err(AppError::McpValidation(
                "Gemini MCP entry codec did not return an object".into(),
            ));
        };

        // Catalog metadata stays host-owned; Core preserves unconsumed fields.
        obj.remove("enabled");
        obj.remove("source");
        obj.remove("id");
        obj.remove("name");
        obj.remove("description");
        obj.remove("tags");
        obj.remove("homepage");
        obj.remove("docs");

        out.insert(id.clone(), Value::Object(obj));
    }

    {
        let obj = root
            .as_object_mut()
            .ok_or_else(|| AppError::Config("~/.gemini/settings.json 根必须是对象".into()))?;
        obj.insert("mcpServers".into(), Value::Object(out));
    }

    write_json_value(&path, &root)?;
    Ok(())
}
