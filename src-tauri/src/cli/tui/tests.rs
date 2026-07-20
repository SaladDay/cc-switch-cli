use std::sync::mpsc;
use std::{ffi::OsString, path::Path};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect};
use serde_json::json;
use serial_test::serial;
use tempfile::TempDir;

use super::app::{
    App, ConfirmAction, ConfirmOverlay, EditorSubmit, LoadingKind, Overlay, ToastKind,
};
use super::data::UiData;
use super::form::ProviderAddField;
use super::*;
use crate::cli::i18n::texts;
use crate::test_support::{
    lock_test_home_and_settings, set_test_home_override, TestHomeSettingsLock,
};
use crate::{AppError, AppType};
use runtime_systems::ProxyMsg;

fn pending_snapshot_app_data(request_id: u64) -> PendingAppDataLoad {
    PendingAppDataLoad {
        kind: AppDataLoadKind::Snapshot,
        request_id,
        generation: 0,
        app_state_epoch: 0,
    }
}

fn pending_full_app_data(request_id: u64) -> PendingAppDataLoad {
    pending_full_app_data_with_epoch(request_id, 0, 0)
}

fn pending_full_app_data_with_epoch(
    request_id: u64,
    generation: u64,
    app_state_epoch: u64,
) -> PendingAppDataLoad {
    PendingAppDataLoad {
        kind: AppDataLoadKind::Full,
        request_id,
        generation,
        app_state_epoch,
    }
}

fn app_with_pending_purge_manifest() -> (App, TempDir, u64, String) {
    let manifest_dir = tempfile::tempdir().expect("manifest fixture directory");
    let store =
        crate::session_manager::paged_manifest::PagedManifestStore::open_at(manifest_dir.path())
            .expect("manifest fixture store");
    let deleted = crate::session_manager::SessionMeta {
        provider_id: "claude".to_string(),
        session_id: "deleted".to_string(),
        source_path: Some("/tmp/deleted.jsonl".to_string()),
        last_active_at: Some(2),
        ..crate::session_manager::SessionMeta::default()
    };
    let kept = crate::session_manager::SessionMeta {
        provider_id: "claude".to_string(),
        session_id: "kept".to_string(),
        source_path: Some("/tmp/kept.jsonl".to_string()),
        last_active_at: Some(1),
        ..crate::session_manager::SessionMeta::default()
    };
    let mut initial_builder = store
        .begin_build("claude")
        .expect("initial manifest builder");
    initial_builder
        .push(deleted.clone())
        .expect("deleted fixture row");
    initial_builder
        .push(kept.clone())
        .expect("kept fixture row");
    let initial = initial_builder.publish().expect("publish initial manifest");
    let mut app = App::new(Some(AppType::Claude));
    let _scan = app.sessions.start_scan("claude".to_string());
    let scope_epoch = app.sessions.scope_epoch;
    assert!(app.sessions.apply_opened_manifest(
        scope_epoch,
        "claude",
        initial.generation,
        initial.total_rows,
        initial.first_page.page_index,
        initial.first_page.rows,
        initial.reader,
    ));
    app.sessions.scan_active = None;
    app.sessions.loading = false;

    let delete_request_id = 19;
    let deleted_key = crate::cli::tui::app::session_key(&deleted);
    app.sessions
        .register_purge_tombstone(delete_request_id, deleted_key.clone());
    let mut purged_builder = store
        .begin_build("claude")
        .expect("purged manifest builder");
    purged_builder.push(kept).expect("kept purged row");
    let purged = purged_builder.publish().expect("publish purged manifest");
    assert!(app.sessions.stage_purged_manifest(
        crate::cli::tui::app::SessionPageSource::Base,
        scope_epoch,
        "claude",
        purged.generation,
        purged.total_rows,
        purged.first_page.rows,
        purged.reader,
        delete_request_id,
        deleted_key.clone(),
    ));
    (app, manifest_dir, delete_request_id, deleted_key)
}

struct EnvGuard {
    _lock: TestHomeSettingsLock,
    old_home: Option<OsString>,
    old_userprofile: Option<OsString>,
    old_cc_switch_config_dir: Option<OsString>,
    old_claude_config_dir: Option<OsString>,
    old_codex_home: Option<OsString>,
}

impl EnvGuard {
    fn set_home(home: &Path) -> Self {
        let lock = lock_test_home_and_settings();
        let old_home = std::env::var_os("HOME");
        let old_userprofile = std::env::var_os("USERPROFILE");
        let old_cc_switch_config_dir = std::env::var_os("CC_SWITCH_CONFIG_DIR");
        let old_claude_config_dir = std::env::var_os("CLAUDE_CONFIG_DIR");
        let old_codex_home = std::env::var_os("CODEX_HOME");
        std::env::set_var("HOME", home);
        std::env::set_var("USERPROFILE", home);
        std::env::set_var("CC_SWITCH_CONFIG_DIR", home.join(".cc-switch"));
        std::env::set_var("CLAUDE_CONFIG_DIR", home.join(".claude"));
        std::env::set_var("CODEX_HOME", home.join(".codex"));
        set_test_home_override(Some(home));
        crate::settings::reload_test_settings();
        Self {
            _lock: lock,
            old_home,
            old_userprofile,
            old_cc_switch_config_dir,
            old_claude_config_dir,
            old_codex_home,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.old_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        match &self.old_userprofile {
            Some(value) => std::env::set_var("USERPROFILE", value),
            None => std::env::remove_var("USERPROFILE"),
        }
        match &self.old_cc_switch_config_dir {
            Some(value) => std::env::set_var("CC_SWITCH_CONFIG_DIR", value),
            None => std::env::remove_var("CC_SWITCH_CONFIG_DIR"),
        }
        match &self.old_claude_config_dir {
            Some(value) => std::env::set_var("CLAUDE_CONFIG_DIR", value),
            None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
        }
        match &self.old_codex_home {
            Some(value) => std::env::set_var("CODEX_HOME", value),
            None => std::env::remove_var("CODEX_HOME"),
        }
        set_test_home_override(self.old_home.as_deref().map(Path::new));
        crate::settings::reload_test_settings();
    }
}

#[test]
fn mcp_import_uses_supported_apps_import_and_info_toast_kind() {
    let mut app = App::new(Some(AppType::OpenClaw));
    let mut data = UiData::default();

    import_mcp_from_supported_apps_with(
        &mut app,
        &mut data,
        || Ok(2),
        |_app_type| Ok(UiData::default()),
    )
    .expect("mcp import should work");

    let toast = app.toast.as_ref().expect("mcp import should show toast");
    assert_eq!(toast.kind, ToastKind::Info);
    assert_eq!(toast.message, texts::tui_toast_mcp_imported(2));
}

#[test]
fn tui_tick_rate_returns_to_200ms() {
    assert_eq!(TUI_TICK_RATE, std::time::Duration::from_millis(200));
}

#[test]
fn frame_draw_wait_draws_first_dirty_frame_immediately() {
    assert_eq!(frame_draw_wait(true, None), Some(Duration::ZERO));
    assert_eq!(frame_draw_wait(false, None), None);
}

#[test]
fn frame_draw_wait_caps_repeated_dirty_wakeups() {
    let almost_one_frame = TUI_FRAME_INTERVAL - Duration::from_micros(1);

    assert_eq!(
        frame_draw_wait(true, Some(almost_one_frame)),
        Some(Duration::from_micros(1))
    );
    assert_eq!(
        frame_draw_wait(true, Some(TUI_FRAME_INTERVAL)),
        Some(Duration::ZERO)
    );
    assert_eq!(
        frame_draw_wait(true, Some(TUI_FRAME_INTERVAL + Duration::from_secs(1))),
        Some(Duration::ZERO)
    );
}

#[test]
fn dirty_frame_deadline_shortens_tick_wait_for_single_input() {
    let elapsed = Duration::from_millis(4);
    let frame_wait = frame_draw_wait(true, Some(elapsed));

    assert_eq!(
        next_loop_timeout(TUI_TICK_RATE - elapsed, frame_wait),
        TUI_FRAME_INTERVAL - elapsed
    );
    assert_eq!(next_loop_timeout(TUI_TICK_RATE, None), TUI_TICK_RATE);
}

#[test]
fn frame_scheduler_stays_clean_until_an_update_and_respects_cap() {
    let started = Instant::now();
    let mut scheduler = FrameScheduler::new();

    assert!(scheduler.is_due(started));
    scheduler.record_draw(started);
    assert_eq!(scheduler.wait(started + Duration::from_secs(1)), None);

    scheduler.mark_dirty();
    assert_eq!(
        scheduler.wait(started + Duration::from_millis(4)),
        Some(TUI_FRAME_INTERVAL - Duration::from_millis(4))
    );
    assert!(scheduler.is_due(started + TUI_FRAME_INTERVAL));
}

#[test]
fn app_switch_cache_miss_queues_background_load_without_blocking() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    data.config.common_snippets.codex = Some("codex shared config".to_string());

    let mut cache = UiDataByAppCache::default();
    let (tx, rx) = mpsc::channel();

    cache
        .switch_to(&mut app, &mut data, Some(&tx), AppType::Codex)
        .expect("switch should not synchronously load app data");

    assert_eq!(app.app_type, AppType::Codex);
    assert!(data.providers.rows.is_empty());
    assert_eq!(data.config.common_snippet, "codex shared config");
    assert_eq!(
        cache.pending_by_app.get(&AppType::Codex).copied(),
        Some(pending_snapshot_app_data(1))
    );
    assert!(cache.by_app.contains_key(&AppType::Claude));

    let req = rx.recv().expect("app data request should be queued");
    assert!(matches!(
        req,
        AppDataReq::Load {
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Codex,
        }
    ));
}

#[test]
fn app_data_send_failure_does_not_block_retry() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache::default();
    let (tx, rx) = mpsc::channel();
    drop(rx);

    cache
        .switch_to(&mut app, &mut data, Some(&tx), AppType::Codex)
        .expect("switch should still use loading projection on send failure");

    assert!(!cache.pending_by_app.contains_key(&AppType::Codex));
    assert!(cache.incomplete_by_app.contains(&AppType::Codex));

    let mut back_data = UiData::default();
    cache
        .switch_to(&mut app, &mut back_data, None, AppType::Claude)
        .expect("switch back should work");

    let (retry_tx, retry_rx) = mpsc::channel();
    cache
        .switch_to(&mut app, &mut back_data, Some(&retry_tx), AppType::Codex)
        .expect("retry switch should queue another load");

    assert_eq!(
        cache.pending_by_app.get(&AppType::Codex).copied(),
        Some(pending_snapshot_app_data(2))
    );
    assert!(matches!(
        retry_rx.recv().expect("retry should send request"),
        AppDataReq::Load {
            request_id: 2,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Codex,
        }
    ));
}

#[test]
fn initial_load_preseeds_extra_visible_apps_for_instant_switch() {
    let mut cache = UiDataByAppCache::default();
    let (tx, rx) = mpsc::channel();

    let pending = cache
        .queue_initial_app_data_load(&tx, &AppType::Claude, &[AppType::Codex])
        .expect("queue initial load");

    // Both the active app and every extra visible app get a pending Initial entry.
    assert_eq!(
        cache.pending_by_app.get(&AppType::Claude).copied(),
        Some(pending)
    );
    let codex_pending = cache
        .pending_by_app
        .get(&AppType::Codex)
        .copied()
        .expect("codex should have a pre-seed pending entry");
    assert_eq!(codex_pending.kind, AppDataLoadKind::Initial);

    // A single InitialLoad request carries the extras as (app, request_id) pairs.
    let req = rx.recv().expect("initial request should be queued");
    let extras = match req {
        AppDataReq::InitialLoad {
            app_type: AppType::Claude,
            extras,
            ..
        } => extras,
        other => panic!("unexpected initial request: {other:?}"),
    };
    assert_eq!(extras, vec![(AppType::Codex, codex_pending.request_id)]);

    // Worker reply for the (non-active) Codex snapshot seeds by_app with real rows.
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let mut startup_overlay = Some(Overlay::None);

    let mut codex_data = UiData::default();
    codex_data.providers.rows.push(super::data::ProviderRow {
        id: "codex-default".to_string(),
        provider: crate::provider::Provider::with_id(
            "codex-default".to_string(),
            "Codex Default".to_string(),
            json!({}),
            None,
        ),
        api_url: None,
        is_current: true,
        is_in_config: true,
        is_saved: true,
        is_default_model: false,
        primary_model_id: None,
        default_model_id: None,
    });

    let handled = handle_initial_app_data_msg(
        &mut app,
        &mut data,
        &mut cache,
        &mut startup_overlay,
        None,
        AppDataMsg::Loaded {
            kind: AppDataLoadKind::Initial,
            request_id: codex_pending.request_id,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Codex,
            result: Ok(codex_data),
        },
    )
    .expect("handling the codex pre-seed should succeed");
    assert!(handled);
    assert!(cache.by_app.contains_key(&AppType::Codex));
    assert!(!cache.pending_by_app.contains_key(&AppType::Codex));
    assert!(!cache.incomplete_by_app.contains(&AppType::Codex));

    // Switching to Codex now hits the cache: real providers render immediately,
    // no background load is queued, and the empty placeholder never shows.
    let (switch_tx, switch_rx) = mpsc::channel();
    cache
        .switch_to(&mut app, &mut data, Some(&switch_tx), AppType::Codex)
        .expect("switch to codex should use the pre-seeded cache");
    assert_eq!(app.app_type, AppType::Codex);
    assert!(
        !data.providers.rows.is_empty(),
        "pre-seeded providers should render on the first switch"
    );
    assert!(
        switch_rx.try_recv().is_err(),
        "a cache hit must not queue a background load"
    );
}

#[test]
fn initial_load_extra_app_failure_does_not_abort_startup() {
    let mut cache = UiDataByAppCache::default();
    let (tx, rx) = mpsc::channel();

    cache
        .queue_initial_app_data_load(&tx, &AppType::Claude, &[AppType::Codex])
        .expect("queue initial load");
    let codex_request_id = cache
        .pending_by_app
        .get(&AppType::Codex)
        .copied()
        .expect("codex pending")
        .request_id;
    let _ = rx.recv().expect("initial request");

    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let mut startup_overlay = Some(Overlay::None);

    // A failed pre-seed for a non-active app is swallowed (no Err propagation),
    // leaving it uncached so the first switch falls back to a lazy load.
    let handled = handle_initial_app_data_msg(
        &mut app,
        &mut data,
        &mut cache,
        &mut startup_overlay,
        None,
        AppDataMsg::Loaded {
            kind: AppDataLoadKind::Initial,
            request_id: codex_request_id,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Codex,
            result: Err("boom".to_string()),
        },
    )
    .expect("a non-active pre-seed failure must not abort startup");
    assert!(handled);
    assert!(!cache.by_app.contains_key(&AppType::Codex));
    assert!(!cache.pending_by_app.contains_key(&AppType::Codex));
}

