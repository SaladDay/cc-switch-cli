use serial_test::serial;
use std::ffi::OsString;
use tempfile::TempDir;

mod app_config {
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub enum AppType {
        Claude,
        Codex,
        Gemini,
        OpenCode,
        OpenClaw,
    }

    impl AppType {
        pub fn as_str(&self) -> &'static str {
            match self {
                AppType::Claude => "claude",
                AppType::Codex => "codex",
                AppType::Gemini => "gemini",
                AppType::OpenCode => "opencode",
                AppType::OpenClaw => "openclaw",
            }
        }
    }
}

mod claude_mcp {
    use crate::error::AppError;

    pub fn set_has_completed_onboarding() -> Result<(), AppError> {
        Ok(())
    }

    pub fn clear_has_completed_onboarding() -> Result<(), AppError> {
        Ok(())
    }
}

mod config {
    use std::path::PathBuf;

    pub(crate) fn home_dir() -> Option<PathBuf> {
        dirs::home_dir()
    }

    pub(crate) fn get_app_config_dir() -> PathBuf {
        if let Some(custom) = std::env::var_os("CC_SWITCH_CONFIG_DIR") {
            let custom = PathBuf::from(custom);
            if !custom.to_string_lossy().trim().is_empty() {
                return custom;
            }
        }

        home_dir().expect("无法获取用户主目录").join(".cc-switch")
    }
}

mod database {
    use super::error::AppError;
    use indexmap::IndexMap;
    use std::collections::HashMap;

    #[derive(Debug, Clone, Default)]
    pub struct Database {
        providers: HashMap<String, IndexMap<String, serde_json::Value>>,
        current: HashMap<String, String>,
    }

    impl Database {
        pub fn insert_provider(&mut self, app_type: &str, id: &str) {
            self.providers
                .entry(app_type.to_string())
                .or_default()
                .insert(id.to_string(), serde_json::json!({}));
        }

        pub fn set_db_current(&mut self, app_type: &str, id: &str) {
            self.current.insert(app_type.to_string(), id.to_string());
        }

        pub fn get_all_providers(
            &self,
            app_type: &str,
        ) -> Result<IndexMap<String, serde_json::Value>, AppError> {
            Ok(self.providers.get(app_type).cloned().unwrap_or_default())
        }

        pub fn get_current_provider(&self, app_type: &str) -> Result<Option<String>, AppError> {
            Ok(self.current.get(app_type).cloned())
        }
    }
}

mod error {
    use std::path::Path;
    use std::sync::PoisonError;

    use thiserror::Error;

    #[derive(Debug, Error)]
    pub enum AppError {
        #[error("配置错误: {0}")]
        Config(String),
        #[error("无效输入: {0}")]
        InvalidInput(String),
        #[error("IO 错误: {path}: {source}")]
        Io {
            path: String,
            #[source]
            source: std::io::Error,
        },
        #[error("JSON 序列化失败: {source}")]
        JsonSerialize {
            #[source]
            source: serde_json::Error,
        },
        #[error("锁获取失败: {0}")]
        Lock(String),
    }

    impl AppError {
        pub fn io(path: impl AsRef<Path>, source: std::io::Error) -> Self {
            Self::Io {
                path: path.as_ref().display().to_string(),
                source,
            }
        }
    }

    impl<T> From<PoisonError<T>> for AppError {
        fn from(err: PoisonError<T>) -> Self {
            Self::Lock(err.to_string())
        }
    }
}

mod services {
    pub mod skill {
        use serde::{Deserialize, Serialize};

        #[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
        #[serde(rename_all = "lowercase")]
        pub enum SyncMethod {
            #[default]
            Auto,
            Symlink,
            Copy,
        }
    }

    pub mod webdav {
        use crate::error::AppError;
        use url::Url;

        pub fn parse_base_url(raw: &str) -> Result<Url, AppError> {
            let trimmed = raw.trim().trim_end_matches('/');
            let url = Url::parse(trimmed).map_err(|e| {
                AppError::InvalidInput(format!("WebDAV base_url 不是合法 URL: {e}"))
            })?;
            let scheme = url.scheme();
            if scheme != "http" && scheme != "https" {
                return Err(AppError::InvalidInput(
                    "WebDAV base_url 仅支持 http/https".to_string(),
                ));
            }

            validate_provider_base_url(&url)?;
            Ok(url)
        }

