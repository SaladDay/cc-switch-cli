use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use crate::error::AppError;
use crate::i18n::texts;

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
    let resolved = resolve_config_dir_without_following_user_symlinks(&path)?;

    if is_system_dir(&path) || is_system_dir(&resolved) {
        return Err(AppError::InvalidInput(texts::config_dir_is_system_dir(
            &path.display().to_string(),
            &resolved.display().to_string(),
        )));
    }

    Ok(())
}

pub(crate) fn resolve_existing_or_new_child_path(path: &Path) -> Result<PathBuf, AppError> {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(AppError::InvalidInput(format!(
            "配置目录路径不能包含父目录组件: {}",
            path.display()
        )));
    }

    match path.canonicalize() {
        Ok(resolved) => {
            if is_system_dir(path) || is_system_dir(&resolved) {
                return Err(AppError::InvalidInput(texts::config_dir_is_system_dir(
                    &path.display().to_string(),
                    &resolved.display().to_string(),
                )));
            }
            Ok(resolved)
        }
        Err(original_err) => {
            let file_name = path.file_name().ok_or_else(|| {
                AppError::InvalidInput(texts::config_dir_invalid_last_component(
                    &path.display().to_string(),
                ))
            })?;
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            let parent_resolved =
                parent
                    .canonicalize()
                    .map_err(|parent_err| AppError::IoContext {
                        context: texts::config_dir_only_final_component_may_be_missing(
                            &path.display().to_string(),
                        ),
                        source: parent_err,
                    })?;

            let resolved = parent_resolved.join(file_name);
            if is_system_dir(&resolved) {
                return Err(AppError::InvalidInput(texts::config_dir_is_system_dir(
                    &path.display().to_string(),
                    &resolved.display().to_string(),
                )));
            }

            log::debug!(
                "Config dir does not exist yet, resolved parent and rebuilt path: {} -> {} ({original_err})",
                path.display(),
                resolved.display()
            );
            Ok(resolved)
        }
    }
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
        return false;
    }

    false
}

/// 获取应用配置文件路径
pub fn get_app_config_path() -> PathBuf {
    get_app_config_dir().join("config.json")
}

/// 将文件权限收紧为仅所有者可读写（Unix: 0o600）
#[cfg(unix)]
pub(crate) fn restrict_file_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path is a symlink",
        ));
    }
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
    let json =
        serde_json::to_string_pretty(data).map_err(|e| AppError::JsonSerialize { source: e })?;

    atomic_write(path, json.as_bytes())
}

/// 原子写入文本文件（用于 TOML/纯文本）
pub fn write_text_file(path: &Path, data: &str) -> Result<(), AppError> {
    atomic_write(path, data.as_bytes())
}

/// 原子写入：写入临时文件后 rename 替换，避免半写状态
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<(), AppError> {
    let managed_write_path = resolve_managed_storage_path(path)?;
    let should_restrict_file = should_restrict_sensitive_config_file(path)?;
    let write_path = managed_write_path.unwrap_or_else(|| path.to_path_buf());
    if path != write_path && should_restrict_file {
        debug_assert!(write_path.is_absolute());
    }

    if path.starts_with(get_app_config_dir()) {
        create_managed_config_parent_dirs(path)?;
    } else if let Some(parent) = write_path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    let parent = write_path
        .parent()
        .ok_or_else(|| AppError::Config("无效的路径".to_string()))?;
    let mut tmp = parent.to_path_buf();
    let file_name = write_path
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
        #[cfg(unix)]
        let mut f = if should_restrict_file {
            use std::os::unix::fs::OpenOptionsExt;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)
                .map_err(|e| AppError::io(&tmp, e))?
        } else {
            fs::File::create(&tmp).map_err(|e| AppError::io(&tmp, e))?
        };

        #[cfg(not(unix))]
        let mut f = fs::File::create(&tmp).map_err(|e| AppError::io(&tmp, e))?;

        f.write_all(data).map_err(|e| AppError::io(&tmp, e))?;
        f.flush().map_err(|e| AppError::io(&tmp, e))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if should_restrict_file {
            restrict_file_permissions(&tmp).map_err(|e| AppError::io(&tmp, e))?;
        } else if let Ok(meta) = fs::metadata(&write_path) {
            let perm = meta.permissions().mode();
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(perm));
        }
    }

    #[cfg(windows)]
    {
        // Windows 上 rename 目标存在会失败，先移除再重命名（尽量接近原子性）
        if write_path.exists() {
            let _ = fs::remove_file(&write_path);
        }
        fs::rename(&tmp, &write_path).map_err(|e| AppError::IoContext {
            context: format!(
                "原子替换失败: {} -> {}",
                tmp.display(),
                write_path.display()
            ),
            source: e,
        })?;
    }

    #[cfg(not(windows))]
    {
        fs::rename(&tmp, &write_path).map_err(|e| AppError::IoContext {
            context: format!(
                "原子替换失败: {} -> {}",
                tmp.display(),
                write_path.display()
            ),
            source: e,
        })?;
    }
    if should_restrict_file {
        restrict_file_permissions(&write_path).map_err(|e| AppError::io(&write_path, e))?;
    }
    Ok(())
}