#[test]
fn lightweight_preseed_excludes_additive_apps() {
    // Pure in-memory snapshot apps are eager-seeded at startup.
    for app in [AppType::Claude, AppType::Codex, AppType::Gemini] {
        assert!(
            is_lightweight_preseed_app(&app),
            "{app:?} should be eagerly pre-seeded"
        );
    }
    // Additive apps read live config files even in SnapshotOnly mode, so they
    // lazy-load on first switch instead of paying that I/O at startup.
    for app in [AppType::OpenCode, AppType::Hermes, AppType::OpenClaw] {
        assert!(
            !is_lightweight_preseed_app(&app),
            "{app:?} should lazy-load, not pre-seed"
        );
    }
}

#[test]
fn current_app_reloaded_keeps_other_apps_cache() {
    let app = App::new(Some(AppType::Claude));
    let data = UiData::default();
    let mut cache = UiDataByAppCache::default();

    // A cold app (Codex) is pre-seeded; the active app (Claude) reloads fresh.
    cache.by_app.insert(AppType::Codex, UiData::default());
    cache.handle_data_reloaded(&app, &data, CacheInvalidation::CurrentAppReloaded);
    assert!(
        cache.by_app.contains_key(&AppType::Codex),
        "current-app reload must not wipe other apps' pre-seed"
    );
    assert!(cache.by_app.contains_key(&AppType::Claude));

    // A genuine cross-app invalidation still clears everything (then re-remembers
    // the active app), so stale cross-app data can't linger.
    cache.by_app.insert(AppType::Codex, UiData::default());
    cache.handle_data_reloaded(&app, &data, CacheInvalidation::DataReloaded);
    assert!(!cache.by_app.contains_key(&AppType::Codex));
    assert!(cache.by_app.contains_key(&AppType::Claude));
}

#[test]
fn app_switch_projection_marks_providers_loading() {
    let data = UiData::default();
    let projection = data.app_switch_loading_projection(&AppType::Codex);
    assert!(
        projection.providers.loading,
        "cold-switch projection must flag loading so the empty CTA isn't shown"
    );
    assert!(projection.providers.rows.is_empty());
}

#[test]
fn stale_app_data_result_does_not_overwrite_current_app() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    data.providers.current_id = "claude-current".to_string();

    let mut cache = UiDataByAppCache::default();
    cache
        .pending_by_app
        .insert(AppType::Codex, pending_snapshot_app_data(1));

    let mut loaded = UiData::default();
    loaded.providers.current_id = "codex-loaded".to_string();

    handle_app_data_msg(
        &mut app,
        &mut data,
        &mut cache,
        None,
        None,
        None,
        AppDataMsg::Loaded {
            kind: AppDataLoadKind::Snapshot,
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Codex,
            result: Ok(loaded),
        },
    );

    assert_eq!(app.app_type, AppType::Claude);
    assert_eq!(data.providers.current_id, "claude-current");
    assert_eq!(
        cache
            .by_app
            .get(&AppType::Codex)
            .map(|cached| cached.providers.current_id.as_str()),
        Some("codex-loaded")
    );
}

#[test]
fn app_data_result_preserves_usage_pricing_that_finished_first() {
    let mut app = App::new(Some(AppType::Codex));
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache::default();
    cache
        .pending_by_app
        .insert(AppType::Codex, pending_snapshot_app_data(2));
    cache.pending_usage_pricing_by_key.insert(
        (AppType::Codex, data::UsageRangePreset::SevenDays),
        PendingDataLoad {
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
        },
    );

    let mut usage = data::UsageSnapshot::default();
    usage.summary_7d.total_cost_usd = 12.5;
    let pricing = data::ModelPricingSnapshot {
        rows: vec![data::ModelPricingRow {
            model_id: "gpt-5.4".to_string(),
            display_name: "GPT 5.4".to_string(),
            recent_total_cost_usd: 12.5,
            ..data::ModelPricingRow::default()
        }],
        ..data::ModelPricingSnapshot::default()
    };

    handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::Loaded {
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Codex,
            range: data::UsageRangePreset::SevenDays,
            result: Box::new(Ok(data::UsagePricingData {
                usage,
                pricing: Some(pricing),
            })),
        },
    );

    assert_eq!(data.usage.summary_7d.total_cost_usd, 12.5);
    assert!(
        !cache.by_app.contains_key(&AppType::Codex),
        "pending base data should not be cached as a complete app snapshot"
    );

    let mut loaded = UiData::default();
    loaded.providers.current_id = "codex-base".to_string();
    handle_app_data_msg(
        &mut app,
        &mut data,
        &mut cache,
        None,
        None,
        None,
        AppDataMsg::Loaded {
            kind: AppDataLoadKind::Snapshot,
            request_id: 2,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Codex,
            result: Ok(loaded),
        },
    );

    assert_eq!(data.providers.current_id, "codex-base");
    assert_eq!(data.usage.summary_7d.total_cost_usd, 12.5);
    assert_eq!(data.pricing.rows.len(), 1);
}

#[test]
fn current_app_data_changed_queues_full_load_without_caching_stale_data() {
    let mut app = App::new(Some(AppType::Codex));
    let mut data = UiData::default();
    data.providers.current_id = "stale-current".to_string();
    data.usage.summary_7d.total_cost_usd = 7.5;
    data.pricing.rows.push(data::ModelPricingRow {
        model_id: "gpt-stale".to_string(),
        display_name: "GPT Stale".to_string(),
        recent_total_cost_usd: 7.5,
        ..data::ModelPricingRow::default()
    });

    let mut cached = UiData::default();
    cached.providers.current_id = "cached-stale".to_string();
    let mut cache = UiDataByAppCache::default();
    cache.by_app.insert(AppType::Codex, cached);
    cache
        .pending_by_app
        .insert(AppType::Codex, pending_snapshot_app_data(99));
    cache.incomplete_by_app.insert(AppType::Codex);
    let (tx, rx) = mpsc::channel();

    apply_cache_invalidation(
        &mut app,
        &mut data,
        &mut cache,
        None,
        Some(&tx),
        None,
        CacheInvalidation::CurrentAppDataChanged,
    )
    .expect("current app refresh should be queued");

    assert_eq!(data.providers.current_id, "stale-current");
    assert!(!cache.by_app.contains_key(&AppType::Codex));
    assert_eq!(
        cache.pending_by_app.get(&AppType::Codex).copied(),
        Some(pending_full_app_data(1))
    );
    assert!(cache.incomplete_by_app.contains(&AppType::Codex));
    assert!(!cache
        .usage_pricing_by_key
        .contains_key(&(AppType::Codex, data::UsageRangePreset::SevenDays)));
    assert!(matches!(
        rx.recv().expect("app data request should be queued"),
        AppDataReq::FullLoad {
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Codex,
        }
    ));

    let mut loaded = UiData::default();
    loaded.providers.current_id = "fresh-current".to_string();
    loaded.usage.summary_7d.total_cost_usd = 9.0;
    loaded.pricing.rows.push(data::ModelPricingRow {
        model_id: "gpt-fresh".to_string(),
        display_name: "GPT Fresh".to_string(),
        recent_total_cost_usd: 9.0,
        ..data::ModelPricingRow::default()
    });
    handle_app_data_msg(
        &mut app,
        &mut data,
        &mut cache,
        None,
        None,
        None,
        AppDataMsg::Loaded {
            kind: AppDataLoadKind::Full,
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Codex,
            result: Ok(loaded),
        },
    );

    assert_eq!(data.providers.current_id, "fresh-current");
    assert_eq!(data.usage.summary_7d.total_cost_usd, 9.0);
    assert_eq!(data.pricing.rows.len(), 1);
    assert_eq!(data.pricing.rows[0].model_id, "gpt-fresh");
    assert!(!cache.pending_by_app.contains_key(&AppType::Codex));
    assert!(!cache.incomplete_by_app.contains(&AppType::Codex));
    assert_eq!(
        cache
            .by_app
            .get(&AppType::Codex)
            .map(|cached| cached.providers.current_id.as_str()),
        Some("fresh-current")
    );
}

#[test]
fn current_app_data_changed_full_load_requeues_custom_usage_and_invalidates_old_usage_loads() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let custom_range =
        data::parse_usage_custom_range("2026-06-01..2026-06-05").expect("valid range");
    app.usage.range = data::UsageRangePreset::Custom(custom_range);
    app.usage.start_loading(
        AppType::Claude,
        data::UsageRangePreset::Custom(custom_range),
    );
    data.usage.custom_range = Some(custom_range);
    data.usage.summary_7d.total_requests = 10;
    data.usage.summary_custom.total_requests = 5;
    data.usage.recent_logs.push(data::UsageLogRow {
        request_id: "fixed-log".to_string(),
        ..data::UsageLogRow::default()
    });
    data.usage.logs_total = 10;
    data.usage.recent_logs_custom.push(data::UsageLogRow {
        request_id: "custom-log".to_string(),
        ..data::UsageLogRow::default()
    });
    data.usage.logs_total_custom = 5;

    let mut cache = UiDataByAppCache::default();
    cache.usage_pricing_by_key.insert(
        (AppType::Claude, data::UsageRangePreset::SevenDays),
        data::UsagePricingData {
            usage: data.usage.clone(),
            pricing: Some(data.pricing.clone()),
        },
    );
    cache.pending_usage_pricing_by_key.insert(
        (AppType::Claude, data::UsageRangePreset::SevenDays),
        PendingDataLoad {
            request_id: 3,
            generation: 0,
            app_state_epoch: 0,
        },
    );
    let (app_tx, app_rx) = mpsc::channel();
    let (usage_tx, usage_rx) = mpsc::channel();

    apply_cache_invalidation(
        &mut app,
        &mut data,
        &mut cache,
        None,
        Some(&app_tx),
        None,
        CacheInvalidation::CurrentAppDataChanged,
    )
    .expect("current app refresh should be queued");

    assert!(!cache
        .pending_usage_pricing_by_key
        .contains_key(&(AppType::Claude, data::UsageRangePreset::SevenDays)));
    assert!(!cache
        .usage_pricing_by_key
        .contains_key(&(AppType::Claude, data::UsageRangePreset::SevenDays)));
    assert!(app.usage.is_loading_for(
        &AppType::Claude,
        data::UsageRangePreset::Custom(custom_range)
    ));
    assert!(matches!(
        app_rx.recv().expect("app data request should be queued"),
        AppDataReq::FullLoad { request_id: 1, .. }
    ));

    let mut loaded = UiData::default();
    loaded.providers.current_id = "fresh-current".to_string();
    loaded.usage.summary_7d.total_requests = 11;
    handle_app_data_msg(
        &mut app,
        &mut data,
        &mut cache,
        None,
        None,
        Some(&usage_tx),
        AppDataMsg::Loaded {
            kind: AppDataLoadKind::Full,
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            result: Ok(loaded),
        },
    );

    assert_eq!(data.providers.current_id, "fresh-current");
    assert_eq!(data.usage.summary_7d.total_requests, 11);
    assert_eq!(data.usage.summary_custom.total_requests, 0);
    assert!(data.usage.recent_logs_custom.is_empty());
    assert!(app.usage.is_loading_for(
        &AppType::Claude,
        data::UsageRangePreset::Custom(custom_range)
    ));
    assert!(matches!(
        usage_rx
            .recv()
            .expect("custom usage/pricing request should be queued"),
        UsagePricingReq::Load {
            request_id: 1,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::Custom(range),
            ..
        } if range == custom_range
    ));
}

#[test]
#[serial(home_settings)]
fn current_app_data_changed_falls_back_to_sync_load_when_worker_unavailable() {
    use crate::provider::Provider;
    use crate::services::ProviderService;

    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    let state = data::load_state().expect("load isolated state");
    ProviderService::add(
        &state,
        AppType::Claude,
        Provider::with_id(
            "fresh-provider".to_string(),
            "Fresh Provider".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://fallback.example.com",
                    "ANTHROPIC_AUTH_TOKEN": "test-token"
                }
            }),
            None,
        ),
    )
    .expect("add provider to isolated state");

    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    data.providers.current_id = "stale-current".to_string();
    let mut cache = UiDataByAppCache::default();
    cache
        .pending_by_app
        .insert(AppType::Claude, pending_snapshot_app_data(9));
    cache.incomplete_by_app.insert(AppType::Claude);

    apply_cache_invalidation(
        &mut app,
        &mut data,
        &mut cache,
        None,
        None,
        None,
        CacheInvalidation::CurrentAppDataChanged,
    )
    .expect("fallback reload should succeed");

    assert!(data
        .providers
        .rows
        .iter()
        .any(|row| row.id == "fresh-provider"));
    assert!(!cache.pending_by_app.contains_key(&AppType::Claude));
    assert!(!cache.incomplete_by_app.contains(&AppType::Claude));
    assert_eq!(
        cache
            .by_app
            .get(&AppType::Claude)
            .map(|cached| cached.providers.rows.len()),
        Some(data.providers.rows.len())
    );
}

#[test]
fn app_data_result_after_cache_invalidation_is_ignored() {
    let mut app = App::new(Some(AppType::Codex));
    let mut data = UiData::default();
    data.providers.current_id = "current-after-reload".to_string();
    let mut cache = UiDataByAppCache::default();
    cache
        .pending_by_app
        .insert(AppType::Codex, pending_snapshot_app_data(4));

    cache.handle_data_reloaded(&app, &data, CacheInvalidation::DataReloaded);

    let mut loaded = UiData::default();
    loaded.providers.current_id = "stale-worker-result".to_string();
    handle_app_data_msg(
        &mut app,
        &mut data,
        &mut cache,
        None,
        None,
        None,
        AppDataMsg::Loaded {
            kind: AppDataLoadKind::Snapshot,
            request_id: 4,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Codex,
            result: Ok(loaded),
        },
    );

    assert_eq!(data.providers.current_id, "current-after-reload");
    assert!(cache.pending_by_app.is_empty());
    assert_eq!(cache.data_generation, 1);
}

#[test]
fn usage_sync_invalidation_preserves_unrelated_app_data_load_token() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    data.providers.current_id = "current-after-sync".to_string();
    let mut cache = UiDataByAppCache::default();
    cache
        .pending_by_app
        .insert(AppType::Claude, pending_full_app_data(2));
    cache.incomplete_by_app.insert(AppType::Claude);
    cache.invalidate_usage_pricing_cache_after_external_usage_sync();
    let (tx, rx) = mpsc::channel();

    let mut loaded = UiData::default();
    loaded.providers.current_id = "stale-full-load".to_string();
    handle_app_data_msg(
        &mut app,
        &mut data,
        &mut cache,
        None,
        Some(&tx),
        None,
        AppDataMsg::Loaded {
            kind: AppDataLoadKind::Full,
            request_id: 2,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            result: Ok(loaded),
        },
    );

    assert_eq!(data.providers.current_id, "stale-full-load");
    assert!(!cache.incomplete_by_app.contains(&AppType::Claude));
    assert!(cache.pending_by_app.is_empty());
    assert_eq!(cache.data_generation, 0);
    assert_eq!(cache.app_state_epoch, 0);
    assert!(rx.try_recv().is_err());
}

