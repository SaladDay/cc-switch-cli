use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Instant};

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
use tokio::{
    sync::{oneshot, RwLock},
    task::JoinHandle,
};
use tower_http::cors::{Any, CorsLayer};

use crate::{
    app_config::AppType, database::Database, provider::Provider, services::proxy::ProxyService,
};

use super::{
    circuit_breaker::CircuitBreakerConfig,
    error::ProxyError,
    handlers,
    provider_router::ProviderRouter,
    types::{ActiveTarget, ProxyConfig, ProxyServerInfo, ProxyStatus},
};

const PROXY_RUNTIME_SESSION_TOKEN_ENV_KEY: &str = "CC_SWITCH_PROXY_SESSION_TOKEN";

#[derive(Clone)]
pub struct ProxyServerState {
    pub db: Arc<Database>,
    pub config: Arc<RwLock<ProxyConfig>>,
    pub status: Arc<RwLock<ProxyStatus>>,
    pub start_time: Arc<RwLock<Option<Instant>>>,
    pub current_providers: Arc<RwLock<HashMap<String, (String, String)>>>,
    pub provider_router: Arc<ProviderRouter>,
}

impl ProxyServerState {
    pub async fn snapshot_status(&self) -> ProxyStatus {
        let mut status = self.status.read().await.clone();

        if let Some(start_time) = *self.start_time.read().await {
            status.uptime_seconds = start_time.elapsed().as_secs();
        }

        let mut active_targets = self
            .current_providers
            .read()
            .await
            .iter()
            .map(|(app_type, (provider_id, provider_name))| ActiveTarget {
                app_type: app_type.clone(),
                provider_id: provider_id.clone(),
                provider_name: provider_name.clone(),
            })
            .collect::<Vec<_>>();
        active_targets.sort_by(|left, right| left.app_type.cmp(&right.app_type));
        status.active_targets = active_targets;

        status
    }

    pub async fn record_request_start(&self) {
        let mut status = self.status.write().await;
        status.total_requests += 1;
        status.active_connections += 1;
        status.last_request_at = Some(chrono::Utc::now().to_rfc3339());
    }

    pub async fn record_estimated_input_tokens(&self, tokens: u64) {
        if tokens == 0 {
            return;
        }

        let mut status = self.status.write().await;
        status.estimated_input_tokens_total =
            status.estimated_input_tokens_total.saturating_add(tokens);
    }

    pub async fn record_estimated_output_tokens(&self, tokens: u64) {
        if tokens == 0 {
            return;
        }

        let mut status = self.status.write().await;
        status.estimated_output_tokens_total =
            status.estimated_output_tokens_total.saturating_add(tokens);
    }

    pub async fn record_active_target(&self, app_type: &AppType, provider: &Provider) {
        self.current_providers.write().await.insert(
            app_type.as_str().to_string(),
            (provider.id.clone(), provider.name.clone()),
        );

        let mut status = self.status.write().await;
        status.current_provider = Some(provider.name.clone());
        status.current_provider_id = Some(provider.id.clone());
    }

    pub async fn sync_successful_provider_selection(
        &self,
        app_type: &AppType,
        provider: &Provider,
        current_provider_id_at_start: &str,
    ) {
        self.record_active_target(app_type, provider).await;

        if provider.id == current_provider_id_at_start {
            return;
        }

        let takeover_enabled = self
            .db
            .get_proxy_config_for_app(app_type.as_str())
            .await
            .map(|config| config.enabled)
            .unwrap_or(false);

        self.db
            .set_current_provider(app_type.as_str(), &provider.id)
            .ok();
        crate::settings::set_current_provider(app_type, Some(&provider.id)).ok();

        if takeover_enabled {
            ProxyService::new(self.db.clone())
                .update_live_backup_from_provider(app_type.as_str(), provider)
                .await
                .ok();
        }

        let mut status = self.status.write().await;
        status.failover_count = status.failover_count.saturating_add(1);
    }

    pub async fn record_request_success(&self) {
        let mut status = self.status.write().await;
        status.active_connections = status.active_connections.saturating_sub(1);
        status.success_requests += 1;
        update_success_rate(&mut status);
        status.last_error = None;
    }

    pub async fn record_request_error(&self, error: &ProxyError) {
        self.record_request_error_message(error.to_string()).await;
    }

    pub async fn record_request_error_message(&self, message: String) {
        let mut status = self.status.write().await;
        status.active_connections = status.active_connections.saturating_sub(1);
        status.failed_requests += 1;
        update_success_rate(&mut status);
        status.last_error = Some(message);
    }

