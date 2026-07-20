use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::ops::ControlFlow;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc, Arc, Condvar, Mutex,
};

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::error::AppError;
use crate::services::{S3SyncService, SkillService, StreamCheckService, WebDavSyncService};
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
    UpdateMsg, UpdateReq, UpdateSystem, UsageLogLoadError, UsagePricingLoadError, UsagePricingMsg,
    UsagePricingReq, UsagePricingSystem, WebDavDone, WebDavErr, WebDavMsg, WebDavReq,
    WebDavReqKind, WebDavSystem,
};

static SESSION_SCAN_GENERATION: AtomicU64 = AtomicU64::new(0);
static SESSION_PROJECT_CATALOG_GENERATION: AtomicU64 = AtomicU64::new(0);
static SESSION_PROJECT_FILTER_GENERATION: AtomicU64 = AtomicU64::new(0);

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
                let mut cfg = webdav_jianguoyun_preset(&username, &password);
                // Saving credentials is not consent to switch the active sync backend.
                cfg.enabled = false;
                cfg.auto_sync = false;
                if let Err(err) = set_webdav_sync_settings(Some(cfg)) {
                    Err(WebDavErr::QuickSetupSave(err.to_string()))
                } else if let Err(err) = WebDavSyncService::check_connection() {
                    Err(WebDavErr::QuickSetupCheck(err.to_string()))
                } else {
                    Ok(WebDavDone::JianguoyunConfigured)
                }
            }
            WebDavReqKind::S3CheckConnection => S3SyncService::check_connection()
                .map(|_| WebDavDone::S3ConnectionChecked)
                .map_err(|e| WebDavErr::Generic(e.to_string())),
            WebDavReqKind::S3FetchRemoteInfo { intent } => S3SyncService::fetch_remote_info()
                .map(|info| WebDavDone::S3RemoteInfoFetched { intent, info })
                .map_err(|e| WebDavErr::Generic(e.to_string())),
            WebDavReqKind::S3Upload => S3SyncService::upload()
                .map(|summary| WebDavDone::S3Uploaded {
                    decision: summary.decision,
                    message: summary.message,
                })
                .map_err(|e| WebDavErr::Generic(e.to_string())),
            WebDavReqKind::S3Download => S3SyncService::download()
                .map(|summary| WebDavDone::S3Downloaded {
                    decision: summary.decision,
                    message: summary.message,
                })
                .map_err(|e| WebDavErr::Generic(e.to_string())),
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
            custom_user_agent,
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
                fetch_provider_models_for_tui(
                    &base_url,
                    api_key.as_deref(),
                    custom_user_agent.as_deref(),
                    strategy,
                )
                .await
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
    let (req_tx, req_rx) = tokio::sync::mpsc::unbounded_channel::<LocalEnvReq>();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("failed to start local env runtime: {e}")))?;

    let handle = std::thread::Builder::new()
        .name("cc-switch-local-env".to_string())
        .spawn(move || runtime.block_on(local_env_worker_loop(req_rx, result_tx)))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn local env worker thread".to_string(),
            source: e,
        })?;

    Ok(LocalEnvSystem {
        req_tx,
        result_rx,
        _handle: Some(handle),
    })
}

pub(crate) fn start_session_system() -> Result<SessionSystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<SessionMsg>();
    let (req_tx, req_rx) = mpsc::channel::<SessionReq>();
    let (control_tx, control_rx) = mpsc::channel::<SessionReq>();
    let (locate_tx, locate_rx) = session_locate_mailbox();
    let search_generation = Arc::new(AtomicU64::new(0));
    let message_generation = Arc::new(AtomicU64::new(0));
    let locate_generation = Arc::new(AtomicU64::new(0));

    let locate_handle = std::thread::Builder::new()
        .name("cc-switch-session-locate".to_string())
        .spawn(move || session_locate_worker_loop(locate_rx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn session locate worker thread".to_string(),
            source: e,
        })?;

    let control_result_tx = result_tx.clone();
    let control_handle = match std::thread::Builder::new()
        .name("cc-switch-session-control".to_string())
        .spawn(move || session_control_worker_loop(control_rx, control_result_tx))
    {
        Ok(handle) => handle,
        Err(e) => {
            drop(locate_tx);
            let _ = locate_handle.join();
            return Err(AppError::IoContext {
                context: "failed to spawn session control worker thread".to_string(),
                source: e,
            });
        }
    };

    let dispatcher_generation = Arc::clone(&search_generation);
    let dispatcher_message_generation = Arc::clone(&message_generation);
    let dispatcher_locate_generation = Arc::clone(&locate_generation);
    let dispatcher_handle = match std::thread::Builder::new()
        .name("cc-switch-session-dispatch".to_string())
        .spawn(move || {
            session_dispatcher_loop(
                req_rx,
                control_tx,
                result_tx,
                dispatcher_generation,
                dispatcher_message_generation,
                dispatcher_locate_generation,
                locate_tx,
            )
        }) {
        Ok(handle) => handle,
        Err(e) => {
            drop(req_tx);
            let _ = control_handle.join();
            let _ = locate_handle.join();
            return Err(AppError::IoContext {
                context: "failed to spawn session dispatcher thread".to_string(),
                source: e,
            });
        }
    };

    Ok(SessionSystem {
        req_tx,
        result_rx,
        _handles: vec![dispatcher_handle, control_handle, locate_handle],
    })
}

/// Route cheap control requests to the single ordering-preserving worker while
/// launching deep searches on independent workers. Advancing the generation is
/// synchronous on this dispatcher, so a newer query or explicit cancellation
/// reaches an in-flight search even when message loading or deletion is busy.
fn session_dispatcher_loop(
    rx: mpsc::Receiver<SessionReq>,
    control_tx: mpsc::Sender<SessionReq>,
    result_tx: mpsc::Sender<SessionMsg>,
    search_generation: Arc<AtomicU64>,
    message_generation: Arc<AtomicU64>,
    locate_generation: Arc<AtomicU64>,
    locate_tx: SessionLocateSender,
) {
    session_dispatcher_loop_with(
        rx,
        control_tx,
        result_tx,
        search_generation,
        message_generation,
        locate_generation,
        launch_session_search,
        launch_session_messages,
        launch_session_page,
        launch_session_refresh,
        move |job| locate_tx.submit(job),
    );
}

struct SessionMessageJob {
    request_id: u64,
    key: String,
    provider_id: String,
    source_path: String,
    generation: u64,
    current_generation: Arc<AtomicU64>,
    result_tx: mpsc::Sender<SessionMsg>,
}

fn launch_session_messages(job: SessionMessageJob) -> Result<(), String> {
    std::thread::Builder::new()
        .name("cc-switch-session-messages".to_string())
        .spawn(move || {
            let result = crate::session_manager::load_messages_cancellable(
                &job.provider_id,
                &job.source_path,
                &|| job.current_generation.load(Ordering::Acquire) != job.generation,
            );
            if job.current_generation.load(Ordering::Acquire) == job.generation {
                let _ = job.result_tx.send(SessionMsg::MessagesLoaded {
                    request_id: job.request_id,
                    key: job.key,
                    result,
                });
            }
        })
        .map(|_| ())
        .map_err(|error| format!("failed to spawn session message worker: {error}"))
}

struct SessionSearchJob {
    request_id: u64,
    scope_epoch: u64,
    view: crate::session_manager::project_scope::SessionViewSpec,
    base: crate::cli::tui::app::SessionPageToken,
    base_reader: crate::session_manager::paged_manifest::ManifestReader,
    query_namespace: crate::session_manager::paged_manifest::QueryManifestNamespace,
    generation: u64,
    current_generation: Arc<AtomicU64>,
    result_tx: mpsc::Sender<SessionMsg>,
}

fn launch_session_search(job: SessionSearchJob) -> Result<(), String> {
    std::thread::Builder::new()
        .name("cc-switch-session-search".to_string())
        .spawn(move || {
            run_session_search(
                job.request_id,
                job.scope_epoch,
                job.view,
                job.base,
                job.base_reader,
                job.query_namespace,
                job.generation,
                job.current_generation,
                &job.result_tx,
            )
        })
        .map(|_| ())
        .map_err(|err| format!("failed to spawn session search worker: {err}"))
}

struct SessionPageJob {
    request_id: u64,
    token: crate::cli::tui::app::SessionPageToken,
    page: usize,
    reader: crate::session_manager::paged_manifest::ManifestReader,
    result_tx: mpsc::Sender<SessionMsg>,
}

struct SessionRefreshJob {
    request_id: u64,
    scope_epoch: u64,
    provider_id: String,
    force: bool,
    scan_generation: u64,
    result_tx: mpsc::Sender<SessionMsg>,
}

fn launch_session_refresh(job: SessionRefreshJob) -> Result<(), String> {
    std::thread::Builder::new()
        .name("cc-switch-session-scan".to_string())
        .spawn(move || {
            run_session_scan(
                job.request_id,
                job.scope_epoch,
                job.provider_id,
                job.force,
                job.scan_generation,
                &job.result_tx,
            )
        })
        .map(|_| ())
        .map_err(|error| format!("failed to spawn session scan worker: {error}"))
}

struct SessionLocateJob {
    request_id: u64,
    scope_epoch: u64,
    source: crate::cli::tui::app::SessionPageSource,
    scope: String,
    generation: String,
    fallback_absolute: usize,
    anchor: Option<crate::cli::tui::app::SessionRowIdentity>,
    reader: crate::session_manager::paged_manifest::ManifestReader,
    locate_generation: u64,
    current_generation: Arc<AtomicU64>,
    result_tx: mpsc::Sender<SessionMsg>,
}

#[derive(Default)]
struct SessionLocateMailboxState {
    pending: Option<SessionLocateJob>,
    closed: bool,
}

struct SessionLocateMailbox {
    state: Mutex<SessionLocateMailboxState>,
    ready: Condvar,
}

struct SessionLocateSender {
    mailbox: Arc<SessionLocateMailbox>,
}

struct SessionLocateReceiver {
    mailbox: Arc<SessionLocateMailbox>,
}

fn session_locate_mailbox() -> (SessionLocateSender, SessionLocateReceiver) {
    let mailbox = Arc::new(SessionLocateMailbox {
        state: Mutex::new(SessionLocateMailboxState::default()),
        ready: Condvar::new(),
    });
    (
        SessionLocateSender {
            mailbox: Arc::clone(&mailbox),
        },
        SessionLocateReceiver { mailbox },
    )
}

impl SessionLocateSender {
    fn submit(&self, job: SessionLocateJob) -> Result<(), String> {
        let mut state = self
            .mailbox
            .state
            .lock()
            .map_err(|_| "sessions locate mailbox is unavailable".to_string())?;
        if state.closed {
            return Err("sessions locate worker is not running".to_string());
        }
        // Replacing the slot drops the superseded reader lease immediately;
        // at most one pending immutable generation can be pinned here.
        state.pending = Some(job);
        self.mailbox.ready.notify_one();
        Ok(())
    }
}

impl Drop for SessionLocateSender {
    fn drop(&mut self) {
        let mut state = self
            .mailbox
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.closed = true;
        self.mailbox.ready.notify_one();
    }
}

