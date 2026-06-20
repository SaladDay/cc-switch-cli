use crate::app_config::AppType;
use crate::cli::failover_policy::{
    ensure_auto_failover_queue_ready, ensure_auto_failover_ready, inspect_auto_failover_gate,
};
use crate::cli::i18n::texts;
use crate::error::AppError;

use super::super::app::ToastKind;
use super::super::data::{load_state, UiData};
use super::super::runtime_systems::ManagedAuthReq;
use super::RuntimeActionContext;

fn visible_apps_mode_label(mode: crate::settings::VisibleAppsMode) -> &'static str {
    match mode {
        crate::settings::VisibleAppsMode::Auto => texts::tui_settings_visible_apps_mode_auto(),
        crate::settings::VisibleAppsMode::Manual => texts::tui_settings_visible_apps_mode_manual(),
    }
}

pub(super) fn set_proxy_enabled(
    ctx: &mut RuntimeActionContext<'_>,
    enabled: bool,
) -> Result<(), AppError> {
    let state = load_state()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("failed to create async runtime: {e}")))?;
    runtime
        .block_on(
            state
                .proxy_service
                .set_managed_session_for_app(ctx.app.app_type.as_str(), enabled),
        )
        .map_err(AppError::Message)?;

    *ctx.data = UiData::load(&ctx.app.app_type)?;
    ctx.app.push_toast(
        if enabled {
            crate::t!("Local proxy enabled.", "本地代理已开启。")
        } else {
            crate::t!("Local proxy disabled.", "本地代理已关闭。")
        },
        super::super::app::ToastKind::Success,
    );
    Ok(())
}

pub(super) fn set_proxy_listen_address(
    ctx: &mut RuntimeActionContext<'_>,
    address: String,
) -> Result<(), AppError> {
    update_proxy_config(ctx, |config| {
        config.listen_address = address;
    })
}

pub(super) fn set_proxy_listen_port(
    ctx: &mut RuntimeActionContext<'_>,
    port: u16,
) -> Result<(), AppError> {
    let state = load_state()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("failed to create async runtime: {e}")))?;
    let status = runtime.block_on(state.proxy_service.get_status());
    let app_running = status
        .active_workers
        .iter()
        .any(|worker| worker.app_type == ctx.app.app_type.as_str());
    if app_running {
        *ctx.data = UiData::load(&ctx.app.app_type)?;
        ctx.app.push_toast(
            texts::tui_toast_proxy_settings_stop_app_route_before_edit_port(),
            super::super::app::ToastKind::Info,
        );
        return Ok(());
    }

    state
        .db
        .set_app_proxy_preferred_port(ctx.app.app_type.as_str(), port)?;

    *ctx.data = UiData::load(&ctx.app.app_type)?;
    ctx.app.push_toast(
        texts::tui_toast_proxy_settings_saved(),
        super::super::app::ToastKind::Success,
    );
    Ok(())
}

pub(super) fn set_proxy_auto_failover(
    ctx: &mut RuntimeActionContext<'_>,
    app_type: AppType,
    enabled: bool,
) -> Result<(), AppError> {
    let state = load_state()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("failed to create async runtime: {e}")))?;

    if enabled {
        let gate = runtime.block_on(inspect_auto_failover_gate(&state, &app_type))?;
        ensure_auto_failover_ready(&gate)?;
    }

    if enabled {
        runtime
            .block_on(
                state
                    .proxy_service
                    .enable_auto_failover_for_app(app_type.as_str()),
            )
            .map_err(AppError::Message)?;
    } else {
        runtime
            .block_on(
                state
                    .proxy_service
                    .set_auto_failover_for_app(app_type.as_str(), false),
            )
            .map_err(AppError::Message)?;
    }

    *ctx.data = UiData::load(&ctx.app.app_type)?;
    ctx.app.push_toast(
        if enabled {
            crate::t!("Automatic failover enabled.", "自动故障转移已开启。")
        } else {
            crate::t!("Automatic failover disabled.", "自动故障转移已关闭。")
        },
        super::super::app::ToastKind::Success,
    );
    Ok(())
}

