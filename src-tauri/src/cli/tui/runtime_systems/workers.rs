use std::sync::mpsc;

use crate::cli::i18n::texts;
use crate::error::AppError;
use crate::services::{SkillService, StreamCheckService, WebDavSyncService};
use crate::settings::{set_webdav_sync_settings, webdav_jianguoyun_preset};

use super::super::data::load_state;
use super::types::{
    fetch_provider_models_for_tui, model_fetch_strategy_for_field, LocalEnvMsg, LocalEnvReq,
    LocalEnvSystem, ModelFetchMsg, ModelFetchReq, ModelFetchSystem, ProxyMsg, ProxyReq,
    ProxySystem, QuotaMsg, QuotaReq, QuotaSystem, SkillsMsg, SkillsReq, SkillsSystem, SpeedtestMsg,
    SpeedtestSystem, StreamCheckMsg, StreamCheckReq, StreamCheckSystem, UpdateMsg, UpdateReq,
    UpdateSystem, WebDavDone, WebDavErr, WebDavMsg, WebDavReq, WebDavReqKind, WebDavSystem,
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
            field,
            claude_idx,
        } = req;
        let strategy = model_fetch_strategy_for_field(field);
        let result = rt
            .block_on(async {
                fetch_provider_models_for_tui(&base_url, api_key.as_deref(), strategy).await
            })
            .map_err(|e| e.to_string());

        let _ = tx.send(ModelFetchMsg::Finished {
            request_id,
            field,
            claude_idx,
            result,
        });
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
        let result = match &target.kind {
            crate::cli::tui::data::QuotaTargetKind::SubscriptionTool { tool } => {
                rt.block_on(crate::services::subscription::get_subscription_quota(tool))
            }
            crate::cli::tui::data::QuotaTargetKind::CodexOAuth { account_id } => Ok(rt.block_on(
                crate::services::CodexOAuthService::get_quota(account_id.as_deref()),
            )),
        };

        let _ = tx.send(QuotaMsg::Finished { target, result });
    }
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
                    SkillsReq::Discover { query } => {
                        let _ = tx.send(SkillsMsg::DiscoverFinished {
                            query,
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
                    SkillsReq::Discover { query } => {
                        let _ = tx.send(SkillsMsg::DiscoverFinished {
                            query,
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
            SkillsReq::Discover { query } => {
                let query_trimmed = query.trim().to_lowercase();
                let result = rt
                    .block_on(async { service.list_skills().await })
                    .map_err(|e| e.to_string())
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

                let _ = tx.send(SkillsMsg::DiscoverFinished { query, result });
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
