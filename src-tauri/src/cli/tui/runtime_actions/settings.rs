use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::error::AppError;

use super::super::data::{load_proxy_config, load_state, UiData};
use super::helpers::open_proxy_help_overlay_with;
use super::RuntimeActionContext;

pub(super) fn set_proxy_enabled(
    ctx: &mut RuntimeActionContext<'_>,
    enabled: bool,
) -> Result<(), AppError> {
    let state = load_state()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("failed to create async runtime: {e}")))?;
    runtime.block_on(state.proxy_service.set_global_enabled(enabled))?;
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
    update_proxy_config(ctx, |config| {
        config.listen_port = port;
    })
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

pub(super) fn set_proxy_takeover(
    ctx: &mut RuntimeActionContext<'_>,
    app_type: AppType,
    enabled: bool,
) -> Result<(), AppError> {
    let state = load_state()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("failed to create async runtime: {e}")))?;

    let status = runtime.block_on(state.proxy_service.get_status());
    if enabled && !status.running {
        ctx.app.push_toast(
            texts::tui_toast_proxy_takeover_requires_running(),
            super::super::app::ToastKind::Warning,
        );
        return Ok(());
    }

    runtime
        .block_on(
            state
                .proxy_service
                .set_takeover_for_app(app_type.as_str(), enabled),
        )
        .map_err(AppError::Message)?;

    *ctx.data = UiData::load(&ctx.app.app_type)?;
    open_proxy_help_overlay_with(ctx.app, ctx.data, load_proxy_config)?;
    ctx.app.push_toast(
        texts::tui_toast_proxy_takeover_updated(app_type.as_str(), enabled),
        super::super::app::ToastKind::Success,
    );
    Ok(())
}

pub(super) fn set_visible_apps(
    ctx: &mut RuntimeActionContext<'_>,
    apps: crate::settings::VisibleApps,
) -> Result<(), AppError> {
    set_visible_apps_with(ctx, apps, UiData::load)
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
            texts::tui_toast_proxy_settings_stop_before_edit(),
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

pub(super) fn set_upstream_proxy_url(
    ctx: &mut RuntimeActionContext<'_>,
    url: Option<String>,
) -> Result<(), AppError> {
    let state = load_state()?;

    // Save to database
    state.db.set_global_proxy_url(url.as_deref())?;

    // Get current enabled state
    let enabled = state.db.get_global_proxy_enabled()?;

    // Update HTTP client based on enabled state
    let effective_url = if enabled {
        url.as_deref().filter(|u| !u.trim().is_empty())
    } else {
        None
    };
    crate::proxy::http_client::update_proxy(effective_url)
        .map_err(|e| AppError::Message(format!("Failed to update HTTP client: {}", e)))?;

    *ctx.data = UiData::load(&ctx.app.app_type)?;
    ctx.app.push_toast(
        if url.is_some() {
            crate::t!("Upstream proxy URL updated.", "上游代理 URL 已更新。")
        } else {
            crate::t!("Upstream proxy cleared.", "上游代理已清除。")
        },
        super::super::app::ToastKind::Success,
    );
    Ok(())
}

pub(super) fn set_upstream_proxy_enabled(
    ctx: &mut RuntimeActionContext<'_>,
    enabled: bool,
) -> Result<(), AppError> {
    let state = load_state()?;

    // Set enabled state in database
    state.db.set_global_proxy_enabled(enabled)?;

    // Get current URL
    let url = state.db.get_global_proxy_url()?;

    // Update HTTP client based on enabled state and URL
    let effective_url = if enabled {
        url.as_deref().filter(|u| !u.trim().is_empty())
    } else {
        None
    };
    crate::proxy::http_client::update_proxy(effective_url)
        .map_err(|e| AppError::Message(format!("Failed to update HTTP client: {}", e)))?;

    *ctx.data = UiData::load(&ctx.app.app_type)?;
    ctx.app.push_toast(
        if enabled {
            crate::t!("Upstream proxy enabled.", "上游代理已开启。")
        } else {
            crate::t!("Upstream proxy disabled.", "上游代理已关闭。")
        },
        super::super::app::ToastKind::Success,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;
    use std::ffi::OsString;
    use std::path::Path;
    use tempfile::TempDir;

    use crate::app_config::AppType;
    use crate::cli::tui::app::App;
    use crate::cli::tui::data::UiData;
    use crate::cli::tui::runtime_systems::RequestTracker;
    use crate::cli::tui::terminal::TuiTerminal;
    use crate::provider::Provider;
    use crate::services::ProviderService;
    use crate::test_support::{
        lock_test_home_and_settings, set_test_home_override, TestHomeSettingsLock,
    };

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
    fn set_openclaw_config_dir_persists_override_and_syncs_live_config() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let target_dir = temp_home.path().join("wsl-openclaw");
        std::fs::create_dir_all(&target_dir).expect("create target openclaw dir");

        let state = crate::store::AppState::try_new().expect("create state");
        ProviderService::add(
            &state,
            AppType::OpenClaw,
            Provider::with_id(
                "demo".to_string(),
                "Demo".to_string(),
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
            webdav_req_tx: None,
            webdav_loading: &mut webdav_loading,
            update_req_tx: None,
            update_check: &mut update_check,
            model_fetch_req_tx: None,
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
            value["models"]["providers"]["demo"]["baseUrl"],
            json!("https://demo.example/v1")
        );
        assert_eq!(
            value["models"]["providers"]["demo"]["models"][0]["id"],
            json!("demo-model")
        );
    }

    #[test]
    fn set_openclaw_config_dir_none_clears_override_and_falls_back_to_default_path() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

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
            webdav_req_tx: None,
            webdav_loading: &mut webdav_loading,
            update_req_tx: None,
            update_check: &mut update_check,
            model_fetch_req_tx: None,
        };

        set_openclaw_config_dir(&mut ctx, None).expect("clear openclaw config dir");

        assert_eq!(crate::settings::get_settings().openclaw_config_dir, None);
        assert_eq!(
            ctx.data.config.openclaw_config_path.as_ref(),
            Some(&temp_home.path().join(".openclaw").join("openclaw.json"))
        );
    }
}
