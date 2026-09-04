use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::AppError;
use crate::gemini_config::get_gemini_settings_path;

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

        servers.insert(id.clone(), spec);
    }

    Ok(servers)
}
