use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const DEFAULT_KIMI_CONFIG_DIR: &str = ".kimi-code";
pub const KIMI_HOME_ENV: &str = "KIMI_CODE_HOME";
pub const KIMI_CREDENTIALS_DIR: &str = "credentials";
pub const KIMI_DEFAULT_CREDENTIAL_FILE: &str = "kimi-code.json";
pub const KIMI_CONFIG_FILE: &str = "config.toml";
pub const KIMI_TUI_FILE: &str = "tui.toml";
pub const KIMI_PROFILES_DIR_NAME: &str = "kimi_profiles";
pub const KIMI_ACTIVE_PROFILE_FILE: &str = "kimi_active_profile";

/// Kimi Code native 登录认证文件结构 (~/.kimi-code/credentials/kimi-code.json)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KimiNativeCredentials {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KimiProfileInfo {
    pub name: String,
    pub path: PathBuf,
    pub is_active: bool,
    pub has_credentials: bool,
    pub has_config: bool,
}

/// 解析 Kimi Code 根目录路径（遵循 KIMI_CODE_HOME 环境变量，默认 ~/.kimi-code）
pub fn get_kimi_config_dir() -> PathBuf {
    if let Some(env_val) = std::env::var_os(KIMI_HOME_ENV) {
        if !env_val.is_empty() {
            return PathBuf::from(env_val);
        }
    }
    dirs::home_dir()
        .map(|p| p.join(DEFAULT_KIMI_CONFIG_DIR))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_KIMI_CONFIG_DIR))
}

/// 获取 cc-switch 管理的 Kimi 配置 profiles 存储目录
pub fn get_kimi_profiles_dir() -> PathBuf {
    crate::config::get_app_config_dir().join(KIMI_PROFILES_DIR_NAME)
}

/// 获取当前激活的 Profile 名称（如果记录过）
pub fn get_active_profile_name() -> Option<String> {
    let path = crate::config::get_app_config_dir().join(KIMI_ACTIVE_PROFILE_FILE);
    if path.exists() {
        fs::read_to_string(path).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    } else {
        None
    }
}

/// 设置当前激活的 Profile 名称记录
pub fn set_active_profile_name(name: Option<&str>) -> Result<()> {
    let path = crate::config::get_app_config_dir().join(KIMI_ACTIVE_PROFILE_FILE);
    if let Some(n) = name {
        write_file_atomic(&path, n.trim(), 0o644)?;
    } else if path.exists() {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

/// 读取当前 native credentials
pub fn read_native_credentials() -> Result<Option<KimiNativeCredentials>> {
    let cred_path = get_kimi_config_dir()
        .join(KIMI_CREDENTIALS_DIR)
        .join(KIMI_DEFAULT_CREDENTIAL_FILE);

    if !cred_path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&cred_path)
        .with_context(|| format!("读取 Kimi 凭据文件失败: {}", cred_path.display()))?;

    let parsed: KimiNativeCredentials = serde_json::from_str(&content)
        .with_context(|| format!("解析 Kimi 凭据文件失败: {}", cred_path.display()))?;

    Ok(Some(parsed))
}

/// 原子写入 native credentials
pub fn write_native_credentials(credentials: &KimiNativeCredentials) -> Result<()> {
    let dir = get_kimi_config_dir().join(KIMI_CREDENTIALS_DIR);
    fs::create_dir_all(&dir)
        .with_context(|| format!("创建 Kimi 凭据目录失败: {}", dir.display()))?;

    let cred_path = dir.join(KIMI_DEFAULT_CREDENTIAL_FILE);
    let content = serde_json::to_string_pretty(credentials)
        .context("序列化 Kimi 凭据失败")?;

    write_file_atomic(&cred_path, &content, 0o600)?;
    Ok(())
}

/// 同步账号认证信息至 native ~/.kimi-code
pub fn sync_kimi_account_to_native(
    access_token: &str,
    refresh_token: &str,
    expires_in: i64,
    expires_at_sec: i64,
) -> Result<()> {
    let creds = KimiNativeCredentials {
        access_token: access_token.to_string(),
        refresh_token: refresh_token.to_string(),
        expires_in: Some(expires_in),
        token_type: Some("Bearer".to_string()),
        scope: None,
        expires_at: Some(expires_at_sec),
    };

    write_native_credentials(&creds)
}

/// 清除 native credentials
pub fn clear_native_credentials() -> Result<()> {
    let cred_path = get_kimi_config_dir()
        .join(KIMI_CREDENTIALS_DIR)
        .join(KIMI_DEFAULT_CREDENTIAL_FILE);

    if cred_path.exists() {
        fs::remove_file(&cred_path)
            .with_context(|| format!("删除 Kimi 凭据失败: {}", cred_path.display()))?;
    }
    Ok(())
}

/// 列出所有已保存的 Kimi 配置 Profiles
pub fn list_profiles() -> Result<Vec<KimiProfileInfo>> {
    let profiles_dir = get_kimi_profiles_dir();
    if !profiles_dir.exists() {
        return Ok(Vec::new());
    }

    let active_name = get_active_profile_name();
    let mut profiles = Vec::new();

    for entry in fs::read_dir(&profiles_dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();
            let has_credentials = path.join(KIMI_CREDENTIALS_DIR).join(KIMI_DEFAULT_CREDENTIAL_FILE).exists();
            let has_config = path.join(KIMI_CONFIG_FILE).exists();
            let is_active = active_name.as_deref() == Some(&name);

            profiles.push(KimiProfileInfo {
                name,
                path,
                is_active,
                has_credentials,
                has_config,
            });
        }
    }

    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(profiles)
}

