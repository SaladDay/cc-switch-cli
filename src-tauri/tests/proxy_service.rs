#![allow(clippy::await_holding_lock)]

use std::{
    net::TcpListener,
    sync::Arc,
    time::{Duration, Instant},
};

use cc_switch_lib::{
    get_claude_settings_path, get_codex_config_path, write_codex_live_atomic, AppState, AppType,
    Database, ProxyService,
};
use serde_json::json;
use serial_test::serial;
use tokio::{io::AsyncWriteExt, net::TcpListener as TokioTcpListener};

#[path = "support.rs"]
mod support;
#[cfg(unix)]
use support::wait_for_process_exit;
use support::{
    cleanup_test_processes, ensure_test_home, lock_test_mutex, reset_test_fs, TestProcessCleanup,
};

fn find_free_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind free local port");
    listener
        .local_addr()
        .expect("read local listener address")
        .port()
}

async fn set_proxy_port_for_app(db: &Arc<Database>, app_type: &str, listen_port: u16) -> u16 {
    db.set_app_proxy_preferred_port(app_type, listen_port)
        .unwrap_or_else(|_| panic!("update {app_type} proxy preferred port"));
    listen_port
}

async fn set_claude_proxy_port(db: &Arc<Database>, listen_port: u16) -> u16 {
    set_proxy_port_for_app(db, "claude", listen_port).await
}

async fn set_codex_proxy_port(db: &Arc<Database>, listen_port: u16) -> u16 {
    set_proxy_port_for_app(db, "codex", listen_port).await
}

fn wait_for<F>(timeout: Duration, mut condition: F)
where
    F: FnMut() -> bool,
{
    let started = Instant::now();
    while started.elapsed() < timeout {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    panic!("condition was not met within {:?}", timeout);
}

fn seed_claude_live_config(value: serde_json::Value) {
    std::fs::create_dir_all(get_claude_settings_path().parent().expect("claude dir"))
        .expect("create claude settings dir");
    std::fs::write(
        get_claude_settings_path(),
        serde_json::to_string_pretty(&value).expect("serialize claude live config"),
    )
    .expect("seed claude live config");
}

fn seed_codex_live_config(auth: serde_json::Value, config_text: &str) {
    std::fs::create_dir_all(get_codex_config_path().parent().expect("codex dir"))
        .expect("create codex config dir");
    write_codex_live_atomic(&auth, Some(config_text)).expect("seed codex live config");
}

fn load_runtime_session_pid_for_app(state: &AppState, app_type: &str) -> u32 {
    let session: serde_json::Value = serde_json::from_str(
        &state
            .db
            .get_setting("proxy_runtime_session")
            .expect("load runtime session setting")
            .expect("persisted runtime session should exist"),
    )
    .expect("parse runtime session setting");
    let session = session
        .get("workers")
        .and_then(|workers| workers.get(app_type))
        .unwrap_or(&session);
    session
        .get("pid")
        .and_then(|value| value.as_u64())
        .expect("runtime session pid") as u32
}

fn load_runtime_session_pid(state: &AppState) -> u32 {
    load_runtime_session_pid_for_app(state, "claude")
}

fn load_runtime_session_worker_count(state: &AppState) -> usize {
    let session: serde_json::Value = serde_json::from_str(
        &state
            .db
            .get_setting("proxy_runtime_session")
            .expect("load runtime session setting")
            .expect("persisted runtime session should exist"),
    )
    .expect("parse runtime session setting");
    session
        .get("workers")
        .and_then(|workers| workers.as_object())
        .map_or(1, serde_json::Map::len)
}

#[cfg(unix)]
struct ManagedSessionCleanup(Option<u32>);

#[cfg(unix)]
impl ManagedSessionCleanup {
    fn new(pid: u32) -> Self {
        Self(Some(pid))
    }
}

#[cfg(unix)]
impl Drop for ManagedSessionCleanup {
    fn drop(&mut self) {
        if let Some(pid) = self.0.take() {
            ensure_process_stopped(pid);
        }
    }
}

#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    let rc = unsafe { libc::kill(pid as i32, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(unix)]
fn ensure_process_stopped(pid: u32) {
    if !is_process_alive(pid) {
        return;
    }

    let _ = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    wait_for(Duration::from_secs(5), || !is_process_alive(pid));
}

#[cfg(unix)]
fn session_id(pid: u32) -> i32 {
    let sid = unsafe { libc::getsid(pid as i32) };
    assert!(sid >= 0, "getsid should succeed for pid {pid}");
    sid
}

#[cfg(unix)]
fn process_group_id(pid: u32) -> i32 {
    let pgid = unsafe { libc::getpgid(pid as i32) };
    assert!(pgid >= 0, "getpgid should succeed for pid {pid}");
    pgid
}

#[tokio::test]
async fn proxy_service_starts_and_stops_without_takeover() {
    let db = Arc::new(Database::memory().expect("create database"));
    let service = ProxyService::new(db.clone());

    let initial = service.get_status().await;
    assert!(!initial.running, "proxy should start in stopped state");

    let mut config = service.get_config().await.expect("get proxy config");
    config.listen_port = 0;

    let started = service
        .start_with_runtime_config(config)
        .await
        .expect("start proxy");
    assert!(started.port > 0, "proxy should bind an ephemeral port");
    assert!(
        service.is_running().await,
        "proxy should report running after start"
    );

    let running = service.get_status().await;
    assert!(running.running, "status should report running after start");
    assert!(running.port > 0, "status should report the bound port");

    service.stop().await.expect("stop proxy");
    assert!(
        !service.is_running().await,
        "proxy should report stopped after stop"
    );
}

#[tokio::test]
async fn proxy_service_stop_returns_error_when_runtime_is_not_running() {
    let db = Arc::new(Database::memory().expect("create database"));
    let service = ProxyService::new(db);

    let error = service
        .stop()
        .await
        .expect_err("stop should return an explicit not-running error");
    assert!(
        error.contains("not running"),
        "unexpected stop error message: {error}"
    );
}

#[tokio::test]
async fn proxy_service_stop_with_restore_swallows_not_running_stop_error() {
    let db = Arc::new(Database::memory().expect("create database"));
    let service = ProxyService::new(db);

    service
        .stop_with_restore()
        .await
        .expect("stop_with_restore should continue when stop reports not-running");
}

#[tokio::test]
async fn proxy_service_runtime_override_does_not_persist_proxy_config() {
    let db = Arc::new(Database::memory().expect("create database"));
    let service = ProxyService::new(db);

    let persisted_before = service.get_config().await.expect("get config");
    let mut runtime_config = persisted_before.clone();
    runtime_config.listen_port = 0;

    let started = service
        .start_with_runtime_config(runtime_config)
        .await
        .expect("start proxy with runtime config");
    assert!(
        started.port > 0,
        "runtime config should bind an ephemeral port"
    );

    let persisted_after = service.get_config().await.expect("read persisted config");
    assert_eq!(
        persisted_after.listen_port, persisted_before.listen_port,
        "runtime override must not change persisted proxy config"
    );

    service.stop().await.expect("stop proxy");
}

#[tokio::test]
async fn proxy_service_status_tracks_runtime_uptime() {
    let db = Arc::new(Database::memory().expect("create database"));
    let service = ProxyService::new(db);

    let mut config = service.get_config().await.expect("get config");
    config.listen_port = 0;

    service
        .start_with_runtime_config(config)
        .await
        .expect("start proxy");
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    let status = service.get_status().await;
    assert!(
        status.uptime_seconds >= 1,
        "running proxy status should derive uptime from runtime state"
    );

    service.stop().await.expect("stop proxy");
}

#[tokio::test]
#[serial]
async fn proxy_service_updates_global_enabled_switch() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let db = Arc::new(Database::memory().expect("create database"));
    let service = ProxyService::new(db);

    let before = service
        .get_global_config()
        .await
        .expect("get global proxy config");
    assert!(
        !before.proxy_enabled,
        "proxy should start disabled by default"
    );

    let updated = service
        .set_global_enabled(true)
        .await
        .expect("enable proxy globally");
    assert!(
        updated.config.proxy_enabled,
        "service should return updated switch state"
    );

    let after = service
        .get_global_config()
        .await
        .expect("reload global proxy config");
    assert!(after.proxy_enabled, "global proxy switch should persist");
}

