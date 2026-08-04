//! Coordinates schema migrations with an already-running supervisor daemon.
//!
//! A daemon from the previous binary does not know about newer session-import
//! locks. Before a migration that rebuilds session usage, the foreground
//! process therefore asks that daemon to shut down, takes over its lifetime
//! pidfile lease, and keeps the lease until database initialization is fully
//! complete. This happens before the database init lock is acquired, keeping
//! the global lock order `daemon.pid -> cc-switch.db.init.lock`.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::database::Database;

use super::ipc::client;
use super::ipc::protocol::{Request, Response, TakeoverFlags, WorkerState};
use super::pidfile::{AcquireError, PidFile};

const QUIESCE_TIMEOUT: Duration = Duration::from_secs(30);
const RESUME_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(75);

#[derive(Debug, Clone)]
struct ResumeTarget {
    app_type: String,
    fallback_provider_id: Option<String>,
}

#[derive(Clone)]
struct ResumePlan {
    targets: Vec<ResumeTarget>,
    /// Preserve the daemon's complete startup environment. Live config paths,
    /// proxy variables, and HOME may differ from the foreground process that
    /// happens to perform the schema migration.
    environment: Vec<(OsString, OsString)>,
}

/// Holds the daemon pidfile while the caller performs database initialization.
/// Before schema mutation begins, dropping the guard restores any daemon that
/// was quiesced. The caller explicitly suppresses that rollback once mutating
/// migration work starts.
pub(crate) struct DaemonMigrationGuard {
    lease: Option<PidFile>,
    resume: Option<ResumePlan>,
    backup_created: bool,
    resume_on_safe_abort: bool,
}

impl DaemonMigrationGuard {
    fn new(lease: Option<PidFile>, resume: Option<ResumePlan>, backup_created: bool) -> Self {
        Self {
            lease,
            resume,
            backup_created,
            resume_on_safe_abort: true,
        }
    }

    pub(crate) fn backup_created(&self) -> bool {
        self.backup_created
    }

    fn release_and_resume(&mut self) {
        let resume = self.resume.take();
        drop(self.lease.take());
        if let Some(plan) = resume {
            plan.resume();
        }
    }

    /// Once schema mutation starts, restarting a previous binary on failure
    /// is no longer guaranteed to be safe.
    pub(crate) fn suppress_safe_abort_resume(&mut self) {
        self.resume_on_safe_abort = false;
    }

    /// Release the lifecycle lease, then restore the daemon and the app workers
    /// that were active before the migration.
    pub(crate) fn resume_after_success(mut self) {
        self.resume_on_safe_abort = false;
        self.release_and_resume();
    }

    /// No database mutation has started, so it is safe to restore a daemon
    /// that was quiesced while a final holder check was being performed.
    pub(crate) fn resume_after_safe_abort(mut self) {
        self.resume_on_safe_abort = false;
        self.release_and_resume();
    }
}

impl Drop for DaemonMigrationGuard {
    fn drop(&mut self) {
        if self.resume_on_safe_abort {
            self.release_and_resume();
        }
    }
}

impl ResumePlan {
    fn from_status(
        takeovers: TakeoverFlags,
        workers: Vec<WorkerState>,
        environment: Vec<(OsString, OsString)>,
    ) -> Self {
        let mut targets: BTreeMap<String, Option<String>> = BTreeMap::new();
        if takeovers.claude {
            targets.insert("claude".to_string(), None);
        }
        if takeovers.codex {
            targets.insert("codex".to_string(), None);
        }
        if takeovers.gemini {
            targets.insert("gemini".to_string(), None);
        }
        for worker in workers {
            if !matches!(worker.app_type.as_str(), "claude" | "codex" | "gemini") {
                continue;
            }
            let fallback = worker
                .runtime_status
                .and_then(|status| status.current_provider_id);
            targets
                .entry(worker.app_type)
                .and_modify(|existing| {
                    if existing.is_none() {
                        *existing = fallback.clone();
                    }
                })
                .or_insert(fallback);
        }
        Self {
            targets: targets
                .into_iter()
                .map(|(app_type, fallback_provider_id)| ResumeTarget {
                    app_type,
                    fallback_provider_id,
                })
                .collect(),
            environment,
        }
    }

