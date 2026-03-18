mod claude;
mod codex;
#[cfg(test)]
mod codex_openai_auth_tests;
mod common;
mod endpoints;
mod gemini;
mod gemini_auth;
mod live;
mod models;
#[cfg(test)]
mod tests;
mod usage;

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app_config::{AppType, MultiAppConfig};
use crate::codex_config::{get_codex_auth_path, get_codex_config_path};
use crate::config::{
    copy_file, delete_file, get_claude_settings_path, get_provider_config_path, read_json_file,
    write_json_file,
};
use crate::error::AppError;
use crate::provider::{Provider, ProviderManager};
use crate::store::AppState;

use gemini_auth::GeminiAuthType;
use live::LiveSnapshot;

pub use common::migrate_legacy_codex_config;
use common::{
    is_codex_official_provider, merge_json_values, strip_codex_common_config_from_full_text,
    strip_common_values,
};

/// 供应商相关业务逻辑
pub struct ProviderService;

#[cfg(test)]
fn state_from_config(config: MultiAppConfig) -> AppState {
    let db = std::sync::Arc::new(crate::Database::memory().expect("create memory database"));
    AppState {
        db: db.clone(),
        config: std::sync::RwLock::new(config),
        proxy_service: crate::ProxyService::new(db),
    }
}

#[derive(Clone)]
struct PostCommitAction {
    app_type: AppType,
    provider: Provider,
    backup: LiveSnapshot,
    sync_mcp: bool,
    sync_all_additive_live: bool,
    refresh_snapshot: bool,
    common_config_snippet: Option<String>,
    takeover_active: bool,
}

impl ProviderService {
    fn parse_common_additive_config_snippet(
        snippet: &str,
        app_key: &str,
        app_display: &str,
    ) -> Result<Value, AppError> {
        let invalid_json_key = match app_key {
            "opencode" => "common_config.opencode.invalid_json",
            "openclaw" => "common_config.openclaw.invalid_json",
            _ => "common_config.additive.invalid_json",
        };
        let not_object_key = match app_key {
            "opencode" => "common_config.opencode.not_object",
            "openclaw" => "common_config.openclaw.not_object",
            _ => "common_config.additive.not_object",
        };
        let value: Value = serde_json::from_str(snippet).map_err(|e| {
            AppError::localized(
                invalid_json_key,
                format!("{app_display} 通用配置片段不是有效的 JSON：{e}"),
                format!("{app_display} common config snippet is not valid JSON: {e}"),
            )
        })?;
        if !value.is_object() {
            return Err(AppError::localized(
                not_object_key,
                format!("{app_display} 通用配置片段必须是 JSON 对象"),
                format!("{app_display} common config snippet must be a JSON object"),
            ));
        }
        Ok(value)
    }

    fn build_openclaw_settings_config_from_live(
        provider_id: &str,
        provider_config: Value,
        current_primary_model: Option<&str>,
    ) -> Value {
        let primary_model = current_primary_model
            .map(str::trim)
            .filter(|model| model.starts_with(&format!("{provider_id}/")))
            .map(str::to_string)
            .or_else(|| crate::openclaw_config::infer_primary_model(provider_id, &provider_config));

        crate::openclaw_config::build_settings_config(provider_config, primary_model)
    }

    fn strip_openclaw_primary_model(settings_config: &mut Value) {
        if let Some(settings) = settings_config.as_object_mut() {
            settings.remove("primaryModel");
        }
    }

    fn run_transaction<R, F>(state: &AppState, f: F) -> Result<R, AppError>
    where
        F: FnOnce(&mut MultiAppConfig) -> Result<(R, Option<PostCommitAction>), AppError>,
    {
        let mut guard = state.config.write().map_err(AppError::from)?;
        let original = guard.clone();
        let (result, action) = match f(&mut guard) {
            Ok(value) => value,
            Err(err) => {
                *guard = original;
                return Err(err);
            }
        };
        drop(guard);

        if let Err(save_err) = state.save() {
            if let Err(rollback_err) = Self::restore_config_only(state, original.clone()) {
                return Err(AppError::localized(
                    "config.save.rollback_failed",
                    format!("保存配置失败: {save_err}；回滚失败: {rollback_err}"),
                    format!("Failed to save config: {save_err}; rollback failed: {rollback_err}"),
                ));
            }
            return Err(save_err);
        }

        if let Some(action) = action {
            if let Err(err) = Self::apply_post_commit(state, &action) {
                if let Err(rollback_err) =
                    Self::rollback_after_failure(state, original.clone(), action.backup.clone())
                {
                    return Err(AppError::localized(
                        "post_commit.rollback_failed",
                        format!("后置操作失败: {err}；回滚失败: {rollback_err}"),
                        format!("Post-commit step failed: {err}; rollback failed: {rollback_err}"),
                    ));
                }
                return Err(err);
            }
        }

        Ok(result)
    }

    fn restore_config_only(state: &AppState, snapshot: MultiAppConfig) -> Result<(), AppError> {
        {
            let mut guard = state.config.write().map_err(AppError::from)?;
            *guard = snapshot;
        }
        state.save()
    }

    fn rollback_after_failure(
        state: &AppState,
        snapshot: MultiAppConfig,
        backup: LiveSnapshot,
    ) -> Result<(), AppError> {
        Self::restore_config_only(state, snapshot)?;
        backup.restore()
    }

    fn apply_post_commit(state: &AppState, action: &PostCommitAction) -> Result<(), AppError> {
        let apply_common_config = action
            .provider
            .meta
            .as_ref()
            .and_then(|meta| meta.apply_common_config)
            .unwrap_or(true);
        if action.takeover_active {
            let backup_snapshot = Self::build_live_backup_snapshot(
                &action.app_type,
                &action.provider,
                action.common_config_snippet.as_deref(),
                apply_common_config,
            )?;
            futures::executor::block_on(
                state
                    .proxy_service
                    .save_live_backup_snapshot(action.app_type.as_str(), &backup_snapshot),
            )
            .map_err(AppError::Message)?;
        } else if action.sync_all_additive_live {
            Self::sync_additive_app_to_live(state, &action.app_type)?;
        } else {
            Self::write_live_snapshot(
                &action.app_type,
                &action.provider,
                action.common_config_snippet.as_deref(),
                apply_common_config,
            )?;
        }
        if action.sync_mcp {
            // 使用 v3.7.0 统一的 MCP 同步机制，支持所有应用
            use crate::services::mcp::McpService;
            McpService::sync_all_enabled(state)?;
        }
        if !action.takeover_active
            && action.refresh_snapshot
            && crate::sync_policy::should_sync_live(&action.app_type)
        {
            Self::refresh_provider_snapshot(state, &action.app_type, &action.provider.id)?;
        }

        // D6: Align upstream live flows - also sync skills (best effort, should not block provider ops).
        if let Err(e) = crate::services::skill::SkillService::sync_all_enabled_best_effort() {
            log::warn!("同步 Skills 失败: {e}");
        }
        Ok(())
    }

    fn sync_additive_app_to_live(state: &AppState, app_type: &AppType) -> Result<(), AppError> {
        let snapshots: Vec<(Provider, Option<String>)> = {
            let guard = state.config.read().map_err(AppError::from)?;
            let Some(manager) = guard.get_manager(app_type) else {
                return Ok(());
            };
            let current_provider_id = manager.current.clone();
            let common_config_snippet = guard.common_config_snippets.get(app_type).cloned();

            manager
                .providers
                .values()
                .cloned()
                .map(|mut provider| {
                    if matches!(app_type, AppType::OpenClaw) && provider.id != current_provider_id {
                        Self::strip_openclaw_primary_model(&mut provider.settings_config);
                    }
                    (provider, common_config_snippet.clone())
                })
                .collect()
        };

        for (provider, common_config_snippet) in snapshots {
            let apply_common_config = provider
                .meta
                .as_ref()
                .and_then(|meta| meta.apply_common_config)
                .unwrap_or(true);
            Self::write_live_snapshot(
                app_type,
                &provider,
                common_config_snippet.as_deref(),
                apply_common_config,
            )?;
        }

        Ok(())
    }

