use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::error::AppError;
use crate::services::{SkillService, StreamCheckService, WebDavSyncService};
use crate::settings::{set_webdav_sync_settings, webdav_jianguoyun_preset};

use super::super::data::{
    load_proxy_snapshot_from_state_async, load_snapshot_state, load_state,
    load_usage_pricing_data_from_state_for_range, UiData, UsageRangePreset,
};
use super::types::{
    fetch_provider_models_for_tui, model_fetch_strategy_for_field, AppDataLoadKind, AppDataMsg,
    AppDataReq, AppDataSystem, LocalEnvMsg, LocalEnvReq, LocalEnvSystem, ManagedAuthMsg,
    ManagedAuthReq, ManagedAuthSystem, ModelFetchMsg, ModelFetchReq, ModelFetchSystem, ProxyMsg,
    ProxyReq, ProxySystem, QuotaMsg, QuotaReq, QuotaSystem, SessionMsg, SessionReq, SessionSystem,
    SessionUsageSyncMsg, SessionUsageSyncReq, SessionUsageSyncSystem, SkillsMsg, SkillsReq,
    SkillsSystem, SpeedtestMsg, SpeedtestSystem, StreamCheckMsg, StreamCheckReq, StreamCheckSystem,
    UpdateMsg, UpdateReq, UpdateSystem, UsagePricingMsg, UsagePricingReq, UsagePricingSystem,
    WebDavDone, WebDavErr, WebDavMsg, WebDavReq, WebDavReqKind, WebDavSystem,
};

pub(crate) fn start_proxy_system() -> Result<ProxySystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<ProxyMsg>();
    let (req_tx, req_rx) = mpsc::channel::<ProxyReq>();

    let handle = std::thread::Builder::new()
        .name("cc-switch-proxy".to_string())
        .spawn(move || proxy_worker_loop(req_rx, result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn proxy worker thread".to_string(),
            source: e,
        })?;

    Ok(ProxySystem {
        req_tx,
        result_rx,
        _handle: handle,
    })
}

fn proxy_worker_loop(rx: mpsc::Receiver<ProxyReq>, tx: mpsc::Sender<ProxyMsg>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let err = e.to_string();
            while let Ok(req) = rx.recv() {
                match req {
                    ProxyReq::SetManagedSessionForCurrentApp {
                        request_id,
                        app_type,
                        enabled,
                    } => {
                        let _ = tx.send(ProxyMsg::ManagedSessionFinished {
                            request_id,
                            app_type,
                            enabled,
                            result: Err(err.clone()),
                        });
                    }
                    ProxyReq::RefreshSnapshot {
                        request_id,
                        app_type,
                    } => {
                        let _ = tx.send(ProxyMsg::SnapshotRefreshed {
                            request_id,
                            app_type,
                            result: Err(err.clone()),
                        });
                    }
                }
            }
            return;
        }
    };

    while let Ok(req) = rx.recv() {
        match req {
            ProxyReq::SetManagedSessionForCurrentApp {
                request_id,
                app_type,
                enabled,
            } => {
                let result = load_state().map_err(|e| e.to_string()).and_then(|state| {
                    rt.block_on(
                        state
                            .proxy_service
                            .set_managed_session_for_app(app_type.as_str(), enabled),
                    )
                });

                let _ = tx.send(ProxyMsg::ManagedSessionFinished {
                    request_id,
                    app_type,
                    enabled,
                    result,
                });
            }
            ProxyReq::RefreshSnapshot {
                request_id,
                app_type,
            } => {
                let result = load_state().map_err(|e| e.to_string()).and_then(|state| {
                    rt.block_on(load_proxy_snapshot_from_state_async(&state, &app_type))
                        .map_err(|e| e.to_string())
                });

                let _ = tx.send(ProxyMsg::SnapshotRefreshed {
                    request_id,
                    app_type,
                    result,
                });
            }
        }
    }
}

pub(crate) fn start_update_system() -> Result<UpdateSystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<UpdateMsg>();
    let (req_tx, req_rx) = mpsc::channel::<UpdateReq>();

    let handle = std::thread::Builder::new()
        .name("cc-switch-update".to_string())
        .spawn(move || update_worker_loop(req_rx, result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn update worker thread".to_string(),
            source: e,
        })?;

    Ok(UpdateSystem {
        req_tx,
        result_rx,
        _handle: handle,
    })
}

fn update_worker_loop(rx: mpsc::Receiver<UpdateReq>, tx: mpsc::Sender<UpdateMsg>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let err = e.to_string();
            while let Ok(req) = rx.recv() {
                let msg = match req {
                    UpdateReq::Check { request_id } => UpdateMsg::CheckFinished {
                        request_id,
                        result: Err(err.clone()),
                    },
                    UpdateReq::Download => UpdateMsg::DownloadFinished(Err(err.clone())),
                };
                let _ = tx.send(msg);
            }
            return;
        }
    };

    let mut last_tag: Option<String> = None;

    while let Ok(req) = rx.recv() {
        match req {
            UpdateReq::Check { request_id } => {
                let result = rt
                    .block_on(crate::cli::commands::update::check_for_update())
                    .map_err(|e| e.to_string());
                if let Ok(ref info) = result {
                    last_tag = Some(info.target_tag.clone());
                }
                let _ = tx.send(UpdateMsg::CheckFinished { request_id, result });
            }
            UpdateReq::Download => {
                let Some(tag) = last_tag.clone() else {
                    let _ = tx.send(UpdateMsg::DownloadFinished(Err(
                        texts::tui_update_err_check_first().to_string(),
                    )));
                    continue;
                };
                let tx2 = tx.clone();
                let result = rt
                    .block_on(crate::cli::commands::update::download_and_apply(
                        &tag,
                        move |dl, total| {
                            let _ = tx2.send(UpdateMsg::DownloadProgress {
                                downloaded: dl,
                                total,
                            });
                        },
                    ))
                    .map(|()| tag)
                    .map_err(|e| e.to_string());
                let _ = tx.send(UpdateMsg::DownloadFinished(result));
            }
        }
    }
}

pub(crate) fn start_webdav_system() -> Result<WebDavSystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<WebDavMsg>();
    let (req_tx, req_rx) = mpsc::channel::<WebDavReq>();

    let handle = std::thread::Builder::new()
        .name("cc-switch-webdav".to_string())
        .spawn(move || webdav_worker_loop(req_rx, result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn webdav worker thread".to_string(),
            source: e,
        })?;

    Ok(WebDavSystem {
        req_tx,
        result_rx,
        _handle: handle,
    })
}

pub(crate) fn drain_latest_webdav_req(
    mut req: WebDavReq,
    rx: &mpsc::Receiver<WebDavReq>,
) -> WebDavReq {
    for next in rx.try_iter() {
        req = next;
    }
    req
}

