use serde_json::json;
use serial_test::serial;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
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
use error::AppError;
use settings_impl::{
    default_visible_apps, get_settings, get_visible_apps, next_visible_app, reload_test_settings,
    set_visible_apps, update_settings, AppSettings, VisibleApps,
};

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
        reload_test_settings();

        Self {
            _temp: temp,
            old_home,
            old_userprofile,
        }
    }

    fn path(&self) -> &Path {
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

        reload_test_settings();
    }
}

fn settings_path(home: &HomeGuard) -> PathBuf {
    home.path().join(".cc-switch").join("settings.json")
}

fn write_settings_json(home: &HomeGuard, value: serde_json::Value) {
    let path = settings_path(home);
    fs::create_dir_all(path.parent().expect("settings parent")).expect("create settings dir");
    fs::write(
        &path,
        serde_json::to_string_pretty(&value).expect("serialize settings json"),
    )
    .expect("write settings json");
}

fn read_settings_json(home: &HomeGuard) -> serde_json::Value {
    let content = fs::read_to_string(settings_path(home)).expect("read settings json");
    serde_json::from_str(&content).expect("parse settings json")
}

#[test]
#[serial]
fn default_visible_apps_hide_gemini() {
    let _home = HomeGuard::new();

    let visible = default_visible_apps();
    assert_eq!(visible, get_visible_apps());
    assert_eq!(
        visible.ordered_enabled(),
        vec![
            AppType::Claude,
            AppType::Codex,
            AppType::OpenCode,
            AppType::OpenClaw,
        ]
    );
    assert!(!visible.is_enabled_for(&AppType::Gemini));
}

#[test]
#[serial]
fn set_visible_apps_persists_visible_apps_as_camel_case_json() {
    let home = HomeGuard::new();

    set_visible_apps(VisibleApps {
        claude: false,
        codex: true,
        gemini: true,
        opencode: false,
        openclaw: true,
    })
    .expect("persist visible apps");

    let raw = fs::read_to_string(settings_path(&home)).expect("read settings json");
    assert!(raw.contains("\"visibleApps\""));
    assert!(!raw.contains("\"visible_apps\""));

    let value = read_settings_json(&home);
    assert_eq!(
        value["visibleApps"],
        json!({
            "claude": false,
            "codex": true,
            "gemini": true,
            "opencode": false,
            "openclaw": true,
        })
    );
}

#[test]
#[serial]
fn load_reads_valid_non_default_visible_apps_from_settings_json() {
    let home = HomeGuard::new();
    write_settings_json(
        &home,
        json!({
            "visibleApps": {
                "claude": false,
                "codex": true,
                "gemini": true,
                "opencode": true,
                "openclaw": false,
            }
        }),
    );

    reload_test_settings();

    let visible = get_visible_apps();
    assert_eq!(
        visible,
        VisibleApps {
            claude: false,
            codex: true,
            gemini: true,
            opencode: true,
            openclaw: false,
        }
    );
    assert_eq!(
        visible.ordered_enabled(),
        vec![AppType::Codex, AppType::Gemini, AppType::OpenCode]
    );
}

#[test]
#[serial]
fn load_partial_visible_apps_object_uses_defaults_for_missing_keys() {
    let home = HomeGuard::new();
    write_settings_json(
        &home,
        json!({
            "visibleApps": {
                "claude": false
            }
        }),
    );

    reload_test_settings();

    assert_eq!(
        get_visible_apps(),
        VisibleApps {
            claude: false,
            codex: true,
            gemini: false,
            opencode: true,
            openclaw: true,
        }
    );
}

#[test]
#[serial]
fn missing_visible_apps_field_uses_defaults_without_losing_other_fields() {
    let home = HomeGuard::new();
    write_settings_json(
        &home,
        json!({
            "showInTray": false,
            "language": "zh"
        }),
    );

    reload_test_settings();

    assert_eq!(get_visible_apps(), default_visible_apps());

    let settings = get_settings();
    assert!(!settings.show_in_tray);
    assert_eq!(settings.language.as_deref(), Some("zh"));
}

#[test]
#[serial]
fn set_visible_apps_rejects_zero_selection() {
    let _home = HomeGuard::new();

    let err = set_visible_apps(VisibleApps {
        claude: false,
        codex: false,
        gemini: false,
        opencode: false,
        openclaw: false,
    })
    .expect_err("zero visible apps should be rejected");

    match err {
        AppError::InvalidInput(message) => assert!(message.contains("At least one app")),
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
#[serial]
fn update_settings_rejects_all_false_visible_apps() {
    let _home = HomeGuard::new();

    let mut settings = AppSettings::default();
    settings.visible_apps = VisibleApps {
        claude: false,
        codex: false,
        gemini: false,
        opencode: false,
        openclaw: false,
    };

    let err =
        update_settings(settings).expect_err("update_settings should reject zero visible apps");

    match err {
        AppError::InvalidInput(message) => assert!(message.contains("At least one app")),
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
#[serial]
fn empty_visible_apps_object_normalizes_without_resetting_the_file() {
    let home = HomeGuard::new();
    let path = settings_path(&home);
    write_settings_json(
        &home,
        json!({
            "showInTray": false,
            "visibleApps": {}
        }),
    );

    let original = fs::read_to_string(&path).expect("read original settings json");
    reload_test_settings();

    assert_eq!(get_visible_apps(), default_visible_apps());
    assert!(!get_settings().show_in_tray);
    assert_eq!(
        fs::read_to_string(&path).expect("read settings json after load"),
        original
    );
}

#[test]
#[serial]
fn load_normalizes_all_false_visible_apps_to_defaults() {
    let home = HomeGuard::new();
    write_settings_json(
        &home,
        json!({
            "visibleApps": {
                "claude": false,
                "codex": false,
                "gemini": false,
                "opencode": false,
                "openclaw": false
            }
        }),
    );

    reload_test_settings();

    let settings = AppSettings::load();
    assert_eq!(settings.visible_apps, default_visible_apps());
    assert_eq!(get_visible_apps(), default_visible_apps());
}

#[test]
#[serial]
fn malformed_visible_apps_json_falls_back_to_full_defaults() {
    let home = HomeGuard::new();
    write_settings_json(
        &home,
        json!({
            "showInTray": false,
            "language": "zh",
            "visibleApps": {
                "claude": "yes"
            }
        }),
    );

    reload_test_settings();

    let settings = get_settings();
    assert_eq!(settings.show_in_tray, AppSettings::default().show_in_tray);
    assert_eq!(settings.language, AppSettings::default().language);
    assert_eq!(settings.visible_apps, default_visible_apps());
}

#[test]
#[serial]
fn next_visible_app_wraps_and_skips_hidden_entries() {
    let visible = VisibleApps {
        claude: true,
        codex: false,
        gemini: false,
        opencode: true,
        openclaw: true,
    };

    assert_eq!(
        next_visible_app(&visible, &AppType::Claude, 1),
        Some(AppType::OpenCode)
    );
    assert_eq!(
        next_visible_app(&visible, &AppType::OpenClaw, 1),
        Some(AppType::Claude)
    );
    assert_eq!(
        next_visible_app(&visible, &AppType::Claude, -1),
        Some(AppType::OpenClaw)
    );
    assert_eq!(
        next_visible_app(&visible, &AppType::OpenCode, -1),
        Some(AppType::Claude)
    );
}
