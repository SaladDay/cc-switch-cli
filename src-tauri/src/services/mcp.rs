use std::collections::{BTreeSet, HashMap};

use crate::app_config::{AppType, McpApps, McpServer, MultiAppConfig};
use crate::error::AppError;
use crate::mcp;
use crate::store::AppState;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpLiveDriftKind {
    InSync,
    LiveOnly,
    DbOnly,
    Changed,
    LiveInvalid,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct McpLiveDriftEntry {
    pub app: AppType,
    pub id: String,
    pub kind: McpLiveDriftKind,
    pub db_spec: Option<Value>,
    pub live_spec: Option<Value>,
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct McpLiveDriftReport {
    pub app: AppType,
    pub entries: Vec<McpLiveDriftEntry>,
}

/// MCP 相关业务逻辑（v3.7.0 统一结构）
pub struct McpService;

impl McpService {
    /// 获取所有 MCP 服务器（统一结构）
    pub fn get_all_servers(state: &AppState) -> Result<HashMap<String, McpServer>, AppError> {
        let cfg = state.config.read()?;

        // 如果是新结构，直接返回
        if let Some(servers) = &cfg.mcp.servers {
            return Ok(servers.clone());
        }

        // 理论上不应该走到这里，因为 load 时会自动迁移
        Err(AppError::localized(
            "mcp.old_structure",
            "检测到旧版 MCP 结构，请重启应用完成迁移",
            "Old MCP structure detected, please restart app to complete migration",
        ))
    }

    pub fn get_live_drift(state: &AppState, app: AppType) -> Result<McpLiveDriftReport, AppError> {
        let live_servers = match Self::read_live_mcp_servers(&app) {
            Ok(servers) => servers,
            Err(err) => {
                return Ok(McpLiveDriftReport {
                    app: app.clone(),
                    entries: vec![McpLiveDriftEntry {
                        app,
                        id: String::new(),
                        kind: McpLiveDriftKind::LiveInvalid,
                        db_spec: None,
                        live_spec: None,
                        message: Some(err.to_string()),
                    }],
                });
            }
        };

        let db_servers = Self::get_all_servers(state)?;
        let mut ids = BTreeSet::new();

        for id in live_servers.keys() {
            ids.insert(id.clone());
        }

        for (id, server) in &db_servers {
            if server.apps.is_enabled_for(&app) || live_servers.contains_key(id) {
                ids.insert(id.clone());
            }
        }

        let mut entries = Vec::new();
        for id in ids {
            let db_server = db_servers
                .get(&id)
                .filter(|server| server.apps.is_enabled_for(&app));
            let db_spec = db_server.map(|server| server.server.clone());
            let live_spec = live_servers.get(&id).cloned();

            let kind = match (&db_spec, &live_spec) {
                (Some(db), Some(live)) => {
                    if normalize_json_value(db) == normalize_json_value(live) {
                        McpLiveDriftKind::InSync
                    } else {
                        McpLiveDriftKind::Changed
                    }
                }
                (Some(_), None) => McpLiveDriftKind::DbOnly,
                (None, Some(_)) => McpLiveDriftKind::LiveOnly,
                (None, None) => continue,
            };

            entries.push(McpLiveDriftEntry {
                app: app.clone(),
                id,
                kind,
                db_spec,
                live_spec,
                message: None,
            });
        }

        Ok(McpLiveDriftReport { app, entries })
    }

    fn read_live_mcp_servers(app: &AppType) -> Result<HashMap<String, Value>, AppError> {
        match app {
            AppType::Codex => mcp::read_codex_live_mcp_servers_map(),
            _ => Ok(HashMap::new()),
        }
    }

    pub fn import_live_server(state: &AppState, app: AppType, id: &str) -> Result<(), AppError> {
        let live_servers = Self::read_live_mcp_servers(&app)?;
        let live_spec = live_servers.get(id).cloned().ok_or_else(|| {
            AppError::McpValidation(format!(
                "{} live MCP server '{}' not found",
                app.as_str(),
                id
            ))
        })?;

        {
            let mut cfg = state.config.write()?;
            let servers = cfg.mcp.servers.get_or_insert_with(HashMap::new);

            if let Some(existing) = servers.get_mut(id) {
                existing.server = live_spec;
                existing.apps.set_enabled_for(&app, true);
            } else {
                let mut apps = McpApps::default();
                apps.set_enabled_for(&app, true);
                servers.insert(
                    id.to_string(),
                    McpServer {
                        id: id.to_string(),
                        name: id.to_string(),
                        server: live_spec,
                        apps,
                        description: None,
                        homepage: None,
                        docs: None,
                        tags: Vec::new(),
                    },
                );
            }
        }

        state.save()?;
        Ok(())
    }

    pub fn push_db_server_to_live(
        state: &AppState,
        app: AppType,
        id: &str,
    ) -> Result<(), AppError> {
        let server = {
            let cfg = state.config.read()?;
            cfg.mcp
                .servers
                .as_ref()
                .and_then(|servers| servers.get(id))
                .cloned()
        }
        .ok_or_else(|| AppError::McpValidation(format!("MCP server '{id}' not found")))?;

        if !server.apps.is_enabled_for(&app) {
            return Err(AppError::McpValidation(format!(
                "MCP server '{}' is not enabled for {}",
                id,
                app.as_str()
            )));
        }

        Self::sync_server_to_app(state, &server, &app)
    }

    /// 添加或更新 MCP 服务器
    pub fn upsert_server(state: &AppState, server: McpServer) -> Result<(), AppError> {
        let (server_id, apps_to_remove) = {
            let mut cfg = state.config.write()?;

            let servers = cfg.mcp.servers.get_or_insert_with(HashMap::new);
            let server_id = server.id.clone();

            let apps_to_remove = servers
                .get(&server_id)
                .map(|existing| {
                    existing
                        .apps
                        .enabled_apps()
                        .into_iter()
                        .filter(|app| !server.apps.is_enabled_for(app))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            // 插入或更新
            servers.insert(server_id.clone(), server.clone());

            (server_id, apps_to_remove)
        };

        state.save()?;

        // 如果是更新：对“由启用变为禁用”的应用，清理对应 live 配置
        for app in apps_to_remove {
            Self::remove_server_from_app(state, &server_id, &app)?;
        }

        // 同步到各个启用的应用
        Self::sync_server_to_apps(state, &server)?;

        Ok(())
    }

    /// 删除 MCP 服务器
    pub fn delete_server(state: &AppState, id: &str) -> Result<bool, AppError> {
        let server = {
            let mut cfg = state.config.write()?;

            if let Some(servers) = &mut cfg.mcp.servers {
                servers.remove(id)
            } else {
                None
            }
        };

        if let Some(server) = server {
            state.save()?;

            // 从所有应用的 live 配置中移除
            Self::remove_server_from_all_apps(state, id, &server)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// 切换指定应用的启用状态
    pub fn toggle_app(
        state: &AppState,
        server_id: &str,
        app: AppType,
        enabled: bool,
    ) -> Result<(), AppError> {
        let server = {
            let mut cfg = state.config.write()?;

            if let Some(servers) = &mut cfg.mcp.servers {
                if let Some(server) = servers.get_mut(server_id) {
                    server.apps.set_enabled_for(&app, enabled);
                    Some(server.clone())
                } else {
                    None
                }
            } else {
                None
            }
        };

        if let Some(server) = server {
            state.save()?;

            // 同步到对应应用
            if enabled {
                Self::sync_server_to_app(state, &server, &app)?;
            } else {
                Self::remove_server_from_app(state, server_id, &app)?;
            }
        }

        Ok(())
    }

    /// 将 MCP 服务器同步到所有启用的应用
    fn sync_server_to_apps(state: &AppState, server: &McpServer) -> Result<(), AppError> {
        let cfg = state.config.read()?;

        for app in server.apps.enabled_apps() {
            Self::sync_server_to_app_internal(&cfg, server, &app)?;
        }

        Ok(())
    }

    /// 将 MCP 服务器同步到指定应用
    fn sync_server_to_app(
        state: &AppState,
        server: &McpServer,
        app: &AppType,
    ) -> Result<(), AppError> {
        let cfg = state.config.read()?;
        Self::sync_server_to_app_internal(&cfg, server, app)
    }

    fn sync_server_to_app_internal(
        cfg: &MultiAppConfig,
        server: &McpServer,
        app: &AppType,
    ) -> Result<(), AppError> {
        match app {
            AppType::Claude => {
                mcp::sync_single_server_to_claude(cfg, &server.id, &server.server)?;
            }
            AppType::Codex => {
                mcp::sync_single_server_to_codex(cfg, &server.id, &server.server)?;
            }
            AppType::Gemini => {
                mcp::sync_single_server_to_gemini(cfg, &server.id, &server.server)?;
            }
            AppType::OpenCode => {
                mcp::sync_single_server_to_opencode(cfg, &server.id, &server.server)?;
            }
            AppType::OpenClaw => {
                mcp::sync_single_server_to_openclaw(cfg, &server.id, &server.server)?;
            }
            AppType::Hermes => {
                mcp::sync_single_server_to_hermes(cfg, &server.id, &server.server)?;
            }
        }
        Ok(())
    }

    /// 从所有曾启用过该服务器的应用中移除
    fn remove_server_from_all_apps(
        state: &AppState,
        id: &str,
        server: &McpServer,
    ) -> Result<(), AppError> {
        // 从所有曾启用的应用中移除
        for app in server.apps.enabled_apps() {
            Self::remove_server_from_app(state, id, &app)?;
        }
        Ok(())
    }

    fn remove_server_from_app(_state: &AppState, id: &str, app: &AppType) -> Result<(), AppError> {
        match app {
            AppType::Claude => mcp::remove_server_from_claude(id)?,
            AppType::Codex => mcp::remove_server_from_codex(id)?,
            AppType::Gemini => mcp::remove_server_from_gemini(id)?,
            AppType::OpenCode => mcp::remove_server_from_opencode(id)?,
            AppType::OpenClaw => mcp::remove_server_from_openclaw(id)?,
            AppType::Hermes => mcp::remove_server_from_hermes(id)?,
        }
        Ok(())
    }

    /// 手动同步所有启用的 MCP 服务器到对应的应用
    pub fn sync_all_enabled(state: &AppState) -> Result<(), AppError> {
        let servers = Self::get_all_servers(state)?;

        for app in AppType::all() {
            for server in servers.values() {
                if server.apps.is_enabled_for(&app) {
                    Self::sync_server_to_app(state, server, &app)?;
                } else {
                    Self::remove_server_from_app(state, &server.id, &app)?;
                }
            }
        }

        Ok(())
    }

    // ========================================================================
    // 兼容层：支持旧的 v3.6.x 命令（已废弃，将在 v4.0 移除）
    // ========================================================================

    /// [已废弃] 获取指定应用的 MCP 服务器（兼容旧 API）
    #[deprecated(since = "3.7.0", note = "Use get_all_servers instead")]
    pub fn get_servers(
        state: &AppState,
        app: AppType,
    ) -> Result<HashMap<String, serde_json::Value>, AppError> {
        let all_servers = Self::get_all_servers(state)?;
        let mut result = HashMap::new();

        for (id, server) in all_servers {
            if server.apps.is_enabled_for(&app) {
                result.insert(id, server.server);
            }
        }

        Ok(result)
    }

    /// [已废弃] 设置 MCP 服务器在指定应用的启用状态（兼容旧 API）
    #[deprecated(since = "3.7.0", note = "Use toggle_app instead")]
    pub fn set_enabled(
        state: &AppState,
        app: AppType,
        id: &str,
        enabled: bool,
    ) -> Result<bool, AppError> {
        Self::toggle_app(state, id, app, enabled)?;
        Ok(true)
    }

    /// [已废弃] 同步启用的 MCP 到指定应用（兼容旧 API）
    #[deprecated(since = "3.7.0", note = "Use sync_all_enabled instead")]
    pub fn sync_enabled(state: &AppState, app: AppType) -> Result<(), AppError> {
        let servers = Self::get_all_servers(state)?;

        for server in servers.values() {
            if server.apps.is_enabled_for(&app) {
                Self::sync_server_to_app(state, server, &app)?;
            }
        }

        Ok(())
    }

    /// 从 Claude 导入 MCP（v3.7.0 已更新为统一结构）
    pub fn import_from_claude(state: &AppState) -> Result<usize, AppError> {
        let mut cfg = state.config.write()?;
        let count = mcp::import_from_claude(&mut cfg)?;
        drop(cfg);
        state.save()?;
        Ok(count)
    }

    /// 从 Codex 导入 MCP（v3.7.0 已更新为统一结构）
    pub fn import_from_codex(state: &AppState) -> Result<usize, AppError> {
        let mut cfg = state.config.write()?;
        let count = mcp::import_from_codex(&mut cfg)?;
        drop(cfg);
        state.save()?;
        Ok(count)
    }

    /// 从 Gemini 导入 MCP（v3.7.0 已更新为统一结构）
    pub fn import_from_gemini(state: &AppState) -> Result<usize, AppError> {
        let mut cfg = state.config.write()?;
        let count = mcp::import_from_gemini(&mut cfg)?;
        drop(cfg);
        state.save()?;
        Ok(count)
    }

    /// 从 OpenCode 导入 MCP
    pub fn import_from_opencode(state: &AppState) -> Result<usize, AppError> {
        let mut cfg = state.config.write()?;
        let count = mcp::import_from_opencode(&mut cfg)?;
        drop(cfg);
        state.save()?;
        Ok(count)
    }

    /// 从 OpenClaw 导入 MCP
    pub fn import_from_openclaw(state: &AppState) -> Result<usize, AppError> {
        let mut cfg = state.config.write()?;
        let count = mcp::import_from_openclaw(&mut cfg)?;
        drop(cfg);
        state.save()?;
        Ok(count)
    }

    /// 从 Hermes 导入 MCP
    pub fn import_from_hermes(state: &AppState) -> Result<usize, AppError> {
        let mut cfg = state.config.write()?;
        let count = mcp::import_from_hermes(&mut cfg)?;
        drop(cfg);
        state.save()?;
        Ok(count)
    }
}

fn normalize_json_value(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(normalize_json_value).collect()),
        Value::Object(map) => {
            let mut normalized = serde_json::Map::new();
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            for key in keys {
                if let Some(value) = map.get(key) {
                    normalized.insert(key.clone(), normalize_json_value(value));
                }
            }
            Value::Object(normalized)
        }
        _ => value.clone(),
    }
}
