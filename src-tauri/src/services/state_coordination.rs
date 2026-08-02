//! Process- and host-wide mutation capabilities.
//!
//! Ordinary workflows hold a shared permit across their complete
//! load/compute/write/projection lifetime. Restore owns the exclusive permit.
//! The OS lock is acquired once per workflow, never once per database row.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

fn process_gate() -> &'static RwLock<()> {
    static GATE: OnceLock<RwLock<()>> = OnceLock::new();
    GATE.get_or_init(|| RwLock::new(()))
}

pub(crate) struct OrdinaryMutationPermit {
    _process: Option<RwLockReadGuard<'static, ()>>,
    file: Option<File>,
}

pub(crate) struct RestoreExclusivePermit {
    _process: RwLockWriteGuard<'static, ()>,
    file: File,
}

/// Opaque barrier for a coherent read of state split across SQLite and the
/// managed filesystem. It deliberately cannot be passed to restore APIs.
pub(crate) struct ConsistentStateSnapshotPermit {
    _exclusive: RestoreExclusivePermit,
}

impl Drop for OrdinaryMutationPermit {
    fn drop(&mut self) {
        if let Some(file) = &self.file {
            let _ = file.unlock();
        }
    }
}

impl Drop for RestoreExclusivePermit {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn lock_path() -> PathBuf {
    crate::config::get_app_config_dir().join("state-mutation.lock")
}

fn open_lock_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn prepare_lock_file() -> Result<File, String> {
    let path = lock_path();
    if let Some(parent) = path.parent() {
        crate::config::create_managed_config_dir_all(parent)
            .map_err(|error| format!("create state coordination directory failed: {error}"))?;
    }
    open_lock_file(&path).map_err(|error| format!("open state coordination lock failed: {error}"))
}

pub(crate) async fn acquire_ordinary_mutation_permit() -> Result<OrdinaryMutationPermit, String> {
    let process = process_gate().read().await;
    let file = prepare_lock_file()?;
    file.lock_shared()
        .map_err(|error| format!("lock shared state coordination file failed: {error}"))?;
    Ok(OrdinaryMutationPermit {
        _process: Some(process),
        file: Some(file),
    })
}

#[cfg(test)]
impl OrdinaryMutationPermit {
    /// In-memory unit tests need the same explicit capability shape without
    /// creating or locking the user's real configuration directory.
    pub(crate) fn for_in_memory_test() -> Self {
        Self {
            _process: None,
            file: None,
        }
    }
}

pub(crate) fn acquire_ordinary_mutation_permit_blocking() -> Result<OrdinaryMutationPermit, String>
{
    futures::executor::block_on(acquire_ordinary_mutation_permit())
}

pub(crate) async fn acquire_restore_exclusive_permit() -> Result<RestoreExclusivePermit, String> {
    let process = process_gate().write().await;
    let file = prepare_lock_file()?;
    file.lock()
        .map_err(|error| format!("lock exclusive state coordination file failed: {error}"))?;
    Ok(RestoreExclusivePermit {
        _process: process,
        file,
    })
}

pub(crate) fn acquire_restore_exclusive_permit_blocking() -> Result<RestoreExclusivePermit, String>
{
    futures::executor::block_on(acquire_restore_exclusive_permit())
}

pub(crate) fn acquire_consistent_state_snapshot_permit_blocking(
) -> Result<ConsistentStateSnapshotPermit, String> {
    acquire_restore_exclusive_permit_blocking().map(|exclusive| ConsistentStateSnapshotPermit {
        _exclusive: exclusive,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        acquire_ordinary_mutation_permit_blocking, acquire_restore_exclusive_permit_blocking,
    };
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    #[serial_test::serial(home_settings)]
    fn exclusive_restore_waits_for_workflow_scoped_shared_permit() {
        let home = tempfile::tempdir().expect("isolated state-coordination home");
        let _environment = crate::test_support::TestEnvGuard::isolated(home.path());
        let ordinary =
            acquire_ordinary_mutation_permit_blocking().expect("acquire ordinary permit");
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            sender
                .send(acquire_restore_exclusive_permit_blocking().map(|_| ()))
                .expect("send restore acquisition result");
        });
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(150)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        drop(ordinary);
        receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("restore resumes after ordinary workflow")
            .expect("acquire restore");
    }
}