        fn validate_provider_base_url(url: &Url) -> Result<(), AppError> {
            let Some((provider_name, dav_example)) = detect_provider(url) else {
                return Ok(());
            };

            let points_under_dav = url
                .path_segments()
                .and_then(|mut segments| segments.next())
                .is_some_and(|segment| segment == "dav");
            if points_under_dav {
                return Ok(());
            }

            Err(AppError::InvalidInput(format!(
                "{provider_name} WebDAV base_url 必须指向 /dav 下的目录，例如 {dav_example}"
            )))
        }

        fn detect_provider(url: &Url) -> Option<(&'static str, &'static str)> {
            match url.host_str()? {
                host if host.eq_ignore_ascii_case("dav.jianguoyun.com") => {
                    Some(("坚果云", "https://dav.jianguoyun.com/dav/..."))
                }
                host if host.eq_ignore_ascii_case("dav.nutstore.net") => {
                    Some(("Nutstore", "https://dav.nutstore.net/dav/..."))
                }
                _ => None,
            }
        }
    }
}

#[path = "../src/settings.rs"]
mod settings_impl;

use app_config::AppType;
use database::Database;
use settings_impl::{get_current_provider, get_effective_current_provider, set_current_provider};

struct HomeGuard {
    _temp: TempDir,
    old_home: Option<OsString>,
    old_userprofile: Option<OsString>,
}

impl HomeGuard {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("create tempdir");
        let old_home = std::env::var_os("HOME");
        let old_userprofile = std::env::var_os("USERPROFILE");
        std::env::set_var("HOME", temp.path());
        std::env::set_var("USERPROFILE", temp.path());

        Self {
            _temp: temp,
            old_home,
            old_userprofile,
        }
    }

    fn path(&self) -> &std::path::Path {
        self._temp.path()
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        if let Some(value) = self.old_home.take() {
            std::env::set_var("HOME", value);
        } else {
            std::env::remove_var("HOME");
        }

        if let Some(value) = self.old_userprofile.take() {
            std::env::set_var("USERPROFILE", value);
        } else {
            std::env::remove_var("USERPROFILE");
        }
    }
}

#[test]
#[serial]
fn settings_current_provider_openclaw_matches_upstream_placeholder_behavior() {
    let _home = HomeGuard::new();

    set_current_provider(&AppType::OpenClaw, Some("local-openclaw"))
        .expect("store local openclaw provider placeholder");
    assert_eq!(
        get_current_provider(&AppType::OpenClaw).as_deref(),
        Some("local-openclaw")
    );

    let mut db = Database::default();
    db.insert_provider("openclaw", "local-openclaw");
    db.set_db_current("openclaw", "db-openclaw");

    assert_eq!(
        get_effective_current_provider(&db, &AppType::OpenClaw)
            .expect("resolve effective openclaw provider")
            .as_deref(),
        Some("local-openclaw"),
        "existing local placeholder should win while it still exists in the database"
    );

    set_current_provider(&AppType::OpenClaw, Some("missing-openclaw"))
        .expect("overwrite local openclaw placeholder");
    assert_eq!(
        get_effective_current_provider(&db, &AppType::OpenClaw)
            .expect("fallback to database current provider")
            .as_deref(),
        Some("db-openclaw"),
        "missing local placeholder should be cleared and fall back to database current"
    );
    assert_eq!(
        get_current_provider(&AppType::OpenClaw),
        None,
        "invalid local placeholder should be removed after fallback"
    );

    set_current_provider(&AppType::OpenClaw, None).expect("clear local placeholder");
    assert_eq!(get_current_provider(&AppType::OpenClaw), None);
}

#[test]
#[serial]
fn settings_current_provider_openclaw_falls_back_to_db_when_cleanup_fails() {
    let home = HomeGuard::new();

    set_current_provider(&AppType::OpenClaw, Some("missing-openclaw"))
        .expect("store invalid openclaw placeholder");

    let settings_dir = home.path().join(".cc-switch");
    std::fs::remove_dir_all(&settings_dir).expect("remove settings dir before blocking writes");
    std::fs::write(&settings_dir, "block settings writes").expect("create file to block cleanup");

    let mut db = Database::default();
    db.insert_provider("openclaw", "db-openclaw");
    db.set_db_current("openclaw", "db-openclaw");

    assert_eq!(
        get_effective_current_provider(&db, &AppType::OpenClaw)
            .expect("cleanup failure should still fall back to database current")
            .as_deref(),
        Some("db-openclaw")
    );
}