fn should_restrict_sensitive_config_file(path: &Path) -> Result<bool, AppError> {
    #[cfg(unix)]
    {
        if !is_sensitive_config_file(path) {
            return Ok(false);
        }

        is_managed_sensitive_config_path(path)
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(false)
    }
}

#[cfg(unix)]
fn is_sensitive_config_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "db" | "json" | "sql"))
        .unwrap_or(false)
}

pub(crate) fn resolve_managed_storage_path(path: &Path) -> Result<Option<PathBuf>, AppError> {
    let raw_root = get_app_config_dir();
    if !path.starts_with(&raw_root) {
        return Ok(None);
    }

    let resolved_root = resolve_config_dir_without_following_user_symlinks(&raw_root)?;
    let suffix = path
        .strip_prefix(&raw_root)
        .unwrap_or_else(|_| Path::new(""));
    validate_managed_storage_suffix(suffix, path)?;
    Ok(Some(resolved_root.join(suffix)))
}

fn validate_managed_storage_suffix(suffix: &Path, original_path: &Path) -> Result<(), AppError> {
    if suffix
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(AppError::InvalidInput(format!(
            "受管配置路径不能包含父目录组件: {}",
            original_path.display()
        )));
    }

    Ok(())
}

pub(crate) fn resolve_config_dir_without_following_user_symlinks(
    path: &Path,
) -> Result<PathBuf, AppError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|e| AppError::io(".", e))?
            .join(path)
    };
    let mut current = PathBuf::new();
    let components = absolute.components().collect::<Vec<_>>();

    for (idx, component) in components.iter().enumerate() {
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => continue,
            Component::ParentDir => {
                return Err(AppError::InvalidInput(format!(
                    "配置目录路径不能包含父目录组件: {}",
                    path.display()
                )));
            }
            Component::Normal(part) => {
                current.push(part);
                match fs::symlink_metadata(&current) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        if is_allowed_platform_config_symlink(&current) {
                            continue;
                        }
                        return Err(AppError::InvalidInput(format!(
                            "配置目录路径不能包含符号链接: {}",
                            current.display()
                        )));
                    }
                    Ok(_) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                        if idx + 1 != components.len() {
                            return Err(AppError::IoContext {
                                context: texts::config_dir_only_final_component_may_be_missing(
                                    &path.display().to_string(),
                                ),
                                source: err,
                            });
                        }
                        break;
                    }
                    Err(err) => return Err(AppError::io(&current, err)),
                }
            }
        }
    }

    resolve_existing_or_new_child_path(&current)
}

pub(crate) fn create_managed_config_parent_dirs(path: &Path) -> Result<(), AppError> {
    if let Some(resolved) = resolve_managed_storage_path(path)? {
        if let Some(parent) = resolved.parent() {
            #[cfg(unix)]
            {
                let config_root =
                    resolve_config_dir_without_following_user_symlinks(&get_app_config_dir())?;
                create_secure_config_dir_all_no_symlink(&config_root, parent)?;
            }

            #[cfg(not(unix))]
            {
                fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
            }
        }
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }
    Ok(())
}

pub(crate) fn create_managed_config_dir_all(path: &Path) -> Result<(), AppError> {
    if let Some(resolved) = resolve_managed_storage_path(path)? {
        #[cfg(unix)]
        {
            let config_root =
                resolve_config_dir_without_following_user_symlinks(&get_app_config_dir())?;
            create_secure_config_dir_all_no_symlink(&config_root, &resolved)?;
        }

        #[cfg(not(unix))]
        {
            fs::create_dir_all(&resolved).map_err(|e| AppError::io(&resolved, e))?;
        }

        return Ok(());
    }

    fs::create_dir_all(path).map_err(|e| AppError::io(path, e))?;
    Ok(())
}

