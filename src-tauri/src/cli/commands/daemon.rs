use std::path::PathBuf;

use clap::Subcommand;

use crate::cli::ui::{highlight, info, success, warning};
use crate::daemon;
use crate::daemon::ipc::client;
use crate::daemon::ipc::protocol::{Request, Response};
use crate::error::AppError;

#[derive(Subcommand, Debug, Clone)]
pub enum DaemonCommand {
    /// Start the supervisor daemon. Without --detach, runs in the foreground
    /// (useful for debugging or running under systemd / launchd).
    Start {
        /// Detach from the terminal (double-fork) and write the pidfile.
        #[arg(long)]
        detach: bool,
    },
    /// Tell the running daemon to stop the worker (if any) and exit.
    Stop,
    /// Print daemon status (running, worker pid, restart count).
    Status,
    /// Show the path to the daemon log file.
    Logs,
}

pub fn execute(cmd: DaemonCommand) -> Result<(), AppError> {
    match cmd {
        DaemonCommand::Start { detach } => start_daemon(detach),
        DaemonCommand::Stop => stop_daemon(),
        DaemonCommand::Status => status_daemon(),
        DaemonCommand::Logs => show_log_path(),
    }
}

fn start_daemon(detach: bool) -> Result<(), AppError> {
    if detach {
        detach_into_background()?;
    }

    let binary_path = current_executable()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| AppError::Message(format!("build daemon runtime failed: {err}")))?;
    runtime
        .block_on(daemon::run(binary_path))
        .map_err(AppError::Message)
}

fn stop_daemon() -> Result<(), AppError> {
    let socket = daemon::paths::socket_path();
    let response = client::round_trip(&socket, &Request::Shutdown)
        .map_err(|err| AppError::Message(format!("send shutdown to daemon: {err}")))?;
    match response {
        Response::Ok => {
            println!("{}", success("daemon shutdown signalled"));
            Ok(())
        }
        Response::Error { message } => Err(AppError::Message(message)),
        other => Err(AppError::Message(format!(
            "unexpected response from daemon: {other:?}"
        ))),
    }
}

fn status_daemon() -> Result<(), AppError> {
    let socket = daemon::paths::socket_path();
    let response = match client::round_trip(&socket, &Request::Status) {
        Ok(r) => r,
        Err(err) => {
            println!("{}", warning(&format!("daemon not reachable: {err}")));
            return Ok(());
        }
    };
    match response {
        Response::Status {
            running,
            address,
            port,
            worker_pid,
            takeovers,
            restart_count,
            last_restart_at,
            workers,
        } => {
            println!("{}", highlight("cc-switch daemon"));
            println!(
                "  worker:        {}",
                if running {
                    format!(
                        "running at {address}:{port} (pid {})",
                        worker_pid.unwrap_or(0)
                    )
                } else {
                    "not running".to_string()
                }
            );
            for worker in &workers {
                println!(
                    "  worker[{}]:   {}:{} (pid {})",
                    worker.app_type,
                    worker.address,
                    worker.port,
                    worker.pid.unwrap_or(0)
                );
            }
            println!(
                "  takeovers:     claude={}, codex={}, gemini={}",
                takeovers.claude, takeovers.codex, takeovers.gemini
            );
            println!("  restart count: {restart_count}");
            if let Some(at) = last_restart_at {
                println!("  last restart:  {at}");
            }
            Ok(())
        }
        Response::Error { message } => Err(AppError::Message(message)),
        other => Err(AppError::Message(format!(
            "unexpected response from daemon: {other:?}"
        ))),
    }
}

fn show_log_path() -> Result<(), AppError> {
    let path = daemon::paths::log_path();
    println!("{}", info(&path.display().to_string()));
    Ok(())
}

fn current_executable() -> Result<PathBuf, AppError> {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_cc-switch") {
        return Ok(PathBuf::from(path));
    }
    std::env::current_exe()
        .map_err(|err| AppError::Message(format!("resolve daemon executable: {err}")))
}

#[cfg(unix)]
fn detach_into_background() -> Result<(), AppError> {
    // Double-fork via libc::daemon. nochdir=1 keeps cwd, noclose=0 redirects
    // stdio to /dev/null so the daemon doesn't keep the parent terminal open.
    let rc = unsafe { libc::daemon(1, 0) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        return Err(AppError::Message(format!("daemonize failed: {err}")));
    }
    Ok(())
}

#[cfg(not(unix))]
fn detach_into_background() -> Result<(), AppError> {
    Err(AppError::Message(
        "--detach is only supported on unix targets".to_string(),
    ))
}
