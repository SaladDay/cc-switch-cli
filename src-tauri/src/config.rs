use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::AppError;

pub(crate) fn home_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(home) = crate::test_support::test_home_override() {
        return Some(home);
    }

    dirs::home_dir()
}

/// If `path` starts with `~` / `~/`, replace the tilde with the home directory.
/// Otherwise return the path unchanged.
fn expand_tilde(path: PathBuf) -> PathBuf {
    let lossy = path.to_string_lossy();
    // Bare "~"
    if lossy == "~" {
        return home_dir().unwrap_or(path);
    }
    // "~/" or "~\" prefix
    if lossy.starts_with("~/") || lossy.starts_with("~\\") {
        let home = home_dir();
        if let Some(home) = home {
            let rest = Path::new(&lossy[1..]);
            return home.join(rest);
        }
    }
    path
}

/// 获取 Claude Code 配置目录路径
///
/// Priority: `CLAUDE_CONFIG_DIR` env var > cc-switch settings override > `$HOME/.claude`
pub fn get_claude_config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        let dir = PathBuf::from(dir);
        if !dir.as_os_str().is_empty() && !dir.to_string_lossy().trim().is_empty() {
            return expand_tilde(dir);
        }
    }
    if let Some(custom) = crate::settings::get_claude_override_dir() {
        return custom;
    }

    home_dir().expect("无法获取用户主目录").join(".claude")
}

/// 默认 Claude MCP 配置文件路径 (~/.claude.json)
pub fn get_default_claude_mcp_path() -> PathBuf {
    home_dir().expect("无法获取用户主目录").join(".claude.json")
}

fn derive_mcp_path_from_override(dir: &Path) -> Option<PathBuf> {
    let file_name = dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())?
        .trim()
        .to_string();
    if file_name.is_empty() {
        return None;
    }
    let parent = dir.parent().unwrap_or_else(|| Path::new(""));
    Some(parent.join(format!("{file_name}.json")))
}

/// 获取 Claude MCP 配置文件路径，若设置了目录覆盖则与覆盖目录同级
pub fn get_claude_mcp_path() -> PathBuf {
    if let Some(custom_dir) = crate::settings::get_claude_override_dir() {
        if let Some(path) = derive_mcp_path_from_override(&custom_dir) {
            return path;
        }
    }
    get_default_claude_mcp_path()
}

/// 获取 Claude Code 主配置文件路径
pub fn get_claude_settings_path() -> PathBuf {
    let dir = get_claude_config_dir();
    let settings = dir.join("settings.json");
    if settings.exists() {
        return settings;
    }
    // 兼容旧版命名：若存在旧文件则继续使用
    let legacy = dir.join("claude.json");
    if legacy.exists() {
        return legacy;
    }
    // 默认新建：回落到标准文件名 settings.json（不再生成 claude.json）
    settings
}

/// 获取应用配置目录路径（默认 $HOME/.cc-switch-tui）
///
/// Priority: CC_SWITCH_TUI_CONFIG_DIR > CC_SWITCH_CONFIG_DIR (deprecated) > default
pub fn get_app_config_dir() -> PathBuf {
    // New env var — takes priority
    if let Some(custom) = env::var_os("CC_SWITCH_TUI_CONFIG_DIR") {
        let custom = PathBuf::from(custom);
        if !custom.to_string_lossy().trim().is_empty() {
            return expand_tilde(custom);
        }
    }

    // Legacy env var — still works but prints deprecation warning
    if let Some(custom) = env::var_os("CC_SWITCH_CONFIG_DIR") {
        let custom = PathBuf::from(custom);
        if custom.to_string_lossy().trim().is_empty() {
            return home_dir()
                .expect("无法获取用户主目录")
                .join(".cc-switch-tui");
        }
        eprintln!("deprecated: CC_SWITCH_CONFIG_DIR is set; use CC_SWITCH_TUI_CONFIG_DIR instead");
        return expand_tilde(custom);
    }

    // CLI mode: no app store override, always use default
    // if let Some(custom) = crate::app_store::get_app_config_dir_override() {
    //     return custom;
    // }

    let path = home_dir()
        .expect("无法获取用户主目录")
        .join(".cc-switch-tui");

    // 一次性迁移老旧 ~/.cc-switch/ → ~/.cc-switch-tui/
    // 嵌入 get_app_config_dir 内部，杜绝"新路径先于迁移创建"窗口
    // AtomicBool guard: 进程内只跑一次，避免测试并发和重复 stat 调用
    use std::sync::atomic::{AtomicBool, Ordering};
    static MIGRATED: AtomicBool = AtomicBool::new(false);
    if !MIGRATED.swap(true, Ordering::Relaxed) {
        migrate_legacy_config_dir_if_needed();
    }

    path
}