#[test]
fn no_op_reload_candidate_preserves_pending_app_data_load() {
    let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache::default();
    let mut proxy_loading = RequestTracker::default();
    let mut webdav_loading = RequestTracker::default();
    let mut update_check = RequestTracker::default();
    cache
        .pending_by_app
        .insert(AppType::Codex, pending_snapshot_app_data(7));

    handle_tui_action(
        &mut terminal,
        &mut app,
        &mut data,
        &mut cache,
        None,
        None,
        None,
        None,
        None,
        &mut proxy_loading,
        None,
        None,
        None,
        &mut webdav_loading,
        None,
        &mut update_check,
        None,
        None,
        None,
        None,
        None,
        Action::EditorSubmit {
            submit: EditorSubmit::ProviderAdd,
            content: "{".to_string(),
        },
    )
    .expect("invalid submit should be handled as a no-op");

    assert_eq!(
        cache.pending_by_app.get(&AppType::Codex).copied(),
        Some(pending_snapshot_app_data(7))
    );
    assert_eq!(cache.data_generation, 0);
}

#[test]
fn switch_to_sessions_queues_scan_without_waiting_for_next_tick() {
    let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
    let mut app = App::new(Some(AppType::Codex));
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache::default();
    let mut proxy_loading = RequestTracker::default();
    let mut webdav_loading = RequestTracker::default();
    let mut update_check = RequestTracker::default();
    let (tx, rx) = mpsc::channel();

    handle_tui_action(
        &mut terminal,
        &mut app,
        &mut data,
        &mut cache,
        None,
        None,
        None,
        None,
        None,
        &mut proxy_loading,
        None,
        Some(&tx),
        None,
        &mut webdav_loading,
        None,
        &mut update_check,
        None,
        None,
        None,
        None,
        None,
        Action::SwitchRoute(route::Route::Sessions),
    )
    .expect("switching to sessions should queue a scan");

    assert!(matches!(app.route, route::Route::Sessions));
    assert!(app.sessions.loading);
    assert_eq!(app.sessions.provider_id.as_deref(), Some("codex"));
    let request_id = app.sessions.scan_active.expect("scan should be active");
    match rx.try_recv().expect("scan request should be queued") {
        SessionReq::Refresh {
            request_id: queued_request_id,
            provider_id,
            ..
        } => {
            assert_eq!(queued_request_id, request_id);
            assert_eq!(provider_id, "codex");
        }
        other => panic!("unexpected sessions request: {other:?}"),
    }

    handle_tui_action(
        &mut terminal,
        &mut app,
        &mut data,
        &mut cache,
        None,
        None,
        None,
        None,
        None,
        &mut proxy_loading,
        None,
        Some(&tx),
        None,
        &mut webdav_loading,
        None,
        &mut update_check,
        None,
        None,
        None,
        None,
        None,
        Action::SwitchRoute(route::Route::Sessions),
    )
    .expect("switching to an already-loading sessions route should not queue another scan");

    assert!(rx.try_recv().is_err());
}

#[test]
fn failed_automatic_sessions_scan_is_not_retried_each_event_loop() {
    let mut app = App::new(Some(AppType::Claude));
    app.route = route::Route::Sessions;
    let (tx, rx) = mpsc::channel();

    queue_sessions_refresh_if_needed(&mut app, Some(&tx));
    let request_id = match rx.recv().expect("initial automatic refresh") {
        SessionReq::Refresh { request_id, .. } => request_id,
        other => panic!("unexpected sessions request: {other:?}"),
    };
    app.sessions
        .fail_scan(request_id, "injected disk-full failure".to_string());

    for _ in 0..20 {
        queue_sessions_refresh_if_needed(&mut app, Some(&tx));
    }

    assert!(rx.try_recv().is_err());
    assert_eq!(app.sessions.scan_seq, 1);
    assert_eq!(
        app.sessions.last_error.as_deref(),
        Some("injected disk-full failure")
    );
}

#[test]
fn returning_to_sessions_rebuilds_a_cancelled_project_view_from_the_saved_base() {
    let manifest_dir = tempfile::tempdir().expect("manifest fixture directory");
    let store =
        crate::session_manager::paged_manifest::PagedManifestStore::open_at(manifest_dir.path())
            .expect("manifest fixture store");
    let mut builder = store.begin_build("claude").expect("manifest builder");
    builder
        .push(crate::session_manager::SessionMeta {
            provider_id: "claude".to_string(),
            session_id: "alpha".to_string(),
            project_dir: Some("/repo/alpha".to_string()),
            source_path: Some("/tmp/alpha.jsonl".to_string()),
            ..crate::session_manager::SessionMeta::default()
        })
        .expect("manifest row");
    let published = builder.publish().expect("publish manifest");

    let mut app = App::new(Some(AppType::Claude));
    app.route = route::Route::Sessions;
    let _ = app.sessions.start_scan("claude".to_string());
    let scope_epoch = app.sessions.scope_epoch;
    assert!(app.sessions.remember_base_manifest(
        scope_epoch,
        "claude",
        published.generation,
        published.total_rows,
        published.reader,
    ));
    app.sessions.scan_active = None;
    app.sessions.loading = false;
    app.sessions.project_scope =
        crate::session_manager::project_scope::SessionProjectScope::exact("/repo/alpha")
            .expect("exact project");
    let (tx, rx) = mpsc::channel();

    queue_sessions_refresh_if_needed(&mut app, Some(&tx));

    assert_eq!(app.pending_deep_search.as_deref(), Some(""));
    assert!(rx.try_recv().is_err(), "saved base avoids a redundant scan");
    let query = app.pending_deep_search.take().expect("rebuild queued");
    runtime_actions::queue_sessions_deep_search(&mut app, Some(&tx), query);
    assert!(matches!(
        rx.try_recv().expect("project view request"),
        SessionReq::Search { view, .. }
            if view.project == app.sessions.project_scope && view.query.is_empty()
    ));
}

#[test]
fn failed_session_view_is_not_requeued_on_each_event_loop() {
    let manifest_dir = tempfile::tempdir().expect("manifest fixture directory");
    let store =
        crate::session_manager::paged_manifest::PagedManifestStore::open_at(manifest_dir.path())
            .expect("manifest fixture store");
    let mut builder = store.begin_build("claude").expect("manifest builder");
    builder
        .push(crate::session_manager::SessionMeta {
            provider_id: "claude".to_string(),
            session_id: "alpha".to_string(),
            source_path: Some("/tmp/alpha.jsonl".to_string()),
            ..crate::session_manager::SessionMeta::default()
        })
        .expect("manifest row");
    let published = builder.publish().expect("publish manifest");

    let mut app = App::new(Some(AppType::Claude));
    app.route = route::Route::Sessions;
    let _ = app.sessions.start_scan("claude".to_string());
    let scope_epoch = app.sessions.scope_epoch;
    let base_generation = published.generation.clone();
    assert!(app.sessions.remember_base_manifest(
        scope_epoch,
        "claude",
        published.generation,
        published.total_rows,
        published.reader,
    ));
    app.sessions.scan_active = None;
    app.sessions.loading = false;
    app.filter.input.set("needle");
    let view = app.sessions.desired_view_spec(Some("needle"));
    app.sessions
        .mark_materialization_failed(scope_epoch, "claude", &base_generation, view);
    let (tx, rx) = mpsc::channel();

    for _ in 0..20 {
        queue_sessions_refresh_if_needed(&mut app, Some(&tx));
    }

    assert!(app.pending_deep_search.is_none());
    assert!(rx.try_recv().is_err());
}

#[test]
fn unavailable_sessions_worker_is_reported_once_per_scope() {
    let mut app = App::new(Some(AppType::Claude));
    app.route = route::Route::Sessions;

    for _ in 0..20 {
        queue_sessions_refresh_if_needed(&mut app, None);
    }

    assert_eq!(app.sessions.scan_seq, 1);
    assert!(!app.sessions.loading);
    assert_eq!(
        app.sessions.last_error.as_deref(),
        Some("sessions worker is not running")
    );
}

#[test]
fn missing_manifest_reconcile_worker_preserves_tombstone_convergence() {
    let (mut app, _manifest_dir, delete_request_id, deleted_key) =
        app_with_pending_purge_manifest();

    maybe_queue_session_manifest_reconcile(&mut app, None);

    assert!(app.sessions.pending_manifest.is_none());
    assert!(app.sessions.purge_refresh_required());
    assert!(app.sessions.purge_tombstone_applies_to_scope(
        delete_request_id,
        &deleted_key,
        "claude"
    ));
    assert_eq!(
        app.sessions.last_error.as_deref(),
        Some("sessions worker is not running")
    );
}

#[test]
fn disconnected_manifest_reconcile_worker_preserves_tombstone_convergence() {
    let (mut app, _manifest_dir, delete_request_id, deleted_key) =
        app_with_pending_purge_manifest();
    let (tx, rx) = mpsc::channel();
    drop(rx);

    maybe_queue_session_manifest_reconcile(&mut app, Some(&tx));

    assert!(app.sessions.pending_manifest.is_none());
    assert!(app.sessions.purge_refresh_required());
    assert!(app.sessions.purge_tombstone_applies_to_scope(
        delete_request_id,
        &deleted_key,
        "claude"
    ));
    assert!(app.sessions.last_error.is_some());
}

#[test]
fn switching_app_on_sessions_route_queues_scan_for_next_app() {
    let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
    let mut app = App::new(Some(AppType::Claude));
    app.route = route::Route::Sessions;
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache::default();
    cache.by_app.insert(AppType::Codex, UiData::default());
    let mut proxy_loading = RequestTracker::default();
    let mut webdav_loading = RequestTracker::default();
    let mut update_check = RequestTracker::default();
    let (session_tx, session_rx) = mpsc::channel();

    handle_tui_action(
        &mut terminal,
        &mut app,
        &mut data,
        &mut cache,
        None,
        None,
        None,
        None,
        None,
        &mut proxy_loading,
        None,
        Some(&session_tx),
        None,
        &mut webdav_loading,
        None,
        &mut update_check,
        None,
        None,
        None,
        None,
        None,
        Action::SetAppType(AppType::Codex),
    )
    .expect("switching app on sessions route should queue a scan");

    assert_eq!(app.app_type, AppType::Codex);
    assert!(matches!(app.route, route::Route::Sessions));
    assert!(app.sessions.loading);
    assert_eq!(app.sessions.provider_id.as_deref(), Some("codex"));
    let request_id = app.sessions.scan_active.expect("scan should be active");
    assert!(matches!(
        session_rx
            .try_recv()
            .expect("app switch should cancel the old search lane"),
        SessionReq::CancelSearch
    ));
    assert!(matches!(
        session_rx
            .try_recv()
            .expect("app switch should cancel the old message lane"),
        SessionReq::CancelMessages
    ));
    assert!(matches!(
        session_rx
            .try_recv()
            .expect("app switch should cancel the old project catalog lane"),
        SessionReq::CancelProjectCatalog
    ));
    assert!(matches!(
        session_rx
            .try_recv()
            .expect("app switch should cancel the old project filter lane"),
        SessionReq::CancelProjectFilter
    ));
    match session_rx
        .try_recv()
        .expect("scan request should be queued")
    {
        SessionReq::Refresh {
            request_id: queued_request_id,
            provider_id,
            ..
        } => {
            assert_eq!(queued_request_id, request_id);
            assert_eq!(provider_id, "codex");
        }
        other => panic!("unexpected sessions request: {other:?}"),
    }
}

#[test]
fn initial_app_data_result_restores_startup_overlay_and_caches_loaded_data() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache::default();
    cache.pending_by_app.insert(
        AppType::Claude,
        PendingAppDataLoad {
            kind: AppDataLoadKind::Initial,
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
        },
    );
    cache.incomplete_by_app.insert(AppType::Claude);
    app.overlay = Overlay::Confirm(ConfirmOverlay {
        title: "Visible apps".to_string(),
        message: "Review detected apps".to_string(),
        action: ConfirmAction::VisibleAppsAutoDetection,
    });
    let mut startup_overlay = Some(Overlay::Confirm(ConfirmOverlay {
        title: "Visible apps".to_string(),
        message: "Review detected apps".to_string(),
        action: ConfirmAction::VisibleAppsAutoDetection,
    }));

    let mut loaded = UiData::default();
    loaded.providers.current_id = "loaded-current".to_string();
    loaded.proxy.running = true;
    loaded.proxy.estimated_input_tokens_total = 10;
    loaded.proxy.estimated_output_tokens_total = 20;

    let handled = handle_initial_app_data_msg(
        &mut app,
        &mut data,
        &mut cache,
        &mut startup_overlay,
        None,
        AppDataMsg::Loaded {
            kind: AppDataLoadKind::Initial,
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            result: Ok(loaded),
        },
    )
    .expect("initial app data result should be handled");

    assert!(handled);
    assert_eq!(data.providers.current_id, "loaded-current");
    assert_eq!(
        cache
            .by_app
            .get(&AppType::Claude)
            .map(|cached| cached.providers.current_id.as_str()),
        Some("loaded-current")
    );
    assert!(cache.pending_by_app.is_empty());
    assert!(!cache.incomplete_by_app.contains(&AppType::Claude));
    assert!(startup_overlay.is_none());
    assert!(matches!(
        app.overlay,
        Overlay::Confirm(ConfirmOverlay {
            action: ConfirmAction::VisibleAppsAutoDetection,
            ..
        })
    ));
    assert_eq!(app.proxy_visual_state, Some(true));
    assert!(app.proxy_visual_transition.is_none());
    assert_eq!(app.proxy_activity_last_input_tokens, Some(10));
    assert_eq!(app.proxy_activity_last_output_tokens, Some(20));
}

#[test]
fn initial_app_data_error_returns_before_empty_shell_is_marked_loaded() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    data.providers.current_id = "empty-shell".to_string();
    let mut cache = UiDataByAppCache::default();
    cache.pending_by_app.insert(
        AppType::Claude,
        PendingAppDataLoad {
            kind: AppDataLoadKind::Initial,
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
        },
    );
    cache.incomplete_by_app.insert(AppType::Claude);
    let mut startup_overlay = None;

    let err = handle_initial_app_data_msg(
        &mut app,
        &mut data,
        &mut cache,
        &mut startup_overlay,
        None,
        AppDataMsg::Loaded {
            kind: AppDataLoadKind::Initial,
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            result: Err("boom".to_string()),
        },
    )
    .expect_err("initial load failure should be returned");

    assert_eq!(err.to_string(), "boom");
    assert_eq!(data.providers.current_id, "empty-shell");
    assert!(cache.by_app.is_empty());
    assert!(cache.incomplete_by_app.contains(&AppType::Claude));
}

