//! Route mode runtime actions: enable/disable, member mutations.
//!
//! Member edits load the stored group doc, apply the change by provider id,
//! and save through the validating DAO so per-member model mappings survive.

use std::collections::BTreeMap;

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::database::{Database, RouteModeGroup, RouteModeMember};
use crate::error::AppError;
use crate::provider::Provider;
use crate::proxy::providers::get_claude_api_format;
use crate::services::StreamCheckService;

use super::super::app::{App, Overlay, RouteMappingsEditorState, ToastKind};
use super::super::data::{load_state, UiData};
use super::super::runtime_systems::{next_model_fetch_request_id, ModelFetchTarget};
use super::super::text_edit::TextInput;
use super::RuntimeActionContext;

fn runtime() -> Result<tokio::runtime::Runtime, AppError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("failed to create async runtime: {e}")))
}

pub(super) fn set_route_mode_enabled(
    ctx: &mut RuntimeActionContext<'_>,
    app_type: AppType,
    enabled: bool,
) -> Result<(), AppError> {
    let state = load_state()?;
    let runtime = runtime()?;

    if enabled {
        runtime
            .block_on(
                state
                    .proxy_service
                    .enable_route_mode_for_app(app_type.as_str()),
            )
            .map_err(AppError::Message)?;
    } else {
        runtime
            .block_on(
                state
                    .proxy_service
                    .disable_route_mode_for_app(app_type.as_str()),
            )
            .map_err(AppError::Message)?;
    }

    *ctx.data = UiData::load(&ctx.app.app_type)?;
    ctx.app.push_toast(
        if enabled {
            crate::t!("Route mode enabled.", "路由模式已开启。")
        } else {
            crate::t!("Route mode disabled.", "路由模式已关闭。")
        },
        ToastKind::Success,
    );
    Ok(())
}

pub(super) fn enable_proxy_and_route_mode(
    ctx: &mut RuntimeActionContext<'_>,
    app_type: AppType,
) -> Result<(), AppError> {
    let state = load_state()?;
    let runtime = runtime()?;

    runtime
        .block_on(
            state
                .proxy_service
                .enable_proxy_and_route_mode_for_app(app_type.as_str()),
        )
        .map_err(AppError::Message)?;

    *ctx.data = UiData::load(&ctx.app.app_type)?;
    ctx.app.push_toast(
        crate::t!(
            "Proxy routing and route mode enabled.",
            "代理路由和路由模式已开启。"
        ),
        ToastKind::Success,
    );
    Ok(())
}

pub(super) fn add_route_mode_member(
    ctx: &mut RuntimeActionContext<'_>,
    app_type: AppType,
    provider_id: String,
) -> Result<(), AppError> {
    // Seed the new member with passthrough mappings for the models this
    // app's agent may request (read from the active provider config).
    // Computed before the mutation so the DB closure stays key-latency cheap.
    let seed = active_provider_request_models(ctx, &app_type);
    // Capture the new member's index inside the closure so the cursor can
    // land on the added row (screen rows are index+1; row 0 is the status
    // toggle). Only set when the push actually happened.
    let mut new_idx = None;
    mutate_route_mode_group(ctx, &app_type, |group| {
        if group
            .members
            .iter()
            .any(|member| member.provider_id == provider_id)
        {
            return Ok(false);
        }
        new_idx = Some(group.members.len());
        let mut member = RouteModeMember::new(provider_id, 1);
        for model in &seed {
            // Insert-if-absent: existing mappings are never clobbered (new
            // members start empty, so this only seeds passthrough defaults).
            member
                .model_mappings
                .entry(model.clone())
                .or_insert(model.clone());
        }
        group.members.push(member);
        Ok(true)
    })?;
    if let Some(idx) = new_idx {
        ctx.app.route_mode_idx = idx + 1;
    }
    Ok(())
}

