//! Ordinary Gemini switching owns provider settings, selection and its MCP tail.
//! Other product workflows still use the legacy snapshot persistence path.

use super::*;
use crate::database::{shared_store_error, Database};
use crate::services::mcp::ProviderMcpSync;
use cc_switch_core::fs::{
    shared_live_config_lock_path, SharedLiveConfigLock, SharedLiveConfigLockError,
};
use cc_switch_store::{McpTransactionGuard, ProviderRow, ProviderWriteOutcome};

impl ProviderService {
    pub(crate) fn switch_gemini_coordinated(
        state: &AppState,
        provider_id: &str,
    ) -> Result<(), AppError> {
        // Match legacy save's config -> database order. Work below uses the
        // explicit snapshot, never re-entering AppState locks or database DAOs.
        let mut config = state.config.write().map_err(AppError::from)?;
        let mut candidate = config.clone();
        let mut conn = state.db.conn.lock().map_err(AppError::from)?;
        // Declare the file guard first so early returns drop the transaction
        // before releasing it. Acquisition still follows the database lock.
        let shared_lock;
        let transaction =
            cc_switch_store::begin_immediate_transaction(&mut conn).map_err(shared_store_error)?;
        let mut rows = cc_switch_store::read_provider_rows(&transaction, Some("gemini"))
            .map_err(shared_store_error)?;
        let providers = Database::read_providers_on(&transaction, "gemini")?;
        let current =
            Database::read_current_provider_on(&transaction, "gemini")?.unwrap_or_default();
        let effective_current = match crate::settings::get_current_provider(&AppType::Gemini) {
            Some(local) if providers.contains_key(&local) => local,
            Some(local) => {
                log::warn!("本地 settings 中的 Gemini 供应商 {local} 不存在，将 fallback 到数据库");
                let _ = crate::settings::set_current_provider(&AppType::Gemini, None);
                current.clone()
            }
            None => current.clone(),
        };
        candidate.apps.insert(
            "gemini".into(),
            crate::provider::ProviderManager { providers, current },
        );
        candidate.common_config_snippets.gemini =
            Database::read_setting_on(&transaction, "common_config_gemini")?;
        candidate.mcp.servers = Some(
            Database::read_mcp_servers_on(&transaction)?
                .into_iter()
                .collect(),
        );
        let mut transaction = McpTransactionGuard::from_provider_transaction(transaction)
            .map_err(shared_store_error)?;
        let mut mcp = ProviderMcpSync::new();

        shared_lock = SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(
            &crate::config::get_home_dir(),
        ))
        .map_err(|error| match error {
            SharedLiveConfigLockError::Unavailable => {
                AppError::Conflict("Live configuration is locked by another operation".into())
            }
            SharedLiveConfigLockError::Io { path, source } => AppError::io(path, source),
        })?;
        let mut native = GeminiOperation::observe_provider()?;
        let snippet = candidate.common_config_snippets.gemini.clone();
        let action = Self::prepare_switch_post_commit_action(
            &mut candidate,
            &AppType::Gemini,
            provider_id,
            Some(&effective_current),
            snippet,
        )?;
        let prepared = Self::prepare_live_snapshot(
            &AppType::Gemini,
            &action.provider,
            action.previous_provider.as_ref(),
            action.common_config_snippet.as_deref(),
            action.previous_common_config_snippet.as_deref(),
            action
                .provider
                .meta
                .as_ref()
                .and_then(|meta| meta.apply_common_config)
                .unwrap_or(false),
        )?;
        let result = (|| {
            persist_switch_rows(
                &mut transaction,
                &mut rows,
                &mut candidate,
                &effective_current,
            )?;
            Self::apply_gemini_with_operation(&prepared, &mut native)?;
            mcp.sync(&mut transaction, &mut native)?;
            if crate::sync_policy::should_sync_live(&AppType::Gemini) {
                let snapshot = Self::read_gemini_provider_snapshot(
                    &action.provider,
                    action.common_config_snippet.as_deref(),
                )?;
                candidate
                    .apps
                    .get_mut("gemini")
                    .and_then(|manager| manager.providers.get_mut(provider_id))
                    .ok_or_else(|| AppError::Config("Gemini switch target disappeared".into()))?
                    .settings_config = snapshot;
                persist_switch_rows(
                    &mut transaction,
                    &mut rows,
                    &mut candidate,
                    &effective_current,
                )?;
            }
            Ok(())
        })();
        let failure = match result {
            Ok(()) => transaction
                .commit_preserving_on_error()
                .err()
                .map(|(transaction, error)| (transaction, shared_store_error(error))),
            Err(error) => Some((transaction, error)),
        };
        if let Some((transaction, error)) = failure {
            let mut errors = mcp.rollback(&mut native);
            let native_error = native.rollback().err();
            let database_error = transaction.rollback().map_err(shared_store_error).err();
            errors.extend(
                native_error
                    .into_iter()
                    .chain(database_error)
                    .map(|error| error.to_string()),
            );
            if errors.is_empty() {
                return Err(error);
            }
            return Err(AppError::localized(
                "post_commit.rollback_failed",
                format!("后置操作失败: {error}；回滚失败: {}", errors.join("; ")),
                format!(
                    "Post-commit step failed: {error}; rollback failed: {}",
                    errors.join("; ")
                ),
            ));
        }
        drop(failure);
        *config = candidate;
        drop(mcp);
        drop(native);
        drop(shared_lock);
        drop(conn);
        drop(config);
        // This best-effort host tail opens its own database and may migrate
        // Skill state. It must run after the provider transaction has ended.
        if let Err(error) = crate::services::skill::SkillService::sync_all_enabled_best_effort() {
            log::warn!("同步 Skills 失败: {error}");
        }
        Ok(())
    }
}

