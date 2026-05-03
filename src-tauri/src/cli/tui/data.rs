use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use indexmap::IndexMap;
use serde_json::Value;

use crate::app_config::{AppType, CommonConfigSnippets, McpServer};
use crate::commands::workspace::{self, DailyMemoryFileInfo, ALLOWED_FILES};
use crate::error::AppError;
use crate::openclaw_config::{
    OpenClawAgentsDefaults, OpenClawEnvConfig, OpenClawHealthWarning, OpenClawToolsConfig,
};
use crate::prompt::Prompt;
use crate::provider::Provider;
use crate::services::config::BackupInfo;
use crate::services::SubscriptionQuota;
use crate::services::{ConfigService, McpService, PromptService, ProviderService, SkillService};
use crate::store::AppState;

#[derive(Debug, Clone)]
pub struct ProviderRow {
    pub id: String,
    pub provider: Provider,
    pub api_url: Option<String>,
    pub is_current: bool,
    pub is_in_config: bool,
    pub is_saved: bool,
    pub is_default_model: bool,
    pub primary_model_id: Option<String>,
    pub default_model_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum QuotaTargetKind {
    SubscriptionTool { tool: String },
    CodexOAuth { account_id: Option<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QuotaTarget {
    pub(crate) app_type: AppType,
    pub(crate) provider_id: String,
    pub(crate) provider_name: String,
    pub(crate) kind: QuotaTargetKind,
}

impl QuotaTarget {
    pub(crate) fn cache_key(&self) -> String {
        let kind = match &self.kind {
            QuotaTargetKind::SubscriptionTool { tool } => format!("subscription:{tool}"),
            QuotaTargetKind::CodexOAuth { account_id } => {
                format!("codex_oauth:{}", account_id.as_deref().unwrap_or("default"))
            }
        };
        format!("{}:{}:{kind}", self.app_type.as_str(), self.provider_id)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ProviderQuotaState {
    pub(crate) target: QuotaTarget,
    pub(crate) loading: bool,
    pub(crate) manual: bool,
    pub(crate) quota: Option<SubscriptionQuota>,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct QuotaSnapshot {
    by_provider: HashMap<String, ProviderQuotaState>,
}

impl QuotaSnapshot {
    pub(crate) fn mark_loading(&mut self, target: QuotaTarget, manual: bool) {
        let provider_id = target.provider_id.clone();
        match self.by_provider.get_mut(&provider_id) {
            Some(state) if state.target == target => {
                state.loading = true;
                state.manual = state.manual || manual;
                state.last_error = None;
            }
            _ => {
                self.by_provider.insert(
                    provider_id,
                    ProviderQuotaState {
                        target,
                        loading: true,
                        manual,
                        quota: None,
                        last_error: None,
                    },
                );
            }
        }
    }

    pub(crate) fn finish(&mut self, target: QuotaTarget, quota: SubscriptionQuota) {
        self.by_provider.insert(
            target.provider_id.clone(),
            ProviderQuotaState {
                target,
                loading: false,
                manual: false,
                quota: Some(quota),
                last_error: None,
            },
        );
    }

    pub(crate) fn finish_error(&mut self, target: QuotaTarget, error: String) {
        let provider_id = target.provider_id.clone();
        match self.by_provider.get_mut(&provider_id) {
            Some(state) if state.target == target => {
                state.loading = false;
                state.manual = false;
                state.last_error = Some(error);
            }
            _ => {
                self.by_provider.insert(
                    provider_id,
                    ProviderQuotaState {
                        target,
                        loading: false,
                        manual: false,
                        quota: None,
                        last_error: Some(error),
                    },
                );
            }
        }
    }

    pub(crate) fn state_for(&self, provider_id: &str) -> Option<&ProviderQuotaState> {
        self.by_provider.get(provider_id)
    }

    pub(crate) fn has_manual_loading(&self, target: &QuotaTarget) -> bool {
        self.by_provider
            .get(&target.provider_id)
            .is_some_and(|state| &state.target == target && state.loading && state.manual)
    }

    pub(crate) fn target_is_current(&self, target: &QuotaTarget) -> bool {
        self.by_provider
            .get(&target.provider_id)
            .is_some_and(|state| &state.target == target)
    }
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
    pub openclaw_config_path: Option<PathBuf>,
    pub openclaw_config_dir: Option<PathBuf>,
    pub openclaw_env: Option<OpenClawEnvConfig>,
    pub openclaw_tools: Option<OpenClawToolsConfig>,
    pub openclaw_agents_defaults: Option<OpenClawAgentsDefaults>,
    pub openclaw_warnings: Option<Vec<OpenClawHealthWarning>>,
    pub openclaw_workspace: OpenClawWorkspaceSnapshot,
}

#[derive(Debug, Clone, Default)]
pub struct OpenClawWorkspaceSnapshot {
    pub directory_path: PathBuf,
    pub file_exists: HashMap<String, bool>,
    pub daily_memory_files: Vec<DailyMemoryFileInfo>,
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
    pub auto_failover_enabled: bool,
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
    pub(crate) quota: QuotaSnapshot,
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
            quota: QuotaSnapshot::default(),
        })
    }

    pub(crate) fn refresh_proxy_snapshot(&mut self, app_type: &AppType) -> Result<(), AppError> {
        self.proxy = load_proxy_snapshot(app_type)?;
        Ok(())
    }
}

pub(crate) fn provider_display_name(app_type: &AppType, row: &ProviderRow) -> String {
    let name = row.provider.name.trim();
    if !name.is_empty() {
        return row.provider.name.clone();
    }

    if matches!(app_type, AppType::OpenClaw) {
        return row.id.clone();
    }

    row.provider.name.clone()
}

pub(crate) fn quota_target_for_current_provider(
    app_type: &AppType,
    data: &UiData,
) -> Option<QuotaTarget> {
    data.providers
        .rows
        .iter()
        .find(|row| row.is_current)
        .and_then(|row| quota_target_for_provider(app_type, row))
}

pub(crate) fn quota_target_for_provider(
    app_type: &AppType,
    row: &ProviderRow,
) -> Option<QuotaTarget> {
    if is_codex_oauth_provider(row) {
        return Some(QuotaTarget {
            app_type: app_type.clone(),
            provider_id: row.id.clone(),
            provider_name: provider_display_name(app_type, row),
            kind: QuotaTargetKind::CodexOAuth {
                account_id: row
                    .provider
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.managed_account_id_for("codex_oauth")),
            },
        });
    }

    let tool = match app_type {
        AppType::Claude if is_claude_official_provider(row) => "claude",
        AppType::Codex if is_codex_official_provider(row) => "codex",
        AppType::Gemini if is_gemini_official_provider(row) => "gemini",
        _ => return None,
    };

    Some(QuotaTarget {
        app_type: app_type.clone(),
        provider_id: row.id.clone(),
        provider_name: provider_display_name(app_type, row),
        kind: QuotaTargetKind::SubscriptionTool {
            tool: tool.to_string(),
        },
    })
}

fn is_codex_oauth_provider(row: &ProviderRow) -> bool {
    row.provider
        .meta
        .as_ref()
        .and_then(|meta| meta.provider_type.as_deref())
        .is_some_and(|value| value == "codex_oauth")
}

fn is_claude_official_provider(row: &ProviderRow) -> bool {
    row.provider
        .category
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("official"))
}

fn is_codex_official_provider(row: &ProviderRow) -> bool {
    if row
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.codex_official)
        .unwrap_or(false)
    {
        return true;
    }