/// True when route mode is on and `provider_id` is its only enabled member.
fn is_last_enabled_member(group: &RouteModeGroup, provider_id: &str) -> bool {
    group.enabled
        && group
            .members
            .iter()
            .any(|member| member.provider_id == provider_id && member.enabled)
        && group.members.iter().filter(|member| member.enabled).count() == 1
}

fn push_last_enabled_member_warning(app: &mut App) {
    app.push_toast(
        crate::t!(
            "Cannot disable or remove the last enabled member while route mode is on; enable another member or turn route mode off first.",
            "路由模式开启时不能停用或移除最后一个启用的成员；请先启用其他成员或关闭路由模式。"
        ),
        ToastKind::Warning,
    );
}

pub(super) fn remove_route_mode_member(
    ctx: &mut RuntimeActionContext<'_>,
    app_type: AppType,
    provider_id: String,
) -> Result<(), AppError> {
    let mut blocked = false;
    mutate_route_mode_group(ctx, &app_type, |group| {
        // Decide the block on the same group instance being mutated so the
        // check and the removal read one consistent snapshot (a second
        // connection could see different enabled flags in between).
        if is_last_enabled_member(group, &provider_id) {
            blocked = true;
            return Ok(false);
        }
        let before = group.members.len();
        group
            .members
            .retain(|member| member.provider_id != provider_id);
        Ok(group.members.len() != before)
    })?;
    if blocked {
        push_last_enabled_member_warning(ctx.app);
    }
    Ok(())
}

pub(super) fn toggle_route_mode_member(
    ctx: &mut RuntimeActionContext<'_>,
    app_type: AppType,
    provider_id: String,
) -> Result<(), AppError> {
    let mut blocked = false;
    mutate_route_mode_group(ctx, &app_type, |group| {
        // Same-snapshot rule as removal. `is_last_enabled_member` implies the
        // target is enabled, so a disabled member is always re-enableable.
        if is_last_enabled_member(group, &provider_id) {
            blocked = true;
            return Ok(false);
        }
        if let Some(member) = group
            .members
            .iter_mut()
            .find(|member| member.provider_id == provider_id)
        {
            member.enabled = !member.enabled;
            return Ok(true);
        }
        Ok(false)
    })?;
    if blocked {
        push_last_enabled_member_warning(ctx.app);
    }
    Ok(())
}

pub(super) fn set_route_mode_member_weight(
    ctx: &mut RuntimeActionContext<'_>,
    app_type: AppType,
    provider_id: String,
    weight: u32,
) -> Result<(), AppError> {
    mutate_route_mode_group(ctx, &app_type, |group| {
        if let Some(member) = group
            .members
            .iter_mut()
            .find(|member| member.provider_id == provider_id)
        {
            if member.weight != weight {
                member.weight = weight;
                return Ok(true);
            }
            return Ok(false);
        }
        Ok(false)
    })
}

pub(super) fn set_route_mode_member_mappings(
    ctx: &mut RuntimeActionContext<'_>,
    app_type: AppType,
    provider_id: String,
    mappings: BTreeMap<String, String>,
) -> Result<(), AppError> {
    mutate_route_mode_group(ctx, &app_type, |group| {
        if let Some(member) = group
            .members
            .iter_mut()
            .find(|member| member.provider_id == provider_id)
        {
            if member.model_mappings == mappings {
                return Ok(false);
            }
            member.model_mappings = mappings;
            return Ok(true);
        }
        Ok(false)
    })
}