fn persist_switch_rows(
    transaction: &mut McpTransactionGuard<'_>,
    rows: &mut [ProviderRow],
    config: &mut MultiAppConfig,
    previous: &str,
) -> Result<(), AppError> {
    let manager = config
        .get_manager_mut(&AppType::Gemini)
        .ok_or_else(|| AppError::Config("Gemini switch manager disappeared".into()))?;
    for row in rows
        .iter_mut()
        .filter(|row| row.id == previous || row.id == manager.current)
    {
        let provider = manager
            .providers
            .get_mut(&row.id)
            .ok_or_else(|| AppError::Config("Gemini switch provider disappeared".into()))?;
        let before: Value = serde_json::from_str(&row.settings_config).unwrap_or(Value::Null);
        // Native snapshots own env/config, not opaque sibling fields stored by
        // another consumer. Preserve those in both the row and published cache.
        if let (Some(original), Some(snapshot)) =
            (before.as_object(), provider.settings_config.as_object_mut())
        {
            for (key, value) in original {
                if !matches!(key.as_str(), "env" | "config") {
                    snapshot.insert(key.clone(), value.clone());
                }
            }
        }
        if before != provider.settings_config {
            let settings = serde_json::to_string(&provider.settings_config)
                .map_err(|source| AppError::JsonSerialize { source })?;
            require_applied(
                transaction
                    .update_provider_settings_config_if_unchanged(
                        &row.id,
                        "gemini",
                        row.source_fingerprint(),
                        &settings,
                    )
                    .map_err(shared_store_error)?,
            )?;
            *row = transaction
                .read_provider(&row.id, "gemini")
                .map_err(shared_store_error)?
                .ok_or_else(|| {
                    AppError::Conflict("Gemini provider changed during switching".into())
                })?;
        }
    }
    // Clear old selections before activating the target, including databases
    // with a unique-current partial index supplied by another consumer.
    for selected in [false, true] {
        for row in rows
            .iter_mut()
            .filter(|row| (row.id == manager.current) == selected)
        {
            if row.is_current == i64::from(selected) {
                continue;
            }
            require_applied(
                transaction
                    .set_provider_current_if_unchanged(
                        &row.id,
                        "gemini",
                        row.source_fingerprint(),
                        selected,
                    )
                    .map_err(shared_store_error)?,
            )?;
            *row = transaction
                .read_provider(&row.id, "gemini")
                .map_err(shared_store_error)?
                .ok_or_else(|| {
                    AppError::Conflict("Gemini provider changed during switching".into())
                })?;
        }
    }
    Ok(())
}

fn require_applied(outcome: ProviderWriteOutcome) -> Result<(), AppError> {
    match outcome {
        ProviderWriteOutcome::Applied => Ok(()),
        ProviderWriteOutcome::NotApplied => Err(AppError::Conflict(
            "Gemini provider changed during switching".into(),
        )),
    }
}