#[test]
fn initial_app_data_drain_prioritizes_error_before_quit_input() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache::default();
    cache.pending_by_app.insert(
        AppType::Claude,
        PendingAppDataLoad {
            kind: AppDataLoadKind::Initial,
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
        },
    );
    cache.incomplete_by_app.insert(AppType::Claude);
    let mut startup_overlay = None;
    let (_tx, rx) = mpsc::channel();
    _tx.send(AppDataMsg::Loaded {
        kind: AppDataLoadKind::Initial,
        request_id: 1,
        generation: 0,
        app_state_epoch: 0,
        app_type: AppType::Claude,
        result: Err("boom".to_string()),
    })
    .expect("queue initial load error");

    let quit_key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    assert!(is_initial_loading_quit_key(&quit_key));
    let err = drain_initial_app_data_messages(
        &mut app,
        &mut data,
        &mut cache,
        &mut startup_overlay,
        None,
        &rx,
    )
    .expect_err("initial load error should win over quit input");

    assert_eq!(err.to_string(), "boom");
    assert!(cache.by_app.is_empty());
    assert!(cache.incomplete_by_app.contains(&AppType::Claude));
}

#[test]
fn initial_loading_input_polling_stops_after_success_or_failure() {
    assert!(should_poll_initial_loading_input(true, false));
    assert!(!should_poll_initial_loading_input(false, false));
    assert!(!should_poll_initial_loading_input(true, true));
    assert!(!should_poll_initial_loading_input(false, true));
}

#[test]
fn initial_loading_quit_waits_for_success_and_never_hides_error() {
    assert!(!should_exit_after_initial_loading(true, false, true));
    assert!(!should_exit_after_initial_loading(false, true, true));
    assert!(!should_exit_after_initial_loading(false, false, false));
    assert!(should_exit_after_initial_loading(false, false, true));
}

#[test]
fn initial_loading_only_accepts_quit_keys() {
    assert!(is_initial_loading_quit_key(&KeyEvent::new(
        KeyCode::Char('q'),
        KeyModifiers::NONE,
    )));
    assert!(is_initial_loading_quit_key(&KeyEvent::new(
        KeyCode::Char('Q'),
        KeyModifiers::NONE,
    )));
    assert!(is_initial_loading_quit_key(&KeyEvent::new(
        KeyCode::Esc,
        KeyModifiers::NONE,
    )));
    assert!(is_initial_loading_quit_key(&KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));
    assert!(!is_initial_loading_quit_key(&KeyEvent::new(
        KeyCode::Char('1'),
        KeyModifiers::NONE,
    )));
    assert!(!is_initial_loading_quit_key(&KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));
    assert!(!is_initial_loading_quit_key(&KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::NONE,
    )));
}

#[test]
fn initial_loading_event_quit_detection_only_uses_pressed_quit_keys() {
    let pressed_quit = event::Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(initial_loading_event_requests_quit(&pressed_quit));

    let mut released_quit = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    released_quit.kind = KeyEventKind::Release;
    assert!(!initial_loading_event_requests_quit(&event::Event::Key(
        released_quit
    )));

    assert!(!initial_loading_event_requests_quit(&event::Event::Mouse(
        event::MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
    )));
}

#[test]
fn initial_loading_quit_recording_ignores_non_quit_events() {
    let mut quit_requested = false;

    record_initial_loading_quit_event(
        &mut quit_requested,
        &event::Event::Key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE)),
    );
    assert!(!quit_requested);

    record_initial_loading_quit_event(
        &mut quit_requested,
        &event::Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
    );
    assert!(quit_requested);
}

#[test]
fn usage_pricing_results_are_tracked_per_app() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache::default();
    let (tx, rx) = mpsc::channel();

    cache.queue_usage_pricing_load(
        &mut app,
        Some(&tx),
        &AppType::Claude,
        data::UsageRangePreset::SevenDays,
    );
    cache.queue_usage_pricing_load(
        &mut app,
        Some(&tx),
        &AppType::Codex,
        data::UsageRangePreset::SevenDays,
    );

    let requests = [rx.recv().unwrap(), rx.recv().unwrap()];
    assert!(requests.iter().any(|req| matches!(
        req,
        UsagePricingReq::Load {
            request_id: 1,
            app_type: AppType::Claude,
            ..
        }
    )));
    assert!(requests.iter().any(|req| matches!(
        req,
        UsagePricingReq::Load {
            request_id: 2,
            app_type: AppType::Codex,
            ..
        }
    )));

    let mut claude_usage = data::UsageSnapshot::default();
    claude_usage.summary_7d.total_cost_usd = 1.0;
    let mut codex_usage = data::UsageSnapshot::default();
    codex_usage.summary_7d.total_cost_usd = 2.0;

    handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::Loaded {
            request_id: 2,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Codex,
            range: data::UsageRangePreset::SevenDays,
            result: Box::new(Ok(data::UsagePricingData {
                usage: codex_usage,
                pricing: Some(data::ModelPricingSnapshot::default()),
            })),
        },
    );
    handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::Loaded {
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::SevenDays,
            result: Box::new(Ok(data::UsagePricingData {
                usage: claude_usage,
                pricing: Some(data::ModelPricingSnapshot::default()),
            })),
        },
    );

    assert_eq!(data.usage.summary_7d.total_cost_usd, 1.0);
    assert_eq!(
        cache
            .usage_pricing_by_key
            .get(&(AppType::Codex, data::UsageRangePreset::SevenDays))
            .map(|usage_pricing| usage_pricing.usage.summary_7d.total_cost_usd),
        Some(2.0)
    );
}

#[test]
fn usage_log_head_is_visible_while_aggregate_load_remains_pending() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache::default();
    let pending = PendingDataLoad {
        request_id: 7,
        generation: 3,
        app_state_epoch: 2,
    };
    cache.pending_usage_pricing_by_key.insert(
        (AppType::Claude, data::UsageRangePreset::SevenDays),
        pending,
    );
    cache.usage_pricing_phase_by_key.insert(
        (AppType::Claude, data::UsageRangePreset::SevenDays),
        UsagePricingLoadPhase::new(pending),
    );

    let rows = (0..data::USAGE_LOG_PAGE_SIZE)
        .map(|index| data::UsageLogRow {
            request_id: format!("head-{index}"),
            ..data::UsageLogRow::default()
        })
        .collect();
    handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::LogHeadLoaded {
            request_id: 7,
            generation: 3,
            app_state_epoch: 2,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::SevenDays,
            result: Ok(data::UsageLogPage {
                rows,
                has_more: true,
                ..data::UsageLogPage::default()
            }),
        },
    );

    assert_eq!(
        data.usage
            .recent_logs_for(data::UsageRangePreset::SevenDays)
            .len(),
        data::USAGE_LOG_PAGE_SIZE
    );
    assert_eq!(
        data.usage.logs_total_for(data::UsageRangePreset::SevenDays),
        data::USAGE_LOG_PAGE_SIZE as u64 + 1
    );
    assert!(cache
        .pending_usage_pricing_by_key
        .contains_key(&(AppType::Claude, data::UsageRangePreset::SevenDays)));

    let mut aggregate = data::UsagePricingData::default();
    aggregate.usage.logs_total = 432;
    handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::Loaded {
            request_id: pending.request_id,
            generation: pending.generation,
            app_state_epoch: pending.app_state_epoch,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::SevenDays,
            result: Box::new(Ok(aggregate)),
        },
    );

    assert_eq!(data.usage.logs_total, 432);
    assert!(cache
        .usage_pricing_phase_by_key
        .get(&(AppType::Claude, data::UsageRangePreset::SevenDays))
        .is_some_and(|phase| {
            phase.aggregate_finished && phase.aggregate_succeeded && phase.head_finished
        }));
}

#[test]
fn usage_log_head_can_arrive_after_its_aggregate_without_being_dropped() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache::default();
    let pending = PendingDataLoad {
        request_id: 11,
        generation: 5,
        app_state_epoch: 3,
    };
    let key = (AppType::Claude, data::UsageRangePreset::SevenDays);
    cache
        .pending_usage_pricing_by_key
        .insert(key.clone(), pending);
    cache
        .usage_pricing_phase_by_key
        .insert(key.clone(), UsagePricingLoadPhase::new(pending));

    let mut aggregate = data::UsagePricingData::default();
    aggregate.usage.logs_total = 987;

    handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::Loaded {
            request_id: pending.request_id,
            generation: pending.generation,
            app_state_epoch: pending.app_state_epoch,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::SevenDays,
            result: Box::new(Ok(aggregate)),
        },
    );
    assert!(!cache.pending_usage_pricing_by_key.contains_key(&key));

    handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::LogHeadLoaded {
            request_id: pending.request_id,
            generation: pending.generation,
            app_state_epoch: pending.app_state_epoch,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::SevenDays,
            result: Ok(data::UsageLogPage {
                rows: vec![data::UsageLogRow {
                    request_id: "late-head".to_string(),
                    ..data::UsageLogRow::default()
                }],
                ..data::UsageLogPage::default()
            }),
        },
    );

    assert_eq!(data.usage.recent_logs[0].request_id, "late-head");
    assert_eq!(data.usage.logs_total, 987);
    assert!(cache
        .usage_pricing_phase_by_key
        .get(&key)
        .is_some_and(|phase| {
            phase.aggregate_finished && phase.aggregate_succeeded && phase.head_finished
        }));
}

#[test]
fn usage_log_detail_refresh_replaces_on_success_and_keeps_old_on_failure() {
    let mut app = App::new(Some(AppType::Claude));
    app.route = route::Route::UsageLogDetail { rowid: 44 };
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache::default();
    app.usage.remember_log_detail(
        AppType::Claude,
        data::UsageRangePreset::SevenDays,
        data::UsageLogRow {
            request_id: "page-two".to_string(),
            model: "old".to_string(),
            cursor_rowid: 44,
            ..data::UsageLogRow::default()
        },
    );

    app.usage.start_log_detail_refresh(20);
    app.usage.start_log_detail_refresh(21);
    handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::LogDetailLoaded {
            request_id: 20,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::SevenDays,
            log_rowid: 44,
            result: Ok(Some(data::UsageLogRow {
                request_id: "page-two".to_string(),
                model: "stale".to_string(),
                cursor_rowid: 44,
                ..data::UsageLogRow::default()
            })),
        },
    );
    assert_eq!(
        app.usage
            .log_detail_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.row_for(
                &AppType::Claude,
                data::UsageRangePreset::SevenDays,
                44,
            ))
            .map(|row| row.model.as_str()),
        Some("old")
    );

    handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::LogDetailLoaded {
            request_id: 21,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::SevenDays,
            log_rowid: 44,
            result: Ok(Some(data::UsageLogRow {
                request_id: "page-two".to_string(),
                model: "fresh".to_string(),
                cursor_rowid: 44,
                ..data::UsageLogRow::default()
            })),
        },
    );
    assert_eq!(
        app.usage
            .log_detail_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.row_for(
                &AppType::Claude,
                data::UsageRangePreset::SevenDays,
                44,
            ))
            .map(|row| row.model.as_str()),
        Some("fresh")
    );

    app.usage.start_log_detail_refresh(22);
    handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::LogDetailLoaded {
            request_id: 22,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::SevenDays,
            log_rowid: 44,
            result: Ok(None),
        },
    );
    assert_eq!(
        app.usage
            .log_detail_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.row_for(
                &AppType::Claude,
                data::UsageRangePreset::SevenDays,
                44,
            ))
            .map(|row| row.model.as_str()),
        Some("fresh")
    );
}

#[test]
fn usage_pricing_load_updates_non_blocking_loading_state() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache::default();
    let (tx, rx) = mpsc::channel();

    cache.queue_usage_pricing_load(
        &mut app,
        Some(&tx),
        &AppType::Claude,
        data::UsageRangePreset::SevenDays,
    );

    assert!(app
        .usage
        .is_loading_for(&AppType::Claude, data::UsageRangePreset::Today));
    assert!(app
        .usage
        .is_loading_for(&AppType::Claude, data::UsageRangePreset::SevenDays));
    assert!(!app
        .usage
        .is_loading_for(&AppType::Codex, data::UsageRangePreset::SevenDays));
    assert!(matches!(
        rx.recv().expect("usage/pricing request should be queued"),
        UsagePricingReq::Load {
            request_id: 1,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::SevenDays,
            ..
        }
    ));

    handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::Loaded {
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::SevenDays,
            result: Box::new(Ok(data::UsagePricingData::default())),
        },
    );

    assert!(!app
        .usage
        .is_loading_for(&AppType::Claude, data::UsageRangePreset::SevenDays));
}

#[test]
fn fixed_usage_ranges_share_one_pending_request_and_canonical_result() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache::default();
    let (tx, rx) = mpsc::channel();

    cache.queue_usage_pricing_load(
        &mut app,
        Some(&tx),
        &AppType::Claude,
        data::UsageRangePreset::Today,
    );
    cache.queue_usage_pricing_load(
        &mut app,
        Some(&tx),
        &AppType::Claude,
        data::UsageRangePreset::ThirtyDays,
    );

    let request = rx.recv().expect("one canonical fixed request");
    assert!(matches!(
        request,
        UsagePricingReq::Load {
            request_id: 1,
            range: data::UsageRangePreset::SevenDays,
            ..
        }
    ));
    assert!(rx.try_recv().is_err());
    assert_eq!(cache.pending_usage_pricing_by_key.len(), 1);
    assert!(cache
        .pending_usage_pricing_by_key
        .contains_key(&(AppType::Claude, data::UsageRangePreset::SevenDays)));

    handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::Loaded {
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::SevenDays,
            result: Box::new(Ok(data::UsagePricingData::default())),
        },
    );

    assert!(!app
        .usage
        .is_loading_for(&AppType::Claude, data::UsageRangePreset::ThirtyDays));
}

#[test]
fn background_session_usage_sync_queues_once() {
    let (tx, rx) = mpsc::channel();
    let mut tracker = RequestTracker::default();

    queue_background_session_usage_sync(Some(&tx), &mut tracker);
    queue_background_session_usage_sync(Some(&tx), &mut tracker);

    assert_eq!(tracker.active, Some(1));
    assert!(matches!(
        rx.recv()
            .expect("session usage sync request should be queued"),
        SessionUsageSyncReq::Run { request_id: 1 }
    ));
    assert!(rx.try_recv().is_err());
}