/// 获取应用配置文件路径
pub fn get_app_config_path() -> PathBuf {
    get_app_config_dir().join("config.json")
}

/// 清理供应商名称，确保文件名安全
pub fn sanitize_provider_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            _ => c,
        })
        .collect::<String>()
        .to_lowercase()
}

/// 获取供应商配置文件路径
pub fn get_provider_config_path(provider_id: &str, provider_name: Option<&str>) -> PathBuf {
    let base_name = provider_name
        .map(sanitize_provider_name)
        .unwrap_or_else(|| sanitize_provider_name(provider_id));

    get_claude_config_dir().join(format!("settings-{base_name}.json"))
}

/// 读取 JSON 配置文件
pub fn read_json_file<T: for<'a> Deserialize<'a>>(path: &Path) -> Result<T, AppError> {
    if !path.exists() {
        return Err(AppError::Config(format!("文件不存在: {}", path.display())));
    }

    let content = fs::read_to_string(path).map_err(|e| AppError::io(path, e))?;

    serde_json::from_str(&content).map_err(|e| AppError::json(path, e))
}

/// 写入 JSON 配置文件
pub fn write_json_file<T: Serialize>(path: &Path, data: &T) -> Result<(), AppError> {
    // 确保目录存在
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    let json =
        serde_json::to_string_pretty(data).map_err(|e| AppError::JsonSerialize { source: e })?;

    atomic_write(path, json.as_bytes())
}

/// 原子写入文本文件（用于 TOML/纯文本）
pub fn write_text_file(path: &Path, data: &str) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }
    atomic_write(path, data.as_bytes())
}

