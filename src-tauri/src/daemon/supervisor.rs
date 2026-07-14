//! The supervisor: spawns and watches the proxy worker, owns the daemon's
//! shared `ProxyService`, and translates IPC requests into actions.
//!
//! Most of the heavy lifting (config rewrites, restoration, common-config
//! preservation) lives in `ProxyService`. The supervisor's job is to keep one
//! worker per active app route, keep the `proxy_runtime_session` row aligned
//! with the actual workers, and survive worker crashes via the restart policy.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex, Notify};

use crate::app_config::AppType;
use crate::database::Database;
use crate::services::ProxyService;

use super::ipc::protocol::{
    Request, Response, TakeoverFlags, WorkerRuntimeStatus, WorkerState, WorkerTargetState,
};
use super::ipc::server::Handler;
use super::restart::{Decision, RestartPolicy};

const PROXY_RUNTIME_SESSION_KEY: &str = "proxy_runtime_session";
pub const DAEMON_SOCKET_ENV: &str = "CC_SWITCH_DAEMON_SOCKET";
pub const SESSION_TOKEN_ENV: &str = "CC_SWITCH_PROXY_SESSION_TOKEN";
pub const RESTORE_GUARD_BYPASS_ENV: &str = "CC_SWITCH_RESTORE_GUARD_BYPASS";
/// Mirrors `services::proxy::PROXY_RUNTIME_KIND_ENV_KEY`. Setting this to
/// `managed_external` makes the worker skip self-publishing the runtime
/// session row — the daemon writes it after WorkerHello.
pub const RUNTIME_KIND_ENV: &str = "CC_SWITCH_PROXY_RUNTIME_KIND";
pub const RUNTIME_KIND_MANAGED_EXTERNAL: &str = "managed_external";

const WORKER_HELLO_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
struct WorkerInfo {
    app_type: AppType,
    pid: u32,
    address: String,
    port: u16,
    session_token: String,
    started_at: chrono::DateTime<chrono::Utc>,
    adopted: bool,
}

#[derive(Default)]
struct SupervisorInner {
    workers: HashMap<AppType, WorkerInfo>,
    pending_hellos: HashMap<String, oneshot::Sender<WorkerInfo>>,
    pending_tokens: HashMap<String, String>,
    pending_worker_pids: HashMap<AppType, u32>,
    pending_startup_failures: HashMap<AppType, String>,
    stopping_workers: HashSet<(AppType, u32)>,
    cancelled_apps: HashSet<AppType>,
    restart: RestartPolicy,
    last_restart_at: Option<chrono::DateTime<chrono::Utc>>,
    restart_count: u32,
    shutdown_requested: bool,
    teardown_in_progress: bool,
}

struct WorkerStopPlan {
    pids: Vec<u32>,
    adopted_pids: Vec<u32>,
    should_shutdown: bool,
    previous_shutdown_requested: bool,
    cancelled_pending: Vec<CancelledPendingWorker>,
    removed_adopted_workers: Vec<(AppType, WorkerInfo)>,
}

struct WorkerStopAllPlan {
    pids: Vec<u32>,
    adopted_pids: Vec<u32>,
}

struct CancelledPendingWorker {
    app: AppType,
    pid: u32,
    token: Option<String>,
    hello: Option<oneshot::Sender<WorkerInfo>>,
}

#[derive(Clone)]
pub struct Supervisor {
    db: Arc<Database>,
    proxy: ProxyService,
    inner: Arc<Mutex<SupervisorInner>>,
    /// Serializes worker spawn so concurrent EnsureWorker IPCs share the same
    /// pending hello rather than racing — a second caller used to overwrite
    /// `pending_hello` and `pending_token`, leaving the first caller waiting
    /// the full 10 s `WORKER_HELLO_TIMEOUT` and then surfacing as
    /// "Resource temporarily unavailable (os error 35)" once the client's 15 s
    /// IPC read timeout expired.
    spawn_lock: Arc<Mutex<()>>,
    socket_path: PathBuf,
    binary_path: PathBuf,
    shutdown_notify: Arc<Notify>,
}

impl Supervisor {
    pub fn new(db: Arc<Database>, socket_path: PathBuf, binary_path: PathBuf) -> Self {
        let proxy = ProxyService::new(db.clone());
        Self {
            db,
            proxy,
            inner: Arc::new(Mutex::new(SupervisorInner::default())),
            spawn_lock: Arc::new(Mutex::new(())),
            socket_path,
            binary_path,
            shutdown_notify: Arc::new(Notify::new()),
        }
    }

    pub fn shutdown_signal(&self) -> Arc<Notify> {
        self.shutdown_notify.clone()
    }

    pub async fn recover_on_startup(&self) -> Result<(), String> {
        self.adopt_persisted_workers_on_startup().await?;
        self.proxy.recover_takeovers_on_startup().await
    }

    async fn adopt_persisted_workers_on_startup(&self) -> Result<(), String> {
        let sessions = self
            .proxy
            .load_live_managed_runtime_sessions_for_recovery()
            .await;
        for session in sessions {
            let started_at = chrono::DateTime::parse_from_rfc3339(&session.started_at)
                .map(|value| value.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now());
            let info = WorkerInfo {
                app_type: session.app_type.clone(),
                pid: session.pid,
                address: session.address,
                port: session.port,
                session_token: session.session_token,
                started_at,
                adopted: true,
            };
            {
                let mut inner = self.inner.lock().await;
                inner.workers.insert(session.app_type.clone(), info);
                inner.cancelled_apps.remove(&session.app_type);
                inner.shutdown_requested = false;
                inner.restart.on_worker_started(Instant::now());
            }
            log::info!(
                "[daemon] adopted existing {} worker pid={}",
                session.app_type.as_str(),
                session.pid
            );
        }
        self.persist_runtime_session().await
    }