#[tokio::test]
#[serial]
async fn app_state_reuses_active_proxy_runtime_across_reloads() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let state = AppState::try_new().expect("create app state");
    let mut config = state
        .proxy_service
        .get_config()
        .await
        .expect("get proxy config");
    config.listen_port = 0;

    let started = state
        .proxy_service
        .start_with_runtime_config(config)
        .await
        .expect("start proxy");
    let reloaded = AppState::try_new().expect("reload app state");

    let status = reloaded.proxy_service.get_status().await;
    assert!(
        status.running,
        "reloaded app state should see the already-running foreground proxy runtime"
    );
    assert_eq!(
        status.port, started.port,
        "reloaded app state should point at the same proxy runtime instance"
    );

    reloaded
        .proxy_service
        .stop()
        .await
        .expect("stop proxy from reloaded state");
}

#[tokio::test]
#[serial]
async fn reloaded_app_state_can_stop_active_proxy_runtime() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let state = AppState::try_new().expect("create app state");
    let mut config = state
        .proxy_service
        .get_config()
        .await
        .expect("get proxy config");
    config.listen_port = 0;
    state
        .proxy_service
        .start_with_runtime_config(config)
        .await
        .expect("start proxy");
    let reloaded = AppState::try_new().expect("reload app state");

    reloaded
        .proxy_service
        .stop()
        .await
        .expect("stop proxy from reloaded state");

    assert!(
        !state.proxy_service.is_running().await,
        "stopping through a reloaded app state should stop the active foreground runtime"
    );
}

