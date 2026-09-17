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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

/// 解析 Kimi Code 根目录路径（遵循 KIMI_CODE_HOME 环境变量，默认 ~/.kimi-code）
pub fn get_kimi_config_dir() -> PathBuf {
    if let Some(env_val) = std::env::var_os(KIMI_HOME_ENV) {
        if !env_val.is_empty() {
            return PathBuf::from(env_val);
        }
    }
    #[cfg(test)]
    {
        // 单元测试未显式设置 KIMI_CODE_HOME 时，绝不能回退到宿主真实目录，防止测试副作用篡改真实凭据
        std::env::temp_dir().join("cc-switch-kimi-test-isolated")
    }
    #[cfg(not(test))]
    {
        dirs::home_dir()
            .map(|p| p.join(DEFAULT_KIMI_CONFIG_DIR))
            .unwrap_or_else(|| PathBuf::from(DEFAULT_KIMI_CONFIG_DIR))
    }
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

/// 读取指定 profile 的 credentials
pub fn read_profile_credentials(profile_name: &str) -> Result<Option<KimiNativeCredentials>> {
    let cred_path = get_kimi_profiles_dir()
        .join(profile_name)
        .join(KIMI_CREDENTIALS_DIR)
        .join(KIMI_DEFAULT_CREDENTIAL_FILE);

    if !cred_path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&cred_path)
        .with_context(|| format!("读取 Profile 凭据文件失败: {}", cred_path.display()))?;

    let parsed: KimiNativeCredentials = serde_json::from_str(&content)
        .with_context(|| format!("解析 Profile 凭据文件失败: {}", cred_path.display()))?;

    Ok(Some(parsed))
}

/// 确保获取有效的 access_token（若已过期，自动尝试 refresh 并写回对应文件）
pub async fn get_valid_access_token(
    creds: &mut KimiNativeCredentials,
    save_path: Option<&Path>,
) -> Result<String> {
    let now = chrono::Utc::now().timestamp();
    let expired = creds.expires_at.map(|exp| exp <= now + 30).unwrap_or(false);

    if !expired && !creds.access_token.is_empty() {
        return Ok(creds.access_token.clone());
    }

    if creds.refresh_token.is_empty() {
        if !creds.access_token.is_empty() {
            return Ok(creds.access_token.clone());
        }
        anyhow::bail!("缺少 refresh_token，无法刷新");
    }

    let refreshed = crate::proxy::providers::kimi_oauth_auth::KimiOAuthManager::refresh_token_raw(&creds.refresh_token)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if let Some(new_at) = refreshed.access_token {
        creds.access_token = new_at;
        if let Some(exp_in) = refreshed.expires_in {
            creds.expires_in = Some(exp_in);
            creds.expires_at = Some(now + exp_in);
        }
        if let Some(new_rt) = refreshed.refresh_token {
            creds.refresh_token = new_rt;
        }

        if let Some(path) = save_path {
            let content = serde_json::to_string_pretty(creds)?;
            let _ = write_file_atomic(path, &content, 0o600);
        }
    }

    Ok(creds.access_token.clone())
}