/// 保存当前活动的 ~/.kimi-code 配置到指定名称的 Profile
pub fn save_profile(name: &str) -> Result<PathBuf> {
    let name = validate_profile_name(name)?;
    let src_dir = get_kimi_config_dir();
    let target_dir = get_kimi_profiles_dir().join(name);

    fs::create_dir_all(&target_dir)
        .with_context(|| format!("创建 Profile 目录失败: {}", target_dir.display()))?;

    // 复制 config.toml
    let src_config = src_dir.join(KIMI_CONFIG_FILE);
    if src_config.exists() {
        fs::copy(&src_config, target_dir.join(KIMI_CONFIG_FILE))?;
    }

    // 复制 tui.toml
    let src_tui = src_dir.join(KIMI_TUI_FILE);
    if src_tui.exists() {
        fs::copy(&src_tui, target_dir.join(KIMI_TUI_FILE))?;
    }

    // 复制 credentials
    let src_creds_dir = src_dir.join(KIMI_CREDENTIALS_DIR);
    if src_creds_dir.exists() {
        let target_creds_dir = target_dir.join(KIMI_CREDENTIALS_DIR);
        fs::create_dir_all(&target_creds_dir)?;
        let src_cred_file = src_creds_dir.join(KIMI_DEFAULT_CREDENTIAL_FILE);
        if src_cred_file.exists() {
            fs::copy(&src_cred_file, target_creds_dir.join(KIMI_DEFAULT_CREDENTIAL_FILE))?;
        }
    }

    set_active_profile_name(Some(name))?;
    Ok(target_dir)
}

/// 切换激活指定的 Profile（将该 Profile 写入当前 ~/.kimi-code 根目录）
pub fn switch_profile(name: &str) -> Result<()> {
    let name = validate_profile_name(name)?;
    let profile_dir = get_kimi_profiles_dir().join(name);
    if !profile_dir.exists() {
        anyhow::bail!("Profile '{}' 不存在", name);
    }

    let target_dir = get_kimi_config_dir();
    fs::create_dir_all(&target_dir)
        .with_context(|| format!("创建目标目录失败: {}", target_dir.display()))?;

    // 恢复 config.toml
    let p_config = profile_dir.join(KIMI_CONFIG_FILE);
    let target_config = target_dir.join(KIMI_CONFIG_FILE);
    if p_config.exists() {
        fs::copy(&p_config, &target_config)?;
    } else if target_config.exists() {
        let _ = fs::remove_file(&target_config);
    }

    // 恢复 tui.toml
    let p_tui = profile_dir.join(KIMI_TUI_FILE);
    let target_tui = target_dir.join(KIMI_TUI_FILE);
    if p_tui.exists() {
        fs::copy(&p_tui, &target_tui)?;
    } else if target_tui.exists() {
        let _ = fs::remove_file(&target_tui);
    }

    // 恢复 credentials
    let p_creds_file = profile_dir.join(KIMI_CREDENTIALS_DIR).join(KIMI_DEFAULT_CREDENTIAL_FILE);
    let target_creds_dir = target_dir.join(KIMI_CREDENTIALS_DIR);
    let target_creds_file = target_creds_dir.join(KIMI_DEFAULT_CREDENTIAL_FILE);

    if p_creds_file.exists() {
        fs::create_dir_all(&target_creds_dir)?;
        fs::copy(&p_creds_file, &target_creds_file)?;
    } else if target_creds_file.exists() {
        let _ = fs::remove_file(&target_creds_file);
    }

    set_active_profile_name(Some(name))?;
    Ok(())
}

/// 删除指定的 Profile
pub fn remove_profile(name: &str) -> Result<()> {
    let name = validate_profile_name(name)?;
    let profile_dir = get_kimi_profiles_dir().join(name);
    if profile_dir.exists() {
        fs::remove_dir_all(&profile_dir)
            .with_context(|| format!("删除 Profile 目录失败: {}", profile_dir.display()))?;
    }

    if get_active_profile_name().as_deref() == Some(name) {
        let _ = set_active_profile_name(None);
    }

    Ok(())
}

