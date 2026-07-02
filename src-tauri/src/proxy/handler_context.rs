use axum::http::HeaderMap;
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app_config::AppType;
use crate::provider::Provider;

use super::{
    error::ProxyError,
    model_mapper::provider_has_explicit_role_mapping,
    provider_router::ProviderRouter,
    server::ProxyServerState,
    session::extract_session_id,
    types::{AppProxyConfig, CopilotOptimizerConfig, OptimizerConfig, RectifierConfig},
};

/// Extract the model identifier from a Gemini API path like
/// `/v1beta/models/gemini-2.5-pro:generateContent` or
/// `/v1/models/gemini-2.5-flash:streamGenerateContent`. Returns `None` if
/// the path does not match the expected `models/<name>[:action]` shape.
fn extract_gemini_model_from_path(path: &str) -> Option<String> {
    // Find the "models/" segment and take what follows up to ":" or end.
    let idx = path.find("/models/")?;
    let after = &path[idx + "/models/".len()..];
    let end = after.find([':', '?', '/']).unwrap_or(after.len());
    let model = &after[..end];
    if model.is_empty() {
        None
    } else {
        Some(model.to_string())
    }
}

pub struct HandlerContext {
    pub start_time: Instant,
    pub state: ProxyServerState,
    pub app_type: AppType,
    pub provider_router: Arc<ProviderRouter>,
    pub route_source: Option<String>,
    providers: Vec<Provider>,
    pub app_proxy: AppProxyConfig,
    pub rectifier_config: RectifierConfig,
    pub optimizer_config: OptimizerConfig,
    pub copilot_optimizer_config: CopilotOptimizerConfig,
    pub request_model: String,
    pub session_id: String,
    pub session_client_provided: bool,
    pub current_provider_id_at_start: String,
}

impl HandlerContext {
    pub async fn load(
        state: &ProxyServerState,
        app_type: AppType,
        headers: &HeaderMap,
        body: &Value,
        path: &str,
    ) -> Result<Self, ProxyError> {
        let _ = crate::settings::reload_settings();
        let current_provider_id_at_start =
            crate::settings::get_effective_current_provider(&state.db, &app_type)
                .ok()
                .flatten()
                .unwrap_or_default();
        state.record_request_start().await;
        let start_time = Instant::now();

        let provider_router = state.provider_router.clone();
        let model_router = state.model_router.clone();
        // Gemini 请求的 model 在 URI 路径中（如 /v1beta/models/gemini-2.5-pro:generateContent），
        // 标准 Claude/Codex/OpenAI 请求的 model 在 JSON body 中。
        let request_model = body
            .get("model")
            .and_then(|value| value.as_str())
            .map(|s| s.to_string())
            .or_else(|| extract_gemini_model_from_path(path))
            .unwrap_or_default();

        let manual_provider = current_provider_id_at_start
            .is_empty()
            .then_some(None)
            .unwrap_or_else(|| {
                state
                    .db
                    .get_provider_by_id(&current_provider_id_at_start, app_type.as_str())
                    .ok()
                    .flatten()
            });

        // A manual Claude provider switch writes role-model mappings into live config
        // (for example client-visible aliases mapped to provider-specific upstream
        // models). Treat that selected provider as the user's active choice and let
        // normal-priority automatic routes yield to it.
        let manual_role_provider = if matches!(app_type, AppType::Claude) {
            manual_provider
                .clone()
                .filter(|provider| provider_has_explicit_role_mapping(provider, &request_model))
        } else {
            None
        };

        // Model route matching first. The router compares generic route priority
        // against the active manual provider choice; it does not special-case model
        // families or provider names.
        let (providers, route_source) = match model_router
            .match_route_respecting_manual_provider(
                app_type.as_str(),
                &request_model,
                manual_role_provider.as_ref(),
            )
            .await
        {
            Ok(Some((_route_id, provider))) => (vec![provider], Some("model_route".to_string())),
            Ok(None) => {
                if let Some(provider) = manual_role_provider {
                    // No model route matched — use manual role mapping as fallback
                    (vec![provider], Some("manual_provider_model".to_string()))
                } else {
                    // RT-04: no match, fallback to existing ProviderRouter
                    let providers = provider_router.select_providers(app_type.as_str()).await?;
                    (providers, None)
                }
            }
            Err(e) => {
                if let Some(provider) = manual_role_provider {
                    log::warn!("model route lookup failed: {e}, using manual role mapping");
                    (vec![provider], Some("manual_provider_model".to_string()))
                } else {
                    // RT-05: match_route error (DB error), log warning and fallback
                    log::warn!("model route lookup failed: {e}, falling back to provider router");
                    let providers = provider_router.select_providers(app_type.as_str()).await?;
                    (providers, None)
                }
            }
        };

        let app_proxy = state
            .db
            .get_proxy_config_for_app(app_type.as_str())
            .await
            .map_err(|error| {
                ProxyError::ConfigError(format!(
                    "load proxy config for {} failed: {error}",
                    app_type.as_str()
                ))
            })?;
        let rectifier_config = state.db.get_rectifier_config().unwrap_or_default();
        let optimizer_config = state.db.get_optimizer_config().unwrap_or_default();
        let copilot_optimizer_config = state.db.get_copilot_optimizer_config().unwrap_or_default();
        let session_result = extract_session_id(headers, body, app_type.as_str());

        Ok(Self {
            start_time,
            state: state.clone(),
            app_type,
            provider_router,
            route_source,
            providers,
            app_proxy,
            rectifier_config,
            optimizer_config,
            copilot_optimizer_config,
            request_model,
            session_id: session_result.session_id,
            session_client_provided: session_result.client_provided,
            current_provider_id_at_start,
        })
    }

