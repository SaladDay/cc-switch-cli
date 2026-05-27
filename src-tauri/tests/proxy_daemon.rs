//! End-to-end tests for the supervisor daemon.
//!
//! Each test runs in a fully isolated sandbox:
//!   - HOME, USERPROFILE → fresh per-test TempDir
//!   - CC_SWITCH_CONFIG_DIR → $sandbox/.cc-switch (so the spawned daemon's
//!     `Database::init()` writes inside the sandbox, NEVER the user's real
//!     ~/.cc-switch)
//!   - XDG_RUNTIME_DIR → $sandbox/run (daemon socket + pidfile)
//!   - XDG_STATE_HOME  → $sandbox/state (daemon log)
//!   - The daemon is spawned by resolving `CARGO_BIN_EXE_cc-switch`, the test
//!     binary built by Cargo. The TestSandbox Drop impl shuts the daemon down
//!     and removes the temp dir.
//!
//! These tests are guarded by `#[cfg(unix)]` because the daemon path is
//! Unix-only.

#![cfg(unix)]

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use serial_test::serial;
use tempfile::TempDir;

/// Global mutex to prevent test sandboxes racing on shared env vars.
fn env_mutex() -> &'static Mutex<()> {
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(()))
}

fn lock_env() -> MutexGuard<'static, ()> {
    env_mutex().lock().unwrap_or_else(|p| p.into_inner())
}

const ENV_KEYS: &[&str] = &[
    "HOME",
    "USERPROFILE",
    "CC_SWITCH_CONFIG_DIR",
    "XDG_RUNTIME_DIR",
    "XDG_STATE_HOME",
    "CLAUDE_CONFIG_DIR",
];

struct TestSandbox {
    _guard: MutexGuard<'static, ()>,
    _root: TempDir,
    runtime_dir: PathBuf,
    socket: PathBuf,
    pidfile: PathBuf,
    original_env: Vec<(&'static str, Option<OsString>)>,
}

impl TestSandbox {
    fn new() -> Self {
        let guard = lock_env();
        let root = TempDir::new().expect("create sandbox tempdir");
        let home = root.path().to_path_buf();
        let config_dir = home.join(".cc-switch");
        let claude_config_dir = home.join(".claude");
        let runtime_dir = home.join("run");
        let state_dir = home.join("state");
        std::fs::create_dir_all(&config_dir).expect("create sandbox cc-switch");
        std::fs::create_dir_all(&claude_config_dir).expect("create sandbox claude");
        std::fs::create_dir_all(&runtime_dir).expect("create sandbox runtime");
        std::fs::create_dir_all(&state_dir).expect("create sandbox state");

        let mut original_env = Vec::new();
        for key in ENV_KEYS {
            original_env.push((*key, std::env::var_os(key)));
        }

        // SAFETY: env mutation is serialized by `env_mutex()`.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("USERPROFILE", &home);
            std::env::set_var("CC_SWITCH_CONFIG_DIR", &config_dir);
            std::env::set_var("CLAUDE_CONFIG_DIR", &claude_config_dir);
            std::env::set_var("XDG_RUNTIME_DIR", &runtime_dir);
            std::env::set_var("XDG_STATE_HOME", &state_dir);
        }

        let socket = runtime_dir.join("cc-switch").join("daemon.sock");
        let pidfile = runtime_dir.join("cc-switch").join("daemon.pid");

        Self {
            _guard: guard,
            _root: root,
            runtime_dir,
            socket,
            pidfile,
            original_env,
        }
    }

    fn socket(&self) -> &Path {
        &self.socket
    }

    fn pidfile(&self) -> &Path {
        &self.pidfile
    }

    fn binary() -> PathBuf {
        PathBuf::from(env!("CARGO_BIN_EXE_cc-switch"))
    }

    /// Spawn the daemon as a background child. Caller is responsible for
    /// keeping the returned Child alive until they want to stop it.
    fn spawn_daemon(&self) -> std::process::Child {
        std::process::Command::new(Self::binary())
            .arg("daemon")
            .arg("start")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn daemon")
    }