#[test]
fn manual_usage_refresh_syncs_sessions_before_requerying() {
    let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
    let mut app = App::new(Some(AppType::Claude));
    app.route = route::Route::Usage;
    let mut data = UiData::default();
    data.usage.summary_7d.total_requests = 42;
    let mut cache = UiDataByAppCache::default();
    let mut proxy_loading = RequestTracker::default();
    let mut webdav_loading = RequestTracker::default();
    let mut update_check = RequestTracker::default();
    let mut session_usage_sync = RequestTracker::default();
    let (usage_tx, usage_rx) = mpsc::channel();
    let (sync_tx, sync_rx) = mpsc::channel();

    handle_tui_action(
        &mut terminal,
        &mut app,
        &mut data,
        &mut cache,
        None,
        None,
        None,
        None,
        None,
        &mut proxy_loading,
        None,
        None,
        None,
        &mut webdav_loading,
        None,
        &mut update_check,
        None,
        None,
        None,
        Some(&usage_tx),
        Some((&sync_tx, &mut session_usage_sync)),
        Action::UsageRefresh,
    )
    .expect("manual usage refresh should queue session sync");

    assert!(matches!(
        sync_rx.recv().expect("session sync should be queued first"),
        SessionUsageSyncReq::Run { request_id: 1 }
    ));
    assert!(usage_rx.try_recv().is_err());
    assert_eq!(data.usage.summary_7d.total_requests, 42);
    assert!(app.usage.manual_session_refreshing());

    handle_tui_action(
        &mut terminal,
        &mut app,
        &mut data,
        &mut cache,
        None,
        None,
        None,
        None,
        None,
        &mut proxy_loading,
        None,
        None,
        None,
        &mut webdav_loading,
        None,
        &mut update_check,
        None,
        None,
        None,
        Some(&usage_tx),
        Some((&sync_tx, &mut session_usage_sync)),
        Action::UsageRefresh,
    )
    .expect("duplicate manual refresh should reuse session sync");
    assert!(sync_rx.try_recv().is_err());
    assert!(usage_rx.try_recv().is_err());

    handle_session_usage_sync_msg(
        &mut app,
        &mut data,
        &mut cache,
        &mut session_usage_sync,
        Some(&usage_tx),
        SessionUsageSyncMsg::Finished {
            request_id: 1,
            result: Ok(()),
        },
    );

    assert!(!app.usage.manual_session_refreshing());

    assert!(matches!(
        usage_rx
            .recv()
            .expect("usage query should follow completed session sync"),
        UsagePricingReq::Load {
            request_id: 1,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::SevenDays,
            ..
        }
    ));
    assert!(usage_rx.try_recv().is_err());
    assert_eq!(data.usage.summary_7d.total_requests, 42);
}

#[test]
fn pricing_manual_refresh_uses_fixed_async_query_without_session_sync() {
    let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
    let mut app = App::new(Some(AppType::Claude));
    app.route = route::Route::Pricing;
    app.usage.range = data::UsageRangePreset::Custom(
        data::parse_usage_custom_range("2026-06-01..2026-06-05").expect("valid custom range"),
    );
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache::default();
    let mut proxy_loading = RequestTracker::default();
    let mut webdav_loading = RequestTracker::default();
    let mut update_check = RequestTracker::default();
    let mut session_usage_sync = RequestTracker::default();
    let (usage_tx, usage_rx) = mpsc::channel();
    let (sync_tx, sync_rx) = mpsc::channel();

    handle_tui_action(
        &mut terminal,
        &mut app,
        &mut data,
        &mut cache,
        None,
        None,
        None,
        None,
        None,
        &mut proxy_loading,
        None,
        None,
        None,
        &mut webdav_loading,
        None,
        &mut update_check,
        None,
        None,
        None,
        Some(&usage_tx),
        Some((&sync_tx, &mut session_usage_sync)),
        Action::UsageRefresh,
    )
    .expect("pricing refresh should use the usage worker");

    assert!(sync_rx.try_recv().is_err());
    assert_eq!(session_usage_sync.active, None);
    assert!(matches!(
        usage_rx
            .recv()
            .expect("fixed pricing query should be queued"),
        UsagePricingReq::Load {
            app_type: AppType::Claude,
            range: data::UsageRangePreset::SevenDays,
            ..
        }
    ));
}

#[test]
fn background_session_usage_sync_queues_final_refresh_without_new_epoch() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    data.usage.summary_7d.total_cost_usd = 3.0;
    app.usage
        .start_loading(AppType::Codex, data::UsageRangePreset::SevenDays);
    let mut cache = UiDataByAppCache::default();
    let mut cached_codex = UiData::default();
    cached_codex.usage.summary_7d.total_cost_usd = 9.0;
    cached_codex.pricing.rows.push(data::ModelPricingRow {
        model_id: "stale-model".to_string(),
        ..data::ModelPricingRow::default()
    });
    cache.by_app.insert(AppType::Codex, cached_codex);
    let mut tracker = RequestTracker::default();
    let request_id = tracker.start();
    let (tx, rx) = mpsc::channel();

    handle_session_usage_sync_msg(
        &mut app,
        &mut data,
        &mut cache,
        &mut tracker,
        Some(&tx),
        SessionUsageSyncMsg::Finished {
            request_id,
            result: Ok(()),
        },
    );

    assert_eq!(tracker.active, None);
    assert_eq!(cache.data_generation, 0);
    assert_eq!(cache.app_state_epoch, 0);
    assert_eq!(data.usage.summary_7d.total_cost_usd, 3.0);
    assert!(app
        .usage
        .is_loading_for(&AppType::Codex, data::UsageRangePreset::SevenDays));
    let cached_codex = cache
        .by_app
        .get(&AppType::Codex)
        .expect("non-current app snapshot should remain cached");
    assert_eq!(cached_codex.usage.summary_7d.total_cost_usd, 9.0);
    assert_eq!(cached_codex.pricing.rows.len(), 1);
    assert!(app
        .usage
        .is_loading_for(&AppType::Claude, data::UsageRangePreset::SevenDays));
    assert!(matches!(
        rx.recv()
            .expect("usage/pricing refresh should be queued after sync"),
        UsagePricingReq::Load {
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::SevenDays,
        }
    ));
}

#[test]
fn completed_session_sync_requeries_only_the_active_custom_range() {
    let mut app = App::new(Some(AppType::Claude));
    app.route = route::Route::Usage;
    let range =
        data::parse_usage_custom_range("2026-06-01..2026-06-05").expect("valid custom range");
    app.usage.range = data::UsageRangePreset::Custom(range);
    let mut data = UiData::default();
    data.usage.begin_custom_range(range);
    let mut cache = UiDataByAppCache::default();
    let mut tracker = RequestTracker::default();
    let request_id = tracker.start();
    let (tx, rx) = mpsc::channel();

    handle_session_usage_sync_msg(
        &mut app,
        &mut data,
        &mut cache,
        &mut tracker,
        Some(&tx),
        SessionUsageSyncMsg::Finished {
            request_id,
            result: Ok(()),
        },
    );

    assert!(matches!(
        rx.recv().expect("active custom query should be queued"),
        UsagePricingReq::Load {
            app_type: AppType::Claude,
            range: data::UsageRangePreset::Custom(queued),
            ..
        } if queued == range
    ));
    assert!(rx.try_recv().is_err(), "fixed range should not be warmed");
    assert!(cache.usage_pricing_dirty_by_key.is_empty());
}

#[test]
fn usage_dirty_refresh_waits_for_running_aggregate_then_queues_one_follow_up() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache::default();
    let (tx, rx) = mpsc::channel();
    let range = data::UsageRangePreset::SevenDays;

    cache.queue_usage_pricing_load(&mut app, Some(&tx), &AppType::Claude, range);
    assert!(matches!(
        rx.recv().expect("initial aggregate should be queued"),
        UsagePricingReq::Load { request_id: 1, .. }
    ));

    // Repeated dirty signals while request 1 is running collapse to one bit.
    // They must not clear its token or interrupt the long SQLite query.
    cache.mark_usage_pricing_dirty(&AppType::Claude, range);
    assert!(!cache.flush_dirty_usage_pricing(&mut app, Some(&tx)));
    cache.mark_usage_pricing_dirty(&AppType::Claude, range);
    assert!(!cache.flush_dirty_usage_pricing(&mut app, Some(&tx)));
    assert!(rx.try_recv().is_err());
    assert_eq!(cache.data_generation, 0);
    assert_eq!(cache.app_state_epoch, 0);
    assert_eq!(cache.pending_usage_pricing_by_key.len(), 1);
    assert!(cache
        .usage_pricing_dirty_by_key
        .contains(&(AppType::Claude, range)));

    assert!(handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::Loaded {
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range,
            result: Box::new(Ok(data::UsagePricingData::default())),
        },
    ));
    assert!(cache.flush_dirty_usage_pricing(&mut app, Some(&tx)));
    assert!(matches!(
        rx.recv().expect("one follow-up aggregate should be queued"),
        UsagePricingReq::Load { request_id: 2, .. }
    ));
    assert!(rx.try_recv().is_err());
    assert!(cache.usage_pricing_dirty_by_key.is_empty());
}

#[test]
fn usage_sync_finish_does_not_cancel_running_aggregate_and_forces_final_follow_up() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache::default();
    let (tx, rx) = mpsc::channel();
    let range = data::UsageRangePreset::SevenDays;
    cache.queue_usage_pricing_load(&mut app, Some(&tx), &AppType::Claude, range);
    let _initial = rx.recv().expect("initial aggregate should be queued");

    let mut tracker = RequestTracker::default();
    let sync_request_id = tracker.start();
    handle_session_usage_sync_msg(
        &mut app,
        &mut data,
        &mut cache,
        &mut tracker,
        Some(&tx),
        SessionUsageSyncMsg::Finished {
            request_id: sync_request_id,
            result: Ok(()),
        },
    );

    assert!(rx.try_recv().is_err());
    assert_eq!(cache.data_generation, 0);
    assert_eq!(cache.app_state_epoch, 0);
    assert_eq!(
        cache
            .pending_usage_pricing_by_key
            .get(&(AppType::Claude, range))
            .map(|pending| pending.request_id),
        Some(1)
    );

    assert!(handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::Loaded {
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range,
            result: Box::new(Ok(data::UsagePricingData::default())),
        },
    ));
    assert!(cache.flush_dirty_usage_pricing(&mut app, Some(&tx)));
    assert!(matches!(
        rx.recv()
            .expect("post-sync aggregate must run after request 1"),
        UsagePricingReq::Load { request_id: 2, .. }
    ));
}

#[test]
fn usage_dirty_refresh_keeps_log_page_and_detail_tokens_valid() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache {
        data_generation: 7,
        app_state_epoch: 3,
        ..UiDataByAppCache::default()
    };
    let (tx, _rx) = mpsc::channel();
    let range = data::UsageRangePreset::SevenDays;

    assert!(app
        .usage
        .log_pager
        .start_request(1, 41, data::UsageLogPageDirection::Older,));
    app.usage.start_log_detail_refresh(42);
    cache.mark_usage_pricing_dirty(&AppType::Claude, range);
    cache.flush_dirty_usage_pricing(&mut app, Some(&tx));
    assert_eq!(cache.data_generation, 7);
    assert_eq!(cache.app_state_epoch, 3);

    assert!(!handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::LogPageLoaded {
            request_id: 41,
            generation: 7,
            app_state_epoch: 3,
            app_type: AppType::Claude,
            range,
            page: 1,
            direction: data::UsageLogPageDirection::Older,
            result: Ok(data::UsageLogPage {
                rows: vec![data::UsageLogRow {
                    request_id: "page-row".to_string(),
                    ..data::UsageLogRow::default()
                }],
                ..data::UsageLogPage::default()
            }),
        },
    ));
    assert!(app.usage.log_pager.page_is_available(1));

    assert!(!handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::LogDetailLoaded {
            request_id: 42,
            generation: 7,
            app_state_epoch: 3,
            app_type: AppType::Claude,
            range,
            log_rowid: 55,
            result: Ok(Some(data::UsageLogRow {
                request_id: "detail-row".to_string(),
                cursor_rowid: 55,
                ..data::UsageLogRow::default()
            })),
        },
    ));
    assert!(app
        .usage
        .log_detail_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.row_for(&AppType::Claude, range, 55))
        .is_some());
}

#[test]
fn cancelled_usage_log_page_clears_pending_without_recording_an_error() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache {
        data_generation: 7,
        app_state_epoch: 3,
        ..UiDataByAppCache::default()
    };
    let range = data::UsageRangePreset::SevenDays;
    let direction = data::UsageLogPageDirection::Older;

    assert!(app.usage.log_pager.start_request(1, 41, direction));
    assert!(!handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::LogPageLoaded {
            request_id: 41,
            generation: 7,
            app_state_epoch: 3,
            app_type: AppType::Claude,
            range,
            page: 1,
            direction,
            result: Err(UsageLogLoadError::Cancelled),
        },
    ));

    assert!(!app.usage.log_pager.page_is_pending(1));
    assert!(app.usage.log_pager.page_error(1).is_none());
}

#[test]
fn background_session_usage_sync_error_still_requeries_existing_usage() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    data.usage.summary_7d.total_cost_usd = 3.0;
    let mut cache = UiDataByAppCache::default();
    let mut tracker = RequestTracker::default();
    let request_id = tracker.start();
    let (tx, rx) = mpsc::channel();

    handle_session_usage_sync_msg(
        &mut app,
        &mut data,
        &mut cache,
        &mut tracker,
        Some(&tx),
        SessionUsageSyncMsg::Finished {
            request_id,
            result: Err("sync failed".to_string()),
        },
    );

    assert_eq!(tracker.active, None);
    assert_eq!(cache.app_state_epoch, 0);
    assert_eq!(data.usage.summary_7d.total_cost_usd, 3.0);
    assert!(app
        .usage
        .is_loading_for(&AppType::Claude, data::UsageRangePreset::SevenDays));
    assert!(matches!(
        rx.recv()
            .expect("existing usage should still be refreshed after sync failure"),
        UsagePricingReq::Load {
            app_type: AppType::Claude,
            range: data::UsageRangePreset::SevenDays,
            ..
        }
    ));
}

#[test]
fn usage_custom_range_action_queues_range_specific_load() {
    let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache::default();
    let mut proxy_loading = RequestTracker::default();
    let mut webdav_loading = RequestTracker::default();
    let mut update_check = RequestTracker::default();
    let (tx, rx) = mpsc::channel();
    let range =
        data::parse_usage_custom_range("2026-06-01..2026-06-05").expect("valid custom range");
    data.usage.recent_logs.push(data::UsageLogRow {
        request_id: "stale-log".to_string(),
        ..data::UsageLogRow::default()
    });
    data.usage.logs_total = 1;
    data.usage.recent_logs_custom.push(data::UsageLogRow {
        request_id: "stale-custom-log".to_string(),
        ..data::UsageLogRow::default()
    });
    data.usage.logs_total_custom = 1;

    handle_tui_action(
        &mut terminal,
        &mut app,
        &mut data,
        &mut cache,
        None,
        None,
        None,
        None,
        None,
        &mut proxy_loading,
        None,
        None,
        None,
        &mut webdav_loading,
        None,
        &mut update_check,
        None,
        None,
        None,
        Some(&tx),
        None,
        Action::UsageCustomRange { range },
    )
    .expect("custom range action should be handled");

    assert!(matches!(
        app.usage.range,
        data::UsageRangePreset::Custom(active) if active == range
    ));
    assert_eq!(data.usage.custom_range, Some(range));
    assert!(!data.usage.trends_custom.is_empty());
    assert_eq!(data.usage.recent_logs.len(), 1);
    assert_eq!(data.usage.logs_total, 1);
    assert!(data
        .usage
        .recent_logs_for(data::UsageRangePreset::Custom(range))
        .is_empty());
    assert_eq!(
        data.usage
            .logs_total_for(data::UsageRangePreset::Custom(range)),
        0
    );
    assert_eq!(
        cache
            .pending_usage_pricing_by_key
            .get(&(AppType::Claude, data::UsageRangePreset::Custom(range)))
            .copied(),
        Some(PendingDataLoad {
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
        })
    );
    assert!(matches!(
        rx.recv().expect("custom usage/pricing request should be queued"),
        UsagePricingReq::Load {
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::Custom(queued_range),
        } if queued_range == range
    ));
}