    if row
        .provider
        .category
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("official"))
    {
        return true;
    }

    let legacy_official_identity = row
        .provider
        .website_url
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("https://chatgpt.com/codex"))
        || row
            .provider
            .name
            .trim()
            .eq_ignore_ascii_case("OpenAI Official");

    if !legacy_official_identity {
        return false;
    }

    let api_key_blank = row
        .provider
        .settings_config
        .get("auth")
        .and_then(|auth| auth.get("OPENAI_API_KEY"))
        .and_then(Value::as_str)
        .is_none_or(|value| value.trim().is_empty());

    api_key_blank && !codex_config_has_base_url(&row.provider.settings_config)
}

fn is_gemini_official_provider(row: &ProviderRow) -> bool {
    if row
        .provider
        .category
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("official"))
    {
        return true;
    }

    if row
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.partner_promotion_key.as_deref())
        .is_some_and(|value| value.eq_ignore_ascii_case("google-official"))
    {
        return true;
    }

    let legacy_google_identity = row.provider.website_url.as_deref().is_some_and(|value| {
        let value = value.trim_end_matches('/');
        value.eq_ignore_ascii_case("https://ai.google.dev")
            || value.eq_ignore_ascii_case("https://aistudio.google.com")
    }) || matches!(
        row.provider.name.trim().to_ascii_lowercase().as_str(),
        "google" | "google official" | "google oauth"
    );

    if !legacy_google_identity {
        return false;
    }

    let api_key_blank = row
        .provider
        .settings_config
        .get("env")
        .and_then(|env| env.get("GEMINI_API_KEY"))
        .and_then(Value::as_str)
        .is_none_or(|value| value.trim().is_empty());
    let base_url_blank = extract_api_url(&row.provider.settings_config, &AppType::Gemini)
        .is_none_or(|value| value.trim().is_empty());

    api_key_blank && base_url_blank
}

fn codex_config_has_base_url(settings_config: &Value) -> bool {
    let Some(config_text) = settings_config
        .get("config")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };

    let Ok(table) = toml::from_str::<toml::Table>(config_text) else {
        return false;
    };

    if table
        .get("base_url")
        .and_then(|value| value.as_str())
        .is_some_and(|value| !value.trim().is_empty())
    {
        return true;
    }

    let Some(provider_key) = table.get("model_provider").and_then(|value| value.as_str()) else {
        return false;
    };

    table
        .get("model_providers")
        .and_then(|value| value.as_table())
        .and_then(|providers| providers.get(provider_key))
        .and_then(|value| value.as_table())
        .and_then(|provider| provider.get("base_url"))
        .and_then(|value| value.as_str())
        .is_some_and(|value| !value.trim().is_empty())
}

fn load_providers(state: &AppState, app_type: &AppType) -> Result<ProvidersSnapshot, AppError> {
    if matches!(app_type, AppType::OpenClaw) {
        ProviderService::sync_openclaw_providers_from_live(state)?;
    }

    let current_id = ProviderService::current(state, app_type.clone())?;
    let providers = ProviderService::list(state, app_type.clone())?;
    let sorted = sort_providers(&providers);

    let openclaw_live_providers = if matches!(app_type, AppType::OpenClaw) {
        crate::openclaw_config::get_providers()?
    } else {
        serde_json::Map::new()
    };
    let openclaw_live_ids = if matches!(app_type, AppType::OpenClaw) {
        ProviderService::valid_openclaw_live_provider_ids()?.unwrap_or_default()
    } else {
        HashSet::new()
    };
    let opencode_live_ids = if matches!(app_type, AppType::OpenCode) {
        crate::opencode_config::get_providers()?
            .into_iter()
            .map(|(id, _)| id)
            .collect::<HashSet<_>>()
    } else {
        HashSet::new()
    };
    let openclaw_default_model = if matches!(app_type, AppType::OpenClaw) {
        crate::openclaw_config::get_default_model()?
    } else {
        None
    };
    let openclaw_default_model_ids =
        openclaw_default_model_ids_by_provider(openclaw_default_model.as_ref());
    let openclaw_primary_default_provider_id = openclaw_default_model
        .as_ref()
        .and_then(|model| openclaw_default_model_ref_parts(&model.primary))
        .map(|(provider_id, _)| provider_id.to_string());

    let rows = sorted
        .into_iter()
        .map(|(id, provider)| {
            let openclaw_live_provider = openclaw_live_providers
                .get(&id)
                .filter(|_| openclaw_live_ids.contains(&id));
            let provider = openclaw_provider_for_row(&id, provider, openclaw_live_provider);

            ProviderRow {
                api_url: extract_api_url(&provider.settings_config, app_type),
                is_current: id == current_id,
                is_in_config: match app_type {
                    AppType::OpenCode => opencode_live_ids.contains(&id),
                    AppType::OpenClaw => openclaw_live_ids.contains(&id),
                    _ => true,
                },
                is_saved: true,
                is_default_model: openclaw_primary_default_provider_id.as_deref()
                    == Some(id.as_str()),
                primary_model_id: extract_primary_model_id(
                    &provider.settings_config,
                    app_type,
                    openclaw_live_provider,
                ),
                default_model_id: openclaw_default_model_ids.get(&id).cloned(),
                id: id.clone(),
                provider,
            }
        })
        .collect::<Vec<_>>();

    let rows = if matches!(app_type, AppType::OpenClaw) {
        rows.into_iter()
            .filter(|row| !openclaw_live_providers.contains_key(&row.id) || row.is_in_config)
            .collect::<Vec<_>>()
    } else {
        rows
    };

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
        AppType::OpenClaw => settings_config
            .get("baseUrl")
            .or_else(|| settings_config.get("base_url"))?
            .as_str()
            .map(|s| s.to_string()),
    }
}

fn extract_primary_model_id(
    settings_config: &Value,
    app_type: &AppType,
    openclaw_live_provider: Option<&Value>,
) -> Option<String> {
    match app_type {
        AppType::OpenClaw => match openclaw_live_provider {
            Some(live_provider) => openclaw_primary_model_id(live_provider),
            None => openclaw_primary_model_id(settings_config),
        },
        _ => None,
    }
}

fn openclaw_provider_for_row(
    _id: &str,
    provider: Provider,
    openclaw_live_provider: Option<&Value>,
) -> Provider {
    let Some(live_provider) = openclaw_live_provider else {
        return provider;
    };

    let mut provider = provider;
    provider.settings_config = live_provider.clone();
    provider
}

