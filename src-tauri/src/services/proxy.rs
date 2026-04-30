mod codex_toml;

use std::{
    collections::HashMap,
    future::Future,
    process::{Command, Stdio},
    sync::{Arc, Mutex as StdMutex, OnceLock, Weak},
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::{
    app_config::AppType,
    codex_config::{get_codex_auth_path, get_codex_config_path, write_codex_live_atomic},
    config::{get_claude_settings_path, read_json_file, write_json_file, write_text_file},
    database::Database,
    gemini_config::{
        env_to_json, get_gemini_env_path, json_to_env, read_gemini_env, write_gemini_env_atomic,
    },
    provider::Provider,
    proxy::{
        switch_lock::SwitchLockManager,
        types::{GlobalProxyConfig, ProxyTakeoverStatus},
        ProxyConfig, ProxyServer, ProxyServerInfo, ProxyStatus,
    },
    AppError,
};

const PROXY_TOKEN_PLACEHOLDER: &str = "PROXY_MANAGED";
const PROXY_RUNTIME_SESSION_KEY: &str = "proxy_runtime_session";
const PROXY_RUNTIME_KIND_ENV_KEY: &str = "CC_SWITCH_PROXY_RUNTIME_KIND";
const PROXY_RUNTIME_SESSION_TOKEN_ENV_KEY: &str = "CC_SWITCH_PROXY_SESSION_TOKEN";

const CLAUDE_MODEL_OVERRIDE_ENV_KEYS: [&str; 6] = [
    "ANTHROPIC_MODEL",
    "ANTHROPIC_REASONING_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL",
];

#[derive(Clone)]
pub struct ProxyService {
    db: Arc<Database>,
    runtime: Arc<ProxyRuntimeState>,
    switch_locks: SwitchLockManager,
}

struct ProxyRuntimeState {
    server: RwLock<Option<ProxyServer>>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HotSwitchOutcome {
    pub logical_target_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PersistedProxyRuntimeSessionKind {
    #[serde(alias = "foreground")]
    Foreground,
    ManagedExternal,
}

impl Default for PersistedProxyRuntimeSessionKind {
    fn default() -> Self {
        Self::Foreground
    }
}

impl PersistedProxyRuntimeSessionKind {
    fn from_env() -> Self {
        match std::env::var(PROXY_RUNTIME_KIND_ENV_KEY).ok().as_deref() {
            Some("managed_external") => Self::ManagedExternal,
            _ => Self::Foreground,
        }
    }

    fn as_env_value(&self) -> &'static str {
        match self {
            Self::Foreground => "foreground",
            Self::ManagedExternal => "managed_external",
        }
    }

    fn is_managed_external(&self) -> bool {
        matches!(self, Self::ManagedExternal)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedProxyRuntimeSession {
    pid: u32,
    address: String,
    port: u16,
    started_at: String,
    #[serde(default)]
    kind: PersistedProxyRuntimeSessionKind,
    #[serde(default)]
    session_token: Option<String>,
}

enum ExternalProxyStatusProbe {
    Matched(ProxyStatus),
    Mismatched,
    Unreachable,
}

fn proxy_runtime_registry() -> &'static StdMutex<HashMap<String, Weak<ProxyRuntimeState>>> {
    static REGISTRY: OnceLock<StdMutex<HashMap<String, Weak<ProxyRuntimeState>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| StdMutex::new(HashMap::new()))
}

impl ProxyService {
    fn run_in_blocking_runtime<T, F, Fut>(&self, task: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(ProxyService) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, String>> + Send + 'static,
    {
        let service = self.clone();
        let handle = std::thread::Builder::new()
            .name("cc-switch-proxy-blocking".to_string())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| format!("failed to create async runtime: {e}"))?;
                runtime.block_on(task(service))
            })
            .map_err(|e| format!("spawn proxy runtime helper failed: {e}"))?;

        handle
            .join()
            .map_err(|_| "proxy runtime helper panicked".to_string())?
    }

    pub fn is_running_blocking(&self) -> Result<bool, String> {
        self.run_in_blocking_runtime(|service| async move { Ok(service.is_running().await) })
    }

    pub fn is_app_takeover_active_blocking(&self, app_type: &AppType) -> Result<bool, String> {
        let app_type = app_type.clone();
        self.run_in_blocking_runtime(move |service| async move {
            service.is_app_takeover_active(&app_type).await
        })
    }

    pub fn recover_takeovers_on_startup_blocking(&self) -> Result<(), String> {
        self.run_in_blocking_runtime(|service| async move {
            service.recover_takeovers_on_startup().await
        })
    }

    pub fn new(db: Arc<Database>) -> Self {
        let runtime = Self::shared_runtime_state(db.runtime_key());
        Self {
            db,
            runtime,
            switch_locks: SwitchLockManager::new(),
        }
    }

    fn shared_runtime_state(runtime_key: &str) -> Arc<ProxyRuntimeState> {
        let mut registry = proxy_runtime_registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(existing) = registry.get(runtime_key).and_then(Weak::upgrade) {
            return existing;
        }

        let runtime = Arc::new(ProxyRuntimeState {
            server: RwLock::new(None),
        });
        registry.insert(runtime_key.to_string(), Arc::downgrade(&runtime));
        runtime
    }

    pub async fn start(&self) -> Result<ProxyServerInfo, String> {
        let _guard = crate::services::state_coordination::acquire_restore_mutation_guard().await?;
        let config = self.get_config().await.map_err(|e| e.to_string())?;
        self.start_with_resolved_config_unlocked(config).await
    }

    pub async fn start_with_runtime_config(
        &self,
        config: ProxyConfig,
    ) -> Result<ProxyServerInfo, String> {
        let _guard = crate::services::state_coordination::acquire_restore_mutation_guard().await?;
        self.start_with_resolved_config_unlocked(config).await
    }

    pub async fn start_managed_session(&self, app_type: &str) -> Result<ProxyServerInfo, String> {
        let _guard = crate::services::state_coordination::acquire_restore_mutation_guard().await?;
        self.start_managed_session_unlocked(app_type).await
    }

    async fn start_managed_session_unlocked(
        &self,
        app_type: &str,
    ) -> Result<ProxyServerInfo, String> {
        Self::ensure_managed_sessions_supported()?;

        let app_type = Self::takeover_app_from_str(app_type)?;
        let current_status = self.get_status().await;
        if current_status.running {
            return Err(
                "proxy is already running; stop the current runtime before starting a managed session"
                    .to_string(),
            );
        }

        let executable = Self::resolve_managed_proxy_executable()?;
        let session_token = uuid::Uuid::new_v4().to_string();
        let mut child = Command::new(executable);
        child
            .arg("proxy")
            .arg("serve")
            .arg("--takeover")
            .arg(app_type.as_str())
            .env(
                PROXY_RUNTIME_KIND_ENV_KEY,
                PersistedProxyRuntimeSessionKind::ManagedExternal.as_env_value(),
            )
            .env(PROXY_RUNTIME_SESSION_TOKEN_ENV_KEY, &session_token)
            .env(
                crate::services::state_coordination::RESTORE_GUARD_BYPASS_ENV_KEY,
                "1",
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        #[cfg(unix)]
        unsafe {
            child.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = child
            .spawn()
            .map_err(|error| format!("spawn managed proxy session failed: {error}"))?;
        let child_pid = child.id();

        let start_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("poll managed proxy process failed: {error}"))?
            {
                return Err(format!(
                    "managed proxy session exited before becoming ready: {}",
                    status
                ));
            }

            if let Some(info) = self
                .managed_session_ready_info(child_pid, session_token.as_str())
                .await
            {
                Self::spawn_managed_child_reaper(child);
                return Ok(info);
            }

            if tokio::time::Instant::now() >= start_deadline {
                let _ = child.kill();
                let _ = child.wait();
                let _ = self.clear_persisted_runtime_session();
                return Err("managed proxy session did not become ready in time".to_string());
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn set_managed_session_for_app(
        &self,
        app_type: &str,
        enabled: bool,
    ) -> Result<(), String> {
        let _guard = crate::services::state_coordination::acquire_restore_mutation_guard().await?;
        self.set_managed_session_for_app_unlocked(app_type, enabled)
            .await
    }

    async fn set_managed_session_for_app_unlocked(
        &self,
        app_type: &str,
        enabled: bool,
    ) -> Result<(), String> {
        let app_type = Self::takeover_app_from_str(app_type)?;

        if enabled {
            let status = self.get_status().await;
            if !status.running {
                self.start_managed_session_unlocked(app_type.as_str())
                    .await?;
                return Ok(());
            }

            if self
                .load_persisted_runtime_session()
                .is_some_and(|session| session.kind.is_managed_external())
            {
                self.enable_takeover_for_app_unlocked(&app_type).await?;
                return Ok(());
            }

            return Err(
                "proxy is already running in foreground mode; stop the current runtime before attaching another app to a managed session"
                    .to_string(),
            );
        }

        let stop_server_when_last = self
            .load_persisted_runtime_session()
            .map(|session| session.kind.is_managed_external())
            .unwrap_or(false);
        self.disable_takeover_for_app_unlocked(&app_type, stop_server_when_last)
            .await
    }

    async fn start_with_resolved_config_unlocked(
        &self,
        config: ProxyConfig,
    ) -> Result<ProxyServerInfo, String> {
        self.sync_persisted_global_proxy_enabled(true).await?;

        if let Some(server) = self.runtime.server.read().await.as_ref() {
            let status = server.get_status().await;
            if status.running {
                return Ok(ProxyServerInfo {
                    address: status.address,
                    port: status.port,
                    started_at: chrono::Utc::now().to_rfc3339(),
                });
            }
        }

        let server = ProxyServer::new(config, self.db.clone());
        let info = server.start().await?;
        if !Self::defer_runtime_session_publish() {
            if let Err(error) = self.persist_runtime_session(&info) {
                let _ = server.stop().await;
                return Err(error);
            }
        }
        *self.runtime.server.write().await = Some(server);
        Ok(info)
    }

    pub(crate) fn publish_runtime_session_if_needed(
        &self,
        info: &ProxyServerInfo,
    ) -> Result<(), String> {
        if Self::defer_runtime_session_publish() && self.load_persisted_runtime_session().is_none()
        {
            self.persist_runtime_session(info)?;
        }
        Ok(())
    }

    pub async fn recover_takeovers_on_startup(&self) -> Result<(), String> {
        for app_type in [AppType::Claude, AppType::Codex, AppType::Gemini] {
            let app_key = app_type.as_str();
            let app_proxy = self
                .db
                .get_proxy_config_for_app(app_key)
                .await
                .map_err(|error| format!("load proxy config for {app_key} failed: {error}"))?;
            let has_backup = self
                .db
                .get_live_backup(app_key)
                .await
                .map_err(|error| format!("load live backup for {app_key} failed: {error}"))?
                .is_some();
            let live_taken_over = self.detect_takeover_in_live_config_for_app(&app_type);

            if !app_proxy.enabled && !has_backup && !live_taken_over {
                continue;
            }

            if has_backup {
                self.restore_live_config_for_app(&app_type).await?;
                self.db
                    .delete_live_backup(app_key)
                    .await
                    .map_err(|error| format!("delete live backup for {app_key} failed: {error}"))?;
            } else if live_taken_over {
                self.restore_live_from_current_provider(&app_type).await?;
            }

            if app_proxy.enabled {
                let mut cleared = app_proxy;
                cleared.enabled = false;
                self.db
                    .update_proxy_config_for_app(cleared)
                    .await
                    .map_err(|error| {
                        format!("clear takeover flag for {app_key} failed: {error}")
                    })?;
            }
        }

        Ok(())
    }

    pub async fn stop(&self) -> Result<(), String> {
        let _guard = crate::services::state_coordination::acquire_restore_mutation_guard().await?;
        self.stop_server_unlocked().await
    }

    pub async fn stop_with_restore(&self) -> Result<(), String> {
        let _guard = crate::services::state_coordination::acquire_restore_mutation_guard().await?;

        if let Err(error) = self.stop_server_unlocked().await {
            log::warn!("stop proxy runtime before restore failed: {error}");
        }

        self.restore_active_takeovers_on_shutdown_unlocked().await
    }

    async fn stop_server_unlocked(&self) -> Result<(), String> {
        let mut stopped_runtime = false;

        if let Some(server) = self.runtime.server.read().await.as_ref() {
            server.stop().await?;
            self.clear_persisted_runtime_session()?;
            self.sync_persisted_global_proxy_enabled(false).await?;
            return Ok(());
        }

        if let Some(session) = self.load_persisted_runtime_session() {
            if session.kind.is_managed_external() {
                let probe = Self::probe_external_proxy_status(&session).await;
                let should_terminate = match probe {
                    ExternalProxyStatusProbe::Matched(_) => true,
                    ExternalProxyStatusProbe::Mismatched => false,
                    ExternalProxyStatusProbe::Unreachable => {
                        Self::has_managed_external_ownership_signal(&session)
                    }
                };

                if Self::is_process_alive(session.pid) && should_terminate {
                    Self::terminate_external_process(session.pid).await?;
                    stopped_runtime = true;
                }
            }
        }

        self.clear_persisted_runtime_session()?;
        self.sync_persisted_global_proxy_enabled(false).await?;
        if stopped_runtime {
            Ok(())
        } else {
            Err("proxy server is not running".to_string())
        }
    }

    pub async fn is_running(&self) -> bool {
        self.get_status().await.running
    }

    pub async fn get_status(&self) -> ProxyStatus {
        if let Some(server) = self.runtime.server.read().await.as_ref() {
            return server.get_status().await;
        }

        if let Some(session) = self.load_persisted_runtime_session() {
            if session.kind.is_managed_external() {
                if Self::is_process_alive(session.pid) {
                    match Self::probe_external_proxy_status(&session).await {
                        ExternalProxyStatusProbe::Matched(status) => return status,
                        ExternalProxyStatusProbe::Mismatched => {
                            let _ = self.clear_persisted_runtime_session();
                            return ProxyStatus::default();
                        }
                        ExternalProxyStatusProbe::Unreachable => {
                            if Self::has_managed_external_ownership_signal(&session) {
                                let uptime_seconds =
                                    chrono::DateTime::parse_from_rfc3339(&session.started_at)
                                        .ok()
                                        .map(|started_at| {
                                            let started_at = started_at.with_timezone(&chrono::Utc);
                                            (chrono::Utc::now() - started_at).num_seconds().max(0)
                                                as u64
                                        })
                                        .unwrap_or(0);

                                return ProxyStatus {
                                    running: true,
                                    address: session.address.clone(),
                                    port: session.port,
                                    uptime_seconds,
                                    managed_session_token: session.session_token.clone(),
                                    ..ProxyStatus::default()
                                };
                            }
                        }
                    }
                }

                let _ = self.clear_persisted_runtime_session();
                return ProxyStatus::default();
            }

            if Self::is_process_alive(session.pid) {
                let uptime_seconds = chrono::DateTime::parse_from_rfc3339(&session.started_at)
                    .ok()
                    .map(|started_at| {
                        let started_at = started_at.with_timezone(&chrono::Utc);
                        (chrono::Utc::now() - started_at).num_seconds().max(0) as u64
                    })
                    .unwrap_or(0);

                return ProxyStatus {
                    running: true,
                    address: session.address,
                    port: session.port,
                    uptime_seconds,
                    ..ProxyStatus::default()
                };
            }

            let _ = self.clear_persisted_runtime_session();
        }

        ProxyStatus::default()
    }

    pub async fn get_config(&self) -> Result<ProxyConfig, AppError> {
        self.db.get_proxy_config().await
    }

    pub async fn update_config(&self, config: &ProxyConfig) -> Result<(), AppError> {
        let _guard = crate::services::state_coordination::acquire_restore_mutation_guard()
            .await
            .map_err(AppError::Message)?;
        self.db.update_proxy_config(config.clone()).await
    }

    pub async fn update_circuit_breaker_configs(
        &self,
        config: crate::proxy::circuit_breaker::CircuitBreakerConfig,
    ) -> Result<(), String> {
        if let Some(server) = self.runtime.server.read().await.as_ref() {
            server.update_circuit_breaker_configs(config).await;
        }

        Ok(())
    }

    pub async fn reset_provider_circuit_breaker(
        &self,
        provider_id: &str,
        app_type: &str,
    ) -> Result<(), String> {
        if let Some(server) = self.runtime.server.read().await.as_ref() {
            server
                .reset_provider_circuit_breaker(provider_id, app_type)
                .await;
        }

        Ok(())
    }

    pub async fn get_global_config(&self) -> Result<GlobalProxyConfig, AppError> {
        self.db.get_global_proxy_config().await
    }

    pub async fn set_global_enabled(&self, enabled: bool) -> Result<GlobalProxyConfig, AppError> {
        let _guard = crate::services::state_coordination::acquire_restore_mutation_guard()
            .await
            .map_err(AppError::Message)?;
        let mut config = self.get_global_config().await?;
        config.proxy_enabled = enabled;
        self.db.update_global_proxy_config(config.clone()).await?;
        Ok(config)
    }

    pub async fn get_takeover_status(&self) -> Result<ProxyTakeoverStatus, String> {
        Ok(ProxyTakeoverStatus {
            claude: self
                .db
                .get_proxy_config_for_app("claude")
                .await
                .map_err(|error| format!("load claude proxy config failed: {error}"))?
                .enabled,
            codex: self
                .db
                .get_proxy_config_for_app("codex")
                .await
                .map_err(|error| format!("load codex proxy config failed: {error}"))?
                .enabled,
            gemini: self
                .db
                .get_proxy_config_for_app("gemini")
                .await
                .map_err(|error| format!("load gemini proxy config failed: {error}"))?
                .enabled,
        })
    }

    pub async fn set_takeover_for_app(&self, app_type: &str, enabled: bool) -> Result<(), String> {
        let app_type = Self::takeover_app_from_str(app_type)?;
        let _guard = crate::services::state_coordination::acquire_restore_mutation_guard().await?;

        if enabled {
            self.enable_takeover_for_app_unlocked(&app_type).await
        } else {
            self.disable_takeover_for_app_unlocked(&app_type, true)
                .await
        }
    }

    pub async fn is_app_takeover_active(&self, app_type: &AppType) -> Result<bool, String> {
        let app_key = app_type.as_str();
        let app_proxy = self
            .db
            .get_proxy_config_for_app(app_key)
            .await
            .map_err(|error| format!("load proxy config for {app_key} failed: {error}"))?;
        if app_proxy.enabled {
            return Ok(true);
        }

        if self
            .db
            .get_live_backup(app_key)
            .await
            .map_err(|error| format!("load live backup for {app_key} failed: {error}"))?
            .is_some()
        {
            return Ok(true);
        }

        Ok(self.detect_takeover_in_live_config_for_app(app_type))
    }

    pub fn detect_takeover_in_live_config_for_app(&self, app_type: &AppType) -> bool {
        match app_type {
            AppType::Claude => self
                .read_claude_live()
                .ok()
                .is_some_and(|live| Self::is_claude_live_taken_over(&live)),
            AppType::Codex => self
                .read_codex_live()
                .ok()
                .is_some_and(|live| Self::is_codex_live_taken_over(&live)),
            AppType::Gemini => self
                .read_gemini_live()
                .ok()
                .is_some_and(|live| Self::is_gemini_live_taken_over(&live)),
            _ => false,
        }
    }

    fn is_claude_live_taken_over(config: &Value) -> bool {
        let Some(env) = config.get("env").and_then(Value::as_object) else {
            return false;
        };

        [
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "OPENROUTER_API_KEY",
            "OPENAI_API_KEY",
        ]
        .iter()
        .any(|key| {
            env.get(*key)
                .and_then(Value::as_str)
                .is_some_and(|value| value == PROXY_TOKEN_PLACEHOLDER)
        })
    }

    fn is_codex_live_taken_over(config: &Value) -> bool {
        config
            .get("auth")
            .and_then(|auth| auth.get("OPENAI_API_KEY"))
            .and_then(Value::as_str)
            .is_some_and(|value| value == PROXY_TOKEN_PLACEHOLDER)
    }

    fn is_gemini_live_taken_over(config: &Value) -> bool {
        config
            .get("env")
            .and_then(|env| env.get("GEMINI_API_KEY"))
            .and_then(Value::as_str)
            .is_some_and(|value| value == PROXY_TOKEN_PLACEHOLDER)
    }

    pub async fn update_live_backup_from_provider(
        &self,
        app_type: &str,
        provider: &Provider,
    ) -> Result<(), String> {
        let app_type = Self::takeover_app_from_str(app_type)?;
        let mut backup_snapshot = self.build_live_snapshot_from_provider(&app_type, provider)?;

        if matches!(app_type, AppType::Codex) {
            let existing_backup_value = self
                .db
                .get_live_backup(app_type.as_str())
                .await
                .map_err(|error| {
                    format!(
                        "load {} existing live backup failed: {error}",
                        app_type.as_str()
                    )
                })?
                .map(|backup| {
                    serde_json::from_str::<Value>(&backup.original_config).map_err(|error| {
                        format!(
                            "parse {} existing live backup failed: {error}",
                            app_type.as_str()
                        )
                    })
                })
                .transpose()?;

            if let Some(existing_value) = existing_backup_value.as_ref() {
                Self::preserve_codex_mcp_servers_in_backup(&mut backup_snapshot, existing_value)?;
            }

            let anchor_config_text = existing_backup_value
                .as_ref()
                .and_then(|value| value.get("config"))
                .and_then(Value::as_str);
            crate::codex_config::normalize_codex_settings_config_model_provider(
                &mut backup_snapshot,
                anchor_config_text,
            )
            .map_err(|error| format!("normalize Codex live backup failed: {error}"))?;
        }

        if matches!(app_type, AppType::Gemini) {
            backup_snapshot = json!({
                "env": backup_snapshot
                    .get("env")
                    .cloned()
                    .unwrap_or_else(|| json!({}))
            });
        }

        self.save_live_backup_snapshot(app_type.as_str(), &backup_snapshot)
            .await
    }

    pub async fn hot_switch_provider(
        &self,
        app_type: &str,
        provider_id: &str,
    ) -> Result<HotSwitchOutcome, String> {
        let _guard = self.switch_locks.lock_for_app(app_type).await;

        let app_type_enum = Self::takeover_app_from_str(app_type)?;
        let provider = self
            .db
            .get_provider_by_id(provider_id, app_type)
            .map_err(|e| format!("读取供应商失败: {e}"))?
            .ok_or_else(|| format!("供应商不存在: {provider_id}"))?;

        let logical_target_changed =
            crate::settings::get_effective_current_provider(&self.db, &app_type_enum)
                .map_err(|e| format!("读取当前供应商失败: {e}"))?
                .as_deref()
                != Some(provider_id);

        let has_backup = self
            .db
            .get_live_backup(app_type_enum.as_str())
            .await
            .map_err(|e| format!("读取 {app_type} 备份失败: {e}"))?
            .is_some();
        let live_taken_over = self.detect_takeover_in_live_config_for_app(&app_type_enum);
        let should_sync_backup = has_backup || live_taken_over;

        self.db
            .set_current_provider(app_type_enum.as_str(), provider_id)
            .map_err(|e| format!("更新当前供应商失败: {e}"))?;
        crate::settings::set_current_provider(&app_type_enum, Some(provider_id))
            .map_err(|e| format!("更新本地当前供应商失败: {e}"))?;

        if should_sync_backup {
            self.update_live_backup_from_provider(app_type, &provider)
                .await?;
        }

        if let Some(server) = self.runtime.server.read().await.as_ref() {
            server
                .set_active_target(app_type_enum.as_str(), &provider.id, &provider.name)
                .await;
        }

        Ok(HotSwitchOutcome {
            logical_target_changed,
        })
    }

    pub async fn switch_proxy_target(
        &self,
        app_type: &str,
        provider_id: &str,
    ) -> Result<(), String> {
        let outcome = self.hot_switch_provider(app_type, provider_id).await?;

        if outcome.logical_target_changed {
            log::info!("代理模式：已切换 {app_type} 的目标供应商为 {provider_id}");
        } else {
            log::debug!("代理模式：{app_type} 已对齐到目标供应商 {provider_id}");
        }

        Ok(())
    }

    fn preserve_codex_mcp_servers_in_backup(
        target_settings: &mut Value,
        existing_backup: &Value,
    ) -> Result<(), String> {
        let target_obj = target_settings
            .as_object_mut()
            .ok_or_else(|| "Codex live backup must be a JSON object".to_string())?;

        let target_config = target_obj
            .get("config")
            .and_then(Value::as_str)
            .unwrap_or("");
        let mut target_doc = if target_config.trim().is_empty() {
            toml_edit::DocumentMut::new()
        } else {
            target_config
                .parse::<toml_edit::DocumentMut>()
                .map_err(|error| format!("parse new Codex config.toml failed: {error}"))?
        };

        let existing_config = existing_backup
            .get("config")
            .and_then(Value::as_str)
            .unwrap_or("");
        if existing_config.trim().is_empty() {
            target_obj.insert("config".to_string(), json!(target_doc.to_string()));
            return Ok(());
        }

        let existing_doc = existing_config
            .parse::<toml_edit::DocumentMut>()
            .map_err(|error| format!("parse existing Codex backup failed: {error}"))?;

        if let Some(existing_mcp_servers) = existing_doc.get("mcp_servers") {
            match target_doc.get_mut("mcp_servers") {
                Some(target_mcp_servers) => {
                    if let (Some(target_table), Some(existing_table)) = (
                        target_mcp_servers.as_table_like_mut(),
                        existing_mcp_servers.as_table_like(),
                    ) {
                        for (server_id, server_item) in existing_table.iter() {
                            if target_table.get(server_id).is_none() {
                                target_table.insert(server_id, server_item.clone());
                            }
                        }
                    }
                }
                None => {
                    target_doc["mcp_servers"] = existing_mcp_servers.clone();
                }
            }
        }

        target_obj.insert("config".to_string(), json!(target_doc.to_string()));
        Ok(())
    }

    pub async fn save_live_backup_snapshot(
        &self,
        app_type: &str,
        snapshot: &Value,
    ) -> Result<(), String> {
        let app_type = Self::takeover_app_from_str(app_type)?;
        let backup = serde_json::to_string(snapshot).map_err(|error| {
            format!(
                "serialize {} live backup failed: {error}",
                app_type.as_str()
            )
        })?;
        self.db
            .save_live_backup(app_type.as_str(), &backup)
            .await
            .map_err(|error| format!("save {} live backup failed: {error}", app_type.as_str()))
    }

    async fn restore_active_takeovers_on_shutdown_unlocked(&self) -> Result<(), String> {
        for app_type in [AppType::Claude, AppType::Codex, AppType::Gemini] {
            self.disable_takeover_for_app_unlocked(&app_type, false)
                .await?;
        }

        Ok(())
    }

    async fn enable_takeover_for_app_unlocked(&self, app_type: &AppType) -> Result<(), String> {
        if !self.is_running().await {
            let config = self.get_config().await.map_err(|e| e.to_string())?;
            self.start_with_resolved_config_unlocked(config).await?;
        }

        let app_key = app_type.as_str();
        let app_proxy = self
            .db
            .get_proxy_config_for_app(app_key)
            .await
            .map_err(|error| format!("load proxy config for {app_key} failed: {error}"))?;
        let has_backup = self
            .db
            .get_live_backup(app_key)
            .await
            .map_err(|error| format!("load live backup for {app_key} failed: {error}"))?
            .is_some();
        let live_taken_over = self.detect_takeover_in_live_config_for_app(app_type);

        if app_proxy.enabled && has_backup && live_taken_over {
            return Ok(());
        }

        let live = self.read_or_current_provider_live(app_type).await?;
        if !has_backup {
            let backup = serde_json::to_string(&live)
                .map_err(|error| format!("serialize {app_key} live backup failed: {error}"))?;
            self.db
                .save_live_backup(app_key, &backup)
                .await
                .map_err(|error| format!("save {app_key} live backup failed: {error}"))?;
        }
        self.sync_live_config_to_current_provider(app_type, &live)
            .await?;

        let (proxy_url, proxy_codex_base_url) = self.build_proxy_urls().await?;
        let mut taken_over = live;
        self.rewrite_live_for_proxy(app_type, &mut taken_over, &proxy_url, &proxy_codex_base_url)?;
        self.write_live_config_for_app(app_type, &taken_over)?;

        if !app_proxy.enabled {
            let mut updated = app_proxy;
            updated.enabled = true;
            self.db
                .update_proxy_config_for_app(updated)
                .await
                .map_err(|error| format!("set takeover flag for {app_key} failed: {error}"))?;
        }

        Ok(())
    }

    async fn disable_takeover_for_app_unlocked(
        &self,
        app_type: &AppType,
        stop_server_when_last: bool,
    ) -> Result<(), String> {
        let app_key = app_type.as_str();
        let app_proxy = self
            .db
            .get_proxy_config_for_app(app_key)
            .await
            .map_err(|error| format!("load proxy config for {app_key} failed: {error}"))?;
        let has_backup = self
            .db
            .get_live_backup(app_key)
            .await
            .map_err(|error| format!("load live backup for {app_key} failed: {error}"))?
            .is_some();
        let live_taken_over = self.detect_takeover_in_live_config_for_app(app_type);

        if !app_proxy.enabled && !has_backup && !live_taken_over {
            return Ok(());
        }

        if has_backup {
            self.restore_live_config_for_app(app_type).await?;
            self.db
                .delete_live_backup(app_key)
                .await
                .map_err(|error| format!("delete live backup for {app_key} failed: {error}"))?;
        } else if live_taken_over {
            self.restore_live_from_current_provider(app_type).await?;
        }

        if app_proxy.enabled {
            let mut cleared = app_proxy;
            cleared.enabled = false;
            self.db
                .update_proxy_config_for_app(cleared)
                .await
                .map_err(|error| format!("clear takeover flag for {app_key} failed: {error}"))?;
        }

        self.db
            .clear_provider_health_for_app(app_key)
            .await
            .map_err(|error| format!("clear provider health for {app_key} failed: {error}"))?;

        if stop_server_when_last
            && !self
                .db
                .is_live_takeover_active()
                .await
                .map_err(|error| format!("check active takeovers failed: {error}"))?
        {
            self.stop_server_unlocked().await?;
        }

        Ok(())
    }

    async fn restore_live_config_for_app(&self, app_type: &AppType) -> Result<(), String> {
        let app_key = app_type.as_str();
        let Some(backup) = self
            .db
            .get_live_backup(app_key)
            .await
            .map_err(|error| format!("load live backup for {app_key} failed: {error}"))?
        else {
            return Ok(());
        };

        let restored: Value = serde_json::from_str(&backup.original_config)
            .map_err(|error| format!("parse {app_key} live backup failed: {error}"))?;
        self.write_live_config_for_app(app_type, &restored)
    }

    async fn restore_live_from_current_provider(&self, app_type: &AppType) -> Result<(), String> {
        let Some(settings) = self.current_provider_settings(app_type).await? else {
            return self.clear_stale_takeover_from_live_config(app_type);
        };
        self.write_live_config_for_app(app_type, &settings)
    }

    async fn current_provider_settings(&self, app_type: &AppType) -> Result<Option<Value>, String> {
        let Some(current_provider) =
            crate::settings::get_effective_current_provider(self.db.as_ref(), app_type).map_err(
                |error| {
                    format!(
                        "load effective current provider for {} failed: {error}",
                        app_type.as_str()
                    )
                },
            )?
        else {
            return Ok(None);
        };

        self.db
            .get_provider_by_id(&current_provider, app_type.as_str())
            .map_err(|error| {
                format!(
                    "load provider {} for {} failed: {error}",
                    current_provider,
                    app_type.as_str()
                )
            })
            .and_then(|provider| {
                provider
                    .map(|provider| {
                        self.build_current_provider_restore_snapshot(app_type, &provider)
                    })
                    .transpose()
            })
    }

    fn build_current_provider_restore_snapshot(
        &self,
        app_type: &AppType,
        provider: &Provider,
    ) -> Result<Value, String> {
        let common_config_snippet =
            self.db
                .get_config_snippet(app_type.as_str())
                .map_err(|error| {
                    format!(
                        "load common config snippet for {} failed: {error}",
                        app_type.as_str()
                    )
                })?;
        let apply_common_config = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.apply_common_config)
            .unwrap_or(true);

        crate::services::provider::ProviderService::build_live_backup_snapshot(
            app_type,
            provider,
            common_config_snippet.as_deref(),
            apply_common_config,
        )
        .and_then(|mut snapshot| {
            if matches!(app_type, AppType::Codex) {
                crate::codex_config::normalize_codex_settings_config_model_provider(
                    &mut snapshot,
                    None,
                )?;
            }
            Ok(snapshot)
        })
        .map_err(|error| {
            format!(
                "build {} current-provider restore snapshot failed: {error}",
                app_type.as_str()
            )
        })
    }

    async fn sync_live_config_to_current_provider(
        &self,
        app_type: &AppType,
        live_config: &Value,
    ) -> Result<(), String> {
        enum LiveTokenSync {
            Claude(&'static str, String),
            Codex(String),
            Gemini(String),
        }

        let Some(token_sync) = (match app_type {
            AppType::Claude => live_config
                .get("env")
                .and_then(Value::as_object)
                .and_then(|env| {
                    [
                        "ANTHROPIC_AUTH_TOKEN",
                        "ANTHROPIC_API_KEY",
                        "OPENROUTER_API_KEY",
                        "OPENAI_API_KEY",
                    ]
                    .into_iter()
                    .find_map(|key| {
                        env.get(key)
                            .and_then(Value::as_str)
                            .map(|value| (key, value.trim().to_string()))
                    })
                })
                .filter(|(_, token)| !token.is_empty() && token != PROXY_TOKEN_PLACEHOLDER)
                .map(|(key, token)| LiveTokenSync::Claude(key, token)),
            AppType::Codex => live_config
                .get("auth")
                .and_then(|auth| auth.get("OPENAI_API_KEY"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|token| !token.is_empty() && *token != PROXY_TOKEN_PLACEHOLDER)
                .map(|token| LiveTokenSync::Codex(token.to_string())),
            AppType::Gemini => live_config
                .get("env")
                .and_then(|env| env.get("GEMINI_API_KEY"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|token| !token.is_empty() && *token != PROXY_TOKEN_PLACEHOLDER)
                .map(|token| LiveTokenSync::Gemini(token.to_string())),
            _ => None,
        }) else {
            return Ok(());
        };

        let Some(provider_id) =
            crate::settings::get_effective_current_provider(self.db.as_ref(), app_type).map_err(
                |error| {
                    format!(
                        "load effective current provider for {} failed: {error}",
                        app_type.as_str()
                    )
                },
            )?
        else {
            return Ok(());
        };

        let Some(mut provider) = self
            .db
            .get_provider_by_id(&provider_id, app_type.as_str())
            .map_err(|error| {
                format!(
                    "load provider {} for {} failed: {error}",
                    provider_id,
                    app_type.as_str()
                )
            })?
        else {
            return Ok(());
        };

        let Some(root) = (match &mut provider.settings_config {
            Value::Object(root) => Some(root),
            Value::Null => {
                provider.settings_config = json!({});
                provider.settings_config.as_object_mut()
            }
            _ => None,
        }) else {
            log::warn!(
                "skip syncing {} live token because provider {} settings root is not an object",
                app_type.as_str(),
                provider_id
            );
            return Ok(());
        };

        match token_sync {
            LiveTokenSync::Claude(token_key, token) => {
                let env_value = root.entry("env".to_string()).or_insert_with(|| json!({}));
                if !env_value.is_object() {
                    *env_value = json!({});
                }

                let Some(env) = env_value.as_object_mut() else {
                    log::warn!(
                        "skip syncing {} live token because provider {} env is not an object",
                        app_type.as_str(),
                        provider_id
                    );
                    return Ok(());
                };

                if matches!(token_key, "ANTHROPIC_AUTH_TOKEN" | "ANTHROPIC_API_KEY") {
                    let mut updated = false;
                    if env.contains_key("ANTHROPIC_AUTH_TOKEN") {
                        env.insert("ANTHROPIC_AUTH_TOKEN".to_string(), json!(token));
                        updated = true;
                    }
                    if env.contains_key("ANTHROPIC_API_KEY") {
                        env.insert("ANTHROPIC_API_KEY".to_string(), json!(token));
                        updated = true;
                    }
                    if !updated {
                        env.insert(token_key.to_string(), json!(token));
                    }
                } else {
                    env.insert(token_key.to_string(), json!(token));
                }
            }
            LiveTokenSync::Codex(token) => {
                let auth_value = root.entry("auth".to_string()).or_insert_with(|| json!({}));
                if !auth_value.is_object() {
                    *auth_value = json!({});
                }

                let Some(auth) = auth_value.as_object_mut() else {
                    log::warn!(
                        "skip syncing {} live token because provider {} auth is not an object",
                        app_type.as_str(),
                        provider_id
                    );
                    return Ok(());
                };

                auth.insert("OPENAI_API_KEY".to_string(), json!(token));
            }
            LiveTokenSync::Gemini(token) => {
                let env_value = root.entry("env".to_string()).or_insert_with(|| json!({}));
                if !env_value.is_object() {
                    *env_value = json!({});
                }

                let Some(env) = env_value.as_object_mut() else {
                    log::warn!(
                        "skip syncing {} live token because provider {} env is not an object",
                        app_type.as_str(),
                        provider_id
                    );
                    return Ok(());
                };

                env.insert("GEMINI_API_KEY".to_string(), json!(token));
            }
        }

        if let Err(error) = self.db.update_provider_settings_config(
            app_type.as_str(),
            &provider_id,
            &provider.settings_config,
        ) {
            log::warn!(
                "sync {} live token to provider {} failed: {error}",
                app_type.as_str(),
                provider_id
            );
        }

        Ok(())
    }

    async fn read_or_current_provider_live(&self, app_type: &AppType) -> Result<Value, String> {
        match self.read_live_config_for_app(app_type) {
            Ok(live) => Ok(live),
            Err(_) => self
                .current_provider_settings(app_type)
                .await?
                .ok_or_else(|| {
                    format!(
                        "missing live config and current provider for {}",
                        app_type.as_str()
                    )
                }),
        }
    }

    async fn build_proxy_urls(&self) -> Result<(String, String), String> {
        let runtime_status = self.get_status().await;
        let persisted = self.get_config().await.map_err(|e| e.to_string())?;
        let listen_address = if runtime_status.running && !runtime_status.address.is_empty() {
            runtime_status.address
        } else {
            persisted.listen_address.clone()
        };
        let listen_port = if runtime_status.running && runtime_status.port != 0 {
            runtime_status.port
        } else {
            persisted.listen_port
        };

        let connect_host = match listen_address.as_str() {
            "0.0.0.0" => "127.0.0.1".to_string(),
            "::" => "::1".to_string(),
            _ => listen_address,
        };
        let connect_host_for_url = if connect_host.contains(':') && !connect_host.starts_with('[') {
            format!("[{connect_host}]")
        } else {
            connect_host
        };

        let proxy_origin = format!("http://{}:{}", connect_host_for_url, listen_port);
        let proxy_codex_base_url = format!("{}/v1", proxy_origin.trim_end_matches('/'));
        Ok((proxy_origin, proxy_codex_base_url))
    }

    fn rewrite_live_for_proxy(
        &self,
        app_type: &AppType,
        live: &mut Value,
        proxy_url: &str,
        proxy_codex_base_url: &str,
    ) -> Result<(), String> {
        match app_type {
            AppType::Claude => {
                if !live.is_object() {
                    *live = json!({});
                }

                let root = live
                    .as_object_mut()
                    .ok_or_else(|| "claude live config root must be an object".to_string())?;
                if !root.get("env").is_some_and(Value::is_object) {
                    root.insert("env".to_string(), json!({}));
                }

                let env = root
                    .get_mut("env")
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| "claude env must be an object".to_string())?;
                env.insert("ANTHROPIC_BASE_URL".to_string(), json!(proxy_url));
                for key in CLAUDE_MODEL_OVERRIDE_ENV_KEYS {
                    env.remove(key);
                }

                let token_keys = [
                    "ANTHROPIC_AUTH_TOKEN",
                    "ANTHROPIC_API_KEY",
                    "OPENROUTER_API_KEY",
                    "OPENAI_API_KEY",
                ];
                let mut replaced_any = false;
                for key in token_keys {
                    if env.contains_key(key) {
                        env.insert(key.to_string(), json!(PROXY_TOKEN_PLACEHOLDER));
                        replaced_any = true;
                    }
                }

                if !replaced_any {
                    env.insert(
                        "ANTHROPIC_AUTH_TOKEN".to_string(),
                        json!(PROXY_TOKEN_PLACEHOLDER),
                    );
                }
            }
            AppType::Codex => {
                if !live.is_object() {
                    *live = json!({});
                }

                let root = live
                    .as_object_mut()
                    .ok_or_else(|| "codex live config root must be an object".to_string())?;
                if !root.get("auth").is_some_and(Value::is_object) {
                    root.insert("auth".to_string(), json!({}));
                }

                let auth = root
                    .get_mut("auth")
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| "codex auth must be an object".to_string())?;
                auth.insert("OPENAI_API_KEY".to_string(), json!(PROXY_TOKEN_PLACEHOLDER));

                let config_text = root
                    .get("config")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                root.insert(
                    "config".to_string(),
                    json!(codex_toml::update_toml_base_url(
                        &config_text,
                        proxy_codex_base_url
                    )),
                );
            }
            AppType::Gemini => {
                if !live.is_object() {
                    *live = json!({});
                }

                let root = live
                    .as_object_mut()
                    .ok_or_else(|| "gemini live config root must be an object".to_string())?;
                if !root.get("env").is_some_and(Value::is_object) {
                    root.insert("env".to_string(), json!({}));
                }

                let env = root
                    .get_mut("env")
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| "gemini env must be an object".to_string())?;
                env.insert("GOOGLE_GEMINI_BASE_URL".to_string(), json!(proxy_url));
                env.insert("GEMINI_API_KEY".to_string(), json!(PROXY_TOKEN_PLACEHOLDER));
            }
            _ => {
                return Err(format!(
                    "proxy takeover not supported for {}",
                    app_type.as_str()
                ));
            }
        }

        Ok(())
    }

    fn read_live_config_for_app(&self, app_type: &AppType) -> Result<Value, String> {
        match app_type {
            AppType::Claude => self.read_claude_live(),
            AppType::Codex => self.read_codex_live(),
            AppType::Gemini => self.read_gemini_live(),
            _ => Err(format!(
                "proxy takeover not supported for {}",
                app_type.as_str()
            )),
        }
    }

    fn write_live_config_for_app(&self, app_type: &AppType, config: &Value) -> Result<(), String> {
        match app_type {
            AppType::Claude => self.write_claude_live(config),
            AppType::Codex => self.write_codex_live(config),
            AppType::Gemini => self.write_gemini_live(config),
            _ => Err(format!(
                "proxy takeover not supported for {}",
                app_type.as_str()
            )),
        }
    }

    fn clear_stale_takeover_from_live_config(&self, app_type: &AppType) -> Result<(), String> {
        let mut live = match self.read_live_config_for_app(app_type) {
            Ok(live) => live,
            Err(_) => return Ok(()),
        };

        match app_type {
            AppType::Claude => {
                if let Some(env) = live.get_mut("env").and_then(Value::as_object_mut) {
                    if env
                        .get("ANTHROPIC_BASE_URL")
                        .and_then(Value::as_str)
                        .is_some_and(codex_toml::is_loopback_proxy_url)
                    {
                        env.remove("ANTHROPIC_BASE_URL");
                    }

                    for key in [
                        "ANTHROPIC_AUTH_TOKEN",
                        "ANTHROPIC_API_KEY",
                        "OPENROUTER_API_KEY",
                        "OPENAI_API_KEY",
                    ] {
                        if env
                            .get(key)
                            .and_then(Value::as_str)
                            .is_some_and(|value| value == PROXY_TOKEN_PLACEHOLDER)
                        {
                            env.remove(key);
                        }
                    }
                }
            }
            AppType::Codex => {
                if let Some(auth) = live.get_mut("auth").and_then(Value::as_object_mut) {
                    if auth
                        .get("OPENAI_API_KEY")
                        .and_then(Value::as_str)
                        .is_some_and(|value| value == PROXY_TOKEN_PLACEHOLDER)
                    {
                        auth.remove("OPENAI_API_KEY");
                    }
                }

                if let Some(config_text) = live.get("config").and_then(Value::as_str) {
                    live["config"] =
                        json!(codex_toml::remove_loopback_base_url_from_toml(config_text));
                }
            }
            AppType::Gemini => {
                if let Some(env) = live.get_mut("env").and_then(Value::as_object_mut) {
                    if env
                        .get("GOOGLE_GEMINI_BASE_URL")
                        .and_then(Value::as_str)
                        .is_some_and(codex_toml::is_loopback_proxy_url)
                    {
                        env.remove("GOOGLE_GEMINI_BASE_URL");
                    }
                    if env
                        .get("GEMINI_API_KEY")
                        .and_then(Value::as_str)
                        .is_some_and(|value| value == PROXY_TOKEN_PLACEHOLDER)
                    {
                        env.remove("GEMINI_API_KEY");
                    }
                }
            }
            _ => {
                return Err(format!(
                    "proxy takeover not supported for {}",
                    app_type.as_str()
                ));
            }
        }

        self.write_live_config_for_app(app_type, &live)
    }

    fn read_claude_live(&self) -> Result<Value, String> {
        let path = get_claude_settings_path();
        if !path.exists() {
            return Err("Claude settings.json does not exist".to_string());
        }

        let value: Value = read_json_file(&path)
            .map_err(|error| format!("read Claude settings.json failed: {error}"))?;
        if value.is_object() {
            Ok(value)
        } else {
            Err("Claude settings.json root must be an object".to_string())
        }
    }

    fn write_claude_live(&self, config: &Value) -> Result<(), String> {
        write_json_file(&get_claude_settings_path(), config)
            .map_err(|error| format!("write Claude settings.json failed: {error}"))
    }

    fn read_codex_live(&self) -> Result<Value, String> {
        let auth_path = get_codex_auth_path();
        let config_path = get_codex_config_path();
        if !auth_path.exists() && !config_path.exists() {
            return Err("Codex live config does not exist".to_string());
        }

        let auth = if auth_path.exists() {
            read_json_file(&auth_path)
                .map_err(|error| format!("read Codex auth.json failed: {error}"))?
        } else {
            json!({})
        };
        let config = if config_path.exists() {
            std::fs::read_to_string(&config_path)
                .map_err(|error| format!("read Codex config.toml failed: {error}"))?
        } else {
            String::new()
        };

        Ok(json!({
            "auth": auth,
            "config": config,
        }))
    }

    fn write_codex_live(&self, config: &Value) -> Result<(), String> {
        let auth = config
            .get("auth")
            .filter(|value| !value.as_object().is_some_and(|object| object.is_empty()));
        let config_text = config.get("config").and_then(Value::as_str);
        let auth_path = get_codex_auth_path();

        // Proxy restore applies the saved backup config without another stable-provider rewrite.
        match (auth, config_text) {
            (Some(auth), Some(config_text)) => write_codex_live_atomic(auth, Some(config_text))
                .map_err(|error| format!("write Codex live config failed: {error}")),
            (Some(auth), None) => write_json_file(&get_codex_auth_path(), auth)
                .map_err(|error| format!("write Codex auth.json failed: {error}")),
            (None, Some(config_text)) => {
                if auth_path.exists() {
                    std::fs::remove_file(&auth_path)
                        .map_err(|error| format!("remove Codex auth.json failed: {error}"))?;
                }
                write_text_file(&get_codex_config_path(), config_text)
                    .map_err(|error| format!("write Codex config.toml failed: {error}"))
            }
            (None, None) => {
                if auth_path.exists() {
                    std::fs::remove_file(&auth_path)
                        .map_err(|error| format!("remove Codex auth.json failed: {error}"))?;
                }
                Ok(())
            }
        }
    }

    fn build_live_snapshot_from_provider(
        &self,
        app_type: &AppType,
        provider: &Provider,
    ) -> Result<Value, String> {
        let common_config_snippet =
            self.db
                .get_config_snippet(app_type.as_str())
                .map_err(|error| {
                    format!(
                        "load common config snippet for {} failed: {error}",
                        app_type.as_str()
                    )
                })?;
        let apply_common_config = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.apply_common_config)
            .unwrap_or(true);

        crate::services::provider::ProviderService::build_live_backup_snapshot(
            app_type,
            provider,
            common_config_snippet.as_deref(),
            apply_common_config,
        )
        .map_err(|error| {
            format!(
                "build {} live snapshot from provider failed: {error}",
                app_type.as_str()
            )
        })
    }

    fn persist_runtime_session(&self, info: &ProxyServerInfo) -> Result<(), String> {
        let session = PersistedProxyRuntimeSession {
            pid: std::process::id(),
            address: info.address.clone(),
            port: info.port,
            started_at: info.started_at.clone(),
            kind: PersistedProxyRuntimeSessionKind::from_env(),
            session_token: std::env::var(PROXY_RUNTIME_SESSION_TOKEN_ENV_KEY)
                .ok()
                .filter(|value| !value.trim().is_empty()),
        };
        let serialized = serde_json::to_string(&session)
            .map_err(|error| format!("serialize proxy runtime session failed: {error}"))?;
        self.db
            .set_setting(PROXY_RUNTIME_SESSION_KEY, &serialized)
            .map_err(|error| format!("persist proxy runtime session failed: {error}"))
    }

    fn defer_runtime_session_publish() -> bool {
        PersistedProxyRuntimeSessionKind::from_env().is_managed_external()
    }

    fn clear_persisted_runtime_session(&self) -> Result<(), String> {
        self.db
            .delete_setting(PROXY_RUNTIME_SESSION_KEY)
            .map_err(|error| format!("clear proxy runtime session failed: {error}"))
    }

    fn load_persisted_runtime_session(&self) -> Option<PersistedProxyRuntimeSession> {
        let raw = self
            .db
            .get_setting(PROXY_RUNTIME_SESSION_KEY)
            .ok()
            .flatten()?;
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }

        match serde_json::from_str(raw) {
            Ok(session) => Some(session),
            Err(_) => {
                let _ = self.clear_persisted_runtime_session();
                None
            }
        }
    }

    fn is_process_alive(pid: u32) -> bool {
        if pid == 0 {
            return false;
        }

        #[cfg(unix)]
        {
            let rc = unsafe { libc::kill(pid as i32, 0) };
            return rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
        }

        #[cfg(not(unix))]
        {
            pid == std::process::id()
        }
    }

    fn has_managed_external_ownership_signal(session: &PersistedProxyRuntimeSession) -> bool {
        session.session_token.is_some() && Self::is_detached_session_leader(session.pid)
    }

    #[cfg(unix)]
    fn is_detached_session_leader(pid: u32) -> bool {
        if pid == 0 {
            return false;
        }

        let sid = unsafe { libc::getsid(pid as i32) };
        let pgid = unsafe { libc::getpgid(pid as i32) };
        sid == pid as i32 && pgid == pid as i32
    }

    #[cfg(not(unix))]
    fn is_detached_session_leader(_pid: u32) -> bool {
        false
    }

    async fn managed_session_ready_info(
        &self,
        child_pid: u32,
        session_token: &str,
    ) -> Option<ProxyServerInfo> {
        let session = self
            .load_persisted_runtime_session()
            .filter(|session| session.pid == child_pid)
            .filter(|session| session.kind.is_managed_external())
            .filter(|session| session.session_token.as_deref() == Some(session_token))?;

        match Self::probe_external_proxy_status(&session).await {
            ExternalProxyStatusProbe::Matched(status) => Some(ProxyServerInfo {
                address: status.address,
                port: status.port,
                started_at: session.started_at,
            }),
            ExternalProxyStatusProbe::Unreachable => Some(ProxyServerInfo {
                address: session.address,
                port: session.port,
                started_at: session.started_at,
            }),
            ExternalProxyStatusProbe::Mismatched => None,
        }
    }

    async fn probe_external_proxy_status(
        session: &PersistedProxyRuntimeSession,
    ) -> ExternalProxyStatusProbe {
        let Some(expected_session_token) = session.session_token.as_deref() else {
            return ExternalProxyStatusProbe::Unreachable;
        };
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(500))
            .build();
        let Ok(client) = client else {
            return ExternalProxyStatusProbe::Unreachable;
        };

        let response = client
            .get(Self::build_session_status_url(session))
            .send()
            .await;
        let Ok(response) = response else {
            return ExternalProxyStatusProbe::Unreachable;
        };
        if !response.status().is_success() {
            return ExternalProxyStatusProbe::Unreachable;
        }

        let mut status = match response.json::<ProxyStatus>().await {
            Ok(status) => status,
            Err(_) => return ExternalProxyStatusProbe::Unreachable,
        };
        if status.managed_session_token.as_deref() != Some(expected_session_token) {
            return ExternalProxyStatusProbe::Mismatched;
        }
        status.running = true;
        if status.address.trim().is_empty() {
            status.address = session.address.clone();
        }
        if status.port == 0 {
            status.port = session.port;
        }
        ExternalProxyStatusProbe::Matched(status)
    }

    fn build_session_status_url(session: &PersistedProxyRuntimeSession) -> String {
        let connect_host = match session.address.as_str() {
            "0.0.0.0" => "127.0.0.1".to_string(),
            "::" => "::1".to_string(),
            value => value.to_string(),
        };
        let connect_host = if connect_host.contains(':') && !connect_host.starts_with('[') {
            format!("[{connect_host}]")
        } else {
            connect_host
        };

        format!("http://{}:{}/status", connect_host, session.port)
    }

    async fn terminate_external_process(pid: u32) -> Result<(), String> {
        if pid == 0 || pid == std::process::id() || !Self::is_process_alive(pid) {
            return Ok(());
        }

        #[cfg(unix)]
        {
            let rc = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
            if rc != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(format!("stop managed proxy session failed: {error}"));
                }
            }

            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while tokio::time::Instant::now() < deadline {
                if !Self::is_process_alive(pid) {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            let rc = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
            if rc != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(format!("force stop managed proxy session failed: {error}"));
                }
            }

            let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
            while tokio::time::Instant::now() < deadline {
                if !Self::is_process_alive(pid) {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }

            return Err(format!(
                "managed proxy session did not exit after termination signal: pid {}",
                pid
            ));
        }

        #[cfg(not(unix))]
        {
            let _ = pid;
            Err("managed proxy session stop is only supported on unix in this build".to_string())
        }
    }

    fn spawn_managed_child_reaper(mut child: std::process::Child) {
        tokio::task::spawn_blocking(move || {
            let _ = child.wait();
        });
    }

    fn ensure_managed_sessions_supported() -> Result<(), String> {
        #[cfg(unix)]
        {
            Ok(())
        }

        #[cfg(not(unix))]
        {
            Err("managed proxy sessions are unsupported on non-unix platforms".to_string())
        }
    }

    fn resolve_managed_proxy_executable() -> Result<std::path::PathBuf, String> {
        if let Some(path) = std::env::var_os("CARGO_BIN_EXE_cc-switch") {
            return Ok(path.into());
        }

        let current_exe = std::env::current_exe()
            .map_err(|error| format!("resolve managed proxy executable failed: {error}"))?;

        if current_exe
            .file_stem()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.starts_with("cc-switch"))
        {
            return Ok(current_exe);
        }

        if let Some(debug_dir) = current_exe.parent().and_then(|parent| parent.parent()) {
            let candidate = debug_dir.join(format!("cc-switch{}", std::env::consts::EXE_SUFFIX));
            if candidate.exists() {
                return Ok(candidate);
            }
        }

        Ok(current_exe)
    }

    async fn sync_persisted_global_proxy_enabled(&self, enabled: bool) -> Result<(), String> {
        let mut config = self
            .db
            .get_global_proxy_config()
            .await
            .map_err(|error| format!("load global proxy config failed: {error}"))?;
        if config.proxy_enabled == enabled {
            return Ok(());
        }

        config.proxy_enabled = enabled;
        self.db
            .update_global_proxy_config(config)
            .await
            .map_err(|error| format!("update global proxy switch failed: {error}"))
    }

    fn read_gemini_live(&self) -> Result<Value, String> {
        let env_path = get_gemini_env_path();
        if !env_path.exists() {
            return Err("Gemini .env does not exist".to_string());
        }

        let env = read_gemini_env().map_err(|error| format!("read Gemini .env failed: {error}"))?;
        Ok(env_to_json(&env))
    }

    fn write_gemini_live(&self, config: &Value) -> Result<(), String> {
        let env =
            json_to_env(config).map_err(|error| format!("build Gemini .env failed: {error}"))?;
        write_gemini_env_atomic(&env).map_err(|error| format!("write Gemini .env failed: {error}"))
    }

    fn takeover_app_from_str(app_type: &str) -> Result<AppType, String> {
        match app_type {
            "claude" => Ok(AppType::Claude),
            "codex" => Ok(AppType::Codex),
            "gemini" => Ok(AppType::Gemini),
            _ => Err(format!("proxy takeover not supported for app: {app_type}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderMeta;
    use crate::proxy::circuit_breaker::CircuitBreakerConfig;
    use crate::test_support::{lock_test_home_and_settings, set_test_home_override};
    use serial_test::serial;
    use std::ffi::OsString;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tempfile::TempDir;

    struct ManagedRuntimeEnvGuard {
        old_kind: Option<OsString>,
        old_token: Option<OsString>,
    }

    impl ManagedRuntimeEnvGuard {
        fn set(token: &str) -> Self {
            let old_kind = std::env::var_os(PROXY_RUNTIME_KIND_ENV_KEY);
            let old_token = std::env::var_os(PROXY_RUNTIME_SESSION_TOKEN_ENV_KEY);
            std::env::set_var(
                PROXY_RUNTIME_KIND_ENV_KEY,
                PersistedProxyRuntimeSessionKind::ManagedExternal.as_env_value(),
            );
            std::env::set_var(PROXY_RUNTIME_SESSION_TOKEN_ENV_KEY, token);
            Self {
                old_kind,
                old_token,
            }
        }
    }

    impl Drop for ManagedRuntimeEnvGuard {
        fn drop(&mut self) {
            match &self.old_kind {
                Some(value) => std::env::set_var(PROXY_RUNTIME_KIND_ENV_KEY, value),
                None => std::env::remove_var(PROXY_RUNTIME_KIND_ENV_KEY),
            }
            match &self.old_token {
                Some(value) => std::env::set_var(PROXY_RUNTIME_SESSION_TOKEN_ENV_KEY, value),
                None => std::env::remove_var(PROXY_RUNTIME_SESSION_TOKEN_ENV_KEY),
            }
        }
    }

    struct TestHomeEnvGuard {
        _lock: crate::test_support::TestHomeSettingsLock,
        old_home: Option<OsString>,
        old_userprofile: Option<OsString>,
    }

    impl TestHomeEnvGuard {
        fn set(home: &Path) -> Self {
            let lock = lock_test_home_and_settings();
            let old_home = std::env::var_os("HOME");
            let old_userprofile = std::env::var_os("USERPROFILE");
            std::env::set_var("HOME", home);
            std::env::set_var("USERPROFILE", home);
            set_test_home_override(Some(home));
            crate::settings::reload_test_settings();
            Self {
                _lock: lock,
                old_home,
                old_userprofile,
            }
        }
    }

    impl Drop for TestHomeEnvGuard {
        fn drop(&mut self) {
            match &self.old_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match &self.old_userprofile {
                Some(value) => std::env::set_var("USERPROFILE", value),
                None => std::env::remove_var("USERPROFILE"),
            }
            set_test_home_override(self.old_home.as_deref().map(Path::new));
            crate::settings::reload_test_settings();
        }
    }

    #[tokio::test]
    #[serial]
    async fn recover_takeovers_on_startup_cleans_claude_placeholder_only_residue_without_backup() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestHomeEnvGuard::set(temp_home.path());
        std::fs::create_dir_all(
            get_claude_settings_path()
                .parent()
                .expect("claude settings parent dir"),
        )
        .expect("create ~/.claude");

        write_json_file(
            &get_claude_settings_path(),
            &json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": PROXY_TOKEN_PLACEHOLDER,
                    "ANTHROPIC_BASE_URL": "https://api.anthropic.com"
                }
            }),
        )
        .expect("seed placeholder-only claude live config");

        let db = Arc::new(Database::memory().expect("create database"));
        let service = ProxyService::new(db);

        service
            .recover_takeovers_on_startup()
            .await
            .expect("startup recovery should clean placeholder-only residue");

        let live: Value =
            read_json_file(&get_claude_settings_path()).expect("read claude live config");
        let env = live
            .get("env")
            .and_then(Value::as_object)
            .expect("claude env should be object");
        assert!(
            !env.contains_key("ANTHROPIC_AUTH_TOKEN"),
            "startup recovery should remove stale proxy placeholder tokens even without loopback base_url"
        );
        assert_eq!(
            env.get("ANTHROPIC_BASE_URL").and_then(Value::as_str),
            Some("https://api.anthropic.com"),
            "startup recovery should keep a non-proxy base_url when only clearing stale placeholder residue"
        );
    }

    #[tokio::test]
    #[serial]
    async fn enabling_claude_takeover_syncs_live_token_back_to_current_provider() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestHomeEnvGuard::set(temp_home.path());
        std::fs::create_dir_all(
            get_claude_settings_path()
                .parent()
                .expect("claude settings parent dir"),
        )
        .expect("create ~/.claude");

        write_json_file(
            &get_claude_settings_path(),
            &json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://api.anthropic.com",
                    "ANTHROPIC_AUTH_TOKEN": "fresh-live-token"
                }
            }),
        )
        .expect("seed claude live config");

        let db = Arc::new(Database::memory().expect("create database"));
        let service = ProxyService::new(db.clone());

        let provider = Provider::with_id(
            "claude-provider".to_string(),
            "Claude Provider".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://api.anthropic.com",
                    "ANTHROPIC_AUTH_TOKEN": "stale-provider-token"
                }
            }),
            None,
        );
        db.save_provider("claude", &provider)
            .expect("save claude provider");
        db.set_current_provider("claude", &provider.id)
            .expect("set current claude provider");

        let mut runtime_config = service.get_config().await.expect("get proxy config");
        runtime_config.listen_port = 0;
        service
            .update_config(&runtime_config)
            .await
            .expect("persist runtime config");

        service
            .set_takeover_for_app("claude", true)
            .await
            .expect("enable claude takeover");
        service.stop().await.expect("stop proxy runtime");

        let updated = db
            .get_provider_by_id("claude-provider", "claude")
            .expect("read claude provider")
            .expect("claude provider exists");
        let env = updated
            .settings_config
            .get("env")
            .and_then(Value::as_object)
            .expect("provider env should exist");

        assert_eq!(
            env.get("ANTHROPIC_AUTH_TOKEN").and_then(Value::as_str),
            Some("fresh-live-token"),
            "enabling takeover should sync the live Claude token back to the current provider before takeover rewrites it"
        );
        assert!(
            !env.contains_key("ANTHROPIC_API_KEY"),
            "sync should not introduce ANTHROPIC_API_KEY when the provider only used ANTHROPIC_AUTH_TOKEN"
        );
    }

    #[tokio::test]
    #[serial]
    async fn enabling_codex_takeover_syncs_live_token_back_to_current_provider() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestHomeEnvGuard::set(temp_home.path());
        std::fs::create_dir_all(
            get_codex_auth_path()
                .parent()
                .expect("codex auth parent dir"),
        )
        .expect("create ~/.codex");

        write_json_file(
            &get_codex_auth_path(),
            &json!({
                "OPENAI_API_KEY": "fresh-live-token"
            }),
        )
        .expect("seed codex auth.json");
        write_text_file(
            &get_codex_config_path(),
            r#"model_provider = "default"

[model_providers.default]
base_url = "https://api.openai.com/v1"
"#,
        )
        .expect("seed codex config.toml");

        let db = Arc::new(Database::memory().expect("create database"));
        let service = ProxyService::new(db.clone());

        let provider = Provider::with_id(
            "codex-provider".to_string(),
            "Codex Provider".to_string(),
            json!({
                "auth": {
                    "OPENAI_API_KEY": "stale-provider-token"
                },
                "config": r#"model_provider = "default"

[model_providers.default]
base_url = "https://api.openai.com/v1"
"#
            }),
            None,
        );
        db.save_provider("codex", &provider)
            .expect("save codex provider");
        db.set_current_provider("codex", &provider.id)
            .expect("set current codex provider");

        let mut runtime_config = service.get_config().await.expect("get proxy config");
        runtime_config.listen_port = 0;
        service
            .update_config(&runtime_config)
            .await
            .expect("persist runtime config");

        service
            .set_takeover_for_app("codex", true)
            .await
            .expect("enable codex takeover");
        service.stop().await.expect("stop proxy runtime");

        let updated = db
            .get_provider_by_id("codex-provider", "codex")
            .expect("read codex provider")
            .expect("codex provider exists");
        let auth = updated
            .settings_config
            .get("auth")
            .and_then(Value::as_object)
            .expect("provider auth should exist");

        assert_eq!(
            auth.get("OPENAI_API_KEY").and_then(Value::as_str),
            Some("fresh-live-token"),
            "enabling takeover should sync the live Codex token back to the current provider before takeover rewrites it"
        );
    }

    #[tokio::test]
    #[serial]
    async fn enabling_codex_takeover_syncs_live_token_to_effective_current_provider() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestHomeEnvGuard::set(temp_home.path());
        std::fs::create_dir_all(
            get_codex_auth_path()
                .parent()
                .expect("codex auth parent dir"),
        )
        .expect("create ~/.codex");

        write_json_file(
            &get_codex_auth_path(),
            &json!({
                "OPENAI_API_KEY": "fresh-live-token"
            }),
        )
        .expect("seed codex auth.json");
        write_text_file(
            &get_codex_config_path(),
            r#"model_provider = "default"

[model_providers.default]
base_url = "https://api.openai.com/v1"
"#,
        )
        .expect("seed codex config.toml");

        let db = Arc::new(Database::memory().expect("create database"));
        let service = ProxyService::new(db.clone());

        let db_current = Provider::with_id(
            "codex-db-current".to_string(),
            "Codex DB Current".to_string(),
            json!({
                "auth": {
                    "OPENAI_API_KEY": "db-current-token"
                },
                "config": r#"model_provider = "default"

[model_providers.default]
base_url = "https://api.openai.com/v1"
"#
            }),
            None,
        );
        let local_current = Provider::with_id(
            "codex-local-current".to_string(),
            "Codex Local Current".to_string(),
            json!({
                "auth": {
                    "OPENAI_API_KEY": "local-current-token"
                },
                "config": r#"model_provider = "default"

[model_providers.default]
base_url = "https://api.openai.com/v1"
"#
            }),
            None,
        );
        db.save_provider("codex", &db_current)
            .expect("save db-current codex provider");
        db.save_provider("codex", &local_current)
            .expect("save local-current codex provider");
        db.set_current_provider("codex", &db_current.id)
            .expect("set db current codex provider");
        crate::settings::set_current_provider(&AppType::Codex, Some(&local_current.id))
            .expect("set local current codex provider");

        let mut runtime_config = service.get_config().await.expect("get proxy config");
        runtime_config.listen_port = 0;
        service
            .update_config(&runtime_config)
            .await
            .expect("persist runtime config");

        service
            .set_takeover_for_app("codex", true)
            .await
            .expect("enable codex takeover");
        service.stop().await.expect("stop proxy runtime");

        let updated_local = db
            .get_provider_by_id("codex-local-current", "codex")
            .expect("read local-current codex provider")
            .expect("local-current codex provider exists");
        let local_auth = updated_local
            .settings_config
            .get("auth")
            .and_then(Value::as_object)
            .expect("local-current provider auth should exist");
        assert_eq!(
            local_auth.get("OPENAI_API_KEY").and_then(Value::as_str),
            Some("fresh-live-token"),
            "takeover sync should follow the effective current provider from local settings"
        );

        let unchanged_db = db
            .get_provider_by_id("codex-db-current", "codex")
            .expect("read db-current codex provider")
            .expect("db-current codex provider exists");
        let db_auth = unchanged_db
            .settings_config
            .get("auth")
            .and_then(Value::as_object)
            .expect("db-current provider auth should exist");
        assert_eq!(
            db_auth.get("OPENAI_API_KEY").and_then(Value::as_str),
            Some("db-current-token"),
            "DB current provider should remain unchanged when local settings override it"
        );
    }

    #[tokio::test]
    #[serial]
    async fn enabling_gemini_takeover_syncs_live_token_to_effective_current_provider() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestHomeEnvGuard::set(temp_home.path());

        let db = Arc::new(Database::memory().expect("create database"));
        let service = ProxyService::new(db.clone());

        service
            .write_gemini_live(&json!({
                "env": {
                    "GEMINI_API_KEY": "fresh-live-token"
                }
            }))
            .expect("seed gemini env");

        let db_current = Provider::with_id(
            "gemini-db-current".to_string(),
            "Gemini DB Current".to_string(),
            json!({
                "env": {
                    "GEMINI_API_KEY": "db-current-token"
                }
            }),
            None,
        );
        let local_current = Provider::with_id(
            "gemini-local-current".to_string(),
            "Gemini Local Current".to_string(),
            json!({
                "env": {
                    "GEMINI_API_KEY": "local-current-token"
                }
            }),
            None,
        );
        db.save_provider("gemini", &db_current)
            .expect("save db-current gemini provider");
        db.save_provider("gemini", &local_current)
            .expect("save local-current gemini provider");
        db.set_current_provider("gemini", &db_current.id)
            .expect("set db current gemini provider");
        crate::settings::set_current_provider(&AppType::Gemini, Some(&local_current.id))
            .expect("set local current gemini provider");

        let mut runtime_config = service.get_config().await.expect("get proxy config");
        runtime_config.listen_port = 0;
        service
            .update_config(&runtime_config)
            .await
            .expect("persist runtime config");

        service
            .set_takeover_for_app("gemini", true)
            .await
            .expect("enable gemini takeover");
        service.stop().await.expect("stop proxy runtime");

        let updated_local = db
            .get_provider_by_id("gemini-local-current", "gemini")
            .expect("read local-current gemini provider")
            .expect("local-current gemini provider exists");
        let local_env = updated_local
            .settings_config
            .get("env")
            .and_then(Value::as_object)
            .expect("local-current provider env should exist");
        assert_eq!(
            local_env.get("GEMINI_API_KEY").and_then(Value::as_str),
            Some("fresh-live-token"),
            "takeover sync should follow the effective current provider from local settings"
        );

        let unchanged_db = db
            .get_provider_by_id("gemini-db-current", "gemini")
            .expect("read db-current gemini provider")
            .expect("db-current gemini provider exists");
        let db_env = unchanged_db
            .settings_config
            .get("env")
            .and_then(Value::as_object)
            .expect("db-current provider env should exist");
        assert_eq!(
            db_env.get("GEMINI_API_KEY").and_then(Value::as_str),
            Some("db-current-token"),
            "DB current provider should remain unchanged when local settings override it"
        );
    }

    #[tokio::test]
    #[serial]
    async fn foreground_runtime_start_and_stop_syncs_global_proxy_switch() {
        let db = Arc::new(Database::memory().expect("create database"));
        let service = ProxyService::new(db);

        assert!(
            !service
                .get_global_config()
                .await
                .expect("read initial global proxy config")
                .proxy_enabled,
            "precondition: global proxy switch should start disabled"
        );

        let mut runtime_config = service.get_config().await.expect("get proxy config");
        runtime_config.listen_port = 0;
        service
            .start_with_runtime_config(runtime_config)
            .await
            .expect("start foreground proxy runtime");

        assert!(
            service
                .get_global_config()
                .await
                .expect("read global proxy config after start")
                .proxy_enabled,
            "starting the foreground proxy runtime should enable the persisted global proxy switch"
        );

        service.stop().await.expect("stop foreground proxy runtime");

        assert!(
            !service
                .get_global_config()
                .await
                .expect("read global proxy config after stop")
                .proxy_enabled,
            "stopping the foreground proxy runtime should disable the persisted global proxy switch"
        );
    }

    #[tokio::test]
    #[serial]
    async fn managed_external_runtime_does_not_publish_session_before_ready_signal() {
        let _env = ManagedRuntimeEnvGuard::set("test-managed-session-token");
        let db = Arc::new(Database::memory().expect("create database"));
        let service = ProxyService::new(db.clone());

        let mut runtime_config = service.get_config().await.expect("get proxy config");
        runtime_config.listen_port = 0;
        service
            .start_with_runtime_config(runtime_config)
            .await
            .expect("start proxy runtime");

        assert!(
            db.get_setting(PROXY_RUNTIME_SESSION_KEY)
                .expect("read runtime session")
                .is_none(),
            "managed external runtime should wait to publish its session until takeover is finished"
        );

        service.stop().await.expect("stop proxy runtime");
    }

    #[tokio::test]
    #[serial]
    async fn managed_external_runtime_publishes_session_when_ready_signal_is_sent() {
        let _env = ManagedRuntimeEnvGuard::set("test-managed-session-token");
        let db = Arc::new(Database::memory().expect("create database"));
        let service = ProxyService::new(db.clone());

        let mut runtime_config = service.get_config().await.expect("get proxy config");
        runtime_config.listen_port = 0;
        let info = service
            .start_with_runtime_config(runtime_config)
            .await
            .expect("start proxy runtime");

        service
            .publish_runtime_session_if_needed(&info)
            .expect("publish managed runtime session");

        let session: PersistedProxyRuntimeSession = serde_json::from_str(
            &db.get_setting(PROXY_RUNTIME_SESSION_KEY)
                .expect("read runtime session")
                .expect("persisted runtime session"),
        )
        .expect("parse runtime session");
        assert!(session.kind.is_managed_external());
        assert_eq!(
            session.session_token.as_deref(),
            Some("test-managed-session-token")
        );

        service.stop().await.expect("stop proxy runtime");
    }

    #[tokio::test]
    #[serial]
    async fn takeover_mutation_waits_for_shared_restore_guard() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestHomeEnvGuard::set(temp_home.path());
        std::fs::create_dir_all(
            get_claude_settings_path()
                .parent()
                .expect("claude settings parent dir"),
        )
        .expect("create claude settings dir");
        write_json_file(
            &get_claude_settings_path(),
            &json!({
                "env": {
                    "ANTHROPIC_API_KEY": "live-key"
                }
            }),
        )
        .expect("seed claude live config");

        let db = Arc::new(Database::init().expect("create database"));
        let provider = Provider::with_id(
            "claude-provider".to_string(),
            "Claude Provider".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_API_KEY": "db-key"
                }
            }),
            Some("claude".to_string()),
        );
        db.save_provider("claude", &provider)
            .expect("save claude provider");
        db.set_current_provider("claude", &provider.id)
            .expect("set current claude provider");

        let service = ProxyService::new(db.clone());
        let mut config = service.get_config().await.expect("read proxy config");
        config.listen_port = 0;
        service
            .update_config(&config)
            .await
            .expect("update proxy config");

        let guard = crate::services::state_coordination::acquire_restore_mutation_guard()
            .await
            .expect("acquire shared restore guard");
        let completed = Arc::new(AtomicBool::new(false));
        let completed_bg = Arc::clone(&completed);
        let service_bg = service.clone();
        let handle = tokio::spawn(async move {
            service_bg
                .set_takeover_for_app("claude", true)
                .await
                .expect("enable takeover after guard release");
            completed_bg.store(true, Ordering::SeqCst);
        });

        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            !completed.load(Ordering::SeqCst),
            "takeover mutation should wait behind the shared restore guard"
        );
        assert!(
            !handle.is_finished(),
            "spawned takeover task should still be blocked on the shared guard"
        );

        drop(guard);

        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("takeover task should complete after guard release")
            .expect("takeover task should succeed");
        assert!(
            completed.load(Ordering::SeqCst),
            "takeover mutation should complete once the shared guard is released"
        );

        service
            .set_takeover_for_app("claude", false)
            .await
            .expect("disable takeover for cleanup");
    }

    #[tokio::test]
    #[serial]
    async fn proxy_config_update_waits_for_shared_restore_guard() {
        let db = Arc::new(Database::memory().expect("create database"));
        let service = ProxyService::new(db.clone());

        let initial = service.get_config().await.expect("read initial config");
        let mut updated = initial.clone();
        updated.listen_port = if initial.listen_port == 15721 {
            15722
        } else {
            initial.listen_port.saturating_add(1)
        };
        let expected_port = updated.listen_port;

        let guard = crate::services::state_coordination::acquire_restore_mutation_guard()
            .await
            .expect("acquire shared restore guard");
        let completed = Arc::new(AtomicBool::new(false));
        let completed_bg = Arc::clone(&completed);
        let service_bg = service.clone();
        let handle = tokio::spawn(async move {
            service_bg
                .update_config(&updated)
                .await
                .expect("update proxy config after guard release");
            completed_bg.store(true, Ordering::SeqCst);
        });

        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            !completed.load(Ordering::SeqCst),
            "proxy config update should wait behind the shared restore guard"
        );
        assert!(
            !handle.is_finished(),
            "spawned config-update task should still be blocked on the shared guard"
        );

        drop(guard);

        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("config update task should complete after guard release")
            .expect("config update task should succeed");
        assert!(
            completed.load(Ordering::SeqCst),
            "proxy config update should complete once the shared guard is released"
        );

        assert_eq!(
            service
                .get_config()
                .await
                .expect("read config after guard release")
                .listen_port,
            expected_port
        );
    }

    #[tokio::test]
    #[serial]
    async fn managed_session_ready_info_accepts_persisted_session_without_status_probe() {
        let db = Arc::new(Database::memory().expect("create database"));
        let service = ProxyService::new(db.clone());

        db.set_setting(
            PROXY_RUNTIME_SESSION_KEY,
            &serde_json::to_string(&PersistedProxyRuntimeSession {
                pid: 4242,
                address: "127.0.0.1".to_string(),
                port: 15721,
                started_at: "2026-03-10T00:00:00Z".to_string(),
                kind: PersistedProxyRuntimeSessionKind::ManagedExternal,
                session_token: Some("expected-session-token".to_string()),
            })
            .expect("serialize runtime session"),
        )
        .expect("persist runtime session");

        let info = service
            .managed_session_ready_info(4242, "expected-session-token")
            .await
            .expect("persisted managed runtime marker should be treated as ready");

        assert_eq!(info.address, "127.0.0.1");
        assert_eq!(info.port, 15721);
        assert_eq!(info.started_at, "2026-03-10T00:00:00Z");
    }

    #[tokio::test]
    async fn managed_session_ready_info_prefers_matching_status_snapshot_when_available() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind fake proxy status listener");
        let port = listener
            .local_addr()
            .expect("read fake proxy listener addr")
            .port();
        let status = serde_json::json!({
            "running": true,
            "address": "127.0.0.1",
            "port": port,
            "active_connections": 0,
            "total_requests": 0,
            "success_requests": 0,
            "failed_requests": 0,
            "success_rate": 0.0,
            "uptime_seconds": 12,
            "current_provider": null,
            "current_provider_id": null,
            "last_request_at": null,
            "last_error": null,
            "failover_count": 0,
            "managed_session_token": "expected-session-token"
        });

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept status request");
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                status.to_string().len(),
                status
            );
            use tokio::io::AsyncWriteExt;
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write fake status response");
        });

        let db = Arc::new(Database::memory().expect("create database"));
        let service = ProxyService::new(db.clone());
        db.set_setting(
            PROXY_RUNTIME_SESSION_KEY,
            &serde_json::to_string(&PersistedProxyRuntimeSession {
                pid: 4242,
                address: "127.0.0.1".to_string(),
                port,
                started_at: "2026-03-10T00:00:00Z".to_string(),
                kind: PersistedProxyRuntimeSessionKind::ManagedExternal,
                session_token: Some("expected-session-token".to_string()),
            })
            .expect("serialize runtime session"),
        )
        .expect("persist runtime session");

        let info = service
            .managed_session_ready_info(4242, "expected-session-token")
            .await
            .expect("matching external status should make the session ready");

        assert_eq!(info.address, "127.0.0.1");
        assert_eq!(info.port, port);

        server.await.expect("fake status server should finish");
    }

    #[tokio::test]
    async fn managed_session_ready_info_rejects_mismatched_status_snapshot() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind fake proxy status listener");
        let port = listener
            .local_addr()
            .expect("read fake proxy listener addr")
            .port();
        let status = serde_json::json!({
            "running": true,
            "address": "127.0.0.1",
            "port": port,
            "active_connections": 0,
            "total_requests": 0,
            "success_requests": 0,
            "failed_requests": 0,
            "success_rate": 0.0,
            "uptime_seconds": 12,
            "current_provider": null,
            "current_provider_id": null,
            "last_request_at": null,
            "last_error": null,
            "failover_count": 0,
            "managed_session_token": "other-session-token"
        });

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept status request");
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                status.to_string().len(),
                status
            );
            use tokio::io::AsyncWriteExt;
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write fake status response");
        });

        let db = Arc::new(Database::memory().expect("create database"));
        let service = ProxyService::new(db.clone());
        db.set_setting(
            PROXY_RUNTIME_SESSION_KEY,
            &serde_json::to_string(&PersistedProxyRuntimeSession {
                pid: 4242,
                address: "127.0.0.1".to_string(),
                port,
                started_at: "2026-03-10T00:00:00Z".to_string(),
                kind: PersistedProxyRuntimeSessionKind::ManagedExternal,
                session_token: Some("expected-session-token".to_string()),
            })
            .expect("serialize runtime session"),
        )
        .expect("persist runtime session");

        assert!(
            service
                .managed_session_ready_info(4242, "expected-session-token")
                .await
                .is_none(),
            "startup should keep waiting when /status reports a different managed session token"
        );

        server.await.expect("fake status server should finish");
    }

    #[tokio::test]
    #[serial]
    async fn hot_updating_running_breaker_configs_refreshes_existing_breakers() {
        let db = Arc::new(Database::memory().expect("create database"));
        let service = ProxyService::new(db.clone());

        let provider = Provider::with_id(
            "p1".to_string(),
            "Provider One".to_string(),
            json!({}),
            None,
        );
        db.save_provider("claude", &provider)
            .expect("save provider");

        let mut app_proxy = db
            .get_proxy_config_for_app("claude")
            .await
            .expect("load app proxy config");
        app_proxy.circuit_failure_threshold = 1;
        app_proxy.circuit_timeout_seconds = 3600;
        db.update_proxy_config_for_app(app_proxy)
            .await
            .expect("persist initial breaker config");

        let mut runtime_config = service.get_config().await.expect("get proxy config");
        runtime_config.listen_port = 0;
        service
            .start_with_runtime_config(runtime_config)
            .await
            .expect("start proxy");

        let router = {
            let server_guard = service.runtime.server.read().await;
            server_guard
                .as_ref()
                .expect("running server")
                .provider_router()
        };

        router
            .record_result("p1", "claude", false, false, Some("fail".to_string()))
            .await
            .expect("open breaker");
        assert!(!router.allow_provider_request("p1", "claude").await.allowed);

        let updated = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 2,
            timeout_seconds: 0,
            error_rate_threshold: 1.0,
            min_requests: u32::MAX,
        };
        db.update_circuit_breaker_config(&updated)
            .await
            .expect("persist updated breaker config");
        service
            .update_circuit_breaker_configs(updated)
            .await
            .expect("hot update running breaker config");

        let permit = router.allow_provider_request("p1", "claude").await;
        assert!(permit.allowed);
        assert!(permit.used_half_open_permit);

        service.stop().await.expect("stop proxy");
    }

    #[tokio::test]
    #[serial]
    async fn resetting_running_provider_breaker_clears_existing_breaker_state() {
        let db = Arc::new(Database::memory().expect("create database"));
        let service = ProxyService::new(db.clone());

        let provider = Provider::with_id(
            "p1".to_string(),
            "Provider One".to_string(),
            json!({}),
            None,
        );
        db.save_provider("claude", &provider)
            .expect("save provider");

        let mut app_proxy = db
            .get_proxy_config_for_app("claude")
            .await
            .expect("load app proxy config");
        app_proxy.circuit_failure_threshold = 1;
        app_proxy.circuit_timeout_seconds = 3600;
        db.update_proxy_config_for_app(app_proxy)
            .await
            .expect("persist breaker config");

        let mut runtime_config = service.get_config().await.expect("get proxy config");
        runtime_config.listen_port = 0;
        service
            .start_with_runtime_config(runtime_config)
            .await
            .expect("start proxy");

        let router = {
            let server_guard = service.runtime.server.read().await;
            server_guard
                .as_ref()
                .expect("running server")
                .provider_router()
        };

        router
            .record_result("p1", "claude", false, false, Some("fail".to_string()))
            .await
            .expect("open breaker");
        assert!(!router.allow_provider_request("p1", "claude").await.allowed);

        service
            .reset_provider_circuit_breaker("p1", "claude")
            .await
            .expect("reset running breaker");

        let permit = router.allow_provider_request("p1", "claude").await;
        assert!(permit.allowed);
        assert!(!permit.used_half_open_permit);

        service.stop().await.expect("stop proxy");
    }

    #[tokio::test]
    #[serial]
    async fn update_live_backup_from_provider_preserves_codex_mcp_servers() {
        let db = Arc::new(Database::memory().expect("init db"));
        let service = ProxyService::new(db.clone());

        db.save_live_backup(
            "codex",
            &serde_json::to_string(&json!({
                "auth": {
                    "OPENAI_API_KEY": "old-token"
                },
                "config": r#"model_provider = "any"
model = "gpt-4"

[model_providers.any]
base_url = "https://old.example/v1"

[mcp_servers.echo]
command = "npx"
args = ["echo-server"]
"#
            }))
            .expect("serialize seed backup"),
        )
        .await
        .expect("seed live backup");

        let provider = Provider::with_id(
            "p2".to_string(),
            "P2".to_string(),
            json!({
                "auth": {
                    "OPENAI_API_KEY": "new-token"
                },
                "config": r#"model_provider = "any"
model = "gpt-5"

[model_providers.any]
base_url = "https://new.example/v1"
"#
            }),
            None,
        );

        service
            .update_live_backup_from_provider("codex", &provider)
            .await
            .expect("update live backup");

        let backup = db
            .get_live_backup("codex")
            .await
            .expect("get live backup")
            .expect("backup exists");
        let stored: Value =
            serde_json::from_str(&backup.original_config).expect("parse backup json");
        let config = stored
            .get("config")
            .and_then(|v| v.as_str())
            .expect("config string");

        assert!(
            config.contains("[mcp_servers.echo]"),
            "existing Codex MCP section should survive proxy hot-switch backup update"
        );
        assert!(
            config.contains("https://new.example/v1"),
            "provider-specific base_url should still update to the new provider"
        );
    }

    #[tokio::test]
    #[serial]
    async fn hot_switch_codex_provider_keeps_model_provider_stable_in_backup_and_restore() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestHomeEnvGuard::set(temp_home.path());

        let db = Arc::new(Database::memory().expect("init db"));
        let service = ProxyService::new(db.clone());

        let provider_a = Provider::with_id(
            "a".to_string(),
            "RightCode".to_string(),
            json!({
                "auth": {
                    "OPENAI_API_KEY": "rightcode-key"
                },
                "config": r#"model_provider = "rightcode"
model = "gpt-5.4"

[model_providers.rightcode]
name = "RightCode"
base_url = "https://rightcode.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#
            }),
            None,
        );
        let provider_b = Provider::with_id(
            "b".to_string(),
            "AiHubMix".to_string(),
            json!({
                "auth": {
                    "OPENAI_API_KEY": "aihubmix-key"
                },
                "config": r#"model_provider = "aihubmix"
model = "gpt-5.4"

[model_providers.aihubmix]
name = "AiHubMix"
base_url = "https://aihubmix.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#
            }),
            None,
        );

        db.save_provider("codex", &provider_a)
            .expect("save provider a");
        db.save_provider("codex", &provider_b)
            .expect("save provider b");
        db.set_current_provider("codex", "a")
            .expect("set current provider");
        crate::settings::set_current_provider(&AppType::Codex, Some("a"))
            .expect("set local current provider");
        db.save_live_backup(
            "codex",
            &serde_json::to_string(&provider_a.settings_config).expect("serialize provider a"),
        )
        .await
        .expect("seed live backup");
        service
            .write_codex_live(&json!({
                "auth": {
                    "OPENAI_API_KEY": PROXY_TOKEN_PLACEHOLDER
                },
                "config": r#"model_provider = "rightcode"
model = "gpt-5.4"

[model_providers.rightcode]
name = "RightCode"
base_url = "http://127.0.0.1:15721/v1"
wire_api = "responses"
requires_openai_auth = true
"#
            }))
            .expect("seed taken-over Codex live config");

        service
            .hot_switch_provider("codex", "b")
            .await
            .expect("hot switch Codex provider");

        let backup = db
            .get_live_backup("codex")
            .await
            .expect("get live backup")
            .expect("backup exists");
        let stored: Value =
            serde_json::from_str(&backup.original_config).expect("parse backup json");
        let backup_config = stored
            .get("config")
            .and_then(Value::as_str)
            .expect("backup config string");
        let parsed_backup: toml::Value =
            toml::from_str(backup_config).expect("parse backup config");
        assert_eq!(
            parsed_backup.get("model_provider").and_then(|v| v.as_str()),
            Some("rightcode"),
            "provider-derived restore backup should retain stable Codex model_provider"
        );
        let backup_model_providers = parsed_backup
            .get("model_providers")
            .and_then(|v| v.as_table())
            .expect("backup model_providers");
        assert!(backup_model_providers.get("aihubmix").is_none());
        assert_eq!(
            backup_model_providers
                .get("rightcode")
                .and_then(|v| v.get("base_url"))
                .and_then(|v| v.as_str()),
            Some("https://aihubmix.example/v1"),
            "stable provider id should point at the hot-switched provider endpoint"
        );

        service
            .restore_live_config_for_app(&AppType::Codex)
            .await
            .expect("restore Codex live config");

        let live = service.read_codex_live().expect("read Codex live config");
        let live_config = live
            .get("config")
            .and_then(Value::as_str)
            .expect("live config string");
        let parsed_live: toml::Value = toml::from_str(live_config).expect("parse live config");
        assert_eq!(
            parsed_live.get("model_provider").and_then(|v| v.as_str()),
            Some("rightcode"),
            "restored Codex live config should not switch history buckets"
        );
        assert_eq!(
            live.get("auth")
                .and_then(|auth| auth.get("OPENAI_API_KEY"))
                .and_then(Value::as_str),
            Some("aihubmix-key"),
            "restore should still use the hot-switched provider auth"
        );
    }

    #[tokio::test]
    #[serial]
    async fn update_live_backup_from_provider_for_gemini_keeps_only_env_snapshot() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestHomeEnvGuard::set(temp_home.path());

        let settings_path = crate::gemini_config::get_gemini_settings_path();
        std::fs::create_dir_all(
            settings_path
                .parent()
                .expect("gemini settings parent directory"),
        )
        .expect("create gemini settings directory");
        write_json_file(
            &settings_path,
            &json!({
                "mcpServers": {
                    "echo": {
                        "command": "npx",
                        "args": ["echo-server"]
                    }
                }
            }),
        )
        .expect("seed gemini settings.json");

        let db = Arc::new(Database::memory().expect("init db"));
        let service = ProxyService::new(db.clone());
        db.save_live_backup("gemini", r#"{"env":{"GEMINI_API_KEY":"stale-token"}}"#)
            .await
            .expect("seed active gemini backup");
        let provider = Provider::with_id(
            "gemini-provider".to_string(),
            "Gemini Provider".to_string(),
            json!({
                "env": {
                    "GEMINI_API_KEY": "gemini-token"
                },
                "config": {
                    "theme": "solarized"
                }
            }),
            None,
        );

        service
            .update_live_backup_from_provider("gemini", &provider)
            .await
            .expect("update gemini live backup");

        let backup = db
            .get_live_backup("gemini")
            .await
            .expect("get gemini live backup")
            .expect("gemini backup exists");
        let stored: Value =
            serde_json::from_str(&backup.original_config).expect("parse gemini backup json");

        assert_eq!(
            stored.get("env"),
            Some(&json!({
                "GEMINI_API_KEY": "gemini-token"
            })),
            "Gemini live backup should keep the provider env for takeover restore"
        );
        assert!(
            stored.get("config").is_none(),
            "Gemini live backup should not snapshot settings.json/config; upstream keeps only env"
        );
    }

    #[tokio::test]
    #[serial]
    async fn update_live_backup_from_provider_applies_claude_common_config_without_takeover() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestHomeEnvGuard::set(temp_home.path());

        let db = Arc::new(Database::memory().expect("init db"));
        db.set_config_snippet(
            "claude",
            Some(
                serde_json::json!({
                    "includeCoAuthoredBy": false
                })
                .to_string(),
            ),
        )
        .expect("set common config snippet");

        let service = ProxyService::new(db.clone());

        let mut provider = Provider::with_id(
            "claude-provider".to_string(),
            "Claude Provider".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "token",
                    "ANTHROPIC_BASE_URL": "https://claude.example"
                }
            }),
            None,
        );
        provider.meta = Some(ProviderMeta {
            apply_common_config: Some(true),
            ..Default::default()
        });

        service
            .update_live_backup_from_provider("claude", &provider)
            .await
            .expect("update claude live backup");

        let backup = db
            .get_live_backup("claude")
            .await
            .expect("get claude live backup")
            .expect("claude backup exists");
        let stored: Value =
            serde_json::from_str(&backup.original_config).expect("parse claude backup json");

        assert_eq!(
            stored.get("includeCoAuthoredBy").and_then(Value::as_bool),
            Some(false),
            "common config should be applied into Claude restore backup even before takeover is active"
        );
    }

    #[tokio::test]
    #[serial]
    async fn switch_proxy_target_updates_live_backup_when_taken_over() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestHomeEnvGuard::set(temp_home.path());

        let db = Arc::new(Database::memory().expect("init db"));
        let service = ProxyService::new(db.clone());

        let provider_a = Provider::with_id(
            "a".to_string(),
            "A".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_API_KEY": "a-key"
                }
            }),
            None,
        );
        let provider_b = Provider::with_id(
            "b".to_string(),
            "B".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_API_KEY": "b-key"
                }
            }),
            None,
        );
        db.save_provider("claude", &provider_a)
            .expect("save provider a");
        db.save_provider("claude", &provider_b)
            .expect("save provider b");
        db.set_current_provider("claude", "a")
            .expect("set current provider");

        db.save_live_backup("claude", "{\"env\":{}}")
            .await
            .expect("seed live backup");

        service
            .switch_proxy_target("claude", "b")
            .await
            .expect("switch proxy target");

        assert_eq!(
            crate::settings::get_current_provider(&AppType::Claude).as_deref(),
            Some("b")
        );

        let backup = db
            .get_live_backup("claude")
            .await
            .expect("get live backup")
            .expect("backup exists");
        let expected = serde_json::to_string(&provider_b.settings_config).expect("serialize");
        assert_eq!(backup.original_config, expected);
    }

    #[tokio::test]
    #[serial]
    async fn restore_live_from_current_provider_applies_codex_common_config() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestHomeEnvGuard::set(temp_home.path());

        let db = Arc::new(Database::memory().expect("init db"));
        db.set_config_snippet(
            "codex",
            Some("disable_response_storage = true\n".to_string()),
        )
        .expect("set codex common config snippet");

        let service = ProxyService::new(db.clone());
        let mut provider = Provider::with_id(
            "codex-provider".to_string(),
            "Codex Provider".to_string(),
            json!({
                "auth": {
                    "OPENAI_API_KEY": "codex-token"
                },
                "config": r#"model_provider = "default"
model = "gpt-5.2-codex"

[model_providers.default]
base_url = "https://api.example/v1"
"#
            }),
            None,
        );
        provider.meta = Some(ProviderMeta {
            apply_common_config: Some(true),
            ..Default::default()
        });
        db.save_provider("codex", &provider)
            .expect("save codex provider");
        db.set_current_provider("codex", &provider.id)
            .expect("set current codex provider");

        service
            .restore_live_from_current_provider(&AppType::Codex)
            .await
            .expect("restore codex live from current provider");

        let auth: Value = read_json_file(&get_codex_auth_path()).expect("read codex auth.json");
        assert_eq!(
            auth.get("OPENAI_API_KEY").and_then(Value::as_str),
            Some("codex-token"),
            "Codex fallback restore should write auth.json from the current provider"
        );

        let config_text =
            std::fs::read_to_string(get_codex_config_path()).expect("read codex config.toml");
        assert!(
            config_text.contains("disable_response_storage = true"),
            "Codex fallback restore should apply the common config snippet like upstream"
        );
    }
}
