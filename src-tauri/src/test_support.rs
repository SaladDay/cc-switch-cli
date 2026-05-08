use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, RwLock};

pub(crate) type TestHomeSettingsLock = MutexGuard<'static, ()>;

pub(crate) fn lock_test_home_and_settings() -> TestHomeSettingsLock {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn test_home_store() -> &'static RwLock<Option<PathBuf>> {
    static STORE: OnceLock<RwLock<Option<PathBuf>>> = OnceLock::new();
    STORE.get_or_init(|| RwLock::new(None))
}

pub(crate) fn set_test_home_override(path: Option<&Path>) {
    let mut guard = test_home_store()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = path.map(Path::to_path_buf);
}

pub(crate) fn test_home_override() -> Option<PathBuf> {
    test_home_store()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Serialises tests that mutate the global CodexOAuthService manager_store.
/// `serial_test::#[serial]` is unreliable with `#[tokio::test]` on multi-threaded
/// runtimes; a dedicated `Mutex` is more predictable.
pub(crate) fn lock_codex_oauth_test() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
