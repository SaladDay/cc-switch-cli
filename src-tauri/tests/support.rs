#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock};
use std::time::{Duration, Instant};

use cc_switch_lib::{
    update_settings, AppSettings, AppState, Database, MultiAppConfig, ProviderService, ProxyService,
};

/// 为测试设置隔离的 HOME 目录，避免污染真实用户数据。
pub fn ensure_test_home() -> &'static Path {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    let home = HOME.get_or_init(|| {
        // 每个测试进程使用独立目录；路径保持短以避开 macOS Unix socket 路径长度限制。
        let base = std::env::temp_dir().join(format!("ccs-t-{}", std::process::id()));
        if base.exists() {
            let _ = std::fs::remove_dir_all(&base);
        }
        std::fs::create_dir_all(&base).expect("create test home");
        base
    });
    std::env::set_var("HOME", home);
    #[cfg(windows)]
    std::env::set_var("USERPROFILE", home);
    std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
    std::env::set_var("XDG_RUNTIME_DIR", home.join(".runtime"));
    std::env::set_var("XDG_STATE_HOME", home.join(".state"));
    std::env::set_var("CC_SWITCH_CONFIG_DIR", home.join(".cc-switch"));
    std::env::set_var("CLAUDE_CONFIG_DIR", home.join(".claude"));
    std::env::set_var("CODEX_HOME", home.join(".codex"));
    home.as_path()
}

/// 清理测试目录中生成的配置文件与缓存。
pub fn reset_test_fs() {
    let home = ensure_test_home();
    cleanup_test_processes_under(home);
    for sub in [
        ".claude",
        ".codex",
        ".cc-switch",
        ".gemini",
        ".openclaw",
        ".config",
        ".runtime",
        ".state",
    ] {
        let path = home.join(sub);
        if path.exists() {
            if let Err(err) = std::fs::remove_dir_all(&path) {
                eprintln!("failed to clean {}: {}", path.display(), err);
            }
        }
    }
    let claude_json = home.join(".claude.json");
    if claude_json.exists() {
        let _ = std::fs::remove_file(&claude_json);
    }

    // 重置内存中的设置缓存，确保测试环境不受上一次调用影响
    let _ = update_settings(AppSettings::default());
}

#[allow(dead_code)]
pub fn enable_codex_official_auth_preservation() {
    update_settings(AppSettings {
        preserve_codex_official_auth_on_switch: true,
        ..Default::default()
    })
    .expect("enable Codex official auth preservation");
}

#[cfg(unix)]
pub struct TestProcessCleanup {
    root: PathBuf,
}

#[cfg(unix)]
impl TestProcessCleanup {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn cleanup_now(&self) {
        cleanup_test_processes_under(&self.root);
    }
}

#[cfg(unix)]
impl Drop for TestProcessCleanup {
    fn drop(&mut self) {
        self.cleanup_now();
    }
}

#[cfg(not(unix))]
pub struct TestProcessCleanup;

#[cfg(not(unix))]
impl TestProcessCleanup {
    pub fn new(_root: impl AsRef<Path>) -> Self {
        Self
    }

    pub fn cleanup_now(&self) {}
}

pub fn cleanup_test_processes() {
    cleanup_test_processes_under(ensure_test_home());
}

pub fn cleanup_test_processes_under(root: &Path) {
    #[cfg(unix)]
    {
        cleanup_daemon_at_paths(
            root,
            &root.join(".runtime").join("cc-switch").join("daemon.sock"),
            &root.join(".runtime").join("cc-switch").join("daemon.pid"),
        );
        cleanup_daemon_at_paths(
            root,
            &root.join("run").join("cc-switch").join("daemon.sock"),
            &root.join("run").join("cc-switch").join("daemon.pid"),
        );
    }

    #[cfg(not(unix))]
    {
        let _ = root;
    }
}

#[cfg(unix)]
pub fn cleanup_daemon_at_paths(root: &Path, socket: &Path, pidfile: &Path) {
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
pub fn is_process_alive(pid: u32) -> bool {
    if pid == 0 || pid == std::process::id() {
        return false;
    }

    let rc = unsafe { libc::kill(pid as i32, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(unix)]
pub fn wait_for_process_exit(pid: u32, timeout: Duration) -> bool {
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
pub fn terminate_process(pid: u32) {
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
fn path_is_under(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

/// 全局互斥锁，避免多测试并发写入相同的 HOME 目录。
pub fn test_mutex() -> &'static Mutex<()> {
    static MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
    MUTEX.get_or_init(|| Mutex::new(()))
}

pub fn lock_test_mutex() -> MutexGuard<'static, ()> {
    test_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn state_from_config(config: MultiAppConfig) -> AppState {
    let _ = ensure_test_home();
    let db = Arc::new(Database::init().expect("create database"));
    db.migrate_from_json(&config)
        .expect("seed database from config");
    let mut config = config;
    ProviderService::migrate_common_config_upstream_semantics_if_needed(&db, &mut config)
        .expect("migrate common config semantics for test state");
    AppState {
        db: db.clone(),
        config: RwLock::new(config),
        proxy_service: ProxyService::new(db),
    }
}

#[allow(dead_code)]
pub struct CurrentDirGuard {
    previous: PathBuf,
}

impl CurrentDirGuard {
    #[allow(dead_code)]
    pub fn change_to(path: &Path) -> Self {
        let previous = std::env::current_dir().expect("read current dir");
        std::fs::create_dir_all(path).expect("create target cwd");
        std::env::set_current_dir(path).expect("switch current dir");
        Self { previous }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.previous).expect("restore current dir");
    }
}
