use super::*;

#[derive(Debug, Clone)]
pub struct FilterState {
    pub active: bool,
    pub input: TextInput,
    pub scope: FilterScope,
}

impl FilterState {
    pub fn new() -> Self {
        Self {
            active: false,
            input: TextInput::new(""),
            scope: FilterScope::Global,
        }
    }

    pub fn query_lower(&self) -> Option<String> {
        let trimmed = self.input.value.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(trimmed.to_lowercase())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterScope {
    Global,
    SessionMessages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkillsDiscoverSource {
    Repos,
    Marketplace,
}

impl SkillsDiscoverSource {
    pub fn toggled(self) -> Self {
        match self {
            Self::Repos => Self::Marketplace,
            Self::Marketplace => Self::Repos,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Nav,
    Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionsPane {
    List,
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsagePane {
    Models,
    Providers,
    Recent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageMetric {
    Cost,
    Tokens,
    Requests,
    Errors,
}

#[derive(Debug, Clone)]
pub struct UsageState {
    pub range: crate::cli::tui::data::UsageRangePreset,
    pub metric: UsageMetric,
    pub pane: UsagePane,
    pub selected_idx: usize,
    pub logs_idx: usize,
    loading_ranges: HashSet<(AppType, crate::cli::tui::data::UsageRangePreset)>,
}

impl Default for UsageState {
    fn default() -> Self {
        Self {
            range: crate::cli::tui::data::UsageRangePreset::SevenDays,
            metric: UsageMetric::Cost,
            pane: UsagePane::Models,
            selected_idx: 0,
            logs_idx: 0,
            loading_ranges: HashSet::new(),
        }
    }
}

impl UsageState {
    pub(crate) fn start_loading(
        &mut self,
        app_type: AppType,
        range: crate::cli::tui::data::UsageRangePreset,
    ) {
        self.loading_ranges.insert((app_type, range));
    }

    pub(crate) fn finish_loading(
        &mut self,
        app_type: &AppType,
        range: crate::cli::tui::data::UsageRangePreset,
    ) {
        self.loading_ranges.remove(&(app_type.clone(), range));
    }

    pub(crate) fn clear_loading(&mut self) {
        self.loading_ranges.clear();
    }

    pub(crate) fn clear_custom_loading_for_app(&mut self, app_type: &AppType) {
        self.loading_ranges.retain(|(loading_app_type, range)| {
            loading_app_type != app_type
                || !matches!(range, crate::cli::tui::data::UsageRangePreset::Custom(_))
        });
    }

    pub(crate) fn is_loading_for(
        &self,
        app_type: &AppType,
        range: crate::cli::tui::data::UsageRangePreset,
    ) -> bool {
        self.loading_ranges
            .iter()
            .any(|(loading_app_type, loading_range)| {
                loading_app_type == app_type && usage_loading_range_matches(*loading_range, range)
            })
    }
}

fn usage_loading_range_matches(
    loading_range: crate::cli::tui::data::UsageRangePreset,
    active_range: crate::cli::tui::data::UsageRangePreset,
) -> bool {
    match (loading_range, active_range) {
        (
            crate::cli::tui::data::UsageRangePreset::Custom(loading),
            crate::cli::tui::data::UsageRangePreset::Custom(active),
        ) => loading == active,
        (crate::cli::tui::data::UsageRangePreset::Custom(_), _) => false,
        (_, crate::cli::tui::data::UsageRangePreset::Custom(_)) => false,
        _ => true,
    }
}

#[derive(Debug, Clone, Default)]
pub struct PricingState {
    pub selected_idx: usize,
}

/// A stashed scan result for one provider, reused on re-entry/app-switch so the
/// list renders instantly instead of re-reading every session file from disk.
/// The cache lives for the whole TUI run (the process is short-lived) and is
/// only refreshed by an explicit manual reload (`r`), never automatically.
#[derive(Debug, Clone)]
pub struct CachedScan {
    pub rows: Vec<crate::session_manager::SessionMeta>,
}

#[derive(Debug, Clone)]
pub struct SessionsState {
    pub provider_id: Option<String>,
    pub time_anchor_ms: i64,
    pub rows: Vec<crate::session_manager::SessionMeta>,
    pub selected_idx: usize,
    pub pane: SessionsPane,
    pub message_idx: usize,
    pub loading: bool,
    pub loaded_once: bool,
    pub last_error: Option<String>,
    pub scan_seq: u64,
    pub scan_active: Option<u64>,
    pub detail_key: Option<String>,
    pub messages_key: Option<String>,
    pub messages: Vec<crate::session_manager::SessionMessage>,
    pub message_filter: TextInput,
    pub messages_loading: bool,
    pub messages_loaded: bool,
    pub messages_error: Option<String>,
    pub message_seq: u64,
    pub message_active: Option<u64>,
    pub delete_seq: u64,
    pub delete_active: HashSet<u64>,
    /// UI 侧 tombstone：在途扫描期间删除成功的会话键（`session_key`），用于挡住
    /// 删除前读到旧列表的 partial/finished 把已删会话放回 UI。在 `finish_scan`
    /// 终态清空（见其时序注释）。键与删除流程一致（provider:session:source_path）。
    pub scan_tombstones: HashSet<String>,
    pub scan_cache: std::collections::HashMap<String, CachedScan>,
    /// When true, the session list shows all providers (the "全部" tab).
    /// When false (default), only the currently selected provider is shown.
    pub show_all_providers: bool,
    /// Deep search state: query string and results from backend full-content search.
    pub deep_search_query: Option<String>,
    pub deep_search_seq: u64,
    pub deep_search_active: Option<u64>,
    pub deep_search_results: Vec<crate::session_manager::SessionSearchHit>,
    /// Pending deep search: (query, ticks since last input). When ticks >= threshold, fire search.
    pub deep_search_pending: Option<(String, u64)>,
}

impl Default for SessionsState {
    fn default() -> Self {
        Self {
            provider_id: None,
            time_anchor_ms: chrono::Utc::now().timestamp_millis(),
            rows: Vec::new(),
            selected_idx: 0,
            pane: SessionsPane::List,
            message_idx: 0,
            loading: false,
            loaded_once: false,
            last_error: None,
            scan_seq: 0,
            scan_active: None,
            detail_key: None,
            messages_key: None,
            messages: Vec::new(),
            message_filter: TextInput::new(""),
            messages_loading: false,
            messages_loaded: false,
            messages_error: None,
            message_seq: 0,
            message_active: None,
            delete_seq: 0,
            delete_active: HashSet::new(),
            scan_tombstones: HashSet::new(),
            scan_cache: std::collections::HashMap::new(),
            show_all_providers: false,
            deep_search_query: None,
            deep_search_seq: 0,
            deep_search_active: None,
            deep_search_results: Vec::new(),
            deep_search_pending: None,
        }
    }
}

impl SessionsState {
    pub(crate) fn loaded_for_provider(&self, provider_id: &str) -> bool {
        self.loaded_once && self.provider_id.as_deref() == Some(provider_id)
    }

    pub(crate) fn reset_time_anchor(&mut self) {
        self.time_anchor_ms = chrono::Utc::now().timestamp_millis();
    }

    pub(crate) fn start_scan(&mut self, provider_id: String) -> u64 {
        if self.provider_id.as_deref() != Some(provider_id.as_str()) {
            self.rows.clear();
            self.selected_idx = 0;
            self.loaded_once = false;
            self.clear_detail();
        }
        self.provider_id = Some(provider_id);
        self.time_anchor_ms = chrono::Utc::now().timestamp_millis();
        self.scan_seq = self.scan_seq.wrapping_add(1);
        self.scan_active = Some(self.scan_seq);
        self.loading = true;
        self.last_error = None;
        self.scan_seq
    }

    pub(crate) fn fail_scan(&mut self, request_id: u64, error: String) {
        if self.scan_active == Some(request_id) {
            self.scan_active = None;
            self.loading = false;
            self.loaded_once = true;
            self.last_error = Some(error);
            // 失败即本次扫描的终态：此后不再接收该 scan 的任何 partial/finished，
            // 继续保留 tombstone 无收益，反而会让残留的 tombstone 把下一轮同 key
            // 的新建/恢复会话误过滤一次。故与 finish_scan 一致地在终态清空。
            // 只在接受当前 request_id 时清空——避免旧扫描的迟到失败误清掉本轮
            // active 扫描登记的 tombstone。
            self.scan_tombstones.clear();
        }
    }

    /// Drop rows tombstoned by an in-flight delete (see `scan_tombstones`). A scan
    /// thread may have read a session file *before* the user deleted it, so the
    /// partial/finished it later delivers still carries that session; filtering
    /// here keeps the deleted row from resurrecting in the UI. No-op (early
    /// return) when there are no tombstones, which is the common case.
    fn drop_tombstoned_rows(&self, rows: &mut Vec<crate::session_manager::SessionMeta>) {
        if self.scan_tombstones.is_empty() {
            return;
        }
        rows.retain(|row| {
            !self
                .scan_tombstones
                .contains(&crate::cli::tui::app::session_key(row))
        });
    }

    pub(crate) fn finish_scan(
        &mut self,
        request_id: u64,
        mut rows: Vec<crate::session_manager::SessionMeta>,
    ) -> bool {
        if self.scan_active != Some(request_id) {
            return false;
        }
        self.drop_tombstoned_rows(&mut rows);
        // 时序论证：本次 finish_scan 对应的扫描线程可能在删除【之前】就读完了
        // 该会话文件，携带的仍是删除前的旧列表，所以终态也必须按 tombstone 过滤，
        // 不能只依赖"下一轮扫描文件已不存在"。过滤完成后再清空 tombstones：删除
        // 已落盘，下一轮扫描该文件已不存在，自然不会再出现，无需继续拦截。
        self.scan_tombstones.clear();
        self.scan_active = None;
        self.loading = false;
        self.loaded_once = true;
        self.last_error = None;
        self.rows = rows;
        if self.detail_key.as_deref().is_some_and(|key| {
            !self
                .rows
                .iter()
                .any(|session| crate::cli::tui::app::session_key(session) == key)
        }) {
            self.clear_detail();
        }
        if let Some(provider_id) = self.provider_id.clone() {
            self.scan_cache.insert(
                provider_id,
                CachedScan {
                    rows: self.rows.clone(),
                },
            );
        }
        true
    }

    /// Apply the stale-while-revalidate first paint: the list built from the
    /// persistent DB cache, delivered before the revalidating scan finishes. The
    /// rows become interactive immediately, but `loading`/`scan_active` stay set
    /// so the header keeps showing the refresh indicator and the eventual
    /// `finish_scan` (same request id) still applies. The in-memory scan cache is
    /// deliberately not written here — only the final, complete list is cached.
    /// Returns true when the snapshot was applied (still the active scan).
    pub(crate) fn apply_cached_snapshot(
        &mut self,
        request_id: u64,
        mut rows: Vec<crate::session_manager::SessionMeta>,
    ) -> bool {
        if self.scan_active != Some(request_id) {
            return false;
        }
        self.drop_tombstoned_rows(&mut rows);
        self.loaded_once = true;
        self.rows = rows;
        true
    }

    /// Progressive fill during a revalidating "all providers" scan: replace one
    /// provider's rows with its freshly-scanned list while the other providers
    /// keep their current rows (cached snapshot or earlier partials). Keeps the
    /// refresh indicator on until `finish_scan` (same request id) lands.
    pub(crate) fn apply_partial_scan(
        &mut self,
        request_id: u64,
        provider_id: &str,
        mut rows: Vec<crate::session_manager::SessionMeta>,
    ) -> bool {
        if self.scan_active != Some(request_id) {
            return false;
        }
        self.drop_tombstoned_rows(&mut rows);
        self.loaded_once = true;
        self.rows.retain(|row| row.provider_id != provider_id);
        self.rows.extend(rows);
        crate::session_manager::sort_by_recent(&mut self.rows);
        true
    }

    /// Restore this provider's list from the in-memory scan cache, skipping the
    /// disk scan entirely. The cache is valid for the whole run; a manual reload
    /// (`r`) bypasses this and re-scans. Returns true on a hit.
    pub(crate) fn restore_from_scan_cache(&mut self, provider_id: &str) -> bool {
        let Some(cached) = self.scan_cache.get(provider_id) else {
            return false;
        };
        let rows = cached.rows.clone();
        if self.provider_id.as_deref() != Some(provider_id) {
            self.clear_detail();
        }
        self.rows = rows;
        self.provider_id = Some(provider_id.to_string());
        self.selected_idx = 0;
        self.reset_time_anchor();
        self.loading = false;
        self.loaded_once = true;
        self.last_error = None;
        self.scan_active = None;
        true
    }

    pub(crate) fn open_detail(&mut self, key: String) {
        if self.detail_key.as_deref() == Some(key.as_str()) {
            return;
        }
        self.detail_key = Some(key);
        self.clear_messages();
    }

    pub(crate) fn message_query_lower(&self) -> Option<String> {
        let trimmed = self.message_filter.value.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(trimmed.to_lowercase())
    }

    pub(crate) fn clear_detail(&mut self) {
        self.detail_key = None;
        self.clear_messages();
    }

    fn clear_messages(&mut self) {
        self.messages_key = None;
        self.messages.clear();
        self.messages_loading = false;
        self.messages_loaded = false;
        self.messages_error = None;
        self.message_idx = 0;
        self.message_active = None;
    }

    pub(crate) fn start_message_load(&mut self, key: String) -> u64 {
        self.message_seq = self.message_seq.wrapping_add(1);
        self.message_active = Some(self.message_seq);
        self.messages_key = Some(key);
        self.messages.clear();
        self.messages_loading = true;
        self.messages_loaded = false;
        self.messages_error = None;
        self.message_idx = 0;
        self.message_seq
    }

    pub(crate) fn fail_message_load(&mut self, request_id: u64, key: &str, error: String) {
        if self.message_active == Some(request_id)
            && self.messages_key.as_deref() == Some(key)
            && self.detail_key.as_deref() == Some(key)
        {
            self.message_active = None;
            self.messages_loading = false;
            self.messages_loaded = true;
            self.messages_error = Some(error);
        }
    }

    pub(crate) fn finish_message_load(
        &mut self,
        request_id: u64,
        key: &str,
        messages: Vec<crate::session_manager::SessionMessage>,
    ) -> bool {
        if self.message_active != Some(request_id)
            || self.messages_key.as_deref() != Some(key)
            || self.detail_key.as_deref() != Some(key)
        {
            return false;
        }
        self.message_active = None;
        self.messages_loading = false;
        self.messages_loaded = true;
        self.messages_error = None;
        self.messages = messages;
        self.message_idx = self.message_idx.min(self.messages.len().saturating_sub(1));
        true
    }

    pub(crate) fn start_delete(&mut self) -> u64 {
        self.delete_seq = self.delete_seq.wrapping_add(1);
        self.delete_active.insert(self.delete_seq);
        self.delete_seq
    }

    pub(crate) fn finish_delete(&mut self, request_id: u64, key: &str) -> bool {
        if !self.delete_active.remove(&request_id) {
            return false;
        }
        self.remove_session_by_key(key)
    }

    pub(crate) fn fail_delete(&mut self, request_id: u64) {
        self.delete_active.remove(&request_id);
    }

    pub(crate) fn remove_session_by_key(&mut self, key: &str) -> bool {
        let before = self.rows.len();
        self.rows
            .retain(|session| crate::cli::tui::app::session_key(session) != key);
        if self.rows.len() == before {
            return false;
        }
        self.selected_idx = self.selected_idx.min(self.rows.len().saturating_sub(1));
        // Drop the deleted session from every cached bucket (current provider,
        // the virtual "all" bucket, and any other provider bucket) so it cannot
        // resurrect as a phantom row when the list is restored from cache after
        // switching provider/all views.
        for cached in self.scan_cache.values_mut() {
            cached
                .rows
                .retain(|session| crate::cli::tui::app::session_key(session) != key);
        }
        if self.detail_key.as_deref() == Some(key) {
            self.clear_detail();
        }
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub message: String,
    pub kind: ToastKind,
    pub remaining_ticks: u16,
    pub persistent: bool,
}

impl Toast {
    pub fn new(message: impl Into<String>, kind: ToastKind) -> Self {
        Self {
            message: message.into(),
            kind,
            remaining_ticks: 12,
            persistent: false,
        }
    }

    pub fn persistent(message: impl Into<String>, kind: ToastKind) -> Self {
        Self {
            message: message.into(),
            kind,
            remaining_ticks: 0,
            persistent: true,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    Quit,
    ProviderDelete {
        id: String,
    },
    ProviderCopy {
        id: String,
    },
    ProviderRemoveFromConfig {
        id: String,
    },
    McpDelete {
        id: String,
    },
    PromptDelete {
        id: String,
    },
    PricingDelete {
        model_id: String,
    },
    SessionDelete {
        key: String,
        provider_id: String,
        session_id: String,
        source_path: String,
    },
    SkillsUninstall {
        directory: String,
    },
    SkillsRepoRemove {
        owner: String,
        name: String,
    },
    ConfigImport {
        path: String,
    },
    ConfigRestoreBackup {
        id: String,
    },
    ConfigReset,
    SettingsSetSkipClaudeOnboarding {
        enabled: bool,
    },
    SettingsSetClaudePluginIntegration {
        enabled: bool,
    },
    SettingsSetCodexUnifiedSessionHistory {
        enabled: bool,
    },
    VisibleAppsAutoDetection,
    VisibleAppsSwitchToManual {
        apps: crate::settings::VisibleApps,
        selected: usize,
    },
    ProviderApiFormatProxyNotice,
    CommonConfigNotice,
    UsageQueryNotice,
    ManagedAuthCancelLogin,
    ProxyEnableAndAutoFailover {
        app_type: AppType,
    },
    PromptOpenImportCandidate {
        filename: String,
        content: String,
    },
    OpenClawDailyMemoryDelete {
        filename: String,
    },
    FormSaveBeforeClose,
    #[allow(dead_code)]
    EditorDiscard,
    EditorSaveBeforeClose,
    WebDavMigrateV1ToV2,
    ClaudeModelFillAll {
        source_idx: usize,
    },
}

#[derive(Debug, Clone)]
pub struct ConfirmOverlay {
    pub title: String,
    pub message: String,
    pub action: ConfirmAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextSubmit {
    ConfigExport,
    ConfigImport,
    ConfigBackupName,
    SettingsProxyListenAddress,
    SettingsProxyListenPort,
    SettingsOpenClawConfigDir,
    #[allow(dead_code)]
    SkillsInstallSpec,
    SkillsDiscoverQuery,
    SkillsRepoAdd,
    OpenClawDailyMemoryFilename,
    OpenClawToolsRule {
        section: OpenClawToolsSection,
        row: Option<usize>,
    },
    OpenClawAgentsRuntimeField {
        field: OpenClawAgentsRuntimeField,
    },
    UsageCustomRange,
    ProviderCustomUserAgent,
    CodexModelCatalogField {
        row: Option<usize>,
        field: form::CodexModelCatalogField,
    },
    WebDavJianguoyunUsername,
    WebDavJianguoyunPassword,
}

#[derive(Debug, Clone)]
pub struct TextInputState {
    pub title: String,
    pub prompt: String,
    pub input: TextInput,
    pub submit: TextSubmit,
    pub secret: bool,
}

impl TextInputState {
    pub const fn is_editing(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone)]
pub struct TextViewState {
    pub title: String,
    pub lines: Vec<String>,
    pub scroll: usize,
    pub action: Option<TextViewAction>,
}

#[derive(Debug, Clone)]
pub enum TextViewAction {
    ProxyToggleManagedRoute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommonSnippetViewSource {
    Global,
    ProviderForm,
}

#[derive(Debug, Clone)]
pub struct ManagedAuthLoginState {
    pub auth_provider: String,
    pub device_code: String,
    pub expires_at_tick: u64,
    pub poll_interval_ticks: u64,
    pub next_poll_tick: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadingKind {
    Generic,
    Proxy,
    WebDav,
    UpdateCheck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpEnvEditorField {
    Key,
    Value,
}

#[derive(Debug, Clone)]
pub struct McpEnvEntryEditorState {
    pub row: Option<usize>,
    pub return_selected: usize,
    pub field: McpEnvEditorField,
    pub key: crate::cli::tui::form::TextInput,
    pub value: crate::cli::tui::form::TextInput,
}

impl McpEnvEntryEditorState {
    pub fn key_active(&self) -> bool {
        matches!(self.field, McpEnvEditorField::Key)
    }

    pub fn value_active(&self) -> bool {
        matches!(self.field, McpEnvEditorField::Value)
    }

    pub fn is_editing(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone)]
pub enum Overlay {
    None,
    Help(crate::cli::tui::help::HelpState),
    Confirm(ConfirmOverlay),
    TextInput(TextInputState),
    BackupPicker {
        selected: usize,
    },
    TextView(TextViewState),
    #[allow(dead_code)]
    CommonSnippetPicker {
        selected: usize,
    },
    ProviderTestMenu {
        provider_id: String,
        selected: usize,
    },
    FailoverQueueManager {
        selected: usize,
    },
    ClaudeModelPicker {
        selected: usize,
        editing: bool,
    },
    ClaudeApiFormatPicker {
        selected: usize,
    },
    UserAgentPicker {
        selected: usize,
    },
    UsageQueryTemplatePicker {
        selected: usize,
    },
    ManagedAccountPicker {
        auth_provider: String,
        selected: usize,
        binding: bool,
        selected_account_id: Option<String>,
    },
    ManagedAccountActionPicker {
        auth_provider: String,
        account_id: String,
        selected: usize,
    },
    HermesModelsPicker {
        editing: bool,
    },
    ModelFetchPicker {
        request_id: u64,
        field: ProviderAddField,
        claude_idx: Option<usize>,
        input: TextInput,
        query: String,
        fetching: bool,
        models: Vec<String>,
        error: Option<String>,
        selected_idx: usize,
    },
    OpenClawToolsProfilePicker {
        selected: Option<usize>,
    },
    OpenClawAgentsFallbackPicker {
        insert_at: usize,
        selected: usize,
        options: Vec<OpenClawModelOption>,
    },
    McpAppsPicker {
        id: String,
        name: String,
        selected: usize,
        apps: crate::app_config::McpApps,
    },
    VisibleAppsPicker {
        selected: usize,
        apps: crate::settings::VisibleApps,
    },
    SkillsAppsPicker {
        directory: String,
        name: String,
        selected: usize,
        apps: crate::app_config::SkillApps,
    },
    SkillsImportPicker {
        skills: Vec<crate::services::skill::UnmanagedSkill>,
        selected_idx: usize,
        selected: HashSet<String>,
    },
    #[allow(dead_code)]
    SkillsSyncMethodPicker {
        selected: usize,
    },
    McpEnvPicker {
        selected: usize,
    },
    McpTypePicker {
        selected: usize,
    },
    McpEnvEntryEditor(McpEnvEntryEditorState),
    Loading {
        kind: LoadingKind,
        title: String,
        message: String,
    },
    SpeedtestRunning {
        url: String,
    },
    SpeedtestResult {
        url: String,
        lines: Vec<String>,
        scroll: usize,
    },
    StreamCheckRunning {
        provider_id: String,
        provider_name: String,
    },
    StreamCheckResult {
        provider_name: String,
        lines: Vec<String>,
        scroll: usize,
    },
    UpdateAvailable {
        current: String,
        latest: String,
        selected: usize,
    },
    UpdateDownloading {
        downloaded: u64,
        total: Option<u64>,
    },
    UpdateResult {
        success: bool,
        message: String,
    },
}

impl Overlay {
    pub fn is_active(&self) -> bool {
        !matches!(self, Overlay::None)
    }

    pub fn can_be_covered_by_help(&self) -> bool {
        matches!(
            self,
            Overlay::BackupPicker { .. }
                | Overlay::TextView(_)
                | Overlay::CommonSnippetPicker { .. }
                | Overlay::ProviderTestMenu { .. }
                | Overlay::FailoverQueueManager { .. }
                | Overlay::ClaudeApiFormatPicker { .. }
                | Overlay::UserAgentPicker { .. }
                | Overlay::UsageQueryTemplatePicker { .. }
                | Overlay::ManagedAccountPicker { .. }
                | Overlay::ManagedAccountActionPicker { .. }
                | Overlay::ClaudeModelPicker { editing: false, .. }
                | Overlay::HermesModelsPicker { editing: false }
                | Overlay::OpenClawToolsProfilePicker { .. }
                | Overlay::OpenClawAgentsFallbackPicker { .. }
                | Overlay::McpAppsPicker { .. }
                | Overlay::VisibleAppsPicker { .. }
                | Overlay::SkillsAppsPicker { .. }
                | Overlay::SkillsImportPicker { .. }
                | Overlay::SkillsSyncMethodPicker { .. }
                | Overlay::McpEnvPicker { .. }
                | Overlay::McpTypePicker { .. }
                | Overlay::SpeedtestResult { .. }
                | Overlay::StreamCheckResult { .. }
                | Overlay::UpdateAvailable { .. }
                | Overlay::UpdateResult { .. }
        )
    }

    /// Whether this overlay is actively accepting text input.
    /// This controls whether the main UI should consider itself in "editing mode" and e.g. respond to vim-style navigation.
    pub fn is_editing(&self) -> bool {
        match self {
            Overlay::TextInput(input) => input.is_editing(),
            Overlay::ClaudeModelPicker { editing, .. } => *editing,
            Overlay::HermesModelsPicker { editing } => *editing,
            Overlay::ModelFetchPicker { .. } => true,
            Overlay::McpEnvEntryEditor(editor) => editor.is_editing(),
            Overlay::None
            | Overlay::Help(_)
            | Overlay::Confirm(_)
            | Overlay::BackupPicker { .. }
            | Overlay::TextView(_)
            | Overlay::CommonSnippetPicker { .. }
            | Overlay::ProviderTestMenu { .. }
            | Overlay::FailoverQueueManager { .. }
            | Overlay::ClaudeApiFormatPicker { .. }
            | Overlay::UserAgentPicker { .. }
            | Overlay::UsageQueryTemplatePicker { .. }
            | Overlay::ManagedAccountPicker { .. }
            | Overlay::ManagedAccountActionPicker { .. }
            | Overlay::OpenClawToolsProfilePicker { .. }
            | Overlay::OpenClawAgentsFallbackPicker { .. }
            | Overlay::McpAppsPicker { .. }
            | Overlay::VisibleAppsPicker { .. }
            | Overlay::SkillsAppsPicker { .. }
            | Overlay::SkillsImportPicker { .. }
            | Overlay::SkillsSyncMethodPicker { .. }
            | Overlay::McpEnvPicker { .. }
            | Overlay::McpTypePicker { .. }
            | Overlay::Loading { .. }
            | Overlay::SpeedtestRunning { .. }
            | Overlay::SpeedtestResult { .. }
            | Overlay::StreamCheckRunning { .. }
            | Overlay::StreamCheckResult { .. }
            | Overlay::UpdateAvailable { .. }
            | Overlay::UpdateDownloading { .. }
            | Overlay::UpdateResult { .. } => false,
        }
    }
}

#[cfg(test)]
mod sessions_state_tests {
    use super::SessionsState;
    use crate::session_manager::SessionMeta;

    fn meta(provider: &str, session: &str, last_active: i64) -> SessionMeta {
        SessionMeta {
            provider_id: provider.to_string(),
            session_id: session.to_string(),
            last_active_at: Some(last_active),
            ..SessionMeta::default()
        }
    }

    /// 渐进回传：partial 只替换对应 provider 的行、保留其他 provider 的行，
    /// 结果按最近活跃排序；stale request id 被忽略。
    #[test]
    fn apply_partial_scan_replaces_only_that_provider() {
        let mut state = SessionsState::default();
        let request_id = state.start_scan("all".to_string());
        state.rows = vec![meta("claude", "c-old", 10), meta("codex", "x-1", 30)];

        assert!(state.apply_partial_scan(request_id, "claude", vec![meta("claude", "c-new", 20)],));
        let ids: Vec<&str> = state.rows.iter().map(|r| r.session_id.as_str()).collect();
        assert_eq!(ids, vec!["x-1", "c-new"]);
        assert!(state.loading, "refresh indicator must stay on");

        // stale request id：不应用
        assert!(!state.apply_partial_scan(request_id + 1, "codex", vec![]));
        assert_eq!(state.rows.len(), 2);
    }

    /// 删除与在途扫描竞态：删除成功时登记 tombstone，之后到达的（删除前读到旧
    /// 列表的）partial/finished 不得把已删会话放回；finish_scan 终态清空 tombstone，
    /// 下一轮扫描的同名行不再被过滤。
    #[test]
    fn scan_tombstone_blocks_deleted_session_from_inflight_scan() {
        let mut state = SessionsState::default();
        let request_id = state.start_scan("all".to_string());
        let a = meta("claude", "a", 10);
        let b = meta("codex", "b", 20);
        state.rows = vec![a.clone(), b.clone()];

        // 模拟"在途扫描期间删除 A"：从 rows 移除并登记 tombstone。键与删除流程
        // 一致（session_key = provider:session:source_path）。
        let key_a = crate::cli::tui::app::session_key(&a);
        state.rows.retain(|row| row.session_id != "a");
        state.scan_tombstones.insert(key_a);

        // 删除前读到旧列表的 partial 把 A 带回 —— 必须被过滤掉。
        assert!(state.apply_partial_scan(request_id, "claude", vec![a.clone()]));
        assert!(
            state.rows.iter().all(|row| row.session_id != "a"),
            "partial 不得复活已删会话"
        );

        // 终态同样带回 A（扫描线程在删除前就读完文件）—— 仍过滤，并清空 tombstone。
        assert!(state.finish_scan(request_id, vec![a.clone(), b.clone()]));
        assert!(
            state.rows.iter().all(|row| row.session_id != "a"),
            "finish_scan 不得复活已删会话"
        );
        assert!(
            state.scan_tombstones.is_empty(),
            "finish_scan 终态应清空 tombstones"
        );

        // 下一轮扫描：tombstone 已清，同 key 的行（例如同名会话被重建）应保留。
        let request_id2 = state.start_scan("all".to_string());
        assert!(state.finish_scan(request_id2, vec![a.clone(), b.clone()]));
        assert!(
            state.rows.iter().any(|row| row.session_id == "a"),
            "tombstone 已清，重新出现的行不应再被过滤"
        );
    }

    /// 修复 2：fail_scan 终态清空 tombstones。扫描进行中删除会话会登记 tombstone，
    /// 若该扫描以失败告终（panic/Err），旧代码只在 finish_scan 清 tombstone、
    /// fail_scan 不清，残留的 tombstone 会把下一轮同 key 的新建/恢复会话误过滤
    /// 一次。fail_scan 现与 finish_scan 一致地在终态清空。
    #[test]
    fn fail_scan_clears_tombstones() {
        let mut state = SessionsState::default();
        let request_id = state.start_scan("all".to_string());
        let a = meta("claude", "a", 10);
        state
            .scan_tombstones
            .insert(crate::cli::tui::app::session_key(&a));

        // 匹配 request_id 的失败终态应清空 tombstones
        state.fail_scan(request_id, "boom".to_string());
        assert!(
            state.scan_tombstones.is_empty(),
            "fail_scan 终态应清空 tombstones"
        );

        // 下一轮扫描带回同 key 的行（同名会话被重建）—— tombstone 已清，不应再过滤
        let request_id2 = state.start_scan("all".to_string());
        assert!(state.finish_scan(request_id2, vec![a.clone()]));
        assert!(
            state.rows.iter().any(|row| row.session_id == "a"),
            "tombstone 已清，重新出现的行不应再被过滤"
        );
    }

    /// 秒开快照同样按 tombstone 过滤：删除成功后即使 stale 快照带回已删会话，也
    /// 不渲染。
    #[test]
    fn scan_tombstone_filters_cached_snapshot() {
        let mut state = SessionsState::default();
        let request_id = state.start_scan("all".to_string());
        let a = meta("claude", "a", 10);
        let b = meta("codex", "b", 20);
        state
            .scan_tombstones
            .insert(crate::cli::tui::app::session_key(&a));

        assert!(state.apply_cached_snapshot(request_id, vec![a, b]));
        let ids: Vec<&str> = state.rows.iter().map(|r| r.session_id.as_str()).collect();
        assert_eq!(ids, vec!["b"], "快照应过滤掉 tombstoned 会话");
    }
}
