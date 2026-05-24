//! Synchronous client used by the foreground TUI/CLI to talk to the daemon.
//!
//! - One TCP-style request/response per connection.
//! - Auto-spawns the daemon (`cc-switch daemon start --detach`) on
//!   `ECONNREFUSED` / missing socket; subsequent retries wait for the socket
//!   to appear.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::protocol::{encode_request, Request, Response};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub enum ClientError {
    NoDaemon(String),
    Io(std::io::Error),
    Protocol(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDaemon(msg) => write!(f, "{msg}"),
            Self::Io(e) => write!(f, "{e}"),
            Self::Protocol(msg) => write!(f, "protocol error: {msg}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// Connect to the daemon's control socket. If the socket is missing or refuses
/// connections, fork-and-exec `cc-switch daemon start --detach` (or whatever
/// `binary_resolver` returns) and retry until the socket comes up or we time
/// out.
pub fn connect_or_spawn<F>(
    socket_path: &Path,
    binary_resolver: F,
) -> Result<UnixStream, ClientError>
where
    F: FnOnce() -> Result<PathBuf, ClientError>,
{
    if let Ok(stream) = UnixStream::connect(socket_path) {
        return Ok(stream);
    }

    let bin = binary_resolver()?;
    let mut cmd = Command::new(&bin);
    cmd.arg("daemon")
        .arg("start")
        .arg("--detach")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.spawn()
        .map_err(|err| ClientError::NoDaemon(format!("spawn daemon failed: {err}")))?;

    let deadline = Instant::now() + CONNECT_TIMEOUT;
    while Instant::now() < deadline {
        if let Ok(stream) = UnixStream::connect(socket_path) {
            return Ok(stream);
        }
        std::thread::sleep(Duration::from_millis(75));
    }
    Err(ClientError::NoDaemon(format!(
        "daemon socket {} did not come up within {}s",
        socket_path.display(),
        CONNECT_TIMEOUT.as_secs()
    )))
}

/// Connect-only (no auto-spawn). Used when the caller has already ensured the
/// daemon is running (e.g. from inside the worker startup path).
pub fn connect(socket_path: &Path) -> Result<UnixStream, ClientError> {
    UnixStream::connect(socket_path).map_err(ClientError::Io)
}

/// Send one request and read one response on `stream`.
pub fn exchange(stream: &mut UnixStream, request: &Request) -> Result<Response, ClientError> {
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .map_err(ClientError::Io)?;
    stream
        .set_write_timeout(Some(READ_TIMEOUT))
        .map_err(ClientError::Io)?;

    let payload = encode_request(request)
        .map_err(|err| ClientError::Protocol(format!("encode request: {err}")))?;
    stream
        .write_all(payload.as_bytes())
        .map_err(ClientError::Io)?;
    stream.write_all(b"\n").map_err(ClientError::Io)?;
    stream.flush().map_err(ClientError::Io)?;
    // Half-close the write side so the server's read_line returns. The Unix
    // domain socket here is bidirectional, so we use shutdown(Write) on the fd
    // via the stream's shutdown method.
    let _ = stream.shutdown(std::net::Shutdown::Write);

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let n = reader.read_line(&mut line).map_err(ClientError::Io)?;
    if n == 0 {
        return Err(ClientError::Protocol(
            "daemon closed connection without response".into(),
        ));
    }
    serde_json::from_str(line.trim())
        .map_err(|err| ClientError::Protocol(format!("decode response: {err}")))
}

/// Convenience: open a socket, send one request, return the response.
pub fn round_trip(socket_path: &Path, request: &Request) -> Result<Response, ClientError> {
    let mut stream = connect(socket_path)?;
    exchange(&mut stream, request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::ipc::protocol::TakeoverFlags;
    use std::sync::Arc;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as TokioBufReader};
    use tokio::net::UnixListener;

    /// Tiny tokio-based echo server for client-side tests. Replies once with
    /// the stub response, then closes.
    fn spawn_test_server(socket: PathBuf, response: Response) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            if socket.exists() {
                let _ = std::fs::remove_file(&socket);
            }
            let listener = UnixListener::bind(&socket).expect("bind test listener");
            let (stream, _) = listener.accept().await.expect("accept test conn");
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = TokioBufReader::new(read_half);
            let mut buf = String::new();
            reader.read_line(&mut buf).await.expect("read request");
            let body = serde_json::to_string(&response).expect("encode test response");
            write_half
                .write_all(body.as_bytes())
                .await
                .expect("write resp");
            write_half.write_all(b"\n").await.expect("write nl");
            write_half.flush().await.expect("flush");
        })
    }

    #[tokio::test]
    async fn round_trip_returns_decoded_response() {
        let tmp = tempfile::tempdir().expect("tmp");
        let sock = tmp.path().join("daemon.sock");
        let stub = Response::Status {
            running: true,
            address: "127.0.0.1".into(),
            port: 1234,
            worker_pid: Some(99),
            takeovers: TakeoverFlags::default(),
            restart_count: 0,
            last_restart_at: None,
            workers: vec![],
        };
        let server = spawn_test_server(sock.clone(), stub.clone());

        // Client API is synchronous; run on a blocking thread so we don't
        // starve the runtime that's hosting the server.
        let client_sock = sock.clone();
        let result = tokio::task::spawn_blocking(move || {
            // Brief wait for the listener to bind.
            for _ in 0..50 {
                if client_sock.exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            round_trip(&client_sock, &Request::Status)
        })
        .await
        .expect("blocking task")
        .expect("round trip");

        assert_eq!(result, stub);
        let _ = Arc::new(()); // unused; silence lint about unused import on some configs
        server.await.expect("server task");
    }
}
