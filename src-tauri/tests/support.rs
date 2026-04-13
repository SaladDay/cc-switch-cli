use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock};

use cc_switch_lib::{
    update_settings, AppSettings, AppState, Database, MultiAppConfig, ProxyService,
};

/// 为测试设置隔离的 HOME 目录，避免污染真实用户数据。
pub fn ensure_test_home() -> &'static Path {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    let home = HOME.get_or_init(|| {
        // 每个测试进程使用独立目录，避免 integration test 二进制并行时互相清理文件。
        let base = std::env::temp_dir().join(format!("cc-switch-test-home-{}", std::process::id()));
        if base.exists() {
            let _ = std::fs::remove_dir_all(&base);
        }
        std::fs::create_dir_all(&base).expect("create test home");
        base
    });
    std::env::set_var("HOME", home);
    #[cfg(windows)]
    std::env::set_var("USERPROFILE", home);
    home.as_path()
}

/// 清理测试目录中生成的配置文件与缓存。
pub fn reset_test_fs() {
    let home = ensure_test_home();
    for sub in [
        ".claude",
        ".codex",
        ".cc-switch",
        ".gemini",
        ".openclaw",
        ".config",
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