    fn wait_for_socket(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.socket.exists() && UnixStream::connect(&self.socket).is_ok() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn read_pid(&self) -> Option<u32> {
        std::fs::read_to_string(&self.pidfile)
            .ok()?
            .trim()
            .parse()
            .ok()
    }
}

impl Drop for TestSandbox {
    fn drop(&mut self) {
        // Ask the daemon to shut down cleanly, then SIGKILL any leftover.
        if self.socket.exists() {
            if let Ok(mut stream) = UnixStream::connect(&self.socket) {
                let _ = stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
                let _ = stream.write_all(b"{\"kind\":\"shutdown\"}\n");
                let _ = stream.flush();
                let mut sink = String::new();
                let _ = BufReader::new(&stream).read_line(&mut sink);
            }
        }

        if let Some(pid) = self.read_pid() {
            unsafe {
                let _ = libc::kill(pid as i32, libc::SIGTERM);
            }
            // Give it a moment to exit, then SIGKILL.
            std::thread::sleep(Duration::from_millis(200));
            unsafe {
                let _ = libc::kill(pid as i32, libc::SIGKILL);
            }
        }

        // Restore environment.
        for (key, value) in &self.original_env {
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }

        let _ = self.runtime_dir.display();
    }
}

fn send_request(socket: &Path, request_json: &str) -> String {
    try_send_request(socket, request_json).expect("daemon socket should be reachable")
}

/// Like `send_request` but returns `None` instead of panicking when the
/// daemon isn't reachable. Used by tests that intentionally drive the daemon
/// to self-exit and then probe whether anything is still listening.
fn try_send_request(socket: &Path, request_json: &str) -> Option<String> {
    let mut stream = UnixStream::connect(socket).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(15))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(15))).ok();
    stream.write_all(request_json.as_bytes()).ok()?;
    stream.write_all(b"\n").ok()?;
    stream.flush().ok()?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut buf = String::new();
    BufReader::new(stream).read_line(&mut buf).ok()?;
    Some(buf.trim().to_string())
}

fn run_cc_switch(args: &[&str]) -> std::process::Output {
    std::process::Command::new(TestSandbox::binary())
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run cc-switch")
}

fn assert_command_success(output: &std::process::Output, command: &str) {
    assert!(
        output.status.success(),
        "{command} should succeed; status={:?}, stdout={}, stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Block until either the daemon process exits or `timeout` elapses.
/// Returns true if the process exited within the timeout.
fn wait_for_daemon_exit(child: &mut std::process::Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return false,
        }
    }
    false
}