impl SessionLocateReceiver {
    fn recv_latest(&self) -> Option<SessionLocateJob> {
        let mut state = self
            .mailbox
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(job) = state.pending.take() {
                return Some(job);
            }
            if state.closed {
                return None;
            }
            state = self
                .mailbox
                .ready
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

/// A single fixed worker owns manifest selection reconciliation. The public
/// dispatcher advances `current_generation` before enqueueing each job, so an
/// active lookup stops between bounded page reads. Its one-slot mailbox
/// collapses a burst of publications to the newest immutable generation
/// without creating one detached thread per publication.
fn session_locate_worker_loop(rx: SessionLocateReceiver) {
    while let Some(job) = rx.recv_latest() {
        locate_session_manifest(job);
    }
}

fn launch_session_page(job: SessionPageJob) -> Result<(), String> {
    std::thread::Builder::new()
        .name("cc-switch-session-page".to_string())
        .spawn(move || {
            let result = if job.reader.scope() != job.token.scope
                || job.reader.generation() != job.token.generation
            {
                Err("session manifest reader does not match its page token".to_string())
            } else {
                job.reader
                    .load_page(job.page)
                    .ok_or_else(|| "session manifest page is unavailable".to_string())
            };
            let _ = job.result_tx.send(SessionMsg::PageLoaded {
                request_id: job.request_id,
                token: job.token,
                page: job.page,
                result,
            });
        })
        .map(|_| ())
        .map_err(|error| format!("failed to spawn session page worker: {error}"))
}

fn session_dispatcher_loop_with<S, L, P, R, M>(
    rx: mpsc::Receiver<SessionReq>,
    control_tx: mpsc::Sender<SessionReq>,
    result_tx: mpsc::Sender<SessionMsg>,
    search_generation: Arc<AtomicU64>,
    message_generation: Arc<AtomicU64>,
    locate_generation: Arc<AtomicU64>,
    mut launch_search: S,
    mut launch_messages: L,
    mut launch_page: P,
    mut launch_refresh: R,
    mut launch_locate: M,
) where
    S: FnMut(SessionSearchJob) -> Result<(), String>,
    L: FnMut(SessionMessageJob) -> Result<(), String>,
    P: FnMut(SessionPageJob) -> Result<(), String>,
    R: FnMut(SessionRefreshJob) -> Result<(), String>,
    M: FnMut(SessionLocateJob) -> Result<(), String>,
{
    while let Ok(req) = rx.recv() {
        match req {
            SessionReq::Search {
                request_id,
                scope_epoch,
                view,
                base,
                base_reader,
                query_namespace,
            } => {
                let generation = search_generation.fetch_add(1, Ordering::AcqRel) + 1;
                let error_scope = base.scope.clone();
                let error_base_generation = base.generation.clone();
                let error_view = view.clone();
                let error_namespace = query_namespace.clone();
                let job = SessionSearchJob {
                    request_id,
                    scope_epoch,
                    view,
                    base,
                    base_reader,
                    query_namespace,
                    generation,
                    current_generation: Arc::clone(&search_generation),
                    result_tx: result_tx.clone(),
                };
                if let Err(err) = launch_search(job) {
                    let _ = result_tx.send(SessionMsg::QueryPublished {
                        request_id,
                        scope_epoch,
                        scope: error_scope,
                        base_generation: error_base_generation,
                        view: error_view,
                        query_namespace: error_namespace,
                        result: Err(err),
                    });
                }
            }
            SessionReq::CancelSearch => {
                search_generation.fetch_add(1, Ordering::AcqRel);
            }
            SessionReq::LoadMessages {
                request_id,
                key,
                provider_id,
                source_path,
            } => {
                let generation = message_generation.fetch_add(1, Ordering::AcqRel) + 1;
                let error_key = key.clone();
                let job = SessionMessageJob {
                    request_id,
                    key,
                    provider_id,
                    source_path,
                    generation,
                    current_generation: Arc::clone(&message_generation),
                    result_tx: result_tx.clone(),
                };
                if let Err(error) = launch_messages(job) {
                    if message_generation.load(Ordering::Acquire) == generation {
                        let _ = result_tx.send(SessionMsg::MessagesLoaded {
                            request_id,
                            key: error_key,
                            result: Err(error),
                        });
                    }
                }
            }
            SessionReq::CancelMessages => {
                message_generation.fetch_add(1, Ordering::AcqRel);
            }
            SessionReq::LoadPage {
                request_id,
                token,
                page,
                reader,
            } => {
                let error_token = token.clone();
                let job = SessionPageJob {
                    request_id,
                    token,
                    page,
                    reader,
                    result_tx: result_tx.clone(),
                };
                if let Err(error) = launch_page(job) {
                    let _ = result_tx.send(SessionMsg::PageLoaded {
                        request_id,
                        token: error_token,
                        page,
                        result: Err(error),
                    });
                }
            }
            SessionReq::Refresh {
                request_id,
                scope_epoch,
                provider_id,
                force,
            } => {
                let scan_generation = SESSION_SCAN_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
                let error_scope = provider_id.clone();
                let job = SessionRefreshJob {
                    request_id,
                    scope_epoch,
                    provider_id,
                    force,
                    scan_generation,
                    result_tx: result_tx.clone(),
                };
                if let Err(error) = launch_refresh(job) {
                    let _ = result_tx.send(SessionMsg::ManifestPublished {
                        request_id,
                        scope_epoch,
                        scope: error_scope,
                        result: Err(error),
                    });
                }
            }
            SessionReq::LocateManifest {
                request_id,
                scope_epoch,
                source,
                scope,
                generation,
                fallback_absolute,
                anchor,
                reader,
            } => {
                let job_locate_generation = locate_generation.fetch_add(1, Ordering::AcqRel) + 1;
                let error_generation = generation.clone();
                let job = SessionLocateJob {
                    request_id,
                    scope_epoch,
                    source,
                    scope,
                    generation,
                    fallback_absolute,
                    anchor,
                    reader,
                    locate_generation: job_locate_generation,
                    current_generation: Arc::clone(&locate_generation),
                    result_tx: result_tx.clone(),
                };
                if let Err(error) = launch_locate(job) {
                    if locate_generation.load(Ordering::Acquire) == job_locate_generation {
                        let _ = result_tx.send(SessionMsg::ManifestLocated {
                            request_id,
                            scope_epoch,
                            generation: error_generation,
                            result: Err(error),
                        });
                    }
                }
            }
            control => {
                if control_tx.send(control).is_err() {
                    break;
                }
            }
        }
    }

    // Dropping the public request sender cancels detached search workers before
    // the dispatcher releases its result sender and shuts down the control lane.
    search_generation.fetch_add(1, Ordering::AcqRel);
    message_generation.fetch_add(1, Ordering::AcqRel);
    locate_generation.fetch_add(1, Ordering::AcqRel);
}

fn session_control_worker_loop(rx: mpsc::Receiver<SessionReq>, tx: mpsc::Sender<SessionMsg>) {
    while let Ok(mut req) = rx.recv() {
        for next in rx.try_iter() {
            match (&req, &next) {
                (SessionReq::Refresh { .. }, SessionReq::Refresh { .. }) => req = next,
                _ => {
                    let _ = handle_session_req(req, &tx);
                    req = next;
                }
            }
        }

        let _ = handle_session_req(req, &tx);
    }
}

fn acknowledge_delete_then_launch_purge<F>(
    tx: &mpsc::Sender<SessionMsg>,
    request_id: u64,
    key: String,
    result: Result<(), String>,
    launch_purge: F,
) -> Result<(), ()>
where
    F: FnOnce(mpsc::Sender<SessionMsg>, u64, String) -> Result<(), String>,
{
    tx.send(SessionMsg::DeleteFinished {
        request_id,
        key: key.clone(),
        result: result.clone(),
    })
    .map_err(|_| ())?;
    if result.is_ok() {
        if let Err(error) = launch_purge(tx.clone(), request_id, key.clone()) {
            // An empty purge result tells the UI that no safe replacement
            // generation was published. It keeps the tombstone and schedules
            // one authoritative refresh instead of pinning the old generation
            // forever after a thread-spawn failure.
            log::debug!("[SESSION-PAGES] failed to spawn purge worker: {error}");
            tx.send(SessionMsg::ManifestsPurged {
                request_id,
                key,
                base: Vec::new(),
            })
            .map_err(|_| ())?;
        }
    }
    Ok(())
}

fn handle_session_req(req: SessionReq, tx: &mpsc::Sender<SessionMsg>) -> Result<(), ()> {
    match req {
        SessionReq::Refresh {
            request_id,
            scope_epoch,
            provider_id,
            force,
        } => {
            // 会话扫描（尤其首扫全量解析，可达数十秒）放到一次性 detached 线程执行，
            // worker 循环立即回去处理后续的 LoadMessages/Search/Delete，避免长扫描把
            // 交互请求全部堵在队列里。UI 入口侧（自动进入页面 + 手动 r）都以
            // scan_active/loading 防抖，保证同一时刻至多一个扫描在飞（详见
            // queue_sessions_refresh_if_needed 与 Action::SessionsRefresh），故这里无需
            // 额外的并发标志；即便偶发起了两个，旧线程的 partial/finished 带旧
            // request_id，UI 侧按 scan_active 天然拒收，最多浪费一次 IO。
            let tx_scan = tx.clone();
            let provider_for_scan = provider_id.clone();
            let scan_generation = SESSION_SCAN_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
            match std::thread::Builder::new()
                .name("cc-switch-session-scan".to_string())
                .spawn(move || {
                    run_session_scan(
                        request_id,
                        scope_epoch,
                        provider_for_scan,
                        force,
                        scan_generation,
                        &tx_scan,
                    )
                }) {
                Ok(_handle) => {} // detached：不 join，worker 立即返回
                Err(err) => {
                    // Never let an old/large scope fall back onto the control
                    // lane: that would make a subsequent scope appear frozen.
                    let _ = tx.send(SessionMsg::ManifestPublished {
                        request_id,
                        scope_epoch,
                        scope: provider_id,
                        result: Err(format!("failed to spawn session scan worker: {err}")),
                    });
                }
            }
            Ok(())
        }
        SessionReq::LoadPage {
            request_id,
            token,
            page,
            reader,
        } => {
            // Public requests normally take the dispatcher's independent page
            // lane. Keep direct/internal callers non-blocking as well.
            let error_token = token.clone();
            let job = SessionPageJob {
                request_id,
                token,
                page,
                reader,
                result_tx: tx.clone(),
            };
            if let Err(error) = launch_session_page(job) {
                tx.send(SessionMsg::PageLoaded {
                    request_id,
                    token: error_token,
                    page,
                    result: Err(error),
                })
                .map_err(|_| ())?;
            }
            Ok(())
        }
        SessionReq::LocateManifest {
            request_id,
            scope_epoch,
            source,
            scope,
            generation,
            fallback_absolute,
            anchor,
            reader,
        } => {
            // Public requests use the dedicated latest-wins locate lane. A
            // direct/internal caller still does only the same bounded page
            // lookup, so it is safe to complete on this control worker.
            let current_generation = Arc::new(AtomicU64::new(1));
            locate_session_manifest(SessionLocateJob {
                request_id,
                scope_epoch,
                source,
                scope,
                generation,
                fallback_absolute,
                anchor,
                reader,
                locate_generation: 1,
                current_generation,
                result_tx: tx.clone(),
            });
            Ok(())
        }
        SessionReq::LoadMessages {
            request_id,
            key,
            provider_id,
            source_path,
        } => {
            // Public requests are intercepted by the dispatcher's latest-wins
            // message lane. Keep direct/internal callers non-blocking too, so
            // a detail read can never delay Delete on this control worker.
            let generation = Arc::new(AtomicU64::new(1));
            let error_key = key.clone();
            let job = SessionMessageJob {
                request_id,
                key,
                provider_id,
                source_path,
                generation: 1,
                current_generation: generation,
                result_tx: tx.clone(),
            };
            if let Err(error) = launch_session_messages(job) {
                tx.send(SessionMsg::MessagesLoaded {
                    request_id,
                    key: error_key,
                    result: Err(error),
                })
                .map_err(|_| ())?;
            }
            Ok(())
        }
        SessionReq::ProjectCatalog {
            request_id,
            scope_epoch,
            base,
            base_reader,
        } => {
            let catalog_generation =
                SESSION_PROJECT_CATALOG_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
            let result_tx = tx.clone();
            let error_tx = tx.clone();
            let error_scope = base.scope.clone();
            let error_generation = base.generation.clone();
            match std::thread::Builder::new()
                .name("cc-switch-session-projects".to_string())
                .spawn(move || {
                    let cancelled = || {
                        SESSION_PROJECT_CATALOG_GENERATION.load(Ordering::Acquire)
                            != catalog_generation
                    };
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        if base.source != crate::cli::tui::app::SessionPageSource::Base {
                            return Err("project catalog source is not a base manifest".to_string());
                        }
                        if base_reader.scope() != base.scope
                            || base_reader.generation() != base.generation
                        {
                            return Err(
                                "project catalog reader does not match its base token".to_string()
                            );
                        }
                        crate::session_manager::project_scope::aggregate_project_directories(
                            &base_reader,
                            &cancelled,
                        )
                        .map_err(|error| error.to_string())
                    }))
                    .unwrap_or_else(|_| Err("project catalog build panicked".to_string()));
                    if cancelled() {
                        return;
                    }
                    let _ = result_tx.send(SessionMsg::ProjectCatalogBuilt {
                        request_id,
                        scope_epoch,
                        scope: base.scope,
                        base_generation: base.generation,
                        result: outcome,
                    });
                }) {
                Ok(_) => Ok(()),
                Err(error) => error_tx
                    .send(SessionMsg::ProjectCatalogBuilt {
                        request_id,
                        scope_epoch,
                        scope: error_scope,
                        base_generation: error_generation,
                        result: Err(format!(
                            "failed to spawn session project catalog worker: {error}"
                        )),
                    })
                    .map_err(|_| ()),
            }
        }
        SessionReq::CancelProjectCatalog => {
            SESSION_PROJECT_CATALOG_GENERATION.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
        SessionReq::ProjectFilter {
            request_id,
            scope_epoch,
            scope,
            base_generation,
            query,
            catalog,
            project_offset,
            fixed_matches,
            trailing_matches,
        } => {
            let filter_generation =
                SESSION_PROJECT_FILTER_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
            let result_tx = tx.clone();
            let error_tx = tx.clone();
            let error_scope = scope.clone();
            let error_base_generation = base_generation.clone();
            let error_query = query.clone();
            match std::thread::Builder::new()
                .name("cc-switch-session-project-filter".to_string())
                .spawn(move || {
                    let cancelled = || {
                        SESSION_PROJECT_FILTER_GENERATION.load(Ordering::Acquire)
                            != filter_generation
                    };
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let mut matches = fixed_matches;
                        for (index, project) in catalog.projects.iter().enumerate() {
                            if cancelled() {
                                return Err("project filter was cancelled".to_string());
                            }
                            if crate::session_manager::project_scope::project_path_contains_query(
                                &project.display_path,
                                &query,
                            ) {
                                matches.push(project_offset.saturating_add(index));
                            }
                        }
                        matches.extend(trailing_matches);
                        Ok(matches)
                    }))
                    .unwrap_or_else(|_| Err("project filter panicked".to_string()));
                    if cancelled() {
                        return;
                    }
                    let _ = result_tx.send(SessionMsg::ProjectFilterBuilt {
                        request_id,
                        scope_epoch,
                        scope,
                        base_generation,
                        query,
                        result: outcome,
                    });
                }) {
                Ok(_) => Ok(()),
                Err(error) => error_tx
                    .send(SessionMsg::ProjectFilterBuilt {
                        request_id,
                        scope_epoch,
                        scope: error_scope,
                        base_generation: error_base_generation,
                        query: error_query,
                        result: Err(format!(
                            "failed to spawn session project filter worker: {error}"
                        )),
                    })
                    .map_err(|_| ()),
            }
        }
        SessionReq::CancelProjectFilter => {
            SESSION_PROJECT_FILTER_GENERATION.fetch_add(1, Ordering::AcqRel);
            Ok(())
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
            // Establish the UI tombstone before a detached repack is allowed to
            // report completion. Since the purge thread is spawned only after
            // this send completes, `ManifestsPurged` cannot overtake the delete
            // acknowledgement on the shared result channel.
            acknowledge_delete_then_launch_purge(
                tx,
                request_id,
                key,
                result,
                move |purge_tx, request_id, key| {
                    // Repack every schema-free cache off the control lane. The
                    // UI tombstone covers the short interval before these
                    // generations are published and installed.
                    std::thread::Builder::new()
                        .name("cc-switch-session-manifest-purge".to_string())
                        .spawn(move || {
                            let purged =
                                crate::session_manager::purge_deleted_session_from_local_caches(
                                    &provider_id,
                                    &session_id,
                                    &source_path,
                                );
                            let base = purged
                                .base
                                .into_iter()
                                .map(|(scope, published)| {
                                    let reader = published.reader.clone();
                                    (scope, published, reader)
                                })
                                .collect();
                            let _ = purge_tx.send(SessionMsg::ManifestsPurged {
                                request_id,
                                key,
                                base,
                            });
                        })
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                },
            )
        }
        // Search requests are intercepted by `session_dispatcher_loop` and
        // never enter the ordering-sensitive control lane.
        SessionReq::Search { .. } | SessionReq::CancelSearch | SessionReq::CancelMessages => Ok(()),
    }
}

