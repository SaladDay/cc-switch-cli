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
pub(crate) mod live_merge;
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
use crate::provider::{Provider, ProviderMeta, UsageScript};
use crate::store::AppState;

use gemini_auth::GeminiAuthType;
use live::LiveSnapshot;

pub use common::migrate_legacy_codex_config;
#[cfg(test)]
use common::strip_codex_common_config_from_full_text;

/// 统一会话开关变更后，立即按新开关状态重写当前官方 Codex 供应商的
/// live 配置，使开关即时生效（无需等下一次切换）。
pub fn reapply_current_codex_official_live(state: &AppState) -> Result<bool, AppError> {
    let current_id = ProviderService::current(state, AppType::Codex)?;
    if current_id.is_empty() {
        return Ok(false);
    }
    let providers = state.db.get_all_providers(AppType::Codex.as_str())?;
    let Some(provider) = providers.get(&current_id) else {
        return Ok(false);
    };
    if provider.category.as_deref() != Some("official") {
        return Ok(false);
    }

    let has_live_backup =
        futures::executor::block_on(state.db.get_live_backup(AppType::Codex.as_str()))
            .ok()
            .flatten()
            .is_some();
    let live_taken_over = state
        .proxy_service
        .detect_takeover_in_live_config_for_app(&AppType::Codex);
    if has_live_backup || live_taken_over {
        futures::executor::block_on(
            state
                .proxy_service
                .update_live_backup_from_provider(AppType::Codex.as_str(), provider),
        )
        .map_err(|e| AppError::Message(format!("更新 Live 备份失败: {e}")))?;
        return Ok(true);
    }

    let common_config_snippet = state.db.get_config_snippet(AppType::Codex.as_str())?;
    ProviderService::write_live_snapshot(
        &AppType::Codex,
        provider,
        common_config_snippet.as_deref(),
        true,
    )?;
    Ok(true)
}

/// 供应商相关业务逻辑
pub struct ProviderService;

fn active_failover_last_provider_error() -> AppError {
    AppError::localized(
        "provider.delete.last_failover_queue_entry",
        "代理故障转移激活时，故障转移队列中必须至少保留一个供应商",
        "At least one provider must remain in the failover queue while proxy failover is active",
    )
}

fn provider_key_invalid_error() -> AppError {
    AppError::localized(
        "provider.key.invalid",
        "供应商标识只能包含小写字母、数字和连字符，且不能以连字符开头或结尾",
        "Provider key can only contain lowercase letters, numbers, and hyphens, and cannot start or end with a hyphen",
    )
}

fn current_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn detect_coding_plan_provider_id(base_url: &str) -> Option<&'static str> {
    let url = base_url.to_lowercase();
    if url.contains("api.kimi.com/coding") {
        Some("kimi")
    } else if url.contains("bigmodel.cn") || url.contains("api.z.ai") {
        Some("zhipu")
    } else if url.contains("api.minimaxi.com")
        || url.contains("api.minimax.com")
        || url.contains("api.minimax.io")
    {
        Some("minimax")
    } else {
        None
    }
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
    previous_provider: Option<Provider>,
    backup: LiveSnapshot,
    sync_mcp: bool,
    refresh_snapshot: bool,
    common_config_snippet: Option<String>,
    previous_common_config_snippet: Option<String>,
    takeover_active: bool,
    activate_provider: bool,
}

#[derive(Clone)]
struct PreparedPostCommitAction {
    action: PostCommitAction,
    effect: PreparedPostCommitEffect,
}

#[derive(Clone)]
enum PreparedPostCommitEffect {
    Live(PreparedLiveWrite),
    ProxyLiveBackup(Value),
}

#[derive(Clone)]
enum PreparedLiveWrite {
    Noop,
    Claude {
        settings: Value,
    },
    Codex {
        auth: PreparedCodexAuthWrite,
        config: crate::codex_config::PreparedCodexConfigText,
    },
    Gemini {
        env: std::collections::HashMap<String, String>,
        settings: Value,
        auth_type: GeminiAuthType,
    },
    GeminiSecurityFlag {
        auth_type: GeminiAuthType,
    },
    OpenCode {
        config: Value,
    },
    Hermes {
        providers: serde_yaml::Value,
    },
    OpenClaw {
        models: Value,
    },
}

#[derive(Clone)]
enum PreparedCodexAuthWrite {
    Preserve,
    Write(Value),
    Delete,
}

impl ProviderService {
    pub fn is_provider_key_app(app_type: &AppType) -> bool {
        matches!(app_type, AppType::OpenClaw | AppType::Hermes)
    }

    pub fn is_valid_provider_key(value: &str) -> bool {
        let mut previous_dash = false;
        let mut saw_char = false;
        for ch in value.chars() {
            let valid = ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-';
            if !valid {
                return false;
            }
            if ch == '-' {
                if !saw_char || previous_dash {
                    return false;
                }
                previous_dash = true;
            } else {
                saw_char = true;
                previous_dash = false;
            }
        }
        saw_char && !previous_dash
    }

    pub fn sanitize_provider_key_text(value: &str) -> String {
        value
            .chars()
            .filter_map(|ch| {
                let ch = ch.to_ascii_lowercase();
                (ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-').then_some(ch)
            })
            .collect()
    }

    pub fn generate_provider_key(name: &str, existing_ids: &[String]) -> String {
        let mut base_id = name
            .trim()
            .to_ascii_lowercase()
            .chars()
            .map(|ch| {
                if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
                    ch
                } else {
                    '-'
                }
            })
            .collect::<String>();

        while base_id.contains("--") {
            base_id = base_id.replace("--", "-");
        }
        base_id = base_id.trim_matches('-').to_string();
        if base_id.is_empty() {
            base_id = "provider".to_string();
        }

        if !existing_ids.iter().any(|existing| existing == &base_id) {
            return base_id;
        }

        let mut counter = 1;
        loop {
            let candidate = format!("{base_id}-{counter}");
            if !existing_ids.iter().any(|existing| existing == &candidate) {
                return candidate;
            }
            counter += 1;
        }
    }

    pub fn validate_provider_key_for_add(
        app_type: &AppType,
        provider_id: &str,
    ) -> Result<(), AppError> {
        if Self::is_provider_key_app(app_type) && !Self::is_valid_provider_key(provider_id) {
            return Err(provider_key_invalid_error());
        }
        Ok(())
    }

    fn provider_copy_id(original_id: &str, existing_ids: &HashSet<String>) -> String {
        let base_id = format!("{}-copy", original_id.trim());

        if !existing_ids.contains(&base_id) {
            return base_id;
        }

        let mut counter = 2;
        loop {
            let candidate = format!("{base_id}-{counter}");
            if !existing_ids.contains(&candidate) {
                return candidate;
            }
            counter += 1;
        }
    }

    fn live_provider_ids(app_type: &AppType) -> Result<HashSet<String>, AppError> {
        let ids = match app_type {
            AppType::OpenCode => crate::opencode_config::get_providers()?
                .into_iter()
                .map(|(id, _)| id)
                .collect(),
            AppType::Hermes => crate::hermes_config::get_providers()?
                .into_iter()
                .map(|(id, _)| id)
                .collect(),
            AppType::OpenClaw => crate::openclaw_config::get_providers()?
                .into_iter()
                .map(|(id, _)| id)
                .collect(),
            _ => HashSet::new(),
        };
        Ok(ids)
    }

