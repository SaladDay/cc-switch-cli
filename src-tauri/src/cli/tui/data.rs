use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use chrono::{Days, Local, NaiveDate, TimeZone};
use indexmap::IndexMap;
use rusqlite::{params, OptionalExtension};
use rust_decimal::{prelude::ToPrimitive, Decimal};
use serde_json::Value;

use crate::app_config::{AppType, CommonConfigSnippets, McpServer};
#[cfg(test)]
pub(crate) use crate::cli::provider_quota::QuotaTargetKind;
pub(crate) use crate::cli::provider_quota::{ProviderUsageQuota, QuotaTarget};
use crate::commands::workspace::{self, DailyMemoryFileInfo, ALLOWED_FILES};
use crate::database::lock_conn;
use crate::error::AppError;
use crate::hermes_config::{HermesMemoryLimits, MemoryKind};
use crate::openclaw_config::{
    OpenClawAgentsDefaults, OpenClawEnvConfig, OpenClawHealthWarning, OpenClawToolsConfig,
};
use crate::prompt::Prompt;
use crate::prompt_files::prompt_file_path;
use crate::provider::Provider;
use crate::services::config::BackupInfo;
use crate::services::{ConfigService, McpService, PromptService, ProviderService, SkillService};
use crate::store::AppState;

const USAGE_CUSTOM_RANGE_MAX_DAYS: i64 = 3660;
const EMPTY_USAGE_SUMMARY: UsageSummarySnapshot = UsageSummarySnapshot {
    total_requests: 0,
    success_count: 0,
    total_cost_usd: 0.0,
    total_tokens: 0,
    input_tokens: 0,
    output_tokens: 0,
    cache_read_tokens: 0,
    cache_creation_tokens: 0,
    avg_latency_ms: None,
};
const EMPTY_USAGE_TREND: [UsageTrendBucket; 0] = [];
const EMPTY_USAGE_PROVIDER_ROWS: [UsageProviderStatsRow; 0] = [];
const EMPTY_USAGE_MODEL_ROWS: [UsageModelStatsRow; 0] = [];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct UiDataReloadToken(u64);

fn next_reload_token() -> UiDataReloadToken {
    static NEXT_RELOAD_TOKEN: AtomicU64 = AtomicU64::new(1);
    UiDataReloadToken(NEXT_RELOAD_TOKEN.fetch_add(1, Ordering::Relaxed))
}