fn webdav_worker_loop(rx: mpsc::Receiver<WebDavReq>, tx: mpsc::Sender<WebDavMsg>) {
    while let Ok(req) = rx.recv() {
        let req = drain_latest_webdav_req(req, &rx);
        let request_id = req.request_id;
        let req_for_msg = req.kind.clone();
        let result = match req.kind {
            WebDavReqKind::CheckConnection => WebDavSyncService::check_connection()
                .map(|_| WebDavDone::ConnectionChecked)
                .map_err(|e| WebDavErr::Generic(e.to_string())),
            WebDavReqKind::Upload => WebDavSyncService::upload()
                .map(|summary| WebDavDone::Uploaded {
                    decision: summary.decision,
                    message: summary.message,
                })
                .map_err(|e| WebDavErr::Generic(e.to_string())),
            WebDavReqKind::Download => WebDavSyncService::download()
                .map(|summary| WebDavDone::Downloaded {
                    decision: summary.decision,
                    message: summary.message,
                })
                .map_err(|e| WebDavErr::Generic(e.to_string())),
            WebDavReqKind::MigrateV1ToV2 => WebDavSyncService::migrate_v1_to_v2()
                .map(|summary| WebDavDone::V1Migrated {
                    message: summary.message,
                })
                .map_err(|e| WebDavErr::Generic(e.to_string())),
            WebDavReqKind::JianguoyunQuickSetup { username, password } => {
                let cfg = webdav_jianguoyun_preset(&username, &password);
                if let Err(err) = set_webdav_sync_settings(Some(cfg)) {
                    Err(WebDavErr::QuickSetupSave(err.to_string()))
                } else if let Err(err) = WebDavSyncService::check_connection() {
                    Err(WebDavErr::QuickSetupCheck(err.to_string()))
                } else {
                    Ok(WebDavDone::JianguoyunConfigured)
                }
            }
        };

        let _ = tx.send(WebDavMsg::Finished {
            request_id,
            req: req_for_msg,
            result,
        });
    }
}

pub(crate) fn start_stream_check_system() -> Result<StreamCheckSystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<StreamCheckMsg>();
    let (req_tx, req_rx) = mpsc::channel::<StreamCheckReq>();

    let handle = std::thread::Builder::new()
        .name("cc-switch-stream-check".to_string())
        .spawn(move || stream_check_worker_loop(req_rx, result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn stream check worker thread".to_string(),
            source: e,
        })?;

    Ok(StreamCheckSystem {
        req_tx,
        result_rx,
        _handle: handle,
    })
}

fn stream_check_worker_loop(rx: mpsc::Receiver<StreamCheckReq>, tx: mpsc::Sender<StreamCheckMsg>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let err = e.to_string();
            while let Ok(req) = rx.recv() {
                let _ = tx.send(StreamCheckMsg::Finished {
                    req,
                    result: Err(err.clone()),
                });
            }
            return;
        }
    };

    while let Ok(mut req) = rx.recv() {
        for next in rx.try_iter() {
            req = next;
        }

        let db = match crate::Database::init() {
            Ok(db) => db,
            Err(err) => {
                let _ = tx.send(StreamCheckMsg::Finished {
                    req,
                    result: Err(err.to_string()),
                });
                continue;
            }
        };

        let config = match db.get_stream_check_config() {
            Ok(config) => config,
            Err(err) => {
                let _ = tx.send(StreamCheckMsg::Finished {
                    req,
                    result: Err(err.to_string()),
                });
                continue;
            }
        };

        let result = rt
            .block_on(async {
                StreamCheckService::check_with_retry(&req.app_type, &req.provider, &config).await
            })
            .map_err(|err| err.to_string());

        if let Ok(ref ok) = result {
            let _ = db.save_stream_check_log(
                &req.provider_id,
                &req.provider_name,
                req.app_type.as_str(),
                ok,
            );
        }

        let _ = tx.send(StreamCheckMsg::Finished { req, result });
    }
}

pub(crate) fn start_speedtest_system() -> Result<SpeedtestSystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<SpeedtestMsg>();
    let (req_tx, req_rx) = mpsc::channel::<String>();

    let handle = std::thread::Builder::new()
        .name("cc-switch-speedtest".to_string())
        .spawn(move || speedtest_worker_loop(req_rx, result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn speedtest worker thread".to_string(),
            source: e,
        })?;

    Ok(SpeedtestSystem {
        req_tx,
        result_rx,
        _handle: handle,
    })
}

fn speedtest_worker_loop(rx: mpsc::Receiver<String>, tx: mpsc::Sender<SpeedtestMsg>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let err = e.to_string();
            while let Ok(url) = rx.recv() {
                let _ = tx.send(SpeedtestMsg::Finished {
                    url,
                    result: Err(err.clone()),
                });
            }
            return;
        }
    };

    while let Ok(mut url) = rx.recv() {
        for next in rx.try_iter() {
            url = next;
        }

        let result = rt
            .block_on(async {
                crate::services::SpeedtestService::test_endpoints(vec![url.clone()], None).await
            })
            .map_err(|e| e.to_string());

        let _ = tx.send(SpeedtestMsg::Finished { url, result });
    }
}

pub(crate) fn start_model_fetch_system() -> Result<ModelFetchSystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<ModelFetchMsg>();
    let (req_tx, req_rx) = mpsc::channel::<ModelFetchReq>();

    let handle = std::thread::Builder::new()
        .name("cc-switch-modelfetch".to_string())
        .spawn(move || model_fetch_worker_loop(req_rx, result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn model fetch worker thread".to_string(),
            source: e,
        })?;

    Ok(ModelFetchSystem {
        req_tx,
        result_rx,
        _handle: handle,
    })
}

fn model_fetch_worker_loop(rx: mpsc::Receiver<ModelFetchReq>, tx: mpsc::Sender<ModelFetchMsg>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let err = e.to_string();
            while let Ok(req) = rx.recv() {
                let ModelFetchReq::Fetch {
                    request_id,
                    field,
                    claude_idx,
                    ..
                } = req;
                let _ = tx.send(ModelFetchMsg::Finished {
                    request_id,
                    field,
                    claude_idx,
                    result: Err(err.clone()),
                });
            }
            return;
        }
    };

    while let Ok(req) = rx.recv() {
        let ModelFetchReq::Fetch {
            request_id,
            base_url,
            api_key,
            codex_oauth,
            codex_oauth_account_id,
            field,
            claude_idx,
        } = req;
        let result = if codex_oauth {
            rt.block_on(async {
                crate::services::CodexOAuthService::get_models(codex_oauth_account_id.as_deref())
                    .await
                    .map(|models| models.into_iter().map(|model| model.id).collect())
            })
        } else {
            let strategy = model_fetch_strategy_for_field(field);
            rt.block_on(async {
                fetch_provider_models_for_tui(&base_url, api_key.as_deref(), strategy).await
            })
            .map_err(|e| e.to_string())
        };

        let _ = tx.send(ModelFetchMsg::Finished {
            request_id,
            field,
            claude_idx,
            result,
        });
    }
}

pub(crate) fn start_managed_auth_system() -> Result<ManagedAuthSystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<ManagedAuthMsg>();
    let (req_tx, req_rx) = mpsc::channel::<ManagedAuthReq>();

    let handle = std::thread::Builder::new()
        .name("cc-switch-managed-auth".to_string())
        .spawn(move || managed_auth_worker_loop(req_rx, result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn managed auth worker thread".to_string(),
            source: e,
        })?;

    Ok(ManagedAuthSystem {
        req_tx,
        result_rx,
        _handle: handle,
    })
}

