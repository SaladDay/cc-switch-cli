use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::cli::tui::data::{
    ProviderRuntimeSnapshot, ProxySnapshot, QuotaSnapshotGeneration, QuotaTarget, UiDataReloadToken,
};
use crate::provider::Provider;
use crate::services::{EndpointLatency, HealthStatus, StreamCheckResult, SyncDecision};

use super::super::form::ProviderAddField;

const KNOWN_COMPAT_SUFFIXES: &[&str] = &[
    "/api/claudecode",
    "/api/anthropic",
    "/apps/anthropic",
    "/api/coding",
    "/claudecode",
    "/anthropic",
    "/step_plan",
    "/coding",
    "/claude",
];

pub(crate) fn next_model_fetch_request_id() -> u64 {
    static NEXT_MODEL_FETCH_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_MODEL_FETCH_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

pub(crate) enum SpeedtestMsg {
    Finished {
        url: String,
        result: Result<Vec<EndpointLatency>, String>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct StreamCheckReq {
    pub(crate) app_type: AppType,
    pub(crate) provider_id: String,
    pub(crate) provider_name: String,
    pub(crate) provider: Provider,
}

pub(crate) enum StreamCheckMsg {
    Finished {
        req: StreamCheckReq,
        result: Result<StreamCheckResult, String>,
    },
}

pub(crate) enum LocalEnvReq {
    Refresh { generation: u64 },
    Shutdown,
}

pub(crate) enum LocalEnvMsg {
    ToolFinished {
        generation: u64,
        result: crate::services::local_env_check::ToolCheckResult,
    },
    BatchFinished {
        generation: u64,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum SessionReq {
    Refresh {
        request_id: u64,
        scope_epoch: u64,
        provider_id: String,
        /// Manual `r` (and delete-recovery) rebuilds from provider sources and
        /// ignores cached `(mtime, size)` metadata. False opens the persisted
        /// manifest only, falling back to one bootstrap build when none exists.
        force: bool,
    },
    LoadPage {
        request_id: u64,
        token: crate::cli::tui::app::SessionPageToken,
        page: usize,
        reader: crate::session_manager::paged_manifest::ManifestReader,
    },
    LocateManifest {
        request_id: u64,
        scope_epoch: u64,
        source: crate::cli::tui::app::SessionPageSource,
        scope: String,
        generation: String,
        fallback_absolute: usize,
        anchor: Option<crate::cli::tui::app::SessionRowIdentity>,
        reader: crate::session_manager::paged_manifest::ManifestReader,
    },
    LoadMessages {
        request_id: u64,
        key: String,
        provider_id: String,
        source_path: String,
    },
    LoadMessagePage {
        request_id: u64,
        key: String,
        transcript_generation: String,
        page: usize,
        /// Page that owns the current user selection. If `page` was only a
        /// speculative prefetch and its generation is stale, refresh this page
        /// instead so a background revision change cannot move the viewport.
        refresh_page: usize,
        /// Stable message identity that owned the selection when a speculative
        /// request began. Refresh follows it across insertion/reordering.
        refresh_message_key: Option<String>,
        reader: crate::session_manager::transcript::TranscriptReader,
    },
    /// Invalidate the current bounded detail read without entering the ordered
    /// delete/control lane.
    CancelMessages,
    Delete {
        request_id: u64,
        key: String,
        provider_id: String,
        session_id: String,
        source_path: String,
    },
    Search {
        request_id: u64,
        scope_epoch: u64,
        view: crate::session_manager::project_scope::SessionViewSpec,
        base: crate::cli::tui::app::SessionPageToken,
        base_reader: crate::session_manager::paged_manifest::ManifestReader,
        query_namespace: crate::session_manager::paged_manifest::QueryManifestNamespace,
    },
    /// Stop the current deep search without affecting refresh, message-load, or
    /// delete work. Query edits and route/scope changes send this immediately;
    /// the search dispatcher advances its generation so provider loops can
    /// cooperatively stop disk and CPU work.
    CancelSearch,
    ProjectCatalog {
        request_id: u64,
        scope_epoch: u64,
        base: crate::cli::tui::app::SessionPageToken,
        base_reader: crate::session_manager::paged_manifest::ManifestReader,
    },
    CancelProjectCatalog,
    ProjectFilter {
        request_id: u64,
        scope_epoch: u64,
        scope: String,
        base_generation: String,
        query: String,
        catalog: std::sync::Arc<crate::session_manager::project_scope::SessionProjectCatalog>,
        project_offset: usize,
        fixed_matches: Vec<usize>,
        trailing_matches: Vec<usize>,
    },
    CancelProjectFilter,
}

pub(crate) enum LoadedMessagePage {
    Page(crate::session_manager::transcript::TranscriptPage),
    Refreshed(Box<RefreshedMessagePages>),
}

pub(crate) struct RefreshedMessagePages {
    pub(crate) reader: crate::session_manager::transcript::TranscriptReader,
    pub(crate) active_page: crate::session_manager::transcript::TranscriptPage,
    pub(crate) requested_page: Option<crate::session_manager::transcript::TranscriptPage>,
}

pub(crate) enum SessionMsg {
    /// Fixed-cost scope open: at most one immutable 100-row manifest page.
    ScopeOpened {
        request_id: u64,
        scope_epoch: u64,
        scope: String,
        /// True when this cached open completes the request. False keeps the
        /// scan active while the same worker revalidates provider sources.
        complete: bool,
        result: Result<
            Option<(
                crate::session_manager::paged_manifest::ManifestReader,
                crate::session_manager::paged_manifest::ManifestPage,
            )>,
            String,
        >,
    },
    ScanProvisional {
        request_id: u64,
        scope_epoch: u64,
        scope: String,
        rows: Vec<crate::session_manager::SessionMeta>,
    },
    /// Authoritative rebuild publication. It contains only page zero; selection
    /// reconciliation reads bounded pages on the worker before switching.
    ManifestPublished {
        request_id: u64,
        scope_epoch: u64,
        scope: String,
        result: Result<
            (
                crate::session_manager::paged_manifest::PublishedManifest,
                crate::session_manager::paged_manifest::ManifestReader,
            ),
            String,
        >,
    },
    CostOverlayReady {
        cost_seq: u64,
        page_token: crate::cli::tui::app::SessionPageToken,
        page_index: usize,
        identities: Vec<crate::cli::tui::app::SessionRowIdentity>,
        overlays: std::collections::HashMap<
            crate::services::session_cost::SessionCostIdentity,
            crate::session_manager::SessionUsageSummary,
        >,
    },
    PageLoaded {
        request_id: u64,
        token: crate::cli::tui::app::SessionPageToken,
        page: usize,
        result: Result<crate::session_manager::paged_manifest::ManifestPage, String>,
    },
    ManifestLocated {
        request_id: u64,
        scope_epoch: u64,
        generation: String,
        result: Result<
            (
                crate::session_manager::paged_manifest::ManifestReader,
                crate::session_manager::paged_manifest::ManifestPage,
                usize,
            ),
            String,
        >,
    },
    MessagesLoaded {
        request_id: u64,
        key: String,
        result: Result<
            (
                crate::session_manager::transcript::TranscriptReader,
                crate::session_manager::transcript::TranscriptPage,
            ),
            String,
        >,
    },
    MessagePageLoaded {
        request_id: u64,
        key: String,
        transcript_generation: String,
        page: usize,
        result: Result<LoadedMessagePage, String>,
    },
    DeleteFinished {
        request_id: u64,
        key: String,
        result: Result<(), String>,
    },
    ManifestsPurged {
        request_id: u64,
        key: String,
        base: Vec<(
            String,
            crate::session_manager::paged_manifest::PublishedManifest,
            crate::session_manager::paged_manifest::ManifestReader,
        )>,
    },
    QueryPublished {
        request_id: u64,
        scope_epoch: u64,
        scope: String,
        base_generation: String,
        view: crate::session_manager::project_scope::SessionViewSpec,
        query_namespace: crate::session_manager::paged_manifest::QueryManifestNamespace,
        result: Result<
            (
                crate::session_manager::paged_manifest::PublishedManifest,
                crate::session_manager::paged_manifest::ManifestReader,
            ),
            String,
        >,
    },
    ProjectCatalogBuilt {
        request_id: u64,
        scope_epoch: u64,
        scope: String,
        base_generation: String,
        result: Result<crate::session_manager::project_scope::SessionProjectCatalog, String>,
    },
    ProjectFilterBuilt {
        request_id: u64,
        scope_epoch: u64,
        scope: String,
        base_generation: String,
        query: String,
        result: Result<Vec<usize>, String>,
    },
}

pub(crate) enum QuotaReq {
    Refresh {
        generation: QuotaSnapshotGeneration,
        target: QuotaTarget,
    },
}

pub(crate) enum QuotaMsg {
    Finished {
        generation: QuotaSnapshotGeneration,
        target: QuotaTarget,
        result: Result<crate::cli::tui::data::ProviderUsageQuota, String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum AppDataLoadKind {
    Initial,
    Snapshot,
    Full,
}

#[derive(Debug, Clone)]
pub(crate) enum AppDataReq {
    InitialLoad {
        request_id: u64,
        generation: u64,
        app_state_epoch: u64,
        app_type: AppType,
        /// Other visible apps to pre-seed from the same in-memory snapshot, each
        /// paired with its own request_id (matching a pending entry registered by
        /// the cache). Lets one initial request warm every visible app so the first
        /// switch renders real data instead of an empty placeholder.
        extras: Vec<(AppType, u64)>,
    },
    Load {
        request_id: u64,
        generation: u64,
        app_state_epoch: u64,
        app_type: AppType,
    },
    FullLoad {
        request_id: u64,
        generation: u64,
        app_state_epoch: u64,
        app_type: AppType,
    },
    DropState {
        ack: mpsc::Sender<()>,
    },
}

pub(crate) enum AppDataMsg {
    Loaded {
        kind: AppDataLoadKind,
        request_id: u64,
        generation: u64,
        app_state_epoch: u64,
        app_type: AppType,
        result: Result<crate::cli::tui::data::UiData, String>,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum UsagePricingReq {
    Load {
        request_id: u64,
        generation: u64,
        app_state_epoch: u64,
        app_type: AppType,
        range: crate::cli::tui::data::UsageRangePreset,
    },
    LoadLogPage {
        request_id: u64,
        generation: u64,
        app_state_epoch: u64,
        app_type: AppType,
        range: crate::cli::tui::data::UsageRangePreset,
        page: usize,
        cursor: crate::cli::tui::data::UsageLogCursor,
        direction: crate::cli::tui::data::UsageLogPageDirection,
        limit: usize,
    },
    LoadLogDetail {
        request_id: u64,
        generation: u64,
        app_state_epoch: u64,
        app_type: AppType,
        range: crate::cli::tui::data::UsageRangePreset,
        log_rowid: i64,
    },
    DropState {
        ack: mpsc::Sender<()>,
    },
}

pub(crate) enum UsagePricingMsg {
    LogHeadLoaded {
        request_id: u64,
        generation: u64,
        app_state_epoch: u64,
        app_type: AppType,
        range: crate::cli::tui::data::UsageRangePreset,
        result: Result<crate::cli::tui::data::UsageLogPage, UsageLogLoadError>,
    },
    Loaded {
        request_id: u64,
        generation: u64,
        app_state_epoch: u64,
        app_type: AppType,
        range: crate::cli::tui::data::UsageRangePreset,
        result: Box<Result<crate::cli::tui::data::UsagePricingData, UsagePricingLoadError>>,
    },
    LogPageLoaded {
        request_id: u64,
        generation: u64,
        app_state_epoch: u64,
        app_type: AppType,
        range: crate::cli::tui::data::UsageRangePreset,
        page: usize,
        direction: crate::cli::tui::data::UsageLogPageDirection,
        result: Result<crate::cli::tui::data::UsageLogPage, UsageLogLoadError>,
    },
    LogDetailLoaded {
        request_id: u64,
        generation: u64,
        app_state_epoch: u64,
        app_type: AppType,
        range: crate::cli::tui::data::UsageRangePreset,
        log_rowid: i64,
        result: Result<Option<crate::cli::tui::data::UsageLogRow>, UsageLogLoadError>,
    },
}

#[derive(Debug)]
pub(crate) enum UsageLogLoadError {
    Cancelled,
    Failed(String),
}

#[derive(Debug)]
pub(crate) enum UsagePricingLoadError {
    /// The query was deliberately interrupted because a newer request or a
    /// DropState barrier superseded it. This is control flow, not a user-facing
    /// failure.
    Cancelled,
    Failed(String),
}

pub(crate) enum SessionUsageSyncReq {
    Run { request_id: u64 },
    RebuildCodex { request_id: u64 },
}

pub(crate) enum SessionUsageSyncMsg {
    Finished {
        request_id: u64,
        result: Result<(), String>,
    },
    CodexRebuilt {
        request_id: u64,
        result: Result<crate::services::session_usage::SessionSyncResult, String>,
    },
}

pub(crate) enum SkillsReq {
    Discover {
        request_id: u64,
        query: String,
        source: crate::cli::tui::app::SkillsDiscoverSource,
        force: bool,
    },
    Install {
        spec: String,
        app: AppType,
    },
    CheckUpdates,
    Update {
        ids: Vec<String>,
    },
    MigrateStorage {
        target: crate::services::skill::SkillStorageLocation,
    },
}

pub(crate) enum SkillsMsg {
    DiscoverFinished {
        request_id: u64,
        query: String,
        source: crate::cli::tui::app::SkillsDiscoverSource,
        result: Result<Vec<crate::services::skill::Skill>, String>,
    },
    InstallFinished {
        spec: String,
        result: Result<crate::services::skill::InstalledSkill, String>,
    },
    UpdatesChecked {
        result: Result<crate::services::skill::SkillUpdateCheckResult, String>,
    },
    SkillsUpdated {
        result: Result<crate::services::skill::SkillUpdateBatchResult, String>,
    },
    StorageMigrated {
        target: crate::services::skill::SkillStorageLocation,
        result: Result<crate::services::skill::MigrationResult, String>,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum WebDavReqKind {
    CheckConnection,
    Upload,
    Download,
    MigrateV1ToV2,
    JianguoyunQuickSetup {
        username: String,
        password: String,
    },
    S3CheckConnection,
    S3FetchRemoteInfo {
        intent: crate::cli::tui::app::CloudSyncTransferIntent,
    },
    S3Upload,
    S3Download,
}

#[derive(Debug, Clone)]
pub(crate) struct WebDavReq {
    pub(crate) request_id: u64,
    pub(crate) kind: WebDavReqKind,
}

#[derive(Debug, Clone)]
pub(crate) enum WebDavDone {
    ConnectionChecked,
    Uploaded {
        decision: SyncDecision,
        message: String,
    },
    Downloaded {
        decision: SyncDecision,
        message: String,
    },
    #[allow(dead_code)]
    V1Migrated {
        message: String,
    },
    JianguoyunConfigured,
    S3ConnectionChecked,
    S3RemoteInfoFetched {
        intent: crate::cli::tui::app::CloudSyncTransferIntent,
        info: Option<crate::services::S3RemoteInfo>,
    },
    S3Uploaded {
        decision: SyncDecision,
        message: String,
    },
    S3Downloaded {
        decision: SyncDecision,
        message: String,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum WebDavErr {
    Generic(String),
    QuickSetupSave(String),
    QuickSetupCheck(String),
}

pub(crate) enum WebDavMsg {
    Finished {
        request_id: u64,
        req: WebDavReqKind,
        result: Result<WebDavDone, WebDavErr>,
    },
}

pub(crate) enum ManagedAuthReq {
    Refresh {
        auth_provider: String,
    },
    StartLogin {
        auth_provider: String,
    },
    PollLogin {
        auth_provider: String,
        device_code: String,
    },
    SetDefault {
        auth_provider: String,
        account_id: String,
    },
    Remove {
        auth_provider: String,
        account_id: String,
    },
}

pub(crate) enum ManagedAuthMsg {
    Status {
        auth_provider: String,
        result: Result<crate::services::ManagedAuthStatus, String>,
    },
    LoginStarted {
        auth_provider: String,
        result: Result<crate::services::ManagedAuthDeviceCodeResponse, String>,
    },
    LoginPolled {
        auth_provider: String,
        device_code: String,
        result: Result<Option<crate::services::ManagedAuthAccount>, String>,
    },
    DefaultSet {
        #[allow(dead_code)]
        auth_provider: String,
        #[allow(dead_code)]
        account_id: String,
        result: Result<crate::services::ManagedAuthStatus, String>,
    },
    Removed {
        #[allow(dead_code)]
        auth_provider: String,
        account_id: String,
        result: Result<crate::services::ManagedAuthStatus, String>,
    },
}

pub(crate) struct SpeedtestSystem {
    pub(crate) req_tx: mpsc::Sender<String>,
    pub(crate) result_rx: mpsc::Receiver<SpeedtestMsg>,
    pub(crate) _handle: std::thread::JoinHandle<()>,
}

pub(crate) struct StreamCheckSystem {
    pub(crate) req_tx: mpsc::Sender<StreamCheckReq>,
    pub(crate) result_rx: mpsc::Receiver<StreamCheckMsg>,
    pub(crate) _handle: std::thread::JoinHandle<()>,
}

pub(crate) struct LocalEnvSystem {
    pub(crate) req_tx: tokio::sync::mpsc::UnboundedSender<LocalEnvReq>,
    pub(crate) result_rx: mpsc::Receiver<LocalEnvMsg>,
    pub(crate) _handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for LocalEnvSystem {
    fn drop(&mut self) {
        let _ = self.req_tx.send(LocalEnvReq::Shutdown);
        let Some(handle) = self._handle.take() else {
            return;
        };
        if handle.thread().id() == std::thread::current().id() {
            log::warn!("local environment worker attempted to join itself during shutdown");
            return;
        }
        if handle.join().is_err() {
            log::warn!("local environment worker panicked during shutdown");
        }
    }
}

pub(crate) struct SessionSystem {
    pub(crate) req_tx: mpsc::Sender<SessionReq>,
    pub(crate) cost_req_tx:
        crate::cli::tui::runtime_systems::session_cost::SessionCostRequestSender,
    pub(crate) result_rx: mpsc::Receiver<SessionMsg>,
    pub(crate) _handles: Vec<std::thread::JoinHandle<()>>,
}

pub(crate) struct QuotaSystem {
    pub(crate) req_tx: mpsc::Sender<QuotaReq>,
    pub(crate) result_rx: mpsc::Receiver<QuotaMsg>,
    pub(crate) _handle: std::thread::JoinHandle<()>,
}

pub(crate) struct AppDataSystem {
    pub(crate) req_tx: mpsc::Sender<AppDataReq>,
    pub(crate) result_rx: mpsc::Receiver<AppDataMsg>,
    pub(crate) _handle: std::thread::JoinHandle<()>,
}

pub(crate) struct UsagePricingSystem {
    pub(crate) req_tx: mpsc::Sender<UsagePricingReq>,
    pub(crate) result_rx: mpsc::Receiver<UsagePricingMsg>,
    pub(crate) _handles: Vec<std::thread::JoinHandle<()>>,
}

pub(crate) struct SessionUsageSyncSystem {
    pub(crate) req_tx: mpsc::Sender<SessionUsageSyncReq>,
    pub(crate) result_rx: mpsc::Receiver<SessionUsageSyncMsg>,
    pub(crate) _handle: std::thread::JoinHandle<()>,
}

#[derive(Debug, Clone)]
pub(crate) enum ProxyReq {
    SetManagedSessionForCurrentApp {
        request_id: u64,
        app_type: AppType,
        enabled: bool,
        base_reload_token: UiDataReloadToken,
    },
    RefreshSnapshot {
        request_id: u64,
        app_type: AppType,
    },
}

pub(crate) enum ManagedSessionOutcome {
    Failed(String),
    Applied {
        snapshot: Result<Box<ProviderRuntimeSnapshot>, String>,
    },
}

pub(crate) enum ProxyMsg {
    ManagedSessionFinished {
        request_id: u64,
        app_type: AppType,
        enabled: bool,
        base_reload_token: UiDataReloadToken,
        outcome: ManagedSessionOutcome,
    },
    SnapshotRefreshed {
        request_id: u64,
        app_type: AppType,
        result: Result<ProxySnapshot, String>,
    },
}

pub(crate) struct ProxySystem {
    pub(crate) req_tx: mpsc::Sender<ProxyReq>,
    pub(crate) result_rx: mpsc::Receiver<ProxyMsg>,
    pub(crate) _handle: std::thread::JoinHandle<()>,
}

#[derive(Debug, Clone)]
pub(crate) struct CodexHistoryReq {
    pub(crate) request_id: u64,
    pub(crate) enabled: bool,
    pub(crate) migrate_existing: bool,
    pub(crate) restore_after_disable: bool,
}

pub(crate) enum CodexHistoryMsg {
    Saved {
        request_id: u64,
        enabled: bool,
        result: Result<crate::services::codex_history::CodexHistoryToggleOutcome, String>,
    },
    RestoreFinished(
        Result<crate::codex_history_migration::CodexOfficialHistoryRestoreOutcome, String>,
    ),
}

pub(crate) struct CodexHistorySystem {
    pub(crate) req_tx: mpsc::Sender<CodexHistoryReq>,
    pub(crate) result_rx: mpsc::Receiver<CodexHistoryMsg>,
    pub(crate) _handles: Vec<std::thread::JoinHandle<()>>,
}

pub(crate) struct SkillsSystem {
    pub(crate) req_tx: mpsc::Sender<SkillsReq>,
    pub(crate) result_rx: mpsc::Receiver<SkillsMsg>,
    pub(crate) _handle: std::thread::JoinHandle<()>,
}

pub(crate) struct WebDavSystem {
    pub(crate) req_tx: mpsc::Sender<WebDavReq>,
    pub(crate) result_rx: mpsc::Receiver<WebDavMsg>,
    pub(crate) _handle: std::thread::JoinHandle<()>,
}

pub(crate) struct ManagedAuthSystem {
    pub(crate) req_tx: mpsc::Sender<ManagedAuthReq>,
    pub(crate) result_rx: mpsc::Receiver<ManagedAuthMsg>,
    pub(crate) _handle: std::thread::JoinHandle<()>,
}

pub(crate) enum UpdateReq {
    Check { request_id: u64 },
    Download,
}

pub(crate) enum UpdateMsg {
    CheckFinished {
        request_id: u64,
        result: Result<crate::cli::commands::update::UpdateCheckInfo, String>,
    },
    DownloadProgress {
        downloaded: u64,
        total: Option<u64>,
    },
    DownloadFinished(Result<String, String>),
}

pub(crate) struct UpdateSystem {
    pub(crate) req_tx: mpsc::Sender<UpdateReq>,
    pub(crate) result_rx: mpsc::Receiver<UpdateMsg>,
    pub(crate) _handle: std::thread::JoinHandle<()>,
}

pub(crate) enum ModelFetchReq {
    Fetch {
        request_id: u64,
        base_url: String,
        is_full_url: bool,
        api_key: Option<String>,
        custom_user_agent: Option<String>,
        api_protocol: Option<String>,
        request_headers: Option<BTreeMap<String, String>>,
        discovery_timeout_ms: Option<u64>,
        codex_oauth: bool,
        codex_oauth_account_id: Option<String>,
        field: ProviderAddField,
        claude_idx: Option<usize>,
    },
}

pub(crate) enum ModelFetchMsg {
    Finished {
        request_id: u64,
        field: ProviderAddField,
        claude_idx: Option<usize>,
        result: Result<Vec<String>, String>,
    },
}

pub(crate) struct ModelFetchSystem {
    pub(crate) req_tx: mpsc::Sender<ModelFetchReq>,
    pub(crate) result_rx: mpsc::Receiver<ModelFetchMsg>,
    pub(crate) _handle: std::thread::JoinHandle<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelFetchStrategy {
    /// Fetch a models endpoint without adding an authentication header.
    /// OMP uses this for providers configured with `auth: none`.
    Anonymous,
    /// Fetch an OMP Ollama discovery registry (`GET /api/tags`) without
    /// imposing OpenAI-compatible authentication or URL suffixes.
    Ollama,
    /// Fetch an OMP llama.cpp discovery registry (`GET /models`) from the
    /// native server root, stripping a configured `/v1` request suffix.
    LlamaCpp,
    Bearer,
    Anthropic,
    GoogleApiKey,
    AzureApiKey,
}

pub(crate) fn model_fetch_strategy_for_field(field: ProviderAddField) -> ModelFetchStrategy {
    match field {
        ProviderAddField::GeminiModel => ModelFetchStrategy::GoogleApiKey,
        ProviderAddField::ClaudeModelConfig => ModelFetchStrategy::Anthropic,
        _ => ModelFetchStrategy::Bearer,
    }
}

pub(crate) fn build_model_fetch_candidate_urls(
    base_url: &str,
    strategy: ModelFetchStrategy,
    is_full_url: bool,
) -> Vec<String> {
    build_model_fetch_candidate_urls_with_inject_v1(base_url, strategy, is_full_url, None)
}

/// Build model-list endpoints, optionally honoring OMP's
/// `discovery.injectV1` setting for `openai-models-list` providers.
pub(crate) fn build_model_fetch_candidate_urls_with_inject_v1(
    base_url: &str,
    strategy: ModelFetchStrategy,
    is_full_url: bool,
    inject_v1: Option<bool>,
) -> Vec<String> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Vec::new();
    }

    // OMP ignores query strings while constructing discovery endpoints. Do
    // the same here so a configured `baseUrl?token=...` does not become the
    // malformed path `...?token=.../models`.
    let base_without_query = match url::Url::parse(base) {
        Ok(mut parsed) => {
            parsed.set_query(None);
            parsed.set_fragment(None);
            parsed.to_string().trim_end_matches('/').to_string()
        }
        Err(_) => base.to_string(),
    };
    let base = base_without_query.as_str();

    if is_full_url {
        let mut urls = Vec::new();
        if let Some(index) = base.find("/v1/") {
            urls.push(format!("{}/v1/models", &base[..index]));
        } else if let Some(index) = base.rfind('/') {
            let root = &base[..index];
            if root
                .find("://")
                .is_some_and(|scheme| root.len() > scheme.saturating_add(3))
            {
                urls.push(format!("{root}/v1/models"));
            }
        }
        return urls;
    }

    if base.ends_with("/models") {
        return vec![base.to_string()];
    }

    let append_models = format!("{base}/models");
    let append_versioned_models = if base.ends_with("/v1") || base.ends_with("/v1beta") {
        None
    } else {
        Some(format!("{base}/v1/models"))
    };

    let mut urls: Vec<String> = Vec::new();
    // OMP's openai-models-list discovery defaults to `/v1/models`; callers
    // can explicitly disable injection to probe the bare `/models` route.
    if let Some(inject_v1) = inject_v1 {
        if inject_v1 {
            return vec![append_versioned_models.unwrap_or(append_models)];
        }
        return vec![append_models];
    }

    match strategy {
        ModelFetchStrategy::Ollama => {
            let root = crate::omp_config::normalize_ollama_base_url(base)
                .unwrap_or_else(|| base.to_string());
            return vec![format!("{root}/api/tags")];
        }
        ModelFetchStrategy::LlamaCpp => {
            let root = strip_trailing_v1(base);
            return vec![format!("{root}/models")];
        }
        ModelFetchStrategy::Anthropic => {
            if let Some(versioned) = append_versioned_models.as_ref() {
                urls.push(versioned.clone());
            } else {
                urls.push(append_models.clone());
            }

            if let Some(stripped) = strip_compat_suffix(base) {
                let root = stripped.trim_end_matches('/');
                if !root.is_empty() && root.contains("://") {
                    urls.push(format!("{root}/v1/models"));
                    urls.push(format!("{root}/models"));
                }
            } else if append_versioned_models.is_some() {
                urls.push(append_models);
            }
        }
        ModelFetchStrategy::Anonymous
        | ModelFetchStrategy::Bearer
        | ModelFetchStrategy::GoogleApiKey
        | ModelFetchStrategy::AzureApiKey => {
            urls.push(append_models);
            if let Some(v1) = append_versioned_models.as_ref() {
                urls.push(v1.clone());
            }
        }
    }

    let mut seen = HashSet::new();
    urls.retain(|url| seen.insert(url.clone()));
    urls
}

fn strip_trailing_v1(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    if trimmed.len() >= 3 && trimmed[trimmed.len() - 3..].eq_ignore_ascii_case("/v1") {
        let root = trimmed[..trimmed.len() - 3].trim_end_matches('/');
        if !root.is_empty() {
            return root.to_string();
        }
    }
    trimmed.to_string()
}

fn strip_compat_suffix(base: &str) -> Option<&str> {
    let lower = base.to_ascii_lowercase();
    KNOWN_COMPAT_SUFFIXES.iter().find_map(|suffix| {
        lower
            .ends_with(suffix)
            .then(|| &base[..base.len() - suffix.len()])
    })
}

pub(crate) fn parse_model_ids_from_response(payload: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    fn collect(value: &Value, in_collection: bool, out: &mut Vec<String>) {
        match value {
            Value::Array(items) => {
                for item in items {
                    collect(item, true, out);
                }
            }
            Value::Object(object) => {
                // OpenAI-style entries use `id`; Gemini/Ollama-style entries
                // use `model` or `name`. Only accept names from collection members so an
                // envelope's own descriptive `name` cannot become a model ID.
                if in_collection {
                    if let Some(id) = object.get("id").and_then(Value::as_str) {
                        let id = id.trim();
                        if !id.is_empty() {
                            out.push(id.to_string());
                        }
                    } else if let Some(model) = object.get("model").and_then(Value::as_str) {
                        let model = model.trim();
                        if !model.is_empty() {
                            out.push(model.to_string());
                        }
                    } else if let Some(name) = object.get("name").and_then(Value::as_str) {
                        let name = name.trim();
                        if !name.is_empty() {
                            out.push(name.strip_prefix("models/").unwrap_or(name).to_string());
                        }
                    }
                }

                // Discovery servers commonly wrap their list in one or more
                // of these keys (`data`, `models`, `result`, or `items`).
                // Recurse through objects as well as arrays to tolerate
                // envelopes such as {"result":{"items":[...]}}.
                for key in ["data", "models", "result", "items"] {
                    if let Some(nested) = object.get(key) {
                        collect(nested, true, out);
                    }
                }
            }
            _ => {}
        }
    }

    collect(payload, false, &mut out);

    let mut seen = HashSet::new();
    out.retain(|model| seen.insert(model.clone()));
    out
}

pub(crate) async fn fetch_provider_models_for_tui(
    base_url: &str,
    is_full_url: bool,
    api_key: Option<&str>,
    custom_user_agent: Option<&str>,
    strategy: ModelFetchStrategy,
    request_headers: Option<&BTreeMap<String, String>>,
) -> Result<Vec<String>, String> {
    fetch_provider_models_for_tui_with_inject_v1(
        base_url,
        is_full_url,
        api_key,
        custom_user_agent,
        strategy,
        request_headers,
        None,
    )
    .await
}

pub(crate) async fn fetch_provider_models_for_tui_with_inject_v1(
    base_url: &str,
    is_full_url: bool,
    api_key: Option<&str>,
    custom_user_agent: Option<&str>,
    strategy: ModelFetchStrategy,
    request_headers: Option<&BTreeMap<String, String>>,
    inject_v1: Option<bool>,
) -> Result<Vec<String>, String> {
    fetch_provider_models_for_tui_with_options(
        base_url,
        is_full_url,
        api_key,
        custom_user_agent,
        strategy,
        request_headers,
        inject_v1,
        None,
    )
    .await
}

/// Fetch provider models with optional OMP discovery URL and timeout
/// overrides. Existing callers can continue using the two compatibility
/// wrappers above, while OMP provider inspection/TUI flows pass through the
/// native `discovery.timeoutMs` value.
pub(crate) async fn fetch_provider_models_for_tui_with_options(
    base_url: &str,
    is_full_url: bool,
    api_key: Option<&str>,
    custom_user_agent: Option<&str>,
    strategy: ModelFetchStrategy,
    request_headers: Option<&BTreeMap<String, String>>,
    inject_v1: Option<bool>,
    discovery_timeout_ms: Option<u64>,
) -> Result<Vec<String>, String> {
    let candidate_urls =
        build_model_fetch_candidate_urls_with_inject_v1(base_url, strategy, is_full_url, inject_v1);
    if candidate_urls.is_empty() {
        return Err(if is_full_url && !base_url.trim().is_empty() {
            "Cannot derive models endpoint from full URL".to_string()
        } else {
            "URL cannot be empty".to_string()
        });
    }

    let client = crate::proxy::http_client::get();

    let key = api_key.map(str::trim).filter(|k| !k.is_empty());
    let custom_user_agent = crate::provider::parse_custom_user_agent(custom_user_agent)
        .ok()
        .flatten();
    if !matches!(
        strategy,
        ModelFetchStrategy::Anonymous | ModelFetchStrategy::Ollama | ModelFetchStrategy::LlamaCpp
    ) && key.is_none()
        && request_headers.is_none_or(BTreeMap::is_empty)
    {
        return Err("API Key or request headers are required to fetch models".to_string());
    }
    if request_headers.is_some_and(|headers| headers.len() > 64) {
        return Err("Too many model-fetch request headers (maximum 64)".to_string());
    }
    let mut last_err = String::from("unknown error");

    for url in candidate_urls {
        let timeout = discovery_timeout_ms
            .map(|timeout| timeout.clamp(1, crate::omp_config::OMP_MAX_DISCOVERY_TIMEOUT_MS))
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(5));
        let mut req = client.get(&url).timeout(timeout);
        if matches!(strategy, ModelFetchStrategy::Ollama) {
            req = req.header(reqwest::header::ACCEPT, "application/json");
        }
        if let Some(key) = key {
            req = match strategy {
                ModelFetchStrategy::Anonymous | ModelFetchStrategy::Ollama => req,
                ModelFetchStrategy::LlamaCpp => {
                    req.header("Authorization", format!("Bearer {key}"))
                }
                ModelFetchStrategy::Bearer => req.header("Authorization", format!("Bearer {key}")),
                ModelFetchStrategy::Anthropic => req
                    .header("Authorization", format!("Bearer {key}"))
                    .header("x-api-key", key)
                    .header("anthropic-version", "2023-06-01"),
                ModelFetchStrategy::GoogleApiKey => req.header("x-goog-api-key", key),
                ModelFetchStrategy::AzureApiKey => req.header("api-key", key),
            };
        }
        if let Some(user_agent) = &custom_user_agent {
            req = req.header(reqwest::header::USER_AGENT, user_agent.clone());
        }
        if let Some(request_headers) = request_headers {
            for (raw_name, raw_value) in request_headers {
                let name = reqwest::header::HeaderName::from_bytes(raw_name.trim().as_bytes())
                    .map_err(|error| {
                        format!("Invalid model-fetch header name {raw_name}: {error}")
                    })?;
                let value = reqwest::header::HeaderValue::from_str(raw_value).map_err(|error| {
                    format!("Invalid model-fetch header value for {name}: {error}")
                })?;
                req = req.header(name, value);
            }
        }

        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                if !status.is_success() {
                    last_err = format!("HTTP {status} ({url})");
                    if status != reqwest::StatusCode::NOT_FOUND
                        && status != reqwest::StatusCode::METHOD_NOT_ALLOWED
                    {
                        return Err(last_err);
                    }
                    continue;
                }
                match resp.json::<Value>().await {
                    Ok(payload) => {
                        let models = parse_model_ids_from_response(&payload);
                        if models.is_empty() {
                            last_err = format!("No model list found in response ({url})");
                        } else {
                            return Ok(models);
                        }
                    }
                    Err(err) => {
                        last_err = format!("Invalid JSON response ({url}): {err}");
                    }
                }
            }
            Err(err) => {
                last_err = err.to_string();
            }
        }
    }

    Err(last_err)
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct RequestTracker {
    pub(crate) seq: u64,
    pub(crate) active: Option<u64>,
}

impl RequestTracker {
    pub(crate) fn start(&mut self) -> u64 {
        self.seq = self.seq.wrapping_add(1);
        self.active = Some(self.seq);
        self.seq
    }

    pub(crate) fn cancel(&mut self) {
        self.active = None;
    }

    pub(crate) fn is_stale(&self, request_id: u64) -> bool {
        matches!(self.active, Some(active_request_id) if active_request_id != request_id)
    }

    pub(crate) fn finish_if_active(&mut self, request_id: u64) -> bool {
        if self.active == Some(request_id) {
            self.active = None;
            true
        } else {
            false
        }
    }
}

fn stream_check_status_label(status: &HealthStatus) -> &'static str {
    match status {
        HealthStatus::Operational => texts::tui_stream_check_status_operational(),
        HealthStatus::Degraded => texts::tui_stream_check_status_degraded(),
        HealthStatus::Failed => texts::tui_stream_check_status_failed(),
    }
}

pub(crate) fn build_stream_check_result_lines(
    provider_name: &str,
    result: &StreamCheckResult,
) -> Vec<String> {
    let response_time = result
        .response_time_ms
        .map(|ms| texts::tui_latency_ms(ms as u128))
        .unwrap_or_else(|| texts::tui_na().to_string());
    let http_status = result
        .http_status
        .map(|status| status.to_string())
        .unwrap_or_else(|| texts::tui_na().to_string());
    let model = if result.model_used.trim().is_empty() {
        texts::tui_na().to_string()
    } else {
        result.model_used.clone()
    };

    vec![
        texts::tui_stream_check_line_provider(provider_name),
        texts::tui_stream_check_line_status(stream_check_status_label(&result.status)),
        texts::tui_stream_check_line_response_time(&response_time),
        texts::tui_stream_check_line_http_status(&http_status),
        texts::tui_stream_check_line_model(&model),
        texts::tui_stream_check_line_retries(&result.retry_count.to_string()),
        texts::tui_stream_check_line_message(&result.message),
    ]
}
