use crate::app_config::AppType;
use crate::config::{get_app_config_dir, home_dir, write_json_file};
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VisibleApps {
    #[serde(default = "default_visible_app_claude")]
    pub claude: bool,
    #[serde(default = "default_visible_app_codex")]
    pub codex: bool,
    #[serde(default = "default_visible_app_gemini")]
    pub gemini: bool,
    #[serde(default = "default_visible_app_opencode")]
    pub opencode: bool,
    #[serde(default = "default_visible_app_hermes")]
    pub hermes: bool,
    #[serde(default = "default_visible_app_openclaw")]
    pub openclaw: bool,
}

fn default_visible_app_claude() -> bool {
    true
}

fn default_visible_app_codex() -> bool {
    true
}

fn default_visible_app_gemini() -> bool {
    false
}

fn default_visible_app_opencode() -> bool {
    true
}

fn default_visible_app_hermes() -> bool {
    true
}

fn default_visible_app_openclaw() -> bool {
    true
}

pub fn default_visible_apps() -> VisibleApps {
    VisibleApps {
        claude: true,
        codex: true,
        gemini: false,
        opencode: true,
        hermes: true,
        openclaw: true,
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum VisibleAppsMode {
    #[default]
    Auto,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VisibleAppsSettings {
    #[serde(default)]
    pub mode: VisibleAppsMode,
    #[serde(default)]
    pub auto_prompt_decided: bool,
    #[serde(default)]
    pub manual_hidden_installed_notices: HashMap<String, bool>,
    #[serde(default)]
    pub last_detected_installed: HashMap<String, bool>,
}

impl Default for VisibleAppsSettings {
    fn default() -> Self {
        Self {
            mode: VisibleAppsMode::Auto,
            auto_prompt_decided: false,
            manual_hidden_installed_notices: HashMap::new(),
            last_detected_installed: HashMap::new(),
        }
    }
}

fn migrated_visible_apps_settings() -> VisibleAppsSettings {
    VisibleAppsSettings {
        mode: VisibleAppsMode::Manual,
        auto_prompt_decided: true,
        manual_hidden_installed_notices: HashMap::new(),
        last_detected_installed: HashMap::new(),
    }
}

impl Default for VisibleApps {
    fn default() -> Self {
        default_visible_apps()
    }
}

impl VisibleApps {
    pub fn ordered_enabled(&self) -> Vec<AppType> {
        app_order()
            .into_iter()
            .filter(|app_type| self.is_enabled_for(app_type))
            .collect()
    }

    pub fn is_enabled_for(&self, app_type: &AppType) -> bool {
        match app_type {
            AppType::Claude => self.claude,
            AppType::Codex => self.codex,
            AppType::Gemini => self.gemini,
            AppType::OpenCode => self.opencode,
            AppType::Hermes => self.hermes,
            AppType::OpenClaw => self.openclaw,
        }
    }

    pub fn set_enabled_for(&mut self, app_type: &AppType, enabled: bool) {
        match app_type {
            AppType::Claude => self.claude = enabled,
            AppType::Codex => self.codex = enabled,
            AppType::Gemini => self.gemini = enabled,
            AppType::OpenCode => self.opencode = enabled,
            AppType::Hermes => self.hermes = enabled,
            AppType::OpenClaw => self.openclaw = enabled,
        }
    }

    pub fn normalize(&mut self) {
        if self.ordered_enabled().is_empty() {
            *self = default_visible_apps();
        }
    }

    pub fn validate(&self) -> Result<(), AppError> {
        if self.ordered_enabled().is_empty() {
            return Err(AppError::InvalidInput(
                "At least one app must remain visible".to_string(),
            ));
        }

        Ok(())
    }
}

fn app_order() -> [AppType; 6] {
    [
        AppType::Claude,
        AppType::Codex,
        AppType::Gemini,
        AppType::OpenCode,
        AppType::Hermes,
        AppType::OpenClaw,
    ]
}

pub fn next_visible_app(
    visible: &VisibleApps,
    current: &AppType,
    direction: i8,
) -> Option<AppType> {
    let ordered = app_order();
    if ordered
        .iter()
        .all(|app_type| !visible.is_enabled_for(app_type))
    {
        return None;
    }

    let current_index = ordered.iter().position(|app_type| app_type == current)?;
    let step = if direction < 0 { -1 } else { 1 };
    let len = ordered.len() as isize;

    for offset in 1..=ordered.len() {
        let index = (current_index as isize + step * offset as isize).rem_euclid(len) as usize;
        let candidate = &ordered[index];
        if visible.is_enabled_for(candidate) {
            return Some(candidate.clone());
        }
    }

    None
}

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
        crate::services::webdav::parse_base_url(&self.base_url)?;
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

/// 本机自动迁移状态。
///
/// 这里记录的是本机启动时执行过的一次性迁移；标记不随数据库同步。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LocalMigrations {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_third_party_history_provider_bucket_v1:
        Option<CodexThirdPartyHistoryProviderBucketMigration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_provider_template_v1: Option<CodexProviderTemplateMigration>,
    /// 统一会话开关的官方历史迁移标记。开关关闭时会被清除，
    /// 这样重新开启能把关闭期间落入 openai 桶的官方会话补迁进来。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_official_history_unify_v1: Option<CodexOfficialHistoryUnifyMigration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexThirdPartyHistoryProviderBucketMigration {
    pub completed_at: String,
    pub target_provider_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_provider_ids: Vec<String>,
    #[serde(default)]
    pub migrated_jsonl_files: usize,
    #[serde(default)]
    pub migrated_state_rows: usize,
    #[serde(default)]
    pub scanned_history_files: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexProviderTemplateMigration {
    pub completed_at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub migrated_provider_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexOfficialHistoryUnifyMigration {
    pub completed_at: String,
    pub target_provider_id: String,
    #[serde(default)]
    pub migrated_jsonl_files: usize,
    #[serde(default)]
    pub migrated_state_rows: usize,
    /// 迁移时的规范化 Codex 目录。标记只对同一目录生效：
    /// 切换 codex_config_dir 后旧标记不会挡住新目录的迁移。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_config_dir: Option<String>,
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
    /// 是否已确认通用配置首次提示
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub common_config_confirmed: Option<bool>,
    /// 是否已确认用量查询首次提示
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_confirmed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_config_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_config_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gemini_config_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opencode_config_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hermes_config_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openclaw_config_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_provider_claude: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_provider_codex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_provider_gemini: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_provider_opencode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_provider_hermes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_provider_openclaw: Option<String>,
    #[serde(default = "default_visible_apps")]
    pub visible_apps: VisibleApps,
    #[serde(default = "migrated_visible_apps_settings")]
    pub visible_apps_settings: VisibleAppsSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// 是否开机自启
    #[serde(default)]
    pub launch_on_startup: bool,
    /// Keep Codex ChatGPT login material in auth.json when switching to third-party providers.
    /// Opt-in: defaults to false so third-party switches cleanly overwrite auth.json.
    #[serde(default)]
    pub preserve_codex_official_auth_on_switch: bool,
    /// Run official Codex providers under the shared "custom" model_provider id
    /// so official sessions share one resume-history bucket with third-party providers.
    #[serde(default)]
    pub unify_codex_session_history: bool,
    /// User opted in to migrate existing official sessions ("openai" bucket)
    /// into the shared bucket. Persisted so a failed migration retries at startup;
    /// cleared when the toggle turns off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unify_codex_migrate_existing: Option<bool>,
    /// Skills 同步方式（auto|symlink|copy）
    #[serde(default)]
    pub skill_sync_method: crate::services::skill::SyncMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<SecuritySettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webdav_sync: Option<WebDavSyncSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_retain_count: Option<u32>,
    /// 首选终端应用，用于会话恢复。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_terminal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_migrations: Option<LocalMigrations>,
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
            common_config_confirmed: None,
            usage_confirmed: None,
            claude_config_dir: None,
            codex_config_dir: None,
            gemini_config_dir: None,
            opencode_config_dir: None,
            hermes_config_dir: None,
            openclaw_config_dir: None,
            current_provider_claude: None,
            current_provider_codex: None,
            current_provider_gemini: None,
            current_provider_opencode: None,
            current_provider_hermes: None,
            current_provider_openclaw: None,
            visible_apps: default_visible_apps(),
            visible_apps_settings: VisibleAppsSettings::default(),
            language: None,
            launch_on_startup: false,
            preserve_codex_official_auth_on_switch: false,
            unify_codex_session_history: false,
            unify_codex_migrate_existing: None,
            skill_sync_method: crate::services::skill::SyncMethod::default(),
            security: None,
            webdav_sync: None,
            backup_retain_count: None,
            preferred_terminal: None,
            local_migrations: None,
            custom_endpoints_claude: HashMap::new(),
            custom_endpoints_codex: HashMap::new(),
        }
    }
}

impl AppSettings {
    fn settings_path() -> PathBuf {
        // settings.json 可以跟随 CC_SWITCH_CONFIG_DIR，且不会形成循环依赖：
        // 路径仅依赖进程环境变量和 HOME，不依赖已持久化的 settings 内容。
        get_app_config_dir().join("settings.json")
    }

    fn normalize_common(&mut self) {
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

        self.opencode_config_dir = self
            .opencode_config_dir
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        self.hermes_config_dir = self
            .hermes_config_dir
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        self.openclaw_config_dir = self
            .openclaw_config_dir
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

        self.preferred_terminal = self
            .preferred_terminal
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
    }

    fn normalize_loaded(&mut self) {
        self.normalize_common();
        self.visible_apps.normalize();
    }

    fn validate(&self) -> Result<(), AppError> {
        self.visible_apps.validate()
    }

    pub fn load() -> Self {
        let path = Self::settings_path();
        if let Ok(content) = fs::read_to_string(&path) {
            match serde_json::from_str::<AppSettings>(&content) {
                Ok(mut settings) => {
                    settings.normalize_loaded();
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
        normalized.normalize_common();
        normalized.validate()?;
        let path = Self::settings_path();

        write_json_file(&path, &normalized)?;
        Ok(())
    }
}

fn settings_store() -> &'static RwLock<AppSettings> {
    static STORE: OnceLock<RwLock<AppSettings>> = OnceLock::new();
    STORE.get_or_init(|| RwLock::new(AppSettings::load()))
}

pub fn reload_settings() -> Result<(), AppError> {
    let fresh_settings = AppSettings::load();
    let mut guard = settings_store().write().expect("写入设置锁失败");
    *guard = fresh_settings;
    Ok(())
}

#[cfg(test)]
pub(crate) fn reload_test_settings() {
    let mut guard = settings_store().write().expect("写入设置锁失败");
    *guard = AppSettings::load();
}

fn resolve_override_path(raw: &str) -> PathBuf {
    if raw == "~" {
        if let Some(home) = home_dir() {
            return home;
        }
    } else if let Some(stripped) = raw.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(stripped);
        }
    } else if let Some(stripped) = raw.strip_prefix("~\\") {
        if let Some(home) = home_dir() {
            return home.join(stripped);
        }
    }

    PathBuf::from(raw)
}

pub fn get_settings() -> AppSettings {
    settings_store().read().expect("读取设置锁失败").clone()
}

pub fn update_settings(mut new_settings: AppSettings) -> Result<(), AppError> {
    let existing_migrations = settings_store()
        .read()
        .ok()
        .and_then(|settings| settings.local_migrations.clone());
    if new_settings.local_migrations.is_none() {
        new_settings.local_migrations = existing_migrations;
    } else if let (Some(incoming_migrations), Some(existing_migrations)) =
        (&mut new_settings.local_migrations, existing_migrations)
    {
        if incoming_migrations
            .codex_third_party_history_provider_bucket_v1
            .is_none()
        {
            incoming_migrations.codex_third_party_history_provider_bucket_v1 =
                existing_migrations.codex_third_party_history_provider_bucket_v1;
        }
        if incoming_migrations.codex_provider_template_v1.is_none() {
            incoming_migrations.codex_provider_template_v1 =
                existing_migrations.codex_provider_template_v1;
        }
        if incoming_migrations
            .codex_official_history_unify_v1
            .is_none()
        {
            incoming_migrations.codex_official_history_unify_v1 =
                existing_migrations.codex_official_history_unify_v1;
        }
    }
    new_settings.normalize_common();
    new_settings.validate()?;
    new_settings.save()?;

    let mut guard = settings_store().write().expect("写入设置锁失败");
    *guard = new_settings;
    Ok(())
}

fn mutate_settings<F>(mutator: F) -> Result<(), AppError>
where
    F: FnOnce(&mut AppSettings),
{
    let mut guard = settings_store().write().unwrap_or_else(|error| {
        log::warn!("设置锁已毒化，使用恢复值: {error}");
        error.into_inner()
    });
    let mut next = guard.clone();
    mutator(&mut next);
    next.normalize_common();
    next.validate()?;
    next.save()?;
    *guard = next;
    Ok(())
}

pub fn is_codex_third_party_history_provider_bucket_migrated() -> bool {
    get_settings()
        .local_migrations
        .as_ref()
        .and_then(|migrations| {
            migrations
                .codex_third_party_history_provider_bucket_v1
                .as_ref()
        })
        .is_some_and(|migration| migration.scanned_history_files)
}

pub fn mark_codex_third_party_history_provider_bucket_migrated(
    migration: CodexThirdPartyHistoryProviderBucketMigration,
) -> Result<(), AppError> {
    mutate_settings(|settings| {
        let migrations = settings
            .local_migrations
            .get_or_insert_with(Default::default);
        migrations.codex_third_party_history_provider_bucket_v1 = Some(migration);
    })
}

pub fn is_codex_provider_template_migrated() -> bool {
    get_settings()
        .local_migrations
        .as_ref()
        .and_then(|migrations| migrations.codex_provider_template_v1.as_ref())
        .is_some()
}

pub fn mark_codex_provider_template_migrated(
    migration: CodexProviderTemplateMigration,
) -> Result<(), AppError> {
    mutate_settings(|settings| {
        let migrations = settings
            .local_migrations
            .get_or_insert_with(Default::default);
        migrations.codex_provider_template_v1 = Some(migration);
    })
}

/// 统一会话迁移标记是否覆盖指定目录。标记里没记目录视为不匹配；
/// 重跑迁移是幂等的，宁可重迁也不漏迁。
pub fn is_codex_official_history_unify_migrated_for_dir(codex_dir: &str) -> bool {
    get_settings()
        .local_migrations
        .as_ref()
        .and_then(|migrations| migrations.codex_official_history_unify_v1.as_ref())
        .is_some_and(|migration| migration.codex_config_dir.as_deref() == Some(codex_dir))
}

/// 条件写入迁移完成标记：仅当此刻开关仍开启且迁移意愿仍在时才写。
pub fn mark_codex_official_history_unify_migrated_if_enabled(
    migration: CodexOfficialHistoryUnifyMigration,
) -> Result<bool, AppError> {
    let mut written = false;
    mutate_settings(|settings| {
        if settings.unify_codex_session_history
            && settings.unify_codex_migrate_existing.unwrap_or(false)
        {
            settings
                .local_migrations
                .get_or_insert_with(Default::default)
                .codex_official_history_unify_v1 = Some(migration);
            written = true;
        }
    })?;
    Ok(written)
}

pub fn clear_codex_official_history_unify_migration() -> Result<(), AppError> {
    mutate_settings(|settings| {
        if let Some(migrations) = settings.local_migrations.as_mut() {
            migrations.codex_official_history_unify_v1 = None;
        }
    })
}

pub fn unify_codex_migrate_existing_requested() -> bool {
    get_settings().unify_codex_migrate_existing.unwrap_or(false)
}

pub fn clear_codex_unify_migrate_existing() -> Result<(), AppError> {
    mutate_settings(|settings| {
        settings.unify_codex_migrate_existing = None;
    })
}

pub fn get_preferred_terminal() -> Option<String> {
    settings_store()
        .read()
        .ok()
        .and_then(|settings| settings.preferred_terminal.clone())
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

pub fn preserve_codex_official_auth_on_switch() -> bool {
    settings_store()
        .read()
        .unwrap_or_else(|error| {
            log::warn!("设置锁已毒化，使用恢复值: {error}");
            error.into_inner()
        })
        .preserve_codex_official_auth_on_switch
}

pub fn unify_codex_session_history() -> bool {
    settings_store()
        .read()
        .unwrap_or_else(|error| {
            log::warn!("设置锁已毒化，使用恢复值: {error}");
            error.into_inner()
        })
        .unify_codex_session_history
}

pub fn get_gemini_override_dir() -> Option<PathBuf> {
    let settings = settings_store().read().ok()?;
    settings
        .gemini_config_dir
        .as_ref()
        .map(|p| resolve_override_path(p))
}

pub fn get_opencode_override_dir() -> Option<PathBuf> {
    let settings = settings_store().read().ok()?;
    settings
        .opencode_config_dir
        .as_ref()
        .map(|p| resolve_override_path(p))
}

pub fn get_hermes_override_dir() -> Option<PathBuf> {
    let settings = settings_store().read().ok()?;
    settings
        .hermes_config_dir
        .as_ref()
        .map(|p| resolve_override_path(p))
}

pub fn get_openclaw_override_dir() -> Option<PathBuf> {
    let settings = settings_store().read().ok()?;
    settings
        .openclaw_config_dir
        .as_ref()
        .map(|p| resolve_override_path(p))
}

pub fn get_current_provider(app_type: &AppType) -> Option<String> {
    let settings = settings_store().read().ok()?;
    match app_type {
        AppType::Claude => settings.current_provider_claude.clone(),
        AppType::Codex => settings.current_provider_codex.clone(),
        AppType::Gemini => settings.current_provider_gemini.clone(),
        AppType::OpenCode => settings.current_provider_opencode.clone(),
        AppType::Hermes => settings.current_provider_hermes.clone(),
        AppType::OpenClaw => settings.current_provider_openclaw.clone(),
    }
}

pub fn set_current_provider(app_type: &AppType, id: Option<&str>) -> Result<(), AppError> {
    let mut settings = get_settings();

    match app_type {
        AppType::Claude => settings.current_provider_claude = id.map(|value| value.to_string()),
        AppType::Codex => settings.current_provider_codex = id.map(|value| value.to_string()),
        AppType::Gemini => settings.current_provider_gemini = id.map(|value| value.to_string()),
        AppType::OpenCode => settings.current_provider_opencode = id.map(|value| value.to_string()),
        AppType::Hermes => settings.current_provider_hermes = id.map(|value| value.to_string()),
        AppType::OpenClaw => settings.current_provider_openclaw = id.map(|value| value.to_string()),
    }

    update_settings(settings)
}

pub fn get_visible_apps() -> VisibleApps {
    settings_store()
        .read()
        .map(|settings| settings.visible_apps.clone())
        .unwrap_or_else(|_| default_visible_apps())
}

pub fn set_visible_apps(visible_apps: VisibleApps) -> Result<(), AppError> {
    visible_apps.validate()?;

    let mut settings = get_settings();
    settings.visible_apps = visible_apps;
    update_settings(settings)
}

pub fn get_visible_apps_settings() -> VisibleAppsSettings {
    settings_store()
        .read()
        .map(|settings| settings.visible_apps_settings.clone())
        .unwrap_or_default()
}

pub fn set_visible_apps_mode(mode: VisibleAppsMode) -> Result<(), AppError> {
    let mut settings = get_settings();
    settings.visible_apps_settings.mode = mode;
    settings.visible_apps_settings.auto_prompt_decided = true;
    update_settings(settings)
}

pub fn get_effective_current_provider(
    db: &crate::database::Database,
    app_type: &AppType,
) -> Result<Option<String>, AppError> {
    if let Some(local_id) = get_current_provider(app_type) {
        let providers = db.get_all_providers(app_type.as_str())?;
        if providers.contains_key(&local_id) {
            return Ok(Some(local_id));
        }

        log::warn!(
            "本地 settings 中的供应商 {} ({}) 在数据库中不存在，将清理并 fallback 到数据库",
            local_id,
            app_type.as_str()
        );
        let _ = set_current_provider(app_type, None);
    }

    db.get_current_provider(app_type.as_str())
}

pub fn get_skill_sync_method() -> crate::services::skill::SyncMethod {
    settings_store()
        .read()
        .map(|s| s.skill_sync_method)
        .unwrap_or_default()
}

pub fn effective_backup_retain_count() -> usize {
    settings_store()
        .read()
        .map(|settings| {
            settings
                .backup_retain_count
                .map(|count| usize::try_from(count).unwrap_or(usize::MAX).max(1))
                .unwrap_or(10)
        })
        .unwrap_or(10)
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

pub fn get_common_config_confirmed() -> bool {
    settings_store()
        .read()
        .map(|s| s.common_config_confirmed.unwrap_or(false))
        .unwrap_or(false)
}

pub fn set_common_config_confirmed(confirmed: bool) -> Result<(), AppError> {
    let mut settings = get_settings();
    settings.common_config_confirmed = Some(confirmed);
    update_settings(settings)
}

pub fn get_usage_confirmed() -> bool {
    settings_store()
        .read()
        .map(|s| s.usage_confirmed.unwrap_or(false))
        .unwrap_or(false)
}

pub fn set_usage_confirmed(confirmed: bool) -> Result<(), AppError> {
    let mut settings = get_settings();
    settings.usage_confirmed = Some(confirmed);
    update_settings(settings)
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

#[cfg(test)]
mod tests {
    use super::AppSettings;

    #[test]
    fn codex_unified_session_history_defaults_off() {
        let settings = AppSettings::default();
        assert!(!settings.unify_codex_session_history);
        assert_eq!(settings.unify_codex_migrate_existing, None);
    }
}
