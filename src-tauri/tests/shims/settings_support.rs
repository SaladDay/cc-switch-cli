use std::ffi::OsString;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

fn environment_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) struct TestEnvGuard {
    _lock: MutexGuard<'static, ()>,
    old_home: Option<OsString>,
    old_userprofile: Option<OsString>,
    old_config_dir: Option<OsString>,
    old_claude_dir: Option<OsString>,
    old_codex_home: Option<OsString>,
}

impl TestEnvGuard {
    pub(crate) fn isolated(home: &Path) -> Self {
        let lock = environment_lock();
        let old_home = std::env::var_os("HOME");
        let old_userprofile = std::env::var_os("USERPROFILE");
        let old_config_dir = std::env::var_os("CC_SWITCH_CONFIG_DIR");
        let old_claude_dir = std::env::var_os("CLAUDE_CONFIG_DIR");
        let old_codex_home = std::env::var_os("CODEX_HOME");

        std::env::set_var("HOME", home);
        std::env::set_var("USERPROFILE", home);
        std::env::set_var("CC_SWITCH_CONFIG_DIR", home.join(".cc-switch"));
        std::env::set_var("CLAUDE_CONFIG_DIR", home.join(".claude"));
        std::env::set_var("CODEX_HOME", home.join(".codex"));
        crate::settings_impl::reload_test_settings();

        Self {
            _lock: lock,
            old_home,
            old_userprofile,
            old_config_dir,
            old_claude_dir,
            old_codex_home,
        }
    }
}

impl Drop for TestEnvGuard {
    fn drop(&mut self) {
        restore_env("HOME", self.old_home.take());
        restore_env("USERPROFILE", self.old_userprofile.take());
        restore_env("CC_SWITCH_CONFIG_DIR", self.old_config_dir.take());
        restore_env("CLAUDE_CONFIG_DIR", self.old_claude_dir.take());
        restore_env("CODEX_HOME", self.old_codex_home.take());
        crate::settings_impl::reload_test_settings();
    }
}

fn restore_env(key: &str, value: Option<OsString>) {
    match value {
        Some(value) => std::env::set_var(key, value),
        None => std::env::remove_var(key),
    }
}