#[test]
fn usage_custom_range_app_switch_does_not_show_stale_custom_cache() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache::default();
    let active_range =
        data::parse_usage_custom_range("2026-06-01..2026-06-05").expect("valid active range");
    let stale_range =
        data::parse_usage_custom_range("2026-05-01..2026-05-05").expect("valid stale range");

    app.usage.range = data::UsageRangePreset::Custom(active_range);
    data.usage.begin_custom_range(active_range);

    let mut stale_usage = data::UsageSnapshot {
        custom_range: Some(stale_range),
        ..Default::default()
    };
    stale_usage.summary_custom.total_requests = 99;
    stale_usage.summary_custom.total_cost_usd = 12.34;
    cache.usage_pricing_by_key.insert(
        (AppType::Codex, data::UsageRangePreset::Custom(stale_range)),
        data::UsagePricingData {
            usage: stale_usage,
            pricing: None,
        },
    );
    cache.by_app.insert(AppType::Codex, UiData::default());

    cache
        .switch_to(&mut app, &mut data, None, AppType::Codex)
        .expect("switch should work");

    assert_eq!(app.app_type, AppType::Codex);
    assert_eq!(data.usage.custom_range, Some(active_range));
    assert_eq!(data.usage.summary_custom.total_requests, 0);
    assert_eq!(data.usage.summary_custom.total_cost_usd, 0.0);
}

#[test]
fn usage_fixed_result_does_not_replace_active_custom_logs() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache::default();
    let active_range =
        data::parse_usage_custom_range("2026-06-01..2026-06-05").expect("valid active range");
    app.usage.range = data::UsageRangePreset::Custom(active_range);
    data.usage.begin_custom_range(active_range);
    data.usage.recent_logs_custom.push(data::UsageLogRow {
        request_id: "custom-log".to_string(),
        ..data::UsageLogRow::default()
    });
    data.usage.logs_total_custom = 1;
    cache.pending_usage_pricing_by_key.insert(
        (AppType::Claude, data::UsageRangePreset::SevenDays),
        PendingDataLoad {
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
        },
    );

    let mut fixed_usage = data::UsageSnapshot::default();
    fixed_usage.summary_7d.total_requests = 10;
    fixed_usage.recent_logs.push(data::UsageLogRow {
        request_id: "fixed-log".to_string(),
        ..data::UsageLogRow::default()
    });
    fixed_usage.logs_total = 10;

    handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::Loaded {
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::SevenDays,
            result: Box::new(Ok(data::UsagePricingData {
                usage: fixed_usage,
                pricing: Some(data::ModelPricingSnapshot::default()),
            })),
        },
    );

    assert_eq!(data.usage.summary_7d.total_requests, 10);
    assert_eq!(
        data.usage
            .logs_total_for(data::UsageRangePreset::Custom(active_range)),
        1
    );
    assert_eq!(
        data.usage
            .recent_logs_for(data::UsageRangePreset::Custom(active_range))[0]
            .request_id,
        "custom-log"
    );
    assert_eq!(
        data.usage.logs_total_for(data::UsageRangePreset::SevenDays),
        10
    );
    assert_eq!(
        data.usage
            .recent_logs_for(data::UsageRangePreset::SevenDays)[0]
            .request_id,
        "fixed-log"
    );
}

#[test]
fn background_usage_refresh_preserves_a_later_page_browsing_snapshot() {
    let mut app = App::new(Some(AppType::Claude));
    app.route = route::Route::UsageLogs;
    app.usage.pane = app::UsagePane::Recent;
    let mut data = UiData::default();
    data.usage.logs_total = 205;
    data.usage.recent_logs = (0..100)
        .map(|index| data::UsageLogRow {
            request_id: format!("old-p0-{index}"),
            created_at: 1_000 - index,
            ..data::UsageLogRow::default()
        })
        .collect();
    app.usage.sync_log_pager(
        &AppType::Claude,
        data::UsageRangePreset::SevenDays,
        &data.usage.recent_logs,
        data.usage.logs_total,
    );
    let page_rows = (0..100)
        .map(|index| data::UsageLogRow {
            request_id: format!("old-p1-{index}"),
            created_at: 899 - index,
            ..data::UsageLogRow::default()
        })
        .collect::<Vec<_>>();
    let next_cursor = page_rows.last().map(data::UsageLogCursor::from_row);
    let page_load = app
        .usage
        .log_pager
        .page_request(1)
        .expect("second-page anchor");
    assert!(app.usage.log_pager.start_load_request(1, 9, page_load));
    assert!(app.usage.log_pager.finish_request(
        1,
        9,
        data::UsageLogPageDirection::Older,
        data::UsageLogPage {
            rows: page_rows,
            next_cursor,
            has_more: true,
            ..data::UsageLogPage::default()
        },
    ));
    app.usage.log_pager.gate.select(100);
    app.usage.logs_idx = 0;

    let mut cache = UiDataByAppCache::default();
    let pending = PendingDataLoad {
        request_id: 10,
        generation: 0,
        app_state_epoch: 0,
    };
    cache.pending_usage_pricing_by_key.insert(
        (AppType::Claude, data::UsageRangePreset::SevenDays),
        pending,
    );
    cache.usage_pricing_phase_by_key.insert(
        (AppType::Claude, data::UsageRangePreset::SevenDays),
        UsagePricingLoadPhase::new(pending),
    );
    let refreshed = data::UsageSnapshot {
        logs_total: 206,
        recent_logs: (0..100)
            .map(|index| data::UsageLogRow {
                request_id: format!("new-p0-{index}"),
                created_at: 2_000 - index,
                ..data::UsageLogRow::default()
            })
            .collect(),
        ..data::UsageSnapshot::default()
    };

    handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::Loaded {
            request_id: 10,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::SevenDays,
            result: Box::new(Ok(data::UsagePricingData {
                usage: refreshed,
                pricing: None,
            })),
        },
    );

    handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::LogHeadLoaded {
            request_id: pending.request_id,
            generation: pending.generation,
            app_state_epoch: pending.app_state_epoch,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::SevenDays,
            result: Ok(data::UsageLogPage {
                rows: (0..100)
                    .map(|index| data::UsageLogRow {
                        request_id: format!("late-p0-{index}"),
                        created_at: 3_000 - index,
                        ..data::UsageLogRow::default()
                    })
                    .collect(),
                has_more: true,
                ..data::UsageLogPage::default()
            }),
        },
    );

    assert_eq!(data.usage.recent_logs[0].request_id, "late-p0-0");
    assert_eq!(data.usage.logs_total, 206);
    assert_eq!(app.usage.log_pager.current_page(), 1);
    assert_eq!(app.usage.log_pager.gate.selected_index(), Some(100));
    assert_eq!(
        app.usage.log_pager.current_rows(&data.usage.recent_logs)[0].request_id,
        "old-p1-0"
    );

    app.usage.request_log_page_refresh_after_aggregate();
    let (tx, rx) = mpsc::channel();
    maybe_queue_usage_log_page_refresh_after_aggregate(&mut app, &mut cache, Some(&tx));
    let request_id = match rx.recv().expect("current page refresh should be queued") {
        UsagePricingReq::LoadLogPage {
            request_id,
            page,
            cursor,
            direction,
            ..
        } => {
            assert_eq!(page, 1);
            assert_eq!(cursor, page_load.cursor);
            assert_eq!(direction, page_load.direction);
            request_id
        }
        other => panic!("unexpected current-page refresh: {other:?}"),
    };
    maybe_queue_usage_log_page_refresh_after_aggregate(&mut app, &mut cache, Some(&tx));
    assert!(rx.try_recv().is_err());
    assert_eq!(
        app.usage.log_pager.current_rows(&data.usage.recent_logs)[0].request_id,
        "old-p1-0"
    );

    app.usage.log_pager.gate.select(199);
    app.usage.logs_idx = 99;
    let refreshed_page = (0..20)
        .map(|index| data::UsageLogRow {
            request_id: format!("refreshed-p1-{index}"),
            created_at: 899 - index,
            cursor_rowid: 10_000 + index,
            ..data::UsageLogRow::default()
        })
        .collect::<Vec<_>>();
    handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::LogPageLoaded {
            request_id,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::SevenDays,
            page: 1,
            direction: page_load.direction,
            result: Ok(data::UsageLogPage {
                next_cursor: refreshed_page.last().map(data::UsageLogCursor::from_row),
                rows: refreshed_page,
                has_more: false,
            }),
        },
    );
    assert_eq!(app.usage.log_pager.current_page(), 1);
    assert_eq!(app.usage.log_pager.gate.selected_index(), Some(119));
    assert_eq!(app.usage.logs_idx, 19);
    assert_eq!(
        app.usage.log_pager.current_rows(&data.usage.recent_logs)[19].request_id,
        "refreshed-p1-19"
    );
    assert!(matches!(
        app.on_usage_logs_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &data,
        ),
        Action::SwitchRoute(route::Route::UsageLogDetail { rowid }) if rowid == 10_019
    ));
}

#[test]
fn current_usage_log_page_refresh_failure_keeps_data_and_warns() {
    let mut app = App::new(Some(AppType::Claude));
    app.route = route::Route::UsageLogs;
    app.usage.pane = app::UsagePane::Recent;
    let mut data = UiData::default();
    data.usage.logs_total = 200;
    data.usage.recent_logs = (0..100)
        .map(|index| data::UsageLogRow {
            request_id: format!("p0-{index}"),
            created_at: 1_000 - index,
            cursor_rowid: index + 1,
            ..data::UsageLogRow::default()
        })
        .collect();
    app.usage.sync_log_pager(
        &AppType::Claude,
        data::UsageRangePreset::SevenDays,
        &data.usage.recent_logs,
        data.usage.logs_total,
    );
    let load = app
        .usage
        .log_pager
        .page_request(1)
        .expect("second page anchor");
    assert!(app.usage.log_pager.start_load_request(1, 70, load));
    let page_rows = (0..100)
        .map(|index| data::UsageLogRow {
            request_id: format!("old-p1-{index}"),
            created_at: 899 - index,
            cursor_rowid: 101 + index,
            ..data::UsageLogRow::default()
        })
        .collect::<Vec<_>>();
    assert!(app.usage.log_pager.finish_request(
        1,
        70,
        load.direction,
        data::UsageLogPage {
            next_cursor: page_rows.last().map(data::UsageLogCursor::from_row),
            rows: page_rows,
            has_more: false,
        },
    ));
    app.usage.log_pager.gate.select(100);
    let refresh = app
        .usage
        .log_pager
        .current_page_refresh_request()
        .expect("current page refresh anchor");
    assert!(app.usage.log_pager.start_load_request(1, 71, refresh));

    let mut cache = UiDataByAppCache::default();
    handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::LogPageLoaded {
            request_id: 71,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::SevenDays,
            page: 1,
            direction: refresh.direction,
            result: Err(UsageLogLoadError::Failed("database busy".to_string())),
        },
    );

    assert_eq!(
        app.usage.log_pager.current_rows(&data.usage.recent_logs)[0].request_id,
        "old-p1-0"
    );
    let toast = app.toast.as_ref().expect("refresh failure should warn");
    assert_eq!(toast.kind, ToastKind::Warning);
    assert!(toast.message.contains("database busy"), "{}", toast.message);
}

#[test]
fn obsolete_custom_usage_result_cannot_replace_the_active_range_or_pager() {
    let active_range =
        data::parse_usage_custom_range("2026-06-01..2026-06-05").expect("active range");
    let stale_range =
        data::parse_usage_custom_range("2026-05-01..2026-05-05").expect("stale range");
    let mut app = App::new(Some(AppType::Claude));
    app.route = route::Route::UsageLogs;
    app.usage.pane = app::UsagePane::Recent;
    app.usage.range = data::UsageRangePreset::Custom(active_range);
    let mut data = UiData::default();
    data.usage.begin_custom_range(active_range);
    data.usage.logs_total_custom = 205;
    data.usage.recent_logs_custom = (0..100)
        .map(|index| data::UsageLogRow {
            request_id: format!("active-p0-{index}"),
            created_at: 1_000 - index,
            ..data::UsageLogRow::default()
        })
        .collect();
    app.usage.sync_log_pager(
        &AppType::Claude,
        app.usage.range,
        &data.usage.recent_logs_custom,
        data.usage.logs_total_custom,
    );
    let page_rows = (0..100)
        .map(|index| data::UsageLogRow {
            request_id: format!("active-p1-{index}"),
            created_at: 899 - index,
            ..data::UsageLogRow::default()
        })
        .collect::<Vec<_>>();
    let next_cursor = page_rows.last().map(data::UsageLogCursor::from_row);
    assert!(app
        .usage
        .log_pager
        .start_request(1, 11, data::UsageLogPageDirection::Older,));
    assert!(app.usage.log_pager.finish_request(
        1,
        11,
        data::UsageLogPageDirection::Older,
        data::UsageLogPage {
            rows: page_rows,
            next_cursor,
            has_more: true,
            ..data::UsageLogPage::default()
        },
    ));
    app.usage.log_pager.gate.select(100);

    let mut cache = UiDataByAppCache::default();
    cache.pending_usage_pricing_by_key.insert(
        (AppType::Claude, data::UsageRangePreset::Custom(stale_range)),
        PendingDataLoad {
            request_id: 12,
            generation: 0,
            app_state_epoch: 0,
        },
    );
    let stale_usage = data::UsageSnapshot {
        custom_range: Some(stale_range),
        recent_logs_custom: vec![data::UsageLogRow {
            request_id: "stale-row".to_string(),
            ..data::UsageLogRow::default()
        }],
        logs_total_custom: 1,
        ..data::UsageSnapshot::default()
    };

    handle_usage_pricing_msg(
        &mut app,
        &mut data,
        &mut cache,
        UsagePricingMsg::Loaded {
            request_id: 12,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::Custom(stale_range),
            result: Box::new(Ok(data::UsagePricingData {
                usage: stale_usage,
                pricing: None,
            })),
        },
    );

    assert_eq!(data.usage.custom_range, Some(active_range));
    assert_eq!(data.usage.recent_logs_custom[0].request_id, "active-p0-0");
    assert_eq!(app.usage.log_pager.current_page(), 1);
    assert_eq!(app.usage.log_pager.gate.selected_index(), Some(100));
}

#[test]
fn switching_apps_clears_usage_log_detail_context() {
    let mut app = App::new(Some(AppType::Claude));
    app.route = route::Route::UsageLogDetail { rowid: 66 };
    app.usage.remember_log_detail(
        AppType::Claude,
        data::UsageRangePreset::SevenDays,
        data::UsageLogRow {
            request_id: "shared-id".to_string(),
            app_type: "claude".to_string(),
            cursor_rowid: 66,
            ..data::UsageLogRow::default()
        },
    );
    let mut data = UiData::default();

    apply_preloaded_app_switch(&mut app, &mut data, AppType::Codex, UiData::default());

    assert_eq!(app.app_type, AppType::Codex);
    assert!(app.usage.log_detail_snapshot.is_none());
    assert!(app.usage.log_pager.gate.is_empty());
}

