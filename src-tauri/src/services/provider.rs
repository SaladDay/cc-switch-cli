use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::app_config::{AppType, MultiAppConfig};
use crate::codex_config::{get_codex_auth_path, get_codex_config_path};
use crate::config::{
    delete_file, get_claude_settings_path, get_provider_config_path, read_json_file,
    write_json_file, write_text_file,
};
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta, UsageData, UsageResult};
use crate::settings::{self, CustomEndpoint};
use crate::store::AppState;
use crate::usage_script;

/// 供应商相关业务逻辑
pub struct ProviderService;

/// 从供应商名称生成 Codex 的 provider ID (lowercase alphanumeric)
/// 示例: "Duck Coding" -> "duckcoding"
fn generate_provider_id_from_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

#[derive(Clone)]
enum LiveSnapshot {
    Claude {
        settings: Option<Value>,
    },
    Codex {
        auth: Option<Value>,
        config: Option<String>,
    },
    Gemini {
        env: Option<HashMap<String, String>>, // 新增
        config: Option<Value>,                // 新增：settings.json 内容
    },
}

#[derive(Clone)]
struct PostCommitAction {
    app_type: AppType,
    provider: Provider,
    backup: LiveSnapshot,
    sync_mcp: bool,
    refresh_snapshot: bool,
    common_config_snippet: Option<String>,
}

impl LiveSnapshot {
    fn restore(&self) -> Result<(), AppError> {
        match self {
            LiveSnapshot::Claude { settings } => {
                let path = get_claude_settings_path();
                if let Some(value) = settings {
                    write_json_file(&path, value)?;
                } else if path.exists() {
                    delete_file(&path)?;
                }
            }
            LiveSnapshot::Codex { auth, config } => {
                let auth_path = get_codex_auth_path();
                let config_path = get_codex_config_path();
                if let Some(value) = auth {
                    write_json_file(&auth_path, value)?;
                } else if auth_path.exists() {
                    delete_file(&auth_path)?;
                }

                if let Some(text) = config {
                    write_text_file(&config_path, text)?;
                } else if config_path.exists() {
                    delete_file(&config_path)?;
                }
            }
            LiveSnapshot::Gemini { env, .. } => {
                // 新增
                use crate::gemini_config::{
                    get_gemini_env_path, get_gemini_settings_path, write_gemini_env_atomic,
                };
                let path = get_gemini_env_path();
                if let Some(env_map) = env {
                    write_gemini_env_atomic(env_map)?;
                } else if path.exists() {
                    delete_file(&path)?;
                }

                let settings_path = get_gemini_settings_path();
                match self {
                    LiveSnapshot::Gemini {
                        config: Some(cfg), ..
                    } => {
                        write_json_file(&settings_path, cfg)?;
                    }
                    LiveSnapshot::Gemini { config: None, .. } if settings_path.exists() => {
                        delete_file(&settings_path)?;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::ffi::OsString;
    use std::path::Path;
    use std::sync::RwLock;
    use tempfile::TempDir;

    struct EnvGuard {
        old_home: Option<OsString>,
        old_userprofile: Option<OsString>,
    }

    impl EnvGuard {
        fn set_home(home: &Path) -> Self {
            let old_home = std::env::var_os("HOME");
            let old_userprofile = std::env::var_os("USERPROFILE");
            std::env::set_var("HOME", home);
            std::env::set_var("USERPROFILE", home);
            Self {
                old_home,
                old_userprofile,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.old_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match &self.old_userprofile {
                Some(value) => std::env::set_var("USERPROFILE", value),
                None => std::env::remove_var("USERPROFILE"),
            }
        }
    }

    #[test]
    fn validate_provider_settings_allows_missing_auth_for_codex() {
        let provider = Provider::with_id(
            "codex".into(),
            "Codex".into(),
            json!({ "config": "base_url = \"https://example.com\"" }),
            None,
        );
        ProviderService::validate_provider_settings(&AppType::Codex, &provider)
            .expect("Codex auth is optional when using OpenAI auth or env_key");
    }

    #[test]
    #[serial]
    fn switch_codex_succeeds_without_auth_json() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let mut config = MultiAppConfig::default();
        config.ensure_app(&AppType::Codex);
        {
            let manager = config
                .get_manager_mut(&AppType::Codex)
                .expect("codex manager");
            manager.current = "p2".to_string();
            manager.providers.insert(
                "p1".to_string(),
                Provider::with_id(
                    "p1".to_string(),
                    "Keyring".to_string(),
                    json!({
                        "config": "requires_openai_auth = true\n",
                    }),
                    None,
                ),
            );
            manager.providers.insert(
                "p2".to_string(),
                Provider::with_id(
                    "p2".to_string(),
                    "Other".to_string(),
                    json!({
                        "config": "requires_openai_auth = true\n",
                    }),
                    None,
                ),
            );
        }

        let state = AppState {
            config: RwLock::new(config),
        };

        ProviderService::switch(&state, AppType::Codex, "p1")
            .expect("switch should succeed without auth.json when using credential store");

        assert!(
            !get_codex_auth_path().exists(),
            "auth.json should remain absent when provider has no auth config"
        );

        let live_config_text =
            std::fs::read_to_string(get_codex_config_path()).expect("read live config.toml");

        let guard = state.config.read().expect("read config after switch");
        let manager = guard
            .get_manager(&AppType::Codex)
            .expect("codex manager after switch");
        assert_eq!(manager.current, "p1", "current provider should update");
        let provider = manager.providers.get("p1").expect("p1 exists");
        assert!(
            provider.settings_config.get("auth").is_none(),
            "snapshot should not inject auth when auth.json is absent"
        );
        assert_eq!(
            provider
                .settings_config
                .get("config")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            live_config_text,
            "provider snapshot should match live config.toml even without auth.json"
        );
    }

    #[test]
    #[serial]
    fn add_first_provider_sets_current() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let mut config = MultiAppConfig::default();
        config.ensure_app(&AppType::Claude);
        let state = AppState {
            config: RwLock::new(config),
        };

        let provider = Provider::with_id(
            "p1".to_string(),
            "First".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "token",
                    "ANTHROPIC_BASE_URL": "https://claude.example"
                }
            }),
            None,
        );

        ProviderService::add(&state, AppType::Claude, provider).expect("add should succeed");

        let cfg = state.config.read().expect("read config");
        let manager = cfg.get_manager(&AppType::Claude).expect("claude manager");
        assert_eq!(
            manager.current, "p1",
            "first provider should become current to avoid empty current provider"
        );
    }

    #[test]
    #[serial]
    fn common_config_snippet_is_merged_into_claude_settings_on_write() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let mut config = MultiAppConfig::default();
        config.ensure_app(&AppType::Claude);
        config.common_config_snippets.claude = Some(
            r#"{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1},"includeCoAuthoredBy":false}"#
                .to_string(),
        );

        let state = AppState {
            config: RwLock::new(config),
        };

        let provider = Provider::with_id(
            "p1".to_string(),
            "First".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "token",
                    "ANTHROPIC_BASE_URL": "https://claude.example"
                }
            }),
            None,
        );

        ProviderService::add(&state, AppType::Claude, provider).expect("add should succeed");

        let settings_path = get_claude_settings_path();
        let live: Value = read_json_file(&settings_path).expect("read live settings");

        assert_eq!(
            live.get("includeCoAuthoredBy").and_then(Value::as_bool),
            Some(false),
            "common snippet should be merged into settings.json"
        );

        let env = live
            .get("env")
            .and_then(Value::as_object)
            .expect("settings.env should be object");

        assert_eq!(
            env.get("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC")
                .and_then(Value::as_i64),
            Some(1),
            "common env key should be present in settings.env"
        );
        assert_eq!(
            env.get("ANTHROPIC_AUTH_TOKEN").and_then(Value::as_str),
            Some("token"),
            "provider env key should remain in settings.env"
        );
    }

    #[test]
    #[serial]
    fn common_config_snippet_is_not_persisted_into_provider_snapshot_on_switch() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let mut config = MultiAppConfig::default();
        config.ensure_app(&AppType::Claude);
        config.common_config_snippets.claude = Some(
            r#"{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1},"includeCoAuthoredBy":false}"#
                .to_string(),
        );

        let state = AppState {
            config: RwLock::new(config),
        };

        let p1 = Provider::with_id(
            "p1".to_string(),
            "First".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "token1",
                    "ANTHROPIC_BASE_URL": "https://claude.one"
                }
            }),
            None,
        );
        let p2 = Provider::with_id(
            "p2".to_string(),
            "Second".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "token2",
                    "ANTHROPIC_BASE_URL": "https://claude.two"
                }
            }),
            None,
        );

        ProviderService::add(&state, AppType::Claude, p1).expect("add p1");
        ProviderService::add(&state, AppType::Claude, p2).expect("add p2");

        ProviderService::switch(&state, AppType::Claude, "p2").expect("switch to p2");

        let cfg = state.config.read().expect("read config");
        let manager = cfg.get_manager(&AppType::Claude).expect("claude manager");
        let p1_after = manager.providers.get("p1").expect("p1 exists");

        assert!(
            p1_after
                .settings_config
                .get("includeCoAuthoredBy")
                .is_none(),
            "common top-level keys should not be persisted into provider snapshot"
        );

        let env = p1_after
            .settings_config
            .get("env")
            .and_then(Value::as_object)
            .expect("provider env should be object");
        assert!(
            !env.contains_key("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"),
            "common env keys should not be persisted into provider snapshot"
        );
        assert_eq!(
            env.get("ANTHROPIC_AUTH_TOKEN").and_then(Value::as_str),
            Some("token1"),
            "provider-specific env should remain in snapshot"
        );
    }

    #[test]
    fn extract_credentials_returns_expected_values() {
        let provider = Provider::with_id(
            "claude".into(),
            "Claude".into(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "token",
                    "ANTHROPIC_BASE_URL": "https://claude.example"
                }
            }),
            None,
        );
        let (api_key, base_url) =
            ProviderService::extract_credentials(&provider, &AppType::Claude).unwrap();
        assert_eq!(api_key, "token");
        assert_eq!(base_url, "https://claude.example");
    }

    #[test]
    #[serial]
    fn common_config_snippet_is_merged_into_gemini_env_on_write() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let mut config = MultiAppConfig::default();
        config.ensure_app(&AppType::Gemini);
        config.common_config_snippets.gemini =
            Some(r#"{"env":{"CC_SWITCH_GEMINI_COMMON":"1"}}"#.to_string());

        let state = AppState {
            config: RwLock::new(config),
        };

        let provider = Provider::with_id(
            "p1".to_string(),
            "First".to_string(),
            json!({
                "env": {
                    "GEMINI_API_KEY": "token"
                }
            }),
            None,
        );

        ProviderService::add(&state, AppType::Gemini, provider).expect("add should succeed");

        let env = crate::gemini_config::read_gemini_env().expect("read gemini env");
        assert_eq!(
            env.get("CC_SWITCH_GEMINI_COMMON").map(String::as_str),
            Some("1"),
            "common snippet env key should be present in ~/.gemini/.env"
        );
        assert_eq!(
            env.get("GEMINI_API_KEY").map(String::as_str),
            Some("token"),
            "provider env key should remain in ~/.gemini/.env"
        );
    }

    #[test]
    #[serial]
    fn common_config_snippet_is_not_persisted_into_gemini_provider_snapshot_on_switch() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let mut config = MultiAppConfig::default();
        config.ensure_app(&AppType::Gemini);
        config.common_config_snippets.gemini =
            Some(r#"{"env":{"CC_SWITCH_GEMINI_COMMON":"1"}}"#.to_string());

        let state = AppState {
            config: RwLock::new(config),
        };

        let p1 = Provider::with_id(
            "p1".to_string(),
            "First".to_string(),
            json!({
                "env": {
                    "GEMINI_API_KEY": "token1"
                }
            }),
            None,
        );
        let p2 = Provider::with_id(
            "p2".to_string(),
            "Second".to_string(),
            json!({
                "env": {
                    "GEMINI_API_KEY": "token2"
                }
            }),
            None,
        );

        ProviderService::add(&state, AppType::Gemini, p1).expect("add p1");
        ProviderService::add(&state, AppType::Gemini, p2).expect("add p2");

        ProviderService::switch(&state, AppType::Gemini, "p2").expect("switch to p2");

        let cfg = state.config.read().expect("read config");
        let manager = cfg.get_manager(&AppType::Gemini).expect("gemini manager");
        let p1_after = manager.providers.get("p1").expect("p1 exists");

        let env = p1_after
            .settings_config
            .get("env")
            .and_then(Value::as_object)
            .expect("provider env should be object");

        assert!(
            !env.contains_key("CC_SWITCH_GEMINI_COMMON"),
            "common env keys should not be persisted into provider snapshot"
        );
        assert_eq!(
            env.get("GEMINI_API_KEY").and_then(Value::as_str),
            Some("token1"),
            "provider-specific env should remain in snapshot"
        );
    }
}