#[tokio::test]
async fn proxy_service_status_falls_back_to_persisted_foreground_session() {
    let db = Arc::new(Database::memory().expect("create database"));
    db.set_setting(
        "proxy_runtime_session",
        &json!({
            "pid": std::process::id(),
            "address": "127.0.0.1",
            "port": 24567,
            "started_at": "2026-03-10T00:00:00Z",
            "kind": "foreground"
        })
        .to_string(),
    )
    .expect("persist proxy runtime session marker");

    let service = ProxyService::new(db);
    let status = service.get_status().await;

    assert!(
        status.running,
        "status should surface a persisted foreground proxy session when no in-memory runtime is attached"
    );
    assert_eq!(
        status.address, "127.0.0.1",
        "status should use the persisted runtime address"
    );
    assert_eq!(
        status.port, 24567,
        "status should use the persisted runtime port"
    );
}

#[tokio::test]
async fn proxy_service_ignores_stale_external_session_marker_without_live_status() {
    let db = Arc::new(Database::memory().expect("create database"));
    db.set_setting(
        "proxy_runtime_session",
        &json!({
            "pid": std::process::id(),
            "address": "127.0.0.1",
            "port": find_free_port(),
            "started_at": "2026-03-10T00:00:00Z",
            "kind": "managed_external"
        })
        .to_string(),
    )
    .expect("persist stale external runtime session marker");

    let service = ProxyService::new(db.clone());
    let status = service.get_status().await;

    assert!(
        !status.running,
        "external session markers should not report running unless local /status confirms the proxy is still ours"
    );
    assert!(
        db.get_setting("proxy_runtime_session")
            .expect("read runtime session marker")
            .is_none(),
        "stale external session markers should be cleared once validation fails"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn proxy_service_does_not_kill_unrelated_process_for_stale_external_marker() {
    let mut unrelated = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn unrelated process");
    let unrelated_pid = unrelated.id();

    let db = Arc::new(Database::memory().expect("create database"));
    db.set_setting(
        "proxy_runtime_session",
        &json!({
            "pid": unrelated_pid,
            "address": "127.0.0.1",
            "port": find_free_port(),
            "started_at": "2026-03-10T00:00:00Z",
            "kind": "managed_external"
        })
        .to_string(),
    )
    .expect("persist stale external runtime session marker");

    let service = ProxyService::new(db.clone());
    let err = service
        .stop()
        .await
        .expect_err("stale external marker should report not running");
    assert_eq!(err, "proxy server is not running");

    assert!(
        is_process_alive(unrelated_pid),
        "stale external markers must not terminate unrelated live processes"
    );
    assert!(
        db.get_setting("proxy_runtime_session")
            .expect("read runtime session marker after stop")
            .is_none(),
        "stop should clear the stale external marker after refusing to kill the unrelated process"
    );

    let _ = unrelated.kill();
    let _ = unrelated.wait();
}

#[cfg(not(unix))]
#[tokio::test]
async fn proxy_service_managed_session_start_is_explicitly_unsupported_on_non_unix() {
    let db = Arc::new(Database::memory().expect("create database"));
    let service = ProxyService::new(db);

    let error = service
        .start_managed_session("claude")
        .await
        .expect_err("managed session start should return an explicit unsupported error");

    assert!(
        error.contains("unsupported") || error.contains("unix"),
        "non-unix managed-session start should fail with a clear platform boundary message"
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn reset_test_fs_stops_test_daemon_before_deleting_runtime() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let _test_process_cleanup = TestProcessCleanup::new(home);

    seed_claude_live_config(json!({
        "env": {
            "ANTHROPIC_API_KEY": "claude-live-key"
        }
    }));

    let state = AppState::try_new().expect("create app state");
    set_claude_proxy_port(&state.db, 0).await;

    state
        .proxy_service
        .set_managed_session_for_app("claude", true)
        .await
        .expect("start managed proxy for claude");
    let runtime_pid = load_runtime_session_pid(&state);
    let daemon_pidfile = home.join(".runtime").join("cc-switch").join("daemon.pid");
    let daemon_pid: u32 = std::fs::read_to_string(&daemon_pidfile)
        .expect("read test daemon pidfile")
        .trim()
        .parse()
        .expect("parse test daemon pid");

    drop(state);
    reset_test_fs();

    assert!(
        wait_for_process_exit(runtime_pid, Duration::from_secs(5)),
        "managed proxy worker should stop before reset removes the test runtime directory"
    );
    assert!(
        wait_for_process_exit(daemon_pid, Duration::from_secs(5)),
        "test daemon should stop before reset removes the test runtime directory"
    );
    assert!(
        !home.join(".runtime").exists(),
        "reset should remove the isolated test runtime directory after daemon cleanup"
    );
}

#[cfg(unix)]
#[test]
#[serial]
fn test_process_cleanup_does_not_kill_unrelated_process() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let mut unrelated = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn unrelated process");
    let unrelated_pid = unrelated.id();

    cleanup_test_processes();

    assert!(
        is_process_alive(unrelated_pid),
        "test process cleanup must not kill unrelated live processes"
    );

    let _ = unrelated.kill();
    let _ = unrelated.wait();
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn proxy_service_can_stop_managed_external_proxy_session() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let _test_process_cleanup = TestProcessCleanup::new(home);

    std::fs::create_dir_all(get_claude_settings_path().parent().expect("claude dir"))
        .expect("create claude settings dir");
    std::fs::write(
        get_claude_settings_path(),
        serde_json::to_string_pretty(&json!({
            "env": {
                "ANTHROPIC_API_KEY": "original-key"
            }
        }))
        .expect("serialize claude live config"),
    )
    .expect("seed claude live config");

    let state = AppState::try_new().expect("create app state");
    let listen_port = set_claude_proxy_port(&state.db, find_free_port()).await;

    let started = state
        .proxy_service
        .start_managed_session("claude")
        .await
        .expect("start managed proxy session");
    assert_eq!(
        started.port, listen_port,
        "managed session should reuse the configured listen port"
    );

    let pid = load_runtime_session_pid(&state);
    assert_ne!(
        pid,
        std::process::id(),
        "managed session should be hosted by an external process"
    );

    state
        .proxy_service
        .stop()
        .await
        .expect("stop managed proxy session");

    wait_for(Duration::from_secs(5), || !is_process_alive(pid));
    assert!(
        !state.proxy_service.is_running().await,
        "managed session stop should leave the proxy stopped"
    );

    ensure_process_stopped(pid);
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn managed_proxy_session_is_detached_from_parent_terminal_session() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let _test_process_cleanup = TestProcessCleanup::new(home);

    std::fs::create_dir_all(get_claude_settings_path().parent().expect("claude dir"))
        .expect("create claude settings dir");
    std::fs::write(
        get_claude_settings_path(),
        serde_json::to_string_pretty(&json!({
            "env": {
                "ANTHROPIC_API_KEY": "original-key"
            }
        }))
        .expect("serialize claude live config"),
    )
    .expect("seed claude live config");

    let state = AppState::try_new().expect("create app state");
    set_claude_proxy_port(&state.db, 0).await;
    set_codex_proxy_port(&state.db, find_free_port()).await;

    state
        .proxy_service
        .start_managed_session("claude")
        .await
        .expect("start managed proxy session");

    let pid = load_runtime_session_pid(&state);

    let _cleanup = ManagedSessionCleanup::new(pid);

    let parent_pid = std::process::id();
    let parent_sid = session_id(parent_pid);
    let parent_pgid = process_group_id(parent_pid);
    let child_sid = session_id(pid);
    let child_pgid = process_group_id(pid);

    assert_ne!(
        child_sid, parent_sid,
        "managed proxy must not stay in the same terminal session as the TUI process"
    );
    assert_ne!(
        child_pgid, parent_pgid,
        "managed proxy must not stay in the same process group as the TUI process"
    );
    assert_eq!(
        child_sid, pid as i32,
        "managed proxy should lead its own detached session on unix"
    );
    assert_eq!(
        child_pgid, pid as i32,
        "managed proxy should lead its own process group on unix"
    );

    state
        .proxy_service
        .stop()
        .await
        .expect("stop managed proxy session");
}

#[cfg(unix)]
#[tokio::test]
async fn proxy_service_rejects_managed_session_start_when_foreground_runtime_is_running() {
    let db = Arc::new(Database::memory().expect("create database"));
    let service = ProxyService::new(db);

    let mut config = service.get_config().await.expect("get proxy config");
    config.listen_port = 0;

    service
        .start_with_runtime_config(config)
        .await
        .expect("start foreground proxy runtime");

    let error = service
        .start_managed_session("claude")
        .await
        .expect_err("managed session start should reject an existing foreground runtime");

    assert!(
        error.contains("already running") || error.contains("foreground"),
        "unexpected error: {error}"
    );

    service.stop().await.expect("stop foreground proxy runtime");
}

#[cfg(unix)]
#[tokio::test]
async fn proxy_service_rejects_managed_session_attach_when_foreground_runtime_is_running() {
    let db = Arc::new(Database::memory().expect("create database"));
    let service = ProxyService::new(db);

    let mut config = service.get_config().await.expect("get proxy config");
    config.listen_port = 0;

    service
        .start_with_runtime_config(config)
        .await
        .expect("start foreground proxy runtime");

    let error = service
        .set_managed_session_for_app("claude", true)
        .await
        .expect_err("managed session attach should reject an existing foreground runtime");

    assert!(
        error.contains("foreground") || error.contains("already running"),
        "unexpected error: {error}"
    );

    service.stop().await.expect("stop foreground proxy runtime");
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn proxy_service_reloaded_app_state_keeps_managed_session_running_for_current_app() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let _test_process_cleanup = TestProcessCleanup::new(home);

    std::fs::create_dir_all(get_claude_settings_path().parent().expect("claude dir"))
        .expect("create claude settings dir");
    std::fs::write(
        get_claude_settings_path(),
        serde_json::to_string_pretty(&json!({
            "env": {
                "ANTHROPIC_API_KEY": "original-key"
            }
        }))
        .expect("serialize claude live config"),
    )
    .expect("seed claude live config");

    let state = AppState::try_new().expect("create app state");
    set_claude_proxy_port(&state.db, 0).await;
    set_codex_proxy_port(&state.db, find_free_port()).await;

    state
        .proxy_service
        .set_managed_session_for_app("claude", true)
        .await
        .expect("start managed proxy for claude");

    let reloaded = AppState::try_new().expect("reload app state");
    let status = reloaded.proxy_service.get_status().await;
    assert!(
        status.running,
        "reloaded app state should still see the managed proxy session as running"
    );
    let takeover = reloaded
        .proxy_service
        .get_takeover_status()
        .await
        .expect("read takeover status after reload");
    assert!(
        takeover.claude,
        "reloaded app state should still report Claude as taken over by cc-switch"
    );

    reloaded
        .proxy_service
        .set_managed_session_for_app("claude", false)
        .await
        .expect("stop managed proxy for claude");
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn managed_session_allows_second_supported_app_to_start_its_own_worker() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let _test_process_cleanup = TestProcessCleanup::new(home);

    seed_claude_live_config(json!({
        "env": {
            "ANTHROPIC_API_KEY": "claude-live-key"
        }
    }));
    seed_codex_live_config(
        json!({
            "OPENAI_API_KEY": "codex-live-key"
        }),
        "model_provider = \"openai\"\nbase_url = \"https://api.openai.com/v1\"\n",
    );

    let state = AppState::try_new().expect("create app state");
    set_claude_proxy_port(&state.db, 0).await;
    set_codex_proxy_port(&state.db, find_free_port()).await;

    state
        .proxy_service
        .set_managed_session_for_app("claude", true)
        .await
        .expect("start managed proxy for claude");
    let claude_pid = load_runtime_session_pid_for_app(&state, "claude");
    let _claude_cleanup = ManagedSessionCleanup::new(claude_pid);

    state
        .proxy_service
        .set_managed_session_for_app("codex", true)
        .await
        .expect("start managed proxy for codex");
    let codex_pid = load_runtime_session_pid_for_app(&state, "codex");
    let _codex_cleanup = ManagedSessionCleanup::new(codex_pid);

    let takeover = state
        .proxy_service
        .get_takeover_status()
        .await
        .expect("read takeover status");
    assert!(
        takeover.claude,
        "claude should stay attached to its daemon-managed worker"
    );
    assert!(
        takeover.codex,
        "codex should attach to its daemon-managed worker"
    );
    assert_eq!(
        load_runtime_session_worker_count(&state),
        2,
        "attaching a second app should persist one worker per app"
    );
    let status = state.proxy_service.get_status().await;
    assert_eq!(
        status.active_workers.len(),
        2,
        "status should expose both daemon-managed workers"
    );
    assert!(
        is_process_alive(claude_pid),
        "claude worker should be alive"
    );
    assert!(is_process_alive(codex_pid), "codex worker should be alive");

    state
        .proxy_service
        .set_managed_session_for_app("claude", false)
        .await
        .expect("disable managed proxy for claude");
    state
        .proxy_service
        .set_managed_session_for_app("codex", false)
        .await
        .expect("disable managed proxy for codex");
}

#[tokio::test]
#[serial]
async fn proxy_service_stop_preserves_takeover_state_until_explicit_restore() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let _test_process_cleanup = TestProcessCleanup::new(home);

    seed_claude_live_config(json!({
        "env": {
            "ANTHROPIC_API_KEY": "original-key",
            "ANTHROPIC_BASE_URL": "https://api.anthropic.com"
        }
    }));

    let state = AppState::try_new().expect("create app state");
    set_claude_proxy_port(&state.db, 0).await;

    state
        .proxy_service
        .set_takeover_for_app("claude", true)
        .await
        .expect("enable claude takeover");

    state
        .proxy_service
        .stop()
        .await
        .expect("stop proxy runtime only");

    assert!(
        !state.proxy_service.is_running().await,
        "stop should still stop the runtime"
    );

    let takeover = state
        .proxy_service
        .get_takeover_status()
        .await
        .expect("read takeover status after stop");
    assert!(
        takeover.claude,
        "stop should preserve the claude takeover flag until an explicit restore path runs"
    );
    assert!(
        state
            .db
            .get_live_backup("claude")
            .await
            .expect("load claude live backup after stop")
            .is_some(),
        "stop should preserve the claude live backup for later restore"
    );
    assert!(
        state
            .proxy_service
            .detect_takeover_in_live_config_for_app(&AppType::Claude),
        "stop should leave the claude live config rewritten for takeover"
    );

    let live_after_stop: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(get_claude_settings_path())
            .expect("read claude live config after stop"),
    )
    .expect("parse claude live config after stop");
    assert_eq!(
        live_after_stop
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_API_KEY"))
            .and_then(|value| value.as_str()),
        Some("PROXY_MANAGED"),
        "stop should not restore the original Claude token"
    );

    state
        .proxy_service
        .set_takeover_for_app("claude", false)
        .await
        .expect("explicit restore after stop");
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn managed_session_keeps_runtime_alive_while_another_supported_app_is_attached() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let _test_process_cleanup = TestProcessCleanup::new(home);

    seed_claude_live_config(json!({
        "env": {
            "ANTHROPIC_API_KEY": "claude-live-key"
        }
    }));
    seed_codex_live_config(
        json!({
            "OPENAI_API_KEY": "codex-live-key"
        }),
        "model_provider = \"openai\"\nbase_url = \"https://api.openai.com/v1\"\n",
    );

    let state = AppState::try_new().expect("create app state");
    set_claude_proxy_port(&state.db, 0).await;
    set_codex_proxy_port(&state.db, find_free_port()).await;

    state
        .proxy_service
        .set_managed_session_for_app("claude", true)
        .await
        .expect("start managed proxy for claude");
    state
        .proxy_service
        .set_managed_session_for_app("codex", true)
        .await
        .expect("start managed proxy for codex");
    let claude_pid = load_runtime_session_pid_for_app(&state, "claude");
    let codex_pid = load_runtime_session_pid_for_app(&state, "codex");
    let _claude_cleanup = ManagedSessionCleanup::new(claude_pid);
    let _codex_cleanup = ManagedSessionCleanup::new(codex_pid);

    state
        .proxy_service
        .set_managed_session_for_app("claude", false)
        .await
        .expect("disable managed proxy for claude");

    assert!(
        state.proxy_service.is_running().await,
        "managed routing should stay up while codex is still attached"
    );
    assert!(
        !is_process_alive(claude_pid),
        "claude worker should stop after disabling only claude"
    );
    assert!(
        is_process_alive(codex_pid),
        "codex worker should remain alive after disabling only claude"
    );
    let takeover = state
        .proxy_service
        .get_takeover_status()
        .await
        .expect("read takeover status after disabling claude");
    assert!(
        !takeover.claude,
        "claude should be detached after disabling its managed session"
    );
    assert!(
        takeover.codex,
        "codex should remain attached to its managed worker"
    );
    assert_eq!(
        load_runtime_session_worker_count(&state),
        1,
        "disabling claude should clear only claude's persisted worker"
    );
    assert_eq!(
        load_runtime_session_pid_for_app(&state, "codex"),
        codex_pid,
        "codex worker metadata should remain persisted"
    );

    state
        .proxy_service
        .set_managed_session_for_app("codex", false)
        .await
        .expect("disable managed proxy for codex");
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn managed_session_disable_last_app_does_not_terminate_when_status_probe_fails() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let _test_process_cleanup = TestProcessCleanup::new(home);

    seed_claude_live_config(json!({
        "env": {
            "ANTHROPIC_API_KEY": "claude-live-key"
        }
    }));

    let state = AppState::try_new().expect("create app state");
    set_claude_proxy_port(&state.db, 0).await;

    state
        .proxy_service
        .set_managed_session_for_app("claude", true)
        .await
        .expect("start managed proxy for claude");
    let runtime_pid = load_runtime_session_pid(&state);
    let _cleanup = ManagedSessionCleanup::new(runtime_pid);

    let mut runtime_session: serde_json::Value = serde_json::from_str(
        &state
            .db
            .get_setting("proxy_runtime_session")
            .expect("read runtime session marker")
            .expect("runtime session marker should exist"),
    )
    .expect("parse runtime session marker");
    if let Some(claude_session) = runtime_session
        .get_mut("workers")
        .and_then(|workers| workers.get_mut("claude"))
    {
        claude_session["port"] = json!(find_free_port());
    } else {
        runtime_session["port"] = json!(find_free_port());
    }
    state
        .db
        .set_setting("proxy_runtime_session", &runtime_session.to_string())
        .expect("persist tampered runtime session marker");

    let status = state.proxy_service.get_status().await;
    assert!(
        !status.running,
        "managed external markers should not report running when /status probe is unreachable"
    );
    assert!(
        state
            .db
            .get_setting("proxy_runtime_session")
            .expect("read runtime session marker after unreachable get_status")
            .is_none(),
        "unreachable managed external markers should be cleared instead of treated as owned"
    );

    state
        .proxy_service
        .set_managed_session_for_app("claude", false)
        .await
        .expect("disable final managed app and clear runtime marker");

    assert!(
        is_process_alive(runtime_pid),
        "unreachable status must not terminate a process based only on stale persisted metadata"
    );

    assert!(
        state
            .db
            .get_setting("proxy_runtime_session")
            .expect("read runtime session marker")
            .is_none(),
        "stopping the last managed app should clear persisted runtime marker"
    );

    let global = state
        .proxy_service
        .get_global_config()
        .await
        .expect("read global proxy switch");
    assert!(
        !global.proxy_enabled,
        "stopping the last managed app should persist global proxy enabled=false"
    );

    ensure_process_stopped(runtime_pid);
}

