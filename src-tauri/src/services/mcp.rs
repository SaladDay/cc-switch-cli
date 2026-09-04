use std::collections::HashMap;

use crate::app_config::{AppType, McpApps, McpServer, MultiAppConfig};
use crate::error::AppError;
use crate::store::AppState;

mod core_bridge;

/// MCP 相关业务逻辑（v3.7.0 统一结构）
pub struct McpService;

impl McpService {
    pub fn supported_mcp_apps() -> impl Iterator<Item = AppType> {
        AppType::all().filter(AppType::supports_mcp)
    }

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

    /// 添加或更新 MCP 服务器
    pub fn upsert_server(state: &AppState, server: McpServer) -> Result<(), AppError> {
        cc_switch_core::validate_mcp_server(&server.id, &server.server)
            .map_err(|error| AppError::McpValidation(error.to_string()))?;
        let mut cfg = state.config.write()?;
        let previous_server = cfg
            .mcp
            .servers
            .as_ref()
            .and_then(|servers| servers.get(&server.id))
            .cloned();
        let affected_apps = Self::supported_mcp_apps()
            .filter(|app| {
                server.apps.is_enabled_for(app)
                    || previous_server
                        .as_ref()
                        .is_some_and(|previous| previous.apps.is_enabled_for(app))
                    || previous_server.is_some()
                        && cc_switch_core::mcp_app_contract(&app.as_core())
                            .is_some_and(|contract| contract.preserves_disabled_entry())
            })
            .collect::<Vec<_>>();

        Self::commit_server_change(
            state,
            previous_server.as_ref(),
            Some(&server),
            &affected_apps,
        )?;
        cfg.mcp
            .servers
            .get_or_insert_with(HashMap::new)
            .insert(server.id.clone(), server);
        Ok(())
    }

    /// 删除 MCP 服务器
    pub fn delete_server(state: &AppState, id: &str) -> Result<bool, AppError> {
        let mut cfg = state.config.write()?;
        let server = cfg
            .mcp
            .servers
            .as_ref()
            .and_then(|servers| servers.get(id))
            .cloned();

        let Some(server) = server else {
            return Ok(false);
        };
        let affected_apps = Self::supported_mcp_apps().collect::<Vec<_>>();
        Self::commit_server_change(state, Some(&server), None, &affected_apps)?;
        cfg.mcp
            .servers
            .as_mut()
            .expect("server map exists")
            .remove(id);
        Ok(true)
    }

    /// 切换指定应用的启用状态
    pub fn toggle_app(
        state: &AppState,
        server_id: &str,
        app: AppType,
        enabled: bool,
    ) -> Result<(), AppError> {
        let mut cfg = state.config.write()?;
        let Some(previous_server) = cfg
            .mcp
            .servers
            .as_ref()
            .and_then(|servers| servers.get(server_id))
            .cloned()
        else {
            return Ok(());
        };
        if previous_server.apps.is_enabled_for(&app) == enabled {
            return Ok(());
        }
        let mut server = previous_server.clone();
        server.apps.set_enabled_for(&app, enabled);
        Self::commit_server_change(state, Some(&previous_server), Some(&server), &[app])?;
        cfg.mcp
            .servers
            .as_mut()
            .expect("server map exists")
            .insert(server_id.to_owned(), server);

        Ok(())
    }

    /// Replace the full supported-app matrix for one MCP server.
    pub fn set_apps(state: &AppState, server_id: &str, apps: McpApps) -> Result<bool, AppError> {
        let mut cfg = state.config.write()?;
        let Some(previous_server) = cfg
            .mcp
            .servers
            .as_ref()
            .and_then(|servers| servers.get(server_id))
            .cloned()
        else {
            return Ok(false);
        };
        let before = previous_server.apps.clone();
        if before == apps {
            return Ok(true);
        }
        let mut server = previous_server.clone();
        server.apps = apps;
        let changed_apps = Self::supported_mcp_apps()
            .filter(|app| before.is_enabled_for(app) != server.apps.is_enabled_for(app))
            .collect::<Vec<_>>();

        Self::commit_server_change(state, Some(&previous_server), Some(&server), &changed_apps)?;
        cfg.mcp
            .servers
            .as_mut()
            .expect("server map exists")
            .insert(server_id.to_owned(), server);
        Ok(true)
    }

    fn commit_server_change(
        state: &AppState,
        previous_server: Option<&McpServer>,
        next_server: Option<&McpServer>,
        affected_apps: &[AppType],
    ) -> Result<(), AppError> {
        state.db.commit_mcp_server_change(
            previous_server,
            next_server,
            affected_apps,
            |targets| core_bridge::apply_server_change(previous_server, next_server, targets),
            core_bridge::rollback_server_change,
        )
    }

    /// 将 MCP 服务器同步到指定应用
    pub(crate) fn project_server_for_app(
        id: &str,
        server: &serde_json::Value,
        app: &AppType,
    ) -> Result<(), AppError> {
        core_bridge::project_server(app, id, server)
    }

    pub(crate) fn remove_server_from_app(id: &str, app: &AppType) -> Result<(), AppError> {
        core_bridge::remove_server(app, id)
    }

