use super::*;

#[derive(Debug, Clone)]
pub enum Action {
    None,
    ReloadData,
    SwitchRoute(Route),
    Quit,
    SetAppType(AppType),
    LocalEnvRefresh,

    SkillsToggle {
        directory: String,
        enabled: bool,
    },
    SkillsSetApps {
        directory: String,
        apps: crate::app_config::SkillApps,
    },
    SkillsInstall {
        spec: String,
    },
    SkillsUninstall {
        directory: String,
    },
    SkillsSync {
        app: Option<AppType>,
    },
    SkillsSetSyncMethod {
        method: SyncMethod,
    },
    SkillsDiscover {
        query: String,
    },
    SkillsRepoAdd {
        spec: String,
    },
    SkillsRepoRemove {
        owner: String,
        name: String,
    },
    SkillsRepoToggleEnabled {
        owner: String,
        name: String,
        enabled: bool,
    },
    SkillsOpenImport,
    SkillsScanUnmanaged,
    SkillsImportFromApps {
        directories: Vec<String>,
    },

    ProviderSwitch {
        id: String,
    },
    ProviderRemoveFromConfig {
        id: String,
    },
    ProviderSetDefaultModel {
        provider_id: String,
        model_id: String,
    },
    ProviderSwitchForce {
        id: String,
    },
    ProviderImportLiveConfig,
    ProviderDelete {
        id: String,
    },
    ProviderSpeedtest {
        url: String,
    },
    ProviderLaunchTemporary {
        id: String,
    },
    ProviderStreamCheck {
        id: String,
    },
    ProviderQuotaRefresh {
        id: String,
    },
    ProviderModelFetch {
        base_url: String,
        api_key: Option<String>,
        field: ProviderAddField,
        claude_idx: Option<usize>,
    },

    McpToggle {
        id: String,
        enabled: bool,
    },
    McpSetApps {
        id: String,
        apps: crate::app_config::McpApps,
    },
    McpDelete {
        id: String,
    },
    McpImport,

    PromptActivate {
        id: String,
    },
    PromptDeactivate {
        id: String,
    },
    PromptDelete {
        id: String,
    },

    ConfigExport {
        path: String,
    },
    ConfigImport {
        path: String,
    },
    ConfigBackup {
        name: Option<String>,
    },
    ConfigRestoreBackup {
        id: String,
    },
    ConfigShowFull,
    ConfigValidate,
    ConfigOpenProxyHelp,
    ConfigCommonSnippetClear {
        app_type: AppType,
    },
    ConfigCommonSnippetApply {
        app_type: AppType,
    },
    ConfigWebDavCheckConnection,
    ConfigWebDavUpload,
    ConfigWebDavDownload,
    ConfigWebDavMigrateV1ToV2,
    ConfigWebDavReset,
    ConfigWebDavJianguoyunQuickSetup {
        username: String,
        password: String,
    },
    OpenClawWorkspaceOpenFile {
        filename: String,
    },
    OpenClawDailyMemoryOpenFile {
        filename: String,
    },
    OpenClawDailyMemorySearch {
        query: String,
    },
    OpenClawDailyMemoryDelete {
        filename: String,
    },
    OpenClawOpenDirectory {
        subdir: String,
    },
    ConfigReset,

    EditorSubmit {
        submit: EditorSubmit,
        content: String,
    },
    EditorDiscard,
    EditorOpenExternal,

    SetSkipClaudeOnboarding {
        enabled: bool,
    },
    SetClaudePluginIntegration {
        enabled: bool,
    },
    SetProxyEnabled {
        enabled: bool,
    },
    SetProxyListenAddress {
        address: String,
    },
    SetProxyListenPort {
        port: u16,
    },
    SetOpenClawConfigDir {
        path: Option<String>,
    },
    SetProxyTakeover {
        app_type: AppType,
        enabled: bool,
    },
    SetManagedProxyForCurrentApp {
        app_type: AppType,
        enabled: bool,
    },
    SetLanguage(Language),
    SetVisibleApps {
        apps: crate::settings::VisibleApps,
    },