pub(super) fn enable_proxy_and_auto_failover(
    ctx: &mut RuntimeActionContext<'_>,
    app_type: AppType,
) -> Result<(), AppError> {
    let state = load_state()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("failed to create async runtime: {e}")))?;
    let gate = runtime.block_on(inspect_auto_failover_gate(&state, &app_type))?;
    ensure_auto_failover_queue_ready(&gate)?;

    runtime
        .block_on(async {
            state
                .proxy_service
                .enable_proxy_and_auto_failover_for_app(app_type.as_str())
                .await
        })
        .map_err(AppError::Message)?;

    *ctx.data = UiData::load(&ctx.app.app_type)?;
    ctx.app.push_toast(
        crate::t!(
            "Proxy routing and automatic failover enabled.",
            "代理路由和自动故障转移已开启。"
        ),
        super::super::app::ToastKind::Success,
    );
    Ok(())
}

pub(super) fn set_openclaw_config_dir(
    ctx: &mut RuntimeActionContext<'_>,
    path: Option<String>,
) -> Result<(), AppError> {
    let mut settings = crate::settings::get_settings();
    settings.openclaw_config_dir = path;
    crate::settings::update_settings(settings)?;

    let state = load_state()?;
    let sync_result = if crate::sync_policy::should_sync_live(&AppType::OpenClaw) {
        crate::services::ProviderService::sync_openclaw_to_live(&state).err()
    } else {
        None
    };

    *ctx.data = UiData::load(&ctx.app.app_type)?;
    ctx.app.push_toast(
        texts::tui_toast_openclaw_config_dir_saved(),
        super::super::app::ToastKind::Success,
    );

    if !crate::sync_policy::should_sync_live(&AppType::OpenClaw) {
        ctx.app.push_toast(
            texts::tui_toast_openclaw_config_dir_sync_skipped(),
            super::super::app::ToastKind::Warning,
        );
    } else if let Some(err) = sync_result {
        ctx.app.push_toast(
            texts::tui_toast_openclaw_config_dir_sync_failed(&err.to_string()),
            super::super::app::ToastKind::Warning,
        );
    }

    Ok(())
}

pub(super) fn set_visible_apps(
    ctx: &mut RuntimeActionContext<'_>,
    apps: crate::settings::VisibleApps,
) -> Result<(), AppError> {
    set_visible_apps_with(ctx, apps, UiData::load)
}

pub(super) fn switch_visible_apps_to_manual(
    ctx: &mut RuntimeActionContext<'_>,
    apps: crate::settings::VisibleApps,
    selected: usize,
) -> Result<(), AppError> {
    if apps.ordered_enabled().is_empty() {
        ctx.app.push_toast(
            texts::tui_toast_visible_apps_zero_selection_warning(),
            super::super::app::ToastKind::Warning,
        );
        ctx.app.overlay = super::super::app::Overlay::VisibleAppsPicker {
            selected,
            apps: crate::settings::get_visible_apps(),
        };
        return Ok(());
    }

    let next_app_data = if apps.is_enabled_for(&ctx.app.app_type) {
        None
    } else {
        let next = match crate::settings::next_visible_app(&apps, &ctx.app.app_type, 1) {
            Some(next) => next,
            None => {
                ctx.app.overlay = super::super::app::Overlay::VisibleAppsPicker {
                    selected,
                    apps: crate::settings::get_visible_apps(),
                };
                return Err(AppError::InvalidInput(
                    "At least one app must remain visible".to_string(),
                ));
            }
        };
        match UiData::load(&next) {
            Ok(next_data) => Some((next, next_data)),
            Err(err) => {
                ctx.app.overlay = super::super::app::Overlay::VisibleAppsPicker {
                    selected,
                    apps: crate::settings::get_visible_apps(),
                };
                return Err(err);
            }
        }
    };

    let mut settings = crate::settings::get_settings();
    settings.visible_apps = apps;
    settings.visible_apps_settings.mode = crate::settings::VisibleAppsMode::Manual;
    settings.visible_apps_settings.auto_prompt_decided = true;
    if let Err(err) = crate::settings::update_settings(settings) {
        ctx.app.overlay = super::super::app::Overlay::VisibleAppsPicker {
            selected,
            apps: crate::settings::get_visible_apps(),
        };
        return Err(err);
    }

    if let Some((next, next_data)) = next_app_data {
        super::apply_preloaded_app_switch(ctx.app, ctx.data, next, next_data);
    }

    ctx.app.push_toast(
        texts::tui_toast_visible_apps_saved(),
        super::super::app::ToastKind::Success,
    );
    ctx.app.overlay = super::super::app::Overlay::VisibleAppsPicker {
        selected,
        apps: crate::settings::get_visible_apps(),
    };
    Ok(())
}

