//! User-space supervisor for the proxy worker.
//!
//! The daemon owns the worker process: it spawns it, watches it, restarts it
//! under a backoff policy, and keeps the SQLite `proxy_runtime_session` row
//! aligned with the actual worker state. Foreground TUI/CLI processes talk to
//! the daemon via a Unix domain socket.

pub mod ipc;
pub mod logging;
pub mod paths;
pub mod pidfile;
pub mod restart;
pub mod supervisor;

use std::path::PathBuf;
use std::sync::Arc;

use log::LevelFilter;

use crate::database::Database;

use self::ipc::client;
use self::ipc::protocol::{Request, Response};
use self::pidfile::{AcquireError, PidFile};
use self::supervisor::Supervisor;

/// Notify the daemon that the persisted global proxy switch should change.
/// The daemon writes the new desired state and aligns worker state with it.
///
/// Returns `Ok(())` if there is no live daemon (socket missing, or socket
/// inode left over from a daemon that died ungracefully so `connect` returns
/// ECONNREFUSED/ENOENT) or the daemon acknowledged. Returns `Err(message)`
/// only when the socket has a live listener but the round-trip failed or the
/// daemon refused.
pub fn notify_global_switch(enabled: bool) -> Result<(), String> {
    use std::io::ErrorKind;
    let socket = paths::socket_path();
    if !socket.exists() {
        return Ok(());
    }
    match client::round_trip(&socket, &Request::SetGlobalEnabled { enabled }) {
        Ok(Response::Ok) => Ok(()),
        Ok(Response::Error { message }) => Err(message),
        Ok(other) => Err(format!("unexpected daemon response: {other:?}")),
        Err(client::ClientError::Io(e))
            if matches!(e.kind(), ErrorKind::ConnectionRefused | ErrorKind::NotFound) =>
        {
            // Stale socket inode from a dead daemon — there is no worker to
            // align with anyone. Best-effort remove so subsequent calls don't
            // hit the same misfire.
            let _ = std::fs::remove_file(&socket);
            Ok(())
        }
        Err(err) => Err(err.to_string()),
    }
}

/// Run the daemon to completion. Acquires the pidfile, installs the file
/// logger, runs startup recovery, binds the IPC socket, and dispatches
/// requests until shutdown is signalled.
pub async fn run(binary_path: PathBuf) -> Result<(), String> {
    let pidfile_path = paths::pidfile_path();
    let socket_path = paths::socket_path();
    let log_path = paths::log_path();

    let _pidfile = match PidFile::acquire(&pidfile_path) {
        Ok(p) => p,
        Err(AcquireError::AlreadyHeld { pid }) => {
            // Another daemon is already running — exit cleanly.
            log::info!(
                "another cc-switch daemon is already running (pid {})",
                pid.map(|p| p.to_string()).unwrap_or_else(|| "?".into())
            );
            return Ok(());
        }
        Err(AcquireError::Io(err)) => {
            return Err(format!(
                "acquire pidfile {} failed: {err}",
                pidfile_path.display()
            ));
        }
    };

    logging::install(&log_path, LevelFilter::Info)?;
    log::info!(
        "[daemon] starting; pid={} socket={} log={}",
        std::process::id(),
        socket_path.display(),
        log_path.display()
    );

    let db =
        Arc::new(Database::init().map_err(|err| format!("daemon: open database failed: {err}"))?);
    crate::services::session_usage::spawn_periodic_session_usage_sync(db.clone(), "daemon");
    Database::spawn_periodic_usage_maintenance(db.clone(), "daemon");
    let supervisor = Supervisor::new(db, socket_path.clone(), binary_path);

    if let Err(err) = supervisor.recover_on_startup().await {
        log::warn!("[daemon] startup recovery: {err}");
    }

    let listener = ipc::server::bind(&socket_path)
        .map_err(|err| format!("bind socket {}: {err}", socket_path.display()))?;
    log::info!("[daemon] listening on {}", socket_path.display());

    let shutdown = supervisor.shutdown_signal();
    let supervisor_arc = Arc::new(supervisor);

    install_signal_handlers(supervisor_arc.clone());

    ipc::server::run(listener, supervisor_arc, async move {
        shutdown.notified().await;
    })
    .await;

    log::info!("[daemon] exiting");
    let _ = std::fs::remove_file(&socket_path);
    Ok(())
}

#[cfg(unix)]
fn install_signal_handlers(supervisor: Arc<Supervisor>) {
    use tokio::signal::unix::{signal, SignalKind};
    let term_supervisor = supervisor.clone();
    tokio::spawn(async move {
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(err) => {
                log::warn!("install SIGTERM handler failed: {err}");
                return;
            }
        };
        if sigterm.recv().await.is_some() {
            log::info!("[daemon] SIGTERM received, shutting down");
            term_supervisor.shutdown().await;
        }
    });
    tokio::spawn(async move {
        let mut sigint = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(err) => {
                log::warn!("install SIGINT handler failed: {err}");
                return;
            }
        };
        if sigint.recv().await.is_some() {
            log::info!("[daemon] SIGINT received, shutting down");
            supervisor.shutdown().await;
        }
    });
}

#[cfg(not(unix))]
fn install_signal_handlers(_: Arc<Supervisor>) {}