#[cfg(unix)]
#[tokio::test]
async fn proxy_service_does_not_kill_unrelated_process_with_reused_pid_and_token_when_status_unreachable(
) {
    let mut unrelated = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn unrelated process");
    let unrelated_pid = unrelated.id();

    let db = Arc::new(Database::memory().expect("create database"));
    db.set_setting(
        "proxy_runtime_session",
        &json!({
            "pid": unrelated_pid,
            "address": "127.0.0.1",
            "port": find_free_port(),
            "started_at": "2026-03-10T00:00:00Z",
            "kind": "managed_external",
            "session_token": "stale-owned-token"
        })
        .to_string(),
    )
    .expect("persist stale external runtime marker with token");

    let service = ProxyService::new(db.clone());
    let err = service
        .stop()
        .await
        .expect_err("stale tokenized external marker should report not running");
    assert_eq!(err, "proxy server is not running");

    assert!(
        is_process_alive(unrelated_pid),
        "unreachable status plus stale token must not terminate unrelated live processes with reused pids"
    );
    assert!(
        db.get_setting("proxy_runtime_session")
            .expect("read runtime marker after stop")
            .is_none(),
        "stop should clear stale tokenized external markers after refusing to kill unrelated processes"
    );

    let _ = unrelated.kill();
    let _ = unrelated.wait();
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn managed_session_still_rejects_opencode_as_unsupported() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let state = AppState::try_new().expect("create app state");

    let error = state
        .proxy_service
        .set_managed_session_for_app("opencode", true)
        .await
        .expect_err("OpenCode should stay unsupported for managed proxy sessions");

    assert!(
        error.contains("unsupported") || error.contains("not supported"),
        "unexpected error: {error}"
    );
    assert!(
        state
            .db
            .get_setting("proxy_runtime_session")
            .expect("load runtime session setting")
            .is_none(),
        "unsupported apps should not create managed runtime session state"
    );
    assert!(
        !state.proxy_service.is_running().await,
        "unsupported apps should not start a managed runtime"
    );
}

