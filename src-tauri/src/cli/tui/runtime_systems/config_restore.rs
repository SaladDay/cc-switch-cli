use std::sync::mpsc;

use crate::error::AppError;
use crate::services::ConfigService;

use super::super::data::UiData;
use super::types::{
    ConfigRestoreDone, ConfigRestoreKind, ConfigRestoreMsg, ConfigRestoreReq, ConfigRestoreSource,
    ConfigRestoreSystem, RestoreUiSnapshot,
};

pub(crate) fn start_config_restore_system() -> Result<ConfigRestoreSystem, AppError> {
    let (result_tx, result_rx) = mpsc::channel::<ConfigRestoreMsg>();
    let (req_tx, req_rx) = mpsc::channel::<ConfigRestoreReq>();

    let handle = std::thread::Builder::new()
        .name("cc-switch-config-restore".to_string())
        .spawn(move || config_restore_worker_loop(req_rx, result_tx))
        .map_err(|error| AppError::IoContext {
            context: "failed to spawn config restore worker thread".to_string(),
            source: error,
        })?;

    Ok(ConfigRestoreSystem {
        req_tx,
        result_rx,
        _handle: handle,
    })
}

fn config_restore_worker_loop(
    rx: mpsc::Receiver<ConfigRestoreReq>,
    tx: mpsc::Sender<ConfigRestoreMsg>,
) {
    while let Ok(req) = rx.recv() {
        let result = (|| {
            let app_type = req.app_type;
            let load_snapshot = |state: &crate::store::AppState| {
                UiData::load_fast_snapshot_from_state(state, &app_type)
                    .map(Box::new)
                    .map_err(|error| error.to_string())
            };

            let (kind, completion) = match req.source {
                ConfigRestoreSource::File(path) => (
                    ConfigRestoreKind::ImportFile,
                    ConfigService::import_config_from_path_and_then(&path, load_snapshot),
                ),
                ConfigRestoreSource::Backup(id) => (
                    ConfigRestoreKind::Backup,
                    ConfigService::restore_from_backup_id_and_then(&id, load_snapshot),
                ),
            };
            let completion = completion.map_err(|error| error.to_string())?;

            Ok(ConfigRestoreDone {
                kind,
                pre_backup_id: completion.pre_restore_backup_id,
                restored: RestoreUiSnapshot {
                    publication: completion.publication,
                    status: completion.status.clone(),
                    app_type,
                    loaded: completion.snapshot.unwrap_or_else(|| {
                        Err(completion
                            .status
                            .pending_retry()
                            .map(|pending| pending.message())
                            .unwrap_or_else(|| {
                                "restore completion snapshot is missing".to_string()
                            }))
                    }),
                },
            })
        })();

        if tx
            .send(ConfigRestoreMsg::Finished {
                request_id: req.request_id,
                result,
            })
            .is_err()
        {
            return;
        }
    }
}