    pub fn providers(&self) -> &[Provider] {
        &self.providers
    }

    pub fn primary_provider(&self) -> Option<&Provider> {
        self.providers.first()
    }

    pub fn streaming_first_byte_timeout(&self) -> Option<Duration> {
        if !self.app_proxy.auto_failover_enabled || self.app_proxy.streaming_first_byte_timeout == 0
        {
            return None;
        }

        Some(Duration::from_secs(
            self.app_proxy.streaming_first_byte_timeout as u64,
        ))
    }

    pub fn streaming_idle_timeout(&self) -> Option<Duration> {
        if !self.app_proxy.auto_failover_enabled {
            return None;
        }

        match self.app_proxy.streaming_idle_timeout {
            0 => None,
            seconds => Some(Duration::from_secs(seconds as u64)),
        }
    }

    pub fn non_streaming_timeout(&self) -> Option<Duration> {
        if !self.app_proxy.auto_failover_enabled || self.app_proxy.non_streaming_timeout == 0 {
            return None;
        }

        Some(Duration::from_secs(
            self.app_proxy.non_streaming_timeout as u64,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;
    use serial_test::serial;
    use std::collections::HashMap;
    use std::env;
    use tempfile::TempDir;
    use tokio::sync::RwLock;

    use crate::{
        database::Database,
        proxy::{
            model_router::ModelRouter, providers::gemini_shadow::GeminiShadowStore,
            types::ProxyConfig,
        },
    };

    struct TempHome {
        #[allow(dead_code)]
        dir: TempDir,
        original_home: Option<String>,
        original_userprofile: Option<String>,
        original_config_dir: Option<String>,
    }

    impl TempHome {
        fn new() -> Self {
            let dir = TempDir::new().expect("create temp home");
            let original_home = env::var("HOME").ok();
            let original_userprofile = env::var("USERPROFILE").ok();
            let original_config_dir = env::var("CC_SWITCH_CONFIG_DIR").ok();

            env::set_var("HOME", dir.path());
            env::set_var("USERPROFILE", dir.path());
            env::set_var("CC_SWITCH_CONFIG_DIR", dir.path().join(".cc-switch"));
            crate::settings::reload_test_settings();

            Self {
                dir,
                original_home,
                original_userprofile,
                original_config_dir,
            }
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            match &self.original_home {
                Some(value) => env::set_var("HOME", value),
                None => env::remove_var("HOME"),
            }

            match &self.original_userprofile {
                Some(value) => env::set_var("USERPROFILE", value),
                None => env::remove_var("USERPROFILE"),
            }

            match &self.original_config_dir {
                Some(value) => env::set_var("CC_SWITCH_CONFIG_DIR", value),
                None => env::remove_var("CC_SWITCH_CONFIG_DIR"),
            }

            crate::settings::reload_test_settings();
        }
    }

    fn test_provider(id: &str, sort_index: usize) -> Provider {
        Provider {
            id: id.to_string(),
            name: format!("Provider {id}"),
            settings_config: json!({}),
            website_url: None,
            category: Some("claude".to_string()),
            created_at: None,
            sort_index: Some(sort_index),
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: true,
        }
    }

    fn test_state(db: Arc<Database>) -> ProxyServerState {
        ProxyServerState {
            db: db.clone(),
            config: Arc::new(RwLock::new(ProxyConfig::default())),
            status: Arc::new(RwLock::new(Default::default())),
            start_time: Arc::new(RwLock::new(None)),
            current_providers: Arc::new(RwLock::new(Default::default())),
            provider_router: Arc::new(ProviderRouter::new(db.clone())),
            model_router: Arc::new(ModelRouter::new(db)),
            codex_chat_history: Arc::new(Default::default()),
            gemini_shadow: Arc::new(GeminiShadowStore::default()),
            provider_token_map: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    #[tokio::test]
    #[serial(home_settings)]
    async fn load_uses_current_provider_id_at_request_start() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().expect("create memory database"));
        let current = test_provider("claude-current", 1);
        let failover = test_provider("claude-failover", 0);

        db.save_provider("claude", &current)
            .expect("save current provider");
        db.save_provider("claude", &failover)
            .expect("save failover provider");
        db.set_current_provider("claude", &current.id)
            .expect("set current provider");
        let mut config = db
            .get_proxy_config_for_app("claude")
            .await
            .expect("read app proxy config");
        config.enabled = true;
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config)
            .await
            .expect("enable auto failover");

        let state = test_state(db);
        let context = HandlerContext::load(
            &state,
            AppType::Claude,
            &HeaderMap::new(),
            &json!({"model": "claude-3-7-sonnet-20250219"}),
            "",
        )
        .await
        .expect("load handler context");

        assert_eq!(context.providers()[0].id, "claude-failover");
        assert_eq!(context.current_provider_id_at_start, "claude-current");
    }

    #[tokio::test]
    #[serial(home_settings)]
    async fn load_uses_effective_current_provider_from_settings_at_request_start() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().expect("create memory database"));
        let current = test_provider("claude-current", 1);
        let failover = test_provider("claude-failover", 0);

        db.save_provider("claude", &current)
            .expect("save current provider");
        db.save_provider("claude", &failover)
            .expect("save failover provider");
        db.set_current_provider("claude", &current.id)
            .expect("set current provider in db");
        crate::settings::set_current_provider(&AppType::Claude, Some(&failover.id))
            .expect("set effective current provider in settings");
        let mut config = db
            .get_proxy_config_for_app("claude")
            .await
            .expect("read app proxy config");
        config.enabled = true;
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config)
            .await
            .expect("enable auto failover");