fn merge_json_values(base: &mut Value, overlay: &Value) {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            for (key, overlay_value) in overlay_map {
                match base_map.get_mut(key) {
                    Some(base_value) => merge_json_values(base_value, overlay_value),
                    None => {
                        base_map.insert(key.clone(), overlay_value.clone());
                    }
                }
            }
        }
        (base_value, overlay_value) => {
            *base_value = overlay_value.clone();
        }
    }
}

fn strip_common_values(target: &mut Value, common: &Value) {
    match (target, common) {
        (Value::Object(target_map), Value::Object(common_map)) => {
            for (key, common_value) in common_map {
                let should_remove = match target_map.get_mut(key) {
                    Some(target_value) => match target_value {
                        Value::Object(_) if matches!(common_value, Value::Object(_)) => {
                            strip_common_values(target_value, common_value);
                            target_value.as_object().is_some_and(|m| m.is_empty())
                        }
                        _ => target_value == common_value,
                    },
                    None => false,
                };

                if should_remove {
                    target_map.remove(key);
                }
            }
        }
        (target_value, common_value) => {
            if target_value == common_value {
                *target_value = Value::Null;
            }
        }
    }
}

/// Gemini 认证类型枚举
///
/// 区分 OAuth 和 API Key 两种认证方式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GeminiAuthType {
    /// Google 官方（使用 OAuth 认证）
    GoogleOfficial,
    /// API Key 认证（包括所有第三方供应商：PackyCode、Generic 等）
    ApiKey,
}

impl ProviderService {
    // 认证类型常量
    const API_KEY_SECURITY_SELECTED_TYPE: &'static str = "gemini-api-key";
    const GOOGLE_OAUTH_SECURITY_SELECTED_TYPE: &'static str = "oauth-personal";

    // Partner Promotion Key 常量
    const GOOGLE_OFFICIAL_PARTNER_KEY: &'static str = "google-official";

    fn parse_common_claude_config_snippet(snippet: &str) -> Result<Value, AppError> {
        let value: Value = serde_json::from_str(snippet).map_err(|e| {
            AppError::localized(
                "common_config.claude.invalid_json",
                format!("Claude 通用配置片段不是有效的 JSON：{e}"),
                format!("Claude common config snippet is not valid JSON: {e}"),
            )
        })?;
        if !value.is_object() {
            return Err(AppError::localized(
                "common_config.claude.not_object",
                "Claude 通用配置片段必须是 JSON 对象",
                "Claude common config snippet must be a JSON object",
            ));
        }
        Ok(value)
    }

    fn parse_common_gemini_config_snippet(snippet: &str) -> Result<Value, AppError> {
        let value: Value = serde_json::from_str(snippet).map_err(|e| {
            AppError::localized(
                "common_config.gemini.invalid_json",
                format!("Gemini 通用配置片段不是有效的 JSON：{e}"),
                format!("Gemini common config snippet is not valid JSON: {e}"),
            )
        })?;
        if !value.is_object() {
            return Err(AppError::localized(
                "common_config.gemini.not_object",
                "Gemini 通用配置片段必须是 JSON 对象",
                "Gemini common config snippet must be a JSON object",
            ));
        }
        Ok(value)
    }

    /// 检测 Gemini 供应商的认证类型
    ///
    /// 只区分两种认证方式：OAuth (Google 官方) 和 API Key (所有其他供应商)
    ///
    /// # 返回值
    ///
    /// - `GeminiAuthType::GoogleOfficial`: Google 官方，使用 OAuth
    /// - `GeminiAuthType::ApiKey`: 其他所有供应商，使用 API Key
    fn detect_gemini_auth_type(provider: &Provider) -> GeminiAuthType {
        // 检查 partner_promotion_key 是否为 google-official
        if let Some(key) = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.partner_promotion_key.as_deref())
        {
            if key.eq_ignore_ascii_case(Self::GOOGLE_OFFICIAL_PARTNER_KEY) {
                return GeminiAuthType::GoogleOfficial;
            }
        }

        // 检查名称是否为 Google
        let name_lower = provider.name.to_ascii_lowercase();
        if name_lower == "google" || name_lower.starts_with("google ") {
            return GeminiAuthType::GoogleOfficial;
        }

