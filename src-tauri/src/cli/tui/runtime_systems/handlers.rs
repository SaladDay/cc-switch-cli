use crate::cli::i18n::texts;
use crate::error::AppError;
use crate::services::SyncDecision;
use crate::settings::{
    get_webdav_sync_settings, set_webdav_sync_settings, webdav_jianguoyun_preset,
    WebDavSyncSettings,
};

use super::super::app::{App, ConfirmAction, ConfirmOverlay, LoadingKind, Overlay, ToastKind};
use super::super::data::{load_state, UiData};
use super::super::runtime_actions::app_display_name;
use super::types::{
    build_stream_check_result_lines, LocalEnvMsg, ModelFetchMsg, ProxyMsg, QuotaMsg,
    RequestTracker, SkillsMsg, SpeedtestMsg, StreamCheckMsg, UpdateMsg, WebDavDone, WebDavErr,
    WebDavMsg, WebDavReqKind,
};

pub(crate) fn handle_stream_check_msg(app: &mut App, msg: StreamCheckMsg) {
    match msg {
        StreamCheckMsg::Finished { req, result } => match result {
            Ok(result) => {
                let lines = build_stream_check_result_lines(&req.provider_name, &result);
                match &app.overlay {
                    Overlay::StreamCheckRunning { provider_id, .. }
                        if provider_id == &req.provider_id =>
                    {
                        app.overlay = Overlay::StreamCheckResult {
                            provider_name: req.provider_name,
                            lines,
                            scroll: 0,
                        };
                    }
                    _ => {
                        app.push_toast(
                            texts::tui_toast_stream_check_finished(),
                            ToastKind::Success,
                        );
                    }
                }
            }
            Err(err) => {
                app.push_toast(texts::tui_toast_stream_check_failed(&err), ToastKind::Error);
                if matches!(&app.overlay, Overlay::StreamCheckRunning { provider_id, .. } if provider_id == &req.provider_id)
                {
                    app.overlay = Overlay::None;
                }
            }
        },
    }
}

pub(crate) fn handle_speedtest_msg(app: &mut App, msg: SpeedtestMsg) {
    match msg {
        SpeedtestMsg::Finished { url, result } => match result {
            Ok(rows) => {
                let mut lines = vec![texts::tui_speedtest_line_url(&url), String::new()];
                for row in rows {
                    let latency = row
                        .latency
                        .map(texts::tui_latency_ms)
                        .unwrap_or_else(|| texts::tui_na().to_string());
                    let status = row
                        .status
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| texts::tui_na().to_string());
                    let err = row.error.unwrap_or_default();

                    lines.push(texts::tui_speedtest_line_latency(&latency));
                    lines.push(texts::tui_speedtest_line_status(&status));
                    if !err.trim().is_empty() {
                        lines.push(texts::tui_speedtest_line_error(&err));
                    }
                }

                match &app.overlay {
                    Overlay::SpeedtestRunning { url: running_url } if running_url == &url => {
                        app.overlay = Overlay::SpeedtestResult {
                            url,
                            lines,
                            scroll: 0,
                        };
                    }
                    _ => {
                        app.push_toast(texts::tui_toast_speedtest_finished(), ToastKind::Success);
                    }
                }
            }
            Err(err) => {
                app.push_toast(texts::tui_toast_speedtest_failed(&err), ToastKind::Error);
                if matches!(&app.overlay, Overlay::SpeedtestRunning { url: running_url } if running_url == &url)
                {
                    app.overlay = Overlay::None;
                }
            }
        },
    }
}

pub(crate) fn handle_local_env_msg(app: &mut App, msg: LocalEnvMsg) {
    match msg {
        LocalEnvMsg::Finished { result } => {
            app.local_env_results = result;
            app.local_env_loading = false;
        }
    }
}

pub(crate) fn handle_quota_msg(app: &mut App, data: &mut UiData, msg: QuotaMsg) {
    match msg {
        QuotaMsg::Finished { target, result } => {
            if !data.quota.target_is_current(&target) {
                return;
            }

            let was_manual = data.quota.has_manual_loading(&target);
            match result {
                Ok(quota) => {
                    let provider_name = target.provider_name.clone();
                    data.quota.finish(target, quota);
                    if was_manual {
                        app.push_toast(
                            texts::tui_toast_quota_refresh_finished(&provider_name),
                            ToastKind::Success,
                        );
                    }
                }
                Err(error) => {
                    data.quota.finish_error(target, error.clone());
                    app.push_toast(
                        texts::tui_toast_quota_refresh_failed(&error),
                        ToastKind::Warning,
                    );
                }
            }
        }
    }
}

