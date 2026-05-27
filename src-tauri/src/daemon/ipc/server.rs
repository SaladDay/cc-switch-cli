//! Async accept loop for the daemon control socket.
//!
//! Each accepted connection reads a single request line, dispatches it via the
//! provided `Handler`, and writes back a single response line. Connections are
//! ephemeral — the foreground client connects, exchanges one request/response,
//! and disconnects.

use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinSet;

use super::protocol::{encode_response, Request, Response};

/// Maximum time we wait for in-flight connections to drain after shutdown
/// is signalled. The handlers that trigger self-shutdown (drop_takeover with
/// no remaining takeovers, set_global_enabled(false)) already do their work
/// before returning Response::Ok, so this drain just covers flushing those
/// final writes onto the socket.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Server-side handler. The supervisor implements this to translate Requests
/// into actions on its internal state (worker child, takeover ops, DB writes).
pub trait Handler: Send + Sync + 'static {
    fn handle(&self, request: Request) -> impl std::future::Future<Output = Response> + Send;
}

/// Bind a Unix domain socket at `path`, removing any stale entry first.
pub fn bind(path: &Path) -> std::io::Result<UnixListener> {
    if let Some(parent) = path.parent() {
        super::super::paths::ensure_private_runtime_dir(parent)?;
    }
    // Remove any leftover socket from a previous (now-dead) daemon. We only
    // reach this code path after pidfile acquisition has confirmed no other
    // daemon owns the lock, so this is safe.
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    let listener = UnixListener::bind(path)?;
    super::super::paths::set_private_runtime_socket_permissions(path)?;
    Ok(listener)
}

/// Run the accept loop until `shutdown` resolves.
///
/// Each connection is handled on its own task tracked in a `JoinSet`. When
/// shutdown fires we stop accepting new connections but drain the in-flight
/// ones with a short deadline so the response that triggered the shutdown
/// (e.g. `drop_takeover` of the last takeover) actually reaches the client
/// before the daemon's tokio runtime drops the task.
pub async fn run<H, F>(listener: UnixListener, handler: std::sync::Arc<H>, shutdown: F)
where
    H: Handler,
    F: std::future::Future<Output = ()> + Send,
{
    tokio::pin!(shutdown);
    let mut tasks: JoinSet<()> = JoinSet::new();
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                log::debug!("daemon ipc: shutdown signalled, draining in-flight connections");
                drain_in_flight(&mut tasks).await;
                return;
            }
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _)) => {
                        let handler = handler.clone();
                        tasks.spawn(async move {
                            if let Err(err) = serve_connection(stream, handler).await {
                                log::warn!("daemon ipc: connection failed: {err}");
                            }
                        });
                    }
                    Err(err) => {
                        log::warn!("daemon ipc: accept failed: {err}");
                    }
                }
            }
            // Reap finished connection tasks so the JoinSet doesn't grow
            // unboundedly on a long-running daemon.
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
}

async fn drain_in_flight(tasks: &mut JoinSet<()>) {
    if tasks.is_empty() {
        return;
    }
    let drain = async { while tasks.join_next().await.is_some() {} };
    if tokio::time::timeout(DRAIN_TIMEOUT, drain).await.is_err() {
        log::warn!(
            "daemon ipc: drain deadline ({}s) elapsed with {} in-flight connection(s)",
            DRAIN_TIMEOUT.as_secs(),
            tasks.len()
        );
        tasks.abort_all();
    }
}

async fn serve_connection<H>(stream: UnixStream, handler: std::sync::Arc<H>) -> std::io::Result<()>
where
    H: Handler,
{
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(());
    }

    let trimmed = line.trim();
    let request = match serde_json::from_str::<Request>(trimmed) {
        Ok(req) => req,
        Err(err) => {
            let resp = Response::Error {
                message: format!("invalid request: {err}"),
            };
            return write_response(&mut write_half, &resp).await;
        }
    };

    let response = handler.handle(request).await;
    write_response(&mut write_half, &response).await
}

async fn write_response(
    write_half: &mut tokio::net::unix::OwnedWriteHalf,
    response: &Response,
) -> std::io::Result<()> {
    let payload = encode_response(response).map_err(std::io::Error::other)?;
    write_half.write_all(payload.as_bytes()).await?;
    write_half.write_all(b"\n").await?;
    write_half.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::oneshot;

    struct Echo;

    impl Handler for Echo {
        async fn handle(&self, request: Request) -> Response {
            match request {
                Request::Status => Response::Ok,
                Request::Shutdown => Response::Ok,
                Request::EnsureWorker { app_type, .. } => Response::Worker {
                    address: format!("addr-for-{app_type}"),
                    port: 1,
                    session_token: "tok".into(),
                    pid: 42,
                    started_at: None,
                },
                _ => Response::Error {
                    message: "unsupported in echo".into(),
                },
            }
        }
    }

    #[tokio::test]
    async fn server_handles_request_response_round_trip() {
        let tmp = tempfile::tempdir().expect("tmp");
        let sock = tmp.path().join("daemon.sock");
        let listener = bind(&sock).expect("bind");

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            run(listener, Arc::new(Echo), async move {
                let _ = shutdown_rx.await;
            })
            .await;
        });

        // Drive a single request through the connection to confirm the loop
        // dispatches and writes back.
        let mut stream = UnixStream::connect(&sock).await.expect("connect");
        let req = serde_json::to_string(&Request::EnsureWorker {
            app_type: "claude".into(),
            fallback_provider_id: None,
        })
        .unwrap();
        stream
            .write_all(format!("{req}\n").as_bytes())
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
        let mut buf = String::new();
        let mut reader = BufReader::new(stream);
        reader.read_line(&mut buf).await.expect("read response");
        let parsed: Response = serde_json::from_str(buf.trim()).expect("parse");
        match parsed {
            Response::Worker { address, .. } => assert_eq!(address, "addr-for-claude"),
            other => panic!("unexpected: {other:?}"),
        }

        let _ = shutdown_tx.send(());
        server.await.expect("server task join");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bind_creates_private_runtime_dir_and_socket() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("tmp");
        let runtime = tmp.path().join("runtime");
        let sock = runtime.join("daemon.sock");

        let listener = bind(&sock).expect("bind");

        let dir_mode = std::fs::metadata(&runtime)
            .expect("read runtime dir metadata")
            .permissions()
            .mode()
            & 0o777;
        let socket_mode = std::fs::metadata(&sock)
            .expect("read socket metadata")
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(dir_mode, 0o700);
        assert_eq!(socket_mode, 0o600);
        drop(listener);
    }

    #[tokio::test]
    async fn server_returns_error_response_for_invalid_json() {
        let tmp = tempfile::tempdir().expect("tmp");
        let sock = tmp.path().join("daemon.sock");
        let listener = bind(&sock).expect("bind");

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            run(listener, Arc::new(Echo), async move {
                let _ = shutdown_rx.await;
            })
            .await;
        });

        let mut stream = UnixStream::connect(&sock).await.expect("connect");
        stream.write_all(b"not-json\n").await.unwrap();
        stream.shutdown().await.unwrap();
        let mut buf = String::new();
        let mut reader = BufReader::new(stream);
        reader.read_line(&mut buf).await.expect("read response");
        let parsed: Response = serde_json::from_str(buf.trim()).expect("parse");
        assert!(matches!(parsed, Response::Error { .. }));

        let _ = shutdown_tx.send(());
        server.await.expect("server task join");
    }
}
