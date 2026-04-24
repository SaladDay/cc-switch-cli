use std::collections::HashSet;

mod claude;
mod codex;
#[cfg(test)]
mod codex_openai_auth_tests;
mod common;
mod common_config;
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
    delete_file, get_claude_settings_path, get_provider_config_path, read_json_file,
    write_json_file,
};
use crate::error::AppError;
use crate::provider::Provider;
use crate::store::AppState;

use gemini_auth::GeminiAuthType;
use live::LiveSnapshot;

pub use common::migrate_legacy_codex_config;
#[cfg(test)]
use common::strip_codex_common_config_from_full_text;

/// 供应商相关业务逻辑
pub struct ProviderService;

fn current_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
fn state_from_config(config: MultiAppConfig) -> AppState {
    let db = std::sync::Arc::new(crate::Database::memory().expect("create memory database"));
    db.migrate_from_json(&config)
        .expect("seed memory database from config");
    let mut config = config;
    ProviderService::migrate_common_config_upstream_semantics_if_needed(&db, &mut config)
        .expect("migrate common config semantics for test state");
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
    refresh_snapshot: bool,
    common_config_snippet: Option<String>,
    takeover_active: bool,
}

impl ProviderService {
    fn is_codex_official_provider(provider: &Provider) -> bool {
        provider
            .meta
            .as_ref()
            .and_then(|meta| meta.codex_official)
            .unwrap_or(false)
            || provider
                .category
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case("official"))
    }

    fn codex_config_has_base_url(config_text: &str) -> bool {
        let Ok(table) = toml::from_str::<toml::Table>(config_text.trim()) else {
            return false;
        };

        if table
            .get("base_url")
            .and_then(|value| value.as_str())
            .is_some_and(|value| !value.trim().is_empty())
        {
            return true;
        }

        let Some(provider_key) = table.get("model_provider").and_then(|value| value.as_str())
        else {
            return false;
        };

        table
            .get("model_providers")
            .and_then(|value| value.as_table())
            .and_then(|providers| providers.get(provider_key))
            .and_then(|value| value.as_table())
            .and_then(|provider| provider.get("base_url"))
            .and_then(|value| value.as_str())
            .is_some_and(|value| !value.trim().is_empty())
    }

    pub fn sync_openclaw_to_live(state: &AppState) -> Result<(), AppError> {
        let (providers, snippet) = {
            let guard = state.config.read().map_err(AppError::from)?;
            let Some(manager) = guard.get_manager(&AppType::OpenClaw) else {
                return Ok(());
            };

            (
                manager.providers.values().cloned().collect::<Vec<_>>(),
                guard
                    .common_config_snippets
                    .get(&AppType::OpenClaw)
                    .cloned(),
            )
        };

        for provider in &providers {
            Self::write_live_snapshot(&AppType::OpenClaw, provider, snippet.as_deref(), true)?;
        }

        Ok(())
    }

    pub(crate) fn valid_openclaw_live_provider_ids() -> Result<Option<HashSet<String>>, AppError> {
        if !crate::openclaw_config::get_openclaw_config_path().exists() {
            return Ok(None);
        }

        let mut valid_provider_ids = HashSet::new();
        for (provider_id, live_provider) in crate::openclaw_config::get_providers()? {
            if provider_id.trim().is_empty() {
                continue;
            }

            let Ok(config) = Self::parse_openclaw_provider_settings(&live_provider) else {
                continue;
            };

            if Self::validate_openclaw_provider_models(&provider_id, &config).is_err() {
                continue;
            }

            if config.models.iter().any(|model| model.id.trim().is_empty()) {
                continue;
            }

            valid_provider_ids.insert(provider_id);
        }

        Ok(Some(valid_provider_ids))
    }

    fn parse_common_opencode_config_snippet(snippet: &str) -> Result<Value, AppError> {
        let value: Value = serde_json::from_str(snippet).map_err(|e| {
            AppError::localized(
                "common_config.opencode.invalid_json",
                format!("OpenCode 通用配置片段不是有效的 JSON：{e}"),
                format!("OpenCode common config snippet is not valid JSON: {e}"),
            )
        })?;
        if !value.is_object() {
            return Err(AppError::localized(
                "common_config.opencode.not_object",
                "OpenCode 通用配置片段必须是 JSON 对象",
                "OpenCode common config snippet must be a JSON object",
            ));
        }
        Ok(value)
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

    fn run_transaction_preserving_current_providers<R, F>(
        state: &AppState,
        preserved_current_apps: &[AppType],
        f: F,
    ) -> Result<R, AppError>
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

        if let Err(save_err) = state.save_preserving_current_providers(preserved_current_apps) {
            if let Err(rollback_err) = Self::restore_config_only_preserving_current_providers(
                state,
                original.clone(),
                preserved_current_apps,
            ) {
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
                if let Err(rollback_err) = Self::rollback_after_failure_preserving_current_providers(
                    state,
                    original.clone(),
                    preserved_current_apps,
                    action.backup.clone(),
                ) {
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

    fn restore_config_only_preserving_current_providers(
        state: &AppState,
        snapshot: MultiAppConfig,
        preserved_current_apps: &[AppType],
    ) -> Result<(), AppError> {
        {
            let mut guard = state.config.write().map_err(AppError::from)?;
            *guard = snapshot;
        }
        state.save_preserving_current_providers(preserved_current_apps)
    }

    fn rollback_after_failure(
        state: &AppState,
        snapshot: MultiAppConfig,
        backup: LiveSnapshot,
    ) -> Result<(), AppError> {
        Self::restore_config_only(state, snapshot)?;
        backup.restore()
    }

    fn rollback_after_failure_preserving_current_providers(
        state: &AppState,
        snapshot: MultiAppConfig,
        preserved_current_apps: &[AppType],
        backup: LiveSnapshot,
    ) -> Result<(), AppError> {
        Self::restore_config_only_preserving_current_providers(
            state,
            snapshot,
            preserved_current_apps,
        )?;
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
                        guard.common_config_snippets.claude.clone(),
                    )
                };
                live_after = common_config::strip_common_config_from_live_settings(
                    app_type,
                    &provider,
                    live_after,
                    common_snippet.as_deref(),
                );
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

                // Read auth from disk; if absent, fall back to the DB snapshot's auth
                // so that WebDAV-synced credentials are not overwritten with empty data.
                let auth = if auth_path.exists() {
                    Some(read_json_file::<Value>(&auth_path)?)
                } else {
                    provider.settings_config.get("auth").cloned()
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
                let live_after = providers.get(provider_id).cloned().ok_or_else(|| {
                    AppError::localized(
                        "openclaw.live.missing_provider",
                        format!("OpenClaw live 配置中缺少供应商: {provider_id}"),
                        format!("OpenClaw live config missing provider: {provider_id}"),
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
        common_config::validate_common_config_snippet(app_type, snippet)
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
        strict_current_provider_id: Option<&str>,
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
            AppType::Codex => Self::migrate_codex_common_config_snippet(
                config,
                strict_current_provider_id,
                old_snippet,
            ),
            AppType::Gemini => Self::migrate_gemini_common_config_snippet(
                config,
                strict_current_provider_id,
                old_snippet,
            ),
            AppType::OpenCode | AppType::OpenClaw => Ok(()),
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

    #[doc(hidden)]
    pub fn migrate_common_config_upstream_semantics_if_needed(
        db: &crate::database::Database,
        config: &mut MultiAppConfig,
    ) -> Result<(), AppError> {
        common_config::migrate_common_config_upstream_semantics_if_needed(db, config)
    }

    fn build_common_config_post_commit_action(
        config: &MultiAppConfig,
        app_type: &AppType,
        current_provider_id: Option<&str>,
        takeover_active: bool,
    ) -> Result<Option<PostCommitAction>, AppError> {
        if app_type.is_additive_mode() {
            return Ok(None);
        }

        let Some(current_provider_id) = current_provider_id else {
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
            refresh_snapshot: false,
            common_config_snippet: config.common_config_snippets.get(app_type).cloned(),
            takeover_active,
        }))
    }

    fn resolve_live_apply_common_config(
        app_type: &AppType,
        provider: &Provider,
        common_config_snippet: Option<&str>,
        requested_apply_common_config: bool,
    ) -> bool {
        if !requested_apply_common_config {
            return false;
        }

        common_config::provider_uses_common_config(app_type, provider, common_config_snippet)
    }

    fn normalize_provider_for_storage(
        app_type: &AppType,
        provider: &mut Provider,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        common_config::normalize_provider_common_config_for_storage(
            app_type,
            provider,
            common_config_snippet,
        )
    }

    pub(crate) fn normalize_settings_config_for_storage(
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

    pub(crate) fn remove_common_config_from_settings_for_preview(
        app_type: &AppType,
        settings_config: &Value,
        common_config_snippet: &str,
    ) -> Result<Value, AppError> {
        common_config::remove_common_config_from_settings(
            app_type,
            settings_config,
            common_config_snippet,
        )
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
            common_config::migrate_provider_subset_usage_for_storage(
                app_type,
                provider,
                common_config_snippet,
            )?;
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
            if let Err(err) = common_config::migrate_provider_subset_usage_for_storage(
                app_type,
                provider,
                common_config_snippet,
            ) {
                log::warn!(
                    "skip normalizing {app_type} provider snapshot '{provider_id}' while applying auto-extracted common config: {err}"
                );
            }
        }
    }

    fn normalize_existing_provider_snapshots_for_storage_strict_current_best_effort_others(
        config: &mut MultiAppConfig,
        app_type: &AppType,
        strict_current_provider_id: Option<&str>,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        let Some(current_provider_id) = strict_current_provider_id.and_then(|provider_id| {
            config.get_manager(app_type).and_then(|manager| {
                manager
                    .providers
                    .contains_key(provider_id)
                    .then(|| provider_id.to_string())
            })
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
            common_config::migrate_provider_subset_usage_for_storage(
                app_type,
                current_provider,
                common_config_snippet,
            )?;
        }

        for (provider_id, provider) in manager.providers.iter_mut() {
            if provider_id == &current_provider_id {
                continue;
            }

            if let Err(err) = common_config::migrate_provider_subset_usage_for_storage(
                app_type,
                provider,
                common_config_snippet,
            ) {
                log::warn!(
                    "skip normalizing {app_type} non-current provider snapshot '{provider_id}' while updating common config snippet: {err}"
                );
            }
        }

        Ok(())
    }

    fn hydrate_missing_provider_snapshots_from_db(
        config: &mut MultiAppConfig,
        app_type: &AppType,
        db_providers: &IndexMap<String, Provider>,
    ) -> Result<(), AppError> {
        let manager = config
            .get_manager_mut(app_type)
            .ok_or_else(|| Self::app_not_found(app_type))?;

        for (provider_id, provider) in db_providers {
            manager
                .providers
                .entry(provider_id.clone())
                .or_insert_with(|| provider.clone());
        }

        Ok(())
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
        let (effective_current_provider, db_providers) = if app_type.is_additive_mode() {
            (None, None)
        } else {
            (
                crate::settings::get_effective_current_provider(&state.db, &app_type)?,
                Some(state.db.get_all_providers(app_type.as_str())?),
            )
        };
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

        Self::run_transaction_preserving_current_providers(
            state,
            std::slice::from_ref(&app_type),
            move |config| {
                config.ensure_app(&app_type_clone);

                if let Some(db_providers) = db_providers.as_ref() {
                    Self::hydrate_missing_provider_snapshots_from_db(
                        config,
                        &app_type_clone,
                        db_providers,
                    )?;
                }

                let old_snippet = config
                    .common_config_snippets
                    .get(&app_type_clone)
                    .cloned()
                    .filter(|value| !value.trim().is_empty());

                Self::migrate_old_common_config_snippet_best_effort(
                    config,
                    &app_type_clone,
                    effective_current_provider.as_deref(),
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
                    effective_current_provider.as_deref(),
                    normalized_snippet.as_deref(),
                )?;
                }

                let action = Self::build_common_config_post_commit_action(
                    config,
                    &app_type_clone,
                    effective_current_provider.as_deref(),
                    takeover_active,
                )?;
                Ok(((), action))
            },
        )
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

    pub(crate) fn sync_openclaw_providers_from_live(state: &AppState) -> Result<(), AppError> {
        live::sync_openclaw_providers_from_live(state)?;
        Ok(())
    }

    /// 获取当前供应商 ID
    pub fn current(state: &AppState, app_type: AppType) -> Result<String, AppError> {
        if app_type.is_additive_mode() {
            return Ok(String::new());
        }
        crate::settings::get_effective_current_provider(&state.db, &app_type)
            .map(|opt| opt.unwrap_or_default())
    }

    /// 新增供应商
    pub fn add(state: &AppState, app_type: AppType, provider: Provider) -> Result<bool, AppError> {
        let mut provider = provider;
        // 归一化 Claude 模型键
        Self::normalize_provider_if_claude(&app_type, &mut provider);
        Self::validate_provider_settings(&app_type, &provider)?;

        let app_type_clone = app_type.clone();
        let provider_clone = provider.clone();
        let stored_current_provider = if app_type.is_additive_mode() {
            None
        } else {
            state.db.get_current_provider(app_type.as_str())?
        };

        Self::run_transaction(state, move |config| {
            let common_config_snippet = config.common_config_snippets.get(&app_type_clone).cloned();
            let mut provider_to_store = provider_clone.clone();
            Self::normalize_provider_for_storage(
                &app_type_clone,
                &mut provider_to_store,
                common_config_snippet.as_deref(),
            )?;

            if matches!(app_type_clone, AppType::OpenClaw)
                && provider_to_store.created_at.is_none()
                && live::is_auto_mirrored_openclaw_snapshot(&provider_to_store)
            {
                provider_to_store.created_at = Some(current_timestamp());
            }

            config.ensure_app(&app_type_clone);
            let manager = config
                .get_manager_mut(&app_type_clone)
                .ok_or_else(|| Self::app_not_found(&app_type_clone))?;

            if !app_type_clone.is_additive_mode() {
                manager.current = stored_current_provider.clone().unwrap_or_default();
            }

            let was_empty = manager.providers.is_empty();
            manager
                .providers
                .insert(provider_to_store.id.clone(), provider_to_store.clone());

            if !app_type_clone.is_additive_mode()
                && stored_current_provider.is_none()
                && (was_empty || manager.current.is_empty())
            {
                manager.current = provider_to_store.id.clone();
            }

            let is_current =
                app_type_clone.is_additive_mode() || manager.current == provider_to_store.id;
            let action = if is_current {
                let backup = Self::capture_live_snapshot(&app_type_clone)?;
                Some(PostCommitAction {
                    app_type: app_type_clone.clone(),
                    provider: provider_to_store.clone(),
                    backup,
                    // Codex current-provider saves rewrite live config from the stored snapshot,
                    // so managed MCP must be synced back after the write.
                    sync_mcp: matches!(&app_type_clone, AppType::Codex),
                    refresh_snapshot: false,
                    common_config_snippet,
                    takeover_active: false,
                })
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
        let (effective_current_provider, stored_current_provider) = if app_type.is_additive_mode() {
            (None, None)
        } else {
            (
                crate::settings::get_effective_current_provider(&state.db, &app_type)?,
                state.db.get_current_provider(app_type.as_str())?,
            )
        };

        Self::run_transaction(state, move |config| {
            let common_config_snippet = config.common_config_snippets.get(&app_type_clone).cloned();
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

            if !app_type_clone.is_additive_mode() {
                manager.current = stored_current_provider.clone().unwrap_or_default();
            }

            let is_current = app_type_clone.is_additive_mode()
                || effective_current_provider.as_deref() == Some(provider_id.as_str());
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
                if matches!(app_type_clone, AppType::OpenClaw)
                    && updated.created_at.is_none()
                    && live::is_auto_mirrored_openclaw_snapshot(&updated)
                {
                    updated.created_at = Some(current_timestamp());
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

            let action = if is_current {
                let backup = Self::capture_live_snapshot(&app_type_clone)?;
                Some(PostCommitAction {
                    app_type: app_type_clone.clone(),
                    provider: merged,
                    backup,
                    // Codex current-provider saves rewrite live config from the stored snapshot,
                    // so managed MCP must be synced back after the write.
                    sync_mcp: matches!(&app_type_clone, AppType::Codex),
                    refresh_snapshot: false,
                    common_config_snippet,
                    takeover_active: false,
                })
            } else {
                None
            };

            Ok((true, action))
        })
    }

    /// 导入当前 live 配置为默认供应商
    pub fn import_default_config(state: &AppState, app_type: AppType) -> Result<(), AppError> {
        if app_type.is_additive_mode() {
            if matches!(app_type, AppType::OpenClaw) {
                return Ok(());
            }

            let providers = crate::opencode_config::get_providers()?;
            if providers.is_empty() {
                return Ok(());
            }

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
                    let name = settings_config
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(&id)
                        .to_string();
                    manager.providers.insert(
                        id.clone(),
                        Provider::with_id(id, name, settings_config, None),
                    );
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
            for app_type in AppType::all() {
                if app_type.is_additive_mode() {
                    if let Some(manager) = guard.get_manager(&app_type) {
                        let snippet = guard.common_config_snippets.get(&app_type).cloned();
                        for provider in manager.providers.values() {
                            result.push((app_type.clone(), provider.clone(), snippet.clone()));
                        }
                    }
                    continue;
                }

                let current_id =
                    match crate::settings::get_effective_current_provider(&state.db, &app_type)? {
                        Some(id) => id,
                        None => continue,
                    };
                let providers = state.db.get_all_providers(app_type.as_str())?;
                match providers.get(&current_id) {
                    Some(provider) => {
                        let snippet = state.db.get_config_snippet(app_type.as_str())?;
                        result.push((app_type.clone(), provider.clone(), snippet));
                    }
                    None => {
                        log::warn!(
                            "sync_current_to_live: {app_type} 当前供应商 {} 不存在于数据库，跳过",
                            current_id
                        );
                    }
                }
            }
            result
        };

        let openclaw_live_provider_ids = match Self::valid_openclaw_live_provider_ids() {
            Ok(provider_ids) => provider_ids,
            Err(err) => {
                log::warn!(
                    "sync_current_to_live: 读取 OpenClaw live providers 失败，跳过 OpenClaw 同步: {err}"
                );
                None
            }
        };

        for (app_type, provider, snippet) in &snapshots {
            if matches!(app_type, AppType::OpenClaw)
                && !openclaw_live_provider_ids
                    .as_ref()
                    .is_some_and(|provider_ids| provider_ids.contains(&provider.id))
            {
                continue;
            }

            if let Err(e) = Self::write_live_snapshot(app_type, provider, snippet.as_deref(), true)
            {
                log::warn!("sync_current_to_live: 写入 {app_type} live 配置失败: {e}");
            }
        }

        if let Err(e) =
            crate::services::prompt::PromptService::sync_all_active_to_live_best_effort(state)
        {
            log::warn!("sync_current_to_live: Prompt 同步失败: {e}");
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
        if !app_type.is_additive_mode() {
            let providers = state.db.get_all_providers(app_type.as_str())?;
            providers.get(provider_id).ok_or_else(|| {
                AppError::localized(
                    "provider.not_found",
                    format!("供应商不存在: {provider_id}"),
                    format!("Provider not found: {provider_id}"),
                )
            })?;

            let is_app_taken_over =
                futures::executor::block_on(state.db.get_live_backup(app_type.as_str()))
                    .ok()
                    .flatten()
                    .is_some();
            let is_proxy_running = state
                .proxy_service
                .is_running_blocking()
                .map_err(AppError::Message)?;
            let live_taken_over = state
                .proxy_service
                .detect_takeover_in_live_config_for_app(&app_type);
            let should_hot_switch = (is_app_taken_over || live_taken_over) && is_proxy_running;

            if should_hot_switch {
                futures::executor::block_on(
                    state
                        .proxy_service
                        .hot_switch_provider(app_type.as_str(), provider_id),
                )
                .map_err(|e| AppError::Message(format!("热切换失败: {e}")))?;

                let mut guard = state.config.write().map_err(AppError::from)?;
                if let Some(manager) = guard.get_manager_mut(&app_type) {
                    manager.current = provider_id.to_string();
                }
                return Ok(());
            }
        }

        let app_type_clone = app_type.clone();
        let provider_id_owned = provider_id.to_string();
        let effective_current_provider = if app_type.is_additive_mode() {
            None
        } else {
            crate::settings::get_effective_current_provider(&state.db, &app_type)?
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

                let action = PostCommitAction {
                    app_type: app_type_clone.clone(),
                    provider,
                    backup: Self::capture_live_snapshot(&app_type_clone)?,
                    sync_mcp: matches!(app_type_clone, AppType::OpenCode),
                    refresh_snapshot: false,
                    common_config_snippet: config
                        .common_config_snippets
                        .get(&app_type_clone)
                        .cloned(),
                    takeover_active: false,
                };

                return Ok(((), Some(action)));
            }

            let backup = Self::capture_live_snapshot(&app_type_clone)?;
            let provider = match app_type_clone {
                AppType::Codex => Self::prepare_switch_codex(
                    config,
                    &provider_id_owned,
                    effective_current_provider.as_deref(),
                )?,
                AppType::Claude => Self::prepare_switch_claude(
                    config,
                    &provider_id_owned,
                    effective_current_provider.as_deref(),
                )?,
                AppType::Gemini => Self::prepare_switch_gemini(
                    config,
                    &provider_id_owned,
                    effective_current_provider.as_deref(),
                )?,
                AppType::OpenCode => unreachable!("additive mode handled above"),
                AppType::OpenClaw => unreachable!("additive mode handled above"),
            };

            let action = PostCommitAction {
                app_type: app_type_clone.clone(),
                provider,
                backup,
                sync_mcp: true, // v3.7.0: 所有应用切换时都同步 MCP，防止配置丢失
                refresh_snapshot: true,
                common_config_snippet: config.common_config_snippets.get(&app_type_clone).cloned(),
                takeover_active: false,
            };

            Ok(((), Some(action)))
        })?;

        if !app_type.is_additive_mode() {
            crate::settings::set_current_provider(&app_type, Some(provider_id))?;
        }

        Ok(())
    }

    fn write_live_snapshot(
        app_type: &AppType,
        provider: &Provider,
        common_config_snippet: Option<&str>,
        apply_common_config: bool,
    ) -> Result<(), AppError> {
        let apply_common_config = Self::resolve_live_apply_common_config(
            app_type,
            provider,
            common_config_snippet,
            apply_common_config,
        );

        match app_type {
            AppType::Codex => {
                Self::write_codex_live(provider, common_config_snippet, apply_common_config)
            }
            AppType::Claude => {
                Self::write_claude_live(provider, common_config_snippet, apply_common_config)
            }
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
            AppType::OpenClaw => {
                let settings_config = provider.settings_config.clone();
                let looks_like_provider = settings_config.get("baseUrl").is_some()
                    || settings_config.get("api").is_some()
                    || settings_config.get("models").is_some();
                if !looks_like_provider {
                    return Ok(());
                }

                let config = Self::parse_openclaw_provider_settings(&settings_config)?;
                Self::validate_openclaw_provider_models(&provider.id, &config)?;
                let write_result =
                    crate::openclaw_config::set_typed_provider(&provider.id, &config).map(|_| ());

                write_result.map_err(Self::normalize_openclaw_live_write_error)
            }
        }
    }

    fn parse_openclaw_provider_settings(
        settings_config: &Value,
    ) -> Result<crate::provider::OpenClawProviderConfig, AppError> {
        let settings_obj = settings_config.as_object().ok_or_else(|| {
            AppError::localized(
                "provider.openclaw.settings.not_object",
                "OpenClaw 配置必须是 JSON 对象",
                "OpenClaw configuration must be a JSON object",
            )
        })?;

        let legacy_aliases = Self::collect_openclaw_legacy_aliases(settings_obj);
        if !legacy_aliases.is_empty() {
            let aliases = legacy_aliases.join(", ");
            return Err(AppError::localized(
                "provider.openclaw.settings.invalid",
                format!(
                    "OpenClaw 配置使用了不支持的旧字段: {aliases}。请改用规范 OpenClaw 字段。"
                ),
                format!(
                    "OpenClaw config uses unsupported legacy alias keys: {aliases}. Use canonical OpenClaw keys instead."
                ),
            ));
        }

        serde_json::from_value(settings_config.clone()).map_err(|err| {
            AppError::localized(
                "provider.openclaw.settings.invalid",
                format!("OpenClaw 配置格式无效: {err}"),
                format!("OpenClaw provider schema is invalid: {err}"),
            )
        })
    }

    fn validate_openclaw_provider_models(
        provider_id: &str,
        config: &crate::provider::OpenClawProviderConfig,
    ) -> Result<(), AppError> {
        if config.models.is_empty() {
            return Err(AppError::localized(
                "provider.openclaw.models.missing",
                format!("OpenClaw 供应商 {provider_id} 至少需要一个模型"),
                format!("OpenClaw provider {provider_id} must define at least one model"),
            ));
        }

        Ok(())
    }

    fn collect_openclaw_legacy_aliases(
        settings_obj: &serde_json::Map<String, Value>,
    ) -> Vec<String> {
        let mut aliases = Vec::new();

        for alias in ["api_key", "base_url", "options", "npm"] {
            if settings_obj.contains_key(alias) {
                aliases.push(alias.to_string());
            }
        }

        if let Some(models) = settings_obj.get("models").and_then(Value::as_array) {
            for (index, model) in models.iter().enumerate() {
                if let Some(model_obj) = model.as_object() {
                    if model_obj.contains_key("context_window") {
                        aliases.push(format!("models[{index}].context_window"));
                    }
                }
            }
        }

        aliases
    }

    fn normalize_openclaw_live_write_error(err: AppError) -> AppError {
        match err {
            AppError::Config(message)
                if message.starts_with("Failed to parse OpenClaw config as JSON5:") =>
            {
                AppError::Config(message.replacen(
                    "Failed to parse OpenClaw config as JSON5",
                    "Failed to parse OpenClaw config as round-trip JSON5 document",
                    1,
                ))
            }
            other => other,
        }
    }

    pub(crate) fn build_effective_live_snapshot(
        app_type: &AppType,
        provider: &Provider,
        common_config_snippet: Option<&str>,
        apply_common_config: bool,
    ) -> Result<Value, AppError> {
        let apply_common_config = Self::resolve_live_apply_common_config(
            app_type,
            provider,
            common_config_snippet,
            apply_common_config,
        );

        match app_type {
            AppType::Claude => {
                let mut effective = common_config::build_effective_settings_with_common_config(
                    app_type,
                    provider,
                    common_config_snippet,
                    apply_common_config,
                )?;
                let _ = Self::normalize_claude_models_in_value(&mut effective);
                Ok(effective)
            }
            AppType::Codex => {
                let effective = common_config::build_effective_settings_with_common_config(
                    app_type,
                    provider,
                    common_config_snippet,
                    apply_common_config,
                )?;
                let settings = effective
                    .as_object()
                    .ok_or_else(|| AppError::Config("Codex 配置必须是 JSON 对象".into()))?;
                let auth = settings.get("auth").cloned();
                let cfg_text = settings.get("config").and_then(Value::as_str).unwrap_or("");

                if !cfg_text.trim().is_empty() {
                    crate::codex_config::validate_config_toml(cfg_text)?;
                }

                let mut backup = serde_json::Map::new();
                if let Some(auth) = auth {
                    backup.insert("auth".to_string(), auth);
                }
                backup.insert("config".to_string(), Value::String(cfg_text.to_string()));
                Ok(Value::Object(backup))
            }
            AppType::Gemini => {
                let content_to_write = common_config::build_effective_settings_with_common_config(
                    app_type,
                    provider,
                    common_config_snippet,
                    apply_common_config,
                )?;

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

                let auth = settings.get("auth").ok_or_else(|| {
                    AppError::localized(
                        "provider.codex.auth.missing",
                        format!("供应商 {} 缺少 auth 配置", provider.id),
                        format!("Provider {} is missing auth configuration", provider.id),
                    )
                })?;
                if !auth.is_object() {
                    return Err(AppError::localized(
                        "provider.codex.auth.not_object",
                        format!("供应商 {} 的 auth 配置必须是 JSON 对象", provider.id),
                        format!(
                            "Provider {} auth configuration must be a JSON object",
                            provider.id
                        ),
                    ));
                }

                if let Some(config_value) = settings.get("config") {
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
                }

                if !Self::is_codex_official_provider(provider) {
                    let config_text = settings
                        .get("config")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if !Self::codex_config_has_base_url(config_text) {
                        return Err(AppError::localized(
                            "provider.codex.base_url.missing",
                            format!("供应商 {} 缺少有效的 Codex Base URL", provider.id),
                            format!("Provider {} is missing a valid Codex base_url", provider.id),
                        ));
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
                let config = Self::parse_openclaw_provider_settings(&provider.settings_config)?;
                Self::validate_openclaw_provider_models(&provider.id, &config)?;
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

    pub(crate) fn build_live_backup_snapshot(
        app_type: &AppType,
        provider: &Provider,
        common_config_snippet: Option<&str>,
        apply_common_config: bool,
    ) -> Result<Value, AppError> {
        Self::build_effective_live_snapshot(
            app_type,
            provider,
            common_config_snippet,
            apply_common_config,
        )
    }

    pub(crate) fn build_effective_live_snapshot_from_state(
        state: &AppState,
        app_type: AppType,
        provider: &Provider,
    ) -> Result<Value, AppError> {
        let common_config_snippet = {
            let config = state.config.read().map_err(AppError::from)?;
            config.common_config_snippets.get(&app_type).cloned()
        };

        Self::build_effective_live_snapshot(
            &app_type,
            provider,
            common_config_snippet.as_deref(),
            true,
        )
    }

    pub(crate) fn get_provider(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
    ) -> Result<Provider, AppError> {
        let config = state.config.read().map_err(AppError::from)?;
        let manager = config
            .get_manager(&app_type)
            .ok_or_else(|| Self::app_not_found(&app_type))?;

        manager.providers.get(provider_id).cloned().ok_or_else(|| {
            AppError::localized(
                "provider.not_found",
                format!("供应商不存在: {provider_id}"),
                format!("Provider not found: {provider_id}"),
            )
        })
    }

    fn app_not_found(app_type: &AppType) -> AppError {
        AppError::localized(
            "provider.app_not_found",
            format!("应用类型不存在: {app_type:?}"),
            format!("App type not found: {app_type:?}"),
        )
    }

    pub fn delete(state: &AppState, app_type: AppType, provider_id: &str) -> Result<(), AppError> {
        let (local_current_provider, stored_current_provider) = if app_type.is_additive_mode() {
            (None, None)
        } else {
            (
                crate::settings::get_current_provider(&app_type),
                state.db.get_current_provider(app_type.as_str())?,
            )
        };
        let provider_snapshot = {
            let config = state.config.read().map_err(AppError::from)?;
            let manager = config
                .get_manager(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;

            if !app_type.is_additive_mode()
                && (local_current_provider.as_deref() == Some(provider_id)
                    || stored_current_provider.as_deref() == Some(provider_id))
            {
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
                _ => unreachable!("non-additive apps should not enter additive delete branch"),
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

            if !app_type.is_additive_mode()
                && (local_current_provider.as_deref() == Some(provider_id)
                    || stored_current_provider.as_deref() == Some(provider_id))
            {
                return Err(AppError::localized(
                    "provider.delete.current",
                    "不能删除当前正在使用的供应商",
                    "Cannot delete the provider currently in use",
                ));
            }

            if !app_type.is_additive_mode() && manager.current == provider_id {
                manager.current = stored_current_provider.clone().unwrap_or_default();
            }

            manager.providers.shift_remove(provider_id);
        }

        state.save()
    }

    pub fn import_openclaw_providers_from_live(state: &AppState) -> Result<usize, AppError> {
        live::import_openclaw_providers_from_live(state)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderSortUpdate {
    pub id: String,
    #[serde(rename = "sortIndex")]
    pub sort_index: usize,
}
