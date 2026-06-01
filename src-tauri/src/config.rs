use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::cli::i18n::texts;
use crate::error::AppError;

pub(crate) fn home_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(home) = crate::test_support::test_home_override() {
        return Some(home);
    }

    dirs::home_dir()
}

/// 获取 Claude Code 配置目录路径
///
/// Priority: `CLAUDE_CONFIG_DIR` env var > cc-switch settings override > `$HOME/.claude`
pub fn get_claude_config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        let dir = PathBuf::from(dir);
        if !dir.as_os_str().is_empty() && !dir.to_string_lossy().trim().is_empty() {
            return dir;
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

/// 获取应用配置目录路径（默认 $HOME/.cc-switch，可由 CC_SWITCH_CONFIG_DIR 覆盖）
pub fn get_app_config_dir() -> PathBuf {
    if let Some(custom) = env::var_os("CC_SWITCH_CONFIG_DIR") {
        let custom = PathBuf::from(custom);
        if custom.to_string_lossy().trim().is_empty() {
            return home_dir().expect("无法获取用户主目录").join(".cc-switch");
        }
        return custom;
    }

    // CLI mode: no app store override, always use default
    // if let Some(custom) = crate::app_store::get_app_config_dir_override() {
    //     return custom;
    // }

    home_dir().expect("无法获取用户主目录").join(".cc-switch")
}

/// 校验 CC_SWITCH_CONFIG_DIR 是否为安全的应用专属目录
///
/// 拒绝系统关键目录（如 `/`、`/etc`、`/usr` 等），防止下游权限操作破坏系统。
/// 未设置环境变量时默认路径 `~/.cc-switch` 始终安全，直接放行。
pub fn validate_config_dir() -> Result<(), AppError> {
    let path = get_app_config_dir();

    // 检查原始路径和 canonicalize 后的路径（macOS 下 /etc -> /private/etc）
    let resolved = path.canonicalize().unwrap_or_else(|_| path.clone());

    if is_system_dir(&path) || is_system_dir(&resolved) {
        return Err(AppError::InvalidInput(texts::config_dir_is_system_dir(
            &path.display().to_string(),
            &resolved.display().to_string(),
        )));
    }

    Ok(())
}

/// 判断路径是否为系统关键目录（不应被应用修改权限）
fn is_system_dir(path: &Path) -> bool {
    // 根目录
    if path == Path::new("/") {
        return true;
    }

    // 一级系统目录
    #[cfg(unix)]
    {
        const SYSTEM_DIRS: &[&str] = &[
            "/bin", "/boot", "/dev", "/etc", "/home", "/lib", "/lib32", "/lib64", "/opt", "/proc",
            "/root", "/run", "/sbin", "/sys", "/tmp", "/usr", "/var",
        ];
        if SYSTEM_DIRS.iter().any(|&sys| path == Path::new(sys)) {
            return true;
        }
    }

    // macOS 特有（含 /private/* 变体，/etc、/tmp、/var 在 macOS 上是这些的符号链接）
    #[cfg(target_os = "macos")]
    {
        const MACOS_SYSTEM_DIRS: &[&str] = &[
            "/Applications",
            "/Library",
            "/System",
            "/Volumes",
            "/private",
            "/private/etc",
            "/private/tmp",
            "/private/var",
        ];
        if MACOS_SYSTEM_DIRS.iter().any(|&sys| path == Path::new(sys)) {
            return true;
        }
    }

    // Windows: 盘符根目录（如 C:\）
    #[cfg(windows)]
    {
        // Should do some more verifications here
        false
    }

    false
}

/// 获取应用配置文件路径
pub fn get_app_config_path() -> PathBuf {
    get_app_config_dir().join("config.json")
}

/// 将目录权限收紧为仅所有者可访问（Unix: 0o700）
#[cfg(unix)]
pub(crate) fn restrict_dir_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::metadata(path)?;
    if !meta.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path is not a directory",
        ));
    }
    let mut perms = meta.permissions();
    if perms.mode() & 0o777 != 0o700 {
        perms.set_mode(0o700);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn restrict_dir_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// 将文件权限收紧为仅所有者可读写（Unix: 0o600）
#[cfg(unix)]
pub(crate) fn restrict_file_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::metadata(path)?;
    if !meta.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path is not a regular file",
        ));
    }
    let mut perms = meta.permissions();
    if perms.mode() & 0o777 != 0o600 {
        perms.set_mode(0o600);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn restrict_file_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// 检查配置目录、数据库文件和备份目录的权限是否安全（Unix only）
///
/// 返回不安全的路径列表：`(路径, 当前权限, 期望权限)`
#[cfg(unix)]
pub fn check_permissions() -> Vec<(PathBuf, u32, u32)> {
    use std::os::unix::fs::PermissionsExt;
    let mut issues = Vec::new();
    let config_dir = get_app_config_dir();
    let db_path = config_dir.join("cc-switch.db");
    let backup_dir = config_dir.join("backups");

    if config_dir.exists() {
        if let Ok(meta) = fs::metadata(&config_dir) {
            let mode = meta.permissions().mode() & 0o777;
            if mode != 0o700 {
                issues.push((config_dir.clone(), mode, 0o700));
            }
        }
    }

    if db_path.exists() {
        if let Ok(meta) = fs::metadata(&db_path) {
            let mode = meta.permissions().mode() & 0o777;
            if mode != 0o600 {
                issues.push((db_path, mode, 0o600));
            }
        }
    }

    if backup_dir.exists() {
        if let Ok(meta) = fs::metadata(&backup_dir) {
            let mode = meta.permissions().mode() & 0o777;
            if mode != 0o700 {
                issues.push((backup_dir, mode, 0o700));
            }
        }
    }

    issues
}

#[cfg(not(unix))]
pub fn check_permissions() -> Vec<(PathBuf, u32, u32)> {
    Vec::new()
}

trait PermissionPrompter {
    fn confirm_custom_dir(&mut self, path: &Path) -> Result<bool, AppError>;
    fn confirm_fix(&mut self) -> Result<bool, AppError>;
}

struct InquirePermissionPrompter;

impl PermissionPrompter for InquirePermissionPrompter {
    fn confirm_custom_dir(&mut self, _path: &Path) -> Result<bool, AppError> {
        inquire::Confirm::new(texts::config_permissions_confirm_custom_dir())
            .with_default(false)
            .prompt()
            .map_err(|e| AppError::Message(format!("Prompt failed: {}", e)))
    }

    fn confirm_fix(&mut self) -> Result<bool, AppError> {
        inquire::Confirm::new(texts::config_permissions_fix_prompt())
            .with_default(true)
            .prompt()
            .map_err(|e| AppError::Message(format!("Prompt failed: {}", e)))
    }
}

fn prompt_fix_permissions_interactive(
    issues: &[(PathBuf, u32, u32)],
    custom_dir: Option<PathBuf>,
    prompter: &mut dyn PermissionPrompter,
) -> Result<(), AppError> {
    eprintln!("{}", texts::config_permissions_insecure_header());
    for (path, current, expected) in issues {
        eprintln!(
            "{}",
            texts::config_permissions_detail(&path.display().to_string(), *current, *expected,)
        );
    }

    if let Some(custom_path) = custom_dir {
        if !custom_path.as_os_str().is_empty() {
            eprintln!(
                "{}",
                texts::config_permissions_custom_dir_notice(&custom_path.display().to_string())
            );
            if !prompter.confirm_custom_dir(&custom_path)? {
                eprintln!("{}", texts::config_permissions_custom_dir_skipped());
                return Ok(());
            }
        }
    }

    if prompter.confirm_fix()? {
        for (path, _, _) in issues {
            if path.is_dir() {
                restrict_dir_permissions(path).map_err(|e| AppError::io(path, e))?;
            } else {
                restrict_file_permissions(path).map_err(|e| AppError::io(path, e))?;
            }
        }
        eprintln!("{}", texts::config_permissions_fixed());
    } else {
        eprintln!("{}", texts::config_permissions_fix_warn_interactive());
    }

    Ok(())
}

fn write_permissions_noninteractive_warning<W: Write>(
    mut output: W,
    issues: &[(PathBuf, u32, u32)],
) -> std::io::Result<()> {
    writeln!(
        output,
        "{}",
        texts::config_permissions_fix_warn_noninteractive()
    )?;
    for (path, current, expected) in issues {
        writeln!(
            output,
            "{}",
            texts::config_permissions_detail(&path.display().to_string(), *current, *expected,)
        )?;
    }
    Ok(())
}

/// 访问数据库前检查权限，若不安全则提示用户是否修复
///
/// - 交互终端：使用 inquire 提示用户，确认后修复，拒绝则警告
/// - 非交互终端（Docker/管道）：仅打印警告到 stderr
pub fn prompt_fix_permissions() -> Result<(), AppError> {
    let issues = check_permissions();
    if issues.is_empty() {
        return Ok(());
    }

    let is_terminal = !cfg!(test)
        && std::io::IsTerminal::is_terminal(&std::io::stdin())
        && std::io::IsTerminal::is_terminal(&std::io::stdout())
        && std::io::IsTerminal::is_terminal(&std::io::stderr());

    if is_terminal {
        let custom_dir = env::var_os("CC_SWITCH_CONFIG_DIR").map(PathBuf::from);
        let mut prompter = InquirePermissionPrompter;
        prompt_fix_permissions_interactive(&issues, custom_dir, &mut prompter)?;
    } else {
        let stderr = std::io::stderr();
        let mut stderr = stderr.lock();
        write_permissions_noninteractive_warning(&mut stderr, &issues)
            .map_err(|e| AppError::Message(format!("Failed to write permission warning: {e}")))?;
    }

    Ok(())
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

    struct SettingsGuard {
        original: crate::settings::AppSettings,
    }

    impl SettingsGuard {
        fn with_claude_config_dir(dir: Option<&str>) -> Self {
            let original = crate::settings::get_settings();
            let mut settings = original.clone();
            settings.claude_config_dir = dir.map(str::to_string);
            crate::settings::update_settings(settings).unwrap();
            Self { original }
        }
    }

    impl Drop for SettingsGuard {
        fn drop(&mut self) {
            let _ = crate::settings::update_settings(self.original.clone());
        }
    }

    struct FakePermissionPrompter {
        custom_dir_response: bool,
        fix_response: bool,
        custom_dir_calls: usize,
        fix_calls: usize,
    }

    impl FakePermissionPrompter {
        fn new(custom_dir_response: bool, fix_response: bool) -> Self {
            Self {
                custom_dir_response,
                fix_response,
                custom_dir_calls: 0,
                fix_calls: 0,
            }
        }
    }

    impl PermissionPrompter for FakePermissionPrompter {
        fn confirm_custom_dir(&mut self, _path: &Path) -> Result<bool, AppError> {
            self.custom_dir_calls += 1;
            Ok(self.custom_dir_response)
        }

        fn confirm_fix(&mut self) -> Result<bool, AppError> {
            self.fix_calls += 1;
            Ok(self.fix_response)
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
            PathBuf::from("/tmp/cc-switch-home-default").join(".cc-switch")
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
            PathBuf::from("/tmp/cc-switch-home-blank").join(".cc-switch")
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
        let _settings = SettingsGuard::with_claude_config_dir(None);
        let _env = ConfigDirEnvGuard::new("CLAUDE_CONFIG_DIR", Some("   "));
        set_test_home_override(Some(Path::new("/tmp/claude-home-blank")));

        assert_eq!(
            get_claude_config_dir(),
            PathBuf::from("/tmp/claude-home-blank").join(".claude")
        );

        set_test_home_override(None);
    }

    #[test]
    fn get_claude_config_dir_falls_back_to_default_when_nothing_set() {
        let _guard = lock_test_home_and_settings();
        let _settings = SettingsGuard::with_claude_config_dir(None);
        let _env = ConfigDirEnvGuard::new("CLAUDE_CONFIG_DIR", None);
        set_test_home_override(Some(Path::new("/tmp/default-home")));

        assert_eq!(
            get_claude_config_dir(),
            PathBuf::from("/tmp/default-home").join(".claude")
        );

        set_test_home_override(None);
    }

    #[test]
    fn get_claude_config_dir_env_overrides_settings() {
        let _guard = lock_test_home_and_settings();
        let _settings = SettingsGuard::with_claude_config_dir(Some("/tmp/settings-override"));
        let _env = ConfigDirEnvGuard::new("CLAUDE_CONFIG_DIR", Some("/tmp/env-override"));
        set_test_home_override(Some(Path::new("/tmp/home")));

        assert_eq!(get_claude_config_dir(), PathBuf::from("/tmp/env-override"));

        set_test_home_override(None);
    }

    #[test]
    fn get_claude_config_dir_blank_env_falls_back_to_settings() {
        let _guard = lock_test_home_and_settings();
        let _settings = SettingsGuard::with_claude_config_dir(Some("/tmp/settings-override"));
        let _env = ConfigDirEnvGuard::new("CLAUDE_CONFIG_DIR", Some("   "));
        set_test_home_override(Some(Path::new("/tmp/home")));

        assert_eq!(
            get_claude_config_dir(),
            PathBuf::from("/tmp/settings-override")
        );

        set_test_home_override(None);
    }

    #[test]
    fn validate_config_dir_ok_when_not_set() {
        let _guard = lock_test_home_and_settings();
        let _env = ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", None);
        assert!(validate_config_dir().is_ok());
    }

    #[test]
    fn validate_config_dir_ok_for_normal_path() {
        let _guard = lock_test_home_and_settings();
        let _env = ConfigDirEnvGuard::new(
            "CC_SWITCH_CONFIG_DIR",
            Some("/tmp/cc-switch-config-override"),
        );
        assert!(validate_config_dir().is_ok());
    }

    #[test]
    fn validate_config_dir_rejects_root() {
        let _guard = lock_test_home_and_settings();
        let _env = ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some("/"));
        assert!(validate_config_dir().is_err());
    }

    #[test]
    fn validate_config_dir_rejects_etc() {
        let _guard = lock_test_home_and_settings();
        let _env = ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some("/etc"));
        assert!(validate_config_dir().is_err());
    }

    #[test]
    fn validate_config_dir_rejects_usr() {
        let _guard = lock_test_home_and_settings();
        let _env = ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some("/usr"));
        assert!(validate_config_dir().is_err());
    }

    #[test]
    fn validate_config_dir_rejects_tmp() {
        let _guard = lock_test_home_and_settings();
        let _env = ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some("/tmp"));
        assert!(validate_config_dir().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn check_permissions_returns_empty_for_secure_permissions() {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let _guard = lock_test_home_and_settings();
        let temp = tempfile::tempdir().expect("create temp dir");
        let _env =
            ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some(temp.path().to_str().unwrap()));

        // Ensure dir has 0o700
        std::fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))
            .expect("set dir perms");

        // Create a db file with 0o600
        let db_path = temp.path().join("cc-switch.db");
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .mode(0o600)
            .open(&db_path)
            .expect("create db file");

        let issues = check_permissions();
        assert!(issues.is_empty(), "expected no issues, got: {:?}", issues);
    }

    #[cfg(unix)]
    #[test]
    fn check_permissions_detects_insecure_dir() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = lock_test_home_and_settings();
        let temp = tempfile::tempdir().expect("create temp dir");
        let _env =
            ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some(temp.path().to_str().unwrap()));

        // Set dir to permissive
        std::fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o755))
            .expect("set dir perms");

        let issues = check_permissions();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].0, temp.path());
        assert_eq!(issues[0].1, 0o755);
        assert_eq!(issues[0].2, 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn check_permissions_detects_insecure_db_file() {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let _guard = lock_test_home_and_settings();
        let temp = tempfile::tempdir().expect("create temp dir");
        let _env =
            ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some(temp.path().to_str().unwrap()));

        // Ensure dir has 0o700 so only the db file is flagged
        std::fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))
            .expect("set dir perms");

        // Create db file with permissive mode
        let db_path = temp.path().join("cc-switch.db");
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .mode(0o644)
            .open(&db_path)
            .expect("create db file");

        let issues = check_permissions();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].0, db_path);
        assert_eq!(issues[0].1, 0o644);
        assert_eq!(issues[0].2, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn check_permissions_detects_both_insecure() {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let _guard = lock_test_home_and_settings();
        let temp = tempfile::tempdir().expect("create temp dir");
        let _env =
            ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some(temp.path().to_str().unwrap()));

        // Set dir to permissive
        std::fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o755))
            .expect("set dir perms");

        // Create db file with permissive mode
        let db_path = temp.path().join("cc-switch.db");
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .mode(0o644)
            .open(&db_path)
            .expect("create db file");

        let issues = check_permissions();
        assert_eq!(issues.len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn check_permissions_detects_insecure_backup_dir() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = lock_test_home_and_settings();
        let temp = tempfile::tempdir().expect("create temp dir");
        let _env =
            ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some(temp.path().to_str().unwrap()));

        std::fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))
            .expect("set config dir perms");
        let backup_dir = temp.path().join("backups");
        std::fs::create_dir(&backup_dir).expect("create backup dir");
        std::fs::set_permissions(&backup_dir, fs::Permissions::from_mode(0o755))
            .expect("set backup dir perms");

        let issues = check_permissions();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].0, backup_dir);
        assert_eq!(issues[0].1, 0o755);
        assert_eq!(issues[0].2, 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn prompt_fix_permissions_does_not_fix_in_test_build() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = lock_test_home_and_settings();
        let temp = tempfile::tempdir().expect("create temp dir");
        let _env =
            ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some(temp.path().to_str().unwrap()));

        std::fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o755))
            .expect("set dir perms");

        prompt_fix_permissions().expect("test build should only warn");

        // Permissions should remain unchanged because cfg!(test) skips the fix logic
        let mode = std::fs::metadata(temp.path())
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o755,
            "test build should not modify directory permissions"
        );
    }

    #[cfg(unix)]
    #[test]
    fn interactive_permission_prompt_fixes_permissions_when_confirmed() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("create temp dir");
        std::fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o755))
            .expect("set dir perms");
        let issues = vec![(temp.path().to_path_buf(), 0o755, 0o700)];
        let mut prompter = FakePermissionPrompter::new(true, true);

        prompt_fix_permissions_interactive(&issues, None, &mut prompter)
            .expect("interactive fix should succeed");

        let mode = std::fs::metadata(temp.path())
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
        assert_eq!(prompter.custom_dir_calls, 0);
        assert_eq!(prompter.fix_calls, 1);
    }

    #[cfg(unix)]
    #[test]
    fn interactive_permission_prompt_fixes_file_permissions_when_confirmed() {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let temp = tempfile::tempdir().expect("create temp dir");
        let db_path = temp.path().join("cc-switch.db");
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .mode(0o644)
            .open(&db_path)
            .expect("create db file");
        let issues = vec![(db_path.clone(), 0o644, 0o600)];
        let mut prompter = FakePermissionPrompter::new(true, true);

        prompt_fix_permissions_interactive(&issues, None, &mut prompter)
            .expect("interactive file fix should succeed");

        let mode = std::fs::metadata(&db_path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(prompter.custom_dir_calls, 0);
        assert_eq!(prompter.fix_calls, 1);
    }

    #[cfg(unix)]
    #[test]
    fn interactive_permission_prompt_leaves_permissions_when_fix_declined() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("create temp dir");
        std::fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o755))
            .expect("set dir perms");
        let issues = vec![(temp.path().to_path_buf(), 0o755, 0o700)];
        let mut prompter = FakePermissionPrompter::new(true, false);

        prompt_fix_permissions_interactive(&issues, None, &mut prompter)
            .expect("interactive prompt should succeed");

        let mode = std::fs::metadata(temp.path())
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o755);
        assert_eq!(prompter.custom_dir_calls, 0);
        assert_eq!(prompter.fix_calls, 1);
    }

    #[cfg(unix)]
    #[test]
    fn interactive_permission_prompt_skips_custom_dir_when_not_confirmed() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("create temp dir");
        std::fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o755))
            .expect("set dir perms");
        let custom_dir = temp.path().to_path_buf();
        let issues = vec![(custom_dir.clone(), 0o755, 0o700)];
        let mut prompter = FakePermissionPrompter::new(false, true);

        prompt_fix_permissions_interactive(&issues, Some(custom_dir), &mut prompter)
            .expect("interactive prompt should succeed");

        let mode = std::fs::metadata(temp.path())
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o755);
        assert_eq!(prompter.custom_dir_calls, 1);
        assert_eq!(prompter.fix_calls, 0);
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

/// 检查 Claude Code 配置状态
#[derive(Serialize, Deserialize)]
#[allow(dead_code)]
pub struct ConfigStatus {
    pub exists: bool,
    pub path: String,
}

/// 获取 Claude Code 配置状态
#[allow(dead_code)]
pub fn get_claude_config_status() -> ConfigStatus {
    let path = get_claude_settings_path();
    ConfigStatus {
        exists: path.exists(),
        path: path.to_string_lossy().to_string(),
    }
}