        // 其他所有情况：API Key 认证
        GeminiAuthType::ApiKey
    }

    /// 确保 Google 官方 Gemini 供应商的安全标志正确设置（OAuth 模式）
    ///
    /// Google 官方 Gemini 使用 OAuth 个人认证，不需要 API Key。
    ///
    /// # 写入两处 settings.json 的原因
    ///
    /// 1. **`~/.cc-switch/settings.json`** (应用级配置):
    ///    - CC-Switch 应用的全局设置
    ///    - 确保应用知道当前使用的认证类型
    ///    - 用于 UI 显示和其他应用逻辑
    ///
    /// 2. **`~/.gemini/settings.json`** (Gemini 客户端配置):
    ///    - Gemini CLI 客户端读取的配置文件
    ///    - 直接影响 Gemini 客户端的认证行为
    ///    - 确保 Gemini 使用正确的认证方式连接 API
    ///
    /// # 设置的值
    ///
    /// ```json
    /// {
    ///   "security": {
    ///     "auth": {
    ///       "selectedType": "oauth-personal"
    ///     }
    ///   }
    /// }
    /// ```
    ///
    /// # OAuth 认证流程
    ///
    /// 1. 用户切换到 Google 官方供应商
    /// 2. CC-Switch 设置 `selectedType = "oauth-personal"`
    /// 3. 用户首次使用 Gemini CLI 时，会自动打开浏览器进行 OAuth 登录
    /// 4. 登录成功后，凭证保存在 Gemini 的 credential store 中
    /// 5. 后续请求自动使用保存的凭证
    pub(crate) fn ensure_google_oauth_security_flag(provider: &Provider) -> Result<(), AppError> {
        // 检测是否为 Google 官方
        let auth_type = Self::detect_gemini_auth_type(provider);
        if auth_type != GeminiAuthType::GoogleOfficial {
            return Ok(());
        }

        // 写入应用级别的 settings.json (~/.cc-switch/settings.json)
        settings::ensure_security_auth_selected_type(Self::GOOGLE_OAUTH_SECURITY_SELECTED_TYPE)?;

        // 写入 Gemini 目录的 settings.json (~/.gemini/settings.json)
        use crate::gemini_config::write_google_oauth_settings;
        write_google_oauth_settings()?;

        Ok(())
    }

    /// 确保 API Key 供应商的安全标志正确设置
    ///
    /// 此函数适用于所有使用 API Key 认证的 Gemini 供应商，包括：
    /// - PackyCode（合作伙伴）
    /// - 其他第三方 Gemini API 服务
    ///
    /// 所有 API Key 供应商使用相同的认证方式和配置逻辑。
    ///
    /// # 设置的值
    ///
    /// ```json
    /// {
    ///   "security": {
    ///     "auth": {
    ///       "selectedType": "gemini-api-key"
    ///     }
    ///   }
    /// }
    /// ```
    pub(crate) fn ensure_api_key_security_flag(_provider: &Provider) -> Result<(), AppError> {
        // 写入应用级别的 settings.json (~/.cc-switch/settings.json)
        settings::ensure_security_auth_selected_type(Self::API_KEY_SECURITY_SELECTED_TYPE)?;

        // 写入 Gemini 目录的 settings.json (~/.gemini/settings.json)
        use crate::gemini_config::write_generic_settings;
        write_generic_settings()?;

        Ok(())
    }

    /// 归一化 Claude 模型键：读旧键(ANTHROPIC_SMALL_FAST_MODEL)，写新键(DEFAULT_*), 并删除旧键
    fn normalize_claude_models_in_value(settings: &mut Value) -> bool {
        let mut changed = false;
        let env = match settings.get_mut("env") {
            Some(v) if v.is_object() => v.as_object_mut().unwrap(),
            _ => return changed,
        };

        let model = env
            .get("ANTHROPIC_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let small_fast = env
            .get("ANTHROPIC_SMALL_FAST_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let current_haiku = env
            .get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let current_sonnet = env
            .get("ANTHROPIC_DEFAULT_SONNET_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let current_opus = env
            .get("ANTHROPIC_DEFAULT_OPUS_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let target_haiku = current_haiku
            .or_else(|| small_fast.clone())
            .or_else(|| model.clone());
        let target_sonnet = current_sonnet
            .or_else(|| model.clone())
            .or_else(|| small_fast.clone());
        let target_opus = current_opus
            .or_else(|| model.clone())
            .or_else(|| small_fast.clone());

        if env.get("ANTHROPIC_DEFAULT_HAIKU_MODEL").is_none() {
            if let Some(v) = target_haiku {
                env.insert(
                    "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(),
                    Value::String(v),
                );
                changed = true;
            }
        }
        if env.get("ANTHROPIC_DEFAULT_SONNET_MODEL").is_none() {
            if let Some(v) = target_sonnet {
                env.insert(
                    "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
                    Value::String(v),
                );
                changed = true;
            }
        }
        if env.get("ANTHROPIC_DEFAULT_OPUS_MODEL").is_none() {
            if let Some(v) = target_opus {
                env.insert("ANTHROPIC_DEFAULT_OPUS_MODEL".to_string(), Value::String(v));
                changed = true;
            }
        }

        if env.remove("ANTHROPIC_SMALL_FAST_MODEL").is_some() {
            changed = true;
        }

        changed
    }

    fn normalize_provider_if_claude(app_type: &AppType, provider: &mut Provider) {
        if matches!(app_type, AppType::Claude) {
            let mut v = provider.settings_config.clone();
            if Self::normalize_claude_models_in_value(&mut v) {
                provider.settings_config = v;
            }
        }
    }
    fn run_transaction<R, F>(state: &AppState, f: F) -> Result<R, AppError>
    where
        F: FnOnce(&mut MultiAppConfig) -> Result<(R, Option<PostCommitAction>), AppError>,
    {
        let mut guard = state.config.write().map_err(AppError::from)?;
        let original = guard.clone();
        let (result, action) = match f(&mut guard) {
            Ok(value) => value,
            Err(err) => {
                *guard = original;
                return Err(err);
            }
        };
        drop(guard);

        if let Err(save_err) = state.save() {
            if let Err(rollback_err) = Self::restore_config_only(state, original.clone()) {
                return Err(AppError::localized(
                    "config.save.rollback_failed",
                    format!("保存配置失败: {save_err}；回滚失败: {rollback_err}"),
                    format!("Failed to save config: {save_err}; rollback failed: {rollback_err}"),
                ));
            }
            return Err(save_err);
        }

        if let Some(action) = action {
            if let Err(err) = Self::apply_post_commit(state, &action) {
                if let Err(rollback_err) =
                    Self::rollback_after_failure(state, original.clone(), action.backup.clone())
                {
                    return Err(AppError::localized(
                        "post_commit.rollback_failed",
                        format!("后置操作失败: {err}；回滚失败: {rollback_err}"),
                        format!("Post-commit step failed: {err}; rollback failed: {rollback_err}"),
                    ));
                }
                return Err(err);
            }
        }

        Ok(result)
    }

    fn restore_config_only(state: &AppState, snapshot: MultiAppConfig) -> Result<(), AppError> {
        {
            let mut guard = state.config.write().map_err(AppError::from)?;
            *guard = snapshot;
        }
        state.save()
    }

    fn rollback_after_failure(
        state: &AppState,
        snapshot: MultiAppConfig,
        backup: LiveSnapshot,
    ) -> Result<(), AppError> {
        Self::restore_config_only(state, snapshot)?;
        backup.restore()
    }

    fn apply_post_commit(state: &AppState, action: &PostCommitAction) -> Result<(), AppError> {
        Self::write_live_snapshot(
            &action.app_type,
            &action.provider,
            action.common_config_snippet.as_deref(),
        )?;
        if action.sync_mcp {
            // 使用 v3.7.0 统一的 MCP 同步机制，支持所有应用
            use crate::services::mcp::McpService;
            McpService::sync_all_enabled(state)?;
        }
        if action.refresh_snapshot {
            Self::refresh_provider_snapshot(state, &action.app_type, &action.provider.id)?;
        }
        Ok(())
    }

    fn refresh_provider_snapshot(
        state: &AppState,
        app_type: &AppType,
        provider_id: &str,
    ) -> Result<(), AppError> {
        match app_type {
            AppType::Claude => {
                let settings_path = get_claude_settings_path();
                if !settings_path.exists() {
                    return Err(AppError::localized(
                        "claude.live.missing",
                        "Claude 设置文件不存在，无法刷新快照",
                        "Claude settings file missing; cannot refresh snapshot",
                    ));
                }
                let mut live_after = read_json_file::<Value>(&settings_path)?;
                let _ = Self::normalize_claude_models_in_value(&mut live_after);

                let common_snippet = {
                    let guard = state.config.read().map_err(AppError::from)?;
                    guard.common_config_snippets.claude.clone()
                };
                if let Some(snippet) = common_snippet.as_deref() {
                    let snippet = snippet.trim();
                    if !snippet.is_empty() {
                        let common = Self::parse_common_claude_config_snippet(snippet)?;
                        strip_common_values(&mut live_after, &common);
                    }
                }
                {
                    let mut guard = state.config.write().map_err(AppError::from)?;
                    if let Some(manager) = guard.get_manager_mut(app_type) {
                        if let Some(target) = manager.providers.get_mut(provider_id) {
                            target.settings_config = live_after;
                        }
                    }
                }
                state.save()?;
            }
            AppType::Codex => {
                let auth_path = get_codex_auth_path();
                let auth = if auth_path.exists() {
                    Some(read_json_file::<Value>(&auth_path)?)
                } else {
                    None
                };
                let cfg_text = crate::codex_config::read_and_validate_codex_config_text()?;

                {
                    let mut guard = state.config.write().map_err(AppError::from)?;
                    if let Some(manager) = guard.get_manager_mut(app_type) {
                        if let Some(target) = manager.providers.get_mut(provider_id) {
                            let obj = target.settings_config.as_object_mut().ok_or_else(|| {
                                AppError::Config(format!(
                                    "供应商 {provider_id} 的 Codex 配置必须是 JSON 对象"
                                ))
                            })?;
                            if let Some(auth) = auth {
                                obj.insert("auth".to_string(), auth);
                            }
                            obj.insert("config".to_string(), Value::String(cfg_text.clone()));
                        }
                    }
                }
                state.save()?;
            }
            AppType::Gemini => {
                use crate::gemini_config::{
                    env_to_json, get_gemini_env_path, get_gemini_settings_path, read_gemini_env,
                };

                let env_path = get_gemini_env_path();
                if !env_path.exists() {
                    return Err(AppError::localized(
                        "gemini.live.missing",
                        "Gemini .env 文件不存在，无法刷新快照",
                        "Gemini .env file missing; cannot refresh snapshot",
                    ));
                }
                let env_map = read_gemini_env()?;
                let mut live_after = env_to_json(&env_map);

                let settings_path = get_gemini_settings_path();
                let config_value = if settings_path.exists() {
                    read_json_file(&settings_path)?
                } else {
                    json!({})
                };

                if let Some(obj) = live_after.as_object_mut() {
                    obj.insert("config".to_string(), config_value);
                }

                let common_snippet = {
                    let guard = state.config.read().map_err(AppError::from)?;
                    guard.common_config_snippets.gemini.clone()
                };
                if let Some(snippet) = common_snippet.as_deref() {
                    let snippet = snippet.trim();
                    if !snippet.is_empty() {
                        let common = Self::parse_common_gemini_config_snippet(snippet)?;
                        strip_common_values(&mut live_after, &common);
                    }
                }

                {
                    let mut guard = state.config.write().map_err(AppError::from)?;
                    if let Some(manager) = guard.get_manager_mut(app_type) {
                        if let Some(target) = manager.providers.get_mut(provider_id) {
                            target.settings_config = live_after;
                        }
                    }
                }
                state.save()?;
            }
        }
        Ok(())
    }

    fn capture_live_snapshot(app_type: &AppType) -> Result<LiveSnapshot, AppError> {
        match app_type {
            AppType::Claude => {
                let path = get_claude_settings_path();
                let settings = if path.exists() {
                    Some(read_json_file::<Value>(&path)?)
                } else {
                    None
                };
                Ok(LiveSnapshot::Claude { settings })
            }
            AppType::Codex => {
                let auth_path = get_codex_auth_path();
                let config_path = get_codex_config_path();
                let auth = if auth_path.exists() {
                    Some(read_json_file::<Value>(&auth_path)?)
                } else {
                    None
                };
                let config = if config_path.exists() {
                    Some(
                        std::fs::read_to_string(&config_path)
                            .map_err(|e| AppError::io(&config_path, e))?,
                    )
                } else {
                    None
                };
                Ok(LiveSnapshot::Codex { auth, config })
            }
            AppType::Gemini => {
                // 新增
                use crate::gemini_config::{
                    get_gemini_env_path, get_gemini_settings_path, read_gemini_env,
                };
                let path = get_gemini_env_path();
                let env = if path.exists() {
                    Some(read_gemini_env()?)
                } else {
                    None
                };
                let settings_path = get_gemini_settings_path();
                let config = if settings_path.exists() {
                    Some(read_json_file(&settings_path)?)
                } else {
                    None
                };
                Ok(LiveSnapshot::Gemini { env, config })
            }
        }
    }

    /// 列出指定应用下的所有供应商
    pub fn list(
        state: &AppState,
        app_type: AppType,
    ) -> Result<HashMap<String, Provider>, AppError> {
        let config = state.config.read().map_err(AppError::from)?;
        let manager = config
            .get_manager(&app_type)
            .ok_or_else(|| Self::app_not_found(&app_type))?;
        Ok(manager.get_all_providers().clone())
    }

    /// 获取当前供应商 ID
    pub fn current(state: &AppState, app_type: AppType) -> Result<String, AppError> {
        let config = state.config.read().map_err(AppError::from)?;
        let manager = config
            .get_manager(&app_type)
            .ok_or_else(|| Self::app_not_found(&app_type))?;
        Ok(manager.current.clone())
    }

    /// 新增供应商
    pub fn add(state: &AppState, app_type: AppType, provider: Provider) -> Result<bool, AppError> {
        let mut provider = provider;
        // 归一化 Claude 模型键
        Self::normalize_provider_if_claude(&app_type, &mut provider);
        Self::validate_provider_settings(&app_type, &provider)?;

        let app_type_clone = app_type.clone();
        let provider_clone = provider.clone();

        Self::run_transaction(state, move |config| {
            config.ensure_app(&app_type_clone);
            let manager = config
                .get_manager_mut(&app_type_clone)
                .ok_or_else(|| Self::app_not_found(&app_type_clone))?;

            let was_empty = manager.providers.is_empty();
            manager
                .providers
                .insert(provider_clone.id.clone(), provider_clone.clone());

            if was_empty && manager.current.is_empty() {
                manager.current = provider_clone.id.clone();
            }

            let is_current = manager.current == provider_clone.id;
            let action = if is_current {
                let backup = Self::capture_live_snapshot(&app_type_clone)?;
                let common_config_snippet =
                    config.common_config_snippets.get(&app_type_clone).cloned();
                Some(PostCommitAction {
                    app_type: app_type_clone.clone(),
                    provider: provider_clone.clone(),
                    backup,
                    sync_mcp: false,
                    refresh_snapshot: false,
                    common_config_snippet,
                })
            } else {
                None
            };

            Ok((true, action))
        })
    }

    /// 更新供应商
    pub fn update(
        state: &AppState,
        app_type: AppType,
        provider: Provider,
    ) -> Result<bool, AppError> {
        let mut provider = provider;
        // 归一化 Claude 模型键
        Self::normalize_provider_if_claude(&app_type, &mut provider);
        Self::validate_provider_settings(&app_type, &provider)?;
        let provider_id = provider.id.clone();
        let app_type_clone = app_type.clone();
        let provider_clone = provider.clone();

        Self::run_transaction(state, move |config| {
            let manager = config
                .get_manager_mut(&app_type_clone)
                .ok_or_else(|| Self::app_not_found(&app_type_clone))?;

            if !manager.providers.contains_key(&provider_id) {
                return Err(AppError::localized(
                    "provider.not_found",
                    format!("供应商不存在: {provider_id}"),
                    format!("Provider not found: {provider_id}"),
                ));
            }

            let is_current = manager.current == provider_id;
            let merged = if let Some(existing) = manager.providers.get(&provider_id) {
                let mut updated = provider_clone.clone();
                match (existing.meta.as_ref(), updated.meta.take()) {
                    // 前端未提供 meta，表示不修改，沿用旧值
                    (Some(old_meta), None) => {
                        updated.meta = Some(old_meta.clone());
                    }
                    (None, None) => {
                        updated.meta = None;
                    }
                    // 前端提供的 meta 视为权威，直接覆盖（其中 custom_endpoints 允许是空，表示删除所有自定义端点）
                    (_old, Some(new_meta)) => {
                        updated.meta = Some(new_meta);
                    }
                }
                updated
            } else {
                provider_clone.clone()
            };

            manager.providers.insert(provider_id.clone(), merged);

            let action = if is_current {
                let backup = Self::capture_live_snapshot(&app_type_clone)?;
                let common_config_snippet =
                    config.common_config_snippets.get(&app_type_clone).cloned();
                Some(PostCommitAction {
                    app_type: app_type_clone.clone(),
                    provider: provider_clone.clone(),
                    backup,
                    sync_mcp: false,
                    refresh_snapshot: false,
                    common_config_snippet,
                })
            } else {
                None
            };

            Ok((true, action))
        })
    }

    /// 导入当前 live 配置为默认供应商
    pub fn import_default_config(state: &AppState, app_type: AppType) -> Result<(), AppError> {
        {
            let config = state.config.read().map_err(AppError::from)?;
            if let Some(manager) = config.get_manager(&app_type) {
                if !manager.get_all_providers().is_empty() {
                    return Ok(());
                }
            }
        }

        let settings_config = match app_type {
            AppType::Codex => {
                let auth_path = get_codex_auth_path();
                if !auth_path.exists() {
                    return Err(AppError::localized(
                        "codex.live.missing",
                        "Codex 配置文件不存在",
                        "Codex configuration file is missing",
                    ));
                }
                let auth: Value = read_json_file(&auth_path)?;
                let config_str = crate::codex_config::read_and_validate_codex_config_text()?;
                json!({ "auth": auth, "config": config_str })
            }
            AppType::Claude => {
                let settings_path = get_claude_settings_path();
                if !settings_path.exists() {
                    return Err(AppError::localized(
                        "claude.live.missing",
                        "Claude Code 配置文件不存在",
                        "Claude settings file is missing",
                    ));
                }
                let mut v = read_json_file::<Value>(&settings_path)?;
                let _ = Self::normalize_claude_models_in_value(&mut v);
                v
            }
            AppType::Gemini => {
                use crate::gemini_config::{
                    env_to_json, get_gemini_env_path, get_gemini_settings_path, read_gemini_env,
                };

                // 读取 .env 文件（环境变量）
                let env_path = get_gemini_env_path();
                if !env_path.exists() {
                    return Err(AppError::localized(
                        "gemini.live.missing",
                        "Gemini 配置文件不存在",
                        "Gemini configuration file is missing",
                    ));
                }

                let env_map = read_gemini_env()?;
                let env_json = env_to_json(&env_map);
                let env_obj = env_json.get("env").cloned().unwrap_or_else(|| json!({}));

                // 读取 settings.json 文件（MCP 配置等）
                let settings_path = get_gemini_settings_path();
                let config_obj = if settings_path.exists() {
                    read_json_file(&settings_path)?
                } else {
                    json!({})
                };

                // 返回完整结构：{ "env": {...}, "config": {...} }
                json!({
                    "env": env_obj,
                    "config": config_obj
                })
            }
        };

        let mut provider = Provider::with_id(
            "default".to_string(),
            "default".to_string(),
            settings_config,
            None,
        );
        provider.category = Some("custom".to_string());

        {
            let mut config = state.config.write().map_err(AppError::from)?;
            let manager = config
                .get_manager_mut(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;
            manager
                .providers
                .insert(provider.id.clone(), provider.clone());
            manager.current = provider.id.clone();
        }

        state.save()?;
        Ok(())
    }

    /// 读取当前 live 配置
    pub fn read_live_settings(app_type: AppType) -> Result<Value, AppError> {
        match app_type {
            AppType::Codex => {
                let auth_path = get_codex_auth_path();
                if !auth_path.exists() {
                    return Err(AppError::localized(
                        "codex.auth.missing",
                        "Codex 配置文件不存在：缺少 auth.json",
                        "Codex configuration missing: auth.json not found",
                    ));
                }
                let auth: Value = read_json_file(&auth_path)?;
                let cfg_text = crate::codex_config::read_and_validate_codex_config_text()?;
                Ok(json!({ "auth": auth, "config": cfg_text }))
            }
            AppType::Claude => {
                let path = get_claude_settings_path();
                if !path.exists() {
                    return Err(AppError::localized(
                        "claude.live.missing",
                        "Claude Code 配置文件不存在",
                        "Claude settings file is missing",
                    ));
                }
                read_json_file(&path)
            }
            AppType::Gemini => {
                use crate::gemini_config::{
                    env_to_json, get_gemini_env_path, get_gemini_settings_path, read_gemini_env,
                };

                // 读取 .env 文件（环境变量）
                let env_path = get_gemini_env_path();
                if !env_path.exists() {
                    return Err(AppError::localized(
                        "gemini.env.missing",
                        "Gemini .env 文件不存在",
                        "Gemini .env file not found",
                    ));
                }

                let env_map = read_gemini_env()?;
                let env_json = env_to_json(&env_map);
                let env_obj = env_json.get("env").cloned().unwrap_or_else(|| json!({}));

                // 读取 settings.json 文件（MCP 配置等）
                let settings_path = get_gemini_settings_path();
                let config_obj = if settings_path.exists() {
                    read_json_file(&settings_path)?
                } else {
                    json!({})
                };

                // 返回完整结构：{ "env": {...}, "config": {...} }
                Ok(json!({
                    "env": env_obj,
                    "config": config_obj
                }))
            }
        }
    }

    /// 获取自定义端点列表
    pub fn get_custom_endpoints(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
    ) -> Result<Vec<CustomEndpoint>, AppError> {
        let cfg = state.config.read().map_err(AppError::from)?;
        let manager = cfg
            .get_manager(&app_type)
            .ok_or_else(|| Self::app_not_found(&app_type))?;

        let Some(provider) = manager.providers.get(provider_id) else {
            return Ok(vec![]);
        };
        let Some(meta) = provider.meta.as_ref() else {
            return Ok(vec![]);
        };
        if meta.custom_endpoints.is_empty() {
            return Ok(vec![]);
        }

        let mut result: Vec<_> = meta.custom_endpoints.values().cloned().collect();
        result.sort_by(|a, b| b.added_at.cmp(&a.added_at));
        Ok(result)
    }

    /// 新增自定义端点
    pub fn add_custom_endpoint(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
        url: String,
    ) -> Result<(), AppError> {
        let normalized = url.trim().trim_end_matches('/').to_string();
        if normalized.is_empty() {
            return Err(AppError::localized(
                "provider.endpoint.url_required",
                "URL 不能为空",
                "URL cannot be empty",
            ));
        }

        {
            let mut cfg = state.config.write().map_err(AppError::from)?;
            let manager = cfg
                .get_manager_mut(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;
            let provider = manager.providers.get_mut(provider_id).ok_or_else(|| {
                AppError::localized(
                    "provider.not_found",
                    format!("供应商不存在: {provider_id}"),
                    format!("Provider not found: {provider_id}"),
                )
            })?;
            let meta = provider.meta.get_or_insert_with(ProviderMeta::default);

            let endpoint = CustomEndpoint {
                url: normalized.clone(),
                added_at: Self::now_millis(),
                last_used: None,
            };
            meta.custom_endpoints.insert(normalized, endpoint);
        }

        state.save()?;
        Ok(())
    }

    /// 删除自定义端点
    pub fn remove_custom_endpoint(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
        url: String,
    ) -> Result<(), AppError> {
        let normalized = url.trim().trim_end_matches('/').to_string();

        {
            let mut cfg = state.config.write().map_err(AppError::from)?;
            if let Some(manager) = cfg.get_manager_mut(&app_type) {
                if let Some(provider) = manager.providers.get_mut(provider_id) {
                    if let Some(meta) = provider.meta.as_mut() {
                        meta.custom_endpoints.remove(&normalized);
                    }
                }
            }
        }

        state.save()?;
        Ok(())
    }

    /// 更新端点最后使用时间
    pub fn update_endpoint_last_used(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
        url: String,
    ) -> Result<(), AppError> {
        let normalized = url.trim().trim_end_matches('/').to_string();

        {
            let mut cfg = state.config.write().map_err(AppError::from)?;
            if let Some(manager) = cfg.get_manager_mut(&app_type) {
                if let Some(provider) = manager.providers.get_mut(provider_id) {
                    if let Some(meta) = provider.meta.as_mut() {
                        if let Some(endpoint) = meta.custom_endpoints.get_mut(&normalized) {
                            endpoint.last_used = Some(Self::now_millis());
                        }
                    }
                }
            }
        }

        state.save()?;
        Ok(())
    }

    /// 更新供应商排序
    pub fn update_sort_order(
        state: &AppState,
        app_type: AppType,
        updates: Vec<ProviderSortUpdate>,
    ) -> Result<bool, AppError> {
        {
            let mut cfg = state.config.write().map_err(AppError::from)?;
            let manager = cfg
                .get_manager_mut(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;

            for update in updates {
                if let Some(provider) = manager.providers.get_mut(&update.id) {
                    provider.sort_index = Some(update.sort_index);
                }
            }
        }

        state.save()?;
        Ok(true)
    }

    /// 执行用量脚本并格式化结果（私有辅助方法）
    async fn execute_and_format_usage_result(
        script_code: &str,
        api_key: &str,
        base_url: &str,
        timeout: u64,
        access_token: Option<&str>,
        user_id: Option<&str>,
    ) -> Result<UsageResult, AppError> {
        match usage_script::execute_usage_script(
            script_code,
            api_key,
            base_url,
            timeout,
            access_token,
            user_id,
        )
        .await
        {
            Ok(data) => {
                let usage_list: Vec<UsageData> = if data.is_array() {
                    serde_json::from_value(data).map_err(|e| {
                        AppError::localized(
                            "usage_script.data_format_error",
                            format!("数据格式错误: {e}"),
                            format!("Data format error: {e}"),
                        )
                    })?
                } else {
                    let single: UsageData = serde_json::from_value(data).map_err(|e| {
                        AppError::localized(
                            "usage_script.data_format_error",
                            format!("数据格式错误: {e}"),
                            format!("Data format error: {e}"),
                        )
                    })?;
                    vec![single]
                };

                Ok(UsageResult {
                    success: true,
                    data: Some(usage_list),
                    error: None,
                })
            }
            Err(err) => {
                let lang = settings::get_settings()
                    .language
                    .unwrap_or_else(|| "zh".to_string());

                let msg = match err {
                    AppError::Localized { zh, en, .. } => {
                        if lang == "en" {
                            en
                        } else {
                            zh
                        }
                    }
                    other => other.to_string(),
                };

                Ok(UsageResult {
                    success: false,
                    data: None,
                    error: Some(msg),
                })
            }
        }
    }

    /// 查询供应商用量（使用已保存的脚本配置）
    pub async fn query_usage(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
    ) -> Result<UsageResult, AppError> {
        let (script_code, timeout, api_key, base_url, access_token, user_id) = {
            let config = state.config.read().map_err(AppError::from)?;
            let manager = config
                .get_manager(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;
            let provider = manager.providers.get(provider_id).cloned().ok_or_else(|| {
                AppError::localized(
                    "provider.not_found",
                    format!("供应商不存在: {provider_id}"),
                    format!("Provider not found: {provider_id}"),
                )
            })?;

            let usage_script = provider
                .meta
                .as_ref()
                .and_then(|m| m.usage_script.as_ref())
                .ok_or_else(|| {
                    AppError::localized(
                        "provider.usage.script.missing",
                        "未配置用量查询脚本",
                        "Usage script is not configured",
                    )
                })?;
            if !usage_script.enabled {
                return Err(AppError::localized(
                    "provider.usage.disabled",
                    "用量查询未启用",
                    "Usage query is disabled",
                ));
            }

            // 直接从 UsageScript 中获取凭证，不再从供应商配置提取
            (
                usage_script.code.clone(),
                usage_script.timeout.unwrap_or(10),
                usage_script.api_key.clone().unwrap_or_default(),
                usage_script.base_url.clone().unwrap_or_default(),
                usage_script.access_token.clone(),
                usage_script.user_id.clone(),
            )
        };

        Self::execute_and_format_usage_result(
            &script_code,
            &api_key,
            &base_url,
            timeout,
            access_token.as_deref(),
            user_id.as_deref(),
        )
        .await
    }

    /// 测试用量脚本（使用临时脚本内容，不保存）
    #[allow(clippy::too_many_arguments)]
    pub async fn test_usage_script(
        _state: &AppState,
        _app_type: AppType,
        _provider_id: &str,
        script_code: &str,
        timeout: u64,
        api_key: Option<&str>,
        base_url: Option<&str>,
        access_token: Option<&str>,
        user_id: Option<&str>,
    ) -> Result<UsageResult, AppError> {
        // 直接使用传入的凭证参数进行测试
        Self::execute_and_format_usage_result(
            script_code,
            api_key.unwrap_or(""),
            base_url.unwrap_or(""),
            timeout,
            access_token,
            user_id,
        )
        .await
    }

    /// 切换指定应用的供应商
    pub fn switch(state: &AppState, app_type: AppType, provider_id: &str) -> Result<(), AppError> {
        let app_type_clone = app_type.clone();
        let provider_id_owned = provider_id.to_string();

        Self::run_transaction(state, move |config| {
            let backup = Self::capture_live_snapshot(&app_type_clone)?;
            let provider = match app_type_clone {
                AppType::Codex => Self::prepare_switch_codex(config, &provider_id_owned)?,
                AppType::Claude => Self::prepare_switch_claude(config, &provider_id_owned)?,
                AppType::Gemini => Self::prepare_switch_gemini(config, &provider_id_owned)?,
            };

            let action = PostCommitAction {
                app_type: app_type_clone.clone(),
                provider,
                backup,
                sync_mcp: true, // v3.7.0: 所有应用切换时都同步 MCP，防止配置丢失
                refresh_snapshot: true,
                common_config_snippet: config.common_config_snippets.get(&app_type_clone).cloned(),
            };

            Ok(((), Some(action)))
        })
    }

    fn prepare_switch_codex(
        config: &mut MultiAppConfig,
        provider_id: &str,
    ) -> Result<Provider, AppError> {
        let provider = config
            .get_manager(&AppType::Codex)
            .ok_or_else(|| Self::app_not_found(&AppType::Codex))?
            .providers
            .get(provider_id)
            .cloned()
            .ok_or_else(|| {
                AppError::localized(
                    "provider.not_found",
                    format!("供应商不存在: {provider_id}"),
                    format!("Provider not found: {provider_id}"),
                )
            })?;

        Self::backfill_codex_current(config, provider_id)?;

        if let Some(manager) = config.get_manager_mut(&AppType::Codex) {
            manager.current = provider_id.to_string();
        }

        Ok(provider)
    }

    fn backfill_codex_current(
        config: &mut MultiAppConfig,
        next_provider: &str,
    ) -> Result<(), AppError> {
        let current_id = config
            .get_manager(&AppType::Codex)
            .map(|m| m.current.clone())
            .unwrap_or_default();

        if current_id.is_empty() || current_id == next_provider {
            return Ok(());
        }

        let auth_path = get_codex_auth_path();
        if !auth_path.exists() {
            return Ok(());
        }

        let auth: Value = read_json_file(&auth_path)?;
        let config_path = get_codex_config_path();
        let config_text = if config_path.exists() {
            std::fs::read_to_string(&config_path).map_err(|e| AppError::io(&config_path, e))?
        } else {
            String::new()
        };

        let live = json!({
            "auth": auth,
            "config": config_text,
        });

        if let Some(manager) = config.get_manager_mut(&AppType::Codex) {
            if let Some(current) = manager.providers.get_mut(&current_id) {
                current.settings_config = live;
            }
        }

        Ok(())
    }

    fn write_codex_live(provider: &Provider) -> Result<(), AppError> {
        use toml_edit::{value, Item, Table};

        let settings = provider
            .settings_config
            .as_object()
            .ok_or_else(|| AppError::Config("Codex 配置必须是 JSON 对象".into()))?;

        // auth 字段现在是可选的（Codex 0.64+ 使用环境变量）
        let auth = settings.get("auth");
        let auth_is_empty = auth
            .map(|a| a.as_object().map(|o| o.is_empty()).unwrap_or(true))
            .unwrap_or(true);

        // 获取存储的 config TOML 文本
        let cfg_text = settings.get("config").and_then(Value::as_str).unwrap_or("");

        // 解析存储的 TOML 以提取字段（如果为空则使用默认值）
        let stored_config: toml::Table = if cfg_text.trim().is_empty() {
            toml::Table::new()
        } else {
            toml::from_str(cfg_text).map_err(|e| {
                AppError::Config(format!(
                    "解析供应商 {} 的 config TOML 失败: {}",
                    provider.id, e
                ))
            })?
        };

        // 提取必要字段
        let base_url = stored_config
            .get("base_url")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let model = stored_config
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("gpt-5.2-codex");
        let wire_api = stored_config
            .get("wire_api")
            .and_then(|v| v.as_str())
            .unwrap_or("chat"); // 默认 'chat'
        let env_key = stored_config
            .get("env_key")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let inferred_requires_openai_auth = env_key == Some("OPENAI_API_KEY") && !auth_is_empty;
        let requires_openai_auth = stored_config
            .get("requires_openai_auth")
            .and_then(|v| v.as_bool())
            .unwrap_or(inferred_requires_openai_auth);

        // 从供应商名称生成 provider ID
        let provider_id = generate_provider_id_from_name(&provider.name);

        // 读取现有 config.toml（保留 MCP 服务器等其他配置）
        let base_text = crate::codex_config::read_and_validate_codex_config_text()?;
        let mut doc = if base_text.trim().is_empty() {
            toml_edit::DocumentMut::default()
        } else {
            base_text
                .parse::<toml_edit::DocumentMut>()
                .map_err(|e| AppError::Config(format!("解析 config.toml 失败: {}", e)))?
        };

        // 移除不应该在根级别的字段
        doc.as_table_mut().remove("base_url");
        doc.as_table_mut().remove("wire_api");
        doc.as_table_mut().remove("env_key");
        doc.as_table_mut().remove("requires_openai_auth");

        // 设置根级别字段
        doc["model_provider"] = value(&provider_id);
        doc["model"] = value(model);

        // 从 stored_config 复制其他根级别字段（如果有）
        for (key, val) in stored_config.iter() {
            match key.as_str() {
                // 跳过已处理的字段和应该在 provider section 的字段
                "base_url" | "wire_api" | "env_key" | "requires_openai_auth" | "name" => continue,
                "model" => continue, // 已在上面设置
                // 复制其他根级别字段（如 model_reasoning_effort, network_access 等）
                _ => {
                    if let Some(toml_val) = Self::toml_value_to_toml_edit_value(val) {
                        doc[key] = Item::Value(toml_val);
                    }
                }
            }
        }

        // 构建 [model_providers.<id>] 表
        let mut provider_table = Table::new();
        provider_table["name"] = value(&provider_id);
        if !base_url.is_empty() {
            provider_table["base_url"] = value(base_url);
        }
        provider_table["wire_api"] = value(wire_api);
        if requires_openai_auth {
            provider_table["requires_openai_auth"] = value(true);
        } else if let Some(env_key) = env_key {
            provider_table["env_key"] = value(env_key);
        }

        // 确保 model_providers 表存在
        if !doc.contains_key("model_providers") {
            doc["model_providers"] = Item::Table(Table::new());
        }

        // 设置 provider
        if let Some(providers_item) = doc.get_mut("model_providers") {
            if let Some(providers_table) = providers_item.as_table_like_mut() {
                providers_table.insert(&provider_id, Item::Table(provider_table));
            }
        }

        // 写回 config.toml
        let new_text = doc.to_string();
        let config_path = get_codex_config_path();
        crate::config::write_text_file(&config_path, &new_text)?;

        // 只在 auth 非空时写入 auth.json（Codex 0.64+ 使用环境变量，不需要 auth.json）
        if !auth_is_empty {
            if let Some(auth_value) = auth {
                let auth_path = get_codex_auth_path();
                write_json_file(&auth_path, auth_value)?;
            }
        }

        Ok(())
    }

    /// 将 toml::Value 转换为 toml_edit::Value
    fn toml_value_to_toml_edit_value(val: &toml::Value) -> Option<toml_edit::Value> {
        use toml_edit::Value as EditValue;
        match val {
            toml::Value::String(s) => Some(EditValue::from(s.as_str())),
            toml::Value::Integer(i) => Some(EditValue::from(*i)),
            toml::Value::Float(f) => Some(EditValue::from(*f)),
            toml::Value::Boolean(b) => Some(EditValue::from(*b)),
            _ => None, // 暂不处理数组和表
        }
    }

    fn prepare_switch_claude(
        config: &mut MultiAppConfig,
        provider_id: &str,
    ) -> Result<Provider, AppError> {
        let provider = config
            .get_manager(&AppType::Claude)
            .ok_or_else(|| Self::app_not_found(&AppType::Claude))?
            .providers
            .get(provider_id)
            .cloned()
            .ok_or_else(|| {
                AppError::localized(
                    "provider.not_found",
                    format!("供应商不存在: {provider_id}"),
                    format!("Provider not found: {provider_id}"),
                )
            })?;

        Self::backfill_claude_current(config, provider_id)?;

        if let Some(manager) = config.get_manager_mut(&AppType::Claude) {
            manager.current = provider_id.to_string();
        }

        Ok(provider)
    }

    fn prepare_switch_gemini(
        config: &mut MultiAppConfig,
        provider_id: &str,
    ) -> Result<Provider, AppError> {
        let provider = config
            .get_manager(&AppType::Gemini)
            .ok_or_else(|| Self::app_not_found(&AppType::Gemini))?
            .providers
            .get(provider_id)
            .cloned()
            .ok_or_else(|| {
                AppError::localized(
                    "provider.not_found",
                    format!("供应商不存在: {provider_id}"),
                    format!("Provider not found: {provider_id}"),
                )
            })?;

        Self::backfill_gemini_current(config, provider_id)?;

        if let Some(manager) = config.get_manager_mut(&AppType::Gemini) {
            manager.current = provider_id.to_string();
        }

        Ok(provider)
    }

    fn backfill_claude_current(
        config: &mut MultiAppConfig,
        next_provider: &str,
    ) -> Result<(), AppError> {
        let settings_path = get_claude_settings_path();
        if !settings_path.exists() {
            return Ok(());
        }

        let current_id = config
            .get_manager(&AppType::Claude)
            .map(|m| m.current.clone())
            .unwrap_or_default();
        if current_id.is_empty() || current_id == next_provider {
            return Ok(());
        }

        let mut live = read_json_file::<Value>(&settings_path)?;
        let _ = Self::normalize_claude_models_in_value(&mut live);
        if let Some(snippet) = config.common_config_snippets.claude.as_deref() {
            let snippet = snippet.trim();
            if !snippet.is_empty() {
                let common = Self::parse_common_claude_config_snippet(snippet)?;
                strip_common_values(&mut live, &common);
            }
        }
        if let Some(manager) = config.get_manager_mut(&AppType::Claude) {
            if let Some(current) = manager.providers.get_mut(&current_id) {
                current.settings_config = live;
            }
        }

        Ok(())
    }

    fn backfill_gemini_current(
        config: &mut MultiAppConfig,
        next_provider: &str,
    ) -> Result<(), AppError> {
        use crate::gemini_config::{
            env_to_json, get_gemini_env_path, get_gemini_settings_path, read_gemini_env,
        };

        let env_path = get_gemini_env_path();
        if !env_path.exists() {
            return Ok(());
        }

        let current_id = config
            .get_manager(&AppType::Gemini)
            .map(|m| m.current.clone())
            .unwrap_or_default();
        if current_id.is_empty() || current_id == next_provider {
            return Ok(());
        }

        let env_map = read_gemini_env()?;
        let mut live = env_to_json(&env_map);

        let settings_path = get_gemini_settings_path();
        let config_value = if settings_path.exists() {
            read_json_file(&settings_path)?
        } else {
            json!({})
        };
        if let Some(obj) = live.as_object_mut() {
            obj.insert("config".to_string(), config_value);
        }

        if let Some(snippet) = config.common_config_snippets.gemini.as_deref() {
            let snippet = snippet.trim();
            if !snippet.is_empty() {
                let common = Self::parse_common_gemini_config_snippet(snippet)?;
                strip_common_values(&mut live, &common);
            }
        }

        if let Some(manager) = config.get_manager_mut(&AppType::Gemini) {
            if let Some(current) = manager.providers.get_mut(&current_id) {
                current.settings_config = live;
            }
        }

        Ok(())
    }

    fn write_claude_live(
        provider: &Provider,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        let settings_path = get_claude_settings_path();
        let mut provider_content = provider.settings_config.clone();
        let _ = Self::normalize_claude_models_in_value(&mut provider_content);

        let content_to_write = if let Some(snippet) = common_config_snippet {
            let snippet = snippet.trim();
            if snippet.is_empty() {
                provider_content
            } else {
                let common = Self::parse_common_claude_config_snippet(snippet)?;
                let mut merged = common;
                merge_json_values(&mut merged, &provider_content);
                let _ = Self::normalize_claude_models_in_value(&mut merged);
                merged
            }
        } else {
            provider_content
        };

        write_json_file(&settings_path, &content_to_write)?;
        Ok(())
    }

    pub(crate) fn write_gemini_live(
        provider: &Provider,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        use crate::gemini_config::{
            get_gemini_settings_path, json_to_env, validate_gemini_settings_strict,
            write_gemini_env_atomic,
        };

        // 一次性检测认证类型，避免重复检测
        let auth_type = Self::detect_gemini_auth_type(provider);

        let provider_content = provider.settings_config.clone();
        let content_to_write = if let Some(snippet) = common_config_snippet {
            let snippet = snippet.trim();
            if snippet.is_empty() {
                provider_content
            } else {
                let common = Self::parse_common_gemini_config_snippet(snippet)?;
                let mut merged = common;
                merge_json_values(&mut merged, &provider_content);
                merged
            }
        } else {
            provider_content
        };

        let mut env_map = json_to_env(&content_to_write)?;

        // 准备要写入 ~/.gemini/settings.json 的配置（缺省时保留现有文件内容）
        let mut config_to_write = if let Some(config_value) = content_to_write.get("config") {
            if config_value.is_null() {
                None // null → 保留现有文件
            } else if config_value.is_object() {
                let obj = config_value.as_object().unwrap();
                if obj.is_empty() {
                    None // 空对象 {} → 保留现有文件
                } else {
                    Some(config_value.clone()) // 有内容 → 才替换
                }
            } else {
                return Err(AppError::localized(
                    "gemini.validation.invalid_config",
                    "Gemini 配置格式错误: config 必须是对象或 null",
                    "Gemini config invalid: config must be an object or null",
                ));
            }
        } else {
            None
        };

        if config_to_write.is_none() {
            let settings_path = get_gemini_settings_path();
            if settings_path.exists() {
                config_to_write = Some(read_json_file(&settings_path)?);
            } else {
                config_to_write = Some(json!({})); // 新建空配置
            }
        }

        match auth_type {
            GeminiAuthType::GoogleOfficial => {
                // Google 官方使用 OAuth，清空 env
                env_map.clear();
                write_gemini_env_atomic(&env_map)?;
            }
            GeminiAuthType::ApiKey => {
                // API Key 供应商（所有第三方服务）
                // 统一处理：验证配置 + 写入 .env 文件
                validate_gemini_settings_strict(&content_to_write)?;
                write_gemini_env_atomic(&env_map)?;
            }
        }

        if let Some(config_value) = config_to_write {
            let settings_path = get_gemini_settings_path();
            write_json_file(&settings_path, &config_value)?;
        }

        match auth_type {
            GeminiAuthType::GoogleOfficial => Self::ensure_google_oauth_security_flag(provider)?,
            GeminiAuthType::ApiKey => Self::ensure_api_key_security_flag(provider)?,
        }

        Ok(())
    }

    fn write_live_snapshot(
        app_type: &AppType,
        provider: &Provider,
        common_config_snippet: Option<&str>,
    ) -> Result<(), AppError> {
        match app_type {
            AppType::Codex => Self::write_codex_live(provider),
            AppType::Claude => Self::write_claude_live(provider, common_config_snippet),
            AppType::Gemini => Self::write_gemini_live(provider, common_config_snippet), // 新增
        }
    }

    fn validate_provider_settings(app_type: &AppType, provider: &Provider) -> Result<(), AppError> {
        match app_type {
            AppType::Claude => {
                if !provider.settings_config.is_object() {
                    return Err(AppError::localized(
                        "provider.claude.settings.not_object",
                        "Claude 配置必须是 JSON 对象",
                        "Claude configuration must be a JSON object",
                    ));
                }
            }
            AppType::Codex => {
                let settings = provider.settings_config.as_object().ok_or_else(|| {
                    AppError::localized(
                        "provider.codex.settings.not_object",
                        "Codex 配置必须是 JSON 对象",
                        "Codex configuration must be a JSON object",
                    )
                })?;

                // auth 字段现在是可选的（Codex 0.64+ 使用环境变量）
                // 如果存在，必须是对象
                if let Some(auth) = settings.get("auth") {
                    if !auth.is_object() {
                        return Err(AppError::localized(
                            "provider.codex.auth.not_object",
                            format!("供应商 {} 的 auth 配置必须是 JSON 对象", provider.id),
                            format!(
                                "Provider {} auth configuration must be a JSON object",
                                provider.id
                            ),
                        ));
                    }
                }

                if let Some(config_value) = settings.get("config") {
                    if !(config_value.is_string() || config_value.is_null()) {
                        return Err(AppError::localized(
                            "provider.codex.config.invalid_type",
                            "Codex config 字段必须是字符串",
                            "Codex config field must be a string",
                        ));
                    }
                    if let Some(cfg_text) = config_value.as_str() {
                        crate::codex_config::validate_config_toml(cfg_text)?;
                    }
                }
            }
            AppType::Gemini => {
                // 新增
                use crate::gemini_config::validate_gemini_settings;
                validate_gemini_settings(&provider.settings_config)?
            }
        }

        // 🔧 验证并清理 UsageScript 配置（所有应用类型通用）
        if let Some(meta) = &provider.meta {
            if let Some(usage_script) = &meta.usage_script {
                Self::validate_usage_script(usage_script)?;
            }
        }

        Ok(())
    }

    /// 验证 UsageScript 配置（边界检查）
    fn validate_usage_script(script: &crate::provider::UsageScript) -> Result<(), AppError> {
        // 验证自动查询间隔 (0-1440 分钟，即最大24小时)
        if let Some(interval) = script.auto_query_interval {
            if interval > 1440 {
                return Err(AppError::localized(
                    "usage_script.interval_too_large",
                    format!(
                        "自动查询间隔不能超过 1440 分钟（24小时），当前值: {interval}"
                    ),
                    format!(
                        "Auto query interval cannot exceed 1440 minutes (24 hours), current: {interval}"
                    ),
                ));
            }
        }

        Ok(())
    }

    #[allow(dead_code)]
    fn extract_credentials(
        provider: &Provider,
        app_type: &AppType,
    ) -> Result<(String, String), AppError> {
        match app_type {
            AppType::Claude => {
                let env = provider
                    .settings_config
                    .get("env")
                    .and_then(|v| v.as_object())
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.claude.env.missing",
                            "配置格式错误: 缺少 env",
                            "Invalid configuration: missing env section",
                        )
                    })?;

                let api_key = env
                    .get("ANTHROPIC_AUTH_TOKEN")
                    .or_else(|| env.get("ANTHROPIC_API_KEY"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.claude.api_key.missing",
                            "缺少 API Key",
                            "API key is missing",
                        )
                    })?
                    .to_string();

                let base_url = env
                    .get("ANTHROPIC_BASE_URL")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.claude.base_url.missing",
                            "缺少 ANTHROPIC_BASE_URL 配置",
                            "Missing ANTHROPIC_BASE_URL configuration",
                        )
                    })?
                    .to_string();

                Ok((api_key, base_url))
            }
            AppType::Codex => {
                let auth = provider
                    .settings_config
                    .get("auth")
                    .and_then(|v| v.as_object())
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.codex.auth.missing",
                            "配置格式错误: 缺少 auth",
                            "Invalid configuration: missing auth section",
                        )
                    })?;

                let api_key = auth
                    .get("OPENAI_API_KEY")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.codex.api_key.missing",
                            "缺少 API Key",
                            "API key is missing",
                        )
                    })?
                    .to_string();

                let config_toml = provider
                    .settings_config
                    .get("config")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let base_url = if config_toml.contains("base_url") {
                    let re = Regex::new(r#"base_url\s*=\s*["']([^"']+)["']"#).map_err(|e| {
                        AppError::localized(
                            "provider.regex_init_failed",
                            format!("正则初始化失败: {e}"),
                            format!("Failed to initialize regex: {e}"),
                        )
                    })?;
                    re.captures(config_toml)
                        .and_then(|caps| caps.get(1))
                        .map(|m| m.as_str().to_string())
                        .ok_or_else(|| {
                            AppError::localized(
                                "provider.codex.base_url.invalid",
                                "config.toml 中 base_url 格式错误",
                                "base_url in config.toml has invalid format",
                            )
                        })?
                } else {
                    return Err(AppError::localized(
                        "provider.codex.base_url.missing",
                        "config.toml 中缺少 base_url 配置",
                        "base_url is missing from config.toml",
                    ));
                };

                Ok((api_key, base_url))
            }
            AppType::Gemini => {
                // 新增
                use crate::gemini_config::json_to_env;

                let env_map = json_to_env(&provider.settings_config)?;

                let api_key = env_map.get("GEMINI_API_KEY").cloned().ok_or_else(|| {
                    AppError::localized(
                        "gemini.missing_api_key",
                        "缺少 GEMINI_API_KEY",
                        "Missing GEMINI_API_KEY",
                    )
                })?;

                let base_url = env_map
                    .get("GOOGLE_GEMINI_BASE_URL")
                    .cloned()
                    .unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string());

                Ok((api_key, base_url))
            }
        }
    }

    fn app_not_found(app_type: &AppType) -> AppError {
        AppError::localized(
            "provider.app_not_found",
            format!("应用类型不存在: {app_type:?}"),
            format!("App type not found: {app_type:?}"),
        )
    }

    fn now_millis() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }

    pub fn delete(state: &AppState, app_type: AppType, provider_id: &str) -> Result<(), AppError> {
        let provider_snapshot = {
            let config = state.config.read().map_err(AppError::from)?;
            let manager = config
                .get_manager(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;

            if manager.current == provider_id {
                return Err(AppError::localized(
                    "provider.delete.current",
                    "不能删除当前正在使用的供应商",
                    "Cannot delete the provider currently in use",
                ));
            }

            manager.providers.get(provider_id).cloned().ok_or_else(|| {
                AppError::localized(
                    "provider.not_found",
                    format!("供应商不存在: {provider_id}"),
                    format!("Provider not found: {provider_id}"),
                )
            })?
        };

        match app_type {
            AppType::Codex => {
                crate::codex_config::delete_codex_provider_config(
                    provider_id,
                    &provider_snapshot.name,
                )?;
            }
            AppType::Claude => {
                // 兼容旧版本：历史上会在 Claude 目录内为每个供应商生成 settings-*.json 副本
                // 这里继续清理这些遗留文件，避免堆积过期配置。
                let by_name = get_provider_config_path(provider_id, Some(&provider_snapshot.name));
                let by_id = get_provider_config_path(provider_id, None);
                delete_file(&by_name)?;
                delete_file(&by_id)?;
            }
            AppType::Gemini => {
                // Gemini 使用单一的 .env 文件，不需要删除单独的供应商配置文件
            }
        }

        {
            let mut config = state.config.write().map_err(AppError::from)?;
            let manager = config
                .get_manager_mut(&app_type)
                .ok_or_else(|| Self::app_not_found(&app_type))?;

            if manager.current == provider_id {
                return Err(AppError::localized(
                    "provider.delete.current",
                    "不能删除当前正在使用的供应商",
                    "Cannot delete the provider currently in use",
                ));
            }

            manager.providers.remove(provider_id);
        }

        state.save()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderSortUpdate {
    pub id: String,
    #[serde(rename = "sortIndex")]
    pub sort_index: usize,
}

#[cfg(test)]
mod codex_openai_auth_tests {
    use super::*;
    use serial_test::serial;
    use std::ffi::OsString;
    use std::path::Path;
    use std::sync::RwLock;
    use tempfile::TempDir;

    struct EnvGuard {
        old_home: Option<OsString>,
        old_userprofile: Option<OsString>,
    }

    impl EnvGuard {
        fn set_home(home: &Path) -> Self {
            let old_home = std::env::var_os("HOME");
            let old_userprofile = std::env::var_os("USERPROFILE");
            std::env::set_var("HOME", home);
            std::env::set_var("USERPROFILE", home);
            Self {
                old_home,
                old_userprofile,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.old_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match &self.old_userprofile {
                Some(value) => std::env::set_var("USERPROFILE", value),
                None => std::env::remove_var("USERPROFILE"),
            }
        }
    }

    #[test]
    #[serial]
    fn switch_codex_provider_uses_openai_auth_instead_of_env_key() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let mut config = MultiAppConfig::default();
        config.ensure_app(&AppType::Codex);
        {
            let manager = config
                .get_manager_mut(&AppType::Codex)
                .expect("codex manager");
            manager.providers.insert(
                "p1".to_string(),
                Provider::with_id(
                    "p1".to_string(),
                    "OpenAI".to_string(),
                    json!({
                        "auth": { "OPENAI_API_KEY": "sk-test" },
                        "config": "base_url = \"https://api.openai.com/v1\"\nmodel = \"gpt-4o\"\nenv_key = \"OPENAI_API_KEY\"\nwire_api = \"chat\""
                    }),
                    None,
                ),
            );
        }

        let state = AppState {
            config: RwLock::new(config),
        };
        ProviderService::switch(&state, AppType::Codex, "p1").expect("switch should succeed");

        let config_text =
            std::fs::read_to_string(get_codex_config_path()).expect("read codex config.toml");
        assert!(
            config_text.contains("requires_openai_auth = true"),
            "config.toml should enable OpenAI auth for Codex model provider"
        );
        assert!(
            !config_text.contains("env_key = \"OPENAI_API_KEY\""),
            "config.toml should not force OPENAI_API_KEY env var by default"
        );
    }
}