#[cfg(unix)]
fn create_secure_config_dir_all_no_symlink(
    config_root: &Path,
    path: &Path,
) -> Result<(), AppError> {
    use std::os::unix::fs::DirBuilderExt;

    let mut current = PathBuf::new();
    let mut managed_component = false;
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => continue,
            Component::ParentDir => unreachable!("paths are resolved before secure creation"),
            Component::Normal(part) => {
                current.push(part);
                if current == config_root {
                    managed_component = true;
                }
                match fs::symlink_metadata(&current) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        if is_allowed_platform_config_symlink(&current) {
                            continue;
                        }
                        return Err(AppError::InvalidInput(format!(
                            "配置目录路径不能包含符号链接: {}",
                            current.display()
                        )));
                    }
                    Ok(meta) if meta.is_dir() => {
                        if managed_component {
                            validate_existing_managed_config_dir(&current)?;
                        }
                    }
                    Ok(_) => {
                        return Err(AppError::InvalidInput(format!(
                            "配置目录路径组件不是目录: {}",
                            current.display()
                        )));
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                        if !managed_component {
                            return Err(AppError::IoContext {
                                context: texts::config_dir_only_final_component_may_be_missing(
                                    &path.display().to_string(),
                                ),
                                source: err,
                            });
                        }
                        match fs::DirBuilder::new().mode(0o700).create(&current) {
                            Ok(()) => {}
                            Err(create_err)
                                if create_err.kind() == std::io::ErrorKind::AlreadyExists =>
                            {
                                validate_existing_managed_config_dir(&current)?;
                            }
                            Err(create_err) => return Err(AppError::io(&current, create_err)),
                        }
                    }
                    Err(err) => return Err(AppError::io(&current, err)),
                }
            }
        }
    }

    Ok(())
}

#[cfg(unix)]
fn validate_existing_managed_config_dir(path: &Path) -> Result<(), AppError> {
    use std::os::unix::fs::PermissionsExt;

    let meta = fs::symlink_metadata(path).map_err(|e| AppError::io(path, e))?;
    if meta.file_type().is_symlink() {
        return Err(AppError::InvalidInput(format!(
            "受管配置目录不能是符号链接: {}",
            path.display()
        )));
    }
    if !meta.is_dir() {
        return Err(AppError::InvalidInput(format!(
            "受管配置路径不是目录: {}",
            path.display()
        )));
    }

    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o022 != 0 {
        return Err(AppError::InvalidInput(format!(
            "受管配置目录不能允许组或其他用户写入: {} ({mode:04o})",
            path.display()
        )));
    }

    Ok(())
}

#[cfg(unix)]
fn is_allowed_platform_config_symlink(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        matches!(path.to_str(), Some("/tmp") | Some("/var") | Some("/etc"))
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        false
    }
}

#[cfg(not(unix))]
fn is_allowed_platform_config_symlink(_path: &Path) -> bool {
    false
}

#[cfg(unix)]
fn is_managed_sensitive_config_path(path: &Path) -> Result<bool, AppError> {
    let config_dir = normalized_absolute_path(&get_app_config_dir())?;
    let path = normalized_absolute_path(path)?;
    if !path.starts_with(&config_dir) {
        return Ok(false);
    }

    let Ok(relative) = path.strip_prefix(&config_dir) else {
        return Ok(false);
    };

    let components = relative.components().collect::<Vec<_>>();
    if components.len() == 1 {
        return Ok(true);
    }

    Ok(matches!(
        components.first(),
        Some(Component::Normal(name)) if *name == "backups"
    ))
}

