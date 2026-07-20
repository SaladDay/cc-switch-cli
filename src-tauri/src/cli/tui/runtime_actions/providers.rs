use crate::cli::i18n::texts;
use crate::cli::tui::form::ClaudeApiFormat;
use crate::error::AppError;
#[cfg(test)]
use crate::openclaw_config::OpenClawDefaultModel;
use crate::proxy::providers::get_claude_api_format;
use crate::services::provider::ProviderSortUpdate;
use crate::services::ProviderService;

use super::super::app::{ConfirmAction, ConfirmOverlay, Overlay, ToastKind};
use super::super::data::load_state;
#[cfg(test)]
use super::super::data::UiData;
use super::super::form::ProviderAddField;
use super::super::runtime_systems::{next_model_fetch_request_id, ModelFetchReq, StreamCheckReq};
use super::super::text_edit::TextInput;
use super::RuntimeActionContext;

fn active_proxy_failover_queue_guard_message() -> &'static str {
    crate::t!(
        "At least one provider must remain in the failover queue while proxy failover is active.",
        "代理故障转移激活时，故障转移队列中必须至少保留一个供应商。"
    )
}

fn provider_is_last_active_failover_queue_entry(
    ctx: &RuntimeActionContext<'_>,
    provider_id: &str,
) -> Result<bool, AppError> {
    if !crate::cli::tui::app::supports_failover_controls(&ctx.app.app_type) {
        return Ok(false);
    }

    let state = load_state()?;
    let app_key = ctx.app.app_type.as_str();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("failed to create async runtime: {e}")))?;
    let auto_failover_enabled = runtime
        .block_on(async { state.db.get_proxy_config_for_app(app_key).await })?
        .auto_failover_enabled;
    if !auto_failover_enabled {
        return Ok(false);
    }

    let queue = state.db.get_failover_queue(app_key)?;
    Ok(queue.len() == 1
        && queue
            .first()
            .is_some_and(|item| item.provider_id == provider_id))
}

fn guard_last_active_failover_queue_entry(
    ctx: &mut RuntimeActionContext<'_>,
    provider_id: &str,
) -> Result<bool, AppError> {
    if provider_is_last_active_failover_queue_entry(ctx, provider_id)? {
        ctx.app.push_toast(
            active_proxy_failover_queue_guard_message(),
            ToastKind::Warning,
        );
        return Ok(true);
    }

    Ok(false)
}

fn refresh_provider_data_after_write(
    ctx: &mut RuntimeActionContext<'_>,
    state: &crate::store::AppState,
) -> Result<(), AppError> {
    refresh_provider_data_after_write_with_config(ctx, state, false)
}

fn refresh_provider_and_config_data_after_write(
    ctx: &mut RuntimeActionContext<'_>,
    state: &crate::store::AppState,
) -> Result<(), AppError> {
    refresh_provider_data_after_write_with_config(ctx, state, true)
}

fn refresh_provider_data_after_write_with_config(
    ctx: &mut RuntimeActionContext<'_>,
    state: &crate::store::AppState,
    refresh_config: bool,
) -> Result<(), AppError> {
    let app_type = ctx.app.app_type.clone();
    state.reload_config_snapshot_from_db()?;
    ctx.data
        .refresh_current_app_provider_data(state, &app_type)?;
    if refresh_config {
        ctx.data.refresh_current_app_config_data(state, &app_type)?;
    }
    ctx.app.clamp_selections(ctx.data);
    ctx.data.mark_current_app_data_changed();
    Ok(())
}

pub(super) fn switch(ctx: &mut RuntimeActionContext<'_>, id: String) -> Result<(), AppError> {
    // Upstream parity: provider switch is a clean write; no live-conflict
    // preview/overlay is surfaced.
    let state = load_state()?;
    do_switch(ctx, state, id)
}

pub(super) fn import_live_config(ctx: &mut RuntimeActionContext<'_>) -> Result<(), AppError> {
    let state = load_state()?;
    let imported = ProviderService::import_live_config(&state, ctx.app.app_type.clone())? > 0;

    refresh_provider_data_after_write(ctx, &state)?;
    ctx.app.pending_overlay = None;
    if imported {
        let toast_message = match ctx.app.app_type {
            crate::app_config::AppType::Codex => texts::tui_toast_codex_live_config_imported(),
            _ => texts::tui_toast_provider_live_config_imported(),
        };
        ctx.app.push_toast(toast_message, ToastKind::Success);
    } else {
        ctx.app
            .push_toast(texts::tui_toast_no_live_config_imported(), ToastKind::Info);
    }
    Ok(())
}