fn openclaw_primary_model_id(provider_value: &Value) -> Option<String> {
    provider_value
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| models.first())
        .and_then(|model| model.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn openclaw_default_model_ids_by_provider(
    default_model: Option<&crate::openclaw_config::OpenClawDefaultModel>,
) -> HashMap<String, String> {
    let Some(default_model) = default_model else {
        return HashMap::new();
    };

    let mut ids = HashMap::new();
    for model_ref in std::iter::once(default_model.primary.as_str())
        .chain(default_model.fallbacks.iter().map(String::as_str))
    {
        let Some((provider_id, model_id)) = openclaw_default_model_ref_parts(model_ref) else {
            continue;
        };
        ids.entry(provider_id.to_string())
            .or_insert_with(|| model_id.to_string());
    }

    ids
}

fn openclaw_default_model_ref_parts(default_ref: &str) -> Option<(&str, &str)> {
    default_ref.split_once('/')
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
    let openclaw_snapshot = load_openclaw_config_snapshot(app_type)?;
    let openclaw_workspace = load_openclaw_workspace_snapshot(app_type)?;

    Ok(ConfigSnapshot {
        config_path,
        config_dir,
        backups,
        common_snippet,
        common_snippets,
        webdav_sync: crate::settings::get_webdav_sync_settings(),
        openclaw_config_path: openclaw_snapshot
            .as_ref()
            .map(|snapshot| snapshot.config_path.clone()),
        openclaw_config_dir: openclaw_snapshot
            .as_ref()
            .map(|snapshot| snapshot.config_dir.clone()),
        openclaw_env: openclaw_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.env.clone()),
        openclaw_tools: openclaw_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.tools.clone()),
        openclaw_agents_defaults: openclaw_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.agents_defaults.clone()),
        openclaw_warnings: openclaw_snapshot.map(|snapshot| snapshot.warnings),
        openclaw_workspace,
    })
}

#[derive(Debug, Clone)]
struct OpenClawConfigSnapshot {
    config_path: PathBuf,
    config_dir: PathBuf,
    env: Option<OpenClawEnvConfig>,
    tools: Option<OpenClawToolsConfig>,
    agents_defaults: Option<OpenClawAgentsDefaults>,
    warnings: Vec<OpenClawHealthWarning>,
}

fn load_openclaw_config_snapshot(
    app_type: &AppType,
) -> Result<Option<OpenClawConfigSnapshot>, AppError> {
    if !matches!(app_type, AppType::OpenClaw) {
        return Ok(None);
    }

    let mut warnings = crate::openclaw_config::scan_openclaw_config_health()?;
    let env = load_openclaw_slice(
        "env",
        "env",
        crate::openclaw_config::get_env_config,
        &mut warnings,
    )?;
    let tools = load_openclaw_optional_slice(
        "tools",
        "tools",
        crate::openclaw_config::get_tools_config,
        &mut warnings,
    )?;
    let agents_defaults = load_openclaw_slice(
        "agents.defaults",
        "agents.defaults",
        crate::openclaw_config::get_agents_defaults,
        &mut warnings,
    )?;

    Ok(Some(OpenClawConfigSnapshot {
        config_path: crate::openclaw_config::get_openclaw_config_path(),
        config_dir: crate::openclaw_config::get_openclaw_dir(),
        env: Some(env),
        tools,
        agents_defaults,
        warnings,
    }))
}

fn load_openclaw_optional_slice<T, F>(
    section_name: &'static str,
    warning_path: &'static str,
    loader: F,
    warnings: &mut Vec<OpenClawHealthWarning>,
) -> Result<Option<T>, AppError>
where
    F: FnOnce() -> Result<T, AppError>,
{
    match loader() {
        Ok(value) => Ok(Some(value)),
        Err(AppError::Config(message)) => {
            log::warn!(
                "Failed to load OpenClaw config section '{section_name}' for TUI snapshot: {message}"
            );
            warnings.push(OpenClawHealthWarning {
                code: "config_parse_failed".to_string(),
                message,
                path: Some(warning_path.to_string()),
            });
            Ok(None)
        }
        Err(err) => Err(err),
    }
}

fn load_openclaw_slice<T, F>(
    section_name: &'static str,
    warning_path: &'static str,
    loader: F,
    warnings: &mut Vec<OpenClawHealthWarning>,
) -> Result<T, AppError>
where
    T: Default,
    F: FnOnce() -> Result<T, AppError>,
{
    match loader() {
        Ok(value) => Ok(value),
        Err(AppError::Config(message)) => {
            log::warn!(
                "Failed to load OpenClaw config section '{section_name}' for TUI snapshot: {message}"
            );
            warnings.push(OpenClawHealthWarning {
                code: "config_parse_failed".to_string(),
                message,
                path: Some(warning_path.to_string()),
            });
            Ok(T::default())
        }
        Err(err) => Err(err),
    }
}