#[tokio::test]
#[serial]
async fn proxy_service_status_prefers_live_status_from_external_session() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let listener = TokioTcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind fake proxy status listener");
    let port = listener
        .local_addr()
        .expect("read fake proxy listener addr")
        .port();
    let expected_status = json!({
        "running": true,
        "address": "127.0.0.1",
        "port": port,
        "active_connections": 2,
        "total_requests": 7,
        "success_requests": 6,
        "failed_requests": 1,
        "success_rate": 85.7,
        "uptime_seconds": 42,
        "current_provider": "Claude Test Provider",
        "current_provider_id": "claude-provider",
        "last_request_at": "2026-03-10T00:00:42Z",
        "last_error": "last upstream failure",
        "failover_count": 0,
        "managed_session_token": "expected-session-token",
        "active_targets": [
            {
                "app_type": "claude",
                "provider_name": "Claude Test Provider",
                "provider_id": "claude-provider"
            }
        ]
    });

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept status request");
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            expected_status.to_string().len(),
            expected_status
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write fake status response");
    });

    let db = Arc::new(Database::memory().expect("create database"));
    db.set_setting(
        "proxy_runtime_session",
        &json!({
            "pid": std::process::id(),
            "address": "127.0.0.1",
            "port": port,
            "started_at": "2026-03-10T00:00:00Z",
            "kind": "managed_external",
            "session_token": "expected-session-token"
        })
        .to_string(),
    )
    .expect("persist proxy runtime session marker");

    let service = ProxyService::new(db);
    let status = service.get_status().await;

    assert_eq!(
        status.total_requests, 7,
        "status should prefer the live /status snapshot over marker-level defaults"
    );
    assert_eq!(
        status.current_provider.as_deref(),
        Some("Claude Test Provider"),
        "status should preserve rich provider details from /status"
    );
    assert_eq!(
        status.active_targets.len(),
        1,
        "status should preserve active target details from /status"
    );

    server.await.expect("fake status server should finish");
}