    fn resume(self) {
        let executable = match std::env::current_exe() {
            Ok(executable) => executable,
            Err(error) => {
                log::warn!("Could not restart daemon after database migration: {error}");
                return;
            }
        };
        let mut command = Command::new(executable);
        command
            .args(["daemon", "start", "--detach"])
            .env_clear()
            .envs(self.environment.iter().cloned())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match command.status() {
            Ok(status) if status.success() => {}
            Ok(status) => {
                log::warn!(
                    "Could not restart daemon after database migration: exit status {status}"
                );
                return;
            }
            Err(error) => {
                log::warn!("Could not restart daemon after database migration: {error}");
                return;
            }
        }

        let socket = super::paths::socket_path();
        let deadline = Instant::now() + RESUME_TIMEOUT;
        while Instant::now() < deadline {
            if client::connect(&socket).is_ok() {
                for target in &self.targets {
                    let request = Request::EnsureWorker {
                        app_type: target.app_type.clone(),
                        fallback_provider_id: target.fallback_provider_id.clone(),
                    };
                    match client::round_trip(&socket, &request) {
                        Ok(Response::Worker { .. }) | Ok(Response::Ok) => {}
                        Ok(Response::Error { message }) => log::warn!(
                            "Could not restore {} daemon worker after migration: {message}",
                            target.app_type
                        ),
                        Ok(other) => log::warn!(
                            "Unexpected response restoring {} daemon worker: {other:?}",
                            target.app_type
                        ),
                        Err(error) => log::warn!(
                            "Could not restore {} daemon worker after migration: {error}",
                            target.app_type
                        ),
                    }
                }
                return;
            }
            thread::sleep(POLL_INTERVAL);
        }
        log::warn!(
            "Daemon did not become reachable within {}s after database migration",
            RESUME_TIMEOUT.as_secs()
        );
    }
}

#[cfg(target_os = "linux")]
fn parse_nul_separated_environment(bytes: &[u8]) -> Vec<(OsString, OsString)> {
    use std::os::unix::ffi::OsStringExt;

    bytes
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let separator = entry.iter().position(|byte| *byte == b'=')?;
            if separator == 0 {
                return None;
            }
            Some((
                OsString::from_vec(entry[..separator].to_vec()),
                OsString::from_vec(entry[separator + 1..].to_vec()),
            ))
        })
        .collect()
}

/// Read the authenticated daemon's startup environment so the replacement
/// process can keep using the same live configuration and network settings.
/// Refusing migration is safer than silently moving an active takeover to the
/// foreground process's HOME or app-specific override.
#[cfg(target_os = "linux")]
fn process_environment(pid: u32) -> Result<Vec<(OsString, OsString)>, String> {
    let path = format!("/proc/{pid}/environ");
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("read daemon environment {path} failed: {error}"))?;
    Ok(parse_nul_separated_environment(&bytes))
}

#[cfg(not(target_os = "linux"))]
fn process_environment(_pid: u32) -> Result<Vec<(OsString, OsString)>, String> {
    Err(
        "this platform cannot reliably recover another process's complete startup environment"
            .to_string(),
    )
}

/// Return PIDs (other than this process) that currently have the target
/// database inode open. A v15 process does not know the v16 session-import
/// lock, so migration must fail closed when an uncoordinated TUI, foreground
/// proxy, orphan worker, or import still owns the database.
pub(crate) fn external_database_holder_pids(database_path: &Path) -> Result<BTreeSet<u32>, String> {
    #[cfg(target_os = "linux")]
    {
        if Path::new("/proc").is_dir() {
            return linux_database_holder_pids(database_path);
        }
    }

    lsof_database_holder_pids(database_path)
}

#[cfg(target_os = "linux")]
fn linux_database_holder_pids(database_path: &Path) -> Result<BTreeSet<u32>, String> {
    use std::os::unix::fs::MetadataExt;

    let target = std::fs::metadata(database_path).map_err(|error| {
        format!(
            "inspect database {} before migration failed: {error}",
            database_path.display()
        )
    })?;
    let mut holders = BTreeSet::new();
    let processes = std::fs::read_dir("/proc")
        .map_err(|error| format!("enumerate /proc before database migration failed: {error}"))?;
    for process in processes.flatten() {
        let Some(pid) = process
            .file_name()
            .to_str()
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == std::process::id() {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(process.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            let Ok(metadata) = fd.path().metadata() else {
                continue;
            };
            if metadata.dev() == target.dev() && metadata.ino() == target.ino() {
                holders.insert(pid);
                break;
            }
        }
    }
    Ok(holders)
}

fn lsof_database_holder_pids(database_path: &Path) -> Result<BTreeSet<u32>, String> {
    let lsof = if Path::new("/usr/sbin/lsof").is_file() {
        Path::new("/usr/sbin/lsof")
    } else {
        Path::new("lsof")
    };
    let output = Command::new(lsof)
        .args(["-F", "p", "--"])
        .arg(database_path)
        .output()
        .map_err(|error| {
            format!(
                "cannot verify whether another process is using database {}: {error}",
                database_path.display()
            )
        })?;
    // lsof uses exit code 1 when there are no matching open files.
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(format!(
            "cannot verify whether another process is using database {}: lsof exited with {}",
            database_path.display(),
            output.status
        ));
    }

    let current_pid = std::process::id();
    let holders = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix('p'))
        .filter_map(|value| value.parse::<u32>().ok())
        .filter(|pid| *pid != current_pid)
        .collect();
    Ok(holders)
}