    async fn ensure_worker_locked(&self, app: AppType) -> Result<WorkerInfo, String> {
        let app_key = app.as_str().to_string();

        let (session_token, hello_rx) = {
            let mut inner = self.inner.lock().await;
            if inner.shutdown_requested || inner.teardown_in_progress {
                return Err("proxy daemon is shutting down".to_string());
            }
            if let Some(info) = inner.workers.get(&app).cloned() {
                if inner.stopping_workers.contains(&(app.clone(), info.pid)) {
                    return Err(format!(
                        "{app_key} proxy worker is stopping; retry after it exits"
                    ));
                }
                inner.cancelled_apps.remove(&app);
                return Ok(info);
            }
            if inner
                .stopping_workers
                .iter()
                .any(|(stopping_app, _)| stopping_app == &app)
            {
                return Err(format!(
                    "{app_key} proxy worker is stopping; retry after it exits"
                ));
            }
            inner.cancelled_apps.remove(&app);
            inner.pending_startup_failures.remove(&app);
            let (tx, rx) = oneshot::channel();
            inner.pending_hellos.insert(app_key.clone(), tx);
            let token = uuid::Uuid::new_v4().to_string();
            inner.pending_tokens.insert(app_key.clone(), token.clone());
            (token, rx)
        };

        let global_config = match self.db.get_global_proxy_config().await {
            Ok(config) => config,
            Err(err) => {
                self.clear_pending_worker_registration(&app).await;
                return Err(format!("load global proxy config failed: {err}"));
            }
        };
        let listen_port = match self.db.get_app_proxy_preferred_port(&app_key) {
            Ok(port) => port,
            Err(err) => {
                self.clear_pending_worker_registration(&app).await;
                return Err(format!("load proxy preference for {app_key} failed: {err}"));
            }
        };

        let mut cmd = Command::new(&self.binary_path);
        cmd.arg("proxy")
            .arg("serve")
            .arg("--listen-address")
            .arg(global_config.listen_address)
            .arg("--listen-port")
            .arg(listen_port.to_string())
            .env(DAEMON_SOCKET_ENV, &self.socket_path)
            .env(SESSION_TOKEN_ENV, &session_token)
            .env(RESTORE_GUARD_BYPASS_ENV, "1")
            .env(RUNTIME_KIND_ENV, RUNTIME_KIND_MANAGED_EXTERNAL)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        #[cfg(unix)]
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut spawned = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                self.clear_pending_worker_registration(&app).await;
                return Err(format!("spawn {app_key} proxy worker failed: {err}"));
            }
        };
        let stderr = spawned.stderr.take();
        let pid = match spawned.id() {
            Some(pid) => pid,
            None => {
                self.clear_pending_worker_registration(&app).await;
                return Err(format!("spawned {app_key} worker has no pid"));
            }
        };
        {
            let mut inner = self.inner.lock().await;
            inner.pending_worker_pids.insert(app.clone(), pid);
        }
        log::info!("[daemon] spawned {app_key} worker pid={pid}");

        let supervisor = self.clone();
        let watch_app = app.clone();
        tokio::spawn(async move {
            supervisor
                .watch_worker(spawned, watch_app, pid, stderr)
                .await;
        });

        let info = match tokio::time::timeout(WORKER_HELLO_TIMEOUT, hello_rx).await {
            Ok(Ok(info)) => info,
            Ok(Err(_)) => {
                self.clear_pending_worker_registration(&app).await;
                if let Some(reason) = self.take_pending_startup_failure(&app).await {
                    return Err(format!("{app_key} worker exited before hello: {reason}"));
                }
                return Err(format!("{app_key} worker exited before hello"));
            }
            Err(_) => {
                self.abandon_starting_worker(&app, Some(pid)).await;
                return Err(format!("{app_key} worker hello timed out"));
            }
        };

        let became_stopping = {
            let inner = self.inner.lock().await;
            inner.shutdown_requested
                || inner.teardown_in_progress
                || inner.stopping_workers.contains(&(app.clone(), info.pid))
        };
        if became_stopping {
            self.abandon_starting_worker(&app, Some(info.pid)).await;
            return Err("proxy daemon is shutting down".to_string());
        }

        {
            let mut inner = self.inner.lock().await;
            inner.workers.insert(app.clone(), info.clone());
            inner.last_restart_at = Some(chrono::Utc::now());
            inner.restart.on_worker_started(Instant::now());
            inner.pending_tokens.remove(&app_key);
            inner.pending_worker_pids.remove(&app);
            inner.shutdown_requested = false;
        }
        self.persist_runtime_session().await?;
        Ok(info)
    }

    async fn handle_ensure_worker(
        &self,
        app_type: &str,
        fallback_provider_id: Option<&str>,
    ) -> Response {
        let app = match parse_app_type(app_type) {
            Some(a) => a,
            None => {
                return Response::Error {
                    message: format!("proxy takeover not supported for app: {app_type}"),
                };
            }
        };

        if let Err(err) = self
            .proxy
            .validate_app_proxy_activation(&app, fallback_provider_id)
            .await
        {
            return Response::Error { message: err };
        }

        let _spawn_guard = self.spawn_lock.lock().await;
        let info = match self.ensure_worker_locked(app.clone()).await {
            Ok(info) => info,
            Err(err) => return Response::Error { message: err },
        };

        let activation = async {
            self.proxy
                .set_global_enabled(true)
                .await
                .map_err(|err| err.to_string())?;
            self.proxy
                .enable_takeover_for_daemon_worker(app.as_str(), fallback_provider_id)
                .await
        }
        .await;

        if let Err(err) = activation {
            log::warn!(
                "[daemon] enabling {} takeover failed after worker start, cleaning up: {err}",
                app.as_str()
            );
            self.stop_worker_after_enable_failure(app.clone()).await;
            return Response::Error { message: err };
        }

        Response::Worker {
            address: info.address,
            port: info.port,
            session_token: info.session_token,
            pid: info.pid,
            started_at: Some(info.started_at.to_rfc3339()),
        }
    }

    async fn clear_pending_worker_registration(&self, app: &AppType) {
        let app_key = app.as_str().to_string();
        let mut inner = self.inner.lock().await;
        inner.pending_tokens.remove(&app_key);
        inner.pending_hellos.remove(&app_key);
        inner.pending_worker_pids.remove(app);
    }

    async fn take_pending_startup_failure(&self, app: &AppType) -> Option<String> {
        let mut inner = self.inner.lock().await;
        inner.pending_startup_failures.remove(app)
    }

    async fn abandon_starting_worker(&self, app: &AppType, pid: Option<u32>) {
        let app_key = app.as_str().to_string();
        {
            let mut inner = self.inner.lock().await;
            inner.pending_tokens.remove(&app_key);
            inner.pending_hellos.remove(&app_key);
            inner.pending_worker_pids.remove(app);
            inner.pending_startup_failures.remove(app);
            if let Some(pid) = pid {
                inner.stopping_workers.insert((app.clone(), pid));
            }
        }
        if let Err(err) = send_sigterm(pid) {
            log::warn!("[daemon] stopping abandoned {app_key} worker failed: {err}");
        }
    }

    async fn stop_worker_after_enable_failure(&self, app: AppType) {
        let plan = self.plan_stop_for_app(app.clone()).await;

        if let Err(err) = self.proxy.clear_daemon_takeover_for_app(app.as_str()).await {
            log::warn!(
                "[daemon] restoring {} takeover after enable failure failed: {err}",
                app.as_str()
            );
        }

        if let Err(err) = self.persist_runtime_session().await {
            log::warn!(
                "[daemon] clearing runtime session after {} enable failure failed: {err}",
                app.as_str()
            );
        }

        let takeovers = self.read_takeover_flags().await;
        let has_active_takeover = takeovers.claude || takeovers.codex || takeovers.gemini;
        if !has_active_takeover {
            if let Err(err) = self.proxy.set_global_enabled(false).await {
                log::warn!(
                    "[daemon] clearing global proxy switch after {} enable failure failed: {err}",
                    app.as_str()
                );
            }
        }
        for pid in &plan.pids {
            if let Err(err) = send_sigterm(Some(*pid)) {
                log::warn!(
                    "[daemon] stopping {} worker after enable failure failed: {err}",
                    app.as_str()
                );
            }
        }
        if plan.should_shutdown && plan.pids.is_empty() {
            self.shutdown_notify.notify_waiters();
        }
    }

    fn has_remaining_workers_locked(inner: &SupervisorInner) -> bool {
        !inner.workers.is_empty() || !inner.pending_worker_pids.is_empty()
    }

    fn remaining_workers_are_only_stopping_locked(inner: &SupervisorInner) -> bool {
        Self::has_remaining_workers_locked(inner)
            && inner
                .workers
                .iter()
                .all(|(app, worker)| inner.stopping_workers.contains(&(app.clone(), worker.pid)))
            && inner
                .pending_worker_pids
                .iter()
                .all(|(app, pid)| inner.stopping_workers.contains(&(app.clone(), *pid)))
    }

    async fn plan_stop_for_app(&self, app: AppType) -> WorkerStopPlan {
        let app_key = app.as_str().to_string();
        let mut inner = self.inner.lock().await;
        let mut pids = Vec::new();
        let mut adopted_pids = Vec::new();
        let previous_shutdown_requested = inner.shutdown_requested;
        let mut cancelled_pending = Vec::new();
        let mut removed_adopted_workers = Vec::new();
        inner.cancelled_apps.insert(app.clone());

        if let Some(info) = inner.workers.get(&app).cloned() {
            let pid = info.pid;
            pids.push(pid);
            if info.adopted {
                adopted_pids.push(pid);
                inner.workers.remove(&app);
                removed_adopted_workers.push((app.clone(), info));
            } else {
                inner.stopping_workers.insert((app.clone(), pid));
            }
        }
        if let Some(pid) = inner.pending_worker_pids.remove(&app) {
            inner.stopping_workers.insert((app.clone(), pid));
            pids.push(pid);
            cancelled_pending.push(CancelledPendingWorker {
                app: app.clone(),
                pid,
                token: inner.pending_tokens.remove(&app_key),
                hello: inner.pending_hellos.remove(&app_key),
            });
        }

        pids.sort_unstable();
        pids.dedup();
        let target_had_worker = !pids.is_empty();
        let no_remaining_workers = !Self::has_remaining_workers_locked(&inner)
            || (target_had_worker && Self::remaining_workers_are_only_stopping_locked(&inner));
        if target_had_worker && no_remaining_workers {
            inner.shutdown_requested = true;
        }

        WorkerStopPlan {
            pids,
            adopted_pids,
            should_shutdown: target_had_worker && no_remaining_workers,
            previous_shutdown_requested,
            cancelled_pending,
            removed_adopted_workers,
        }
    }

    async fn rollback_stop_plan_for_app(&self, app: &AppType, mut plan: WorkerStopPlan) {
        let mut inner = self.inner.lock().await;
        for pid in &plan.pids {
            inner.stopping_workers.remove(&(app.clone(), *pid));
        }
        inner.cancelled_apps.remove(app);
        for pending in plan.cancelled_pending.drain(..) {
            let app_key = pending.app.as_str().to_string();
            inner.pending_worker_pids.insert(pending.app, pending.pid);
            if let Some(token) = pending.token {
                inner.pending_tokens.insert(app_key.clone(), token);
            }
            if let Some(hello) = pending.hello {
                inner.pending_hellos.insert(app_key, hello);
            }
        }
        for (worker_app, worker) in plan.removed_adopted_workers.drain(..) {
            inner.workers.insert(worker_app, worker);
        }
        inner.shutdown_requested = plan.previous_shutdown_requested;
    }

    async fn plan_stop_all_workers(&self, teardown_in_progress: bool) -> WorkerStopAllPlan {
        let mut inner = self.inner.lock().await;
        inner.shutdown_requested = true;
        if teardown_in_progress {
            inner.teardown_in_progress = true;
        }
        inner
            .cancelled_apps
            .extend([AppType::Claude, AppType::Codex, AppType::Gemini]);

        let workers = inner
            .workers
            .iter()
            .map(|(app, worker)| (app.clone(), worker.pid, worker.adopted))
            .collect::<Vec<_>>();
        let pending = inner
            .pending_worker_pids
            .iter()
            .map(|(app, pid)| (app.clone(), *pid))
            .collect::<Vec<_>>();

        let mut pids = Vec::new();
        let mut adopted_pids = Vec::new();
        for (app, pid, adopted) in workers {
            pids.push(pid);
            if adopted {
                adopted_pids.push(pid);
                inner.workers.remove(&app);
            } else {
                inner.stopping_workers.insert((app, pid));
            }
        }
        for (app, pid) in pending {
            inner.stopping_workers.insert((app, pid));
            pids.push(pid);
        }

        let pending_apps = inner
            .pending_worker_pids
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for app in pending_apps {
            inner.pending_worker_pids.remove(&app);
            let app_key = app.as_str().to_string();
            inner.pending_tokens.remove(&app_key);
            if let Some(tx) = inner.pending_hellos.remove(&app_key) {
                drop(tx);
            }
        }

        pids.sort_unstable();
        pids.dedup();
        adopted_pids.sort_unstable();
        adopted_pids.dedup();
        WorkerStopAllPlan { pids, adopted_pids }
    }

    async fn stop_planned_workers(&self, pids: &[u32], adopted_pids: &[u32]) {
        for pid in pids {
            let _ = send_sigterm(Some(*pid));
        }
        if adopted_pids.is_empty() {
            return;
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            if adopted_pids
                .iter()
                .all(|pid| !is_process_alive_for_signal(*pid))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        for pid in adopted_pids {
            if is_process_alive_for_signal(*pid) {
                let _ = send_sigkill(Some(*pid));
            }
        }
    }

    async fn handle_drop_takeover(&self, app_type: &str) -> Response {
        let app = match parse_app_type(app_type) {
            Some(a) => a,
            None => {
                return Response::Error {
                    message: format!("proxy takeover not supported for app: {app_type}"),
                };
            }
        };

        let _spawn_guard = self.spawn_lock.lock().await;
        let stop_plan = self.plan_stop_for_app(app.clone()).await;
        if let Err(err) = self.proxy.clear_daemon_takeover_for_app(app.as_str()).await {
            self.rollback_stop_plan_for_app(&app, stop_plan).await;
            return Response::Error { message: err };
        }
        let takeovers = self.read_takeover_flags().await;
        let has_active_takeover = takeovers.claude || takeovers.codex || takeovers.gemini;
        let mut global_disable_error = None;
        if !has_active_takeover {
            if let Err(err) = self.proxy.set_global_enabled(false).await {
                global_disable_error = Some(err.to_string());
            }
        }

        self.stop_planned_workers(&stop_plan.pids, &stop_plan.adopted_pids)
            .await;
        if !stop_plan.adopted_pids.is_empty() {
            let _ = self.persist_runtime_session().await;
        }
        let has_spawned_worker = stop_plan
            .pids
            .iter()
            .any(|pid| !stop_plan.adopted_pids.contains(pid));
        if has_spawned_worker {
            tokio::time::sleep(Duration::from_millis(100)).await;
        } else if stop_plan.should_shutdown {
            self.shutdown_notify.notify_waiters();
        }
        if let Some(message) = global_disable_error {
            return Response::Error { message };
        }
        Response::Ok
    }

    async fn handle_worker_hello(
        &self,
        pid: u32,
        address: String,
        port: u16,
        session_token: String,
    ) -> Response {
        let mut inner = self.inner.lock().await;
        let app_key = inner
            .pending_tokens
            .iter()
            .find_map(|(app_type, token)| (token == &session_token).then(|| app_type.clone()));
        let Some(app_key) = app_key else {
            log::warn!("[daemon] worker hello with mismatched token (pid={pid})");
            return Response::Error {
                message: "session token mismatch".to_string(),
            };
        };
        let Some(app_type) = parse_app_type(&app_key) else {
            return Response::Error {
                message: format!("proxy takeover not supported for app: {app_key}"),
            };
        };
        if let Some(expected_pid) = inner.pending_worker_pids.get(&app_type) {
            if *expected_pid != pid {
                log::warn!(
                    "[daemon] worker hello pid mismatch for {app_key}: expected {expected_pid}, got {pid}"
                );
                return Response::Error {
                    message: "worker pid mismatch".to_string(),
                };
            }
        }
        let Some(tx) = inner.pending_hellos.remove(&app_key) else {
            log::warn!("[daemon] worker hello received but no pending ensure (pid={pid})");
            return Response::Error {
                message: "no pending worker registration".to_string(),
            };
        };
        inner.pending_worker_pids.remove(&app_type);
        let info = WorkerInfo {
            app_type,
            pid,
            address,
            port,
            session_token,
            started_at: chrono::Utc::now(),
            adopted: false,
        };
        if tx.send(info).is_err() {
            log::warn!("[daemon] worker hello dropped: ensure waiter cancelled");
        }
        Response::Ok
    }

    async fn handle_set_global_enabled(&self, enabled: bool) -> Response {
        if enabled {
            match self.proxy.set_global_enabled(true).await {
                Ok(_) => return Response::Ok,
                Err(err) => {
                    return Response::Error {
                        message: err.to_string(),
                    };
                }
            }
        }

        let _spawn_guard = self.spawn_lock.lock().await;
        if let Err(err) = self.proxy.set_global_enabled(false).await {
            return Response::Error {
                message: err.to_string(),
            };
        }

        // Disabling: drop every active takeover so each app's live config is
        // restored, then stop the worker. We snapshot the active list under
        // the inner lock so we don't hold it while running per-app restores
        // (which acquire the file-level state mutation guard).
        let mut active = Vec::new();
        for app in [AppType::Claude, AppType::Codex, AppType::Gemini] {
            match self.db.get_proxy_config_for_app(app.as_str()).await {
                Ok(config) if config.enabled => active.push(app),
                Ok(_) => {}
                Err(err) => log::warn!(
                    "[daemon] set_global_enabled(false): read {} proxy config failed: {err}",
                    app.as_str()
                ),
            }
        }
        for app in &active {
            if let Err(err) = self.proxy.clear_daemon_takeover_for_app(app.as_str()).await {
                log::warn!(
                    "[daemon] set_global_enabled(false): drop takeover for {} failed: {err}",
                    app.as_str()
                );
            }
        }

        let stop_plan = self.plan_stop_all_workers(false).await;
        self.stop_planned_workers(&stop_plan.pids, &stop_plan.adopted_pids)
            .await;
        if !stop_plan.adopted_pids.is_empty() {
            let _ = self.persist_runtime_session().await;
        }
        let has_spawned_worker = stop_plan
            .pids
            .iter()
            .any(|pid| !stop_plan.adopted_pids.contains(pid));
        if has_spawned_worker {
            // Brief pause so the worker has a chance to exit and the watcher
            // task can clear the runtime session row before we ack. The
            // watcher then sees `active_takeovers.is_empty()` and signals
            // daemon shutdown.
            tokio::time::sleep(Duration::from_millis(100)).await;
        } else {
            // No worker to drain — signal shutdown directly so the daemon
            // doesn't stay idle after a "disable everything" with nothing
            // currently running.
            self.shutdown_notify.notify_waiters();
        }
        Response::Ok
    }

    async fn handle_status(&self) -> Response {
        let (worker_infos, restart_count, last_restart_at) = {
            let inner = self.inner.lock().await;
            let worker_infos = inner.workers.values().cloned().collect::<Vec<_>>();
            (worker_infos, inner.restart_count, inner.last_restart_at)
        };
        let takeovers = self.read_takeover_flags().await;
        let mut worker_infos = worker_infos;
        worker_infos.sort_by(|left, right| left.app_type.as_str().cmp(right.app_type.as_str()));
        let mut workers = Vec::with_capacity(worker_infos.len());
        for info in worker_infos {
            let runtime_status = self.probe_worker_runtime_status(&info).await;
            workers.push(WorkerState {
                app_type: info.app_type.as_str().to_string(),
                running: true,
                address: info.address.clone(),
                port: info.port,
                pid: Some(info.pid),
                started_at: Some(info.started_at.to_rfc3339()),
                runtime_status,
            });
        }
        workers.sort_by(|left, right| left.app_type.cmp(&right.app_type));
        let primary = workers.first();
        Response::Status {
            running: !workers.is_empty(),
            address: primary.map(|w| w.address.clone()).unwrap_or_default(),
            port: primary.map(|w| w.port).unwrap_or_default(),
            worker_pid: primary.and_then(|w| w.pid),
            takeovers,
            restart_count,
            last_restart_at: last_restart_at.map(|d| d.to_rfc3339()),
            workers,
        }
    }

    async fn probe_worker_runtime_status(&self, info: &WorkerInfo) -> Option<WorkerRuntimeStatus> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(500))
            .build()
            .ok()?;
        let response = client
            .get(worker_status_url(&info.address, info.port))
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }

        let status = response
            .json::<crate::proxy::types::ProxyStatus>()
            .await
            .ok()?;
        if status.managed_session_token.as_deref() != Some(info.session_token.as_str()) {
            return None;
        }

        Some(WorkerRuntimeStatus {
            active_connections: status.active_connections,
            total_requests: status.total_requests,
            estimated_input_tokens_total: status.estimated_input_tokens_total,
            estimated_output_tokens_total: status.estimated_output_tokens_total,
            success_requests: status.success_requests,
            failed_requests: status.failed_requests,
            uptime_seconds: status.uptime_seconds,
            current_provider: status.current_provider,
            current_provider_id: status.current_provider_id,
            last_request_at: status.last_request_at,
            last_error: status.last_error,
            failover_count: status.failover_count,
            active_targets: status
                .active_targets
                .into_iter()
                .map(|target| WorkerTargetState {
                    app_type: target.app_type,
                    provider_name: target.provider_name,
                    provider_id: target.provider_id,
                })
                .collect(),
        })
    }

    pub async fn shutdown(&self) {
        let _spawn_guard = self.spawn_lock.lock().await;
        let stop_plan = self.plan_stop_all_workers(true).await;
        self.stop_planned_workers(&stop_plan.pids, &stop_plan.adopted_pids)
            .await;
        if let Err(err) = self.proxy.stop_with_restore().await {
            log::warn!("[daemon] shutdown: stop_with_restore failed: {err}");
        }
        let _ = self.clear_runtime_session();
        self.shutdown_notify.notify_waiters();
    }

    async fn handle_shutdown(&self) -> Response {
        self.shutdown().await;
        Response::Ok
    }

    async fn read_takeover_flags(&self) -> TakeoverFlags {
        let status = self.proxy.get_takeover_status().await.unwrap_or_default();
        TakeoverFlags {
            claude: status.claude,
            codex: status.codex,
            gemini: status.gemini,
        }
    }

    async fn watch_worker(
        &self,
        mut child: Child,
        app: AppType,
        pid: u32,
        stderr: Option<tokio::process::ChildStderr>,
    ) {
        let app_key = app.as_str().to_string();
        let stderr_task = stderr.map(|mut stderr| {
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                match stderr.read_to_end(&mut bytes).await {
                    Ok(_) => Some(bytes),
                    Err(err) => Some(format!("failed to read worker stderr: {err}").into_bytes()),
                }
            })
        });
        let exit_status = match child.wait().await {
            Ok(status) => status,
            Err(err) => {
                log::warn!("[daemon] waitpid {app_key} worker={pid} failed: {err}");
                return;
            }
        };
        let stderr_output = match stderr_task {
            Some(task) => task.await.ok().flatten().unwrap_or_default(),
            None => Vec::new(),
        };
        let startup_failure = worker_exit_message(&exit_status, &stderr_output);
        log::info!("[daemon] {app_key} worker pid={pid} exited: {exit_status}");
        if let Some(message) = startup_failure.as_deref() {
            log::warn!("[daemon] {app_key} worker pid={pid} stderr: {message}");
        }

        let (intentional, has_remaining_workers, teardown_in_progress) =
            self.record_worker_exit(&app, pid, startup_failure).await;

        let _ = self.persist_runtime_session().await;

        if intentional {
            log::info!("[daemon] {app_key} worker exit was expected, not restarting");
            if !has_remaining_workers && !teardown_in_progress {
                log::info!("[daemon] no remaining workers, exiting");
                self.shutdown_notify.notify_waiters();
            }
            return;
        }

        if let Err(err) = self.proxy.clear_daemon_takeover_for_app(app.as_str()).await {
            log::warn!("[daemon] restore takeover for {app_key} failed: {err}");
        }

        let decision = {
            let mut inner = self.inner.lock().await;
            inner.restart.on_worker_exited(Instant::now())
        };

        match decision {
            Decision::Restart { delay, attempt } => {
                log::warn!(
                    "[daemon] {app_key} worker pid={pid} crashed; restarting in {:?} (attempt {})",
                    delay,
                    attempt + 1
                );
                tokio::time::sleep(delay).await;
                if !self.should_restart_after_crash(&app).await {
                    log::info!(
                        "[daemon] {} worker restart cancelled after route was disabled",
                        app.as_str()
                    );
                    if !has_remaining_workers && !teardown_in_progress {
                        self.shutdown_notify.notify_waiters();
                    }
                    return;
                }
                {
                    let mut inner = self.inner.lock().await;
                    inner.restart_count = inner.restart_count.saturating_add(1);
                }
                if let Err(err) = self.respawn_after_crash(app).await {
                    log::error!("[daemon] respawn {app_key} after crash failed: {err}");
                }
            }
            Decision::GiveUp => {
                log::error!(
                    "[daemon] {app_key} worker pid={pid} circuit-broke after repeated crashes"
                );
                if !has_remaining_workers && !teardown_in_progress {
                    self.shutdown_notify.notify_waiters();
                }
            }
        }
    }

    async fn record_worker_exit(
        &self,
        app: &AppType,
        pid: u32,
        startup_failure: Option<String>,
    ) -> (bool, bool, bool) {
        let app_key = app.as_str().to_string();
        let mut inner = self.inner.lock().await;

        let registered_pid = inner.workers.get(app).map(|worker| worker.pid);
        let was_registered = registered_pid == Some(pid);
        if was_registered {
            inner.workers.remove(app);
        }

        let pending_pid = inner.pending_worker_pids.get(app).copied();
        let was_pending_startup = pending_pid == Some(pid);
        if was_pending_startup {
            inner.pending_worker_pids.remove(app);
            inner.pending_tokens.remove(&app_key);
            if let Some(message) = startup_failure {
                inner.pending_startup_failures.insert(app.clone(), message);
            }
            if let Some(tx) = inner.pending_hellos.remove(&app_key) {
                drop(tx);
            }
        }

        let was_stopping = inner.stopping_workers.remove(&(app.clone(), pid));
        let stale_exit = registered_pid.is_some_and(|current_pid| current_pid != pid)
            || pending_pid.is_some_and(|current_pid| current_pid != pid);
        let intentional = inner.shutdown_requested
            || was_stopping
            || (!was_registered && was_pending_startup)
            || stale_exit;

        let has_remaining_workers = Self::has_remaining_workers_locked(&inner);
        (
            intentional,
            has_remaining_workers,
            inner.teardown_in_progress,
        )
    }

    async fn should_restart_after_crash(&self, app: &AppType) -> bool {
        let inner = self.inner.lock().await;
        !inner.shutdown_requested
            && !inner.teardown_in_progress
            && !inner.cancelled_apps.contains(app)
    }

    fn respawn_after_crash<'a>(
        &'a self,
        app: AppType,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            let _spawn_guard = self.spawn_lock.lock().await;
            if !self.should_restart_after_crash(&app).await {
                return Err(format!(
                    "{} proxy worker restart was cancelled",
                    app.as_str()
                ));
            }
            let _info = self.ensure_worker_locked(app.clone()).await?;
            {
                let inner = self.inner.lock().await;
                if inner.shutdown_requested
                    || inner.teardown_in_progress
                    || inner.cancelled_apps.contains(&app)
                {
                    return Err("proxy daemon is shutting down".to_string());
                }
            }
            if let Err(err) = self.proxy.set_takeover_for_app(app.as_str(), true).await {
                log::warn!(
                    "[daemon] re-applying takeover for {} after restart failed: {err}",
                    app.as_str()
                );
            }
            Ok(())
        })
    }

    async fn persist_runtime_session(&self) -> Result<(), String> {
        let workers = {
            let inner = self.inner.lock().await;
            inner
                .workers
                .iter()
                .map(|(app, info)| {
                    (
                        app.as_str().to_string(),
                        json!({
                            "pid": info.pid,
                            "address": info.address,
                            "port": info.port,
                            "started_at": info.started_at.to_rfc3339(),
                            "kind": "managed_external",
                            "session_token": info.session_token,
                            "app_type": app.as_str(),
                        }),
                    )
                })
                .collect::<serde_json::Map<_, _>>()
        };
        if workers.is_empty() {
            return self.clear_runtime_session();
        }
        let payload = json!({ "workers": workers });
        let serialized = serde_json::to_string(&payload)
            .map_err(|err| format!("serialize runtime session failed: {err}"))?;
        self.db
            .set_setting(PROXY_RUNTIME_SESSION_KEY, &serialized)
            .map_err(|err| format!("persist runtime session failed: {err}"))
    }

    fn clear_runtime_session(&self) -> Result<(), String> {
        self.db
            .delete_setting(PROXY_RUNTIME_SESSION_KEY)
            .map_err(|err| format!("clear runtime session failed: {err}"))
    }
}

