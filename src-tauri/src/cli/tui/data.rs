use std::path::PathBuf;

use indexmap::IndexMap;
use serde_json::Value;

use crate::app_config::{AppType, CommonConfigSnippets, McpServer};
use crate::error::AppError;
use crate::prompt::Prompt;
use crate::provider::Provider;
use crate::services::config::BackupInfo;
use crate::services::{ConfigService, McpService, PromptService, ProviderService, SkillService};
use crate::store::AppState;

#[derive(Debug, Clone)]
pub struct ProviderRow {
    pub id: String,
    pub provider: Provider,
    pub api_url: Option<String>,
    pub is_current: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ProvidersSnapshot {
    pub current_id: String,
    pub rows: Vec<ProviderRow>,
}

#[derive(Debug, Clone)]
pub struct McpRow {
    pub id: String,
    pub server: McpServer,
}

#[derive(Debug, Clone, Default)]
pub struct McpSnapshot {
    pub rows: Vec<McpRow>,
}

#[derive(Debug, Clone)]
pub struct PromptRow {
    pub id: String,
    pub prompt: Prompt,
}

#[derive(Debug, Clone, Default)]
pub struct PromptsSnapshot {
    pub rows: Vec<PromptRow>,
}

#[derive(Debug, Clone, Default)]
pub struct ConfigSnapshot {
    pub config_path: PathBuf,
    pub config_dir: PathBuf,
    pub backups: Vec<BackupInfo>,
    pub common_snippet: String,
    pub common_snippets: CommonConfigSnippets,
    pub webdav_sync: Option<crate::settings::WebDavSyncSettings>,
}

#[derive(Debug, Clone, Default)]
pub struct SkillsSnapshot {
    pub installed: Vec<crate::services::skill::InstalledSkill>,
    pub repos: Vec<crate::services::skill::SkillRepo>,
    pub sync_method: crate::services::skill::SyncMethod,
}

#[derive(Debug, Clone, Default)]
pub struct ProxyTargetSnapshot {
    pub provider_name: String,
}

#[derive(Debug, Clone, Default)]
pub struct ProxySnapshot {
    pub enabled: bool,
    pub running: bool,
    pub managed_runtime: bool,
    pub claude_takeover: bool,
    pub codex_takeover: bool,
    pub gemini_takeover: bool,
    pub default_cost_multiplier: Option<String>,
    pub configured_listen_address: String,
    pub configured_listen_port: u16,
    pub listen_address: String,
    pub listen_port: u16,
    pub uptime_seconds: u64,
    pub total_requests: u64,
    pub estimated_input_tokens_total: u64,
    pub estimated_output_tokens_total: u64,
    pub success_rate: Option<f32>,
    pub current_provider: Option<String>,
    pub last_error: Option<String>,
    pub current_app_target: Option<ProxyTargetSnapshot>,
}

impl ProxySnapshot {
    pub fn takeover_enabled_for(&self, app_type: &AppType) -> Option<bool> {
        match app_type {
            AppType::Claude => Some(self.claude_takeover),
            AppType::Codex => Some(self.codex_takeover),
            AppType::Gemini => Some(self.gemini_takeover),
            AppType::OpenCode => None,
            AppType::OpenClaw => None,
        }
    }

    pub fn routes_current_app_through_proxy(&self, app_type: &AppType) -> Option<bool> {
        self.takeover_enabled_for(app_type)
            .map(|takeover_enabled| self.running && takeover_enabled)
    }
}

#[derive(Debug, Clone, Default)]
pub struct UiData {
    pub providers: ProvidersSnapshot,
    pub mcp: McpSnapshot,
    pub prompts: PromptsSnapshot,
    pub config: ConfigSnapshot,
    pub skills: SkillsSnapshot,
    pub proxy: ProxySnapshot,
}

pub(crate) fn load_state() -> Result<AppState, AppError> {
    AppState::try_new()
}

impl UiData {
    pub fn load(app_type: &AppType) -> Result<Self, AppError> {
        let state = load_state()?;

        let providers = load_providers(&state, app_type)?;
        let mcp = load_mcp(&state)?;
        let prompts = load_prompts(&state, app_type)?;
        let config = load_config_snapshot(&state, app_type)?;
        let skills = load_skills_snapshot()?;
        let proxy = load_proxy_snapshot(app_type)?;

        Ok(Self {
            providers,
            mcp,
            prompts,
            config,
            skills,
            proxy,
        })
    }