fn database_holders_error(database_path: &Path, holders: &BTreeSet<u32>) -> String {
    let pids = holders
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "cannot safely migrate database {} while another process has it open (PID: {pids}); close all older cc-switch TUI/proxy processes and retry",
        database_path.display()
    )
}

pub(crate) fn ensure_no_external_database_holders(database_path: &Path) -> Result<(), String> {
    let holders = external_database_holder_pids(database_path)?;
    if holders.is_empty() {
        Ok(())
    } else {
        Err(database_holders_error(database_path, &holders))
    }
}

fn database_still_needs_migration(database_path: &Path) -> Result<bool, String> {
    Database::existing_database_needs_migration(database_path).map_err(|error| {
        format!(
            "recheck database {} before migration failed: {error}",
            database_path.display()
        )
    })
}

fn create_pre_migration_backup(database_path: &Path) -> Result<bool, String> {
    log::info!(
        "Creating pre-migration database backup before daemon quiescence (target v{})",
        crate::database::SCHEMA_VERSION
    );
    Database::backup_database_path(database_path)
        .map(|path| path.is_some())
        .map_err(|error| {
            format!("Pre-migration backup failed; database migration was not started: {error}")
        })
}

fn resume_after_failed_quiesce(resume: &Option<ResumePlan>) {
    if let Some(plan) = resume.clone() {
        plan.resume();
    }
}

