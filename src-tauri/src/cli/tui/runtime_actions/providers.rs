use std::path::Path;

use crate::cli::i18n::texts;
use crate::cli::tui::form::ClaudeApiFormat;
use crate::codex_config::{get_codex_auth_path, get_codex_config_path};
use crate::config::get_claude_settings_path;
use crate::error::AppError;
use crate::proxy::providers::get_claude_api_format;
use crate::services::ProviderService;

use super::super::app::{ConfirmAction, ConfirmOverlay, Overlay, ToastKind};
use super::super::data::{load_state, UiData};
use super::super::form::ProviderAddField;
use super::super::runtime_systems::{next_model_fetch_request_id, ModelFetchReq, StreamCheckReq};
use super::RuntimeActionContext;

pub(super) fn switch(ctx: &mut RuntimeActionContext<'_>, id: String) -> Result<(), AppError> {
    if let Some((title, message)) = provider_switch_first_use_guard_content(ctx) {
        ctx.app.pending_overlay = None;
        ctx.app.overlay = Overlay::ProviderSwitchFirstUseConfirm {
            provider_id: id,
            title,
            message,
            selected: 0,
        };
        return Ok(());
    }

    switch_force(ctx, id)
}

pub(super) fn switch_force(ctx: &mut RuntimeActionContext<'_>, id: String) -> Result<(), AppError> {
    do_switch(ctx, id)
}

pub(super) fn import_live_config(ctx: &mut RuntimeActionContext<'_>) -> Result<(), AppError> {
    let state = load_state()?;
    let settings_config = ProviderService::read_live_settings(ctx.app.app_type.clone())?;
    let provider_id = next_imported_live_provider_id(&ctx.data.providers.rows);
    let provider_name = match ctx.app.app_type {
        crate::app_config::AppType::Codex => texts::tui_codex_imported_live_config_name(),
        _ => texts::tui_provider_imported_live_config_name(),
    };
    let mut provider = crate::provider::Provider::with_id(
        provider_id.clone(),
        provider_name.to_string(),
        settings_config,
        None,
    );
    provider.category = Some("custom".to_string());
    provider.created_at = Some(current_timestamp());

    match ctx.app.app_type {
        crate::app_config::AppType::Codex => {
            let mut config = state.config.write().map_err(AppError::from)?;
            let manager = config.get_manager_mut(&ctx.app.app_type).ok_or_else(|| {
                AppError::localized(
                    "app.not_found",
                    format!("应用未初始化: {}", ctx.app.app_type.as_str()),
                    format!("App not initialized: {}", ctx.app.app_type.as_str()),
                )
            })?;
            manager.providers.insert(provider.id.clone(), provider);
            manager.current = provider_id;
            drop(config);
            state.save()?;
        }
        _ => {
            ProviderService::add(&state, ctx.app.app_type.clone(), provider)?;
            ProviderService::switch(&state, ctx.app.app_type.clone(), &provider_id)?;
        }
    }

    *ctx.data = UiData::load(&ctx.app.app_type)?;
    ctx.app.pending_overlay = None;
    let toast_message = match ctx.app.app_type {
        crate::app_config::AppType::Codex => texts::tui_toast_codex_live_config_imported(),
        _ => texts::tui_toast_provider_live_config_imported(),
    };
    ctx.app.push_toast(toast_message, ToastKind::Success);
    Ok(())
}

