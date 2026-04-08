use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use cc_switch_lib::{
    get_app_config_dir, update_settings, AppError, AppSettings, MultiAppConfig,
};

mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs};

fn cfg_path() -> PathBuf {
    get_app_config_dir().join("config.json")
}

struct ConfigDirEnvGuard {
    original: Option<OsString>,
}

impl ConfigDirEnvGuard {
    fn set(value: Option<&str>) -> Self {
        let original = std::env::var_os("CC_SWITCH_CONFIG_DIR");
        match value {
            Some(value) => unsafe { std::env::set_var("CC_SWITCH_CONFIG_DIR", value) },
            None => unsafe { std::env::remove_var("CC_SWITCH_CONFIG_DIR") },
        }
        Self { original }
    }
}

impl Drop for ConfigDirEnvGuard {
    fn drop(&mut self) {
        match self.original.as_ref() {
            Some(value) => unsafe { std::env::set_var("CC_SWITCH_CONFIG_DIR", value) },
            None => unsafe { std::env::remove_var("CC_SWITCH_CONFIG_DIR") },
        }
    }
}

#[test]
fn load_v1_config_returns_error_and_does_not_write() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let path = cfg_path();
    fs::create_dir_all(path.parent().unwrap()).expect("create cfg dir");

    // 最小 v1 形状：providers + current，且不含 version/apps/mcp
    let v1_json = r#"{"providers":{},"current":""}"#;
    fs::write(&path, v1_json).expect("seed v1 json");
    let before = fs::read_to_string(&path).expect("read before");

    let err = MultiAppConfig::load().expect_err("v1 should not be auto-migrated");
    match err {
        AppError::Localized { key, .. } => assert_eq!(key, "config.unsupported_v1"),
        other => panic!("expected Localized v1 error, got {other:?}"),
    }

    // 文件不应有任何变化，且不应生成 .bak
    let after = fs::read_to_string(&path).expect("read after");
    assert_eq!(before, after, "config.json should not be modified");
    let bak = home.join(".cc-switch").join("config.json.bak");
    assert!(!bak.exists(), ".bak should not be created on load error");
}

#[test]
fn load_v1_with_extra_version_still_treated_as_v1() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let path = cfg_path();
    std::fs::create_dir_all(path.parent().unwrap()).expect("create cfg dir");

    // 畸形：包含 providers + current + version，但没有 apps，应按 v1 处理
    let v1_like = r#"{"providers":{},"current":"","version":2}"#;
    std::fs::write(&path, v1_like).expect("seed v1-like json");
    let before = std::fs::read_to_string(&path).expect("read before");

    let err = MultiAppConfig::load().expect_err("v1-like should not be parsed as v2");
    match err {
        AppError::Localized { key, .. } => assert_eq!(key, "config.unsupported_v1"),
        other => panic!("expected Localized v1 error, got {other:?}"),
    }

    let after = std::fs::read_to_string(&path).expect("read after");
    assert_eq!(before, after, "config.json should not be modified");
    let bak = home.join(".cc-switch").join("config.json.bak");
    assert!(!bak.exists(), ".bak should not be created on v1-like error");
}

#[test]
fn load_invalid_json_returns_parse_error_and_does_not_write() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let path = cfg_path();
    fs::create_dir_all(path.parent().unwrap()).expect("create cfg dir");

    fs::write(&path, "{not json").expect("seed invalid json");
    let before = fs::read_to_string(&path).expect("read before");

    let err = MultiAppConfig::load().expect_err("invalid json should error");
    match err {
        AppError::Json { .. } => {}
        other => panic!("expected Json error, got {other:?}"),
    }

    let after = fs::read_to_string(&path).expect("read after");
    assert_eq!(before, after, "config.json should remain unchanged");
    let bak = home.join(".cc-switch").join("config.json.bak");
    assert!(!bak.exists(), ".bak should not be created on parse error");
}

#[test]
fn load_valid_v2_config_succeeds() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();
    let path = cfg_path();
    fs::create_dir_all(path.parent().unwrap()).expect("create cfg dir");

    // 使用默认结构序列化为 v2
    let default_cfg = MultiAppConfig::default();
    let json = serde_json::to_string_pretty(&default_cfg).expect("serialize default cfg");
    fs::write(&path, json).expect("write v2 json");

    let loaded = MultiAppConfig::load().expect("v2 should load successfully");
    assert_eq!(loaded.version, 2);
    assert!(loaded
        .get_manager(&cc_switch_lib::AppType::Claude)
        .is_some());
    assert!(loaded.get_manager(&cc_switch_lib::AppType::Codex).is_some());
    assert!(loaded
        .get_manager(&cc_switch_lib::AppType::OpenClaw)
        .is_some());
}

#[test]
fn default_config_contains_openclaw_prompt_root_and_manager() {
    let config = MultiAppConfig::default();

    assert!(config
        .get_manager(&cc_switch_lib::AppType::OpenClaw)
        .is_some());
    assert!(
        config.prompts.openclaw.prompts.is_empty(),
        "default OpenClaw prompt store should exist"
    );
}

#[test]
fn update_settings_persists_openclaw_override_dir() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let _config_dir = ConfigDirEnvGuard::set(None);

    let mut settings = AppSettings::default();
    settings.openclaw_config_dir = Some("~/custom-openclaw".to_string());
    update_settings(settings).expect("save settings with openclaw override");

    let path = home.join(".cc-switch").join("settings.json");
    let raw = fs::read_to_string(&path).expect("read settings.json");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("parse settings.json");
    assert_eq!(
        value
            .get("openclawConfigDir")
            .and_then(|entry| entry.as_str()),
        Some("~/custom-openclaw")
    );
}

#[test]
fn update_settings_uses_cc_switch_config_dir_override_for_settings_path() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let override_dir = home.join("custom-config-root");
    let _config_dir = ConfigDirEnvGuard::set(Some(override_dir.to_string_lossy().as_ref()));

    let mut settings = AppSettings::default();
    settings.openclaw_config_dir = Some("~/custom-openclaw".to_string());
    update_settings(settings).expect("save settings with config dir override");

    let override_settings = override_dir.join("settings.json");
    assert!(override_settings.exists(), "settings.json should be written to override dir");
    let raw = fs::read_to_string(&override_settings).expect("read overridden settings.json");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("parse overridden settings");
    assert_eq!(
        value
            .get("openclawConfigDir")
            .and_then(|entry| entry.as_str()),
        Some("~/custom-openclaw")
    );
    let default_settings = home.join(".cc-switch").join("settings.json");
    assert_ne!(override_settings, default_settings, "override path should differ from default path");
}
