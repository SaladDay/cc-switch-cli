mod app;
mod data;
mod form;
mod route;
mod runtime_actions;
mod runtime_skills;
mod runtime_systems;
mod terminal;
#[cfg(test)]
mod tests;
mod text_edit;
mod theme;
mod ui;

use std::sync::mpsc;
use std::time::{Duration, Instant};

use crossterm::event::{self, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::error::AppError;

use app::{Action, App, ToastKind};
use runtime_actions::handle_action;
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
    ProxyReq, UpdateMsg, WebDavReq, WebDavReqKind,
};
pub(crate) use runtime_systems::{fetch_provider_models_for_tui, ModelFetchStrategy};
use runtime_systems::{
    handle_local_env_msg, handle_managed_auth_msg, handle_model_fetch_msg, handle_proxy_msg,
    handle_quota_msg, handle_session_msg, handle_skills_msg, handle_speedtest_msg,
    handle_stream_check_msg, handle_update_msg, handle_webdav_msg, start_local_env_system,
    start_managed_auth_system, start_model_fetch_system, start_proxy_system, start_quota_system,
    start_session_system, start_skills_system, start_speedtest_system, start_stream_check_system,
    start_update_system, start_webdav_system, LocalEnvReq, ManagedAuthReq, QuotaReq,
    RequestTracker,
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
    let data = load_data(&app.app_type)?;
    Ok((app, data))
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

fn queue_managed_auth_refresh(
    app: &mut App,
    managed_auth_req_tx: Option<&mpsc::Sender<ManagedAuthReq>>,
    auth_provider: &str,
) {
    let Some(tx) = managed_auth_req_tx else {
        app.managed_auth_loading = false;
        app.push_toast(
            texts::tui_toast_managed_auth_worker_unavailable("auth worker is not running"),
            ToastKind::Warning,
        );
        return;
    };

    app.managed_auth_loading = true;
    if let Err(error) = tx.send(ManagedAuthReq::Refresh {
        auth_provider: auth_provider.to_string(),
    }) {
        app.managed_auth_loading = false;
        app.push_toast(
            texts::tui_toast_managed_auth_request_failed(&error.to_string()),
            ToastKind::Warning,
        );
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

fn queue_sessions_refresh_if_needed(
    app: &mut App,
    session_req_tx: Option<&mpsc::Sender<runtime_systems::SessionReq>>,
) {
    if !matches!(app.route, route::Route::Sessions) {
        return;
    }
    let provider_id = app.app_type.as_str().to_string();
    if app.sessions.loaded_for_provider(&provider_id) || app.sessions.loading {
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
    }) {
        app.sessions.fail_scan(request_id, err.to_string());
        app.push_toast(
            texts::tui_sessions_toast_refresh_failed(&err.to_string()),
            ToastKind::Warning,
        );
    }
}

pub fn run(app_override: Option<AppType>) -> Result<(), AppError> {
    let _panic_hook = PanicRestoreHookGuard::install();
    let mut terminal = TuiTerminal::new()?;
    let (mut app, mut data) = initialize_app_state_with(
        app_override,
        data::UiData::load,
        apply_visible_apps_startup_policy,
    )?;
    let mut proxy_open_flash = ProxyOpenFlash::default();
    app.reset_proxy_activity(
        data.proxy.estimated_input_tokens_total,
        data.proxy.estimated_output_tokens_total,
    );
    app.observe_proxy_visual_state(&data);

    let tick_rate = TUI_TICK_RATE;
    let mut last_tick = Instant::now();
    let mut last_frame = Instant::now();
    let mut proxy_loading = RequestTracker::default();
    let mut webdav_loading = RequestTracker::default();
    let mut update_check = RequestTracker::default();

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
        Ok(system) => {
            if let Err(err) = system.req_tx.send(LocalEnvReq::Refresh) {
                app.local_env_loading = false;
                app.push_toast(
                    texts::tui_toast_local_env_check_request_failed(&err.to_string()),
                    ToastKind::Warning,
                );
            }
            Some(system)
        }
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
    queue_current_quota_refresh_if_due(&mut app, &mut data, quota.as_ref().map(|s| &s.req_tx));

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
        Ok(system) => {
            queue_managed_auth_refresh(&mut app, Some(&system.req_tx), "codex_oauth");
            Some(system)
        }
        Err(err) => {
            app.managed_auth_loading = false;
            app.push_toast(
                texts::tui_toast_managed_auth_worker_unavailable(&err.to_string()),
                ToastKind::Warning,
            );
            None
        }
    };

    loop {
        app.last_size = terminal.size()?;
        app.observe_proxy_visual_state(&data);
        let frame_dt = last_frame.elapsed();
        last_frame = Instant::now();
        terminal.draw(|f| {
            let area = f.area();
            proxy_open_flash.sync(&app, area);
            ui::render(f, &app, &data);
            proxy_open_flash.process(frame_dt, f.buffer_mut(), area);
        })?;

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
                if let Err(err) = handle_proxy_msg(&mut app, &mut data, &mut proxy_loading, msg) {
                    app.push_toast(err.to_string(), ToastKind::Error);
                }
            }
        }

        if let Some(quota) = quota.as_ref() {
            while let Ok(msg) = quota.result_rx.try_recv() {
                handle_quota_msg(&mut app, &mut data, msg);
            }
        }

        if let Some(skills) = skills.as_ref() {
            while let Ok(msg) = skills.result_rx.try_recv() {
                if let Err(err) = handle_skills_msg(&mut app, &mut data, msg) {
                    app.push_toast(err.to_string(), ToastKind::Error);
                }
            }
        }

        if let Some(webdav) = webdav.as_ref() {
            while let Ok(msg) = webdav.result_rx.try_recv() {
                if let Err(err) = handle_webdav_msg(&mut app, &mut data, &mut webdav_loading, msg) {
                    app.push_toast(err.to_string(), ToastKind::Error);
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
                    let action = app.on_key(key, &data);
                    if let Action::ProviderQuotaRefresh { id } = action {
                        queue_provider_quota_refresh(
                            &mut app,
                            &mut data,
                            quota.as_ref().map(|s| &s.req_tx),
                            &id,
                        );
                    } else if let Err(err) = handle_action(
                        &mut terminal,
                        &mut app,
                        &mut data,
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
                        action,
                    ) {
                        if matches!(
                            &err,
                            AppError::Localized { key, .. } if *key == "tui_terminal_error"
                        ) {
                            return Err(err);
                        }
                        app.push_toast(err.to_string(), ToastKind::Error);
                    }
                }
                event::Event::Mouse(mouse) => {
                    if let MouseEventKind::ScrollUp | MouseEventKind::ScrollDown = mouse.kind {
                        let code = if matches!(mouse.kind, MouseEventKind::ScrollUp) {
                            event::KeyCode::Up
                        } else {
                            event::KeyCode::Down
                        };
                        let key = event::KeyEvent::new(code, event::KeyModifiers::NONE);
                        let action = app.on_key(key, &data);
                        if let Action::ProviderQuotaRefresh { id } = action {
                            queue_provider_quota_refresh(
                                &mut app,
                                &mut data,
                                quota.as_ref().map(|s| &s.req_tx),
                                &id,
                            );
                        } else if let Err(err) = handle_action(
                            &mut terminal,
                            &mut app,
                            &mut data,
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
                            action,
                        ) {
                            if matches!(
                                &err,
                                AppError::Localized { key, .. } if *key == "tui_terminal_error"
                            ) {
                                return Err(err);
                            }
                            app.push_toast(err.to_string(), ToastKind::Error);
                        }
                    }
                }
                event::Event::Resize(_, _) => {}
                _ => {}
            }
        }

        if last_tick.elapsed() >= tick_rate {
            app.on_tick();
            if app.should_poll_proxy_activity() {
                if let Err(err) = data.refresh_proxy_snapshot(&app.app_type) {
                    log::debug!("refresh proxy snapshot failed: {err}");
                } else {
                    app.observe_proxy_token_activity(
                        data.proxy.estimated_input_tokens_total,
                        data.proxy.estimated_output_tokens_total,
                    );
                }
            }
            queue_current_quota_refresh_if_due(
                &mut app,
                &mut data,
                quota.as_ref().map(|s| &s.req_tx),
            );
            last_tick = Instant::now();
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