#[test]
#[serial]
fn daemon_starts_and_serves_status_request() {
    let sandbox = TestSandbox::new();

    // Spawn the daemon in the foreground (no --detach) as a child process.
    // The sandbox env is set, so the daemon writes its socket + db inside
    // the temp dir and does NOT touch the user's real config.
    let mut child = sandbox.spawn_daemon();

    if !sandbox.wait_for_socket(Duration::from_secs(10)) {
        let _ = child.kill();
        panic!("daemon socket did not come up within 10s");
    }
    assert!(
        sandbox.pidfile().exists(),
        "pidfile should be written under {}",
        sandbox.pidfile().display()
    );

    let response = send_request(sandbox.socket(), r#"{"kind":"status"}"#);
    assert!(
        response.contains("\"kind\":\"status\""),
        "expected status response, got {response}"
    );
    // The worker hasn't been requested yet, so it should report not running.
    assert!(
        response.contains("\"running\":false"),
        "expected running:false before EnsureWorker, got {response}"
    );

    // Clean shutdown via Shutdown RPC.
    let _ = send_request(sandbox.socket(), r#"{"kind":"shutdown"}"#);
    let _ = child.wait();
    assert!(
        !sandbox.pidfile().exists(),
        "pidfile should be removed after shutdown"
    );
}

#[test]
#[serial]
fn ensure_worker_spawns_a_worker_and_drop_takeover_brings_it_down() {
    let sandbox = TestSandbox::new();
    seed_minimal_claude_provider(&sandbox);

    let mut daemon = sandbox.spawn_daemon();
    assert!(
        sandbox.wait_for_socket(Duration::from_secs(10)),
        "daemon socket should come up"
    );

    let response = send_request(
        sandbox.socket(),
        r#"{"kind":"ensure_worker","app_type":"claude"}"#,
    );
    assert!(
        response.contains("\"kind\":\"worker\""),
        "expected Worker response, got {response}"
    );

    // Worker is now running; status should reflect it.
    let status = send_request(sandbox.socket(), r#"{"kind":"status"}"#);
    assert!(
        status.contains("\"running\":true"),
        "expected running:true after EnsureWorker, got {status}"
    );

    // Drop takeover: daemon should signal the worker to exit and return Ok.
    let drop_resp = send_request(
        sandbox.socket(),
        r#"{"kind":"drop_takeover","app_type":"claude"}"#,
    );
    assert!(
        drop_resp.contains("\"kind\":\"ok\""),
        "expected Ok response, got {drop_resp}"
    );

    // With no remaining takeovers the daemon self-exits — no shutdown RPC
    // needed. The pidfile + socket are removed by the daemon's cleanup path.
    assert!(
        wait_for_daemon_exit(&mut daemon, Duration::from_secs(5)),
        "daemon should self-exit after the last takeover is dropped"
    );
}

#[test]
#[serial]
fn proxy_enable_and_disable_cli_manage_daemon_worker() {
    let sandbox = TestSandbox::new();
    seed_minimal_claude_provider(&sandbox);

    let enable = run_cc_switch(&["proxy", "enable"]);
    assert_command_success(&enable, "proxy enable");
    assert!(
        sandbox.wait_for_socket(Duration::from_secs(10)),
        "proxy enable should auto-start the daemon socket"
    );

    let status = run_cc_switch(&["daemon", "status"]);
    assert_command_success(&status, "daemon status after proxy enable");
    let status_stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        status_stdout.contains("worker:        running at"),
        "daemon status should report a running worker after proxy enable, got {status_stdout}"
    );
    assert!(
        status_stdout.contains("takeovers:     claude=true"),
        "daemon status should report claude takeover after proxy enable, got {status_stdout}"
    );
    let taken_over_url = read_claude_settings_base_url().expect("read taken-over claude base url");
    assert!(
        taken_over_url.starts_with("http://127.0.0.1:"),
        "proxy enable should rewrite Claude base URL to local worker, got {taken_over_url}"
    );

    let disable = run_cc_switch(&["proxy", "disable"]);
    assert_command_success(&disable, "proxy disable");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if !sandbox.socket().exists() && !sandbox.pidfile().exists() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "proxy disable should stop the daemon after the last takeover"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        read_claude_settings_base_url().as_deref(),
        None,
        "proxy disable should restore Claude live config without a base URL"
    );
}

#[test]
#[serial]
fn normal_startup_keeps_existing_daemon_route_visible() {
    let sandbox = TestSandbox::new();
    seed_minimal_claude_provider(&sandbox);

    let mut daemon = sandbox.spawn_daemon();
    assert!(
        sandbox.wait_for_socket(Duration::from_secs(10)),
        "daemon socket should come up"
    );

    let enable = run_cc_switch(&["proxy", "enable"]);
    assert_command_success(&enable, "proxy enable");

    let show = run_cc_switch(&["proxy", "show"]);
    assert_command_success(&show, "proxy show");
    let show_stdout = String::from_utf8_lossy(&show.stdout);
    assert!(
        show_stdout.contains("Claude=开启") || show_stdout.contains("Claude=on"),
        "startup recovery should not clear active daemon takeover; got {show_stdout}"
    );
    assert!(
        show_stdout.contains("运行 127.0.0.1:") || show_stdout.contains("running 127.0.0.1:"),
        "proxy show should still see the daemon worker; got {show_stdout}"
    );

    let disable = run_cc_switch(&["proxy", "disable"]);
    assert_command_success(&disable, "proxy disable");
    let _ = wait_for_daemon_exit(&mut daemon, Duration::from_secs(5));
}

#[test]
#[serial]
fn set_global_enabled_false_clears_takeovers_and_stops_the_worker() {
    let sandbox = TestSandbox::new();
    seed_minimal_claude_provider(&sandbox);

    let mut daemon = sandbox.spawn_daemon();
    assert!(
        sandbox.wait_for_socket(Duration::from_secs(10)),
        "daemon socket should come up"
    );

    // Bring up a worker for claude.
    let ensure = send_request(
        sandbox.socket(),
        r#"{"kind":"ensure_worker","app_type":"claude"}"#,
    );
    assert!(
        ensure.contains("\"kind\":\"worker\""),
        "expected Worker response, got {ensure}"
    );

    let pre_status = send_request(sandbox.socket(), r#"{"kind":"status"}"#);
    assert!(
        pre_status.contains("\"running\":true"),
        "worker should be running before disable, got {pre_status}"
    );
    assert!(
        pre_status.contains("\"claude\":true"),
        "claude takeover should be on before disable, got {pre_status}"
    );

    // Flip the global switch off via IPC. Daemon should clear takeovers,
    // stop the worker, and self-exit once nothing is left to supervise.
    let disable = send_request(
        sandbox.socket(),
        r#"{"kind":"set_global_enabled","enabled":false}"#,
    );
    assert!(
        disable.contains("\"kind\":\"ok\""),
        "expected Ok response, got {disable}"
    );

    assert!(
        wait_for_daemon_exit(&mut daemon, Duration::from_secs(5)),
        "daemon should self-exit after set_global_enabled(false) clears the last takeover"
    );
    assert!(
        !sandbox.socket().exists(),
        "socket file should be removed on daemon exit"
    );
    assert!(
        !sandbox.pidfile().exists(),
        "pidfile should be removed on daemon exit"
    );
}

/// Regression for the file-lock deadlock that surfaced as
/// "daemon drop takeover failed: Resource temporarily unavailable (os error 35)".
///
/// The TUI invokes `ProxyService::set_managed_session_for_app` from a worker
/// thread. The foreground used to take the cross-process state-mutation guard
/// and THEN make a synchronous IPC call to the daemon. The daemon's handler
/// also acquires that guard, so it blocked behind the foreground's hold; the
/// foreground's `read_line` then timed out after 15s.
///
/// With the foreground guard removed, `set_managed_session_for_app` should
/// round-trip in well under that timeout.
#[test]
#[serial]
fn set_managed_session_for_app_does_not_deadlock_on_state_mutation_lock() {
    let sandbox = TestSandbox::new();
    seed_minimal_claude_provider(&sandbox);

    let mut daemon = sandbox.spawn_daemon();
    assert!(
        sandbox.wait_for_socket(Duration::from_secs(10)),
        "daemon socket should come up"
    );

    // Bring the worker up via IPC so there's an active takeover to drop.
    let ensure = send_request(
        sandbox.socket(),
        r#"{"kind":"ensure_worker","app_type":"claude"}"#,
    );
    assert!(
        ensure.contains("\"kind\":\"worker\""),
        "expected Worker response, got {ensure}"
    );

    // Drive the same code path the TUI uses: load the AppState in this process
    // and call the proxy service directly. With the deadlock unfixed this hangs
    // for the full 15s IPC read timeout and then errors out.
    let state = cc_switch_lib::AppState::try_new().expect("create app state in sandbox");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("create test runtime");

    let started = Instant::now();
    let result = runtime.block_on(
        state
            .proxy_service
            .set_managed_session_for_app("claude", false),
    );
    let elapsed = started.elapsed();

    assert!(
        result.is_ok(),
        "set_managed_session_for_app(false) should succeed, got {result:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "set_managed_session_for_app(false) should not block on the state \
         mutation lock; took {elapsed:?}"
    );

    // The drop was for the last (only) takeover, so the daemon self-exits.
    assert!(
        wait_for_daemon_exit(&mut daemon, Duration::from_secs(5)),
        "daemon should self-exit after the last takeover is dropped"
    );
}

/// Regression for the symptom the user reported on the TUI:
/// "✗ daemon ensure worker failed: Resource temporarily unavailable (os error 35)"
/// when toggling proxy on and off via the main TUI proxy action.
///
/// The TUI's `SetManagedProxyForCurrentApp` action funnels into
/// `ProxyService::set_managed_session_for_app(app, enabled)` on a worker thread.
/// This test drives that exact code path through the public service API and
/// cycles enable→disable→enable→disable several times. With the daemon healthy
/// and no concurrent foreground guard holders, every round trip should complete
/// quickly — well under the 15 s IPC read timeout that produces EAGAIN.
#[test]
#[serial]
fn set_managed_session_for_app_round_trips_on_repeated_toggles() {
    let sandbox = TestSandbox::new();
    seed_minimal_claude_provider(&sandbox);

    let mut daemon = sandbox.spawn_daemon();
    assert!(
        sandbox.wait_for_socket(Duration::from_secs(10)),
        "daemon socket should come up"
    );

    let state = cc_switch_lib::AppState::try_new().expect("create app state in sandbox");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("create test runtime");

    for round in 0..3 {
        let enable_started = Instant::now();
        let enable = runtime.block_on(
            state
                .proxy_service
                .set_managed_session_for_app("claude", true),
        );
        let enable_elapsed = enable_started.elapsed();
        assert!(
            enable.is_ok(),
            "round {round}: enable should succeed, got {enable:?}"
        );
        assert!(
            enable_elapsed < Duration::from_secs(5),
            "round {round}: enable round-trip should not approach the 15 s IPC timeout; took {enable_elapsed:?}"
        );

        let status = send_request(sandbox.socket(), r#"{"kind":"status"}"#);
        assert!(
            status.contains("\"running\":true"),
            "round {round}: worker should be running after enable, got {status}"
        );

        let disable_started = Instant::now();
        let disable = runtime.block_on(
            state
                .proxy_service
                .set_managed_session_for_app("claude", false),
        );
        let disable_elapsed = disable_started.elapsed();
        assert!(
            disable.is_ok(),
            "round {round}: disable should succeed, got {disable:?}"
        );
        assert!(
            disable_elapsed < Duration::from_secs(5),
            "round {round}: disable round-trip should not approach the 15 s IPC timeout; took {disable_elapsed:?}"
        );

        // After disable, the daemon self-exits. Wait for the socket to go away
        // so the next round's enable spawns a fresh daemon rather than racing
        // the previous one's teardown.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match try_send_request(sandbox.socket(), r#"{"kind":"status"}"#) {
                None => break,
                Some(status) if status.contains("\"running\":false") => {
                    // Daemon hasn't finished tearing down the socket yet but
                    // already reports stopped — wait for the socket to vanish.
                    if !sandbox.socket().exists() {
                        break;
                    }
                }
                Some(_) => {}
            }
            assert!(
                Instant::now() < deadline,
                "round {round}: daemon did not self-exit after disable"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    // The original daemon handle was for round 0's daemon; subsequent rounds
    // each spawned a fresh detached daemon on enable. The first one is long
    // dead — just reap it.
    let _ = daemon.wait();
}

/// Concurrency reproducer for "daemon ensure worker failed: Resource
/// temporarily unavailable (os error 35)".
///
/// The supervisor's `ensure_worker` overwrites `pending_hello`/`pending_token`
/// when called concurrently — the first caller's oneshot is dropped and that
/// caller waits the full 10 s `WORKER_HELLO_TIMEOUT` before returning. If
/// `set_takeover_for_app` afterwards waits even briefly on the file lock, the
/// 15 s client read timeout fires and the foreground sees os error 35.
///
/// We hit this from a single foreground process by issuing two
/// `set_managed_session_for_app(true)` calls in parallel — the same code path
/// the TUI uses when the user toggles proxy.
#[test]
#[serial]
fn concurrent_set_managed_session_does_not_time_out_on_ipc_read() {
    let sandbox = TestSandbox::new();
    seed_minimal_claude_provider(&sandbox);

    let mut daemon = sandbox.spawn_daemon();
    assert!(
        sandbox.wait_for_socket(Duration::from_secs(10)),
        "daemon socket should come up"
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("create test runtime");

    let started = Instant::now();
    let (a, b) = runtime.block_on(async {
        let state_a = cc_switch_lib::AppState::try_new().expect("create state a");
        let state_b = cc_switch_lib::AppState::try_new().expect("create state b");
        tokio::join!(
            state_a
                .proxy_service
                .set_managed_session_for_app("claude", true),
            state_b
                .proxy_service
                .set_managed_session_for_app("claude", true),
        )
    });
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(10),
        "concurrent enables should not approach the 15 s IPC timeout; took {elapsed:?}, a={a:?}, b={b:?}"
    );
    assert!(
        a.is_ok() && b.is_ok(),
        "both concurrent enables should succeed; a={a:?}, b={b:?}"
    );

    let _ = send_request(sandbox.socket(), r#"{"kind":"shutdown"}"#);
    let _ = daemon.wait();
}

/// Stress reproducer for the user-reported flake when toggling proxy via the
/// TUI: kill the worker process behind the daemon's back, then immediately
/// drive `set_managed_session_for_app("claude", true)`. The supervisor's
/// `inner.worker` is briefly stale (the watcher hasn't observed exit yet), so
/// `ensure_worker` returns the dead worker, then `set_takeover_for_app` runs in
/// the daemon and probes the dead session. This must still complete well under
/// the 15 s IPC timeout — if the daemon ever starts a foreground server inside
/// itself or blocks on the persisted session probe for too long, the client
/// surfaces "Resource temporarily unavailable (os error 35)".
#[test]
#[serial]
fn set_managed_session_for_app_recovers_when_worker_was_killed_externally() {
    let sandbox = TestSandbox::new();
    seed_minimal_claude_provider(&sandbox);

    let mut daemon = sandbox.spawn_daemon();
    assert!(
        sandbox.wait_for_socket(Duration::from_secs(10)),
        "daemon socket should come up"
    );

    let state = cc_switch_lib::AppState::try_new().expect("create app state in sandbox");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("create test runtime");

    runtime
        .block_on(
            state
                .proxy_service
                .set_managed_session_for_app("claude", true),
        )
        .expect("initial enable should succeed");

    // Read the worker pid out of the daemon's status response, then SIGKILL it
    // without telling the daemon. Picks up everything between `worker_pid":` and
    // the next non-digit, which is sufficient for the supervisor's status JSON.
    let status_before = send_request(sandbox.socket(), r#"{"kind":"status"}"#);
    let worker_pid = parse_worker_pid(&status_before).expect("worker pid in status");
    unsafe {
        libc::kill(worker_pid as i32, libc::SIGKILL);
    }

    // Race the daemon's watcher: re-enable immediately while inner.worker is
    // still the now-dead worker.
    let started = Instant::now();
    let result = runtime.block_on(
        state
            .proxy_service
            .set_managed_session_for_app("claude", true),
    );
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(5),
        "set_managed_session_for_app(true) should not approach the 15 s IPC timeout after a worker kill; took {elapsed:?}, result={result:?}"
    );
    assert!(
        result.is_ok(),
        "set_managed_session_for_app(true) should recover after a SIGKILL'd worker, got {result:?}"
    );

    let _ = send_request(sandbox.socket(), r#"{"kind":"shutdown"}"#);
    let _ = daemon.wait();
}

fn parse_worker_pid(status_json: &str) -> Option<u32> {
    let key = "\"worker_pid\":";
    let start = status_json.find(key)? + key.len();
    let tail = &status_json[start..];
    let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Seed minimal Claude live config and a per-test proxy port so worker startup
/// does not depend on the host's default proxy port being free.
fn seed_minimal_claude_provider(sandbox: &TestSandbox) {
    let claude_dir = std::env::var_os("HOME")
        .map(|h| Path::new(&h).join(".claude"))
        .expect("HOME set in sandbox");
    std::fs::create_dir_all(&claude_dir).expect("create sandbox .claude");
    std::fs::write(
        claude_dir.join("settings.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"live-key"},"workspace":{"path":"/tmp/workspace"}}"#,
    )
    .expect("seed sandbox claude settings");

    let listen_port = free_loopback_port();
    let state = cc_switch_lib::AppState::try_new().expect("create app state in sandbox");
    let provider = cc_switch_lib::Provider::with_id(
        "claude-provider".to_string(),
        "Claude Provider".to_string(),
        serde_json::json!({
            "env": {
                "ANTHROPIC_API_KEY": "db-key"
            }
        }),
        Some("claude".to_string()),
    );
    state
        .db
        .save_provider("claude", &provider)
        .expect("save sandbox claude provider");
    state
        .db
        .set_current_provider("claude", &provider.id)
        .expect("set sandbox current claude provider");
    state
        .db
        .set_app_proxy_preferred_port("claude", listen_port)
        .expect("update sandbox claude proxy preferred port");

    let _ = sandbox; // tie lifetime so the sandbox outlives this seed
}

fn read_claude_settings_base_url() -> Option<String> {
    let settings_path = cc_switch_lib::get_claude_settings_path();
    let source = std::fs::read_to_string(settings_path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&source).ok()?;
    value
        .get("env")?
        .get("ANTHROPIC_BASE_URL")?
        .as_str()
        .map(ToString::to_string)
}

fn free_loopback_port() -> u16 {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral port");
    listener
        .local_addr()
        .expect("read ephemeral address")
        .port()
}

#[test]
#[serial]
fn sigterm_restores_takeover_and_stops_worker() {
    let sandbox = TestSandbox::new();
    seed_minimal_claude_provider(&sandbox);

    let mut daemon = sandbox.spawn_daemon();
    assert!(
        sandbox.wait_for_socket(Duration::from_secs(10)),
        "daemon socket should come up"
    );

    let ensure = send_request(
        sandbox.socket(),
        r#"{"kind":"ensure_worker","app_type":"claude"}"#,
    );
    assert!(
        ensure.contains("\"kind\":\"worker\""),
        "expected Worker response, got {ensure}"
    );
    let taken_over_url = read_claude_settings_base_url().expect("read taken-over claude base url");
    assert!(
        taken_over_url.starts_with("http://127.0.0.1:"),
        "claude base URL should point at local proxy before SIGTERM, got {taken_over_url}"
    );

    let pid = sandbox.read_pid().expect("read daemon pid");
    unsafe {
        let rc = libc::kill(pid as i32, libc::SIGTERM);
        assert_eq!(rc, 0, "SIGTERM should be delivered to daemon");
    }

    assert!(
        wait_for_daemon_exit(&mut daemon, Duration::from_secs(5)),
        "daemon should exit after SIGTERM"
    );
    assert_eq!(
        read_claude_settings_base_url().as_deref(),
        None,
        "SIGTERM shutdown should restore the original Claude live config without a base URL"
    );
    assert!(
        !sandbox.socket().exists(),
        "socket file should be removed on SIGTERM cleanup"
    );
    assert!(
        !sandbox.pidfile().exists(),
        "pidfile should be removed on SIGTERM cleanup"
    );
}

#[test]
#[serial]
fn second_daemon_invocation_exits_cleanly_when_one_already_runs() {
    let sandbox = TestSandbox::new();

    let mut first = sandbox.spawn_daemon();
    assert!(
        sandbox.wait_for_socket(Duration::from_secs(10)),
        "first daemon should bring up socket"
    );

    // Second daemon: should detect the pidfile is locked and exit 0.
    let second = std::process::Command::new(TestSandbox::binary())
        .arg("daemon")
        .arg("start")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn second daemon");
    let exit = second.wait_with_output().expect("await second daemon");
    assert!(
        exit.status.success(),
        "second daemon should exit cleanly, got {:?}",
        exit.status
    );

    // First daemon should still be alive and reachable.
    let response = send_request(sandbox.socket(), r#"{"kind":"status"}"#);
    assert!(response.contains("\"kind\":\"status\""));

    let _ = send_request(sandbox.socket(), r#"{"kind":"shutdown"}"#);
    let _ = first.wait();
}

/// Self-exit invariant: after the last `drop_takeover`, the daemon should
/// shut itself down so an idle supervisor doesn't outlive its purpose and
/// later get SIGKILL'd (which is the path that leaks a stale socket inode
/// and breaks subsequent disables — the original user-reported bug).
#[test]
#[serial]
fn daemon_self_exits_after_last_drop_takeover() {
    let sandbox = TestSandbox::new();
    seed_minimal_claude_provider(&sandbox);

    let mut daemon = sandbox.spawn_daemon();
    assert!(
        sandbox.wait_for_socket(Duration::from_secs(10)),
        "daemon socket should come up"
    );

    let ensure = send_request(
        sandbox.socket(),
        r#"{"kind":"ensure_worker","app_type":"claude"}"#,
    );
    assert!(
        ensure.contains("\"kind\":\"worker\""),
        "expected Worker response, got {ensure}"
    );

    let drop_resp = send_request(
        sandbox.socket(),
        r#"{"kind":"drop_takeover","app_type":"claude"}"#,
    );
    assert!(
        drop_resp.contains("\"kind\":\"ok\""),
        "expected Ok response from drop_takeover, got {drop_resp}"
    );

    assert!(
        wait_for_daemon_exit(&mut daemon, Duration::from_secs(5)),
        "daemon should self-exit after the last takeover is dropped"
    );

    // Socket + pidfile must be cleaned up so the next disable doesn't see a
    // stale socket and produce ECONNREFUSED.
    assert!(
        !sandbox.socket().exists(),
        "socket file should be removed on daemon exit, still at {}",
        sandbox.socket().display()
    );
    assert!(
        !sandbox.pidfile().exists(),
        "pidfile should be removed on daemon exit, still at {}",
        sandbox.pidfile().display()
    );

    // And the meta DB should agree: no runtime session row left behind.
    let state = cc_switch_lib::AppState::try_new().expect("create app state in sandbox");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("create test runtime");
    let status = runtime.block_on(state.proxy_service.get_status());
    assert!(
        !status.running,
        "proxy_runtime_session must be cleared after daemon self-exit, got {status:?}"
    );
}

/// `set_managed_session_for_app("claude", false)` must succeed even when the
/// daemon socket is stale (file present on disk, no listener bound). This is
/// the exact failure mode the user hit on the TUI `P` hotkey: a prior daemon
/// crashed/got SIGKILL'd, leaving `daemon.sock` behind, and every subsequent
/// disable attempt tripped over `Connection refused (os error 61)`.
#[test]
#[serial]
fn set_managed_session_for_app_false_recovers_when_socket_is_stale() {
    let sandbox = TestSandbox::new();
    seed_minimal_claude_provider(&sandbox);

    // Fabricate a stale socket inode: bind a UnixListener and immediately drop
    // it. On macOS + Linux the file lingers on disk, but connect() returns
    // ECONNREFUSED — exactly what a dead-daemon leftover looks like.
    std::fs::create_dir_all(sandbox.socket().parent().expect("socket parent"))
        .expect("ensure runtime dir");
    let listener =
        std::os::unix::net::UnixListener::bind(sandbox.socket()).expect("bind stale socket");
    drop(listener);
    assert!(
        sandbox.socket().exists(),
        "stale socket should exist on disk after bind+drop"
    );
    assert!(
        UnixStream::connect(sandbox.socket()).is_err(),
        "stale socket should refuse connections"
    );

    let state = cc_switch_lib::AppState::try_new().expect("create app state in sandbox");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("create test runtime");

    let started = Instant::now();
    let result = runtime.block_on(
        state
            .proxy_service
            .set_managed_session_for_app("claude", false),
    );
    let elapsed = started.elapsed();

    assert!(
        result.is_ok(),
        "set_managed_session_for_app(false) should fall back to local cleanup \
         when the daemon socket is stale; got {result:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "stale-socket fallback should be fast (no IPC timeout); took {elapsed:?}"
    );
    assert!(
        !sandbox.socket().exists(),
        "stale socket should be removed by the fallback so subsequent calls \
         take the no-socket short-circuit"
    );

    // Meta DB: no phantom runtime session.
    let status = runtime.block_on(state.proxy_service.get_status());
    assert!(
        !status.running,
        "proxy status must report not-running after stale-socket disable, got {status:?}"
    );
}

/// `notify_global_switch` is the proxy-settings-page sibling of the same
/// bug: it used to bubble up ECONNREFUSED when the daemon socket was stale.
/// It now treats a stale socket as "no daemon — nothing to align" and
/// returns Ok, after cleaning up the stale inode.
#[test]
#[serial]
fn notify_global_switch_treats_stale_socket_as_no_daemon() {
    let sandbox = TestSandbox::new();

    std::fs::create_dir_all(sandbox.socket().parent().expect("socket parent"))
        .expect("ensure runtime dir");
    let listener =
        std::os::unix::net::UnixListener::bind(sandbox.socket()).expect("bind stale socket");
    drop(listener);
    assert!(sandbox.socket().exists(), "stale socket should exist");

    let result = cc_switch_lib::daemon::notify_global_switch(false);
    assert!(
        result.is_ok(),
        "notify_global_switch must succeed against a stale socket, got {result:?}"
    );
    assert!(
        !sandbox.socket().exists(),
        "stale socket should be removed by notify_global_switch fallback"
    );
}