/// `f` on the mapping editor's actual-model field: fetch the member
/// provider's upstream model list and open the model picker on top of the
/// editor. The editor parks in `pending_overlay` and every early-return
/// below restores it so the user keeps the draft row.
pub(super) fn route_member_model_fetch(
    ctx: &mut RuntimeActionContext<'_>,
    provider_id: String,
) -> Result<(), AppError> {
    if ctx.model_fetch_req_tx.is_none() {
        // Worker unavailable: bail before parking the editor so the draft
        // row survives; `model_fetch` would only toast and leave the parked
        // editor to be discarded.
        ctx.app.push_toast(
            texts::tui_toast_model_fetch_worker_disabled(),
            ToastKind::Warning,
        );
        return Ok(());
    }
    if !matches!(ctx.app.overlay, Overlay::RouteMappingsEditor(_)) {
        // Stale trigger: the editor is no longer open, nothing to fetch for.
        // Match before taking so an unrelated overlay is never discarded.
        return Ok(());
    }
    let Overlay::RouteMappingsEditor(editor) =
        std::mem::replace(&mut ctx.app.overlay, Overlay::None)
    else {
        return Ok(());
    };
    let fail = |ctx: &mut RuntimeActionContext<'_>,
                editor: RouteMappingsEditorState,
                message: &'static str| {
        ctx.app.overlay = Overlay::RouteMappingsEditor(editor);
        ctx.app.push_toast(message, ToastKind::Warning);
    };

    let app_type = ctx.app.app_type.clone();
    let Some(row) = ctx
        .data
        .providers
        .rows
        .iter()
        .find(|row| row.id == provider_id)
    else {
        fail(ctx, editor, texts::tui_route_member_provider_missing());
        return Ok(());
    };
    let provider = row.provider.clone();

    let codex_oauth = provider.is_codex_oauth();
    // Bound managed account for this member; None lets the OAuth service use
    // its default account (same fallback as the provider form).
    let codex_oauth_account_id = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.managed_account_id_for("codex_oauth"));
    // Members with a saved custom User-Agent need it on the model-list fetch
    // too; the form flow forwards the user-supplied value the same way.
    let custom_user_agent = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.custom_user_agent.clone())
        .filter(|user_agent| !user_agent.trim().is_empty());
    let (base_url, api_key) = if codex_oauth {
        // Codex OAuth model lists come from the OAuth service, not an
        // endpoint; base URL and key are unused by the worker in that path.
        (String::new(), None)
    } else {
        let base_url = match StreamCheckService::extract_base_url(&provider, &app_type) {
            Ok(url) if !url.trim().is_empty() => url,
            _ => {
                fail(ctx, editor, texts::tui_model_fetch_need_endpoint());
                return Ok(());
            }
        };
        (base_url, extract_route_member_api_key(&app_type, &provider))
    };
    if !codex_oauth && api_key.is_none() {
        // Gate before parking the editor: the worker could only fail inside
        // the picker, and the user would have to Esc back to the draft.
        fail(ctx, editor, texts::tui_model_fetch_need_api_key());
        return Ok(());
    }
    let is_full_url = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.is_full_url)
        .unwrap_or(false);
    let api_protocol = match app_type {
        AppType::Claude => match get_claude_api_format(&provider) {
            "anthropic" => Some("anthropic-messages".to_string()),
            "gemini_native" => Some("google-generative-ai".to_string()),
            _ => None,
        },
        AppType::Gemini => Some("google-generative-ai".to_string()),
        _ => None,
    };

    ctx.app.pending_overlay = Some(Overlay::RouteMappingsEditor(editor));
    super::providers::model_fetch(
        ctx,
        base_url,
        is_full_url,
        api_key,
        custom_user_agent,
        api_protocol,
        None,
        codex_oauth,
        codex_oauth_account_id,
        ModelFetchTarget::RouteMappings { provider_id },
    )
}

/// Request models from this app's active provider (`ProviderRow.is_current`);
/// empty when no provider is currently active.
fn active_provider_request_models(
    ctx: &RuntimeActionContext<'_>,
    app_type: &AppType,
) -> Vec<String> {
    ctx.data
        .providers
        .rows
        .iter()
        .find(|row| row.is_current)
        .map(|row| {
            crate::services::request_models::request_model_candidates(&row.provider, app_type)
        })
        .unwrap_or_default()
}