/// 解析 JWT 中的 user_id
pub fn extract_user_id_from_jwt(token: &str) -> Option<String> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;

    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let payload_b64 = parts[1].trim_end_matches('=');
    let decoded = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let val: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    val.get("user_id")
        .or_else(|| val.get("sub"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// 同步账号认证信息至 native ~/.kimi-code
pub fn sync_kimi_account_to_native(
    access_token: &str,
    refresh_token: &str,
    expires_in: i64,
    expires_at_sec: i64,
) -> Result<()> {
    #[cfg(test)]
    if std::env::var_os(KIMI_HOME_ENV).is_none() {
        return Ok(());
    }

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

/// 解析凭据对应的账号昵称
pub fn resolve_account_nickname(cred: &KimiNativeCredentials) -> Option<String> {
    let manager = crate::services::kimi_oauth::KimiOAuthService::manager();
    let user_id = extract_user_id_from_jwt(&cred.access_token);
    manager
        .find_account_sync(&cred.refresh_token, user_id.as_deref())
        .map(|a| a.login)
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

            let account = if has_credentials {
                read_profile_credentials(&name).ok().flatten().and_then(|c| resolve_account_nickname(&c))
            } else {
                None
            };

            profiles.push(KimiProfileInfo {
                name,
                path,
                is_active,
                has_credentials,
                has_config,
                account,
            });
        }
    }

    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(profiles)
}

#[derive(Debug, Clone, Serialize)]
pub struct KimiProfileQuotaItem {
    pub profile: KimiProfileInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usages: Option<KimiUsagesResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 批量查询所有 Profiles 的实时 Quota（并发查询）
pub async fn fetch_all_profiles_quota() -> Vec<KimiProfileQuotaItem> {
    let profiles = match list_profiles() {
        Ok(p) => p,
        Err(_) => return vec![],
    };

    let mut tasks = Vec::new();
    for p in profiles {
        tasks.push(async move {
            let mut creds = match read_profile_credentials(&p.name) {
                Ok(Some(c)) => c,
                Ok(None) => {
                    return KimiProfileQuotaItem {
                        profile: p,
                        usages: None,
                        error: None,
                    };
                }
                Err(e) => {
                    return KimiProfileQuotaItem {
                        profile: p,
                        usages: None,
                        error: Some(e.to_string()),
                    };
                }
            };

            let cred_path = p.path.join(KIMI_CREDENTIALS_DIR).join(KIMI_DEFAULT_CREDENTIAL_FILE);
            match get_valid_access_token(&mut creds, Some(&cred_path)).await {
                Ok(token) => {
                    match fetch_kimi_usages(&token).await {
                        Ok(u) => KimiProfileQuotaItem {
                            profile: p,
                            usages: Some(u),
                            error: None,
                        },
                        Err(e) => KimiProfileQuotaItem {
                            profile: p,
                            usages: None,
                            error: Some(e.to_string()),
                        },
                    }
                }
                Err(e) => KimiProfileQuotaItem {
                    profile: p,
                    usages: None,
                    error: Some(e.to_string()),
                },
            }
        });
    }

    futures::future::join_all(tasks).await
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KimiQuotaItem {
    #[serde(default)]
    pub used_ratio: Option<f64>,
    #[serde(default)]
    pub reset_time: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KimiQuotaUsages {
    #[serde(default)]
    pub limit_5h: Option<KimiQuotaItem>,
    #[serde(default)]
    pub limit_7d: Option<KimiQuotaItem>,
    #[serde(default)]
    pub limit_month_total: Option<KimiQuotaItem>,
    #[serde(default)]
    pub limit_month_code: Option<KimiQuotaItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KimiLimitDetail {
    #[serde(default)]
    pub limit: Option<String>,
    #[serde(default)]
    pub used: Option<String>,
    #[serde(default)]
    pub remaining: Option<String>,
    #[serde(default, rename = "resetTime")]
    pub reset_time: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KimiLimitWindowInfo {
    #[serde(default)]
    pub duration: Option<i64>,
    #[serde(default, rename = "timeUnit")]
    pub time_unit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KimiLimitWindow {
    #[serde(default)]
    pub window: Option<KimiLimitWindowInfo>,
    #[serde(default)]
    pub detail: Option<KimiLimitDetail>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KimiUsagesResponse {
    #[serde(default)]
    pub limits: Vec<KimiLimitWindow>,
    #[serde(default)]
    pub usages: Option<KimiQuotaUsages>,
}

pub async fn fetch_kimi_usages(access_token: &str) -> Result<KimiUsagesResponse> {
    let client = crate::proxy::http_client::get();
    let resp = client
        .get("https://api.kimi.com/coding/v1/usages")
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Accept", "application/json")
        .send()
        .await
        .context("请求 Kimi usages 接口失败")?;

    if !resp.status().is_success() {
        anyhow::bail!("Kimi usages 接口返回错误: {}", resp.status());
    }

    let usages: KimiUsagesResponse = resp.json().await.context("解析 Kimi usages 响应失败")?;
    Ok(usages)
}

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