fn validate_profile_name(name: &str) -> Result<&str> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Profile 名称不能为空");
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains("..") {
        anyhow::bail!("Profile 名称包含非法字符: {}", name);
    }
    Ok(trimmed)
}

fn write_file_atomic(path: &Path, content: &str, #[allow(unused_variables)] mode: u32) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("无效的路径: {}", path.display()))?;

    fs::create_dir_all(parent)
        .with_context(|| format!("创建目录失败: {}", parent.display()))?;

    let filename = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("无效的文件名: {}", path.display()))?
        .to_string_lossy();

    let temp_path = parent.join(format!(
        ".{filename}.tmp.{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temp_path)
        .with_context(|| format!("创建临时文件失败: {}", temp_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
    }

    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    drop(file);

    fs::rename(&temp_path, path).with_context(|| {
        let _ = fs::remove_file(&temp_path);
        format!("重命名临时文件到目标文件失败: {}", path.display())
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_and_read_native_credentials() {
        let _lock = crate::test_support::lock_test_home_and_settings();
        let temp = tempfile::tempdir().unwrap();
        let old_env = std::env::var_os(KIMI_HOME_ENV);
        std::env::set_var(KIMI_HOME_ENV, temp.path());

        let creds = KimiNativeCredentials {
            access_token: "test_at".to_string(),
            refresh_token: "test_rt".to_string(),
            expires_in: Some(3600),
            token_type: Some("Bearer".to_string()),
            scope: None,
            expires_at: Some(1720000000),
        };

        write_native_credentials(&creds).unwrap();

        let read_back = read_native_credentials().unwrap();
        assert_eq!(read_back, Some(creds));

        clear_native_credentials().unwrap();
        assert_eq!(read_native_credentials().unwrap(), None);

        if let Some(val) = old_env {
            std::env::set_var(KIMI_HOME_ENV, val);
        } else {
            std::env::remove_var(KIMI_HOME_ENV);
        }
    }

    #[test]
    fn test_profile_save_and_switch() {
        let _lock = crate::test_support::lock_test_home_and_settings();
        let temp_home = tempfile::tempdir().unwrap();
        let temp_app = tempfile::tempdir().unwrap();

        let old_home_env = std::env::var_os(KIMI_HOME_ENV);
        let old_app_env = std::env::var_os("CC_SWITCH_CONFIG_DIR");

        std::env::set_var(KIMI_HOME_ENV, temp_home.path());
        std::env::set_var("CC_SWITCH_CONFIG_DIR", temp_app.path());

        // 写入初始环境
        let config_file = temp_home.path().join(KIMI_CONFIG_FILE);
        fs::write(&config_file, "default_model = 'kimi-k2'").unwrap();

        let creds = KimiNativeCredentials {
            access_token: "work_token".to_string(),
            refresh_token: "work_rt".to_string(),
            expires_in: Some(3600),
            token_type: Some("Bearer".to_string()),
            scope: None,
            expires_at: None,
        };
        write_native_credentials(&creds).unwrap();

        // 保存为 work profile
        save_profile("work").unwrap();

        let profiles = list_profiles().unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "work");
        assert!(profiles[0].is_active);
        assert!(profiles[0].has_credentials);
        assert!(profiles[0].has_config);

        // 修改当前环境为 personal
        fs::write(&config_file, "default_model = 'kimi-k1.5'").unwrap();
        let personal_creds = KimiNativeCredentials {
            access_token: "personal_token".to_string(),
            refresh_token: "personal_rt".to_string(),
            expires_in: Some(3600),
            token_type: Some("Bearer".to_string()),
            scope: None,
            expires_at: None,
        };
        write_native_credentials(&personal_creds).unwrap();

        // 保存为 personal profile
        save_profile("personal").unwrap();
        assert_eq!(list_profiles().unwrap().len(), 2);
        assert_eq!(get_active_profile_name().as_deref(), Some("personal"));

        // 切换回 work profile
        switch_profile("work").unwrap();
        assert_eq!(get_active_profile_name().as_deref(), Some("work"));

        // 验证当前配置和凭据已被还原为 work
        let current_config = fs::read_to_string(&config_file).unwrap();
        assert_eq!(current_config, "default_model = 'kimi-k2'");
        let current_creds = read_native_credentials().unwrap().unwrap();
        assert_eq!(current_creds.access_token, "work_token");

        // 删除 personal profile
        remove_profile("personal").unwrap();
        assert_eq!(list_profiles().unwrap().len(), 1);

        if let Some(val) = old_home_env {
            std::env::set_var(KIMI_HOME_ENV, val);
        } else {
            std::env::remove_var(KIMI_HOME_ENV);
        }
        if let Some(val) = old_app_env {
            std::env::set_var("CC_SWITCH_CONFIG_DIR", val);
        } else {
            std::env::remove_var("CC_SWITCH_CONFIG_DIR");
        }
    }
}