fn managed_auth_worker_loop(rx: mpsc::Receiver<ManagedAuthReq>, tx: mpsc::Sender<ManagedAuthMsg>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let err = e.to_string();
            while let Ok(req) = rx.recv() {
                let msg = match req {
                    ManagedAuthReq::Refresh { auth_provider } => ManagedAuthMsg::Status {
                        auth_provider,
                        result: Err(err.clone()),
                    },
                    ManagedAuthReq::StartLogin { auth_provider } => ManagedAuthMsg::LoginStarted {
                        auth_provider,
                        result: Err(err.clone()),
                    },
                    ManagedAuthReq::PollLogin {
                        auth_provider,
                        device_code,
                    } => ManagedAuthMsg::LoginPolled {
                        auth_provider,
                        device_code,
                        result: Err(err.clone()),
                    },
                    ManagedAuthReq::SetDefault {
                        auth_provider,
                        account_id,
                    } => ManagedAuthMsg::DefaultSet {
                        auth_provider,
                        account_id,
                        result: Err(err.clone()),
                    },
                    ManagedAuthReq::Remove {
                        auth_provider,
                        account_id,
                    } => ManagedAuthMsg::Removed {
                        auth_provider,
                        account_id,
                        result: Err(err.clone()),
                    },
                };
                let _ = tx.send(msg);
            }
            return;
        }
    };

    while let Ok(req) = rx.recv() {
        match req {
            ManagedAuthReq::Refresh { auth_provider } => {
                let result = rt.block_on(crate::services::AuthService::get_status(&auth_provider));
                let _ = tx.send(ManagedAuthMsg::Status {
                    auth_provider,
                    result,
                });
            }
            ManagedAuthReq::StartLogin { auth_provider } => {
                let result = rt.block_on(crate::services::AuthService::start_login(&auth_provider));
                let _ = tx.send(ManagedAuthMsg::LoginStarted {
                    auth_provider,
                    result,
                });
            }
            ManagedAuthReq::PollLogin {
                auth_provider,
                device_code,
            } => {
                let result = rt.block_on(crate::services::AuthService::poll_for_account(
                    &auth_provider,
                    &device_code,
                ));
                let _ = tx.send(ManagedAuthMsg::LoginPolled {
                    auth_provider,
                    device_code,
                    result,
                });
            }
            ManagedAuthReq::SetDefault {
                auth_provider,
                account_id,
            } => {
                let result = rt.block_on(async {
                    crate::services::AuthService::set_default_account(&auth_provider, &account_id)
                        .await?;
                    crate::services::AuthService::get_status(&auth_provider).await
                });
                let _ = tx.send(ManagedAuthMsg::DefaultSet {
                    auth_provider,
                    account_id,
                    result,
                });
            }
            ManagedAuthReq::Remove {
                auth_provider,
                account_id,
            } => {
                let result = rt.block_on(async {
                    crate::services::AuthService::remove_account(&auth_provider, &account_id)
                        .await?;
                    crate::services::AuthService::get_status(&auth_provider).await
                });
                let _ = tx.send(ManagedAuthMsg::Removed {
                    auth_provider,
                    account_id,
                    result,
                });
            }
        }
    }
}

pub(crate) fn start_local_env_system() -> Result<LocalEnvSystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<LocalEnvMsg>();
    let (req_tx, req_rx) = mpsc::channel::<LocalEnvReq>();

    let handle = std::thread::Builder::new()
        .name("cc-switch-local-env".to_string())
        .spawn(move || local_env_worker_loop(req_rx, result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn local env worker thread".to_string(),
            source: e,
        })?;

    Ok(LocalEnvSystem {
        req_tx,
        result_rx,
        _handle: handle,
    })
}

pub(crate) fn start_session_system() -> Result<SessionSystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<SessionMsg>();
    let (req_tx, req_rx) = mpsc::channel::<SessionReq>();

    let handle = std::thread::Builder::new()
        .name("cc-switch-sessions".to_string())
        .spawn(move || session_worker_loop(req_rx, result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn sessions worker thread".to_string(),
            source: e,
        })?;

    Ok(SessionSystem {
        req_tx,
        result_rx,
        _handle: handle,
    })
}

fn session_worker_loop(rx: mpsc::Receiver<SessionReq>, tx: mpsc::Sender<SessionMsg>) {
    while let Ok(mut req) = rx.recv() {
        for next in rx.try_iter() {
            match (&req, &next) {
                (SessionReq::Refresh { .. }, SessionReq::Refresh { .. }) => req = next,
                (SessionReq::LoadMessages { .. }, SessionReq::LoadMessages { .. }) => req = next,
                // Drop a superseded search: only the latest query matters, and
                // stale results are ignored by request id anyway, so skip the
                // wasted full scan.
                (SessionReq::Search { .. }, SessionReq::Search { .. }) => req = next,
                _ => {
                    let _ = handle_session_req(req, &tx);
                    req = next;
                }
            }
        }

        let _ = handle_session_req(req, &tx);
    }
}

fn handle_session_req(req: SessionReq, tx: &mpsc::Sender<SessionMsg>) -> Result<(), ()> {
    match req {
        SessionReq::Refresh {
            request_id,
            provider_id,
        } => {
            let result = std::panic::catch_unwind(|| {
                if provider_id == "all" {
                    crate::session_manager::scan_sessions()
                } else {
                    crate::session_manager::scan_sessions_for_provider(&provider_id)
                }
            })
            .map_err(|_| "session scan panicked".to_string());
            tx.send(SessionMsg::ScanFinished { request_id, result })
                .map_err(|_| ())
        }
        SessionReq::LoadMessages {
            request_id,
            key,
            provider_id,
            source_path,
        } => {
            let result = crate::session_manager::load_messages(&provider_id, &source_path);
            tx.send(SessionMsg::MessagesLoaded {
                request_id,
                key,
                result,
            })
            .map_err(|_| ())
        }
        SessionReq::Delete {
            request_id,
            key,
            provider_id,
            session_id,
            source_path,
        } => {
            let result =
                crate::session_manager::delete_session(&provider_id, &session_id, &source_path)
                    .and_then(|deleted| {
                        if deleted {
                            Ok(())
                        } else {
                            Err("Session was not deleted".to_string())
                        }
                    });
            tx.send(SessionMsg::DeleteFinished {
                request_id,
                key,
                result,
            })
            .map_err(|_| ())
        }
        SessionReq::Search {
            request_id,
            query,
            sessions,
        } => {
            let result = std::panic::catch_unwind(|| {
                crate::session_manager::search_sessions_in(&sessions, &query)
            })
            .map_err(|_| "session search panicked".to_string());
            tx.send(SessionMsg::SearchFinished { request_id, result })
                .map_err(|_| ())
        }
    }
}

#[cfg(test)]
pub(crate) fn drain_session_reqs_for_test(
    mut req: SessionReq,
    rx: &mpsc::Receiver<SessionReq>,
) -> Vec<SessionReq> {
    let mut drained = Vec::new();
    for next in rx.try_iter() {
        match (&req, &next) {
            (SessionReq::Refresh { .. }, SessionReq::Refresh { .. })
            | (SessionReq::LoadMessages { .. }, SessionReq::LoadMessages { .. }) => {
                req = next;
            }
            _ => {
                drained.push(req);
                req = next;
            }
        }
    }
    drained.push(req);
    drained
}

fn local_env_worker_loop(rx: mpsc::Receiver<LocalEnvReq>, tx: mpsc::Sender<LocalEnvMsg>) {
    while let Ok(mut req) = rx.recv() {
        for next in rx.try_iter() {
            req = next;
        }

        match req {
            LocalEnvReq::Refresh => {
                let result = crate::services::local_env_check::check_local_environment();
                let _ = tx.send(LocalEnvMsg::Finished { result });
            }
        }
    }
}

pub(crate) fn start_quota_system() -> Result<QuotaSystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<QuotaMsg>();
    let (req_tx, req_rx) = mpsc::channel::<QuotaReq>();

    let handle = std::thread::Builder::new()
        .name("cc-switch-quota".to_string())
        .spawn(move || quota_worker_loop(req_rx, result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn quota worker thread".to_string(),
            source: e,
        })?;

    Ok(QuotaSystem {
        req_tx,
        result_rx,
        _handle: handle,
    })
}

fn quota_worker_loop(rx: mpsc::Receiver<QuotaReq>, tx: mpsc::Sender<QuotaMsg>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let err = e.to_string();
            while let Ok(req) = rx.recv() {
                let QuotaReq::Refresh { target } = req;
                let _ = tx.send(QuotaMsg::Finished {
                    target,
                    result: Err(err.clone()),
                });
            }
            return;
        }
    };

    while let Ok(req) = rx.recv() {
        let QuotaReq::Refresh { target } = req;
        let result = rt.block_on(crate::cli::provider_quota::query_quota(&target));

        let _ = tx.send(QuotaMsg::Finished { target, result });
    }
}