pub(super) fn managed_auth_refresh(
    ctx: &mut RuntimeActionContext<'_>,
    auth_provider: String,
) -> Result<(), AppError> {
    let Some(tx) = ctx.managed_auth_req_tx else {
        ctx.app.managed_auth_loading = false;
        ctx.app.push_toast(
            texts::tui_toast_managed_auth_worker_unavailable(
                texts::tui_error_managed_auth_worker_unavailable(),
            ),
            ToastKind::Warning,
        );
        return Ok(());
    };

    ctx.app.managed_auth_loading = true;
    if let Err(err) = tx.send(ManagedAuthReq::Refresh { auth_provider }) {
        ctx.app.managed_auth_loading = false;
        ctx.app.push_toast(
            texts::tui_toast_managed_auth_request_failed(&err.to_string()),
            ToastKind::Warning,
        );
    }

    Ok(())
}

pub(super) fn managed_auth_start_login(
    ctx: &mut RuntimeActionContext<'_>,
    auth_provider: String,
) -> Result<(), AppError> {
    let Some(tx) = ctx.managed_auth_req_tx else {
        ctx.app.push_toast(
            texts::tui_toast_managed_auth_worker_unavailable(
                texts::tui_error_managed_auth_worker_unavailable(),
            ),
            ToastKind::Warning,
        );
        return Ok(());
    };

    ctx.app.managed_auth_loading = true;
    if let Err(err) = tx.send(ManagedAuthReq::StartLogin { auth_provider }) {
        ctx.app.managed_auth_loading = false;
        ctx.app.push_toast(
            texts::tui_toast_managed_auth_request_failed(&err.to_string()),
            ToastKind::Warning,
        );
    }

    Ok(())
}

pub(super) fn managed_auth_set_default(
    ctx: &mut RuntimeActionContext<'_>,
    auth_provider: String,
    account_id: String,
) -> Result<(), AppError> {
    let Some(tx) = ctx.managed_auth_req_tx else {
        ctx.app.push_toast(
            texts::tui_toast_managed_auth_worker_unavailable(
                texts::tui_error_managed_auth_worker_unavailable(),
            ),
            ToastKind::Warning,
        );
        return Ok(());
    };

    ctx.app.managed_auth_loading = true;
    if let Err(err) = tx.send(ManagedAuthReq::SetDefault {
        auth_provider,
        account_id,
    }) {
        ctx.app.managed_auth_loading = false;
        ctx.app.push_toast(
            texts::tui_toast_managed_auth_request_failed(&err.to_string()),
            ToastKind::Warning,
        );
    }

    Ok(())
}

pub(super) fn managed_auth_remove(
    ctx: &mut RuntimeActionContext<'_>,
    auth_provider: String,
    account_id: String,
) -> Result<(), AppError> {
    let Some(tx) = ctx.managed_auth_req_tx else {
        ctx.app.push_toast(
            texts::tui_toast_managed_auth_worker_unavailable(
                texts::tui_error_managed_auth_worker_unavailable(),
            ),
            ToastKind::Warning,
        );
        return Ok(());
    };

    ctx.app.managed_auth_loading = true;
    if let Err(err) = tx.send(ManagedAuthReq::Remove {
        auth_provider,
        account_id,
    }) {
        ctx.app.managed_auth_loading = false;
        ctx.app.push_toast(
            texts::tui_toast_managed_auth_request_failed(&err.to_string()),
            ToastKind::Warning,
        );
    }

    Ok(())
}

pub(super) fn set_visible_apps_mode(
    ctx: &mut RuntimeActionContext<'_>,
    mode: crate::settings::VisibleAppsMode,
) -> Result<(), AppError> {
    crate::settings::set_visible_apps_mode(mode)?;
    if mode == crate::settings::VisibleAppsMode::Auto {
        let detection = crate::services::visible_apps::detect_visible_app_installation();
        let outcome = crate::services::visible_apps::apply_startup_policy(&detection)?;
        for notice in &outcome.notices {
            ctx.app.push_toast(
                crate::services::visible_apps::notice_message(notice),
                super::super::app::ToastKind::Info,
            );
        }
        switch_current_app_if_hidden(ctx)?;
        if !outcome.notices.is_empty() {
            return Ok(());
        }
    }

    ctx.app.push_toast(
        texts::tui_toast_visible_apps_mode_saved(visible_apps_mode_label(mode)),
        super::super::app::ToastKind::Success,
    );
    Ok(())
}