impl Handler for Supervisor {
    async fn handle(&self, request: Request) -> Response {
        match request {
            Request::EnsureWorker {
                app_type,
                fallback_provider_id,
            } => {
                self.handle_ensure_worker(&app_type, fallback_provider_id.as_deref())
                    .await
            }
            Request::DropTakeover { app_type } => self.handle_drop_takeover(&app_type).await,
            Request::Status => self.handle_status().await,
            Request::WorkerHello {
                pid,
                address,
                port,
                session_token,
            } => {
                self.handle_worker_hello(pid, address, port, session_token)
                    .await
            }
            Request::SetGlobalEnabled { enabled } => self.handle_set_global_enabled(enabled).await,
            Request::Shutdown => self.handle_shutdown().await,
        }
    }
}

fn parse_app_type(s: &str) -> Option<AppType> {
    match s {
        "claude" => Some(AppType::Claude),
        "codex" => Some(AppType::Codex),
        "gemini" => Some(AppType::Gemini),
        _ => None,
    }
}

fn worker_exit_message(exit_status: &std::process::ExitStatus, stderr: &[u8]) -> Option<String> {
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    if !stderr.is_empty() {
        return Some(truncate_worker_stderr(&stderr));
    }
    (!exit_status.success()).then(|| exit_status.to_string())
}