/// Quiesce a daemon that may have been launched by the previous binary and
/// acquire its pidfile lease. The daemon is contacted only when its PID is
/// proven to hold this exact database. The function never signals a PID from
/// the pidfile; shutdown is performed only through the existing
/// authenticated-by-filesystem Unix socket.
pub(crate) fn quiesce_for_database_migration(
    database_path: &Path,
) -> Result<DaemonMigrationGuard, String> {
    let pidfile = super::paths::pidfile_path();
    let socket = super::paths::socket_path();
    let initial_holders = external_database_holder_pids(database_path)?;

    let owner_pid = match PidFile::acquire(&pidfile) {
        Ok(lease) => {
            if !initial_holders.is_empty() {
                drop(lease);
                return Err(database_holders_error(database_path, &initial_holders));
            }
            let backup_created = create_pre_migration_backup(database_path)?;
            return Ok(DaemonMigrationGuard::new(Some(lease), None, backup_created));
        }
        Err(AcquireError::AlreadyHeld { pid }) => pid,
        Err(AcquireError::Io(error)) => {
            return Err(format!(
                "acquire daemon migration lease {} failed: {error}",
                pidfile.display()
            ));
        }
    };

    let Some(mut owner_pid) = owner_pid else {
        if initial_holders.is_empty() {
            // A daemon (or another foreground migrator) for a different
            // database owns the legacy global pidfile. It must not be touched.
            let backup_created = create_pre_migration_backup(database_path)?;
            return Ok(DaemonMigrationGuard::new(None, None, backup_created));
        }
        return Err(database_holders_error(database_path, &initial_holders));
    };

    if !initial_holders.contains(&owner_pid) {
        if initial_holders.is_empty() {
            // The global daemon belongs to another CC_SWITCH_CONFIG_DIR.
            let backup_created = create_pre_migration_backup(database_path)?;
            return Ok(DaemonMigrationGuard::new(None, None, backup_created));
        }
        return Err(database_holders_error(database_path, &initial_holders));
    }

    let deadline = Instant::now() + QUIESCE_TIMEOUT;
    let mut resume = None;
    let mut worker_pids = Vec::new();
    let mut shutdown_requested = false;
    let mut backup_created = false;
    let mut last_error = String::new();

    loop {
        let still_needs_migration = match database_still_needs_migration(database_path) {
            Ok(needs_migration) => needs_migration,
            Err(error) => {
                if shutdown_requested {
                    resume_after_failed_quiesce(&resume);
                }
                return Err(error);
            }
        };
        if !still_needs_migration {
            // Another new-version foreground process completed the migration.
            // Do not mistake its pidfile lease (or the daemon it just resumed)
            // for a legacy daemon, and do not wait for that foreground process
            // itself to exit.
            return Ok(DaemonMigrationGuard::new(None, resume, backup_created));
        }

        if !shutdown_requested {
            match client::round_trip(&socket, &Request::Status) {
                Ok(Response::Status {
                    takeovers, workers, ..
                }) => {
                    // Status is the daemon identity proof. Re-check the
                    // pidfile after IPC so a lease handoff cannot make us send
                    // Shutdown to a newly resumed daemon.
                    match PidFile::acquire(&pidfile) {
                        Ok(lease) => {
                            drop(lease);
                            continue;
                        }
                        Err(AcquireError::AlreadyHeld {
                            pid: Some(current_pid),
                        }) if current_pid == owner_pid => {}
                        Err(AcquireError::AlreadyHeld {
                            pid: Some(current_pid),
                        }) => {
                            owner_pid = current_pid;
                            worker_pids.clear();
                            continue;
                        }
                        Err(AcquireError::AlreadyHeld { pid: None }) => {
                            last_error =
                                "daemon pidfile owner changed during identity check".to_string();
                            continue;
                        }
                        Err(AcquireError::Io(error)) => {
                            return Err(format!(
                                "verify daemon migration lease {} failed: {error}",
                                pidfile.display()
                            ));
                        }
                    }

                    if !database_still_needs_migration(database_path)? {
                        return Ok(DaemonMigrationGuard::new(None, resume, backup_created));
                    }

                    worker_pids = workers.iter().filter_map(|worker| worker.pid).collect();
                    let mut coordinated_pids = BTreeSet::from([owner_pid]);
                    coordinated_pids.extend(worker_pids.iter().copied());
                    let current_holders = external_database_holder_pids(database_path)?;
                    if !current_holders.contains(&owner_pid) {
                        if current_holders.is_empty() {
                            // The current global daemon belongs to a different
                            // database. It must stay online while this target
                            // migrates independently.
                            if !backup_created {
                                backup_created = create_pre_migration_backup(database_path)?;
                            }
                            return Ok(DaemonMigrationGuard::new(None, resume, backup_created));
                        }
                        return Err(database_holders_error(database_path, &current_holders));
                    }
                    let uncoordinated = current_holders
                        .difference(&coordinated_pids)
                        .copied()
                        .collect::<BTreeSet<_>>();
                    if !uncoordinated.is_empty() {
                        return Err(database_holders_error(database_path, &uncoordinated));
                    }
                    if !backup_created {
                        // The daemon is still serving here. A backup failure
                        // must return before Shutdown so existing workers stay
                        // available.
                        backup_created = create_pre_migration_backup(database_path)?;
                    }

                    // Online backup can take long enough for a daemon lease
                    // handoff. Authenticate the same owner again before
                    // capturing its environment or sending Shutdown.
                    match PidFile::acquire(&pidfile) {
                        Ok(lease) => {
                            drop(lease);
                            continue;
                        }
                        Err(AcquireError::AlreadyHeld {
                            pid: Some(current_pid),
                        }) if current_pid == owner_pid => {}
                        Err(AcquireError::AlreadyHeld {
                            pid: Some(current_pid),
                        }) => {
                            owner_pid = current_pid;
                            worker_pids.clear();
                            continue;
                        }
                        Err(AcquireError::AlreadyHeld { pid: None }) => {
                            last_error = "daemon pidfile owner changed during backup".to_string();
                            continue;
                        }
                        Err(AcquireError::Io(error)) => {
                            return Err(format!(
                                "verify daemon migration lease {} after backup failed: {error}",
                                pidfile.display()
                            ));
                        }
                    }
                    if !database_still_needs_migration(database_path)? {
                        return Ok(DaemonMigrationGuard::new(None, resume, backup_created));
                    }
                    let current_holders = external_database_holder_pids(database_path)?;
                    if !current_holders.contains(&owner_pid) {
                        continue;
                    }
                    let mut coordinated_pids = BTreeSet::from([owner_pid]);
                    coordinated_pids.extend(worker_pids.iter().copied());
                    let uncoordinated = current_holders
                        .difference(&coordinated_pids)
                        .copied()
                        .collect::<BTreeSet<_>>();
                    if !uncoordinated.is_empty() {
                        return Err(database_holders_error(database_path, &uncoordinated));
                    }

                    let daemon_environment = process_environment(owner_pid).map_err(|error| {
                        format!(
                            "cannot safely preserve daemon environment before migrating {} (PID {owner_pid}): {error}; stop the daemon manually and retry",
                            database_path.display()
                        )
                    })?;
                    resume = Some(ResumePlan::from_status(
                        takeovers,
                        workers,
                        daemon_environment,
                    ));
                    match client::round_trip(&socket, &Request::Shutdown) {
                        Ok(Response::Ok) => shutdown_requested = true,
                        Ok(Response::Error { message }) => {
                            return Err(format!(
                                "daemon refused shutdown before database migration: {message}"
                            ));
                        }
                        Ok(other) => {
                            return Err(format!(
                                "unexpected daemon shutdown response before migration: {other:?}"
                            ));
                        }
                        Err(error) => {
                            last_error = format!("request daemon shutdown failed: {error}");
                        }
                    }
                }
                Ok(other) => {
                    last_error = format!("unexpected daemon status response: {other:?}");
                }
                Err(error) => {
                    last_error = format!("query daemon status failed: {error}");
                }
            }
        }

        match PidFile::acquire(&pidfile) {
            Ok(lease) => {
                let still_needs_migration = match database_still_needs_migration(database_path) {
                    Ok(needs_migration) => needs_migration,
                    Err(error) => {
                        drop(lease);
                        if shutdown_requested {
                            resume_after_failed_quiesce(&resume);
                        }
                        return Err(error);
                    }
                };
                if !still_needs_migration {
                    return Ok(DaemonMigrationGuard::new(
                        Some(lease),
                        resume,
                        backup_created,
                    ));
                }
                for pid in &worker_pids {
                    if let Err(error) = wait_for_process_exit(*pid, deadline, "daemon worker") {
                        drop(lease);
                        resume_after_failed_quiesce(&resume);
                        return Err(error);
                    }
                }
                let remaining = match external_database_holder_pids(database_path) {
                    Ok(holders) => holders,
                    Err(error) => {
                        drop(lease);
                        resume_after_failed_quiesce(&resume);
                        return Err(error);
                    }
                };
                if !remaining.is_empty() {
                    drop(lease);
                    resume_after_failed_quiesce(&resume);
                    return Err(database_holders_error(database_path, &remaining));
                }
                if !backup_created {
                    backup_created = match create_pre_migration_backup(database_path) {
                        Ok(created) => created,
                        Err(error) => {
                            drop(lease);
                            if shutdown_requested {
                                resume_after_failed_quiesce(&resume);
                            }
                            return Err(error);
                        }
                    };
                }
                return Ok(DaemonMigrationGuard::new(
                    Some(lease),
                    resume,
                    backup_created,
                ));
            }
            Err(AcquireError::AlreadyHeld {
                pid: Some(current_pid),
            }) if current_pid != owner_pid => {
                owner_pid = current_pid;
                worker_pids.clear();
                shutdown_requested = false;
            }
            Err(AcquireError::AlreadyHeld { .. }) => {}
            Err(AcquireError::Io(error)) => {
                if shutdown_requested {
                    resume_after_failed_quiesce(&resume);
                }
                return Err(format!(
                    "acquire daemon migration lease {} failed: {error}",
                    pidfile.display()
                ));
            }
        }

        if Instant::now() >= deadline {
            if shutdown_requested {
                resume_after_failed_quiesce(&resume);
            }
            return Err(format!(
                "daemon did not quiesce within {}s before database migration{}",
                QUIESCE_TIMEOUT.as_secs(),
                if last_error.is_empty() {
                    String::new()
                } else {
                    format!(": {last_error}")
                }
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn wait_for_process_exit(pid: u32, deadline: Instant, label: &str) -> Result<(), String> {
    while process_is_alive(pid) {
        if Instant::now() >= deadline {
            return Err(format!(
                "{label} process {pid} did not exit within {}s before database migration",
                QUIESCE_TIMEOUT.as_secs()
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
    Ok(())
}

fn process_is_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if result == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_an_external_process_holding_the_database() {
        use std::io::{BufRead, Write};
        use std::process::{Command, Stdio};

        let temp = tempfile::tempdir().expect("temp database directory");
        let database_path = temp.path().join("cc-switch.db");
        std::fs::write(&database_path, b"placeholder").expect("create database placeholder");

        let mut child = Command::new("sh")
            .args([
                "-c",
                "exec 9<\"$1\"; printf 'ready\\n'; read -r _",
                "cc-switch-holder",
            ])
            .arg(&database_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn database holder");
        let mut ready = String::new();
        std::io::BufReader::new(child.stdout.take().expect("holder stdout"))
            .read_line(&mut ready)
            .expect("wait for holder readiness");
        assert_eq!(ready, "ready\n");

        let holders = external_database_holder_pids(&database_path).expect("enumerate holders");
        assert!(holders.contains(&child.id()), "holders={holders:?}");

        child
            .stdin
            .as_mut()
            .expect("holder stdin")
            .write_all(b"done\n")
            .expect("release holder");
        assert!(child.wait().expect("wait for holder").success());
    }

    #[test]
    fn migration_guard_owns_daemon_lifetime_lease_until_drop() {
        let home = tempfile::tempdir().expect("isolated daemon home");
        let canonical_home = std::fs::canonicalize(home.path()).expect("canonical test home");
        let _env = crate::test_support::TestEnvGuard::isolated(&canonical_home);
        let database_path = canonical_home.join("cc-switch.db");
        std::fs::write(&database_path, []).expect("create database placeholder");

        let guard =
            quiesce_for_database_migration(&database_path).expect("acquire migration lease");
        assert!(matches!(
            PidFile::acquire(super::super::paths::pidfile_path()),
            Err(AcquireError::AlreadyHeld { .. })
        ));
        drop(guard);

        let lease = PidFile::acquire(super::super::paths::pidfile_path())
            .expect("lease should be released after migration guard");
        drop(lease);
    }

    #[test]
    fn concurrent_foreground_migrator_is_not_treated_as_daemon() {
        use std::io::{BufRead, Write};
        use std::path::PathBuf;
        use std::process::{Command, Stdio};
        use std::sync::mpsc;

        const CHILD_ENV: &str = "CC_SWITCH_TEST_CONCURRENT_MIGRATOR_CHILD";
        const TEST_NAME: &str =
            "daemon::migration::tests::concurrent_foreground_migrator_is_not_treated_as_daemon";

        if let Some(home) = std::env::var_os(CHILD_ENV) {
            let home = PathBuf::from(home);
            let _env = crate::test_support::TestEnvGuard::isolated(&home);
            let database_path = home.join(".cc-switch/cc-switch.db");
            let conn = rusqlite::Connection::open(&database_path).expect("open migration database");
            let lease = PidFile::acquire(super::super::paths::pidfile_path())
                .expect("acquire foreground migration lease");

            println!("MIGRATOR_READY");
            std::io::stdout().flush().expect("flush ready marker");
            let mut command = String::new();
            std::io::BufReader::new(std::io::stdin())
                .read_line(&mut command)
                .expect("wait for migrate command");
            assert_eq!(command.trim(), "migrate");

            conn.pragma_update(None, "user_version", crate::database::SCHEMA_VERSION)
                .expect("publish completed schema");
            drop(lease);
            println!("MIGRATOR_RELEASED");
            std::io::stdout().flush().expect("flush released marker");

            command.clear();
            std::io::BufReader::new(std::io::stdin())
                .read_line(&mut command)
                .expect("wait for child exit command");
            assert_eq!(command.trim(), "done");
            drop(conn);
            return;
        }

        let home = tempfile::tempdir().expect("isolated daemon home");
        let canonical_home = std::fs::canonicalize(home.path()).expect("canonical test home");
        let _env = crate::test_support::TestEnvGuard::isolated(&canonical_home);
        let database_path = canonical_home.join(".cc-switch/cc-switch.db");
        std::fs::create_dir_all(database_path.parent().expect("database parent"))
            .expect("create database parent");
        {
            let conn = rusqlite::Connection::open(&database_path).expect("create migration db");
            conn.execute("CREATE TABLE migration_sentinel (id INTEGER)", [])
                .expect("create user table");
            conn.pragma_update(None, "user_version", 15)
                .expect("seed v15 schema");
        }

        let mut child = Command::new(std::env::current_exe().expect("resolve test binary"))
            .args(["--exact", TEST_NAME, "--nocapture", "--test-threads=1"])
            .env(CHILD_ENV, &canonical_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn foreground migrator");
        let mut child_stdout = std::io::BufReader::new(child.stdout.take().expect("child stdout"));
        let mut output = String::new();
        loop {
            output.clear();
            child_stdout
                .read_line(&mut output)
                .expect("read child readiness");
            assert!(!output.is_empty(), "child exited before ready marker");
            if output.contains("MIGRATOR_READY") {
                break;
            }
        }
        assert!(
            database_still_needs_migration(&database_path)
                .expect("probe v15 database held by foreground migrator"),
            "child should still expose v15 before migration"
        );

        let path_for_thread = database_path.clone();
        let (result_tx, result_rx) = mpsc::channel();
        let quiesce_thread = std::thread::spawn(move || {
            let _ = result_tx.send(quiesce_for_database_migration(&path_for_thread));
        });
        std::thread::sleep(Duration::from_millis(150));
        child
            .stdin
            .as_mut()
            .expect("child stdin")
            .write_all(b"migrate\n")
            .expect("release migrator lease");
        loop {
            output.clear();
            child_stdout
                .read_line(&mut output)
                .expect("read child migration marker");
            assert!(!output.is_empty(), "child exited before migration marker");
            if output.contains("MIGRATOR_RELEASED") {
                break;
            }
        }

        let guard = result_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("second migrator should not wait for foreground process exit")
            .expect("second migrator should observe completed schema");
        assert!(
            !guard.backup_created(),
            "a completed concurrent migration needs no duplicate backup"
        );
        assert!(
            child
                .try_wait()
                .expect("poll foreground migrator")
                .is_none(),
            "quiescence must complete while the first foreground process remains alive"
        );
        drop(guard);

        child
            .stdin
            .as_mut()
            .expect("child stdin")
            .write_all(b"done\n")
            .expect("finish foreground migrator");
        assert!(child.wait().expect("wait for migrator").success());
        quiesce_thread.join().expect("join quiescence thread");
    }

    #[test]
    fn pre_shutdown_failures_do_not_shutdown_a_running_daemon() {
        use std::ffi::OsString;
        use std::io::{BufRead, Write};
        use std::os::unix::net::UnixListener;
        use std::path::PathBuf;
        use std::process::{Command, Stdio};

        const CHILD_ENV: &str = "CC_SWITCH_TEST_BACKUP_FAILURE_DAEMON_CHILD";
        const TEST_NAME: &str =
            "daemon::migration::tests::pre_shutdown_failures_do_not_shutdown_a_running_daemon";

        if let Some(home) = std::env::var_os(CHILD_ENV) {
            let home = PathBuf::from(home);
            let _env = crate::test_support::TestEnvGuard::isolated(&home);
            let database_path = home.join(".cc-switch/cc-switch.db");
            let conn = rusqlite::Connection::open(&database_path).expect("open daemon database");
            let lease = PidFile::acquire(super::super::paths::pidfile_path())
                .expect("acquire fake daemon lease");
            let socket_path = super::super::paths::socket_path();
            if socket_path.exists() {
                std::fs::remove_file(&socket_path).expect("remove stale test socket");
            }
            let listener = UnixListener::bind(&socket_path).expect("bind fake daemon socket");
            listener
                .set_nonblocking(true)
                .expect("set fake daemon nonblocking");
            println!("FAKE_DAEMON_READY");
            std::io::stdout().flush().expect("flush daemon marker");

            let deadline = Instant::now() + Duration::from_secs(2);
            let mut saw_status = false;
            let mut saw_shutdown = false;
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut line = String::new();
                        std::io::BufReader::new(
                            stream.try_clone().expect("clone fake daemon stream"),
                        )
                        .read_line(&mut line)
                        .expect("read fake daemon request");
                        let request: Request =
                            serde_json::from_str(line.trim()).expect("decode daemon request");
                        let response = match request {
                            Request::Status => {
                                saw_status = true;
                                Response::Status {
                                    running: false,
                                    address: String::new(),
                                    port: 0,
                                    worker_pid: None,
                                    takeovers: TakeoverFlags::default(),
                                    restart_count: 0,
                                    last_restart_at: None,
                                    workers: Vec::new(),
                                }
                            }
                            Request::Shutdown => {
                                saw_shutdown = true;
                                Response::Ok
                            }
                            _ => Response::Error {
                                message: "unexpected test request".to_string(),
                            },
                        };
                        let payload = super::super::ipc::protocol::encode_response(&response)
                            .expect("encode fake daemon response");
                        stream
                            .write_all(format!("{payload}\n").as_bytes())
                            .expect("write fake daemon response");
                        stream.flush().expect("flush fake daemon response");
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(error) => panic!("accept fake daemon request failed: {error}"),
                }
            }

            assert!(
                saw_status,
                "migration coordinator never authenticated daemon"
            );
            assert!(
                !saw_shutdown,
                "backup failure must happen before daemon Shutdown"
            );
            drop(listener);
            let _ = std::fs::remove_file(&socket_path);
            drop(lease);
            drop(conn);
            return;
        }

        struct RestoreUnset(Vec<(&'static str, Option<OsString>)>);
        impl RestoreUnset {
            fn new(keys: &[&'static str]) -> Self {
                let values = keys
                    .iter()
                    .map(|key| (*key, std::env::var_os(key)))
                    .collect();
                for key in keys {
                    std::env::remove_var(key);
                }
                Self(values)
            }
        }
        impl Drop for RestoreUnset {
            fn drop(&mut self) {
                for (key, value) in &self.0 {
                    crate::test_support::restore_env(key, value);
                }
            }
        }

        let home = tempfile::tempdir().expect("isolated daemon home");
        let canonical_home = std::fs::canonicalize(home.path()).expect("canonical test home");
        let _env = crate::test_support::TestEnvGuard::isolated(&canonical_home);
        let _identity = RestoreUnset::new(&[
            "CC_SWITCH_CONFIG_DIR",
            "CLAUDE_CONFIG_DIR",
            "CODEX_HOME",
            "XDG_CONFIG_HOME",
        ]);
        let database_path = canonical_home.join(".cc-switch/cc-switch.db");
        std::fs::create_dir_all(database_path.parent().expect("database parent"))
            .expect("create database parent");
        {
            let conn = rusqlite::Connection::open(&database_path).expect("create migration db");
            conn.execute("CREATE TABLE migration_sentinel (id INTEGER)", [])
                .expect("create user table");
            conn.pragma_update(None, "user_version", 15)
                .expect("seed v15 schema");
        }
        std::fs::write(
            canonical_home.join(".cc-switch/backups"),
            b"not a directory",
        )
        .expect("block backup directory");

        let mut child = Command::new(std::env::current_exe().expect("resolve test binary"))
            .args(["--exact", TEST_NAME, "--nocapture", "--test-threads=1"])
            .env(CHILD_ENV, &canonical_home)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn fake daemon");
        let mut child_stdout = std::io::BufReader::new(child.stdout.take().expect("child stdout"));
        let mut output = String::new();
        loop {
            output.clear();
            child_stdout
                .read_line(&mut output)
                .expect("read fake daemon readiness");
            assert!(!output.is_empty(), "fake daemon exited before ready marker");
            if output.contains("FAKE_DAEMON_READY") {
                break;
            }
        }

        let error = quiesce_for_database_migration(&database_path)
            .err()
            .expect("blocked backup directory must fail migration");
        assert!(
            error.contains("Pre-migration backup failed"),
            "unexpected migration error: {error}"
        );
        assert!(
            child.wait().expect("wait for fake daemon").success(),
            "daemon received Shutdown despite backup failure"
        );

        #[cfg(not(target_os = "linux"))]
        {
            // The backup now succeeds, so the coordinator reaches daemon
            // identity preservation. Platforms that cannot recover the exact
            // startup environment must still fail before Shutdown.
            std::fs::remove_file(canonical_home.join(".cc-switch/backups"))
                .expect("unblock backup directory");
            let mut child = Command::new(std::env::current_exe().expect("resolve test binary"))
                .args(["--exact", TEST_NAME, "--nocapture", "--test-threads=1"])
                .env(CHILD_ENV, &canonical_home)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .spawn()
                .expect("spawn second fake daemon");
            let mut child_stdout =
                std::io::BufReader::new(child.stdout.take().expect("second child stdout"));
            loop {
                output.clear();
                child_stdout
                    .read_line(&mut output)
                    .expect("read second fake daemon readiness");
                assert!(
                    !output.is_empty(),
                    "second fake daemon exited before ready marker"
                );
                if output.contains("FAKE_DAEMON_READY") {
                    break;
                }
            }

            let error = quiesce_for_database_migration(&database_path)
                .err()
                .expect("unrecoverable daemon environment must fail migration");
            assert!(
                error.contains("cannot safely preserve daemon environment"),
                "unexpected migration error: {error}"
            );
            assert!(
                child.wait().expect("wait for second fake daemon").success(),
                "daemon received Shutdown despite unprovable environment identity"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_authenticated_daemon_environment_from_the_process() {
        use std::io::{BufRead, Write};
        use std::process::{Command, Stdio};

        const KEY: &str = "CC_SWITCH_TEST_DAEMON_ENVIRONMENT";
        const VALUE: &str = "custom path=with equals";

        let mut child = Command::new("sh")
            .args(["-c", "printf 'ready\\n'; read -r _"])
            .env(KEY, VALUE)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn environment holder");
        let mut ready = String::new();
        std::io::BufReader::new(child.stdout.take().expect("holder stdout"))
            .read_line(&mut ready)
            .expect("wait for environment holder");
        assert_eq!(ready, "ready\n");

        let environment = process_environment(child.id()).expect("read child environment");
        assert!(
            environment
                .iter()
                .any(|(key, value)| key == KEY && value == VALUE),
            "daemon environment value was not preserved exactly; parsed keys={:?}",
            environment
                .iter()
                .map(|(key, _)| key.to_string_lossy())
                .collect::<Vec<_>>()
        );

        child
            .stdin
            .as_mut()
            .expect("holder stdin")
            .write_all(b"done\n")
            .expect("release environment holder");
        assert!(child.wait().expect("wait for holder").success());
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn unsupported_platform_refuses_to_guess_daemon_environment() {
        let error = process_environment(std::process::id())
            .expect_err("unsupported platforms must fail closed");
        assert!(error.contains("cannot reliably recover"), "{error}");
    }

    #[test]
    fn resume_plan_unions_takeovers_and_workers_with_provider_fallbacks() {
        let plan = ResumePlan::from_status(
            TakeoverFlags {
                claude: true,
                codex: false,
                gemini: false,
            },
            vec![WorkerState {
                app_type: "codex".to_string(),
                running: true,
                address: "127.0.0.1".to_string(),
                port: 15722,
                pid: Some(42),
                started_at: None,
                runtime_status: Some(super::super::ipc::protocol::WorkerRuntimeStatus {
                    current_provider_id: Some("provider-a".to_string()),
                    ..Default::default()
                }),
            }],
            vec![(OsString::from("HOME"), OsString::from("/tmp/daemon-home"))],
        );

        assert_eq!(plan.targets.len(), 2);
        assert_eq!(plan.targets[0].app_type, "claude");
        assert_eq!(plan.targets[0].fallback_provider_id, None);
        assert_eq!(plan.targets[1].app_type, "codex");
        assert_eq!(
            plan.targets[1].fallback_provider_id.as_deref(),
            Some("provider-a")
        );
        assert_eq!(
            plan.environment,
            vec![(OsString::from("HOME"), OsString::from("/tmp/daemon-home"))]
        );
    }
}