pub(crate) fn start_usage_pricing_system() -> Result<UsagePricingSystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<UsagePricingMsg>();
    let (req_tx, req_rx) = mpsc::channel::<UsagePricingReq>();

    let handle = std::thread::Builder::new()
        .name("cc-switch-usage-pricing".to_string())
        .spawn(move || usage_pricing_worker_loop(req_rx, result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn usage/pricing worker thread".to_string(),
            source: e,
        })?;

    Ok(UsagePricingSystem {
        req_tx,
        result_rx,
        _handle: handle,
    })
}

pub(crate) fn start_session_usage_sync_system() -> Result<SessionUsageSyncSystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<SessionUsageSyncMsg>();
    let (req_tx, req_rx) = mpsc::channel::<SessionUsageSyncReq>();

    let handle = std::thread::Builder::new()
        .name("cc-switch-session-usage".to_string())
        .spawn(move || session_usage_sync_worker_loop(req_rx, result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn session usage sync worker thread".to_string(),
            source: e,
        })?;

    Ok(SessionUsageSyncSystem {
        req_tx,
        result_rx,
        _handle: handle,
    })
}

pub(crate) fn start_app_data_system() -> Result<AppDataSystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<AppDataMsg>();
    let (req_tx, req_rx) = mpsc::channel::<AppDataReq>();

    let handle = std::thread::Builder::new()
        .name("cc-switch-app-data".to_string())
        .spawn(move || app_data_worker_loop(req_rx, result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn app data worker thread".to_string(),
            source: e,
        })?;

    Ok(AppDataSystem {
        req_tx,
        result_rx,
        _handle: handle,
    })
}

fn session_usage_sync_worker_loop(
    rx: mpsc::Receiver<SessionUsageSyncReq>,
    tx: mpsc::Sender<SessionUsageSyncMsg>,
) {
    while let Ok(mut req) = rx.recv() {
        for next in rx.try_iter() {
            req = next;
        }

        let SessionUsageSyncReq::Run { request_id } = req;
        let result = match crate::Database::init() {
            Ok(db) => {
                crate::services::session_usage::run_session_usage_sync_cycle(&db, "tui-background")
                    .and_then(|result| {
                        if result.errors.is_empty() {
                            Ok(())
                        } else {
                            Err(AppError::Message(format!(
                                "{} session usage sync error(s); first: {}",
                                result.errors.len(),
                                result.errors[0]
                            )))
                        }
                    })
                    .map_err(|error| error.to_string())
            }
            Err(error) => Err(error.to_string()),
        };

        let _ = tx.send(SessionUsageSyncMsg::Finished { request_id, result });
    }
}

fn app_data_worker_loop(rx: mpsc::Receiver<AppDataReq>, tx: mpsc::Sender<AppDataMsg>) {
    let mut state_cache: Option<(u64, crate::store::AppState)> = None;
    let mut deferred = VecDeque::new();

    while let Some(req) = deferred.pop_front().or_else(|| rx.recv().ok()) {
        match req {
            AppDataReq::DropState { ack } => {
                state_cache = None;
                let _ = ack.send(());
            }
            req @ (AppDataReq::InitialLoad { .. }
            | AppDataReq::Load { .. }
            | AppDataReq::FullLoad { .. }) => {
                let mut backlog = VecDeque::from([req]);
                drain_latest_by_key(&mut backlog, &mut deferred, &rx, app_data_req_key);

                while let Some(req) = backlog.pop_front() {
                    handle_app_data_req(&mut state_cache, req, &tx);
                    drain_latest_by_key(&mut backlog, &mut deferred, &rx, app_data_req_key);
                }
            }
        }
    }
}

fn drain_latest_by_key<T, K, F>(
    backlog: &mut VecDeque<T>,
    deferred: &mut VecDeque<T>,
    rx: &mpsc::Receiver<T>,
    key: F,
) where
    K: Eq + Hash,
    F: Fn(&T) -> Option<K>,
{
    let mut latest_by_key = HashMap::<K, T>::new();
    while let Some(req) = backlog.pop_front() {
        if let Some(key) = key(&req) {
            latest_by_key.insert(key, req);
        } else {
            deferred.push_back(req);
        }
    }
    for req in rx.try_iter() {
        if deferred.is_empty() {
            if let Some(key) = key(&req) {
                latest_by_key.insert(key, req);
                continue;
            }
        }
        deferred.push_back(req);
    }
    backlog.extend(latest_by_key.into_values());
}