fn do_switch(ctx: &mut RuntimeActionContext<'_>, id: String) -> Result<(), AppError> {
    let state = load_state()?;
    let previous_current_id = ctx.data.providers.current_id.clone();
    let switched_provider = ctx
        .data
        .providers
        .rows
        .iter()
        .find(|row| row.id == id)
        .map(|row| row.provider.clone());
    ProviderService::switch(&state, ctx.app.app_type.clone(), &id)?;
    if let Some(provider) = switched_provider.as_ref() {
        if let Err(err) =
            crate::claude_plugin::sync_claude_plugin_on_provider_switch(&ctx.app.app_type, provider)
        {
            ctx.app.push_toast(
                texts::tui_toast_claude_plugin_sync_failed(&err.to_string()),
                ToastKind::Warning,
            );
        }
    }
    *ctx.data = UiData::load(&ctx.app.app_type)?;
    ctx.app.pending_overlay = None;

    let proxy_ready = ctx
        .data
        .proxy
        .routes_current_app_through_proxy(&ctx.app.app_type)
        .unwrap_or(false);
    let proxy_overlay = switched_provider.as_ref().and_then(|provider| {
        provider_switch_proxy_notice_overlay(&ctx.app.app_type, provider, proxy_ready)
    });
    let shared_config_overlay =
        maybe_provider_switch_shared_config_notice(&ctx.app.app_type, &previous_current_id, &id)?;

    match (proxy_overlay, shared_config_overlay) {
        (Some(proxy_overlay), Some(shared_config_overlay)) => {
            ctx.app.overlay = proxy_overlay;
            ctx.app.pending_overlay = Some(shared_config_overlay);
        }
        (Some(proxy_overlay), None) => {
            ctx.app.overlay = proxy_overlay;
        }
        (None, Some(shared_config_overlay)) => {
            ctx.app.overlay = shared_config_overlay;
        }
        (None, None) => {
            ctx.app.overlay = Overlay::None;
        }
    }

    Ok(())
}

fn provider_switch_first_use_guard_content(
    ctx: &RuntimeActionContext<'_>,
) -> Option<(String, String)> {
    if !ctx.data.providers.current_id.trim().is_empty() {
        return None;
    }

    match ctx.app.app_type {
        crate::app_config::AppType::Claude => {
            let path = get_claude_settings_path();
            path.exists().then(|| {
                let display = display_path_with_tilde(&path);
                (
                    texts::tui_provider_switch_first_use_title().to_string(),
                    texts::tui_provider_switch_first_use_message(&display),
                )
            })
        }
        crate::app_config::AppType::Codex => {
            let config_path = get_codex_config_path();
            if !config_path.exists() {
                return None;
            }

            let auth_path = get_codex_auth_path();
            let mut paths = vec![display_path_with_tilde(&config_path)];
            if auth_path.exists() {
                paths.push(display_path_with_tilde(&auth_path));
            }

            let joined = paths.join(", ");
            Some((
                texts::tui_codex_provider_switch_first_use_title().to_string(),
                texts::tui_codex_provider_switch_first_use_message(&joined),
            ))
        }
        _ => None,
    }
}

fn provider_switch_proxy_notice_overlay(
    app_type: &crate::app_config::AppType,
    provider: &crate::provider::Provider,
    proxy_ready: bool,
) -> Option<Overlay> {
    provider_switch_proxy_notice_api_format(app_type, provider, proxy_ready).map(|api_format| {
        Overlay::Confirm(ConfirmOverlay {
            title: texts::tui_claude_api_format_requires_proxy_title().to_string(),
            message: texts::tui_claude_api_format_requires_proxy_message(api_format),
            action: ConfirmAction::ProviderApiFormatProxyNotice,
        })
    })
}

fn maybe_provider_switch_shared_config_notice(
    app_type: &crate::app_config::AppType,
    previous_current_id: &str,
    next_provider_id: &str,
) -> Result<Option<Overlay>, AppError> {
    if !matches!(
        app_type,
        crate::app_config::AppType::Claude | crate::app_config::AppType::Codex
    ) {
        return Ok(None);
    }

    if previous_current_id.trim().is_empty() || previous_current_id == next_provider_id {
        return Ok(None);
    }

    let already_shown = match app_type {
        crate::app_config::AppType::Claude => {
            crate::settings::get_provider_switch_common_config_tip_shown()
        }
        crate::app_config::AppType::Codex => {
            crate::settings::get_provider_switch_common_config_tip_shown_codex()
        }
        _ => false,
    };
    if already_shown {
        return Ok(None);
    }

    match app_type {
        crate::app_config::AppType::Claude => {
            crate::settings::set_provider_switch_common_config_tip_shown(true)?;
        }
        crate::app_config::AppType::Codex => {
            crate::settings::set_provider_switch_common_config_tip_shown_codex(true)?;
        }
        _ => {}
    }

    let message = match app_type {
        crate::app_config::AppType::Codex => {
            texts::tui_codex_provider_switch_shared_config_tip_message()
        }
        _ => texts::tui_provider_switch_shared_config_tip_message(),
    };
    Ok(Some(Overlay::Confirm(ConfirmOverlay {
        title: texts::tui_provider_switch_shared_config_tip_title().to_string(),
        message,
        action: ConfirmAction::ProviderSwitchSharedConfigNotice,
    })))
}