#[tokio::test]
async fn proxy_service_rejects_external_status_with_mismatched_session_token() {
    let listener = TokioTcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind fake proxy status listener");
    let port = listener
        .local_addr()
        .expect("read fake proxy listener addr")
        .port();
    let mismatched_status = json!({
        "running": true,
        "address": "127.0.0.1",
        "port": port,
        "active_connections": 0,
        "total_requests": 5,
        "success_requests": 5,
        "failed_requests": 0,
        "success_rate": 100.0,
        "uptime_seconds": 12,
        "current_provider": "Wrong Provider",
        "current_provider_id": "wrong-provider",
        "last_request_at": null,
        "last_error": null,
        "failover_count": 0,
        "managed_session_token": "other-session-token"
    });

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept status request");
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            mismatched_status.to_string().len(),
            mismatched_status
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write fake status response");
    });

    let db = Arc::new(Database::memory().expect("create database"));
    db.set_setting(
        "proxy_runtime_session",
        &json!({
            "pid": std::process::id(),
            "address": "127.0.0.1",
            "port": port,
            "started_at": "2026-03-10T00:00:00Z",
            "kind": "managed_external",
            "session_token": "expected-session-token"
        })
        .to_string(),
    )
    .expect("persist proxy runtime session marker");

    let service = ProxyService::new(db.clone());
    let status = service.get_status().await;

    assert!(
        !status.running,
        "managed external sessions must be rejected when /status returns a different session token"
    );
    assert!(
        db.get_setting("proxy_runtime_session")
            .expect("read runtime session marker")
            .is_none(),
        "mismatched session tokens should clear the stale marker"
    );

    server.await.expect("fake status server should finish");
}