fn app_data_req_key(req: &AppDataReq) -> Option<(AppType, AppDataLoadKind)> {
    match req {
        AppDataReq::InitialLoad { app_type, .. } => {
            Some((app_type.clone(), AppDataLoadKind::Initial))
        }
        AppDataReq::Load { app_type, .. } => Some((app_type.clone(), AppDataLoadKind::Snapshot)),
        AppDataReq::FullLoad { app_type, .. } => Some((app_type.clone(), AppDataLoadKind::Full)),
        AppDataReq::DropState { .. } => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum UsagePricingReqRangeKey {
    Fixed(UsageRangePreset),
    Custom,
}

fn usage_pricing_req_key(req: &UsagePricingReq) -> Option<(AppType, UsagePricingReqRangeKey)> {
    match req {
        UsagePricingReq::Load {
            app_type, range, ..
        } => {
            let range_key = match range {
                UsageRangePreset::Custom(_) => UsagePricingReqRangeKey::Custom,
                _ => UsagePricingReqRangeKey::Fixed(*range),
            };
            Some((app_type.clone(), range_key))
        }
        UsagePricingReq::DropState { .. } => None,
    }
}

fn usage_pricing_req_is_custom(req: &UsagePricingReq) -> bool {
    matches!(
        req,
        UsagePricingReq::Load {
            range: UsageRangePreset::Custom(_),
            ..
        }
    )
}

#[derive(Default)]
struct UsagePricingCustomState {
    running: bool,
    running_request_id: Option<u64>,
    running_interrupt: Option<rusqlite::InterruptHandle>,
    running_cancel: Option<Arc<AtomicBool>>,
    cancel_requested: bool,
    pending: Vec<UsagePricingReq>,
    drop_acks: Vec<mpsc::Sender<()>>,
}

#[derive(Default)]
struct UsagePricingCustomCompletion {
    next: Option<UsagePricingReq>,
    drop_acks: Vec<mpsc::Sender<()>>,
}

impl UsagePricingCustomState {
    fn enqueue(&mut self, req: UsagePricingReq) -> Option<UsagePricingReq> {
        if !self.running {
            self.running = true;
            self.running_request_id = usage_pricing_req_request_id(&req);
            self.cancel_requested = false;
            return Some(req);
        }
        if self.req_is_newer_than_running(&req) {
            self.cancel_running();
        }

        if let UsagePricingReq::Load { app_type, .. } = &req {
            self.pending
                .retain(|pending| !usage_pricing_req_matches_app(pending, app_type));
        }
        self.pending.push(req);
        None
    }

    fn set_running_interrupt_handle(&mut self, handle: rusqlite::InterruptHandle) {
        if !self.running {
            return;
        }
        if self.cancel_requested || self.has_pending_newer_than_running() {
            handle.interrupt();
        }
        self.running_interrupt = Some(handle);
    }

    fn set_running_cancel_token(&mut self, token: Arc<AtomicBool>) {
        if !self.running {
            token.store(true, Ordering::Relaxed);
            return;
        }
        if self.cancel_requested || self.has_pending_newer_than_running() {
            token.store(true, Ordering::Relaxed);
        }
        self.running_cancel = Some(token);
    }

    fn complete(&mut self) -> UsagePricingCustomCompletion {
        self.running_interrupt = None;
        self.running_cancel = None;
        let drop_acks = self.drop_acks.drain(..).collect();
        if let Some(next) = self.pending.pop() {
            self.running_request_id = usage_pricing_req_request_id(&next);
            self.cancel_requested = false;
            return UsagePricingCustomCompletion {
                next: Some(next),
                drop_acks,
            };
        }
        self.running = false;
        self.running_request_id = None;
        self.cancel_requested = false;
        UsagePricingCustomCompletion {
            next: None,
            drop_acks,
        }
    }

    fn is_idle(&self) -> bool {
        !self.running && self.pending.is_empty()
    }

    fn clear_pending_and_ack_when_idle(&mut self, ack: mpsc::Sender<()>) {
        self.pending.clear();
        if self.is_idle() {
            let _ = ack.send(());
        } else {
            self.cancel_running();
            self.drop_acks.push(ack);
        }
    }

    fn cancel_running(&mut self) {
        self.cancel_requested = true;
        if let Some(token) = &self.running_cancel {
            token.store(true, Ordering::Relaxed);
        }
        if let Some(handle) = &self.running_interrupt {
            handle.interrupt();
        }
    }

    fn req_is_newer_than_running(&self, req: &UsagePricingReq) -> bool {
        match (usage_pricing_req_request_id(req), self.running_request_id) {
            (Some(req_id), Some(running_id)) => req_id > running_id,
            (Some(_), None) => true,
            _ => false,
        }
    }

    fn has_pending_newer_than_running(&self) -> bool {
        let Some(running_id) = self.running_request_id else {
            return !self.pending.is_empty();
        };
        self.pending
            .iter()
            .filter_map(usage_pricing_req_request_id)
            .any(|request_id| request_id > running_id)
    }
}

fn usage_pricing_req_request_id(req: &UsagePricingReq) -> Option<u64> {
    match req {
        UsagePricingReq::Load { request_id, .. } => Some(*request_id),
        UsagePricingReq::DropState { .. } => None,
    }
}

#[cfg(test)]
fn usage_pricing_custom_state_snapshot(
    state: &UsagePricingCustomState,
) -> (bool, Option<u64>, bool, bool, Vec<u64>) {
    (
        state.running,
        state.running_request_id,
        state.cancel_requested,
        state.is_idle(),
        state
            .pending
            .iter()
            .filter_map(usage_pricing_req_request_id)
            .collect(),
    )
}

fn spawn_usage_pricing_custom_req(
    req: UsagePricingReq,
    state: Arc<Mutex<UsagePricingCustomState>>,
    tx: mpsc::Sender<UsagePricingMsg>,
) {
    let cancel_token = Arc::new(AtomicBool::new(false));
    let registered_cancel_token = {
        match state.lock() {
            Ok(mut state_guard) => {
                state_guard.set_running_cancel_token(Arc::clone(&cancel_token));
                true
            }
            Err(_) => false,
        }
    };
    if !registered_cancel_token {
        send_usage_pricing_req_error(
            &req,
            &tx,
            "custom usage worker state is unavailable".to_string(),
        );
        finish_usage_pricing_custom_req(state, tx);
        return;
    }

    let fallback_req = req.clone();
    let fallback_state = Arc::clone(&state);
    let fallback_tx = tx.clone();
    match std::thread::Builder::new()
        .name("cc-switch-usage-custom".to_string())
        .spawn(move || {
            let interrupt_state = Arc::clone(&state);
            let panic_req = req.clone();
            let panic_tx = tx.clone();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                handle_usage_pricing_uncached_req_with_cancel(
                    req,
                    &tx,
                    cancel_token,
                    move |handle| {
                        if let Ok(mut state) = interrupt_state.lock() {
                            state.set_running_interrupt_handle(handle);
                        }
                    },
                );
            }));
            if result.is_err() {
                send_usage_pricing_req_error(
                    &panic_req,
                    &panic_tx,
                    "custom usage worker panicked".to_string(),
                );
            }
            finish_usage_pricing_custom_req(state, tx);
        }) {
        Ok(_handle) => {}
        Err(err) => {
            send_usage_pricing_req_error(
                &fallback_req,
                &fallback_tx,
                format!("failed to spawn custom usage worker: {err}"),
            );
            finish_usage_pricing_custom_req(fallback_state, fallback_tx);
        }
    }
}

fn finish_usage_pricing_custom_req(
    state: Arc<Mutex<UsagePricingCustomState>>,
    tx: mpsc::Sender<UsagePricingMsg>,
) {
    let completion = match state.lock() {
        Ok(mut state) => state.complete(),
        Err(_) => UsagePricingCustomCompletion::default(),
    };
    for ack in completion.drop_acks {
        let _ = ack.send(());
    }
    if let Some(req) = completion.next {
        spawn_usage_pricing_custom_req(req, state, tx);
    }
}

struct UsagePricingCustomRunner {
    state: Arc<Mutex<UsagePricingCustomState>>,
    tx: mpsc::Sender<UsagePricingMsg>,
}

impl UsagePricingCustomRunner {
    fn new(tx: mpsc::Sender<UsagePricingMsg>) -> Self {
        Self {
            state: Arc::new(Mutex::new(UsagePricingCustomState::default())),
            tx,
        }
    }

    fn dispatch(&self, req: UsagePricingReq) {
        let launch = match self.state.lock() {
            Ok(mut state) => state.enqueue(req),
            Err(_) => {
                send_usage_pricing_req_error(
                    &req,
                    &self.tx,
                    "custom usage worker state is unavailable".to_string(),
                );
                return;
            }
        };
        if let Some(req) = launch {
            spawn_usage_pricing_custom_req(req, Arc::clone(&self.state), self.tx.clone());
        }
    }

    fn drop_state(&self, ack: mpsc::Sender<()>) {
        match self.state.lock() {
            Ok(mut state) => state.clear_pending_and_ack_when_idle(ack),
            Err(_) => {
                let _ = ack.send(());
            }
        }
    }
}

fn usage_pricing_req_matches_app(req: &UsagePricingReq, app_type: &AppType) -> bool {
    matches!(
        req,
        UsagePricingReq::Load {
            app_type: req_app_type,
            ..
        } if req_app_type == app_type
    )
}

fn state_for_epoch(
    state_cache: &mut Option<(u64, crate::store::AppState)>,
    epoch: u64,
) -> Result<&crate::store::AppState, AppError> {
    let needs_reload = state_cache
        .as_ref()
        .is_none_or(|(cached_epoch, _)| *cached_epoch != epoch);
    if needs_reload {
        *state_cache = Some((epoch, load_snapshot_state()?));
    }
    Ok(&state_cache.as_ref().expect("state cache initialized").1)
}

