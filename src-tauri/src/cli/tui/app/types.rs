use super::*;

#[derive(Debug, Clone)]
pub struct FilterState {
    pub active: bool,
    pub buffer: String,
}

impl FilterState {
    pub fn new() -> Self {
        Self {
            active: false,
            buffer: String::new(),
        }
    }

    pub fn query_lower(&self) -> Option<String> {
        let trimmed = self.buffer.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(trimmed.to_lowercase())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Nav,
    Content,
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
}

impl Toast {
    pub fn new(message: impl Into<String>, kind: ToastKind) -> Self {
        Self {
            message: message.into(),
            kind,
            remaining_ticks: 12,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    Quit,
    ProviderDelete { id: String },
    McpDelete { id: String },
    PromptDelete { id: String },
    SkillsUninstall { directory: String },
    SkillsRepoRemove { owner: String, name: String },
    ConfigImport { path: String },
    ConfigRestoreBackup { id: String },
    ConfigReset,
    SettingsSetSkipClaudeOnboarding { enabled: bool },
    SettingsSetClaudePluginIntegration { enabled: bool },
    ProviderApiFormatProxyNotice,
    ProviderSwitchSharedConfigNotice,
    OpenClawDailyMemoryDelete { filename: String },
    EditorDiscard,
    EditorSaveBeforeClose,
    WebDavMigrateV1ToV2,
}

#[derive(Debug, Clone)]
pub struct ConfirmOverlay {
    pub title: String,
    pub message: String,
    pub action: ConfirmAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextSubmit {
    ConfigExport,
    ConfigImport,
    ConfigBackupName,
    SettingsProxyListenAddress,
    SettingsProxyListenPort,
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
    WebDavJianguoyunUsername,
    WebDavJianguoyunPassword,
}

#[derive(Debug, Clone)]
pub struct TextInputState {
    pub title: String,
    pub prompt: String,
    pub buffer: String,
    pub submit: TextSubmit,
    pub secret: bool,
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
    ProxyToggleTakeover { app_type: AppType, enabled: bool },
}

impl TextViewAction {
    pub fn key_label(&self) -> &'static str {
        match self {
            TextViewAction::ProxyToggleTakeover { enabled: true, .. } => texts::tui_key_takeover(),
            TextViewAction::ProxyToggleTakeover { enabled: false, .. } => texts::tui_key_restore(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadingKind {
    Generic,
    Proxy,
    WebDav,
    UpdateCheck,
}

#[derive(Debug, Clone)]
pub enum Overlay {
    None,
    Help,
    Confirm(ConfirmOverlay),
    ProviderSwitchFirstUseConfirm {
        provider_id: String,
        title: String,
        message: String,
        selected: usize,
    },
    TextInput(TextInputState),
    BackupPicker {
        selected: usize,
    },
    TextView(TextViewState),
    CommonSnippetPicker {
        selected: usize,
    },
    CommonSnippetView {
        app_type: AppType,
        view: TextViewState,
    },
    ClaudeModelPicker {
        selected: usize,
        editing: bool,
    },
    ClaudeApiFormatPicker {
        selected: usize,
    },
    ModelFetchPicker {
        request_id: u64,
        field: ProviderAddField,
        claude_idx: Option<usize>,
        input: String,
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
    SkillsSyncMethodPicker {
        selected: usize,
    },
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
}