#[test]
fn usage_custom_range_manual_refresh_is_async_preserves_data_and_deduplicates() {
    let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
    let mut app = App::new(Some(AppType::Claude));
    app.route = route::Route::Usage;
    let mut data = UiData::default();
    let mut cache = UiDataByAppCache::default();
    let mut proxy_loading = RequestTracker::default();
    let mut webdav_loading = RequestTracker::default();
    let mut update_check = RequestTracker::default();
    let (tx, rx) = mpsc::channel();
    let range =
        data::parse_usage_custom_range("2026-06-01..2026-06-05").expect("valid custom range");

    app.usage.range = data::UsageRangePreset::Custom(range);
    data.usage.custom_range = Some(range);
    data.usage.summary_custom.total_requests = 42;

    handle_tui_action(
        &mut terminal,
        &mut app,
        &mut data,
        &mut cache,
        None,
        None,
        None,
        None,
        None,
        &mut proxy_loading,
        None,
        None,
        None,
        &mut webdav_loading,
        None,
        &mut update_check,
        None,
        None,
        None,
        Some(&tx),
        None,
        Action::UsageRefresh,
    )
    .expect("usage refresh should be handled");

    assert_eq!(data.usage.custom_range, Some(range));
    assert_eq!(data.usage.summary_custom.total_requests, 42);
    assert!(app
        .usage
        .is_loading_for(&AppType::Claude, data::UsageRangePreset::Custom(range)));
    assert!(matches!(
        rx.recv().expect("custom usage/pricing reload should be queued"),
        UsagePricingReq::Load {
            request_id: 1,
            app_type: AppType::Claude,
            range: data::UsageRangePreset::Custom(queued_range),
            ..
        } if queued_range == range
    ));

    handle_tui_action(
        &mut terminal,
        &mut app,
        &mut data,
        &mut cache,
        None,
        None,
        None,
        None,
        None,
        &mut proxy_loading,
        None,
        None,
        None,
        &mut webdav_loading,
        None,
        &mut update_check,
        None,
        None,
        None,
        Some(&tx),
        None,
        Action::UsageRefresh,
    )
    .expect("duplicate usage refresh should be handled");

    assert!(rx.try_recv().is_err(), "in-flight refresh should be reused");
    assert_eq!(data.usage.summary_custom.total_requests, 42);
}

#[test]
fn skills_scan_unmanaged_uses_info_toast_kind() {
    let mut app = App::new(Some(AppType::OpenCode));

    scan_unmanaged_skills_with(&mut app, || Ok(Vec::new())).expect("skills scan should work");

    let toast = app.toast.as_ref().expect("skills scan should show toast");
    assert_eq!(toast.kind, ToastKind::Info);
    assert_eq!(toast.message, texts::tui_toast_unmanaged_scanned(0));
}

#[test]
fn opening_skills_import_picker_selects_all_by_default() {
    let mut app = App::new(Some(AppType::Claude));

    open_skills_import_picker_with(&mut app, || {
        Ok(vec![crate::services::skill::UnmanagedSkill {
            directory: "hello-skill".to_string(),
            name: "Hello Skill".to_string(),
            description: Some("A local skill".to_string()),
            found_in: vec!["claude".to_string()],
            path: "/tmp/hello-skill".to_string(),
        }])
    })
    .expect("import picker should open");

    assert!(matches!(
        &app.overlay,
        Overlay::SkillsImportPicker {
            skills,
            selected_idx: 0,
            selected,
        } if skills.len() == 1
            && skills[0].directory == "hello-skill"
            && selected.contains("hello-skill")
    ));
}

#[test]
fn skills_import_from_apps_uses_info_toast_kind() {
    let mut app = App::new(Some(AppType::OpenCode));
    let mut data = UiData::default();

    finish_skills_import_with(
        &mut app,
        &mut data,
        || Ok(vec![]),
        |_app_type| Ok(UiData::default()),
    )
    .expect("skills import should work");

    let toast = app.toast.as_ref().expect("skills import should show toast");
    assert_eq!(toast.kind, ToastKind::Info);
    assert_eq!(toast.message, texts::tui_toast_unmanaged_imported(0));
}

#[test]
fn proxy_help_overlay_uses_on_demand_proxy_config() {
    let mut app = App::new(Some(AppType::Claude));
    let data = UiData::default();

    open_proxy_help_overlay_with(&mut app, &data, || {
        Ok(Some(crate::proxy::ProxyConfig {
            listen_address: "127.0.0.1".to_string(),
            listen_port: 3456,
            ..crate::proxy::ProxyConfig::default()
        }))
    })
    .expect("proxy help overlay should open");

    let Overlay::TextView(view) = &app.overlay else {
        panic!("expected proxy help overlay");
    };
    let joined = view.lines.join("\n");
    assert!(joined.contains("cc-switch proxy serve --listen-address 127.0.0.1 --listen-port 3456"));
    assert!(joined.contains("ANTHROPIC_BASE_URL=http://127.0.0.1:3456"));
}

#[test]
fn managed_proxy_action_enqueues_background_request_and_shows_loading_overlay() {
    let mut app = App::new(Some(AppType::Claude));
    let mut loading = RequestTracker::default();
    let (tx, rx) = mpsc::channel();

    queue_managed_proxy_action(&mut app, Some(&tx), &mut loading, AppType::Claude, true)
        .expect("queue proxy action should succeed");

    let req = rx.recv().expect("proxy request should be queued");
    assert!(matches!(
        req,
        ProxyReq::SetManagedSessionForCurrentApp {
            request_id: 1,
            app_type: AppType::Claude,
            enabled: true,
        }
    ));
    assert_eq!(loading.active, Some(1));
    assert!(matches!(
        app.overlay,
        Overlay::Loading {
            kind: LoadingKind::Proxy,
            ..
        }
    ));
}

#[test]
fn proxy_snapshot_refresh_enqueues_single_background_request() {
    let mut tracker = RequestTracker::default();
    let (tx, rx) = mpsc::channel();

    queue_proxy_snapshot_refresh(&mut tracker, Some(&tx), &AppType::Codex);
    queue_proxy_snapshot_refresh(&mut tracker, Some(&tx), &AppType::Claude);

    let req = rx.recv().expect("proxy snapshot request should be queued");
    assert!(matches!(
        req,
        ProxyReq::RefreshSnapshot {
            request_id: 1,
            app_type: AppType::Codex,
        }
    ));
    assert!(rx.try_recv().is_err(), "in-flight refresh should not stack");
    assert_eq!(tracker.active, Some(1));
}

#[test]
fn proxy_snapshot_refresh_can_be_requeued_after_app_switch_cancel() {
    let mut tracker = RequestTracker {
        seq: 1,
        active: Some(1),
    };
    let (tx, rx) = mpsc::channel();

    tracker.cancel();
    queue_proxy_snapshot_refresh(&mut tracker, Some(&tx), &AppType::OpenCode);

    let req = rx
        .recv()
        .expect("new app snapshot request should be queued");
    assert!(matches!(
        req,
        ProxyReq::RefreshSnapshot {
            request_id: 2,
            app_type: AppType::OpenCode,
        }
    ));
    assert_eq!(tracker.active, Some(2));
}

#[test]
fn proxy_snapshot_refresh_send_failure_clears_active_request() {
    let mut tracker = RequestTracker::default();
    let (tx, rx) = mpsc::channel();
    drop(rx);

    queue_proxy_snapshot_refresh(&mut tracker, Some(&tx), &AppType::Claude);

    assert_eq!(tracker.active, None);
}

#[test]
fn proxy_snapshot_after_app_switch_queues_only_after_successful_change() {
    let (tx, rx) = mpsc::channel();
    let mut tracker = RequestTracker {
        seq: 7,
        active: Some(7),
    };

    queue_proxy_snapshot_refresh_after_app_switch(
        &mut tracker,
        Some(&tx),
        &AppType::Claude,
        &AppType::Codex,
        false,
    );
    assert_eq!(tracker.active, Some(7));
    assert!(rx.try_recv().is_err());

    queue_proxy_snapshot_refresh_after_app_switch(
        &mut tracker,
        Some(&tx),
        &AppType::Claude,
        &AppType::Claude,
        true,
    );
    assert_eq!(tracker.active, Some(7));
    assert!(rx.try_recv().is_err());

    queue_proxy_snapshot_refresh_after_app_switch(
        &mut tracker,
        Some(&tx),
        &AppType::Claude,
        &AppType::Codex,
        true,
    );
    let req = rx
        .recv()
        .expect("successful app switch should queue proxy snapshot");
    assert!(matches!(
        req,
        ProxyReq::RefreshSnapshot {
            request_id: 8,
            app_type: AppType::Codex,
        }
    ));
    assert_eq!(tracker.active, Some(8));
}

#[test]
fn proxy_snapshot_result_updates_proxy_without_cache_invalidation() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let mut proxy_loading = RequestTracker::default();
    let mut proxy_snapshot_refresh = RequestTracker {
        seq: 1,
        active: Some(1),
    };
    let proxy = data::ProxySnapshot {
        running: true,
        estimated_input_tokens_total: 12,
        estimated_output_tokens_total: 34,
        ..data::ProxySnapshot::default()
    };
    app.reset_proxy_activity(0, 0);

    let invalidation = handle_proxy_msg(
        &mut app,
        &mut data,
        &mut proxy_loading,
        &mut proxy_snapshot_refresh,
        ProxyMsg::SnapshotRefreshed {
            request_id: 1,
            app_type: AppType::Claude,
            result: Ok(proxy),
        },
    )
    .expect("snapshot result should be handled");

    assert_eq!(invalidation, CacheInvalidation::None);
    assert!(data.proxy.running);
    assert_eq!(data.proxy.estimated_input_tokens_total, 12);
    assert_eq!(data.proxy.estimated_output_tokens_total, 34);
    assert_eq!(app.proxy_activity_last_input_tokens, Some(12));
    assert_eq!(app.proxy_activity_last_output_tokens, Some(34));
    assert_eq!(proxy_snapshot_refresh.active, None);
}

#[test]
fn proxy_snapshot_result_ignores_stale_or_wrong_app() {
    let mut app = App::new(Some(AppType::Claude));
    let mut data = UiData::default();
    let mut proxy_loading = RequestTracker::default();
    let mut proxy_snapshot_refresh = RequestTracker {
        seq: 2,
        active: Some(2),
    };

    handle_proxy_msg(
        &mut app,
        &mut data,
        &mut proxy_loading,
        &mut proxy_snapshot_refresh,
        ProxyMsg::SnapshotRefreshed {
            request_id: 1,
            app_type: AppType::Claude,
            result: Ok(data::ProxySnapshot {
                running: true,
                ..data::ProxySnapshot::default()
            }),
        },
    )
    .expect("stale snapshot result should be ignored");
    assert!(!data.proxy.running);
    assert_eq!(proxy_snapshot_refresh.active, Some(2));

    handle_proxy_msg(
        &mut app,
        &mut data,
        &mut proxy_loading,
        &mut proxy_snapshot_refresh,
        ProxyMsg::SnapshotRefreshed {
            request_id: 2,
            app_type: AppType::Codex,
            result: Ok(data::ProxySnapshot {
                running: true,
                ..data::ProxySnapshot::default()
            }),
        },
    )
    .expect("wrong-app snapshot result should be ignored");
    assert!(!data.proxy.running);
    assert_eq!(proxy_snapshot_refresh.active, None);
}

#[test]
fn proxy_open_flash_runner_persists_effect_across_frames() {
    let mut flash = ProxyOpenFlash::default();
    let mut app = App::new(Some(AppType::Claude));
    app.proxy_visual_transition = Some(super::app::ProxyVisualTransition {
        from_on: false,
        to_on: true,
        started_tick: 10,
    });
    let area = Rect::new(0, 0, 20, 2);

    flash.sync(&app, area);
    assert!(flash.active());

    let mut first = Buffer::empty(area);
    flash.process(std::time::Duration::from_millis(500), &mut first, area);
    assert!(flash.active(), "flash should still be active at peak frame");

    let mut second = Buffer::empty(area);
    flash.process(std::time::Duration::from_millis(100), &mut second, area);
    assert!(
        flash.active(),
        "flash should still be active during return phase"
    );
}

#[test]
fn managed_proxy_action_warns_when_worker_is_unavailable() {
    let mut app = App::new(Some(AppType::Claude));
    let mut loading = RequestTracker::default();

    queue_managed_proxy_action(&mut app, None, &mut loading, AppType::Claude, true)
        .expect("missing worker should not crash");

    let toast = app.toast.as_ref().expect("warning toast should be shown");
    assert_eq!(toast.kind, ToastKind::Warning);
    assert_eq!(
        toast.message,
        texts::tui_toast_proxy_request_failed(texts::tui_error_proxy_worker_unavailable())
    );
    assert!(matches!(app.overlay, Overlay::None));
    assert_eq!(loading.active, None);
}

#[test]
fn normalize_ctrl_h_becomes_backspace() {
    let key = KeyEvent::new_with_kind(
        KeyCode::Char('h'),
        KeyModifiers::CONTROL,
        KeyEventKind::Press,
    );
    let normalized = normalize_key_event(key);
    assert_eq!(normalized.code, KeyCode::Backspace);
    assert!(!normalized.modifiers.contains(KeyModifiers::CONTROL));
}

#[test]
fn normalize_plain_h_unchanged() {
    let key = KeyEvent::new_with_kind(KeyCode::Char('h'), KeyModifiers::NONE, KeyEventKind::Press);
    let normalized = normalize_key_event(key);
    assert_eq!(normalized.code, KeyCode::Char('h'));
    assert_eq!(normalized.modifiers, KeyModifiers::NONE);
}

#[test]
fn normalize_real_backspace_unchanged() {
    let key = KeyEvent::new_with_kind(KeyCode::Backspace, KeyModifiers::NONE, KeyEventKind::Press);
    let normalized = normalize_key_event(key);
    assert_eq!(normalized.code, KeyCode::Backspace);
}

#[test]
fn quick_setup_helper_saves_preset_and_runs_connection_check() {
    let mut captured = None;
    let mut checked = false;

    apply_webdav_jianguoyun_quick_setup(
        " demo@nutstore.com ",
        " app-password ",
        |cfg| {
            captured = Some(cfg);
            Ok(())
        },
        || {
            checked = true;
            Ok(())
        },
    )
    .expect("quick setup helper should succeed");

    let saved = captured.expect("settings should be saved");
    assert!(saved.enabled);
    assert_eq!(saved.base_url, "https://dav.jianguoyun.com/dav");
    assert_eq!(saved.remote_root, "cc-switch-sync");
    assert_eq!(saved.profile, "default");
    assert_eq!(saved.username, "demo@nutstore.com");
    assert_eq!(saved.password, "app-password");
    assert!(checked, "connection check should be called");
}