fn load_openclaw_workspace_snapshot(
    app_type: &AppType,
) -> Result<OpenClawWorkspaceSnapshot, AppError> {
    if !matches!(app_type, AppType::OpenClaw) {
        return Ok(OpenClawWorkspaceSnapshot::default());
    }

    let directory_path = crate::openclaw_config::get_openclaw_dir().join("workspace");
    let file_exists = ALLOWED_FILES
        .iter()
        .map(|filename| {
            let exists = workspace::workspace_file_exists((*filename).to_string())?;
            Ok(((*filename).to_string(), exists))
        })
        .collect::<Result<HashMap<_, _>, String>>()
        .map_err(AppError::Message)?;
    let daily_memory_files = workspace::list_daily_memory_files().map_err(AppError::Message)?;

    Ok(OpenClawWorkspaceSnapshot {
        directory_path,
        file_exists,
        daily_memory_files,
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
        let app_proxy_config = state.db.get_proxy_config_for_app(app_type.as_str()).await?;
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
            auto_failover_enabled: app_proxy_config.auto_failover_enabled,
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
    use crate::provider::{AuthBinding, AuthBindingSource, ProviderMeta};
    use serde_json::json;
    use serial_test::serial;
    use std::path::Path;
    use tempfile::tempdir;

    use crate::settings::{get_settings, update_settings, AppSettings};
    use crate::test_support::{lock_test_home_and_settings, set_test_home_override};

    struct HomeGuard {
        old_home: Option<std::ffi::OsString>,
        old_userprofile: Option<std::ffi::OsString>,
        old_config_dir: Option<std::ffi::OsString>,
    }

    impl HomeGuard {
        fn set(home: &Path) -> Self {
            let old_home = std::env::var_os("HOME");
            let old_userprofile = std::env::var_os("USERPROFILE");
            let old_config_dir = std::env::var_os("CC_SWITCH_CONFIG_DIR");
            std::env::set_var("HOME", home);
            std::env::set_var("USERPROFILE", home);
            std::env::set_var("CC_SWITCH_CONFIG_DIR", home.join(".cc-switch"));
            set_test_home_override(Some(home));
            crate::settings::reload_test_settings();
            Self {
                old_home,
                old_userprofile,
                old_config_dir,
            }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.old_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match &self.old_userprofile {
                Some(value) => std::env::set_var("USERPROFILE", value),
                None => std::env::remove_var("USERPROFILE"),
            }
            match &self.old_config_dir {
                Some(value) => std::env::set_var("CC_SWITCH_CONFIG_DIR", value),
                None => std::env::remove_var("CC_SWITCH_CONFIG_DIR"),
            }
            set_test_home_override(self.old_home.as_deref().map(Path::new));
            crate::settings::reload_test_settings();
        }
    }

    struct SettingsGuard {
        previous: AppSettings,
    }

    impl SettingsGuard {
        fn with_opencode_dir(path: &Path) -> Self {
            let previous = get_settings();
            let mut settings = AppSettings::default();
            settings.opencode_config_dir = Some(path.display().to_string());
            update_settings(settings).expect("set opencode override dir");
            Self { previous }
        }

        fn with_openclaw_dir(path: &Path) -> Self {
            let previous = get_settings();
            let mut settings = AppSettings::default();
            settings.openclaw_config_dir = Some(path.display().to_string());
            update_settings(settings).expect("set openclaw override dir");
            Self { previous }
        }
    }

    impl Drop for SettingsGuard {
        fn drop(&mut self) {
            update_settings(self.previous.clone()).expect("restore previous settings");
        }
    }

    fn test_provider_row(id: &str, name: &str, settings_config: serde_json::Value) -> ProviderRow {
        ProviderRow {
            id: id.to_string(),
            provider: Provider::with_id(id.to_string(), name.to_string(), settings_config, None),
            api_url: None,
            is_current: true,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: None,
            default_model_id: None,
        }
    }

    #[test]
    #[serial]
    fn load_proxy_snapshot_reads_app_auto_failover_state() {
        let _guard = lock_test_home_and_settings();
        let temp = tempdir().expect("create tempdir");
        let _home = HomeGuard::set(temp.path());

        let state = load_state().expect("load state");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create runtime");
        runtime.block_on(async {
            let mut config = state
                .db
                .get_proxy_config_for_app("claude")
                .await
                .expect("read claude app proxy config");
            config.auto_failover_enabled = true;
            state
                .db
                .update_proxy_config_for_app(config)
                .await
                .expect("persist claude app proxy config");
        });

        let snapshot = load_proxy_snapshot(&AppType::Claude).expect("load proxy snapshot");
        assert!(snapshot.auto_failover_enabled);
    }

    #[test]
    fn quota_target_detects_official_claude_by_explicit_category() {
        let mut official = test_provider_row("official", "Claude Official", json!({"env": {}}));
        official.provider.category = Some("official".to_string());
        let stripped_custom = test_provider_row("stripped-custom", "Custom", json!({"env": {}}));
        let custom = test_provider_row(
            "custom",
            "Claude Custom",
            json!({"env": {"ANTHROPIC_BASE_URL": "https://api.example.com"}}),
        );

        assert!(matches!(
            quota_target_for_provider(&AppType::Claude, &official).map(|target| target.kind),
            Some(QuotaTargetKind::SubscriptionTool { tool }) if tool == "claude"
        ));
        assert!(quota_target_for_provider(&AppType::Claude, &stripped_custom).is_none());
        assert!(quota_target_for_provider(&AppType::Claude, &custom).is_none());
    }

    #[test]
    fn quota_target_detects_codex_official_and_skips_api_key_providers() {
        let missing_key = test_provider_row("official", "OpenAI Official", json!({"auth": {}}));
        let no_key_custom_base_url = test_provider_row(
            "base-url",
            "OpenAI Official",
            json!({
                "auth": {},
                "config": r#"base_url = "https://api.example.com/v1""#
            }),
        );
        let mut metadata_official = test_provider_row(
            "metadata",
            "Codex Official",
            json!({"auth": {"OPENAI_API_KEY": "sk-custom"}}),
        );
        metadata_official.provider.meta = Some(ProviderMeta {
            codex_official: Some(true),
            ..ProviderMeta::default()
        });
        let api_key = test_provider_row(
            "api-key",
            "Custom OpenAI",
            json!({"auth": {"OPENAI_API_KEY": "sk-custom"}}),
        );

        assert!(matches!(
            quota_target_for_provider(&AppType::Codex, &missing_key).map(|target| target.kind),
            Some(QuotaTargetKind::SubscriptionTool { tool }) if tool == "codex"
        ));
        assert!(matches!(
            quota_target_for_provider(&AppType::Codex, &metadata_official)
                .map(|target| target.kind),
            Some(QuotaTargetKind::SubscriptionTool { tool }) if tool == "codex"
        ));
        assert!(quota_target_for_provider(&AppType::Codex, &no_key_custom_base_url).is_none());
        assert!(quota_target_for_provider(&AppType::Codex, &api_key).is_none());
    }

    #[test]
    fn quota_target_detects_gemini_official_and_skips_api_key_providers() {
        let mut explicit_official =
            test_provider_row("google-official", "Google Official", json!({"env": {}}));
        explicit_official.provider.category = Some("official".to_string());

        let google_oauth = test_provider_row(
            "google-oauth",
            "Google OAuth",
            json!({"env": {}, "config": {}}),
        );
        let mut partner_official = test_provider_row(
            "partner",
            "Gemini",
            json!({"env": {"GEMINI_API_KEY": "sk"}}),
        );
        partner_official.provider.meta = Some(ProviderMeta {
            partner_promotion_key: Some("google-official".to_string()),
            ..ProviderMeta::default()
        });
        let api_key = test_provider_row(
            "api-key",
            "Google OAuth",
            json!({"env": {"GEMINI_API_KEY": "sk-custom"}}),
        );
        let base_url = test_provider_row(
            "base-url",
            "Google OAuth",
            json!({"env": {"GOOGLE_GEMINI_BASE_URL": "https://api.example.com"}}),
        );
        let stripped_custom = test_provider_row("custom", "Custom Gemini", json!({"env": {}}));

        assert!(matches!(
            quota_target_for_provider(&AppType::Gemini, &explicit_official)
                .map(|target| target.kind),
            Some(QuotaTargetKind::SubscriptionTool { tool }) if tool == "gemini"
        ));
        assert!(matches!(
            quota_target_for_provider(&AppType::Gemini, &google_oauth).map(|target| target.kind),
            Some(QuotaTargetKind::SubscriptionTool { tool }) if tool == "gemini"
        ));
        assert!(matches!(
            quota_target_for_provider(&AppType::Gemini, &partner_official)
                .map(|target| target.kind),
            Some(QuotaTargetKind::SubscriptionTool { tool }) if tool == "gemini"
        ));
        assert!(quota_target_for_provider(&AppType::Gemini, &api_key).is_none());
        assert!(quota_target_for_provider(&AppType::Gemini, &base_url).is_none());
        assert!(quota_target_for_provider(&AppType::Gemini, &stripped_custom).is_none());
    }

    #[test]
    fn quota_target_detects_codex_oauth_managed_account() {
        let mut row = test_provider_row("codex-oauth", "Codex OAuth", json!({}));
        row.provider.meta = Some(ProviderMeta {
            provider_type: Some("codex_oauth".to_string()),
            auth_binding: Some(AuthBinding {
                source: AuthBindingSource::ManagedAccount,
                auth_provider: Some("codex_oauth".to_string()),
                account_id: Some("acct-1".to_string()),
            }),
            ..ProviderMeta::default()
        });

        let target =
            quota_target_for_provider(&AppType::Claude, &row).expect("codex oauth quota target");

        assert_eq!(target.provider_id, "codex-oauth");
        assert!(matches!(
            target.kind,
            QuotaTargetKind::CodexOAuth { account_id } if account_id.as_deref() == Some("acct-1")
        ));
    }

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
    #[serial]
    fn load_providers_opencode_marks_live_config_membership() {
        let _guard = lock_test_home_and_settings();
        let temp = tempdir().expect("create tempdir");
        let opencode_dir = temp.path().join("opencode");
        std::fs::create_dir_all(&opencode_dir).expect("create opencode dir");
        let _home = HomeGuard::set(temp.path());
        let _settings = SettingsGuard::with_opencode_dir(&opencode_dir);

        crate::opencode_config::set_provider(
            "in-config",
            json!({
                "npm": "@ai-sdk/openai-compatible",
                "options": {
                    "baseURL": "https://live.example.com/v1"
                },
                "models": {
                    "main": {"name": "Main"}
                }
            }),
        )
        .expect("seed live opencode provider");

        let state = load_state().expect("load state");
        {
            let mut config = state.config.write().expect("lock config");
            let manager = config
                .get_manager_mut(&AppType::OpenCode)
                .expect("opencode manager");
            manager.providers.insert(
                "in-config".to_string(),
                Provider::with_id(
                    "in-config".to_string(),
                    "In Config".to_string(),
                    json!({
                        "options": {
                            "baseURL": "https://saved-live.example.com/v1"
                        }
                    }),
                    None,
                ),
            );
            manager.providers.insert(
                "saved-only".to_string(),
                Provider::with_id(
                    "saved-only".to_string(),
                    "Saved Only".to_string(),
                    json!({
                        "options": {
                            "baseURL": "https://saved.example.com/v1"
                        }
                    }),
                    None,
                ),
            );
        }
        state.save().expect("persist opencode providers");

        let snapshot = load_providers(&state, &AppType::OpenCode).expect("load opencode rows");
        let in_config = snapshot
            .rows
            .iter()
            .find(|row| row.id == "in-config")
            .expect("in-config provider row");
        let saved_only = snapshot
            .rows
            .iter()
            .find(|row| row.id == "saved-only")
            .expect("saved-only provider row");

        assert!(in_config.is_in_config);
        assert!(!in_config.is_current);
        assert!(!saved_only.is_in_config);
        assert!(saved_only.is_saved);
        assert_eq!(
            saved_only.api_url.as_deref(),
            Some("https://saved.example.com/v1")
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

    #[test]
    fn openclaw_default_model_ids_by_provider_includes_fallback_only_provider() {
        let default_model = crate::openclaw_config::OpenClawDefaultModel {
            primary: "primary/model-primary".to_string(),
            fallbacks: vec![
                "fallback-only/shared-model".to_string(),
                "primary/model-fallback".to_string(),
            ],
            extra: std::collections::HashMap::new(),
        };

        let model_ids = openclaw_default_model_ids_by_provider(Some(&default_model));

        assert_eq!(
            model_ids.get("primary").map(String::as_str),
            Some("model-primary")
        );
        assert_eq!(
            model_ids.get("fallback-only").map(String::as_str),
            Some("shared-model")
        );
    }

    #[test]
    #[serial]
    fn openclaw_config_snapshot_includes_slice_and_warning_data() {
        let _guard = lock_test_home_and_settings();
        let temp = tempdir().expect("tempdir");
        let _home = HomeGuard::set(temp.path());

        let openclaw_dir = temp.path().join("openclaw");
        std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let source = r#"{
  env: {
    OPENCLAW_ENV_TOKEN: 'demo-token',
  },
  tools: {
    profile: 'unsupported-profile',
    allow: ['bash'],
  },
  agents: {
    defaults: {
      timeout: 42,
      model: {
        primary: 'demo/main',
      },
    },
  },
}"#;
        std::fs::write(openclaw_dir.join("openclaw.json"), source).expect("write openclaw config");

        let state = load_state().expect("load state");
        let snapshot = load_config_snapshot(&state, &AppType::OpenClaw).expect("load snapshot");

        assert_eq!(snapshot.openclaw_config_dir.as_ref(), Some(&openclaw_dir));
        assert_eq!(
            snapshot.openclaw_config_path.as_ref(),
            Some(&openclaw_dir.join("openclaw.json"))
        );
        assert_eq!(
            snapshot
                .openclaw_env
                .as_ref()
                .and_then(|env| env.vars.get("OPENCLAW_ENV_TOKEN"))
                .and_then(|value| value.as_str()),
            Some("demo-token")
        );
        assert_eq!(
            snapshot
                .openclaw_tools
                .as_ref()
                .and_then(|tools| tools.profile.as_deref()),
            Some("unsupported-profile")
        );
        assert_eq!(
            snapshot
                .openclaw_agents_defaults
                .as_ref()
                .and_then(|defaults| defaults.model.as_ref())
                .map(|model| model.primary.as_str()),
            Some("demo/main")
        );
        assert!(snapshot
            .openclaw_warnings
            .as_ref()
            .is_some_and(|warnings| warnings.iter().any(|warning| {
                warning.code == "invalid_tools_profile" || warning.code == "legacy_agents_timeout"
            })));
    }

    #[test]
    #[serial]
    fn openclaw_config_snapshot_keeps_tools_parse_warning_when_tools_section_is_malformed() {
        let _guard = lock_test_home_and_settings();
        let temp = tempdir().expect("tempdir");
        let _home = HomeGuard::set(temp.path());

        let openclaw_dir = temp.path().join("openclaw");
        std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let source = r#"{
  tools: {
    profile: 'coding',
    allow: 'Read',
  },
}"#;
        std::fs::write(openclaw_dir.join("openclaw.json"), source).expect("write openclaw config");

        let state = load_state().expect("load state");
        let snapshot = load_config_snapshot(&state, &AppType::OpenClaw).expect("load snapshot");

        assert!(snapshot
            .openclaw_warnings
            .as_ref()
            .is_some_and(|warnings| warnings
                .iter()
                .any(|warning| warning.code == "config_parse_failed"
                    && warning.path.as_deref() == Some("tools"))));
        assert!(snapshot.openclaw_tools.is_none());
    }

    #[test]
    #[serial]
    fn non_openclaw_config_snapshot_leaves_openclaw_fields_unset() {
        let _guard = lock_test_home_and_settings();
        let temp = tempdir().expect("tempdir");
        let _home = HomeGuard::set(temp.path());

        let state = load_state().expect("load state");
        let snapshot = load_config_snapshot(&state, &AppType::Claude).expect("load snapshot");

        assert!(snapshot.openclaw_config_path.is_none());
        assert!(snapshot.openclaw_config_dir.is_none());
        assert!(snapshot.openclaw_env.is_none());
        assert!(snapshot.openclaw_tools.is_none());
        assert!(snapshot.openclaw_agents_defaults.is_none());
        assert!(snapshot.openclaw_warnings.is_none());
    }

    #[test]
    #[serial]
    fn openclaw_workspace_snapshot_uses_presence_probe_for_invalid_utf8_file() {
        let _guard = lock_test_home_and_settings();
        let temp = tempdir().expect("tempdir");
        let openclaw_dir = temp.path().join(".openclaw");
        std::fs::create_dir_all(openclaw_dir.join("workspace")).expect("create workspace dir");
        let _home = HomeGuard::set(temp.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        std::fs::write(openclaw_dir.join("workspace/AGENTS.md"), [0xff, 0xfe, 0xfd])
            .expect("write invalid utf8 workspace file");

        let snapshot =
            load_openclaw_workspace_snapshot(&AppType::OpenClaw).expect("load workspace snapshot");

        assert_eq!(snapshot.file_exists.get("AGENTS.md"), Some(&true));
    }

    #[test]
    fn openclaw_default_model_ids_by_provider_prefers_primary_reference_for_same_provider() {
        let default_model = crate::openclaw_config::OpenClawDefaultModel {
            primary: "demo/shared-model".to_string(),
            fallbacks: vec!["demo/secondary-model".to_string()],
            extra: std::collections::HashMap::new(),
        };

        assert_eq!(
            openclaw_default_model_ids_by_provider(Some(&default_model))
                .get("demo")
                .map(String::as_str),
            Some("shared-model")
        );
    }

    #[test]
    fn extract_primary_model_id_openclaw_prefers_live_provider_models() {
        let saved = json!({
            "models": [
                {"id": "snapshot-primary"},
                {"id": "fallback-1"}
            ]
        });
        let live = json!({
            "models": [
                {"id": "live-primary"},
                {"id": "snapshot-primary"},
                {"id": "fallback-1"}
            ]
        });

        assert_eq!(
            extract_primary_model_id(&saved, &AppType::OpenClaw, Some(&live)),
            Some("live-primary".to_string())
        );
    }

    #[test]
    fn extract_primary_model_id_openclaw_falls_back_to_saved_provider_models() {
        let saved = json!({
            "models": [
                {"id": "snapshot-primary"},
                {"id": "fallback-1"}
            ]
        });

        assert_eq!(
            extract_primary_model_id(&saved, &AppType::OpenClaw, None),
            Some("snapshot-primary".to_string())
        );
    }

    #[test]
    fn extract_primary_model_id_openclaw_does_not_fall_back_when_live_provider_has_no_models() {
        let saved = json!({
            "models": [
                {"id": "snapshot-primary"},
                {"id": "fallback-1"}
            ]
        });
        let live = json!({"models": []});

        assert_eq!(
            extract_primary_model_id(&saved, &AppType::OpenClaw, Some(&live)),
            None
        );
    }

    #[test]
    #[serial]
    fn load_providers_openclaw_syncs_live_only_provider_into_local_manager() {
        let _guard = lock_test_home_and_settings();
        let temp = tempdir().expect("create tempdir");
        let openclaw_dir = temp.path().join(".openclaw");
        std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        let _home = HomeGuard::set(temp.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        crate::openclaw_config::set_provider(
            "live-only",
            json!({
                "baseUrl": "https://api.example.com/v1",
                "models": [
                    {"id": "openclaw-live-model", "name": "Live Only Model"}
                ]
            }),
        )
        .expect("seed live openclaw provider");

        let state = load_state().expect("load state");
        assert!(
            state
                .config
                .read()
                .expect("read config before load")
                .get_manager(&AppType::OpenClaw)
                .expect("openclaw manager before load")
                .providers
                .is_empty(),
            "precondition: local manager should start empty"
        );

        let snapshot = load_providers(&state, &AppType::OpenClaw).expect("load openclaw rows");
        let row = snapshot
            .rows
            .iter()
            .find(|row| row.id == "live-only")
            .expect("live-only provider should appear in TUI rows");
        assert!(row.is_in_config);
        assert_eq!(provider_display_name(&AppType::OpenClaw, row), "live-only");
        assert_eq!(row.provider.name, "live-only");
        assert_eq!(row.primary_model_id.as_deref(), Some("openclaw-live-model"));
        assert!(row.is_saved);

        let providers =
            ProviderService::list(&state, AppType::OpenClaw).expect("list providers after load");
        assert!(
            providers.contains_key("live-only"),
            "loading OpenClaw rows should mirror live providers into the local manager"
        );
    }

    #[test]
    #[serial]
    fn load_providers_openclaw_skips_modeless_live_provider() {
        let _guard = lock_test_home_and_settings();
        let temp = tempdir().expect("create tempdir");
        let openclaw_dir = temp.path().join(".openclaw");
        std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        let _home = HomeGuard::set(temp.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        std::fs::write(
            openclaw_dir.join("openclaw.json"),
            r#"{
  models: {
    mode: 'merge',
    providers: {
      modeless: {
        baseUrl: 'https://api.example.com/v1',
        models: [],
      },
    },
  },
}
"#,
        )
        .expect("seed modeless openclaw provider");

        let state = load_state().expect("load state");

        let snapshot = load_providers(&state, &AppType::OpenClaw).expect("load openclaw rows");
        assert!(
            snapshot.rows.iter().all(|row| row.id != "modeless"),
            "OpenClaw rows without models should stay hidden from the TUI"
        );

        let providers =
            ProviderService::list(&state, AppType::OpenClaw).expect("list providers after load");
        assert!(
            !providers.contains_key("modeless"),
            "modeless OpenClaw providers should not be mirrored into the local manager"
        );
    }

    #[test]
    #[serial]
    fn load_providers_openclaw_skips_blank_model_id_live_provider() {
        let _guard = lock_test_home_and_settings();
        let temp = tempdir().expect("create tempdir");
        let openclaw_dir = temp.path().join(".openclaw");
        std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        let _home = HomeGuard::set(temp.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        std::fs::write(
            openclaw_dir.join("openclaw.json"),
            r#"{
  models: {
    mode: 'merge',
    providers: {
      keep: {
        baseUrl: 'https://keep.example.com/v1',
        models: [{ id: 'keep-model', name: 'Keep Model' }],
      },
      'blank-model-id': {
        baseUrl: 'https://blank.example.com/v1',
        models: [{ id: '   ', name: 'Blank Id' }],
      },
    },
  },
}
"#,
        )
        .expect("seed openclaw providers with blank model id");

        let state = load_state().expect("load state");

        let snapshot = load_providers(&state, &AppType::OpenClaw).expect("load openclaw rows");
        assert!(
            snapshot.rows.iter().any(|row| row.id == "keep"),
            "valid OpenClaw providers should still load when a sibling live entry is invalid"
        );
        assert!(
            snapshot.rows.iter().all(|row| row.id != "blank-model-id"),
            "blank-model-id OpenClaw rows should stay hidden from the TUI"
        );

        let providers =
            ProviderService::list(&state, AppType::OpenClaw).expect("list providers after load");
        assert!(providers.contains_key("keep"));
        assert!(
            !providers.contains_key("blank-model-id"),
            "blank OpenClaw model ids should not be mirrored into the local manager"
        );
    }

    #[test]
    #[serial]
    fn load_providers_openclaw_hides_invalid_live_row_but_preserves_saved_metadata() {
        let _guard = lock_test_home_and_settings();
        let temp = tempdir().expect("create tempdir");
        let openclaw_dir = temp.path().join(".openclaw");
        std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        let _home = HomeGuard::set(temp.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let state = load_state().expect("load state");
        {
            let mut config = state.config.write().expect("lock config");
            let manager = config
                .get_manager_mut(&AppType::OpenClaw)
                .expect("openclaw manager");
            let mut provider = Provider::with_id(
                "preserved".to_string(),
                "Saved Provider Name".to_string(),
                json!({
                    "baseUrl": "https://saved.example.com/v1",
                    "models": [
                        {"id": "saved-model", "name": "Saved Model"}
                    ]
                }),
                None,
            );
            provider.notes = Some("keep this metadata".to_string());
            manager.providers.insert("preserved".to_string(), provider);
        }
        state.save().expect("persist saved snapshot provider");

        std::fs::write(
            openclaw_dir.join("openclaw.json"),
            r#"{
  models: {
    mode: 'merge',
    providers: {
      preserved: {
        baseUrl: 'https://live.invalid.example.com/v1',
        models: [],
      },
    },
  },
}
"#,
        )
        .expect("seed invalid openclaw provider");

        let snapshot = load_providers(&state, &AppType::OpenClaw).expect("load openclaw rows");
        assert!(
            snapshot.rows.iter().all(|row| row.id != "preserved"),
            "invalid-but-present OpenClaw live entries should stay hidden from the TUI"
        );

        let providers =
            ProviderService::list(&state, AppType::OpenClaw).expect("list providers after load");
        let preserved = providers
            .get("preserved")
            .expect("saved snapshot row should remain in the local mirror");
        assert_eq!(preserved.name, "Saved Provider Name");
        assert_eq!(preserved.notes.as_deref(), Some("keep this metadata"));
        assert_eq!(
            preserved.settings_config["baseUrl"],
            json!("https://saved.example.com/v1"),
            "invalid live rows should not become authoritative for the saved mirror"
        );
    }

    #[test]
    #[serial]
    fn load_providers_openclaw_keeps_customized_name_when_metadata_exists() {
        let _guard = lock_test_home_and_settings();
        let temp = tempdir().expect("create tempdir");
        let openclaw_dir = temp.path().join(".openclaw");
        std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        let _home = HomeGuard::set(temp.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let state = load_state().expect("load state");
        {
            let mut config = state.config.write().expect("lock config");
            let manager = config
                .get_manager_mut(&AppType::OpenClaw)
                .expect("openclaw manager");
            let mut provider = Provider::with_id(
                "shared-id".to_string(),
                "Saved Snapshot Name".to_string(),
                json!({
                    "baseUrl": "https://snapshot.example.com/v1",
                    "models": [
                        {"id": "snapshot-model", "name": "Snapshot Model"}
                    ]
                }),
                None,
            );
            provider.notes = Some("customized via deeplink".to_string());
            manager.providers.insert("shared-id".to_string(), provider);
        }
        state.save().expect("persist snapshot provider");

        crate::openclaw_config::set_provider(
            "shared-id",
            json!({
                "baseUrl": "https://live.example.com/v1",
                "models": [
                    {"id": "live-model", "name": "Live Model Name"}
                ]
            }),
        )
        .expect("seed live openclaw provider");

        let snapshot = load_providers(&state, &AppType::OpenClaw).expect("load openclaw rows");
        let row = snapshot
            .rows
            .iter()
            .find(|row| row.id == "shared-id")
            .expect("existing snapshot provider should still be listed");

        assert_eq!(
            provider_display_name(&AppType::OpenClaw, row),
            "Saved Snapshot Name"
        );
        assert_eq!(row.provider.name, "Saved Snapshot Name");
        assert_eq!(row.api_url.as_deref(), Some("https://live.example.com/v1"));
        assert_eq!(row.primary_model_id.as_deref(), Some("live-model"));
        assert_eq!(
            row.provider.settings_config["baseUrl"].as_str(),
            Some("https://live.example.com/v1")
        );

        let providers =
            ProviderService::list(&state, AppType::OpenClaw).expect("list providers after sync");
        assert_eq!(
            providers
                .get("shared-id")
                .map(|provider| provider.name.as_str()),
            Some("Saved Snapshot Name")
        );
    }

    #[test]
    #[serial]
    fn load_providers_openclaw_normalizes_uncustomized_snapshot_names_to_provider_id() {
        for saved_name in ["", "Live Model Name", "Old Model Name"] {
            let _guard = lock_test_home_and_settings();
            let temp = tempdir().expect("create tempdir");
            let openclaw_dir = temp.path().join(".openclaw");
            std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
            let _home = HomeGuard::set(temp.path());
            let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

            let state = load_state().expect("load state");
            {
                let mut config = state.config.write().expect("lock config");
                let manager = config
                    .get_manager_mut(&AppType::OpenClaw)
                    .expect("openclaw manager");
                manager.providers.insert(
                    "shared-id".to_string(),
                    Provider::with_id(
                        "shared-id".to_string(),
                        saved_name.to_string(),
                        json!({
                            "baseUrl": "https://snapshot.example.com/v1",
                            "models": [
                                {"id": "snapshot-model", "name": "Snapshot Model"}
                            ]
                        }),
                        None,
                    ),
                );
            }
            state.save().expect("persist snapshot provider");

            crate::openclaw_config::set_provider(
                "shared-id",
                json!({
                    "baseUrl": "https://live.example.com/v1",
                    "models": [
                        {"id": "live-model", "name": "Live Model Name"}
                    ]
                }),
            )
            .expect("seed live openclaw provider");

            let snapshot = load_providers(&state, &AppType::OpenClaw).expect("load openclaw rows");
            let row = snapshot
                .rows
                .iter()
                .find(|row| row.id == "shared-id")
                .expect("existing snapshot provider should still be listed");

            assert_eq!(provider_display_name(&AppType::OpenClaw, row), "shared-id");
            assert_eq!(row.provider.name, "shared-id");

            let providers = ProviderService::list(&state, AppType::OpenClaw)
                .expect("list providers after normalization");
            assert_eq!(
                providers
                    .get("shared-id")
                    .map(|provider| provider.name.as_str()),
                Some("shared-id")
            );
        }
    }

    #[test]
    #[serial]
    fn load_providers_openclaw_normalizes_model_like_name_with_only_default_common_config_meta() {
        let _guard = lock_test_home_and_settings();
        let temp = tempdir().expect("create tempdir");
        let openclaw_dir = temp.path().join(".openclaw");
        std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        let _home = HomeGuard::set(temp.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let state = load_state().expect("load state");
        {
            let mut config = state.config.write().expect("lock config");
            let manager = config
                .get_manager_mut(&AppType::OpenClaw)
                .expect("openclaw manager");
            let mut provider = Provider::with_id(
                "shared-id".to_string(),
                "Old Model Name".to_string(),
                json!({
                    "baseUrl": "https://snapshot.example.com/v1",
                    "models": [
                        {"id": "snapshot-model", "name": "Snapshot Model"}
                    ]
                }),
                None,
            );
            provider.meta = Some(crate::provider::ProviderMeta {
                apply_common_config: Some(false),
                ..Default::default()
            });
            manager.providers.insert("shared-id".to_string(), provider);
        }
        state.save().expect("persist snapshot provider");

        crate::openclaw_config::set_provider(
            "shared-id",
            json!({
                "baseUrl": "https://live.example.com/v1",
                "models": [
                    {"id": "live-model", "name": "Live Model Name"}
                ]
            }),
        )
        .expect("seed live openclaw provider");

        let snapshot = load_providers(&state, &AppType::OpenClaw).expect("load openclaw rows");
        let row = snapshot
            .rows
            .iter()
            .find(|row| row.id == "shared-id")
            .expect("existing snapshot provider should still be listed");

        assert_eq!(provider_display_name(&AppType::OpenClaw, row), "shared-id");
        assert_eq!(row.provider.name, "shared-id");
    }

    #[test]
    #[serial]
    fn load_providers_openclaw_keeps_model_like_name_when_row_has_real_metadata() {
        let _guard = lock_test_home_and_settings();
        let temp = tempdir().expect("create tempdir");
        let openclaw_dir = temp.path().join(".openclaw");
        std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        let _home = HomeGuard::set(temp.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let state = load_state().expect("load state");
        {
            let mut config = state.config.write().expect("lock config");
            let manager = config
                .get_manager_mut(&AppType::OpenClaw)
                .expect("openclaw manager");
            let mut provider = Provider::with_id(
                "shared-id".to_string(),
                "Old Model Name".to_string(),
                json!({
                    "baseUrl": "https://snapshot.example.com/v1",
                    "models": [
                        {"id": "snapshot-model", "name": "Snapshot Model"}
                    ]
                }),
                None,
            );
            provider.meta = Some(crate::provider::ProviderMeta {
                apply_common_config: Some(false),
                cost_multiplier: Some("1.2".to_string()),
                ..Default::default()
            });
            manager.providers.insert("shared-id".to_string(), provider);
        }
        state.save().expect("persist snapshot provider");

        crate::openclaw_config::set_provider(
            "shared-id",
            json!({
                "baseUrl": "https://live.example.com/v1",
                "models": [
                    {"id": "live-model", "name": "Live Model Name"}
                ]
            }),
        )
        .expect("seed live openclaw provider");

        let snapshot = load_providers(&state, &AppType::OpenClaw).expect("load openclaw rows");
        let row = snapshot
            .rows
            .iter()
            .find(|row| row.id == "shared-id")
            .expect("existing snapshot provider should still be listed");

        assert_eq!(
            provider_display_name(&AppType::OpenClaw, row),
            "Old Model Name"
        );
        assert_eq!(row.provider.name, "Old Model Name");
    }

    #[test]
    #[serial]
    fn load_providers_openclaw_keeps_saved_only_snapshot_rows_missing_from_live_and_marks_them_out_of_config(
    ) {
        let _guard = lock_test_home_and_settings();
        let temp = tempdir().expect("create tempdir");
        let openclaw_dir = temp.path().join(".openclaw");
        std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        let _home = HomeGuard::set(temp.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        crate::openclaw_config::set_provider(
            "keep",
            json!({
                "baseUrl": "https://keep.example.com/v1",
                "models": [
                    {"id": "keep-model", "name": "Keep Model"}
                ]
            }),
        )
        .expect("seed live openclaw provider");

        let state = load_state().expect("load state");
        {
            let mut config = state.config.write().expect("lock config");
            let manager = config
                .get_manager_mut(&AppType::OpenClaw)
                .expect("openclaw manager");
            let mut provider = Provider::with_id(
                "saved-only".to_string(),
                "Saved Only".to_string(),
                json!({
                    "baseUrl": "https://saved.example.com/v1",
                    "models": [
                        {"id": "saved-model", "name": "Saved Model"}
                    ]
                }),
                None,
            );
            provider.notes = Some("keep this note".to_string());
            manager.providers.insert("saved-only".to_string(), provider);
        }
        state.save().expect("persist stale snapshot provider");

        let snapshot = load_providers(&state, &AppType::OpenClaw).expect("load openclaw rows");
        let row = snapshot
            .rows
            .iter()
            .find(|row| row.id == "saved-only")
            .expect("saved-only OpenClaw rows should remain visible for re-adding to config");
        assert!(!row.is_in_config);
        assert!(row.is_saved);
        assert_eq!(row.api_url.as_deref(), Some("https://saved.example.com/v1"));
        assert_eq!(row.primary_model_id.as_deref(), Some("saved-model"));

        let providers =
            ProviderService::list(&state, AppType::OpenClaw).expect("list providers after sync");
        let saved_only = providers.get("saved-only").expect(
            "loading the OpenClaw screen should preserve saved-only rows in the local mirror",
        );
        assert_eq!(saved_only.name, "Saved Only");
        assert_eq!(saved_only.notes.as_deref(), Some("keep this note"));
        assert_eq!(
            saved_only.settings_config["baseUrl"],
            json!("https://saved.example.com/v1"),
            "rows missing from live OpenClaw config should keep their saved metadata and settings"
        );
        assert!(providers.contains_key("keep"));

        let reloaded_state = load_state().expect("reload state after screen load");
        let reloaded_providers = ProviderService::list(&reloaded_state, AppType::OpenClaw)
            .expect("list providers after reload");
        let reloaded_saved_only = reloaded_providers
            .get("saved-only")
            .expect("saved-only row should remain persisted after mirror sync");
        assert_eq!(reloaded_saved_only.name, "Saved Only");
        assert_eq!(reloaded_saved_only.notes.as_deref(), Some("keep this note"));
    }

    #[test]
    #[serial]
    fn load_providers_openclaw_keeps_saved_snapshot_rows_when_live_config_is_missing() {
        let _guard = lock_test_home_and_settings();
        let temp = tempdir().expect("create tempdir");
        let openclaw_dir = temp.path().join(".openclaw");
        std::fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        let _home = HomeGuard::set(temp.path());
        let _settings = SettingsGuard::with_openclaw_dir(&openclaw_dir);

        let state = load_state().expect("load state");
        {
            let mut config = state.config.write().expect("lock config");
            let manager = config
                .get_manager_mut(&AppType::OpenClaw)
                .expect("openclaw manager");
            manager.providers.insert(
                "saved-only".to_string(),
                Provider::with_id(
                    "saved-only".to_string(),
                    "Saved Only".to_string(),
                    json!({
                        "baseUrl": "https://saved.example.com/v1",
                        "models": [
                            {"id": "saved-model", "name": "Saved Model"}
                        ]
                    }),
                    None,
                ),
            );
        }
        state.save().expect("persist saved snapshot provider");

        let openclaw_path = crate::openclaw_config::get_openclaw_config_path();
        assert!(
            !openclaw_path.exists(),
            "precondition: openclaw.json should be absent for this regression test"
        );

        let snapshot = load_providers(&state, &AppType::OpenClaw).expect("load openclaw rows");
        assert!(
            snapshot.rows.iter().any(|row| row.id == "saved-only"),
            "opening the OpenClaw screen should not purge saved metadata when openclaw.json is missing"
        );

        let providers =
            ProviderService::list(&state, AppType::OpenClaw).expect("list providers after load");
        assert!(
            providers.contains_key("saved-only"),
            "missing openclaw.json should leave the mirrored saved provider metadata intact"
        );
    }

    #[test]
    fn provider_display_name_openclaw_prefers_saved_name_over_primary_model_name() {
        let row = ProviderRow {
            id: "shared-id".to_string(),
            provider: Provider::with_id(
                "shared-id".to_string(),
                "Saved Snapshot Name".to_string(),
                json!({
                    "models": [
                        {"id": "live-model", "name": "Live Model Name"}
                    ]
                }),
                None,
            ),
            api_url: Some("https://live.example.com/v1".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("live-model".to_string()),
            default_model_id: None,
        };

        assert_eq!(
            provider_display_name(&AppType::OpenClaw, &row),
            "Saved Snapshot Name"
        );
        assert_eq!(row.provider.name, "Saved Snapshot Name");
    }

    #[test]
    fn provider_display_name_openclaw_falls_back_to_provider_id_when_name_is_blank() {
        let row = ProviderRow {
            id: "shared-id".to_string(),
            provider: Provider::with_id(
                "shared-id".to_string(),
                "   ".to_string(),
                json!({
                    "models": [
                        {"id": "live-model", "name": "Live Model Name"}
                    ]
                }),
                None,
            ),
            api_url: Some("https://live.example.com/v1".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: Some("live-model".to_string()),
            default_model_id: None,
        };

        assert_eq!(provider_display_name(&AppType::OpenClaw, &row), "shared-id");
    }

    #[test]
    fn provider_display_name_keeps_saved_name_for_non_openclaw_rows() {
        let row = ProviderRow {
            id: "shared-id".to_string(),
            provider: Provider::with_id(
                "shared-id".to_string(),
                "Saved Snapshot Name".to_string(),
                json!({
                    "models": [
                        {"id": "other-model", "name": "Should Not Leak"}
                    ]
                }),
                None,
            ),
            api_url: Some("https://example.com/v1".to_string()),
            is_current: false,
            is_in_config: true,
            is_saved: true,
            is_default_model: false,
            primary_model_id: None,
            default_model_id: None,
        };

        assert_eq!(
            provider_display_name(&AppType::Claude, &row),
            "Saved Snapshot Name"
        );
    }
}
