mod app;
mod clipboard;
pub(crate) mod data;
mod form;
#[cfg(test)]
pub(crate) use form::{ProviderAddField, ProviderAddFormState};
pub(crate) mod help;
pub(crate) mod icons;
mod input;
mod keymap;
mod route;
mod runtime_actions;
mod runtime_skills;
mod runtime_systems;
mod terminal;
#[cfg(test)]
mod tests;
mod text_edit;
pub(crate) mod theme;
mod ui;

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crossterm::event::{self, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::error::AppError;

use app::{Action, App, EditorSubmit, Overlay, ToastKind};
use runtime_actions::{apply_preloaded_app_switch, handle_action};
#[cfg(test)]
use runtime_actions::{
    import_mcp_from_supported_apps_with, open_proxy_help_overlay_with, queue_managed_proxy_action,
    run_external_editor_for_current_editor,
};
#[cfg(test)]
use runtime_skills::{
    finish_skills_import_with, open_skills_import_picker_with, scan_unmanaged_skills_with,
};
pub(crate) use runtime_systems::build_stream_check_result_lines;
#[cfg(test)]
use runtime_systems::{
    apply_webdav_jianguoyun_quick_setup, build_model_fetch_candidate_urls, drain_latest_webdav_req,
    model_fetch_strategy_for_field, parse_model_ids_from_response, update_webdav_last_error_with,
    UpdateMsg, WebDavReqKind,
};
pub(crate) use runtime_systems::{fetch_provider_models_for_tui, ModelFetchStrategy};
use runtime_systems::{
    handle_codex_history_msg, handle_local_env_msg, handle_managed_auth_msg,
    handle_model_fetch_msg, handle_proxy_msg, handle_quota_msg, handle_session_msg,
    handle_skills_msg, handle_speedtest_msg, handle_stream_check_msg, handle_update_msg,
    handle_webdav_msg, start_app_data_system, start_codex_history_system, start_local_env_system,
    start_managed_auth_system, start_model_fetch_system, start_proxy_system, start_quota_system,
    start_session_system, start_session_usage_sync_system, start_skills_system,
    start_speedtest_system, start_stream_check_system, start_update_system,
    start_usage_pricing_system, start_webdav_system, AppDataLoadKind, AppDataMsg, AppDataReq,
    CodexHistoryReq, LocalEnvReq, ManagedAuthReq, ModelFetchReq, ProxyMsgEffect, ProxyReq,
    QuotaReq, RequestTracker, SessionReq, SessionUsageSyncMsg, SessionUsageSyncReq, SkillsReq,
    StreamCheckReq, UpdateReq, UsageLogLoadError, UsagePricingLoadError, UsagePricingMsg,
    UsagePricingReq, WebDavReq,
};
use terminal::{PanicRestoreHookGuard, TuiTerminal};

pub(super) const TUI_TICK_RATE: Duration = Duration::from_millis(200);
/// Maximum steady-state redraw rate. Input can still be reduced more often,
/// but terminal writes are coalesced to one frame per interval.
pub(super) const TUI_FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
const TUI_TICKS_PER_MINUTE: u64 = 60 * 1000 / 200;

/// Returns how long a dirty frame must wait before it may be drawn.
///
/// Keeping the timing decision in durations makes the cap deterministic in
/// tests and avoids special-casing input, worker, and tick wake-ups.
fn frame_draw_wait(dirty: bool, elapsed_since_draw: Option<Duration>) -> Option<Duration> {
    if !dirty {
        return None;
    }

    Some(match elapsed_since_draw {
        None => Duration::ZERO,
        Some(elapsed) => TUI_FRAME_INTERVAL.saturating_sub(elapsed),
    })
}

fn next_loop_timeout(tick_wait: Duration, frame_wait: Option<Duration>) -> Duration {
    frame_wait.map_or(tick_wait, |wait| tick_wait.min(wait))
}

#[derive(Debug)]
struct FrameScheduler {
    dirty: bool,
    last_draw: Option<Instant>,
}

impl FrameScheduler {
    fn new() -> Self {
        Self {
            dirty: true,
            last_draw: None,
        }
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn wait(&self, now: Instant) -> Option<Duration> {
        frame_draw_wait(
            self.dirty,
            self.last_draw
                .map(|last_draw| now.saturating_duration_since(last_draw)),
        )
    }

    fn is_due(&self, now: Instant) -> bool {
        self.wait(now) == Some(Duration::ZERO)
    }

    fn record_draw(&mut self, completed_at: Instant) {
        self.dirty = false;
        self.last_draw = Some(completed_at);
    }
}

fn apply_visible_apps_startup_policy(
) -> Result<crate::services::visible_apps::VisibleAppsStartupOutcome, AppError> {
    let detection = crate::services::visible_apps::detect_visible_app_installation();
    crate::services::visible_apps::apply_startup_policy(&detection)
}

fn resolve_initial_app_type(app_override: Option<AppType>) -> AppType {
    let requested = app_override.unwrap_or(AppType::Claude);
    let visible_apps = crate::settings::get_visible_apps();

    if visible_apps.is_enabled_for(&requested) {
        return requested;
    }

    crate::settings::next_visible_app(&visible_apps, &requested, 1).unwrap_or(requested)
}

#[cfg(test)]
fn initialize_app_state_with<F, FVisibleApps>(
    app_override: Option<AppType>,
    load_data: F,
    apply_visible_apps: FVisibleApps,
) -> Result<(App, data::UiData), AppError>
where
    F: FnOnce(&AppType) -> Result<data::UiData, AppError>,
    FVisibleApps:
        FnOnce() -> Result<crate::services::visible_apps::VisibleAppsStartupOutcome, AppError>,
{
    let (app, _) = initialize_app_shell_with(app_override, apply_visible_apps)?;
    let data = load_data(&app.app_type)?;
    Ok((app, data))
}

fn initialize_app_shell_with<FVisibleApps>(
    app_override: Option<AppType>,
    apply_visible_apps: FVisibleApps,
) -> Result<(App, data::UiData), AppError>
where
    FVisibleApps:
        FnOnce() -> Result<crate::services::visible_apps::VisibleAppsStartupOutcome, AppError>,
{
    let visible_apps_outcome = apply_visible_apps()?;
    let app_type = resolve_initial_app_type(app_override);
    let mut app = App::new(Some(app_type));
    app.common_config_notice_confirmed = crate::settings::get_common_config_confirmed();
    app.usage_query_notice_confirmed = crate::settings::get_usage_confirmed();
    if visible_apps_outcome.should_prompt {
        app.prompt_visible_apps_auto_detection();
    }
    for notice in &visible_apps_outcome.notices {
        app.push_toast(
            crate::services::visible_apps::notice_message(notice),
            ToastKind::Info,
        );
    }
    Ok((app, data::UiData::default()))
}

#[cfg(test)]
fn initialize_app_state_for_test<F>(
    app_override: Option<AppType>,
    load_data: F,
) -> Result<(App, data::UiData), AppError>
where
    F: FnOnce(&AppType) -> Result<data::UiData, AppError>,
{
    let detection = crate::services::visible_apps::VisibleAppsDetection::default();
    initialize_app_state_with(app_override, load_data, || {
        crate::services::visible_apps::apply_startup_policy(&detection)
    })
}

#[derive(Default)]
struct ProxyOpenFlash {
    effect: Option<tachyonfx::Effect>,
    started_tick: Option<u64>,
}

impl ProxyOpenFlash {
    fn sync(&mut self, app: &App, area: ratatui::layout::Rect) {
        let Some(transition) = app.proxy_visual_transition else {
            return;
        };

        if transition.to_on && self.started_tick != Some(transition.started_tick) {
            self.effect = Some(ui::proxy_open_flash_effect(area));
            self.started_tick = Some(transition.started_tick);
        }
    }

    fn process(
        &mut self,
        frame_dt: Duration,
        buf: &mut ratatui::buffer::Buffer,
        area: ratatui::layout::Rect,
    ) {
        let Some(effect) = self.effect.as_mut() else {
            return;
        };

        effect.set_area(area);
        effect.process(frame_dt.into(), buf, area);

        if effect.done() {
            self.effect = None;
        }
    }

    fn active(&self) -> bool {
        self.effect.is_some()
    }
}

fn queue_quota_refresh(
    app: &mut App,
    data: &mut data::UiData,
    quota_req_tx: Option<&mpsc::Sender<QuotaReq>>,
    target: data::QuotaTarget,
    manual: bool,
) {
    let Some(tx) = quota_req_tx else {
        if manual {
            app.push_toast(
                texts::tui_toast_quota_worker_unavailable("quota worker is not running"),
                ToastKind::Warning,
            );
        }
        return;
    };

    let generation = data.quota.generation();
    data.quota.mark_loading(target.clone(), manual);
    if let Err(error) = tx.send(QuotaReq::Refresh {
        generation,
        target: target.clone(),
    }) {
        data.quota.finish_error(target, error.to_string());
        app.push_toast(
            texts::tui_toast_quota_refresh_failed(&error.to_string()),
            ToastKind::Warning,
        );
    } else if manual {
        app.push_toast(
            texts::tui_toast_quota_refresh_started(&target.provider_name),
            ToastKind::Info,
        );
    }
}

/// Liveness of the background session-usage import, sampled once per tick.
/// The progress numbers themselves stay a render-time read; only the round's
/// start tick has to be remembered, and only the loop sees every tick.
fn session_usage_sync_is_active() -> bool {
    crate::services::session_usage::sync_progress::snapshot().is_some()
}

fn queue_current_quota_refresh_if_due(
    app: &mut App,
    data: &mut data::UiData,
    quota_req_tx: Option<&mpsc::Sender<QuotaReq>>,
) {
    let Some(target) = data::quota_target_for_current_provider(&app.app_type, data) else {
        app.quota_auto_target_key = None;
        app.quota_last_auto_tick = None;
        return;
    };

    let target_key = target.cache_key();
    let target_changed = app.quota_auto_target_key.as_deref() != Some(target_key.as_str());
    let target_missing_state = data.quota.state_for(&target.provider_id).is_none();
    let refresh_interval_ticks = target
        .auto_query_interval_minutes
        .checked_mul(TUI_TICKS_PER_MINUTE)
        .filter(|ticks| *ticks > 0);
    let due = refresh_interval_ticks.is_some_and(|ticks| {
        app.quota_last_auto_tick
            .is_none_or(|last_tick| app.tick.saturating_sub(last_tick) >= ticks)
    });

    if target_changed || target_missing_state || due {
        app.quota_auto_target_key = Some(target_key);
        app.quota_last_auto_tick = Some(app.tick);
        queue_quota_refresh(app, data, quota_req_tx, target, false);
    }
}

fn refresh_quota_after_provider_runtime_change(
    app: &mut App,
    data: &mut data::UiData,
    quota_req_tx: Option<&mpsc::Sender<QuotaReq>>,
) {
    data.quota.invalidate();
    app.quota_auto_target_key = None;
    app.quota_last_auto_tick = None;
    queue_current_quota_refresh_if_due(app, data, quota_req_tx);
}

fn queue_provider_quota_refresh(
    app: &mut App,
    data: &mut data::UiData,
    quota_req_tx: Option<&mpsc::Sender<QuotaReq>>,
    provider_id: &str,
) {
    let Some(row) = data.providers.rows.iter().find(|row| row.id == provider_id) else {
        return;
    };
    let Some(target) = data::quota_target_for_provider(&app.app_type, row) else {
        app.push_toast(texts::tui_toast_quota_not_available(), ToastKind::Info);
        return;
    };

    queue_quota_refresh(app, data, quota_req_tx, target, true);
}

fn queue_proxy_snapshot_refresh(
    tracker: &mut RequestTracker,
    proxy_req_tx: Option<&mpsc::Sender<ProxyReq>>,
    app_type: &AppType,
) {
    let Some(tx) = proxy_req_tx else {
        return;
    };
    if tracker.active.is_some() {
        return;
    }

    let request_id = tracker.start();
    if tx
        .send(ProxyReq::RefreshSnapshot {
            request_id,
            app_type: app_type.clone(),
        })
        .is_err()
    {
        tracker.cancel();
    }
}

fn queue_proxy_snapshot_refresh_after_app_switch(
    tracker: &mut RequestTracker,
    proxy_req_tx: Option<&mpsc::Sender<ProxyReq>>,
    before_app_type: &AppType,
    app_type: &AppType,
    action_succeeded: bool,
) {
    if !action_succeeded || before_app_type == app_type {
        return;
    }

    tracker.cancel();
    queue_proxy_snapshot_refresh(tracker, proxy_req_tx, app_type);
}

#[derive(Clone, Copy)]
struct ProxyEffectWorkers<'a> {
    app_data: Option<&'a mpsc::Sender<AppDataReq>>,
    proxy: Option<&'a mpsc::Sender<ProxyReq>>,
    quota: Option<&'a mpsc::Sender<QuotaReq>>,
}

fn apply_proxy_msg_effect(
    app: &mut App,
    data: &mut data::UiData,
    data_cache: &mut UiDataByAppCache,
    workers: ProxyEffectWorkers<'_>,
    proxy_snapshot_refresh: &mut RequestTracker,
    effect: ProxyMsgEffect,
) {
    match effect {
        ProxyMsgEffect::None => {}
        ProxyMsgEffect::ApplyProviderRuntime {
            app_type,
            base_reload_token,
            snapshot,
        } => {
            let applied = data_cache.apply_provider_runtime_snapshot(
                app,
                data,
                &app_type,
                base_reload_token,
                snapshot,
            );

            proxy_snapshot_refresh.cancel();
            if applied.current_app && applied.snapshot_applied {
                app.reset_proxy_activity(
                    data.proxy.estimated_input_tokens_total,
                    data.proxy.estimated_output_tokens_total,
                );
            } else {
                // The managed operation changed process-global runtime state
                // while another app was visible, or its result was older than
                // a completed foreground update. Refresh the visible app rather
                // than applying an app-scoped stale snapshot to it.
                queue_proxy_snapshot_refresh(proxy_snapshot_refresh, workers.proxy, &app.app_type);
            }

            refresh_quota_after_provider_runtime_change(app, data, workers.quota);

            if applied.needs_base_reload {
                let _ = data_cache.queue_app_data_load(app, workers.app_data, &app_type);
            }
        }
        ProxyMsgEffect::ReloadProviderRuntime { app_type } => {
            // The backend mutation succeeded, but its targeted projection could
            // not be built. Keep the visible stale-while-revalidate data and
            // recover through the existing background app-data system.
            data_cache.remove_app_snapshot(&app_type);
            let _ = data_cache.queue_app_data_load(app, workers.app_data, &app_type);

            refresh_quota_after_provider_runtime_change(app, data, workers.quota);
            proxy_snapshot_refresh.cancel();
            queue_proxy_snapshot_refresh(proxy_snapshot_refresh, workers.proxy, &app.app_type);
        }
    }
}

#[derive(Default)]
struct UiDataByAppCache {
    by_app: HashMap<AppType, data::UiData>,
    pending_by_app: HashMap<AppType, PendingAppDataLoad>,
    incomplete_by_app: HashSet<AppType>,
    usage_pricing_by_key: HashMap<UsagePricingLoadKey, data::UsagePricingData>,
    pending_usage_pricing_by_key: HashMap<UsagePricingLoadKey, PendingDataLoad>,
    usage_pricing_phase_by_key: HashMap<UsagePricingLoadKey, UsagePricingLoadPhase>,
    /// Usage rows are imported by a separate background connection. A dirty
    /// key means that an aggregate which started before the latest import
    /// progress must be followed by one fresh aggregate after it completes.
    ///
    /// Keeping this separate from `data_generation` is deliberate: log page
    /// and detail requests use that generation as their snapshot token and
    /// must remain valid while the importer is making progress.
    usage_pricing_dirty_by_key: HashSet<UsagePricingLoadKey>,
    next_app_data_request_id: u64,
    next_usage_pricing_request_id: u64,
    data_generation: u64,
    app_state_epoch: u64,
}

type UsagePricingLoadKey = (AppType, data::UsageRangePreset);

fn usage_pricing_logical_range(range: data::UsageRangePreset) -> data::UsageRangePreset {
    match range {
        data::UsageRangePreset::Custom(_) => range,
        // Every fixed request computes the same Today/7d/30d snapshot and the
        // same unbounded log head. Use one key so rapid range switching cannot
        // enqueue three equivalent full-table scans.
        data::UsageRangePreset::Today
        | data::UsageRangePreset::SevenDays
        | data::UsageRangePreset::ThirtyDays => data::UsageRangePreset::SevenDays,
    }
}

fn usage_pricing_load_key(
    app_type: &AppType,
    range: data::UsageRangePreset,
) -> UsagePricingLoadKey {
    (app_type.clone(), usage_pricing_logical_range(range))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingAppDataLoad {
    kind: AppDataLoadKind,
    request_id: u64,
    generation: u64,
    app_state_epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingDataLoad {
    request_id: u64,
    generation: u64,
    app_state_epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct UsagePricingLoadPhase {
    token: PendingDataLoad,
    aggregate_finished: bool,
    aggregate_succeeded: bool,
    head_finished: bool,
}

impl UsagePricingLoadPhase {
    fn new(token: PendingDataLoad) -> Self {
        Self {
            token,
            aggregate_finished: false,
            aggregate_succeeded: false,
            head_finished: false,
        }
    }
}

/// Fixed aggregates can always merge because they do not replace custom
/// fields. A custom aggregate may only merge while that exact window remains
/// selected.
fn usage_pricing_range_matches_active(
    cached_range: data::UsageRangePreset,
    active_range: data::UsageRangePreset,
) -> bool {
    match (cached_range, active_range) {
        (data::UsageRangePreset::Custom(cached), data::UsageRangePreset::Custom(active)) => {
            cached == active
        }
        (data::UsageRangePreset::Custom(_), _) => false,
        _ => true,
    }
}

/// Whether a cached `(app, cached_range)` entry already provides the data a
/// request for `requested` needs. The fixed ranges (Today/7d/30d) are computed
/// together, so any cached fixed range covers another; a custom range must
/// match exactly, and fixed/custom never cover each other.
fn usage_pricing_cache_satisfies(
    cached: data::UsageRangePreset,
    requested: data::UsageRangePreset,
) -> bool {
    match (cached, requested) {
        (data::UsageRangePreset::Custom(cached), data::UsageRangePreset::Custom(requested)) => {
            cached == requested
        }
        (data::UsageRangePreset::Custom(_), _) | (_, data::UsageRangePreset::Custom(_)) => false,
        _ => true,
    }
}

fn align_usage_to_active_range(
    usage: &mut data::UsageSnapshot,
    active_range: data::UsageRangePreset,
) {
    let data::UsageRangePreset::Custom(active_custom_range) = active_range else {
        return;
    };
    if usage.custom_range != Some(active_custom_range) {
        usage.begin_custom_range(active_custom_range);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CacheInvalidation {
    None,
    CurrentAppDataChanged,
    /// The current app's data was fully reloaded fresh (e.g. the post-startup
    /// SyncLive refresh, or a current-app mutation's reload). Only the current
    /// app's cache entry is refreshed; other apps' pre-seeded snapshots are kept
    /// (their DB rows weren't touched), so a cold switch to them stays instant.
    CurrentAppReloaded,
    DataReloaded,
    AppStateRecreated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppDataLoadQueued {
    Queued,
    AlreadyPending,
    Unavailable,
    SendFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppDataLoadFinish {
    Accepted,
    Stale,
    Ignored,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProviderRuntimeApply {
    current_app: bool,
    snapshot_applied: bool,
    needs_base_reload: bool,
}

impl UiDataByAppCache {
    fn remember_current(&mut self, app_type: &AppType, data: &data::UiData) {
        if self.pending_by_app.contains_key(app_type) || self.incomplete_by_app.contains(app_type) {
            return;
        }
        self.by_app.insert(app_type.clone(), data.clone());
    }

    fn update_usage_pricing(
        &mut self,
        app_type: &AppType,
        range: data::UsageRangePreset,
        usage_pricing: data::UsagePricingData,
    ) {
        let range = usage_pricing_logical_range(range);
        if let Some(cached) = self.by_app.get_mut(app_type) {
            cached.usage.merge_range(range, usage_pricing.usage.clone());
            if let Some(pricing) = &usage_pricing.pricing {
                cached.pricing = pricing.clone();
            }
        }
        self.usage_pricing_by_key
            .insert(usage_pricing_load_key(app_type, range), usage_pricing);
    }

    fn merge_usage_pricing(
        &self,
        app_type: &AppType,
        data: &mut data::UiData,
        active_range: data::UsageRangePreset,
    ) {
        for ((cached_app_type, range), usage_pricing) in &self.usage_pricing_by_key {
            if cached_app_type != app_type {
                continue;
            }
            if !usage_pricing_range_matches_active(*range, active_range) {
                continue;
            }
            data.usage.merge_range(*range, usage_pricing.usage.clone());
            if let Some(pricing) = &usage_pricing.pricing {
                data.pricing = pricing.clone();
            }
        }
        align_usage_to_active_range(&mut data.usage, active_range);
    }

    fn clear(&mut self) {
        self.data_generation = self.data_generation.wrapping_add(1);
        self.by_app.clear();
        self.pending_by_app.clear();
        self.incomplete_by_app.clear();
        self.usage_pricing_by_key.clear();
        self.pending_usage_pricing_by_key.clear();
        self.usage_pricing_phase_by_key.clear();
        self.usage_pricing_dirty_by_key.clear();
    }

    fn clear_after_app_state_recreated(&mut self) {
        self.app_state_epoch = self.app_state_epoch.wrapping_add(1);
        self.clear();
    }

    /// Invalidate completed aggregates after the external session importer
    /// commits, without invalidating unrelated app loads or stable log-page
    /// request tokens. Existing values remain visible in `by_app` until their
    /// stale-while-revalidate refresh completes.
    fn invalidate_usage_pricing_cache_after_external_usage_sync(&mut self) {
        self.usage_pricing_by_key.clear();
        // Every aggregate that was already running crossed the import's commit
        // boundary, including requests for apps that are not currently
        // visible. Keep their tokens valid, but require one fresh successor so
        // a late result cannot repopulate the cache with a pre-sync snapshot.
        self.usage_pricing_dirty_by_key
            .extend(self.pending_usage_pricing_by_key.keys().cloned());
    }

    /// Invalidate one app after the proxy persisted new usage. As with an
    /// external import, in-flight requests finish normally and earn exactly
    /// one follow-up instead of being cancelled.
    fn invalidate_usage_pricing_cache_for_app(&mut self, app_type: &AppType) {
        self.usage_pricing_by_key
            .retain(|(cached_app_type, _), _| cached_app_type != app_type);
        self.usage_pricing_dirty_by_key.extend(
            self.pending_usage_pricing_by_key
                .keys()
                .filter(|(pending_app_type, _)| pending_app_type == app_type)
                .cloned(),
        );
    }

    fn remove_app_snapshot(&mut self, app_type: &AppType) {
        self.by_app.remove(app_type);
        self.pending_by_app.remove(app_type);
        self.incomplete_by_app.remove(app_type);
    }

    fn apply_provider_runtime_snapshot(
        &mut self,
        app: &mut App,
        data: &mut data::UiData,
        app_type: &AppType,
        base_reload_token: data::UiDataReloadToken,
        snapshot: Box<data::ProviderRuntimeSnapshot>,
    ) -> ProviderRuntimeApply {
        // A base-data request that started before the managed proxy mutation may
        // otherwise land afterward and overwrite this newer provider/proxy
        // projection. Retire its token; the caller queues one fresh successor.
        let superseded_pending = self.pending_by_app.remove(app_type).is_some();
        let was_incomplete = self.incomplete_by_app.remove(app_type);

        let current_app = &app.app_type == app_type;
        let applied = if current_app {
            if data.reload_token == base_reload_token {
                // `switch_to` remembers the visible snapshot before leaving
                // this app, so retaining an older duplicate here only creates
                // another stale source.
                self.by_app.remove(app_type);
                data.apply_provider_runtime_snapshot(*snapshot);
                app.clamp_selections(data);
                true
            } else {
                false
            }
        } else if let Some(cached) = self.by_app.get_mut(app_type) {
            if cached.reload_token == base_reload_token {
                cached.apply_provider_runtime_snapshot(*snapshot);
                true
            } else {
                false
            }
        } else {
            false
        };

        ProviderRuntimeApply {
            current_app,
            snapshot_applied: applied,
            needs_base_reload: superseded_pending || was_incomplete || !applied,
        }
    }

    fn remove_usage_pricing_for_app(&mut self, app_type: &AppType) {
        self.usage_pricing_by_key
            .retain(|(cached_app_type, _), _| cached_app_type != app_type);
        self.pending_usage_pricing_by_key
            .retain(|(cached_app_type, _), _| cached_app_type != app_type);
        self.usage_pricing_phase_by_key
            .retain(|(cached_app_type, _), _| cached_app_type != app_type);
        self.usage_pricing_dirty_by_key
            .retain(|(cached_app_type, _)| cached_app_type != app_type);
    }

    fn queue_current_app_data_refresh(
        &mut self,
        app_data_req_tx: Option<&mpsc::Sender<AppDataReq>>,
        app_type: &AppType,
    ) -> AppDataLoadQueued {
        if self.pending_by_app.contains_key(app_type) {
            return AppDataLoadQueued::AlreadyPending;
        }

        let Some(tx) = app_data_req_tx else {
            return AppDataLoadQueued::Unavailable;
        };

        self.next_app_data_request_id = self.next_app_data_request_id.wrapping_add(1);
        let request_id = self.next_app_data_request_id;
        let pending = PendingAppDataLoad {
            kind: AppDataLoadKind::Full,
            request_id,
            generation: self.data_generation,
            app_state_epoch: self.app_state_epoch,
        };
        self.pending_by_app.insert(app_type.clone(), pending);
        self.incomplete_by_app.insert(app_type.clone());
        if tx
            .send(AppDataReq::FullLoad {
                request_id,
                generation: pending.generation,
                app_state_epoch: pending.app_state_epoch,
                app_type: app_type.clone(),
            })
            .is_err()
        {
            self.remove_app_snapshot(app_type);
            AppDataLoadQueued::SendFailed
        } else {
            AppDataLoadQueued::Queued
        }
    }

    fn queue_app_data_load(
        &mut self,
        app: &mut App,
        app_data_req_tx: Option<&mpsc::Sender<AppDataReq>>,
        app_type: &AppType,
    ) -> AppDataLoadQueued {
        if self.pending_by_app.contains_key(app_type) {
            return AppDataLoadQueued::AlreadyPending;
        }

        let Some(tx) = app_data_req_tx else {
            self.incomplete_by_app.insert(app_type.clone());
            app.push_toast(
                "App data worker is not running; reload data to refresh this app.".to_string(),
                ToastKind::Warning,
            );
            return AppDataLoadQueued::Unavailable;
        };

        self.next_app_data_request_id = self.next_app_data_request_id.wrapping_add(1);
        let request_id = self.next_app_data_request_id;
        let pending = PendingAppDataLoad {
            kind: AppDataLoadKind::Snapshot,
            request_id,
            generation: self.data_generation,
            app_state_epoch: self.app_state_epoch,
        };
        self.pending_by_app.insert(app_type.clone(), pending);
        self.incomplete_by_app.insert(app_type.clone());
        if let Err(err) = tx.send(AppDataReq::Load {
            request_id,
            generation: pending.generation,
            app_state_epoch: pending.app_state_epoch,
            app_type: app_type.clone(),
        }) {
            self.pending_by_app.remove(app_type);
            app.push_toast(
                format!("App data refresh request failed: {err}"),
                ToastKind::Warning,
            );
            AppDataLoadQueued::SendFailed
        } else {
            AppDataLoadQueued::Queued
        }
    }

    fn queue_initial_app_data_load(
        &mut self,
        app_data_req_tx: &mpsc::Sender<AppDataReq>,
        app_type: &AppType,
        extra_apps: &[AppType],
    ) -> Result<PendingAppDataLoad, AppError> {
        if self.pending_by_app.contains_key(app_type) {
            return Err(AppError::Message(
                "Initial app data load is already pending.".to_string(),
            ));
        }

        self.next_app_data_request_id = self.next_app_data_request_id.wrapping_add(1);
        let pending = PendingAppDataLoad {
            kind: AppDataLoadKind::Initial,
            request_id: self.next_app_data_request_id,
            generation: self.data_generation,
            app_state_epoch: self.app_state_epoch,
        };
        self.pending_by_app.insert(app_type.clone(), pending);
        self.incomplete_by_app.insert(app_type.clone());

        // Warm the remaining visible apps from the same in-memory snapshot the
        // worker already holds, so the first switch to them renders real data
        // instead of the empty "no providers" placeholder. Each gets its own
        // pending Initial entry keyed by request_id; they share generation/epoch.
        let mut extras: Vec<(AppType, u64)> = Vec::new();
        for extra in extra_apps {
            if extra == app_type || self.pending_by_app.contains_key(extra) {
                continue;
            }
            self.next_app_data_request_id = self.next_app_data_request_id.wrapping_add(1);
            let extra_request_id = self.next_app_data_request_id;
            self.pending_by_app.insert(
                extra.clone(),
                PendingAppDataLoad {
                    kind: AppDataLoadKind::Initial,
                    request_id: extra_request_id,
                    generation: pending.generation,
                    app_state_epoch: pending.app_state_epoch,
                },
            );
            self.incomplete_by_app.insert(extra.clone());
            extras.push((extra.clone(), extra_request_id));
        }

        if let Err(err) = app_data_req_tx.send(AppDataReq::InitialLoad {
            request_id: pending.request_id,
            generation: pending.generation,
            app_state_epoch: pending.app_state_epoch,
            app_type: app_type.clone(),
            extras: extras.clone(),
        }) {
            self.pending_by_app.remove(app_type);
            self.incomplete_by_app.remove(app_type);
            for (extra, _) in &extras {
                self.pending_by_app.remove(extra);
                self.incomplete_by_app.remove(extra);
            }
            return Err(AppError::Message(format!(
                "Initial app data load request failed: {err}"
            )));
        }

        Ok(pending)
    }

    fn finish_app_data_load(
        &mut self,
        kind: AppDataLoadKind,
        app_type: &AppType,
        request_id: u64,
        generation: u64,
        app_state_epoch: u64,
    ) -> AppDataLoadFinish {
        if generation != self.data_generation || app_state_epoch != self.app_state_epoch {
            let completed = PendingAppDataLoad {
                kind,
                request_id,
                generation,
                app_state_epoch,
            };
            if self.pending_by_app.get(app_type).copied() == Some(completed) {
                self.pending_by_app.remove(app_type);
            }
            return AppDataLoadFinish::Stale;
        }

        if self.pending_by_app.get(app_type).copied()
            != Some(PendingAppDataLoad {
                kind,
                request_id,
                generation,
                app_state_epoch,
            })
        {
            return AppDataLoadFinish::Ignored;
        }
        self.pending_by_app.remove(app_type);
        AppDataLoadFinish::Accepted
    }

    fn mark_app_data_loaded(&mut self, app_type: &AppType) {
        self.incomplete_by_app.remove(app_type);
    }

    /// Queue a usage/pricing load only if that (app, range) isn't already cached
    /// or in flight. Used to lazily populate usage when the Usage view opens,
    /// now that the startup full-load defers the aggregation.
    fn ensure_usage_pricing_loaded(
        &mut self,
        app: &mut App,
        usage_pricing_req_tx: Option<&mpsc::Sender<UsagePricingReq>>,
        app_type: &AppType,
        range: data::UsageRangePreset,
    ) {
        let key = usage_pricing_load_key(app_type, range);
        if self.pending_usage_pricing_by_key.contains_key(&key) {
            return;
        }
        // A cached fixed range already covers any other fixed range, so don't
        // re-aggregate when the user toggles Today/7d/30d.
        let already_cached = self
            .usage_pricing_by_key
            .keys()
            .any(|(cached_app, cached_range)| {
                cached_app == app_type && usage_pricing_cache_satisfies(*cached_range, range)
            });
        if already_cached {
            return;
        }
        self.queue_usage_pricing_load(app, usage_pricing_req_tx, app_type, range);
    }

    /// Record that a session-import commit happened after the currently
    /// visible aggregate may have started. This does not cancel an in-flight
    /// query. `flush_dirty_usage_pricing` starts one follow-up once all
    /// aggregate requests already known to the UI have completed.
    fn mark_usage_pricing_dirty(&mut self, app_type: &AppType, range: data::UsageRangePreset) {
        self.usage_pricing_dirty_by_key
            .insert(usage_pricing_load_key(app_type, range));
    }

    /// Start at most one dirty aggregate. The current route's visible
    /// projection wins when both a custom window and the fixed cache need
    /// refreshing.
    fn flush_dirty_usage_pricing(
        &mut self,
        app: &mut App,
        usage_pricing_req_tx: Option<&mpsc::Sender<UsagePricingReq>>,
    ) -> bool {
        if !self.pending_usage_pricing_by_key.is_empty() {
            return false;
        }

        let visible_range = app
            .usage_pricing_load_range_for_current_route()
            .unwrap_or(app.usage.range);
        let active_key = usage_pricing_load_key(&app.app_type, visible_range);
        let key = if self.usage_pricing_dirty_by_key.contains(&active_key) {
            active_key
        } else {
            let Some(key) = self.usage_pricing_dirty_by_key.iter().next().cloned() else {
                return false;
            };
            key
        };
        self.usage_pricing_dirty_by_key.remove(&key);
        let (app_type, range) = key;
        self.queue_usage_pricing_load(app, usage_pricing_req_tx, &app_type, range)
    }

    fn queue_usage_pricing_load(
        &mut self,
        app: &mut App,
        usage_pricing_req_tx: Option<&mpsc::Sender<UsagePricingReq>>,
        app_type: &AppType,
        range: data::UsageRangePreset,
    ) -> bool {
        let range = usage_pricing_logical_range(range);
        let key = usage_pricing_load_key(app_type, range);
        if self.pending_usage_pricing_by_key.contains_key(&key) {
            app.usage.start_loading(app_type.clone(), range);
            return true;
        }

        if matches!(range, data::UsageRangePreset::Custom(_)) {
            app.usage.clear_custom_loading_for_app(app_type);
            self.pending_usage_pricing_by_key
                .retain(|(cached_app_type, cached_range), _| {
                    cached_app_type != app_type
                        || !matches!(cached_range, data::UsageRangePreset::Custom(_))
                });
            self.usage_pricing_phase_by_key
                .retain(|(cached_app_type, cached_range), _| {
                    cached_app_type != app_type
                        || !matches!(cached_range, data::UsageRangePreset::Custom(_))
                });
            self.usage_pricing_by_key
                .retain(|(cached_app_type, cached_range), _| {
                    cached_app_type != app_type
                        || !matches!(cached_range, data::UsageRangePreset::Custom(_))
                });
            self.usage_pricing_dirty_by_key
                .retain(|(cached_app_type, cached_range)| {
                    cached_app_type != app_type
                        || !matches!(cached_range, data::UsageRangePreset::Custom(_))
                });
        }

        let Some(tx) = usage_pricing_req_tx else {
            if matches!(range, data::UsageRangePreset::Custom(_)) {
                app.push_toast(
                    "Usage/pricing worker is not running; custom range cannot be loaded."
                        .to_string(),
                    ToastKind::Warning,
                );
            }
            return false;
        };

        self.next_usage_pricing_request_id = self.next_usage_pricing_request_id.wrapping_add(1);
        let request_id = self.next_usage_pricing_request_id;
        let pending = PendingDataLoad {
            request_id,
            generation: self.data_generation,
            app_state_epoch: self.app_state_epoch,
        };
        self.pending_usage_pricing_by_key
            .insert(key.clone(), pending);
        self.usage_pricing_phase_by_key
            .insert(key.clone(), UsagePricingLoadPhase::new(pending));

        if let Err(err) = tx.send(UsagePricingReq::Load {
            request_id,
            generation: pending.generation,
            app_state_epoch: pending.app_state_epoch,
            app_type: app_type.clone(),
            range,
        }) {
            self.pending_usage_pricing_by_key.remove(&key);
            if self
                .usage_pricing_phase_by_key
                .get(&key)
                .is_some_and(|phase| phase.token == pending)
            {
                self.usage_pricing_phase_by_key.remove(&key);
            }
            log::debug!("home-usage: usage aggregate request failed to queue: {err}");
            app.push_toast(
                format!("Usage/pricing refresh request failed: {err}"),
                ToastKind::Warning,
            );
            false
        } else {
            log::debug!("home-usage: usage aggregate queued app={app_type:?} range={range:?}");
            app.usage.start_loading(app_type.clone(), range);
            true
        }
    }

    fn finish_usage_pricing_load(
        &mut self,
        app_type: &AppType,
        request_id: u64,
        generation: u64,
        app_state_epoch: u64,
        range: data::UsageRangePreset,
        aggregate_succeeded: bool,
    ) -> bool {
        let key = usage_pricing_load_key(app_type, range);
        let completed = PendingDataLoad {
            request_id,
            generation,
            app_state_epoch,
        };
        if self.pending_usage_pricing_by_key.get(&key).copied() != Some(completed) {
            return false;
        }
        self.pending_usage_pricing_by_key.remove(&key);
        if let Some(phase) = self.usage_pricing_phase_by_key.get_mut(&key) {
            if phase.token == completed {
                phase.aggregate_finished = true;
                phase.aggregate_succeeded = aggregate_succeeded;
            }
        }
        true
    }

    /// Finish the head phase and report whether its matching aggregate already
    /// supplied an exact count. The phase token remains as the latest-token
    /// guard after either half completes, so both delivery orders are valid.
    fn finish_usage_log_head(
        &mut self,
        app_type: &AppType,
        request_id: u64,
        generation: u64,
        app_state_epoch: u64,
        range: data::UsageRangePreset,
    ) -> Option<bool> {
        let key = usage_pricing_load_key(app_type, range);
        let completed = PendingDataLoad {
            request_id,
            generation,
            app_state_epoch,
        };
        let phase = self.usage_pricing_phase_by_key.get_mut(&key)?;
        if phase.token != completed {
            return None;
        }
        phase.head_finished = true;
        Some(phase.aggregate_finished && phase.aggregate_succeeded)
    }

    fn update_usage_log_head(
        &mut self,
        app_type: &AppType,
        range: data::UsageRangePreset,
        page: data::UsageLogPage,
        preserve_exact_total: bool,
    ) {
        if let Some(cached) = self.by_app.get_mut(app_type) {
            if preserve_exact_total {
                cached
                    .usage
                    .merge_log_head_preserving_exact_total(range, page.clone());
            } else {
                cached.usage.merge_log_head(range, page.clone());
            }
        }
        let key = usage_pricing_load_key(app_type, range);
        if let Some(cached) = self.usage_pricing_by_key.get_mut(&key) {
            if preserve_exact_total {
                cached
                    .usage
                    .merge_log_head_preserving_exact_total(range, page);
            } else {
                cached.usage.merge_log_head(range, page);
            }
        }
    }

    fn handle_data_reloaded(
        &mut self,
        app: &App,
        data: &data::UiData,
        invalidation: CacheInvalidation,
    ) {
        match invalidation {
            CacheInvalidation::None | CacheInvalidation::CurrentAppDataChanged => {}
            // Current app reloaded fresh: keep every other app's cache (and the
            // data_generation) intact; only the current entry is refreshed below.
            CacheInvalidation::CurrentAppReloaded => {}
            CacheInvalidation::DataReloaded => self.clear(),
            CacheInvalidation::AppStateRecreated => self.clear_after_app_state_recreated(),
        }

        if !matches!(
            invalidation,
            CacheInvalidation::None | CacheInvalidation::CurrentAppDataChanged
        ) {
            self.remember_current(&app.app_type, data);
        }
    }

    fn switch_to(
        &mut self,
        app: &mut App,
        data: &mut data::UiData,
        app_data_req_tx: Option<&mpsc::Sender<AppDataReq>>,
        next: AppType,
    ) -> Result<(), AppError> {
        if app.app_type == next {
            return Ok(());
        }

        let current = app.app_type.clone();
        self.remember_current(&current, data);

        let mut next_data = if let Some(cached) = self.by_app.get(&next) {
            cached.clone()
        } else {
            self.queue_app_data_load(app, app_data_req_tx, &next);
            data.app_switch_loading_projection(&next)
        };
        // A selected custom window and the fixed snapshot occupy disjoint
        // fields, so this retains both while Main consumes only the fixed
        // 30-day projection.
        self.merge_usage_pricing(&next, &mut next_data, app.usage.range);
        next_data.quota = data.quota.clone();

        apply_preloaded_app_switch(app, data, next, next_data);
        app.clamp_selections(data);
        app.maybe_prompt_import_candidate(data);
        Ok(())
    }
}

fn handle_usage_pricing_msg(
    app: &mut App,
    data: &mut data::UiData,
    data_cache: &mut UiDataByAppCache,
    msg: UsagePricingMsg,
) -> bool {
    match msg {
        UsagePricingMsg::LogHeadLoaded {
            request_id,
            generation,
            app_state_epoch,
            app_type,
            range,
            result,
        } => {
            let Some(preserve_exact_total) = data_cache.finish_usage_log_head(
                &app_type,
                request_id,
                generation,
                app_state_epoch,
                range,
            ) else {
                return false;
            };
            let Ok(page) = result else {
                // The full load reports the actionable error. A failed fast
                // head must not turn a later successful aggregate into a toast.
                return false;
            };
            data_cache.update_usage_log_head(&app_type, range, page.clone(), preserve_exact_total);
            if app.app_type == app_type
                && usage_pricing_range_matches_active(range, app.usage.range)
            {
                if preserve_exact_total {
                    data.usage
                        .merge_log_head_preserving_exact_total(range, page);
                } else {
                    data.usage.merge_log_head(range, page);
                }
                app.clamp_selections(data);
            }
            false
        }
        UsagePricingMsg::Loaded {
            request_id,
            generation,
            app_state_epoch,
            app_type,
            range,
            result,
        } => {
            let aggregate_succeeded = result.is_ok();
            if !data_cache.finish_usage_pricing_load(
                &app_type,
                request_id,
                generation,
                app_state_epoch,
                range,
                aggregate_succeeded,
            ) {
                return false;
            }
            app.usage.finish_loading(&app_type, range);

            match *result {
                Ok(usage_pricing) => {
                    data_cache.update_usage_pricing(&app_type, range, usage_pricing.clone());
                    if app.app_type == app_type {
                        // Aggregate/head refreshes are background updates. The
                        // active pager owns a stable keyset snapshot, so do not
                        // invalidate it here. Fixed and custom fields are
                        // disjoint, so a fixed result may land after a route
                        // change; only an obsolete custom result is cache-only.
                        if usage_pricing_range_matches_active(range, app.usage.range) {
                            data.usage.merge_range(range, usage_pricing.usage);
                            app.clamp_selections(data);
                        }
                        if let Some(pricing) = usage_pricing.pricing {
                            data.pricing = pricing;
                        }
                        data_cache.remember_current(&app.app_type, data);
                    }
                }
                Err(UsagePricingLoadError::Cancelled) => {}
                Err(UsagePricingLoadError::Failed(err)) => {
                    if app.app_type == app_type {
                        app.push_toast(
                            format!("Usage/pricing refresh failed: {err}"),
                            ToastKind::Warning,
                        );
                    }
                }
            }
            true
        }
        UsagePricingMsg::LogPageLoaded {
            request_id,
            generation,
            app_state_epoch,
            app_type,
            range,
            page,
            direction,
            result,
        } => {
            if generation != data_cache.data_generation
                || app_state_epoch != data_cache.app_state_epoch
            {
                if app.app_type == app_type && app.usage.range == range {
                    app.usage
                        .log_pager
                        .cancel_request(page, request_id, direction);
                }
                return false;
            }
            if app.app_type != app_type || app.usage.range != range {
                return false;
            }
            let refresh_targets_current_page = app
                .usage
                .log_pager
                .request_is_refresh(page, request_id, direction)
                && app.usage.log_pager.current_page() == page;
            match result {
                Ok(loaded) => {
                    let accepted = app
                        .usage
                        .log_pager
                        .finish_request(page, request_id, direction, loaded);
                    if accepted && refresh_targets_current_page {
                        app.usage.sync_current_log_selection(
                            data.usage.recent_logs_for(app.usage.range),
                        );
                    }
                }
                Err(UsageLogLoadError::Cancelled) => {
                    app.usage
                        .log_pager
                        .cancel_request(page, request_id, direction);
                }
                Err(UsageLogLoadError::Failed(err)) => {
                    let refresh_failed =
                        app.usage
                            .log_pager
                            .fail_request(page, request_id, direction, err.clone())
                            && refresh_targets_current_page;
                    if refresh_failed {
                        app.push_toast(
                            format!("Usage log page refresh failed: {err}"),
                            ToastKind::Warning,
                        );
                    }
                }
            }
            false
        }
        UsagePricingMsg::LogDetailLoaded {
            request_id,
            generation,
            app_state_epoch,
            app_type,
            range,
            log_rowid,
            result,
        } => {
            if generation != data_cache.data_generation
                || app_state_epoch != data_cache.app_state_epoch
            {
                if app.app_type == app_type && app.usage.range == range {
                    app.usage.finish_log_detail_refresh(request_id);
                }
                return false;
            }
            if app.app_type != app_type || app.usage.range != range {
                return false;
            }
            if !app.usage.finish_log_detail_refresh(request_id) {
                return false;
            }
            match result {
                Ok(Some(row)) if row.cursor_rowid == log_rowid => {
                    app.usage.remember_log_detail(app_type, range, row);
                }
                Ok(Some(_)) => app.push_toast(
                    "Usage log refresh returned a mismatched request.".to_string(),
                    ToastKind::Warning,
                ),
                Ok(None) => app.push_toast(
                    "The usage log no longer exists; keeping the previous details.".to_string(),
                    ToastKind::Warning,
                ),
                Err(UsageLogLoadError::Cancelled) => {}
                Err(UsageLogLoadError::Failed(err)) => app.push_toast(
                    format!("Usage log refresh failed: {err}"),
                    ToastKind::Warning,
                ),
            }
            false
        }
    }
}

fn queue_background_session_usage_sync(
    sync_req_tx: Option<&mpsc::Sender<SessionUsageSyncReq>>,
    sync_tracker: &mut RequestTracker,
) -> bool {
    let Some(tx) = sync_req_tx else {
        return false;
    };
    if sync_tracker.active.is_some() {
        return true;
    }

    let request_id = sync_tracker.start();
    if let Err(err) = tx.send(SessionUsageSyncReq::Run { request_id }) {
        sync_tracker.cancel();
        log::debug!("queue background session usage sync failed: {err}");
        false
    } else {
        true
    }
}

fn queue_current_session_cost(
    app: &mut App,
    cost_req_tx: Option<&runtime_systems::session_cost::SessionCostRequestSender>,
) -> bool {
    let Some(tx) = cost_req_tx else {
        app.sessions.fail_cost_overlay();
        return false;
    };
    let Some(request) = app.sessions.start_cost_overlay() else {
        let _ = queue_pending_session_cost_cancel(app, Some(tx));
        return false;
    };
    if tx.submit(request).is_err() {
        app.sessions.fail_cost_overlay();
        log::debug!("queue session cost projection failed: worker unavailable");
        false
    } else {
        true
    }
}

fn queue_pending_session_cost_cancel(
    app: &mut App,
    cost_req_tx: Option<&runtime_systems::session_cost::SessionCostRequestSender>,
) -> bool {
    let Some(cost_seq) = app.sessions.take_cost_cancel_request() else {
        return false;
    };
    let Some(tx) = cost_req_tx else {
        app.sessions.fail_cost_overlay();
        return false;
    };
    if tx.cancel(cost_seq).is_err() {
        app.sessions.fail_cost_overlay();
        log::debug!("cancel session cost projection failed: worker unavailable");
        false
    } else {
        true
    }
}

fn queue_pending_session_cost(
    app: &mut App,
    cost_req_tx: Option<&runtime_systems::session_cost::SessionCostRequestSender>,
) -> bool {
    let cancelled = queue_pending_session_cost_cancel(app, cost_req_tx);
    if !app.sessions.take_cost_overlay_request() {
        return cancelled;
    }
    queue_current_session_cost(app, cost_req_tx) || cancelled
}

fn queue_codex_usage_rebuild(
    app: &mut App,
    sync_req_tx: Option<&mpsc::Sender<SessionUsageSyncReq>>,
    sync_tracker: &mut RequestTracker,
) -> bool {
    if app.usage.codex_usage_rebuilding() {
        app.push_toast(
            texts::tui_toast_codex_usage_rebuild_running().to_string(),
            ToastKind::Info,
        );
        return false;
    }

    let Some(tx) = sync_req_tx else {
        app.push_toast(
            texts::tui_toast_codex_usage_rebuild_unavailable().to_string(),
            ToastKind::Error,
        );
        return false;
    };

    // A rebuild may be queued behind an already-running background import.
    // Replacing the active request token makes that older completion stale;
    // the worker is single-threaded and will run the rebuild next.
    let request_id = sync_tracker.start();
    if let Err(error) = tx.send(SessionUsageSyncReq::RebuildCodex { request_id }) {
        sync_tracker.cancel();
        app.push_toast(
            texts::tui_toast_codex_usage_rebuild_queued_failed(&error.to_string()),
            ToastKind::Error,
        );
        return false;
    }

    app.usage.start_codex_usage_rebuild();
    true
}

/// Ticks the home page waits before kicking the first session-usage scan
/// (~2s at [`TUI_TICK_RATE`]), so the first frames render undisturbed.
const USAGE_SESSION_SYNC_HOME_DELAY_TICKS: u64 = 10;
/// Cadence of the background session-usage refresh (60s at [`TUI_TICK_RATE`]).
const USAGE_AUTO_SYNC_INTERVAL_TICKS: u64 = 60 * 1000 / 200;
/// Proxy traffic already writes usage rows to SQLite; only the aggregate view
/// needs refreshing. Keep that query independent from the much heavier local
/// session scan and throttle fixed projections to once every ten seconds.
const USAGE_PROXY_REFRESH_INTERVAL_TICKS: u64 = 10 * 1000 / 200;

/// Lazily kick off the session-usage sync the first time a Usage route — or,
/// after a short delay, the home page — is shown this run. The sync scans the
/// whole session-log history (expensive on large histories), so it is deferred
/// off startup.
fn maybe_queue_usage_session_sync(
    app: &App,
    sync_req_tx: Option<&mpsc::Sender<SessionUsageSyncReq>>,
    sync_tracker: &mut RequestTracker,
    started: &mut bool,
    auto_sync_enabled: bool,
) {
    if *started || !auto_sync_enabled {
        return;
    }
    let on_usage_route = matches!(
        app.route,
        route::Route::Usage | route::Route::UsageLogs | route::Route::UsageLogDetail { .. }
    );
    // The home chart wants the same data, but never at the cost of the first
    // paint: wait a couple of seconds before scanning.
    let on_home_route =
        matches!(app.route, route::Route::Main) && app.tick >= USAGE_SESSION_SYNC_HOME_DELAY_TICKS;
    if !on_usage_route && !on_home_route {
        return;
    }
    *started = true;
    log::debug!(
        "home-usage: initial session sync queued (route={:?}, tick={})",
        app.route,
        app.tick
    );
    let _ = queue_background_session_usage_sync(sync_req_tx, sync_tracker);
}

/// Re-import local session logs every [`USAGE_AUTO_SYNC_INTERVAL_TICKS`] so the
/// home chart keeps up with activity outside the proxy. Opt-out through the
/// `usageAutoSync` setting; skipped while a scan is already running.
fn queue_usage_session_sync_if_due(
    app: &mut App,
    sync_req_tx: Option<&mpsc::Sender<SessionUsageSyncReq>>,
    sync_tracker: &mut RequestTracker,
) {
    if !crate::settings::usage_auto_sync_enabled() {
        return;
    }
    // Seeding on the first check measures the interval from TUI start instead
    // of firing a scan on the very first tick.
    let last_tick = *app.usage_last_auto_sync_tick.get_or_insert(app.tick);
    if app.tick.saturating_sub(last_tick) < USAGE_AUTO_SYNC_INTERVAL_TICKS {
        return;
    }
    app.usage_last_auto_sync_tick = Some(app.tick);

    if sync_tracker.active.is_some() {
        log::debug!("home-usage: periodic session sync skipped, a scan is already running");
        return;
    }
    log::debug!(
        "home-usage: periodic session sync queued at tick {}",
        app.tick
    );
    queue_background_session_usage_sync(sync_req_tx, sync_tracker);
}

/// Refresh the Usage projection consumed by the current route after proxy
/// activity, without tying it to `usageAutoSync`, which controls local
/// session-log I/O.
///
/// Fixed projections stay near-live at the home card's ten-second cadence.
/// Custom windows can span up to ten years, and nested log routes own
/// interactive queries; both retain the existing session-sync and
/// manual-refresh paths instead of joining this fast loop.
fn queue_usage_proxy_refresh_if_due(
    app: &mut App,
    data_cache: &mut UiDataByAppCache,
    usage_pricing_req_tx: Option<&mpsc::Sender<UsagePricingReq>>,
) {
    let Some(range) = app.proxy_refreshed_usage_projection_for_current_route() else {
        return;
    };
    let Some(tx) = usage_pricing_req_tx else {
        return;
    };
    if !app.take_proxy_usage_refresh_due(USAGE_PROXY_REFRESH_INTERVAL_TICKS) {
        return;
    }

    let app_type = app.app_type.clone();
    data_cache.invalidate_usage_pricing_cache_for_app(&app_type);
    data_cache.mark_usage_pricing_dirty(&app_type, range);
    data_cache.flush_dirty_usage_pricing(app, Some(tx));
}

/// Lazily load the active app's usage/pricing when Main, Usage, or Pricing
/// consumes it. The startup full-load no longer computes it, so this fills it
/// in on demand; the cache guard keeps it from re-querying every frame.
fn maybe_queue_usage_pricing_on_view(
    app: &mut App,
    data_cache: &mut UiDataByAppCache,
    usage_pricing_req_tx: Option<&mpsc::Sender<UsagePricingReq>>,
) {
    let Some(range) = app.usage_pricing_load_range_for_current_route() else {
        return;
    };
    let app_type = app.app_type.clone();
    data_cache.ensure_usage_pricing_loaded(app, usage_pricing_req_tx, &app_type, range);
}

fn maybe_queue_usage_log_prefetch(
    app: &mut App,
    data: &data::UiData,
    data_cache: &mut UiDataByAppCache,
    usage_pricing_req_tx: Option<&mpsc::Sender<UsagePricingReq>>,
) {
    if !matches!(app.route, route::Route::UsageLogs)
        || !matches!(app.usage.pane, app::UsagePane::Recent)
    {
        return;
    }

    // A manual current-page refresh owns the single log-query worker until it
    // completes. Speculative prefetching here would supersede that request.
    let current_page = app.usage.log_pager.current_page();
    if app.usage.log_page_refresh_after_aggregate_requested()
        || app.usage.log_pager.has_refresh_pending()
        || app.usage.log_pager.page_is_pending(current_page)
    {
        return;
    }

    let first_page = data.usage.recent_logs_for(app.usage.range);
    app.usage.sync_log_pager(
        &app.app_type,
        app.usage.range,
        first_page,
        data.usage.logs_total_for(app.usage.range),
    );
    let Some(page) = app.usage.log_pager.preferred_prefetch_page() else {
        return;
    };
    let Some(load) = app.usage.log_pager.page_request(page) else {
        return;
    };

    data_cache.next_usage_pricing_request_id =
        data_cache.next_usage_pricing_request_id.wrapping_add(1);
    let request_id = data_cache.next_usage_pricing_request_id;
    if !app
        .usage
        .log_pager
        .start_load_request(page, request_id, load)
    {
        return;
    }

    let Some(tx) = usage_pricing_req_tx else {
        app.usage.log_pager.fail_request(
            page,
            request_id,
            load.direction,
            "usage log worker is not running".to_string(),
        );
        return;
    };
    if let Err(err) = tx.send(UsagePricingReq::LoadLogPage {
        request_id,
        generation: data_cache.data_generation,
        app_state_epoch: data_cache.app_state_epoch,
        app_type: app.app_type.clone(),
        range: app.usage.range,
        page,
        cursor: load.cursor,
        direction: load.direction,
        limit: data::USAGE_LOG_PAGE_SIZE,
    }) {
        app.usage.log_pager.fail_request(
            page,
            request_id,
            load.direction,
            format!("failed to queue usage log page: {err}"),
        );
    }
}

/// Refresh a visible keyset page only after all aggregate work has settled.
/// The aggregate owns the same log-query worker for its fast head query, so
/// waiting here prevents the two refresh halves from repeatedly cancelling one
/// another. The old page remains visible until its exact anchored query wins.
fn maybe_queue_usage_log_page_refresh_after_aggregate(
    app: &mut App,
    data_cache: &mut UiDataByAppCache,
    usage_pricing_req_tx: Option<&mpsc::Sender<UsagePricingReq>>,
) {
    if !app.usage.log_page_refresh_after_aggregate_requested() {
        return;
    }
    if !matches!(app.route, route::Route::UsageLogs)
        || !matches!(app.usage.pane, app::UsagePane::Recent)
    {
        app.usage.finish_log_page_refresh_after_aggregate();
        return;
    }
    if !data_cache.pending_usage_pricing_by_key.is_empty()
        || !data_cache.usage_pricing_dirty_by_key.is_empty()
    {
        return;
    }

    let page = app.usage.log_pager.current_page();
    if page == 0 {
        app.usage.finish_log_page_refresh_after_aggregate();
        return;
    }
    if app.usage.log_pager.page_is_pending(page) {
        return;
    }
    let Some(load) = app.usage.log_pager.current_page_refresh_request() else {
        app.usage.finish_log_page_refresh_after_aggregate();
        return;
    };

    data_cache.next_usage_pricing_request_id =
        data_cache.next_usage_pricing_request_id.wrapping_add(1);
    let request_id = data_cache.next_usage_pricing_request_id;
    if !app
        .usage
        .log_pager
        .start_load_request(page, request_id, load)
    {
        return;
    }

    let Some(tx) = usage_pricing_req_tx else {
        app.usage.log_pager.fail_request(
            page,
            request_id,
            load.direction,
            "usage log worker is not running".to_string(),
        );
        app.push_toast(
            "Usage log worker is not running; keeping the previous page.".to_string(),
            ToastKind::Warning,
        );
        app.usage.finish_log_page_refresh_after_aggregate();
        return;
    };
    if let Err(err) = tx.send(UsagePricingReq::LoadLogPage {
        request_id,
        generation: data_cache.data_generation,
        app_state_epoch: data_cache.app_state_epoch,
        app_type: app.app_type.clone(),
        range: app.usage.range,
        page,
        cursor: load.cursor,
        direction: load.direction,
        limit: data::USAGE_LOG_PAGE_SIZE,
    }) {
        app.usage.log_pager.fail_request(
            page,
            request_id,
            load.direction,
            format!("failed to queue usage log page refresh: {err}"),
        );
        app.push_toast(
            format!("Failed to queue usage log page refresh: {err}"),
            ToastKind::Warning,
        );
    }
    app.usage.finish_log_page_refresh_after_aggregate();
}

fn queue_usage_log_detail_refresh(
    app: &mut App,
    data_cache: &mut UiDataByAppCache,
    usage_pricing_req_tx: Option<&mpsc::Sender<UsagePricingReq>>,
    log_rowid: i64,
) {
    data_cache.next_usage_pricing_request_id =
        data_cache.next_usage_pricing_request_id.wrapping_add(1);
    let request_id = data_cache.next_usage_pricing_request_id;
    app.usage.start_log_detail_refresh(request_id);

    let Some(tx) = usage_pricing_req_tx else {
        app.usage.finish_log_detail_refresh(request_id);
        app.push_toast(
            "Usage log worker is not running; keeping the previous details.".to_string(),
            ToastKind::Warning,
        );
        return;
    };
    if let Err(err) = tx.send(UsagePricingReq::LoadLogDetail {
        request_id,
        generation: data_cache.data_generation,
        app_state_epoch: data_cache.app_state_epoch,
        app_type: app.app_type.clone(),
        range: app.usage.range,
        log_rowid,
    }) {
        app.usage.finish_log_detail_refresh(request_id);
        app.push_toast(
            format!("Failed to queue usage log refresh: {err}"),
            ToastKind::Warning,
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionUsageSyncTerminal {
    Run,
    CodexRebuild,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManualSessionCostSyncAction {
    None,
    QueueRun,
    RequeryVisiblePage,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum ManualSessionCostSyncState {
    #[default]
    Idle,
    /// Metadata was published while another sync was already running. Its
    /// terminal cannot satisfy the post-publication import promise.
    NeedsRunAfterCurrent,
    /// The Run queued after the latest metadata publication is in flight.
    AwaitingManualRun,
}

fn manual_session_cost_sync_action(
    state: &mut ManualSessionCostSyncState,
    terminal: Option<SessionUsageSyncTerminal>,
    sessions_visible: bool,
) -> ManualSessionCostSyncAction {
    let Some(terminal) = terminal else {
        return ManualSessionCostSyncAction::None;
    };
    match (*state, terminal) {
        (ManualSessionCostSyncState::Idle, _) => ManualSessionCostSyncAction::None,
        (ManualSessionCostSyncState::NeedsRunAfterCurrent, _) => {
            *state = ManualSessionCostSyncState::AwaitingManualRun;
            ManualSessionCostSyncAction::QueueRun
        }
        (ManualSessionCostSyncState::AwaitingManualRun, SessionUsageSyncTerminal::Run) => {
            *state = ManualSessionCostSyncState::Idle;
            if sessions_visible {
                ManualSessionCostSyncAction::RequeryVisiblePage
            } else {
                ManualSessionCostSyncAction::None
            }
        }
        (ManualSessionCostSyncState::AwaitingManualRun, SessionUsageSyncTerminal::CodexRebuild) => {
            ManualSessionCostSyncAction::QueueRun
        }
    }
}

fn handle_session_usage_sync_msg(
    app: &mut App,
    data: &mut data::UiData,
    data_cache: &mut UiDataByAppCache,
    sync_tracker: &mut RequestTracker,
    usage_pricing_req_tx: Option<&mpsc::Sender<UsagePricingReq>>,
    msg: SessionUsageSyncMsg,
) -> Option<SessionUsageSyncTerminal> {
    let request_id = match &msg {
        SessionUsageSyncMsg::Finished { request_id, .. }
        | SessionUsageSyncMsg::CodexRebuilt { request_id, .. } => *request_id,
    };
    if !sync_tracker.finish_if_active(request_id) {
        return None;
    }

    let terminal = match msg {
        SessionUsageSyncMsg::Finished { result, .. } => {
            app.usage.finish_manual_session_refresh();
            if let Err(err) = result {
                log::debug!("background session usage sync failed: {err}");
            }
            SessionUsageSyncTerminal::Run
        }
        SessionUsageSyncMsg::CodexRebuilt { result, .. } => {
            app.usage.finish_codex_usage_rebuild();
            match result {
                Ok(result) => {
                    let errors = result.errors.len();
                    let kind = if errors > 0 || result.deferred_files > 0 {
                        ToastKind::Warning
                    } else {
                        ToastKind::Success
                    };
                    app.push_toast(
                        texts::tui_toast_codex_usage_rebuilt(
                            result.imported,
                            errors,
                            result.suspected_duplicates,
                            result.deferred_files,
                        ),
                        kind,
                    );
                }
                Err(error) => app.push_toast(
                    texts::tui_toast_codex_usage_rebuild_failed(&error),
                    ToastKind::Error,
                ),
            }
            SessionUsageSyncTerminal::CodexRebuild
        }
    };

    // A best-effort import may commit records before another source reports an
    // error. Treat every completed cycle as a Usage invalidation boundary; the
    // follow-up query is also useful for proxy rows written during the sync.
    data_cache.invalidate_usage_pricing_cache_after_external_usage_sync();

    if usage_pricing_req_tx.is_none() {
        log::debug!("background session usage sync finished; usage/pricing worker unavailable");
        return Some(terminal);
    }

    // Completed aggregates are now stale, but their request tokens remain
    // valid. Keep the visible snapshot in place and arrange one final query
    // after any aggregate already in flight has completed.
    let current_app_type = app.app_type.clone();
    // Refresh the projection the current route consumes. This matters when the
    // Usage page was left on a custom range: Main and Pricing still consume
    // fixed data and must not queue that potentially large custom query ahead
    // of their visible projection. Routes without a Usage/Pricing projection
    // retain the selected range.
    let visible_range = app
        .usage_pricing_load_range_for_current_route()
        .unwrap_or(app.usage.range);
    data_cache.mark_usage_pricing_dirty(&current_app_type, visible_range);
    let aggregate_queued = data_cache.flush_dirty_usage_pricing(app, usage_pricing_req_tx);
    let aggregate_will_complete =
        aggregate_queued || !data_cache.pending_usage_pricing_by_key.is_empty();
    if matches!(app.route, route::Route::UsageLogs)
        && matches!(app.usage.pane, app::UsagePane::Recent)
        && app.usage.log_pager.current_page() > 0
        && aggregate_will_complete
    {
        app.usage.request_log_page_refresh_after_aggregate();
    }
    data_cache.remember_current(&app.app_type, data);
    Some(terminal)
}

fn handle_app_data_msg(
    app: &mut App,
    data: &mut data::UiData,
    data_cache: &mut UiDataByAppCache,
    quota_req_tx: Option<&mpsc::Sender<QuotaReq>>,
    app_data_req_tx: Option<&mpsc::Sender<AppDataReq>>,
    usage_pricing_req_tx: Option<&mpsc::Sender<UsagePricingReq>>,
    msg: AppDataMsg,
) {
    match msg {
        AppDataMsg::Loaded {
            kind,
            request_id,
            generation,
            app_state_epoch,
            app_type,
            result,
        } => {
            match data_cache.finish_app_data_load(
                kind,
                &app_type,
                request_id,
                generation,
                app_state_epoch,
            ) {
                AppDataLoadFinish::Accepted => {}
                AppDataLoadFinish::Stale => {
                    if app.app_type == app_type {
                        let _ =
                            data_cache.queue_current_app_data_refresh(app_data_req_tx, &app_type);
                    }
                    return;
                }
                AppDataLoadFinish::Ignored => return,
            }

            match result {
                Ok(mut loaded) => {
                    if matches!(kind, AppDataLoadKind::Full) {
                        data_cache.remove_usage_pricing_for_app(&app_type);
                        data_cache.mark_app_data_loaded(&app_type);
                        if app.app_type == app_type {
                            app.usage.invalidate_log_pages();
                            *data = loaded;
                            // A full reload of the CURRENT app must not wipe the
                            // pre-seeded snapshots of the other apps (their DB rows
                            // weren't touched), so scope the invalidation to the
                            // current app instead of a global clear.
                            if let Err(err) = apply_loaded_data_cache_invalidation(
                                app,
                                data,
                                data_cache,
                                quota_req_tx,
                                usage_pricing_req_tx,
                                CacheInvalidation::CurrentAppReloaded,
                            ) {
                                app.push_toast(err.to_string(), ToastKind::Warning);
                            }
                        } else {
                            data_cache.by_app.insert(app_type, loaded);
                        }
                        return;
                    }

                    let active_range = if app.app_type == app_type {
                        app.usage.range
                    } else {
                        data::UsageRangePreset::SevenDays
                    };
                    data_cache.merge_usage_pricing(&app_type, &mut loaded, active_range);
                    data_cache.mark_app_data_loaded(&app_type);
                    if app.app_type == app_type {
                        loaded.quota = data.quota.clone();
                        app.usage.invalidate_log_pages();
                        *data = loaded;
                        app.reset_proxy_activity(
                            data.proxy.estimated_input_tokens_total,
                            data.proxy.estimated_output_tokens_total,
                        );
                        app.clamp_selections(data);
                        app.maybe_prompt_import_candidate(data);
                        data_cache.remember_current(&app.app_type, data);
                        if matches!(kind, AppDataLoadKind::Snapshot) {
                            queue_current_quota_refresh_if_due(app, data, quota_req_tx);
                        }
                    } else {
                        data_cache.by_app.insert(app_type, loaded);
                    }
                }
                Err(err) => {
                    if app.app_type == app_type {
                        if matches!(kind, AppDataLoadKind::Full) {
                            app.usage.clear_loading();
                        }
                        app.push_toast(
                            format!("App data refresh failed: {err}"),
                            ToastKind::Warning,
                        );
                    }
                }
            }
        }
    }
}

fn handle_initial_app_data_msg(
    app: &mut App,
    data: &mut data::UiData,
    data_cache: &mut UiDataByAppCache,
    startup_overlay: &mut Option<Overlay>,
    quota_req_tx: Option<&mpsc::Sender<QuotaReq>>,
    msg: AppDataMsg,
) -> Result<bool, AppError> {
    match msg {
        AppDataMsg::Loaded {
            kind,
            request_id,
            generation,
            app_state_epoch,
            app_type,
            result,
        } => {
            if !matches!(kind, AppDataLoadKind::Initial) {
                return Ok(false);
            }
            match data_cache.finish_app_data_load(
                kind,
                &app_type,
                request_id,
                generation,
                app_state_epoch,
            ) {
                AppDataLoadFinish::Accepted => {}
                AppDataLoadFinish::Stale | AppDataLoadFinish::Ignored => return Ok(false),
            }

            if app.app_type == app_type {
                // The active app must load for the UI to be usable, so a failure
                // here still aborts startup and leaves it "incomplete" (unchanged
                // behavior: mark_app_data_loaded only runs after a successful load).
                let mut loaded = result.map_err(AppError::Message)?;
                data_cache.mark_app_data_loaded(&app_type);
                loaded.quota = data.quota.clone();
                app.usage.invalidate_log_pages();
                *data = loaded;
                app.overlay = startup_overlay.take().unwrap_or(Overlay::None);
                app.reset_proxy_activity(
                    data.proxy.estimated_input_tokens_total,
                    data.proxy.estimated_output_tokens_total,
                );
                app.observe_proxy_visual_state(data);
                app.clamp_selections(data);
                app.maybe_prompt_import_candidate(data);
                data_cache.remember_current(&app.app_type, data);
                queue_current_quota_refresh_if_due(app, data, quota_req_tx);
            } else {
                // A pre-seeded extra app only warms the cache. Mark it done either
                // way; on failure skip the insert so the first switch falls back to
                // a lazy load instead of aborting startup over a non-active app.
                data_cache.mark_app_data_loaded(&app_type);
                if let Ok(loaded) = result {
                    data_cache.by_app.insert(app_type, loaded);
                }
            }
            Ok(true)
        }
    }
}

fn drain_initial_app_data_messages(
    app: &mut App,
    data: &mut data::UiData,
    data_cache: &mut UiDataByAppCache,
    startup_overlay: &mut Option<Overlay>,
    quota_req_tx: Option<&mpsc::Sender<QuotaReq>>,
    result_rx: &mpsc::Receiver<AppDataMsg>,
) -> Result<bool, AppError> {
    let mut loaded = false;
    while let Ok(msg) = result_rx.try_recv() {
        if handle_initial_app_data_msg(app, data, data_cache, startup_overlay, quota_req_tx, msg)? {
            loaded = true;
        }
    }
    Ok(loaded)
}

fn cache_invalidation_for_action(action: &Action) -> CacheInvalidation {
    match action {
        Action::None
        | Action::SwitchRoute(_)
        | Action::Quit
        | Action::SetAppType(_)
        | Action::LocalEnvRefresh
        | Action::CopyToClipboard { .. }
        | Action::SessionsRefresh
        | Action::SessionsDeepSearch { .. }
        | Action::SessionsDeepSearchCancel
        | Action::SessionsProjectCatalogLoad
        | Action::SessionsProjectFilter { .. }
        | Action::SessionsProjectFilterCancel
        | Action::SessionsProjectApply { .. }
        | Action::SessionMessagesLoad { .. }
        | Action::SessionResume { .. }
        | Action::SessionDelete { .. }
        | Action::ProviderSpeedtest { .. }
        | Action::ProviderLaunchTemporary { .. }
        | Action::ProviderStreamCheck { .. }
        | Action::ProviderQuotaRefresh { .. }
        | Action::ProviderModelFetch { .. }
        | Action::UsageCustomRange { .. }
        | Action::UsageRefresh
        | Action::UsageRebuildCodex
        | Action::UsageLogDetailRefresh { .. }
        | Action::ManagedAuthRefresh { .. }
        | Action::ManagedAuthStartLogin { .. }
        | Action::ManagedAuthSetDefault { .. }
        | Action::ManagedAuthRemove { .. }
        | Action::SkillsInstall { .. }
        | Action::SkillsDiscover { .. }
        | Action::SkillsCheckUpdates
        | Action::SkillsUpdate { .. }
        | Action::SkillsOpenImport
        | Action::SkillsScanUnmanaged
        | Action::EditorDiscard
        | Action::EditorOpenExternal
        | Action::EditorFormatCommonSnippet { .. }
        | Action::EditorExtractCommonSnippet { .. }
        | Action::PromptFormOpenExternal
        | Action::PromptOpenImportCandidate { .. }
        | Action::ConfigExport { .. }
        | Action::ConfigShowFull
        | Action::ConfigValidate
        | Action::ConfigOpenProxyHelp
        | Action::ConfirmCommonConfigNotice
        | Action::ConfirmUsageQueryNotice
        | Action::ConfigWebDavCheckConnection
        | Action::ConfigWebDavUpload
        | Action::ConfigWebDavJianguoyunQuickSetup { .. }
        | Action::ConfigS3CheckConnection
        | Action::ConfigS3FetchRemoteInfo { .. }
        | Action::ConfigS3Upload
        | Action::OpenClawWorkspaceOpenFile { .. }
        | Action::OpenClawDailyMemoryOpenFile { .. }
        | Action::OpenClawDailyMemorySearch { .. }
        | Action::OpenClawOpenDirectory { .. }
        | Action::HermesMemoryOpen { .. }
        | Action::SetSkipClaudeOnboarding { .. }
        | Action::SetClaudePluginIntegration { .. }
        | Action::SetPreserveCodexOfficialAuth { .. }
        | Action::SetCodexUnifiedSessionHistory { .. }
        | Action::SetPreferredEditor { .. }
        | Action::SetManagedProxyForCurrentApp { .. }
        | Action::SetLanguage(_)
        | Action::CheckUpdate
        | Action::ConfirmUpdate
        | Action::CancelUpdate
        | Action::CancelUpdateCheck => CacheInvalidation::None,

        Action::ConfigImport { .. }
        | Action::ConfigRestoreBackup { .. }
        | Action::ConfigReset
        | Action::ConfigWebDavDownload
        | Action::ConfigWebDavMigrateV1ToV2
        | Action::ConfigS3Download => CacheInvalidation::AppStateRecreated,

        Action::ProviderSwitch { .. }
        | Action::ProviderRemoveFromConfig { .. }
        | Action::ProviderSetDefaultModel { .. }
        | Action::ProviderImportLiveConfig
        | Action::ProviderDelete { .. }
        | Action::ProviderSetFailoverQueue { .. }
        | Action::ProviderMoveFailoverQueue { .. }
        | Action::EditorSubmit {
            submit: EditorSubmit::ProviderAdd | EditorSubmit::ProviderEdit { .. },
            ..
        } => CacheInvalidation::CurrentAppDataChanged,

        Action::ReloadData
        | Action::SetVisibleAppsMode { .. }
        | Action::SetVisibleApps { .. }
        | Action::ConfirmVisibleAppsAutoDetection { .. }
        | Action::SwitchVisibleAppsToManual { .. }
        | Action::SkillsToggle { .. }
        | Action::SkillsSetApps { .. }
        | Action::SkillsUninstall { .. }
        | Action::SkillsSync { .. }
        | Action::SkillsSetSyncMethod { .. }
        | Action::SkillsRepoAdd { .. }
        | Action::SkillsRepoRemove { .. }
        | Action::SkillsRepoToggleEnabled { .. }
        | Action::SkillsImportFromApps { .. }
        | Action::PricingDelete { .. }
        | Action::McpToggle { .. }
        | Action::McpSetApps { .. }
        | Action::McpDelete { .. }
        | Action::McpImport
        | Action::PromptActivate { .. }
        | Action::PromptDeactivate { .. }
        | Action::PromptUpdateMetadata { .. }
        | Action::PromptSave { .. }
        | Action::PromptDelete { .. }
        | Action::ConfigBackup { .. }
        | Action::ConfigWebDavReset
        | Action::ConfigWebDavSave { .. }
        | Action::ConfigWebDavSetEnabled { .. }
        | Action::ConfigS3Save { .. }
        | Action::ConfigS3SetEnabled { .. }
        | Action::ConfigS3Reset
        | Action::OpenClawDailyMemoryDelete { .. }
        | Action::HermesMemorySetEnabled { .. }
        | Action::HermesOpenMemoryDirectory
        | Action::EditorSubmit { .. }
        | Action::SetProxyEnabled { .. }
        | Action::SetProxyListenAddress { .. }
        | Action::SetProxyListenPort { .. }
        | Action::SetProxyAutoFailover { .. }
        | Action::EnableProxyAndAutoFailover { .. }
        | Action::SetOpenClawConfigDir { .. } => CacheInvalidation::DataReloaded,
    }
}

fn effective_cache_invalidation(
    candidate: CacheInvalidation,
    before_token: data::UiDataReloadToken,
    data: &data::UiData,
) -> CacheInvalidation {
    if matches!(candidate, CacheInvalidation::None) || data.reload_token == before_token {
        CacheInvalidation::None
    } else {
        candidate
    }
}

fn drop_cached_worker_state(
    app_data_req_tx: Option<&mpsc::Sender<AppDataReq>>,
    usage_pricing_req_tx: Option<&mpsc::Sender<UsagePricingReq>>,
) -> Result<(), AppError> {
    let mut acks = Vec::new();

    if let Some(tx) = app_data_req_tx {
        let (ack_tx, ack_rx) = mpsc::channel();
        if tx.send(AppDataReq::DropState { ack: ack_tx }).is_ok() {
            acks.push(("app data", ack_rx));
        }
    }

    if let Some(tx) = usage_pricing_req_tx {
        let (ack_tx, ack_rx) = mpsc::channel();
        if tx.send(UsagePricingReq::DropState { ack: ack_tx }).is_ok() {
            acks.push(("usage/pricing", ack_rx));
        }
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    for (name, ack_rx) in acks {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match ack_rx.recv_timeout(remaining) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err(AppError::Message(format!(
                    "timed out waiting for {name} worker to release cached app state"
                )));
            }
        }
    }

    Ok(())
}

fn apply_current_app_data_changed(
    app: &mut App,
    data: &mut data::UiData,
    data_cache: &mut UiDataByAppCache,
    quota_req_tx: Option<&mpsc::Sender<QuotaReq>>,
    app_data_req_tx: Option<&mpsc::Sender<AppDataReq>>,
    usage_pricing_req_tx: Option<&mpsc::Sender<UsagePricingReq>>,
) -> Result<(), AppError> {
    let app_type = app.app_type.clone();
    data_cache.remove_app_snapshot(&app_type);
    data_cache.remove_usage_pricing_for_app(&app_type);

    match data_cache.queue_current_app_data_refresh(app_data_req_tx, &app_type) {
        AppDataLoadQueued::Queued | AppDataLoadQueued::AlreadyPending => Ok(()),
        AppDataLoadQueued::Unavailable | AppDataLoadQueued::SendFailed => {
            *data = data::UiData::load(&app_type)?;
            data_cache.mark_app_data_loaded(&app_type);
            apply_loaded_data_cache_invalidation(
                app,
                data,
                data_cache,
                quota_req_tx,
                usage_pricing_req_tx,
                CacheInvalidation::CurrentAppReloaded,
            )
        }
    }
}

fn apply_cache_invalidation(
    app: &mut App,
    data: &mut data::UiData,
    data_cache: &mut UiDataByAppCache,
    quota_req_tx: Option<&mpsc::Sender<QuotaReq>>,
    app_data_req_tx: Option<&mpsc::Sender<AppDataReq>>,
    usage_pricing_req_tx: Option<&mpsc::Sender<UsagePricingReq>>,
    invalidation: CacheInvalidation,
) -> Result<(), AppError> {
    if matches!(invalidation, CacheInvalidation::AppStateRecreated) {
        drop_cached_worker_state(app_data_req_tx, usage_pricing_req_tx)?;
    }

    if matches!(invalidation, CacheInvalidation::CurrentAppDataChanged) {
        return apply_current_app_data_changed(
            app,
            data,
            data_cache,
            quota_req_tx,
            app_data_req_tx,
            usage_pricing_req_tx,
        );
    }

    apply_loaded_data_cache_invalidation(
        app,
        data,
        data_cache,
        quota_req_tx,
        usage_pricing_req_tx,
        invalidation,
    )
}

fn apply_loaded_data_cache_invalidation(
    app: &mut App,
    data: &mut data::UiData,
    data_cache: &mut UiDataByAppCache,
    quota_req_tx: Option<&mpsc::Sender<QuotaReq>>,
    usage_pricing_req_tx: Option<&mpsc::Sender<UsagePricingReq>>,
    invalidation: CacheInvalidation,
) -> Result<(), AppError> {
    let selected_custom_range = if matches!(invalidation, CacheInvalidation::None) {
        None
    } else if let data::UsageRangePreset::Custom(range) = app.usage.range {
        data.usage.begin_custom_range(range);
        Some(range)
    } else {
        None
    };

    if !matches!(invalidation, CacheInvalidation::None) {
        app.usage.invalidate_log_pages();
        app.clamp_selections(data);
        app.usage.clear_loading();
    }

    data_cache.handle_data_reloaded(app, data, invalidation);
    if !matches!(invalidation, CacheInvalidation::None) {
        queue_current_quota_refresh_if_due(app, data, quota_req_tx);
        let refresh_range = app
            .usage_pricing_load_range_for_current_route()
            .or_else(|| selected_custom_range.map(data::UsageRangePreset::Custom));
        if let Some(range) = refresh_range {
            let current_app_type = app.app_type.clone();
            data_cache.queue_usage_pricing_load(
                app,
                usage_pricing_req_tx,
                &current_app_type,
                range,
            );
            data_cache.remember_current(&app.app_type, data);
        }
    }

    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "top-level TUI dispatcher coordinates worker channels, cache, and trackers"
)]
fn handle_tui_action(
    terminal: &mut TuiTerminal,
    app: &mut App,
    data: &mut data::UiData,
    data_cache: &mut UiDataByAppCache,
    app_data_req_tx: Option<&mpsc::Sender<AppDataReq>>,
    speedtest_req_tx: Option<&mpsc::Sender<String>>,
    stream_check_req_tx: Option<&mpsc::Sender<StreamCheckReq>>,
    skills_req_tx: Option<&mpsc::Sender<SkillsReq>>,
    proxy_req_tx: Option<&mpsc::Sender<ProxyReq>>,
    proxy_loading: &mut RequestTracker,
    local_env_req_tx: Option<&tokio::sync::mpsc::UnboundedSender<LocalEnvReq>>,
    session_req_tx: Option<&mpsc::Sender<SessionReq>>,
    webdav_req_tx: Option<&mpsc::Sender<WebDavReq>>,
    webdav_loading: &mut RequestTracker,
    update_req_tx: Option<&mpsc::Sender<UpdateReq>>,
    update_check: &mut RequestTracker,
    model_fetch_req_tx: Option<&mpsc::Sender<ModelFetchReq>>,
    managed_auth_req_tx: Option<&mpsc::Sender<ManagedAuthReq>>,
    quota_req_tx: Option<&mpsc::Sender<QuotaReq>>,
    usage_pricing_req_tx: Option<&mpsc::Sender<UsagePricingReq>>,
    session_usage_sync: Option<(&mpsc::Sender<SessionUsageSyncReq>, &mut RequestTracker)>,
    action: Action,
) -> Result<(), AppError> {
    if app.sessions.take_message_cancel_pending() {
        if let Some(tx) = session_req_tx {
            let _ = tx.send(SessionReq::CancelMessages);
        }
    }
    let should_queue_sessions_refresh = matches!(
        &action,
        Action::SwitchRoute(route::Route::Sessions)
            | Action::SetAppType(_)
            | Action::SessionsDeepSearchCancel
    );
    let result = match action {
        Action::None => Ok(()),
        Action::ProviderQuotaRefresh { id } => {
            queue_provider_quota_refresh(app, data, quota_req_tx, &id);
            Ok(())
        }
        Action::SetAppType(next) => {
            if let Some(tx) = session_req_tx {
                let _ = tx.send(SessionReq::CancelSearch);
                let _ = tx.send(SessionReq::CancelMessages);
                let _ = tx.send(SessionReq::CancelProjectCatalog);
                let _ = tx.send(SessionReq::CancelProjectFilter);
            }
            app.sessions.clear_detail();
            let _ = app.sessions.take_message_cancel_pending();
            app.sessions.deep_search_active = None;
            app.sessions.deep_search_pending = None;
            app.pending_deep_search = None;
            app.pending_project_catalog = false;
            app.pending_project_filter = None;
            app.sessions.cancel_project_catalog();
            app.sessions.cancel_project_filter();
            app.sessions.deep_search_query = None;
            app.sessions.clear_deep_search_results();
            data_cache.switch_to(app, data, app_data_req_tx, next)?;
            if matches!(app.route, route::Route::Sessions) {
                let query = app.filter.input.value.trim();
                if !query.is_empty() {
                    app.sessions.deep_search_pending = Some((query.to_lowercase(), 0));
                }
            }
            let current_app_type = app.app_type.clone();
            let preload_range = app
                .usage_pricing_load_range_for_current_route()
                .unwrap_or(data::UsageRangePreset::SevenDays);
            data_cache.queue_usage_pricing_load(
                app,
                usage_pricing_req_tx,
                &current_app_type,
                preload_range,
            );
            queue_current_quota_refresh_if_due(app, data, quota_req_tx);
            Ok(())
        }
        Action::UsageCustomRange { range } => {
            app.usage.invalidate_log_pages();
            app.usage.range = data::UsageRangePreset::Custom(range);
            data.usage.begin_custom_range(range);
            app.clamp_selections(data);
            let current_app_type = app.app_type.clone();
            data_cache.queue_usage_pricing_load(
                app,
                usage_pricing_req_tx,
                &current_app_type,
                data::UsageRangePreset::Custom(range),
            );
            data_cache.remember_current(&app.app_type, data);
            Ok(())
        }
        Action::UsageRefresh => {
            let current_app_type = app.app_type.clone();
            let is_usage_route = matches!(
                app.route,
                route::Route::Usage | route::Route::UsageLogs | route::Route::UsageLogDetail { .. }
            );
            let range = app
                .usage_pricing_load_range_for_current_route()
                .unwrap_or(app.usage.range);

            // Match upstream's manual flow: import session logs first, then
            // invalidate/requery Usage. Repeated `r` shares the active import;
            // if either worker is unavailable, fall back to the async DB query.
            let sync_queued = is_usage_route
                && usage_pricing_req_tx.is_some()
                && session_usage_sync
                    .map(|(tx, tracker)| queue_background_session_usage_sync(Some(tx), tracker))
                    .unwrap_or(false);
            if sync_queued {
                app.usage.start_manual_session_refresh();
            } else {
                data_cache.queue_usage_pricing_load(
                    app,
                    usage_pricing_req_tx,
                    &current_app_type,
                    range,
                );
                if matches!(app.route, route::Route::UsageLogs)
                    && matches!(app.usage.pane, app::UsagePane::Recent)
                    && app.usage.log_pager.current_page() > 0
                {
                    app.usage.request_log_page_refresh_after_aggregate();
                }
            }
            Ok(())
        }
        Action::UsageRebuildCodex => {
            let (sync_req_tx, sync_tracker) = match session_usage_sync {
                Some((tx, tracker)) => (Some(tx), tracker),
                None => {
                    app.push_toast(
                        texts::tui_toast_codex_usage_rebuild_unavailable().to_string(),
                        ToastKind::Error,
                    );
                    return Ok(());
                }
            };
            let _ = queue_codex_usage_rebuild(app, sync_req_tx, sync_tracker);
            Ok(())
        }
        Action::UsageLogDetailRefresh { rowid } => {
            queue_usage_log_detail_refresh(app, data_cache, usage_pricing_req_tx, rowid);
            Ok(())
        }
        Action::SetCodexUnifiedSessionHistory { .. } => Err(AppError::Message(
            "Codex history settings must be dispatched through the background worker".to_string(),
        )),
        other => {
            let candidate = cache_invalidation_for_action(&other);
            if matches!(candidate, CacheInvalidation::AppStateRecreated) {
                drop_cached_worker_state(app_data_req_tx, usage_pricing_req_tx)?;
            }
            let before_token = data.reload_token;
            handle_action(
                terminal,
                app,
                data,
                speedtest_req_tx,
                stream_check_req_tx,
                skills_req_tx,
                proxy_req_tx,
                proxy_loading,
                local_env_req_tx,
                session_req_tx,
                webdav_req_tx,
                webdav_loading,
                update_req_tx,
                update_check,
                model_fetch_req_tx,
                managed_auth_req_tx,
                other,
            )?;
            let invalidation = effective_cache_invalidation(candidate, before_token, data);
            apply_cache_invalidation(
                app,
                data,
                data_cache,
                quota_req_tx,
                app_data_req_tx,
                usage_pricing_req_tx,
                invalidation,
            )
        }
    };

    if result.is_ok() && should_queue_sessions_refresh {
        queue_sessions_refresh_if_needed(app, session_req_tx);
    }

    result
}

fn queue_codex_history_action(
    app: &mut App,
    req_tx: Option<&mpsc::Sender<CodexHistoryReq>>,
    tracker: &mut RequestTracker,
    action: Action,
) -> Action {
    let Action::SetCodexUnifiedSessionHistory {
        enabled,
        migrate_existing,
        restore_after_disable,
    } = action
    else {
        return action;
    };

    if tracker.active.is_some() {
        app.push_toast(
            texts::tui_toast_codex_history_change_in_progress(),
            ToastKind::Info,
        );
        return Action::None;
    }

    let Some(tx) = req_tx else {
        app.push_toast(
            texts::tui_toast_codex_history_worker_unavailable("worker is not running"),
            ToastKind::Warning,
        );
        return Action::None;
    };

    let request_id = tracker.start();
    if let Err(err) = tx.send(CodexHistoryReq {
        request_id,
        enabled,
        migrate_existing,
        restore_after_disable,
    }) {
        tracker.cancel();
        app.push_toast(
            texts::tui_toast_codex_history_request_failed(&err.to_string()),
            ToastKind::Error,
        );
    }

    Action::None
}

fn queue_sessions_refresh_if_needed(
    app: &mut App,
    session_req_tx: Option<&mpsc::Sender<runtime_systems::SessionReq>>,
) {
    if !matches!(app.route, route::Route::Sessions) {
        return;
    }
    let provider_id = app.app_type.as_str().to_string();
    let query = app.filter.query_lower().unwrap_or_default();
    let desired = app.sessions.desired_view_spec(Some(&query));

    let purge_refresh = app.sessions.purge_refresh_required();
    // Desired views are rebuilt from the saved authoritative base before the
    // scan-attempt guard is considered. That guard prevents failed-scan retry
    // storms; it must not block recovery or a debounced project/query change.
    let current_base_available = app.sessions.provider_id.as_deref() == Some(provider_id.as_str())
        && app.sessions.base_query_source().is_some();
    if !purge_refresh && current_base_available {
        if desired.is_base_view() {
            if !app.sessions.base_view_is_current() {
                app.sessions.ensure_base_restore_staged();
            }
        } else if !app.sessions.materialized_view_is_current(Some(&query))
            && !app
                .sessions
                .materialization_failed_for_current_base(&desired)
            && app.sessions.deep_search_active.is_none()
            && app.sessions.deep_search_pending.is_none()
            && app.pending_deep_search.is_none()
        {
            app.pending_deep_search = Some(query);
        }
        return;
    }
    if !purge_refresh && app.sessions.loaded_for_provider(&provider_id) {
        return;
    }
    if (app.sessions.provider_id.as_deref() == Some(provider_id.as_str())
        && app.sessions.scan_active.is_some())
        || (!purge_refresh && app.sessions.scan_attempted_for_scope(&provider_id))
    {
        return;
    }

    let Some(tx) = session_req_tx else {
        let request_id = app.sessions.start_scan(provider_id);
        if purge_refresh {
            app.sessions.consume_purge_refresh();
        }
        app.sessions
            .fail_scan(request_id, "sessions worker is not running".to_string());
        app.push_toast(
            texts::tui_sessions_toast_worker_unavailable("sessions worker is not running"),
            ToastKind::Warning,
        );
        return;
    };

    let request_id = app.sessions.start_scan(provider_id.clone());
    if purge_refresh {
        app.sessions.consume_purge_refresh();
    }
    if let Err(err) = tx.send(runtime_systems::SessionReq::Refresh {
        request_id,
        scope_epoch: app.sessions.scope_epoch,
        provider_id,
        // Entering the page opens the persisted manifest only. A missing
        // manifest bootstraps once; source revalidation is otherwise manual.
        force: purge_refresh,
    }) {
        app.sessions.fail_scan(request_id, err.to_string());
        app.push_toast(
            texts::tui_sessions_toast_refresh_failed(&err.to_string()),
            ToastKind::Warning,
        );
    }
}

fn maybe_queue_session_page_load(
    app: &mut App,
    session_req_tx: Option<&mpsc::Sender<runtime_systems::SessionReq>>,
) {
    if !matches!(app.route, route::Route::Sessions) {
        return;
    }
    let Some(page) = app.sessions.remote.preferred_load_page() else {
        return;
    };
    let Some((request_id, token, reader)) = app.sessions.next_page_request(page) else {
        return;
    };
    let Some(tx) = session_req_tx else {
        app.sessions.fail_page_request(
            request_id,
            &token,
            page,
            "sessions worker is not running".to_string(),
        );
        return;
    };
    if let Err(error) = tx.send(runtime_systems::SessionReq::LoadPage {
        request_id,
        token: token.clone(),
        page,
        reader,
    }) {
        app.sessions
            .fail_page_request(request_id, &token, page, error.to_string());
    }
}

fn maybe_queue_session_message_page_load(
    app: &mut App,
    session_req_tx: Option<&mpsc::Sender<runtime_systems::SessionReq>>,
) {
    if !matches!(app.route, route::Route::Sessions)
        || app.sessions.detail_key.is_none()
        || !app.sessions.messages_loaded
    {
        return;
    }
    if runtime_actions::queue_pending_session_message_refresh(app, session_req_tx) {
        return;
    }
    let Some(page) = app.sessions.message_remote.preferred_load_page() else {
        return;
    };
    let Some(key) = app.sessions.messages_key.clone() else {
        return;
    };
    let Some((request_id, transcript_generation, refresh_page, refresh_message_key, reader)) =
        app.sessions.next_message_page_request(page)
    else {
        return;
    };
    let Some(tx) = session_req_tx else {
        app.sessions.fail_message_page_request(
            request_id,
            &key,
            &transcript_generation,
            page,
            "sessions worker is not running".to_string(),
        );
        return;
    };
    if let Err(error) = tx.send(runtime_systems::SessionReq::LoadMessagePage {
        request_id,
        key: key.clone(),
        transcript_generation: transcript_generation.clone(),
        page,
        refresh_page,
        refresh_message_key,
        reader,
    }) {
        app.sessions.fail_message_page_request(
            request_id,
            &key,
            &transcript_generation,
            page,
            error.to_string(),
        );
    }
}

fn maybe_queue_session_manifest_reconcile(
    app: &mut App,
    session_req_tx: Option<&mpsc::Sender<runtime_systems::SessionReq>>,
) {
    let Some((request_id, source, scope, generation, fallback_absolute, anchor, reader)) =
        app.sessions.next_manifest_reconcile()
    else {
        return;
    };
    let scope_epoch = app.sessions.scope_epoch;
    let Some(tx) = session_req_tx else {
        app.sessions.fail_manifest_reconcile(
            request_id,
            scope_epoch,
            &generation,
            "sessions worker is not running".to_string(),
        );
        return;
    };
    if let Err(error) = tx.send(runtime_systems::SessionReq::LocateManifest {
        request_id,
        scope_epoch,
        source,
        scope,
        generation: generation.clone(),
        fallback_absolute,
        anchor,
        reader,
    }) {
        app.sessions.fail_manifest_reconcile(
            request_id,
            scope_epoch,
            &generation,
            error.to_string(),
        );
    }
}

fn initialize_loaded_app(
    app: &mut App,
    data: &mut data::UiData,
    data_cache: &mut UiDataByAppCache,
    quota_req_tx: Option<&mpsc::Sender<QuotaReq>>,
) {
    app.reset_proxy_activity(
        data.proxy.estimated_input_tokens_total,
        data.proxy.estimated_output_tokens_total,
    );
    app.observe_proxy_visual_state(data);
    app.clamp_selections(data);
    app.maybe_prompt_import_candidate(data);
    data_cache.remember_current(&app.app_type, data);
    queue_current_quota_refresh_if_due(app, data, quota_req_tx);
}

fn queue_local_env_refresh_if_available(
    app: &mut App,
    local_env_req_tx: Option<&tokio::sync::mpsc::UnboundedSender<LocalEnvReq>>,
) {
    let Some(tx) = local_env_req_tx else {
        app.stop_local_env_refresh();
        return;
    };
    let generation = app.begin_local_env_refresh();
    if let Err(err) = tx.send(LocalEnvReq::Refresh { generation }) {
        app.fail_local_env_refresh(generation);
        app.push_toast(
            texts::tui_toast_local_env_check_request_failed(&err.to_string()),
            ToastKind::Warning,
        );
    }
}

fn is_initial_loading_quit_key(key: &KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => true,
        KeyCode::Char('c') => key.modifiers.contains(KeyModifiers::CONTROL),
        _ => false,
    }
}

fn initial_loading_event_requests_quit(event: &event::Event) -> bool {
    match event {
        event::Event::Key(key) if key.kind == KeyEventKind::Press => {
            let key = normalize_key_event(*key);
            is_initial_loading_quit_key(&key)
        }
        _ => false,
    }
}

fn record_initial_loading_quit_event(quit_requested: &mut bool, event: &event::Event) {
    if initial_loading_event_requests_quit(event) {
        *quit_requested = true;
    }
}

fn drain_initial_loading_queued_events() -> Result<bool, AppError> {
    const MAX_EVENTS: usize = 256;
    const TIME_BUDGET: Duration = Duration::from_millis(4);

    let mut quit_requested = false;
    let started = Instant::now();
    let mut drained = 0usize;
    while drained < MAX_EVENTS
        && started.elapsed() < TIME_BUDGET
        && event::poll(Duration::ZERO).map_err(|e| AppError::Message(e.to_string()))?
    {
        let event = event::read().map_err(|e| AppError::Message(e.to_string()))?;
        record_initial_loading_quit_event(&mut quit_requested, &event);
        drained = drained.saturating_add(1);
    }
    Ok(quit_requested)
}

fn should_poll_initial_loading_input(
    initial_data_loading: bool,
    has_initial_data_error: bool,
) -> bool {
    initial_data_loading && !has_initial_data_error
}

fn should_exit_after_initial_loading(
    initial_data_loading: bool,
    has_initial_data_error: bool,
    quit_requested: bool,
) -> bool {
    !initial_data_loading && !has_initial_data_error && quit_requested
}

/// Apps whose startup snapshot (SnapshotOnly `UiData`) is a pure in-memory read
/// of the already-loaded `MultiAppConfig`, cheap enough to eagerly pre-seed at
/// startup. Additive apps (OpenCode/Hermes/OpenClaw) read live config files even
/// in SnapshotOnly mode, so they are excluded and lazy-load on first switch.
fn is_lightweight_preseed_app(app_type: &AppType) -> bool {
    matches!(app_type, AppType::Claude | AppType::Codex | AppType::Gemini)
}

pub fn run(app_override: Option<AppType>) -> Result<(), AppError> {
    let _panic_hook = PanicRestoreHookGuard::install();
    let mut terminal = TuiTerminal::new()?;
    let (mut app, mut data) =
        initialize_app_shell_with(app_override, apply_visible_apps_startup_policy)?;
    let mut startup_overlay = (!matches!(app.overlay, Overlay::None)).then(|| app.overlay.clone());

    let tick_rate = TUI_TICK_RATE;
    let mut last_tick = Instant::now();
    let mut last_frame = Instant::now();
    let mut frame_scheduler = FrameScheduler::new();
    let mut proxy_open_flash = ProxyOpenFlash::default();
    let mut proxy_loading = RequestTracker::default();
    let mut proxy_snapshot_refresh = RequestTracker::default();
    let mut webdav_loading = RequestTracker::default();
    let mut update_check = RequestTracker::default();
    let mut session_usage_sync = RequestTracker::default();
    let mut manual_session_cost_sync = ManualSessionCostSyncState::Idle;
    let mut codex_history_save = RequestTracker::default();
    // Session usage sync scans every session log file, which is expensive with a
    // large history. It only feeds the Usage view, so defer it until the user
    // first opens a Usage route instead of paying it on every startup.
    let mut session_usage_sync_started = false;

    let speedtest = match start_speedtest_system() {
        Ok(system) => Some(system),
        Err(err) => {
            app.push_toast(
                texts::tui_toast_speedtest_unavailable(&err.to_string()),
                ToastKind::Warning,
            );
            None
        }
    };

    let stream_check = match start_stream_check_system() {
        Ok(system) => Some(system),
        Err(err) => {
            app.push_toast(
                texts::tui_toast_stream_check_unavailable(&err.to_string()),
                ToastKind::Warning,
            );
            None
        }
    };

    let skills = match start_skills_system() {
        Ok(system) => Some(system),
        Err(err) => {
            app.push_toast(
                texts::tui_toast_skills_worker_unavailable(&err.to_string()),
                ToastKind::Warning,
            );
            None
        }
    };

    let local_env = match start_local_env_system() {
        Ok(system) => Some(system),
        Err(err) => {
            app.stop_local_env_refresh();
            app.push_toast(
                texts::tui_toast_local_env_check_unavailable(&err.to_string()),
                ToastKind::Warning,
            );
            None
        }
    };

    let sessions = match start_session_system() {
        Ok(system) => Some(system),
        Err(err) => {
            app.push_toast(
                texts::tui_sessions_toast_worker_unavailable(&err.to_string()),
                ToastKind::Warning,
            );
            None
        }
    };

    let proxy_system = match start_proxy_system() {
        Ok(system) => Some(system),
        Err(err) => {
            app.push_toast(
                texts::tui_toast_proxy_worker_unavailable(&err.to_string()),
                ToastKind::Warning,
            );
            None
        }
    };

    let codex_history = match start_codex_history_system() {
        Ok(system) => Some(system),
        Err(err) => {
            app.push_toast(
                texts::tui_toast_codex_history_worker_unavailable(&err.to_string()),
                ToastKind::Warning,
            );
            None
        }
    };

    let quota = match start_quota_system() {
        Ok(system) => Some(system),
        Err(err) => {
            app.push_toast(
                texts::tui_toast_quota_worker_unavailable(&err.to_string()),
                ToastKind::Warning,
            );
            None
        }
    };

    let app_data = match start_app_data_system() {
        Ok(system) => Some(system),
        Err(err) => {
            app.push_toast(
                format!("App data worker unavailable: {err}"),
                ToastKind::Warning,
            );
            None
        }
    };

    let usage_pricing = match start_usage_pricing_system() {
        Ok(system) => Some(system),
        Err(err) => {
            app.push_toast(
                format!("Usage/pricing worker unavailable: {err}"),
                ToastKind::Warning,
            );
            None
        }
    };

    let session_usage = match start_session_usage_sync_system() {
        Ok(system) => Some(system),
        Err(err) => {
            log::debug!("Session usage sync worker unavailable: {err}");
            None
        }
    };

    let webdav = match start_webdav_system() {
        Ok(system) => Some(system),
        Err(err) => {
            app.push_toast(
                texts::tui_toast_webdav_worker_unavailable(&err.to_string()),
                ToastKind::Warning,
            );
            None
        }
    };

    let update_system = match start_update_system() {
        Ok(system) => Some(system),
        Err(err) => {
            app.push_toast(
                texts::tui_toast_update_check_failed(&err.to_string()),
                ToastKind::Warning,
            );
            None
        }
    };

    let model_fetch = match start_model_fetch_system() {
        Ok(system) => Some(system),
        Err(err) => {
            app.push_toast(
                texts::tui_toast_model_fetch_worker_unavailable(&err.to_string()),
                ToastKind::Warning,
            );
            None
        }
    };

    let managed_auth = match start_managed_auth_system() {
        Ok(system) => Some(system),
        Err(err) => {
            app.managed_auth_loading = false;
            app.push_toast(
                texts::tui_toast_managed_auth_worker_unavailable(&err.to_string()),
                ToastKind::Warning,
            );
            None
        }
    };

    let mut data_cache = UiDataByAppCache::default();
    let mut initial_data_loading = false;
    let mut initial_data_error: Option<AppError> = None;
    let mut initial_loading_quit_requested = false;
    if let Some(app_data) = app_data.as_ref() {
        initial_data_loading = true;
        // Pre-seed the OTHER visible apps from the same startup snapshot so cold
        // switches are instant — but only the lightweight ones whose snapshot is
        // a pure in-memory read (Claude/Codex/Gemini). Additive apps
        // (OpenCode/Hermes/OpenClaw) read live config files even in SnapshotOnly
        // mode, so they are left to lazy-load on first switch (the providers
        // loading state covers the brief gap) rather than pay that I/O at startup.
        let extra_apps: Vec<AppType> = crate::settings::get_visible_apps()
            .ordered_enabled()
            .into_iter()
            .filter(|candidate| candidate != &app.app_type)
            .filter(is_lightweight_preseed_app)
            .collect();
        if let Err(err) =
            data_cache.queue_initial_app_data_load(&app_data.req_tx, &app.app_type, &extra_apps)
        {
            initial_data_loading = false;
            initial_data_error = Some(err);
        }
    } else {
        data = data::UiData::load(&app.app_type)?;
        app.overlay = startup_overlay.take().unwrap_or(Overlay::None);
        initialize_loaded_app(
            &mut app,
            &mut data,
            &mut data_cache,
            quota.as_ref().map(|s| &s.req_tx),
        );
        queue_local_env_refresh_if_available(&mut app, local_env.as_ref().map(|s| &s.req_tx));
    }

    let mut input_reader = input::InputReader::crossterm();

    loop {
        if let Some(err) = initial_data_error.take() {
            return Err(err);
        }

        app.last_size = terminal.size()?;
        if !initial_data_loading {
            app.observe_proxy_visual_state(&data);
        }
        if frame_scheduler.is_due(Instant::now()) {
            let frame_started = Instant::now();
            let frame_dt = frame_started.saturating_duration_since(last_frame);
            terminal.draw(|f| {
                let area = f.area();
                if !initial_data_loading {
                    proxy_open_flash.sync(&app, area);
                }
                ui::render(f, &app, &data);
                if !initial_data_loading {
                    proxy_open_flash.process(frame_dt, f.buffer_mut(), area);
                }
            })?;
            let completed_at = Instant::now();
            // Animation time follows wall-clock frame starts, while the redraw
            // cap starts after terminal I/O completes.
            last_frame = frame_started;
            frame_scheduler.record_draw(completed_at);
            // Effects advance only while drawing, so keep scheduling frames
            // until the active effect reports completion.
            if proxy_open_flash.active() {
                frame_scheduler.mark_dirty();
            }
        }

        if initial_data_loading {
            if let Some(app_data) = app_data.as_ref() {
                match drain_initial_app_data_messages(
                    &mut app,
                    &mut data,
                    &mut data_cache,
                    &mut startup_overlay,
                    quota.as_ref().map(|s| &s.req_tx),
                    &app_data.result_rx,
                ) {
                    Ok(true) => {
                        initial_data_loading = false;
                        frame_scheduler.mark_dirty();
                        let current_app_type = app.app_type.clone();
                        let _ = data_cache.queue_current_app_data_refresh(
                            Some(&app_data.req_tx),
                            &current_app_type,
                        );
                        if drain_initial_loading_queued_events()? {
                            initial_loading_quit_requested = true;
                        }
                        queue_local_env_refresh_if_available(
                            &mut app,
                            local_env.as_ref().map(|s| &s.req_tx),
                        );
                    }
                    Ok(false) => {}
                    Err(err) => {
                        initial_data_loading = false;
                        initial_data_error = Some(err);
                    }
                }
            }

            if !should_poll_initial_loading_input(
                initial_data_loading,
                initial_data_error.is_some(),
            ) {
                if should_exit_after_initial_loading(
                    initial_data_loading,
                    initial_data_error.is_some(),
                    initial_loading_quit_requested,
                ) {
                    break;
                }
                continue;
            }

            let timeout = next_loop_timeout(
                tick_rate.saturating_sub(last_tick.elapsed()),
                frame_scheduler.wait(Instant::now()),
            );
            if event::poll(timeout).map_err(|e| AppError::Message(e.to_string()))? {
                let event = event::read().map_err(|e| AppError::Message(e.to_string()))?;
                if let Some(app_data) = app_data.as_ref() {
                    match drain_initial_app_data_messages(
                        &mut app,
                        &mut data,
                        &mut data_cache,
                        &mut startup_overlay,
                        quota.as_ref().map(|s| &s.req_tx),
                        &app_data.result_rx,
                    ) {
                        Ok(true) => {
                            initial_data_loading = false;
                            frame_scheduler.mark_dirty();
                            let current_app_type = app.app_type.clone();
                            let _ = data_cache.queue_current_app_data_refresh(
                                Some(&app_data.req_tx),
                                &current_app_type,
                            );
                            if drain_initial_loading_queued_events()? {
                                initial_loading_quit_requested = true;
                            }
                            queue_local_env_refresh_if_available(
                                &mut app,
                                local_env.as_ref().map(|s| &s.req_tx),
                            );
                        }
                        Ok(false) => {}
                        Err(err) => {
                            initial_data_loading = false;
                            initial_data_error = Some(err);
                        }
                    }
                }
                if matches!(&event, event::Event::Resize(..)) {
                    frame_scheduler.mark_dirty();
                }
                record_initial_loading_quit_event(&mut initial_loading_quit_requested, &event);
                if !should_poll_initial_loading_input(
                    initial_data_loading,
                    initial_data_error.is_some(),
                ) {
                    if should_exit_after_initial_loading(
                        initial_data_loading,
                        initial_data_error.is_some(),
                        initial_loading_quit_requested,
                    ) {
                        break;
                    }
                    continue;
                }
            }

            if last_tick.elapsed() >= tick_rate {
                app.on_tick();
                app.note_session_sync_round(session_usage_sync_is_active());
                last_tick = Instant::now();
                frame_scheduler.mark_dirty();
            }

            // Fire pending deep search from debounce timer
            if matches!(app.route, route::Route::Sessions) {
                if let Some(query) = app.pending_deep_search.take() {
                    runtime_actions::queue_pending_sessions_deep_search(
                        &mut app,
                        sessions.as_ref().map(|system| &system.req_tx),
                        query,
                    );
                }
            }
            if app.pending_project_catalog {
                runtime_actions::queue_sessions_project_catalog(
                    &mut app,
                    sessions.as_ref().map(|system| &system.req_tx),
                );
            }
            if let Some(query) = app.pending_project_filter.take() {
                runtime_actions::queue_sessions_project_filter(
                    &mut app,
                    sessions.as_ref().map(|system| &system.req_tx),
                    query,
                );
            }

            if app.should_quit {
                break;
            }
            continue;
        }

        queue_sessions_refresh_if_needed(&mut app, sessions.as_ref().map(|s| &s.req_tx));
        maybe_queue_session_page_load(&mut app, sessions.as_ref().map(|s| &s.req_tx));
        maybe_queue_session_message_page_load(&mut app, sessions.as_ref().map(|s| &s.req_tx));
        maybe_queue_session_manifest_reconcile(&mut app, sessions.as_ref().map(|s| &s.req_tx));

        if let Some(speedtest) = speedtest.as_ref() {
            while let Ok(msg) = speedtest.result_rx.try_recv() {
                frame_scheduler.mark_dirty();
                handle_speedtest_msg(&mut app, msg);
            }
        }

        if let Some(stream_check) = stream_check.as_ref() {
            while let Ok(msg) = stream_check.result_rx.try_recv() {
                frame_scheduler.mark_dirty();
                handle_stream_check_msg(&mut app, msg);
            }
        }

        if let Some(codex_history) = codex_history.as_ref() {
            while let Ok(msg) = codex_history.result_rx.try_recv() {
                frame_scheduler.mark_dirty();
                handle_codex_history_msg(&mut app, &mut codex_history_save, msg);
            }
        }

        if let Some(local_env) = local_env.as_ref() {
            while let Ok(msg) = local_env.result_rx.try_recv() {
                frame_scheduler.mark_dirty();
                handle_local_env_msg(&mut app, msg);
            }
        }

        if let Some(sessions) = sessions.as_ref() {
            while let Ok(msg) = sessions.result_rx.try_recv() {
                frame_scheduler.mark_dirty();
                handle_session_msg(&mut app, msg);
                let _ = queue_pending_session_cost(&mut app, Some(&sessions.cost_req_tx));
                if app.sessions.take_manual_usage_sync_request() {
                    if session_usage_sync.active.is_some() {
                        manual_session_cost_sync = ManualSessionCostSyncState::NeedsRunAfterCurrent;
                    } else if queue_background_session_usage_sync(
                        session_usage.as_ref().map(|system| &system.req_tx),
                        &mut session_usage_sync,
                    ) {
                        manual_session_cost_sync = ManualSessionCostSyncState::AwaitingManualRun;
                    } else {
                        manual_session_cost_sync = ManualSessionCostSyncState::Idle;
                    }
                }
                if app.sessions.take_message_cancel_pending() {
                    let _ = sessions.req_tx.send(SessionReq::CancelMessages);
                }
            }
        }

        if let Some(proxy) = proxy_system.as_ref() {
            while let Ok(msg) = proxy.result_rx.try_recv() {
                frame_scheduler.mark_dirty();
                let effect = handle_proxy_msg(
                    &mut app,
                    &mut data,
                    &mut proxy_loading,
                    &mut proxy_snapshot_refresh,
                    msg,
                );
                apply_proxy_msg_effect(
                    &mut app,
                    &mut data,
                    &mut data_cache,
                    ProxyEffectWorkers {
                        app_data: app_data.as_ref().map(|system| &system.req_tx),
                        proxy: Some(&proxy.req_tx),
                        quota: quota.as_ref().map(|system| &system.req_tx),
                    },
                    &mut proxy_snapshot_refresh,
                    effect,
                );
            }
        }

        if let Some(quota) = quota.as_ref() {
            while let Ok(msg) = quota.result_rx.try_recv() {
                frame_scheduler.mark_dirty();
                handle_quota_msg(&mut app, &mut data, msg);
            }
        }

        if let Some(app_data) = app_data.as_ref() {
            while let Ok(msg) = app_data.result_rx.try_recv() {
                frame_scheduler.mark_dirty();
                handle_app_data_msg(
                    &mut app,
                    &mut data,
                    &mut data_cache,
                    quota.as_ref().map(|s| &s.req_tx),
                    Some(&app_data.req_tx),
                    usage_pricing.as_ref().map(|s| &s.req_tx),
                    msg,
                );
            }
        }

        if let Some(usage_pricing) = usage_pricing.as_ref() {
            while let Ok(msg) = usage_pricing.result_rx.try_recv() {
                frame_scheduler.mark_dirty();
                if handle_usage_pricing_msg(&mut app, &mut data, &mut data_cache, msg)
                    && session_usage_sync.active.is_none()
                {
                    // A completed session sync can mark an in-flight aggregate
                    // dirty. Once it finishes, launch one fresh follow-up
                    // without disturbing log page/detail snapshot tokens.
                    data_cache.flush_dirty_usage_pricing(&mut app, Some(&usage_pricing.req_tx));
                }
            }
        }

        if let Some(session_usage) = session_usage.as_ref() {
            while let Ok(msg) = session_usage.result_rx.try_recv() {
                frame_scheduler.mark_dirty();
                let terminal = handle_session_usage_sync_msg(
                    &mut app,
                    &mut data,
                    &mut data_cache,
                    &mut session_usage_sync,
                    usage_pricing.as_ref().map(|s| &s.req_tx),
                    msg,
                );
                match manual_session_cost_sync_action(
                    &mut manual_session_cost_sync,
                    terminal,
                    matches!(app.route, route::Route::Sessions),
                ) {
                    ManualSessionCostSyncAction::RequeryVisiblePage => {
                        let _ = queue_current_session_cost(
                            &mut app,
                            sessions.as_ref().map(|system| &system.cost_req_tx),
                        );
                    }
                    ManualSessionCostSyncAction::QueueRun => {
                        if !queue_background_session_usage_sync(
                            Some(&session_usage.req_tx),
                            &mut session_usage_sync,
                        ) {
                            manual_session_cost_sync = ManualSessionCostSyncState::Idle;
                        }
                    }
                    ManualSessionCostSyncAction::None => {}
                }
            }
        }

        if let Some(skills) = skills.as_ref() {
            while let Ok(msg) = skills.result_rx.try_recv() {
                frame_scheduler.mark_dirty();
                match handle_skills_msg(&mut app, &mut data, msg) {
                    Ok(invalidation) => {
                        if let Err(err) = apply_cache_invalidation(
                            &mut app,
                            &mut data,
                            &mut data_cache,
                            quota.as_ref().map(|s| &s.req_tx),
                            app_data.as_ref().map(|s| &s.req_tx),
                            usage_pricing.as_ref().map(|s| &s.req_tx),
                            invalidation,
                        ) {
                            app.push_toast(err.to_string(), ToastKind::Error);
                        }
                    }
                    Err(err) => app.push_toast(err.to_string(), ToastKind::Error),
                }
            }
        }

        if let Some(webdav) = webdav.as_ref() {
            while let Ok(msg) = webdav.result_rx.try_recv() {
                frame_scheduler.mark_dirty();
                match handle_webdav_msg(&mut app, &mut data, &mut webdav_loading, msg) {
                    Ok(invalidation) => {
                        if let Err(err) = apply_cache_invalidation(
                            &mut app,
                            &mut data,
                            &mut data_cache,
                            quota.as_ref().map(|s| &s.req_tx),
                            app_data.as_ref().map(|s| &s.req_tx),
                            usage_pricing.as_ref().map(|s| &s.req_tx),
                            invalidation,
                        ) {
                            app.push_toast(err.to_string(), ToastKind::Error);
                        }
                    }
                    Err(err) => app.push_toast(err.to_string(), ToastKind::Error),
                }
            }
        }

        if let Some(us) = update_system.as_ref() {
            while let Ok(msg) = us.result_rx.try_recv() {
                frame_scheduler.mark_dirty();
                handle_update_msg(&mut app, &mut update_check, msg);
            }
        }

        if let Some(mf) = model_fetch.as_ref() {
            while let Ok(msg) = mf.result_rx.try_recv() {
                frame_scheduler.mark_dirty();
                handle_model_fetch_msg(&mut app, msg);
            }
        }

        if let Some(auth) = managed_auth.as_ref() {
            while let Ok(msg) = auth.result_rx.try_recv() {
                frame_scheduler.mark_dirty();
                handle_managed_auth_msg(&mut app, msg);
            }
        }

        if app.should_poll_managed_auth_login() {
            if let Some(login) = app.managed_auth_login.as_mut() {
                login.next_poll_tick = app.tick.saturating_add(login.poll_interval_ticks.max(1));
                if let Some(auth) = managed_auth.as_ref() {
                    if let Err(err) = auth.req_tx.send(ManagedAuthReq::PollLogin {
                        auth_provider: login.auth_provider.clone(),
                        device_code: login.device_code.clone(),
                    }) {
                        app.push_toast(
                            texts::tui_toast_managed_auth_request_failed(&err.to_string()),
                            ToastKind::Warning,
                        );
                    }
                }
            }
        }

        let timeout = next_loop_timeout(
            tick_rate.saturating_sub(last_tick.elapsed()),
            frame_scheduler.wait(Instant::now()),
        );
        let inputs = input_reader
            .read_batch(timeout)
            .map_err(|e| AppError::Message(e.to_string()))?;
        if !inputs.is_empty() {
            // Input is reduced independently of rendering. Even a no-op wheel
            // segment may mark the frame dirty, but the scheduler still caps
            // terminal writes to one per frame interval.
            frame_scheduler.mark_dirty();
        }
        for input in inputs {
            match input {
                input::UiInput::Key(key) => {
                    let key = normalize_key_event(key);
                    let before_app_type = app.app_type.clone();
                    let action = app.on_key(key, &data);
                    let action = queue_codex_history_action(
                        &mut app,
                        codex_history.as_ref().map(|system| &system.req_tx),
                        &mut codex_history_save,
                        action,
                    );
                    let action_result = handle_tui_action(
                        &mut terminal,
                        &mut app,
                        &mut data,
                        &mut data_cache,
                        app_data.as_ref().map(|s| &s.req_tx),
                        speedtest.as_ref().map(|s| &s.req_tx),
                        stream_check.as_ref().map(|s| &s.req_tx),
                        skills.as_ref().map(|s| &s.req_tx),
                        proxy_system.as_ref().map(|s| &s.req_tx),
                        &mut proxy_loading,
                        local_env.as_ref().map(|s| &s.req_tx),
                        sessions.as_ref().map(|s| &s.req_tx),
                        webdav.as_ref().map(|s| &s.req_tx),
                        &mut webdav_loading,
                        update_system.as_ref().map(|s| &s.req_tx),
                        &mut update_check,
                        model_fetch.as_ref().map(|s| &s.req_tx),
                        managed_auth.as_ref().map(|s| &s.req_tx),
                        quota.as_ref().map(|s| &s.req_tx),
                        usage_pricing.as_ref().map(|s| &s.req_tx),
                        session_usage
                            .as_ref()
                            .map(|s| (&s.req_tx, &mut session_usage_sync)),
                        action,
                    );
                    let action_succeeded = action_result.is_ok();
                    if let Err(err) = action_result {
                        if matches!(
                            &err,
                            AppError::Localized { key, .. } if *key == "tui_terminal_error"
                        ) {
                            return Err(err);
                        }
                        app.push_toast(err.to_string(), ToastKind::Error);
                    }
                    queue_proxy_snapshot_refresh_after_app_switch(
                        &mut proxy_snapshot_refresh,
                        proxy_system.as_ref().map(|s| &s.req_tx),
                        &before_app_type,
                        &app.app_type,
                        action_succeeded,
                    );
                }
                input::UiInput::Wheel {
                    direction,
                    steps,
                    gesture,
                } => {
                    let before_app_type = app.app_type.clone();
                    let action = app.on_wheel(direction, steps, gesture, &data);
                    let action = queue_codex_history_action(
                        &mut app,
                        codex_history.as_ref().map(|system| &system.req_tx),
                        &mut codex_history_save,
                        action,
                    );
                    let action_result = handle_tui_action(
                        &mut terminal,
                        &mut app,
                        &mut data,
                        &mut data_cache,
                        app_data.as_ref().map(|s| &s.req_tx),
                        speedtest.as_ref().map(|s| &s.req_tx),
                        stream_check.as_ref().map(|s| &s.req_tx),
                        skills.as_ref().map(|s| &s.req_tx),
                        proxy_system.as_ref().map(|s| &s.req_tx),
                        &mut proxy_loading,
                        local_env.as_ref().map(|s| &s.req_tx),
                        sessions.as_ref().map(|s| &s.req_tx),
                        webdav.as_ref().map(|s| &s.req_tx),
                        &mut webdav_loading,
                        update_system.as_ref().map(|s| &s.req_tx),
                        &mut update_check,
                        model_fetch.as_ref().map(|s| &s.req_tx),
                        managed_auth.as_ref().map(|s| &s.req_tx),
                        quota.as_ref().map(|s| &s.req_tx),
                        usage_pricing.as_ref().map(|s| &s.req_tx),
                        session_usage
                            .as_ref()
                            .map(|s| (&s.req_tx, &mut session_usage_sync)),
                        action,
                    );
                    let action_succeeded = action_result.is_ok();
                    if let Err(err) = action_result {
                        if matches!(
                            &err,
                            AppError::Localized { key, .. } if *key == "tui_terminal_error"
                        ) {
                            return Err(err);
                        }
                        app.push_toast(err.to_string(), ToastKind::Error);
                    }
                    queue_proxy_snapshot_refresh_after_app_switch(
                        &mut proxy_snapshot_refresh,
                        proxy_system.as_ref().map(|s| &s.req_tx),
                        &before_app_type,
                        &app.app_type,
                        action_succeeded,
                    );
                }
                input::UiInput::Resize { .. } => {}
            }
            if app.should_quit {
                break;
            }
        }
        if !app.should_quit {
            let _ = queue_pending_session_cost(
                &mut app,
                sessions.as_ref().map(|system| &system.cost_req_tx),
            );
        }

        // Kick off the deferred session-usage sync the first time the user lands
        // on a Usage route (not at startup).
        maybe_queue_usage_session_sync(
            &app,
            session_usage.as_ref().map(|s| &s.req_tx),
            &mut session_usage_sync,
            &mut session_usage_sync_started,
            crate::settings::usage_auto_sync_enabled(),
        );
        // Lazily aggregate usage/pricing (deferred off the startup full-load)
        // once the current route consumes one of its projections.
        maybe_queue_usage_pricing_on_view(
            &mut app,
            &mut data_cache,
            usage_pricing.as_ref().map(|s| &s.req_tx),
        );
        maybe_queue_usage_log_page_refresh_after_aggregate(
            &mut app,
            &mut data_cache,
            usage_pricing.as_ref().map(|s| &s.req_tx),
        );
        maybe_queue_usage_log_prefetch(
            &mut app,
            &data,
            &mut data_cache,
            usage_pricing.as_ref().map(|s| &s.req_tx),
        );

        if last_tick.elapsed() >= tick_rate {
            app.on_tick();
            app.note_session_sync_round(session_usage_sync_is_active());
            if app.should_poll_proxy_activity() {
                queue_proxy_snapshot_refresh(
                    &mut proxy_snapshot_refresh,
                    proxy_system.as_ref().map(|s| &s.req_tx),
                    &app.app_type,
                );
            }
            queue_current_quota_refresh_if_due(
                &mut app,
                &mut data,
                quota.as_ref().map(|s| &s.req_tx),
            );
            queue_usage_session_sync_if_due(
                &mut app,
                session_usage.as_ref().map(|s| &s.req_tx),
                &mut session_usage_sync,
            );
            queue_usage_proxy_refresh_if_due(
                &mut app,
                &mut data_cache,
                usage_pricing.as_ref().map(|s| &s.req_tx),
            );
            last_tick = Instant::now();
            frame_scheduler.mark_dirty();
        }

        // Fire pending deep search from debounce timer
        if matches!(app.route, route::Route::Sessions) {
            if let Some(query) = app.pending_deep_search.take() {
                runtime_actions::queue_pending_sessions_deep_search(
                    &mut app,
                    sessions.as_ref().map(|system| &system.req_tx),
                    query,
                );
            }
        }
        if app.pending_project_catalog {
            runtime_actions::queue_sessions_project_catalog(
                &mut app,
                sessions.as_ref().map(|system| &system.req_tx),
            );
        }
        if let Some(query) = app.pending_project_filter.take() {
            runtime_actions::queue_sessions_project_filter(
                &mut app,
                sessions.as_ref().map(|system| &system.req_tx),
                query,
            );
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

fn normalize_key_event(mut key: KeyEvent) -> KeyEvent {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('h') {
        key.code = KeyCode::Backspace;
        key.modifiers.remove(KeyModifiers::CONTROL);
    }
    key
}