    pub async fn record_upstream_failure(
        &self,
        status_code: reqwest::StatusCode,
        summary: Option<String>,
    ) {
        let mut status = self.status.write().await;
        status.active_connections = status.active_connections.saturating_sub(1);
        status.failed_requests += 1;
        update_success_rate(&mut status);
        status.last_error = Some(match summary {
            Some(summary) if !summary.is_empty() => {
                format!("upstream returned {}: {summary}", status_code.as_u16())
            }
            _ => format!("upstream returned {}", status_code.as_u16()),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;
    use serial_test::serial;
    use std::env;
    use tempfile::TempDir;

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

    fn test_provider(id: &str, name: &str) -> Provider {
        Provider {
            id: id.to_string(),
            name: name.to_string(),
            settings_config: json!({}),
            website_url: None,
            category: Some("claude".to_string()),
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    fn test_provider_with_settings(
        id: &str,
        name: &str,
        settings_config: serde_json::Value,
    ) -> Provider {
        Provider {
            id: id.to_string(),
            name: name.to_string(),
            settings_config,
            website_url: None,
            category: Some("claude".to_string()),
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    fn test_state(db: Arc<Database>) -> ProxyServerState {
        ProxyServerState {
            db: db.clone(),
            config: Arc::new(RwLock::new(ProxyConfig::default())),
            status: Arc::new(RwLock::new(ProxyStatus::default())),
            start_time: Arc::new(RwLock::new(None)),
            current_providers: Arc::new(RwLock::new(HashMap::new())),
            provider_router: Arc::new(ProviderRouter::new(db)),
        }
    }

    async fn set_takeover_enabled(db: &Database, app_type: &str, enabled: bool) {
        let mut config = db
            .get_proxy_config_for_app(app_type)
            .await
            .expect("read app proxy config");
        config.enabled = enabled;
        db.update_proxy_config_for_app(config)
            .await
            .expect("update app proxy config");
    }

    #[tokio::test]
    #[serial(home_settings)]
    async fn sync_successful_provider_selection_updates_state_after_failover() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().expect("create memory database"));
        let current = test_provider_with_settings(
            "claude-current",
            "Claude Current",
            json!({"apiKey": "current-key", "base_url": "https://current.example"}),
        );
        let failover = test_provider_with_settings(
            "claude-failover",
            "Claude Failover",
            json!({"apiKey": "failover-key", "base_url": "https://failover.example"}),
        );

        db.save_provider("claude", &current)
            .expect("save current provider");
        db.save_provider("claude", &failover)
            .expect("save failover provider");
        db.set_current_provider("claude", &current.id)
            .expect("set current provider");
        crate::settings::set_current_provider(&AppType::Claude, Some(&current.id))
            .expect("set local current provider");
        db.save_live_backup(
            "claude",
            &serde_json::to_string(&current.settings_config).expect("serialize current backup"),
        )
        .await
        .expect("save current live backup");
        set_takeover_enabled(&db, "claude", true).await;

        let state = test_state(db.clone());
        state
            .sync_successful_provider_selection(&AppType::Claude, &failover, &current.id)
            .await;

        assert_eq!(
            db.get_current_provider("claude")
                .expect("read current provider after sync")
                .as_deref(),
            Some("claude-failover")
        );
        assert_eq!(
            crate::settings::get_current_provider(&AppType::Claude).as_deref(),
            Some("claude-failover")
        );
        let backup = db
            .get_live_backup("claude")
            .await
            .expect("read live backup after sync")
            .expect("live backup should remain present");
        let backup_snapshot: serde_json::Value =
            serde_json::from_str(&backup.original_config).expect("parse live backup snapshot");
        assert_eq!(
            backup_snapshot
                .get("base_url")
                .and_then(serde_json::Value::as_str),
            Some("https://failover.example")
        );

        let status = state.snapshot_status().await;
        assert_eq!(
            status.current_provider_id.as_deref(),
            Some("claude-failover")
        );
        assert_eq!(status.current_provider.as_deref(), Some("Claude Failover"));
        assert_eq!(status.failover_count, 1);
        assert_eq!(status.active_targets.len(), 1);
        assert_eq!(status.active_targets[0].app_type, "claude");
        assert_eq!(status.active_targets[0].provider_id, "claude-failover");
    }

    #[tokio::test]
    async fn sync_successful_provider_selection_keeps_failover_count_when_provider_unchanged() {
        let db = Arc::new(Database::memory().expect("create memory database"));
        let current = test_provider("claude-current", "Claude Current");

        db.save_provider("claude", &current)
            .expect("save current provider");
        db.set_current_provider("claude", &current.id)
            .expect("set current provider");

        let state = test_state(db.clone());
        state
            .sync_successful_provider_selection(&AppType::Claude, &current, &current.id)
            .await;

        assert_eq!(
            db.get_current_provider("claude")
                .expect("read current provider after sync")
                .as_deref(),
            Some("claude-current")
        );

        let status = state.snapshot_status().await;
        assert_eq!(
            status.current_provider_id.as_deref(),
            Some("claude-current")
        );
        assert_eq!(status.current_provider.as_deref(), Some("Claude Current"));
        assert_eq!(status.failover_count, 0);
        assert_eq!(status.active_targets.len(), 1);
        assert_eq!(status.active_targets[0].provider_id, "claude-current");
    }

    #[tokio::test]
    #[serial(home_settings)]
    async fn sync_successful_provider_selection_skips_backup_update_when_takeover_disabled() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().expect("create memory database"));
        let current = test_provider_with_settings(
            "claude-current",
            "Claude Current",
            json!({"apiKey": "current-key", "base_url": "https://current.example"}),
        );
        let failover = test_provider_with_settings(
            "claude-failover",
            "Claude Failover",
            json!({"apiKey": "failover-key", "base_url": "https://failover.example"}),
        );

        db.save_provider("claude", &current)
            .expect("save current provider");
        db.save_provider("claude", &failover)
            .expect("save failover provider");
        db.set_current_provider("claude", &current.id)
            .expect("set current provider");
        crate::settings::set_current_provider(&AppType::Claude, Some(&current.id))
            .expect("set local current provider");
        db.save_live_backup(
            "claude",
            &serde_json::to_string(&current.settings_config).expect("serialize current backup"),
        )
        .await
        .expect("save current live backup");

        let state = test_state(db.clone());
        state
            .sync_successful_provider_selection(&AppType::Claude, &failover, &current.id)
            .await;

        assert_eq!(
            db.get_current_provider("claude")
                .expect("read current provider after sync")
                .as_deref(),
            Some("claude-failover")
        );
        assert_eq!(
            crate::settings::get_current_provider(&AppType::Claude).as_deref(),
            Some("claude-failover")
        );
        let backup = db
            .get_live_backup("claude")
            .await
            .expect("read live backup after sync")
            .expect("live backup should remain present");
        let backup_snapshot: serde_json::Value =
            serde_json::from_str(&backup.original_config).expect("parse live backup snapshot");
        assert_eq!(
            backup_snapshot
                .get("base_url")
                .and_then(serde_json::Value::as_str),
            Some("https://current.example")
        );

        let status = state.snapshot_status().await;
        assert_eq!(
            status.current_provider_id.as_deref(),
            Some("claude-failover")
        );
        assert_eq!(status.current_provider.as_deref(), Some("Claude Failover"));
        assert_eq!(status.failover_count, 1);
        assert_eq!(status.active_targets.len(), 1);
        assert_eq!(status.active_targets[0].provider_id, "claude-failover");
    }
}

fn update_success_rate(status: &mut ProxyStatus) {
    status.success_rate = if status.total_requests == 0 {
        0.0
    } else {
        (status.success_requests as f32 / status.total_requests as f32) * 100.0
    };
}

pub struct ProxyServer {
    state: ProxyServerState,
    shutdown_tx: Arc<RwLock<Option<oneshot::Sender<()>>>>,
    server_handle: Arc<RwLock<Option<JoinHandle<()>>>>,
}

impl ProxyServer {
    pub fn new(config: ProxyConfig, db: Arc<Database>) -> Self {
        let provider_router = Arc::new(ProviderRouter::new(db.clone()));
        let managed_session_token = std::env::var(PROXY_RUNTIME_SESSION_TOKEN_ENV_KEY)
            .ok()
            .filter(|value| !value.trim().is_empty());
        let status = ProxyStatus {
            managed_session_token,
            ..ProxyStatus::default()
        };

        Self {
            state: ProxyServerState {
                db,
                config: Arc::new(RwLock::new(config)),
                status: Arc::new(RwLock::new(status)),
                start_time: Arc::new(RwLock::new(None)),
                current_providers: Arc::new(RwLock::new(HashMap::new())),
                provider_router,
            },
            shutdown_tx: Arc::new(RwLock::new(None)),
            server_handle: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn start(&self) -> Result<ProxyServerInfo, String> {
        if self.shutdown_tx.read().await.is_some() {
            let status = self.get_status().await;
            return Ok(ProxyServerInfo {
                address: status.address,
                port: status.port,
                started_at: chrono::Utc::now().to_rfc3339(),
            });
        }

        let bind_config = self.state.config.read().await.clone();
        let addr: SocketAddr =
            format!("{}:{}", bind_config.listen_address, bind_config.listen_port)
                .parse()
                .map_err(|e| format!("invalid bind address: {e}"))?;

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("bind proxy listener failed: {e}"))?;
        let local_addr = listener
            .local_addr()
            .map_err(|e| format!("read proxy listener address failed: {e}"))?;

        super::http_client::set_proxy_port(local_addr.port());

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        *self.shutdown_tx.write().await = Some(shutdown_tx);

        {
            let mut status = self.state.status.write().await;
            status.running = true;
            status.address = bind_config.listen_address.clone();
            status.port = local_addr.port();
        }
        *self.state.start_time.write().await = Some(Instant::now());

        let app = self.build_router();
        let state = self.state.clone();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await;

            state.status.write().await.running = false;
            *state.start_time.write().await = None;
        });
        *self.server_handle.write().await = Some(handle);

        Ok(ProxyServerInfo {
            address: bind_config.listen_address,
            port: local_addr.port(),
            started_at: chrono::Utc::now().to_rfc3339(),
        })
    }

    pub async fn set_active_target(&self, app_type: &str, provider_id: &str, provider_name: &str) {
        let mut current_providers = self.state.current_providers.write().await;
        current_providers.insert(
            app_type.to_string(),
            (provider_id.to_string(), provider_name.to_string()),
        );
    }

    pub async fn stop(&self) -> Result<(), String> {
        if let Some(tx) = self.shutdown_tx.write().await.take() {
            let _ = tx.send(());
        } else {
            return Ok(());
        }

        if let Some(handle) = self.server_handle.write().await.take() {
            handle
                .await
                .map_err(|e| format!("join proxy task failed: {e}"))?;
        }
        Ok(())
    }

    pub async fn get_status(&self) -> ProxyStatus {
        self.state.snapshot_status().await
    }

    pub async fn update_circuit_breaker_configs(&self, config: CircuitBreakerConfig) {
        self.state.provider_router.update_all_configs(config).await;
    }

    pub async fn reset_provider_circuit_breaker(&self, provider_id: &str, app_type: &str) {
        self.state
            .provider_router
            .reset_provider_breaker(provider_id, app_type)
            .await;
    }

    #[cfg(test)]
    pub(crate) fn provider_router(&self) -> Arc<ProviderRouter> {
        self.state.provider_router.clone()
    }

    fn build_router(&self) -> Router {
        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);

        Router::new()
            .route("/health", get(handlers::health_check))
            .route("/status", get(handlers::get_status))
            .route("/v1/messages", post(handlers::handle_messages))
            .route("/claude/v1/messages", post(handlers::handle_messages))
            .route("/chat/completions", post(handlers::handle_chat_completions))
            .route(
                "/v1/chat/completions",
                post(handlers::handle_chat_completions),
            )
            .route(
                "/v1/v1/chat/completions",
                post(handlers::handle_chat_completions),
            )
            .route(
                "/codex/v1/chat/completions",
                post(handlers::handle_chat_completions),
            )
            .route("/responses", post(handlers::handle_responses))
            .route("/v1/responses", post(handlers::handle_responses))
            .route("/v1/v1/responses", post(handlers::handle_responses))
            .route("/codex/v1/responses", post(handlers::handle_responses))
            .route(
                "/responses/compact",
                post(handlers::handle_responses_compact),
            )
            .route(
                "/v1/responses/compact",
                post(handlers::handle_responses_compact),
            )
            .route(
                "/v1/v1/responses/compact",
                post(handlers::handle_responses_compact),
            )
            .route(
                "/codex/v1/responses/compact",
                post(handlers::handle_responses_compact),
            )
            .route("/v1beta/*path", post(handlers::handle_gemini))
            .route("/gemini/v1beta/*path", post(handlers::handle_gemini))
            .layer(DefaultBodyLimit::max(200 * 1024 * 1024))
            .layer(cors)
            .with_state(self.state.clone())
    }
}
