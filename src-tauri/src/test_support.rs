use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, RwLock};
use std::time::{Duration, Instant};

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

pub(crate) struct TestEnvGuard {
    _lock: TestHomeSettingsLock,
    home: PathBuf,
    old_home: Option<OsString>,
    old_userprofile: Option<OsString>,
    old_cc_switch_config_dir: Option<OsString>,
    old_claude_config_dir: Option<OsString>,
    old_codex_home: Option<OsString>,
    old_xdg_runtime_dir: Option<OsString>,
}

impl TestEnvGuard {
    pub(crate) fn isolated(home: &Path) -> Self {
        let lock = lock_test_home_and_settings();
        let old_home = std::env::var_os("HOME");
        let old_userprofile = std::env::var_os("USERPROFILE");
        let old_cc_switch_config_dir = std::env::var_os("CC_SWITCH_CONFIG_DIR");
        let old_claude_config_dir = std::env::var_os("CLAUDE_CONFIG_DIR");
        let old_codex_home = std::env::var_os("CODEX_HOME");
        let old_xdg_runtime_dir = std::env::var_os("XDG_RUNTIME_DIR");

        std::env::set_var("HOME", home);
        std::env::set_var("USERPROFILE", home);
        std::env::set_var("CC_SWITCH_CONFIG_DIR", home.join(".cc-switch"));
        std::env::set_var("CLAUDE_CONFIG_DIR", home.join(".claude"));
        std::env::set_var("CODEX_HOME", home.join(".codex"));
        std::env::set_var("XDG_RUNTIME_DIR", home.join(".runtime"));
        set_test_home_override(Some(home));
        crate::settings::reload_test_settings();

        Self {
            _lock: lock,
            home: home.to_path_buf(),
            old_home,
            old_userprofile,
            old_cc_switch_config_dir,
            old_claude_config_dir,
            old_codex_home,
            old_xdg_runtime_dir,
        }
    }

    pub(crate) fn set_home(home: &Path) -> Self {
        Self::isolated(home)
    }
}

impl Drop for TestEnvGuard {
    fn drop(&mut self) {
        cleanup_test_processes_under(&self.home);
        restore_env("HOME", &self.old_home);
        restore_env("USERPROFILE", &self.old_userprofile);
        restore_env("CC_SWITCH_CONFIG_DIR", &self.old_cc_switch_config_dir);
        restore_env("CLAUDE_CONFIG_DIR", &self.old_claude_config_dir);
        restore_env("CODEX_HOME", &self.old_codex_home);
        restore_env("XDG_RUNTIME_DIR", &self.old_xdg_runtime_dir);
        set_test_home_override(self.old_home.as_deref().map(Path::new));
        crate::settings::reload_test_settings();
    }
}

pub(crate) fn restore_env(key: &str, value: &Option<OsString>) {
    match value {
        Some(value) => std::env::set_var(key, value),
        None => std::env::remove_var(key),
    }
}

pub(crate) fn cleanup_test_processes_under(root: &Path) {
    #[cfg(unix)]
    {
        cleanup_daemon_at_paths(
            root,
            &root.join(".runtime").join("cc-switch").join("daemon.sock"),
            &root.join(".runtime").join("cc-switch").join("daemon.pid"),
        );
    }

    #[cfg(not(unix))]
    {
        let _ = root;
    }
}

#[cfg(unix)]
fn cleanup_daemon_at_paths(root: &Path, socket: &Path, pidfile: &Path) {
    if !path_is_under(socket, root) || !path_is_under(pidfile, root) {
        return;
    }

    let pidfile_lock_held = pidfile_lock_is_held(pidfile);
    let pid = read_pidfile(pidfile);
    let shutdown_requested = request_daemon_shutdown(socket);

    if let Some(pid) = pid {
        if shutdown_requested && wait_for_process_exit(pid, Duration::from_secs(5)) {
            return;
        }
        if pidfile_lock_held {
            terminate_process(pid);
        }
    } else if shutdown_requested {
        let _ = wait_for_path_absent(socket, Duration::from_secs(5));
    }
}

#[cfg(unix)]
fn request_daemon_shutdown(socket: &Path) -> bool {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    if !socket.exists() {
        return false;
    }

    if let Ok(mut stream) = UnixStream::connect(socket) {
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = stream.write_all(b"{\"kind\":\"shutdown\"}\n");
        let _ = stream.flush();
        let mut sink = String::new();
        let _ = BufReader::new(&stream).read_line(&mut sink);
        return true;
    }

    false
}

#[cfg(unix)]
fn pidfile_lock_is_held(path: &Path) -> bool {
    use std::fs::OpenOptions;
    use std::os::unix::io::AsRawFd;

    let Ok(file) = OpenOptions::new().read(true).write(true).open(path) else {
        return false;
    };

    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        let _ = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        return false;
    }

    std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock
}

#[cfg(unix)]
fn read_pidfile(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(unix)]
fn terminate_process(pid: u32) {
    if !is_process_alive(pid) {
        return;
    }

    let _ = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    if wait_for_process_exit(pid, Duration::from_secs(2)) {
        return;
    }

    let _ = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    let _ = wait_for_process_exit(pid, Duration::from_secs(5));
}

#[cfg(unix)]
fn wait_for_process_exit(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !is_process_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    !is_process_alive(pid)
}

#[cfg(unix)]
fn wait_for_path_absent(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    !path.exists()
}

#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    if pid == 0 || pid == std::process::id() {
        return false;
    }

    let rc = unsafe { libc::kill(pid as i32, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(unix)]
fn path_is_under(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}
