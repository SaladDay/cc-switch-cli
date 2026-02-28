use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};
use url::Url;

use crate::error::AppError;

/// 自定义端点配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomEndpoint {
    pub url: String,
    pub added_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SecurityAuthSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SecuritySettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<SecurityAuthSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WebDavSyncStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_remote_etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_local_manifest_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_remote_manifest_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WebDavSyncSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub base_url: String,
    #[serde(default = "default_webdav_remote_root")]
    pub remote_root: String,
    #[serde(default = "default_webdav_profile")]
    pub profile: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub auto_sync: bool,
    #[serde(default)]
    pub status: WebDavSyncStatus,
}

fn default_webdav_remote_root() -> String {
    "cc-switch-sync".to_string()
}

fn default_webdav_profile() -> String {
    "default".to_string()
}

const JIANGUOYUN_WEBDAV_BASE_URL: &str = "https://dav.jianguoyun.com/dav";

impl Default for WebDavSyncSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: String::new(),
            remote_root: default_webdav_remote_root(),
            profile: default_webdav_profile(),
            username: String::new(),
            password: String::new(),
            auto_sync: false,
            status: WebDavSyncStatus::default(),
        }
    }
}

impl WebDavSyncSettings {
    pub fn jianguoyun_preset(username: &str, password: &str) -> Self {
        let mut settings = Self {
            enabled: true,
            base_url: JIANGUOYUN_WEBDAV_BASE_URL.to_string(),
            remote_root: default_webdav_remote_root(),
            profile: default_webdav_profile(),
            username: username.to_string(),
            password: password.to_string(),
            ..Self::default()
        };
        settings.normalize();
        settings
    }

    pub fn normalize(&mut self) {
        self.base_url = self.base_url.trim().trim_end_matches('/').to_string();
        self.remote_root = sanitize_path_segment(&self.remote_root);
        self.profile = sanitize_path_segment(&self.profile);
        self.username = self.username.trim().to_string();
        self.password = self.password.trim().to_string();
    }

    pub fn validate(&self) -> Result<(), AppError> {
        if !self.enabled && self.base_url.is_empty() {
            return Ok(());
        }
        if self.base_url.is_empty() {
            return Err(AppError::InvalidInput(
                "WebDAV base_url 不能为空".to_string(),
            ));
        }
        let url = Url::parse(&self.base_url)
            .map_err(|e| AppError::InvalidInput(format!("WebDAV base_url 不是合法 URL: {e}")))?;
        let scheme = url.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(AppError::InvalidInput(
                "WebDAV base_url 仅支持 http/https".to_string(),
            ));
        }
        if self.remote_root.is_empty() || self.profile.is_empty() {
            return Err(AppError::InvalidInput(
                "WebDAV remote_root/profile 不能为空".to_string(),
            ));
        }
        if self.remote_root.contains("..") || self.profile.contains("..") {
            return Err(AppError::InvalidInput(
                "WebDAV remote_root/profile 不能包含 '..'".to_string(),
            ));
        }
        Ok(())
    }
}

fn sanitize_path_segment(raw: &str) -> String {
    raw.trim()
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/// 应用设置结构，允许覆盖默认配置目录
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(default = "default_show_in_tray")]
    pub show_in_tray: bool,
    #[serde(default = "default_minimize_to_tray_on_close")]
    pub minimize_to_tray_on_close: bool,
    /// 是否启用 Claude 插件联动
    #[serde(default)]
    pub enable_claude_plugin_integration: bool,
    /// 是否跳过 Claude Code 初次安装确认
    #[serde(default)]
    pub skip_claude_onboarding: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_config_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_config_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gemini_config_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// 是否开机自启
    #[serde(default)]
    pub launch_on_startup: bool,
    /// Skills 同步方式（auto|symlink|copy）
    #[serde(default)]
    pub skill_sync_method: crate::services::skill::SyncMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<SecuritySettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webdav_sync: Option<WebDavSyncSettings>,
    /// Claude 自定义端点列表
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub custom_endpoints_claude: HashMap<String, CustomEndpoint>,
    /// Codex 自定义端点列表
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub custom_endpoints_codex: HashMap<String, CustomEndpoint>,
}

fn default_show_in_tray() -> bool {
    true
}

fn default_minimize_to_tray_on_close() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            show_in_tray: true,
            minimize_to_tray_on_close: true,
            enable_claude_plugin_integration: false,
            skip_claude_onboarding: false,
            claude_config_dir: None,
            codex_config_dir: None,
            gemini_config_dir: None,
            language: None,
            launch_on_startup: false,
            skill_sync_method: crate::services::skill::SyncMethod::default(),
            security: None,
            webdav_sync: None,
            custom_endpoints_claude: HashMap::new(),
            custom_endpoints_codex: HashMap::new(),
        }
    }
}

impl AppSettings {
    fn settings_path() -> PathBuf {
        // settings.json 必须使用固定路径，不能被 app_config_dir 覆盖
        // 否则会造成循环依赖：读取 settings 需要知道路径，但路径在 settings 中
        dirs::home_dir()
            .expect("无法获取用户主目录")
            .join(".cc-switch")
            .join("settings.json")
    }

    fn normalize_paths(&mut self) {
        self.claude_config_dir = self
            .claude_config_dir
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        self.codex_config_dir = self
            .codex_config_dir
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        self.gemini_config_dir = self
            .gemini_config_dir
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        self.language = self
            .language
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| matches!(*s, "en" | "zh"))
            .map(|s| s.to_string());