#[test]
fn quick_setup_helper_stops_when_save_fails() {
    let mut checked = false;
    let err = apply_webdav_jianguoyun_quick_setup(
        "u",
        "p",
        |_cfg| Err(AppError::Message("save failed".to_string())),
        || {
            checked = true;
            Ok(())
        },
    )
    .expect_err("save failure should be returned");

    assert!(err.to_string().contains("save failed"));
    assert!(!checked, "connection check should not run when save fails");
}

#[test]
fn stream_check_result_lines_include_core_fields() {
    let result = crate::services::stream_check::StreamCheckResult {
        status: crate::services::stream_check::HealthStatus::Degraded,
        success: true,
        message: "slow but working".to_string(),
        response_time_ms: Some(6789),
        http_status: Some(200),
        model_used: "gpt-5.1-codex".to_string(),
        tested_at: 1_700_000_000,
        retry_count: 1,
        error_category: None,
    };

    let lines = build_stream_check_result_lines("Provider One", &result);
    let joined = lines.join("\n");

    assert!(joined.contains("Provider One"));
    assert!(joined.contains("gpt-5.1-codex"));
    assert!(joined.contains("200"));
    assert!(joined.contains("6789"));
    assert!(joined.contains("slow but working"));
}

#[test]
fn external_editor_helper_replaces_editor_buffer_and_keeps_initial_text() {
    let mut app = App::new(Some(crate::AppType::Claude));
    app.open_editor(
        "Prompt",
        super::app::EditorKind::Plain,
        "hello",
        super::app::EditorSubmit::PromptEdit {
            id: "pr1".to_string(),
        },
    );

    run_external_editor_for_current_editor(&mut app, |current| {
        assert_eq!(current, "hello");
        Ok("hello from external\neditor".to_string())
    })
    .expect("external editor helper should succeed");

    let editor = app.editor.as_ref().expect("editor should stay open");
    assert_eq!(editor.text(), "hello from external\neditor");
    assert_eq!(editor.initial_text, "hello");
    assert!(editor.is_dirty(), "updated buffer should remain unsaved");
}

#[test]
fn external_editor_helper_preserves_buffer_on_error() {
    let mut app = App::new(Some(crate::AppType::Claude));
    app.open_editor(
        "Prompt",
        super::app::EditorKind::Plain,
        "hello",
        super::app::EditorSubmit::PromptEdit {
            id: "pr1".to_string(),
        },
    );

    let err = run_external_editor_for_current_editor(&mut app, |_current| {
        Err(AppError::Message("boom".to_string()))
    })
    .expect_err("external editor helper should surface the edit error");

    assert!(err.to_string().contains("boom"));
    let editor = app.editor.as_ref().expect("editor should stay open");
    assert_eq!(editor.text(), "hello");
    assert_eq!(editor.initial_text, "hello");
    assert!(
        !editor.is_dirty(),
        "failed external edit must not dirty the buffer"
    );
}

#[test]
fn drain_latest_webdav_req_prefers_last_enqueued_request() {
    let (tx, rx) = mpsc::channel();
    tx.send(WebDavReq {
        request_id: 1,
        kind: WebDavReqKind::CheckConnection,
    })
    .expect("send check request");
    tx.send(WebDavReq {
        request_id: 2,
        kind: WebDavReqKind::Upload,
    })
    .expect("send upload request");
    tx.send(WebDavReq {
        request_id: 3,
        kind: WebDavReqKind::JianguoyunQuickSetup {
            username: "u@example.com".to_string(),
            password: "p".to_string(),
        },
    })
    .expect("send quick setup request");

    let first = rx.recv().expect("receive first request");
    let latest = drain_latest_webdav_req(first, &rx);
    assert!(matches!(
        latest,
        WebDavReq {
            request_id: 3,
            kind: WebDavReqKind::JianguoyunQuickSetup { username, password }
        }
            if username == "u@example.com" && password == "p"
    ));
}

#[test]
fn update_webdav_last_error_with_updates_status_when_present() {
    let mut captured = None;
    update_webdav_last_error_with(
        Some("network timeout".to_string()),
        || Some(crate::settings::WebDavSyncSettings::default()),
        |cfg| {
            captured = Some(cfg);
            Ok(())
        },
    );

    let saved = captured.expect("expected settings to be saved");
    assert_eq!(saved.status.last_error.as_deref(), Some("network timeout"));
}

#[test]
fn update_webdav_last_error_with_skips_when_settings_absent() {
    let mut saved = false;
    update_webdav_last_error_with(
        Some("network timeout".to_string()),
        || None,
        |_cfg| {
            saved = true;
            Ok(())
        },
    );
    assert!(
        !saved,
        "set callback should not run when webdav settings are missing"
    );
}

#[test]
fn update_success_does_not_force_exit_when_overlay_hidden() {
    let mut app = App::new(None);
    app.overlay = Overlay::None;
    let mut update_check = RequestTracker::default();

    handle_update_msg(
        &mut app,
        &mut update_check,
        UpdateMsg::DownloadFinished(Ok("v9.9.9".to_string())),
    );

    assert!(
        !app.should_quit,
        "successful update should not force exit without user confirmation"
    );
    assert!(
        matches!(app.overlay, Overlay::UpdateResult { success: true, .. }),
        "successful update should show result overlay even when progress overlay was hidden"
    );
}

#[test]
fn update_check_finished_is_ignored_when_canceled() {
    let mut app = App::new(None);
    app.overlay = Overlay::None;
    let mut update_check = RequestTracker::default();

    let info = crate::cli::commands::update::UpdateCheckInfo {
        current_version: "4.7.0".to_string(),
        target_tag: "v9.9.9".to_string(),
        is_already_latest: false,
        is_downgrade: false,
        is_homebrew_managed: false,
    };

    handle_update_msg(
        &mut app,
        &mut update_check,
        UpdateMsg::CheckFinished {
            request_id: 1,
            result: Ok(info),
        },
    );

    assert!(
        matches!(app.overlay, Overlay::None),
        "update check result should be ignored after cancel/hide"
    );
}

#[test]
fn update_check_finished_is_processed_when_request_id_matches() {
    let mut app = App::new(None);
    app.overlay = Overlay::Loading {
        kind: LoadingKind::UpdateCheck,
        title: texts::tui_update_checking_title().to_string(),
        message: texts::tui_loading().to_string(),
    };
    let mut update_check = RequestTracker {
        active: Some(7),
        ..Default::default()
    };

    let info = crate::cli::commands::update::UpdateCheckInfo {
        current_version: "4.7.0".to_string(),
        target_tag: "v9.9.9".to_string(),
        is_already_latest: false,
        is_downgrade: false,
        is_homebrew_managed: false,
    };

    handle_update_msg(
        &mut app,
        &mut update_check,
        UpdateMsg::CheckFinished {
            request_id: 7,
            result: Ok(info),
        },
    );

    assert_eq!(update_check.active, None);
    assert!(matches!(
        app.overlay,
        Overlay::UpdateAvailable {
            latest,
            selected: 0,
            ..
        } if latest == "v9.9.9"
    ));
}

#[test]
fn update_check_finished_for_homebrew_update_shows_brew_toast() {
    let mut app = App::new(None);
    app.overlay = Overlay::Loading {
        kind: LoadingKind::UpdateCheck,
        title: texts::tui_update_checking_title().to_string(),
        message: texts::tui_loading().to_string(),
    };
    let mut update_check = RequestTracker {
        active: Some(7),
        ..Default::default()
    };

    let info = crate::cli::commands::update::UpdateCheckInfo {
        current_version: "4.7.0".to_string(),
        target_tag: "v9.9.9".to_string(),
        is_already_latest: false,
        is_downgrade: false,
        is_homebrew_managed: true,
    };

    handle_update_msg(
        &mut app,
        &mut update_check,
        UpdateMsg::CheckFinished {
            request_id: 7,
            result: Ok(info),
        },
    );

    assert_eq!(update_check.active, None);
    assert!(matches!(app.overlay, Overlay::None));
    let toast = app.toast.as_ref().expect("homebrew update should toast");
    assert_eq!(toast.kind, ToastKind::Info);
    assert!(toast.message.contains("v9.9.9"));
    assert!(toast.message.contains("brew upgrade cc-switch"));
}

#[test]
fn update_check_finished_is_ignored_when_request_id_mismatch() {
    let mut app = App::new(None);
    app.overlay = Overlay::None;
    let mut update_check = RequestTracker {
        active: Some(2),
        ..Default::default()
    };

    let stale = crate::cli::commands::update::UpdateCheckInfo {
        current_version: "4.7.0".to_string(),
        target_tag: "v1.0.0".to_string(),
        is_already_latest: false,
        is_downgrade: false,
        is_homebrew_managed: false,
    };
    handle_update_msg(
        &mut app,
        &mut update_check,
        UpdateMsg::CheckFinished {
            request_id: 1,
            result: Ok(stale),
        },
    );

    assert_eq!(update_check.active, Some(2));
    assert!(matches!(app.overlay, Overlay::None));

    let latest = crate::cli::commands::update::UpdateCheckInfo {
        current_version: "4.7.0".to_string(),
        target_tag: "v9.9.9".to_string(),
        is_already_latest: false,
        is_downgrade: false,
        is_homebrew_managed: false,
    };
    handle_update_msg(
        &mut app,
        &mut update_check,
        UpdateMsg::CheckFinished {
            request_id: 2,
            result: Ok(latest),
        },
    );

    assert_eq!(update_check.active, None);
    assert!(matches!(app.overlay, Overlay::UpdateAvailable { .. }));
}

#[test]
fn model_fetch_strategy_matches_provider_field() {
    assert_eq!(
        model_fetch_strategy_for_field(ProviderAddField::CodexModel),
        ModelFetchStrategy::Bearer
    );
    assert_eq!(
        model_fetch_strategy_for_field(ProviderAddField::GeminiModel),
        ModelFetchStrategy::GoogleApiKey
    );
    assert_eq!(
        model_fetch_strategy_for_field(ProviderAddField::ClaudeModelConfig),
        ModelFetchStrategy::Anthropic
    );
    assert_eq!(
        model_fetch_strategy_for_field(ProviderAddField::HermesModels),
        ModelFetchStrategy::Bearer
    );
}

#[test]
fn model_fetch_candidate_urls_prefers_v1_for_anthropic_base() {
    let urls = build_model_fetch_candidate_urls(
        "https://api.anthropic.com",
        ModelFetchStrategy::Anthropic,
    );
    assert_eq!(
        urls,
        vec![
            "https://api.anthropic.com/v1/models".to_string(),
            "https://api.anthropic.com/models".to_string()
        ]
    );
}

#[test]
fn model_fetch_candidate_urls_strip_anthropic_compat_suffix() {
    let urls = build_model_fetch_candidate_urls(
        "https://api.deepseek.com/anthropic",
        ModelFetchStrategy::Anthropic,
    );
    assert_eq!(
        urls,
        vec![
            "https://api.deepseek.com/anthropic/v1/models".to_string(),
            "https://api.deepseek.com/v1/models".to_string(),
            "https://api.deepseek.com/models".to_string(),
        ]
    );
}

#[test]
fn model_fetch_candidate_urls_for_gemini_v1beta_keeps_models_endpoint() {
    let urls = build_model_fetch_candidate_urls(
        "https://generativelanguage.googleapis.com/v1beta",
        ModelFetchStrategy::GoogleApiKey,
    );
    assert_eq!(
        urls,
        vec!["https://generativelanguage.googleapis.com/v1beta/models".to_string()]
    );
}

#[tokio::test]
async fn model_fetch_sends_trimmed_custom_user_agent() {
    use std::sync::{Arc, Mutex};

    use axum::{http::HeaderMap, routing::get, Router};

    let observed_user_agent = Arc::new(Mutex::new(None::<String>));
    let handler_user_agent = Arc::clone(&observed_user_agent);
    let app = Router::new().route(
        "/v1/models",
        get(move |headers: HeaderMap| {
            let observed_user_agent = Arc::clone(&handler_user_agent);
            async move {
                let user_agent = headers
                    .get(reqwest::header::USER_AGENT)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                *observed_user_agent
                    .lock()
                    .expect("capture model fetch user agent") = user_agent;
                axum::Json(json!({ "data": [{ "id": "model-a" }] }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind model fetch test server");
    let address = listener.local_addr().expect("model fetch listener address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("model fetch test server should run");
    });

    let models = fetch_provider_models_for_tui(
        &format!("http://{address}"),
        Some("sk-test"),
        Some("  cc-switch-model-fetch/test  "),
        ModelFetchStrategy::Bearer,
    )
    .await
    .expect("model fetch should succeed");
    server.abort();

    assert_eq!(models, vec!["model-a"]);
    assert_eq!(
        observed_user_agent
            .lock()
            .expect("read model fetch user agent")
            .as_deref(),
        Some("cc-switch-model-fetch/test")
    );
}

#[test]
#[serial(home_settings)]
fn startup_hidden_requested_app_bootstrap_uses_visible_app_normalization_before_loading_data() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    crate::settings::set_visible_apps(crate::settings::VisibleApps {
        claude: true,
        codex: true,
        gemini: false,
        opencode: true,
        hermes: false,
        openclaw: true,
    })
    .expect("save visible apps");

    let mut loaded_app_type = None;
    let (app, _data) = initialize_app_state_for_test(Some(AppType::Gemini), |app_type| {
        loaded_app_type = Some(app_type.clone());
        Ok(UiData::default())
    })
    .expect("bootstrap app state");

    assert_eq!(loaded_app_type, Some(AppType::OpenCode));
    assert_eq!(app.app_type, AppType::OpenCode);
}

#[test]
#[serial(home_settings)]
fn startup_reads_persisted_common_config_notice_confirmation() {
    let temp_home = TempDir::new().expect("create temp home");
    let _env = EnvGuard::set_home(temp_home.path());
    crate::settings::set_common_config_confirmed(true).expect("save confirmation");

    let (app, _data) =
        initialize_app_state_for_test(Some(AppType::Claude), |_| Ok(UiData::default()))
            .expect("bootstrap app state");

    assert!(app.common_config_notice_confirmed);
}

#[test]
fn parse_model_ids_supports_multiple_shapes_and_dedups_stably() {
    let data_payload = json!({
        "data": [
            {"id": "gpt-4o"},
            {"id": "gpt-4o-mini"},
            {"id": "gpt-4o"},
            {"id": "o3"}
        ]
    });
    assert_eq!(
        parse_model_ids_from_response(&data_payload),
        vec!["gpt-4o", "gpt-4o-mini", "o3"]
    );

    let gemini_payload = json!({
        "models": [
            {"name": "models/gemini-2.0-pro"},
            {"name": "models/gemini-2.0-flash"}
        ]
    });
    assert_eq!(
        parse_model_ids_from_response(&gemini_payload),
        vec!["gemini-2.0-pro", "gemini-2.0-flash"]
    );
}