    CheckUpdate,
    ConfirmUpdate,
    CancelUpdate,
    CancelUpdateCheck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigItem {
    Path,
    ShowFull,
    Export,
    Import,
    Backup,
    Restore,
    Validate,
    CommonSnippet,
    Proxy,
    OpenClawWorkspace,
    OpenClawEnv,
    OpenClawTools,
    OpenClawAgents,
    WebDavSync,
    Reset,
}

#[derive(Debug, Clone)]
pub(crate) struct ConfigItemMetadata {
    pub label: &'static str,
    pub detail_title: Option<&'static str>,
    pub detail_route: Option<Route>,
    pub openclaw_only: bool,
}

fn config_item_metadata(label: &'static str) -> ConfigItemMetadata {
    ConfigItemMetadata {
        label,
        detail_title: None,
        detail_route: None,
        openclaw_only: false,
    }
}

fn openclaw_config_item_metadata(
    label: &'static str,
    detail_title: &'static str,
    detail_route: Route,
) -> ConfigItemMetadata {
    ConfigItemMetadata {
        label,
        detail_title: Some(detail_title),
        detail_route: Some(detail_route),
        openclaw_only: true,
    }
}

impl ConfigItem {
    pub const ALL: [ConfigItem; 14] = [
        ConfigItem::Path,
        ConfigItem::ShowFull,
        ConfigItem::Export,
        ConfigItem::Import,
        ConfigItem::Backup,
        ConfigItem::Restore,
        ConfigItem::Validate,
        ConfigItem::CommonSnippet,
        ConfigItem::OpenClawWorkspace,
        ConfigItem::OpenClawEnv,
        ConfigItem::OpenClawTools,
        ConfigItem::OpenClawAgents,
        ConfigItem::WebDavSync,
        ConfigItem::Reset,
    ];

    pub(crate) fn metadata(&self) -> ConfigItemMetadata {
        match self {
            ConfigItem::Path => config_item_metadata(texts::tui_config_item_show_path()),
            ConfigItem::ShowFull => config_item_metadata(texts::tui_config_item_show_full()),
            ConfigItem::Export => config_item_metadata(texts::tui_config_item_export()),
            ConfigItem::Import => config_item_metadata(texts::tui_config_item_import()),
            ConfigItem::Backup => config_item_metadata(texts::tui_config_item_backup()),
            ConfigItem::Restore => config_item_metadata(texts::tui_config_item_restore()),
            ConfigItem::Validate => config_item_metadata(texts::tui_config_item_validate()),
            ConfigItem::CommonSnippet => {
                config_item_metadata(texts::tui_config_item_common_snippet())
            }
            ConfigItem::Proxy => config_item_metadata(texts::tui_config_item_proxy()),
            ConfigItem::OpenClawWorkspace => openclaw_config_item_metadata(
                texts::tui_config_item_openclaw_workspace(),
                texts::tui_openclaw_workspace_title(),
                Route::ConfigOpenClawWorkspace,
            ),
            ConfigItem::OpenClawEnv => openclaw_config_item_metadata(
                texts::tui_config_item_openclaw_env(),
                texts::tui_openclaw_config_env_title(),
                Route::ConfigOpenClawEnv,
            ),
            ConfigItem::OpenClawTools => openclaw_config_item_metadata(
                texts::tui_config_item_openclaw_tools(),
                texts::tui_openclaw_config_tools_title(),
                Route::ConfigOpenClawTools,
            ),
            ConfigItem::OpenClawAgents => openclaw_config_item_metadata(
                texts::tui_config_item_openclaw_agents(),
                texts::tui_openclaw_config_agents_title(),
                Route::ConfigOpenClawAgents,
            ),
            ConfigItem::WebDavSync => config_item_metadata(texts::tui_config_item_webdav_sync()),
            ConfigItem::Reset => config_item_metadata(texts::tui_config_item_reset()),
        }
    }

    pub(crate) fn visible_for_app(&self, app_type: &AppType) -> bool {
        !self.metadata().openclaw_only || matches!(app_type, AppType::OpenClaw)
    }

    pub(crate) fn listed_in_config_menu(&self, app_type: &AppType) -> bool {
        self.visible_for_app(app_type)
            && !matches!(
                self,
                ConfigItem::OpenClawWorkspace
                    | ConfigItem::OpenClawEnv
                    | ConfigItem::OpenClawTools
                    | ConfigItem::OpenClawAgents
            )
    }

    pub(crate) fn label(&self) -> &'static str {
        self.metadata().label
    }

    pub(crate) fn detail_title(&self) -> Option<&'static str> {
        self.metadata().detail_title
    }

    pub(crate) fn detail_route(&self) -> Option<Route> {
        self.metadata().detail_route
    }

    pub(crate) fn from_openclaw_route(route: &Route) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|item| item.detail_route().as_ref() == Some(route))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsItem {
    Language,
    VisibleApps,
    OpenClawConfigDir,
    SkipClaudeOnboarding,
    ClaudePluginIntegration,
    Proxy,
    CheckForUpdates,
}