pub(super) fn confirm_visible_apps_auto_detection(
    ctx: &mut RuntimeActionContext<'_>,
    use_auto: bool,
) -> Result<(), AppError> {
    let detection = crate::services::visible_apps::detect_visible_app_installation();
    if use_auto {
        let changed = crate::services::visible_apps::accept_auto_detection(&detection)?;
        if !changed.is_empty() {
            let notice =
                crate::services::visible_apps::VisibleAppsNotice::AutoUpdated { apps: changed };
            ctx.app.push_toast(
                crate::services::visible_apps::notice_message(&notice),
                super::super::app::ToastKind::Info,
            );
        } else {
            ctx.app.push_toast(
                texts::tui_toast_visible_apps_mode_saved(
                    texts::tui_settings_visible_apps_mode_auto(),
                ),
                super::super::app::ToastKind::Success,
            );
        }
    } else {
        crate::services::visible_apps::keep_manual_visibility(&detection)?;
        ctx.app.push_toast(
            texts::tui_toast_visible_apps_mode_saved(texts::tui_settings_visible_apps_mode_manual()),
            super::super::app::ToastKind::Success,
        );
    }

    switch_current_app_if_hidden(ctx)?;

    Ok(())
}

fn switch_current_app_if_hidden(ctx: &mut RuntimeActionContext<'_>) -> Result<(), AppError> {
    let visible_apps = crate::settings::get_visible_apps();
    if visible_apps.is_enabled_for(&ctx.app.app_type) {
        return Ok(());
    }

    let next = crate::settings::next_visible_app(&visible_apps, &ctx.app.app_type, 1)
        .unwrap_or_else(|| ctx.app.app_type.clone());
    if next == ctx.app.app_type {
        return Ok(());
    }

    let next_data = UiData::load(&next)?;
    super::apply_preloaded_app_switch(ctx.app, ctx.data, next, next_data);
    Ok(())
}

pub(super) fn set_visible_apps_with<F>(
    ctx: &mut RuntimeActionContext<'_>,
    apps: crate::settings::VisibleApps,
    load_data: F,
) -> Result<(), AppError>
where
    F: FnOnce(&AppType) -> Result<UiData, AppError>,
{
    if apps.ordered_enabled().is_empty() {
        ctx.app.push_toast(
            texts::tui_toast_visible_apps_zero_selection_warning(),
            super::super::app::ToastKind::Warning,
        );
        return Ok(());
    }

    if apps.is_enabled_for(&ctx.app.app_type) {
        crate::settings::set_visible_apps(apps)?;
        ctx.app.push_toast(
            texts::tui_toast_visible_apps_saved(),
            super::super::app::ToastKind::Success,
        );
        return Ok(());
    }

    let next = crate::settings::next_visible_app(&apps, &ctx.app.app_type, 1).ok_or_else(|| {
        AppError::InvalidInput("At least one app must remain visible".to_string())
    })?;
    let next_data = load_data(&next)?;

    crate::settings::set_visible_apps(apps)?;
    super::apply_preloaded_app_switch(ctx.app, ctx.data, next, next_data);
    ctx.app.push_toast(
        texts::tui_toast_visible_apps_saved(),
        super::super::app::ToastKind::Success,
    );
    Ok(())
}