pub(crate) fn handle_model_fetch_msg(app: &mut App, msg: ModelFetchMsg) {
    match msg {
        ModelFetchMsg::Finished {
            request_id,
            field,
            claude_idx,
            result,
        } => {
            if let Overlay::ModelFetchPicker {
                request_id: current_request_id,
                fetching: ref mut f,
                models: ref mut m,
                error: ref mut e,
                field: ref current_field,
                claude_idx: ref current_claude_idx,
                ..
            } = app.overlay
            {
                if current_request_id != request_id {
                    return;
                }
                if current_field == &field && current_claude_idx == &claude_idx {
                    *f = false;
                    match result {
                        Ok(fetched_models) => {
                            if fetched_models.is_empty() {
                                *e = Some(texts::tui_model_fetch_no_models().to_string());
                            } else {
                                *m = fetched_models;
                                *e = None;
                            }
                        }
                        Err(err) => {
                            *e = Some(texts::tui_model_fetch_error_hint(&err));
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn handle_skills_msg(
    app: &mut App,
    data: &mut UiData,
    msg: SkillsMsg,
) -> Result<(), AppError> {
    match msg {
        SkillsMsg::DiscoverFinished { query, result } => match result {
            Ok(skills) => {
                app.overlay = Overlay::None;
                app.skills_discover_results = skills;
                app.skills_discover_idx = 0;
                app.skills_discover_query = query.clone();
                app.push_toast(
                    texts::tui_toast_skills_discover_finished(app.skills_discover_results.len()),
                    ToastKind::Success,
                );
            }
            Err(err) => {
                app.overlay = Overlay::None;
                app.push_toast(
                    texts::tui_toast_skills_discover_failed(&err),
                    ToastKind::Error,
                );
            }
        },
        SkillsMsg::InstallFinished { spec, result } => match result {
            Ok(installed) => {
                app.overlay = Overlay::None;
                *data = UiData::load(&app.app_type)?;

                for row in app.skills_discover_results.iter_mut() {
                    if row.directory.eq_ignore_ascii_case(&installed.directory) {
                        row.installed = true;
                    }
                }

                app.push_toast(
                    texts::tui_toast_skill_installed(&installed.directory),
                    ToastKind::Success,
                );
            }
            Err(err) => {
                app.overlay = Overlay::None;
                app.push_toast(
                    texts::tui_toast_skill_install_failed(&spec, &err),
                    ToastKind::Error,
                );
            }
        },
    }

    Ok(())
}

fn is_webdav_loading_overlay(app: &App) -> bool {
    matches!(
        &app.overlay,
        Overlay::Loading {
            kind: LoadingKind::WebDav,
            ..
        }
    )
}

pub(crate) fn handle_webdav_msg(
    app: &mut App,
    data: &mut UiData,
    webdav_loading: &mut RequestTracker,
    msg: WebDavMsg,
) -> Result<(), AppError> {
    match msg {
        WebDavMsg::Finished {
            request_id,
            req,
            result,
        } => match result {
            Ok(done) => {
                if webdav_loading.is_stale(request_id) {
                    return Ok(());
                }

                if webdav_loading.finish_if_active(request_id) && is_webdav_loading_overlay(app) {
                    app.overlay = Overlay::None;
                }

                match done {
                    WebDavDone::ConnectionChecked => {
                        update_webdav_last_error(None);
                        app.push_toast(texts::tui_toast_webdav_connection_ok(), ToastKind::Success);
                    }
                    WebDavDone::Uploaded { decision, message } => {
                        let msg = match decision {
                            SyncDecision::Upload => texts::tui_toast_webdav_upload_ok().to_string(),
                            _ => message,
                        };
                        app.push_toast(msg, ToastKind::Success);
                    }
                    WebDavDone::Downloaded { decision, message } => {
                        match decision {
                            SyncDecision::V1MigrationNeeded => {
                                app.overlay = Overlay::Confirm(ConfirmOverlay {
                                    title: texts::tui_webdav_v1_migration_title().to_string(),
                                    message: texts::tui_webdav_v1_migration_message().to_string(),
                                    action: ConfirmAction::WebDavMigrateV1ToV2,
                                });
                            }
                            _ => {
                                let msg = match decision {
                                    SyncDecision::Download => {
                                        texts::tui_toast_webdav_download_ok().to_string()
                                    }
                                    _ => message,
                                };
                                if let Ok(state) = load_state() {
                                    if let Err(e) = crate::services::provider::ProviderService::sync_current_to_live(
                                    &state,
                                ) {
                                    log::warn!("WebDAV 下载后同步 live 配置失败: {e}");
                                }
                                }
                                app.push_toast(msg, ToastKind::Success);
                            }
                        }
                    }
                    WebDavDone::V1Migrated { message: _ } => {
                        if let Ok(state) = load_state() {
                            if let Err(e) =
                                crate::services::provider::ProviderService::sync_current_to_live(
                                    &state,
                                )
                            {
                                log::warn!("WebDAV V1 迁移后同步 live 配置失败: {e}");
                            }
                        }
                        app.push_toast(
                            texts::tui_toast_webdav_v1_migration_ok(),
                            ToastKind::Success,
                        );
                    }
                    WebDavDone::JianguoyunConfigured => {
                        app.push_toast(
                            texts::tui_toast_webdav_jianguoyun_configured(),
                            ToastKind::Success,
                        );
                    }
                }
                *data = UiData::load(&app.app_type)?;
            }
            Err(err) => {
                if webdav_loading.is_stale(request_id) {
                    return Ok(());
                }

                if webdav_loading.finish_if_active(request_id) && is_webdav_loading_overlay(app) {
                    app.overlay = Overlay::None;
                }
                let error_detail = match &err {
                    WebDavErr::Generic(e)
                    | WebDavErr::QuickSetupSave(e)
                    | WebDavErr::QuickSetupCheck(e) => e.clone(),
                };
                update_webdav_last_error(Some(error_detail));
                let msg = match req {
                    WebDavReqKind::CheckConnection => {
                        let detail = match err {
                            WebDavErr::Generic(e)
                            | WebDavErr::QuickSetupSave(e)
                            | WebDavErr::QuickSetupCheck(e) => e,
                        };
                        texts::tui_toast_webdav_action_failed(
                            texts::tui_webdav_loading_title_check_connection(),
                            &detail,
                        )
                    }
                    WebDavReqKind::Upload => {
                        let detail = match err {
                            WebDavErr::Generic(e)
                            | WebDavErr::QuickSetupSave(e)
                            | WebDavErr::QuickSetupCheck(e) => e,
                        };
                        texts::tui_toast_webdav_action_failed(
                            texts::tui_webdav_loading_title_upload(),
                            &detail,
                        )
                    }
                    WebDavReqKind::Download => {
                        let detail = match err {
                            WebDavErr::Generic(e)
                            | WebDavErr::QuickSetupSave(e)
                            | WebDavErr::QuickSetupCheck(e) => e,
                        };
                        texts::tui_toast_webdav_action_failed(
                            texts::tui_webdav_loading_title_download(),
                            &detail,
                        )
                    }
                    WebDavReqKind::MigrateV1ToV2 => {
                        let detail = match err {
                            WebDavErr::Generic(e)
                            | WebDavErr::QuickSetupSave(e)
                            | WebDavErr::QuickSetupCheck(e) => e,
                        };
                        texts::tui_toast_webdav_action_failed(
                            texts::tui_webdav_loading_title_v1_migration(),
                            &detail,
                        )
                    }
                    WebDavReqKind::JianguoyunQuickSetup { .. } => match err {
                        WebDavErr::QuickSetupCheck(e) => {
                            texts::tui_toast_webdav_quick_setup_failed(&e)
                        }
                        WebDavErr::QuickSetupSave(e) | WebDavErr::Generic(e) => {
                            texts::tui_toast_webdav_action_failed(
                                texts::tui_webdav_loading_title_quick_setup(),
                                &e,
                            )
                        }
                    },
                };
                *data = UiData::load(&app.app_type)?;
                app.push_toast(msg, ToastKind::Error);
            }
        },
    }
    Ok(())
}

pub(crate) fn handle_proxy_msg(
    app: &mut App,
    data: &mut UiData,
    proxy_loading: &mut RequestTracker,
    msg: ProxyMsg,
) -> Result<(), AppError> {
    match msg {
        ProxyMsg::ManagedSessionFinished {
            request_id,
            app_type,
            enabled,
            result,
        } => {
            if !proxy_loading.finish_if_active(request_id) {
                return Ok(());
            }

            if matches!(
                &app.overlay,
                Overlay::Loading {
                    kind: LoadingKind::Proxy,
                    ..
                }
            ) {
                app.overlay = Overlay::None;
            }

            match result {
                Ok(()) => {
                    *data = UiData::load(&app.app_type)?;
                    app.reset_proxy_activity(
                        data.proxy.estimated_input_tokens_total,
                        data.proxy.estimated_output_tokens_total,
                    );
                    app.push_toast(
                        texts::tui_toast_proxy_managed_current_app_updated(
                            app_display_name(&app_type),
                            enabled,
                        ),
                        ToastKind::Success,
                    );
                }
                Err(err) => {
                    app.push_toast(err, ToastKind::Error);
                }
            }
        }
    }

    Ok(())
}

pub(crate) fn apply_webdav_jianguoyun_quick_setup<FSave, FCheck>(
    username: &str,
    password: &str,
    save_settings: FSave,
    check_connection: FCheck,
) -> Result<(), AppError>
where
    FSave: FnOnce(WebDavSyncSettings) -> Result<(), AppError>,
    FCheck: FnOnce() -> Result<(), AppError>,
{
    let cfg = webdav_jianguoyun_preset(username, password);
    save_settings(cfg)?;
    check_connection()?;
    Ok(())
}

pub(crate) fn update_webdav_last_error_with<FGet, FSet>(
    last_error: Option<String>,
    get: FGet,
    set: FSet,
) where
    FGet: FnOnce() -> Option<WebDavSyncSettings>,
    FSet: FnOnce(WebDavSyncSettings) -> Result<(), AppError>,
{
    let Some(mut cfg) = get() else {
        return;
    };
    cfg.status.last_error = last_error;
    let _ = set(cfg);
}

fn update_webdav_last_error(last_error: Option<String>) {
    update_webdav_last_error_with(last_error, get_webdav_sync_settings, |cfg| {
        set_webdav_sync_settings(Some(cfg))
    });
}

pub(crate) fn handle_update_msg(app: &mut App, update_check: &mut RequestTracker, msg: UpdateMsg) {
    match msg {
        UpdateMsg::CheckFinished { request_id, result } => {
            if !update_check.finish_if_active(request_id) {
                return;
            }

            match result {
                Ok(info) => {
                    if info.is_already_latest {
                        app.overlay = Overlay::None;
                        app.push_toast(
                            texts::tui_toast_already_latest(&info.current_version),
                            ToastKind::Success,
                        );
                    } else if info.is_downgrade {
                        app.overlay = Overlay::None;
                        app.push_toast(
                            texts::tui_toast_update_downgrade(
                                &info.current_version,
                                &info.target_tag,
                            ),
                            ToastKind::Info,
                        );
                    } else {
                        app.overlay = Overlay::UpdateAvailable {
                            current: info.current_version,
                            latest: info.target_tag,
                            selected: 0,
                        };
                    }
                }
                Err(e) => {
                    app.overlay = Overlay::None;
                    app.push_toast(texts::tui_toast_update_check_failed(&e), ToastKind::Error);
                }
            }
        }
        UpdateMsg::DownloadProgress { downloaded, total } => {
            if let Overlay::UpdateDownloading {
                downloaded: ref mut dl,
                total: ref mut t,
            } = app.overlay
            {
                *dl = downloaded;
                *t = total;
            }
        }
        UpdateMsg::DownloadFinished(result) => match result {
            Ok(tag) => {
                app.overlay = Overlay::UpdateResult {
                    success: true,
                    message: texts::tui_update_success(&tag),
                };
            }
            Err(e) => {
                app.overlay = Overlay::UpdateResult {
                    success: false,
                    message: e,
                };
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::AppType;
    use crate::cli::tui::data::{QuotaTarget, QuotaTargetKind};
    use crate::services::{CredentialStatus, SubscriptionQuota};

    fn quota_target() -> QuotaTarget {
        QuotaTarget {
            app_type: AppType::Claude,
            provider_id: "official".to_string(),
            provider_name: "Claude Official".to_string(),
            kind: QuotaTargetKind::SubscriptionTool {
                tool: "claude".to_string(),
            },
        }
    }

    fn quota_result() -> SubscriptionQuota {
        SubscriptionQuota {
            tool: "claude".to_string(),
            credential_status: CredentialStatus::Valid,
            credential_message: None,
            success: true,
            tiers: Vec::new(),
            extra_usage: None,
            error: None,
            queried_at: Some(chrono::Utc::now().timestamp_millis()),
        }
    }

    #[test]
    fn manual_quota_refresh_success_shows_finished_toast() {
        let mut app = App::new(Some(AppType::Claude));
        let mut data = UiData::default();
        let target = quota_target();
        data.quota.mark_loading(target.clone(), true);

        handle_quota_msg(
            &mut app,
            &mut data,
            QuotaMsg::Finished {
                target: target.clone(),
                result: Ok(quota_result()),
            },
        );

        let toast = app
            .toast
            .as_ref()
            .expect("manual refresh completion should show a toast");
        assert_eq!(toast.kind, ToastKind::Success);
        assert_eq!(
            toast.message,
            texts::tui_toast_quota_refresh_finished("Claude Official")
        );
        assert!(!data.quota.has_manual_loading(&target));
    }

    #[test]
    fn automatic_quota_refresh_success_stays_quiet() {
        let mut app = App::new(Some(AppType::Claude));
        let mut data = UiData::default();
        let target = quota_target();
        data.quota.mark_loading(target.clone(), false);

        handle_quota_msg(
            &mut app,
            &mut data,
            QuotaMsg::Finished {
                target,
                result: Ok(quota_result()),
            },
        );

        assert!(
            app.toast.is_none(),
            "automatic background quota refresh should not interrupt the user"
        );
    }
}