fn truncate_worker_stderr(stderr: &str) -> String {
    const LIMIT: usize = 4096;
    let char_count = stderr.chars().count();
    if char_count <= LIMIT {
        return stderr.to_string();
    }

    let mut tail = stderr.chars().rev().take(LIMIT).collect::<Vec<_>>();
    tail.reverse();
    format!("...{}", tail.into_iter().collect::<String>())
}

fn send_sigterm(pid: Option<u32>) -> Result<(), String> {
    let Some(pid) = pid else {
        return Ok(());
    };
    if pid == 0 {
        return Ok(());
    }
    send_signal(pid, libc::SIGTERM, "SIGTERM")
}

fn send_sigkill(pid: Option<u32>) -> Result<(), String> {
    let Some(pid) = pid else {
        return Ok(());
    };
    if pid == 0 {
        return Ok(());
    }
    send_signal(pid, libc::SIGKILL, "SIGKILL")
}

fn send_signal(pid: u32, signal: libc::c_int, label: &str) -> Result<(), String> {
    unsafe {
        let rc = libc::kill(pid as i32, signal);
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::ESRCH) {
                return Err(format!("{label} worker {pid}: {err}"));
            }
        }
    }
    Ok(())
}

fn is_process_alive_for_signal(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    unsafe {
        let rc = libc::kill(pid as i32, 0);
        rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

fn worker_status_url(address: &str, port: u16) -> String {
    let connect_host = match address {
        "0.0.0.0" => "127.0.0.1".to_string(),
        "::" => "::1".to_string(),
        value => value.to_string(),
    };
    let connect_host = if connect_host.contains(':') && !connect_host.starts_with('[') {
        format!("[{connect_host}]")
    } else {
        connect_host
    };
    format!("http://{connect_host}:{port}/status")
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::daemon::ipc::protocol::Response;
    use crate::provider::Provider;
    use crate::test_support::TestEnvGuard;

    fn supervisor_for_test(db: Arc<Database>, dir: &Path) -> Supervisor {
        Supervisor::new(
            db,
            dir.join("daemon.sock"),
            PathBuf::from("/bin/cc-switch-test-missing"),
        )
    }

    fn worker_info_for_test(app_type: AppType, pid: u32) -> WorkerInfo {
        WorkerInfo {
            app_type,
            pid,
            address: "127.0.0.1".to_string(),
            port: 18080,
            session_token: "token".to_string(),
            started_at: chrono::DateTime::parse_from_rfc3339("2026-03-10T00:00:00Z")
                .expect("valid timestamp")
                .with_timezone(&chrono::Utc),
            adopted: false,
        }
    }

    async fn spawn_status_server_for_test(
        token: &'static str,
    ) -> (tokio::task::JoinHandle<()>, u16) {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind fake proxy status listener");
        let port = listener
            .local_addr()
            .expect("read fake proxy listener addr")
            .port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept status request");
            let status = json!({
                "running": true,
                "address": "127.0.0.1",
                "port": port,
                "active_connections": 0,
                "total_requests": 0,
                "success_requests": 0,
                "failed_requests": 0,
                "success_rate": 0.0,
                "uptime_seconds": 12,
                "current_provider": null,
                "current_provider_id": null,
                "last_request_at": null,
                "last_error": null,
                "failover_count": 0,
                "managed_session_token": token
            });
            let body = status.to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            use tokio::io::AsyncWriteExt;
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write fake status response");
        });
        (server, port)
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn ensure_worker_validation_failure_does_not_start_worker_or_write_session() {
        let temp_home = tempfile::tempdir().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());
        let db = Arc::new(Database::memory().expect("create database"));
        let supervisor = supervisor_for_test(db.clone(), temp_home.path());

        let response = supervisor.handle_ensure_worker("claude", None).await;

        assert!(
            matches!(response, Response::Error { message } if message.contains("no active provider"))
        );
        assert_eq!(
            db.get_setting(PROXY_RUNTIME_SESSION_KEY)
                .expect("read runtime session"),
            None
        );
        let inner = supervisor.inner.lock().await;
        assert!(inner.workers.is_empty());
        assert!(inner.pending_hellos.is_empty());
        assert!(inner.pending_tokens.is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn ensure_worker_accepts_fallback_provider_when_current_provider_is_missing() {
        let temp_home = tempfile::tempdir().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());
        let db = Arc::new(Database::memory().expect("create database"));
        let provider = Provider::with_id(
            "p1".to_string(),
            "Provider".to_string(),
            json!({"env": {"ANTHROPIC_BASE_URL": "https://example.com", "ANTHROPIC_AUTH_TOKEN": "token"}}),
            None,
        );
        db.save_provider("claude", &provider)
            .expect("save provider");
        let supervisor = supervisor_for_test(db.clone(), temp_home.path());

        let response = supervisor
            .handle_ensure_worker("claude", Some(&provider.id))
            .await;

        assert!(
            matches!(response, Response::Error { ref message } if message.contains("spawn claude proxy worker failed")),
            "fallback provider should pass active-provider validation before spawn fails: {response:?}"
        );
        assert_eq!(
            db.get_setting(PROXY_RUNTIME_SESSION_KEY)
                .expect("read runtime session"),
            None
        );
        let inner = supervisor.inner.lock().await;
        assert!(inner.workers.is_empty());
        assert!(inner.pending_hellos.is_empty());
        assert!(inner.pending_tokens.is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn ensure_worker_spawn_failure_clears_pending_registration() {
        let temp_home = tempfile::tempdir().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());
        let db = Arc::new(Database::memory().expect("create database"));
        let provider = Provider::with_id(
            "p1".to_string(),
            "Provider".to_string(),
            json!({"env": {"ANTHROPIC_BASE_URL": "https://example.com", "ANTHROPIC_AUTH_TOKEN": "token"}}),
            None,
        );
        db.save_provider("claude", &provider)
            .expect("save provider");
        db.set_current_provider("claude", &provider.id)
            .expect("set current provider");
        let supervisor = supervisor_for_test(db.clone(), temp_home.path());

        let response = supervisor.handle_ensure_worker("claude", None).await;

        assert!(
            matches!(response, Response::Error { message } if message.contains("spawn claude proxy worker failed"))
        );
        assert_eq!(
            db.get_setting(PROXY_RUNTIME_SESSION_KEY)
                .expect("read runtime session"),
            None
        );
        let inner = supervisor.inner.lock().await;
        assert!(inner.workers.is_empty());
        assert!(inner.pending_hellos.is_empty());
        assert!(inner.pending_tokens.is_empty());
    }

    #[tokio::test]
    async fn old_worker_exit_does_not_remove_restarted_worker_for_same_app() {
        let db = Arc::new(Database::memory().expect("create database"));
        let supervisor = supervisor_for_test(db, Path::new("/tmp"));
        let app = AppType::Claude;
        let old_pid = 1001;
        let new_pid = 1002;

        {
            let mut inner = supervisor.inner.lock().await;
            inner
                .workers
                .insert(app.clone(), worker_info_for_test(app.clone(), new_pid));
            inner.stopping_workers.insert((app.clone(), old_pid));
        }

        let (intentional, has_remaining_workers, teardown_in_progress) =
            supervisor.record_worker_exit(&app, old_pid, None).await;

        assert!(intentional);
        assert!(has_remaining_workers);
        assert!(!teardown_in_progress);
        let inner = supervisor.inner.lock().await;
        assert_eq!(inner.workers.get(&app).map(|info| info.pid), Some(new_pid));
        assert!(inner.stopping_workers.is_empty());
    }

    #[tokio::test]
    async fn status_and_runtime_session_preserve_worker_started_at() {
        let db = Arc::new(Database::memory().expect("create database"));
        let supervisor = supervisor_for_test(db.clone(), Path::new("/tmp"));
        let started_at = "2026-03-10T00:00:00+00:00";
        let mut worker = worker_info_for_test(AppType::Claude, 1001);
        worker.started_at = chrono::DateTime::parse_from_rfc3339(started_at)
            .expect("valid timestamp")
            .with_timezone(&chrono::Utc);

        {
            let mut inner = supervisor.inner.lock().await;
            inner.workers.insert(AppType::Claude, worker);
        }

        match supervisor.handle_status().await {
            Response::Status { workers, .. } => {
                assert_eq!(workers.len(), 1);
                assert_eq!(workers[0].started_at.as_deref(), Some(started_at));
            }
            other => panic!("expected status response, got {other:?}"),
        }

        supervisor
            .persist_runtime_session()
            .await
            .expect("persist runtime session");
        let first = db
            .get_setting(PROXY_RUNTIME_SESSION_KEY)
            .expect("read runtime session")
            .expect("runtime session");
        supervisor
            .persist_runtime_session()
            .await
            .expect("persist runtime session again");
        let second = db
            .get_setting(PROXY_RUNTIME_SESSION_KEY)
            .expect("read runtime session")
            .expect("runtime session");

        assert_eq!(first, second);
        let payload: serde_json::Value = serde_json::from_str(&first).expect("parse session");
        assert_eq!(
            payload["workers"]["claude"]["started_at"].as_str(),
            Some(started_at)
        );
    }

    #[tokio::test]
    async fn daemon_status_includes_live_worker_runtime_totals() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind fake proxy status listener");
        let port = listener
            .local_addr()
            .expect("read fake proxy listener addr")
            .port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept status request");
            let status = json!({
                "running": true,
                "address": "127.0.0.1",
                "port": port,
                "active_connections": 1,
                "total_requests": 7,
                "estimated_input_tokens_total": 52_726_211u64,
                "estimated_output_tokens_total": 454_466u64,
                "success_requests": 6,
                "failed_requests": 1,
                "success_rate": 85.7,
                "uptime_seconds": 91_381,
                "current_provider": "MiniMax My",
                "current_provider_id": "minimax-my",
                "last_request_at": "2026-06-01T02:58:44.732068565+00:00",
                "last_error": null,
                "failover_count": 2,
                "managed_session_token": "token",
                "active_targets": [{
                    "app_type": "claude",
                    "provider_name": "MiniMax My",
                    "provider_id": "minimax-my"
                }]
            });
            let body = status.to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            use tokio::io::AsyncWriteExt;
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write fake status response");
        });

        let db = Arc::new(Database::memory().expect("create database"));
        let supervisor = supervisor_for_test(db, Path::new("/tmp"));
        let mut worker = worker_info_for_test(AppType::Claude, 1001);
        worker.port = port;
        {
            let mut inner = supervisor.inner.lock().await;
            inner.workers.insert(AppType::Claude, worker);
        }

        match supervisor.handle_status().await {
            Response::Status { workers, .. } => {
                assert_eq!(workers.len(), 1);
                let runtime = workers[0]
                    .runtime_status
                    .as_ref()
                    .expect("worker runtime status");
                assert_eq!(runtime.total_requests, 7);
                assert_eq!(runtime.estimated_input_tokens_total, 52_726_211);
                assert_eq!(runtime.estimated_output_tokens_total, 454_466);
                assert_eq!(runtime.current_provider.as_deref(), Some("MiniMax My"));
                assert_eq!(runtime.active_targets.len(), 1);
                assert_eq!(runtime.active_targets[0].provider_id, "minimax-my");
            }
            other => panic!("expected status response, got {other:?}"),
        }
        server.await.expect("fake status server should finish");
    }

    #[tokio::test]
    async fn startup_recovery_adopts_persisted_live_managed_workers() {
        let (status_server, port) = spawn_status_server_for_test("daemon-token").await;
        let db = Arc::new(Database::memory().expect("create database"));
        let supervisor = supervisor_for_test(db.clone(), Path::new("/tmp"));
        let started_at = "2026-03-10T00:00:00+00:00";
        db.set_setting(
            PROXY_RUNTIME_SESSION_KEY,
            &json!({
                "workers": {
                    "claude": {
                        "pid": std::process::id(),
                        "address": "127.0.0.1",
                        "port": port,
                        "started_at": started_at,
                        "kind": "managed_external",
                        "session_token": "daemon-token"
                    }
                }
            })
            .to_string(),
        )
        .expect("write runtime session");

        supervisor
            .adopt_persisted_workers_on_startup()
            .await
            .expect("adopt persisted worker");

        match supervisor.handle_status().await {
            Response::Status {
                running, workers, ..
            } => {
                assert!(running);
                assert_eq!(workers.len(), 1);
                assert_eq!(workers[0].app_type, "claude");
                assert_eq!(workers[0].pid, Some(std::process::id()));
                assert_eq!(workers[0].port, port);
                assert_eq!(workers[0].started_at.as_deref(), Some(started_at));
            }
            other => panic!("expected status response, got {other:?}"),
        }
        status_server
            .await
            .expect("fake status server should finish");
    }

    #[tokio::test]
    async fn pending_startup_exit_records_worker_stderr() {
        let db = Arc::new(Database::memory().expect("create database"));
        let supervisor = supervisor_for_test(db, Path::new("/tmp"));
        let app = AppType::Claude;
        let pid = 1001;
        let (tx, _rx) = oneshot::channel();

        {
            let mut inner = supervisor.inner.lock().await;
            inner.pending_hellos.insert(app.as_str().to_string(), tx);
            inner
                .pending_tokens
                .insert(app.as_str().to_string(), "token".to_string());
            inner.pending_worker_pids.insert(app.clone(), pid);
        }

        let (intentional, has_remaining_workers, teardown_in_progress) = supervisor
            .record_worker_exit(
                &app,
                pid,
                Some("Error: bind proxy listener failed".to_string()),
            )
            .await;

        assert!(intentional);
        assert!(!has_remaining_workers);
        assert!(!teardown_in_progress);
        assert_eq!(
            supervisor.take_pending_startup_failure(&app).await,
            Some("Error: bind proxy listener failed".to_string())
        );
    }

    #[tokio::test]
    async fn ensure_worker_does_not_reuse_stopping_worker() {
        let db = Arc::new(Database::memory().expect("create database"));
        let supervisor = supervisor_for_test(db, Path::new("/tmp"));
        let app = AppType::Claude;
        let pid = 1001;

        {
            let mut inner = supervisor.inner.lock().await;
            inner
                .workers
                .insert(app.clone(), worker_info_for_test(app.clone(), pid));
            inner.stopping_workers.insert((app.clone(), pid));
        }

        let error = supervisor
            .ensure_worker_locked(app)
            .await
            .expect_err("stopping worker must not be reused");

        assert!(error.contains("worker is stopping"));
    }

    #[tokio::test]
    async fn ensure_worker_rejects_shutdown_in_progress() {
        let db = Arc::new(Database::memory().expect("create database"));
        let supervisor = supervisor_for_test(db, Path::new("/tmp"));

        {
            let mut inner = supervisor.inner.lock().await;
            inner.shutdown_requested = true;
        }

        let error = supervisor
            .ensure_worker_locked(AppType::Claude)
            .await
            .expect_err("shutdown should reject new workers");

        assert!(error.contains("shutting down"));
    }

    #[tokio::test]
    async fn drop_inactive_app_does_not_shutdown_other_worker() {
        let db = Arc::new(Database::memory().expect("create database"));
        let supervisor = supervisor_for_test(db, Path::new("/tmp"));

        {
            let mut inner = supervisor.inner.lock().await;
            inner
                .workers
                .insert(AppType::Claude, worker_info_for_test(AppType::Claude, 1001));
        }

        let plan = supervisor.plan_stop_for_app(AppType::Codex).await;

        assert!(plan.pids.is_empty());
        assert!(!plan.should_shutdown);
        let inner = supervisor.inner.lock().await;
        assert!(!inner.shutdown_requested);
        assert!(
            inner.cancelled_apps.contains(&AppType::Codex),
            "dropping an inactive app should still cancel any delayed restart for that route"
        );
        assert_eq!(
            inner.workers.get(&AppType::Claude).map(|info| info.pid),
            Some(1001)
        );
    }

    #[tokio::test]
    async fn stop_plan_removes_adopted_worker_without_waiting_for_watcher() {
        let db = Arc::new(Database::memory().expect("create database"));
        let supervisor = supervisor_for_test(db.clone(), Path::new("/tmp"));
        let app = AppType::Claude;
        let mut worker = worker_info_for_test(app.clone(), 1001);
        worker.adopted = true;

        {
            let mut inner = supervisor.inner.lock().await;
            inner.workers.insert(app.clone(), worker);
        }
        supervisor
            .persist_runtime_session()
            .await
            .expect("persist adopted session");

        let plan = supervisor.plan_stop_for_app(app.clone()).await;
        assert_eq!(plan.pids, vec![1001]);
        assert_eq!(plan.adopted_pids, vec![1001]);
        assert!(plan.should_shutdown);

        let inner = supervisor.inner.lock().await;
        assert!(inner.workers.is_empty());
        assert!(
            !inner.stopping_workers.contains(&(app, 1001)),
            "adopted workers have no child watcher, so they should not wait in stopping state"
        );
        drop(inner);

        supervisor
            .persist_runtime_session()
            .await
            .expect("clear adopted session");
        assert_eq!(
            db.get_setting(PROXY_RUNTIME_SESSION_KEY)
                .expect("read runtime session"),
            None
        );
    }

    #[tokio::test]
    async fn drop_takeover_cancels_delayed_restart_for_target_app() {
        let db = Arc::new(Database::memory().expect("create database"));
        let supervisor = supervisor_for_test(db, Path::new("/tmp"));

        let plan = supervisor.plan_stop_for_app(AppType::Claude).await;

        assert!(plan.pids.is_empty());
        assert!(!plan.should_shutdown);
        assert!(
            !supervisor
                .should_restart_after_crash(&AppType::Claude)
                .await,
            "disabled app should not restart after crash backoff"
        );
    }

    #[tokio::test]
    async fn drop_takeover_cancels_pending_worker_for_target_app() {
        let db = Arc::new(Database::memory().expect("create database"));
        let supervisor = supervisor_for_test(db, Path::new("/tmp"));
        let app = AppType::Claude;
        let app_key = app.as_str().to_string();
        let pending_pid = 1002;

        {
            let mut inner = supervisor.inner.lock().await;
            let (tx, _rx) = oneshot::channel();
            inner.pending_hellos.insert(app_key.clone(), tx);
            inner
                .pending_tokens
                .insert(app_key.clone(), "token".to_string());
            inner.pending_worker_pids.insert(app.clone(), pending_pid);
        }

        let plan = supervisor.plan_stop_for_app(app.clone()).await;

        assert_eq!(plan.pids, vec![pending_pid]);
        assert!(plan.should_shutdown);
        let inner = supervisor.inner.lock().await;
        assert!(inner.shutdown_requested);
        assert!(inner.pending_hellos.is_empty());
        assert!(inner.pending_tokens.is_empty());
        assert!(inner.pending_worker_pids.is_empty());
        assert!(inner.stopping_workers.contains(&(app, pending_pid)));
        assert!(inner.cancelled_apps.contains(&AppType::Claude));
    }

    #[tokio::test]
    async fn global_disable_cancels_pending_workers() {
        let db = Arc::new(Database::memory().expect("create database"));
        let supervisor = supervisor_for_test(db, Path::new("/tmp"));
        let app = AppType::Claude;
        let app_key = app.as_str().to_string();
        let pending_pid = 1002;

        {
            let mut inner = supervisor.inner.lock().await;
            let (tx, _rx) = oneshot::channel();
            inner.pending_hellos.insert(app_key.clone(), tx);
            inner
                .pending_tokens
                .insert(app_key.clone(), "token".to_string());
            inner.pending_worker_pids.insert(app.clone(), pending_pid);
        }

        let plan = supervisor.plan_stop_all_workers(false).await;

        assert_eq!(plan.pids, vec![pending_pid]);
        assert!(plan.adopted_pids.is_empty());
        let inner = supervisor.inner.lock().await;
        assert!(inner.shutdown_requested);
        assert!(!inner.teardown_in_progress);
        assert!(inner.pending_hellos.is_empty());
        assert!(inner.pending_tokens.is_empty());
        assert!(inner.pending_worker_pids.is_empty());
        assert!(inner.stopping_workers.contains(&(app, pending_pid)));
        assert!(inner.cancelled_apps.contains(&AppType::Claude));
    }

    #[tokio::test]
    async fn shutdown_teardown_prevents_worker_exit_from_signalling_shutdown() {
        let db = Arc::new(Database::memory().expect("create database"));
        let supervisor = supervisor_for_test(db, Path::new("/tmp"));
        let app = AppType::Claude;
        let pid = 1001;

        {
            let mut inner = supervisor.inner.lock().await;
            inner
                .workers
                .insert(app.clone(), worker_info_for_test(app.clone(), pid));
        }

        let plan = supervisor.plan_stop_all_workers(true).await;
        assert_eq!(plan.pids, vec![pid]);
        assert!(plan.adopted_pids.is_empty());

        let (intentional, has_remaining_workers, teardown_in_progress) =
            supervisor.record_worker_exit(&app, pid, None).await;

        assert!(intentional);
        assert!(!has_remaining_workers);
        assert!(teardown_in_progress);
    }

    #[tokio::test]
    async fn old_worker_exit_keeps_daemon_alive_for_pending_restarted_worker() {
        let db = Arc::new(Database::memory().expect("create database"));
        let supervisor = supervisor_for_test(db, Path::new("/tmp"));
        let app = AppType::Claude;
        let old_pid = 1001;
        let pending_pid = 1002;

        {
            let mut inner = supervisor.inner.lock().await;
            inner.pending_worker_pids.insert(app.clone(), pending_pid);
            inner.stopping_workers.insert((app.clone(), old_pid));
        }

        let (intentional, has_remaining_workers, teardown_in_progress) =
            supervisor.record_worker_exit(&app, old_pid, None).await;

        assert!(intentional);
        assert!(has_remaining_workers);
        assert!(!teardown_in_progress);
        let inner = supervisor.inner.lock().await;
        assert_eq!(
            inner.pending_worker_pids.get(&app).copied(),
            Some(pending_pid)
        );
        assert!(inner.stopping_workers.is_empty());
    }
}