/// Ctrl+R on the mapping editor's request field: open the model picker
/// pre-filled with this app's env-sourced request models. Purely local — no
/// worker channel, no network — but the editor still parks in
/// `pending_overlay` and every early-return below restores it.
pub(super) fn route_member_request_models_fetch(
    ctx: &mut RuntimeActionContext<'_>,
) -> Result<(), AppError> {
    if !matches!(ctx.app.overlay, Overlay::RouteMappingsEditor(_)) {
        // Stale trigger: the editor is no longer open, nothing to pick for.
        // Match before taking so an unrelated overlay is never discarded.
        return Ok(());
    }
    let Overlay::RouteMappingsEditor(editor) =
        std::mem::replace(&mut ctx.app.overlay, Overlay::None)
    else {
        return Ok(());
    };

    let candidates = active_provider_request_models(ctx, &ctx.app.app_type);
    if candidates.is_empty() {
        ctx.app.overlay = Overlay::RouteMappingsEditor(editor);
        ctx.app.push_toast(
            texts::tui_route_member_no_request_models(),
            ToastKind::Warning,
        );
        return Ok(());
    }

    ctx.app.pending_overlay = Some(Overlay::RouteMappingsEditor(editor));
    ctx.app.overlay = Overlay::ModelFetchPicker {
        request_id: next_model_fetch_request_id(),
        target: ModelFetchTarget::RouteMappingsRequest,
        input: TextInput::new(""),
        query: String::new(),
        // Local candidate list: the picker renders instantly and Enter
        // writes back synchronously.
        fetching: false,
        models: candidates,
        filtered_indices: None,
        filter_incomplete: false,
        error: None,
        selected_idx: 0,
        selection_active: false,
    };
    Ok(())
}

fn extract_route_member_api_key(app_type: &AppType, provider: &Provider) -> Option<String> {
    match app_type {
        AppType::Claude => StreamCheckService::extract_claude_key(provider),
        AppType::Codex => StreamCheckService::extract_codex_key(provider),
        AppType::Gemini => crate::gemini_config::json_to_env(&provider.settings_config)
            .ok()
            .and_then(|env| env.get("GEMINI_API_KEY").cloned())
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty())
            .or_else(|| {
                provider
                    .settings_config
                    .get("apiKey")
                    .or_else(|| provider.settings_config.get("api_key"))
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            }),
        _ => None,
    }
}

/// Loads the stored group, applies `mutate`, and saves only when something
/// changed. Clamps the route-mode cursor when the member list shrank so it
/// never points past the last row.
fn mutate_route_mode_group(
    ctx: &mut RuntimeActionContext<'_>,
    app_type: &AppType,
    mutate: impl FnOnce(&mut RouteModeGroup) -> Result<bool, AppError>,
) -> Result<(), AppError> {
    // A bare writable handle skips the full `AppState::try_new` ceremony
    // (snapshot export, startup recovery, proxy service) so member edits stay
    // key-latency cheap; `Database::init` still validates schema/dir safely.
    let db = Database::init()?;
    let mut group = db.get_route_mode_group(app_type.as_str())?;
    let changed = mutate(&mut group)?;
    if !changed {
        return Ok(());
    }
    db.save_route_mode_group(app_type.as_str(), &group)?;
    ctx.app.route_mode_idx = ctx.app.route_mode_idx.min(group.members.len());

    // Mark before the targeted refresh: if that refresh fails, the bumped
    // token still routes the action through CurrentAppDataChanged so the
    // background full reload reconciles the visible table. A failure here is
    // swallowed on purpose — propagating it would make the dispatcher skip
    // cache invalidation entirely (`result.is_ok()` gate), leaving the UI
    // stale with no recovery refresh.
    ctx.data.mark_current_app_data_changed();
    let _ = ctx.data.refresh_route_mode_data(app_type);
    Ok(())
}
