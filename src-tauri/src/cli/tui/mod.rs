mod app;
mod data;
mod form;
pub(crate) mod help;
pub(crate) mod icons;
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

use crossterm::event::{self, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};

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
    handle_local_env_msg, handle_managed_auth_msg, handle_model_fetch_msg, handle_proxy_msg,
    handle_quota_msg, handle_session_msg, handle_skills_msg, handle_speedtest_msg,
    handle_stream_check_msg, handle_update_msg, handle_webdav_msg, start_app_data_system,
    start_local_env_system, start_managed_auth_system, start_model_fetch_system,
    start_proxy_system, start_quota_system, start_session_system, start_session_usage_sync_system,
    start_skills_system, start_speedtest_system, start_stream_check_system, start_update_system,
    start_usage_pricing_system, start_webdav_system, AppDataLoadKind, AppDataMsg, AppDataReq,
    LocalEnvReq, ManagedAuthReq, ModelFetchReq, ProxyReq, QuotaReq, RequestTracker, SessionReq,
    SessionUsageSyncMsg, SessionUsageSyncReq, SkillsReq, StreamCheckReq, UpdateReq,
    UsagePricingMsg, UsagePricingReq, WebDavReq,
};
use terminal::{PanicRestoreHookGuard, TuiTerminal};