fn handle_app_data_req(
    state_cache: &mut Option<(u64, crate::store::AppState)>,
    req: AppDataReq,
    tx: &mpsc::Sender<AppDataMsg>,
) {
    let (kind, request_id, generation, app_state_epoch, app_type, result) = match req {
        AppDataReq::InitialLoad {
            request_id,
            generation,
            app_state_epoch,
            app_type,
            extras,
        } => {
            // Build the active app first and send it immediately so the UI paints
            // as soon as possible; the config snapshot is reloaded once here.
            let result = state_for_epoch(state_cache, app_state_epoch)
                .and_then(|state| {
                    state
                        .reload_config_snapshot_from_db()
                        .and_then(|()| UiData::load_fast_snapshot_from_state(state, &app_type))
                })
                .map_err(|err| err.to_string());
            let _ = tx.send(AppDataMsg::Loaded {
                kind: AppDataLoadKind::Initial,
                request_id,
                generation,
                app_state_epoch,
                app_type,
                result,
            });

            // Warm the remaining visible apps from the SAME cached state (no extra
            // DB open, SnapshotOnly), one Initial message each.
            for (extra_app, extra_request_id) in extras {
                let extra_result = state_for_epoch(state_cache, app_state_epoch)
                    .and_then(|state| UiData::load_fast_snapshot_from_state(state, &extra_app))
                    .map_err(|err| err.to_string());
                let _ = tx.send(AppDataMsg::Loaded {
                    kind: AppDataLoadKind::Initial,
                    request_id: extra_request_id,
                    generation,
                    app_state_epoch,
                    app_type: extra_app,
                    result: extra_result,
                });
            }
            return;
        }
        AppDataReq::Load {
            request_id,
            generation,
            app_state_epoch,
            app_type,
        } => {
            let result = state_for_epoch(state_cache, app_state_epoch)
                .and_then(|state| {
                    state
                        .reload_config_snapshot_from_db()
                        .and_then(|()| UiData::load_fast_snapshot_from_state(state, &app_type))
                })
                .map_err(|err| err.to_string());
            (
                AppDataLoadKind::Snapshot,
                request_id,
                generation,
                app_state_epoch,
                app_type,
                result,
            )
        }
        AppDataReq::FullLoad {
            request_id,
            generation,
            app_state_epoch,
            app_type,
        } => {
            // Skip the usage/pricing aggregation here; it is deferred and loaded
            // lazily by the usage-pricing worker when the Usage view is opened.
            let result =
                UiData::load_without_usage_pricing(&app_type).map_err(|err| err.to_string());
            (
                AppDataLoadKind::Full,
                request_id,
                generation,
                app_state_epoch,
                app_type,
                result,
            )
        }
        AppDataReq::DropState { .. } => return,
    };

    let _ = tx.send(AppDataMsg::Loaded {
        kind,
        request_id,
        generation,
        app_state_epoch,
        app_type,
        result,
    });
}

fn usage_pricing_worker_loop(
    rx: mpsc::Receiver<UsagePricingReq>,
    tx: mpsc::Sender<UsagePricingMsg>,
) {
    let mut state_cache: Option<(u64, crate::store::AppState)> = None;
    let mut deferred = VecDeque::new();
    let custom_runner = UsagePricingCustomRunner::new(tx.clone());

    while let Some(req) = deferred.pop_front().or_else(|| rx.recv().ok()) {
        match req {
            UsagePricingReq::DropState { ack } => {
                state_cache = None;
                custom_runner.drop_state(ack);
            }
            req @ UsagePricingReq::Load { .. } => {
                let mut backlog = VecDeque::from([req]);
                drain_latest_by_key(&mut backlog, &mut deferred, &rx, usage_pricing_req_key);

                while let Some(req) = backlog.pop_front() {
                    if usage_pricing_req_is_custom(&req) {
                        custom_runner.dispatch(req);
                    } else {
                        handle_usage_pricing_req(&mut state_cache, req, &tx);
                    }
                    drain_latest_by_key(&mut backlog, &mut deferred, &rx, usage_pricing_req_key);
                }
            }
        }
    }
}

fn handle_usage_pricing_req(
    state_cache: &mut Option<(u64, crate::store::AppState)>,
    req: UsagePricingReq,
    tx: &mpsc::Sender<UsagePricingMsg>,
) {
    let UsagePricingReq::Load {
        request_id,
        generation,
        app_state_epoch,
        app_type,
        range,
    } = req
    else {
        return;
    };
    let result = state_for_epoch(state_cache, app_state_epoch)
        .and_then(|state| load_usage_pricing_data_from_state_for_range(state, &app_type, range))
        .map_err(|err| err.to_string());

    let _ = tx.send(UsagePricingMsg::Loaded {
        request_id,
        generation,
        app_state_epoch,
        app_type,
        range,
        result,
    });
}

fn handle_usage_pricing_uncached_req_with_cancel<F>(
    req: UsagePricingReq,
    tx: &mpsc::Sender<UsagePricingMsg>,
    cancel_token: Arc<AtomicBool>,
    on_interrupt_handle: F,
) where
    F: FnOnce(rusqlite::InterruptHandle),
{
    let UsagePricingReq::Load {
        request_id,
        generation,
        app_state_epoch,
        app_type,
        range,
    } = req
    else {
        return;
    };
    let result = load_snapshot_state()
        .and_then(|state| {
            let handle = {
                let conn = state.db.conn.lock().map_err(AppError::from)?;
                let cancel_for_handler = Arc::clone(&cancel_token);
                conn.progress_handler(
                    1_000,
                    Some(move || cancel_for_handler.load(Ordering::Relaxed)),
                );
                conn.get_interrupt_handle()
            };
            on_interrupt_handle(handle);
            load_usage_pricing_data_from_state_for_range(&state, &app_type, range)
        })
        .map_err(|err| err.to_string());

    let _ = tx.send(UsagePricingMsg::Loaded {
        request_id,
        generation,
        app_state_epoch,
        app_type,
        range,
        result,
    });
}

fn send_usage_pricing_req_error(
    req: &UsagePricingReq,
    tx: &mpsc::Sender<UsagePricingMsg>,
    err: String,
) {
    let &UsagePricingReq::Load {
        request_id,
        generation,
        app_state_epoch,
        ref app_type,
        range,
    } = req
    else {
        return;
    };

    let _ = tx.send(UsagePricingMsg::Loaded {
        request_id,
        generation,
        app_state_epoch,
        app_type: app_type.clone(),
        range,
        result: Err(err),
    });
}

pub(crate) fn start_skills_system() -> Result<SkillsSystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<SkillsMsg>();
    let (req_tx, req_rx) = mpsc::channel::<SkillsReq>();

    let handle = std::thread::Builder::new()
        .name("cc-switch-skills".to_string())
        .spawn(move || skills_worker_loop(req_rx, result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn skills worker thread".to_string(),
            source: e,
        })?;

    Ok(SkillsSystem {
        req_tx,
        result_rx,
        _handle: handle,
    })
}