    fn duplicate_provider_with_overrides(
        source: &Provider,
        provider: Option<Provider>,
        existing_ids: &HashSet<String>,
    ) -> Provider {
        let mut duplicate = provider.unwrap_or_else(|| {
            let mut duplicate = source.clone();
            duplicate.name = format!("{} copy", source.name.trim());
            duplicate
        });
        duplicate.id = Self::provider_copy_id(&source.id, existing_ids);
        duplicate.name = if duplicate.name.trim().is_empty() {
            format!("{} copy", source.name.trim())
        } else {
            duplicate.name.trim().to_string()
        };
        duplicate.created_at = Some(current_timestamp());
        duplicate.in_failover_queue = false;
        duplicate.sort_index = source.sort_index.map(|idx| idx + 1);
        duplicate
    }

    fn normalize_duplicate_provider_snapshot(app_type: &AppType, provider: &mut Provider) {
        if !matches!(app_type, AppType::Hermes) {
            return;
        }

        if let Some(settings) = provider.settings_config.as_object_mut() {
            settings.remove(crate::hermes_config::PROVIDER_SOURCE_FIELD);
            settings.remove("provider_key");
        }
    }

    fn shift_sort_indices_for_duplicate(
        manager: &mut crate::provider::ProviderManager,
        source_id: &str,
        insert_sort_index: Option<usize>,
    ) {
        let Some(insert_sort_index) = insert_sort_index else {
            return;
        };

        for (id, provider) in manager.providers.iter_mut() {
            if id != source_id
                && provider
                    .sort_index
                    .is_some_and(|idx| idx >= insert_sort_index)
            {
                provider.sort_index = provider.sort_index.map(|idx| idx + 1);
            }
        }
    }

    pub fn duplicate(
        state: &AppState,
        app_type: AppType,
        source_id: &str,
        provider_override: Option<Provider>,
    ) -> Result<Provider, AppError> {
        let app_type_clone = app_type.clone();
        let source_id = source_id.to_string();
        let live_ids = if app_type.is_additive_mode() {
            Self::live_provider_ids(&app_type)?
        } else {
            HashSet::new()
        };

        Self::run_transaction(state, move |config| {
            let common_config_snippet = config.common_config_snippets.get(&app_type_clone).cloned();
            config.ensure_app(&app_type_clone);
            let manager = config
                .get_manager_mut(&app_type_clone)
                .ok_or_else(|| Self::app_not_found(&app_type_clone))?;
            let source = manager
                .providers
                .get(&source_id)
                .ok_or_else(|| {
                    AppError::localized(
                        "provider.not_found",
                        format!("供应商不存在: {source_id}"),
                        format!("Provider not found: {source_id}"),
                    )
                })?
                .clone();

            let mut existing_ids = manager.providers.keys().cloned().collect::<HashSet<_>>();
            existing_ids.extend(live_ids);
            let mut duplicate =
                Self::duplicate_provider_with_overrides(&source, provider_override, &existing_ids);
            Self::normalize_duplicate_provider_snapshot(&app_type_clone, &mut duplicate);

            Self::normalize_provider_if_claude(&app_type_clone, &mut duplicate);
            Self::inject_coding_plan_usage_script(&app_type_clone, &mut duplicate);
            Self::validate_provider_settings(&app_type_clone, &duplicate)?;
            Self::normalize_provider_for_storage(
                &app_type_clone,
                &mut duplicate,
                common_config_snippet.as_deref(),
            )?;

            if app_type_clone.is_additive_mode() {
                Self::set_provider_live_config_managed(&mut duplicate, false);
            }

            Self::shift_sort_indices_for_duplicate(manager, &source_id, duplicate.sort_index);
            manager
                .providers
                .insert(duplicate.id.clone(), duplicate.clone());

            Ok((duplicate, None))
        })
    }

    fn inject_coding_plan_usage_script(app_type: &AppType, provider: &mut Provider) {
        if !matches!(app_type, AppType::Claude) {
            return;
        }
        if provider
            .meta
            .as_ref()
            .and_then(|meta| meta.usage_script.as_ref())
            .is_some()
        {
            return;
        }

        let base_url = provider
            .settings_config
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let Some(coding_plan_provider) = detect_coding_plan_provider_id(base_url) else {
            return;
        };

        provider
            .meta
            .get_or_insert_with(ProviderMeta::default)
            .usage_script = Some(UsageScript {
            enabled: true,
            language: "javascript".to_string(),
            code: String::new(),
            timeout: Some(10),
            api_key: None,
            base_url: None,
            access_token: None,
            user_id: None,
            template_type: Some("token_plan".to_string()),
            auto_query_interval: Some(5),
            coding_plan_provider: Some(coding_plan_provider.to_string()),
        });
    }

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

    pub(crate) fn codex_live_write_category(provider: &Provider) -> Option<&str> {
        if Self::is_codex_official_provider(provider) {
            Some("official")
        } else {
            provider.category.as_deref()
        }
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
                manager
                    .providers
                    .values()
                    .filter(|provider| Self::provider_live_config_managed(provider) != Some(false))
                    .cloned()
                    .collect::<Vec<_>>(),
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

    fn provider_live_config_managed(provider: &Provider) -> Option<bool> {
        provider
            .meta
            .as_ref()
            .and_then(|meta| meta.live_config_managed)
    }

    fn set_provider_live_config_managed(provider: &mut Provider, managed: bool) {
        provider
            .meta
            .get_or_insert_with(Default::default)
            .live_config_managed = Some(managed);
    }

    fn additive_provider_exists_in_live_config(
        app_type: &AppType,
        provider_id: &str,
        live_config_managed: Option<bool>,
    ) -> Result<bool, AppError> {
        let read_presence = || match app_type {
            AppType::OpenCode => crate::opencode_config::get_providers()
                .map(|providers| providers.contains_key(provider_id)),
            AppType::Hermes => crate::hermes_config::get_providers()
                .map(|providers| providers.contains_key(provider_id)),
            AppType::OpenClaw => Self::valid_openclaw_live_provider_ids()
                .map(|ids| ids.is_some_and(|ids| ids.contains(provider_id))),
            _ => Ok(false),
        };

        if live_config_managed == Some(false) {
            Ok(read_presence().unwrap_or(false))
        } else {
            read_presence()
        }
    }

    #[allow(dead_code)]
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
        Self::run_transaction_with_resolution(state, live_merge::ConflictPolicy::Fail.into(), f)
    }

    fn run_transaction_with_resolution<R, F>(
        state: &AppState,
        resolution: live_merge::ConflictResolution<'_>,
        f: F,
    ) -> Result<R, AppError>
    where
        F: FnOnce(&mut MultiAppConfig) -> Result<(R, Option<PostCommitAction>), AppError>,
    {
        Self::run_staged_transaction(state, None, resolution, f)
    }