fn run_session_search(
    request_id: u64,
    scope_epoch: u64,
    view: crate::session_manager::project_scope::SessionViewSpec,
    base: crate::cli::tui::app::SessionPageToken,
    base_reader: crate::session_manager::paged_manifest::ManifestReader,
    query_namespace: crate::session_manager::paged_manifest::QueryManifestNamespace,
    generation: u64,
    current_generation: Arc<AtomicU64>,
    tx: &mpsc::Sender<SessionMsg>,
) {
    let is_cancelled = || current_generation.load(Ordering::Acquire) != generation;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if base.source != crate::cli::tui::app::SessionPageSource::Base {
            return Err("session query source is not a base manifest".to_string());
        }
        if base_reader.scope() != base.scope || base_reader.generation() != base.generation {
            return Err("session query base reader does not match its token".to_string());
        }
        if is_cancelled() {
            return Err("session query was cancelled".to_string());
        }
        let query_store =
            crate::session_manager::paged_manifest::QueryManifestStore::open(&query_namespace)
                .map_err(|error| error.to_string())?;
        let builder = query_store
            .begin_build(&base_reader)
            .map_err(|error| error.to_string())?;
        let published = crate::session_manager::query_manifest::build_view_manifest(
            &base_reader,
            &view,
            builder,
            &is_cancelled,
        )
        .map_err(|error| error.to_string())?;
        let reader = published.reader.clone();
        Ok((published, reader))
    }));

    if is_cancelled() {
        return;
    }
    let result = match outcome {
        Ok(result) => result,
        Err(_) => Err("session search panicked".to_string()),
    };
    if is_cancelled() {
        return;
    }
    let _ = tx.send(SessionMsg::QueryPublished {
        request_id,
        scope_epoch,
        scope: base.scope,
        base_generation: base.generation,
        view,
        query_namespace,
        result,
    });
}

fn run_session_scan(
    request_id: u64,
    scope_epoch: u64,
    provider_id: String,
    force: bool,
    scan_generation: u64,
    tx: &mpsc::Sender<SessionMsg>,
) {
    let manifest_store = match crate::session_manager::paged_manifest::PagedManifestStore::open() {
        Ok(store) => store,
        Err(error) => {
            let _ = tx.send(SessionMsg::ManifestPublished {
                request_id,
                scope_epoch,
                scope: provider_id,
                result: Err(error.to_string()),
            });
            return;
        }
    };

    // Opening a scope reads one immutable page only. It happens before any
    // revalidation work and therefore remains fixed-cost for very large stores.
    let mut needs_provisional = false;
    if !force {
        let opened = manifest_store
            .open_reader(&provider_id)
            .and_then(|reader| reader.load_page(0).map(|page| (reader, page)));
        needs_provisional = opened.is_none();
        let _ = tx.send(SessionMsg::ScopeOpened {
            request_id,
            scope_epoch,
            scope: provider_id.clone(),
            result: Ok(opened),
        });
    }

    let store = match crate::session_manager::scan_cache_store::ScanCacheStore::open() {
        Ok(store) => Some(store),
        Err(error) => {
            log::debug!("[SESSION-SCAN] 打开 sidecar 失败，回退无缓存扫描: {error}");
            None
        }
    };
    let is_cancelled = || SESSION_SCAN_GENERATION.load(Ordering::Acquire) != scan_generation;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut builder = manifest_store
            .begin_build(&provider_id)
            .map_err(|error| error.to_string())?;
        let mut provisional = crate::session_manager::paged_manifest::BoundedRecencyPreview::new(
            crate::session_manager::paged_manifest::PAGE_SIZE,
        );
        let mut seen = 0usize;
        let mut last_preview = 0usize;
        let mut next_preview_at = 1usize;
        let one_provider = provider_id.as_str();
        let providers: &[&str] = if provider_id == "all" {
            &crate::session_manager::CACHED_PROVIDERS
        } else {
            std::slice::from_ref(&one_provider)
        };
        for provider in providers {
            if is_cancelled() {
                return Err("session manifest build was cancelled".to_string());
            }
            let mut push_error = None;
            let mut on_session = |row: crate::session_manager::SessionMeta| {
                if needs_provisional {
                    provisional.push(row.clone());
                    seen = seen.saturating_add(1);
                    if seen >= next_preview_at {
                        last_preview = seen;
                        let _ = tx.send(SessionMsg::ScanProvisional {
                            request_id,
                            scope_epoch,
                            scope: provider_id.clone(),
                            rows: provisional.snapshot(),
                        });
                        next_preview_at = if next_preview_at == 1 {
                            16
                        } else {
                            next_preview_at.saturating_mul(4)
                        };
                    }
                }
                if let Err(error) = builder.push(row) {
                    push_error = Some(error.to_string());
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            };
            let scan = crate::session_manager::stream_sessions_for_provider_cancellable(
                store.as_ref(),
                provider,
                force,
                &mut on_session,
                &is_cancelled,
            );
            if let Some(error) = push_error {
                return Err(error);
            }
            match scan {
                Ok((_stats, Some(enricher))) => builder
                    .add_identity_enricher(enricher)
                    .map_err(|error| error.to_string())?,
                Ok((_stats, None)) => {}
                Err(_) => return Err("session manifest scan was stopped".to_string()),
            }
        }
        if needs_provisional && seen > last_preview {
            let _ = tx.send(SessionMsg::ScanProvisional {
                request_id,
                scope_epoch,
                scope: provider_id.clone(),
                rows: provisional.snapshot(),
            });
        }
        if is_cancelled() {
            return Err("session manifest build was cancelled".to_string());
        }
        builder.publish().map_err(|error| error.to_string())
    }))
    .unwrap_or_else(|_| Err("session manifest scan panicked".to_string()));
    if is_cancelled() {
        return;
    }
    let result = result.map(|published| {
        let reader = published.reader.clone();
        (published, reader)
    });
    let _ = tx.send(SessionMsg::ManifestPublished {
        request_id,
        scope_epoch,
        scope: provider_id,
        result,
    });
}

const SESSION_LOCATE_PAGE_RADIUS: usize = 1;

fn locate_session_manifest(job: SessionLocateJob) {
    let is_cancelled = || job.current_generation.load(Ordering::Acquire) != job.locate_generation;
    let result = resolve_session_manifest_selection(
        job.source,
        &job.scope,
        &job.generation,
        job.fallback_absolute,
        job.anchor.as_ref(),
        &job.reader,
        &is_cancelled,
    );
    if is_cancelled() {
        return;
    }
    let result = match result {
        Ok(Some((page, selected))) => Ok((job.reader, page, selected)),
        Ok(None) => return,
        Err(error) => Err(error),
    };
    let _ = job.result_tx.send(SessionMsg::ManifestLocated {
        request_id: job.request_id,
        scope_epoch: job.scope_epoch,
        generation: job.generation,
        result,
    });
}