    pub(crate) fn refresh_proxy_snapshot(&mut self, app_type: &AppType) -> Result<(), AppError> {
        self.proxy = load_proxy_snapshot(app_type)?;
        Ok(())
    }
}

fn load_providers(state: &AppState, app_type: &AppType) -> Result<ProvidersSnapshot, AppError> {
    let current_id = ProviderService::current(state, app_type.clone())?;
    let providers = ProviderService::list(state, app_type.clone())?;
    let sorted = sort_providers(&providers);

    let rows = sorted
        .into_iter()
        .map(|(id, provider)| ProviderRow {
            api_url: extract_api_url(&provider.settings_config, app_type),
            is_current: id == current_id,
            id: id.clone(),
            provider,
        })
        .collect::<Vec<_>>();

    Ok(ProvidersSnapshot { current_id, rows })
}

fn sort_providers(providers: &IndexMap<String, Provider>) -> Vec<(String, Provider)> {
    let mut items = providers
        .iter()
        .map(|(id, p)| (id.clone(), p.clone()))
        .collect::<Vec<_>>();

    items.sort_by(|(_, a), (_, b)| match (a.sort_index, b.sort_index) {
        (Some(idx_a), Some(idx_b)) => idx_a.cmp(&idx_b),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.created_at.cmp(&b.created_at),
    });

    items
}

fn extract_api_url(settings_config: &Value, app_type: &AppType) -> Option<String> {
    match app_type {
        AppType::Claude => settings_config
            .get("env")?
            .get("ANTHROPIC_BASE_URL")?
            .as_str()
            .map(|s| s.to_string()),
        AppType::Codex => {
            if let Some(config_str) = settings_config.get("config")?.as_str() {
                for line in config_str.lines() {
                    let line = line.trim();
                    if line.starts_with("base_url") {
                        if let Some(url_part) = line.split('=').nth(1) {
                            let url = url_part.trim().trim_matches('"').trim_matches('\'');
                            if !url.is_empty() {
                                return Some(url.to_string());
                            }
                        }
                    }
                }
            }
            None
        }
        AppType::Gemini => settings_config
            .get("env")
            .and_then(|env| {
                env.get("GOOGLE_GEMINI_BASE_URL")
                    .or_else(|| env.get("GEMINI_BASE_URL"))
                    .or_else(|| env.get("BASE_URL"))
            })?
            .as_str()
            .map(|s| s.to_string()),
        AppType::OpenCode => settings_config
            .get("options")?
            .get("baseURL")?
            .as_str()
            .map(|s| s.to_string()),
        AppType::OpenClaw => crate::openclaw_config::provider_config_from_settings(settings_config)
            .get("baseUrl")?
            .as_str()
            .map(|s| s.to_string()),
    }
}

fn load_mcp(state: &AppState) -> Result<McpSnapshot, AppError> {
    let servers = McpService::get_all_servers(state)?;
    let mut rows = servers
        .into_iter()
        .map(|(id, server)| McpRow { id, server })
        .collect::<Vec<_>>();

    rows.sort_by(|a, b| a.id.cmp(&b.id));

    Ok(McpSnapshot { rows })
}

fn load_prompts(state: &AppState, app_type: &AppType) -> Result<PromptsSnapshot, AppError> {
    let prompts = PromptService::get_prompts(state, app_type.clone())?;
    let mut rows = prompts
        .into_iter()
        .map(|(id, prompt)| PromptRow { id, prompt })
        .collect::<Vec<_>>();

    rows.sort_by(|a, b| {
        b.prompt
            .updated_at
            .unwrap_or(0)
            .cmp(&a.prompt.updated_at.unwrap_or(0))
    });

    Ok(PromptsSnapshot { rows })
}

fn load_config_snapshot(state: &AppState, app_type: &AppType) -> Result<ConfigSnapshot, AppError> {
    let config_dir = crate::config::get_app_config_dir();
    let config_path = config_dir.join("cc-switch.db");
    let backups = ConfigService::list_backups(&config_path)?;
    let (common_snippet, common_snippets) = {
        let guard = state.config.read().map_err(AppError::from)?;
        let common_snippets = guard.common_config_snippets.clone();
        let common_snippet = common_snippets.get(app_type).cloned().unwrap_or_default();
        (common_snippet, common_snippets)
    };

    Ok(ConfigSnapshot {
        config_path,
        config_dir,
        backups,
        common_snippet,
        common_snippets,
        webdav_sync: crate::settings::get_webdav_sync_settings(),
    })
}

pub(crate) fn load_proxy_config() -> Result<Option<crate::proxy::ProxyConfig>, AppError> {
    let state = load_state()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("failed to create async runtime: {e}")))?;

    runtime.block_on(async { state.db.get_proxy_config().await.map(Some) })
}

