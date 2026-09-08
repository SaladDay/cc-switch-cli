//! MCP publications participate in the provider caller's final decision.

use cc_switch_core::OperationReceipt;
use cc_switch_store::McpTransactionGuard;

use super::{
    bulk::CatalogBatch,
    toggle::{observe_native, NativeToggle},
    *,
};
use crate::{
    config::ConfigWriteTarget,
    database::{shared_store_error, Database},
    gemini_config::operation::GeminiOperation,
};

enum NativeRecovery {
    App(AppType, Box<dyn NativeToggle>),
    Gemini(Option<OperationReceipt<ConfigWriteTarget>>),
}

pub(crate) struct ProviderMcpSync {
    initialized: Vec<AppType>,
    native: Vec<NativeRecovery>,
}

impl ProviderMcpSync {
    /// Decide initialization before any provider publication can create paths.
    pub(crate) fn new() -> Self {
        Self {
            initialized: McpService::supported_mcp_apps()
                .filter(crate::sync_policy::should_sync_live)
                .collect(),
            native: Vec::new(),
        }
    }

    /// The caller already holds the shared file lock and Gemini session. Do not
    /// open a second transaction or acquire that non-reentrant session again.
    pub(crate) fn sync(
        &mut self,
        transaction: &mut McpTransactionGuard<'_>,
        gemini: &mut GeminiOperation,
    ) -> Result<(), AppError> {
        let servers = transaction
            .read_servers()
            .map_err(shared_store_error)?
            .into_iter()
            .map(Database::mcp_server_from_shared_row)
            .collect::<Vec<_>>();
        if servers.is_empty() {
            return Ok(());
        }
        McpService::sync_all_apps(|app| {
            if !self.initialized.contains(app) {
                return Ok(());
            }
            let batch = CatalogBatch::prepare(transaction, app, &servers)?;
            if *app == AppType::Gemini {
                batch.apply(transaction, |changes| {
                    let (contents, snapshots) = crate::gemini_mcp::prepare_toggles(
                        gemini.settings_path(),
                        gemini.contents(),
                        changes.iter().map(|change| {
                            (
                                change.id,
                                change.server,
                                change.enabled(),
                                change.previous_snapshot,
                            )
                        }),
                    )?;
                    let receipt = gemini.write_settings_retained(contents)?;
                    self.native.push(NativeRecovery::Gemini(Some(receipt)));
                    Ok(snapshots)
                })
            } else {
                let mut native = observe_native(app)?;
                let result = batch.apply(transaction, |changes| native.apply_batch(changes));
                // Even a failed application can own recovery work and a local lock.
                self.native.push(NativeRecovery::App(app.clone(), native));
                result
            }
        })
    }

    /// Keep every binding/local lock alive until all compensation has finished.
    /// Gemini MCP has its own position: an earlier App's leaf link can refer to
    /// Gemini settings, so grouping all Gemini recovery at the end is not safe.
    pub(crate) fn rollback(&mut self, gemini: &mut GeminiOperation) -> Vec<String> {
        let mut failures = Vec::new();
        for native in self.native.iter_mut().rev() {
            let (app, result) = match native {
                NativeRecovery::App(app, native) => (app.as_str(), native.rollback()),
                NativeRecovery::Gemini(receipt) => (
                    "gemini",
                    receipt
                        .take()
                        .map_or(Ok(()), |receipt| gemini.rollback_retained(receipt)),
                ),
            };
            if let Err(error) = result {
                failures.push(format!("native recovery for {app}: {error}"));
            }
        }
        failures
    }
}