/// 原子写入：写入临时文件后 rename 替换，避免半写状态
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config("无效的路径".to_string()))?;
    let mut tmp = parent.to_path_buf();
    let file_name = path
        .file_name()
        .ok_or_else(|| AppError::Config("无效的文件名".to_string()))?
        .to_string_lossy()
        .to_string();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    tmp.push(format!("{file_name}.tmp.{ts}"));

    {
        let mut f = fs::File::create(&tmp).map_err(|e| AppError::io(&tmp, e))?;
        f.write_all(data).map_err(|e| AppError::io(&tmp, e))?;
        f.flush().map_err(|e| AppError::io(&tmp, e))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path) {
            let perm = meta.permissions().mode();
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(perm));
        }
    }

    #[cfg(windows)]
    {
        // Windows 上 rename 目标存在会失败，先移除再重命名（尽量接近原子性）
        if path.exists() {
            let _ = fs::remove_file(path);
        }
        fs::rename(&tmp, path).map_err(|e| AppError::IoContext {
            context: format!("原子替换失败: {} -> {}", tmp.display(), path.display()),
            source: e,
        })?;
    }

    #[cfg(not(windows))]
    {
        fs::rename(&tmp, path).map_err(|e| AppError::IoContext {
            context: format!("原子替换失败: {} -> {}", tmp.display(), path.display()),
            source: e,
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{lock_test_home_and_settings, set_test_home_override};
    use std::ffi::OsString;

    struct ConfigDirEnvGuard {
        key: String,
        original: Option<OsString>,
    }

    impl ConfigDirEnvGuard {
        fn new(key: &str, value: Option<&str>) -> Self {
            let original = env::var_os(key);
            match value {
                Some(v) => unsafe { env::set_var(key, v) },
                None => unsafe { env::remove_var(key) },
            }
            Self {
                key: key.to_string(),
                original,
            }
        }
    }

    impl Drop for ConfigDirEnvGuard {
        fn drop(&mut self) {
            match self.original.as_ref() {
                Some(value) => unsafe { env::set_var(&self.key, value) },
                None => unsafe { env::remove_var(&self.key) },
            }
        }
    }

    #[test]
    fn derive_mcp_path_from_override_preserves_folder_name() {
        let override_dir = PathBuf::from("/tmp/profile/.claude");
        let derived = derive_mcp_path_from_override(&override_dir)
            .expect("should derive path for nested dir");
        assert_eq!(derived, PathBuf::from("/tmp/profile/.claude.json"));
    }

    #[test]
    fn derive_mcp_path_from_override_handles_non_hidden_folder() {
        let override_dir = PathBuf::from("/data/claude-config");
        let derived = derive_mcp_path_from_override(&override_dir)
            .expect("should derive path for standard dir");
        assert_eq!(derived, PathBuf::from("/data/claude-config.json"));
    }

    #[test]
    fn derive_mcp_path_from_override_supports_relative_rootless_dir() {
        let override_dir = PathBuf::from("claude");
        let derived = derive_mcp_path_from_override(&override_dir)
            .expect("should derive path for single segment");
        assert_eq!(derived, PathBuf::from("claude.json"));
    }

    #[test]
    fn derive_mcp_path_from_root_like_dir_returns_none() {
        let override_dir = PathBuf::from("/");
        assert!(derive_mcp_path_from_override(&override_dir).is_none());
    }

    #[test]
    fn get_app_config_dir_defaults_to_home_dot_cc_switch() {
        let _guard = lock_test_home_and_settings();
        let _env = ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", None);
        set_test_home_override(Some(Path::new("/tmp/cc-switch-home-default")));

        assert_eq!(
            get_app_config_dir(),
            PathBuf::from("/tmp/cc-switch-home-default").join(".cc-switch-tui")
        );

        set_test_home_override(None);
    }

    #[test]
    fn get_app_config_dir_uses_env_override_when_set() {
        let _guard = lock_test_home_and_settings();
        let _env = ConfigDirEnvGuard::new(
            "CC_SWITCH_CONFIG_DIR",
            Some("/tmp/cc-switch-config-override"),
        );
        set_test_home_override(Some(Path::new("/tmp/cc-switch-home-ignored")));

        assert_eq!(
            get_app_config_dir(),
            PathBuf::from("/tmp/cc-switch-config-override")
        );

        set_test_home_override(None);
    }

    #[test]
    fn get_app_config_dir_ignores_blank_env_override() {
        let _guard = lock_test_home_and_settings();
        let _env = ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some("   "));
        set_test_home_override(Some(Path::new("/tmp/cc-switch-home-blank")));

        assert_eq!(
            get_app_config_dir(),
            PathBuf::from("/tmp/cc-switch-home-blank").join(".cc-switch-tui")
        );

        set_test_home_override(None);
    }

    #[test]
    fn get_app_config_dir_prefers_new_env_var() {
        let _guard = lock_test_home_and_settings();
        let _new =
            ConfigDirEnvGuard::new("CC_SWITCH_TUI_CONFIG_DIR", Some("/tmp/cc-switch-tui-new"));
        let _old = ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some("/tmp/cc-switch-old"));
        set_test_home_override(Some(Path::new("/tmp/cc-switch-home")));

        assert_eq!(
            get_app_config_dir(),
            PathBuf::from("/tmp/cc-switch-tui-new")
        );

        set_test_home_override(None);
    }

    #[test]
    fn get_app_config_dir_new_env_var_alone_works() {
        let _guard = lock_test_home_and_settings();
        let _new =
            ConfigDirEnvGuard::new("CC_SWITCH_TUI_CONFIG_DIR", Some("/tmp/cc-switch-tui-alone"));
        let _old = ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", None);
        set_test_home_override(Some(Path::new("/tmp/cc-switch-home")));

        assert_eq!(
            get_app_config_dir(),
            PathBuf::from("/tmp/cc-switch-tui-alone")
        );

        set_test_home_override(None);
    }

    #[test]
    fn get_claude_config_dir_respects_env_var() {
        let _guard = lock_test_home_and_settings();
        let _env = ConfigDirEnvGuard::new("CLAUDE_CONFIG_DIR", Some("/tmp/claude-custom"));
        set_test_home_override(Some(Path::new("/tmp/claude-home")));

        assert_eq!(get_claude_config_dir(), PathBuf::from("/tmp/claude-custom"));

        set_test_home_override(None);
    }

    #[test]
    fn get_claude_config_dir_ignores_blank_env_var() {
        let _guard = lock_test_home_and_settings();
        let original_settings = crate::settings::get_settings();
        let mut settings = original_settings.clone();
        settings.claude_config_dir = None;
        crate::settings::update_settings(settings).unwrap();
        let _env = ConfigDirEnvGuard::new("CLAUDE_CONFIG_DIR", Some("   "));
        set_test_home_override(Some(Path::new("/tmp/claude-home-blank")));

        assert_eq!(
            get_claude_config_dir(),
            PathBuf::from("/tmp/claude-home-blank").join(".claude")
        );

        crate::settings::update_settings(original_settings).unwrap();
        set_test_home_override(None);
    }

    #[test]
    fn get_claude_config_dir_falls_back_to_default_when_nothing_set() {
        let _guard = lock_test_home_and_settings();
        let original_settings = crate::settings::get_settings();
        let mut settings = original_settings.clone();
        settings.claude_config_dir = None;
        crate::settings::update_settings(settings).unwrap();
        let _env = ConfigDirEnvGuard::new("CLAUDE_CONFIG_DIR", None);
        set_test_home_override(Some(Path::new("/tmp/default-home")));

        assert_eq!(
            get_claude_config_dir(),
            PathBuf::from("/tmp/default-home").join(".claude")
        );

        crate::settings::update_settings(original_settings).unwrap();
        set_test_home_override(None);
    }

    #[test]
    fn get_claude_config_dir_env_overrides_settings() {
        let _guard = lock_test_home_and_settings();
        let original_settings = crate::settings::get_settings();
        let mut settings = original_settings.clone();
        settings.claude_config_dir = Some("/tmp/settings-override".to_string());
        crate::settings::update_settings(settings).unwrap();
        let _env = ConfigDirEnvGuard::new("CLAUDE_CONFIG_DIR", Some("/tmp/env-override"));
        set_test_home_override(Some(Path::new("/tmp/home")));

        assert_eq!(get_claude_config_dir(), PathBuf::from("/tmp/env-override"));

        crate::settings::update_settings(original_settings).unwrap();
        set_test_home_override(None);
    }

    #[test]
    fn get_claude_config_dir_blank_env_falls_back_to_settings() {
        let _guard = lock_test_home_and_settings();
        let original_settings = crate::settings::get_settings();
        let mut settings = original_settings.clone();
        settings.claude_config_dir = Some("/tmp/settings-override".to_string());
        crate::settings::update_settings(settings).unwrap();
        let _env = ConfigDirEnvGuard::new("CLAUDE_CONFIG_DIR", Some("   "));
        set_test_home_override(Some(Path::new("/tmp/home")));

        assert_eq!(
            get_claude_config_dir(),
            PathBuf::from("/tmp/settings-override")
        );

        crate::settings::update_settings(original_settings).unwrap();
        set_test_home_override(None);
    }

    // ──── migration tests ────

    #[test]
    fn migration_copies_config_json_and_db() {
        let _guard = lock_test_home_and_settings();
        let _tui = ConfigDirEnvGuard::new("CC_SWITCH_TUI_CONFIG_DIR", None);
        let _old = ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", None);

        let temp = tempfile::tempdir().expect("create temp dir");
        let home = temp.path();
        set_test_home_override(Some(home));

        let old_dir = home.join(".cc-switch");
        let new_dir = home.join(".cc-switch-tui");
        let marker = new_dir.join(".migrated-from-cc-switch");

        fs::create_dir_all(old_dir.join("skills")).unwrap();
        fs::write(old_dir.join("config.json"), r#"{"version":"1.0"}"#).unwrap();
        fs::write(old_dir.join("cc-switch.db"), "fake-db").unwrap();
        fs::write(old_dir.join("skills").join("my-skill.md"), "# Skill").unwrap();

        migrate_legacy_config_dir_if_needed();

        assert!(
            new_dir.join("config.json").exists(),
            "config.json should be copied"
        );
        assert!(
            new_dir.join("cc-switch.db").exists(),
            "cc-switch.db should be copied"
        );
        assert!(
            new_dir.join("skills").join("my-skill.md").exists(),
            "skills/ should be recursively copied"
        );
        assert!(marker.exists(), "migration marker should be written");

        set_test_home_override(None);
    }

    #[test]
    fn migration_skips_when_target_has_marker() {
        let _guard = lock_test_home_and_settings();
        let _tui = ConfigDirEnvGuard::new("CC_SWITCH_TUI_CONFIG_DIR", None);
        let _old = ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", None);

        let temp = tempfile::tempdir().expect("create temp dir");
        let home = temp.path();
        set_test_home_override(Some(home));

        let old_dir = home.join(".cc-switch");
        let new_dir = home.join(".cc-switch-tui");
        let marker = new_dir.join(".migrated-from-cc-switch");

        fs::create_dir_all(&old_dir).unwrap();
        fs::write(old_dir.join("config.json"), "v1").unwrap();
        fs::create_dir_all(&new_dir).unwrap();
        fs::write(&marker, "already migrated").unwrap();

        migrate_legacy_config_dir_if_needed();

        assert!(
            !new_dir.join("config.json").exists(),
            "should not copy when marker exists"
        );

        set_test_home_override(None);
    }

    #[test]
    fn migration_skips_when_env_override_set() {
        let _guard = lock_test_home_and_settings();
        let _tui = ConfigDirEnvGuard::new("CC_SWITCH_TUI_CONFIG_DIR", None);
        let _old =
            ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some("/tmp/custom-override-test"));

        let temp = tempfile::tempdir().expect("create temp dir");
        let home = temp.path();
        set_test_home_override(Some(home));

        let old_dir = home.join(".cc-switch");
        let new_dir = home.join(".cc-switch-tui");

        fs::create_dir_all(&old_dir).unwrap();
        fs::write(old_dir.join("config.json"), "v1").unwrap();

        migrate_legacy_config_dir_if_needed();

        assert!(
            !new_dir.exists(),
            "should not create target dir when env override set"
        );

        set_test_home_override(None);
    }

    #[test]
    fn migration_is_idempotent() {
        let _guard = lock_test_home_and_settings();
        let _tui = ConfigDirEnvGuard::new("CC_SWITCH_TUI_CONFIG_DIR", None);
        let _old = ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", None);

        let temp = tempfile::tempdir().expect("create temp dir");
        let home = temp.path();
        set_test_home_override(Some(home));

        let old_dir = home.join(".cc-switch");
        let new_dir = home.join(".cc-switch-tui");
        let marker = new_dir.join(".migrated-from-cc-switch");

        fs::create_dir_all(&old_dir).unwrap();
        fs::write(old_dir.join("config.json"), "v1").unwrap();

        // First run
        migrate_legacy_config_dir_if_needed();
        assert!(new_dir.join("config.json").exists());
        let mtime_after_first = fs::metadata(&marker).unwrap().modified().unwrap();

        // Second run — should be a no-op
        migrate_legacy_config_dir_if_needed();
        let mtime_after_second = fs::metadata(&marker).unwrap().modified().unwrap();
        assert_eq!(
            mtime_after_first, mtime_after_second,
            "second migration should not overwrite marker"
        );

        set_test_home_override(None);
    }

    #[test]
    fn migration_preserves_old_directory() {
        let _guard = lock_test_home_and_settings();
        let _tui = ConfigDirEnvGuard::new("CC_SWITCH_TUI_CONFIG_DIR", None);
        let _old = ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", None);

        let temp = tempfile::tempdir().expect("create temp dir");
        let home = temp.path();
        set_test_home_override(Some(home));

        let old_dir = home.join(".cc-switch");

        fs::create_dir_all(&old_dir).unwrap();
        fs::write(old_dir.join("config.json"), "v1").unwrap();

        migrate_legacy_config_dir_if_needed();

        assert!(old_dir.exists(), "old directory must be preserved");
        assert!(
            old_dir.join("config.json").exists(),
            "old files must be preserved"
        );

        set_test_home_override(None);
    }

    #[test]
    fn migration_copies_only_3_most_recent_backups() {
        let _guard = lock_test_home_and_settings();
        let _tui = ConfigDirEnvGuard::new("CC_SWITCH_TUI_CONFIG_DIR", None);
        let _old = ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", None);

        let temp = tempfile::tempdir().expect("create temp dir");
        let home = temp.path();
        set_test_home_override(Some(home));

        let old_dir = home.join(".cc-switch");
        let backup_dir = old_dir.join("backups");
        fs::create_dir_all(&backup_dir).unwrap();

        // Create 5 backup files with increasing mtime
        for i in 1..=5 {
            let path = backup_dir.join(format!("backup-{}.json", i));
            fs::write(&path, format!("backup {}", i)).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        migrate_legacy_config_dir_if_needed();

        let new_backup_dir = home.join(".cc-switch-tui").join("backups");
        let copied: Vec<_> = fs::read_dir(&new_backup_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();

        assert_eq!(
            copied.len(),
            3,
            "only 3 most recent backups should be copied"
        );
        assert!(
            copied.contains(&"backup-3.json".to_string()),
            "third most recent should be copied"
        );
        assert!(
            copied.contains(&"backup-4.json".to_string()),
            "second most recent should be copied"
        );
        assert!(
            copied.contains(&"backup-5.json".to_string()),
            "most recent should be copied"
        );

        set_test_home_override(None);
    }
}

/// 复制文件
pub fn copy_file(from: &Path, to: &Path) -> Result<(), AppError> {
    fs::copy(from, to).map_err(|e| AppError::IoContext {
        context: format!("复制文件失败 ({} -> {})", from.display(), to.display()),
        source: e,
    })?;
    Ok(())
}

/// 删除文件
pub fn delete_file(path: &Path) -> Result<(), AppError> {
    if path.exists() {
        fs::remove_file(path).map_err(|e| AppError::io(path, e))?;
    }
    Ok(())
}

/// 递归复制目录内容（跳过软链接）
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// 复制备份目录中最近 3 个（按修改时间）条目
fn copy_recent_backups(src: &Path, dst: &Path, limit: usize) -> std::io::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(src)?
        .filter_map(|e| e.ok())
        .filter(|e| !e.file_type().map_or(true, |t| t.is_symlink()))
        .collect();
    entries.sort_by_key(|e| {
        e.metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH)
    });
    entries.reverse();
    entries.truncate(limit);

    fs::create_dir_all(dst)?;
    for entry in entries {
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if entry.file_type().map_or(false, |t| t.is_dir()) {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// 提取迁移前置检查逻辑，返回 (old_dir, new_dir, marker) 若条件满足，否则 None。
fn migration_guard() -> Option<(PathBuf, PathBuf, PathBuf)> {
    if env::var_os("CC_SWITCH_TUI_CONFIG_DIR").is_some()
        || env::var_os("CC_SWITCH_CONFIG_DIR").is_some()
    {
        return None;
    }

    let home = home_dir()?;
    let old_dir = home.join(".cc-switch");
    let new_dir = home.join(".cc-switch-tui");
    let marker = new_dir.join(".migrated-from-cc-switch");

    if !old_dir.exists() || !old_dir.is_dir() {
        return None;
    }
    if marker.exists() {
        return None;
    }
    let has_contents = fs::read_dir(&old_dir).map_or(false, |mut rd| rd.next().is_some());
    if !has_contents {
        return None;
    }

    Some((old_dir, new_dir, marker))
}

/// 检查是否存在尚未迁移的旧版配置目录。
///
/// 返回 true 表示 ~/.cc-switch/ 存在且未迁移，应提示用户确认。
pub fn check_legacy_config_dir_migration_needed() -> bool {
    migration_guard().is_some()
}

/// 用户拒绝迁移：写入标记文件以永不再次提示。
///
/// 错误仅记录到 stderr，绝不阻塞启动。
pub fn skip_legacy_config_dir_migration() {
    let (_, new_dir, marker) = match migration_guard() {
        Some(v) => v,
        None => return,
    };

    if let Err(e) = std::fs::create_dir_all(&new_dir)
        .and_then(|_| std::fs::write(&marker, "User declined migration"))
    {
        eprintln!(
            "cc-switch: failed to write skip-migration marker at {}: {e}",
            marker.display()
        );
    }
}

/// 首次运行时自动将旧版 ~/.cc-switch/ 迁移到 ~/.cc-switch-tui/
///
/// 仅在以下条件全部满足时执行：
/// - 未设置 CC_SWITCH_TUI_CONFIG_DIR 或 CC_SWITCH_CONFIG_DIR 环境变量
/// - 旧目录 ~/.cc-switch/ 存在且非空
/// - 目标目录不存在 .migrated-from-cc-switch 标记文件
///
/// 非破坏性：旧目录完好保留。错误仅记录警告，绝不阻塞启动。
pub fn migrate_legacy_config_dir_if_needed() {
    let (old_dir, new_dir, marker) = match migration_guard() {
        Some(v) => v,
        None => return,
    };

    // Perform migration (errors caught, never propagate)
    if let Err(e) = try_migrate(&old_dir, &new_dir, &marker) {
        eprintln!(
            "cc-switch: legacy config migration failed: {e} (old data preserved at {})",
            old_dir.display()
        );
    }
}

fn try_migrate(old_dir: &Path, new_dir: &Path, marker: &Path) -> std::io::Result<()> {
    fs::create_dir_all(new_dir)?;

    for entry in fs::read_dir(old_dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let src_path = entry.path();
        let file_name = entry.file_name();
        let dst_path = new_dir.join(&file_name);

        if file_name == "backups" && file_type.is_dir() {
            copy_recent_backups(&src_path, &dst_path, 3)?;
        } else if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }

    // Write marker file to prevent re-migration
    fs::write(
        marker,
        format!(
            "Migrated from {} on {}",
            old_dir.display(),
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        ),
    )?;

    eprintln!(
        "cc-switch: config migrated from {} to {} (old directory preserved)",
        old_dir.display(),
        new_dir.display()
    );
    Ok(())
}

/// 检查 Claude Code 配置状态
#[derive(Serialize, Deserialize)]
pub struct ConfigStatus {
    pub exists: bool,
    pub path: String,
}

/// 获取 Claude Code 配置状态
pub fn get_claude_config_status() -> ConfigStatus {
    let path = get_claude_settings_path();
    ConfigStatus {
        exists: path.exists(),
        path: path.to_string_lossy().to_string(),
    }
}