/// Resolve against the fallback page and its immediate neighbours only. A
/// refresh normally moves the selected row by a handful of positions, so this
/// preserves its identity across page-boundary shifts. A large reorder falls
/// back to the same absolute position instead of scanning an unbounded number
/// of immutable pages.
fn resolve_session_manifest_selection<F>(
    source: crate::cli::tui::app::SessionPageSource,
    scope: &str,
    generation: &str,
    fallback_absolute: usize,
    anchor: Option<&crate::cli::tui::app::SessionRowIdentity>,
    reader: &crate::session_manager::paged_manifest::ManifestReader,
    is_cancelled: &F,
) -> Result<Option<(crate::session_manager::paged_manifest::ManifestPage, usize)>, String>
where
    F: Fn() -> bool,
{
    if is_cancelled() {
        return Ok(None);
    }
    let source_label = match source {
        crate::cli::tui::app::SessionPageSource::Base => "base",
        crate::cli::tui::app::SessionPageSource::Query => "query",
    };
    if reader.scope() != scope || reader.generation() != generation {
        return Err(format!(
            "session {source_label} reader does not match its token"
        ));
    }

    let fallback = fallback_absolute.min(reader.total_rows().saturating_sub(1));
    let fallback_page_index = fallback / crate::session_manager::paged_manifest::PAGE_SIZE;
    let mut candidate_pages = Vec::with_capacity(1 + SESSION_LOCATE_PAGE_RADIUS * 2);
    candidate_pages.push(fallback_page_index);
    if anchor.is_some() {
        for distance in 1..=SESSION_LOCATE_PAGE_RADIUS {
            if let Some(previous) = fallback_page_index.checked_sub(distance) {
                candidate_pages.push(previous);
            }
            let next = fallback_page_index.saturating_add(distance);
            if next < reader.page_count() {
                candidate_pages.push(next);
            }
        }
    }

    let mut fallback_page = None;
    for page_index in candidate_pages {
        if is_cancelled() {
            return Ok(None);
        }
        let page = reader
            .load_page(page_index)
            .ok_or_else(|| format!("session manifest page {page_index} is unavailable"))?;
        if is_cancelled() {
            return Ok(None);
        }
        if let Some(selected) =
            anchor.and_then(|anchor| page.rows.iter().position(|row| anchor.matches(row)))
        {
            return Ok(Some((page, selected)));
        }
        if page_index == fallback_page_index {
            fallback_page = Some(page);
        }
    }

    let page =
        fallback_page.ok_or_else(|| "session manifest fallback page is unavailable".to_string())?;
    let selected = fallback
        .saturating_sub(fallback_page_index * crate::session_manager::paged_manifest::PAGE_SIZE)
        .min(page.rows.len().saturating_sub(1));
    Ok(Some((page, selected)))
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

async fn local_env_worker_loop(
    rx: tokio::sync::mpsc::UnboundedReceiver<LocalEnvReq>,
    tx: mpsc::Sender<LocalEnvMsg>,
) {
    use crate::services::local_env_check::check_local_environment_progressive;

    local_env_worker_loop_with(rx, tx, |cancellation, batch_tx, generation| {
        let batch_cancellation = cancellation.clone();
        async move {
            check_local_environment_progressive(cancellation.clone(), |result| {
                if batch_tx
                    .send(LocalEnvMsg::ToolFinished { generation, result })
                    .is_err()
                {
                    cancellation.cancel();
                }
            })
            .await;

            if !batch_cancellation.is_cancelled() {
                let _ = batch_tx.send(LocalEnvMsg::BatchFinished { generation });
            }
        }
    })
    .await;
}

async fn local_env_worker_loop_with<F, Fut>(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<LocalEnvReq>,
    tx: mpsc::Sender<LocalEnvMsg>,
    run_batch: F,
) where
    F: Fn(
        crate::services::local_env_check::LocalEnvCancellation,
        mpsc::Sender<LocalEnvMsg>,
        u64,
    ) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    use crate::services::local_env_check::LocalEnvCancellation;

    let mut active = tokio::task::JoinSet::new();
    let mut active_cancellation = None::<LocalEnvCancellation>;
    let mut active_generation = None::<u64>;
    let mut pending_generation = None::<u64>;
    let mut requests_closed = false;

    loop {
        if active.is_empty() {
            active_cancellation = None;
            active_generation = None;
            if let Some(generation) = pending_generation.take() {
                let cancellation = LocalEnvCancellation::default();
                active_cancellation = Some(cancellation.clone());
                active_generation = Some(generation);
                active.spawn(run_batch(cancellation, tx.clone(), generation));
            } else if requests_closed {
                break;
            }
        }

        tokio::select! {
            request = rx.recv(), if !requests_closed => {
                match request {
                    Some(request) => {
                        let mut latest_generation = None;
                        let mut shutdown_requested = false;
                        let mut next_request = Some(request);
                        while let Some(request) = next_request {
                            match request {
                                LocalEnvReq::Refresh { generation } => {
                                    if !shutdown_requested {
                                        latest_generation = Some(generation);
                                    }
                                }
                                LocalEnvReq::Shutdown => shutdown_requested = true,
                            }
                            next_request = rx.try_recv().ok();
                        }

                        if shutdown_requested {
                            requests_closed = true;
                            pending_generation = None;
                            if let Some(cancellation) = &active_cancellation {
                                cancellation.cancel();
                            }
                            active.abort_all();
                        } else if let Some(generation) = latest_generation {
                            pending_generation = Some(generation);
                            if let Some(cancellation) = &active_cancellation {
                                cancellation.cancel();
                            }
                        }
                    }
                    None => {
                        requests_closed = true;
                        pending_generation = None;
                        if let Some(cancellation) = &active_cancellation {
                            cancellation.cancel();
                        }
                        active.abort_all();
                    }
                }
            }
            finished = active.join_next(), if !active.is_empty() => {
                if let Some(Err(error)) = finished {
                    log::debug!("local environment check task failed: {error}");
                    if let Some(generation) = active_generation {
                        let _ = tx.send(LocalEnvMsg::BatchFinished { generation });
                    }
                }
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
    let (aggregate_tx, aggregate_rx) = mpsc::channel::<UsagePricingReq>();
    let (log_tx, log_rx) = mpsc::channel::<UsagePricingReq>();

    let aggregate_result_tx = result_tx.clone();
    let aggregate_handle = std::thread::Builder::new()
        .name("cc-switch-usage-pricing".to_string())
        .spawn(move || usage_pricing_worker_loop(aggregate_rx, aggregate_result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn usage/pricing worker thread".to_string(),
            source: e,
        })?;

    let log_handle = std::thread::Builder::new()
        .name("cc-switch-usage-logs".to_string())
        .spawn(move || usage_log_worker_loop(log_rx, result_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn usage log worker thread".to_string(),
            source: e,
        })?;

    let dispatcher_handle = std::thread::Builder::new()
        .name("cc-switch-usage-dispatch".to_string())
        .spawn(move || usage_pricing_dispatcher_loop(req_rx, aggregate_tx, log_tx))
        .map_err(|e| AppError::IoContext {
            context: "failed to spawn usage dispatcher thread".to_string(),
            source: e,
        })?;

    Ok(UsagePricingSystem {
        req_tx,
        result_rx,
        _handles: vec![aggregate_handle, log_handle, dispatcher_handle],
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum UsagePricingReqRangeKey {
    Fixed,
    Custom,
    LogPage(UsageRangePreset, usize),
    LogDetail(UsageRangePreset, i64),
}

fn usage_pricing_req_key(req: &UsagePricingReq) -> Option<(AppType, UsagePricingReqRangeKey)> {
    match req {
        UsagePricingReq::Load {
            app_type, range, ..
        } => {
            let range_key = match range {
                UsageRangePreset::Custom(_) => UsagePricingReqRangeKey::Custom,
                _ => UsagePricingReqRangeKey::Fixed,
            };
            Some((app_type.clone(), range_key))
        }
        UsagePricingReq::LoadLogPage {
            app_type,
            range,
            page,
            ..
        } => Some((
            app_type.clone(),
            UsagePricingReqRangeKey::LogPage(*range, *page),
        )),
        UsagePricingReq::LoadLogDetail {
            app_type,
            range,
            log_rowid,
            ..
        } => Some((
            app_type.clone(),
            UsagePricingReqRangeKey::LogDetail(*range, *log_rowid),
        )),
        UsagePricingReq::DropState { .. } => None,
    }
}

#[derive(Default)]
struct UsagePricingAggregateState {
    running: bool,
    running_request_id: Option<u64>,
    running_interrupt: Option<rusqlite::InterruptHandle>,
    running_cancel: Option<Arc<AtomicBool>>,
    cancel_requested: bool,
    pending: Vec<UsagePricingReq>,
    drop_acks: Vec<mpsc::Sender<()>>,
}

#[derive(Default)]
struct UsagePricingAggregateCompletion {
    next: Option<UsagePricingReq>,
    drop_acks: Vec<mpsc::Sender<()>>,
}

impl UsagePricingAggregateState {
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
            token.store(true, Ordering::Release);
            return;
        }
        if self.cancel_requested || self.has_pending_newer_than_running() {
            token.store(true, Ordering::Release);
        }
        self.running_cancel = Some(token);
    }

    fn complete(&mut self) -> UsagePricingAggregateCompletion {
        self.running_interrupt = None;
        self.running_cancel = None;
        let drop_acks = self.drop_acks.drain(..).collect();
        if let Some(next) = self.pending.pop() {
            self.running_request_id = usage_pricing_req_request_id(&next);
            self.cancel_requested = false;
            return UsagePricingAggregateCompletion {
                next: Some(next),
                drop_acks,
            };
        }
        self.running = false;
        self.running_request_id = None;
        self.cancel_requested = false;
        UsagePricingAggregateCompletion {
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
            token.store(true, Ordering::Release);
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
        UsagePricingReq::Load { request_id, .. }
        | UsagePricingReq::LoadLogPage { request_id, .. }
        | UsagePricingReq::LoadLogDetail { request_id, .. } => Some(*request_id),
        UsagePricingReq::DropState { .. } => None,
    }
}

#[cfg(test)]
fn usage_pricing_aggregate_state_snapshot(
    state: &UsagePricingAggregateState,
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

fn spawn_usage_pricing_aggregate_req(
    req: UsagePricingReq,
    state: Arc<Mutex<UsagePricingAggregateState>>,
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
            "usage aggregate worker state is unavailable".to_string(),
        );
        finish_usage_pricing_aggregate_req(state, tx);
        return;
    }

    let fallback_req = req.clone();
    let fallback_state = Arc::clone(&state);
    let fallback_tx = tx.clone();
    match std::thread::Builder::new()
        .name("cc-switch-usage-aggregate-query".to_string())
        .spawn(move || {
            let interrupt_state = Arc::clone(&state);
            let panic_req = req.clone();
            let panic_tx = tx.clone();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                handle_usage_pricing_aggregate_req_with_cancel(
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
                    "usage aggregate worker panicked".to_string(),
                );
            }
            finish_usage_pricing_aggregate_req(state, tx);
        }) {
        Ok(_handle) => {}
        Err(err) => {
            send_usage_pricing_req_error(
                &fallback_req,
                &fallback_tx,
                format!("failed to spawn usage aggregate worker: {err}"),
            );
            finish_usage_pricing_aggregate_req(fallback_state, fallback_tx);
        }
    }
}

fn finish_usage_pricing_aggregate_req(
    state: Arc<Mutex<UsagePricingAggregateState>>,
    tx: mpsc::Sender<UsagePricingMsg>,
) {
    let completion = match state.lock() {
        Ok(mut state) => state.complete(),
        Err(_) => UsagePricingAggregateCompletion::default(),
    };
    for ack in completion.drop_acks {
        let _ = ack.send(());
    }
    if let Some(req) = completion.next {
        spawn_usage_pricing_aggregate_req(req, state, tx);
    }
}

struct UsagePricingAggregateRunner {
    state: Arc<Mutex<UsagePricingAggregateState>>,
    tx: mpsc::Sender<UsagePricingMsg>,
}

impl UsagePricingAggregateRunner {
    fn new(tx: mpsc::Sender<UsagePricingMsg>) -> Self {
        Self {
            state: Arc::new(Mutex::new(UsagePricingAggregateState::default())),
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
                    "usage aggregate worker state is unavailable".to_string(),
                );
                return;
            }
        };
        if let Some(req) = launch {
            spawn_usage_pricing_aggregate_req(req, Arc::clone(&self.state), self.tx.clone());
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
        } | UsagePricingReq::LoadLogPage {
            app_type: req_app_type,
            ..
        } | UsagePricingReq::LoadLogDetail {
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

fn usage_pricing_dispatcher_loop(
    rx: mpsc::Receiver<UsagePricingReq>,
    aggregate_tx: mpsc::Sender<UsagePricingReq>,
    log_tx: mpsc::Sender<UsagePricingReq>,
) {
    while let Ok(req) = rx.recv() {
        match req {
            req @ UsagePricingReq::Load { .. } => {
                // The log head and aggregate deliberately use different queues
                // and workers. A full-table aggregate can therefore never keep
                // the first log page (or a later keyset page) behind it.
                let _ = log_tx.send(req.clone());
                let _ = aggregate_tx.send(req);
            }
            req @ (UsagePricingReq::LoadLogPage { .. } | UsagePricingReq::LoadLogDetail { .. }) => {
                let _ = log_tx.send(req);
            }
            UsagePricingReq::DropState { ack } => {
                // Preserve DropState as a barrier across both independent
                // connections. A disconnected worker is already dropped and
                // therefore needs no acknowledgement.
                let mut receivers = Vec::with_capacity(2);
                let (log_ack_tx, log_ack_rx) = mpsc::channel();
                if log_tx
                    .send(UsagePricingReq::DropState { ack: log_ack_tx })
                    .is_ok()
                {
                    receivers.push(log_ack_rx);
                }
                let (aggregate_ack_tx, aggregate_ack_rx) = mpsc::channel();
                if aggregate_tx
                    .send(UsagePricingReq::DropState {
                        ack: aggregate_ack_tx,
                    })
                    .is_ok()
                {
                    receivers.push(aggregate_ack_rx);
                }
                for receiver in receivers {
                    let _ = receiver.recv();
                }
                let _ = ack.send(());
            }
        }
    }
}

#[derive(Default)]
struct UsageLogQueryState {
    running: bool,
    running_token: Option<UsageLogRequestToken>,
    cancel_requested: bool,
    running_interrupt: Option<rusqlite::InterruptHandle>,
    running_cancel: Option<Arc<AtomicBool>>,
    pending: Option<UsagePricingReq>,
    drop_acks: Vec<mpsc::Sender<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UsageLogRequestToken {
    request_id: u64,
    generation: u64,
    app_state_epoch: u64,
}

fn usage_log_request_token(req: &UsagePricingReq) -> Option<UsageLogRequestToken> {
    match req {
        UsagePricingReq::Load {
            request_id,
            generation,
            app_state_epoch,
            ..
        }
        | UsagePricingReq::LoadLogPage {
            request_id,
            generation,
            app_state_epoch,
            ..
        }
        | UsagePricingReq::LoadLogDetail {
            request_id,
            generation,
            app_state_epoch,
            ..
        } => Some(UsageLogRequestToken {
            request_id: *request_id,
            generation: *generation,
            app_state_epoch: *app_state_epoch,
        }),
        UsagePricingReq::DropState { .. } => None,
    }
}

#[derive(Default)]
struct UsageLogQueryCompletion {
    next: Option<UsagePricingReq>,
    drop_acks: Vec<mpsc::Sender<()>>,
}

impl UsageLogQueryState {
    fn enqueue(
        &mut self,
        req: UsagePricingReq,
    ) -> (Option<UsagePricingReq>, Option<UsagePricingReq>) {
        let token = usage_log_request_token(&req)
            .expect("only query requests enter the usage log query state");
        if !self.running {
            self.running = true;
            self.running_token = Some(token);
            self.cancel_requested = false;
            return (Some(req), None);
        }

        self.cancel_running();
        let superseded = self.pending.replace(req);
        (None, superseded)
    }

    fn set_running_cancel_token(
        &mut self,
        request_token: UsageLogRequestToken,
        cancel_token: Arc<AtomicBool>,
    ) -> bool {
        if self.running_token != Some(request_token) {
            cancel_token.store(true, Ordering::Release);
            return false;
        }
        if self.cancel_requested {
            cancel_token.store(true, Ordering::Release);
        }
        self.running_cancel = Some(cancel_token);
        true
    }

    fn set_running_interrupt_handle(
        &mut self,
        request_token: UsageLogRequestToken,
        handle: rusqlite::InterruptHandle,
    ) -> bool {
        if self.running_token != Some(request_token) {
            handle.interrupt();
            return false;
        }
        if self.cancel_requested {
            handle.interrupt();
        }
        self.running_interrupt = Some(handle);
        true
    }

    fn cancel_running(&mut self) {
        self.cancel_requested = true;
        if let Some(token) = &self.running_cancel {
            token.store(true, Ordering::Release);
        }
        if let Some(handle) = &self.running_interrupt {
            handle.interrupt();
        }
    }

    fn complete(&mut self, request_token: UsageLogRequestToken) -> UsageLogQueryCompletion {
        if self.running_token != Some(request_token) {
            return UsageLogQueryCompletion::default();
        }
        self.running_interrupt = None;
        self.running_cancel = None;
        let drop_acks = self.drop_acks.drain(..).collect();
        if let Some(next) = self.pending.take() {
            self.running_token = usage_log_request_token(&next);
            self.cancel_requested = false;
            return UsageLogQueryCompletion {
                next: Some(next),
                drop_acks,
            };
        }
        self.running = false;
        self.running_token = None;
        self.cancel_requested = false;
        UsageLogQueryCompletion {
            next: None,
            drop_acks,
        }
    }

    fn clear_and_ack_when_idle(&mut self, ack: mpsc::Sender<()>) -> Option<UsagePricingReq> {
        let superseded = self.pending.take();
        if self.running {
            self.cancel_running();
            self.drop_acks.push(ack);
        } else {
            let _ = ack.send(());
        }
        superseded
    }
}

struct UsageLogQueryRunner {
    state: Arc<Mutex<UsageLogQueryState>>,
    tx: mpsc::Sender<UsagePricingMsg>,
}

impl UsageLogQueryRunner {
    fn new(tx: mpsc::Sender<UsagePricingMsg>) -> Self {
        Self {
            state: Arc::new(Mutex::new(UsageLogQueryState::default())),
            tx,
        }
    }

    fn dispatch(&self, req: UsagePricingReq) {
        let (launch, superseded) = match self.state.lock() {
            Ok(mut state) => state.enqueue(req),
            Err(_) => {
                send_usage_log_req_error(
                    &req,
                    &self.tx,
                    "usage log worker state is unavailable".to_string(),
                );
                return;
            }
        };
        if let Some(superseded) = superseded {
            send_usage_log_req_cancelled(&superseded, &self.tx);
        }
        if let Some(req) = launch {
            spawn_usage_log_query(req, Arc::clone(&self.state), self.tx.clone());
        }
    }

    fn drop_state(&self, ack: mpsc::Sender<()>) {
        let superseded = match self.state.lock() {
            Ok(mut state) => state.clear_and_ack_when_idle(ack),
            Err(_) => {
                let _ = ack.send(());
                None
            }
        };
        if let Some(superseded) = superseded {
            send_usage_log_req_cancelled(&superseded, &self.tx);
        }
    }
}

fn usage_log_worker_loop(rx: mpsc::Receiver<UsagePricingReq>, tx: mpsc::Sender<UsagePricingMsg>) {
    let runner = UsageLogQueryRunner::new(tx);
    while let Ok(req) = rx.recv() {
        match req {
            UsagePricingReq::DropState { ack } => runner.drop_state(ack),
            req @ (UsagePricingReq::Load { .. }
            | UsagePricingReq::LoadLogPage { .. }
            | UsagePricingReq::LoadLogDetail { .. }) => runner.dispatch(req),
        }
    }
}

fn spawn_usage_log_query(
    req: UsagePricingReq,
    state: Arc<Mutex<UsageLogQueryState>>,
    tx: mpsc::Sender<UsagePricingMsg>,
) {
    let request_token = usage_log_request_token(&req)
        .expect("only query requests are spawned by the usage log query runner");
    let cancel_token = Arc::new(AtomicBool::new(false));
    let registration = match state.lock() {
        Ok(mut guard) => {
            Ok(guard.set_running_cancel_token(request_token, Arc::clone(&cancel_token)))
        }
        Err(_) => Err(()),
    };
    match registration {
        Ok(true) => {}
        Ok(false) => {
            send_usage_log_req_cancelled(&req, &tx);
            finish_usage_log_query(state, tx, request_token);
            return;
        }
        Err(()) => {
            send_usage_log_req_error(
                &req,
                &tx,
                "usage log worker state is unavailable".to_string(),
            );
            finish_usage_log_query(state, tx, request_token);
            return;
        }
    }

    let fallback_req = req.clone();
    let fallback_state = Arc::clone(&state);
    let fallback_tx = tx.clone();
    let spawn = std::thread::Builder::new()
        .name("cc-switch-usage-log-query".to_string())
        .spawn(move || {
            let interrupt_state = Arc::clone(&state);
            let panic_req = req.clone();
            let panic_tx = tx.clone();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                handle_usage_log_req_with_cancel(req, &tx, Arc::clone(&cancel_token), |handle| {
                    if let Ok(mut state) = interrupt_state.lock() {
                        state.set_running_interrupt_handle(request_token, handle);
                    }
                });
            }));
            if result.is_err() {
                send_usage_log_req_error(
                    &panic_req,
                    &panic_tx,
                    "usage log worker panicked".to_string(),
                );
            }
            finish_usage_log_query(state, tx, request_token);
        });

    if let Err(err) = spawn {
        send_usage_log_req_error(
            &fallback_req,
            &fallback_tx,
            format!("failed to spawn usage log worker: {err}"),
        );
        finish_usage_log_query(fallback_state, fallback_tx, request_token);
    }
}

fn finish_usage_log_query(
    state: Arc<Mutex<UsageLogQueryState>>,
    tx: mpsc::Sender<UsagePricingMsg>,
    request_token: UsageLogRequestToken,
) {
    let completion = match state.lock() {
        Ok(mut state) => state.complete(request_token),
        Err(_) => UsageLogQueryCompletion::default(),
    };
    for ack in completion.drop_acks {
        let _ = ack.send(());
    }
    if let Some(next) = completion.next {
        spawn_usage_log_query(next, state, tx);
    }
}

fn handle_usage_log_req_with_cancel<F>(
    req: UsagePricingReq,
    tx: &mpsc::Sender<UsagePricingMsg>,
    cancel_token: Arc<AtomicBool>,
    on_interrupt_handle: F,
) where
    F: FnOnce(rusqlite::InterruptHandle),
{
    if cancel_token.load(Ordering::Acquire) {
        send_usage_log_req_cancelled(&req, tx);
        return;
    }

    let db = match crate::Database::open_readonly_current_schema() {
        Ok(db) => db,
        Err(err) => {
            if cancel_token.load(Ordering::Acquire) {
                send_usage_log_req_cancelled(&req, tx);
            } else {
                send_usage_log_req_error(&req, tx, err.to_string());
            }
            return;
        }
    };
    let interrupt = match db.conn.lock() {
        Ok(conn) => install_usage_query_cancellation(&conn, Arc::clone(&cancel_token)),
        Err(err) => {
            send_usage_log_req_error(&req, tx, err.to_string());
            return;
        }
    };
    on_interrupt_handle(interrupt);

    match req {
        UsagePricingReq::Load {
            request_id,
            generation,
            app_state_epoch,
            app_type,
            range,
        } => {
            let result = crate::cli::tui::data::load_usage_log_page_from_database(
                &db,
                &app_type,
                range,
                None,
                crate::cli::tui::data::UsageLogPageDirection::Older,
                crate::cli::tui::data::USAGE_LOG_PAGE_SIZE,
            );
            let result = usage_log_result_after_query(result, &cancel_token);
            let _ = tx.send(UsagePricingMsg::LogHeadLoaded {
                request_id,
                generation,
                app_state_epoch,
                app_type,
                range,
                result,
            });
        }
        UsagePricingReq::LoadLogPage {
            request_id,
            generation,
            app_state_epoch,
            app_type,
            range,
            page,
            cursor,
            direction,
            limit,
        } => {
            let result = crate::cli::tui::data::load_usage_log_page_from_database(
                &db,
                &app_type,
                range,
                Some(&cursor),
                direction,
                limit,
            );
            let result = usage_log_result_after_query(result, &cancel_token);
            let _ = tx.send(UsagePricingMsg::LogPageLoaded {
                request_id,
                generation,
                app_state_epoch,
                app_type,
                range,
                page,
                direction,
                result,
            });
        }
        UsagePricingReq::LoadLogDetail {
            request_id,
            generation,
            app_state_epoch,
            app_type,
            range,
            log_rowid,
        } => {
            let result = crate::cli::tui::data::load_usage_log_detail_from_database(
                &db, &app_type, range, log_rowid,
            );
            let result = usage_log_result_after_query(result, &cancel_token);
            let _ = tx.send(UsagePricingMsg::LogDetailLoaded {
                request_id,
                generation,
                app_state_epoch,
                app_type,
                range,
                log_rowid,
                result,
            });
        }
        UsagePricingReq::DropState { .. } => {}
    }
}

fn usage_log_result_after_query<T>(
    result: Result<T, AppError>,
    cancel_token: &AtomicBool,
) -> Result<T, UsageLogLoadError> {
    if cancel_token.load(Ordering::Acquire) {
        Err(UsageLogLoadError::Cancelled)
    } else {
        result.map_err(|err| UsageLogLoadError::Failed(err.to_string()))
    }
}

fn send_usage_log_req_cancelled(req: &UsagePricingReq, tx: &mpsc::Sender<UsagePricingMsg>) {
    send_usage_log_req_result_error(req, tx, UsageLogLoadError::Cancelled);
}

fn send_usage_log_req_error(
    req: &UsagePricingReq,
    tx: &mpsc::Sender<UsagePricingMsg>,
    error: String,
) {
    send_usage_log_req_result_error(req, tx, UsageLogLoadError::Failed(error));
}

fn send_usage_log_req_result_error(
    req: &UsagePricingReq,
    tx: &mpsc::Sender<UsagePricingMsg>,
    error: UsageLogLoadError,
) {
    match req {
        UsagePricingReq::Load {
            request_id,
            generation,
            app_state_epoch,
            app_type,
            range,
        } => {
            let _ = tx.send(UsagePricingMsg::LogHeadLoaded {
                request_id: *request_id,
                generation: *generation,
                app_state_epoch: *app_state_epoch,
                app_type: app_type.clone(),
                range: *range,
                result: Err(error),
            });
        }
        UsagePricingReq::LoadLogPage {
            request_id,
            generation,
            app_state_epoch,
            app_type,
            range,
            page,
            direction,
            ..
        } => {
            let _ = tx.send(UsagePricingMsg::LogPageLoaded {
                request_id: *request_id,
                generation: *generation,
                app_state_epoch: *app_state_epoch,
                app_type: app_type.clone(),
                range: *range,
                page: *page,
                direction: *direction,
                result: Err(error),
            });
        }
        UsagePricingReq::LoadLogDetail {
            request_id,
            generation,
            app_state_epoch,
            app_type,
            range,
            log_rowid,
        } => {
            let _ = tx.send(UsagePricingMsg::LogDetailLoaded {
                request_id: *request_id,
                generation: *generation,
                app_state_epoch: *app_state_epoch,
                app_type: app_type.clone(),
                range: *range,
                log_rowid: *log_rowid,
                result: Err(error),
            });
        }
        UsagePricingReq::DropState { .. } => {}
    }
}

fn usage_pricing_worker_loop(
    rx: mpsc::Receiver<UsagePricingReq>,
    tx: mpsc::Sender<UsagePricingMsg>,
) {
    let mut deferred = VecDeque::new();
    let aggregate_runner = UsagePricingAggregateRunner::new(tx.clone());

    while let Some(req) = deferred.pop_front().or_else(|| rx.recv().ok()) {
        match req {
            UsagePricingReq::DropState { ack } => {
                aggregate_runner.drop_state(ack);
            }
            req @ UsagePricingReq::Load { .. } => {
                let mut backlog = VecDeque::from([req]);
                drain_latest_by_key(&mut backlog, &mut deferred, &rx, usage_pricing_req_key);

                while let Some(req) = backlog.pop_front() {
                    // Every aggregate, including the fixed Today/7d/30d
                    // snapshot, runs on a disposable cancellable connection.
                    // This keeps the queue responsive to DropState and prevents
                    // a stale AppState from surviving an invalidation barrier.
                    aggregate_runner.dispatch(req);
                    drain_latest_by_key(&mut backlog, &mut deferred, &rx, usage_pricing_req_key);
                }
            }
            UsagePricingReq::LoadLogPage { .. } | UsagePricingReq::LoadLogDetail { .. } => {}
        }
    }
}

fn handle_usage_pricing_aggregate_req_with_cancel<F>(
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
    let result = if cancel_token.load(Ordering::Acquire) {
        Err(UsagePricingLoadError::Cancelled)
    } else {
        let loaded = load_snapshot_state().and_then(|state| {
            if cancel_token.load(Ordering::Acquire) {
                return Err(AppError::Message("usage aggregate cancelled".to_string()));
            }
            let handle = {
                let conn = state.db.conn.lock().map_err(AppError::from)?;
                install_usage_query_cancellation(&conn, Arc::clone(&cancel_token))
            };
            on_interrupt_handle(handle);
            load_usage_pricing_data_from_state_for_range(&state, &app_type, range)
        });
        if cancel_token.load(Ordering::Acquire) {
            Err(UsagePricingLoadError::Cancelled)
        } else {
            loaded.map_err(|err| UsagePricingLoadError::Failed(err.to_string()))
        }
    };

    let _ = tx.send(UsagePricingMsg::Loaded {
        request_id,
        generation,
        app_state_epoch,
        app_type,
        range,
        result: Box::new(result),
    });
}

fn install_usage_query_cancellation(
    conn: &rusqlite::Connection,
    cancel_token: Arc<AtomicBool>,
) -> rusqlite::InterruptHandle {
    conn.progress_handler(1_000, Some(move || cancel_token.load(Ordering::Acquire)));
    conn.get_interrupt_handle()
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
        result: Box::new(Err(UsagePricingLoadError::Failed(err))),
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

    struct LocalEnvTaskDropSignal(mpsc::Sender<()>);

    impl Drop for LocalEnvTaskDropSignal {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }

    #[test]
    fn dropping_local_env_system_aborts_active_batch_and_joins_worker() {
        let (req_tx, req_rx) = tokio::sync::mpsc::unbounded_channel();
        let (result_tx, result_rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build local environment test runtime");
            runtime.block_on(local_env_worker_loop_with(
                req_rx,
                result_tx,
                move |_cancellation, _batch_tx, _generation| {
                    let started_tx = started_tx.clone();
                    let drop_signal = LocalEnvTaskDropSignal(dropped_tx.clone());
                    async move {
                        let _drop_signal = drop_signal;
                        let _ = started_tx.send(());
                        std::future::pending::<()>().await;
                    }
                },
            ));
        });
        let system = LocalEnvSystem {
            req_tx,
            result_rx,
            _handle: Some(handle),
        };
        system
            .req_tx
            .send(LocalEnvReq::Refresh { generation: 1 })
            .expect("start local environment batch");
        started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("local environment batch should start");

        let started = std::time::Instant::now();
        drop(system);

        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "local environment shutdown should not wait for the probe deadline"
        );
        dropped_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("active local environment task should be dropped before shutdown returns");
    }

    #[test]
    fn delete_acknowledgement_is_queued_before_purge_can_run() {
        let (tx, rx) = mpsc::channel();
        acknowledge_delete_then_launch_purge(
            &tx,
            41,
            "claude:session:/tmp/session.jsonl".to_string(),
            Ok(()),
            |purge_tx, request_id, key| {
                purge_tx
                    .send(SessionMsg::ManifestsPurged {
                        request_id,
                        key,
                        base: Vec::new(),
                    })
                    .map_err(|error| error.to_string())
            },
        )
        .expect("acknowledge deletion");

        assert!(matches!(
            rx.recv().expect("delete acknowledgement"),
            SessionMsg::DeleteFinished { request_id: 41, .. }
        ));
        assert!(matches!(
            rx.recv().expect("purge completion"),
            SessionMsg::ManifestsPurged { request_id: 41, .. }
        ));
    }

    #[test]
    fn purge_spawn_failure_is_reported_after_delete_acknowledgement() {
        let (tx, rx) = mpsc::channel();
        acknowledge_delete_then_launch_purge(
            &tx,
            42,
            "claude:session:/tmp/session.jsonl".to_string(),
            Ok(()),
            |_, _, _| Err("injected thread spawn failure".to_string()),
        )
        .expect("acknowledge deletion and report purge failure");

        assert!(matches!(
            rx.recv().expect("delete acknowledgement"),
            SessionMsg::DeleteFinished { request_id: 42, .. }
        ));
        assert!(matches!(
            rx.recv().expect("empty purge completion"),
            SessionMsg::ManifestsPurged {
                request_id: 42,
                base,
                ..
            } if base.is_empty()
        ));
    }

    #[test]
    fn session_project_filter_runs_off_thread_and_returns_raw_option_indices() {
        let manifest_dir = tempfile::tempdir().expect("manifest fixture directory");
        let manifest_store = crate::session_manager::paged_manifest::PagedManifestStore::open_at(
            manifest_dir.path(),
        )
        .expect("manifest fixture store");
        let mut builder = manifest_store
            .begin_build("claude")
            .expect("manifest fixture builder");
        for (session_id, project_dir) in [("alpha", "/repo/alpha"), ("beta", "/repo/beta")] {
            builder
                .push(crate::session_manager::SessionMeta {
                    provider_id: "claude".to_string(),
                    session_id: session_id.to_string(),
                    project_dir: Some(project_dir.to_string()),
                    source_path: Some(format!("/tmp/{session_id}.jsonl")),
                    ..crate::session_manager::SessionMeta::default()
                })
                .expect("manifest fixture row");
        }
        let published = builder.publish().expect("publish fixture");
        let catalog = crate::session_manager::project_scope::aggregate_project_directories(
            &published.reader,
            &|| false,
        )
        .expect("project catalog");
        let (tx, rx) = mpsc::channel();

        handle_session_req(
            SessionReq::ProjectFilter {
                request_id: 41,
                scope_epoch: 7,
                scope: "claude".to_string(),
                base_generation: published.generation.clone(),
                query: "beta".to_string(),
                catalog: Arc::new(catalog),
                project_offset: 1,
                fixed_matches: vec![0],
                trailing_matches: vec![3],
            },
            &tx,
        )
        .expect("spawn project filter");

        assert!(matches!(
            rx.recv_timeout(std::time::Duration::from_secs(1))
                .expect("project filter result"),
            SessionMsg::ProjectFilterBuilt {
                request_id: 41,
                scope_epoch: 7,
                scope,
                query,
                result: Ok(indices),
                ..
            } if scope == "claude" && query == "beta" && indices == vec![0, 2, 3]
        ));
    }

    #[test]
    fn session_search_has_an_independent_lane_and_cancel_generation() {
        let manifest_dir = tempfile::tempdir().expect("manifest fixture directory");
        let manifest_store = crate::session_manager::paged_manifest::PagedManifestStore::open_at(
            manifest_dir.path(),
        )
        .expect("manifest fixture store");
        let mut manifest_builder = manifest_store
            .begin_build("claude")
            .expect("manifest fixture builder");
        manifest_builder
            .push(crate::session_manager::SessionMeta {
                provider_id: "claude".to_string(),
                session_id: "base".to_string(),
                source_path: Some("/tmp/base.jsonl".to_string()),
                ..crate::session_manager::SessionMeta::default()
            })
            .expect("manifest fixture row");
        let published = manifest_builder.publish().expect("publish fixture");
        let base_reader = published.reader.clone();
        let base_generation = published.generation;
        let (req_tx, req_rx) = mpsc::channel();
        let (control_tx, control_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let message_generation = Arc::new(AtomicU64::new(0));
        let (search_started_tx, search_started_rx) = mpsc::channel();
        let (search_cancelled_tx, search_cancelled_rx) = mpsc::channel();

        let control_result_tx = result_tx.clone();
        let control =
            std::thread::spawn(move || session_control_worker_loop(control_rx, control_result_tx));
        let dispatcher_generation = Arc::clone(&generation);
        let dispatcher = std::thread::spawn(move || {
            session_dispatcher_loop_with(
                req_rx,
                control_tx,
                result_tx,
                dispatcher_generation,
                message_generation,
                Arc::new(AtomicU64::new(0)),
                move |job| {
                    let started = search_started_tx.clone();
                    let cancelled = search_cancelled_tx.clone();
                    std::thread::spawn(move || {
                        let _ = started.send(());
                        while job.current_generation.load(Ordering::Acquire) == job.generation {
                            std::thread::yield_now();
                        }
                        let _ = cancelled.send(());
                    });
                    Ok(())
                },
                |job| {
                    job.result_tx
                        .send(SessionMsg::MessagesLoaded {
                            request_id: job.request_id,
                            key: job.key,
                            result: Err("unsupported provider".to_string()),
                        })
                        .map_err(|error| error.to_string())
                },
                |_| Ok(()),
                |_| Ok(()),
                |_| Ok(()),
            )
        });

        req_tx
            .send(SessionReq::Search {
                request_id: 1,
                scope_epoch: 1,
                view: crate::session_manager::project_scope::SessionViewSpec::all_projects(
                    "needle",
                ),
                base: crate::cli::tui::app::SessionPageToken {
                    scope_epoch: 1,
                    view_epoch: 1,
                    source: crate::cli::tui::app::SessionPageSource::Base,
                    scope: "claude".to_string(),
                    generation: base_generation,
                },
                base_reader,
                query_namespace:
                    crate::session_manager::paged_manifest::QueryManifestNamespace::for_test(
                        "tui-worker-search",
                    ),
            })
            .expect("queue blocking search");
        search_started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("search lane starts");

        req_tx
            .send(SessionReq::LoadMessages {
                request_id: 2,
                key: "test".to_string(),
                provider_id: "unsupported".to_string(),
                source_path: String::new(),
            })
            .expect("queue control request while search runs");
        let control_result = result_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("control lane remains responsive");
        assert!(matches!(
            control_result,
            SessionMsg::MessagesLoaded { request_id: 2, .. }
        ));

        req_tx
            .send(SessionReq::CancelSearch)
            .expect("cancel active search");
        search_cancelled_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("search observes generation cancellation");

        drop(req_tx);
        dispatcher.join().expect("dispatcher exits");
        control.join().expect("control worker exits");
    }

    #[test]
    fn session_manifest_locate_lane_coalesces_to_latest_generation() {
        let manifest_dir = tempfile::tempdir().expect("manifest fixture directory");
        let manifest_store = crate::session_manager::paged_manifest::PagedManifestStore::open_at(
            manifest_dir.path(),
        )
        .expect("manifest fixture store");
        let mut builder = manifest_store
            .begin_build("claude")
            .expect("manifest fixture builder");
        builder
            .push(crate::session_manager::SessionMeta {
                provider_id: "claude".to_string(),
                session_id: "selected".to_string(),
                source_path: Some("/tmp/selected.jsonl".to_string()),
                ..crate::session_manager::SessionMeta::default()
            })
            .expect("manifest fixture row");
        let published = builder.publish().expect("publish fixture");
        let generation = published.generation;
        let reader = published.reader;
        let current_generation = Arc::new(AtomicU64::new(3));
        let (job_tx, job_rx) = session_locate_mailbox();
        let (result_tx, result_rx) = mpsc::channel();

        for request_id in 1..=3 {
            job_tx
                .submit(SessionLocateJob {
                    request_id,
                    scope_epoch: 1,
                    source: crate::cli::tui::app::SessionPageSource::Base,
                    scope: "claude".to_string(),
                    generation: generation.clone(),
                    fallback_absolute: 0,
                    anchor: None,
                    reader: reader.clone(),
                    locate_generation: request_id,
                    current_generation: Arc::clone(&current_generation),
                    result_tx: result_tx.clone(),
                })
                .expect("queue manifest location");
        }
        drop(job_tx);
        drop(result_tx);

        let worker = std::thread::spawn(move || session_locate_worker_loop(job_rx));
        assert!(matches!(
            result_rx.recv().expect("latest manifest location"),
            SessionMsg::ManifestLocated { request_id: 3, .. }
        ));
        assert!(
            result_rx.recv().is_err(),
            "older jobs must not publish results"
        );
        worker.join().expect("locate worker exits");
    }

    #[test]
    fn session_manifest_locator_uses_bounded_neighbourhood_then_fallback() {
        let manifest_dir = tempfile::tempdir().expect("manifest fixture directory");
        let manifest_store = crate::session_manager::paged_manifest::PagedManifestStore::open_at(
            manifest_dir.path(),
        )
        .expect("manifest fixture store");
        let mut builder = manifest_store
            .begin_build("claude")
            .expect("manifest fixture builder");
        for index in 0..401 {
            builder
                .push(crate::session_manager::SessionMeta {
                    provider_id: "claude".to_string(),
                    session_id: format!("session-{index:03}"),
                    source_path: Some(format!("/tmp/session-{index:03}.jsonl")),
                    last_active_at: Some(index as i64),
                    ..crate::session_manager::SessionMeta::default()
                })
                .expect("manifest fixture row");
        }
        let published = builder.publish().expect("publish fixture");
        let reader = published.reader;
        let distant_page = reader.load_page(4).expect("distant manifest page");
        let distant_anchor = crate::cli::tui::app::SessionRowIdentity::capture(
            distant_page.rows.first().expect("distant row"),
        );

        let location = resolve_session_manifest_selection(
            crate::cli::tui::app::SessionPageSource::Base,
            "claude",
            reader.generation(),
            0,
            Some(&distant_anchor),
            &reader,
            &|| false,
        )
        .expect("bounded manifest location")
        .expect("location is not cancelled");

        assert_eq!(location.0.page_index, 0);
        assert_eq!(location.1, 0);
        assert!(!distant_anchor.matches(&location.0.rows[location.1]));
    }

    #[test]
    fn session_manifest_locator_observes_cancellation_between_page_reads() {
        let manifest_dir = tempfile::tempdir().expect("manifest fixture directory");
        let manifest_store = crate::session_manager::paged_manifest::PagedManifestStore::open_at(
            manifest_dir.path(),
        )
        .expect("manifest fixture store");
        let published = manifest_store
            .begin_build("claude")
            .expect("manifest fixture builder")
            .publish()
            .expect("publish fixture");
        let checks = std::cell::Cell::new(0usize);
        let cancelled_after_first_read = || {
            let check = checks.get();
            checks.set(check + 1);
            check >= 2
        };

        let location = resolve_session_manifest_selection(
            crate::cli::tui::app::SessionPageSource::Base,
            "claude",
            published.reader.generation(),
            0,
            None,
            &published.reader,
            &cancelled_after_first_read,
        )
        .expect("cancellation is not an error");

        assert!(location.is_none());
    }

    #[test]
    fn session_messages_are_latest_wins_and_never_enter_the_delete_lane() {
        let (req_tx, req_rx) = mpsc::channel();
        let (control_tx, control_rx) = mpsc::channel();
        let (result_tx, _result_rx) = mpsc::channel();
        let search_generation = Arc::new(AtomicU64::new(0));
        let message_generation = Arc::new(AtomicU64::new(0));
        let dispatcher_message_generation = Arc::clone(&message_generation);
        let (message_job_tx, message_job_rx) = mpsc::channel();

        let dispatcher = std::thread::spawn(move || {
            session_dispatcher_loop_with(
                req_rx,
                control_tx,
                result_tx,
                search_generation,
                dispatcher_message_generation,
                Arc::new(AtomicU64::new(0)),
                |_| Ok(()),
                move |job| message_job_tx.send(job).map_err(|error| error.to_string()),
                |_| Ok(()),
                |_| Ok(()),
                |_| Ok(()),
            )
        });

        for request_id in 1..=2 {
            req_tx
                .send(SessionReq::LoadMessages {
                    request_id,
                    key: format!("session-{request_id}"),
                    provider_id: "codex".to_string(),
                    source_path: format!("/tmp/session-{request_id}.jsonl"),
                })
                .expect("queue message preview");
        }
        req_tx
            .send(SessionReq::Delete {
                request_id: 3,
                key: "delete".to_string(),
                provider_id: "codex".to_string(),
                session_id: "delete".to_string(),
                source_path: "/tmp/delete.jsonl".to_string(),
            })
            .expect("queue delete");

        let first = message_job_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("first message job");
        let second = message_job_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("second message job");
        assert!(matches!(
            control_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("delete reaches control lane"),
            SessionReq::Delete { request_id: 3, .. }
        ));
        assert_ne!(
            first.current_generation.load(Ordering::Acquire),
            first.generation,
            "the older message read must be cancelled by the newer request"
        );
        assert_eq!(
            second.current_generation.load(Ordering::Acquire),
            second.generation,
            "the newest message read remains current"
        );

        req_tx
            .send(SessionReq::CancelMessages)
            .expect("cancel message preview");
        req_tx
            .send(SessionReq::Delete {
                request_id: 4,
                key: "barrier".to_string(),
                provider_id: "codex".to_string(),
                session_id: "barrier".to_string(),
                source_path: "/tmp/barrier.jsonl".to_string(),
            })
            .expect("queue dispatcher barrier");
        assert!(matches!(
            control_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("barrier reaches control lane"),
            SessionReq::Delete { request_id: 4, .. }
        ));
        assert_ne!(
            second.current_generation.load(Ordering::Acquire),
            second.generation,
            "closing detail must cancel the newest read"
        );

        drop(req_tx);
        dispatcher.join().expect("dispatcher exits");
    }

    #[test]
    fn session_page_load_bypasses_a_blocked_control_lane() {
        let manifest_dir = tempfile::tempdir().expect("manifest fixture directory");
        let manifest_store = crate::session_manager::paged_manifest::PagedManifestStore::open_at(
            manifest_dir.path(),
        )
        .expect("manifest fixture store");
        let published = manifest_store
            .begin_build("codex")
            .expect("manifest fixture builder")
            .publish()
            .expect("publish fixture");
        let page_reader = published.reader.clone();
        let page_generation = published.generation;
        let (req_tx, req_rx) = mpsc::channel();
        let (control_tx, control_rx) = mpsc::channel();
        let (result_tx, _result_rx) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let message_generation = Arc::new(AtomicU64::new(0));
        let (page_started_tx, page_started_rx) = mpsc::channel();
        let (refresh_started_tx, refresh_started_rx) = mpsc::channel();

        // Keep the control receiver alive without consuming it. A message load
        // queued first therefore represents a control lane blocked on a large
        // transcript, while the dispatcher must still launch the page job.
        let dispatcher = std::thread::spawn(move || {
            session_dispatcher_loop_with(
                req_rx,
                control_tx,
                result_tx,
                generation,
                message_generation,
                Arc::new(AtomicU64::new(0)),
                |_| Ok(()),
                |_| Ok(()),
                move |job| {
                    page_started_tx
                        .send((job.request_id, job.page))
                        .map_err(|error| error.to_string())
                },
                move |job| {
                    refresh_started_tx
                        .send((job.request_id, job.scope_epoch))
                        .map_err(|error| error.to_string())
                },
                |_| Ok(()),
            )
        });

        req_tx
            .send(SessionReq::LoadMessages {
                request_id: 1,
                key: "large".to_string(),
                provider_id: "codex".to_string(),
                source_path: "/tmp/large.jsonl".to_string(),
            })
            .expect("queue blocked message load");
        req_tx
            .send(SessionReq::LoadPage {
                request_id: 2,
                token: crate::cli::tui::app::SessionPageToken {
                    scope_epoch: 1,
                    view_epoch: 1,
                    source: crate::cli::tui::app::SessionPageSource::Base,
                    scope: "codex".to_string(),
                    generation: page_generation,
                },
                page: 3,
                reader: page_reader,
            })
            .expect("queue page load");
        req_tx
            .send(SessionReq::Refresh {
                request_id: 3,
                scope_epoch: 7,
                provider_id: "codex".to_string(),
                force: false,
            })
            .expect("queue refresh");

        assert_eq!(
            page_started_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("page lane should not wait for control"),
            (2, 3)
        );
        assert_eq!(
            refresh_started_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("refresh lane should not wait for control"),
            (3, 7)
        );

        drop(req_tx);
        dispatcher.join().expect("dispatcher exits");
        drop(control_rx);
    }

    #[test]
    fn session_worker_preview_selects_only_true_recency_top_k() {
        let rows: Vec<_> = (0..1_000)
            .map(|timestamp| crate::session_manager::SessionMeta {
                provider_id: "claude".to_string(),
                session_id: format!("s-{timestamp}"),
                last_active_at: Some(timestamp),
                ..crate::session_manager::SessionMeta::default()
            })
            .collect();

        let mut preview = crate::session_manager::paged_manifest::BoundedRecencyPreview::new(
            crate::session_manager::paged_manifest::PAGE_SIZE,
        );
        for row in rows {
            preview.push(row);
        }
        let preview = preview.snapshot();

        assert_eq!(
            preview.len(),
            crate::session_manager::paged_manifest::PAGE_SIZE
        );
        assert_eq!(preview[0].session_id, "s-999");
        assert_eq!(preview[99].session_id, "s-900");
    }

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
            scope_epoch: 2,
            provider_id: "claude".to_string(),
            force: false,
        })
        .expect("queue refresh");
        drop(tx);

        let drained = drain_session_reqs_for_test(
            SessionReq::Refresh {
                request_id: 1,
                scope_epoch: 1,
                provider_id: "claude".to_string(),
                force: false,
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
    fn usage_log_requests_bypass_a_blocked_aggregate_worker() {
        let (public_tx, public_rx) = mpsc::channel();
        let (aggregate_tx, aggregate_rx) = mpsc::channel();
        let (log_tx, log_rx) = mpsc::channel();
        let dispatcher = std::thread::spawn(move || {
            usage_pricing_dispatcher_loop(public_rx, aggregate_tx, log_tx);
        });

        public_tx
            .send(UsagePricingReq::Load {
                request_id: 1,
                generation: 4,
                app_state_epoch: 2,
                app_type: AppType::Claude,
                range: UsageRangePreset::SevenDays,
            })
            .expect("queue aggregate and head");
        assert!(matches!(
            log_rx.recv().expect("head routed to log worker"),
            UsagePricingReq::Load { request_id: 1, .. }
        ));

        // Hold the aggregate request as if its worker were blocked inside a
        // long-running query. The dispatcher and log queue must stay live.
        let _blocked_aggregate = aggregate_rx.recv().expect("aggregate routed");
        public_tx
            .send(UsagePricingReq::LoadLogPage {
                request_id: 2,
                generation: 4,
                app_state_epoch: 2,
                app_type: AppType::Claude,
                range: UsageRangePreset::SevenDays,
                page: 1,
                cursor: crate::cli::tui::data::UsageLogCursor {
                    created_at: 10,
                    rowid: 1,
                },
                direction: crate::cli::tui::data::UsageLogPageDirection::Older,
                limit: crate::cli::tui::data::USAGE_LOG_PAGE_SIZE,
            })
            .expect("queue page while aggregate is blocked");
        assert!(matches!(
            log_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("page must not wait for aggregate"),
            UsagePricingReq::LoadLogPage {
                request_id: 2,
                page: 1,
                ..
            }
        ));
        assert!(aggregate_rx.try_recv().is_err());

        drop(public_tx);
        dispatcher.join().expect("dispatcher exits");
    }

    #[test]
    fn usage_log_query_state_cancels_running_and_keeps_only_latest_generation() {
        let mut state = UsageLogQueryState::default();
        let running = UsagePricingReq::Load {
            request_id: 1,
            generation: 4,
            app_state_epoch: 2,
            app_type: AppType::Claude,
            range: UsageRangePreset::SevenDays,
        };
        assert!(state.enqueue(running).0.is_some());
        assert_eq!(
            state.running_token,
            Some(UsageLogRequestToken {
                request_id: 1,
                generation: 4,
                app_state_epoch: 2,
            })
        );

        let cancel = Arc::new(AtomicBool::new(false));
        let running_token = state.running_token.expect("running query token");
        assert!(state.set_running_cancel_token(running_token, Arc::clone(&cancel)));
        let pending = UsagePricingReq::LoadLogPage {
            request_id: 2,
            generation: 5,
            app_state_epoch: 2,
            app_type: AppType::Codex,
            range: UsageRangePreset::Today,
            page: 1,
            cursor: crate::cli::tui::data::UsageLogCursor {
                created_at: 10,
                rowid: 20,
            },
            direction: crate::cli::tui::data::UsageLogPageDirection::Older,
            limit: crate::cli::tui::data::USAGE_LOG_PAGE_SIZE,
        };
        assert!(state.enqueue(pending).0.is_none());
        assert!(cancel.load(Ordering::Acquire));

        let latest = UsagePricingReq::LoadLogDetail {
            request_id: 3,
            generation: 6,
            app_state_epoch: 3,
            app_type: AppType::OpenCode,
            range: UsageRangePreset::ThirtyDays,
            log_rowid: 99,
        };
        let (_, superseded) = state.enqueue(latest);
        assert!(matches!(
            superseded,
            Some(UsagePricingReq::LoadLogPage {
                request_id: 2,
                generation: 5,
                ..
            })
        ));

        let completion = state.complete(running_token);
        assert_eq!(
            state.running_token,
            Some(UsageLogRequestToken {
                request_id: 3,
                generation: 6,
                app_state_epoch: 3,
            })
        );
        assert!(matches!(
            completion.next,
            Some(UsagePricingReq::LoadLogDetail {
                request_id: 3,
                generation: 6,
                app_state_epoch: 3,
                log_rowid: 99,
                ..
            })
        ));
        let stale_completion = state.complete(running_token);
        assert!(stale_completion.next.is_none());
        assert_eq!(
            state.running_token,
            Some(UsageLogRequestToken {
                request_id: 3,
                generation: 6,
                app_state_epoch: 3,
            })
        );
    }

    #[test]
    fn usage_log_query_state_remembers_cancellation_before_sqlite_handle_registration() {
        let mut state = UsageLogQueryState::default();
        assert!(state
            .enqueue(UsagePricingReq::Load {
                request_id: 1,
                generation: 1,
                app_state_epoch: 1,
                app_type: AppType::Claude,
                range: UsageRangePreset::SevenDays,
            })
            .0
            .is_some());
        assert!(state
            .enqueue(UsagePricingReq::Load {
                request_id: 2,
                generation: 2,
                app_state_epoch: 1,
                app_type: AppType::Codex,
                range: UsageRangePreset::Today,
            })
            .0
            .is_none());

        let cancel = Arc::new(AtomicBool::new(false));
        let running_token = state.running_token.expect("running query token");
        assert!(state.set_running_cancel_token(running_token, Arc::clone(&cancel)));
        assert!(cancel.load(Ordering::Acquire));

        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        let interrupt = install_usage_query_cancellation(&conn, Arc::clone(&cancel));
        assert!(state.set_running_interrupt_handle(running_token, interrupt));
        let result = conn.query_row(
            "WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x + 1 FROM n WHERE x < 1000000000) SELECT sum(x) FROM n",
            [],
            |row| row.get::<_, i64>(0),
        );
        assert!(result.is_err());
    }

    #[test]
    fn usage_pricing_drain_keeps_latest_request_per_app() {
        let (tx, rx) = mpsc::channel();
        tx.send(UsagePricingReq::Load {
            request_id: 2,
            generation: 0,
            app_state_epoch: 0,
            app_type: AppType::Claude,
            range: UsageRangePreset::ThirtyDays,
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
            range: UsageRangePreset::Today,
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
                UsagePricingReq::LoadLogPage { .. } => {
                    panic!("unexpected LoadLogPage in backlog")
                }
                UsagePricingReq::LoadLogDetail { .. } => {
                    panic!("unexpected LoadLogDetail in backlog")
                }
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
                UsagePricingReq::LoadLogPage { .. } => {
                    panic!("unexpected LoadLogPage in backlog")
                }
                UsagePricingReq::LoadLogDetail { .. } => {
                    panic!("unexpected LoadLogDetail in backlog")
                }
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
                UsagePricingReq::LoadLogPage { .. } => {
                    panic!("unexpected LoadLogPage in backlog")
                }
                UsagePricingReq::LoadLogDetail { .. } => {
                    panic!("unexpected LoadLogDetail in backlog")
                }
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
    fn usage_pricing_aggregate_state_runs_one_and_keeps_latest_pending() {
        let older_custom = UsageRangePreset::Custom(crate::cli::tui::data::UsageCustomRange {
            start: 1_700_000_000,
            end: 1_700_086_399,
        });
        let newer_custom = UsageRangePreset::Custom(crate::cli::tui::data::UsageCustomRange {
            start: 1_700_086_400,
            end: 1_700_172_799,
        });
        let mut state = UsagePricingAggregateState::default();

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
            usage_pricing_aggregate_state_snapshot(&state),
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
            usage_pricing_aggregate_state_snapshot(&state),
            (true, Some(3), false, false, Vec::new())
        );

        let completion = state.complete();
        assert!(completion.next.is_none());
        assert!(completion.drop_acks.is_empty());
        assert_eq!(
            usage_pricing_aggregate_state_snapshot(&state),
            (false, None, false, true, Vec::new())
        );
    }

    #[test]
    fn usage_pricing_aggregate_state_remembers_drop_before_interrupt_handle() {
        let mut state = UsagePricingAggregateState::default();

        assert!(state
            .enqueue(UsagePricingReq::Load {
                request_id: 1,
                generation: 0,
                app_state_epoch: 0,
                app_type: AppType::Claude,
                range: UsageRangePreset::SevenDays,
            })
            .is_some());
        let cancel_token = Arc::new(AtomicBool::new(false));
        state.set_running_cancel_token(Arc::clone(&cancel_token));
        let (ack_tx, ack_rx) = mpsc::channel();
        state.clear_pending_and_ack_when_idle(ack_tx);

        assert!(cancel_token.load(Ordering::Relaxed));
        assert!(ack_rx.try_recv().is_err());
        assert_eq!(
            usage_pricing_aggregate_state_snapshot(&state),
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
            usage_pricing_aggregate_state_snapshot(&state),
            (false, None, false, true, Vec::new())
        );
    }

    #[test]
    fn usage_pricing_aggregate_runner_defers_drop_ack_until_worker_completes() {
        let (tx, _rx) = mpsc::channel();
        let runner = UsagePricingAggregateRunner::new(tx);

        {
            let mut state = runner.state.lock().expect("aggregate state should lock");
            assert!(state
                .enqueue(UsagePricingReq::Load {
                    request_id: 1,
                    generation: 0,
                    app_state_epoch: 0,
                    app_type: AppType::Claude,
                    range: UsageRangePreset::SevenDays,
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
            .expect("aggregate state should lock")
            .complete();
        assert!(completion.next.is_none());
        assert_eq!(completion.drop_acks.len(), 1);
        for ack in completion.drop_acks {
            let _ = ack.send(());
        }
        assert!(ack_rx.recv().is_ok());
    }

    #[test]
    fn sqlite_usage_query_cancellation_interrupts_a_fixed_aggregate_promptly() {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
        let cancel = Arc::new(AtomicBool::new(false));
        let interrupt = install_usage_query_cancellation(&conn, Arc::clone(&cancel));
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();

        let query = std::thread::spawn(move || {
            let _ = started_tx.send(());
            let result = conn.query_row(
                "WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x + 1 FROM n WHERE x < 1000000000) SELECT sum(x) FROM n",
                [],
                |row| row.get::<_, i64>(0),
            );
            let _ = done_tx.send(result);
        });

        started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("query thread starts");
        cancel.store(true, Ordering::Release);
        interrupt.interrupt();
        let result = done_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("interrupted query returns before the UI barrier timeout");
        assert!(result.is_err());
        query.join().expect("query thread exits");
    }
}
