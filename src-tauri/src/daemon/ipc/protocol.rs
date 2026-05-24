//! Wire protocol for the daemon control socket.
//!
//! Framing: one JSON object per line (newline-delimited). Each connection is
//! request/response style — the client writes one Request line, the server
//! writes one Response line, and either side may close.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Request {
    /// Foreground asks the daemon to bring the named app's worker up if it
    /// isn't already, and enable proxy takeover for that app.
    EnsureWorker { app_type: String },
    /// Foreground asks the daemon to disable takeover for the named app. The
    /// daemon stops that app's worker and may exit if no workers remain.
    DropTakeover { app_type: String },
    /// Foreground asks for current daemon + worker state.
    Status,
    /// Worker → daemon, sent once on worker startup. Identifies the bound
    /// listener and the session token so the daemon can publish the
    /// `proxy_runtime_session` row on the worker's behalf.
    WorkerHello {
        pid: u32,
        address: String,
        port: u16,
        session_token: String,
    },
    /// Foreground asks the daemon to set the global desired proxy switch and
    /// align worker state with it. On `enabled: false`, the daemon clears all
    /// active per-app takeovers and stops all workers. On `enabled: true`, the
    /// daemon writes the desired switch only; app routes start through
    /// `EnsureWorker`.
    SetGlobalEnabled { enabled: bool },
    /// Force the daemon to stop the worker (if any) and exit.
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Worker {
        address: String,
        port: u16,
        session_token: String,
        pid: u32,
    },
    Status {
        running: bool,
        address: String,
        port: u16,
        worker_pid: Option<u32>,
        takeovers: TakeoverFlags,
        restart_count: u32,
        last_restart_at: Option<String>,
        #[serde(default)]
        workers: Vec<WorkerState>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TakeoverFlags {
    pub claude: bool,
    pub codex: bool,
    pub gemini: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerState {
    pub app_type: String,
    pub running: bool,
    pub address: String,
    pub port: u16,
    pub pid: Option<u32>,
}

/// Encode a request as a single JSON line (no trailing newline).
pub fn encode_request(req: &Request) -> Result<String, serde_json::Error> {
    serde_json::to_string(req)
}

/// Encode a response as a single JSON line (no trailing newline).
pub fn encode_response(resp: &Response) -> Result<String, serde_json::Error> {
    serde_json::to_string(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip_request(req: Request) {
        let line = encode_request(&req).expect("encode");
        let decoded: Request = serde_json::from_str(&line).expect("decode");
        assert_eq!(decoded, req);
    }

    fn roundtrip_response(resp: Response) {
        let line = encode_response(&resp).expect("encode");
        let decoded: Response = serde_json::from_str(&line).expect("decode");
        assert_eq!(decoded, resp);
    }

    #[test]
    fn ensure_worker_roundtrips() {
        roundtrip_request(Request::EnsureWorker {
            app_type: "claude".to_string(),
        });
    }

    #[test]
    fn drop_takeover_roundtrips() {
        roundtrip_request(Request::DropTakeover {
            app_type: "codex".to_string(),
        });
    }

    #[test]
    fn status_request_roundtrips() {
        roundtrip_request(Request::Status);
    }

    #[test]
    fn worker_hello_roundtrips() {
        roundtrip_request(Request::WorkerHello {
            pid: 4242,
            address: "127.0.0.1".to_string(),
            port: 15721,
            session_token: "tok".to_string(),
        });
    }

    #[test]
    fn shutdown_request_roundtrips() {
        roundtrip_request(Request::Shutdown);
    }

    #[test]
    fn set_global_enabled_roundtrips_both_polarities() {
        roundtrip_request(Request::SetGlobalEnabled { enabled: true });
        roundtrip_request(Request::SetGlobalEnabled { enabled: false });
    }

    #[test]
    fn ok_response_roundtrips() {
        roundtrip_response(Response::Ok);
    }

    #[test]
    fn worker_response_roundtrips() {
        roundtrip_response(Response::Worker {
            address: "127.0.0.1".to_string(),
            port: 15721,
            session_token: "tok".to_string(),
            pid: 9999,
        });
    }

    #[test]
    fn status_response_roundtrips() {
        roundtrip_response(Response::Status {
            running: true,
            address: "127.0.0.1".to_string(),
            port: 15721,
            worker_pid: Some(9999),
            takeovers: TakeoverFlags {
                claude: true,
                codex: false,
                gemini: true,
            },
            restart_count: 2,
            last_restart_at: Some("2026-05-15T12:34:56Z".to_string()),
            workers: vec![WorkerState {
                app_type: "claude".to_string(),
                running: true,
                address: "127.0.0.1".to_string(),
                port: 15721,
                pid: Some(9999),
            }],
        });
    }

    #[test]
    fn error_response_roundtrips() {
        roundtrip_response(Response::Error {
            message: "boom".to_string(),
        });
    }

    #[test]
    fn encoded_lines_have_no_embedded_newlines() {
        let line = encode_request(&Request::WorkerHello {
            pid: 1,
            address: "a".into(),
            port: 1,
            session_token: "t".into(),
        })
        .unwrap();
        assert!(!line.contains('\n'));
    }
}