    fn refresh_provider_snapshot(
        state: &AppState,
        app_type: &AppType,
        provider_id: &str,
    ) -> Result<(), AppError> {
        match app_type {
            AppType::Claude => {
                let settings_path = get_claude_settings_path();
                if !settings_path.exists() {
                    return Err(AppError::localized(
                        "claude.live.missing",
                        "Claude 设置文件不存在，无法刷新快照",
                        "Claude settings file missing; cannot refresh snapshot",
                    ));
                }
                let mut live_after = read_json_file::<Value>(&settings_path)?;
                let _ = Self::normalize_claude_models_in_value(&mut live_after);

                let common_snippet = {
                    let guard = state.config.read().map_err(AppError::from)?;
                    guard.common_config_snippets.claude.clone()
                };
                if let Some(snippet) = common_snippet.as_deref() {
                    let snippet = snippet.trim();
                    if !snippet.is_empty() {
                        let common = Self::parse_common_claude_config_snippet_for_strip(snippet)?;
                        strip_common_values(&mut live_after, &common);
                    }
                }
                {
                    let mut guard = state.config.write().map_err(AppError::from)?;
                    if let Some(manager) = guard.get_manager_mut(app_type) {
                        if let Some(target) = manager.providers.get_mut(provider_id) {
                            target.settings_config = live_after;
                        }
                    }
                }
                state.save()?;
            }
            AppType::Codex => {
                let auth_path = get_codex_auth_path();
                let auth = if auth_path.exists() {
                    Some(read_json_file::<Value>(&auth_path)?)
                } else {
                    None
                };
                let cfg_text = crate::codex_config::read_and_validate_codex_config_text()?;
                let common_snippet_extracted =
                    Self::extract_codex_common_config_from_config_toml(&cfg_text)?;
                let cfg_text_for_storage =
                    Self::strip_codex_mcp_servers_from_snapshot_config(&cfg_text)?;

                let (provider, common_snippet_for_strip) = {
                    let guard = state.config.read().map_err(AppError::from)?;
                    (
                        guard
                            .get_manager(app_type)
                            .and_then(|manager| manager.providers.get(provider_id))
                            .cloned()
                            .ok_or_else(|| {
                                AppError::localized(
                                    "provider.not_found",
                                    format!("供应商不存在: {provider_id}"),
                                    format!("Provider not found: {provider_id}"),
                                )
                            })?,
                        guard.common_config_snippets.codex.clone(),
                    )
                };
                let effective_common_snippet = if common_snippet_for_strip
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .is_empty()
                    && !common_snippet_extracted.trim().is_empty()
                {
                    Some(common_snippet_extracted.clone())
                } else {
                    common_snippet_for_strip.clone()
                };

                let mut raw_settings = serde_json::Map::new();
                if let Some(auth) = auth {
                    raw_settings.insert("auth".to_string(), auth);
                }
                raw_settings.insert("config".to_string(), Value::String(cfg_text_for_storage));
                let settings_to_store = Self::normalize_settings_config_for_storage(
                    app_type,
                    &provider,
                    Value::Object(raw_settings),
                    effective_common_snippet.as_deref(),
                )?;

                {
                    let mut guard = state.config.write().map_err(AppError::from)?;
                    if !common_snippet_extracted.trim().is_empty()
                        && guard
                            .common_config_snippets
                            .codex
                            .as_deref()
                            .unwrap_or_default()
                            .trim()
                            .is_empty()
                    {
                        guard.common_config_snippets.codex = Some(common_snippet_extracted.clone());
                        Self::normalize_existing_provider_snapshots_for_storage_best_effort(
                            &mut guard,
                            app_type,
                            Some(common_snippet_extracted.as_str()),
                        );
                    }
                    if let Some(manager) = guard.get_manager_mut(app_type) {
                        if let Some(target) = manager.providers.get_mut(provider_id) {
                            target.settings_config = settings_to_store.clone();
                        }
                    }
                }
                state.save()?;
            }
            AppType::Gemini => {
                use crate::gemini_config::{
                    env_to_json, get_gemini_env_path, get_gemini_settings_path, read_gemini_env,
                };

                let env_path = get_gemini_env_path();
                if !env_path.exists() {
                    return Err(AppError::localized(
                        "gemini.live.missing",
                        "Gemini .env 文件不存在，无法刷新快照",
                        "Gemini .env file missing; cannot refresh snapshot",
                    ));
                }
                let env_map = read_gemini_env()?;
                let mut live_after = env_to_json(&env_map);

                let settings_path = get_gemini_settings_path();
                let config_value = if settings_path.exists() {
                    read_json_file(&settings_path)?
                } else {
                    json!({})
                };

                if let Some(obj) = live_after.as_object_mut() {
                    obj.insert("config".to_string(), config_value);
                }

                let (provider, common_snippet) = {
                    let guard = state.config.read().map_err(AppError::from)?;
                    (
                        guard
                            .get_manager(app_type)
                            .and_then(|manager| manager.providers.get(provider_id))
                            .cloned()
                            .ok_or_else(|| {
                                AppError::localized(
                                    "provider.not_found",
                                    format!("供应商不存在: {provider_id}"),
                                    format!("Provider not found: {provider_id}"),
                                )
                            })?,
                        guard.common_config_snippets.gemini.clone(),
                    )
                };
                let live_after = Self::normalize_settings_config_for_storage(
                    app_type,
                    &provider,
                    live_after,
                    common_snippet.as_deref(),
                )?;

                {
                    let mut guard = state.config.write().map_err(AppError::from)?;
                    if let Some(manager) = guard.get_manager_mut(app_type) {
                        if let Some(target) = manager.providers.get_mut(provider_id) {
                            target.settings_config = live_after;
                        }
                    }
                }
                state.save()?;
            }
            AppType::OpenCode => {
                let providers = crate::opencode_config::get_providers()?;
                let live_after = providers.get(provider_id).cloned().ok_or_else(|| {
                    AppError::localized(
                        "opencode.live.missing_provider",
                        format!("OpenCode live 配置中缺少供应商: {provider_id}"),
                        format!("OpenCode live config missing provider: {provider_id}"),
                    )
                })?;

                {
                    let mut guard = state.config.write().map_err(AppError::from)?;
                    if let Some(manager) = guard.get_manager_mut(app_type) {
                        if let Some(target) = manager.providers.get_mut(provider_id) {
                            target.settings_config = live_after;
                        }
                    }
                }
                state.save()?;
            }
            AppType::OpenClaw => {
                let providers = crate::openclaw_config::get_providers()?;
                let current_primary = crate::openclaw_config::get_primary_model()?;
                let live_after = providers.get(provider_id).cloned().ok_or_else(|| {
                    AppError::localized(
                        "openclaw.live.missing_provider",
                        format!("OpenClaw live 配置中缺少供应商: {provider_id}"),
                        format!("OpenClaw live config missing provider: {provider_id}"),
                    )
                })?;

                let snapshot = Self::build_openclaw_settings_config_from_live(
                    provider_id,
                    live_after,
                    current_primary.as_deref(),
                );

                {
                    let mut guard = state.config.write().map_err(AppError::from)?;
                    if let Some(manager) = guard.get_manager_mut(app_type) {
                        if let Some(target) = manager.providers.get_mut(provider_id) {
                            target.settings_config = snapshot;
                        }
                    }
                }
                state.save()?;
            }
        }
        Ok(())
    }

    fn capture_live_snapshot(app_type: &AppType) -> Result<LiveSnapshot, AppError> {
        live::capture_live_snapshot(app_type)
    }

    fn validate_common_config_snippet(
        app_type: &AppType,
        snippet: Option<&str>,
    ) -> Result<(), AppError> {
        let Some(snippet) = snippet.map(str::trim) else {
            return Ok(());
        };
        if snippet.is_empty() {
            return Ok(());
        }

        match app_type {
            AppType::Claude => {
                Self::parse_common_claude_config_snippet(snippet)?;
            }
            AppType::Codex => {
                snippet.parse::<toml_edit::DocumentMut>().map_err(|e| {
                    AppError::Config(format!("Common config TOML parse error: {e}"))
                })?;
            }
            AppType::Gemini => {
                Self::parse_common_gemini_config_snippet(snippet)?;
            }
            AppType::OpenCode => {
                Self::parse_common_additive_config_snippet(snippet, "opencode", "OpenCode")?;
            }
            AppType::OpenClaw => {
                Self::parse_common_additive_config_snippet(snippet, "openclaw", "OpenClaw")?;
            }
        }

        Ok(())
    }

