use std::sync::mpsc;
use std::{ffi::OsString, path::Path};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{buffer::Buffer, layout::Rect};
use serde_json::json;
use serial_test::serial;
use tempfile::TempDir;

use super::app::{App, LoadingKind, Overlay, ToastKind};
use super::data::UiData;
use super::form::ProviderAddField;
use super::*;
use crate::cli::i18n::texts;
use crate::test_support::{
    lock_test_home_and_settings, set_test_home_override, TestHomeSettingsLock,
};
use crate::{AppError, AppType};

struct EnvGuard {
    _lock: TestHomeSettingsLock,
    old_home: Option<OsString>,
    old_userprofile: Option<OsString>,
}

impl EnvGuard {
    fn set_home(home: &Path) -> Self {
        let lock = lock_test_home_and_settings();
        let old_home = std::env::var_os("HOME");
        let old_userprofile = std::env::var_os("USERPROFILE");
        std::env::set_var("HOME", home);
        std::env::set_var("USERPROFILE", home);
        set_test_home_override(Some(home));
        crate::settings::reload_test_settings();
        Self {
            _lock: lock,
            old_home,
            old_userprofile,
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
    let mut update_check = RequestTracker::default();
    update_check.active = Some(7);

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
    let mut update_check = RequestTracker::default();
    update_check.active = Some(7);

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
    let mut update_check = RequestTracker::default();
    update_check.active = Some(2);

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