pub(super) const TUI_TICK_RATE: Duration = Duration::from_millis(200);
const QUOTA_REFRESH_INTERVAL_TICKS: u64 = 5 * 60 * 1000 / 200;

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

    #[cfg(test)]
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

    data.quota.mark_loading(target.clone(), manual);
    if let Err(error) = tx.send(QuotaReq::Refresh {
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
    let due = app
        .quota_last_auto_tick
        .is_none_or(|last_tick| app.tick.saturating_sub(last_tick) >= QUOTA_REFRESH_INTERVAL_TICKS);

    if target_changed || target_missing_state || due {
        app.quota_auto_target_key = Some(target_key);
        app.quota_last_auto_tick = Some(app.tick);
        queue_quota_refresh(app, data, quota_req_tx, target, false);
    }
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

#[derive(Default)]
struct UiDataByAppCache {
    by_app: HashMap<AppType, data::UiData>,
    pending_by_app: HashMap<AppType, PendingAppDataLoad>,
    incomplete_by_app: HashSet<AppType>,
    usage_pricing_by_key: HashMap<UsagePricingLoadKey, data::UsagePricingData>,
    pending_usage_pricing_by_key: HashMap<UsagePricingLoadKey, PendingDataLoad>,
    next_app_data_request_id: u64,
    next_usage_pricing_request_id: u64,
    data_generation: u64,
    app_state_epoch: u64,
}

type UsagePricingLoadKey = (AppType, data::UsageRangePreset);

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
        if let Some(cached) = self.by_app.get_mut(app_type) {
            cached.usage.merge_range(range, usage_pricing.usage.clone());
            if let Some(pricing) = &usage_pricing.pricing {
                cached.pricing = pricing.clone();
            }
        }
        self.usage_pricing_by_key
            .insert((app_type.clone(), range), usage_pricing);
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
    }

    fn clear_after_app_state_recreated(&mut self) {
        self.app_state_epoch = self.app_state_epoch.wrapping_add(1);
        self.clear();
    }

    fn clear_usage_pricing_after_external_usage_sync(&mut self) {
        self.data_generation = self.data_generation.wrapping_add(1);
        self.app_state_epoch = self.app_state_epoch.wrapping_add(1);
        self.pending_by_app.clear();
        self.usage_pricing_by_key.clear();
        self.pending_usage_pricing_by_key.clear();
        for cached in self.by_app.values_mut() {
            cached.usage = data::UsageSnapshot::default();
            cached.pricing = data::ModelPricingSnapshot::default();
        }
    }

    fn remove_app_snapshot(&mut self, app_type: &AppType) {
        self.by_app.remove(app_type);
        self.pending_by_app.remove(app_type);
        self.incomplete_by_app.remove(app_type);
    }

    fn remove_usage_pricing_for_app(&mut self, app_type: &AppType) {
        self.usage_pricing_by_key
            .retain(|(cached_app_type, _), _| cached_app_type != app_type);
        self.pending_usage_pricing_by_key
            .retain(|(cached_app_type, _), _| cached_app_type != app_type);
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
        if self
            .pending_usage_pricing_by_key
            .contains_key(&(app_type.clone(), range))
        {
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

    fn queue_usage_pricing_load(
        &mut self,
        app: &mut App,
        usage_pricing_req_tx: Option<&mpsc::Sender<UsagePricingReq>>,
        app_type: &AppType,
        range: data::UsageRangePreset,
    ) {
        if matches!(range, data::UsageRangePreset::Custom(_)) {
            app.usage.clear_custom_loading_for_app(app_type);
            self.pending_usage_pricing_by_key
                .retain(|(cached_app_type, cached_range), _| {
                    cached_app_type != app_type
                        || !matches!(cached_range, data::UsageRangePreset::Custom(_))
                });
            self.usage_pricing_by_key
                .retain(|(cached_app_type, cached_range), _| {
                    cached_app_type != app_type
                        || !matches!(cached_range, data::UsageRangePreset::Custom(_))
                });
        }

        let key = (app_type.clone(), range);
        if self.pending_usage_pricing_by_key.contains_key(&key) {
            app.usage.start_loading(app_type.clone(), range);
            return;
        }

        let Some(tx) = usage_pricing_req_tx else {
            if matches!(range, data::UsageRangePreset::Custom(_)) {
                app.push_toast(
                    "Usage/pricing worker is not running; custom range cannot be loaded."
                        .to_string(),
                    ToastKind::Warning,
                );
            }
            return;
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

        if let Err(err) = tx.send(UsagePricingReq::Load {
            request_id,
            generation: pending.generation,
            app_state_epoch: pending.app_state_epoch,
            app_type: app_type.clone(),
            range,
        }) {
            self.pending_usage_pricing_by_key.remove(&key);
            app.push_toast(
                format!("Usage/pricing refresh request failed: {err}"),
                ToastKind::Warning,
            );
        } else {
            app.usage.start_loading(app_type.clone(), range);
        }
    }

    fn finish_usage_pricing_load(
        &mut self,
        app_type: &AppType,
        request_id: u64,
        generation: u64,
        app_state_epoch: u64,
        range: data::UsageRangePreset,
    ) -> bool {
        let key = (app_type.clone(), range);
        if self.pending_usage_pricing_by_key.get(&key).copied()
            != Some(PendingDataLoad {
                request_id,
                generation,
                app_state_epoch,
            })
        {
            return false;
        }
        self.pending_usage_pricing_by_key.remove(&key);
        true
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
) {
    match msg {
        UsagePricingMsg::Loaded {
            request_id,
            generation,
            app_state_epoch,
            app_type,
            range,
            result,
        } => {
            if !data_cache.finish_usage_pricing_load(
                &app_type,
                request_id,
                generation,
                app_state_epoch,
                range,
            ) {
                return;
            }
            app.usage.finish_loading(&app_type, range);

            match result {
                Ok(usage_pricing) => {
                    data_cache.update_usage_pricing(&app_type, range, usage_pricing.clone());
                    if app.app_type == app_type {
                        data.usage.merge_range(range, usage_pricing.usage);
                        if let Some(pricing) = usage_pricing.pricing {
                            data.pricing = pricing;
                        }
                        app.clamp_selections(data);
                        data_cache.remember_current(&app.app_type, data);
                    }
                }
                Err(err) => {
                    if app.app_type == app_type {
                        app.push_toast(
                            format!("Usage/pricing refresh failed: {err}"),
                            ToastKind::Warning,
                        );
                    }
                }
            }
        }
    }
}

fn queue_background_session_usage_sync(
    sync_req_tx: Option<&mpsc::Sender<SessionUsageSyncReq>>,
    sync_tracker: &mut RequestTracker,
) {
    let Some(tx) = sync_req_tx else {
        return;
    };
    if sync_tracker.active.is_some() {
        return;
    }

    let request_id = sync_tracker.start();
    if let Err(err) = tx.send(SessionUsageSyncReq::Run { request_id }) {
        sync_tracker.cancel();
        log::debug!("queue background session usage sync failed: {err}");
    }
}

/// Lazily kick off the session-usage sync the first time a Usage route is shown
/// this run. The sync scans the whole session-log history (expensive on large
/// histories) and only feeds the Usage view, so it is deferred off startup.
fn maybe_queue_usage_session_sync(
    app: &App,
    sync_req_tx: Option<&mpsc::Sender<SessionUsageSyncReq>>,
    sync_tracker: &mut RequestTracker,
    started: &mut bool,
) {
    if *started {
        return;
    }
    if !matches!(
        app.route,
        route::Route::Usage | route::Route::UsageLogs | route::Route::UsageLogDetail { .. }
    ) {
        return;
    }
    *started = true;
    queue_background_session_usage_sync(sync_req_tx, sync_tracker);
}

/// Lazily load the active app's usage/pricing when a Usage or Pricing view is
/// shown. The startup full-load no longer computes it, so this fills it in on
/// demand; the cache guard keeps it from re-querying every frame.
fn maybe_queue_usage_pricing_on_view(
    app: &mut App,
    data_cache: &mut UiDataByAppCache,
    usage_pricing_req_tx: Option<&mpsc::Sender<UsagePricingReq>>,
) {
    let is_usage = matches!(
        app.route,
        route::Route::Usage | route::Route::UsageLogs | route::Route::UsageLogDetail { .. }
    );
    let is_pricing = matches!(app.route, route::Route::Pricing);
    if !is_usage && !is_pricing {
        return;
    }
    let app_type = app.app_type.clone();
    // The Pricing view needs the pricing snapshot, which only fixed ranges
    // produce (a custom range yields `pricing: None`); force a fixed range there.
    let range = if is_pricing && matches!(app.usage.range, data::UsageRangePreset::Custom(_)) {
        data::UsageRangePreset::SevenDays
    } else {
        app.usage.range
    };
    data_cache.ensure_usage_pricing_loaded(app, usage_pricing_req_tx, &app_type, range);
}

fn handle_session_usage_sync_msg(
    app: &mut App,
    data: &mut data::UiData,
    data_cache: &mut UiDataByAppCache,
    sync_tracker: &mut RequestTracker,
    usage_pricing_req_tx: Option<&mpsc::Sender<UsagePricingReq>>,
    msg: SessionUsageSyncMsg,
) {
    let SessionUsageSyncMsg::Finished { request_id, result } = msg;
    if !sync_tracker.finish_if_active(request_id) {
        return;
    }

    if let Err(err) = result {
        log::debug!("background session usage sync failed: {err}");
        return;
    }

    if usage_pricing_req_tx.is_none() {
        log::debug!("background session usage sync finished; usage/pricing worker unavailable");
        return;
    }

    app.usage.clear_loading();
    data_cache.clear_usage_pricing_after_external_usage_sync();

    let current_app_type = app.app_type.clone();
    // Always refresh the range the user is actually viewing (Today / 30d / custom),
    // otherwise the active range would show stale numbers after the sync finishes
    // while the user is sitting on the Usage view.
    let active_range = app.usage.range;
    data_cache.queue_usage_pricing_load(app, usage_pricing_req_tx, &current_app_type, active_range);
    // Keep the default 7-day window warm too (used as the cached default).
    if !matches!(active_range, data::UsageRangePreset::SevenDays) {
        data_cache.queue_usage_pricing_load(
            app,
            usage_pricing_req_tx,
            &current_app_type,
            data::UsageRangePreset::SevenDays,
        );
    }
    data_cache.remember_current(&app.app_type, data);
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
        | Action::SessionsRefresh
        | Action::SessionsDeepSearch { .. }
        | Action::SessionMessagesLoad { .. }
        | Action::SessionResume { .. }
        | Action::SessionDelete { .. }
        | Action::ProviderSpeedtest { .. }
        | Action::ProviderLaunchTemporary { .. }
        | Action::ProviderStreamCheck { .. }
        | Action::ProviderQuotaRefresh { .. }
        | Action::ProviderModelFetch { .. }
        | Action::UsageCustomRange { .. }
        | Action::ManagedAuthRefresh { .. }
        | Action::ManagedAuthStartLogin { .. }
        | Action::ManagedAuthSetDefault { .. }
        | Action::ManagedAuthRemove { .. }
        | Action::SkillsInstall { .. }
        | Action::SkillsDiscover { .. }
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
        | Action::OpenClawWorkspaceOpenFile { .. }
        | Action::OpenClawDailyMemoryOpenFile { .. }
        | Action::OpenClawDailyMemorySearch { .. }
        | Action::OpenClawOpenDirectory { .. }
        | Action::HermesMemoryOpen { .. }
        | Action::SetSkipClaudeOnboarding { .. }
        | Action::SetClaudePluginIntegration { .. }
        | Action::SetCodexUnifiedSessionHistory { .. }
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
        | Action::ConfigWebDavMigrateV1ToV2 => CacheInvalidation::AppStateRecreated,

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
    let active_custom_range = if matches!(invalidation, CacheInvalidation::None) {
        None
    } else if let data::UsageRangePreset::Custom(range) = app.usage.range {
        data.usage.begin_custom_range(range);
        app.clamp_selections(data);
        Some(range)
    } else {
        None
    };

    if !matches!(invalidation, CacheInvalidation::None) {
        app.usage.clear_loading();
    }

    data_cache.handle_data_reloaded(app, data, invalidation);
    if !matches!(invalidation, CacheInvalidation::None) {
        queue_current_quota_refresh_if_due(app, data, quota_req_tx);
        if let Some(range) = active_custom_range {
            let current_app_type = app.app_type.clone();
            data_cache.queue_usage_pricing_load(
                app,
                usage_pricing_req_tx,
                &current_app_type,
                data::UsageRangePreset::Custom(range),
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
    local_env_req_tx: Option<&mpsc::Sender<LocalEnvReq>>,
    session_req_tx: Option<&mpsc::Sender<SessionReq>>,
    webdav_req_tx: Option<&mpsc::Sender<WebDavReq>>,
    webdav_loading: &mut RequestTracker,
    update_req_tx: Option<&mpsc::Sender<UpdateReq>>,
    update_check: &mut RequestTracker,
    model_fetch_req_tx: Option<&mpsc::Sender<ModelFetchReq>>,
    managed_auth_req_tx: Option<&mpsc::Sender<ManagedAuthReq>>,
    quota_req_tx: Option<&mpsc::Sender<QuotaReq>>,
    usage_pricing_req_tx: Option<&mpsc::Sender<UsagePricingReq>>,
    action: Action,
) -> Result<(), AppError> {
    let should_queue_sessions_refresh = matches!(
        &action,
        Action::SwitchRoute(route::Route::Sessions) | Action::SetAppType(_)
    );
    let result = match action {
        Action::None => Ok(()),
        Action::ProviderQuotaRefresh { id } => {
            queue_provider_quota_refresh(app, data, quota_req_tx, &id);
            Ok(())
        }
        Action::SetAppType(next) => {
            data_cache.switch_to(app, data, app_data_req_tx, next)?;
            let current_app_type = app.app_type.clone();
            data_cache.queue_usage_pricing_load(
                app,
                usage_pricing_req_tx,
                &current_app_type,
                data::UsageRangePreset::SevenDays,
            );
            if matches!(app.usage.range, data::UsageRangePreset::Custom(_)) {
                data_cache.queue_usage_pricing_load(
                    app,
                    usage_pricing_req_tx,
                    &current_app_type,
                    app.usage.range,
                );
            }
            queue_current_quota_refresh_if_due(app, data, quota_req_tx);
            Ok(())
        }
        Action::UsageCustomRange { range } => {
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

fn queue_sessions_refresh_if_needed(
    app: &mut App,
    session_req_tx: Option<&mpsc::Sender<runtime_systems::SessionReq>>,
) {
    if !matches!(app.route, route::Route::Sessions) {
        return;
    }
    // When "show all providers" is on, use "all" as the virtual provider id
    // so the worker scans every provider and the cache is keyed separately.
    let provider_id = if app.sessions.show_all_providers {
        "all".to_string()
    } else {
        app.app_type.as_str().to_string()
    };
    if app.sessions.loaded_for_provider(&provider_id) || app.sessions.loading {
        return;
    }

    // Reuse a fresh cached scan so toggling between apps is instant; `r`
    // (Action::SessionsRefresh) bypasses this path and always re-scans.
    if app.sessions.restore_from_scan_cache(&provider_id) {
        return;
    }

    let Some(tx) = session_req_tx else {
        app.sessions.loading = false;
        app.sessions.loaded_once = true;
        app.push_toast(
            texts::tui_sessions_toast_worker_unavailable("sessions worker is not running"),
            ToastKind::Warning,
        );
        return;
    };

    let request_id = app.sessions.start_scan(provider_id.clone());
    if let Err(err) = tx.send(runtime_systems::SessionReq::Refresh {
        request_id,
        provider_id,
        // Entering the page uses the cached snapshot + delta revalidate; only
        // manual `r` forces a full re-parse.
        force: false,
    }) {
        app.sessions.fail_scan(request_id, err.to_string());
        app.push_toast(
            texts::tui_sessions_toast_refresh_failed(&err.to_string()),
            ToastKind::Warning,
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
    local_env_req_tx: Option<&mpsc::Sender<LocalEnvReq>>,
) {
    let Some(tx) = local_env_req_tx else {
        return;
    };
    app.local_env_loading = true;
    if let Err(err) = tx.send(LocalEnvReq::Refresh) {
        app.local_env_loading = false;
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
    let mut quit_requested = false;
    while event::poll(Duration::ZERO).map_err(|e| AppError::Message(e.to_string()))? {
        let event = event::read().map_err(|e| AppError::Message(e.to_string()))?;
        record_initial_loading_quit_event(&mut quit_requested, &event);
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
    // 后台会话用量导入进行时，Usage 页数字的周期性刷新节流
    let mut last_usage_sync_data_refresh = Instant::now();
    let mut last_frame = Instant::now();
    let mut proxy_open_flash = ProxyOpenFlash::default();
    let mut proxy_loading = RequestTracker::default();
    let mut proxy_snapshot_refresh = RequestTracker::default();
    let mut webdav_loading = RequestTracker::default();
    let mut update_check = RequestTracker::default();
    let mut session_usage_sync = RequestTracker::default();
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
            app.local_env_loading = false;
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

    loop {
        if let Some(err) = initial_data_error.take() {
            return Err(err);
        }

        app.last_size = terminal.size()?;
        if !initial_data_loading {
            app.observe_proxy_visual_state(&data);
        }
        let frame_dt = last_frame.elapsed();
        last_frame = Instant::now();
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

            let timeout = tick_rate.saturating_sub(last_tick.elapsed());
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
                last_tick = Instant::now();
            }

            // Fire pending deep search from debounce timer
            if let Some(query) = app.pending_deep_search.take() {
                // Directly send search request without full handle_tui_action
                if let Some(sessions_sys) = sessions.as_ref() {
                    let request_id = {
                        app.sessions.deep_search_query = Some(query.clone());
                        app.sessions.deep_search_results.clear();
                        app.sessions.deep_search_seq = app.sessions.deep_search_seq.wrapping_add(1);
                        app.sessions.deep_search_active = Some(app.sessions.deep_search_seq);
                        app.sessions.deep_search_seq
                    };
                    let sessions_snapshot = app.sessions.rows.clone();
                    let _ = sessions_sys
                        .req_tx
                        .send(runtime_systems::SessionReq::Search {
                            request_id,
                            query,
                            sessions: sessions_snapshot,
                        });
                }
            }

            if app.should_quit {
                break;
            }
            continue;
        }

        queue_sessions_refresh_if_needed(&mut app, sessions.as_ref().map(|s| &s.req_tx));

        if let Some(speedtest) = speedtest.as_ref() {
            while let Ok(msg) = speedtest.result_rx.try_recv() {
                handle_speedtest_msg(&mut app, msg);
            }
        }

        if let Some(stream_check) = stream_check.as_ref() {
            while let Ok(msg) = stream_check.result_rx.try_recv() {
                handle_stream_check_msg(&mut app, msg);
            }
        }

        if let Some(local_env) = local_env.as_ref() {
            while let Ok(msg) = local_env.result_rx.try_recv() {
                handle_local_env_msg(&mut app, msg);
            }
        }

        if let Some(sessions) = sessions.as_ref() {
            while let Ok(msg) = sessions.result_rx.try_recv() {
                handle_session_msg(&mut app, msg);
            }
        }

        if let Some(proxy) = proxy_system.as_ref() {
            while let Ok(msg) = proxy.result_rx.try_recv() {
                match handle_proxy_msg(
                    &mut app,
                    &mut data,
                    &mut proxy_loading,
                    &mut proxy_snapshot_refresh,
                    msg,
                ) {
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

        if let Some(quota) = quota.as_ref() {
            while let Ok(msg) = quota.result_rx.try_recv() {
                handle_quota_msg(&mut app, &mut data, msg);
            }
        }

        if let Some(app_data) = app_data.as_ref() {
            while let Ok(msg) = app_data.result_rx.try_recv() {
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
                handle_usage_pricing_msg(&mut app, &mut data, &mut data_cache, msg);
            }
        }

        if let Some(session_usage) = session_usage.as_ref() {
            while let Ok(msg) = session_usage.result_rx.try_recv() {
                handle_session_usage_sync_msg(
                    &mut app,
                    &mut data,
                    &mut data_cache,
                    &mut session_usage_sync,
                    usage_pricing.as_ref().map(|s| &s.req_tx),
                    msg,
                );
            }
        }

        if let Some(skills) = skills.as_ref() {
            while let Ok(msg) = skills.result_rx.try_recv() {
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
                handle_update_msg(&mut app, &mut update_check, msg);
            }
        }

        if let Some(mf) = model_fetch.as_ref() {
            while let Ok(msg) = mf.result_rx.try_recv() {
                handle_model_fetch_msg(&mut app, msg);
            }
        }

        if let Some(auth) = managed_auth.as_ref() {
            while let Ok(msg) = auth.result_rx.try_recv() {
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

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout).map_err(|e| AppError::Message(e.to_string()))? {
            match event::read().map_err(|e| AppError::Message(e.to_string()))? {
                event::Event::Key(key) if key.kind == KeyEventKind::Press => {
                    let key = normalize_key_event(key);
                    let before_app_type = app.app_type.clone();
                    let action = app.on_key(key, &data);
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
                event::Event::Mouse(mouse) => {
                    if let MouseEventKind::ScrollUp | MouseEventKind::ScrollDown = mouse.kind {
                        let code = if matches!(mouse.kind, MouseEventKind::ScrollUp) {
                            event::KeyCode::Up
                        } else {
                            event::KeyCode::Down
                        };
                        let key = event::KeyEvent::new(code, event::KeyModifiers::NONE);
                        let before_app_type = app.app_type.clone();
                        let action = app.on_key(key, &data);
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
                }
                event::Event::Resize(_, _) => {}
                _ => {}
            }
        }

        // Kick off the deferred session-usage sync the first time the user lands
        // on a Usage route (not at startup).
        maybe_queue_usage_session_sync(
            &app,
            session_usage.as_ref().map(|s| &s.req_tx),
            &mut session_usage_sync,
            &mut session_usage_sync_started,
        );
        // Lazily aggregate usage/pricing (deferred off the startup full-load)
        // once the user is actually on a Usage route.
        maybe_queue_usage_pricing_on_view(
            &mut app,
            &mut data_cache,
            usage_pricing.as_ref().map(|s| &s.req_tx),
        );

        if last_tick.elapsed() >= tick_rate {
            app.on_tick();
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
            // 后台会话用量导入进行时：每 ~2.5s 重新聚合一次当前区间，让
            // Usage 页数字随导入进度增长（P0 起最近文件最先入库，Today/7d
            // 很快就有意义）；同步空闲时零开销。
            if session_usage_sync.active.is_some()
                && matches!(
                    app.route,
                    route::Route::Usage
                        | route::Route::UsageLogs
                        | route::Route::UsageLogDetail { .. }
                )
                && last_usage_sync_data_refresh.elapsed() >= Duration::from_millis(2500)
            {
                let usage_pricing_tx = usage_pricing.as_ref().map(|s| &s.req_tx);
                if usage_pricing_tx.is_some() {
                    data_cache.clear_usage_pricing_after_external_usage_sync();
                    let range = app.usage.range;
                    let app_type = app.app_type.clone();
                    data_cache.queue_usage_pricing_load(
                        &mut app,
                        usage_pricing_tx,
                        &app_type,
                        range,
                    );
                }
                last_usage_sync_data_refresh = Instant::now();
            }
            last_tick = Instant::now();
        }

        // Fire pending deep search from debounce timer
        if let Some(query) = app.pending_deep_search.take() {
            if let Some(sessions_sys) = sessions.as_ref() {
                app.sessions.deep_search_query = Some(query.clone());
                app.sessions.deep_search_results.clear();
                app.sessions.deep_search_seq = app.sessions.deep_search_seq.wrapping_add(1);
                app.sessions.deep_search_active = Some(app.sessions.deep_search_seq);
                let request_id = app.sessions.deep_search_seq;
                let sessions_snapshot = app.sessions.rows.clone();
                let _ = sessions_sys
                    .req_tx
                    .send(runtime_systems::SessionReq::Search {
                        request_id,
                        query,
                        sessions: sessions_snapshot,
                    });
            }
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