#[tokio::test]
async fn proxy_service_get_status_clears_only_stale_worker_from_multi_app_session() {
    let listener = TokioTcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind fake proxy status listener");
    let port = listener
        .local_addr()
        .expect("read fake proxy listener addr")
        .port();
    let healthy_status = json!({
        "running": true,
        "address": "127.0.0.1",
        "port": port,
        "active_connections": 0,
        "total_requests": 1,
        "success_requests": 1,
        "failed_requests": 0,
        "success_rate": 100.0,
        "uptime_seconds": 10,
        "current_provider": null,
        "current_provider_id": null,
        "last_request_at": null,
        "last_error": null,
        "failover_count": 0,
        "managed_session_token": "codex-token"
    });

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept status request");
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            healthy_status.to_string().len(),
            healthy_status
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write fake status response");
    });

    let db = Arc::new(Database::memory().expect("create database"));
    db.set_setting(
        "proxy_runtime_session",
        &json!({
            "workers": {
                "claude": {
                    "pid": 0,
                    "address": "127.0.0.1",
                    "port": find_free_port(),
                    "started_at": "2026-03-10T00:00:00Z",
                    "kind": "managed_external",
                    "session_token": "claude-token",
                    "app_type": "claude"
                },
                "codex": {
                    "pid": std::process::id(),
                    "address": "127.0.0.1",
                    "port": port,
                    "started_at": "2026-03-10T00:00:00Z",
                    "kind": "managed_external",
                    "session_token": "codex-token",
                    "app_type": "codex"
                }
            }
        })
        .to_string(),
    )
    .expect("persist multi-app runtime sessions");

    let service = ProxyService::new(db.clone());
    let status = service.get_status().await;

    assert!(status.running, "healthy codex worker should remain visible");
    assert_eq!(status.active_workers.len(), 1);
    assert_eq!(status.active_workers[0].app_type, "codex");

    let stored: serde_json::Value = serde_json::from_str(
        &db.get_setting("proxy_runtime_session")
            .expect("read runtime session marker")
            .expect("runtime session marker should remain"),
    )
    .expect("parse runtime session marker");
    let workers = stored
        .get("workers")
        .and_then(|value| value.as_object())
        .expect("runtime session workers map");
    assert!(
        !workers.contains_key("claude"),
        "stale worker metadata should be removed for only the stale app"
    );
    assert!(
        workers.contains_key("codex"),
        "healthy worker metadata should not be erased by another stale app"
    );

    server.await.expect("fake status server should finish");
}