fn update_proxy_config(
    ctx: &mut RuntimeActionContext<'_>,
    mutate: impl FnOnce(&mut crate::proxy::ProxyConfig),
) -> Result<(), AppError> {
    let state = load_state()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("failed to create async runtime: {e}")))?;

    let status = runtime.block_on(state.proxy_service.get_status());
    if status.running {
        *ctx.data = UiData::load(&ctx.app.app_type)?;
        ctx.app.push_toast(
            texts::tui_toast_proxy_settings_stop_proxy_before_edit_address(),
            super::super::app::ToastKind::Info,
        );
        return Ok(());
    }

    let mut config = runtime.block_on(state.proxy_service.get_config())?;
    mutate(&mut config);
    runtime.block_on(state.proxy_service.update_config(&config))?;

    *ctx.data = UiData::load(&ctx.app.app_type)?;
    ctx.app.push_toast(
        texts::tui_toast_proxy_settings_saved(),
        super::super::app::ToastKind::Success,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;
    use tempfile::TempDir;

    use crate::app_config::AppType;
    use crate::cli::tui::app::App;
    use crate::cli::tui::data::UiData;
    use crate::cli::tui::runtime_systems::RequestTracker;
    use crate::cli::tui::terminal::TuiTerminal;
    use crate::provider::Provider;
    use crate::services::ProviderService;
    use crate::test_support::TestEnvGuard;

    #[test]
    fn set_openclaw_config_dir_persists_override_and_syncs_live_config() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());

        let target_dir = temp_home.path().join("wsl-openclaw");
        std::fs::create_dir_all(&target_dir).expect("create target openclaw dir");

        let state = crate::store::AppState::try_new().expect("create state");
        ProviderService::add(
            &state,
            AppType::OpenClaw,
            Provider::with_id(
                "settings-dir-demo".to_string(),
                "Settings Dir Demo".to_string(),
                json!({
                    "apiKey": "sk-demo",
                    "baseUrl": "https://demo.example/v1",
                    "models": [{ "id": "demo-model" }]
                }),
                None,
            ),
        )
        .expect("add openclaw provider");

        let mut terminal = TuiTerminal::new_for_test().expect("create test terminal");
        let mut app = App::new(Some(AppType::OpenClaw));
        let mut data = UiData::load(&AppType::OpenClaw).expect("load ui data");
        let mut proxy_loading = RequestTracker::default();
        let mut webdav_loading = RequestTracker::default();
        let mut update_check = RequestTracker::default();
        let mut ctx = RuntimeActionContext {
            terminal: &mut terminal,
            app: &mut app,
            data: &mut data,
            speedtest_req_tx: None,
            stream_check_req_tx: None,
            skills_req_tx: None,
            proxy_req_tx: None,
            proxy_loading: &mut proxy_loading,
            local_env_req_tx: None,
            session_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut webdav_loading,
            update_req_tx: None,
            update_check: &mut update_check,
            model_fetch_req_tx: None,
            managed_auth_req_tx: None,
        };

        set_openclaw_config_dir(&mut ctx, Some(target_dir.display().to_string()))
            .expect("save openclaw config dir");

        assert_eq!(
            crate::settings::get_settings().openclaw_config_dir,
            Some(target_dir.display().to_string())
        );
        assert_eq!(
            ctx.data.config.openclaw_config_path.as_ref(),
            Some(&target_dir.join("openclaw.json"))
        );

        let live_path = target_dir.join("openclaw.json");
        let source = std::fs::read_to_string(&live_path).expect("read synced openclaw config");
        let value: serde_json::Value =
            json5::from_str(&source).expect("parse synced openclaw config as json5");
        assert_eq!(
            value["models"]["providers"]["settings-dir-demo"]["baseUrl"],
            json!("https://demo.example/v1")
        );
        assert_eq!(
            value["models"]["providers"]["settings-dir-demo"]["models"][0]["id"],
            json!("demo-model")
        );
    }

    #[test]
    fn set_openclaw_config_dir_none_clears_override_and_falls_back_to_default_path() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());

        let override_dir = temp_home.path().join("custom-openclaw");
        std::fs::create_dir_all(&override_dir).expect("create override dir");
        let mut settings = crate::settings::get_settings();
        settings.openclaw_config_dir = Some(override_dir.display().to_string());
        crate::settings::update_settings(settings).expect("seed openclaw override");

        let mut terminal = TuiTerminal::new_for_test().expect("create test terminal");
        let mut app = App::new(Some(AppType::OpenClaw));
        let mut data = UiData::load(&AppType::OpenClaw).expect("load ui data");
        let mut proxy_loading = RequestTracker::default();
        let mut webdav_loading = RequestTracker::default();
        let mut update_check = RequestTracker::default();
        let mut ctx = RuntimeActionContext {
            terminal: &mut terminal,
            app: &mut app,
            data: &mut data,
            speedtest_req_tx: None,
            stream_check_req_tx: None,
            skills_req_tx: None,
            proxy_req_tx: None,
            proxy_loading: &mut proxy_loading,
            local_env_req_tx: None,
            session_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut webdav_loading,
            update_req_tx: None,
            update_check: &mut update_check,
            model_fetch_req_tx: None,
            managed_auth_req_tx: None,
        };

        set_openclaw_config_dir(&mut ctx, None).expect("clear openclaw config dir");

        assert_eq!(crate::settings::get_settings().openclaw_config_dir, None);
        assert_eq!(
            ctx.data.config.openclaw_config_path.as_ref(),
            Some(&temp_home.path().join(".openclaw").join("openclaw.json"))
        );
    }
}
