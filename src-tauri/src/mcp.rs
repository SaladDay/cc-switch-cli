use cc_switch_core::codex::{McpDocument as CodexMcpDocument, McpEntry as CodexMcpEntry};
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::app_config::{AppType, McpConfig, MultiAppConfig};
use crate::error::AppError;

mod codex_import;

#[cfg(test)]
mod codex_document_tests;
#[cfg(test)]
mod connection_tests;

/// 基础校验：允许 stdio/http/sse；或省略 type（视为 stdio）。对应必填字段存在
fn validate_server_spec(spec: &Value) -> Result<(), AppError> {
    use cc_switch_core::McpConnectionError;

    cc_switch_core::validate_mcp_connection(spec).map_err(|error| {
        let message = match error {
            McpConnectionError::NotObject => "MCP 服务器连接定义必须为 JSON 对象",
            McpConnectionError::NonStringType => "MCP 服务器 type 必须是字符串",
            McpConnectionError::UnsupportedTransport(_) => {
                "MCP 服务器 type 必须是 'stdio'、'http' 或 'sse'（或省略表示 stdio）"
            }
            McpConnectionError::MissingCommand => "stdio 类型的 MCP 服务器缺少 command 字段",
            McpConnectionError::MissingHttpUrl => "http 类型的 MCP 服务器缺少 url 字段",
            McpConnectionError::MissingSseUrl => "sse 类型的 MCP 服务器缺少 url 字段",
            _ => return AppError::McpValidation(error.to_string()),
        };
        AppError::McpValidation(message.to_owned())
    })
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

fn extract_server_spec(entry: &Value) -> Result<Value, AppError> {
    let obj = entry
        .as_object()
        .ok_or_else(|| AppError::McpValidation("MCP 服务器条目必须为 JSON 对象".into()))?;
    let server = obj
        .get("server")
        .ok_or_else(|| AppError::McpValidation("MCP 服务器条目缺少 server 字段".into()))?;

    if !server.is_object() {
        return Err(AppError::McpValidation(
            "MCP 服务器 server 字段必须为 JSON 对象".into(),
        ));
    }

    Ok(server.clone())
}

/// 返回已启用的 MCP 服务器（过滤 enabled==true）
fn collect_enabled_servers(cfg: &McpConfig) -> HashMap<String, Value> {
    let mut out = HashMap::new();
    for (id, entry) in cfg.servers.iter() {
        let enabled = entry
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !enabled {
            continue;
        }
        match extract_server_spec(entry) {
            Ok(spec) => {
                out.insert(id.clone(), spec);
            }
            Err(err) => {
                log::warn!("跳过无效的 MCP 条目 '{id}': {err}");
            }
        }
    }
    out
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

/// 将 config.json 中 enabled==true 的项投影写入 ~/.claude.json
pub fn sync_enabled_to_claude(config: &MultiAppConfig) -> Result<(), AppError> {
    if !crate::sync_policy::should_sync_live(&AppType::Claude) {
        return Ok(());
    }
    let enabled = collect_enabled_servers(&config.mcp.claude);
    crate::claude_mcp::set_mcp_servers_map(&enabled)
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
/// 按原生配置更新 Codex 启用状态，不覆盖已有连接、其他字段和应用状态。
/// 返回新增服务器数与启用状态发生变化的服务器数之和。
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
    let mut imported = std::collections::HashSet::new();

    // helper：处理一组 servers 表
    let mut import_servers_tbl = |servers_tbl: &toml::value::Table| {
        let mut changed = 0usize;
        for (id, entry_val) in servers_tbl.iter() {
            let Some(entry_tbl) = entry_val.as_table() else {
                continue;
            };

            let spec_v = match codex_import::server_spec(entry_tbl) {
                Ok(spec) => spec,
                Err(error) => {
                    log::warn!("跳过无效 Codex MCP 项 '{id}': {error}");
                    continue;
                }
            };

            // 校验：单项失败继续处理
            if let Err(e) = validate_server_spec(&spec_v) {
                log::warn!("跳过无效 Codex MCP 项 '{id}': {e}");
                continue;
            }

            // Keep the first valid legacy/canonical entry authoritative for
            // both its connection and state, including repeated imports.
            if !imported.insert(id.clone()) {
                continue;
            }
            // The CLI retains non-boolean flags as extensions and historically
            // accepts them as enabled. Valid boolean flags follow Core.
            let enabled = cc_switch_core::McpConfigTarget::Codex
                .entry_enabled_flag(&spec_v)
                .unwrap_or(true);

            if let Some(existing) = servers.get_mut(id) {
                // Import observes state; it does not activate the native entry.
                if existing.apps.codex != enabled {
                    existing.apps.codex = enabled;
                    changed += 1;
                    log::info!("MCP 服务器 '{id}' Codex 启用状态: {enabled}");
                }
            } else {
                // New records retain the native Codex state.
                servers.insert(
                    id.clone(),
                    McpServer {
                        id: id.clone(),
                        name: id.clone(),
                        server: spec_v,
                        apps: McpApps {
                            claude: false,
                            codex: enabled,
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

/// 将 config.json 中 Codex 的 enabled==true 项以 TOML 形式写入 ~/.codex/config.toml
///
/// 格式策略：
/// - 唯一正确格式：[mcp_servers] 顶层表（Codex 官方标准）
/// - 自动清理错误格式：[mcp.servers]（如果存在）
/// - 读取现有 config.toml；若语法无效则报错，不尝试覆盖
/// - 仅更新 `mcp_servers` 表，保留其它键
/// - 仅写入启用项；无启用项时清理 mcp_servers 表
pub fn sync_enabled_to_codex(config: &MultiAppConfig) -> Result<(), AppError> {
    if !crate::sync_policy::should_sync_live(&AppType::Codex) {
        return Ok(());
    }
    // 1) 收集启用项（Codex 维度）
    let enabled = collect_enabled_servers(&config.mcp.codex);

    // 2) 读取现有 config.toml 文本；保持无效 TOML 的错误返回（不覆盖文件）
    let base_text = crate::codex_config::read_and_validate_codex_config_text()?;

    // 3) 使用 toml_edit 解析（允许空文件）
    let mut doc = if base_text.trim().is_empty() {
        CodexMcpDocument::default()
    } else {
        parse_codex_mcp_document(&base_text)
            .map_err(|e| AppError::McpValidation(format!("解析 config.toml 失败: {e}")))?
    };

    // 4) 清理可能存在的错误格式 [mcp.servers]
    if doc.clear_legacy_servers() {
        log::warn!("检测到错误的 MCP 格式 [mcp.servers]，正在清理并迁移到 [mcp_servers]");
    }

    // 5) 构造目标 servers 表（稳定的键顺序）
    if enabled.is_empty() {
        // 无启用项：移除 mcp_servers 表
        doc.clear_servers();
    } else {
        // 构建 servers 表
        let mut entries = Vec::new();
        let mut ids: Vec<_> = enabled.keys().cloned().collect();
        ids.sort();
        for id in &ids {
            let spec = enabled.get(id).expect("spec must exist");
            // 复用通用转换函数（已包含扩展字段支持）
            match json_server_to_codex_entry(spec) {
                Ok(table) => {
                    entries.push((id.as_str(), table));
                }
                Err(err) => {
                    log::error!("跳过无效的 MCP 服务器 '{id}': {err}");
                }
            }
        }
        // 使用唯一正确的格式：[mcp_servers]
        doc.replace_servers(entries);
    }

    // 6) 写回（仅改 TOML，不触碰 auth.json）；toml_edit 会尽量保留未改区域的注释/空白/顺序
    let new_text = doc.render();
    let path = crate::codex_config::get_codex_config_path();
    crate::config::write_text_file(&path, &new_text)?;
    Ok(())
}

/// 将 config.json 中 enabled==true 的项投影写入 ~/.gemini/settings.json
pub fn sync_enabled_to_gemini(config: &MultiAppConfig) -> Result<(), AppError> {
    if !crate::sync_policy::should_sync_live(&AppType::Gemini) {
        return Ok(());
    }
    let enabled = collect_enabled_servers(&config.mcp.gemini);
    crate::gemini_mcp::set_mcp_servers_map(&enabled)
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

/// OpenCode MCP: CC Switch 统一格式 → OpenCode 格式
// The CLI imports only portable connection fields for OpenCode and Hermes.
// Native field names belong to Core; selection and tolerant optional-field
// handling remain the CLI's existing catalog policy.
fn portable_mcp_spec(spec: &Value) -> Result<Value, AppError> {
    let obj = spec
        .as_object()
        .ok_or_else(|| AppError::McpValidation("MCP spec must be a JSON object".into()))?;
    let typ = obj.get("type").and_then(Value::as_str).unwrap_or("stdio");
    let fields: &[&str] = match typ {
        "stdio" => &["command", "args", "env"],
        "http" | "sse" => &["url", "headers"],
        other => {
            return Err(AppError::McpValidation(format!(
                "Unknown MCP type: {other}"
            )))
        }
    };
    let mut result = serde_json::Map::new();
    result.insert("type".into(), json!(typ));
    for &field in fields {
        if let Some(value) = obj.get(field) {
            let include = match field {
                "args" => value.as_array().is_some_and(|array| !array.is_empty()),
                "env" | "headers" => value.as_object().is_some_and(|map| !map.is_empty()),
                _ => true,
            };
            if include {
                result.insert(field.into(), value.clone());
            }
        }
    }
    Ok(Value::Object(result))
}

pub(crate) fn convert_to_opencode_mcp_spec(spec: &Value) -> Result<Value, AppError> {
    cc_switch_core::McpConfigTarget::OpenCode
        .encode_server(&portable_mcp_spec(spec)?)
        .map_err(|error| AppError::McpValidation(error.to_string()))
}

/// OpenCode MCP: OpenCode 格式 → CC Switch 统一格式
fn convert_from_opencode_mcp_spec(spec: &Value) -> Result<Value, AppError> {
    let mut native = spec
        .as_object()
        .cloned()
        .ok_or_else(|| AppError::McpValidation("OpenCode MCP spec must be a JSON object".into()))?;
    // Only the native command array and environment map define these fields.
    native.remove("args");
    native.remove("env");
    let unified = cc_switch_core::McpConfigTarget::OpenCode
        .decode_server(&Value::Object(native))
        .map_err(|error| AppError::McpValidation(error.to_string()))?;
    portable_mcp_spec(&unified)
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

        let enabled = cc_switch_core::McpConfigTarget::OpenCode
            .entry_enabled_flag(spec)
            .map_err(|error| AppError::McpValidation(error.to_string()))?;

        if let Some(existing) = servers.get_mut(id) {
            if existing.apps.opencode != enabled {
                existing.apps.opencode = enabled;
                changed += 1;
                log::info!("MCP 服务器 '{id}' OpenCode 启用状态: {enabled}");
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
                        opencode: enabled,
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
// v3.7.0 新增：单个服务器同步和删除函数
// ============================================================================

/// 将单个 MCP 服务器同步到 Claude live 配置
pub fn sync_single_server_to_claude(
    _config: &MultiAppConfig,
    id: &str,
    server_spec: &Value,
) -> Result<(), AppError> {
    if !crate::sync_policy::should_sync_live(&AppType::Claude) {
        return Ok(());
    }
    // 读取现有的 MCP 配置
    let current = crate::claude_mcp::read_mcp_servers_map()?;

    // 创建新的 HashMap，包含现有的所有服务器 + 当前要同步的服务器
    let mut updated = current;
    updated.insert(id.to_string(), server_spec.clone());

    // 写回
    crate::claude_mcp::set_mcp_servers_map(&updated)
}

/// 从 Claude live 配置中移除单个 MCP 服务器
pub fn remove_server_from_claude(id: &str) -> Result<(), AppError> {
    if !crate::sync_policy::should_sync_live(&AppType::Claude) {
        return Ok(());
    }
    // 读取现有的 MCP 配置
    let mut current = crate::claude_mcp::read_mcp_servers_map()?;

    // 移除指定服务器
    current.remove(id);

    // 写回
    crate::claude_mcp::set_mcp_servers_map(&current)
}

/// 通用 JSON 值到 TOML 值转换器（支持简单类型和浅层嵌套）
///
/// 支持的类型转换：
/// - String → TOML String
/// - Number (i64) → TOML Integer
/// - Number (f64) → TOML Float
/// - Boolean → TOML Boolean
/// - Array[简单类型] → TOML Array
/// - Object → TOML Inline Table (仅字符串值)
///
/// 不支持的类型（返回 None）：
/// - null
/// - 深度嵌套对象
/// - 混合类型数组
fn json_value_to_toml_item(value: &Value, field_name: &str) -> Option<toml_edit::Item> {
    use toml_edit::{Array, InlineTable, Item};

    match value {
        Value::String(s) => Some(toml_edit::value(s.as_str())),

        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(toml_edit::value(i))
            } else if let Some(f) = n.as_f64() {
                Some(toml_edit::value(f))
            } else {
                log::warn!("跳过字段 '{field_name}': 无法转换的数字类型 {n}");
                None
            }
        }

        Value::Bool(b) => Some(toml_edit::value(*b)),

        Value::Array(arr) => {
            // 只支持简单类型的数组（字符串、数字、布尔）
            let mut toml_arr = Array::default();
            let mut all_same_type = true;

            for item in arr {
                match item {
                    Value::String(s) => toml_arr.push(s.as_str()),
                    Value::Number(n) if n.is_i64() => toml_arr.push(n.as_i64().unwrap()),
                    Value::Number(n) if n.is_f64() => toml_arr.push(n.as_f64().unwrap()),
                    Value::Bool(b) => toml_arr.push(*b),
                    _ => {
                        all_same_type = false;
                        break;
                    }
                }
            }

            if all_same_type && !toml_arr.is_empty() {
                Some(Item::Value(toml_edit::Value::Array(toml_arr)))
            } else {
                log::warn!("跳过字段 '{field_name}': 不支持的数组类型（混合类型或嵌套结构）");
                None
            }
        }

        Value::Object(obj) => {
            // 只支持浅层对象（所有值都是字符串）→ TOML Inline Table
            let mut inline_table = InlineTable::new();
            let mut all_strings = true;

            for (k, v) in obj {
                if let Some(s) = v.as_str() {
                    // InlineTable 需要 Value 类型，toml_edit::value() 返回 Item，需要提取内部的 Value
                    inline_table.insert(k, s.into());
                } else {
                    all_strings = false;
                    break;
                }
            }

            if all_strings && !inline_table.is_empty() {
                Some(Item::Value(toml_edit::Value::InlineTable(inline_table)))
            } else {
                log::warn!("跳过字段 '{field_name}': 对象值包含非字符串类型，建议使用子表语法");
                None
            }
        }

        Value::Null => {
            log::debug!("跳过字段 '{field_name}': TOML 不支持 null 值");
            None
        }
    }
}

/// 将 JSON MCP 服务器规范转换为 Core 原生条目。
///
/// 策略：
/// 1. 核心字段（type, command, args, url, headers, env, cwd）使用强类型处理
/// 2. 扩展字段（timeout、retry 等）通过白名单列表自动转换
/// 3. 其他未知字段使用通用转换器尝试转换
pub(crate) fn json_server_to_codex_entry(spec: &Value) -> Result<CodexMcpEntry, AppError> {
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
    let mut t =
        CodexMcpEntry::parse(&text).map_err(|error| AppError::McpValidation(error.to_string()))?;

    // 2. 处理扩展字段和其他未知字段
    if let Some(obj) = spec.as_object() {
        for (key, value) in obj {
            // 跳过已处理的核心字段
            if core_fields.contains(&key.as_str()) {
                continue;
            }

            // 尝试使用通用转换器
            if let Some(toml_item) = json_value_to_toml_item(value, key) {
                t.insert_native_value(key, &toml_item.to_string())
                    .map_err(|error| AppError::McpValidation(error.to_string()))?;

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

/// 把单个 MCP server 表写入 `[mcp_servers]`，并保证该键是表。
///
/// `config.toml` 允许用户手工编辑。若 `mcp_servers` 是 inline table，
/// `as_table_mut` 会误判为不可用；若它是标量，直接通过 IndexMut 写入会 panic。
/// 与上游保持一致，在一个 doc 级辅助函数中统一处理这两种情况。
fn upsert_mcp_server_table(
    doc: &mut CodexMcpDocument,
    id: &str,
    table: CodexMcpEntry,
) -> Result<(), AppError> {
    let repaired = doc.upsert_server(id, table);
    if repaired {
        log::warn!("config.toml 的 mcp_servers 不是表，已重置为空表");
    }
    Ok(())
}

// Keep the CLI's accepted grammar and native diagnostics before shared edits.
pub(crate) fn parse_codex_mcp_document(
    contents: &str,
) -> Result<CodexMcpDocument, Box<dyn std::error::Error + Send + Sync>> {
    contents.parse::<toml_edit::DocumentMut>()?;
    Ok(CodexMcpDocument::parse(contents)?)
}

/// 从 `[mcp_servers]` 和历史错误格式 `[mcp.servers]` 中删除单个服务器。
///
/// 使用 `as_table_like_mut` 同时支持普通表与合法的 inline table，避免界面
/// 报告删除成功但 live 配置仍保留该服务器。
fn remove_mcp_server_from_doc(doc: &mut CodexMcpDocument, id: &str) {
    let removed = doc.remove_server(id);
    if removed.malformed_official_collection {
        log::warn!("config.toml 的 mcp_servers 不是表，无法删除服务器 '{id}'");
    }
    if removed.removed_legacy {
        log::warn!("从错误的 MCP 格式 [mcp.servers] 中清理了服务器 '{id}'");
    }
}

/// 将单个 MCP 服务器同步到 Codex live 配置
/// 始终使用 Codex 官方格式 [mcp_servers]，并清理可能存在的错误格式 [mcp.servers]
pub fn sync_single_server_to_codex(
    _config: &MultiAppConfig,
    id: &str,
    server_spec: &Value,
) -> Result<(), AppError> {
    if !crate::sync_policy::should_sync_live(&AppType::Codex) {
        return Ok(());
    }
    // 读取现有的 config.toml
    let config_path = crate::codex_config::get_codex_config_path();

    let mut doc = if config_path.exists() {
        let content =
            std::fs::read_to_string(&config_path).map_err(|e| AppError::io(&config_path, e))?;
        // 解析失败必须报错而不是用空文档顶替：写回空文档会把用户
        // config.toml 里的其它段落（model/model_providers/注释等）整体清空。
        parse_codex_mcp_document(&content)
            .map_err(|e| AppError::McpValidation(format!("解析 config.toml 失败: {e}")))?
    } else {
        CodexMcpDocument::default()
    };

    // 清理可能存在的错误格式 [mcp.servers]
    if doc.clear_legacy_servers() {
        log::warn!("检测到错误的 MCP 格式 [mcp.servers]，正在清理并迁移到 [mcp_servers]");
    }

    // 将 JSON 服务器规范转换为 TOML 表
    let toml_table = json_server_to_codex_entry(server_spec)?;
    upsert_mcp_server_table(&mut doc, id, toml_table)?;

    // 写回文件
    let new_text = doc.render();
    crate::config::write_text_file(&config_path, &new_text)?;

    Ok(())
}

/// 从 Codex live 配置中移除单个 MCP 服务器
/// 从正确的 [mcp_servers] 表中删除，同时清理可能存在于错误位置 [mcp.servers] 的数据
pub fn remove_server_from_codex(id: &str) -> Result<(), AppError> {
    if !crate::sync_policy::should_sync_live(&AppType::Codex) {
        return Ok(());
    }
    let config_path = crate::codex_config::get_codex_config_path();

    if !config_path.exists() {
        return Ok(()); // 文件不存在，无需删除
    }

    let content =
        std::fs::read_to_string(&config_path).map_err(|e| AppError::io(&config_path, e))?;

    // 尝试解析现有配置，如果失败则直接返回（无法删除不存在的内容）
    let mut doc = match parse_codex_mcp_document(&content) {
        Ok(doc) => doc,
        Err(e) => {
            log::warn!("解析 Codex config.toml 失败: {e}，跳过删除操作");
            return Ok(());
        }
    };

    remove_mcp_server_from_doc(&mut doc, id);

    // 写回文件
    let new_text = doc.render();
    crate::config::write_text_file(&config_path, &new_text)?;

    Ok(())
}

/// 将单个 MCP 服务器同步到 Gemini live 配置
pub fn sync_single_server_to_gemini(
    _config: &MultiAppConfig,
    id: &str,
    server_spec: &Value,
) -> Result<(), AppError> {
    sync_gemini_server(id, server_spec)
}

pub(crate) fn sync_gemini_server(id: &str, server_spec: &Value) -> Result<(), AppError> {
    if !crate::sync_policy::should_sync_live(&AppType::Gemini) {
        return Ok(());
    }

    crate::gemini_mcp::update_mcp_servers_map(|servers| {
        servers.insert(id.to_string(), server_spec.clone());
    })
}

/// 从 Gemini live 配置中移除单个 MCP 服务器
pub fn remove_server_from_gemini(id: &str) -> Result<(), AppError> {
    if !crate::sync_policy::should_sync_live(&AppType::Gemini) {
        return Ok(());
    }

    crate::gemini_mcp::update_mcp_servers_map(|servers| {
        servers.remove(id);
    })
}

/// 将单个 MCP 服务器同步到 OpenCode live 配置
pub fn sync_single_server_to_opencode(
    _config: &MultiAppConfig,
    id: &str,
    server_spec: &Value,
) -> Result<(), AppError> {
    if !crate::sync_policy::should_sync_live(&AppType::OpenCode) {
        return Ok(());
    }

    let spec = convert_to_opencode_mcp_spec(server_spec)?;
    crate::opencode_config::set_mcp_server(id, spec)
}

/// 从 OpenCode live 配置中移除单个 MCP 服务器
pub fn remove_server_from_opencode(id: &str) -> Result<(), AppError> {
    if !crate::sync_policy::should_sync_live(&AppType::OpenCode) {
        return Ok(());
    }

    crate::opencode_config::remove_mcp_server(id)
}

// ============================================================================
// Hermes MCP sync / remove / import
// ============================================================================
//
// Behavioural notes (aligned with upstream `mcp/hermes.rs`):
// - Hermes has NO explicit `type` field; it infers `stdio` from `command`
//   and `http` from `url`.
// - Hermes carries extra per-server fields: `enabled` / `timeout` /
//   `connect_timeout` / `tools` / `sampling` / `roots` / `auth`. These are
//   preserved on merge-on-write and stripped on import.

/// Hermes-private fields preserved on write and stripped on import.
const HERMES_EXTRA_FIELDS: &[&str] = &[
    "enabled",
    "timeout",
    "connect_timeout",
    "tools",
    "sampling",
    "roots",
    "auth",
];

fn should_sync_hermes_mcp() -> bool {
    crate::hermes_config::get_hermes_dir().exists()
}

/// Convert CC Switch's unified MCP format to the Hermes YAML shape.
fn convert_to_hermes_mcp_spec(spec: &Value) -> Result<Value, AppError> {
    cc_switch_core::McpConfigTarget::Hermes
        .encode_server(&portable_mcp_spec(spec)?)
        .map_err(|error| AppError::McpValidation(error.to_string()))
}

/// Import only portable connection fields, leaving native metadata in Hermes.
fn convert_from_hermes_mcp_spec(id: &str, spec: &Value) -> Result<Value, AppError> {
    let unified = cc_switch_core::McpConfigTarget::Hermes
        .decode_server(spec)
        .map_err(|error| AppError::McpValidation(format!("Hermes MCP server '{id}': {error}")))?;
    portable_mcp_spec(&unified)
}
/// Merge: core fields come from `new_spec`, Hermes-specific fields are
/// preserved from `existing`.
fn merge_hermes_spec(existing: &Value, new_spec: &Value) -> Value {
    let mut result = serde_json::Map::new();

    if let Some(existing_obj) = existing.as_object() {
        for &field in HERMES_EXTRA_FIELDS {
            if let Some(val) = existing_obj.get(field) {
                result.insert(field.to_string(), val.clone());
            }
        }
    }

    if let Some(new_obj) = new_spec.as_object() {
        for (key, val) in new_obj {
            if HERMES_EXTRA_FIELDS.contains(&key.as_str()) && result.contains_key(key) {
                continue; // Existing Hermes-private fields win.
            }
            result.insert(key.clone(), val.clone());
        }
    }

    Value::Object(result)
}

/// Sync a single MCP server to the Hermes live config using
/// merge-on-write semantics (preserves Hermes-private fields).
pub fn sync_single_server_to_hermes(
    _config: &MultiAppConfig,
    id: &str,
    server_spec: &Value,
) -> Result<(), AppError> {
    if !crate::sync_policy::should_sync_live(&AppType::Hermes) {
        return Ok(());
    }
    if !should_sync_hermes_mcp() {
        return Ok(());
    }

    let hermes_spec = convert_to_hermes_mcp_spec(server_spec)?;
    let id_owned = id.to_string();

    crate::hermes_config::update_mcp_servers_yaml(|servers| {
        let id_yaml = serde_yaml::Value::String(id_owned.clone());

        let merged_json = if let Some(existing_yaml) = servers.get(&id_yaml) {
            let existing_json = crate::hermes_config::yaml_to_json(existing_yaml)?;
            merge_hermes_spec(&existing_json, &hermes_spec)
        } else {
            hermes_spec.clone()
        };

        let merged_yaml_value = crate::hermes_config::json_to_yaml(&merged_json)?;
        servers.insert(id_yaml, merged_yaml_value);
        Ok(())
    })
}

/// Remove a single MCP server from the Hermes live config.
pub fn remove_server_from_hermes(id: &str) -> Result<(), AppError> {
    if !crate::sync_policy::should_sync_live(&AppType::Hermes) {
        return Ok(());
    }
    if !should_sync_hermes_mcp() {
        return Ok(());
    }

    let id_owned = id.to_string();
    crate::hermes_config::update_mcp_servers_yaml(|servers| {
        servers.remove(serde_yaml::Value::String(id_owned.clone()));
        Ok(())
    })
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

        let enabled = cc_switch_core::McpConfigTarget::Hermes
            .entry_enabled_flag(&spec_json)
            .map_err(|error| AppError::McpValidation(error.to_string()))?;

        if let Some(existing) = servers.get_mut(&id) {
            if existing.apps.hermes != enabled {
                existing.apps.hermes = enabled;
                changed += 1;
                log::info!("MCP server '{id}' Hermes enabled state: {enabled}");
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
                        hermes: enabled,
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
mod portable_mcp_tests {
    use super::*;

    #[test]
    fn export_keeps_the_existing_portable_field_policy() {
        for (spec, opencode, hermes) in [
            (
                json!({"command":"node", "args":["server", 42], "env":{"KEY":false},
                    "cwd":"/private", "timeout":900, "enabled":false, "url":"ignored"}),
                json!({"type":"local", "command":["node","server",42],
                    "environment":{"KEY":false}, "enabled":true}),
                json!({"command":"node", "args":["server",42], "env":{"KEY":false}, "enabled":true}),
            ),
            (
                json!({"type":"http", "url":"https://example.com/mcp", "headers":{"X-Test":42},
                    "command":"ignored", "auth":"oauth", "args":[], "env":{}}),
                json!({"type":"remote", "url":"https://example.com/mcp", "headers":{"X-Test":42}, "enabled":true}),
                json!({"url":"https://example.com/mcp", "headers":{"X-Test":42}, "enabled":true}),
            ),
            (
                json!({"type":false, "command":"node", "args":false, "env":[], "headers":{}}),
                json!({"type":"local", "command":["node"], "enabled":true}),
                json!({"command":"node", "enabled":true}),
            ),
            (
                json!({"type":"sse", "headers":{}, "url":null}),
                json!({"type":"remote", "url":null, "enabled":true}),
                json!({"url":null, "enabled":true}),
            ),
        ] {
            assert_eq!(convert_to_opencode_mcp_spec(&spec).unwrap(), opencode);
            assert_eq!(convert_to_hermes_mcp_spec(&spec).unwrap(), hermes);
        }
    }

    #[test]
    fn opencode_import_uses_only_native_connection_fields() {
        for (native, expected) in [
            (
                json!({"command":["node"], "args":["ignored"], "env":{"IGNORED":"yes"},
                    "environment":{}, "url":"ignored", "enabled":false, "timeout":900}),
                json!({"type":"stdio", "command":"node"}),
            ),
            (
                json!({"type":"local", "command":["node",42], "environment":{"KEY":false}}),
                json!({"type":"stdio", "command":"node", "args":[42], "env":{"KEY":false}}),
            ),
            (
                json!({"type":"remote", "url":"https://example.com/mcp", "headers":{},
                    "command":["ignored"], "oauth":true}),
                json!({"type":"sse", "url":"https://example.com/mcp"}),
            ),
        ] {
            assert_eq!(convert_from_opencode_mcp_spec(&native).unwrap(), expected);
        }
    }

    #[test]
    fn hermes_import_preserves_endpoint_precedence_and_optional_field_handling() {
        let native = json!({"command":"node", "url":"ignored", "args":[], "env":false,
            "enabled":false, "headers":{"ignored":"yes"}, "timeout":90});
        assert_eq!(
            convert_from_hermes_mcp_spec("test", &native).unwrap(),
            json!({"type":"stdio", "command":"node"})
        );
    }
}

#[cfg(test)]
mod codex_mcp_tests {
    use super::*;

    #[test]
    fn codex_entry_encoding_preserves_tolerant_fields_and_extensions() {
        for (spec, expected) in [
            (
                json!({"command":"node", "args":["server",42,false],
                    "env":{"KEEP":"yes", "DROP":false}, "cwd":" /work ",
                    "startup_timeout_sec":3.5, "enabled":false, "name":"extension",
                    "headers":{"CROSS":"preserved"}, "unknown":{"key":"value"},
                    "nested":{"not":"all strings", "number":42}, "empty":[]}),
                json!({"type":"stdio", "command":"node", "args":["server"],
                    "env":{"KEEP":"yes"}, "cwd":" /work ",
                    "startup_timeout_sec":3.5, "enabled":false, "name":"extension",
                    "headers":{"CROSS":"preserved"}, "unknown":{"key":"value"}}),
            ),
            (
                json!({"type":"http", "url":"https://example.com/mcp",
                    "headers":{"Authorization":"test-key", "DROP":42},
                    "http_headers":{"IGNORED":"native alias"},
                    "env":{"CROSS":"preserved"}, "args":["mixed",42,false], "retry_count":2}),
                json!({"type":"http", "url":"https://example.com/mcp",
                    "http_headers":{"Authorization":"test-key"},
                    "env":{"CROSS":"preserved"}, "args":["mixed",42,false], "retry_count":2}),
            ),
            (
                json!({"type":"sse", "url":false, "headers":false,
                    "http_headers":{"IGNORED":"native alias"}}),
                json!({"type":"sse", "url":""}),
            ),
            (
                json!({"type":"future", "command":"preserved", "args":[42]}),
                json!({"type":"future", "command":"preserved", "args":[42]}),
            ),
            (
                json!({"command":false, "args":[42], "env":{}, "cwd":"  "}),
                json!({"type":"stdio", "command":""}),
            ),
            (Value::Null, json!({"type":"stdio", "command":""})),
        ] {
            let mut document = CodexMcpDocument::default();
            document.upsert_server("entry", json_server_to_codex_entry(&spec).expect("encode"));
            let parsed: toml::Value = toml::from_str(&document.render()).expect("native TOML");
            assert_eq!(
                serde_json::to_value(&parsed["mcp_servers"]["entry"]).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn upsert_normalizes_non_table_mcp_servers_without_panicking() {
        for malformed in [
            "mcp_servers = \"x\"\n",
            "mcp_servers = []\n",
            "mcp_servers = 42\n",
        ] {
            let mut doc = malformed
                .parse::<CodexMcpDocument>()
                .expect("fixture parses");
            let table = json_server_to_codex_entry(&json!({
                "type": "stdio",
                "command": "npx"
            }))
            .expect("server table");

            upsert_mcp_server_table(&mut doc, "echo", table)
                .unwrap_or_else(|error| panic!("upsert must not fail for {malformed:?}: {error}"));

            let doc = doc
                .render()
                .parse::<toml_edit::DocumentMut>()
                .expect("rendered native TOML");

            let servers = doc
                .get("mcp_servers")
                .and_then(toml_edit::Item::as_table_like)
                .expect("mcp_servers must be normalized to a table");
            assert!(servers.contains_key("echo"));
        }
    }

    #[test]
    fn upsert_preserves_existing_servers_in_a_valid_table() {
        let mut doc = "[mcp_servers.keep]\ncommand = \"keep\"\n"
            .parse::<CodexMcpDocument>()
            .expect("fixture parses");
        let table = json_server_to_codex_entry(&json!({
            "type": "stdio",
            "command": "npx"
        }))
        .expect("server table");

        upsert_mcp_server_table(&mut doc, "added", table).expect("upsert");

        let doc = doc
            .render()
            .parse::<toml_edit::DocumentMut>()
            .expect("rendered native TOML");

        let servers = doc
            .get("mcp_servers")
            .and_then(toml_edit::Item::as_table_like)
            .expect("table");
        assert!(servers.contains_key("keep"), "existing server must survive");
        assert!(servers.contains_key("added"));
    }

    #[test]
    fn upsert_preserves_existing_servers_in_an_inline_table() {
        let mut doc = "mcp_servers = { keep = { command = \"keep\" } }\n"
            .parse::<CodexMcpDocument>()
            .expect("fixture parses");
        let table = json_server_to_codex_entry(&json!({
            "type": "stdio",
            "command": "npx"
        }))
        .expect("server table");

        upsert_mcp_server_table(&mut doc, "added", table).expect("upsert");

        let doc = doc
            .render()
            .parse::<toml_edit::DocumentMut>()
            .expect("rendered native TOML");

        let servers = doc
            .get("mcp_servers")
            .and_then(toml_edit::Item::as_table_like)
            .expect("inline table");
        assert!(servers.contains_key("keep"), "existing server must survive");
        assert!(servers.contains_key("added"));
    }

    #[test]
    fn remove_deletes_from_inline_table_form_too() {
        let mut doc = "mcp_servers = { drop = { command = \"x\" }, keep = { command = \"y\" } }\n"
            .parse::<CodexMcpDocument>()
            .expect("fixture parses");

        remove_mcp_server_from_doc(&mut doc, "drop");

        let doc = doc
            .render()
            .parse::<toml_edit::DocumentMut>()
            .expect("rendered native TOML");

        let servers = doc
            .get("mcp_servers")
            .and_then(toml_edit::Item::as_table_like)
            .expect("mcp_servers must still be table-like");
        assert!(!servers.contains_key("drop"));
        assert!(servers.contains_key("keep"), "siblings must survive");
    }

    #[test]
    fn remove_is_a_noop_on_non_table_mcp_servers() {
        let mut doc = "mcp_servers = 42\n"
            .parse::<CodexMcpDocument>()
            .expect("fixture parses");

        remove_mcp_server_from_doc(&mut doc, "whatever");

        assert_eq!(doc.render(), "mcp_servers = 42\n");
    }

    #[test]
    fn http_headers_are_only_written_to_codex_http_headers() {
        let table = json_server_to_codex_entry(&json!({
            "type": "http",
            "url": "https://mcp.example.com",
            "headers": {
                "Authorization": "Bearer top-secret",
                "X-Api-Key": "also-secret"
            },
            "timeout": 30
        }))
        .expect("server table");

        let mut document = CodexMcpDocument::default();
        document.upsert_server("entry", table);
        let document: toml_edit::DocumentMut = document.render().parse().unwrap();
        let table = document["mcp_servers"]["entry"].as_table().unwrap();
        let headers = table
            .get("http_headers")
            .and_then(toml_edit::Item::as_table)
            .expect("Codex http_headers table should be written");
        assert_eq!(
            headers
                .get("Authorization")
                .and_then(toml_edit::Item::as_str),
            Some("Bearer top-secret")
        );
        assert!(
            table.get("headers").is_none(),
            "legacy headers must not be emitted a second time"
        );
        assert_eq!(
            table.get("timeout").and_then(toml_edit::Item::as_integer),
            Some(30)
        );
    }
}

#[cfg(test)]
mod hermes_mcp_tests {
    use super::*;

    #[test]
    fn convert_stdio_to_hermes() {
        let spec = json!({
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-filesystem"],
            "env": { "HOME": "/Users/test" }
        });
        let result = convert_to_hermes_mcp_spec(&spec).unwrap();
        assert!(result.get("type").is_none());
        assert_eq!(result["command"], "npx");
        assert_eq!(result["args"][0], "-y");
        assert_eq!(result["env"]["HOME"], "/Users/test");
        assert_eq!(result["enabled"], true);
    }

    #[test]
    fn convert_sse_to_hermes() {
        let spec = json!({
            "type": "sse",
            "url": "https://example.com/mcp",
            "headers": { "Authorization": "Bearer xxx" }
        });
        let result = convert_to_hermes_mcp_spec(&spec).unwrap();
        assert!(result.get("type").is_none());
        assert_eq!(result["url"], "https://example.com/mcp");
        assert_eq!(result["headers"]["Authorization"], "Bearer xxx");
        assert_eq!(result["enabled"], true);
    }

    #[test]
    fn convert_stdio_empty_collections_are_omitted() {
        let spec = json!({
            "type": "stdio",
            "command": "node",
            "args": [],
            "env": {}
        });
        let result = convert_to_hermes_mcp_spec(&spec).unwrap();
        assert_eq!(result["command"], "node");
        assert!(result.get("args").is_none());
        assert!(result.get("env").is_none());
    }

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

    #[test]
    fn merge_preserves_hermes_extra_fields() {
        let existing = json!({
            "command": "old-cmd",
            "args": ["old-arg"],
            "enabled": true,
            "timeout": 30,
            "connect_timeout": 10,
            "tools": { "include": ["read_file"] },
            "sampling": { "enabled": true }
        });
        let new_spec = json!({
            "command": "new-cmd",
            "args": ["new-arg"],
            "env": { "KEY": "value" },
            "enabled": true
        });
        let merged = merge_hermes_spec(&existing, &new_spec);
        assert_eq!(merged["command"], "new-cmd");
        assert_eq!(merged["args"][0], "new-arg");
        assert_eq!(merged["env"]["KEY"], "value");
        assert_eq!(merged["timeout"], 30);
        assert_eq!(merged["connect_timeout"], 10);
        assert_eq!(merged["tools"]["include"][0], "read_file");
        assert_eq!(merged["sampling"]["enabled"], true);
    }

    #[test]
    fn merge_preserves_auth_field_on_roundtrip() {
        let existing = json!({
            "url": "https://mcp.example.com",
            "auth": "oauth",
            "enabled": true
        });
        let new_spec = json!({
            "url": "https://mcp.example.com/updated",
            "headers": { "X-Trace": "abc" },
            "enabled": true
        });
        let merged = merge_hermes_spec(&existing, &new_spec);
        assert_eq!(merged["url"], "https://mcp.example.com/updated");
        assert_eq!(merged["headers"]["X-Trace"], "abc");
        assert_eq!(merged["auth"], "oauth");
    }
}