        if let Some(webdav) = self.webdav_sync.as_mut() {
            webdav.normalize();
        }
    }

    pub fn load() -> Self {
        let path = Self::settings_path();
        if let Ok(content) = fs::read_to_string(&path) {
            match serde_json::from_str::<AppSettings>(&content) {
                Ok(mut settings) => {
                    settings.normalize_paths();
                    settings
                }
                Err(err) => {
                    log::warn!(
                        "解析设置文件失败，将使用默认设置。路径: {}, 错误: {}",
                        path.display(),
                        err
                    );
                    Self::default()
                }
            }
        } else {
            Self::default()
        }
    }

    pub fn save(&self) -> Result<(), AppError> {
        let mut normalized = self.clone();
        normalized.normalize_paths();
        let path = Self::settings_path();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }

        let json = serde_json::to_string_pretty(&normalized)
            .map_err(|e| AppError::JsonSerialize { source: e })?;
        fs::write(&path, json).map_err(|e| AppError::io(&path, e))?;
        Ok(())
    }
}

fn settings_store() -> &'static RwLock<AppSettings> {
    static STORE: OnceLock<RwLock<AppSettings>> = OnceLock::new();
    STORE.get_or_init(|| RwLock::new(AppSettings::load()))
}

fn resolve_override_path(raw: &str) -> PathBuf {
    if raw == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    } else if let Some(stripped) = raw.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    } else if let Some(stripped) = raw.strip_prefix("~\\") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }

    PathBuf::from(raw)
}

pub fn get_settings() -> AppSettings {
    settings_store().read().expect("读取设置锁失败").clone()
}

pub fn update_settings(mut new_settings: AppSettings) -> Result<(), AppError> {
    new_settings.normalize_paths();
    new_settings.save()?;

    let mut guard = settings_store().write().expect("写入设置锁失败");
    *guard = new_settings;
    Ok(())
}

pub fn ensure_security_auth_selected_type(selected_type: &str) -> Result<(), AppError> {
    let mut settings = get_settings();
    let current = settings
        .security
        .as_ref()
        .and_then(|sec| sec.auth.as_ref())
        .and_then(|auth| auth.selected_type.as_deref());

    if current == Some(selected_type) {
        return Ok(());
    }

    let mut security = settings.security.unwrap_or_default();
    let mut auth = security.auth.unwrap_or_default();
    auth.selected_type = Some(selected_type.to_string());
    security.auth = Some(auth);
    settings.security = Some(security);

    update_settings(settings)
}

pub fn get_claude_override_dir() -> Option<PathBuf> {
    let settings = settings_store().read().ok()?;
    settings
        .claude_config_dir
        .as_ref()
        .map(|p| resolve_override_path(p))
}

pub fn get_codex_override_dir() -> Option<PathBuf> {
    let settings = settings_store().read().ok()?;
    settings
        .codex_config_dir
        .as_ref()
        .map(|p| resolve_override_path(p))
}

pub fn get_gemini_override_dir() -> Option<PathBuf> {
    let settings = settings_store().read().ok()?;
    settings
        .gemini_config_dir
        .as_ref()
        .map(|p| resolve_override_path(p))
}

pub fn get_skill_sync_method() -> crate::services::skill::SyncMethod {
    settings_store()
        .read()
        .map(|s| s.skill_sync_method)
        .unwrap_or_default()
}

pub fn set_skill_sync_method(method: crate::services::skill::SyncMethod) -> Result<(), AppError> {
    let mut settings = get_settings();
    settings.skill_sync_method = method;
    update_settings(settings)
}

pub fn get_webdav_sync_settings() -> Option<WebDavSyncSettings> {
    settings_store()
        .read()
        .ok()
        .and_then(|s| s.webdav_sync.clone())
}

pub fn set_webdav_sync_settings(webdav_sync: Option<WebDavSyncSettings>) -> Result<(), AppError> {
    let mut settings = get_settings();
    settings.webdav_sync = match webdav_sync {
        Some(mut cfg) => {
            cfg.normalize();
            cfg.validate()?;
            Some(cfg)
        }
        None => None,
    };
    update_settings(settings)
}

pub fn update_webdav_sync_status(status: WebDavSyncStatus) -> Result<(), AppError> {
    let mut settings = get_settings();
    if let Some(ref mut webdav) = settings.webdav_sync {
        webdav.status = status;
    }
    update_settings(settings)
}

pub fn webdav_jianguoyun_preset(username: &str, password: &str) -> WebDavSyncSettings {
    WebDavSyncSettings::jianguoyun_preset(username, password)
}

pub fn get_skip_claude_onboarding() -> bool {
    settings_store()
        .read()
        .map(|s| s.skip_claude_onboarding)
        .unwrap_or(false)
}

pub fn get_enable_claude_plugin_integration() -> bool {
    settings_store()
        .read()
        .map(|s| s.enable_claude_plugin_integration)
        .unwrap_or(false)
}

pub fn set_enable_claude_plugin_integration(enabled: bool) -> Result<(), AppError> {
    let mut settings = get_settings();
    settings.enable_claude_plugin_integration = enabled;
    update_settings(settings)
}

pub fn set_skip_claude_onboarding(enabled: bool) -> Result<(), AppError> {
    if enabled {
        crate::claude_mcp::set_has_completed_onboarding()?;
    } else {
        crate::claude_mcp::clear_has_completed_onboarding()?;
    }

    let mut settings = get_settings();
    settings.skip_claude_onboarding = enabled;
    update_settings(settings)
}
