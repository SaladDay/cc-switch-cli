use axum::http::HeaderMap;
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app_config::AppType;
use crate::provider::Provider;

use super::{
    error::ProxyError,
    provider_router::ProviderRouter,
    server::ProxyServerState,
    session::extract_session_id,
    types::{AppProxyConfig, OptimizerConfig, RectifierConfig},
};

pub struct HandlerContext {
    pub start_time: Instant,
    pub state: ProxyServerState,
    pub app_type: AppType,
    pub provider_router: Arc<ProviderRouter>,
    providers: Vec<Provider>,
    pub app_proxy: AppProxyConfig,
    pub rectifier_config: RectifierConfig,
    pub optimizer_config: OptimizerConfig,
    pub request_model: String,
    pub session_id: String,
    pub current_provider_id_at_start: String,
}

impl HandlerContext {
    pub async fn load(
        state: &ProxyServerState,
        app_type: AppType,
        headers: &HeaderMap,
        body: &Value,
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
        let providers = provider_router.select_providers(app_type.as_str()).await?;

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
        let request_model = body
            .get("model")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown")
            .to_string();
        let session_id = extract_session_id(headers, body, app_type.as_str());

        Ok(Self {
            start_time,
            state: state.clone(),
            app_type,
            provider_router,
            providers,
            app_proxy,
            rectifier_config,
            optimizer_config,
            request_model,
            session_id,
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
    use std::env;
    use tempfile::TempDir;
    use tokio::sync::RwLock;

    use crate::{database::Database, proxy::types::ProxyConfig};

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
            provider_router: Arc::new(ProviderRouter::new(db)),
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
}