    pub(crate) fn replace_servers_for_app(
        servers: &HashMap<String, serde_json::Value>,
        app: &AppType,
    ) -> Result<(), AppError> {
        core_bridge::replace_servers(app, servers)
    }

    /// 手动同步所有启用的 MCP 服务器到对应的应用。
    ///
    /// Best-effort：单个应用投影失败不阻断其余应用。各应用的 live 文件互相独立，
    /// 一处损坏没有理由让其它应用的 MCP 状态保持陈旧。全部执行完后聚合错误，
    /// 保留调用方对部分失败的可见性。
    pub fn sync_all_enabled(state: &AppState) -> Result<(), AppError> {
        Self::sync_all_enabled_except(state, None)
    }

    pub(crate) fn sync_all_enabled_except(
        state: &AppState,
        excluded: Option<&AppType>,
    ) -> Result<(), AppError> {
        let mut failures = Vec::new();
        for app in Self::supported_mcp_apps() {
            if excluded == Some(&app) {
                continue;
            }
            if let Err(err) = Self::sync_owned_servers_to_app(state, &app) {
                log::warn!("同步 MCP 到 {app:?} 失败: {err}");
                failures.push(format!("{}: {err}", app.as_str()));
            }
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(AppError::Message(format!(
                "部分应用 MCP 同步失败: {}",
                failures.join("; ")
            )))
        }
    }

    pub(crate) fn project_enabled_codex_text(
        state: &AppState,
        base_text: &str,
    ) -> Result<String, AppError> {
        let servers = state
            .db
            .get_owned_mcp_servers(&AppType::Codex)?
            .into_iter()
            .map(|target| (target.server.id.clone(), target.server))
            .collect();
        core_bridge::project_servers(&AppType::Codex, base_text, &servers)
    }

    /// 只把启用状态投影到单个应用。某个应用的 live 被整体重写后用它做
    /// 定向重投影，避免把无关应用的失败面牵连进目标应用的关键路径。
    pub fn sync_enabled_for_app(state: &AppState, app: &AppType) -> Result<(), AppError> {
        Self::sync_owned_servers_to_app(state, app)
    }

    pub(crate) fn sync_enabled_for_config(
        config: &MultiAppConfig,
        app: &AppType,
    ) -> Result<(), AppError> {
        Self::project_servers_to_app(Self::servers_from_config(config)?, app)
    }

    fn servers_from_config(
        config: &MultiAppConfig,
    ) -> Result<&HashMap<String, McpServer>, AppError> {
        config.mcp.servers.as_ref().ok_or_else(|| {
            AppError::localized(
                "mcp.old_structure",
                "检测到旧版 MCP 结构，请重启应用完成迁移",
                "Old MCP structure detected, please restart app to complete migration",
            )
        })
    }

    fn project_servers_to_app(
        servers: &HashMap<String, McpServer>,
        app: &AppType,
    ) -> Result<(), AppError> {
        core_bridge::sync_servers(app, servers)
    }

    fn sync_owned_servers_to_app(state: &AppState, app: &AppType) -> Result<(), AppError> {
        state.db.commit_mcp_app_sync(
            app,
            |servers| core_bridge::apply_owned_server_sync(app, servers),
            core_bridge::rollback_server_change,
        )
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
        Self::sync_owned_servers_to_app(state, &app)
    }

    /// 从 Claude 导入 MCP（v3.7.0 已更新为统一结构）
    pub fn import_from_claude(state: &AppState) -> Result<usize, AppError> {
        Self::import_from_app(state, AppType::Claude)
    }

    /// 从 Codex 导入 MCP（v3.7.0 已更新为统一结构）
    pub fn import_from_codex(state: &AppState) -> Result<usize, AppError> {
        Self::import_from_app(state, AppType::Codex)
    }

    /// 从 Gemini 导入 MCP（v3.7.0 已更新为统一结构）
    pub fn import_from_gemini(state: &AppState) -> Result<usize, AppError> {
        Self::import_from_app(state, AppType::Gemini)
    }

    /// 从 OpenCode 导入 MCP
    pub fn import_from_opencode(state: &AppState) -> Result<usize, AppError> {
        Self::import_from_app(state, AppType::OpenCode)
    }

    /// 从 Hermes 导入 MCP
    pub fn import_from_hermes(state: &AppState) -> Result<usize, AppError> {
        Self::import_from_app(state, AppType::Hermes)
    }

    pub fn import_from_supported_apps(state: &AppState) -> Result<usize, AppError> {
        let mut total = 0;
        total += Self::import_from_claude(state)?;
        total += Self::import_from_codex(state)?;
        total += Self::import_from_gemini(state)?;
        total += Self::import_from_opencode(state)?;
        total += Self::import_from_hermes(state)?;
        Ok(total)
    }

    fn import_from_app(state: &AppState, app: AppType) -> Result<usize, AppError> {
        let changed = state.db.import_native_mcp_servers(
            &app,
            || core_bridge::observe_imports(&app),
            core_bridge::ImportObservation::is_current,
        )?;
        let mut config = state.config.write()?;
        config.mcp.servers = Some(state.db.get_all_mcp_servers()?.into_iter().collect());
        Ok(changed)
    }
}