    fn run_staged_transaction<R, F>(
        state: &AppState,
        preserved_current_apps: Option<&[AppType]>,
        resolution: live_merge::ConflictResolution<'_>,
        f: F,
    ) -> Result<R, AppError>
    where
        F: FnOnce(&mut MultiAppConfig) -> Result<(R, Option<PostCommitAction>), AppError>,
    {
        let original = {
            let guard = state.config.read().map_err(AppError::from)?;
            guard.clone()
        };
        let mut candidate = original.clone();
        let (result, action) = f(&mut candidate)?;
        let prepared = action
            .map(|action| Self::prepare_post_commit_action(state, action, resolution))
            .transpose()?;

        {
            let mut guard = state.config.write().map_err(AppError::from)?;
            *guard = candidate.clone();
        }

        let save_result = match preserved_current_apps {
            Some(apps) => state.save_config_snapshot_preserving_current_providers(&candidate, apps),
            None => state.save_config_snapshot(&candidate),
        };

        if let Err(save_err) = save_result {
            let rollback_result = match preserved_current_apps {
                Some(apps) => {
                    Self::restore_config_only_preserving_current_providers(state, original, apps)
                }
                None => Self::restore_config_only(state, original),
            };
            if let Err(rollback_err) = rollback_result {
                return Err(AppError::localized(
                    "config.save.rollback_failed",
                    format!("保存配置失败: {save_err}；回滚失败: {rollback_err}"),
                    format!("Failed to save config: {save_err}; rollback failed: {rollback_err}"),
                ));
            }
            return Err(save_err);
        }

        if let Some(prepared) = prepared {
            if let Err(err) = Self::apply_prepared_post_commit_action(state, &prepared) {
                let rollback_result = match preserved_current_apps {
                    Some(apps) => Self::rollback_after_failure_preserving_current_providers(
                        state,
                        original,
                        apps,
                        prepared.action.backup.clone(),
                    ),
                    None => Self::rollback_after_failure(
                        state,
                        original,
                        prepared.action.backup.clone(),
                    ),
                };
                if let Err(rollback_err) = rollback_result {
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

    #[expect(
        dead_code,
        reason = "kept for callers that preserve current providers without custom resolution"
    )]
    fn run_transaction_preserving_current_providers<R, F>(
        state: &AppState,
        preserved_current_apps: &[AppType],
        f: F,
    ) -> Result<R, AppError>
    where
        F: FnOnce(&mut MultiAppConfig) -> Result<(R, Option<PostCommitAction>), AppError>,
    {
        Self::run_transaction_preserving_current_providers_with_resolution(
            state,
            preserved_current_apps,
            live_merge::ConflictPolicy::Fail.into(),
            f,
        )
    }

    fn run_transaction_preserving_current_providers_with_resolution<R, F>(
        state: &AppState,
        preserved_current_apps: &[AppType],
        resolution: live_merge::ConflictResolution<'_>,
        f: F,
    ) -> Result<R, AppError>
    where
        F: FnOnce(&mut MultiAppConfig) -> Result<(R, Option<PostCommitAction>), AppError>,
    {
        Self::run_staged_transaction(state, Some(preserved_current_apps), resolution, f)
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

    fn prepare_post_commit_action(
        state: &AppState,
        action: PostCommitAction,
        resolution: live_merge::ConflictResolution<'_>,
    ) -> Result<PreparedPostCommitAction, AppError> {
        let effect = if action.takeover_active {
            let backup_snapshot = futures::executor::block_on(
                state
                    .proxy_service
                    .prepare_live_backup_from_provider_with_resolution(
                        action.app_type.as_str(),
                        &action.provider,
                        resolution,
                    ),
            )
            .map_err(AppError::Message)?;
            PreparedPostCommitEffect::ProxyLiveBackup(backup_snapshot)
        } else {
            let apply_common_config = action
                .provider
                .meta
                .as_ref()
                .and_then(|meta| meta.apply_common_config)
                .unwrap_or(false);
            PreparedPostCommitEffect::Live(Self::prepare_live_snapshot_with_resolution(
                &action.app_type,
                &action.provider,
                action.previous_provider.as_ref(),
                action.common_config_snippet.as_deref(),
                action.previous_common_config_snippet.as_deref(),
                apply_common_config,
                resolution,
            )?)
        };

        Ok(PreparedPostCommitAction { action, effect })
    }

    fn apply_prepared_post_commit_action(
        state: &AppState,
        prepared: &PreparedPostCommitAction,
    ) -> Result<(), AppError> {
        match &prepared.effect {
            PreparedPostCommitEffect::Live(live) => {
                Self::apply_prepared_live_snapshot(live)?;
                if prepared.action.activate_provider
                    && matches!(prepared.action.app_type, AppType::Hermes)
                {
                    crate::hermes_config::set_current_provider(
                        &prepared.action.provider.id,
                        &prepared.action.provider.settings_config,
                    )?;
                }
            }
            PreparedPostCommitEffect::ProxyLiveBackup(snapshot) => {
                futures::executor::block_on(
                    state
                        .proxy_service
                        .save_live_backup_snapshot(prepared.action.app_type.as_str(), snapshot),
                )
                .map_err(AppError::Message)?;
            }
        }

        if prepared.action.sync_mcp {
            use crate::services::mcp::McpService;
            McpService::sync_all_enabled(state)?;
        }
        if !prepared.action.takeover_active
            && prepared.action.refresh_snapshot
            && crate::sync_policy::should_sync_live(&prepared.action.app_type)
        {
            Self::refresh_provider_snapshot(
                state,
                &prepared.action.app_type,
                &prepared.action.provider.id,
            )?;
        }

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
                let mut settings_for_storage = Value::Object(raw_settings);
                let restore_provider_token =
                    crate::codex_config::should_restore_codex_provider_token_for_backfill(
                        Self::codex_live_write_category(&provider),
                        &provider.settings_config,
                    );
                if let Err(err) = crate::codex_config::restore_codex_settings_for_backfill(
                    &mut settings_for_storage,
                    &provider.settings_config,
                    restore_provider_token,
                ) {
                    log::warn!(
                        "Failed to restore Codex settings while refreshing '{}': {err}",
                        provider.id
                    );
                }
                if Self::codex_live_write_category(&provider) == Some("official") {
                    if let Err(err) =
                        crate::codex_config::strip_codex_unified_session_bucket_from_settings(
                            &mut settings_for_storage,
                        )
                    {
                        log::warn!(
                            "Failed to strip unified session bucket while refreshing '{}': {err}",
                            provider.id
                        );
                    }
                }
                let mut snapshot_provider = provider.clone();
                snapshot_provider.settings_config = settings_for_storage;

                {
                    let mut guard = state.config.write().map_err(AppError::from)?;
                    let did_auto_extract = !common_snippet_extracted.trim().is_empty()
                        && guard
                            .common_config_snippets
                            .codex
                            .as_deref()
                            .unwrap_or_default()
                            .trim()
                            .is_empty();
                    if did_auto_extract {
                        guard.common_config_snippets.codex = Some(common_snippet_extracted.clone());
                        Self::migrate_codex_common_config_snippet(
                            &mut guard,
                            None,
                            common_snippet_extracted.as_str(),
                        )?;
                    }

                    if did_auto_extract {
                        snapshot_provider = Self::migrate_provider_snapshot_for_storage(
                            app_type,
                            &snapshot_provider,
                            effective_common_snippet.as_deref(),
                        )?;
                    } else {
                        Self::normalize_provider_for_storage(
                            app_type,
                            &mut snapshot_provider,
                            effective_common_snippet.as_deref(),
                        )?;
                    }
                    if let Some(manager) = guard.get_manager_mut(app_type) {
                        if let Some(target) = manager.providers.get_mut(provider_id) {
                            *target = snapshot_provider;
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
            AppType::Hermes => {
                let providers = crate::hermes_config::get_providers()?;
                let live_after = providers.get(provider_id).cloned().ok_or_else(|| {
                    AppError::localized(
                        "hermes.live.missing_provider",
                        format!("Hermes live 配置中缺少供应商: {provider_id}"),
                        format!("Hermes live config missing provider: {provider_id}"),
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
            AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => Ok(()),
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
        previous_common_config_snippet: Option<String>,
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
            current_provider_id,
            takeover_active,
            previous_common_config_snippet,
        )
    }

    fn build_post_commit_action_for_current_provider(
        config: &MultiAppConfig,
        app_type: &AppType,
        current_provider_id: &str,
        takeover_active: bool,
        previous_common_config_snippet: Option<String>,
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
            previous_provider: None,
            backup: Self::capture_live_snapshot(app_type)?,
            sync_mcp: matches!(app_type, AppType::Codex) && !takeover_active,
            refresh_snapshot: false,
            common_config_snippet: config.common_config_snippets.get(app_type).cloned(),
            previous_common_config_snippet,
            takeover_active,
            activate_provider: false,
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

    pub(crate) fn provider_uses_common_config_for_app(
        app_type: &AppType,
        provider: &Provider,
        common_config_snippet: Option<&str>,
    ) -> bool {
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

    fn live_settings_differ_from_provider_snapshot(
        app_type: &AppType,
        live_settings: &Value,
        provider_settings: &Value,
    ) -> bool {
        match app_type {
            AppType::Codex => {
                live_settings.get("config").and_then(Value::as_str)
                    != provider_settings.get("config").and_then(Value::as_str)
            }
            AppType::Gemini => live_settings.get("env") != provider_settings.get("env"),
            AppType::Claude => live_settings != provider_settings,
            AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => false,
        }
    }

    fn live_settings_indicate_common_config_usage(
        app_type: &AppType,
        provider: &Provider,
        old_snippet: &str,
    ) -> bool {
        let Ok(live_settings) = Self::read_live_settings(app_type.clone()) else {
            return false;
        };

        if common_config::settings_contain_common_config(app_type, &live_settings, old_snippet) {
            return true;
        }

        Self::validate_common_config_snippet(app_type, Some(old_snippet)).is_err()
            && Self::live_settings_differ_from_provider_snapshot(
                app_type,
                &live_settings,
                &provider.settings_config,
            )
    }

    fn mark_provider_common_config_enabled_if_unset(
        config: &mut MultiAppConfig,
        app_type: &AppType,
        provider_id: Option<&str>,
    ) {
        let Some(provider_id) = provider_id else {
            return;
        };
        let Some(provider) = config
            .get_manager_mut(app_type)
            .and_then(|manager| manager.providers.get_mut(provider_id))
        else {
            return;
        };

        if provider
            .meta
            .as_ref()
            .and_then(|meta| meta.apply_common_config)
            .is_none()
        {
            provider
                .meta
                .get_or_insert_with(Default::default)
                .apply_common_config = Some(true);
        }
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

    pub(crate) fn migrate_provider_snapshot_for_storage(
        app_type: &AppType,
        provider: &Provider,
        common_config_snippet: Option<&str>,
    ) -> Result<Provider, AppError> {
        let mut snapshot_provider = provider.clone();
        common_config::migrate_provider_subset_usage_for_storage(
            app_type,
            &mut snapshot_provider,
            common_config_snippet,
        )?;
        Ok(snapshot_provider)
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

    pub(crate) fn settings_contain_common_config_for_preview(
        app_type: &AppType,
        settings_config: &Value,
        common_config_snippet: &str,
    ) -> bool {
        common_config::settings_contain_common_config(
            app_type,
            settings_config,
            common_config_snippet,
        )
    }

    pub fn extract_common_config_snippet(
        state: &AppState,
        app_type: AppType,
    ) -> Result<String, AppError> {
        let current_id = Self::current(state, app_type.clone())?;
        if current_id.trim().is_empty() {
            return Err(AppError::Message("No current provider".to_string()));
        }

        let providers = state.db.get_all_providers(app_type.as_str())?;
        let provider = providers
            .get(&current_id)
            .ok_or_else(|| AppError::Message(format!("Provider {current_id} not found")))?;

        Self::extract_common_config_snippet_from_settings(app_type, &provider.settings_config)
    }

    pub fn extract_common_config_snippet_from_settings(
        app_type: AppType,
        settings_config: &Value,
    ) -> Result<String, AppError> {
        match app_type {
            AppType::Claude => Self::extract_claude_common_config(settings_config),
            AppType::Codex => Self::extract_codex_common_config(settings_config),
            AppType::Gemini => Self::extract_gemini_common_config(settings_config),
            AppType::OpenCode => Self::extract_opencode_common_config(settings_config),
            AppType::Hermes => Self::extract_opencode_common_config(settings_config),
            AppType::OpenClaw => Self::extract_openclaw_common_config(settings_config),
        }
    }

    fn extract_claude_common_config(settings: &Value) -> Result<String, AppError> {
        let mut config = settings.clone();

        const ENV_EXCLUDES: &[&str] = &[
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_MODEL",
            "ANTHROPIC_REASONING_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_BASE_URL",
        ];
        const TOP_LEVEL_EXCLUDES: &[&str] = &["apiBaseUrl", "primaryModel", "smallFastModel"];

        if let Some(env) = config.get_mut("env").and_then(Value::as_object_mut) {
            for key in ENV_EXCLUDES {
                env.remove(*key);
            }
            if env.is_empty() {
                if let Some(obj) = config.as_object_mut() {
                    obj.remove("env");
                }
            }
        }

        if let Some(obj) = config.as_object_mut() {
            for key in TOP_LEVEL_EXCLUDES {
                obj.remove(*key);
            }
        }

        if config.as_object().is_none_or(|obj| obj.is_empty()) {
            return Ok("{}".to_string());
        }

        serde_json::to_string_pretty(&config)
            .map_err(|e| AppError::Message(format!("Serialization failed: {e}")))
    }

    fn extract_codex_common_config(settings: &Value) -> Result<String, AppError> {
        let config_toml = settings
            .get("config")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Self::extract_codex_common_config_from_config_toml(config_toml)
    }

    fn extract_gemini_common_config(settings: &Value) -> Result<String, AppError> {
        let env = settings.get("env").and_then(Value::as_object);

        let mut snippet = serde_json::Map::new();
        if let Some(env) = env {
            for (key, value) in env {
                if key == "GOOGLE_GEMINI_BASE_URL" || key == "GEMINI_API_KEY" {
                    continue;
                }
                let Value::String(v) = value else {
                    continue;
                };
                let trimmed = v.trim();
                if !trimmed.is_empty() {
                    snippet.insert(key.to_string(), Value::String(trimmed.to_string()));
                }
            }
        }

        if snippet.is_empty() {
            return Ok("{}".to_string());
        }

        serde_json::to_string_pretty(&Value::Object(snippet))
            .map_err(|e| AppError::Message(format!("Serialization failed: {e}")))
    }

    fn extract_opencode_common_config(settings: &Value) -> Result<String, AppError> {
        let mut config = settings.clone();

        if let Some(obj) = config.as_object_mut() {
            if let Some(options) = obj.get_mut("options").and_then(Value::as_object_mut) {
                options.remove("apiKey");
                options.remove("baseURL");
            }
        }

        if config.is_null() || config.as_object().is_some_and(|obj| obj.is_empty()) {
            return Ok("{}".to_string());
        }

        serde_json::to_string_pretty(&config)
            .map_err(|e| AppError::Message(format!("Serialization failed: {e}")))
    }

    fn extract_openclaw_common_config(settings: &Value) -> Result<String, AppError> {
        let mut config = settings.clone();

        if let Some(obj) = config.as_object_mut() {
            obj.remove("apiKey");
            obj.remove("baseUrl");
        }

        if config.is_null() || config.as_object().is_some_and(|obj| obj.is_empty()) {
            return Ok("{}".to_string());
        }

        serde_json::to_string_pretty(&config)
            .map_err(|e| AppError::Message(format!("Serialization failed: {e}")))
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
            common_config::normalize_provider_common_config_for_storage(
                app_type,
                provider,
                common_config_snippet,
            )?;
        }

        Ok(())
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
            common_config::normalize_provider_common_config_for_storage(
                app_type,
                current_provider,
                common_config_snippet,
            )?;
        }

        for (provider_id, provider) in manager.providers.iter_mut() {
            if provider_id == &current_provider_id {
                continue;
            }

            if let Err(err) = common_config::normalize_provider_common_config_for_storage(
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
        Self::set_common_config_snippet_with_resolution(
            state,
            app_type,
            snippet,
            live_merge::ConflictPolicy::Fail.into(),
        )
    }

    pub(crate) fn set_common_config_snippet_with_resolution(
        state: &AppState,
        app_type: AppType,
        snippet: Option<String>,
        resolution: live_merge::ConflictResolution<'_>,
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
        let old_snippet_before_transaction = if app_type.is_additive_mode() {
            None
        } else {
            let config = state.config.read().map_err(AppError::from)?;
            config
                .common_config_snippets
                .get(&app_type)
                .cloned()
                .filter(|value| !value.trim().is_empty())
        };
        let current_live_uses_old_common_config =
            if let (Some(current_provider_id), Some(old_snippet)) = (
                effective_current_provider.as_deref(),
                old_snippet_before_transaction.as_deref(),
            ) {
                db_providers
                    .as_ref()
                    .and_then(|providers| providers.get(current_provider_id))
                    .is_some_and(|provider| {
                        common_config::provider_uses_common_config(
                            &app_type,
                            provider,
                            Some(old_snippet),
                        ) || Self::live_settings_indicate_common_config_usage(
                            &app_type,
                            provider,
                            old_snippet,
                        )
                    })
            } else {
                false
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

        Self::run_transaction_preserving_current_providers_with_resolution(
            state,
            std::slice::from_ref(&app_type),
            resolution,
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
                if current_live_uses_old_common_config {
                    Self::mark_provider_common_config_enabled_if_unset(
                        config,
                        &app_type_clone,
                        effective_current_provider.as_deref(),
                    );
                }

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
                    old_snippet,
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
        if matches!(app_type, AppType::Hermes) {
            return crate::hermes_config::get_current_provider_id()
                .map(|opt| opt.unwrap_or_default());
        }
        if app_type.is_additive_mode() {
            return Ok(String::new());
        }
        crate::settings::get_effective_current_provider(&state.db, &app_type)
            .map(|opt| opt.unwrap_or_default())
    }

    /// 新增供应商
    pub fn add(state: &AppState, app_type: AppType, provider: Provider) -> Result<bool, AppError> {
        Self::add_with_resolution(
            state,
            app_type,
            provider,
            live_merge::ConflictPolicy::Fail.into(),
        )
    }

    pub(crate) fn add_with_resolution(
        state: &AppState,
        app_type: AppType,
        provider: Provider,
        resolution: live_merge::ConflictResolution<'_>,
    ) -> Result<bool, AppError> {
        let mut provider = provider;
        // 归一化 Claude 模型键
        Self::normalize_provider_if_claude(&app_type, &mut provider);
        Self::inject_coding_plan_usage_script(&app_type, &mut provider);
        Self::validate_provider_settings(&app_type, &provider)?;

        let app_type_clone = app_type.clone();
        let provider_clone = provider.clone();
        let stored_current_provider = if app_type.is_additive_mode() {
            None
        } else {
            state.db.get_current_provider(app_type.as_str())?
        };

        Self::run_transaction_with_resolution(state, resolution, move |config| {
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
            if app_type_clone.is_additive_mode() {
                Self::set_provider_live_config_managed(&mut provider_to_store, true);
            }
            Self::validate_provider_key_for_add(&app_type_clone, &provider_to_store.id)?;

            config.ensure_app(&app_type_clone);
            let manager = config
                .get_manager_mut(&app_type_clone)
                .ok_or_else(|| Self::app_not_found(&app_type_clone))?;

            if !app_type_clone.is_additive_mode() {
                manager.current = stored_current_provider.clone().unwrap_or_default();
            }

            let was_empty = manager.providers.is_empty();
            if manager.providers.contains_key(&provider_to_store.id) {
                return Err(AppError::localized(
                    "provider.id.exists",
                    format!("供应商 ID 已存在: {}", provider_to_store.id),
                    format!("Provider ID already exists: {}", provider_to_store.id),
                ));
            }
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
                    previous_provider: None,
                    backup,
                    // Codex current-provider saves rewrite live config from the stored snapshot,
                    // so managed MCP must be synced back after the write.
                    sync_mcp: matches!(&app_type_clone, AppType::Codex),
                    refresh_snapshot: false,
                    common_config_snippet,
                    previous_common_config_snippet: None,
                    takeover_active: false,
                    activate_provider: false,
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
        Self::update_with_resolution(
            state,
            app_type,
            provider,
            live_merge::ConflictPolicy::Fail.into(),
        )
    }

    pub(crate) fn update_with_resolution(
        state: &AppState,
        app_type: AppType,
        provider: Provider,
        resolution: live_merge::ConflictResolution<'_>,
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

        Self::run_transaction_with_resolution(state, resolution, move |config| {
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

            let existing_live_config_managed = manager
                .providers
                .get(&provider_id)
                .and_then(Self::provider_live_config_managed);
            let current_live_uses_existing_common_config = !app_type_clone.is_additive_mode()
                && effective_current_provider.as_deref() == Some(provider_id.as_str())
                && common_config_snippet
                    .as_deref()
                    .filter(|snippet| !snippet.trim().is_empty())
                    .is_some_and(|snippet| {
                        manager.providers.get(&provider_id).is_some_and(|existing| {
                            common_config::provider_uses_common_config(
                                &app_type_clone,
                                existing,
                                Some(snippet),
                            ) || Self::live_settings_indicate_common_config_usage(
                                &app_type_clone,
                                existing,
                                snippet,
                            )
                        })
                    });
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

            if current_live_uses_existing_common_config
                && merged
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.apply_common_config)
                    .is_none()
            {
                merged
                    .meta
                    .get_or_insert_with(Default::default)
                    .apply_common_config = Some(true);
            }

            Self::normalize_provider_for_storage(
                &app_type_clone,
                &mut merged,
                common_config_snippet.as_deref(),
            )?;

            let should_write_live = if app_type_clone.is_additive_mode() {
                let live_config_managed = Self::additive_provider_exists_in_live_config(
                    &app_type_clone,
                    &provider_id,
                    Self::provider_live_config_managed(&merged).or(existing_live_config_managed),
                )?;
                Self::set_provider_live_config_managed(&mut merged, live_config_managed);
                live_config_managed
            } else {
                effective_current_provider.as_deref() == Some(provider_id.as_str())
            };

            let previous_provider = manager.providers.get(&provider_id).cloned();
            manager
                .providers
                .insert(provider_id.clone(), merged.clone());

            let action = if should_write_live {
                let backup = Self::capture_live_snapshot(&app_type_clone)?;
                Some(PostCommitAction {
                    app_type: app_type_clone.clone(),
                    provider: merged,
                    previous_provider,
                    backup,
                    // Codex current-provider saves rewrite live config from the stored snapshot,
                    // so managed MCP must be synced back after the write.
                    sync_mcp: matches!(&app_type_clone, AppType::Codex),
                    refresh_snapshot: false,
                    common_config_snippet,
                    previous_common_config_snippet: None,
                    takeover_active: false,
                    activate_provider: false,
                })
            } else {
                None
            };

            Ok((true, action))
        })
    }

    /// 导入当前 live 配置为默认供应商。
    ///
    /// 返回 `Ok(true)` 表示实际导入，`Ok(false)` 表示该 app 已有非官方 seed provider 而跳过。
    pub fn import_default_config(state: &AppState, app_type: AppType) -> Result<bool, AppError> {
        if app_type.is_additive_mode() {
            return Ok(false);
        }

        if state.db.has_non_official_seed_provider(app_type.as_str())? {
            return Ok(false);
        }

        let settings_config = match app_type {
            AppType::Codex => crate::codex_config::read_codex_live_settings_with_model_catalog()?,
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
            AppType::Hermes => unreachable!("additive mode apps are handled earlier"),
            AppType::OpenClaw => unreachable!("additive mode apps are handled earlier"),
        };

        let mut provider = Provider::with_id(
            "default".to_string(),
            "default".to_string(),
            settings_config,
            None,
        );
        provider.category = Some(
            if matches!(app_type, AppType::Codex) {
                let config_text = provider
                    .settings_config
                    .get("config")
                    .and_then(Value::as_str);
                let has_provider_key = crate::codex_config::extract_codex_api_key(
                    provider.settings_config.get("auth"),
                    config_text,
                )
                .is_some();
                let has_login_material = provider
                    .settings_config
                    .get("auth")
                    .is_some_and(crate::codex_config::codex_auth_has_login_material);

                if has_login_material && !has_provider_key {
                    "official"
                } else {
                    "custom"
                }
            } else {
                "custom"
            }
            .to_string(),
        );
        if matches!(app_type, AppType::Codex) && provider.category.as_deref() == Some("official") {
            crate::codex_config::strip_codex_unified_session_bucket_from_settings(
                &mut provider.settings_config,
            )?;
        }

        state.db.save_provider(app_type.as_str(), &provider)?;
        state
            .db
            .set_current_provider(app_type.as_str(), &provider.id)?;
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
        Ok(true)
    }

    /// 读取当前 live 配置
    pub fn read_live_settings(app_type: AppType) -> Result<Value, AppError> {
        match app_type {
            AppType::Codex => crate::codex_config::read_codex_live_settings_with_model_catalog(),
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
            AppType::Hermes => {
                let config_path = crate::hermes_config::get_hermes_config_path();
                if !config_path.exists() {
                    return Err(AppError::localized(
                        "hermes.config.missing",
                        "Hermes 配置文件不存在",
                        "Hermes configuration file not found",
                    ));
                }
                crate::hermes_config::read_hermes_config_json()
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

    pub fn remove_from_live_config(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
    ) -> Result<(), AppError> {
        if !app_type.is_additive_mode() {
            return Err(AppError::localized(
                "provider.remove_from_live_config.unsupported",
                "只有累加模式应用支持从 live 配置中移除供应商",
                "Only additive-mode apps support removing a provider from live config",
            ));
        }

        Self::guard_additive_default_provider_removal(&app_type, provider_id)?;

        let original = {
            let config = state.config.read().map_err(AppError::from)?;
            let manager = config
                .get_manager(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;
            if !manager.providers.contains_key(provider_id) {
                return Err(AppError::localized(
                    "provider.not_found",
                    format!("供应商不存在: {provider_id}"),
                    format!("Provider not found: {provider_id}"),
                ));
            }
            config.clone()
        };

        let backup = Self::capture_live_snapshot(&app_type)?;
        match &app_type {
            AppType::OpenCode => {
                if crate::opencode_config::get_opencode_dir().exists() {
                    crate::opencode_config::remove_provider(provider_id)?;
                }
            }
            AppType::Hermes => {
                if crate::hermes_config::get_hermes_dir().exists() {
                    crate::hermes_config::remove_provider(provider_id)?;
                }
            }
            AppType::OpenClaw => {
                if crate::openclaw_config::get_openclaw_dir().exists() {
                    crate::openclaw_config::remove_provider(provider_id)?;
                }
            }
            _ => unreachable!("non-additive apps should not enter remove-from-live branch"),
        }

        {
            let mut config = state.config.write().map_err(AppError::from)?;
            let manager = config
                .get_manager_mut(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;
            let provider = manager.providers.get_mut(provider_id).ok_or_else(|| {
                AppError::localized(
                    "provider.not_found",
                    format!("供应商不存在: {provider_id}"),
                    format!("Provider not found: {provider_id}"),
                )
            })?;
            Self::set_provider_live_config_managed(provider, false);
        }

        if let Err(save_err) = state.save() {
            let config_restore = Self::restore_config_only(state, original);
            let live_restore = backup.restore();
            if let Err(rollback_err) = config_restore {
                return Err(AppError::localized(
                    "config.save.rollback_failed",
                    format!("保存配置失败: {save_err}；回滚失败: {rollback_err}"),
                    format!("Failed to save config: {save_err}; rollback failed: {rollback_err}"),
                ));
            }
            if let Err(rollback_err) = live_restore {
                return Err(AppError::localized(
                    "post_commit.rollback_failed",
                    format!("保存配置失败: {save_err}；live 回滚失败: {rollback_err}"),
                    format!(
                        "Failed to save config: {save_err}; live rollback failed: {rollback_err}"
                    ),
                ));
            }
            return Err(save_err);
        }

        Ok(())
    }

    pub fn import_live_config(state: &AppState, app_type: AppType) -> Result<usize, AppError> {
        match app_type {
            AppType::OpenCode => Self::import_opencode_providers_from_live(state),
            AppType::OpenClaw => Self::import_openclaw_providers_from_live(state),
            AppType::Hermes => Self::import_hermes_providers_from_live(state),
            _ => Self::import_default_config(state, app_type).map(usize::from),
        }
    }

    pub fn set_default_model(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
        model_id: Option<&str>,
    ) -> Result<String, AppError> {
        match app_type {
            AppType::Hermes => {
                Self::switch(state, AppType::Hermes, provider_id)?;
                Ok(provider_id.to_string())
            }
            AppType::OpenClaw => Self::set_openclaw_default_model(provider_id, model_id),
            _ => Err(AppError::localized(
                "provider.set_default_model.unsupported",
                "只有 Hermes 和 OpenClaw 支持设置默认供应商/模型",
                "Only Hermes and OpenClaw support setting a default provider/model",
            )),
        }
    }

    fn set_openclaw_default_model(
        provider_id: &str,
        model_id: Option<&str>,
    ) -> Result<String, AppError> {
        let live_provider = crate::openclaw_config::get_providers()?
            .remove(provider_id)
            .ok_or_else(|| {
                AppError::localized(
                    "provider.set_default_model.openclaw_provider_missing",
                    format!("请先将该 OpenClaw 供应商加入当前配置: {provider_id}"),
                    format!(
                        "Add this OpenClaw provider to the current config first: {provider_id}"
                    ),
                )
            })?;
        let ordered_model_ids = Self::openclaw_provider_model_ids(&live_provider);
        if ordered_model_ids.is_empty() {
            return Err(AppError::localized(
                "provider.set_default_model.openclaw_no_models",
                "该 OpenClaw 供应商在当前配置中没有可用模型",
                "This OpenClaw provider has no models in the current config",
            ));
        }

        let selected_model_id = match model_id {
            Some(id) if ordered_model_ids.iter().any(|candidate| candidate == id) => id.to_string(),
            Some(id) => {
                return Err(AppError::localized(
                    "provider.set_default_model.openclaw_model_missing",
                    format!("该 OpenClaw 供应商在当前配置中没有模型: {id}"),
                    format!(
                        "This OpenClaw provider does not have model in the current config: {id}"
                    ),
                ));
            }
            None => ordered_model_ids[0].clone(),
        };
        let primary = format!("{provider_id}/{selected_model_id}");
        let fallbacks = ordered_model_ids
            .iter()
            .filter(|candidate| *candidate != &selected_model_id)
            .map(|candidate| format!("{provider_id}/{candidate}"))
            .collect();
        let model = crate::openclaw_config::OpenClawDefaultModel {
            primary: primary.clone(),
            fallbacks,
            extra: crate::openclaw_config::get_default_model()?
                .map(|existing| existing.extra)
                .unwrap_or_default(),
        };
        crate::openclaw_config::set_default_model(&model)?;
        Ok(primary)
    }

    fn openclaw_default_model_references_provider(provider_id: &str) -> Result<bool, AppError> {
        Ok(
            crate::openclaw_config::get_default_model()?.is_some_and(|model| {
                model
                    .primary
                    .split_once('/')
                    .is_some_and(|(default_provider_id, _)| default_provider_id == provider_id)
            }),
        )
    }

    fn guard_additive_default_provider_removal(
        app_type: &AppType,
        provider_id: &str,
    ) -> Result<(), AppError> {
        match app_type {
            AppType::Hermes
                if crate::hermes_config::get_current_provider_id()?.as_deref()
                    == Some(provider_id) =>
            {
                Err(AppError::localized(
                    "provider.remove_from_config.hermes_current",
                    "不能从配置中移除 Hermes 当前默认供应商",
                    "Cannot remove the current default Hermes provider from config",
                ))
            }
            AppType::OpenClaw if Self::openclaw_default_model_references_provider(provider_id)? => {
                Err(AppError::localized(
                    "provider.remove_from_config.openclaw_default",
                    "不能从配置中移除被当前默认模型引用的 OpenClaw 供应商",
                    "Cannot remove the OpenClaw provider referenced by the current default model from config",
                ))
            }
            _ => Ok(()),
        }
    }

    fn openclaw_provider_model_ids(provider_value: &Value) -> Vec<String> {
        provider_value
            .get("models")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter_map(|model| model.get("id").and_then(|value| value.as_str()))
            .map(str::to_string)
            .collect()
    }

    /// 将所有应用的当前供应商配置同步到 live 文件。
    ///
    /// 用于 WebDAV 下载、备份恢复等场景：数据库已更新，但 live 配置文件
    /// （`~/.codex/config.toml`、Claude `settings.json` 等）尚未同步。
    /// 对齐上游 `sync_current_to_live` 行为。
    pub fn sync_current_to_live(state: &AppState) -> Result<(), AppError> {
        Self::sync_current_to_live_with_resolution(state, live_merge::ConflictPolicy::Fail.into())
    }

    pub(crate) fn sync_current_to_live_with_resolution(
        state: &AppState,
        resolution: live_merge::ConflictResolution<'_>,
    ) -> Result<(), AppError> {
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
                            if Self::provider_live_config_managed(provider) == Some(false) {
                                continue;
                            }
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

            Self::write_live_snapshot_with_resolution(
                app_type,
                provider,
                snippet.as_deref(),
                None,
                true,
                resolution,
            )?;
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

    pub(crate) fn preview_switch_live_conflicts(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
    ) -> Result<Vec<live_merge::ConfigConflict>, AppError> {
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
            let running_takeover_active = state
                .proxy_service
                .is_app_takeover_active_blocking(&app_type)
                .map_err(AppError::Message)?;
            let live_taken_over = state
                .proxy_service
                .detect_takeover_in_live_config_for_app(&app_type);
            if is_app_taken_over || live_taken_over || running_takeover_active {
                return Ok(Vec::new());
            }
        }

        let effective_current_provider = if app_type.is_additive_mode() {
            None
        } else {
            crate::settings::get_effective_current_provider(&state.db, &app_type)?
        };
        let previous_common_config_snippet = if app_type.is_additive_mode() {
            None
        } else {
            state.db.get_config_snippet(app_type.as_str())?
        };
        let mut candidate = {
            let guard = state.config.read().map_err(AppError::from)?;
            guard.clone()
        };
        let action = Self::prepare_switch_post_commit_action(
            &mut candidate,
            &app_type,
            provider_id,
            effective_current_provider.as_deref(),
            previous_common_config_snippet,
        )?;

        let mut collector = live_merge::ConflictCollector::default();
        {
            let resolver = &mut collector as &mut dyn live_merge::ConflictResolver;
            let resolver = std::cell::RefCell::new(resolver);
            Self::prepare_post_commit_action(
                state,
                action,
                live_merge::ConflictResolution::Resolver(&resolver),
            )?;
        }

        Ok(collector.into_conflicts())
    }

    fn prepare_switch_post_commit_action(
        config: &mut MultiAppConfig,
        app_type: &AppType,
        provider_id: &str,
        effective_current_provider: Option<&str>,
        previous_common_config_snippet: Option<String>,
    ) -> Result<PostCommitAction, AppError> {
        if app_type.is_additive_mode() {
            let provider = {
                let provider = config
                    .get_manager_mut(app_type)
                    .ok_or_else(|| Self::app_not_found(app_type))?
                    .providers
                    .get_mut(provider_id)
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.not_found",
                            format!("供应商不存在: {provider_id}"),
                            format!("Provider not found: {provider_id}"),
                        )
                    })?;
                Self::set_provider_live_config_managed(provider, true);
                provider.clone()
            };

            return Ok(PostCommitAction {
                app_type: app_type.clone(),
                provider,
                previous_provider: None,
                backup: Self::capture_live_snapshot(app_type)?,
                sync_mcp: matches!(app_type, AppType::OpenCode),
                refresh_snapshot: false,
                common_config_snippet: config.common_config_snippets.get(app_type).cloned(),
                previous_common_config_snippet: None,
                takeover_active: false,
                activate_provider: matches!(app_type, AppType::Hermes),
            });
        }

        let backup = Self::capture_live_snapshot(app_type)?;
        let previous_provider = effective_current_provider
            .filter(|current_id| *current_id != provider_id)
            .and_then(|current_id| {
                config
                    .get_manager(app_type)
                    .and_then(|manager| manager.providers.get(current_id))
                    .cloned()
            });
        let provider = match app_type {
            AppType::Codex => {
                Self::prepare_switch_codex(config, provider_id, effective_current_provider)?
            }
            AppType::Claude => {
                Self::prepare_switch_claude(config, provider_id, effective_current_provider)?
            }
            AppType::Gemini => {
                Self::prepare_switch_gemini(config, provider_id, effective_current_provider)?
            }
            AppType::OpenCode => unreachable!("additive mode handled above"),
            AppType::Hermes => unreachable!("additive mode handled above"),
            AppType::OpenClaw => unreachable!("additive mode handled above"),
        };

        Ok(PostCommitAction {
            app_type: app_type.clone(),
            provider,
            previous_provider,
            backup,
            sync_mcp: true,
            refresh_snapshot: true,
            common_config_snippet: config.common_config_snippets.get(app_type).cloned(),
            previous_common_config_snippet,
            takeover_active: false,
            activate_provider: false,
        })
    }

    /// 切换指定应用的供应商
    pub fn switch(state: &AppState, app_type: AppType, provider_id: &str) -> Result<(), AppError> {
        Self::switch_with_resolution(
            state,
            app_type,
            provider_id,
            live_merge::ConflictPolicy::Fail.into(),
        )
    }

    pub(crate) fn switch_with_resolution(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
        resolution: live_merge::ConflictResolution<'_>,
    ) -> Result<(), AppError> {
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
            let running_takeover_active = state
                .proxy_service
                .is_app_takeover_active_blocking(&app_type)
                .map_err(AppError::Message)?;
            let live_taken_over = state
                .proxy_service
                .detect_takeover_in_live_config_for_app(&app_type);
            let should_hot_switch = is_app_taken_over || live_taken_over || running_takeover_active;

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
        let previous_common_config_snippet = if app_type.is_additive_mode() {
            None
        } else {
            state.db.get_config_snippet(app_type.as_str())?
        };

        Self::run_transaction_with_resolution(state, resolution, move |config| {
            let action = Self::prepare_switch_post_commit_action(
                config,
                &app_type_clone,
                &provider_id_owned,
                effective_current_provider.as_deref(),
                previous_common_config_snippet,
            )?;
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
        Self::write_live_snapshot_with_resolution(
            app_type,
            provider,
            common_config_snippet,
            None,
            apply_common_config,
            live_merge::ConflictPolicy::Fail.into(),
        )
    }

    fn write_live_snapshot_with_resolution(
        app_type: &AppType,
        provider: &Provider,
        common_config_snippet: Option<&str>,
        previous_common_config_snippet: Option<&str>,
        apply_common_config: bool,
        resolution: live_merge::ConflictResolution<'_>,
    ) -> Result<(), AppError> {
        let prepared = Self::prepare_live_snapshot_with_resolution(
            app_type,
            provider,
            None,
            common_config_snippet,
            previous_common_config_snippet,
            apply_common_config,
            resolution,
        )?;
        Self::apply_prepared_live_snapshot(&prepared)
    }

    fn prepare_live_snapshot_with_resolution(
        app_type: &AppType,
        provider: &Provider,
        previous_provider: Option<&Provider>,
        common_config_snippet: Option<&str>,
        previous_common_config_snippet: Option<&str>,
        apply_common_config: bool,
        resolution: live_merge::ConflictResolution<'_>,
    ) -> Result<PreparedLiveWrite, AppError> {
        let apply_common_config = Self::resolve_live_apply_common_config(
            app_type,
            provider,
            common_config_snippet,
            apply_common_config,
        );

        match app_type {
            AppType::Codex => Self::prepare_codex_live_write(
                provider,
                common_config_snippet,
                previous_common_config_snippet,
                apply_common_config,
                false,
                resolution,
            ),
            AppType::Claude => Self::prepare_claude_live_write(
                provider,
                previous_provider,
                common_config_snippet,
                previous_common_config_snippet,
                apply_common_config,
                false,
                resolution,
            ),
            AppType::Gemini => Self::prepare_gemini_live_write(
                provider,
                if apply_common_config {
                    common_config_snippet
                } else {
                    None
                },
                previous_common_config_snippet,
                false,
                resolution,
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

                let config = match serde_json::from_value::<crate::provider::OpenCodeProviderConfig>(
                    config_to_write.clone(),
                ) {
                    Ok(config) => {
                        let previous_config = previous_provider.and_then(|previous| {
                            serde_json::from_value::<crate::provider::OpenCodeProviderConfig>(
                                previous.settings_config.clone(),
                            )
                            .ok()
                        });
                        crate::opencode_config::prepare_typed_provider_with_base_and_resolution(
                            &provider.id,
                            previous_config.as_ref(),
                            &config,
                            resolution,
                        )?
                    }
                    Err(_) => crate::opencode_config::prepare_provider_with_base_and_resolution(
                        &provider.id,
                        previous_provider.map(|previous| previous.settings_config.clone()),
                        config_to_write,
                        resolution,
                    )?,
                };
                Ok(PreparedLiveWrite::OpenCode { config })
            }
            AppType::Hermes => {
                if !provider.settings_config.is_object() {
                    return Err(AppError::localized(
                        "provider.hermes.settings.not_object",
                        "Hermes 配置必须是 JSON 对象",
                        "Hermes configuration must be a JSON object",
                    ));
                }
                let providers = crate::hermes_config::prepare_provider_with_resolution(
                    &provider.id,
                    provider.settings_config.clone(),
                    resolution,
                )?;
                Ok(PreparedLiveWrite::Hermes { providers })
            }
            AppType::OpenClaw => {
                let settings_config = provider.settings_config.clone();
                let looks_like_provider = settings_config.get("baseUrl").is_some()
                    || settings_config.get("api").is_some()
                    || settings_config.get("models").is_some();
                if !looks_like_provider {
                    return Ok(PreparedLiveWrite::Noop);
                }

                let config = Self::parse_openclaw_provider_settings(&settings_config)?;
                Self::validate_openclaw_provider_models(&provider.id, &config)?;
                let models = crate::openclaw_config::prepare_typed_provider_with_resolution(
                    &provider.id,
                    &config,
                    resolution,
                )
                .map_err(Self::normalize_openclaw_live_write_error)?;
                Ok(PreparedLiveWrite::OpenClaw { models })
            }
        }
    }

    fn apply_prepared_live_snapshot(prepared: &PreparedLiveWrite) -> Result<(), AppError> {
        match prepared {
            PreparedLiveWrite::Noop => Ok(()),
            PreparedLiveWrite::Claude { .. } => Self::apply_claude_live_write(prepared),
            PreparedLiveWrite::Codex { .. } => Self::apply_codex_live_write(prepared),
            PreparedLiveWrite::Gemini { .. } | PreparedLiveWrite::GeminiSecurityFlag { .. } => {
                Self::apply_gemini_live_write(prepared)
            }
            PreparedLiveWrite::OpenCode { config } => {
                crate::opencode_config::write_prepared_config(config)
            }
            PreparedLiveWrite::Hermes { providers } => {
                crate::hermes_config::write_prepared_providers(providers).map(|_| ())
            }
            PreparedLiveWrite::OpenClaw { models } => {
                crate::openclaw_config::write_prepared_models(models)
                    .map(|_| ())
                    .map_err(Self::normalize_openclaw_live_write_error)
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
            AppType::Hermes => Err(AppError::Config(
                "Hermes does not support proxy takeover backups".into(),
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
            AppType::Hermes => {
                if !provider.settings_config.is_object() {
                    return Err(AppError::localized(
                        "provider.hermes.settings.not_object",
                        "Hermes 配置必须是 JSON 对象",
                        "Hermes configuration must be a JSON object",
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

        let apply_common_config = Self::resolve_live_apply_common_config(
            &app_type,
            provider,
            common_config_snippet.as_deref(),
            true,
        );
        Self::build_effective_live_snapshot(
            &app_type,
            provider,
            common_config_snippet.as_deref(),
            apply_common_config,
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
        if app_type.supports_failover() {
            let app_key = app_type.as_str();
            let (takeover_enabled, auto_failover_enabled) = state.db.get_proxy_flags_sync(app_key);
            if takeover_enabled && auto_failover_enabled {
                let queue = state.db.get_failover_queue(app_key)?;
                if queue.len() == 1
                    && queue
                        .first()
                        .is_some_and(|item| item.provider_id == provider_id)
                {
                    return Err(active_failover_last_provider_error());
                }
            }
        }

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
                AppType::Hermes => {
                    if crate::hermes_config::get_hermes_dir().exists() {
                        crate::hermes_config::remove_provider(provider_id)?;
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
            AppType::Hermes => {
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

    pub fn import_hermes_providers_from_live(state: &AppState) -> Result<usize, AppError> {
        live::import_hermes_providers_from_live(state)
    }

    pub fn import_opencode_providers_from_live(state: &AppState) -> Result<usize, AppError> {
        live::import_opencode_providers_from_live(state)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderSortUpdate {
    pub id: String,
    #[serde(rename = "sortIndex")]
    pub sort_index: usize,
}