#[cfg(unix)]
fn normalized_absolute_path(path: &Path) -> Result<PathBuf, AppError> {
    let base = if path.is_absolute() {
        PathBuf::new()
    } else {
        env::current_dir().map_err(|e| AppError::io(".", e))?
    };

    let mut normalized = PathBuf::new();
    for component in base.join(path).components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(AppError::InvalidInput(format!(
                        "路径包含无效的父目录组件: {}",
                        path.display()
                    )));
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }

    Ok(normalized)
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

    #[test]
    fn validate_config_dir_rejects_parent_dir_components_even_when_parent_resolves() {
        let _guard = lock_test_home_and_settings();
        let temp = tempfile::tempdir().expect("create temp dir");
        std::fs::create_dir(temp.path().join("child")).expect("create child dir");
        let config_dir = temp.path().join("child").join("..").join("cc-switch");
        let _env = ConfigDirEnvGuard::new(
            "CC_SWITCH_CONFIG_DIR",
            Some(config_dir.to_str().expect("utf8 temp path")),
        );

        assert!(
            validate_config_dir().is_err(),
            "config dir should reject parent components instead of normalizing to the parent"
        );
        assert!(
            resolve_config_dir_without_following_user_symlinks(&config_dir).is_err(),
            "managed config root resolution must reject parent components"
        );
    }

    #[test]
    fn validate_config_dir_rejects_parent_dir_components_when_parent_does_not_resolve() {
        let _guard = lock_test_home_and_settings();
        let _env =
            ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some("/tmp/cc-switch-new-child/.."));

        assert!(validate_config_dir().is_err());
    }

    #[test]
    fn validate_config_dir_rejects_parent_dir_components_when_resolved_to_system_dir() {
        let _guard = lock_test_home_and_settings();
        let _env = ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some("/usr/bin/.."));

        assert!(
            validate_config_dir().is_err(),
            "resolved config dir should reject the system parent directory"
        );
    }

    #[cfg(unix)]
    #[test]
    fn validate_config_dir_rejects_symlink_parent_without_touching_target() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let _guard = lock_test_home_and_settings();
        let temp = tempfile::tempdir().expect("create temp dir");
        let external_parent = temp.path().join("external");
        let external_config = external_parent.join(".cc-switch");
        let link_parent = temp.path().join("link");
        std::fs::create_dir(&external_parent).expect("create external parent");
        std::fs::create_dir(&external_config).expect("create external config");
        std::fs::set_permissions(&external_config, fs::Permissions::from_mode(0o755))
            .expect("set insecure external config perms");
        symlink(&external_parent, &link_parent).expect("create symlink parent");

        let raw_config = link_parent.join(".cc-switch");
        let _env =
            ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some(raw_config.to_str().unwrap()));

        assert!(
            validate_config_dir().is_err(),
            "validation should reject the symlink parent component"
        );
        let mode = std::fs::metadata(&external_config)
            .expect("metadata external config")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o755, "validation must not chmod the symlink target");
    }

    #[cfg(unix)]
    #[test]
    fn write_json_file_restricts_sensitive_file_without_changing_existing_config_dir() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = lock_test_home_and_settings();
        let temp = tempfile::tempdir().expect("create temp dir");
        let _env =
            ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some(temp.path().to_str().unwrap()));
        std::fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o755))
            .expect("set config dir perms");

        let path = temp.path().join("config.json");
        write_json_file(&path, &serde_json::json!({ "token": "secret" }))
            .expect("write sensitive json");

        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        let dir_mode = std::fs::metadata(temp.path())
            .expect("metadata config dir")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(dir_mode, 0o755);
    }

    #[cfg(unix)]
    #[test]
    fn existing_config_dir_validation_preserves_directory_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("create temp dir");
        let dir = temp.path().join("cc-switch");
        std::fs::create_dir(&dir).expect("create dir");
        std::fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).expect("set dir perms");

        validate_existing_managed_config_dir(&dir).expect("existing directory should be accepted");

        let mode = std::fs::metadata(&dir)
            .expect("metadata dir")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o755);
    }

    #[cfg(unix)]
    #[test]
    fn existing_config_dir_validation_rejects_symlink() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("create temp dir");
        let target = temp.path().join("target");
        let link = temp.path().join("link");
        std::fs::create_dir(&target).expect("create target");
        symlink(&target, &link).expect("create symlink");

        let err = validate_existing_managed_config_dir(&link)
            .expect_err("existing symlink must not be accepted");
        assert!(err.to_string().contains("符号链接"));
    }

    #[cfg(unix)]
    #[test]
    fn write_json_file_rejects_other_user_writable_config_dir_without_changing_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = lock_test_home_and_settings();
        let temp = tempfile::tempdir().expect("create temp dir");
        let _env =
            ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some(temp.path().to_str().unwrap()));
        std::fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o777))
            .expect("set config dir perms");

        let path = temp.path().join("config.json");
        let err = write_json_file(&path, &serde_json::json!({ "token": "secret" }))
            .expect_err("other-user-writable config dir must be rejected");

        assert!(err.to_string().contains("不能允许组或其他用户写入"));
        assert!(!path.exists(), "rejected write must not create the target");
        let mode = std::fs::metadata(temp.path())
            .expect("metadata config dir")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o777, "validation must not chmod the directory");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn write_json_file_restricts_sensitive_files_under_macos_tmp_alias() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = lock_test_home_and_settings();
        let temp = tempfile::tempdir_in("/tmp").expect("create /tmp temp dir");
        let config_dir = temp.path().join("cc-switch");
        std::fs::create_dir(&config_dir).expect("create config dir");
        std::fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o700))
            .expect("set config dir perms");
        let _env =
            ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some(config_dir.to_str().unwrap()));

        let path = get_app_config_dir().join("settings.json");
        write_json_file(&path, &serde_json::json!({ "token": "secret" }))
            .expect("write sensitive json");

        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn write_json_file_rejects_sensitive_file_under_symlinked_config_subdir() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let _guard = lock_test_home_and_settings();
        let root = tempfile::tempdir().expect("create temp dir");
        let config_dir = root.path().join("cc-switch");
        let external_dir = root.path().join("external-backups");
        std::fs::create_dir(&config_dir).expect("create config dir");
        std::fs::create_dir(&external_dir).expect("create external dir");
        std::fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o700))
            .expect("set config dir perms");
        std::fs::set_permissions(&external_dir, fs::Permissions::from_mode(0o755))
            .expect("set external dir perms");
        symlink(&external_dir, config_dir.join("backups")).expect("create backups symlink");

        let _env =
            ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some(config_dir.to_str().unwrap()));
        let err = write_json_file(
            &config_dir.join("backups").join("secret.json"),
            &serde_json::json!({ "token": "secret" }),
        )
        .expect_err("sensitive writes must reject symlinked config subdirs");

        assert!(
            err.to_string().contains("符号链接") || err.to_string().contains("symlink"),
            "unexpected error: {err}"
        );
        let external_mode = std::fs::metadata(&external_dir)
            .expect("metadata external dir")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            external_mode, 0o755,
            "symlink target permissions must not be modified"
        );
        assert!(
            !external_dir.join("secret.json").exists(),
            "write should not follow symlink and create the sensitive file outside config dir"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_json_file_rejects_unresolved_parent_dir_components_without_chmodding_parent() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = lock_test_home_and_settings();
        let root = tempfile::tempdir().expect("create temp dir");
        std::fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755))
            .expect("set root perms");
        let config_dir = root.path().join("child").join("..");
        let _env =
            ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some(config_dir.to_str().unwrap()));

        write_json_file(
            &get_app_config_dir().join("settings.json"),
            &serde_json::json!({ "token": "secret" }),
        )
        .expect_err("unresolved parent components should be rejected before chmod");

        let root_mode = std::fs::metadata(root.path())
            .expect("metadata root")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            root_mode, 0o755,
            "invalid config dir writes must not chmod the resolved parent"
        );
        assert!(
            !root.path().join("settings.json").exists(),
            "invalid config dir write should not create the normalized target"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_json_file_rejects_symlink_parent_even_when_followed_by_dotdot() {
        use std::os::unix::fs::symlink;

        let _guard = lock_test_home_and_settings();
        let root = tempfile::tempdir().expect("create temp dir");
        let real_parent = root.path().join("real-parent");
        let external_parent = root.path().join("external-parent");
        std::fs::create_dir(&real_parent).expect("create real parent");
        std::fs::create_dir(&external_parent).expect("create external parent");
        symlink(&external_parent, real_parent.join("link")).expect("create symlink parent");

        let config_dir = real_parent.join("link").join("..").join("cc-switch");
        let _env =
            ConfigDirEnvGuard::new("CC_SWITCH_CONFIG_DIR", Some(config_dir.to_str().unwrap()));

        let err = write_json_file(
            &get_app_config_dir().join("settings.json"),
            &serde_json::json!({ "token": "secret" }),
        )
        .expect_err("symlink component should be rejected before lexical dotdot collapse");

        assert!(
            err.to_string().contains("符号链接") || err.to_string().contains("symlink"),
            "unexpected error: {err}"
        );
        assert!(
            !real_parent.join("cc-switch/settings.json").exists(),
            "rejected write must not create the normalized target"
        );
        assert!(
            !external_parent.join("cc-switch/settings.json").exists(),
            "write must not follow the symlinked parent from the raw path"
        );
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