fn skills_worker_loop(rx: mpsc::Receiver<SkillsReq>, tx: mpsc::Sender<SkillsMsg>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let err = e.to_string();
            while let Ok(req) = rx.recv() {
                match req {
                    SkillsReq::Discover {
                        request_id,
                        query,
                        source,
                        ..
                    } => {
                        let _ = tx.send(SkillsMsg::DiscoverFinished {
                            request_id,
                            query,
                            source,
                            result: Err(err.clone()),
                        });
                    }
                    SkillsReq::Install { spec, .. } => {
                        let _ = tx.send(SkillsMsg::InstallFinished {
                            spec,
                            result: Err(err.clone()),
                        });
                    }
                }
            }
            return;
        }
    };

    let service = match SkillService::new() {
        Ok(service) => service,
        Err(e) => {
            let err = e.to_string();
            while let Ok(req) = rx.recv() {
                match req {
                    SkillsReq::Discover {
                        request_id,
                        query,
                        source,
                        ..
                    } => {
                        let _ = tx.send(SkillsMsg::DiscoverFinished {
                            request_id,
                            query,
                            source,
                            result: Err(err.clone()),
                        });
                    }
                    SkillsReq::Install { spec, .. } => {
                        let _ = tx.send(SkillsMsg::InstallFinished {
                            spec,
                            result: Err(err.clone()),
                        });
                    }
                }
            }
            return;
        }
    };

    while let Ok(req) = rx.recv() {
        match req {
            SkillsReq::Discover {
                request_id,
                query,
                source,
                force,
            } => {
                let query_trimmed = query.trim().to_lowercase();
                let installed_skill_keys = crate::services::SkillService::load_index()
                    .map(|index| {
                        index
                            .skills
                            .values()
                            .map(|skill| {
                                (
                                    skill.directory.to_lowercase(),
                                    skill
                                        .repo_owner
                                        .as_deref()
                                        .unwrap_or_default()
                                        .to_lowercase(),
                                    skill
                                        .repo_name
                                        .as_deref()
                                        .unwrap_or_default()
                                        .to_lowercase(),
                                )
                            })
                            .collect::<std::collections::HashSet<_>>()
                    })
                    .unwrap_or_default();
                let result = match source {
                    crate::cli::tui::app::SkillsDiscoverSource::Repos => rt
                        .block_on(async { service.list_skills_cached(force).await })
                        .map_err(|e| e.to_string()),
                    crate::cli::tui::app::SkillsDiscoverSource::Marketplace => rt
                        .block_on(async { service.search_skills_sh(&query, 50, 0).await })
                        .map(|result| {
                            result
                                .skills
                                .into_iter()
                                .map(|skill| crate::services::skill::Skill {
                                    installed: installed_skill_keys.contains(&(
                                        skill.directory.to_lowercase(),
                                        skill.repo_owner.to_lowercase(),
                                        skill.repo_name.to_lowercase(),
                                    )),
                                    key: skill.key,
                                    name: skill.name,
                                    description: format!("{} installs", skill.installs),
                                    directory: skill.directory,
                                    readme_url: skill.readme_url,
                                    repo_owner: Some(skill.repo_owner),
                                    repo_name: Some(skill.repo_name),
                                    repo_branch: Some(skill.repo_branch),
                                })
                                .collect::<Vec<_>>()
                        })
                        .map_err(|e| e.to_string()),
                }
                .map(|mut skills| {
                    if !query_trimmed.is_empty() {
                        skills.retain(|s| {
                            s.name.to_lowercase().contains(&query_trimmed)
                                || s.directory.to_lowercase().contains(&query_trimmed)
                                || s.description.to_lowercase().contains(&query_trimmed)
                                || s.key.to_lowercase().contains(&query_trimmed)
                        });
                    }
                    skills
                });

                let _ = tx.send(SkillsMsg::DiscoverFinished {
                    request_id,
                    query,
                    source,
                    result,
                });
            }
            SkillsReq::Install { spec, app } => {
                let spec_clone = spec.clone();
                let app_clone = app.clone();
                let result = rt
                    .block_on(async { service.install(&spec_clone, &app_clone).await })
                    .map_err(|e| e.to_string());
                let _ = tx.send(SkillsMsg::InstallFinished { spec, result });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delete_req(request_id: u64, key: &str) -> SessionReq {
        SessionReq::Delete {
            request_id,
            key: key.to_string(),
            provider_id: "claude".to_string(),
            session_id: key.to_string(),
            source_path: format!("/tmp/{key}.jsonl"),
        }
    }

    #[test]
    fn session_req_drain_never_coalesces_deletes() {
        let (tx, rx) = mpsc::channel();
        tx.send(delete_req(2, "beta")).expect("queue beta delete");
        tx.send(delete_req(3, "gamma")).expect("queue gamma delete");
        drop(tx);

        let drained = drain_session_reqs_for_test(delete_req(1, "alpha"), &rx);

        let keys = drained
            .into_iter()
            .map(|req| match req {
                SessionReq::Delete { key, .. } => key,
                _ => panic!("expected delete request"),
            })
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn session_req_drain_keeps_only_latest_refresh() {
        let (tx, rx) = mpsc::channel();
        tx.send(SessionReq::Refresh {
            request_id: 2,
            provider_id: "claude".to_string(),
        })
        .expect("queue refresh");
        drop(tx);

        let drained = drain_session_reqs_for_test(
            SessionReq::Refresh {
                request_id: 1,
                provider_id: "claude".to_string(),
            },
            &rx,
        );

        assert_eq!(drained.len(), 1);
        assert!(matches!(
            drained[0],
            SessionReq::Refresh { request_id: 2, .. }
        ));
    }

    #[test]
    fn usage_pricing_drain_keeps_latest_request_per_app() {
        let (tx, rx) = mpsc::channel();
        tx.send(UsagePricingReq::Load {
            request_id: 2,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: UsageRangePreset::SevenDays,
        })
        .expect("queue newer claude request");
        tx.send(UsagePricingReq::Load {
            request_id: 3,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Codex,
            range: UsageRangePreset::SevenDays,
        })
        .expect("queue codex request");
        drop(tx);

        let mut backlog = std::collections::VecDeque::from([UsagePricingReq::Load {
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: UsageRangePreset::SevenDays,
        }]);

        let mut deferred = std::collections::VecDeque::new();
        drain_latest_by_key(&mut backlog, &mut deferred, &rx, usage_pricing_req_key);

        let mut drained = backlog
            .into_iter()
            .map(|req| match req {
                UsagePricingReq::Load {
                    request_id,
                    app_type,
                    ..
                } => (app_type, request_id),
                UsagePricingReq::DropState { .. } => panic!("unexpected DropState in backlog"),
            })
            .collect::<Vec<_>>();
        drained.sort_by_key(|(_, request_id)| *request_id);

        assert_eq!(drained, vec![(AppType::Claude, 2), (AppType::Codex, 3)]);
        assert!(deferred.is_empty());
    }

    #[test]
    fn drain_latest_by_app_preserves_drop_state_as_barrier() {
        let (tx, rx) = mpsc::channel();
        tx.send(UsagePricingReq::Load {
            request_id: 2,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: UsageRangePreset::SevenDays,
        })
        .expect("queue newer claude request before barrier");
        let (ack_tx, _ack_rx) = mpsc::channel();
        tx.send(UsagePricingReq::DropState { ack: ack_tx })
            .expect("queue drop-state barrier");
        tx.send(UsagePricingReq::Load {
            request_id: 3,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: UsageRangePreset::SevenDays,
        })
        .expect("queue claude request after barrier");
        drop(tx);

        let mut backlog = std::collections::VecDeque::from([UsagePricingReq::Load {
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: UsageRangePreset::SevenDays,
        }]);
        let mut deferred = std::collections::VecDeque::new();

        drain_latest_by_key(&mut backlog, &mut deferred, &rx, usage_pricing_req_key);

        let next = backlog.pop_front().expect("latest request before barrier");
        assert!(matches!(
            next,
            UsagePricingReq::Load {
                request_id: 2,
                app_type: AppType::Claude,
                ..
            }
        ));
        assert!(backlog.is_empty());

        assert!(matches!(
            deferred.pop_front(),
            Some(UsagePricingReq::DropState { .. })
        ));
        assert!(matches!(
            deferred.pop_front(),
            Some(UsagePricingReq::Load {
                request_id: 3,
                app_type: AppType::Claude,
                ..
            })
        ));
        assert!(deferred.is_empty());
    }

    #[test]
    fn usage_pricing_drain_keeps_distinct_ranges_for_same_app() {
        let custom = UsageRangePreset::Custom(crate::cli::tui::data::UsageCustomRange {
            start: 1_700_000_000,
            end: 1_700_086_399,
        });
        let (tx, rx) = mpsc::channel();
        tx.send(UsagePricingReq::Load {
            request_id: 2,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: custom,
        })
        .expect("queue custom request");
        drop(tx);

        let mut backlog = std::collections::VecDeque::from([UsagePricingReq::Load {
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: UsageRangePreset::SevenDays,
        }]);
        let mut deferred = std::collections::VecDeque::new();

        drain_latest_by_key(&mut backlog, &mut deferred, &rx, usage_pricing_req_key);

        let mut drained = backlog
            .into_iter()
            .map(|req| match req {
                UsagePricingReq::Load {
                    request_id, range, ..
                } => (range, request_id),
                UsagePricingReq::DropState { .. } => panic!("unexpected DropState in backlog"),
            })
            .collect::<Vec<_>>();
        drained.sort_by_key(|(_, request_id)| *request_id);

        assert_eq!(drained, vec![(UsageRangePreset::SevenDays, 1), (custom, 2)]);
        assert!(deferred.is_empty());
    }

    #[test]
    fn usage_pricing_drain_keeps_only_latest_custom_range_per_app() {
        let older_custom = UsageRangePreset::Custom(crate::cli::tui::data::UsageCustomRange {
            start: 1_700_000_000,
            end: 1_700_086_399,
        });
        let newer_custom = UsageRangePreset::Custom(crate::cli::tui::data::UsageCustomRange {
            start: 1_700_086_400,
            end: 1_700_172_799,
        });
        let (tx, rx) = mpsc::channel();
        tx.send(UsagePricingReq::Load {
            request_id: 2,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: newer_custom,
        })
        .expect("queue newer custom request");
        drop(tx);

        let mut backlog = std::collections::VecDeque::from([UsagePricingReq::Load {
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: older_custom,
        }]);
        let mut deferred = std::collections::VecDeque::new();

        drain_latest_by_key(&mut backlog, &mut deferred, &rx, usage_pricing_req_key);

        let drained = backlog
            .into_iter()
            .map(|req| match req {
                UsagePricingReq::Load {
                    request_id, range, ..
                } => (range, request_id),
                UsagePricingReq::DropState { .. } => panic!("unexpected DropState in backlog"),
            })
            .collect::<Vec<_>>();

        assert_eq!(drained, vec![(newer_custom, 2)]);
        assert!(deferred.is_empty());
    }

    #[test]
    fn app_data_drain_keeps_initial_and_snapshot_loads_distinct() {
        let (tx, rx) = mpsc::channel();
        tx.send(AppDataReq::Load {
            request_id: 2,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
        })
        .expect("queue snapshot request");
        drop(tx);

        let mut backlog = std::collections::VecDeque::from([AppDataReq::InitialLoad {
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            extras: Vec::new(),
        }]);
        let mut deferred = std::collections::VecDeque::new();

        drain_latest_by_key(&mut backlog, &mut deferred, &rx, app_data_req_key);

        let mut drained = backlog
            .into_iter()
            .map(|req| match req {
                AppDataReq::InitialLoad { request_id, .. } => {
                    (AppDataLoadKind::Initial, request_id)
                }
                AppDataReq::Load { request_id, .. } => (AppDataLoadKind::Snapshot, request_id),
                AppDataReq::FullLoad { request_id, .. } => (AppDataLoadKind::Full, request_id),
                AppDataReq::DropState { .. } => panic!("unexpected DropState in backlog"),
            })
            .collect::<Vec<_>>();
        drained.sort_by_key(|(_, request_id)| *request_id);

        assert_eq!(
            drained,
            vec![
                (AppDataLoadKind::Initial, 1),
                (AppDataLoadKind::Snapshot, 2)
            ]
        );
        assert!(deferred.is_empty());
    }

    #[test]
    fn usage_pricing_custom_state_runs_one_and_keeps_latest_pending() {
        let older_custom = UsageRangePreset::Custom(crate::cli::tui::data::UsageCustomRange {
            start: 1_700_000_000,
            end: 1_700_086_399,
        });
        let newer_custom = UsageRangePreset::Custom(crate::cli::tui::data::UsageCustomRange {
            start: 1_700_086_400,
            end: 1_700_172_799,
        });
        let mut state = UsagePricingCustomState::default();

        let first = UsagePricingReq::Load {
            request_id: 1,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: older_custom,
        };
        let stale_pending = UsagePricingReq::Load {
            request_id: 2,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: older_custom,
        };
        let latest_pending = UsagePricingReq::Load {
            request_id: 3,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: newer_custom,
        };

        assert!(state.enqueue(first).is_some());
        let cancel_token = Arc::new(AtomicBool::new(false));
        state.set_running_cancel_token(Arc::clone(&cancel_token));
        assert!(state.enqueue(stale_pending).is_none());
        assert!(state.enqueue(latest_pending).is_none());
        assert!(cancel_token.load(Ordering::Relaxed));
        assert_eq!(
            usage_pricing_custom_state_snapshot(&state),
            (true, Some(1), true, false, vec![3])
        );

        let completion = state.complete();
        let next = completion.next.expect("latest pending request");
        assert!(completion.drop_acks.is_empty());
        assert!(matches!(
            next,
            UsagePricingReq::Load {
                request_id: 3,
                range,
                ..
            } if range == newer_custom
        ));
        assert_eq!(
            usage_pricing_custom_state_snapshot(&state),
            (true, Some(3), false, false, Vec::new())
        );

        let completion = state.complete();
        assert!(completion.next.is_none());
        assert!(completion.drop_acks.is_empty());
        assert_eq!(
            usage_pricing_custom_state_snapshot(&state),
            (false, None, false, true, Vec::new())
        );
    }

    #[test]
    fn usage_pricing_custom_state_remembers_drop_before_interrupt_handle() {
        let custom = UsageRangePreset::Custom(crate::cli::tui::data::UsageCustomRange {
            start: 1_700_000_000,
            end: 1_700_086_399,
        });
        let mut state = UsagePricingCustomState::default();

        assert!(state
            .enqueue(UsagePricingReq::Load {
                request_id: 1,
                generation: 0,
                app_state_epoch: 0,
                app_type: AppType::Claude,
                range: custom,
            })
            .is_some());
        let cancel_token = Arc::new(AtomicBool::new(false));
        state.set_running_cancel_token(Arc::clone(&cancel_token));
        let (ack_tx, ack_rx) = mpsc::channel();
        state.clear_pending_and_ack_when_idle(ack_tx);

        assert!(cancel_token.load(Ordering::Relaxed));
        assert!(ack_rx.try_recv().is_err());
        assert_eq!(
            usage_pricing_custom_state_snapshot(&state),
            (true, Some(1), true, false, Vec::new())
        );

        let completion = state.complete();
        assert!(completion.next.is_none());
        assert_eq!(completion.drop_acks.len(), 1);
        for ack in completion.drop_acks {
            let _ = ack.send(());
        }
        assert!(ack_rx.recv().is_ok());
        assert_eq!(
            usage_pricing_custom_state_snapshot(&state),
            (false, None, false, true, Vec::new())
        );
    }

    #[test]
    fn usage_pricing_custom_runner_defers_drop_ack_until_worker_completes() {
        let (tx, _rx) = mpsc::channel();
        let runner = UsagePricingCustomRunner::new(tx);
        let custom = UsageRangePreset::Custom(crate::cli::tui::data::UsageCustomRange {
            start: 1_700_000_000,
            end: 1_700_086_399,
        });

        {
            let mut state = runner.state.lock().expect("custom state should lock");
            assert!(state
                .enqueue(UsagePricingReq::Load {
                    request_id: 1,
                    generation: 0,
                    app_state_epoch: 0,
                    app_type: AppType::Claude,
                    range: custom,
                })
                .is_some());
            state.set_running_cancel_token(Arc::new(AtomicBool::new(false)));
        }

        let (ack_tx, ack_rx) = mpsc::channel();
        runner.drop_state(ack_tx);
        assert!(ack_rx.try_recv().is_err());

        let completion = runner
            .state
            .lock()
            .expect("custom state should lock")
            .complete();
        assert!(completion.next.is_none());
        assert_eq!(completion.drop_acks.len(), 1);
        for ack in completion.drop_acks {
            let _ = ack.send(());
        }
        assert!(ack_rx.recv().is_ok());
    }
}
