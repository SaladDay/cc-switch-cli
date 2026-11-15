//! CLI entry points for the upstream Claude editor. File diffing, ownership,
//! conflict decisions and durable publication are implemented in claude_editor.
use super::{claude_editor, live_projection_ownership, ProviderService};
use crate::{app_config::AppType, error::AppError, provider::Provider, store::AppState};
use serde_json::Value;

impl ProviderService {
    pub(crate) fn claude_editor_view(
        state: &AppState,
        settings: &Value,
    ) -> Result<claude_editor::EditorView, AppError> {
        claude_editor::view(state, settings)
    }

    pub(crate) fn save_claude_from_editor(
        state: &AppState,
        mut provider: Provider,
        editor: claude_editor::EditorSave,
        add: bool,
        copy_source: Option<&str>,
    ) -> Result<bool, AppError> {
        let app = AppType::Claude;
        let _mutation = futures::executor::block_on(
            crate::services::state_coordination::acquire_restore_mutation_guard(),
        )
        .map_err(AppError::Message)?;
        let _switch =
            futures::executor::block_on(state.proxy_service.lock_switch_for_app("claude"));
        if crate::mode::operation::settle(&state.db, "claude")?.is_some() {
            state.reload_config_snapshot_from_db()?;
            return Err(AppError::Message(
                "An unfinished configuration write was recovered; reopen the editor before saving"
                    .into(),
            ));
        }
        let copy_snapshot = if let Some(source_id) = copy_source {
            state.reload_config_snapshot_from_db()?;
            let snapshot = state.config.read().map_err(AppError::from)?.clone();
            let manager = snapshot
                .get_manager(&app)
                .ok_or_else(|| Self::app_not_found(&app))?;
            let source = manager
                .providers
                .get(source_id)
                .ok_or_else(|| AppError::Message(format!("Provider not found: {source_id}")))?;
            provider = Self::duplicate_provider_with_overrides(
                source,
                Some(provider),
                &manager.providers.keys().cloned().collect(),
            );
            Some(snapshot)
        } else {
            None
        };
        let existing = state.db.get_provider_by_id(&provider.id, "claude")?;
        if add && existing.is_some() {
            return Err(AppError::Message(format!(
                "Provider ID already exists: {}",
                provider.id
            )));
        }
        if !add && existing.is_none() {
            return Err(AppError::Message(format!(
                "Provider not found: {}",
                provider.id
            )));
        }
        Self::normalize_provider_if_claude(&app, &mut provider);
        let plan = claude_editor::plan_save(
            existing.as_ref().map(|row| &row.settings_config),
            &provider.settings_config,
            &editor.base,
        )?;
        provider.settings_config = plan.row_settings.clone();
        if add {
            Self::inject_coding_plan_usage_script(&app, &mut provider);
            // Retain the compatibility marker for older clients.
            provider
                .meta
                .get_or_insert_with(Default::default)
                .apply_common_config = Some(true);
        }
        Self::validate_provider_settings(&app, &provider)?;
        let current = crate::settings::get_effective_current_provider(&state.db, &app)?;
        let ownership = live_projection_ownership(state, &app)?;
        let first = add && current.is_none();
        let write_keys =
            !ownership.takeover_active && (first || current.as_deref() == Some(&provider.id));
        if let (Some(original), Some(source_id)) = (&copy_snapshot, copy_source) {
            Self::run_transaction_preserving_current_providers(
                state,
                &[AppType::Claude, AppType::Codex, AppType::Gemini],
                |config| {
                    *config = original.clone();
                    let manager = config
                        .get_manager_mut(&app)
                        .ok_or_else(|| Self::app_not_found(&app))?;
                    Self::shift_sort_indices_for_duplicate(manager, source_id, provider.sort_index);
                    manager
                        .providers
                        .insert(provider.id.clone(), provider.clone());
                    Ok(((), None))
                },
            )?;
        } else {
            state.db.save_provider("claude", &provider)?;
        }
        let written = claude_editor::write_live(
            &state.db,
            &plan,
            editor.on_conflict,
            write_keys.then_some(claude_editor::KeyFieldWrite {
                prev: existing.as_ref(),
                target: &provider,
                set_pointer: first,
            }),
        )
        .and_then(|()| {
            state.reload_config_snapshot_from_db()?;
            if ownership.takeover_active && current.as_deref() == Some(&provider.id) {
                let config = state.config.read().map_err(AppError::from)?;
                let mut action = Self::build_post_commit_action_for_current_provider(
                    &config,
                    &app,
                    &provider.id,
                    true,
                    ownership.live_taken_over,
                    ownership.refresh_stale_backup,
                    None,
                )?
                .expect("saved provider is present");
                action.previous_provider = existing.clone();
                // An editor save already separated globals from the row.
                action.common_config_snippet = None;
                drop(config);
                let prepared = Self::prepare_post_commit_action(state, action)?;
                Self::apply_prepared_post_commit_action(state, &prepared)?;
            }
            Ok(())
        });
        if let Err(error) = written {
            if !crate::mode::operation::has_pending("claude") {
                if let Some(original) = copy_snapshot {
                    Self::restore_config_only_preserving_current_providers(
                        state,
                        original,
                        &[AppType::Claude, AppType::Codex, AppType::Gemini],
                    )?;
                } else {
                    match existing {
                        Some(ref row) => state.db.save_provider("claude", row)?,
                        None => state.db.delete_provider("claude", &provider.id)?,
                    }
                }
            }
            state.reload_config_snapshot_from_db()?;
            return Err(error);
        }
        state.reload_config_snapshot_from_db()?;
        Ok(true)
    }
}
