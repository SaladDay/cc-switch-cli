use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock};

use cc_switch_lib::{
    update_settings, AppSettings, AppState, Database, MultiAppConfig, ProxyService,
};

/// 为测试设置隔离的 HOME 目录，避免污染真实用户数据。
pub fn ensure_test_home() -> &'static Path {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let base = std::env::temp_dir().join("cc-switch-test-home");
        if base.exists() {
            let _ = std::fs::remove_dir_all(&base);
        }
        std::fs::create_dir_all(&base).expect("create test home");
        std::env::set_var("HOME", &base);
        #[cfg(windows)]
        std::env::set_var("USERPROFILE", &base);
        base
    })
    .as_path()
}

/// 清理测试目录中生成的配置文件与缓存。
pub fn reset_test_fs() {
    let home = ensure_test_home();
    for sub in [
        ".claude",
        ".codex",
        ".cc-switch",
        ".gemini",
        ".config",
        ".openclaw",
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