        let state = test_state(db);
        let context = HandlerContext::load(
            &state,
            AppType::Claude,
            &HeaderMap::new(),
            &json!({"model": "claude-3-7-sonnet-20250219"}),
            "",
        )
        .await
        .expect("load handler context");

        assert_eq!(context.current_provider_id_at_start, "claude-failover");
    }

    #[tokio::test]
    #[serial(home_settings)]
    async fn load_captures_current_provider_before_later_awaits() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().expect("create memory database"));
        let current = test_provider("claude-current", 1);
        let failover = test_provider("claude-failover", 0);

        db.save_provider("claude", &current)
            .expect("save current provider");
        db.save_provider("claude", &failover)
            .expect("save failover provider");
        db.set_current_provider("claude", &current.id)
            .expect("set current provider");
        let mut config = db
            .get_proxy_config_for_app("claude")
            .await
            .expect("read app proxy config");
        config.enabled = true;
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config)
            .await
            .expect("enable auto failover");

        let state = test_state(db.clone());
        let status_guard = state.status.write().await;
        let load_task = {
            let state = state.clone();
            tokio::spawn(async move {
                HandlerContext::load(
                    &state,
                    AppType::Claude,
                    &HeaderMap::new(),
                    &json!({"model": "claude-3-7-sonnet-20250219"}),
                    "",
                )
                .await
            })
        };

        tokio::task::yield_now().await;
        db.set_current_provider("claude", &failover.id)
            .expect("switch current provider during blocked request start");
        drop(status_guard);

        let context = load_task
            .await
            .expect("join handler context load")
            .expect("load handler context");

        assert_eq!(context.providers()[0].id, "claude-failover");
        assert_eq!(context.current_provider_id_at_start, "claude-current");
    }

    #[tokio::test]
    #[serial(home_settings)]
    async fn model_route_match_bypasses_failover_queue() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().expect("create memory database"));
        let current = test_provider("claude-current", 1);
        let failover = test_provider("claude-failover", 0);

        db.save_provider("claude", &current)
            .expect("save current provider");
        db.save_provider("claude", &failover)
            .expect("save failover provider");
        db.set_current_provider("claude", &current.id)
            .expect("set current provider");

        // Enable auto failover so select_providers would normally return the queue
        let mut config = db
            .get_proxy_config_for_app("claude")
            .await
            .expect("read app proxy config");
        config.enabled = true;
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config)
            .await
            .expect("enable auto failover");

        // Create model route: pattern "*sonnet*" → claude-current (priority 1)
        use crate::model_route::ModelRoute;
        let route = ModelRoute {
            id: String::new(),
            app_type: "claude".into(),
            pattern: "*sonnet*".into(),
            provider_id: "claude-current".into(),
            priority: 1,
            enabled: true,
            hit_count: 0,
            last_hit_at: None,
            created_at: None,
            updated_at: None,
        };
        db.create_model_route(&route).expect("create model route");

        let state = test_state(db);
        let context = HandlerContext::load(
            &state,
            AppType::Claude,
            &HeaderMap::new(),
            &json!({"model": "claude-sonnet-4-6"}),
            "",
        )
        .await
        .expect("load handler context");

        // Model route matched — single provider, not the failover queue
        assert_eq!(context.providers().len(), 1);
        assert_eq!(context.providers()[0].id, "claude-current");
        assert_eq!(context.route_source, Some("model_route".to_string()));
    }

    #[tokio::test]
    #[serial(home_settings)]
    async fn manual_role_mapping_beats_normal_priority_model_route() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().expect("create memory database"));
        let mut current = test_provider("deepseek-current", 1);
        current.name = "DeepSeek".to_string();
        current.settings_config = json!({
            "env": {
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "deepseek-v4-pro[1m]"
            }
        });
        let route_target = test_provider("pp-coder", 0);

        db.save_provider("claude", &current)
            .expect("save current provider");
        db.save_provider("claude", &route_target)
            .expect("save route target provider");
        db.set_current_provider("claude", &current.id)
            .expect("set current provider");

        let mut config = db
            .get_proxy_config_for_app("claude")
            .await
            .expect("read app proxy config");
        config.enabled = true;
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config)
            .await
            .expect("enable auto failover");

        use crate::model_route::ModelRoute;
        let route = ModelRoute {
            id: String::new(),
            app_type: "claude".into(),
            pattern: "*".into(),
            provider_id: route_target.id.clone(),
            priority: 0,
            enabled: true,
            hit_count: 0,
            last_hit_at: None,
            created_at: None,
            updated_at: None,
        };
        db.create_model_route(&route).expect("create model route");

        let state = test_state(db);
        let context = HandlerContext::load(
            &state,
            AppType::Claude,
            &HeaderMap::new(),
            &json!({"model": "claude-opus-4-8[1M]"}),
            "",
        )
        .await
        .expect("load handler context");

        // Normal-priority automatic routes are fallbacks. A manual provider with an
        // explicit mapping must keep the request on the selected provider.
        assert_eq!(context.providers().len(), 1);
        assert_eq!(context.providers()[0].id, "deepseek-current");
        assert_eq!(
            context.route_source,
            Some("manual_provider_model".to_string())
        );
    }

    #[tokio::test]
    #[serial(home_settings)]
    async fn higher_priority_model_route_beats_manual_role_mapping() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().expect("create memory database"));

        // Manual provider: deepseek-current, with explicit opus role mapping
        let mut current = test_provider("deepseek-current", 1);
        current.name = "DeepSeek".to_string();
        current.settings_config = json!({
            "env": {
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "deepseek-v4-pro[1m]"
            }
        });

        // Another provider that a *specific* model route should direct to
        let specific_target = test_provider("specific-opus-prov", 0);

        db.save_provider("claude", &current)
            .expect("save current provider");
        db.save_provider("claude", &specific_target)
            .expect("save specific target provider");
        db.set_current_provider("claude", &current.id)
            .expect("set current provider");

        let mut config = db
            .get_proxy_config_for_app("claude")
            .await
            .expect("read app proxy config");
        config.enabled = true;
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config)
            .await
            .expect("enable auto failover");

        use crate::model_route::ModelRoute;
        let specific_route = ModelRoute {
            id: String::new(),
            app_type: "claude".into(),
            pattern: "*".into(),
            provider_id: specific_target.id.clone(),
            priority: -2,
            enabled: true,
            hit_count: 0,
            last_hit_at: None,
            created_at: None,
            updated_at: None,
        };
        db.create_model_route(&specific_route)
            .expect("create specific model route");

        let state = test_state(db);
        let context = HandlerContext::load(
            &state,
            AppType::Claude,
            &HeaderMap::new(),
            &json!({"model": "claude-opus-4-8[1M]"}),
            "",
        )
        .await
        .expect("load handler context");

        // Routes with explicit higher priority can still win over manual selection.
        assert_eq!(context.providers().len(), 1);
        assert_eq!(context.providers()[0].id, "specific-opus-prov");
        assert_eq!(context.route_source, Some("model_route".to_string()));
    }

    #[tokio::test]
    #[serial(home_settings)]
    async fn role_specific_opus_route_beats_current_glm_role_mapping() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().expect("create memory database"));

        let mut current = test_provider("glm-current", 1);
        current.name = "Zhipu GLM".to_string();
        current.settings_config = json!({
            "env": {
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "glm-5.1"
            }
        });
        let route_target = test_provider("pp-coder", 0);

        db.save_provider("claude", &current)
            .expect("save current provider");
        db.save_provider("claude", &route_target)
            .expect("save route target provider");
        db.set_current_provider("claude", &current.id)
            .expect("set current provider");

        use crate::model_route::ModelRoute;
        let route = ModelRoute {
            id: String::new(),
            app_type: "claude".into(),
            pattern: "*opus*".into(),
            provider_id: route_target.id.clone(),
            priority: 0,
            enabled: true,
            hit_count: 0,
            last_hit_at: None,
            created_at: None,
            updated_at: None,
        };
        db.create_model_route(&route).expect("create model route");

        let state = test_state(db);
        let context = HandlerContext::load(
            &state,
            AppType::Claude,
            &HeaderMap::new(),
            &json!({"model": "claude-opus-4-8"}),
            "",
        )
        .await
        .expect("load handler context");

        assert_eq!(context.providers().len(), 1);
        assert_eq!(context.providers()[0].id, "pp-coder");
        assert_eq!(context.route_source, Some("model_route".to_string()));
    }

    #[tokio::test]
    #[serial(home_settings)]
    async fn no_model_route_falls_back_to_provider_router() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().expect("create memory database"));
        let current = test_provider("claude-current", 1);
        let failover = test_provider("claude-failover", 0);

        db.save_provider("claude", &current)
            .expect("save current provider");
        db.save_provider("claude", &failover)
            .expect("save failover provider");
        db.set_current_provider("claude", &current.id)
            .expect("set current provider");

        let mut config = db
            .get_proxy_config_for_app("claude")
            .await
            .expect("read app proxy config");
        config.enabled = true;
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config)
            .await
            .expect("enable auto failover");

        // No model route matches "gemini-2.5-pro"
        let state = test_state(db);
        let context = HandlerContext::load(
            &state,
            AppType::Claude,
            &HeaderMap::new(),
            &json!({"model": "gemini-2.5-pro"}),
            "",
        )
        .await
        .expect("load handler context");

        // Falls back to normal ProviderRouter behavior (failover queue)
        assert_eq!(context.providers()[0].id, "claude-failover");
        assert_eq!(context.route_source, None);
    }
}