#[cfg(unix)]
#[tokio::test]
async fn proxy_service_does_not_kill_process_when_status_token_mismatches() {
    let mut unrelated = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn unrelated process");
    let unrelated_pid = unrelated.id();

    let listener = TokioTcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind fake proxy status listener");
    let port = listener
        .local_addr()
        .expect("read fake proxy listener addr")
        .port();
    let mismatched_status = json!({
        "running": true,
        "address": "127.0.0.1",
        "port": port,
        "active_connections": 0,
        "total_requests": 5,
        "success_requests": 5,
        "failed_requests": 0,
        "success_rate": 100.0,
        "uptime_seconds": 12,
        "current_provider": "Wrong Provider",
        "current_provider_id": "wrong-provider",
        "last_request_at": null,
        "last_error": null,
        "failover_count": 0,
        "managed_session_token": "other-session-token"
    });

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept status request");
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            mismatched_status.to_string().len(),
            mismatched_status
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write fake status response");
    });

    let db = Arc::new(Database::memory().expect("create database"));
    db.set_setting(
        "proxy_runtime_session",
        &json!({
            "pid": unrelated_pid,
            "address": "127.0.0.1",
            "port": port,
            "started_at": "2026-03-10T00:00:00Z",
            "kind": "managed_external",
            "session_token": "expected-session-token"
        })
        .to_string(),
    )
    .expect("persist proxy runtime session marker");

    let service = ProxyService::new(db.clone());
    let err = service
        .stop()
        .await
        .expect_err("mismatched status token should report not running");
    assert_eq!(err, "proxy server is not running");

    assert!(
        is_process_alive(unrelated_pid),
        "stop must not terminate a live process when /status reports a different managed session token"
    );
    assert!(
        db.get_setting("proxy_runtime_session")
            .expect("read runtime session marker after stop")
            .is_none(),
        "mismatched session tokens should clear the stale marker during stop"
    );

    let _ = unrelated.kill();
    let _ = unrelated.wait();
    server.await.expect("fake status server should finish");
}