fn load_proxy_snapshot(app_type: &AppType) -> Result<ProxySnapshot, AppError> {
    let state = load_state()?;
    let current_app = app_type.as_str().to_string();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("failed to create async runtime: {e}")))?;

    runtime.block_on(async {
        let config = state.proxy_service.get_global_config().await?;
        let runtime_status = state.proxy_service.get_status().await;
        let takeover = state
            .proxy_service
            .get_takeover_status()
            .await
            .map_err(AppError::Message)?;

        let current_app_target = runtime_status
            .active_targets
            .iter()
            .find(|target| target.app_type.eq_ignore_ascii_case(&current_app))
            .map(|target| ProxyTargetSnapshot {
                provider_name: target.provider_name.clone(),
            });
        let listen_address = if runtime_status.address.trim().is_empty() {
            config.listen_address.clone()
        } else {
            runtime_status.address.clone()
        };
        let listen_port = if runtime_status.port == 0 {
            config.listen_port
        } else {
            runtime_status.port
        };
        let default_cost_multiplier = state
            .db
            .get_default_cost_multiplier(app_type.as_str())
            .await
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        Ok(ProxySnapshot {
            enabled: config.proxy_enabled,
            running: runtime_status.running,
            managed_runtime: runtime_status.managed_session_token.is_some(),
            claude_takeover: takeover.claude,
            codex_takeover: takeover.codex,
            gemini_takeover: takeover.gemini,
            default_cost_multiplier,
            configured_listen_address: config.listen_address.clone(),
            configured_listen_port: config.listen_port,
            listen_address,
            listen_port,
            uptime_seconds: runtime_status.uptime_seconds,
            total_requests: runtime_status.total_requests,
            estimated_input_tokens_total: runtime_status.estimated_input_tokens_total,
            estimated_output_tokens_total: runtime_status.estimated_output_tokens_total,
            success_rate: (runtime_status.total_requests > 0)
                .then_some(runtime_status.success_rate),
            current_provider: runtime_status
                .current_provider
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            last_error: runtime_status
                .last_error
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            current_app_target,
        })
    })
}