#[derive(Debug, Clone)]
pub struct ProviderRow {
    pub id: String,
    pub provider: Provider,
    pub api_url: Option<String>,
    pub is_current: bool,
    pub is_in_config: bool,
    // Tracked in the snapshot and asserted in tests, but no longer surfaced in any
    // view since the provider detail page was removed (Enter now opens edit directly).
    #[allow(dead_code)]
    pub is_saved: bool,
    pub is_default_model: bool,
    pub primary_model_id: Option<String>,
    // See `is_saved`: retained for the OpenClaw default-model resolution and its tests.
    #[allow(dead_code)]
    pub default_model_id: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ProviderQuotaState {
    pub(crate) target: QuotaTarget,
    pub(crate) loading: bool,
    pub(crate) manual: bool,
    pub(crate) quota: Option<ProviderUsageQuota>,
    pub(crate) last_error: Option<String>,
    pub(crate) updated_at: Option<i64>,
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
                        updated_at: None,
                    },
                );
            }
        }
    }

    pub(crate) fn finish(&mut self, target: QuotaTarget, quota: ProviderUsageQuota) {
        self.by_provider.insert(
            target.provider_id.clone(),
            ProviderQuotaState {
                target,
                loading: false,
                manual: false,
                quota: Some(quota),
                last_error: None,
                updated_at: Some(chrono::Utc::now().timestamp_millis()),
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
                if state.quota.is_none() {
                    state.updated_at = Some(chrono::Utc::now().timestamp_millis());
                }
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
                        updated_at: Some(chrono::Utc::now().timestamp_millis()),
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
    pub live_ids: HashSet<String>,
    /// True only for the transient projection shown while a cold-switched app's
    /// real data is still loading. Lets the renderer show a "loading" state
    /// instead of the "no providers / import config" empty CTA, so a freshly
    /// switched-to app never momentarily looks empty.
    pub loading: bool,
}

#[derive(Debug, Clone)]
pub struct McpRow {
    pub id: String,
    pub server: Arc<McpServer>,
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

#[derive(Debug, Clone)]
pub struct PromptImportCandidate {
    pub filename: String,
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub struct PromptsSnapshot {
    pub rows: Vec<PromptRow>,
    pub import_candidate: Option<PromptImportCandidate>,
}

#[derive(Debug, Clone, Default)]
pub struct ConfigSnapshot {
    pub config_path: PathBuf,
    pub config_dir: PathBuf,
    pub backups: Vec<BackupInfo>,
    pub common_snippet: String,
    pub common_snippets: CommonConfigSnippets,
    pub webdav_sync: Option<crate::settings::WebDavSyncSettings>,
    pub s3_sync: Option<crate::settings::S3SyncSettings>,
    pub openclaw_config_path: Option<PathBuf>,
    #[allow(dead_code)]
    pub openclaw_config_dir: Option<PathBuf>,
    pub openclaw_env: Option<OpenClawEnvConfig>,
    pub openclaw_tools: Option<OpenClawToolsConfig>,
    pub openclaw_agents_defaults: Option<OpenClawAgentsDefaults>,
    pub openclaw_warnings: Option<Vec<OpenClawHealthWarning>>,
    pub openclaw_workspace: OpenClawWorkspaceSnapshot,
    pub hermes_memory: HermesMemorySnapshot,
}

#[derive(Debug, Clone, Default)]
pub struct OpenClawWorkspaceSnapshot {
    pub directory_path: PathBuf,
    pub file_exists: HashMap<String, bool>,
    pub daily_memory_files: Vec<DailyMemoryFileInfo>,
}

#[derive(Debug, Clone, Default)]
pub struct HermesMemorySnapshot {
    pub directory_path: PathBuf,
    pub memory_content: String,
    pub user_content: String,
    pub limits: HermesMemoryLimits,
}

impl HermesMemorySnapshot {
    pub fn content(&self, kind: MemoryKind) -> &str {
        match kind {
            MemoryKind::Memory => &self.memory_content,
            MemoryKind::User => &self.user_content,
        }
    }

    pub fn limit(&self, kind: MemoryKind) -> usize {
        match kind {
            MemoryKind::Memory => self.limits.memory,
            MemoryKind::User => self.limits.user,
        }
    }

    pub fn enabled(&self, kind: MemoryKind) -> bool {
        match kind {
            MemoryKind::Memory => self.limits.memory_enabled,
            MemoryKind::User => self.limits.user_enabled,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SkillsSnapshot {
    pub installed: Vec<crate::services::skill::InstalledSkill>,
    pub repos: Vec<crate::services::skill::SkillRepo>,
    pub sync_method: crate::services::skill::SyncMethod,
}

#[derive(Debug, Clone, Default)]
pub struct ProxyTargetSnapshot {
    pub provider_id: String,
    pub provider_name: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderHealthSnapshot {
    pub is_healthy: bool,
    pub consecutive_failures: u32,
}

#[derive(Debug, Clone, Default)]
pub struct ProxySnapshot {
    pub enabled: bool,
    pub running: bool,
    pub managed_runtime: bool,
    #[allow(dead_code)]
    pub active_worker_apps: HashSet<String>,
    pub auto_failover_enabled: bool,
    pub claude_takeover: bool,
    pub codex_takeover: bool,
    pub gemini_takeover: bool,
    #[allow(dead_code)]
    pub default_cost_multiplier: Option<String>,
    pub configured_listen_address: String,
    pub configured_listen_port: u16,
    pub listen_address: String,
    pub listen_port: u16,
    pub uptime_seconds: u64,
    #[allow(dead_code)]
    pub total_requests: u64,
    pub estimated_input_tokens_total: u64,
    pub estimated_output_tokens_total: u64,
    #[allow(dead_code)]
    pub success_rate: Option<f32>,
    #[allow(dead_code)]
    pub current_provider: Option<String>,
    pub last_error: Option<String>,
    pub current_app_target: Option<ProxyTargetSnapshot>,
    pub provider_health: Arc<HashMap<String, ProviderHealthSnapshot>>,
}

impl ProxySnapshot {
    pub fn has_active_worker_for(&self, app_type: &AppType) -> bool {
        self.active_worker_apps
            .contains(&app_type.as_str().to_ascii_lowercase())
    }

    pub fn takeover_enabled_for(&self, app_type: &AppType) -> Option<bool> {
        match app_type {
            AppType::Claude => Some(self.claude_takeover),
            AppType::Codex => Some(self.codex_takeover),
            AppType::Gemini => Some(self.gemini_takeover),
            AppType::OpenCode => None,
            AppType::Hermes => None,
            AppType::OpenClaw => None,
        }
    }

    pub fn routes_current_app_through_proxy(&self, app_type: &AppType) -> Option<bool> {
        let takeover_enabled = self.takeover_enabled_for(app_type)?;
        if !self.running || !takeover_enabled {
            return Some(false);
        }

        if self.managed_runtime && !self.active_worker_apps.is_empty() {
            return Some(self.has_active_worker_for(app_type));
        }

        Some(true)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum UsageRangePreset {
    Today,
    #[default]
    SevenDays,
    ThirtyDays,
    Custom(UsageCustomRange),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UsageCustomRange {
    pub start: i64,
    pub end: i64,
}

impl UsageRangePreset {
    pub fn label(self) -> String {
        match self {
            Self::Today => "Today".to_string(),
            Self::SevenDays => "7d".to_string(),
            Self::ThirtyDays => "30d".to_string(),
            Self::Custom(range) => range.label(),
        }
    }

    fn days(self) -> u64 {
        match self {
            Self::Today => 1,
            Self::SevenDays => 7,
            Self::ThirtyDays => 30,
            Self::Custom(range) => range.days(),
        }
    }

    fn uses_hourly_trend(self, start: i64, end: i64) -> bool {
        matches!(self, Self::Today) || end.saturating_sub(start) <= 24 * 60 * 60
    }
}

impl UsageCustomRange {
    pub fn label(self) -> String {
        format!(
            "{}..{}",
            format_usage_custom_range_date(self.start),
            format_usage_custom_range_date(self.end)
        )
    }

    fn days(self) -> u64 {
        let start = Local
            .timestamp_opt(self.start, 0)
            .single()
            .map(|datetime| datetime.date_naive());
        let end = Local
            .timestamp_opt(self.end, 0)
            .single()
            .map(|datetime| datetime.date_naive());
        match (start, end) {
            (Some(start), Some(end)) if end >= start => {
                end.signed_duration_since(start).num_days() as u64 + 1
            }
            _ => 1,
        }
    }
}

pub(crate) fn usage_custom_range_default_input() -> String {
    let today = Local::now().date_naive();
    let start = today.checked_sub_days(Days::new(6)).unwrap_or(today);
    format!("{}..{}", start.format("%Y-%m-%d"), today.format("%Y-%m-%d"))
}

pub(crate) fn parse_usage_custom_range(raw: &str) -> Result<UsageCustomRange, String> {
    let trimmed = raw.trim();
    let parts = split_usage_custom_range_input(trimmed);
    let [start_raw, end_raw] = parts.as_slice() else {
        return Err("Use YYYY-MM-DD..YYYY-MM-DD".to_string());
    };

    let start_date = parse_usage_custom_date(start_raw)?;
    let end_date = parse_usage_custom_date(end_raw)?;
    if end_date < start_date {
        return Err("Start date must be before end date".to_string());
    }
    if end_date.signed_duration_since(start_date).num_days() + 1 > USAGE_CUSTOM_RANGE_MAX_DAYS {
        return Err("Custom range cannot exceed 3660 days".to_string());
    }

    let start = local_midnight_timestamp(start_date);
    let end = local_end_of_day_timestamp(end_date);

    Ok(UsageCustomRange { start, end })
}

fn split_usage_custom_range_input(input: &str) -> Vec<&str> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    for separator in ["..", " - ", " to ", " TO ", ",", "，", ";", "；"] {
        if let Some((start, end)) = trimmed.split_once(separator) {
            return vec![start.trim(), end.trim()];
        }
    }

    let parts = trimmed.split_whitespace().collect::<Vec<_>>();
    if parts.len() == 2 {
        return parts;
    }

    vec![trimmed]
}

fn parse_usage_custom_date(raw: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d")
        .map_err(|_| "Use date format YYYY-MM-DD".to_string())
}

fn format_usage_custom_range_date(timestamp: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|datetime| datetime.date_naive().format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "-".to_string())
}

#[derive(Debug, Clone, Default)]
pub struct UsageSummarySnapshot {
    pub total_requests: u64,
    pub success_count: u64,
    pub total_cost_usd: f64,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub avg_latency_ms: Option<u64>,
}

impl UsageSummarySnapshot {
    pub fn total_tokens(&self) -> u64 {
        if self.total_tokens > 0 {
            return self.total_tokens;
        }

        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_creation_tokens)
    }

    pub fn success_rate(&self) -> Option<f64> {
        (self.total_requests > 0)
            .then(|| self.success_count as f64 * 100.0 / self.total_requests as f64)
    }

    pub fn cache_hit_rate(&self) -> Option<f64> {
        let denominator = self
            .input_tokens
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_creation_tokens);
        (denominator > 0).then(|| self.cache_read_tokens as f64 * 100.0 / denominator as f64)
    }
}

#[derive(Debug, Clone, Default)]
pub struct UsageTrendBucket {
    pub key: String,
    pub label: String,
    pub request_count: u64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub error_count: u64,
}

#[derive(Debug, Clone, Default)]
pub struct UsageProviderStatsRow {
    pub provider_id: String,
    pub provider_name: Option<String>,
    pub request_count: u64,
    pub success_count: u64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub avg_latency_ms: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct UsageModelStatsRow {
    pub model: String,
    pub request_count: u64,
    pub success_count: u64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub avg_latency_ms: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct UsageLogRow {
    pub request_id: String,
    pub created_at: i64,
    pub app_type: String,
    pub provider_id: String,
    pub provider_name: Option<String>,
    pub model: String,
    pub request_model: Option<String>,
    pub status_code: u16,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub(crate) input_token_semantics: i64,
    pub total_cost_usd: f64,
    pub latency_ms: u64,
    pub first_token_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub session_id: Option<String>,
    pub provider_type: Option<String>,
    pub is_streaming: bool,
    pub error_message: Option<String>,
    /// True when the database value exceeded the bounded TUI projection.
    /// The complete value remains in SQLite; list/detail reads never materialize
    /// it in the interactive process.
    pub error_message_truncated: bool,
    pub data_source: Option<String>,
    /// Bitset identifying bounded text projections whose database value was
    /// longer than the value retained by the TUI.
    pub(crate) text_truncation: u16,
    /// Stable SQLite row identity used only by the schema-free keyset pager.
    /// `proxy_request_logs` is a normal rowid table, and the existing
    /// `(app_type, created_at DESC)` index carries rowid as its final key.
    pub(crate) cursor_rowid: i64,
}

pub(crate) const USAGE_LOG_PAGE_SIZE: usize = 100;
pub(crate) const USAGE_LOG_ERROR_MESSAGE_MAX_CHARS: usize = 4_096;
pub(crate) const USAGE_TEXT_MAX_CHARS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub(crate) enum UsageLogTextField {
    RequestId,
    AppType,
    ProviderId,
    ProviderName,
    Model,
    RequestModel,
    SessionId,
    ProviderType,
    DataSource,
}

impl UsageLogTextField {
    const fn mask(self) -> u16 {
        1 << self as u16
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum UsageLogPageDirection {
    /// Rows older than the current page, in the normal newest-to-oldest order.
    Older,
    /// Rows newer than the current page. The SQL query runs in reverse index
    /// order and the worker reverses the bounded result before publishing it.
    Newer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UsageLogCursor {
    pub created_at: i64,
    pub rowid: i64,
}

impl UsageLogCursor {
    pub(crate) fn from_row(row: &UsageLogRow) -> Self {
        Self {
            created_at: row.created_at,
            rowid: row.cursor_rowid,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct UsageLogPage {
    pub rows: Vec<UsageLogRow>,
    pub next_cursor: Option<UsageLogCursor>,
    pub has_more: bool,
}

impl UsageLogRow {
    pub fn total_tokens(&self) -> u64 {
        effective_total_tokens(
            &self.app_type,
            self.data_source.as_deref(),
            self.input_tokens,
            self.output_tokens,
            self.cache_read_tokens,
            self.cache_creation_tokens,
            self.input_token_semantics,
        )
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status_code)
    }

    pub(crate) fn text_was_truncated(&self, field: UsageLogTextField) -> bool {
        self.text_truncation & field.mask() != 0
    }

    pub(crate) fn text_truncation_mask(&self) -> u16 {
        self.text_truncation
    }
}

#[derive(Debug, Clone, Default)]
pub struct UsageSnapshot {
    pub summary_today: UsageSummarySnapshot,
    pub summary_7d: UsageSummarySnapshot,
    pub summary_30d: UsageSummarySnapshot,
    pub trends_today: Vec<UsageTrendBucket>,
    pub trends_7d: Vec<UsageTrendBucket>,
    pub trends_30d: Vec<UsageTrendBucket>,
    pub top_providers_today: Vec<UsageProviderStatsRow>,
    pub top_providers_7d: Vec<UsageProviderStatsRow>,
    pub top_providers_30d: Vec<UsageProviderStatsRow>,
    pub top_models_today: Vec<UsageModelStatsRow>,
    pub top_models_7d: Vec<UsageModelStatsRow>,
    pub top_models_30d: Vec<UsageModelStatsRow>,
    pub custom_range: Option<UsageCustomRange>,
    pub summary_custom: UsageSummarySnapshot,
    pub trends_custom: Vec<UsageTrendBucket>,
    pub top_providers_custom: Vec<UsageProviderStatsRow>,
    pub top_models_custom: Vec<UsageModelStatsRow>,
    pub recent_logs: Vec<UsageLogRow>,
    pub logs_total: u64,
    pub recent_logs_custom: Vec<UsageLogRow>,
    pub logs_total_custom: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ModelPricingRow {
    pub model_id: String,
    pub display_name: String,
    pub input_cost_per_million: String,
    pub output_cost_per_million: String,
    pub cache_read_cost_per_million: String,
    pub cache_creation_cost_per_million: String,
    pub recent_request_count: u64,
    pub recent_total_tokens: u64,
    pub recent_total_cost_usd: f64,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct ModelPricingSnapshot {
    pub rows: Vec<ModelPricingRow>,
    pub recent_unknown_models: u64,
    pub recent_unmatched_total_tokens: u64,
    pub recent_unmatched_total_cost_usd: f64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct UsagePricingData {
    pub usage: UsageSnapshot,
    pub pricing: Option<ModelPricingSnapshot>,
}

impl ModelPricingSnapshot {
    pub fn total_models(&self) -> usize {
        self.rows.len()
    }

    pub fn recently_used_models(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| row.recent_request_count > 0)
            .count()
    }

    pub fn recent_total_cost_usd(&self) -> f64 {
        self.rows
            .iter()
            .map(|row| row.recent_total_cost_usd)
            .sum::<f64>()
            + self.recent_unmatched_total_cost_usd
    }

    pub fn recent_total_tokens(&self) -> u64 {
        self.rows
            .iter()
            .fold(self.recent_unmatched_total_tokens, |total, row| {
                total.saturating_add(row.recent_total_tokens)
            })
    }

    pub fn has_data(&self) -> bool {
        !self.rows.is_empty()
            || self.recent_unknown_models > 0
            || self.recent_unmatched_total_tokens > 0
            || self.recent_unmatched_total_cost_usd > 0.0
    }
}

impl ConfigSnapshot {
    fn loading_projection(&self, app_type: &AppType) -> Self {
        Self {
            config_path: self.config_path.clone(),
            config_dir: self.config_dir.clone(),
            backups: self.backups.clone(),
            common_snippet: self
                .common_snippets
                .get(app_type)
                .cloned()
                .unwrap_or_default(),
            common_snippets: self.common_snippets.clone(),
            webdav_sync: self.webdav_sync.clone(),
            s3_sync: self.s3_sync.clone(),
            openclaw_config_path: None,
            openclaw_config_dir: None,
            openclaw_env: None,
            openclaw_tools: None,
            openclaw_agents_defaults: None,
            openclaw_warnings: None,
            openclaw_workspace: OpenClawWorkspaceSnapshot::default(),
            hermes_memory: HermesMemorySnapshot::default(),
        }
    }
}

impl UsageSnapshot {
    pub(crate) fn begin_custom_range(&mut self, range: UsageCustomRange) {
        self.custom_range = Some(range);
        self.summary_custom = UsageSummarySnapshot::default();
        self.trends_custom = empty_usage_trend(UsageRangePreset::Custom(range));
        self.top_providers_custom.clear();
        self.top_models_custom.clear();
        self.recent_logs_custom.clear();
        self.logs_total_custom = 0;
    }

    pub(crate) fn merge_range(&mut self, range: UsageRangePreset, mut loaded: UsageSnapshot) {
        loaded.recent_logs.truncate(USAGE_LOG_PAGE_SIZE);
        loaded.recent_logs_custom.truncate(USAGE_LOG_PAGE_SIZE);
        match range {
            UsageRangePreset::Custom(custom_range) => {
                self.custom_range = loaded.custom_range.or(Some(custom_range));
                self.summary_custom = loaded.summary_custom;
                self.trends_custom = loaded.trends_custom;
                self.top_providers_custom = loaded.top_providers_custom;
                self.top_models_custom = loaded.top_models_custom;
                self.recent_logs_custom = loaded.recent_logs_custom;
                self.logs_total_custom = loaded.logs_total_custom;
            }
            UsageRangePreset::Today
            | UsageRangePreset::SevenDays
            | UsageRangePreset::ThirtyDays => {
                let custom_range = self.custom_range;
                let summary_custom = self.summary_custom.clone();
                let trends_custom = self.trends_custom.clone();
                let top_providers_custom = self.top_providers_custom.clone();
                let top_models_custom = self.top_models_custom.clone();
                let recent_logs_custom = self.recent_logs_custom.clone();
                let logs_total_custom = self.logs_total_custom;

                *self = loaded;
                self.custom_range = custom_range;
                self.summary_custom = summary_custom;
                self.trends_custom = trends_custom;
                self.top_providers_custom = top_providers_custom;
                self.top_models_custom = top_models_custom;
                self.recent_logs_custom = recent_logs_custom;
                self.logs_total_custom = logs_total_custom;
            }
        }
    }

    /// Publish the first log page without waiting for the more expensive
    /// aggregate queries. Until the exact count arrives, `has_more` contributes
    /// one virtual row so the pager can expose a next-page boundary.
    pub(crate) fn merge_log_head(&mut self, range: UsageRangePreset, page: UsageLogPage) {
        self.merge_log_head_with_total_policy(range, page, false);
    }

    /// Merge a fast log head that completed after its matching aggregate.
    ///
    /// The aggregate owns the exact total. A late head still owns fresher row
    /// content, but its `rows + has_more` count is only a lower bound and must
    /// not downgrade that exact value.
    pub(crate) fn merge_log_head_preserving_exact_total(
        &mut self,
        range: UsageRangePreset,
        page: UsageLogPage,
    ) {
        self.merge_log_head_with_total_policy(range, page, true);
    }

    fn merge_log_head_with_total_policy(
        &mut self,
        range: UsageRangePreset,
        mut page: UsageLogPage,
        preserve_exact_total: bool,
    ) {
        if page.rows.len() > USAGE_LOG_PAGE_SIZE {
            page.rows.truncate(USAGE_LOG_PAGE_SIZE);
            page.has_more = true;
        }
        let lower_bound = u64::try_from(page.rows.len())
            .unwrap_or(u64::MAX)
            .saturating_add(u64::from(page.has_more));

        match range {
            UsageRangePreset::Custom(custom_range) => {
                self.custom_range = Some(custom_range);
                self.recent_logs_custom = page.rows;
                self.logs_total_custom = if preserve_exact_total {
                    self.logs_total_custom.max(lower_bound)
                } else {
                    lower_bound
                };
            }
            UsageRangePreset::Today
            | UsageRangePreset::SevenDays
            | UsageRangePreset::ThirtyDays => {
                self.recent_logs = page.rows;
                self.logs_total = if preserve_exact_total {
                    self.logs_total.max(lower_bound)
                } else {
                    lower_bound
                };
            }
        }
    }

    pub fn summary_for(&self, range: UsageRangePreset) -> &UsageSummarySnapshot {
        match range {
            UsageRangePreset::Today => &self.summary_today,
            UsageRangePreset::SevenDays => &self.summary_7d,
            UsageRangePreset::ThirtyDays => &self.summary_30d,
            UsageRangePreset::Custom(custom_range) if self.custom_range == Some(custom_range) => {
                &self.summary_custom
            }
            UsageRangePreset::Custom(_) => &EMPTY_USAGE_SUMMARY,
        }
    }

    pub fn trend_for(&self, range: UsageRangePreset) -> &[UsageTrendBucket] {
        match range {
            UsageRangePreset::Today => &self.trends_today,
            UsageRangePreset::SevenDays => &self.trends_7d,
            UsageRangePreset::ThirtyDays => &self.trends_30d,
            UsageRangePreset::Custom(custom_range) if self.custom_range == Some(custom_range) => {
                &self.trends_custom
            }
            UsageRangePreset::Custom(_) => &EMPTY_USAGE_TREND,
        }
    }

    pub fn top_providers_for(&self, range: UsageRangePreset) -> &[UsageProviderStatsRow] {
        match range {
            UsageRangePreset::Today => &self.top_providers_today,
            UsageRangePreset::SevenDays => &self.top_providers_7d,
            UsageRangePreset::ThirtyDays => &self.top_providers_30d,
            UsageRangePreset::Custom(custom_range) if self.custom_range == Some(custom_range) => {
                &self.top_providers_custom
            }
            UsageRangePreset::Custom(_) => &EMPTY_USAGE_PROVIDER_ROWS,
        }
    }

    pub fn top_models_for(&self, range: UsageRangePreset) -> &[UsageModelStatsRow] {
        match range {
            UsageRangePreset::Today => &self.top_models_today,
            UsageRangePreset::SevenDays => &self.top_models_7d,
            UsageRangePreset::ThirtyDays => &self.top_models_30d,
            UsageRangePreset::Custom(custom_range) if self.custom_range == Some(custom_range) => {
                &self.top_models_custom
            }
            UsageRangePreset::Custom(_) => &EMPTY_USAGE_MODEL_ROWS,
        }
    }

    pub fn recent_logs_for(&self, range: UsageRangePreset) -> &[UsageLogRow] {
        match range {
            UsageRangePreset::Custom(custom_range) if self.custom_range == Some(custom_range) => {
                &self.recent_logs_custom
            }
            UsageRangePreset::Custom(_) => &[],
            UsageRangePreset::Today
            | UsageRangePreset::SevenDays
            | UsageRangePreset::ThirtyDays => &self.recent_logs,
        }
    }

    pub fn logs_total_for(&self, range: UsageRangePreset) -> u64 {
        match range {
            UsageRangePreset::Custom(custom_range) if self.custom_range == Some(custom_range) => {
                self.logs_total_custom
            }
            UsageRangePreset::Custom(_) => 0,
            UsageRangePreset::Today
            | UsageRangePreset::SevenDays
            | UsageRangePreset::ThirtyDays => self.logs_total,
        }
    }

    pub fn has_data_for(&self, range: UsageRangePreset) -> bool {
        let summary = self.summary_for(range);
        let has_stats = summary.total_requests > 0
            || summary.total_tokens() > 0
            || summary.total_cost_usd > 0.0
            || self.trend_for(range).iter().any(|bucket| {
                bucket.request_count > 0
                    || bucket.total_tokens > 0
                    || bucket.total_cost_usd > 0.0
                    || bucket.error_count > 0
            })
            || !self.top_providers_for(range).is_empty()
            || !self.top_models_for(range).is_empty();
        if has_stats {
            return true;
        }

        matches!(
            range,
            UsageRangePreset::Custom(custom_range)
                if self.custom_range == Some(custom_range)
                    && (!self.recent_logs_custom.is_empty() || self.logs_total_custom > 0)
        )
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
    pub usage: UsageSnapshot,
    pub pricing: ModelPricingSnapshot,
    pub model_routes: ModelRouteSnapshot,
    pub(crate) quota: QuotaSnapshot,
    pub(crate) reload_token: UiDataReloadToken,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ModelRouteRow {
    pub id: String,
    pub pattern: String,
    pub provider_id: String,
    pub provider_name: String,
    pub priority: i32,
    pub enabled: bool,
    pub hit_count: i64,
    pub last_hit_at: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ModelRouteSnapshot {
    pub rows: Vec<ModelRouteRow>,
}

pub(crate) fn load_state() -> Result<AppState, AppError> {
    AppState::try_new()
}

pub(crate) fn load_snapshot_state() -> Result<AppState, AppError> {
    AppState::try_open_snapshot()
}

impl UiData {
    pub(crate) fn mark_current_app_data_changed(&mut self) {
        self.reload_token = next_reload_token();
    }

    pub(crate) fn refresh_current_app_provider_data(
        &mut self,
        state: &AppState,
        app_type: &AppType,
    ) -> Result<(), AppError> {
        self.providers = load_providers_with_mode(state, app_type, ProviderLoadMode::SyncLive)?;
        self.proxy = load_proxy_snapshot_from_state(state, app_type)?;
        Ok(())
    }

    pub(crate) fn refresh_current_app_config_data(
        &mut self,
        state: &AppState,
        app_type: &AppType,
    ) -> Result<(), AppError> {
        self.config = load_config_snapshot(state, app_type)?;
        Ok(())
    }

    pub fn load(app_type: &AppType) -> Result<Self, AppError> {
        let state = load_state()?;

        let mut data = Self::load_base_from_state(&state, app_type)?;
        let usage_pricing = load_usage_pricing_data_from_state(&state, app_type)?;
        data.usage = usage_pricing.usage;
        if let Some(pricing) = usage_pricing.pricing {
            data.pricing = pricing;
        }

        Ok(data)
    }

    /// Like [`load`], but skips the usage/pricing aggregation (several DB
    /// GROUP-BY queries + the pricing snapshot). Used for the active app's
    /// post-startup full reload so that work is deferred until the Usage view is
    /// actually opened (`usage`/`pricing` are left at their defaults and filled
    /// in lazily by the usage-pricing worker).
    pub fn load_without_usage_pricing(app_type: &AppType) -> Result<Self, AppError> {
        let state = load_state()?;
        Self::load_base_from_state(&state, app_type)
    }

    pub(crate) fn load_fast_snapshot_from_state(
        state: &AppState,
        app_type: &AppType,
    ) -> Result<Self, AppError> {
        Self::load_base_from_state_with_mode(state, app_type, ProviderLoadMode::SnapshotOnly)
    }

    fn load_base_from_state(state: &AppState, app_type: &AppType) -> Result<Self, AppError> {
        Self::load_base_from_state_with_mode(state, app_type, ProviderLoadMode::SyncLive)
    }

    fn load_base_from_state_with_mode(
        state: &AppState,
        app_type: &AppType,
        provider_load_mode: ProviderLoadMode,
    ) -> Result<Self, AppError> {
        let providers = load_providers_with_mode(state, app_type, provider_load_mode)?;
        let mcp = load_mcp(state)?;
        let prompts = load_prompts(state, app_type)?;
        let config = load_config_snapshot(state, app_type)?;
        let skills = match provider_load_mode {
            ProviderLoadMode::SyncLive => load_skills_snapshot()?,
            ProviderLoadMode::SnapshotOnly => load_skills_snapshot_from_state(state)?,
        };
        let proxy = load_proxy_snapshot_from_state(state, app_type)?;

        Ok(Self {
            providers,
            mcp,
            prompts,
            config,
            skills,
            proxy,
            usage: UsageSnapshot::default(),
            pricing: ModelPricingSnapshot::default(),
            model_routes: ModelRouteSnapshot::default(),
            quota: QuotaSnapshot::default(),
            reload_token: next_reload_token(),
        })
    }

    pub(crate) fn app_switch_loading_projection(&self, app_type: &AppType) -> Self {
        let mut proxy = self.proxy.clone();
        proxy.auto_failover_enabled = false;
        proxy.default_cost_multiplier = None;
        proxy.current_app_target = None;
        proxy.provider_health = Arc::default();

        Self {
            providers: ProvidersSnapshot {
                loading: true,
                ..ProvidersSnapshot::default()
            },
            mcp: self.mcp.clone(),
            prompts: PromptsSnapshot::default(),
            config: self.config.loading_projection(app_type),
            skills: self.skills.clone(),
            proxy,
            usage: UsageSnapshot::default(),
            pricing: ModelPricingSnapshot::default(),
            model_routes: ModelRouteSnapshot::default(),
            quota: QuotaSnapshot::default(),
            reload_token: next_reload_token(),
        }
    }

    // This method is called repeatedly during editing.
    // The ProvidersSnapshot in UiData never change, so it is safe to cache the existing_provider_ids.
    // However, the tests relied on a mutable and ProvidersSnapshot in UiData,
    // so keep it re-computed every time for simplicity.
    // If performance becomes an issue, we can refactor the tests to allow caching the existing_provider_ids.
    pub(crate) fn existing_provider_ids(&self) -> Vec<String> {
        let mut ids = self
            .providers
            .rows
            .iter()
            .map(|row| row.id.clone())
            .collect::<HashSet<_>>();
        ids.extend(self.providers.live_ids.iter().cloned());
        ids.into_iter().collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderLoadMode {
    SyncLive,
    SnapshotOnly,
}

pub(crate) fn load_usage_pricing_data_from_state(
    state: &AppState,
    app_type: &AppType,
) -> Result<UsagePricingData, AppError> {
    load_usage_pricing_data_from_state_for_range(state, app_type, UsageRangePreset::SevenDays)
}

pub(crate) fn load_usage_pricing_data_from_state_for_range(
    state: &AppState,
    app_type: &AppType,
    range: UsageRangePreset,
) -> Result<UsagePricingData, AppError> {
    let pricing = match range {
        UsageRangePreset::Custom(_) => None,
        UsageRangePreset::Today | UsageRangePreset::SevenDays | UsageRangePreset::ThirtyDays => {
            Some(load_model_pricing_snapshot(state, app_type)?)
        }
    };

    Ok(UsagePricingData {
        usage: load_usage_snapshot_for_range(state, app_type, range)?,
        pricing,
    })
}

pub(crate) fn load_usage_log_page_from_state(
    state: &AppState,
    app_type: &AppType,
    range: UsageRangePreset,
    cursor: Option<&UsageLogCursor>,
    direction: UsageLogPageDirection,
    limit: usize,
) -> Result<UsageLogPage, AppError> {
    load_usage_log_page_from_database(&state.db, app_type, range, cursor, direction, limit)
}

pub(crate) fn load_usage_log_page_from_database(
    db: &crate::Database,
    app_type: &AppType,
    range: UsageRangePreset,
    cursor: Option<&UsageLogCursor>,
    direction: UsageLogPageDirection,
    limit: usize,
) -> Result<UsageLogPage, AppError> {
    let log_range = match range {
        UsageRangePreset::Custom(custom) => Some((custom.start, custom.end)),
        UsageRangePreset::Today | UsageRangePreset::SevenDays | UsageRangePreset::ThirtyDays => {
            None
        }
    };
    let conn = lock_conn!(db.conn);
    load_usage_log_page(
        &conn,
        app_type.as_str(),
        log_range,
        cursor,
        direction,
        limit,
    )
}

pub(crate) fn load_usage_log_detail_from_state(
    state: &AppState,
    app_type: &AppType,
    range: UsageRangePreset,
    rowid: i64,
) -> Result<Option<UsageLogRow>, AppError> {
    load_usage_log_detail_from_database(&state.db, app_type, range, rowid)
}

pub(crate) fn load_usage_log_detail_from_database(
    db: &crate::Database,
    app_type: &AppType,
    range: UsageRangePreset,
    rowid: i64,
) -> Result<Option<UsageLogRow>, AppError> {
    let log_range = match range {
        UsageRangePreset::Custom(custom) => Some((custom.start, custom.end)),
        UsageRangePreset::Today | UsageRangePreset::SevenDays | UsageRangePreset::ThirtyDays => {
            None
        }
    };
    let conn = lock_conn!(db.conn);
    load_usage_log_detail(&conn, app_type.as_str(), log_range, rowid)
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

pub(crate) fn provider_is_read_only(app_type: &AppType, row: &ProviderRow) -> bool {
    matches!(app_type, AppType::Hermes)
        && row
            .provider
            .settings_config
            .get(crate::hermes_config::PROVIDER_SOURCE_FIELD)
            .and_then(Value::as_str)
            == Some(crate::hermes_config::PROVIDER_SOURCE_DICT)
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
    crate::cli::provider_quota::quota_target_for_provider(app_type, &row.id, &row.provider)
}

#[cfg(test)]
fn load_providers(state: &AppState, app_type: &AppType) -> Result<ProvidersSnapshot, AppError> {
    load_providers_with_mode(state, app_type, ProviderLoadMode::SyncLive)
}

fn load_providers_with_mode(
    state: &AppState,
    app_type: &AppType,
    mode: ProviderLoadMode,
) -> Result<ProvidersSnapshot, AppError> {
    if mode == ProviderLoadMode::SyncLive && matches!(app_type, AppType::OpenClaw) {
        ProviderService::sync_openclaw_providers_from_live(state)?;
    }

    let providers = ProviderService::list(state, app_type.clone())?;
    let current_id = current_provider_for_mode(state, app_type, mode, &providers)?;
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
    let hermes_live_ids = if matches!(app_type, AppType::Hermes) {
        crate::hermes_config::get_providers()?
            .into_iter()
            .map(|(id, _)| id)
            .collect::<HashSet<_>>()
    } else {
        HashSet::new()
    };
    let hermes_current_provider_id = if matches!(app_type, AppType::Hermes) {
        crate::hermes_config::get_current_provider_id()?
    } else {
        None
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

    let mut rows = sorted
        .into_iter()
        .map(|(id, provider)| {
            let openclaw_live_provider = openclaw_live_providers
                .get(&id)
                .filter(|_| openclaw_live_ids.contains(&id));
            let provider = openclaw_provider_for_row(&id, provider, openclaw_live_provider);

            ProviderRow {
                api_url: extract_api_url(&provider.settings_config, app_type),
                is_current: match app_type {
                    AppType::Hermes => hermes_current_provider_id.as_deref() == Some(id.as_str()),
                    _ => id == current_id,
                },
                is_in_config: match app_type {
                    AppType::OpenCode => opencode_live_ids.contains(&id),
                    AppType::Hermes => hermes_live_ids.contains(&id),
                    AppType::OpenClaw => openclaw_live_ids.contains(&id),
                    _ => true,
                },
                is_saved: true,
                is_default_model: match app_type {
                    AppType::Hermes => hermes_current_provider_id.as_deref() == Some(id.as_str()),
                    _ => openclaw_primary_default_provider_id.as_deref() == Some(id.as_str()),
                },
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

    if matches!(app_type, AppType::OpenClaw) && mode == ProviderLoadMode::SnapshotOnly {
        let saved_ids = rows
            .iter()
            .map(|row| row.id.clone())
            .collect::<HashSet<_>>();
        let mut live_only_ids = openclaw_live_ids
            .iter()
            .filter(|id| !saved_ids.contains(*id))
            .cloned()
            .collect::<Vec<_>>();
        live_only_ids.sort();

        for id in live_only_ids {
            let Some(live_provider) = openclaw_live_providers.get(&id) else {
                continue;
            };
            let provider = Provider::with_id(id.clone(), id.clone(), live_provider.clone(), None);

            rows.push(ProviderRow {
                api_url: extract_api_url(&provider.settings_config, app_type),
                is_current: false,
                is_in_config: true,
                is_saved: false,
                is_default_model: openclaw_primary_default_provider_id.as_deref()
                    == Some(id.as_str()),
                primary_model_id: extract_primary_model_id(
                    &provider.settings_config,
                    app_type,
                    Some(live_provider),
                ),
                default_model_id: openclaw_default_model_ids.get(&id).cloned(),
                id,
                provider,
            });
        }
    }

    let rows = if matches!(app_type, AppType::OpenClaw) {
        rows.into_iter()
            .filter(|row| !openclaw_live_providers.contains_key(&row.id) || row.is_in_config)
            .collect::<Vec<_>>()
    } else {
        rows
    };

    let live_ids = match app_type {
        AppType::OpenCode => opencode_live_ids,
        AppType::Hermes => hermes_live_ids,
        AppType::OpenClaw => openclaw_live_providers.keys().cloned().collect(),
        _ => HashSet::new(),
    };

    Ok(ProvidersSnapshot {
        current_id,
        rows,
        live_ids,
        loading: false,
    })
}

fn current_provider_for_mode(
    state: &AppState,
    app_type: &AppType,
    mode: ProviderLoadMode,
    providers: &IndexMap<String, Provider>,
) -> Result<String, AppError> {
    if mode == ProviderLoadMode::SyncLive {
        return ProviderService::current(state, app_type.clone());
    }

    if matches!(app_type, AppType::Hermes) {
        return crate::hermes_config::get_current_provider_id().map(|opt| opt.unwrap_or_default());
    }
    if app_type.is_additive_mode() {
        return Ok(String::new());
    }
    if let Some(local_id) = crate::settings::get_current_provider(app_type) {
        if providers.contains_key(&local_id) {
            return Ok(local_id);
        }
    }

    state
        .db
        .get_current_provider(app_type.as_str())
        .map(|opt| opt.unwrap_or_default())
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
        AppType::Hermes => settings_config
            .get("base_url")
            .or_else(|| settings_config.get("baseUrl"))
            .or_else(|| settings_config.get("baseURL"))
            .or_else(|| settings_config.get("endpoint"))?
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
        AppType::Hermes => hermes_primary_model_id(settings_config),
        AppType::OpenClaw => match openclaw_live_provider {
            Some(live_provider) => openclaw_primary_model_id(live_provider),
            None => openclaw_primary_model_id(settings_config),
        },
        _ => None,
    }
}

fn hermes_primary_model_id(provider_value: &Value) -> Option<String> {
    provider_value
        .get("model")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            provider_value
                .get("models")
                .and_then(Value::as_object)
                .and_then(|models| models.keys().next().cloned())
        })
        .or_else(|| {
            provider_value
                .get("models")
                .and_then(Value::as_array)
                .and_then(|models| models.first())
                .and_then(|model| model.get("id"))
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(ToOwned::to_owned)
        })
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
        .map(|(id, server)| McpRow {
            id,
            server: Arc::new(server),
        })
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

    sort_prompt_rows(&mut rows);

    let import_candidate = if rows.is_empty() {
        load_prompt_import_candidate(app_type)
    } else {
        None
    };

    Ok(PromptsSnapshot {
        rows,
        import_candidate,
    })
}

fn sort_prompt_rows(rows: &mut [PromptRow]) {
    rows.sort_by(|a, b| {
        a.prompt
            .created_at
            .unwrap_or(0)
            .cmp(&b.prompt.created_at.unwrap_or(0))
            .then_with(|| a.id.cmp(&b.id))
    });
}

fn load_prompt_import_candidate(app_type: &AppType) -> Option<PromptImportCandidate> {
    let path = prompt_file_path(app_type).ok()?;
    if !path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&path).ok()?;
    if content.trim().is_empty() {
        return None;
    }

    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.display().to_string());

    Some(PromptImportCandidate { filename, content })
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
    let settings = crate::settings::get_settings();
    let openclaw_snapshot = load_openclaw_config_snapshot(app_type)?;
    let openclaw_workspace = load_openclaw_workspace_snapshot(app_type)?;
    let hermes_memory = load_hermes_memory_snapshot(app_type)?;

    Ok(ConfigSnapshot {
        config_path,
        config_dir,
        backups,
        common_snippet,
        common_snippets,
        webdav_sync: settings.webdav_sync,
        s3_sync: settings.s3_sync,
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
        hermes_memory,
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

fn load_hermes_memory_snapshot(app_type: &AppType) -> Result<HermesMemorySnapshot, AppError> {
    if !matches!(app_type, AppType::Hermes) {
        return Ok(HermesMemorySnapshot::default());
    }

    Ok(HermesMemorySnapshot {
        directory_path: crate::hermes_config::get_hermes_dir().join("memories"),
        memory_content: crate::hermes_config::read_memory(MemoryKind::Memory)?,
        user_content: crate::hermes_config::read_memory(MemoryKind::User)?,
        limits: crate::hermes_config::read_memory_limits()?,
    })
}

fn load_usage_snapshot_for_range(
    state: &AppState,
    app_type: &AppType,
    range: UsageRangePreset,
) -> Result<UsageSnapshot, AppError> {
    match range {
        UsageRangePreset::Custom(custom_range) => {
            load_usage_custom_snapshot(state, app_type, custom_range)
        }
        UsageRangePreset::Today | UsageRangePreset::SevenDays | UsageRangePreset::ThirtyDays => {
            load_usage_fixed_snapshot(state, app_type)
        }
    }
}

fn load_usage_fixed_snapshot(
    state: &AppState,
    app_type: &AppType,
) -> Result<UsageSnapshot, AppError> {
    let app_key = app_type.as_str();
    let now = Local::now().timestamp();
    let today_start = usage_range_start(UsageRangePreset::Today);
    let seven_start = usage_range_start(UsageRangePreset::SevenDays);
    let thirty_start = usage_range_start(UsageRangePreset::ThirtyDays);

    let conn = lock_conn!(state.db.conn);
    let summary_today = load_usage_summary(&conn, app_key, today_start, now)?;
    let summary_7d = load_usage_summary(&conn, app_key, seven_start, now)?;
    let summary_30d = load_usage_summary(&conn, app_key, thirty_start, now)?;
    let trends_today = load_usage_trend(&conn, app_key, UsageRangePreset::Today, today_start, now)?;
    let trends_7d = load_usage_trend(
        &conn,
        app_key,
        UsageRangePreset::SevenDays,
        seven_start,
        now,
    )?;
    let trends_30d = load_usage_trend(
        &conn,
        app_key,
        UsageRangePreset::ThirtyDays,
        thirty_start,
        now,
    )?;
    let top_providers_today = load_usage_top_providers(&conn, app_key, today_start, now)?;
    let top_providers_7d = load_usage_top_providers(&conn, app_key, seven_start, now)?;
    let top_providers_30d = load_usage_top_providers(&conn, app_key, thirty_start, now)?;
    let top_models_today = load_usage_top_models(&conn, app_key, today_start, now)?;
    let top_models_7d = load_usage_top_models(&conn, app_key, seven_start, now)?;
    let top_models_30d = load_usage_top_models(&conn, app_key, thirty_start, now)?;
    let recent_logs = load_usage_recent_logs(&conn, app_key, None, 100)?;
    let logs_total = load_usage_logs_total(&conn, app_key, None)?;

    Ok(UsageSnapshot {
        summary_today,
        summary_7d,
        summary_30d,
        trends_today,
        trends_7d,
        trends_30d,
        top_providers_today,
        top_providers_7d,
        top_providers_30d,
        top_models_today,
        top_models_7d,
        top_models_30d,
        recent_logs,
        logs_total,
        ..UsageSnapshot::default()
    })
}

fn load_usage_custom_snapshot(
    state: &AppState,
    app_type: &AppType,
    custom_range: UsageCustomRange,
) -> Result<UsageSnapshot, AppError> {
    let app_key = app_type.as_str();
    let conn = lock_conn!(state.db.conn);
    let summary_custom = load_usage_summary(&conn, app_key, custom_range.start, custom_range.end)?;
    let trends_custom = load_usage_trend(
        &conn,
        app_key,
        UsageRangePreset::Custom(custom_range),
        custom_range.start,
        custom_range.end,
    )?;
    let top_providers_custom =
        load_usage_top_providers(&conn, app_key, custom_range.start, custom_range.end)?;
    let top_models_custom =
        load_usage_top_models(&conn, app_key, custom_range.start, custom_range.end)?;
    let log_range = Some((custom_range.start, custom_range.end));
    let recent_logs = load_usage_recent_logs(&conn, app_key, log_range, 100)?;
    let logs_total = load_usage_logs_total(&conn, app_key, log_range)?;

    Ok(UsageSnapshot {
        custom_range: Some(custom_range),
        summary_custom,
        trends_custom,
        top_providers_custom,
        top_models_custom,
        recent_logs_custom: recent_logs,
        logs_total_custom: logs_total,
        ..UsageSnapshot::default()
    })
}

fn load_model_pricing_snapshot(
    state: &AppState,
    app_type: &AppType,
) -> Result<ModelPricingSnapshot, AppError> {
    let app_key = app_type.as_str();
    let now = Local::now().timestamp();
    let thirty_start = usage_range_start(UsageRangePreset::ThirtyDays);

    let conn = lock_conn!(state.db.conn);
    load_model_pricing_snapshot_from_conn(&conn, app_key, thirty_start, now)
}

fn load_model_pricing_snapshot_from_conn(
    conn: &rusqlite::Connection,
    app_key: &str,
    thirty_start: i64,
    now: i64,
) -> Result<ModelPricingSnapshot, AppError> {
    let mut pricing_stmt = conn.prepare(&format!(
        "SELECT
            substr(CAST(model_id AS TEXT), 1, {text_limit}),
            substr(CAST(display_name AS TEXT), 1, {text_limit}),
            substr(CAST(input_cost_per_million AS TEXT), 1, {text_limit}),
            substr(CAST(output_cost_per_million AS TEXT), 1, {text_limit}),
            substr(CAST(cache_read_cost_per_million AS TEXT), 1, {text_limit}),
            substr(CAST(cache_creation_cost_per_million AS TEXT), 1, {text_limit})
         FROM model_pricing
         ORDER BY LOWER(model_id)",
        text_limit = USAGE_TEXT_MAX_CHARS,
    ))?;

    let rows = pricing_stmt.query_map([], |row| {
        Ok(ModelPricingRow {
            model_id: row.get(0)?,
            display_name: row.get(1)?,
            input_cost_per_million: row.get(2)?,
            output_cost_per_million: row.get(3)?,
            cache_read_cost_per_million: row.get(4)?,
            cache_creation_cost_per_million: row.get(5)?,
            recent_request_count: 0,
            recent_total_tokens: 0,
            recent_total_cost_usd: 0.0,
            last_used_at: None,
        })
    })?;
    let mut rows = rows.collect::<Result<Vec<_>, _>>()?;
    let row_indices = rows
        .iter()
        .enumerate()
        .map(|(idx, row)| (row.model_id.clone(), idx))
        .collect::<HashMap<_, _>>();

    let total_tokens_expr = usage_real_total_tokens_sql(Some("l"));
    let fresh_input_expr = crate::services::sql_helpers::fresh_input_sql("l");
    let effective_filter = crate::services::usage_stats::effective_usage_log_filter("l");
    let mut recent_stmt = conn.prepare(&format!(
        "SELECT
            substr(CAST(COALESCE(NULLIF(TRIM(l.model), ''), 'unknown') AS TEXT), 1, {text_limit}) AS response_model,
            substr(CAST(NULLIF(TRIM(l.request_model), '') AS TEXT), 1, {text_limit}) AS request_model,
            substr(CAST(COALESCE(NULLIF(TRIM(l.cost_multiplier), ''), '1') AS TEXT), 1, {text_limit}) AS cost_multiplier,
            COUNT(*) AS request_count,
            COALESCE(SUM({total_tokens_expr}), 0) AS total_tokens,
            COALESCE(SUM({fresh_input_expr}), 0) AS fresh_input_tokens,
            COALESCE(SUM(l.output_tokens), 0) AS output_tokens,
            COALESCE(SUM(l.cache_read_tokens), 0) AS cache_read_tokens,
            COALESCE(SUM(l.cache_creation_tokens), 0) AS cache_creation_tokens,
            COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0.0) AS total_cost_usd,
            MAX(l.created_at) AS last_used_at
            FROM proxy_request_logs l
            WHERE l.app_type = ?1 AND l.created_at >= ?2 AND l.created_at <= ?3
              AND {effective_filter}
            GROUP BY
                COALESCE(NULLIF(TRIM(l.model), ''), 'unknown'),
                NULLIF(TRIM(l.request_model), ''),
                COALESCE(NULLIF(TRIM(l.cost_multiplier), ''), '1')",
        text_limit = USAGE_TEXT_MAX_CHARS,
    ))?;
    let recent_rows = recent_stmt.query_map(params![app_key, thirty_start, now], |row| {
        Ok(RecentPricingUsageRow {
            response_model: row.get(0)?,
            request_model: normalize_optional_string(row.get::<_, Option<String>>(1)?),
            cost_multiplier: row.get(2)?,
            request_count: non_negative_u64(row.get::<_, i64>(3)?),
            total_tokens: non_negative_u64(row.get::<_, i64>(4)?),
            fresh_input_tokens: non_negative_u64(row.get::<_, i64>(5)?),
            output_tokens: non_negative_u64(row.get::<_, i64>(6)?),
            cache_read_tokens: non_negative_u64(row.get::<_, i64>(7)?),
            cache_creation_tokens: non_negative_u64(row.get::<_, i64>(8)?),
            total_cost_usd: row.get::<_, f64>(9)?.max(0.0),
            last_used_at: row.get::<_, Option<i64>>(10)?,
        })
    })?;

    let mut recent_unknown_models = HashSet::new();
    let mut recent_unmatched_total_tokens = 0u64;
    let mut recent_unmatched_total_cost_usd = 0.0f64;
    for recent in recent_rows {
        let recent = recent?;
        let Some(matched) = find_pricing_match_for_log(conn, &recent)? else {
            recent_unknown_models.insert(unmatched_pricing_model_key(
                &recent.response_model,
                recent.request_model.as_deref(),
            ));
            recent_unmatched_total_tokens =
                recent_unmatched_total_tokens.saturating_add(recent.total_tokens);
            recent_unmatched_total_cost_usd += recent.total_cost_usd;
            continue;
        };
        let Some(idx) = row_indices.get(&matched.model_id).copied() else {
            recent_unknown_models.insert(unmatched_pricing_model_key(
                &recent.response_model,
                recent.request_model.as_deref(),
            ));
            recent_unmatched_total_tokens =
                recent_unmatched_total_tokens.saturating_add(recent.total_tokens);
            recent_unmatched_total_cost_usd += recent.total_cost_usd;
            continue;
        };
        let row = &mut rows[idx];
        row.recent_request_count = row
            .recent_request_count
            .saturating_add(recent.request_count);
        row.recent_total_tokens = row.recent_total_tokens.saturating_add(recent.total_tokens);
        row.recent_total_cost_usd += recent.total_cost_usd;
        row.last_used_at = match (row.last_used_at, recent.last_used_at) {
            (Some(current), Some(next)) => Some(current.max(next)),
            (None, Some(next)) => Some(next),
            (current, None) => current,
        };
    }

    rows.sort_by(|a, b| {
        let a_unused = a.recent_request_count == 0;
        let b_unused = b.recent_request_count == 0;
        a_unused
            .cmp(&b_unused)
            .then_with(|| b.recent_request_count.cmp(&a.recent_request_count))
            .then_with(|| {
                b.recent_total_cost_usd
                    .partial_cmp(&a.recent_total_cost_usd)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| {
                a.model_id
                    .to_ascii_lowercase()
                    .cmp(&b.model_id.to_ascii_lowercase())
            })
    });

    Ok(ModelPricingSnapshot {
        rows,
        recent_unknown_models: recent_unknown_models.len() as u64,
        recent_unmatched_total_tokens,
        recent_unmatched_total_cost_usd,
    })
}

#[derive(Debug, Clone)]
struct RecentPricingUsageRow {
    response_model: String,
    request_model: Option<String>,
    cost_multiplier: String,
    request_count: u64,
    total_tokens: u64,
    fresh_input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    total_cost_usd: f64,
    last_used_at: Option<i64>,
}

fn find_pricing_match_for_log(
    conn: &rusqlite::Connection,
    recent: &RecentPricingUsageRow,
) -> Result<Option<crate::services::usage_stats::ModelPricingMatch>, AppError> {
    let response_match =
        crate::services::usage_stats::find_model_pricing_match(conn, &recent.response_model)?;

    let request_match = match recent.request_model.as_deref() {
        Some(request_model)
            if !request_model
                .trim()
                .eq_ignore_ascii_case(recent.response_model.trim()) =>
        {
            crate::services::usage_stats::find_model_pricing_match(conn, request_model)?
        }
        _ => None,
    };

    match (response_match, request_match) {
        (Some(response), Some(request)) => {
            let response_score = pricing_match_cost_delta(&response.pricing, recent);
            let request_score = pricing_match_cost_delta(&request.pricing, recent);
            if request_score < response_score {
                Ok(Some(request))
            } else {
                Ok(Some(response))
            }
        }
        (Some(response), None) => Ok(Some(response)),
        (None, Some(request)) => Ok(Some(request)),
        (None, None) => Ok(None),
    }
}

fn pricing_match_cost_delta(
    pricing: &crate::proxy::usage::calculator::ModelPricing,
    recent: &RecentPricingUsageRow,
) -> f64 {
    let expected = expected_pricing_cost_usd(pricing, recent);
    (expected - recent.total_cost_usd).abs()
}

fn expected_pricing_cost_usd(
    pricing: &crate::proxy::usage::calculator::ModelPricing,
    recent: &RecentPricingUsageRow,
) -> f64 {
    let million = Decimal::from(1_000_000u32);
    let multiplier = recent
        .cost_multiplier
        .trim()
        .parse::<Decimal>()
        .unwrap_or(Decimal::ONE);
    let total = ((Decimal::from(recent.fresh_input_tokens) * pricing.input_cost_per_million)
        + (Decimal::from(recent.output_tokens) * pricing.output_cost_per_million)
        + (Decimal::from(recent.cache_read_tokens) * pricing.cache_read_cost_per_million)
        + (Decimal::from(recent.cache_creation_tokens) * pricing.cache_creation_cost_per_million))
        / million
        * multiplier;

    total.to_f64().unwrap_or(f64::INFINITY)
}

fn unmatched_pricing_model_key(response_model: &str, request_model: Option<&str>) -> String {
    let response_model = response_model.trim();
    if !response_model.is_empty() && !matches!(response_model, "unknown" | "null" | "none") {
        return response_model.to_string();
    }

    request_model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(response_model)
        .to_string()
}

fn usage_range_start(range: UsageRangePreset) -> i64 {
    let today = Local::now().date_naive();
    let start_date = today
        .checked_sub_days(Days::new(range.days().saturating_sub(1)))
        .unwrap_or(today);
    local_midnight_timestamp(start_date)
}

fn local_midnight_timestamp(date: NaiveDate) -> i64 {
    let Some(naive) = date.and_hms_opt(0, 0, 0) else {
        return 0;
    };
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|datetime| datetime.timestamp())
        .unwrap_or(0)
}

fn local_end_of_day_timestamp(date: NaiveDate) -> i64 {
    let Some(naive) = date.and_hms_opt(23, 59, 59) else {
        return 0;
    };
    Local
        .from_local_datetime(&naive)
        .latest()
        .map(|datetime| datetime.timestamp())
        .unwrap_or(0)
}

fn load_usage_summary(
    conn: &rusqlite::Connection,
    app_key: &str,
    start: i64,
    end: i64,
) -> Result<UsageSummarySnapshot, AppError> {
    let total_tokens_expr = usage_real_total_tokens_sql(Some("l"));
    let fresh_input_expr = crate::services::sql_helpers::fresh_input_sql("l");
    let effective_filter = crate::services::usage_stats::effective_usage_log_filter("l");
    let sql = format!(
        "SELECT
            COUNT(*),
            COALESCE(SUM(CASE WHEN l.status_code >= 200 AND l.status_code < 300 THEN 1 ELSE 0 END), 0),
            COALESCE(SUM({total_tokens_expr}), 0),
            COALESCE(SUM({fresh_input_expr}), 0),
            COALESCE(SUM(l.output_tokens), 0),
            COALESCE(SUM(l.cache_read_tokens), 0),
            COALESCE(SUM(l.cache_creation_tokens), 0),
            COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0.0),
            AVG(CASE WHEN l.latency_ms > 0 THEN l.latency_ms END)
         FROM proxy_request_logs l
         WHERE l.app_type = ?1 AND l.created_at >= ?2 AND l.created_at <= ?3
           AND {effective_filter}"
    );
    conn.query_row(&sql, params![app_key, start, end], |row| {
        Ok(UsageSummarySnapshot {
            total_requests: non_negative_u64(row.get::<_, i64>(0)?),
            success_count: non_negative_u64(row.get::<_, i64>(1)?),
            total_tokens: non_negative_u64(row.get::<_, i64>(2)?),
            input_tokens: non_negative_u64(row.get::<_, i64>(3)?),
            output_tokens: non_negative_u64(row.get::<_, i64>(4)?),
            cache_read_tokens: non_negative_u64(row.get::<_, i64>(5)?),
            cache_creation_tokens: non_negative_u64(row.get::<_, i64>(6)?),
            total_cost_usd: row.get::<_, f64>(7)?,
            avg_latency_ms: optional_average_u64(row.get::<_, Option<f64>>(8)?),
        })
    })
    .map_err(AppError::from)
}

fn load_usage_trend(
    conn: &rusqlite::Connection,
    app_key: &str,
    range: UsageRangePreset,
    start: i64,
    end: i64,
) -> Result<Vec<UsageTrendBucket>, AppError> {
    let mut buckets = empty_usage_trend(range);
    let positions = buckets
        .iter()
        .enumerate()
        .map(|(idx, bucket)| (bucket.key.clone(), idx))
        .collect::<HashMap<_, _>>();

    let hourly = range.uses_hourly_trend(start, end);
    let bucket_expr = match range {
        UsageRangePreset::Today => "strftime('%H', l.created_at, 'unixepoch', 'localtime')",
        UsageRangePreset::Custom(_) if hourly => {
            "strftime('%Y-%m-%d %H', l.created_at, 'unixepoch', 'localtime')"
        }
        UsageRangePreset::SevenDays | UsageRangePreset::ThirtyDays => {
            "date(l.created_at, 'unixepoch', 'localtime')"
        }
        UsageRangePreset::Custom(_) => "date(l.created_at, 'unixepoch', 'localtime')",
    };
    let total_tokens_expr = usage_stats_total_tokens_sql(Some("l"));
    let effective_filter = crate::services::usage_stats::effective_usage_log_filter("l");
    let sql = format!(
        "SELECT
            {bucket_expr} AS bucket,
            COUNT(*),
            COALESCE(SUM({total_tokens_expr}), 0),
            COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0.0),
            COALESCE(SUM(CASE WHEN l.status_code < 200 OR l.status_code >= 300 THEN 1 ELSE 0 END), 0)
         FROM proxy_request_logs l
         WHERE l.app_type = ?1 AND l.created_at >= ?2 AND l.created_at <= ?3
           AND {effective_filter}
         GROUP BY bucket
         ORDER BY bucket"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![app_key, start, end], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, f64>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;

    for row in rows {
        let (key, request_count, total_tokens, total_cost_usd, error_count) = row?;
        let Some(idx) = positions.get(&key).copied() else {
            continue;
        };
        buckets[idx].request_count = non_negative_u64(request_count);
        buckets[idx].total_tokens = non_negative_u64(total_tokens);
        buckets[idx].total_cost_usd = total_cost_usd.max(0.0);
        buckets[idx].error_count = non_negative_u64(error_count);
    }

    Ok(buckets)
}

fn empty_usage_trend(range: UsageRangePreset) -> Vec<UsageTrendBucket> {
    match range {
        UsageRangePreset::Today => (0..24)
            .map(|hour| UsageTrendBucket {
                key: format!("{hour:02}"),
                label: format!("{hour:02}"),
                ..UsageTrendBucket::default()
            })
            .collect(),
        UsageRangePreset::SevenDays | UsageRangePreset::ThirtyDays => {
            let today = Local::now().date_naive();
            let start = today
                .checked_sub_days(Days::new(range.days().saturating_sub(1)))
                .unwrap_or(today);
            (0..range.days())
                .map(|offset| {
                    let date = start.checked_add_days(Days::new(offset)).unwrap_or(start);
                    UsageTrendBucket {
                        key: date.format("%Y-%m-%d").to_string(),
                        label: date.format("%m/%d").to_string(),
                        ..UsageTrendBucket::default()
                    }
                })
                .collect()
        }
        UsageRangePreset::Custom(custom_range) => empty_usage_custom_trend(custom_range),
    }
}

fn empty_usage_custom_trend(range: UsageCustomRange) -> Vec<UsageTrendBucket> {
    let Some(start_datetime) = Local.timestamp_opt(range.start, 0).single() else {
        return Vec::new();
    };
    let Some(end_datetime) = Local.timestamp_opt(range.end, 0).single() else {
        return Vec::new();
    };
    if UsageRangePreset::Custom(range).uses_hourly_trend(range.start, range.end) {
        let same_day = start_datetime.date_naive() == end_datetime.date_naive();
        let mut cursor = range.start - range.start.rem_euclid(3600);
        let end = range.end - range.end.rem_euclid(3600);
        let mut buckets = Vec::new();
        while cursor <= end {
            if let Some(datetime) = Local.timestamp_opt(cursor, 0).single() {
                buckets.push(UsageTrendBucket {
                    key: datetime.format("%Y-%m-%d %H").to_string(),
                    label: if same_day {
                        datetime.format("%H").to_string()
                    } else {
                        datetime.format("%m/%d %H").to_string()
                    },
                    ..UsageTrendBucket::default()
                });
            }
            cursor = cursor.saturating_add(3600);
        }
        return buckets;
    }

    let start = start_datetime.date_naive();
    let days = range.days();
    (0..days)
        .map(|offset| {
            let date = start.checked_add_days(Days::new(offset)).unwrap_or(start);
            UsageTrendBucket {
                key: date.format("%Y-%m-%d").to_string(),
                label: date.format("%m/%d").to_string(),
                ..UsageTrendBucket::default()
            }
        })
        .collect()
}

fn load_usage_top_providers(
    conn: &rusqlite::Connection,
    app_key: &str,
    start: i64,
    end: i64,
) -> Result<Vec<UsageProviderStatsRow>, AppError> {
    let total_tokens_expr = usage_stats_total_tokens_sql(Some("l"));
    let effective_filter = crate::services::usage_stats::effective_usage_log_filter("l");
    let provider_name_expr = usage_provider_name_sql("l", "p");
    let mut stmt = conn.prepare(&format!(
        "SELECT
            CASE WHEN substr(CAST(l.provider_id AS TEXT), {text_probe}, 1) <> ''
                THEN substr(CAST(l.provider_id AS TEXT), 1, {marked_limit}) || '…'
                ELSE substr(CAST(l.provider_id AS TEXT), 1, {text_limit}) END,
            CASE WHEN substr(CAST({provider_name_expr} AS TEXT), {text_probe}, 1) <> ''
                THEN substr(CAST({provider_name_expr} AS TEXT), 1, {marked_limit}) || '…'
                ELSE substr(CAST({provider_name_expr} AS TEXT), 1, {text_limit}) END,
            COUNT(*),
            COALESCE(SUM(CASE WHEN l.status_code >= 200 AND l.status_code < 300 THEN 1 ELSE 0 END), 0),
            COALESCE(SUM({total_tokens_expr}), 0),
            COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0.0),
            AVG(CASE WHEN l.latency_ms > 0 THEN l.latency_ms END)
         FROM proxy_request_logs l
         LEFT JOIN providers p ON p.id = l.provider_id AND p.app_type = l.app_type
         WHERE l.app_type = ?1 AND l.created_at >= ?2 AND l.created_at <= ?3
           AND {effective_filter}
         GROUP BY l.provider_id, p.name
         ORDER BY COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0.0) DESC, COUNT(*) DESC
        LIMIT 8",
        text_limit = USAGE_TEXT_MAX_CHARS,
        text_probe = USAGE_TEXT_MAX_CHARS.saturating_add(1),
        marked_limit = USAGE_TEXT_MAX_CHARS.saturating_sub(1),
    ))?;

    let rows = stmt.query_map(params![app_key, start, end], |row| {
        Ok(UsageProviderStatsRow {
            provider_id: row.get(0)?,
            provider_name: normalize_optional_string(row.get::<_, Option<String>>(1)?),
            request_count: non_negative_u64(row.get::<_, i64>(2)?),
            success_count: non_negative_u64(row.get::<_, i64>(3)?),
            total_tokens: non_negative_u64(row.get::<_, i64>(4)?),
            total_cost_usd: row.get::<_, f64>(5)?.max(0.0),
            avg_latency_ms: optional_average_u64(row.get::<_, Option<f64>>(6)?),
        })
    })?;

    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

fn load_usage_top_models(
    conn: &rusqlite::Connection,
    app_key: &str,
    start: i64,
    end: i64,
) -> Result<Vec<UsageModelStatsRow>, AppError> {
    let total_tokens_expr = usage_stats_total_tokens_sql(Some("l"));
    let effective_filter = crate::services::usage_stats::effective_usage_log_filter("l");
    let mut stmt = conn.prepare(&format!(
        "SELECT
            CASE
                WHEN substr(CAST(COALESCE(NULLIF(TRIM(l.model), ''), 'unknown') AS TEXT), {text_probe}, 1) <> ''
                THEN substr(CAST(COALESCE(NULLIF(TRIM(l.model), ''), 'unknown') AS TEXT), 1, {marked_limit}) || '…'
                ELSE substr(CAST(COALESCE(NULLIF(TRIM(l.model), ''), 'unknown') AS TEXT), 1, {text_limit})
            END AS model_name,
            COUNT(*),
            COALESCE(SUM(CASE WHEN l.status_code >= 200 AND l.status_code < 300 THEN 1 ELSE 0 END), 0),
            COALESCE(SUM({total_tokens_expr}), 0),
            COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0.0),
            AVG(CASE WHEN l.latency_ms > 0 THEN l.latency_ms END)
         FROM proxy_request_logs l
         WHERE l.app_type = ?1 AND l.created_at >= ?2 AND l.created_at <= ?3
           AND {effective_filter}
         GROUP BY COALESCE(NULLIF(TRIM(l.model), ''), 'unknown')
         ORDER BY COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0.0) DESC, COUNT(*) DESC
        LIMIT 8",
        text_limit = USAGE_TEXT_MAX_CHARS,
        text_probe = USAGE_TEXT_MAX_CHARS.saturating_add(1),
        marked_limit = USAGE_TEXT_MAX_CHARS.saturating_sub(1),
    ))?;

    let rows = stmt.query_map(params![app_key, start, end], |row| {
        Ok(UsageModelStatsRow {
            model: row.get(0)?,
            request_count: non_negative_u64(row.get::<_, i64>(1)?),
            success_count: non_negative_u64(row.get::<_, i64>(2)?),
            total_tokens: non_negative_u64(row.get::<_, i64>(3)?),
            total_cost_usd: row.get::<_, f64>(4)?.max(0.0),
            avg_latency_ms: optional_average_u64(row.get::<_, Option<f64>>(5)?),
        })
    })?;

    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

fn load_usage_recent_logs(
    conn: &rusqlite::Connection,
    app_key: &str,
    range: Option<(i64, i64)>,
    limit: u16,
) -> Result<Vec<UsageLogRow>, AppError> {
    Ok(load_usage_log_page(
        conn,
        app_key,
        range,
        None,
        UsageLogPageDirection::Older,
        usize::from(limit),
    )?
    .rows)
}

fn load_usage_log_page(
    conn: &rusqlite::Connection,
    app_key: &str,
    range: Option<(i64, i64)>,
    cursor: Option<&UsageLogCursor>,
    direction: UsageLogPageDirection,
    limit: usize,
) -> Result<UsageLogPage, AppError> {
    let limit = limit.min(USAGE_LOG_PAGE_SIZE);
    let query_limit = limit.saturating_add(1);
    let mut rows = Vec::with_capacity(query_limit);
    match cursor {
        Some(cursor) => {
            let same_timestamp = build_usage_log_page_query(
                app_key,
                range,
                Some(cursor),
                direction,
                UsageLogSeekPart::SameTimestamp,
                query_limit,
            );
            append_usage_log_query_rows(conn, same_timestamp, &mut rows)?;

            let remaining = query_limit.saturating_sub(rows.len());
            if remaining > 0 {
                let cross_timestamp = build_usage_log_page_query(
                    app_key,
                    range,
                    Some(cursor),
                    direction,
                    UsageLogSeekPart::CrossTimestamp,
                    remaining,
                );
                append_usage_log_query_rows(conn, cross_timestamp, &mut rows)?;
            }
        }
        None => {
            let head = build_usage_log_page_query(
                app_key,
                range,
                None,
                direction,
                UsageLogSeekPart::Head,
                query_limit,
            );
            append_usage_log_query_rows(conn, head, &mut rows)?;
        }
    }
    let has_more = rows.len() > limit;
    if has_more {
        rows.truncate(limit);
    }
    if matches!(direction, UsageLogPageDirection::Newer) {
        rows.reverse();
    }
    let next_cursor = match direction {
        UsageLogPageDirection::Older => rows.last(),
        UsageLogPageDirection::Newer => rows.first(),
    }
    .map(UsageLogCursor::from_row);
    Ok(UsageLogPage {
        rows,
        next_cursor,
        has_more,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsageLogSeekPart {
    Head,
    SameTimestamp,
    CrossTimestamp,
}

type UsageLogSqlQuery = (String, Vec<rusqlite::types::Value>);

fn append_usage_log_query_rows(
    conn: &rusqlite::Connection,
    (sql, values): UsageLogSqlQuery,
    rows: &mut Vec<UsageLogRow>,
) -> Result<(), AppError> {
    let mut stmt = conn.prepare(&sql)?;
    let mapped = stmt.query_map(rusqlite::params_from_iter(values), usage_log_row_from_sql)?;
    for row in mapped {
        rows.push(row?);
    }
    Ok(())
}

fn build_usage_log_page_query(
    app_key: &str,
    range: Option<(i64, i64)>,
    cursor: Option<&UsageLogCursor>,
    direction: UsageLogPageDirection,
    part: UsageLogSeekPart,
    limit: usize,
) -> UsageLogSqlQuery {
    let effective_filter = crate::services::usage_stats::effective_usage_log_filter("l");
    let provider_name_expr = usage_provider_name_sql("l", "p");
    let mut filters = vec!["l.app_type = ?".to_string()];
    let mut values = vec![rusqlite::types::Value::Text(app_key.to_string())];

    if let Some((start, end)) = range {
        filters.push("l.created_at >= ? AND l.created_at <= ?".to_string());
        values.push(rusqlite::types::Value::Integer(start));
        values.push(rusqlite::types::Value::Integer(end));
    }
    match (part, cursor) {
        (UsageLogSeekPart::Head, None) => {}
        (UsageLogSeekPart::SameTimestamp, Some(cursor)) => {
            let comparator = match direction {
                UsageLogPageDirection::Older => ">",
                UsageLogPageDirection::Newer => "<",
            };
            filters.push(format!("l.created_at = ? AND l.rowid {comparator} ?"));
            values.push(rusqlite::types::Value::Integer(cursor.created_at));
            values.push(rusqlite::types::Value::Integer(cursor.rowid));
        }
        (UsageLogSeekPart::CrossTimestamp, Some(cursor)) => {
            let comparator = match direction {
                UsageLogPageDirection::Older => "<",
                UsageLogPageDirection::Newer => ">",
            };
            filters.push(format!("l.created_at {comparator} ?"));
            values.push(rusqlite::types::Value::Integer(cursor.created_at));
        }
        _ => unreachable!("usage log seek part must match cursor presence"),
    }
    filters.push(effective_filter);
    let query_limit = limit.min(USAGE_LOG_PAGE_SIZE.saturating_add(1)) as i64;
    values.push(rusqlite::types::Value::Integer(query_limit));

    // SQLite stores rowid as the implicit final field of every ordinary index.
    // These two orders are exact forward/reverse scans of the existing
    // `(app_type, created_at DESC)` index, including arbitrarily large timestamp
    // ties, so paging never needs a temporary sort or a schema change.
    let order = match (part, direction) {
        (UsageLogSeekPart::SameTimestamp, UsageLogPageDirection::Older) => "l.rowid ASC",
        (UsageLogSeekPart::SameTimestamp, UsageLogPageDirection::Newer) => "l.rowid DESC",
        (_, UsageLogPageDirection::Older) => "l.created_at DESC, l.rowid ASC",
        (_, UsageLogPageDirection::Newer) => "l.created_at ASC, l.rowid DESC",
    };

    let sql = format!(
        "SELECT
            substr(CAST(l.request_id AS TEXT), 1, {text_limit}),
            l.created_at,
            substr(CAST(l.app_type AS TEXT), 1, {text_limit}),
            substr(CAST(l.provider_id AS TEXT), 1, {text_limit}),
            substr(CAST({provider_name_expr} AS TEXT), 1, {text_limit}),
            substr(CAST(l.model AS TEXT), 1, {text_limit}),
            substr(CAST(l.request_model AS TEXT), 1, {text_limit}),
            l.status_code,
            l.input_tokens,
            l.output_tokens,
            l.cache_read_tokens,
            l.cache_creation_tokens,
            l.input_token_semantics,
            CAST(l.total_cost_usd AS REAL),
            l.latency_ms,
            l.first_token_ms,
            l.duration_ms,
            substr(CAST(l.session_id AS TEXT), 1, {text_limit}),
            substr(CAST(l.provider_type AS TEXT), 1, {text_limit}),
            l.is_streaming,
            substr(CAST(l.error_message AS TEXT), 1, {error_limit}),
            substr(CAST(l.data_source AS TEXT), 1, {text_limit}),
            CASE
                WHEN l.error_message IS NOT NULL
                 AND substr(CAST(l.error_message AS TEXT), {error_probe}, 1) <> ''
                THEN 1 ELSE 0
            END,
            l.rowid,
            {text_truncation_mask}
         FROM proxy_request_logs l
         LEFT JOIN providers p ON p.id = l.provider_id AND p.app_type = l.app_type
         WHERE {filters}
         ORDER BY {order}
         LIMIT ?",
        filters = filters.join(" AND "),
        error_limit = USAGE_LOG_ERROR_MESSAGE_MAX_CHARS,
        error_probe = USAGE_LOG_ERROR_MESSAGE_MAX_CHARS.saturating_add(1),
        text_limit = USAGE_TEXT_MAX_CHARS,
        text_truncation_mask = usage_log_text_truncation_mask_sql(&provider_name_expr),
    );
    (sql, values)
}

fn load_usage_log_detail(
    conn: &rusqlite::Connection,
    app_key: &str,
    range: Option<(i64, i64)>,
    rowid: i64,
) -> Result<Option<UsageLogRow>, AppError> {
    let effective_filter = crate::services::usage_stats::effective_usage_log_filter("l");
    let provider_name_expr = usage_provider_name_sql("l", "p");
    let mut filters = vec!["l.rowid = ?".to_string(), "l.app_type = ?".to_string()];
    let mut values = vec![
        rusqlite::types::Value::Integer(rowid),
        rusqlite::types::Value::Text(app_key.to_string()),
    ];
    if let Some((start, end)) = range {
        filters.push("l.created_at >= ? AND l.created_at <= ?".to_string());
        values.push(rusqlite::types::Value::Integer(start));
        values.push(rusqlite::types::Value::Integer(end));
    }
    filters.push(effective_filter);

    let sql = format!(
        "SELECT
            substr(CAST(l.request_id AS TEXT), 1, {text_limit}),
            l.created_at,
            substr(CAST(l.app_type AS TEXT), 1, {text_limit}),
            substr(CAST(l.provider_id AS TEXT), 1, {text_limit}),
            substr(CAST({provider_name_expr} AS TEXT), 1, {text_limit}),
            substr(CAST(l.model AS TEXT), 1, {text_limit}),
            substr(CAST(l.request_model AS TEXT), 1, {text_limit}),
            l.status_code,
            l.input_tokens,
            l.output_tokens,
            l.cache_read_tokens,
            l.cache_creation_tokens,
            l.input_token_semantics,
            CAST(l.total_cost_usd AS REAL),
            l.latency_ms,
            l.first_token_ms,
            l.duration_ms,
            substr(CAST(l.session_id AS TEXT), 1, {text_limit}),
            substr(CAST(l.provider_type AS TEXT), 1, {text_limit}),
            l.is_streaming,
            substr(CAST(l.error_message AS TEXT), 1, {error_limit}),
            substr(CAST(l.data_source AS TEXT), 1, {text_limit}),
            CASE
                WHEN l.error_message IS NOT NULL
                 AND substr(CAST(l.error_message AS TEXT), {error_probe}, 1) <> ''
                THEN 1 ELSE 0
            END,
            l.rowid,
            {text_truncation_mask}
         FROM proxy_request_logs l
         LEFT JOIN providers p ON p.id = l.provider_id AND p.app_type = l.app_type
         WHERE {filters}
         LIMIT 1",
        filters = filters.join(" AND "),
        error_limit = USAGE_LOG_ERROR_MESSAGE_MAX_CHARS,
        error_probe = USAGE_LOG_ERROR_MESSAGE_MAX_CHARS.saturating_add(1),
        text_limit = USAGE_TEXT_MAX_CHARS,
        text_truncation_mask = usage_log_text_truncation_mask_sql(&provider_name_expr),
    );
    conn.query_row(
        &sql,
        rusqlite::params_from_iter(values),
        usage_log_row_from_sql,
    )
    .optional()
    .map_err(AppError::from)
}

fn usage_log_row_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<UsageLogRow> {
    Ok(UsageLogRow {
        request_id: row.get(0)?,
        created_at: row.get(1)?,
        app_type: row.get(2)?,
        provider_id: row.get(3)?,
        provider_name: normalize_optional_string(row.get::<_, Option<String>>(4)?),
        model: row.get(5)?,
        request_model: normalize_optional_string(row.get::<_, Option<String>>(6)?),
        status_code: clamp_u16(row.get::<_, i64>(7)?),
        input_tokens: non_negative_u64(row.get::<_, i64>(8)?),
        output_tokens: non_negative_u64(row.get::<_, i64>(9)?),
        cache_read_tokens: non_negative_u64(row.get::<_, i64>(10)?),
        cache_creation_tokens: non_negative_u64(row.get::<_, i64>(11)?),
        input_token_semantics: row.get(12)?,
        total_cost_usd: row.get::<_, f64>(13)?.max(0.0),
        latency_ms: non_negative_u64(row.get::<_, i64>(14)?),
        first_token_ms: row.get::<_, Option<i64>>(15)?.map(non_negative_u64),
        duration_ms: row.get::<_, Option<i64>>(16)?.map(non_negative_u64),
        session_id: normalize_optional_string(row.get::<_, Option<String>>(17)?),
        provider_type: normalize_optional_string(row.get::<_, Option<String>>(18)?),
        is_streaming: row.get::<_, i64>(19)? != 0,
        error_message: normalize_optional_string(row.get::<_, Option<String>>(20)?),
        data_source: normalize_optional_string(row.get::<_, Option<String>>(21)?),
        error_message_truncated: row.get::<_, i64>(22)? != 0,
        cursor_rowid: row.get(23)?,
        text_truncation: row.get::<_, i64>(24)?.clamp(0, i64::from(u16::MAX)) as u16,
    })
}

fn usage_log_text_truncation_mask_sql(provider_name_expr: &str) -> String {
    let text_limit = USAGE_TEXT_MAX_CHARS;
    [
        ("l.request_id", UsageLogTextField::RequestId),
        ("l.app_type", UsageLogTextField::AppType),
        ("l.provider_id", UsageLogTextField::ProviderId),
        (provider_name_expr, UsageLogTextField::ProviderName),
        ("l.model", UsageLogTextField::Model),
        ("l.request_model", UsageLogTextField::RequestModel),
        ("l.session_id", UsageLogTextField::SessionId),
        ("l.provider_type", UsageLogTextField::ProviderType),
        ("l.data_source", UsageLogTextField::DataSource),
    ]
    .into_iter()
    .map(|(expression, field)| {
        format!(
            "CASE WHEN substr(CAST({expression} AS TEXT), {}, 1) <> '' THEN {} ELSE 0 END",
            text_limit.saturating_add(1),
            field.mask()
        )
    })
    .collect::<Vec<_>>()
    .join(" | ")
}

fn load_usage_logs_total(
    conn: &rusqlite::Connection,
    app_key: &str,
    range: Option<(i64, i64)>,
) -> Result<u64, AppError> {
    let effective_filter = crate::services::usage_stats::effective_usage_log_filter("l");
    let count = match range {
        Some((start, end)) => conn.query_row(
            &format!(
                "SELECT COUNT(*)
                 FROM proxy_request_logs l
                 WHERE l.app_type = ?1
                   AND l.created_at >= ?2
                   AND l.created_at <= ?3
                   AND {effective_filter}"
            ),
            params![app_key, start, end],
            |row| row.get::<_, i64>(0),
        )?,
        None => conn.query_row(
            &format!(
                "SELECT COUNT(*)
                 FROM proxy_request_logs l
                 WHERE l.app_type = ?1
                   AND {effective_filter}"
            ),
            params![app_key],
            |row| row.get::<_, i64>(0),
        )?,
    };
    Ok(non_negative_u64(count))
}

fn effective_total_tokens(
    app_type: &str,
    _data_source: Option<&str>,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    input_token_semantics: i64,
) -> u64 {
    let billable_input = crate::services::sql_helpers::fresh_input_tokens(
        app_type,
        input_tokens,
        cache_read_tokens,
        cache_creation_tokens,
        input_token_semantics,
    );

    billable_input
        .saturating_add(output_tokens)
        .saturating_add(cache_read_tokens)
        .saturating_add(cache_creation_tokens)
}

fn usage_stats_total_tokens_sql(alias: Option<&str>) -> String {
    let column = |name: &str| match alias {
        Some(alias) => format!("{alias}.{name}"),
        None => name.to_string(),
    };
    let fresh_input = crate::services::sql_helpers::fresh_input_sql(alias.unwrap_or(""));
    let output = column("output_tokens");

    format!("{fresh_input} + {output}")
}

fn usage_real_total_tokens_sql(alias: Option<&str>) -> String {
    let column = |name: &str| match alias {
        Some(alias) => format!("{alias}.{name}"),
        None => name.to_string(),
    };
    let stats_total = usage_stats_total_tokens_sql(alias);
    let cache_read = column("cache_read_tokens");
    let cache_creation = column("cache_creation_tokens");

    format!("{stats_total} + {cache_read} + {cache_creation}")
}

fn usage_provider_name_sql(log_alias: &str, provider_alias: &str) -> String {
    format!(
        "COALESCE(NULLIF(TRIM({provider_alias}.name), ''), CASE {log_alias}.provider_id \
         WHEN '_session' THEN 'Claude (Session)' \
         WHEN '_codex_session' THEN 'Codex (Session)' \
         WHEN '_gemini_session' THEN 'Gemini (Session)' \
         WHEN '_opencode_session' THEN 'OpenCode (Session)' \
         ELSE {log_alias}.provider_id END)"
    )
}

fn non_negative_u64(value: i64) -> u64 {
    value.max(0) as u64
}

fn clamp_u16(value: i64) -> u16 {
    value.clamp(0, u16::MAX as i64) as u16
}

fn optional_average_u64(value: Option<f64>) -> Option<u64> {
    value
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(|value| value.round() as u64)
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn load_proxy_config() -> Result<Option<crate::proxy::ProxyConfig>, AppError> {
    let state = load_state()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("failed to create async runtime: {e}")))?;

    runtime.block_on(async { state.db.get_proxy_config().await.map(Some) })
}

fn load_proxy_snapshot_from_state(
    state: &AppState,
    app_type: &AppType,
) -> Result<ProxySnapshot, AppError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("failed to create async runtime: {e}")))?;

    runtime.block_on(load_proxy_snapshot_from_state_async(state, app_type))
}

pub(crate) async fn load_proxy_snapshot_from_state_async(
    state: &AppState,
    app_type: &AppType,
) -> Result<ProxySnapshot, AppError> {
    let current_app = app_type.as_str().to_string();
    let config = state.db.get_global_proxy_config_or_default().await?;
    let app_proxy_config = state
        .db
        .get_proxy_config_for_app_or_default(app_type.as_str())
        .await?;
    let configured_listen_port = state.db.get_app_proxy_preferred_port(app_type.as_str())?;
    let runtime_status = state
        .proxy_service
        .get_status_snapshot_for_app(app_type)
        .await;
    let claude_takeover = state
        .db
        .get_proxy_config_for_app_or_default("claude")
        .await?
        .enabled;
    let codex_takeover = state
        .db
        .get_proxy_config_for_app_or_default("codex")
        .await?
        .enabled;
    let gemini_takeover = state
        .db
        .get_proxy_config_for_app_or_default("gemini")
        .await?
        .enabled;

    let current_app_target = proxy_target_snapshot_for_app(&runtime_status, &current_app);
    let provider_health = Arc::new(
        state
            .db
            .list_provider_health_for_app(app_type.as_str())
            .await?
            .into_iter()
            .map(|health| {
                (
                    health.provider_id,
                    ProviderHealthSnapshot {
                        is_healthy: health.is_healthy,
                        consecutive_failures: health.consecutive_failures,
                    },
                )
            })
            .collect(),
    );
    let active_worker_apps = runtime_status
        .active_workers
        .iter()
        .map(|worker| worker.app_type.trim().to_ascii_lowercase())
        .filter(|app| !app.is_empty())
        .collect::<HashSet<_>>();
    let listen_address = if runtime_status.address.trim().is_empty() {
        config.listen_address.clone()
    } else {
        runtime_status.address.clone()
    };
    let listen_port = runtime_status
        .active_workers
        .iter()
        .find(|worker| worker.app_type.eq_ignore_ascii_case(&current_app))
        .map(|worker| worker.port)
        .or_else(|| (runtime_status.port != 0).then_some(runtime_status.port))
        .unwrap_or(configured_listen_port);
    let default_cost_multiplier = state
        .db
        .get_default_cost_multiplier_or_default(app_type.as_str())
        .await
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    Ok(ProxySnapshot {
        enabled: config.proxy_enabled,
        running: runtime_status.running,
        managed_runtime: runtime_status.managed_session_token.is_some()
            || !runtime_status.active_workers.is_empty(),
        active_worker_apps,
        auto_failover_enabled: app_proxy_config.auto_failover_enabled,
        claude_takeover,
        codex_takeover,
        gemini_takeover,
        default_cost_multiplier,
        configured_listen_address: config.listen_address.clone(),
        configured_listen_port,
        listen_address,
        listen_port,
        uptime_seconds: runtime_status.uptime_seconds,
        total_requests: runtime_status.total_requests,
        estimated_input_tokens_total: runtime_status.estimated_input_tokens_total,
        estimated_output_tokens_total: runtime_status.estimated_output_tokens_total,
        success_rate: (runtime_status.total_requests > 0).then_some(runtime_status.success_rate),
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
        provider_health,
    })
}

fn proxy_target_snapshot_for_app(
    runtime_status: &crate::proxy::types::ProxyStatus,
    app_type: &str,
) -> Option<ProxyTargetSnapshot> {
    runtime_status
        .active_targets
        .iter()
        .find(|target| target.app_type.eq_ignore_ascii_case(app_type))
        .map(|target| ProxyTargetSnapshot {
            provider_id: target.provider_id.clone(),
            provider_name: target.provider_name.clone(),
        })
}

fn load_skills_snapshot() -> Result<SkillsSnapshot, AppError> {
    Ok(SkillsSnapshot {
        installed: SkillService::list_installed()?,
        repos: SkillService::list_repos()?,
        sync_method: SkillService::get_sync_method()?,
    })
}

fn load_skills_snapshot_from_state(state: &AppState) -> Result<SkillsSnapshot, AppError> {
    let mut installed = state
        .db
        .get_all_installed_skills()?
        .into_values()
        .collect::<Vec<_>>();
    installed.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    Ok(SkillsSnapshot {
        installed,
        repos: state.db.get_skill_repos()?,
        sync_method: SkillService::get_sync_method()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::Prompt;
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
            let settings = AppSettings {
                opencode_config_dir: Some(path.display().to_string()),
                ..Default::default()
            };
            update_settings(settings).expect("set opencode override dir");
            Self { previous }
        }

        fn with_openclaw_dir(path: &Path) -> Self {
            let previous = get_settings();
            let settings = AppSettings {
                openclaw_config_dir: Some(path.display().to_string()),
                ..Default::default()
            };
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

    fn prompt_row(id: &str, created_at: Option<i64>, updated_at: Option<i64>) -> PromptRow {
        PromptRow {
            id: id.to_string(),
            prompt: Prompt {
                id: id.to_string(),
                name: id.to_string(),
                content: String::new(),
                description: None,
                enabled: false,
                created_at,
                updated_at,
            },
        }
    }

    fn insert_pricing_usage_log(
        conn: &rusqlite::Connection,
        request_id: &str,
        provider_id: &str,
        model: &str,
        request_model: Option<&str>,
        created_at: i64,
    ) -> Result<(), AppError> {
        conn.execute(
            "INSERT INTO proxy_request_logs (
                request_id, provider_id, app_type, model, request_model,
                input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                total_cost_usd, latency_ms, status_code, created_at
             ) VALUES (?1, ?2, 'claude', ?3, ?4, 1000, 2000, 100, 50, '0.1234', 25, 200, ?5)",
            params![request_id, provider_id, model, request_model, created_at],
        )?;
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "test helper mirrors usage log columns"
    )]
    fn insert_usage_log(
        conn: &rusqlite::Connection,
        request_id: &str,
        app_type: &str,
        provider_id: &str,
        model: &str,
        created_at: i64,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_creation_tokens: u64,
    ) -> Result<(), AppError> {
        conn.execute(
            "INSERT INTO proxy_request_logs (
                request_id, provider_id, app_type, model,
                input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                total_cost_usd, latency_ms, status_code, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '0.2500', 40, 200, ?9)",
            params![
                request_id,
                provider_id,
                app_type,
                model,
                input_tokens as i64,
                output_tokens as i64,
                cache_read_tokens as i64,
                cache_creation_tokens as i64,
                created_at
            ],
        )?;
        Ok(())
    }

    fn pricing_row<'a>(snapshot: &'a ModelPricingSnapshot, model_id: &str) -> &'a ModelPricingRow {
        snapshot
            .rows
            .iter()
            .find(|row| row.model_id == model_id)
            .unwrap_or_else(|| panic!("pricing row {model_id} should exist"))
    }

    #[test]
    fn pricing_snapshot_matches_normalized_catalog_model() -> Result<(), AppError> {
        let db = crate::Database::memory()?;
        let now = 1_800_000_000;
        let start = now - 30 * 24 * 60 * 60;
        let conn = db.conn.lock().expect("lock memory db");

        insert_pricing_usage_log(
            &conn,
            "normalized-1",
            "provider-1",
            "openai/gpt-5.4-2026-03-05-high",
            None,
            now - 60,
        )?;

        let snapshot = load_model_pricing_snapshot_from_conn(&conn, "claude", start, now)?;
        let row = pricing_row(&snapshot, "gpt-5.4");

        assert_eq!(row.recent_request_count, 1);
        assert_eq!(row.recent_total_tokens, 3150);
        assert_eq!(row.recent_total_cost_usd, 0.1234);
        assert_eq!(row.last_used_at, Some(now - 60));
        assert_eq!(snapshot.recent_unknown_models, 0);
        Ok(())
    }

    #[test]
    fn pricing_snapshot_counts_distinct_unknown_recent_models() -> Result<(), AppError> {
        let db = crate::Database::memory()?;
        let now = 1_800_000_000;
        let start = now - 30 * 24 * 60 * 60;
        let conn = db.conn.lock().expect("lock memory db");

        insert_pricing_usage_log(
            &conn,
            "unknown-1",
            "provider-1",
            "vendor/unknown-new-model",
            None,
            now - 90,
        )?;
        insert_pricing_usage_log(
            &conn,
            "unknown-2",
            "provider-1",
            "vendor/unknown-new-model",
            None,
            now - 30,
        )?;

        let snapshot = load_model_pricing_snapshot_from_conn(&conn, "claude", start, now)?;

        assert_eq!(snapshot.recent_unknown_models, 1);
        assert_eq!(snapshot.recent_unmatched_total_tokens, 6300);
        assert!((snapshot.recent_unmatched_total_cost_usd - 0.2468).abs() < f64::EPSILON);
        assert!((snapshot.recent_total_cost_usd() - 0.2468).abs() < f64::EPSILON);
        assert_eq!(pricing_row(&snapshot, "gpt-5.4").recent_request_count, 0);
        Ok(())
    }

    #[test]
    fn pricing_snapshot_falls_back_to_request_model_when_response_unknown() -> Result<(), AppError>
    {
        let db = crate::Database::memory()?;
        let now = 1_800_000_000;
        let start = now - 30 * 24 * 60 * 60;
        let conn = db.conn.lock().expect("lock memory db");

        insert_pricing_usage_log(
            &conn,
            "request-source-1",
            "request-provider",
            "response-only-model",
            Some("openai/gpt-5.4-2026-03-05-high"),
            now - 10,
        )?;

        let snapshot = load_model_pricing_snapshot_from_conn(&conn, "claude", start, now)?;
        let row = pricing_row(&snapshot, "gpt-5.4");

        assert_eq!(row.recent_request_count, 1);
        assert_eq!(row.recent_total_tokens, 3150);
        assert_eq!(snapshot.recent_unknown_models, 0);
        Ok(())
    }

    #[test]
    fn pricing_snapshot_prefers_logged_response_model_over_current_provider_config(
    ) -> Result<(), AppError> {
        let db = crate::Database::memory()?;
        let mut provider = Provider::with_id(
            "request-provider".to_string(),
            "Request Provider".to_string(),
            json!({}),
            None,
        );
        provider.meta = Some(ProviderMeta {
            pricing_model_source: Some("request".to_string()),
            ..ProviderMeta::default()
        });
        db.save_provider("claude", &provider)?;

        let now = 1_800_000_000;
        let start = now - 30 * 24 * 60 * 60;
        let conn = db.conn.lock().expect("lock memory db");

        insert_pricing_usage_log(
            &conn,
            "response-source-1",
            "request-provider",
            "gpt-5.4",
            Some("gpt-5.2"),
            now - 10,
        )?;

        let snapshot = load_model_pricing_snapshot_from_conn(&conn, "claude", start, now)?;

        assert_eq!(pricing_row(&snapshot, "gpt-5.4").recent_request_count, 1);
        assert_eq!(pricing_row(&snapshot, "gpt-5.2").recent_request_count, 0);
        assert_eq!(snapshot.recent_unknown_models, 0);
        Ok(())
    }

    #[test]
    fn usage_snapshot_matches_backend_cache_and_total_token_semantics() -> Result<(), AppError> {
        let db = crate::Database::memory()?;
        let now = Local::now().timestamp();
        let start = now - 60 * 60;
        let conn = db.conn.lock().expect("lock memory db");

        insert_usage_log(
            &conn,
            "codex-cache-inclusive",
            "codex",
            "_codex_session",
            "gpt-5.4",
            now - 60,
            1_000,
            200,
            600,
            50,
        )?;

        let summary = load_usage_summary(&conn, "codex", start, now)?;
        assert_eq!(summary.input_tokens, 400);
        assert_eq!(summary.output_tokens, 200);
        assert_eq!(summary.cache_read_tokens, 600);
        assert_eq!(summary.cache_creation_tokens, 50);
        assert_eq!(summary.total_tokens(), 1_250);
        let expected_hit_rate = 600.0 * 100.0 / 1_050.0;
        assert!(
            (summary.cache_hit_rate().expect("cache hit rate") - expected_hit_rate).abs() < 1e-9
        );

        let trends = load_usage_trend(&conn, "codex", UsageRangePreset::Today, start, now)?;
        let active_bucket = trends
            .iter()
            .find(|bucket| bucket.request_count == 1)
            .expect("active trend bucket");
        assert_eq!(active_bucket.total_tokens, 600);

        let providers = load_usage_top_providers(&conn, "codex", start, now)?;
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].provider_id, "_codex_session");
        assert_eq!(
            providers[0].provider_name.as_deref(),
            Some("Codex (Session)")
        );
        assert_eq!(providers[0].total_tokens, 600);

        let models = load_usage_top_models(&conn, "codex", start, now)?;
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model, "gpt-5.4");
        assert_eq!(models[0].total_tokens, 600);

        let recent_logs = load_usage_recent_logs(&conn, "codex", None, 10)?;
        assert_eq!(recent_logs.len(), 1);
        assert_eq!(
            recent_logs[0].provider_name.as_deref(),
            Some("Codex (Session)")
        );
        assert_eq!(recent_logs[0].total_tokens(), 1_250);

        Ok(())
    }

    #[test]
    fn usage_log_total_tokens_respects_each_input_semantics() {
        let row = |input_tokens, input_token_semantics| UsageLogRow {
            app_type: "codex".to_string(),
            input_tokens,
            output_tokens: 10,
            cache_read_tokens: 30,
            cache_creation_tokens: 20,
            input_token_semantics,
            ..UsageLogRow::default()
        };

        assert_eq!(row(100, 0).total_tokens(), 130);
        assert_eq!(row(100, 1).total_tokens(), 110);
        assert_eq!(row(50, 2).total_tokens(), 110);
    }

    #[test]
    fn pricing_snapshot_normalizes_mixed_input_semantics_before_grouping() -> Result<(), AppError> {
        let db = crate::Database::memory()?;
        let now = 1_800_000_000;
        let start = now - 30 * 24 * 60 * 60;
        let conn = db.conn.lock().expect("lock memory db");

        conn.execute_batch(
            "INSERT OR REPLACE INTO model_pricing (
                model_id, display_name, input_cost_per_million,
                output_cost_per_million, cache_read_cost_per_million,
                cache_creation_cost_per_million
             ) VALUES
                ('semantic-response', 'Semantic Response', '1', '0', '0', '0'),
                ('semantic-request', 'Semantic Request', '1.0625', '0', '0', '0');",
        )?;

        for (request_id, input_tokens, semantics, total_cost) in [
            ("legacy", 100, 0, "0.00017"),
            ("total", 100, 1, "0"),
            ("fresh", 50, 2, "0"),
        ] {
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model,
                    input_tokens, output_tokens, cache_read_tokens,
                    cache_creation_tokens, input_token_semantics,
                    total_cost_usd, latency_ms, status_code, created_at
                 ) VALUES (
                    ?1, 'provider', 'codex', 'semantic-response', 'semantic-request',
                    ?2, 10, 30, 20, ?3, ?4, 1, 200, ?5
                 )",
                params![request_id, input_tokens, semantics, total_cost, now - 1],
            )?;
        }

        let snapshot = load_model_pricing_snapshot_from_conn(&conn, "codex", start, now)?;

        assert_eq!(
            pricing_row(&snapshot, "semantic-response").recent_request_count,
            3
        );
        assert_eq!(
            pricing_row(&snapshot, "semantic-response").recent_total_tokens,
            350
        );
        assert_eq!(
            pricing_row(&snapshot, "semantic-request").recent_request_count,
            0
        );
        Ok(())
    }

    #[test]
    fn usage_custom_range_filters_recent_logs_and_total() -> Result<(), AppError> {
        let db = crate::Database::memory()?;
        let now = Local::now().timestamp();
        let start = now - 60 * 60;
        let conn = db.conn.lock().expect("lock memory db");

        insert_usage_log(
            &conn,
            "codex-inside-range",
            "codex",
            "_codex_session",
            "gpt-5.4",
            now - 60,
            100,
            20,
            0,
            0,
        )?;
        insert_usage_log(
            &conn,
            "codex-outside-range",
            "codex",
            "_codex_session",
            "gpt-5.4",
            now - 2 * 60 * 60,
            100,
            20,
            0,
            0,
        )?;

        let range = Some((start, now));
        let recent_logs = load_usage_recent_logs(&conn, "codex", range, 10)?;
        assert_eq!(recent_logs.len(), 1);
        assert_eq!(recent_logs[0].request_id, "codex-inside-range");
        assert_eq!(load_usage_logs_total(&conn, "codex", range)?, 1);
        assert_eq!(load_usage_logs_total(&conn, "codex", None)?, 2);

        Ok(())
    }

    #[test]
    fn usage_log_head_exposes_a_next_page_before_exact_count_arrives() {
        let mut snapshot = UsageSnapshot::default();
        let rows = (0..USAGE_LOG_PAGE_SIZE)
            .map(|index| UsageLogRow {
                request_id: format!("request-{index}"),
                ..UsageLogRow::default()
            })
            .collect::<Vec<_>>();

        snapshot.merge_log_head(
            UsageRangePreset::SevenDays,
            UsageLogPage {
                rows,
                has_more: true,
                ..UsageLogPage::default()
            },
        );

        assert_eq!(
            snapshot.recent_logs_for(UsageRangePreset::Today).len(),
            USAGE_LOG_PAGE_SIZE
        );
        assert_eq!(
            snapshot.logs_total_for(UsageRangePreset::ThirtyDays),
            USAGE_LOG_PAGE_SIZE as u64 + 1
        );
    }

    #[test]
    fn usage_log_head_stays_scoped_to_its_custom_range() {
        let range = UsageCustomRange { start: 10, end: 20 };
        let mut snapshot = UsageSnapshot::default();
        snapshot.merge_log_head(
            UsageRangePreset::Custom(range),
            UsageLogPage {
                rows: vec![UsageLogRow {
                    request_id: "custom-request".to_string(),
                    ..UsageLogRow::default()
                }],
                ..UsageLogPage::default()
            },
        );

        assert_eq!(
            snapshot
                .recent_logs_for(UsageRangePreset::Custom(range))
                .first()
                .map(|row| row.request_id.as_str()),
            Some("custom-request")
        );
        assert!(snapshot
            .recent_logs_for(UsageRangePreset::Custom(UsageCustomRange {
                start: 30,
                end: 40,
            }))
            .is_empty());
    }

    #[test]
    fn usage_log_keyset_pages_are_stable_across_equal_timestamps() -> Result<(), AppError> {
        let db = crate::Database::memory()?;
        let conn = db.conn.lock().expect("lock memory db");
        for (request_id, created_at) in [("c", 100), ("b", 100), ("a", 100), ("z", 99)] {
            insert_usage_log(
                &conn,
                request_id,
                "codex",
                "_codex_session",
                "gpt-5.4",
                created_at,
                10,
                5,
                0,
                0,
            )?;
        }

        let first =
            load_usage_log_page(&conn, "codex", None, None, UsageLogPageDirection::Older, 2)?;
        assert_eq!(
            first
                .rows
                .iter()
                .map(|row| row.request_id.as_str())
                .collect::<Vec<_>>(),
            vec!["c", "b"]
        );
        assert!(first.has_more);

        let second = load_usage_log_page(
            &conn,
            "codex",
            None,
            first.next_cursor.as_ref(),
            UsageLogPageDirection::Older,
            2,
        )?;
        assert_eq!(
            second
                .rows
                .iter()
                .map(|row| row.request_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "z"]
        );
        assert!(!second.has_more);
        Ok(())
    }

    #[test]
    fn usage_log_keyset_walks_large_timestamp_ties_in_both_directions() -> Result<(), AppError> {
        const ROWS: usize = 1_037;
        const PAGE_SIZE: usize = 37;

        let db = crate::Database::memory()?;
        let conn = db.conn.lock().expect("lock memory db");
        for index in 0..ROWS {
            insert_usage_log(
                &conn,
                &format!("tie-{index:04}"),
                "codex",
                "_codex_session",
                "gpt-5.4",
                100,
                10,
                5,
                0,
                0,
            )?;
        }

        let mut older_pages = Vec::new();
        let mut cursor = None;
        loop {
            let page = load_usage_log_page(
                &conn,
                "codex",
                None,
                cursor.as_ref(),
                UsageLogPageDirection::Older,
                PAGE_SIZE,
            )?;
            assert!(!page.rows.is_empty());
            older_pages.push(
                page.rows
                    .iter()
                    .map(|row| row.cursor_rowid)
                    .collect::<Vec<_>>(),
            );
            if !page.has_more {
                break;
            }
            cursor = page.next_cursor;
        }

        let older_rowids = older_pages.iter().flatten().copied().collect::<Vec<_>>();
        assert_eq!(older_rowids.len(), ROWS);
        assert!(older_rowids.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(
            older_rowids.iter().copied().collect::<HashSet<_>>().len(),
            ROWS,
        );

        for current_page in (1..older_pages.len()).rev() {
            let first_rowid = older_pages[current_page][0];
            let page = load_usage_log_page(
                &conn,
                "codex",
                None,
                Some(&UsageLogCursor {
                    created_at: 100,
                    rowid: first_rowid,
                }),
                UsageLogPageDirection::Newer,
                PAGE_SIZE,
            )?;
            assert_eq!(page.rows.len(), PAGE_SIZE);
            assert_eq!(
                page.rows
                    .iter()
                    .map(|row| row.cursor_rowid)
                    .collect::<Vec<_>>(),
                older_pages[current_page - 1],
            );
        }
        Ok(())
    }

    #[test]
    fn usage_log_keyset_plan_uses_existing_index_without_a_temp_sort() -> Result<(), AppError> {
        let db = crate::Database::memory()?;
        let conn = db.conn.lock().expect("lock memory db");
        insert_usage_log(
            &conn,
            "plan-row",
            "codex",
            "_codex_session",
            "gpt-5.4",
            100,
            10,
            5,
            0,
            0,
        )?;

        for direction in [UsageLogPageDirection::Older, UsageLogPageDirection::Newer] {
            let cursor = UsageLogCursor {
                created_at: 100,
                rowid: 1,
            };
            for part in [
                UsageLogSeekPart::SameTimestamp,
                UsageLogSeekPart::CrossTimestamp,
            ] {
                let (sql, values) = build_usage_log_page_query(
                    "codex",
                    None,
                    Some(&cursor),
                    direction,
                    part,
                    USAGE_LOG_PAGE_SIZE + 1,
                );
                assert!(
                    !sql.contains("l.created_at < ? OR") && !sql.contains("l.created_at > ? OR"),
                    "keyset seek regressed to an OR predicate: {sql}",
                );
                let mut statement = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
                let details = statement
                    .query_map(rusqlite::params_from_iter(values), |row| {
                        row.get::<_, String>(3)
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                let plan = details.join("\n");
                assert!(
                    plan.contains("idx_request_logs_app_created_at"),
                    "unexpected {direction:?}/{part:?} plan:\n{plan}",
                );
                assert!(
                    !plan.to_ascii_uppercase().contains("TEMP B-TREE"),
                    "temporary sort in {direction:?}/{part:?} plan:\n{plan}",
                );
            }
        }

        let (_, values) = build_usage_log_page_query(
            "codex",
            None,
            None,
            UsageLogPageDirection::Older,
            UsageLogSeekPart::Head,
            usize::MAX,
        );
        assert_eq!(
            values.last(),
            Some(&rusqlite::types::Value::Integer(
                USAGE_LOG_PAGE_SIZE.saturating_add(1) as i64,
            )),
        );
        Ok(())
    }

    #[test]
    fn usage_log_queries_cap_error_text_before_materializing_rows() -> Result<(), AppError> {
        let db = crate::Database::memory()?;
        let conn = db.conn.lock().expect("lock memory db");
        insert_usage_log(
            &conn,
            "long-error",
            "codex",
            "_codex_session",
            "gpt-5.4",
            100,
            10,
            5,
            0,
            0,
        )?;
        conn.execute(
            "UPDATE proxy_request_logs SET error_message = ?1 WHERE request_id = 'long-error'",
            params!["界".repeat(5_000)],
        )?;

        let (list_sql, _) = build_usage_log_page_query(
            "codex",
            None,
            None,
            UsageLogPageDirection::Older,
            UsageLogSeekPart::Head,
            101,
        );
        assert!(list_sql.contains("substr(CAST(l.error_message AS TEXT), 1, 4096)"));
        assert!(list_sql.contains("substr(CAST(l.error_message AS TEXT), 4097, 1) <> ''"));

        let page = load_usage_log_page(
            &conn,
            "codex",
            None,
            None,
            UsageLogPageDirection::Older,
            100,
        )?;
        let row = page.rows.first().expect("list row");
        assert_eq!(
            row.error_message
                .as_deref()
                .map(|value| value.chars().count()),
            Some(USAGE_LOG_ERROR_MESSAGE_MAX_CHARS),
        );
        assert!(row.error_message_truncated);

        let rowid = row.cursor_rowid;
        let detail = load_usage_log_detail(&conn, "codex", None, rowid)?.expect("detail row");
        assert_eq!(
            detail
                .error_message
                .as_deref()
                .map(|value| value.chars().count()),
            Some(USAGE_LOG_ERROR_MESSAGE_MAX_CHARS),
        );
        assert!(detail.error_message_truncated);

        conn.execute(
            "UPDATE proxy_request_logs SET error_message = ?1 WHERE request_id = 'long-error'",
            params!["x".repeat(USAGE_LOG_ERROR_MESSAGE_MAX_CHARS)],
        )?;
        let exact =
            load_usage_log_detail(&conn, "codex", None, rowid)?.expect("exact-limit detail row");
        assert!(!exact.error_message_truncated);

        conn.execute(
            "UPDATE proxy_request_logs SET error_message = ?1 WHERE rowid = ?2",
            params!["界".repeat(USAGE_LOG_ERROR_MESSAGE_MAX_CHARS), rowid],
        )?;
        let exact_multibyte = load_usage_log_detail(&conn, "codex", None, rowid)?
            .expect("exact multibyte detail row");
        assert_eq!(
            exact_multibyte
                .error_message
                .as_deref()
                .map(|value| value.chars().count()),
            Some(USAGE_LOG_ERROR_MESSAGE_MAX_CHARS),
        );
        assert!(!exact_multibyte.error_message_truncated);
        Ok(())
    }

    #[test]
    fn usage_text_fields_are_bounded_before_row_materialization_and_rowid_stays_unique(
    ) -> Result<(), AppError> {
        let db = crate::Database::memory()?;
        let conn = db.conn.lock().expect("lock memory db");
        for (request_id, model) in [("seed-a", "model-a"), ("seed-b", "model-b")] {
            insert_usage_log(
                &conn, request_id, "codex", "provider", model, 100, 10, 5, 0, 0,
            )?;
        }
        let mut rowid_stmt = conn.prepare("SELECT rowid FROM proxy_request_logs ORDER BY rowid")?;
        let rowids = rowid_stmt
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(rowid_stmt);
        let common = "界".repeat(USAGE_TEXT_MAX_CHARS + 40);
        for (index, rowid) in rowids.iter().copied().enumerate() {
            conn.execute(
                "UPDATE proxy_request_logs
                 SET request_id = ?1,
                     provider_id = ?2,
                     model = ?3,
                     request_model = ?4,
                     session_id = ?5,
                     provider_type = ?6,
                     data_source = ?7,
                     status_code = ?8
                 WHERE rowid = ?9",
                params![
                    format!("{common}-{index}"),
                    &common,
                    &common,
                    &common,
                    &common,
                    &common,
                    &common,
                    200 + index as i64,
                    rowid,
                ],
            )?;
        }

        let page = load_usage_log_page(
            &conn,
            "codex",
            None,
            None,
            UsageLogPageDirection::Older,
            100,
        )?;
        assert_eq!(page.rows.len(), 2);
        assert_ne!(page.rows[0].cursor_rowid, page.rows[1].cursor_rowid);
        assert_eq!(page.rows[0].request_id, page.rows[1].request_id);
        for row in &page.rows {
            for value in [
                row.request_id.as_str(),
                row.provider_id.as_str(),
                row.provider_name
                    .as_deref()
                    .expect("provider name fallback"),
                row.model.as_str(),
                row.request_model.as_deref().expect("request model"),
                row.session_id.as_deref().expect("session id"),
                row.provider_type.as_deref().expect("provider type"),
                row.data_source.as_deref().expect("data source"),
            ] {
                assert_eq!(value.chars().count(), USAGE_TEXT_MAX_CHARS);
            }
            for field in [
                UsageLogTextField::RequestId,
                UsageLogTextField::ProviderId,
                UsageLogTextField::ProviderName,
                UsageLogTextField::Model,
                UsageLogTextField::RequestModel,
                UsageLogTextField::SessionId,
                UsageLogTextField::ProviderType,
                UsageLogTextField::DataSource,
            ] {
                assert!(row.text_was_truncated(field));
            }
        }

        let first =
            load_usage_log_detail(&conn, "codex", None, rowids[0])?.expect("first detail by rowid");
        let second = load_usage_log_detail(&conn, "codex", None, rowids[1])?
            .expect("second detail by rowid");
        assert_eq!(first.cursor_rowid, rowids[0]);
        assert_eq!(second.cursor_rowid, rowids[1]);
        assert_eq!(first.status_code, 200);
        assert_eq!(second.status_code, 201);

        let providers = load_usage_top_providers(&conn, "codex", 0, 200)?;
        assert_eq!(
            providers[0].provider_id.chars().count(),
            USAGE_TEXT_MAX_CHARS
        );
        assert!(providers[0].provider_id.ends_with('…'));
        assert!(providers[0]
            .provider_name
            .as_deref()
            .is_some_and(|name| name.ends_with('…')));
        let models = load_usage_top_models(&conn, "codex", 0, 200)?;
        assert_eq!(models[0].model.chars().count(), USAGE_TEXT_MAX_CHARS);
        assert!(models[0].model.ends_with('…'));
        Ok(())
    }

    #[test]
    fn usage_log_detail_refreshes_one_existing_request_without_paging() -> Result<(), AppError> {
        let db = crate::Database::memory()?;
        let conn = db.conn.lock().expect("lock memory db");
        insert_usage_log(
            &conn,
            "detail-request",
            "codex",
            "_codex_session",
            "gpt-old",
            100,
            10,
            5,
            0,
            0,
        )?;

        let rowid = conn.query_row(
            "SELECT rowid FROM proxy_request_logs WHERE request_id = ?1",
            params!["detail-request"],
            |row| row.get::<_, i64>(0),
        )?;
        let original = load_usage_log_detail(&conn, "codex", None, rowid)?.expect("detail exists");
        assert_eq!(original.model, "gpt-old");

        conn.execute(
            "UPDATE proxy_request_logs
             SET model = 'gpt-new', total_cost_usd = '1.5000', error_message = 'updated'
             WHERE request_id = ?1",
            params!["detail-request"],
        )?;
        let refreshed =
            load_usage_log_detail(&conn, "codex", None, rowid)?.expect("updated detail exists");
        assert_eq!(refreshed.model, "gpt-new");
        assert_eq!(refreshed.total_cost_usd, 1.5);
        assert_eq!(refreshed.error_message.as_deref(), Some("updated"));

        assert!(load_usage_log_detail(&conn, "claude", None, rowid)?.is_none());
        assert!(load_usage_log_detail(&conn, "codex", Some((101, 200)), rowid)?.is_none());
        Ok(())
    }

    #[test]
    fn parse_usage_custom_range_accepts_common_separators() {
        for input in [
            "2026-06-01..2026-06-05",
            "2026-06-01 - 2026-06-05",
            "2026-06-01 to 2026-06-05",
            "2026-06-01,2026-06-05",
            "2026-06-01，2026-06-05",
            "2026-06-01 2026-06-05",
        ] {
            let range = parse_usage_custom_range(input).expect("range should parse");
            assert_eq!(range.label(), "2026-06-01..2026-06-05");
        }
    }

    #[test]
    fn parse_usage_custom_range_rejects_invalid_ranges() {
        for input in [
            "",
            "2026-06-01",
            "2026/06/01..2026/06/05",
            "2026-06-05..2026-06-01",
            "2010-01-01..2025-01-01",
        ] {
            assert!(
                parse_usage_custom_range(input).is_err(),
                "{input} should be rejected"
            );
        }
    }

    #[test]
    fn prompt_rows_sort_by_stable_created_time_not_updated_time() {
        let mut rows = vec![
            prompt_row("first", Some(1), Some(300)),
            prompt_row("second", Some(2), Some(200)),
            prompt_row("third", Some(3), Some(100)),
        ];

        sort_prompt_rows(&mut rows);
        let initial_order = rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>();
        assert_eq!(initial_order, vec!["first", "second", "third"]);

        rows[0].prompt.updated_at = Some(1);
        rows[1].prompt.updated_at = Some(999);
        rows[2].prompt.updated_at = Some(500);

        sort_prompt_rows(&mut rows);
        let refreshed_order = rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>();
        assert_eq!(refreshed_order, vec!["first", "second", "third"]);
    }

    #[test]
    #[serial]
    fn load_proxy_snapshot_reads_app_auto_failover_state() {
        let _guard = lock_test_home_and_settings();
        let temp = tempdir().expect("create tempdir");
        let _home = HomeGuard::set(temp.path());

        let state = load_state().expect("load state");
        let provider = Provider::with_id(
            "queued".to_string(),
            "Queued".to_string(),
            json!({"env": {"ANTHROPIC_BASE_URL": "https://queued.example"}}),
            None,
        );
        state
            .db
            .save_provider("claude", &provider)
            .expect("save queued provider");
        state
            .db
            .add_to_failover_queue("claude", &provider.id)
            .expect("queue provider");
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
            config.enabled = true;
            config.auto_failover_enabled = true;
            state
                .db
                .update_proxy_config_for_app(config)
                .await
                .expect("persist claude app proxy config");
            state
                .db
                .update_provider_health_with_threshold(
                    &provider.id,
                    "claude",
                    false,
                    Some("upstream timeout".to_string()),
                    4,
                )
                .await
                .expect("record provider health");
        });

        let snapshot =
            load_proxy_snapshot_from_state(&state, &AppType::Claude).expect("load proxy snapshot");
        assert!(snapshot.auto_failover_enabled);
        assert_eq!(
            snapshot.provider_health.get(&provider.id),
            Some(&ProviderHealthSnapshot {
                is_healthy: true,
                consecutive_failures: 1,
            })
        );
    }

    #[test]
    fn proxy_target_snapshot_keeps_provider_id_and_filters_by_app() {
        let status = crate::proxy::types::ProxyStatus {
            active_targets: vec![
                crate::proxy::types::ActiveTarget {
                    app_type: "codex".to_string(),
                    provider_name: "Codex Target".to_string(),
                    provider_id: "codex-target".to_string(),
                },
                crate::proxy::types::ActiveTarget {
                    app_type: "Claude".to_string(),
                    provider_name: "Claude Target".to_string(),
                    provider_id: "claude-target".to_string(),
                },
            ],
            ..crate::proxy::types::ProxyStatus::default()
        };

        let target = proxy_target_snapshot_for_app(&status, "claude")
            .expect("matching active target snapshot");
        assert_eq!(target.provider_id, "claude-target");
        assert_eq!(target.provider_name, "Claude Target");
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
        assert!(snapshot.live_ids.contains("in-config"));
    }

    #[test]
    #[serial]
    fn existing_provider_ids_includes_opencode_live_only_ids() {
        let _guard = lock_test_home_and_settings();
        let temp = tempdir().expect("create tempdir");
        let opencode_dir = temp.path().join("opencode");
        std::fs::create_dir_all(&opencode_dir).expect("create opencode dir");
        let _home = HomeGuard::set(temp.path());
        let _settings = SettingsGuard::with_opencode_dir(&opencode_dir);

        crate::opencode_config::set_provider(
            "live-only",
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

        let data = UiData::load(&AppType::OpenCode).expect("load opencode data");
        let ids = data.existing_provider_ids();
        assert!(ids.iter().any(|id| id == "saved-only"));
        assert!(ids.iter().any(|id| id == "live-only"));
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

        let managed_worker_active = ProxySnapshot {
            running: true,
            managed_runtime: true,
            active_worker_apps: HashSet::from([AppType::Claude.as_str().to_string()]),
            claude_takeover: false,
            ..ProxySnapshot::default()
        };
        assert_eq!(
            managed_worker_active.routes_current_app_through_proxy(&AppType::Claude),
            Some(false)
        );
        assert_eq!(
            managed_worker_active.routes_current_app_through_proxy(&AppType::Codex),
            Some(false)
        );

        let worker_for_another_app = ProxySnapshot {
            running: true,
            managed_runtime: true,
            active_worker_apps: HashSet::from([AppType::Codex.as_str().to_string()]),
            claude_takeover: true,
            ..ProxySnapshot::default()
        };
        assert_eq!(
            worker_for_another_app.routes_current_app_through_proxy(&AppType::Claude),
            Some(false)
        );

        let stopped = ProxySnapshot {
            running: false,
            managed_runtime: true,
            claude_takeover: true,
            active_worker_apps: HashSet::from([AppType::Claude.as_str().to_string()]),
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
                provider_id: "claude-test".to_string(),
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
    fn load_fast_snapshot_openclaw_projects_live_only_provider_without_persisting() {
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

        let state = load_state().expect("initialize database");
        assert!(
            ProviderService::list(&state, AppType::OpenClaw)
                .expect("list providers before snapshot")
                .is_empty(),
            "precondition: live-only provider should not be persisted before snapshot load"
        );
        drop(state);

        let snapshot_state = load_snapshot_state().expect("load readonly snapshot state");
        let snapshot = UiData::load_fast_snapshot_from_state(&snapshot_state, &AppType::OpenClaw)
            .expect("load OpenClaw snapshot rows");
        let row = snapshot
            .providers
            .rows
            .iter()
            .find(|row| row.id == "live-only")
            .expect("live-only provider should appear in snapshot rows");
        assert!(row.is_in_config);
        assert!(!row.is_saved);
        assert_eq!(provider_display_name(&AppType::OpenClaw, row), "live-only");
        assert_eq!(row.primary_model_id.as_deref(), Some("openclaw-live-model"));

        drop(snapshot_state);
        let reloaded_state = load_state().expect("reload state after snapshot load");
        assert!(
            ProviderService::list(&reloaded_state, AppType::OpenClaw)
                .expect("list providers after snapshot")
                .is_empty(),
            "snapshot-only OpenClaw load must not persist live-only providers"
        );
    }

    #[test]
    #[serial]
    fn load_fast_snapshot_does_not_clear_invalid_settings_current_provider() {
        let _guard = lock_test_home_and_settings();
        let temp = tempdir().expect("create tempdir");
        let _home = HomeGuard::set(temp.path());
        crate::settings::set_current_provider(&AppType::Claude, Some("missing-local-current"))
            .expect("seed invalid local current provider");

        let state = load_state().expect("initialize database");
        drop(state);

        let snapshot_state = load_snapshot_state().expect("load readonly snapshot state");
        let snapshot = UiData::load_fast_snapshot_from_state(&snapshot_state, &AppType::Claude)
            .expect("load Claude snapshot rows");

        assert_eq!(snapshot.providers.current_id, "");
        assert_eq!(
            crate::settings::get_current_provider(&AppType::Claude).as_deref(),
            Some("missing-local-current"),
            "snapshot provider load must not repair or clear local settings"
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