    fn should_skip_common_config_migration_error(app_type: &AppType, err: &AppError) -> bool {
        match (app_type, err) {
            (AppType::Claude, AppError::Localized { key, .. }) => {
                key.starts_with("common_config.claude.")
            }
            (AppType::Codex, AppError::Config(message)) => {
                message.starts_with("Common config TOML parse error:")
            }
            (AppType::Gemini, AppError::Localized { key, .. }) => {
                key.starts_with("common_config.gemini.")
            }
            _ => false,
        }
    }

    fn migrate_old_common_config_snippet_best_effort(
        config: &mut MultiAppConfig,
        app_type: &AppType,
        old_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        let Some(old_snippet) = old_snippet.map(str::trim) else {
            return Ok(());
        };
        if old_snippet.is_empty() {
            return Ok(());
        }

        let result = match app_type {
            AppType::Claude => Self::migrate_claude_common_config_snippet(config, old_snippet),
            AppType::Codex => Self::migrate_codex_common_config_snippet(config, old_snippet),
            AppType::Gemini => Self::migrate_gemini_common_config_snippet(config, old_snippet),
            AppType::OpenCode => Ok(()),
            AppType::OpenClaw => Ok(()),
        };

        match result {
            Ok(()) => Ok(()),
            Err(err) if Self::should_skip_common_config_migration_error(app_type, &err) => {
                log::warn!(
                    "skip migrating {app_type} provider snapshots from invalid stored common config snippet: {err}"
                );
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    fn build_common_config_post_commit_action(
        config: &mut MultiAppConfig,
        app_type: &AppType,
        takeover_active: bool,
    ) -> Result<Option<PostCommitAction>, AppError> {
        if matches!(app_type, AppType::OpenCode) {
            return Ok(None);
        }

        let Some(current_provider_id) =
            Self::self_heal_current_provider(config, app_type, "set_common_config_snippet")
        else {
            return Ok(None);
        };

        Self::build_post_commit_action_for_current_provider(
            config,
            app_type,
            &current_provider_id,
            takeover_active,
        )
    }

    fn build_post_commit_action_for_current_provider(
        config: &MultiAppConfig,
        app_type: &AppType,
        current_provider_id: &str,
        takeover_active: bool,
    ) -> Result<Option<PostCommitAction>, AppError> {
        let provider = config
            .get_manager(app_type)
            .and_then(|manager| manager.providers.get(current_provider_id).cloned());

        let Some(provider) = provider else {
            return Ok(None);
        };

        Ok(Some(PostCommitAction {
            app_type: app_type.clone(),
            provider,
            backup: Self::capture_live_snapshot(app_type)?,
            sync_mcp: matches!(app_type, AppType::Codex) && !takeover_active,
            sync_all_additive_live: false,
            refresh_snapshot: false,
            common_config_snippet: config.common_config_snippets.get(app_type).cloned(),
            takeover_active,
        }))
    }

    fn normalize_provider_for_storage(
        app_type: &AppType,
        provider: &mut Provider,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        match app_type {
            AppType::Claude => {
                Self::strip_common_claude_config_from_provider(provider, common_config_snippet)?;
            }
            AppType::Codex => {
                Self::strip_common_codex_config_from_provider(provider, common_config_snippet)?;
            }
            AppType::Gemini => {
                Self::strip_common_gemini_config_from_provider(provider, common_config_snippet)?;
            }
            AppType::OpenCode => {}
            AppType::OpenClaw => {}
        }

        Ok(())
    }

    fn normalize_settings_config_for_storage(
        app_type: &AppType,
        provider: &Provider,
        settings_config: Value,
        common_config_snippet: Option<&str>,
    ) -> Result<Value, AppError> {
        let mut snapshot_provider = provider.clone();
        snapshot_provider.settings_config = settings_config;
        Self::normalize_provider_for_storage(
            app_type,
            &mut snapshot_provider,
            common_config_snippet,
        )?;
        Ok(snapshot_provider.settings_config)
    }

    fn normalize_existing_provider_snapshots_for_storage(
        config: &mut MultiAppConfig,
        app_type: &AppType,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        let Some(manager) = config.get_manager_mut(app_type) else {
            return Ok(());
        };

        for provider in manager.providers.values_mut() {
            Self::normalize_provider_for_storage(app_type, provider, common_config_snippet)?;
        }

        Ok(())
    }

    fn normalize_existing_provider_snapshots_for_storage_best_effort(
        config: &mut MultiAppConfig,
        app_type: &AppType,
        common_config_snippet: Option<&str>,
    ) {
        let Some(manager) = config.get_manager_mut(app_type) else {
            return;
        };

        for (provider_id, provider) in manager.providers.iter_mut() {
            if let Err(err) =
                Self::normalize_provider_for_storage(app_type, provider, common_config_snippet)
            {
                log::warn!(
                    "skip normalizing {app_type} provider snapshot '{provider_id}' while applying auto-extracted common config: {err}"
                );
            }
        }
    }

    fn normalize_existing_provider_snapshots_for_storage_strict_current_best_effort_others(
        config: &mut MultiAppConfig,
        app_type: &AppType,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        let Some(current_provider_id) = config.get_manager(app_type).and_then(|manager| {
            if manager.current.is_empty() || !manager.providers.contains_key(&manager.current) {
                None
            } else {
                Some(manager.current.clone())
            }
        }) else {
            return Self::normalize_existing_provider_snapshots_for_storage(
                config,
                app_type,
                common_config_snippet,
            );
        };

        let Some(manager) = config.get_manager_mut(app_type) else {
            return Ok(());
        };

        if let Some(current_provider) = manager.providers.get_mut(&current_provider_id) {
            Self::normalize_provider_for_storage(
                app_type,
                current_provider,
                common_config_snippet,
            )?;
        }

        for (provider_id, provider) in manager.providers.iter_mut() {
            if provider_id == &current_provider_id {
                continue;
            }

            if let Err(err) =
                Self::normalize_provider_for_storage(app_type, provider, common_config_snippet)
            {
                log::warn!(
                    "skip normalizing {app_type} non-current provider snapshot '{provider_id}' while updating common config snippet: {err}"
                );
            }
        }

        Ok(())
    }

    fn self_heal_current_provider(
        config: &mut MultiAppConfig,
        app_type: &AppType,
        log_context: &str,
    ) -> Option<String> {
        let manager = config.get_manager_mut(app_type)?;

        if manager.providers.is_empty() {
            manager.current.clear();
            return None;
        }

        if manager.current.is_empty() || !manager.providers.contains_key(&manager.current) {
            let previous_current = manager.current.clone();
            manager.current = Self::fallback_current_provider_id(manager);
            if manager.current.is_empty() {
                return None;
            }
            log::warn!(
                "{log_context}: {app_type} current provider '{}' is invalid, self-healed to '{}'",
                previous_current,
                manager.current
            );
        }

        Some(manager.current.clone())
    }

    fn fallback_current_provider_id(manager: &ProviderManager) -> String {
        let mut provider_list: Vec<_> = manager.providers.iter().collect();
        provider_list.sort_by(|(_, a), (_, b)| match (a.sort_index, b.sort_index) {
            (Some(idx_a), Some(idx_b)) => idx_a.cmp(&idx_b),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.created_at.cmp(&b.created_at),
        });

        provider_list
            .first()
            .map(|(id, _)| (*id).clone())
            .unwrap_or_default()
    }

    pub fn set_common_config_snippet(
        state: &AppState,
        app_type: AppType,
        snippet: Option<String>,
    ) -> Result<(), AppError> {
        let normalized_snippet = snippet.and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        });
        Self::validate_common_config_snippet(&app_type, normalized_snippet.as_deref())?;

        let app_type_clone = app_type.clone();
        let takeover_active = if app_type.is_additive_mode() {
            false
        } else {
            let is_running = state
                .proxy_service
                .is_running_blocking()
                .map_err(AppError::Message)?;
            if !is_running {
                false
            } else {
                state
                    .proxy_service
                    .is_app_takeover_active_blocking(&app_type)
                    .map_err(AppError::Message)?
            }
        };

        Self::run_transaction(state, move |config| {
            config.ensure_app(&app_type_clone);

            if !app_type_clone.is_additive_mode() {
                Self::self_heal_current_provider(
                    config,
                    &app_type_clone,
                    "set_common_config_snippet",
                );
            }

            let old_snippet = config
                .common_config_snippets
                .get(&app_type_clone)
                .cloned()
                .filter(|value| !value.trim().is_empty());

            Self::migrate_old_common_config_snippet_best_effort(
                config,
                &app_type_clone,
                old_snippet.as_deref(),
            )?;

            config
                .common_config_snippets
                .set(&app_type_clone, normalized_snippet.clone());

            if matches!(
                app_type_clone,
                AppType::Claude | AppType::Codex | AppType::Gemini
            ) {
                Self::normalize_existing_provider_snapshots_for_storage_strict_current_best_effort_others(
                    config,
                    &app_type_clone,
                    normalized_snippet.as_deref(),
                )?;
            }

            let action = Self::build_common_config_post_commit_action(
                config,
                &app_type_clone,
                takeover_active,
            )?;
            Ok(((), action))
        })
    }

    pub fn clear_common_config_snippet(
        state: &AppState,
        app_type: AppType,
    ) -> Result<(), AppError> {
        Self::set_common_config_snippet(state, app_type, None)
    }

    /// 列出指定应用下的所有供应商
    pub fn list(
        state: &AppState,
        app_type: AppType,
    ) -> Result<IndexMap<String, Provider>, AppError> {
        let config = state.config.read().map_err(AppError::from)?;
        let manager = config
            .get_manager(&app_type)
            .ok_or_else(|| Self::app_not_found(&app_type))?;
        Ok(manager.get_all_providers().clone())
    }

    /// 获取当前供应商 ID
    pub fn current(state: &AppState, app_type: AppType) -> Result<String, AppError> {
        if matches!(app_type, AppType::OpenCode) {
            return Ok(String::new());
        }

        {
            let config = state.config.read().map_err(AppError::from)?;
            let manager = config
                .get_manager(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;

            if manager.current.is_empty() || manager.providers.contains_key(&manager.current) {
                return Ok(manager.current.clone());
            }
        }

        let app_type_clone = app_type.clone();
        Self::run_transaction(state, move |config| {
            let manager = config
                .get_manager_mut(&app_type_clone)
                .ok_or_else(|| Self::app_not_found(&app_type_clone))?;

            if manager.current.is_empty() || manager.providers.contains_key(&manager.current) {
                return Ok((manager.current.clone(), None));
            }

            manager.current = Self::fallback_current_provider_id(manager);

            Ok((manager.current.clone(), None))
        })
    }

    /// 新增供应商
    pub fn add(state: &AppState, app_type: AppType, provider: Provider) -> Result<bool, AppError> {
        let mut provider = provider;
        // 归一化 Claude 模型键
        Self::normalize_provider_if_claude(&app_type, &mut provider);
        Self::validate_provider_settings(&app_type, &provider)?;

        let app_type_clone = app_type.clone();
        let provider_clone = provider.clone();

        Self::run_transaction(state, move |config| {
            let common_config_snippet = config.common_config_snippets.get(&app_type_clone).cloned();
            let mut provider_to_store = provider_clone.clone();
            Self::normalize_provider_for_storage(
                &app_type_clone,
                &mut provider_to_store,
                common_config_snippet.as_deref(),
            )?;

            config.ensure_app(&app_type_clone);
            let previous_current = config
                .get_manager(&app_type_clone)
                .map(|manager| manager.current.clone())
                .unwrap_or_default();
            let healed_current = if !matches!(app_type_clone, AppType::OpenCode) {
                Self::self_heal_current_provider(config, &app_type_clone, "add")
            } else {
                None
            };
            let manager = config
                .get_manager_mut(&app_type_clone)
                .ok_or_else(|| Self::app_not_found(&app_type_clone))?;

            let was_empty = manager.providers.is_empty();
            manager
                .providers
                .insert(provider_to_store.id.clone(), provider_to_store.clone());

            if !matches!(app_type_clone, AppType::OpenCode)
                && was_empty
                && manager.current.is_empty()
            {
                manager.current = provider_to_store.id.clone();
            }

            let is_current = matches!(app_type_clone, AppType::OpenCode)
                || manager.current == provider_to_store.id;
            let current_was_healed = !matches!(app_type_clone, AppType::OpenCode)
                && healed_current.as_deref() != Some(previous_current.as_str());
            let current_provider_id = if current_was_healed && !is_current {
                Some(manager.current.clone())
            } else {
                None
            };
            let action = if is_current || matches!(app_type_clone, AppType::OpenClaw) {
                let backup = Self::capture_live_snapshot(&app_type_clone)?;
                let mut action_provider = provider_to_store.clone();
                if matches!(app_type_clone, AppType::OpenClaw) && !is_current {
                    Self::strip_openclaw_primary_model(&mut action_provider.settings_config);
                }
                Some(PostCommitAction {
                    app_type: app_type_clone.clone(),
                    provider: action_provider,
                    backup,
                    // Codex current-provider saves rewrite live config from the stored snapshot,
                    // so managed MCP must be synced back after the write.
                    sync_mcp: matches!(&app_type_clone, AppType::Codex),
                    sync_all_additive_live: false,
                    refresh_snapshot: false,
                    common_config_snippet,
                    takeover_active: false,
                })
            } else if let Some(current_provider_id) = current_provider_id {
                Self::build_post_commit_action_for_current_provider(
                    config,
                    &app_type_clone,
                    &current_provider_id,
                    false,
                )?
            } else {
                None
            };

            Ok((true, action))
        })
    }

    /// 更新供应商
    pub fn update(
        state: &AppState,
        app_type: AppType,
        provider: Provider,
    ) -> Result<bool, AppError> {
        let mut provider = provider;
        // 归一化 Claude 模型键
        Self::normalize_provider_if_claude(&app_type, &mut provider);
        Self::validate_provider_settings(&app_type, &provider)?;
        let provider_id = provider.id.clone();
        let app_type_clone = app_type.clone();
        let provider_clone = provider.clone();

        Self::run_transaction(state, move |config| {
            let common_config_snippet = config.common_config_snippets.get(&app_type_clone).cloned();
            let previous_current = config
                .get_manager(&app_type_clone)
                .map(|manager| manager.current.clone())
                .unwrap_or_default();
            let healed_current = if !matches!(app_type_clone, AppType::OpenCode) {
                Self::self_heal_current_provider(config, &app_type_clone, "update")
            } else {
                None
            };
            let manager = config
                .get_manager_mut(&app_type_clone)
                .ok_or_else(|| Self::app_not_found(&app_type_clone))?;

            if !manager.providers.contains_key(&provider_id) {
                return Err(AppError::localized(
                    "provider.not_found",
                    format!("供应商不存在: {provider_id}"),
                    format!("Provider not found: {provider_id}"),
                ));
            }

            let is_current =
                matches!(app_type_clone, AppType::OpenCode) || manager.current == provider_id;
            let mut merged = if let Some(existing) = manager.providers.get(&provider_id) {
                let mut updated = provider_clone.clone();
                match (existing.meta.as_ref(), updated.meta.take()) {
                    // 前端未提供 meta，表示不修改，沿用旧值
                    (Some(old_meta), None) => {
                        updated.meta = Some(old_meta.clone());
                    }
                    (None, None) => {
                        updated.meta = None;
                    }
                    // 前端提供的 meta 视为权威，直接覆盖（其中 custom_endpoints 允许是空，表示删除所有自定义端点）
                    (_old, Some(new_meta)) => {
                        updated.meta = Some(new_meta);
                    }
                }
                updated
            } else {
                provider_clone.clone()
            };

            Self::normalize_provider_for_storage(
                &app_type_clone,
                &mut merged,
                common_config_snippet.as_deref(),
            )?;

            manager
                .providers
                .insert(provider_id.clone(), merged.clone());

            let current_was_healed = !matches!(app_type_clone, AppType::OpenCode)
                && healed_current.as_deref() != Some(previous_current.as_str());
            let current_provider_id = if current_was_healed && !is_current {
                Some(manager.current.clone())
            } else {
                None
            };
            let action = if is_current || matches!(app_type_clone, AppType::OpenClaw) {
                let backup = Self::capture_live_snapshot(&app_type_clone)?;
                let mut action_provider = merged.clone();
                if matches!(app_type_clone, AppType::OpenClaw) && !is_current {
                    Self::strip_openclaw_primary_model(&mut action_provider.settings_config);
                }
                Some(PostCommitAction {
                    app_type: app_type_clone.clone(),
                    provider: action_provider,
                    backup,
                    // Codex current-provider saves rewrite live config from the stored snapshot,
                    // so managed MCP must be synced back after the write.
                    sync_mcp: matches!(&app_type_clone, AppType::Codex),
                    sync_all_additive_live: false,
                    refresh_snapshot: false,
                    common_config_snippet,
                    takeover_active: false,
                })
            } else if let Some(current_provider_id) = current_provider_id {
                Self::build_post_commit_action_for_current_provider(
                    config,
                    &app_type_clone,
                    &current_provider_id,
                    false,
                )?
            } else {
                None
            };

            Ok((true, action))
        })
    }

    /// 导入当前 live 配置为默认供应商
    pub fn import_default_config(state: &AppState, app_type: AppType) -> Result<(), AppError> {
        if app_type.is_additive_mode() {
            let providers = match app_type {
                AppType::OpenCode => crate::opencode_config::get_providers()?,
                AppType::OpenClaw => crate::openclaw_config::get_providers()?,
                _ => serde_json::Map::new(),
            };
            if providers.is_empty() {
                return Ok(());
            }

            let current_primary_model = if matches!(app_type, AppType::OpenClaw) {
                crate::openclaw_config::get_primary_model()?
            } else {
                None
            };
            let current_provider_id = current_primary_model
                .as_deref()
                .and_then(|model| model.split('/').next().map(str::to_string));

            {
                let mut config = state.config.write().map_err(AppError::from)?;
                config.ensure_app(&app_type);
                let manager = config
                    .get_manager_mut(&app_type)
                    .ok_or_else(|| Self::app_not_found(&app_type))?;

                if !manager.get_all_providers().is_empty() {
                    return Ok(());
                }

                for (id, settings_config) in providers {
                    let snapshot = if matches!(app_type, AppType::OpenClaw) {
                        Self::build_openclaw_settings_config_from_live(
                            &id,
                            settings_config.clone(),
                            current_primary_model.as_deref(),
                        )
                    } else {
                        settings_config.clone()
                    };
                    let provider_view = if matches!(app_type, AppType::OpenClaw) {
                        crate::openclaw_config::provider_config_from_settings(&snapshot)
                    } else {
                        settings_config.clone()
                    };
                    let name = provider_view
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(&id)
                        .to_string();
                    manager
                        .providers
                        .insert(id.clone(), Provider::with_id(id, name, snapshot, None));
                }

                if let Some(current_provider_id) = current_provider_id {
                    manager.current = current_provider_id;
                }
            }

            state.save()?;
            return Ok(());
        }

        {
            let config = state.config.read().map_err(AppError::from)?;
            if let Some(manager) = config.get_manager(&app_type) {
                if !manager.get_all_providers().is_empty() {
                    return Ok(());
                }
            }
        }

        let settings_config = match app_type {
            AppType::Codex => {
                let auth_path = get_codex_auth_path();
                if !auth_path.exists() {
                    return Err(AppError::localized(
                        "codex.live.missing",
                        "Codex 配置文件不存在",
                        "Codex configuration file is missing",
                    ));
                }
                let auth: Value = read_json_file(&auth_path)?;
                let config_str = crate::codex_config::read_and_validate_codex_config_text()?;
                json!({ "auth": auth, "config": config_str })
            }
            AppType::Claude => {
                let settings_path = get_claude_settings_path();
                if !settings_path.exists() {
                    return Err(AppError::localized(
                        "claude.live.missing",
                        "Claude Code 配置文件不存在",
                        "Claude settings file is missing",
                    ));
                }
                let mut v = read_json_file::<Value>(&settings_path)?;
                let _ = Self::normalize_claude_models_in_value(&mut v);
                v
            }
            AppType::Gemini => {
                use crate::gemini_config::{
                    env_to_json, get_gemini_env_path, get_gemini_settings_path, read_gemini_env,
                };

                // 读取 .env 文件（环境变量）
                let env_path = get_gemini_env_path();
                if !env_path.exists() {
                    return Err(AppError::localized(
                        "gemini.live.missing",
                        "Gemini 配置文件不存在",
                        "Gemini configuration file is missing",
                    ));
                }

                let env_map = read_gemini_env()?;
                let env_json = env_to_json(&env_map);
                let env_obj = env_json.get("env").cloned().unwrap_or_else(|| json!({}));

                // 读取 settings.json 文件（MCP 配置等）
                let settings_path = get_gemini_settings_path();
                let config_obj = if settings_path.exists() {
                    read_json_file(&settings_path)?
                } else {
                    json!({})
                };

                // 返回完整结构：{ "env": {...}, "config": {...} }
                json!({
                    "env": env_obj,
                    "config": config_obj
                })
            }
            AppType::OpenCode => unreachable!("additive mode apps are handled earlier"),
            AppType::OpenClaw => unreachable!("additive mode apps are handled earlier"),
        };

        let mut provider = Provider::with_id(
            "default".to_string(),
            "default".to_string(),
            settings_config,
            None,
        );
        provider.category = Some("custom".to_string());

        let common_config_snippet = {
            let config = state.config.read().map_err(AppError::from)?;
            config.common_config_snippets.get(&app_type).cloned()
        };
        provider.settings_config = Self::normalize_settings_config_for_storage(
            &app_type,
            &provider,
            provider.settings_config.clone(),
            common_config_snippet.as_deref(),
        )?;

        {
            let mut config = state.config.write().map_err(AppError::from)?;
            config.ensure_app(&app_type);
            let manager = config
                .get_manager_mut(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;
            manager
                .providers
                .insert(provider.id.clone(), provider.clone());
            manager.current = provider.id.clone();
        }

        state.save()?;
        Ok(())
    }

    /// 读取当前 live 配置
    pub fn read_live_settings(app_type: AppType) -> Result<Value, AppError> {
        match app_type {
            AppType::Codex => {
                let auth_path = get_codex_auth_path();
                let config_path = get_codex_config_path();
                if !config_path.exists() {
                    return Err(AppError::localized(
                        "codex.live.missing",
                        "Codex 配置文件不存在",
                        "Codex configuration is missing",
                    ));
                }

                let mut live_settings = serde_json::Map::new();
                if auth_path.exists() {
                    live_settings.insert("auth".to_string(), read_json_file(&auth_path)?);
                }
                if config_path.exists() {
                    let cfg_text = crate::codex_config::read_and_validate_codex_config_text()?;
                    live_settings.insert("config".to_string(), Value::String(cfg_text));
                }

                Ok(Value::Object(live_settings))
            }
            AppType::Claude => {
                let path = get_claude_settings_path();
                if !path.exists() {
                    return Err(AppError::localized(
                        "claude.live.missing",
                        "Claude Code 配置文件不存在",
                        "Claude settings file is missing",
                    ));
                }
                read_json_file(&path)
            }
            AppType::Gemini => {
                use crate::gemini_config::{
                    env_to_json, get_gemini_env_path, get_gemini_settings_path, read_gemini_env,
                };

                // 读取 .env 文件（环境变量）
                let env_path = get_gemini_env_path();
                if !env_path.exists() {
                    return Err(AppError::localized(
                        "gemini.env.missing",
                        "Gemini .env 文件不存在",
                        "Gemini .env file not found",
                    ));
                }

                let env_map = read_gemini_env()?;
                let env_json = env_to_json(&env_map);
                let env_obj = env_json.get("env").cloned().unwrap_or_else(|| json!({}));

                // 读取 settings.json 文件（MCP 配置等）
                let settings_path = get_gemini_settings_path();
                let config_obj = if settings_path.exists() {
                    read_json_file(&settings_path)?
                } else {
                    json!({})
                };

                // 返回完整结构：{ "env": {...}, "config": {...} }
                Ok(json!({
                    "env": env_obj,
                    "config": config_obj
                }))
            }
            AppType::OpenCode => {
                let config_path = crate::opencode_config::get_opencode_config_path();
                if !config_path.exists() {
                    return Err(AppError::localized(
                        "opencode.config.missing",
                        "OpenCode 配置文件不存在",
                        "OpenCode configuration file not found",
                    ));
                }
                crate::opencode_config::read_opencode_config()
            }
            AppType::OpenClaw => {
                let config_path = crate::openclaw_config::get_openclaw_config_path();
                if !config_path.exists() {
                    return Err(AppError::localized(
                        "openclaw.config.missing",
                        "OpenClaw 配置文件不存在",
                        "OpenClaw configuration file not found",
                    ));
                }
                crate::openclaw_config::read_openclaw_config()
            }
        }
    }

    /// 更新供应商排序
    pub fn update_sort_order(
        state: &AppState,
        app_type: AppType,
        updates: Vec<ProviderSortUpdate>,
    ) -> Result<bool, AppError> {
        {
            let mut cfg = state.config.write().map_err(AppError::from)?;
            let manager = cfg
                .get_manager_mut(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;

            for update in updates {
                if let Some(provider) = manager.providers.get_mut(&update.id) {
                    provider.sort_index = Some(update.sort_index);
                }
            }
        }

        state.save()?;
        Ok(true)
    }

    /// 将所有应用的当前供应商配置同步到 live 文件。
    ///
    /// 用于 WebDAV 下载、备份恢复等场景：数据库已更新，但 live 配置文件
    /// （`~/.codex/config.toml`、Claude `settings.json` 等）尚未同步。
    /// 对齐上游 `sync_current_to_live` 行为。
    pub fn sync_current_to_live(state: &AppState) -> Result<(), AppError> {
        use crate::services::mcp::McpService;

        // 在读锁下收集所有需要的数据，避免持锁写文件
        let snapshots: Vec<(AppType, Provider, Option<String>)> = {
            let guard = state.config.read().map_err(AppError::from)?;
            let mut result = Vec::new();
            for app_type in [
                AppType::Claude,
                AppType::Codex,
                AppType::Gemini,
                AppType::OpenCode,
                AppType::OpenClaw,
            ] {
                if let Some(manager) = guard.get_manager(&app_type) {
                    if app_type.is_additive_mode() {
                        let snippet = guard.common_config_snippets.get(&app_type).cloned();
                        let current_provider_id = manager.current.clone();
                        for provider in manager.providers.values() {
                            let mut provider = provider.clone();
                            if matches!(app_type, AppType::OpenClaw)
                                && provider.id != current_provider_id
                            {
                                Self::strip_openclaw_primary_model(&mut provider.settings_config);
                            }
                            result.push((app_type.clone(), provider, snippet.clone()));
                        }
                        continue;
                    }

                    if manager.current.is_empty() {
                        continue;
                    }
                    match manager.providers.get(&manager.current) {
                        Some(provider) => {
                            let snippet = guard.common_config_snippets.get(&app_type).cloned();
                            result.push((app_type.clone(), provider.clone(), snippet));
                        }
                        None => {
                            log::warn!(
                                "sync_current_to_live: {app_type} 当前供应商 {} 不存在，跳过",
                                manager.current
                            );
                        }
                    }
                }
            }
            result
        };

        for (app_type, provider, snippet) in &snapshots {
            if let Err(e) = Self::write_live_snapshot(app_type, provider, snippet.as_deref(), true)
            {
                log::warn!("sync_current_to_live: 写入 {app_type} live 配置失败: {e}");
            }
        }

        if let Err(e) = McpService::sync_all_enabled(state) {
            log::warn!("sync_current_to_live: MCP 同步失败: {e}");
        }

        if let Err(e) = crate::services::skill::SkillService::sync_all_enabled_best_effort() {
            log::warn!("sync_current_to_live: Skills 同步失败: {e}");
        }

        Ok(())
    }

    /// 切换指定应用的供应商
    pub fn switch(state: &AppState, app_type: AppType, provider_id: &str) -> Result<(), AppError> {
        let app_type_clone = app_type.clone();
        let provider_id_owned = provider_id.to_string();
        let takeover_active = if app_type.is_additive_mode() {
            false
        } else {
            let is_running = state
                .proxy_service
                .is_running_blocking()
                .map_err(AppError::Message)?;
            if !is_running {
                false
            } else {
                state
                    .proxy_service
                    .is_app_takeover_active_blocking(&app_type)
                    .map_err(AppError::Message)?
            }
        };

        Self::run_transaction(state, move |config| {
            if app_type_clone.is_additive_mode() {
                let provider = config
                    .get_manager(&app_type_clone)
                    .ok_or_else(|| Self::app_not_found(&app_type_clone))?
                    .providers
                    .get(&provider_id_owned)
                    .cloned()
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.not_found",
                            format!("供应商不存在: {provider_id_owned}"),
                            format!("Provider not found: {provider_id_owned}"),
                        )
                    })?;

                if matches!(app_type_clone, AppType::OpenClaw) {
                    if let Some(manager) = config.get_manager_mut(&app_type_clone) {
                        manager.current = provider_id_owned.clone();
                    }
                }

                let action = PostCommitAction {
                    app_type: app_type_clone.clone(),
                    provider,
                    backup: Self::capture_live_snapshot(&app_type_clone)?,
                    sync_mcp: true,
                    sync_all_additive_live: matches!(app_type_clone, AppType::OpenClaw),
                    refresh_snapshot: false,
                    common_config_snippet: config
                        .common_config_snippets
                        .get(&app_type_clone)
                        .cloned(),
                    takeover_active: false,
                };

                return Ok(((), Some(action)));
            }

            if takeover_active {
                let provider = config
                    .get_manager(&app_type_clone)
                    .ok_or_else(|| Self::app_not_found(&app_type_clone))?
                    .providers
                    .get(&provider_id_owned)
                    .cloned()
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.not_found",
                            format!("供应商不存在: {provider_id_owned}"),
                            format!("Provider not found: {provider_id_owned}"),
                        )
                    })?;

                if let Some(manager) = config.get_manager_mut(&app_type_clone) {
                    manager.current = provider_id_owned.clone();
                }

                let action = PostCommitAction {
                    app_type: app_type_clone.clone(),
                    provider,
                    backup: Self::capture_live_snapshot(&app_type_clone)?,
                    sync_mcp: false,
                    sync_all_additive_live: false,
                    refresh_snapshot: false,
                    common_config_snippet: config
                        .common_config_snippets
                        .get(&app_type_clone)
                        .cloned(),
                    takeover_active: true,
                };

                return Ok(((), Some(action)));
            }

            let backup = Self::capture_live_snapshot(&app_type_clone)?;
            let provider = match app_type_clone {
                AppType::Codex => Self::prepare_switch_codex(config, &provider_id_owned)?,
                AppType::Claude => Self::prepare_switch_claude(config, &provider_id_owned)?,
                AppType::Gemini => Self::prepare_switch_gemini(config, &provider_id_owned)?,
                AppType::OpenCode => unreachable!("additive mode handled above"),
                AppType::OpenClaw => unreachable!("additive mode handled above"),
            };

            let action = PostCommitAction {
                app_type: app_type_clone.clone(),
                provider,
                backup,
                sync_mcp: true, // v3.7.0: 所有应用切换时都同步 MCP，防止配置丢失
                sync_all_additive_live: false,
                refresh_snapshot: true,
                common_config_snippet: config.common_config_snippets.get(&app_type_clone).cloned(),
                takeover_active: false,
            };

            Ok(((), Some(action)))
        })
    }

    fn write_live_snapshot(
        app_type: &AppType,
        provider: &Provider,
        common_config_snippet: Option<&str>,
        apply_common_config: bool,
    ) -> Result<(), AppError> {
        match app_type {
            AppType::Codex => {
                Self::write_codex_live(provider, common_config_snippet, apply_common_config)
            }
            AppType::Claude => Self::write_claude_live(
                provider,
                if apply_common_config {
                    common_config_snippet
                } else {
                    None
                },
            ),
            AppType::Gemini => Self::write_gemini_live(
                provider,
                if apply_common_config {
                    common_config_snippet
                } else {
                    None
                },
            ),
            AppType::OpenCode => {
                let config_to_write = if let Some(obj) = provider.settings_config.as_object() {
                    if obj.contains_key("$schema") || obj.contains_key("provider") {
                        obj.get("provider")
                            .and_then(|providers| providers.get(&provider.id))
                            .cloned()
                            .unwrap_or_else(|| provider.settings_config.clone())
                    } else {
                        provider.settings_config.clone()
                    }
                } else {
                    provider.settings_config.clone()
                };

                match serde_json::from_value::<crate::provider::OpenCodeProviderConfig>(
                    config_to_write.clone(),
                ) {
                    Ok(config) => crate::opencode_config::set_typed_provider(&provider.id, &config),
                    Err(_) => crate::opencode_config::set_provider(&provider.id, config_to_write),
                }
            }
            AppType::OpenClaw => crate::openclaw_config::apply_provider_snapshot(
                &provider.id,
                &provider.settings_config,
            ),
        }
    }

    pub(crate) fn build_live_backup_snapshot(
        app_type: &AppType,
        provider: &Provider,
        common_config_snippet: Option<&str>,
        apply_common_config: bool,
    ) -> Result<Value, AppError> {
        match app_type {
            AppType::Claude => {
                let mut provider_content = provider.settings_config.clone();
                let _ = Self::normalize_claude_models_in_value(&mut provider_content);

                if !apply_common_config {
                    return Ok(provider_content);
                }

                let Some(snippet) = common_config_snippet.map(str::trim) else {
                    return Ok(provider_content);
                };
                if snippet.is_empty() {
                    return Ok(provider_content);
                }

                let common = Self::parse_common_claude_config_snippet(snippet)?;
                let mut merged = common;
                merge_json_values(&mut merged, &provider_content);
                let _ = Self::normalize_claude_models_in_value(&mut merged);
                Ok(merged)
            }
            AppType::Codex => {
                let settings = provider
                    .settings_config
                    .as_object()
                    .ok_or_else(|| AppError::Config("Codex 配置必须是 JSON 对象".into()))?;
                let auth = settings.get("auth").cloned();
                let cfg_text = settings.get("config").and_then(Value::as_str).unwrap_or("");

                let cfg_text_owned;
                let cfg_text = if is_codex_official_provider(provider)
                    && !cfg_text.trim().is_empty()
                {
                    if let Ok(mut doc) = cfg_text.parse::<toml_edit::DocumentMut>() {
                        let mp_key = doc
                            .get("model_provider")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        if let Some(key) = mp_key {
                            if let Some(section) = doc
                                .get_mut("model_providers")
                                .and_then(|v| v.as_table_like_mut())
                                .and_then(|t| t.get_mut(&key))
                                .and_then(|v| v.as_table_like_mut())
                            {
                                if section.get("wire_api").is_none() {
                                    section.insert("wire_api", toml_edit::value("responses"));
                                }
                                if section.get("requires_openai_auth").is_none() {
                                    section.insert("requires_openai_auth", toml_edit::value(true));
                                }
                            }
                        }
                        cfg_text_owned = doc.to_string();
                        &cfg_text_owned
                    } else {
                        cfg_text
                    }
                } else {
                    cfg_text
                };

                if !cfg_text.trim().is_empty() {
                    crate::codex_config::validate_config_toml(cfg_text)?;
                }

                let final_text = if apply_common_config {
                    if let Some(snippet) = common_config_snippet.map(str::trim) {
                        if !snippet.is_empty() && !cfg_text.trim().is_empty() {
                            let mut doc = cfg_text
                                .parse::<toml_edit::DocumentMut>()
                                .map_err(|e| AppError::Config(format!("TOML parse error: {e}")))?;
                            let common_doc =
                                snippet.parse::<toml_edit::DocumentMut>().map_err(|e| {
                                    AppError::Config(format!("Common config TOML parse error: {e}"))
                                })?;
                            Self::merge_toml_tables(doc.as_table_mut(), common_doc.as_table());
                            doc.to_string()
                        } else {
                            cfg_text.to_string()
                        }
                    } else {
                        cfg_text.to_string()
                    }
                } else {
                    cfg_text.to_string()
                };

                let mut backup = serde_json::Map::new();
                if let Some(auth) = auth {
                    backup.insert("auth".to_string(), auth);
                }
                backup.insert("config".to_string(), Value::String(final_text));
                Ok(Value::Object(backup))
            }
            AppType::Gemini => {
                let provider_content = provider.settings_config.clone();
                let content_to_write = if apply_common_config {
                    if let Some(snippet) = common_config_snippet.map(str::trim) {
                        if snippet.is_empty() {
                            provider_content
                        } else {
                            let common = Self::parse_common_gemini_config_snippet(snippet)?;
                            let mut merged = common;
                            merge_json_values(&mut merged, &provider_content);
                            merged
                        }
                    } else {
                        provider_content
                    }
                } else {
                    provider_content
                };

                let env_obj = content_to_write
                    .get("env")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let settings_path = crate::gemini_config::get_gemini_settings_path();
                let config_value = if let Some(config_value) = content_to_write.get("config") {
                    if config_value.is_null() {
                        if settings_path.exists() {
                            read_json_file(&settings_path)?
                        } else {
                            json!({})
                        }
                    } else if let Some(provider_config) = config_value.as_object() {
                        if provider_config.is_empty() {
                            if settings_path.exists() {
                                read_json_file(&settings_path)?
                            } else {
                                json!({})
                            }
                        } else {
                            let mut merged = if settings_path.exists() {
                                read_json_file(&settings_path)?
                            } else {
                                json!({})
                            };

                            if !merged.is_object() {
                                merged = json!({});
                            }

                            let merged_map = merged.as_object_mut().ok_or_else(|| {
                                AppError::localized(
                                    "gemini.validation.invalid_settings",
                                    "Gemini 现有 settings.json 格式错误: 必须是对象",
                                    "Gemini existing settings.json invalid: must be a JSON object",
                                )
                            })?;
                            for (key, value) in provider_config {
                                merged_map.insert(key.clone(), value.clone());
                            }
                            merged
                        }
                    } else {
                        return Err(AppError::localized(
                            "gemini.validation.invalid_config",
                            "Gemini 配置格式错误: config 必须是对象或 null",
                            "Gemini config invalid: config must be an object or null",
                        ));
                    }
                } else if settings_path.exists() {
                    read_json_file(&settings_path)?
                } else {
                    json!({})
                };

                Ok(json!({
                    "env": env_obj,
                    "config": config_value,
                }))
            }
            AppType::OpenCode => Err(AppError::Config(
                "OpenCode does not support proxy takeover backups".into(),
            )),
            AppType::OpenClaw => Err(AppError::Config(
                "OpenClaw does not support proxy takeover backups".into(),
            )),
        }
    }

    fn validate_provider_settings(app_type: &AppType, provider: &Provider) -> Result<(), AppError> {
        match app_type {
            AppType::Claude => {
                if !provider.settings_config.is_object() {
                    return Err(AppError::localized(
                        "provider.claude.settings.not_object",
                        "Claude 配置必须是 JSON 对象",
                        "Claude configuration must be a JSON object",
                    ));
                }
            }
            AppType::Codex => {
                let settings = provider.settings_config.as_object().ok_or_else(|| {
                    AppError::localized(
                        "provider.codex.settings.not_object",
                        "Codex 配置必须是 JSON 对象",
                        "Codex configuration must be a JSON object",
                    )
                })?;

                let is_official = is_codex_official_provider(provider);

                // config 字段必须存在且是字符串
                let config_value = settings.get("config").ok_or_else(|| {
                    AppError::localized(
                        "provider.codex.config.missing",
                        format!("供应商 {} 缺少 config 配置", provider.id),
                        format!("Provider {} is missing config configuration", provider.id),
                    )
                })?;
                if !(config_value.is_string() || config_value.is_null()) {
                    return Err(AppError::localized(
                        "provider.codex.config.invalid_type",
                        "Codex config 字段必须是字符串",
                        "Codex config field must be a string",
                    ));
                }
                if let Some(cfg_text) = config_value.as_str() {
                    crate::codex_config::validate_config_toml(cfg_text)?;
                }

                // auth 规则：
                // - 官方供应商：auth 可选（使用 codex login 保存的凭证）
                // - 第三方/自定义：必须提供 auth.OPENAI_API_KEY
                match settings.get("auth") {
                    Some(auth) => {
                        let auth_obj = auth.as_object().ok_or_else(|| {
                            AppError::localized(
                                "provider.codex.auth.not_object",
                                format!("供应商 {} 的 auth 配置必须是 JSON 对象", provider.id),
                                format!(
                                    "Provider {} auth configuration must be a JSON object",
                                    provider.id
                                ),
                            )
                        })?;
                        if !is_official {
                            let api_key = auth_obj
                                .get("OPENAI_API_KEY")
                                .and_then(|v| v.as_str())
                                .map(str::trim)
                                .unwrap_or("");
                            if api_key.is_empty() {
                                return Err(AppError::localized(
                                    "provider.codex.api_key.missing",
                                    format!("供应商 {} 缺少 OPENAI_API_KEY", provider.id),
                                    format!("Provider {} is missing OPENAI_API_KEY", provider.id),
                                ));
                            }
                        }
                    }
                    None => {
                        if !is_official {
                            return Err(AppError::localized(
                                "provider.codex.auth.missing",
                                format!("供应商 {} 缺少 auth 配置", provider.id),
                                format!("Provider {} is missing auth configuration", provider.id),
                            ));
                        }
                    }
                }
            }
            AppType::Gemini => {
                use crate::gemini_config::validate_gemini_settings;
                validate_gemini_settings(&provider.settings_config)?
            }
            AppType::OpenCode => {
                if !provider.settings_config.is_object() {
                    return Err(AppError::localized(
                        "provider.opencode.settings.not_object",
                        "OpenCode 配置必须是 JSON 对象",
                        "OpenCode configuration must be a JSON object",
                    ));
                }
            }
            AppType::OpenClaw => {
                if !provider.settings_config.is_object() {
                    return Err(AppError::localized(
                        "provider.openclaw.settings.not_object",
                        "OpenClaw 配置必须是 JSON 对象",
                        "OpenClaw configuration must be a JSON object",
                    ));
                }

                let provider_config = crate::openclaw_config::provider_config_from_settings(
                    &provider.settings_config,
                );
                if !provider_config.is_object() {
                    return Err(AppError::localized(
                        "provider.openclaw.provider.not_object",
                        "OpenClaw 供应商配置必须是 JSON 对象",
                        "OpenClaw provider config must be a JSON object",
                    ));
                }

                let api_key = provider_config
                    .get("apiKey")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .unwrap_or("");
                if api_key.is_empty() {
                    return Err(AppError::localized(
                        "provider.openclaw.api_key.missing",
                        "OpenClaw 供应商缺少 apiKey 配置",
                        "OpenClaw provider is missing apiKey configuration",
                    ));
                }

                let base_url = provider_config
                    .get("baseUrl")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .unwrap_or("");
                if base_url.is_empty() {
                    return Err(AppError::localized(
                        "provider.openclaw.base_url.missing",
                        "OpenClaw 供应商缺少 baseUrl 配置",
                        "OpenClaw provider is missing baseUrl configuration",
                    ));
                }

                let has_model_id = provider_config
                    .get("models")
                    .and_then(Value::as_array)
                    .map(|models| {
                        models.iter().any(|model| {
                            model
                                .get("id")
                                .and_then(Value::as_str)
                                .map(str::trim)
                                .is_some_and(|id| !id.is_empty())
                        })
                    })
                    .unwrap_or(false);
                if !has_model_id {
                    return Err(AppError::localized(
                        "provider.openclaw.model_id.missing",
                        "OpenClaw 供应商至少需要一个带 id 的模型配置",
                        "OpenClaw provider requires at least one model with a non-empty id",
                    ));
                }
            }
        }

        // 🔧 验证并清理 UsageScript 配置（所有应用类型通用）
        if let Some(meta) = &provider.meta {
            if let Some(usage_script) = &meta.usage_script {
                Self::validate_usage_script(usage_script)?;
            }
        }

        Ok(())
    }

    fn app_not_found(app_type: &AppType) -> AppError {
        AppError::localized(
            "provider.app_not_found",
            format!("应用类型不存在: {app_type:?}"),
            format!("App type not found: {app_type:?}"),
        )
    }

    pub fn delete(state: &AppState, app_type: AppType, provider_id: &str) -> Result<(), AppError> {
        let provider_snapshot = {
            let config = state.config.read().map_err(AppError::from)?;
            let manager = config
                .get_manager(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;

            if !matches!(app_type, AppType::OpenCode) && manager.current == provider_id {
                return Err(AppError::localized(
                    "provider.delete.current",
                    "不能删除当前正在使用的供应商",
                    "Cannot delete the provider currently in use",
                ));
            }

            manager.providers.get(provider_id).cloned().ok_or_else(|| {
                AppError::localized(
                    "provider.not_found",
                    format!("供应商不存在: {provider_id}"),
                    format!("Provider not found: {provider_id}"),
                )
            })?
        };

        if app_type.is_additive_mode() {
            match app_type {
                AppType::OpenCode => {
                    if crate::opencode_config::get_opencode_dir().exists() {
                        crate::opencode_config::remove_provider(provider_id)?;
                    }
                }
                AppType::OpenClaw => {
                    if crate::openclaw_config::get_openclaw_dir().exists() {
                        crate::openclaw_config::remove_provider(provider_id)?;
                    }
                }
                _ => {}
            }

            {
                let mut config = state.config.write().map_err(AppError::from)?;
                let manager = config
                    .get_manager_mut(&app_type)
                    .ok_or_else(|| Self::app_not_found(&app_type))?;
                manager.providers.shift_remove(provider_id);
            }

            return state.save();
        }

        match app_type {
            AppType::Codex => {
                crate::codex_config::delete_codex_provider_config(
                    provider_id,
                    &provider_snapshot.name,
                )?;
            }
            AppType::Claude => {
                // 兼容旧版本：历史上会在 Claude 目录内为每个供应商生成 settings-*.json 副本
                // 这里继续清理这些遗留文件，避免堆积过期配置。
                let by_name = get_provider_config_path(provider_id, Some(&provider_snapshot.name));
                let by_id = get_provider_config_path(provider_id, None);
                delete_file(&by_name)?;
                delete_file(&by_id)?;
            }
            AppType::Gemini => {
                // Gemini 使用单一的 .env 文件，不需要删除单独的供应商配置文件
            }
            AppType::OpenCode => {
                let _ = provider_snapshot;
            }
            AppType::OpenClaw => {
                let _ = provider_snapshot;
            }
        }

        {
            let mut config = state.config.write().map_err(AppError::from)?;
            let manager = config
                .get_manager_mut(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;

            if !matches!(app_type, AppType::OpenCode) && manager.current == provider_id {
                return Err(AppError::localized(
                    "provider.delete.current",
                    "不能删除当前正在使用的供应商",
                    "Cannot delete the provider currently in use",
                ));
            }

            manager.providers.shift_remove(provider_id);
        }

        state.save()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderSortUpdate {
    pub id: String,
    #[serde(rename = "sortIndex")]
    pub sort_index: usize,
}