fn next_imported_live_provider_id(rows: &[crate::cli::tui::data::ProviderRow]) -> String {
    const BASE_ID: &str = "imported-current";

    if rows.iter().all(|row| row.id != BASE_ID) {
        return BASE_ID.to_string();
    }

    let mut suffix = 2usize;
    loop {
        let candidate = format!("{BASE_ID}-{suffix}");
        if rows.iter().all(|row| row.id != candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

fn current_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn display_path_with_tilde(path: &Path) -> String {
    let display = path.display().to_string();
    let Some(home) = dirs::home_dir() else {
        return display;
    };
    let home = home.display().to_string();
    if display == home {
        "~".to_string()
    } else if let Some(suffix) = display.strip_prefix(&(home + "/")) {
        format!("~/{suffix}")
    } else {
        display
    }
}

fn provider_requires_local_proxy(
    app_type: &crate::app_config::AppType,
    provider: &crate::provider::Provider,
) -> Option<&'static str> {
    if !matches!(app_type, crate::app_config::AppType::Claude) {
        return None;
    }

    let api_format = get_claude_api_format(provider);
    ClaudeApiFormat::from_raw(api_format)
        .requires_proxy()
        .then_some(api_format)
}

fn provider_switch_proxy_notice_api_format(
    app_type: &crate::app_config::AppType,
    provider: &crate::provider::Provider,
    proxy_ready: bool,
) -> Option<&'static str> {
    provider_requires_local_proxy(app_type, provider).filter(|_| !proxy_ready)
}

pub(super) fn delete(ctx: &mut RuntimeActionContext<'_>, id: String) -> Result<(), AppError> {
    let state = load_state()?;
    ProviderService::delete(&state, ctx.app.app_type.clone(), &id)?;
    ctx.app
        .push_toast(texts::tui_toast_provider_deleted(), ToastKind::Success);
    *ctx.data = UiData::load(&ctx.app.app_type)?;
    Ok(())
}

pub(super) fn speedtest(ctx: &mut RuntimeActionContext<'_>, url: String) -> Result<(), AppError> {
    let Some(tx) = ctx.speedtest_req_tx else {
        if matches!(&ctx.app.overlay, Overlay::SpeedtestRunning { url: running_url } if running_url == &url)
        {
            ctx.app.overlay = Overlay::None;
        }
        ctx.app
            .push_toast(texts::tui_toast_speedtest_disabled(), ToastKind::Warning);
        return Ok(());
    };

    if let Err(err) = tx.send(url.clone()) {
        if matches!(&ctx.app.overlay, Overlay::SpeedtestRunning { url: running_url } if running_url == &url)
        {
            ctx.app.overlay = Overlay::None;
        }
        ctx.app.push_toast(
            texts::tui_toast_speedtest_request_failed(&err.to_string()),
            ToastKind::Error,
        );
    }
    Ok(())
}

pub(super) fn stream_check(ctx: &mut RuntimeActionContext<'_>, id: String) -> Result<(), AppError> {
    let Some(tx) = ctx.stream_check_req_tx else {
        if matches!(&ctx.app.overlay, Overlay::StreamCheckRunning { provider_id, .. } if provider_id == &id)
        {
            ctx.app.overlay = Overlay::None;
        }
        ctx.app
            .push_toast(texts::tui_toast_stream_check_disabled(), ToastKind::Warning);
        return Ok(());
    };

    let Some(row) = ctx.data.providers.rows.iter().find(|row| row.id == id) else {
        return Ok(());
    };
    let req = StreamCheckReq {
        app_type: ctx.app.app_type.clone(),
        provider_id: row.id.clone(),
        provider_name: row.provider.name.clone(),
        provider: row.provider.clone(),
    };

    if let Err(err) = tx.send(req) {
        if matches!(&ctx.app.overlay, Overlay::StreamCheckRunning { provider_id, .. } if provider_id == &id)
        {
            ctx.app.overlay = Overlay::None;
        }
        ctx.app.push_toast(
            texts::tui_toast_stream_check_request_failed(&err.to_string()),
            ToastKind::Error,
        );
    }
    Ok(())
}

pub(super) fn model_fetch(
    ctx: &mut RuntimeActionContext<'_>,
    base_url: String,
    api_key: Option<String>,
    field: ProviderAddField,
    claude_idx: Option<usize>,
) -> Result<(), AppError> {
    let Some(tx) = ctx.model_fetch_req_tx else {
        ctx.app.push_toast(
            texts::tui_toast_model_fetch_worker_disabled(),
            ToastKind::Warning,
        );
        return Ok(());
    };
    let request_id = next_model_fetch_request_id();

    ctx.app.overlay = Overlay::ModelFetchPicker {
        request_id,
        field: field.clone(),
        claude_idx,
        input: String::new(),
        query: String::new(),
        fetching: true,
        models: Vec::new(),
        error: None,
        selected_idx: 0,
    };

    if let Err(err) = tx.send(ModelFetchReq::Fetch {
        request_id,
        base_url,
        api_key,
        field,
        claude_idx,
    }) {
        if let Overlay::ModelFetchPicker {
            fetching, error, ..
        } = &mut ctx.app.overlay
        {
            *fetching = false;
            *error = Some(texts::tui_model_fetch_error_hint(&err.to_string()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use serde_json::json;
    use serial_test::serial;
    use tempfile::TempDir;

    use super::*;
    use crate::cli::tui::app::App;
    use crate::cli::tui::app::{ConfirmAction, ConfirmOverlay};
    use crate::cli::tui::runtime_systems::RequestTracker;
    use crate::cli::tui::terminal::TuiTerminal;
    use crate::provider::Provider;
    use crate::{AppType, MultiAppConfig};

    struct EnvGuard {
        old_home: Option<OsString>,
        old_userprofile: Option<OsString>,
    }

    impl EnvGuard {
        fn set_home(home: &Path) -> Self {
            let old_home = std::env::var_os("HOME");
            let old_userprofile = std::env::var_os("USERPROFILE");
            std::env::set_var("HOME", home);
            std::env::set_var("USERPROFILE", home);
            Self {
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
        }
    }

    fn codex_test_config(current_id: &str) -> MultiAppConfig {
        let mut config = MultiAppConfig::default();
        let manager = config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = current_id.to_string();
        manager.providers.insert(
            "old-provider".to_string(),
            Provider::with_id(
                "old-provider".to_string(),
                "Legacy".to_string(),
                json!({
                    "auth": {"OPENAI_API_KEY": "stale"},
                    "config": "stale-config"
                }),
                None,
            ),
        );
        manager.providers.insert(
            "new-provider".to_string(),
            Provider::with_id(
                "new-provider".to_string(),
                "Latest".to_string(),
                json!({
                    "auth": {"OPENAI_API_KEY": "fresh-key"},
                    "config": "model_provider = \"latest\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.latest]\nbase_url = \"https://api.example.com/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
                }),
                None,
            ),
        );
        config
    }

    fn seed_codex_live_files(
        config_text: Option<&str>,
        auth: Option<serde_json::Value>,
    ) -> Result<(), AppError> {
        let config_path = crate::codex_config::get_codex_config_path();
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| AppError::io(parent, err))?;
        }
        if let Some(text) = config_text {
            std::fs::write(&config_path, text).map_err(|err| AppError::io(&config_path, err))?;
        }

        if let Some(auth) = auth {
            let auth_path = crate::codex_config::get_codex_auth_path();
            if let Some(parent) = auth_path.parent() {
                std::fs::create_dir_all(parent).map_err(|err| AppError::io(parent, err))?;
            }
            std::fs::write(
                &auth_path,
                serde_json::to_string_pretty(&auth).expect("serialize codex auth"),
            )
            .map_err(|err| AppError::io(&auth_path, err))?;
        }

        Ok(())
    }

    fn claude_test_config(current_id: &str, api_format: &str) -> MultiAppConfig {
        let mut config = MultiAppConfig::default();
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = current_id.to_string();
        manager.providers.insert(
            "old-provider".to_string(),
            Provider::with_id(
                "old-provider".to_string(),
                "Legacy Claude".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_BASE_URL": "https://api.anthropic.com",
                        "ANTHROPIC_API_KEY": "sk-old"
                    },
                    "api_format": "anthropic"
                }),
                None,
            ),
        );
        manager.providers.insert(
            "proxy-provider".to_string(),
            Provider::with_id(
                "proxy-provider".to_string(),
                "Proxy Claude".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_BASE_URL": "https://example.com",
                        "ANTHROPIC_API_KEY": "sk-new"
                    },
                    "api_format": api_format
                }),
                None,
            ),
        );
        config
    }

    fn claude_provider_with_api_format(api_format: &str) -> Provider {
        Provider::with_id(
            "proxy-provider".to_string(),
            "Proxy Claude".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://example.com",
                    "ANTHROPIC_API_KEY": "sk-new"
                },
                "api_format": api_format
            }),
            None,
        )
    }

    fn run_codex_switch(
        current_id: &str,
        config_text: Option<&str>,
        auth: Option<serde_json::Value>,
        shared_tip_shown: bool,
    ) -> Result<(App, UiData), AppError> {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let mut settings = crate::settings::get_settings();
        settings.provider_switch_common_config_tip_shown_codex = shared_tip_shown;
        crate::settings::update_settings(settings)?;

        seed_codex_live_files(config_text, auth)?;

        codex_test_config(current_id).save()?;

        let mut terminal = TuiTerminal::new_for_test()?;
        let mut app = App::new(Some(AppType::Codex));
        let mut data = UiData::load(&AppType::Codex)?;
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

        switch(&mut ctx, "new-provider".to_string())?;

        Ok((app, data))
    }

    fn seed_claude_live_settings(value: serde_json::Value) -> Result<(), AppError> {
        let settings_path = crate::get_claude_settings_path();
        if let Some(parent) = settings_path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| AppError::io(parent, err))?;
        }
        std::fs::write(
            &settings_path,
            serde_json::to_string_pretty(&value).expect("serialize claude live settings"),
        )
        .map_err(|err| AppError::io(&settings_path, err))?;
        Ok(())
    }

    fn run_claude_switch(
        current_id: &str,
        api_format: &str,
        seed_live: bool,
        shared_tip_shown: bool,
    ) -> Result<(App, UiData), AppError> {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let mut settings = crate::settings::get_settings();
        settings.provider_switch_common_config_tip_shown = shared_tip_shown;
        crate::settings::update_settings(settings)?;

        if seed_live {
            seed_claude_live_settings(json!({
                "env": {
                    "ANTHROPIC_API_KEY": "live-key"
                },
                "permissions": {
                    "allow": ["Bash"]
                }
            }))?;
        }

        claude_test_config(current_id, api_format).save()?;

        let mut terminal = TuiTerminal::new_for_test()?;
        let mut app = App::new(Some(AppType::Claude));
        let mut data = UiData::load(&AppType::Claude)?;
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

        switch(&mut ctx, "proxy-provider".to_string())?;

        Ok((app, data))
    }

    #[test]
    #[serial]
    fn provider_switch_does_not_show_restart_toast_when_live_sync_succeeds() {
        let (app, data) = run_codex_switch(
            "old-provider",
            Some("model_provider = \"legacy\"\nmodel = \"gpt-4\"\n"),
            Some(json!({"OPENAI_API_KEY": "legacy-key"})),
            true,
        )
        .expect("switch should succeed");

        assert_eq!(data.providers.current_id, "new-provider");
        assert!(
            app.toast.is_none(),
            "provider switch should not show restart toast"
        );
    }

    #[test]
    #[serial]
    fn provider_switch_does_not_show_restart_toast_when_live_sync_is_skipped() {
        let (app, data) =
            run_codex_switch("old-provider", None, None, true).expect("switch should succeed");

        assert_eq!(data.providers.current_id, "new-provider");
        assert!(
            app.toast.is_none(),
            "provider switch should not show restart toast"
        );
    }

    #[test]
    #[serial]
    fn provider_switch_shows_first_use_guard_before_overwriting_existing_codex_settings() {
        let (app, data) = run_codex_switch(
            "",
            Some("model_provider = \"legacy\"\nmodel = \"gpt-4\"\n"),
            None,
            false,
        )
        .expect("guarded switch should succeed");

        assert_eq!(data.providers.current_id, "");
        assert!(matches!(
            app.overlay,
            Overlay::ProviderSwitchFirstUseConfirm {
                provider_id,
                title,
                message,
                selected,
            } if provider_id == "new-provider"
                && title == texts::tui_codex_provider_switch_first_use_title()
                && message == texts::tui_codex_provider_switch_first_use_message("~/.codex/config.toml")
                && selected == 0
        ));
    }

    #[test]
    #[serial]
    fn provider_switch_first_use_without_existing_codex_settings_switches_normally() {
        let (app, data) = run_codex_switch("", None, None, false).expect("switch should succeed");

        assert_eq!(data.providers.current_id, "new-provider");
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    #[serial]
    fn provider_switch_codex_auth_only_state_does_not_trigger_first_use_guard() {
        let (app, data) = run_codex_switch(
            "",
            None,
            Some(json!({"OPENAI_API_KEY": "legacy-key"})),
            false,
        )
        .expect("switch should succeed");

        assert_eq!(data.providers.current_id, "new-provider");
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    #[serial]
    fn provider_switch_existing_codex_install_with_current_provider_skips_first_use_guard() {
        let (app, data) = run_codex_switch(
            "old-provider",
            Some("model_provider = \"legacy\"\nmodel = \"gpt-4\"\n"),
            None,
            true,
        )
        .expect("switch should succeed");

        assert_eq!(data.providers.current_id, "new-provider");
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    #[serial]
    fn provider_import_codex_live_config_succeeds_without_auth_json() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        seed_codex_live_files(
            Some("model_provider = \"legacy\"\nmodel = \"gpt-4\"\n"),
            None,
        )
        .expect("seed codex live files");
        codex_test_config("").save().expect("save codex config");

        let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
        let mut app = App::new(Some(AppType::Codex));
        let mut data = UiData::load(&AppType::Codex).expect("load data");
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

        import_live_config(&mut ctx).expect("import live config should succeed");

        assert_eq!(data.providers.current_id, "imported-current");
        assert!(data.providers.rows.iter().any(|row| {
            row.id == "imported-current"
                && row
                    .provider
                    .settings_config
                    .get("config")
                    .and_then(|value| value.as_str())
                    .map(|value| value.contains("model_provider = \"legacy\""))
                    .unwrap_or(false)
                && row.provider.settings_config.get("auth").is_none()
        }));
    }

    #[test]
    #[serial]
    fn codex_provider_switch_shows_one_time_shared_config_tip_after_first_real_switch() {
        let (app, data) = run_codex_switch(
            "old-provider",
            Some("model_provider = \"legacy\"\nmodel = \"gpt-4\"\n"),
            Some(json!({"OPENAI_API_KEY": "legacy-key"})),
            false,
        )
        .expect("switch should succeed");

        assert_eq!(data.providers.current_id, "new-provider");
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                title,
                message,
                action: ConfirmAction::ProviderSwitchSharedConfigNotice,
            }) if title == texts::tui_provider_switch_shared_config_tip_title()
                && message == texts::tui_codex_provider_switch_shared_config_tip_message()
        ));
        assert!(crate::settings::get_provider_switch_common_config_tip_shown_codex());
    }

    #[test]
    #[serial]
    fn provider_switch_warns_when_claude_provider_requires_proxy_and_proxy_is_not_running() {
        let (overlay, current_id) = run_claude_switch("old-provider", "openai_chat", false, false)
            .expect("switch should succeed");

        assert_eq!(current_id.providers.current_id, "proxy-provider");
        assert!(matches!(
            overlay.overlay,
            Overlay::Confirm(ConfirmOverlay { title, message, action })
                if title == texts::tui_claude_api_format_requires_proxy_title()
                    && message == texts::tui_claude_api_format_requires_proxy_message("openai_chat")
                    && matches!(action, ConfirmAction::ProviderApiFormatProxyNotice)
        ));
    }

    #[test]
    #[serial]
    fn provider_switch_warns_for_openai_responses_when_proxy_is_not_running() {
        let (overlay, current_id) =
            run_claude_switch("old-provider", "openai_responses", false, false)
                .expect("switch should succeed");

        assert_eq!(current_id.providers.current_id, "proxy-provider");
        assert!(matches!(
            overlay.overlay,
            Overlay::Confirm(ConfirmOverlay { title, message, action })
                if title == texts::tui_claude_api_format_requires_proxy_title()
                    && message == texts::tui_claude_api_format_requires_proxy_message("openai_responses")
                    && matches!(action, ConfirmAction::ProviderApiFormatProxyNotice)
        ));
    }

    #[test]
    fn provider_switch_notice_is_suppressed_when_current_app_already_routes_through_proxy() {
        let provider = claude_provider_with_api_format("openai_chat");

        let notice = provider_switch_proxy_notice_api_format(&AppType::Claude, &provider, true);

        assert_eq!(notice, None);
    }

    #[test]
    fn provider_switch_notice_uses_openai_responses_api_format_when_proxy_is_not_ready() {
        let provider = claude_provider_with_api_format("openai_responses");

        let notice = provider_switch_proxy_notice_api_format(&AppType::Claude, &provider, false);

        assert_eq!(notice, Some("openai_responses"));
    }

    #[test]
    #[serial]
    fn provider_switch_does_not_warn_when_claude_provider_uses_anthropic_format() {
        let (overlay, current_id) = run_claude_switch("old-provider", "anthropic", false, true)
            .expect("switch should succeed");

        assert_eq!(current_id.providers.current_id, "proxy-provider");
        assert!(matches!(overlay.overlay, Overlay::None));
    }

    #[test]
    #[serial]
    fn provider_switch_shows_first_use_guard_before_overwriting_existing_claude_settings() {
        let (app, data) =
            run_claude_switch("", "anthropic", true, false).expect("guarded switch should succeed");

        assert_eq!(data.providers.current_id, "");
        assert!(matches!(
            app.overlay,
            Overlay::ProviderSwitchFirstUseConfirm {
                provider_id,
                title,
                message,
                selected,
            } if provider_id == "proxy-provider"
                && title == texts::tui_provider_switch_first_use_title()
                && message == texts::tui_provider_switch_first_use_message("~/.claude/settings.json")
                && selected == 0
        ));
    }

    #[test]
    #[serial]
    fn provider_switch_first_use_without_existing_claude_settings_switches_normally() {
        let (app, data) =
            run_claude_switch("", "anthropic", false, false).expect("switch should succeed");

        assert_eq!(data.providers.current_id, "proxy-provider");
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    #[serial]
    fn provider_switch_existing_install_with_current_provider_skips_first_use_guard() {
        let (app, data) = run_claude_switch("old-provider", "anthropic", true, true)
            .expect("switch should succeed");

        assert_eq!(data.providers.current_id, "proxy-provider");
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    #[serial]
    fn provider_import_live_config_adds_and_selects_imported_provider() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        seed_claude_live_settings(json!({
            "env": {
                "ANTHROPIC_API_KEY": "live-key"
            },
            "permissions": {
                "allow": ["Bash"]
            }
        }))
        .expect("seed live settings");
        claude_test_config("", "anthropic")
            .save()
            .expect("save config");

        let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
        let mut app = App::new(Some(AppType::Claude));
        let mut data = UiData::load(&AppType::Claude).expect("load data");
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

        import_live_config(&mut ctx).expect("import live config should succeed");

        assert_eq!(data.providers.current_id, "imported-current");
        assert!(data.providers.rows.iter().any(|row| {
            row.id == "imported-current"
                && row
                    .provider
                    .settings_config
                    .get("permissions")
                    .and_then(|value| value.get("allow"))
                    .is_some()
        }));
        assert_eq!(
            app.toast.as_ref().map(|toast| toast.message.as_str()),
            Some(texts::tui_toast_provider_live_config_imported())
        );
    }

    #[test]
    #[serial]
    fn provider_switch_shows_one_time_shared_config_tip_after_first_real_switch() {
        let (app, data) = run_claude_switch("old-provider", "anthropic", true, false)
            .expect("switch should succeed");

        assert_eq!(data.providers.current_id, "proxy-provider");
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                title,
                message,
                action: ConfirmAction::ProviderSwitchSharedConfigNotice,
            }) if title == texts::tui_provider_switch_shared_config_tip_title()
                && message == texts::tui_provider_switch_shared_config_tip_message()
        ));
        assert!(crate::settings::get_provider_switch_common_config_tip_shown());
    }

    #[test]
    #[serial]
    fn provider_switch_queues_shared_config_tip_behind_proxy_notice() {
        let (app, data) = run_claude_switch("old-provider", "openai_chat", true, false)
            .expect("switch should succeed");

        assert_eq!(data.providers.current_id, "proxy-provider");
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::ProviderApiFormatProxyNotice,
                ..
            })
        ));
        assert!(matches!(
            app.pending_overlay,
            Some(Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::ProviderSwitchSharedConfigNotice,
                ..
            }))
        ));
    }
}
