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
        }
    }

    pub(crate) fn finish_scan(
        &mut self,
        request_id: u64,
        rows: Vec<crate::session_manager::SessionMeta>,
    ) -> bool {
        if self.scan_active != Some(request_id) {
            return false;
        }
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
    ProviderSwitchLiveConflicts {
        provider_id: String,
        conflicts: Vec<crate::services::provider::live_merge::ConfigConflict>,
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
                | Overlay::ProviderSwitchLiveConflicts { .. }
                | Overlay::FailoverQueueManager { .. }
                | Overlay::ClaudeApiFormatPicker { .. }
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
            | Overlay::ProviderSwitchLiveConflicts { .. }
            | Overlay::FailoverQueueManager { .. }
            | Overlay::ClaudeApiFormatPicker { .. }
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