fn load_skills_snapshot() -> Result<SkillsSnapshot, AppError> {
    Ok(SkillsSnapshot {
        installed: SkillService::list_installed()?,
        repos: SkillService::list_repos()?,
        sync_method: SkillService::get_sync_method()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_api_url_gemini_prefers_google_env_key() {
        let settings = json!({
            "env": {
                "GOOGLE_GEMINI_BASE_URL": "https://google.example",
                "GEMINI_BASE_URL": "https://legacy.example",
                "BASE_URL": "https://fallback.example"
            }
        });

        assert_eq!(
            extract_api_url(&settings, &AppType::Gemini),
            Some("https://google.example".to_string())
        );
    }

    #[test]
    fn extract_api_url_gemini_falls_back_to_legacy_keys() {
        let settings = json!({
            "env": {
                "GEMINI_BASE_URL": "https://legacy.example",
                "BASE_URL": "https://fallback.example"
            }
        });

        assert_eq!(
            extract_api_url(&settings, &AppType::Gemini),
            Some("https://legacy.example".to_string())
        );
    }

    #[test]
    fn extract_api_url_opencode_reads_options_base_url() {
        let settings = json!({
            "options": {
                "baseURL": "https://opencode.example"
            }
        });

        assert_eq!(
            extract_api_url(&settings, &AppType::OpenCode),
            Some("https://opencode.example".to_string())
        );
    }

    #[test]
    fn proxy_snapshot_returns_app_specific_takeover_state() {
        let snapshot = ProxySnapshot {
            claude_takeover: true,
            codex_takeover: false,
            gemini_takeover: true,
            ..ProxySnapshot::default()
        };

        assert_eq!(snapshot.takeover_enabled_for(&AppType::Claude), Some(true));
        assert_eq!(snapshot.takeover_enabled_for(&AppType::Codex), Some(false));
        assert_eq!(snapshot.takeover_enabled_for(&AppType::Gemini), Some(true));
        assert_eq!(snapshot.takeover_enabled_for(&AppType::OpenCode), None);
    }

    #[test]
    fn proxy_snapshot_distinguishes_running_route_from_stale_takeover_flag() {
        let active = ProxySnapshot {
            running: true,
            managed_runtime: true,
            claude_takeover: true,
            ..ProxySnapshot::default()
        };
        assert_eq!(
            active.routes_current_app_through_proxy(&AppType::Claude),
            Some(true)
        );

        let stopped = ProxySnapshot {
            running: false,
            managed_runtime: true,
            claude_takeover: true,
            ..ProxySnapshot::default()
        };
        assert_eq!(
            stopped.routes_current_app_through_proxy(&AppType::Claude),
            Some(false)
        );
        assert_eq!(
            stopped.routes_current_app_through_proxy(&AppType::OpenCode),
            None
        );
    }

    #[test]
    fn proxy_snapshot_can_store_rich_runtime_fields_without_internal_token() {
        let snapshot = ProxySnapshot {
            running: true,
            managed_runtime: true,
            default_cost_multiplier: Some("1.5".to_string()),
            listen_address: "127.0.0.1".to_string(),
            listen_port: 15721,
            uptime_seconds: 42,
            total_requests: 7,
            estimated_input_tokens_total: 420,
            estimated_output_tokens_total: 960,
            success_rate: Some(85.7),
            current_provider: Some("Claude Test Provider".to_string()),
            last_error: Some("last upstream failure".to_string()),
            current_app_target: Some(ProxyTargetSnapshot {
                provider_name: "Claude Test Provider".to_string(),
            }),
            ..ProxySnapshot::default()
        };

        assert!(snapshot.running);
        assert!(snapshot.managed_runtime);
        assert_eq!(snapshot.default_cost_multiplier.as_deref(), Some("1.5"));
        assert_eq!(snapshot.listen_address, "127.0.0.1");
        assert_eq!(snapshot.listen_port, 15721);
        assert_eq!(snapshot.estimated_input_tokens_total, 420);
        assert_eq!(snapshot.estimated_output_tokens_total, 960);
        assert_eq!(snapshot.success_rate, Some(85.7));
        assert_eq!(
            snapshot
                .current_app_target
                .as_ref()
                .map(|target| target.provider_name.as_str()),
            Some("Claude Test Provider")
        );
    }
}