impl SettingsItem {
    pub const ALL: [SettingsItem; 7] = [
        SettingsItem::Language,
        SettingsItem::VisibleApps,
        SettingsItem::OpenClawConfigDir,
        SettingsItem::SkipClaudeOnboarding,
        SettingsItem::ClaudePluginIntegration,
        SettingsItem::Proxy,
        SettingsItem::CheckForUpdates,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalProxySettingsItem {
    ListenAddress,
    ListenPort,
}

impl LocalProxySettingsItem {
    pub const ALL: [LocalProxySettingsItem; 2] = [
        LocalProxySettingsItem::ListenAddress,
        LocalProxySettingsItem::ListenPort,
    ];
}

#[derive(Debug, Clone)]
pub enum WebDavConfigItem {
    Settings,
    CheckConnection,
    Upload,
    Download,
    Reset,
    JianguoyunQuickSetup,
}

impl WebDavConfigItem {
    pub const ALL: [WebDavConfigItem; 6] = [
        WebDavConfigItem::Settings,
        WebDavConfigItem::CheckConnection,
        WebDavConfigItem::Upload,
        WebDavConfigItem::Download,
        WebDavConfigItem::Reset,
        WebDavConfigItem::JianguoyunQuickSetup,
    ];

    pub(crate) fn label(&self) -> &'static str {
        match self {
            WebDavConfigItem::Settings => texts::tui_config_item_webdav_settings(),
            WebDavConfigItem::CheckConnection => texts::tui_config_item_webdav_check_connection(),
            WebDavConfigItem::Upload => texts::tui_config_item_webdav_upload(),
            WebDavConfigItem::Download => texts::tui_config_item_webdav_download(),
            WebDavConfigItem::Reset => texts::tui_config_item_webdav_reset(),
            WebDavConfigItem::JianguoyunQuickSetup => {
                texts::tui_config_item_webdav_jianguoyun_quick_setup()
            }
        }
    }
}

pub(crate) const PROXY_HERO_TRANSITION_TICKS: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxyVisualTransition {
    pub from_on: bool,
    pub to_on: bool,
    pub started_tick: u64,
}

#[derive(Debug, Clone)]
pub struct App {
    pub app_type: AppType,
    pub route: Route,
    pub route_stack: Vec<Route>,
    pub focus: Focus,
    pub nav_idx: usize,

    pub filter: FilterState,
    pub editor: Option<EditorState>,
    pub form: Option<FormState>,
    pub pending_overlay: Option<Overlay>,
    pub overlay: Overlay,
    pub toast: Option<Toast>,
    pub should_quit: bool,
    pub last_size: Size,
    pub tick: u64,
    pub proxy_input_activity_samples: Vec<u64>,
    pub proxy_output_activity_samples: Vec<u64>,
    pub proxy_activity_last_input_tokens: Option<u64>,
    pub proxy_activity_last_output_tokens: Option<u64>,
    pub proxy_visual_state: Option<bool>,
    pub proxy_visual_transition: Option<ProxyVisualTransition>,
    pub quota_auto_target_key: Option<String>,
    pub quota_last_auto_tick: Option<u64>,

    pub local_env_results: Vec<crate::services::local_env_check::ToolCheckResult>,
    pub local_env_loading: bool,

    pub provider_idx: usize,
    pub mcp_idx: usize,
    pub prompt_idx: usize,
    pub skills_idx: usize,
    pub skills_discover_idx: usize,
    pub skills_repo_idx: usize,
    pub skills_unmanaged_idx: usize,
    pub skills_discover_results: Vec<crate::services::skill::Skill>,
    pub skills_discover_query: String,
    pub skills_unmanaged_results: Vec<crate::services::skill::UnmanagedSkill>,
    pub skills_unmanaged_selected: HashSet<String>,
    pub config_idx: usize,
    pub workspace_idx: usize,
    pub daily_memory_idx: usize,
    pub openclaw_tools_form: Option<OpenClawToolsFormState>,
    pub openclaw_agents_form: Option<OpenClawAgentsFormState>,
    pub openclaw_daily_memory_search_query: String,
    pub openclaw_daily_memory_search_results:
        Vec<crate::commands::workspace::DailyMemorySearchResult>,
    pub config_webdav_idx: usize,
    pub webdav_quick_setup_username: Option<String>,
    pub language_idx: usize,
    pub settings_idx: usize,
    pub settings_proxy_idx: usize,
}
