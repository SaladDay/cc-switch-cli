use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;
use std::sync::OnceLock;

use tokio::sync::{Mutex, MutexGuard};

pub(crate) const RESTORE_GUARD_BYPASS_ENV_KEY: &str = "CC_SWITCH_RESTORE_GUARD_BYPASS";

fn process_mutex() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub(crate) struct RestoreMutationGuard {
    _process_guard: MutexGuard<'static, ()>,
    _file: File,
}

impl RestoreMutationGuard {
    fn lock_path() -> PathBuf {
        crate::config::get_app_config_dir().join("state-mutation.lock")
    }
}

impl Drop for RestoreMutationGuard {
    fn drop(&mut self) {
        let _ = self._file.unlock();
    }
}

pub(crate) async fn acquire_restore_mutation_guard() -> Result<RestoreMutationGuard, String> {
    let lock_path = RestoreMutationGuard::lock_path();
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create state coordination dir failed: {error}"))?;
    }

    if std::env::var_os(RESTORE_GUARD_BYPASS_ENV_KEY).is_some() {
        return Ok(RestoreMutationGuard {
            _process_guard: process_mutex().lock().await,
            _file: OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&lock_path)
                .map_err(|error| format!("open bypass state coordination lock failed: {error}"))?,
        });
    }

    let process_guard = process_mutex().lock().await;

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| format!("open state coordination lock failed: {error}"))?;
    file.lock()
        .map_err(|error| format!("lock state coordination file failed: {error}"))?;

    Ok(RestoreMutationGuard {
        _process_guard: process_guard,
        _file: file,
    })
}

pub(crate) fn clear_restore_mutation_guard_bypass_env() {
    unsafe { std::env::remove_var(RESTORE_GUARD_BYPASS_ENV_KEY) };
}
