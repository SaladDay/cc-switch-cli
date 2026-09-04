use serde_json::{json, Value};
use std::collections::HashMap;

use crate::app_config::{AppType, McpConfig, MultiAppConfig};
use crate::error::AppError;

/// 基础校验：允许 stdio/http/sse；或省略 type（视为 stdio）。对应必填字段存在
fn validate_server_spec(spec: &Value) -> Result<(), AppError> {
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

#[allow(dead_code)] // v3.7.0: 旧的验证逻辑，保留用于未来可能的迁移
fn validate_mcp_entry(entry: &Value) -> Result<(), AppError> {
    let obj = entry
        .as_object()
        .ok_or_else(|| AppError::McpValidation("MCP 服务器条目必须为 JSON 对象".into()))?;

    let server = obj
        .get("server")
        .ok_or_else(|| AppError::McpValidation("MCP 服务器条目缺少 server 字段".into()))?;
    validate_server_spec(server)?;

    for key in ["name", "description", "homepage", "docs"] {
        if let Some(val) = obj.get(key) {
            if !val.is_string() {
                return Err(AppError::McpValidation(format!(
                    "MCP 服务器 {key} 必须为字符串"
                )));
            }
        }
    }

    if let Some(tags) = obj.get("tags") {
        let arr = tags
            .as_array()
            .ok_or_else(|| AppError::McpValidation("MCP 服务器 tags 必须为字符串数组".into()))?;
        if !arr.iter().all(|item| item.is_string()) {
            return Err(AppError::McpValidation(
                "MCP 服务器 tags 必须为字符串数组".into(),
            ));
        }
    }

    if let Some(enabled) = obj.get("enabled") {
        if !enabled.is_boolean() {
            return Err(AppError::McpValidation(
                "MCP 服务器 enabled 必须为布尔值".into(),
            ));
        }
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
    use crate::app_config::{McpApps, McpServer};

    let text_opt = crate::claude_mcp::read_mcp_json()?;
    let Some(text) = text_opt else { return Ok(0) };

    let v: Value = serde_json::from_str(&text)
        .map_err(|e| AppError::McpValidation(format!("解析 ~/.claude.json 失败: {e}")))?;
    let Some(map) = v.get("mcpServers").and_then(|x| x.as_object()) else {
        return Ok(0);
    };

    // 确保新结构存在
    if config.mcp.servers.is_none() {
        config.mcp.servers = Some(HashMap::new());
    }
    let servers = config.mcp.servers.as_mut().unwrap();

    let mut changed = 0;
    let mut errors = Vec::new();

    for (id, spec) in map.iter() {
        // 校验：单项失败不中止，收集错误继续处理
        if let Err(e) = validate_server_spec(spec) {
            log::warn!("跳过无效 MCP 服务器 '{id}': {e}");
            errors.push(format!("{id}: {e}"));
            continue;
        }

        if let Some(existing) = servers.get_mut(id.as_str()) {
            // 已存在：仅启用 Claude 应用
            if !existing.apps.claude {
                existing.apps.claude = true;
                changed += 1;
                log::info!("MCP 服务器 '{id}' 已启用 Claude 应用");
            }
        } else {
            // 新建服务器：默认仅启用 Claude
            servers.insert(
                id.clone(),
                McpServer {
                    id: id.clone(),
                    name: id.clone(),
                    server: spec.clone(),
                    apps: McpApps {
                        claude: true,
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
            changed += 1;
            log::info!("导入新 MCP 服务器 '{id}'");
        }
    }

    if !errors.is_empty() {
        log::warn!("导入完成，但有 {} 项失败: {:?}", errors.len(), errors);
    }

    Ok(changed)
}

/// 从 ~/.codex/config.toml 导入 MCP 到统一结构（v3.7.0+）
///
/// 格式支持：
/// - 正确格式：[mcp_servers.*]（Codex 官方标准）
/// - 错误格式：[mcp.servers.*]（容错读取，用于迁移错误写入的配置）
///
/// 已存在的服务器将启用 Codex 应用，不覆盖其他字段和应用状态
pub fn import_from_codex(config: &mut MultiAppConfig) -> Result<usize, AppError> {
    use crate::app_config::{McpApps, McpServer};

    let text = crate::codex_config::read_and_validate_codex_config_text()?;
    if text.trim().is_empty() {
        return Ok(0);
    }

    let root: toml::Table = toml::from_str(&text)
        .map_err(|e| AppError::McpValidation(format!("解析 ~/.codex/config.toml 失败: {e}")))?;

    // 确保新结构存在
    if config.mcp.servers.is_none() {
        config.mcp.servers = Some(HashMap::new());
    }
    let servers = config.mcp.servers.as_mut().unwrap();

    let mut changed_total = 0usize;

    // helper：处理一组 servers 表
    let mut import_servers_tbl = |servers_tbl: &toml::value::Table| {
        let mut changed = 0usize;
        for (id, entry_val) in servers_tbl.iter() {
            let Some(entry_tbl) = entry_val.as_table() else {
                continue;
            };

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

            let spec_v = serde_json::Value::Object(spec);

            // 校验：单项失败继续处理
            if let Err(e) = validate_server_spec(&spec_v) {
                log::warn!("跳过无效 Codex MCP 项 '{id}': {e}");
                continue;
            }

            if let Some(existing) = servers.get_mut(id) {
                // 已存在：仅启用 Codex 应用
                if !existing.apps.codex {
                    existing.apps.codex = true;
                    changed += 1;
                    log::info!("MCP 服务器 '{id}' 已启用 Codex 应用");
                }
            } else {
                // 新建服务器：默认仅启用 Codex
                servers.insert(
                    id.clone(),
                    McpServer {
                        id: id.clone(),
                        name: id.clone(),
                        server: spec_v,
                        apps: McpApps {
                            claude: false,
                            codex: true,
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
                changed += 1;
                log::info!("导入新 MCP 服务器 '{id}'");
            }
        }
        changed
    };

    // 1) 处理 mcp.servers
    if let Some(mcp_val) = root.get("mcp") {
        if let Some(mcp_tbl) = mcp_val.as_table() {
            if let Some(servers_val) = mcp_tbl.get("servers") {
                if let Some(servers_tbl) = servers_val.as_table() {
                    changed_total += import_servers_tbl(servers_tbl);
                }
            }
        }
    }

    // 2) 处理 mcp_servers
    if let Some(servers_val) = root.get("mcp_servers") {
        if let Some(servers_tbl) = servers_val.as_table() {
            changed_total += import_servers_tbl(servers_tbl);
        }
    }

    Ok(changed_total)
}

/// 从 ~/.gemini/settings.json 导入 mcpServers 到统一结构（v3.7.0+）
/// 已存在的服务器将启用 Gemini 应用，不覆盖其他字段和应用状态
pub fn import_from_gemini(config: &mut MultiAppConfig) -> Result<usize, AppError> {
    use crate::app_config::{McpApps, McpServer};

    let map = crate::gemini_mcp::read_mcp_servers_map()?;
    if map.is_empty() {
        return Ok(0);
    }

    // 确保新结构存在
    if config.mcp.servers.is_none() {
        config.mcp.servers = Some(HashMap::new());
    }
    let servers = config.mcp.servers.as_mut().unwrap();

    let mut changed = 0;
    let mut errors = Vec::new();

    for (id, spec) in map.iter() {
        // 校验：单项失败不中止，收集错误继续处理
        if let Err(e) = validate_server_spec(spec) {
            log::warn!("跳过无效 MCP 服务器 '{id}': {e}");
            errors.push(format!("{id}: {e}"));
            continue;
        }

        if let Some(existing) = servers.get_mut(id) {
            // 已存在：仅启用 Gemini 应用
            if !existing.apps.gemini {
                existing.apps.gemini = true;
                changed += 1;
                log::info!("MCP 服务器 '{id}' 已启用 Gemini 应用");
            }
        } else {
            // 新建服务器：默认仅启用 Gemini
            servers.insert(
                id.clone(),
                McpServer {
                    id: id.clone(),
                    name: id.clone(),
                    server: spec.clone(),
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
            changed += 1;
            log::info!("导入新 MCP 服务器 '{id}'");
        }
    }

    if !errors.is_empty() {
        log::warn!("导入完成，但有 {} 项失败: {:?}", errors.len(), errors);
    }

    Ok(changed)
}

/// OpenCode MCP: OpenCode 格式 → CC Switch 统一格式
fn convert_from_opencode_mcp_spec(spec: &Value) -> Result<Value, AppError> {
    let obj = spec
        .as_object()
        .ok_or_else(|| AppError::McpValidation("OpenCode MCP spec must be a JSON object".into()))?;

    let typ = obj.get("type").and_then(|v| v.as_str()).unwrap_or("local");
    let mut result = serde_json::Map::new();

    match typ {
        "local" => {
            result.insert("type".into(), json!("stdio"));
            if let Some(command) = obj.get("command").and_then(|v| v.as_array()) {
                if let Some(cmd) = command.first().and_then(|v| v.as_str()) {
                    result.insert("command".into(), json!(cmd));
                }
                if command.len() > 1 {
                    result.insert("args".into(), Value::Array(command[1..].to_vec()));
                }
            }
            if let Some(env) = obj.get("environment") {
                if env.is_object() && !env.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                    result.insert("env".into(), env.clone());
                }
            }
        }
        "remote" => {
            result.insert("type".into(), json!("sse"));
            if let Some(url) = obj.get("url") {
                result.insert("url".into(), url.clone());
            }
            if let Some(headers) = obj.get("headers") {
                if headers.is_object() && !headers.as_object().map(|o| o.is_empty()).unwrap_or(true)
                {
                    result.insert("headers".into(), headers.clone());
                }
            }
        }
        other => {
            return Err(AppError::McpValidation(format!(
                "Unknown OpenCode MCP type: {other}"
            )));
        }
    }

    Ok(Value::Object(result))
}

/// 从 ~/.config/opencode/opencode.json 导入 MCP 到统一结构
pub fn import_from_opencode(config: &mut MultiAppConfig) -> Result<usize, AppError> {
    use crate::app_config::{McpApps, McpServer};

    let map = crate::opencode_config::get_mcp_servers()?;
    if map.is_empty() {
        return Ok(0);
    }

    if config.mcp.servers.is_none() {
        config.mcp.servers = Some(HashMap::new());
    }
    let servers = config.mcp.servers.as_mut().unwrap();

    let mut changed = 0;
    let mut errors = Vec::new();

    for (id, spec) in map.iter() {
        let unified = match convert_from_opencode_mcp_spec(spec) {
            Ok(spec) => spec,
            Err(err) => {
                log::warn!("跳过无效 OpenCode MCP 服务器 '{id}': {err}");
                errors.push(format!("{id}: {err}"));
                continue;
            }
        };

        if let Err(err) = validate_server_spec(&unified) {
            log::warn!("跳过无效 MCP 服务器 '{id}': {err}");
            errors.push(format!("{id}: {err}"));
            continue;
        }

        if let Some(existing) = servers.get_mut(id) {
            if !existing.apps.opencode {
                existing.apps.opencode = true;
                changed += 1;
                log::info!("MCP 服务器 '{id}' 已启用 OpenCode 应用");
            }
        } else {
            servers.insert(
                id.clone(),
                McpServer {
                    id: id.clone(),
                    name: id.clone(),
                    server: unified,
                    apps: McpApps {
                        claude: false,
                        codex: false,
                        gemini: false,
                        opencode: true,
                        hermes: false,
                    },
                    description: None,
                    homepage: None,
                    docs: None,
                    tags: Vec::new(),
                },
            );
            changed += 1;
            log::info!("导入新 OpenCode MCP 服务器 '{id}'");
        }
    }

    if !errors.is_empty() {
        log::warn!("导入完成，但有 {} 项失败: {:?}", errors.len(), errors);
    }

    Ok(changed)
}

// ============================================================================
// Hermes MCP import
// ============================================================================
//
// Behavioural notes (aligned with upstream `mcp/hermes.rs`):
// - Hermes has NO explicit `type` field; it infers `stdio` from `command`
//   and `http` from `url`.
// - Hermes-specific fields are stripped on import.

/// Convert Hermes YAML shape back to CC Switch's unified format, stripping
/// Hermes-private fields on the import path.
fn convert_from_hermes_mcp_spec(id: &str, spec: &Value) -> Result<Value, AppError> {
    let obj = spec
        .as_object()
        .ok_or_else(|| AppError::McpValidation("Hermes MCP spec must be a JSON object".into()))?;

    let mut result = serde_json::Map::new();

    if obj.contains_key("command") {
        result.insert("type".into(), json!("stdio"));

        if let Some(command) = obj.get("command") {
            result.insert("command".into(), command.clone());
        }
        if let Some(args) = obj.get("args") {
            if args.is_array() && !args.as_array().map(|a| a.is_empty()).unwrap_or(true) {
                result.insert("args".into(), args.clone());
            }
        }
        if let Some(env) = obj.get("env") {
            if env.is_object() && !env.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                result.insert("env".into(), env.clone());
            }
        }
    } else if obj.contains_key("url") {
        result.insert("type".into(), json!("sse"));

        if let Some(url) = obj.get("url") {
            result.insert("url".into(), url.clone());
        }
        if let Some(headers) = obj.get("headers") {
            if headers.is_object() && !headers.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                result.insert("headers".into(), headers.clone());
            }
        }
    } else {
        return Err(AppError::McpValidation(format!(
            "Hermes MCP server '{id}' has neither a 'command' nor 'url' field"
        )));
    }

    Ok(Value::Object(result))
}

/// Import MCP servers from the Hermes `mcp_servers:` section into the
/// unified store.
pub fn import_from_hermes(config: &mut MultiAppConfig) -> Result<usize, AppError> {
    use crate::app_config::{McpApps, McpServer};

    let yaml_map = crate::hermes_config::get_mcp_servers_yaml()?;
    if yaml_map.is_empty() {
        return Ok(0);
    }

    if config.mcp.servers.is_none() {
        config.mcp.servers = Some(HashMap::new());
    }
    let servers = config.mcp.servers.as_mut().unwrap();

    let mut changed = 0usize;
    let mut errors = Vec::new();

    for (key, spec_yaml) in &yaml_map {
        let id = match key.as_str() {
            Some(s) => s.to_string(),
            None => {
                log::warn!("Skipping Hermes MCP server with non-string key");
                continue;
            }
        };

        let spec_json = match crate::hermes_config::yaml_to_json(spec_yaml) {
            Ok(j) => j,
            Err(e) => {
                log::warn!("Skipping Hermes MCP '{id}': YAML->JSON conversion failed: {e}");
                errors.push(format!("{id}: {e}"));
                continue;
            }
        };

        let unified_spec = match convert_from_hermes_mcp_spec(&id, &spec_json) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("Skipping invalid Hermes MCP '{id}': {e}");
                errors.push(format!("{id}: {e}"));
                continue;
            }
        };

        if let Err(e) = validate_server_spec(&unified_spec) {
            log::warn!("Skipping MCP '{id}' that remained invalid after conversion: {e}");
            errors.push(format!("{id}: {e}"));
            continue;
        }

        if let Some(existing) = servers.get_mut(&id) {
            if !existing.apps.hermes {
                existing.apps.hermes = true;
                changed += 1;
                log::info!("MCP server '{id}' enabled for Hermes");
            }
        } else {
            servers.insert(
                id.clone(),
                McpServer {
                    id: id.clone(),
                    name: id.clone(),
                    server: unified_spec,
                    apps: McpApps {
                        claude: false,
                        codex: false,
                        gemini: false,
                        opencode: false,
                        hermes: true,
                    },
                    description: None,
                    homepage: None,
                    docs: None,
                    tags: Vec::new(),
                },
            );
            changed += 1;
            log::info!("Imported new MCP server '{id}' from Hermes");
        }
    }

    if !errors.is_empty() {
        log::warn!(
            "Hermes MCP import finished with {} failure(s): {:?}",
            errors.len(),
            errors
        );
    }

    Ok(changed)
}

#[cfg(test)]
mod hermes_mcp_tests {
    use super::*;

    #[test]
    fn convert_from_hermes_stdio_strips_extras() {
        let spec = json!({
            "command": "npx",
            "args": ["-y", "x"],
            "env": { "HOME": "/Users/test" },
            "enabled": true,
            "timeout": 30,
            "connect_timeout": 10,
            "tools": { "include": ["read_file"] },
            "sampling": { "enabled": true }
        });
        let result = convert_from_hermes_mcp_spec("fs", &spec).unwrap();
        assert_eq!(result["type"], "stdio");
        assert_eq!(result["command"], "npx");
        assert!(result.get("enabled").is_none());
        assert!(result.get("timeout").is_none());
        assert!(result.get("connect_timeout").is_none());
        assert!(result.get("tools").is_none());
        assert!(result.get("sampling").is_none());
    }

    #[test]
    fn convert_from_hermes_http_strips_extras_and_auth() {
        let spec = json!({
            "url": "https://mcp.example.com",
            "auth": "oauth",
            "enabled": true,
            "timeout": 60
        });
        let result = convert_from_hermes_mcp_spec("remote", &spec).unwrap();
        assert_eq!(result["type"], "sse");
        assert_eq!(result["url"], "https://mcp.example.com");
        assert!(
            result.get("auth").is_none(),
            "auth must be stripped on import"
        );
        assert!(result.get("enabled").is_none());
    }

    #[test]
    fn convert_from_hermes_missing_endpoint_errors() {
        let spec = json!({ "enabled": true, "timeout": 30 });
        assert!(convert_from_hermes_mcp_spec("bad", &spec).is_err());
    }
}