fn do_switch(
    ctx: &mut RuntimeActionContext<'_>,
    state: crate::store::AppState,
    id: String,
) -> Result<(), AppError> {
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
    refresh_provider_data_after_write(ctx, &state)?;
    ctx.app.pending_overlay = None;

    let proxy_ready = ctx
        .data
        .proxy
        .routes_current_app_through_proxy(&ctx.app.app_type)
        .unwrap_or(false);
    let proxy_overlay = switched_provider.as_ref().and_then(|provider| {
        provider_switch_proxy_notice_overlay(&ctx.app.app_type, provider, proxy_ready)
    });
    ctx.app.overlay = proxy_overlay.unwrap_or(Overlay::None);

    if ctx.app.app_type.is_additive_mode() {
        ctx.app.push_toast(
            texts::tui_toast_provider_added_to_app_config(ctx.app.app_type.as_str()),
            ToastKind::Success,
        );
    }

    Ok(())
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

pub(super) fn set_failover_queue(
    ctx: &mut RuntimeActionContext<'_>,
    id: String,
    enabled: bool,
) -> Result<(), AppError> {
    if !crate::cli::tui::app::supports_failover_controls(&ctx.app.app_type) {
        return Ok(());
    }
    if ctx.data.providers.rows.iter().all(|row| row.id != id) {
        return Err(AppError::InvalidInput(format!("Provider not found: {id}")));
    }

    let state = load_state()?;
    if enabled {
        state
            .db
            .add_to_failover_queue(ctx.app.app_type.as_str(), &id)?;
    } else {
        if guard_last_active_failover_queue_entry(ctx, &id)? {
            return Ok(());
        }
        state
            .db
            .remove_from_failover_queue(ctx.app.app_type.as_str(), &id)?;
    }

    refresh_provider_data_after_write(ctx, &state)?;
    ctx.app.push_toast(
        if enabled {
            crate::t!(
                "Provider added to the failover queue.",
                "供应商已加入故障转移队列。"
            )
        } else {
            crate::t!(
                "Provider removed from the failover queue.",
                "供应商已移出故障转移队列。"
            )
        },
        ToastKind::Success,
    );
    Ok(())
}

pub(super) fn move_failover_queue(
    ctx: &mut RuntimeActionContext<'_>,
    id: String,
    direction: crate::cli::tui::app::MoveDirection,
) -> Result<(), AppError> {
    if !crate::cli::tui::app::supports_failover_controls(&ctx.app.app_type) {
        return Ok(());
    }

    let mut queued = ctx
        .data
        .providers
        .rows
        .iter()
        .filter(|row| row.provider.in_failover_queue)
        .cloned()
        .collect::<Vec<_>>();
    queued.sort_by(
        |a, b| match (a.provider.sort_index, b.provider.sort_index) {
            (Some(a_idx), Some(b_idx)) => a_idx.cmp(&b_idx).then_with(|| a.id.cmp(&b.id)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.id.cmp(&b.id),
        },
    );

    let Some(index) = queued.iter().position(|row| row.id == id) else {
        ctx.app.push_toast(
            crate::t!(
                "Add this provider to the failover queue before moving it.",
                "请先将该供应商加入故障转移队列再调整顺序。"
            ),
            ToastKind::Info,
        );
        return Ok(());
    };

    let target = match direction {
        crate::cli::tui::app::MoveDirection::Up if index > 0 => index - 1,
        crate::cli::tui::app::MoveDirection::Down if index + 1 < queued.len() => index + 1,
        _ => {
            ctx.app.push_toast(
                crate::t!(
                    "Provider is already at the edge of the failover queue.",
                    "该供应商已在故障转移队列边界。"
                ),
                ToastKind::Info,
            );
            return Ok(());
        }
    };

    queued.swap(index, target);
    let updates = queued
        .iter()
        .enumerate()
        .map(|(sort_index, row)| ProviderSortUpdate {
            id: row.id.clone(),
            sort_index,
        })
        .collect::<Vec<_>>();

    let state = load_state()?;
    ProviderService::update_sort_order(&state, ctx.app.app_type.clone(), updates)?;
    refresh_provider_data_after_write(ctx, &state)?;
    ctx.app.push_toast(
        crate::t!("Failover queue order updated.", "故障转移队列顺序已更新。"),
        ToastKind::Success,
    );
    Ok(())
}

pub(super) fn delete(ctx: &mut RuntimeActionContext<'_>, id: String) -> Result<(), AppError> {
    if guard_last_active_failover_queue_entry(ctx, &id)? {
        return Ok(());
    }

    let state = load_state()?;
    ProviderService::delete(&state, ctx.app.app_type.clone(), &id)?;
    ctx.app
        .push_toast(texts::tui_toast_provider_deleted(), ToastKind::Success);
    refresh_provider_data_after_write(ctx, &state)?;
    Ok(())
}

pub(super) fn remove_from_config(
    ctx: &mut RuntimeActionContext<'_>,
    id: String,
) -> Result<(), AppError> {
    match ctx.app.app_type {
        crate::app_config::AppType::OpenClaw => {
            let state = load_state()?;
            ProviderService::remove_from_live_config(&state, ctx.app.app_type.clone(), &id)?;
            ctx.app.push_toast(
                texts::tui_toast_provider_removed_from_config(),
                ToastKind::Success,
            );
            refresh_provider_data_after_write(ctx, &state)?;
            Ok(())
        }
        crate::app_config::AppType::OpenCode | crate::app_config::AppType::Hermes => {
            let state = load_state()?;
            ProviderService::remove_from_live_config(&state, ctx.app.app_type.clone(), &id)?;
            ctx.app.push_toast(
                texts::tui_toast_provider_removed_from_app_config(ctx.app.app_type.as_str()),
                ToastKind::Success,
            );
            refresh_provider_data_after_write(ctx, &state)?;
            Ok(())
        }
        _ => delete(ctx, id),
    }
}

pub(super) fn set_default_model(
    ctx: &mut RuntimeActionContext<'_>,
    provider_id: String,
    _model_id: String,
) -> Result<(), AppError> {
    let state = load_state()?;
    let default =
        ProviderService::set_default_model(&state, ctx.app.app_type.clone(), &provider_id, None)?;
    let message = if matches!(ctx.app.app_type, crate::app_config::AppType::Hermes) {
        texts::tui_toast_provider_enabled(&provider_id)
    } else {
        texts::tui_toast_provider_set_as_default(&default)
    };
    ctx.app.push_toast(message, ToastKind::Success);
    refresh_provider_and_config_data_after_write(ctx, &state)?;
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
        provider_name: crate::cli::tui::data::provider_display_name(&ctx.app.app_type, row),
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

#[expect(
    clippy::too_many_arguments,
    reason = "model fetch carries the selected provider draft and result destination"
)]
pub(super) fn model_fetch(
    ctx: &mut RuntimeActionContext<'_>,
    base_url: String,
    api_key: Option<String>,
    custom_user_agent: Option<String>,
    codex_oauth: bool,
    codex_oauth_account_id: Option<String>,
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
        field,
        claude_idx,
        input: TextInput::new(""),
        query: String::new(),
        fetching: true,
        models: Vec::new(),
        filtered_indices: None,
        filter_incomplete: false,
        error: None,
        selected_idx: 0,
        selection_active: false,
    };
    if matches!(field, ProviderAddField::HermesModels) {
        ctx.app.pending_overlay = Some(Overlay::HermesModelsPicker { editing: false });
    }

    if let Err(err) = tx.send(ModelFetchReq::Fetch {
        request_id,
        base_url,
        api_key,
        custom_user_agent,
        codex_oauth,
        codex_oauth_account_id,
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
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::path::Path;

    use serde_json::json;
    use serial_test::serial;
    use tempfile::TempDir;

    use super::*;
    use crate::cli::tui::app::{App, MoveDirection};
    use crate::cli::tui::app::{ConfirmAction, ConfirmOverlay};
    use crate::cli::tui::runtime_systems::RequestTracker;
    use crate::cli::tui::terminal::TuiTerminal;
    use crate::provider::Provider;
    use crate::settings::{get_settings, update_settings, AppSettings};
    use crate::test_support::{
        lock_test_home_and_settings, set_test_home_override, TestHomeSettingsLock,
    };
    use crate::{AppState, AppType, MultiAppConfig};

    struct EnvGuard {
        _lock: TestHomeSettingsLock,
        old_home: Option<OsString>,
        old_userprofile: Option<OsString>,
        old_config_dir: Option<OsString>,
        old_claude_config_dir: Option<OsString>,
        old_codex_home: Option<OsString>,
    }

    impl EnvGuard {
        fn set_home(home: &Path) -> Self {
            let lock = lock_test_home_and_settings();
            let old_home = std::env::var_os("HOME");
            let old_userprofile = std::env::var_os("USERPROFILE");
            let old_config_dir = std::env::var_os("CC_SWITCH_CONFIG_DIR");
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
                old_config_dir,
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
            match &self.old_config_dir {
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

    struct SettingsGuard {
        previous: AppSettings,
    }

    impl SettingsGuard {
        fn with_opencode_dir(path: &Path) -> Self {
            let previous = get_settings();
            let settings = AppSettings {
                opencode_config_dir: Some(path.display().to_string()),
                ..Default::default()
            };
            update_settings(settings).expect("set opencode override dir");
            Self { previous }
        }

        fn with_openclaw_dir(path: &Path) -> Self {
            let previous = get_settings();
            let settings = AppSettings {
                openclaw_config_dir: Some(path.display().to_string()),
                ..Default::default()
            };
            update_settings(settings).expect("set openclaw override dir");
            Self { previous }
        }

        fn with_hermes_dir(path: &Path) -> Self {
            let previous = get_settings();
            let settings = AppSettings {
                hermes_config_dir: Some(path.display().to_string()),
                ..Default::default()
            };
            update_settings(settings).expect("set hermes override dir");
            Self { previous }
        }
    }

    impl Drop for SettingsGuard {
        fn drop(&mut self) {
            update_settings(self.previous.clone()).expect("restore previous settings");
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

    struct SwitchFixture {
        _temp_home: TempDir,
        _env: EnvGuard,
        app: App,
        data: UiData,
    }

    const MATCHING_CODEX_LIVE_CONFIG: &str = "model_provider = \"latest\"\nmodel = \"gpt-5.2-codex\"\n\n[model_providers.latest]\nbase_url = \"https://api.example.com/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n\n[projects.local]\ntrust_level = \"trusted\"\n";

    fn run_codex_switch(
        current_id: &str,
        config_text: Option<&str>,
        auth: Option<serde_json::Value>,
    ) -> Result<SwitchFixture, AppError> {
        let temp_home = TempDir::new().expect("create temp home");
        let env = EnvGuard::set_home(temp_home.path());

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
            session_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut webdav_loading,
            update_req_tx: None,
            update_check: &mut update_check,
            model_fetch_req_tx: None,
            managed_auth_req_tx: None,
        };

        switch(&mut ctx, "new-provider".to_string())?;

        Ok(SwitchFixture {
            _temp_home: temp_home,
            _env: env,
            app,
            data,
        })
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
    ) -> Result<SwitchFixture, AppError> {
        let temp_home = TempDir::new().expect("create temp home");
        let env = EnvGuard::set_home(temp_home.path());

        if seed_live {
            seed_claude_live_settings(json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://example.com",
                    "ANTHROPIC_API_KEY": "sk-new",
                    "LOCAL_ONLY": "preserve-me"
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
            session_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut webdav_loading,
            update_req_tx: None,
            update_check: &mut update_check,
            model_fetch_req_tx: None,
            managed_auth_req_tx: None,
        };

        switch(&mut ctx, "proxy-provider".to_string())?;

        Ok(SwitchFixture {
            _temp_home: temp_home,
            _env: env,
            app,
            data,
        })
    }

    fn claude_queue_provider(id: &str) -> Provider {
        Provider::with_id(
            id.to_string(),
            format!("Provider {id}"),
            json!({"env":{"ANTHROPIC_BASE_URL":format!("https://{id}.example.com")}}),
            None,
        )
    }

    fn add_claude_queue_provider(state: &AppState, id: &str) -> Result<(), AppError> {
        seed_claude_live_settings(json!({
            "env": {
                "ANTHROPIC_BASE_URL": format!("https://{id}.example.com"),
                "LOCAL_ONLY": "preserve-me"
            }
        }))?;
        ProviderService::add(state, AppType::Claude, claude_queue_provider(id)).map(|_| ())
    }

    fn reload_fixture_data(fixture: &mut RuntimeActionFixture) {
        fixture.data = UiData::load(&fixture.app.app_type).expect("reload ui data");
    }

    struct RuntimeActionFixture {
        terminal: TuiTerminal,
        app: App,
        data: UiData,
        proxy_loading: RequestTracker,
        webdav_loading: RequestTracker,
        update_check: RequestTracker,
    }

    impl RuntimeActionFixture {
        fn new(app_type: AppType) -> Self {
            Self {
                terminal: TuiTerminal::new_for_test().expect("create terminal"),
                app: App::new(Some(app_type.clone())),
                data: UiData::load(&app_type).expect("load ui data"),
                proxy_loading: RequestTracker::default(),
                webdav_loading: RequestTracker::default(),
                update_check: RequestTracker::default(),
            }
        }

        fn ctx(&mut self) -> RuntimeActionContext<'_> {
            RuntimeActionContext {
                terminal: &mut self.terminal,
                app: &mut self.app,
                data: &mut self.data,
                speedtest_req_tx: None,
                stream_check_req_tx: None,
                skills_req_tx: None,
                proxy_req_tx: None,
                proxy_loading: &mut self.proxy_loading,
                local_env_req_tx: None,
                session_req_tx: None,
                webdav_req_tx: None,
                webdav_loading: &mut self.webdav_loading,
                update_req_tx: None,
                update_check: &mut self.update_check,
                model_fetch_req_tx: None,
                managed_auth_req_tx: None,
            }
        }
    }

    #[test]
    #[serial(home_settings)]
    fn active_proxy_failover_rejects_removing_last_queued_provider() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let state = load_state().expect("load state");
        add_claude_queue_provider(&state, "p1").expect("add provider");
        state
            .db
            .add_to_failover_queue("claude", "p1")
            .expect("queue provider");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create runtime");
        runtime.block_on(async {
            let mut config = state.db.get_proxy_config_for_app("claude").await.unwrap();
            config.enabled = true;
            config.auto_failover_enabled = true;
            state.db.update_proxy_config_for_app(config).await.unwrap();
        });

        let mut fixture = RuntimeActionFixture::new(AppType::Claude);
        reload_fixture_data(&mut fixture);
        fixture.data.proxy.running = true;
        fixture.data.proxy.claude_takeover = true;
        fixture.data.proxy.auto_failover_enabled = true;
        set_failover_queue(&mut fixture.ctx(), "p1".to_string(), false)
            .expect("attempt queue removal");

        assert!(state
            .db
            .is_in_failover_queue("claude", "p1")
            .expect("read queue membership"));
        assert!(matches!(fixture.app.toast, Some(toast) if toast.kind == ToastKind::Warning));
    }

    #[test]
    #[serial(home_settings)]
    fn persisted_failover_rejects_removing_last_queued_provider_even_when_proxy_stopped() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let state = load_state().expect("load state");
        add_claude_queue_provider(&state, "p1").expect("add provider");
        state
            .db
            .add_to_failover_queue("claude", "p1")
            .expect("queue provider");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create runtime");
        runtime.block_on(async {
            let mut config = state.db.get_proxy_config_for_app("claude").await.unwrap();
            config.enabled = true;
            config.auto_failover_enabled = true;
            state.db.update_proxy_config_for_app(config).await.unwrap();
        });

        let mut fixture = RuntimeActionFixture::new(AppType::Claude);
        reload_fixture_data(&mut fixture);
        fixture.data.proxy.running = false;
        fixture.data.proxy.claude_takeover = true;
        fixture.data.proxy.auto_failover_enabled = true;
        set_failover_queue(&mut fixture.ctx(), "p1".to_string(), false)
            .expect("attempt queue removal");

        assert!(state
            .db
            .is_in_failover_queue("claude", "p1")
            .expect("read queue membership"));
        assert!(matches!(fixture.app.toast, Some(toast) if toast.kind == ToastKind::Warning));
    }

    #[test]
    #[serial(home_settings)]
    fn active_proxy_failover_allows_removing_one_of_multiple_queued_providers() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let state = load_state().expect("load state");
        add_claude_queue_provider(&state, "p1").expect("add first provider");
        add_claude_queue_provider(&state, "p2").expect("add second provider");
        state
            .db
            .add_to_failover_queue("claude", "p1")
            .expect("queue first provider");
        state
            .db
            .add_to_failover_queue("claude", "p2")
            .expect("queue second provider");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create runtime");
        runtime.block_on(async {
            let mut config = state.db.get_proxy_config_for_app("claude").await.unwrap();
            config.enabled = true;
            config.auto_failover_enabled = true;
            state.db.update_proxy_config_for_app(config).await.unwrap();
        });

        let mut fixture = RuntimeActionFixture::new(AppType::Claude);
        reload_fixture_data(&mut fixture);
        fixture.data.proxy.running = true;
        fixture.data.proxy.claude_takeover = true;
        fixture.data.proxy.auto_failover_enabled = true;
        set_failover_queue(&mut fixture.ctx(), "p1".to_string(), false)
            .expect("remove one queued provider");

        assert!(!state
            .db
            .is_in_failover_queue("claude", "p1")
            .expect("read first queue membership"));
        assert!(state
            .db
            .is_in_failover_queue("claude", "p2")
            .expect("read second queue membership"));
    }

    #[test]
    #[serial(home_settings)]
    fn active_proxy_failover_allows_reordering_queued_providers() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let state = load_state().expect("load state");
        add_claude_queue_provider(&state, "p1").expect("add first provider");
        add_claude_queue_provider(&state, "p2").expect("add second provider");
        state
            .db
            .add_to_failover_queue("claude", "p1")
            .expect("queue first provider");
        state
            .db
            .add_to_failover_queue("claude", "p2")
            .expect("queue second provider");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create runtime");
        runtime.block_on(async {
            let mut config = state.db.get_proxy_config_for_app("claude").await.unwrap();
            config.enabled = true;
            config.auto_failover_enabled = true;
            state.db.update_proxy_config_for_app(config).await.unwrap();
        });

        let mut fixture = RuntimeActionFixture::new(AppType::Claude);
        reload_fixture_data(&mut fixture);
        fixture.data.proxy.running = true;
        fixture.data.proxy.claude_takeover = true;
        fixture.data.proxy.auto_failover_enabled = true;
        move_failover_queue(&mut fixture.ctx(), "p2".to_string(), MoveDirection::Up)
            .expect("move queued provider up");

        let queue = state.db.get_failover_queue("claude").expect("read queue");
        assert_eq!(queue[0].provider_id, "p2");
        assert_eq!(queue[1].provider_id, "p1");
    }

    #[test]
    #[serial(home_settings)]
    fn active_proxy_failover_rejects_deleting_last_queued_provider() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let state = load_state().expect("load state");
        add_claude_queue_provider(&state, "p1").expect("add provider");
        state
            .db
            .add_to_failover_queue("claude", "p1")
            .expect("queue provider");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create runtime");
        runtime.block_on(async {
            let mut config = state.db.get_proxy_config_for_app("claude").await.unwrap();
            config.enabled = true;
            config.auto_failover_enabled = true;
            state.db.update_proxy_config_for_app(config).await.unwrap();
        });

        let mut fixture = RuntimeActionFixture::new(AppType::Claude);
        reload_fixture_data(&mut fixture);
        fixture.data.proxy.running = true;
        fixture.data.proxy.claude_takeover = true;
        fixture.data.proxy.auto_failover_enabled = true;
        delete(&mut fixture.ctx(), "p1".to_string()).expect("attempt delete provider");

        assert!(state
            .db
            .get_provider_by_id("p1", "claude")
            .expect("read provider")
            .is_some());
        assert!(state
            .db
            .is_in_failover_queue("claude", "p1")
            .expect("read queue membership"));
    }

    #[test]
    #[serial(home_settings)]
    fn provider_failover_queue_toggle_updates_database() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let state = load_state().expect("load state");
        seed_claude_live_settings(json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://example.com",
                "LOCAL_ONLY": "preserve-me"
            }
        }))
        .expect("seed live settings");
        ProviderService::add(
            &state,
            AppType::Claude,
            Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
                None,
            ),
        )
        .expect("add provider");

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
            session_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut webdav_loading,
            update_req_tx: None,
            update_check: &mut update_check,
            model_fetch_req_tx: None,
            managed_auth_req_tx: None,
        };

        set_failover_queue(&mut ctx, "p1".to_string(), true).expect("enable failover queue");
        assert!(state
            .db
            .is_in_failover_queue("claude", "p1")
            .expect("read failover queue membership"));
        assert!(ctx
            .data
            .providers
            .rows
            .iter()
            .any(|row| row.id == "p1" && row.provider.in_failover_queue));

        set_failover_queue(&mut ctx, "p1".to_string(), false).expect("disable failover queue");
        assert!(!state
            .db
            .is_in_failover_queue("claude", "p1")
            .expect("read failover queue membership"));
        assert!(ctx
            .data
            .providers
            .rows
            .iter()
            .any(|row| row.id == "p1" && !row.provider.in_failover_queue));
    }

    #[test]
    #[serial(home_settings)]
    fn provider_failover_queue_move_updates_sort_order() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let state = load_state().expect("load state");
        let mut first = Provider::with_id(
            "first".to_string(),
            "First".to_string(),
            json!({"env":{"ANTHROPIC_BASE_URL":"https://first.example.com"}}),
            None,
        );
        first.sort_index = Some(0);
        let mut second = Provider::with_id(
            "second".to_string(),
            "Second".to_string(),
            json!({"env":{"ANTHROPIC_BASE_URL":"https://second.example.com"}}),
            None,
        );
        second.sort_index = Some(1);
        seed_claude_live_settings(json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://first.example.com",
                "LOCAL_ONLY": "preserve-me"
            }
        }))
        .expect("seed first live settings");
        ProviderService::add(&state, AppType::Claude, first).expect("add first provider");
        seed_claude_live_settings(json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://second.example.com",
                "LOCAL_ONLY": "preserve-me"
            }
        }))
        .expect("seed second live settings");
        ProviderService::add(&state, AppType::Claude, second).expect("add second provider");
        state
            .db
            .add_to_failover_queue("claude", "first")
            .expect("queue first provider");

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
            session_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut webdav_loading,
            update_req_tx: None,
            update_check: &mut update_check,
            model_fetch_req_tx: None,
            managed_auth_req_tx: None,
        };

        set_failover_queue(&mut ctx, "second".to_string(), true)
            .expect("queue second provider before moving");
        assert!(ctx
            .data
            .providers
            .rows
            .iter()
            .any(|row| row.id == "second" && row.provider.in_failover_queue));

        move_failover_queue(
            &mut ctx,
            "second".to_string(),
            crate::cli::tui::app::MoveDirection::Up,
        )
        .expect("move second provider up");

        let queue = state.db.get_failover_queue("claude").expect("read queue");
        assert_eq!(queue[0].provider_id, "second");
        assert_eq!(queue[1].provider_id, "first");
    }

    #[test]
    #[serial(home_settings)]
    fn provider_switch_does_not_show_restart_toast_when_live_sync_succeeds() {
        let fixture = run_codex_switch(
            "old-provider",
            Some(MATCHING_CODEX_LIVE_CONFIG),
            Some(json!({"OPENAI_API_KEY": "fresh-key", "LOCAL_ONLY": "preserve-me"})),
        )
        .expect("switch should succeed");

        assert_eq!(fixture.data.providers.current_id, "new-provider");
        assert!(
            fixture.app.toast.is_none(),
            "provider switch should not show restart toast"
        );
    }

    #[test]
    #[serial(home_settings)]
    fn provider_switch_does_not_show_restart_toast_when_live_sync_is_skipped() {
        let fixture = run_codex_switch("old-provider", None, None).expect("switch should succeed");

        assert_eq!(fixture.data.providers.current_id, "new-provider");
        assert!(
            fixture.app.toast.is_none(),
            "provider switch should not show restart toast"
        );
    }

    #[test]
    #[serial(home_settings)]
    fn provider_switch_overwrites_existing_codex_settings_without_prompt() {
        let fixture = run_codex_switch("", Some(MATCHING_CODEX_LIVE_CONFIG), None)
            .expect("switch should succeed");

        assert_eq!(fixture.data.providers.current_id, "new-provider");
        assert!(matches!(fixture.app.overlay, Overlay::None));
    }

    #[test]
    #[serial(home_settings)]
    fn provider_switch_without_existing_codex_settings_switches_normally() {
        let fixture = run_codex_switch("", None, None).expect("switch should succeed");

        assert_eq!(fixture.data.providers.current_id, "new-provider");
        assert!(matches!(fixture.app.overlay, Overlay::None));
    }

    #[test]
    #[serial(home_settings)]
    fn provider_switch_codex_auth_only_state_switches_normally() {
        let fixture = run_codex_switch(
            "",
            None,
            Some(json!({"OPENAI_API_KEY": "fresh-key", "LOCAL_ONLY": "preserve-me"})),
        )
        .expect("switch should succeed");

        assert_eq!(fixture.data.providers.current_id, "new-provider");
        assert!(matches!(fixture.app.overlay, Overlay::None));
    }

    #[test]
    #[serial(home_settings)]
    fn opencode_switch_toggles_config_membership_without_current_provider() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        let opencode_dir = temp_home.path().join("opencode");
        std::fs::create_dir_all(&opencode_dir).expect("create opencode dir");
        let _settings = SettingsGuard::with_opencode_dir(&opencode_dir);

        let mut config = MultiAppConfig::default();
        let manager = config
            .get_manager_mut(&AppType::OpenCode)
            .expect("opencode manager");
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "OpenCode Provider".to_string(),
                json!({
                    "npm": "@ai-sdk/openai-compatible",
                    "options": {
                        "baseURL": "https://opencode.example.com/v1"
                    },
                    "models": {
                        "main": {"name": "Main"}
                    }
                }),
                None,
            ),
        );
        config.save().expect("persist opencode provider");

        let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
        let mut app = App::new(Some(AppType::OpenCode));
        let mut data = UiData::load(&AppType::OpenCode).expect("load initial opencode data");
        assert_eq!(data.providers.current_id, "");
        assert!(
            data.providers
                .rows
                .iter()
                .any(|row| row.id == "p1" && !row.is_in_config),
            "precondition: saved provider should start outside OpenCode config"
        );
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

        switch(&mut ctx, "p1".to_string()).expect("add opencode provider to config");

        assert_eq!(ctx.data.providers.current_id, "");
        assert!(crate::opencode_config::get_providers()
            .expect("read opencode providers")
            .contains_key("p1"));
        assert!(ctx
            .data
            .providers
            .rows
            .iter()
            .any(|row| row.id == "p1" && row.is_in_config && !row.is_current));
        let added_row = ctx
            .data
            .providers
            .rows
            .iter()
            .find(|row| row.id == "p1")
            .expect("added provider should remain saved");
        assert_eq!(
            added_row
                .provider
                .meta
                .as_ref()
                .and_then(|meta| meta.live_config_managed),
            Some(true)
        );
        assert!(ctx.app.toast.is_some());

        remove_from_config(&mut ctx, "p1".to_string())
            .expect("remove opencode provider from config");

        assert_eq!(ctx.data.providers.current_id, "");
        assert!(!crate::opencode_config::get_providers()
            .expect("read opencode providers after remove")
            .contains_key("p1"));
        let removed_row = ctx
            .data
            .providers
            .rows
            .iter()
            .find(|row| row.id == "p1")
            .expect("removed provider should remain saved");
        assert!(!removed_row.is_in_config);
        assert!(!removed_row.is_current);
        assert!(removed_row.is_saved);
        assert_eq!(
            removed_row
                .provider
                .meta
                .as_ref()
                .and_then(|meta| meta.live_config_managed),
            Some(false)
        );
    }

    #[test]
    #[serial(home_settings)]
    fn hermes_remove_from_config_rejects_current_provider_and_keeps_non_current_visible_for_re_add()
    {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        let hermes_dir = temp_home.path().join(".hermes");
        std::fs::create_dir_all(&hermes_dir).expect("create hermes dir");
        std::fs::write(
            hermes_dir.join("config.yaml"),
            "custom_providers: []\nmodel: {}\n",
        )
        .expect("write hermes config");
        let _settings = SettingsGuard::with_hermes_dir(&hermes_dir);

        let mut config = MultiAppConfig::default();
        let manager = config
            .get_manager_mut(&AppType::Hermes)
            .expect("hermes manager");
        manager.providers.insert(
            "p1".to_string(),
            Provider::with_id(
                "p1".to_string(),
                "Hermes Provider".to_string(),
                json!({
                    "base_url": "https://hermes.example.com/v1",
                    "api_key": "sk-demo",
                    "models": [{"id": "main", "name": "Main"}]
                }),
                None,
            ),
        );
        manager.providers.insert(
            "p2".to_string(),
            Provider::with_id(
                "p2".to_string(),
                "Hermes Secondary".to_string(),
                json!({
                    "base_url": "https://secondary.example.com/v1",
                    "api_key": "sk-secondary",
                    "models": [{"id": "secondary", "name": "Secondary"}]
                }),
                None,
            ),
        );
        config.save().expect("persist hermes provider");

        let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
        let mut app = App::new(Some(AppType::Hermes));
        let mut data = UiData::load(&AppType::Hermes).expect("load initial hermes data");
        assert_eq!(data.providers.current_id, "");
        assert!(
            data.providers
                .rows
                .iter()
                .any(|row| row.id == "p1" && !row.is_in_config && !row.is_current),
            "precondition: saved provider should start outside Hermes config"
        );
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

        switch(&mut ctx, "p1".to_string()).expect("add and enable hermes provider");

        assert_eq!(ctx.data.providers.current_id, "p1");
        assert_eq!(
            crate::hermes_config::get_current_provider_id().expect("read hermes current"),
            Some("p1".to_string())
        );
        assert!(crate::hermes_config::get_providers()
            .expect("read hermes providers")
            .contains_key("p1"));
        assert!(ctx
            .data
            .providers
            .rows
            .iter()
            .any(|row| row.id == "p1" && row.is_in_config && row.is_current));

        let err = remove_from_config(&mut ctx, "p1".to_string())
            .expect_err("current Hermes provider should not be removable from live config");
        assert!(matches!(
            err,
            AppError::Localized {
                key: "provider.remove_from_config.hermes_current",
                ..
            }
        ));
        assert!(crate::hermes_config::get_providers()
            .expect("read hermes providers after failed remove")
            .contains_key("p1"));

        crate::hermes_config::set_provider(
            "p2",
            json!({
                "base_url": "https://secondary.example.com/v1",
                "api_key": "sk-secondary",
                "models": [{"id": "secondary", "name": "Secondary"}]
            }),
        )
        .expect("add secondary hermes provider to live config");
        remove_from_config(&mut ctx, "p2".to_string())
            .expect("remove non-current hermes provider from config");

        assert!(!crate::hermes_config::get_providers()
            .expect("read hermes providers after remove")
            .contains_key("p2"));
        let removed_row = ctx
            .data
            .providers
            .rows
            .iter()
            .find(|row| row.id == "p2")
            .expect("removed provider should remain visible");
        assert!(!removed_row.is_in_config);
        assert!(removed_row.is_saved);
        assert_eq!(
            removed_row
                .provider
                .meta
                .as_ref()
                .and_then(|meta| meta.live_config_managed),
            Some(false)
        );
    }

    #[test]
    #[serial(home_settings)]
    fn provider_switch_existing_codex_install_with_current_provider_switches_normally() {
        let fixture = run_codex_switch("old-provider", Some(MATCHING_CODEX_LIVE_CONFIG), None)
            .expect("switch should succeed");

        assert_eq!(fixture.data.providers.current_id, "new-provider");
        assert!(matches!(fixture.app.overlay, Overlay::None));
    }

    #[test]
    #[serial(home_settings)]
    fn provider_import_codex_live_config_adds_default_provider() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        seed_codex_live_files(
            Some("model_provider = \"legacy\"\nmodel = \"gpt-4\"\n"),
            Some(json!({"OPENAI_API_KEY": "fresh-key", "LOCAL_ONLY": "preserve-me"})),
        )
        .expect("seed codex live files");
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
            session_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut webdav_loading,
            update_req_tx: None,
            update_check: &mut update_check,
            model_fetch_req_tx: None,
            managed_auth_req_tx: None,
        };

        import_live_config(&mut ctx).expect("import live config should succeed");

        assert_eq!(data.providers.current_id, "default");
        assert!(data.providers.rows.iter().any(|row| {
            row.id == "default"
                && row
                    .provider
                    .settings_config
                    .get("config")
                    .and_then(|value| value.as_str())
                    .map(|value| value.contains("model_provider = \"legacy\""))
                    .unwrap_or(false)
                && row.provider.settings_config.get("auth").is_some()
        }));
    }

    #[test]
    #[serial(home_settings)]
    fn provider_switch_warns_when_claude_provider_requires_proxy_and_proxy_is_not_running() {
        let fixture =
            run_claude_switch("old-provider", "openai_chat", false).expect("switch should succeed");

        assert_eq!(fixture.data.providers.current_id, "proxy-provider");
        assert!(matches!(
            fixture.app.overlay,
            Overlay::Confirm(ConfirmOverlay { title, message, action })
                if title.as_str() == texts::tui_claude_api_format_requires_proxy_title()
                    && message.as_str()
                        == texts::tui_claude_api_format_requires_proxy_message("openai_chat")
                            .as_str()
                    && matches!(action, ConfirmAction::ProviderApiFormatProxyNotice)
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn provider_switch_proxy_notice_uses_refreshed_proxy_snapshot() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        claude_test_config("old-provider", "openai_chat")
            .save()
            .expect("persist claude providers");

        let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
        let mut app = App::new(Some(AppType::Claude));
        let mut data = UiData::load(&AppType::Claude).expect("load claude data");
        data.proxy.running = true;
        data.proxy.claude_takeover = true;
        data.proxy.managed_runtime = false;
        assert_eq!(
            data.proxy
                .routes_current_app_through_proxy(&AppType::Claude),
            Some(true),
            "precondition: stale in-memory proxy snapshot should look ready"
        );

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

        switch(&mut ctx, "proxy-provider".to_string()).expect("switch provider");

        assert_eq!(ctx.data.providers.current_id, "proxy-provider");
        assert!(matches!(
            &ctx.app.overlay,
            Overlay::Confirm(ConfirmOverlay { title, message, action })
                if title.as_str() == texts::tui_claude_api_format_requires_proxy_title()
                    && message.as_str()
                        == texts::tui_claude_api_format_requires_proxy_message("openai_chat")
                            .as_str()
                    && matches!(action, ConfirmAction::ProviderApiFormatProxyNotice)
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn provider_switch_warns_for_openai_responses_when_proxy_is_not_running() {
        let fixture = run_claude_switch("old-provider", "openai_responses", false)
            .expect("switch should succeed");

        assert_eq!(fixture.data.providers.current_id, "proxy-provider");
        assert!(matches!(
            fixture.app.overlay,
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
    #[serial(home_settings)]
    fn provider_switch_does_not_warn_when_claude_provider_uses_anthropic_format() {
        let fixture =
            run_claude_switch("old-provider", "anthropic", false).expect("switch should succeed");

        assert_eq!(fixture.data.providers.current_id, "proxy-provider");
        assert!(matches!(fixture.app.overlay, Overlay::None));
    }

    #[test]
    #[serial(home_settings)]
    fn provider_switch_overwrites_existing_claude_settings_without_prompt() {
        let fixture = run_claude_switch("", "anthropic", true).expect("switch should succeed");

        assert_eq!(fixture.data.providers.current_id, "proxy-provider");
        assert!(matches!(fixture.app.overlay, Overlay::None));
    }

    #[test]
    #[serial(home_settings)]
    fn provider_switch_without_existing_claude_settings_switches_normally() {
        let fixture = run_claude_switch("", "anthropic", false).expect("switch should succeed");

        assert_eq!(fixture.data.providers.current_id, "proxy-provider");
        assert!(matches!(fixture.app.overlay, Overlay::None));
    }

    #[test]
    #[serial(home_settings)]
    fn provider_switch_existing_install_with_current_provider_switches_normally() {
        let fixture =
            run_claude_switch("old-provider", "anthropic", true).expect("switch should succeed");

        assert_eq!(fixture.data.providers.current_id, "proxy-provider");
        assert!(matches!(fixture.app.overlay, Overlay::None));
    }

    #[test]
    #[serial(home_settings)]
    fn provider_import_live_config_adds_and_selects_default_provider() {
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
            session_req_tx: None,
            webdav_req_tx: None,
            webdav_loading: &mut webdav_loading,
            update_req_tx: None,
            update_check: &mut update_check,
            model_fetch_req_tx: None,
            managed_auth_req_tx: None,
        };

        import_live_config(&mut ctx).expect("import live config should succeed");

        assert_eq!(data.providers.current_id, "default");
        assert!(data.providers.rows.iter().any(|row| {
            row.id == "default"
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
    #[serial(home_settings)]
    fn openclaw_set_default_model_preserves_provider_model_order_as_fallbacks() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(temp_home.path());

        crate::openclaw_config::set_provider(
            "p1",
            json!({
                "api": "openai-completions",
                "models": [
                    {"id": "model-primary", "name": "Primary"},
                    {"id": "model-fallback-1", "name": "Fallback 1"},
                    {"id": "model-fallback-2", "name": "Fallback 2"}
                ]
            }),
        )
        .expect("seed live openclaw provider");

        let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
        let mut app = App::new(Some(AppType::OpenClaw));
        let mut data = UiData::default();
        data.config.openclaw_agents_defaults =
            Some(crate::openclaw_config::OpenClawAgentsDefaults {
                model: Some(OpenClawDefaultModel {
                    primary: "stale-provider/stale-model".to_string(),
                    fallbacks: Vec::new(),
                    extra: HashMap::new(),
                }),
                models: None,
                extra: HashMap::new(),
            });
        data.providers
            .rows
            .push(crate::cli::tui::data::ProviderRow {
                id: "p1".to_string(),
                provider: Provider::with_id(
                    "p1".to_string(),
                    "Provider One".to_string(),
                    json!({
                        "api": "openai-completions",
                        "models": [
                            {"id": "model-primary", "name": "Primary"},
                            {"id": "model-fallback-1", "name": "Fallback 1"},
                            {"id": "model-fallback-2", "name": "Fallback 2"}
                        ]
                    }),
                    None,
                ),
                api_url: Some("https://example.com".to_string()),
                is_current: false,
                is_in_config: true,
                is_saved: true,
                is_default_model: false,
                primary_model_id: Some("model-primary".to_string()),
                default_model_id: None,
            });
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

        set_default_model(&mut ctx, "p1".to_string(), "model-primary".to_string())
            .expect("set default model");

        let default_model = crate::openclaw_config::get_default_model()
            .expect("read default model")
            .expect("default model should exist");
        assert_eq!(default_model.primary, "p1/model-primary");
        assert_eq!(
            default_model.fallbacks,
            vec![
                "p1/model-fallback-1".to_string(),
                "p1/model-fallback-2".to_string()
            ]
        );
        let refreshed_snapshot_model = ctx
            .data
            .config
            .openclaw_agents_defaults
            .as_ref()
            .and_then(|defaults| defaults.model.as_ref())
            .expect("ui config snapshot should refresh after setting default model");
        assert_eq!(refreshed_snapshot_model.primary, "p1/model-primary");
        assert_eq!(
            refreshed_snapshot_model.fallbacks,
            vec![
                "p1/model-fallback-1".to_string(),
                "p1/model-fallback-2".to_string()
            ]
        );
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_set_default_model_uses_live_primary_when_snapshot_primary_is_stale() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(temp_home.path());

        crate::openclaw_config::set_provider(
            "p1",
            json!({
                "api": "openai-completions",
                "models": [
                    {"id": "live-primary", "name": "Live Primary"},
                    {"id": "snapshot-primary", "name": "Snapshot Primary"},
                    {"id": "fallback-2", "name": "Fallback 2"}
                ]
            }),
        )
        .expect("seed live openclaw provider");
        crate::openclaw_config::set_default_model(&OpenClawDefaultModel {
            primary: "p1/snapshot-primary".to_string(),
            fallbacks: vec!["p1/live-primary".to_string(), "p1/fallback-2".to_string()],
            extra: HashMap::new(),
        })
        .expect("seed existing default model");

        let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
        let mut app = App::new(Some(AppType::OpenClaw));
        let mut data = UiData::default();
        data.providers
            .rows
            .push(crate::cli::tui::data::ProviderRow {
                id: "p1".to_string(),
                provider: Provider::with_id(
                    "p1".to_string(),
                    "Provider One".to_string(),
                    json!({
                        "api": "openai-completions",
                        "models": [
                            {"id": "snapshot-primary", "name": "Snapshot Primary"},
                            {"id": "live-primary", "name": "Live Primary"},
                            {"id": "fallback-2", "name": "Fallback 2"}
                        ]
                    }),
                    None,
                ),
                api_url: Some("https://example.com".to_string()),
                is_current: false,
                is_in_config: true,
                is_saved: true,
                is_default_model: true,
                primary_model_id: Some("snapshot-primary".to_string()),
                default_model_id: Some("snapshot-primary".to_string()),
            });
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

        set_default_model(&mut ctx, "p1".to_string(), "snapshot-primary".to_string())
            .expect("set default model from x action");

        let default_model = crate::openclaw_config::get_default_model()
            .expect("read default model")
            .expect("default model should exist");
        assert_eq!(default_model.primary, "p1/live-primary");
        assert_eq!(
            default_model.fallbacks,
            vec![
                "p1/snapshot-primary".to_string(),
                "p1/fallback-2".to_string()
            ]
        );
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_set_default_model_preserves_existing_extra_fields() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(temp_home.path());

        crate::openclaw_config::set_provider(
            "p1",
            json!({
                "api": "openai-completions",
                "models": [
                    {"id": "model-primary", "name": "Primary"},
                    {"id": "model-fallback-1", "name": "Fallback 1"}
                ]
            }),
        )
        .expect("seed live openclaw provider");
        crate::openclaw_config::set_default_model(&OpenClawDefaultModel {
            primary: "p1/model-fallback-1".to_string(),
            fallbacks: vec!["p1/model-primary".to_string()],
            extra: HashMap::from([("reasoningEffort".to_string(), json!("high"))]),
        })
        .expect("seed existing default model");

        let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
        let mut app = App::new(Some(AppType::OpenClaw));
        let mut data = UiData::default();
        data.providers
            .rows
            .push(crate::cli::tui::data::ProviderRow {
                id: "p1".to_string(),
                provider: Provider::with_id(
                    "p1".to_string(),
                    "Provider One".to_string(),
                    json!({
                        "api": "openai-completions",
                        "models": [
                            {"id": "model-primary", "name": "Primary"},
                            {"id": "model-fallback-1", "name": "Fallback 1"}
                        ]
                    }),
                    None,
                ),
                api_url: Some("https://example.com".to_string()),
                is_current: false,
                is_in_config: true,
                is_saved: true,
                is_default_model: true,
                primary_model_id: Some("model-primary".to_string()),
                default_model_id: Some("model-fallback-1".to_string()),
            });
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

        set_default_model(&mut ctx, "p1".to_string(), "model-primary".to_string())
            .expect("set default model");

        let default_model = crate::openclaw_config::get_default_model()
            .expect("read default model")
            .expect("default model should exist");
        assert_eq!(
            default_model.extra.get("reasoningEffort"),
            Some(&json!("high"))
        );
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_remove_from_config_rejects_default_provider_even_without_ui_guard() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(temp_home.path());

        crate::openclaw_config::set_provider(
            "p1",
            json!({
                "api": "openai-completions",
                "models": [{"id": "model-primary"}]
            }),
        )
        .expect("seed live openclaw provider");
        crate::openclaw_config::set_default_model(&OpenClawDefaultModel {
            primary: "p1/model-primary".to_string(),
            fallbacks: Vec::new(),
            extra: HashMap::new(),
        })
        .expect("seed default model");

        let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
        let mut app = App::new(Some(AppType::OpenClaw));
        let mut data = UiData::default();
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

        let err = remove_from_config(&mut ctx, "p1".to_string())
            .expect_err("default provider should not be removable from live config");
        match err {
            AppError::Localized { zh, .. } => assert!(zh.contains("默认")),
            AppError::Config(msg) => assert!(msg.contains("默认")),
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(crate::openclaw_config::get_providers()
            .expect("read providers after failed remove")
            .contains_key("p1"));
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_remove_from_config_keeps_removed_provider_visible_for_re_add() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(temp_home.path());

        crate::openclaw_config::set_provider(
            "p1",
            json!({
                "api": "openai-completions",
                "models": [{"id": "primary-model"}]
            }),
        )
        .expect("seed primary live openclaw provider");
        crate::openclaw_config::set_provider(
            "p2",
            json!({
                "api": "openai-completions",
                "models": [{"id": "shared-model"}]
            }),
        )
        .expect("seed fallback live openclaw provider");
        crate::openclaw_config::set_default_model(&OpenClawDefaultModel {
            primary: "p1/primary-model".to_string(),
            fallbacks: vec!["p2/shared-model".to_string()],
            extra: HashMap::new(),
        })
        .expect("seed default model with fallback-only provider reference");

        let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
        let mut app = App::new(Some(AppType::OpenClaw));
        let mut data = UiData::load(&AppType::OpenClaw).expect("load initial openclaw ui data");
        assert!(
            data.providers
                .rows
                .iter()
                .any(|row| row.id == "p2" && row.is_in_config),
            "precondition: fallback provider should start visible in config"
        );
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

        remove_from_config(&mut ctx, "p2".to_string())
            .expect("fallback-only default reference should be removable");
        assert!(ctx.app.toast.is_some());
        assert!(!crate::openclaw_config::get_providers()
            .expect("read providers after successful remove")
            .contains_key("p2"));
        let default_model = crate::openclaw_config::get_default_model()
            .expect("read default model after remove")
            .expect("default model should remain present");
        assert_eq!(default_model.primary, "p1/primary-model");
        assert_eq!(default_model.fallbacks, vec!["p2/shared-model".to_string()]);
        let removed_row = ctx
            .data
            .providers
            .rows
            .iter()
            .find(|row| row.id == "p2")
            .expect("removed provider should remain visible after reload for re-adding");
        assert!(!removed_row.is_in_config);
        assert!(removed_row.is_saved);
        assert_eq!(
            removed_row
                .provider
                .meta
                .as_ref()
                .and_then(|meta| meta.live_config_managed),
            Some(false)
        );
    }

    #[test]
    #[serial(home_settings)]
    fn openclaw_set_default_model_uses_live_primary_when_snapshot_model_is_missing() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        let _settings = SettingsGuard::with_openclaw_dir(temp_home.path());

        crate::openclaw_config::set_provider(
            "p1",
            json!({
                "api": "openai-completions",
                "models": [{"id": "live-model-only"}]
            }),
        )
        .expect("seed live openclaw provider");

        let mut terminal = TuiTerminal::new_for_test().expect("create terminal");
        let mut app = App::new(Some(AppType::OpenClaw));
        let mut data = UiData::default();
        data.providers
            .rows
            .push(crate::cli::tui::data::ProviderRow {
                id: "p1".to_string(),
                provider: Provider::with_id(
                    "p1".to_string(),
                    "Provider One".to_string(),
                    json!({
                        "api": "openai-completions",
                        "models": [
                            {"id": "model-primary"},
                            {"id": "model-fallback-1"}
                        ]
                    }),
                    None,
                ),
                api_url: Some("https://example.com".to_string()),
                is_current: false,
                is_in_config: true,
                is_saved: true,
                is_default_model: false,
                primary_model_id: Some("model-primary".to_string()),
                default_model_id: None,
            });
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

        set_default_model(&mut ctx, "p1".to_string(), "model-primary".to_string())
            .expect("x action should fall back to live primary");

        let default_model = crate::openclaw_config::get_default_model()
            .expect("read default model")
            .expect("default model should exist");
        assert_eq!(default_model.primary, "p1/live-model-only");
        assert!(default_model.fallbacks.is_empty());
    }
}
