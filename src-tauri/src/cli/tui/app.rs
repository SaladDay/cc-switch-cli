use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::Size;
use std::collections::HashSet;
use unicode_width::UnicodeWidthChar;

use crate::app_config::AppType;
use crate::cli::i18n::current_language;
use crate::cli::i18n::texts;
use crate::cli::i18n::Language;
use crate::services::skill::SyncMethod;

use super::data::UiData;
use super::form::{
    CodexWireApi, FormFocus, FormMode, FormState, GeminiAuthType, McpAddField, McpAddFormState,
    ProviderAddField, ProviderAddFormState,
};
use super::route::{NavItem, Route};

const PROVIDER_NOTES_MAX_CHARS: usize = 120;

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
    EditorDiscard,
    EditorSaveBeforeClose,
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
    McpValidateCommand,
    SkillsInstallSpec,
    SkillsDiscoverQuery,
    SkillsRepoAdd,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadingKind {
    Generic,
    WebDav,
    UpdateCheck,
}

#[derive(Debug, Clone)]
pub enum Overlay {
    None,
    Help,
    Confirm(ConfirmOverlay),
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
    McpAppsPicker {
        id: String,
        name: String,
        selected: usize,
        apps: crate::app_config::McpApps,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorKind {
    Plain,
    Json,
}

#[derive(Debug, Clone)]
pub enum EditorSubmit {
    PromptEdit { id: String },
    ProviderFormApplyJson,
    ProviderFormApplyCodexAuth,
    ProviderFormApplyCodexConfigToml,
    ProviderAdd,
    ProviderEdit { id: String },
    McpAdd,
    McpEdit { id: String },
    ConfigCommonSnippet { app_type: AppType },
    ConfigWebDavSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorMode {
    Edit,
}

#[derive(Debug, Clone)]
pub struct EditorState {
    pub title: String,
    pub kind: EditorKind,
    pub submit: EditorSubmit,
    pub mode: EditorMode,
    pub lines: Vec<String>,
    pub scroll: usize,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub initial_text: String,
}

impl EditorState {
    pub fn new(
        title: impl Into<String>,
        kind: EditorKind,
        submit: EditorSubmit,
        initial: impl Into<String>,
    ) -> Self {
        let initial_text = initial.into();
        let mut lines = initial_text
            .lines()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        if lines.is_empty() {
            lines.push(String::new());
        }

        Self {
            title: title.into(),
            kind,
            submit,
            mode: EditorMode::Edit,
            lines,
            scroll: 0,
            cursor_row: 0,
            cursor_col: 0,
            initial_text,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.text().trim_end() != self.initial_text.trim_end()
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    fn line_len_chars(&self, row: usize) -> usize {
        self.lines.get(row).map(|s| s.chars().count()).unwrap_or(0)
    }

    pub(super) fn wrap_line_segments(line: &str, width: u16) -> Vec<String> {
        let width = width as usize;
        if width == 0 {
            return vec![String::new()];
        }

        let mut segments = Vec::new();
        let mut current = String::new();
        let mut current_width = 0usize;

        for ch in line.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
            if current_width.saturating_add(ch_width) > width && !current.is_empty() {
                segments.push(current);
                current = String::new();
                current_width = 0;
            }

            current.push(ch);
            current_width = current_width.saturating_add(ch_width);
        }

        if segments.is_empty() {
            segments.push(current);
        } else {
            segments.push(current);
        }

        segments
    }

    fn wrapped_line_height(line: &str, width: u16) -> usize {
        Self::wrap_line_segments(line, width).len().max(1)
    }

    fn wrapped_cursor_subline_and_x(line: &str, width: u16, cursor_col: usize) -> (usize, u16) {
        let width = width as usize;
        if width == 0 {
            return (0, 0);
        }

        let mut subline = 0usize;
        let mut current_width = 0usize;
        let mut col = 0usize;

        for ch in line.chars() {
            if col >= cursor_col {
                break;
            }

            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
            if current_width.saturating_add(ch_width) > width && current_width > 0 {
                subline = subline.saturating_add(1);
                current_width = 0;
            }

            current_width = current_width.saturating_add(ch_width);
            col = col.saturating_add(1);
        }

        let x = current_width.min(width.saturating_sub(1)) as u16;
        (subline, x)
    }

    pub(super) fn cursor_visual_offset_from_scroll(&self, width: u16) -> (usize, u16) {
        if self.lines.is_empty() {
            return (0, 0);
        }

        let cursor_row = self.cursor_row.min(self.lines.len().saturating_sub(1));
        let scroll = self
            .scroll
            .min(self.lines.len().saturating_sub(1))
            .min(cursor_row);

        let mut y = 0usize;
        for row in scroll..cursor_row {
            y = y.saturating_add(Self::wrapped_line_height(&self.lines[row], width));
        }

        let cursor_col = self.cursor_col.min(self.line_len_chars(cursor_row));
        let (subline, x) =
            Self::wrapped_cursor_subline_and_x(&self.lines[cursor_row], width, cursor_col);
        (y.saturating_add(subline), x)
    }

    fn ensure_cursor_visible(&mut self, viewport: Size) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);
        self.cursor_col = self.cursor_col.min(self.line_len_chars(self.cursor_row));

        if !self.lines.is_empty() {
            self.scroll = self.scroll.min(self.lines.len() - 1);
        } else {
            self.scroll = 0;
        }

        if self.cursor_row < self.scroll {
            self.scroll = self.cursor_row;
        }

        let height = viewport.height as usize;
        if height == 0 {
            return;
        }

        let width = viewport.width.max(1);

        let (mut cursor_y, _) = self.cursor_visual_offset_from_scroll(width);
        while cursor_y >= height && self.scroll < self.cursor_row {
            let removed = Self::wrapped_line_height(&self.lines[self.scroll], width);
            cursor_y = cursor_y.saturating_sub(removed);
            self.scroll = self.scroll.saturating_add(1);
        }
    }

    fn byte_index(line: &str, col: usize) -> usize {
        line.char_indices()
            .nth(col)
            .map(|(i, _)| i)
            .unwrap_or(line.len())
    }

    fn insert_char(&mut self, c: char) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);
        let line = &mut self.lines[self.cursor_row];
        let idx = Self::byte_index(line, self.cursor_col);
        line.insert(idx, c);
        self.cursor_col += 1;
    }

    fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.insert_char(c);
        }
    }

    fn newline(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);
        let line = &mut self.lines[self.cursor_row];
        let idx = Self::byte_index(line, self.cursor_col);
        let rest = line.split_off(idx);
        let next_row = self.cursor_row + 1;
        self.lines.insert(next_row, rest);
        self.cursor_row = next_row;
        self.cursor_col = 0;
    }

    fn backspace(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);

        if self.cursor_col > 0 {
            let line = &mut self.lines[self.cursor_row];
            let start = Self::byte_index(line, self.cursor_col.saturating_sub(1));
            let end = Self::byte_index(line, self.cursor_col);
            if start < end && end <= line.len() {
                line.replace_range(start..end, "");
                self.cursor_col -= 1;
            }
            return;
        }

        if self.cursor_row == 0 {
            return;
        }

        let current = self.lines.remove(self.cursor_row);
        self.cursor_row -= 1;
        let prev = &mut self.lines[self.cursor_row];
        self.cursor_col = prev.chars().count();
        prev.push_str(&current);
    }

    fn delete(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);

        let line_len = self.line_len_chars(self.cursor_row);
        if self.cursor_col < line_len {
            let line = &mut self.lines[self.cursor_row];
            let start = Self::byte_index(line, self.cursor_col);
            let end = Self::byte_index(line, self.cursor_col + 1);
            if start < end && end <= line.len() {
                line.replace_range(start..end, "");
            }
            return;
        }

        if self.cursor_row + 1 >= self.lines.len() {
            return;
        }

        let next = self.lines.remove(self.cursor_row + 1);
        self.lines[self.cursor_row].push_str(&next);
    }
}

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
    SkillsScanUnmanaged,
    SkillsImportFromApps {
        directories: Vec<String>,
    },

    ProviderSwitch {
        id: String,
    },
    ProviderDelete {
        id: String,
    },
    ProviderSpeedtest {
        url: String,
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
    McpValidate {
        command: String,
    },

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
    ConfigCommonSnippetClear {
        app_type: AppType,
    },
    ConfigCommonSnippetApply {
        app_type: AppType,
    },
    ConfigWebDavCheckConnection,
    ConfigWebDavUpload,
    ConfigWebDavDownload,
    ConfigWebDavReset,
    ConfigWebDavJianguoyunQuickSetup {
        username: String,
        password: String,
    },
    ConfigReset,

    EditorSubmit {
        submit: EditorSubmit,
        content: String,
    },
    EditorDiscard,

    SetSkipClaudeOnboarding {
        enabled: bool,
    },
    SetLanguage(Language),

    CheckUpdate,
    ConfirmUpdate,
    CancelUpdate,
    CancelUpdateCheck,
}

#[derive(Debug, Clone)]
pub enum ConfigItem {
    Path,
    ShowFull,
    Export,
    Import,
    Backup,
    Restore,
    Validate,
    CommonSnippet,
    WebDavSync,
    Reset,
}

impl ConfigItem {
    pub const ALL: [ConfigItem; 10] = [
        ConfigItem::Path,
        ConfigItem::ShowFull,
        ConfigItem::Export,
        ConfigItem::Import,
        ConfigItem::Backup,
        ConfigItem::Restore,
        ConfigItem::Validate,
        ConfigItem::CommonSnippet,
        ConfigItem::WebDavSync,
        ConfigItem::Reset,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsItem {
    Language,
    SkipClaudeOnboarding,
    CheckForUpdates,
}

impl SettingsItem {
    pub const ALL: [SettingsItem; 3] = [
        SettingsItem::Language,
        SettingsItem::SkipClaudeOnboarding,
        SettingsItem::CheckForUpdates,
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
    pub overlay: Overlay,
    pub toast: Option<Toast>,
    pub should_quit: bool,
    pub last_size: Size,
    pub tick: u64,

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
    pub config_webdav_idx: usize,
    pub webdav_quick_setup_username: Option<String>,
    pub language_idx: usize,
    pub settings_idx: usize,
}

impl App {
    pub fn new(app_override: Option<AppType>) -> Self {
        let app_type = app_override.unwrap_or(AppType::Claude);
        Self {
            app_type,
            route: Route::Main,
            route_stack: Vec::new(),
            focus: Focus::Nav,
            nav_idx: 0,
            filter: FilterState::new(),
            editor: None,
            form: None,
            overlay: Overlay::None,
            toast: None,
            should_quit: false,
            last_size: Size::new(0, 0),
            tick: 0,
            local_env_results: Vec::new(),
            local_env_loading: true,
            provider_idx: 0,
            mcp_idx: 0,
            prompt_idx: 0,
            skills_idx: 0,
            skills_discover_idx: 0,
            skills_repo_idx: 0,
            skills_unmanaged_idx: 0,
            skills_discover_results: Vec::new(),
            skills_discover_query: String::new(),
            skills_unmanaged_results: Vec::new(),
            skills_unmanaged_selected: HashSet::new(),
            config_idx: 0,
            config_webdav_idx: 0,
            webdav_quick_setup_username: None,
            language_idx: 0,
            settings_idx: 0,
        }
    }

    pub fn nav_item(&self) -> NavItem {
        NavItem::ALL
            .get(self.nav_idx)
            .copied()
            .unwrap_or(NavItem::Main)
    }

    fn nav_item_for_route(route: &Route) -> NavItem {
        match route {
            Route::Main => NavItem::Main,
            Route::Providers | Route::ProviderDetail { .. } => NavItem::Providers,
            Route::Mcp => NavItem::Mcp,
            Route::Prompts => NavItem::Prompts,
            Route::Config | Route::ConfigWebDav => NavItem::Config,
            Route::Skills
            | Route::SkillsDiscover
            | Route::SkillsRepos
            | Route::SkillsUnmanaged
            | Route::SkillDetail { .. } => NavItem::Skills,
            Route::Settings => NavItem::Settings,
        }
    }

    fn set_route_no_history(&mut self, route: Route) -> Action {
        if route == self.route {
            return Action::None;
        }

        self.route = route.clone();
        self.focus = route_default_focus(&route);

        let nav_item = Self::nav_item_for_route(&route);
        if let Some(idx) = NavItem::ALL.iter().position(|item| *item == nav_item) {
            self.nav_idx = idx;
        }

        if matches!(route, Route::Main) {
            self.route_stack.clear();
            self.focus = Focus::Nav;
        }

        Action::SwitchRoute(route)
    }

    fn push_route_and_switch(&mut self, route: Route) -> Action {
        if route == self.route {
            return Action::None;
        }
        self.route_stack.push(self.route.clone());
        self.set_route_no_history(route)
    }

    fn pop_route_and_switch(&mut self) -> Action {
        if let Some(prev) = self.route_stack.pop() {
            self.set_route_no_history(prev)
        } else {
            self.set_route_no_history(Route::Main)
        }
    }

    pub fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        if let Some(toast) = &mut self.toast {
            if toast.remaining_ticks > 0 {
                toast.remaining_ticks -= 1;
            }
            if toast.remaining_ticks == 0 {
                self.toast = None;
            }
        }
    }

    pub fn push_toast(&mut self, message: impl Into<String>, kind: ToastKind) {
        self.toast = Some(Toast::new(message, kind));
    }

    pub fn open_help(&mut self) {
        self.overlay = Overlay::Help;
    }

    pub fn close_overlay(&mut self) {
        self.overlay = Overlay::None;
    }

    pub fn on_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        self.clamp_selections(data);

        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            self.should_quit = true;
            return Action::Quit;
        }

        if self.overlay.is_active() {
            return self.on_overlay_key(key, data);
        }

        if self.editor.is_some() {
            return self.on_editor_key(key);
        }

        if self.form.is_some() {
            return self.on_form_key(key, data);
        }

        if self.filter.active {
            return self.on_filter_key(key);
        }

        // Global actions.
        match key.code {
            KeyCode::Char('?') => {
                self.open_help();
                return Action::None;
            }
            KeyCode::Char('/') => {
                self.filter.active = true;
                return Action::None;
            }
            KeyCode::Char('[') => return Action::SetAppType(cycle_app_type(&self.app_type, -1)),
            KeyCode::Char(']') => return Action::SetAppType(cycle_app_type(&self.app_type, 1)),
            KeyCode::Left => {
                self.focus = Focus::Nav;
                return Action::None;
            }
            KeyCode::Right => {
                if route_has_content_list(&self.route) {
                    self.focus = Focus::Content;
                } else {
                    self.focus = Focus::Nav;
                }
                return Action::None;
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                return self.on_back_key();
            }
            _ => {}
        }

        // Navigation + route-specific actions.
        match self.focus {
            Focus::Nav => self.on_nav_key(key),
            Focus::Content => self.on_content_key(key, data),
        }
    }

    fn on_back_key(&mut self) -> Action {
        match self.route {
            Route::Main => {
                self.overlay = Overlay::Confirm(ConfirmOverlay {
                    title: crate::cli::i18n::texts::tui_confirm_exit_title().to_string(),
                    message: crate::cli::i18n::texts::tui_confirm_exit_message().to_string(),
                    action: ConfirmAction::Quit,
                });
                Action::None
            }
            _ => self.pop_route_and_switch(),
        }
    }

    fn on_filter_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.filter.active = false;
                self.filter.buffer.clear();
            }
            KeyCode::Enter => {
                self.filter.active = false;
            }
            KeyCode::Backspace => {
                self.filter.buffer.pop();
            }
            KeyCode::Char(c) => {
                if !c.is_control() {
                    self.filter.buffer.push(c);
                }
            }
            _ => {}
        }
        Action::None
    }

    fn on_nav_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Up => {
                self.nav_idx = self.nav_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                self.nav_idx = (self.nav_idx + 1).min(NavItem::ALL.len() - 1);
                Action::None
            }
            KeyCode::Enter => {
                if let Some(route) = self.nav_item().to_route() {
                    self.push_route_and_switch(route)
                } else {
                    self.overlay = Overlay::Confirm(ConfirmOverlay {
                        title: crate::cli::i18n::texts::tui_confirm_exit_title().to_string(),
                        message: crate::cli::i18n::texts::tui_confirm_exit_message().to_string(),
                        action: ConfirmAction::Quit,
                    });
                    Action::None
                }
            }
            _ => Action::None,
        }
    }

    fn on_content_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        match self.route.clone() {
            Route::Providers => self.on_providers_key(key, data),
            Route::ProviderDetail { id } => self.on_provider_detail_key(key, data, &id),
            Route::Mcp => self.on_mcp_key(key, data),
            Route::Prompts => self.on_prompts_key(key, data),
            Route::Config => self.on_config_key(key, data),
            Route::ConfigWebDav => self.on_config_webdav_key(key, data),
            Route::Skills => self.on_skills_installed_key(key, data),
            Route::SkillsDiscover => self.on_skills_discover_key(key),
            Route::SkillsRepos => self.on_skills_repos_key(key, data),
            Route::SkillsUnmanaged => self.on_skills_unmanaged_key(key),
            Route::SkillDetail { directory } => self.on_skill_detail_key(key, data, &directory),
            Route::Settings => self.on_settings_key(key),
            Route::Main => match key.code {
                KeyCode::Char('r') => Action::LocalEnvRefresh,
                _ => Action::None,
            },
        }
    }

    fn on_skills_installed_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        let visible = visible_skills_installed(&self.filter, data);

        match key.code {
            KeyCode::Up => {
                self.skills_idx = self.skills_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                if !visible.is_empty() {
                    self.skills_idx = (self.skills_idx + 1).min(visible.len() - 1);
                }
                Action::None
            }
            KeyCode::Enter => {
                let Some(skill) = visible.get(self.skills_idx) else {
                    return Action::None;
                };
                self.push_route_and_switch(Route::SkillDetail {
                    directory: skill.directory.clone(),
                })
            }
            KeyCode::Char('x') | KeyCode::Char(' ') => {
                let Some(skill) = visible.get(self.skills_idx) else {
                    return Action::None;
                };
                let enabled = !skill.apps.is_enabled_for(&self.app_type);
                Action::SkillsToggle {
                    directory: skill.directory.clone(),
                    enabled,
                }
            }
            KeyCode::Char('i') => self.push_route_and_switch(Route::SkillsUnmanaged),
            _ => Action::None,
        }
    }

    fn on_skills_discover_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Up => {
                self.skills_discover_idx = self.skills_discover_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                let visible = visible_skills_discover(&self.filter, &self.skills_discover_results);
                if !visible.is_empty() {
                    self.skills_discover_idx =
                        (self.skills_discover_idx + 1).min(visible.len() - 1);
                }
                Action::None
            }
            KeyCode::Char('f') => {
                self.overlay = Overlay::TextInput(TextInputState {
                    title: texts::tui_skills_discover_title().to_string(),
                    prompt: texts::tui_skills_discover_prompt().to_string(),
                    buffer: self.skills_discover_query.clone(),
                    submit: TextSubmit::SkillsDiscoverQuery,
                    secret: false,
                });
                Action::None
            }
            KeyCode::Enter => {
                let visible = visible_skills_discover(&self.filter, &self.skills_discover_results);
                let Some(skill) = visible.get(self.skills_discover_idx) else {
                    return Action::None;
                };
                if skill.installed {
                    self.push_toast(texts::tui_toast_skill_already_installed(), ToastKind::Info);
                    return Action::None;
                }
                Action::SkillsInstall {
                    spec: skill.key.clone(),
                }
            }
            KeyCode::Char('r') => self.push_route_and_switch(Route::SkillsRepos),
            _ => Action::None,
        }
    }

    fn on_skills_repos_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        let visible = visible_skills_repos(&self.filter, data);
        match key.code {
            KeyCode::Up => {
                self.skills_repo_idx = self.skills_repo_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                if !visible.is_empty() {
                    self.skills_repo_idx = (self.skills_repo_idx + 1).min(visible.len() - 1);
                }
                Action::None
            }
            KeyCode::Char('a') => {
                self.overlay = Overlay::TextInput(TextInputState {
                    title: texts::tui_skills_repos_add_title().to_string(),
                    prompt: texts::tui_skills_repos_add_prompt().to_string(),
                    buffer: String::new(),
                    submit: TextSubmit::SkillsRepoAdd,
                    secret: false,
                });
                Action::None
            }
            KeyCode::Char('d') => {
                let Some(repo) = visible.get(self.skills_repo_idx) else {
                    return Action::None;
                };
                self.overlay = Overlay::Confirm(ConfirmOverlay {
                    title: texts::tui_skills_repos_remove_title().to_string(),
                    message: texts::tui_confirm_remove_repo_message(&repo.owner, &repo.name),
                    action: ConfirmAction::SkillsRepoRemove {
                        owner: repo.owner.clone(),
                        name: repo.name.clone(),
                    },
                });
                Action::None
            }
            KeyCode::Char('x') | KeyCode::Char(' ') => {
                let Some(repo) = visible.get(self.skills_repo_idx) else {
                    return Action::None;
                };
                Action::SkillsRepoToggleEnabled {
                    owner: repo.owner.clone(),
                    name: repo.name.clone(),
                    enabled: !repo.enabled,
                }
            }
            _ => Action::None,
        }
    }

    fn on_skills_unmanaged_key(&mut self, key: KeyEvent) -> Action {
        let visible = visible_skills_unmanaged(&self.filter, &self.skills_unmanaged_results);
        match key.code {
            KeyCode::Up => {
                self.skills_unmanaged_idx = self.skills_unmanaged_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                if !visible.is_empty() {
                    self.skills_unmanaged_idx =
                        (self.skills_unmanaged_idx + 1).min(visible.len() - 1);
                }
                Action::None
            }
            KeyCode::Char('x') | KeyCode::Char(' ') | KeyCode::Enter => {
                let Some(skill) = visible.get(self.skills_unmanaged_idx) else {
                    return Action::None;
                };
                if self.skills_unmanaged_selected.contains(&skill.directory) {
                    self.skills_unmanaged_selected.remove(&skill.directory);
                } else {
                    self.skills_unmanaged_selected
                        .insert(skill.directory.clone());
                }
                Action::None
            }
            KeyCode::Char('i') => {
                if self.skills_unmanaged_selected.is_empty() {
                    self.push_toast(texts::tui_toast_no_unmanaged_selected(), ToastKind::Info);
                    return Action::None;
                }
                let mut directories = self
                    .skills_unmanaged_selected
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>();
                directories.sort();
                Action::SkillsImportFromApps { directories }
            }
            KeyCode::Char('r') => Action::SkillsScanUnmanaged,
            _ => Action::None,
        }
    }

    fn on_skill_detail_key(&mut self, key: KeyEvent, data: &UiData, directory: &str) -> Action {
        let Some(skill) = data
            .skills
            .installed
            .iter()
            .find(|s| s.directory.eq_ignore_ascii_case(directory))
        else {
            return Action::None;
        };

        match key.code {
            KeyCode::Char('x') | KeyCode::Char(' ') => Action::SkillsToggle {
                directory: skill.directory.clone(),
                enabled: !skill.apps.is_enabled_for(&self.app_type),
            },
            KeyCode::Char('d') => {
                self.overlay = Overlay::Confirm(ConfirmOverlay {
                    title: texts::tui_skills_uninstall_title().to_string(),
                    message: texts::tui_confirm_uninstall_skill_message(
                        &skill.name,
                        &skill.directory,
                    ),
                    action: ConfirmAction::SkillsUninstall {
                        directory: skill.directory.clone(),
                    },
                });
                Action::None
            }
            KeyCode::Char('s') => Action::SkillsSync {
                app: Some(self.app_type.clone()),
            },
            KeyCode::Char('S') => Action::SkillsSync { app: None },
            _ => Action::None,
        }
    }

    fn on_providers_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        let visible = visible_providers(&self.filter, data);
        match key.code {
            KeyCode::Up => {
                self.provider_idx = self.provider_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                if !visible.is_empty() {
                    self.provider_idx = (self.provider_idx + 1).min(visible.len() - 1);
                }
                Action::None
            }
            KeyCode::Enter => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                self.push_route_and_switch(Route::ProviderDetail { id: row.id.clone() })
            }
            KeyCode::Char('a') => {
                self.open_provider_add_form();
                Action::None
            }
            KeyCode::Char('e') => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                self.open_provider_edit_form(row);
                Action::None
            }
            KeyCode::Char('s') => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                if row.is_current {
                    self.push_toast(texts::tui_toast_provider_already_in_use(), ToastKind::Info);
                    return Action::None;
                }
                Action::ProviderSwitch { id: row.id.clone() }
            }
            KeyCode::Char('d') => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                if row.is_current {
                    self.push_toast(
                        texts::tui_toast_provider_cannot_delete_current(),
                        ToastKind::Warning,
                    );
                    return Action::None;
                }
                self.overlay = Overlay::Confirm(ConfirmOverlay {
                    title: texts::tui_confirm_delete_provider_title().to_string(),
                    message: texts::tui_confirm_delete_provider_message(
                        &row.provider.name,
                        &row.id,
                    ),
                    action: ConfirmAction::ProviderDelete { id: row.id.clone() },
                });
                Action::None
            }
            KeyCode::Char('t') => {
                let Some(row) = visible.get(self.provider_idx) else {
                    return Action::None;
                };
                let Some(url) = row.api_url.clone() else {
                    self.push_toast(texts::tui_toast_provider_no_api_url(), ToastKind::Warning);
                    return Action::None;
                };
                self.overlay = Overlay::SpeedtestRunning { url: url.clone() };
                Action::ProviderSpeedtest { url }
            }
            _ => Action::None,
        }
    }

    fn on_provider_detail_key(&mut self, key: KeyEvent, data: &UiData, id: &str) -> Action {
        let Some(row) = data.providers.rows.iter().find(|p| p.id == id) else {
            return Action::None;
        };

        match key.code {
            KeyCode::Char('e') => {
                self.open_provider_edit_form(row);
                Action::None
            }
            KeyCode::Enter => Action::None,
            KeyCode::Char('s') => {
                if row.is_current {
                    self.push_toast(texts::tui_toast_provider_already_in_use(), ToastKind::Info);
                    return Action::None;
                }
                Action::ProviderSwitch { id: row.id.clone() }
            }
            KeyCode::Char('t') => {
                let Some(url) = row.api_url.clone() else {
                    self.push_toast(texts::tui_toast_provider_no_api_url(), ToastKind::Warning);
                    return Action::None;
                };
                self.overlay = Overlay::SpeedtestRunning { url: url.clone() };
                Action::ProviderSpeedtest { url }
            }
            _ => Action::None,
        }
    }

    fn on_mcp_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        let visible = visible_mcp(&self.filter, data);
        match key.code {
            KeyCode::Up => {
                self.mcp_idx = self.mcp_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                if !visible.is_empty() {
                    self.mcp_idx = (self.mcp_idx + 1).min(visible.len() - 1);
                }
                Action::None
            }
            KeyCode::Char('a') => {
                self.open_mcp_add_form();
                Action::None
            }
            KeyCode::Char('e') => {
                let Some(row) = visible.get(self.mcp_idx) else {
                    return Action::None;
                };
                self.open_mcp_edit_form(row);
                Action::None
            }
            KeyCode::Char('x') => {
                let Some(row) = visible.get(self.mcp_idx) else {
                    return Action::None;
                };
                let enabled = row.server.apps.is_enabled_for(&self.app_type);
                Action::McpToggle {
                    id: row.id.clone(),
                    enabled: !enabled,
                }
            }
            KeyCode::Char('m') => {
                let Some(row) = visible.get(self.mcp_idx) else {
                    return Action::None;
                };
                self.overlay = Overlay::McpAppsPicker {
                    id: row.id.clone(),
                    name: row.server.name.clone(),
                    selected: app_type_picker_index(&self.app_type),
                    apps: row.server.apps.clone(),
                };
                Action::None
            }
            KeyCode::Char('i') => Action::McpImport,
            KeyCode::Char('v') => {
                self.overlay = Overlay::TextInput(TextInputState {
                    title: texts::tui_input_validate_command_title().to_string(),
                    prompt: texts::tui_input_validate_command_prompt().to_string(),
                    buffer: String::new(),
                    submit: TextSubmit::McpValidateCommand,
                    secret: false,
                });
                Action::None
            }
            KeyCode::Char('d') => {
                let Some(row) = visible.get(self.mcp_idx) else {
                    return Action::None;
                };
                self.overlay = Overlay::Confirm(ConfirmOverlay {
                    title: texts::tui_confirm_delete_mcp_title().to_string(),
                    message: texts::tui_confirm_delete_mcp_message(&row.server.name, &row.id),
                    action: ConfirmAction::McpDelete { id: row.id.clone() },
                });
                Action::None
            }
            _ => Action::None,
        }
    }

    fn on_prompts_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        let visible = visible_prompts(&self.filter, data);
        match key.code {
            KeyCode::Up => {
                self.prompt_idx = self.prompt_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                if !visible.is_empty() {
                    self.prompt_idx = (self.prompt_idx + 1).min(visible.len() - 1);
                }
                Action::None
            }
            KeyCode::Enter => {
                let Some(row) = visible.get(self.prompt_idx) else {
                    return Action::None;
                };
                self.overlay = Overlay::TextView(TextViewState {
                    title: texts::tui_prompt_title(&row.prompt.name),
                    lines: row.prompt.content.lines().map(|s| s.to_string()).collect(),
                    scroll: 0,
                });
                Action::None
            }
            KeyCode::Char('a') => {
                let Some(row) = visible.get(self.prompt_idx) else {
                    return Action::None;
                };
                Action::PromptActivate { id: row.id.clone() }
            }
            KeyCode::Char('x') => {
                let active = data.prompts.rows.iter().find(|p| p.prompt.enabled);
                let Some(active) = active else {
                    self.push_toast(
                        texts::tui_toast_prompt_no_active_to_deactivate(),
                        ToastKind::Info,
                    );
                    return Action::None;
                };
                Action::PromptDeactivate {
                    id: active.id.clone(),
                }
            }
            KeyCode::Char('d') => {
                let Some(row) = visible.get(self.prompt_idx) else {
                    return Action::None;
                };
                if row.prompt.enabled {
                    self.push_toast(
                        texts::tui_toast_prompt_cannot_delete_active(),
                        ToastKind::Warning,
                    );
                    return Action::None;
                }
                self.overlay = Overlay::Confirm(ConfirmOverlay {
                    title: texts::tui_confirm_delete_prompt_title().to_string(),
                    message: texts::tui_confirm_delete_prompt_message(&row.prompt.name, &row.id),
                    action: ConfirmAction::PromptDelete { id: row.id.clone() },
                });
                Action::None
            }
            KeyCode::Char('e') => {
                let Some(row) = visible.get(self.prompt_idx) else {
                    return Action::None;
                };
                self.open_editor(
                    texts::tui_prompt_title(&row.prompt.name),
                    EditorKind::Plain,
                    row.prompt.content.clone(),
                    EditorSubmit::PromptEdit { id: row.id.clone() },
                );
                Action::None
            }
            _ => Action::None,
        }
    }

    fn on_config_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        let items = visible_config_items(&self.filter);
        match key.code {
            KeyCode::Up => {
                self.config_idx = self.config_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                if !items.is_empty() {
                    self.config_idx = (self.config_idx + 1).min(items.len() - 1);
                }
                Action::None
            }
            KeyCode::Char('e') => {
                let Some(item) = items.get(self.config_idx) else {
                    return Action::None;
                };
                if matches!(item, ConfigItem::CommonSnippet) {
                    self.overlay = Overlay::CommonSnippetPicker {
                        selected: snippet_picker_index_for_app_type(&self.app_type),
                    };
                }
                Action::None
            }
            KeyCode::Enter => {
                let Some(item) = items.get(self.config_idx) else {
                    return Action::None;
                };
                match item {
                    ConfigItem::Path => {
                        self.overlay = Overlay::TextView(TextViewState {
                            title: texts::tui_config_paths_title().to_string(),
                            lines: vec![
                                texts::tui_config_paths_config_file(
                                    &data.config.config_path.display().to_string(),
                                ),
                                texts::tui_config_paths_config_dir(
                                    &data.config.config_dir.display().to_string(),
                                ),
                            ],
                            scroll: 0,
                        });
                        Action::None
                    }
                    ConfigItem::ShowFull => Action::ConfigShowFull,
                    ConfigItem::Export => {
                        self.overlay = Overlay::TextInput(TextInputState {
                            title: texts::tui_config_export_title().to_string(),
                            prompt: texts::tui_config_export_prompt().to_string(),
                            buffer: texts::tui_default_config_export_path().to_string(),
                            submit: TextSubmit::ConfigExport,
                            secret: false,
                        });
                        Action::None
                    }
                    ConfigItem::Import => {
                        self.overlay = Overlay::TextInput(TextInputState {
                            title: texts::tui_config_import_title().to_string(),
                            prompt: texts::tui_config_import_prompt().to_string(),
                            buffer: texts::tui_default_config_export_path().to_string(),
                            submit: TextSubmit::ConfigImport,
                            secret: false,
                        });
                        Action::None
                    }
                    ConfigItem::Backup => {
                        self.overlay = Overlay::TextInput(TextInputState {
                            title: texts::tui_config_backup_title().to_string(),
                            prompt: texts::tui_config_backup_prompt().to_string(),
                            buffer: String::new(),
                            submit: TextSubmit::ConfigBackupName,
                            secret: false,
                        });
                        Action::None
                    }
                    ConfigItem::Restore => {
                        if data.config.backups.is_empty() {
                            self.push_toast(texts::tui_toast_no_backups_found(), ToastKind::Info);
                            return Action::None;
                        }
                        self.overlay = Overlay::BackupPicker { selected: 0 };
                        Action::None
                    }
                    ConfigItem::Validate => Action::ConfigValidate,
                    ConfigItem::CommonSnippet => {
                        self.overlay = Overlay::CommonSnippetPicker {
                            selected: snippet_picker_index_for_app_type(&self.app_type),
                        };
                        Action::None
                    }
                    ConfigItem::WebDavSync => self.push_route_and_switch(Route::ConfigWebDav),
                    ConfigItem::Reset => {
                        self.overlay = Overlay::Confirm(ConfirmOverlay {
                            title: texts::tui_config_reset_title().to_string(),
                            message: texts::tui_config_reset_message().to_string(),
                            action: ConfirmAction::ConfigReset,
                        });
                        Action::None
                    }
                }
            }
            _ => Action::None,
        }
    }

    fn on_config_webdav_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        let items = visible_webdav_config_items(&self.filter);
        match key.code {
            KeyCode::Up => {
                self.config_webdav_idx = self.config_webdav_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                if !items.is_empty() {
                    self.config_webdav_idx = (self.config_webdav_idx + 1).min(items.len() - 1);
                }
                Action::None
            }
            KeyCode::Char('e') => {
                let Some(item) = items.get(self.config_webdav_idx) else {
                    return Action::None;
                };
                if matches!(item, WebDavConfigItem::Settings) {
                    let webdav_json = match data.config.webdav_sync.as_ref() {
                        Some(cfg) => {
                            serde_json::to_string_pretty(cfg).unwrap_or_else(|_| "{}".to_string())
                        }
                        None => serde_json::to_string_pretty(
                            &crate::settings::WebDavSyncSettings::default(),
                        )
                        .unwrap_or_else(|_| "{}".to_string()),
                    };
                    self.open_editor(
                        texts::tui_webdav_settings_editor_title(),
                        EditorKind::Json,
                        webdav_json,
                        EditorSubmit::ConfigWebDavSettings,
                    );
                }
                Action::None
            }
            KeyCode::Enter => {
                let Some(item) = items.get(self.config_webdav_idx) else {
                    return Action::None;
                };
                match item {
                    WebDavConfigItem::Settings => {
                        let webdav_json = match data.config.webdav_sync.as_ref() {
                            Some(cfg) => serde_json::to_string_pretty(cfg)
                                .unwrap_or_else(|_| "{}".to_string()),
                            None => serde_json::to_string_pretty(
                                &crate::settings::WebDavSyncSettings::default(),
                            )
                            .unwrap_or_else(|_| "{}".to_string()),
                        };
                        self.open_editor(
                            texts::tui_webdav_settings_editor_title(),
                            EditorKind::Json,
                            webdav_json,
                            EditorSubmit::ConfigWebDavSettings,
                        );
                        Action::None
                    }
                    WebDavConfigItem::CheckConnection => Action::ConfigWebDavCheckConnection,
                    WebDavConfigItem::Upload => Action::ConfigWebDavUpload,
                    WebDavConfigItem::Download => Action::ConfigWebDavDownload,
                    WebDavConfigItem::Reset => Action::ConfigWebDavReset,
                    WebDavConfigItem::JianguoyunQuickSetup => {
                        self.webdav_quick_setup_username = None;
                        self.overlay = Overlay::TextInput(TextInputState {
                            title: texts::tui_webdav_jianguoyun_setup_title().to_string(),
                            prompt: texts::tui_webdav_jianguoyun_username_prompt().to_string(),
                            buffer: String::new(),
                            submit: TextSubmit::WebDavJianguoyunUsername,
                            secret: false,
                        });
                        Action::None
                    }
                }
            }
            _ => Action::None,
        }
    }

    fn on_settings_key(&mut self, key: KeyEvent) -> Action {
        let settings_len = SettingsItem::ALL.len();
        match key.code {
            KeyCode::Up => {
                self.settings_idx = self.settings_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                self.settings_idx = (self.settings_idx + 1).min(settings_len - 1);
                Action::None
            }
            KeyCode::Enter => match SettingsItem::ALL.get(self.settings_idx) {
                Some(SettingsItem::Language) => {
                    let next = match current_language() {
                        Language::English => Language::Chinese,
                        Language::Chinese => Language::English,
                    };
                    Action::SetLanguage(next)
                }
                Some(SettingsItem::SkipClaudeOnboarding) => {
                    let current = crate::settings::get_skip_claude_onboarding();
                    let next = !current;
                    let path = crate::config::get_claude_mcp_path();

                    self.overlay = Overlay::Confirm(ConfirmOverlay {
                        title: texts::tui_confirm_title().to_string(),
                        message: texts::skip_claude_onboarding_confirm(
                            next,
                            path.to_string_lossy().as_ref(),
                        ),
                        action: ConfirmAction::SettingsSetSkipClaudeOnboarding { enabled: next },
                    });
                    Action::None
                }
                Some(SettingsItem::CheckForUpdates) => Action::CheckUpdate,
                None => Action::None,
            },
            _ => Action::None,
        }
    }

    fn on_overlay_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        if matches!(key.code, KeyCode::Char('e')) {
            match &self.overlay {
                Overlay::CommonSnippetPicker { selected } => {
                    let app_type = snippet_picker_app_type(*selected);
                    self.open_common_snippet_editor(app_type, data, None);
                    return Action::None;
                }
                Overlay::CommonSnippetView { app_type, view } => {
                    let app_type = app_type.clone();
                    let initial = view.lines.join("\n");
                    self.open_common_snippet_editor(app_type, data, Some(initial));
                    return Action::None;
                }
                _ => {}
            }
        }

        match &mut self.overlay {
            Overlay::None => Action::None,
            Overlay::Help => match key.code {
                KeyCode::Esc | KeyCode::Char('?') => {
                    self.overlay = Overlay::None;
                    Action::None
                }
                _ => Action::None,
            },
            Overlay::Confirm(confirm) => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    let action = match &confirm.action {
                        ConfirmAction::Quit => Action::Quit,
                        ConfirmAction::ProviderDelete { id } => {
                            Action::ProviderDelete { id: id.clone() }
                        }
                        ConfirmAction::McpDelete { id } => Action::McpDelete { id: id.clone() },
                        ConfirmAction::PromptDelete { id } => {
                            Action::PromptDelete { id: id.clone() }
                        }
                        ConfirmAction::SkillsUninstall { directory } => Action::SkillsUninstall {
                            directory: directory.clone(),
                        },
                        ConfirmAction::SkillsRepoRemove { owner, name } => {
                            Action::SkillsRepoRemove {
                                owner: owner.clone(),
                                name: name.clone(),
                            }
                        }
                        ConfirmAction::ConfigImport { path } => {
                            Action::ConfigImport { path: path.clone() }
                        }
                        ConfirmAction::ConfigRestoreBackup { id } => {
                            Action::ConfigRestoreBackup { id: id.clone() }
                        }
                        ConfirmAction::ConfigReset => Action::ConfigReset,
                        ConfirmAction::SettingsSetSkipClaudeOnboarding { enabled } => {
                            Action::SetSkipClaudeOnboarding { enabled: *enabled }
                        }
                        ConfirmAction::EditorDiscard => Action::EditorDiscard,
                        ConfirmAction::EditorSaveBeforeClose => {
                            if let Some(editor) = self.editor.as_ref() {
                                Action::EditorSubmit {
                                    submit: editor.submit.clone(),
                                    content: editor.text(),
                                }
                            } else {
                                Action::None
                            }
                        }
                    };
                    self.overlay = Overlay::None;
                    action
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    let action = if matches!(confirm.action, ConfirmAction::EditorSaveBeforeClose) {
                        self.editor = None;
                        Action::None
                    } else {
                        Action::None
                    };
                    self.overlay = Overlay::None;
                    action
                }
                KeyCode::Esc => {
                    self.overlay = Overlay::None;
                    Action::None
                }
                _ => Action::None,
            },
            Overlay::TextInput(input) => match key.code {
                KeyCode::Esc => {
                    if matches!(
                        input.submit,
                        TextSubmit::WebDavJianguoyunUsername | TextSubmit::WebDavJianguoyunPassword
                    ) {
                        self.webdav_quick_setup_username = None;
                    }
                    self.overlay = Overlay::None;
                    Action::None
                }
                KeyCode::Enter => {
                    let raw = input.buffer.trim().to_string();
                    let submit = input.submit;
                    self.overlay = Overlay::None;
                    match submit {
                        TextSubmit::ConfigExport => {
                            if raw.is_empty() {
                                self.push_toast(
                                    texts::tui_toast_export_path_empty(),
                                    ToastKind::Warning,
                                );
                                return Action::None;
                            }
                            Action::ConfigExport { path: raw }
                        }
                        TextSubmit::ConfigImport => {
                            if raw.is_empty() {
                                self.push_toast(
                                    texts::tui_toast_import_path_empty(),
                                    ToastKind::Warning,
                                );
                                return Action::None;
                            }
                            self.overlay = Overlay::Confirm(ConfirmOverlay {
                                title: texts::tui_config_import_title().to_string(),
                                message: texts::tui_confirm_import_message(&raw),
                                action: ConfirmAction::ConfigImport { path: raw },
                            });
                            Action::None
                        }
                        TextSubmit::ConfigBackupName => {
                            let name = if raw.is_empty() { None } else { Some(raw) };
                            Action::ConfigBackup { name }
                        }
                        TextSubmit::McpValidateCommand => {
                            if raw.is_empty() {
                                self.push_toast(
                                    texts::tui_toast_command_empty(),
                                    ToastKind::Warning,
                                );
                                return Action::None;
                            }
                            Action::McpValidate { command: raw }
                        }
                        TextSubmit::SkillsInstallSpec => {
                            if raw.is_empty() {
                                self.push_toast(
                                    texts::tui_toast_skill_spec_empty(),
                                    ToastKind::Warning,
                                );
                                return Action::None;
                            }
                            Action::SkillsInstall { spec: raw }
                        }
                        TextSubmit::SkillsDiscoverQuery => {
                            self.skills_discover_query = raw.clone();
                            Action::SkillsDiscover { query: raw }
                        }
                        TextSubmit::SkillsRepoAdd => {
                            if raw.is_empty() {
                                self.push_toast(
                                    texts::tui_toast_repo_spec_empty(),
                                    ToastKind::Warning,
                                );
                                return Action::None;
                            }
                            Action::SkillsRepoAdd { spec: raw }
                        }
                        TextSubmit::WebDavJianguoyunUsername => {
                            if raw.is_empty() {
                                self.push_toast(
                                    texts::tui_toast_webdav_username_empty(),
                                    ToastKind::Warning,
                                );
                                self.overlay = Overlay::TextInput(TextInputState {
                                    title: texts::tui_webdav_jianguoyun_setup_title().to_string(),
                                    prompt: texts::tui_webdav_jianguoyun_username_prompt()
                                        .to_string(),
                                    buffer: String::new(),
                                    submit: TextSubmit::WebDavJianguoyunUsername,
                                    secret: false,
                                });
                                return Action::None;
                            }
                            self.webdav_quick_setup_username = Some(raw);
                            self.overlay = Overlay::TextInput(TextInputState {
                                title: texts::tui_webdav_jianguoyun_setup_title().to_string(),
                                prompt: texts::tui_webdav_jianguoyun_app_password_prompt()
                                    .to_string(),
                                buffer: String::new(),
                                submit: TextSubmit::WebDavJianguoyunPassword,
                                secret: true,
                            });
                            Action::None
                        }
                        TextSubmit::WebDavJianguoyunPassword => {
                            if raw.is_empty() {
                                self.push_toast(
                                    texts::tui_toast_webdav_password_empty(),
                                    ToastKind::Warning,
                                );
                                self.overlay = Overlay::TextInput(TextInputState {
                                    title: texts::tui_webdav_jianguoyun_setup_title().to_string(),
                                    prompt: texts::tui_webdav_jianguoyun_app_password_prompt()
                                        .to_string(),
                                    buffer: String::new(),
                                    submit: TextSubmit::WebDavJianguoyunPassword,
                                    secret: true,
                                });
                                return Action::None;
                            }
                            let username =
                                self.webdav_quick_setup_username.take().unwrap_or_default();
                            if username.trim().is_empty() {
                                self.push_toast(
                                    texts::tui_toast_webdav_username_empty(),
                                    ToastKind::Warning,
                                );
                                return Action::None;
                            }
                            Action::ConfigWebDavJianguoyunQuickSetup {
                                username,
                                password: raw,
                            }
                        }
                    }
                }
                KeyCode::Backspace => {
                    input.buffer.pop();
                    Action::None
                }
                KeyCode::Char(c) => {
                    if !c.is_control() {
                        input.buffer.push(c);
                    }
                    Action::None
                }
                _ => Action::None,
            },
            Overlay::BackupPicker { selected } => {
                let backups = &data.config.backups;
                match key.code {
                    KeyCode::Esc => {
                        self.overlay = Overlay::None;
                        Action::None
                    }
                    KeyCode::Up => {
                        *selected = selected.saturating_sub(1);
                        Action::None
                    }
                    KeyCode::Down => {
                        if !backups.is_empty() {
                            *selected = (*selected + 1).min(backups.len() - 1);
                        }
                        Action::None
                    }
                    KeyCode::Enter => {
                        let Some(backup) = backups.get(*selected) else {
                            return Action::None;
                        };
                        let id = backup.id.clone();
                        self.overlay = Overlay::Confirm(ConfirmOverlay {
                            title: texts::tui_confirm_restore_backup_title().to_string(),
                            message: texts::tui_confirm_restore_backup_message(
                                &backup.display_name,
                            ),
                            action: ConfirmAction::ConfigRestoreBackup { id },
                        });
                        Action::None
                    }
                    _ => Action::None,
                }
            }
            Overlay::TextView(view) => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.overlay = Overlay::None;
                    Action::None
                }
                KeyCode::Up => {
                    view.scroll = view.scroll.saturating_sub(1);
                    Action::None
                }
                KeyCode::Down => {
                    if !view.lines.is_empty() {
                        view.scroll = (view.scroll + 1).min(view.lines.len() - 1);
                    }
                    Action::None
                }
                _ => Action::None,
            },
            Overlay::CommonSnippetPicker { selected } => match key.code {
                KeyCode::Esc => {
                    self.overlay = Overlay::None;
                    Action::None
                }
                KeyCode::Up => {
                    *selected = selected.saturating_sub(1);
                    Action::None
                }
                KeyCode::Down => {
                    *selected = (*selected + 1).min(2);
                    Action::None
                }
                KeyCode::Enter => {
                    let app_type = snippet_picker_app_type(*selected);
                    self.open_common_snippet_view(app_type, data);
                    Action::None
                }
                _ => Action::None,
            },
            Overlay::CommonSnippetView { app_type, view } => match key.code {
                KeyCode::Char('a') => Action::ConfigCommonSnippetApply {
                    app_type: app_type.clone(),
                },
                KeyCode::Char('c') => Action::ConfigCommonSnippetClear {
                    app_type: app_type.clone(),
                },
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.overlay = Overlay::None;
                    Action::None
                }
                KeyCode::Up => {
                    view.scroll = view.scroll.saturating_sub(1);
                    Action::None
                }
                KeyCode::Down => {
                    if !view.lines.is_empty() {
                        view.scroll = (view.scroll + 1).min(view.lines.len() - 1);
                    }
                    Action::None
                }
                _ => Action::None,
            },
            Overlay::SkillsSyncMethodPicker { selected } => match key.code {
                KeyCode::Esc => {
                    self.overlay = Overlay::None;
                    Action::None
                }
                KeyCode::Up => {
                    *selected = selected.saturating_sub(1);
                    Action::None
                }
                KeyCode::Down => {
                    *selected = (*selected + 1).min(2);
                    Action::None
                }
                KeyCode::Enter => {
                    let method = sync_method_for_picker_index(*selected);
                    let unchanged = method == data.skills.sync_method;
                    self.overlay = Overlay::None;
                    if unchanged {
                        Action::None
                    } else {
                        Action::SkillsSetSyncMethod { method }
                    }
                }
                _ => Action::None,
            },
            Overlay::ClaudeModelPicker { selected, editing } => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    self.overlay = Overlay::None;
                    return Action::None;
                };
                if !matches!(provider.app_type, AppType::Claude) {
                    self.overlay = Overlay::None;
                    return Action::None;
                }

                *selected = (*selected).min(4);

                if *editing {
                    match key.code {
                        KeyCode::Esc | KeyCode::Enter => {
                            *editing = false;
                            Action::None
                        }
                        KeyCode::Left => {
                            if let Some(input) = provider.claude_model_input_mut(*selected) {
                                input.move_left();
                            }
                            Action::None
                        }
                        KeyCode::Right => {
                            if let Some(input) = provider.claude_model_input_mut(*selected) {
                                input.move_right();
                            }
                            Action::None
                        }
                        KeyCode::Home => {
                            if let Some(input) = provider.claude_model_input_mut(*selected) {
                                input.move_home();
                            }
                            Action::None
                        }
                        KeyCode::End => {
                            if let Some(input) = provider.claude_model_input_mut(*selected) {
                                input.move_end();
                            }
                            Action::None
                        }
                        KeyCode::Backspace => {
                            if let Some(input) = provider.claude_model_input_mut(*selected) {
                                if input.backspace() {
                                    provider.mark_claude_model_config_touched();
                                }
                            }
                            Action::None
                        }
                        KeyCode::Delete => {
                            if let Some(input) = provider.claude_model_input_mut(*selected) {
                                if input.delete() {
                                    provider.mark_claude_model_config_touched();
                                }
                            }
                            Action::None
                        }
                        KeyCode::Char(c) => {
                            if c.is_control() {
                                return Action::None;
                            }
                            if let Some(input) = provider.claude_model_input_mut(*selected) {
                                if input.insert_char(c) {
                                    provider.mark_claude_model_config_touched();
                                }
                            }
                            Action::None
                        }
                        _ => Action::None,
                    }
                } else {
                    match key.code {
                        KeyCode::Esc => {
                            self.overlay = Overlay::None;
                            Action::None
                        }
                        KeyCode::Up => {
                            *selected = selected.saturating_sub(1);
                            Action::None
                        }
                        KeyCode::Down => {
                            *selected = (*selected + 1).min(4);
                            Action::None
                        }
                        KeyCode::Char(' ') | KeyCode::Enter => {
                            *editing = true;
                            Action::None
                        }
                        _ => Action::None,
                    }
                }
            }
            Overlay::Loading { kind, .. } => match key.code {
                KeyCode::Esc => {
                    let kind = *kind;
                    self.overlay = Overlay::None;
                    if kind == LoadingKind::UpdateCheck {
                        Action::CancelUpdateCheck
                    } else {
                        Action::None
                    }
                }
                _ => Action::None,
            },
            Overlay::McpAppsPicker {
                id, selected, apps, ..
            } => match key.code {
                KeyCode::Esc => {
                    self.overlay = Overlay::None;
                    Action::None
                }
                KeyCode::Up => {
                    *selected = selected.saturating_sub(1);
                    Action::None
                }
                KeyCode::Down => {
                    *selected = (*selected + 1).min(2);
                    Action::None
                }
                KeyCode::Char('x') | KeyCode::Char(' ') => {
                    let app_type = app_type_for_picker_index(*selected);
                    let enabled = apps.is_enabled_for(&app_type);
                    apps.set_enabled_for(&app_type, !enabled);
                    Action::None
                }
                KeyCode::Enter => {
                    let id = id.clone();
                    let next = apps.clone();
                    let unchanged = data
                        .mcp
                        .rows
                        .iter()
                        .find(|row| row.id == id)
                        .map(|row| row.server.apps == next)
                        .unwrap_or(false);

                    self.overlay = Overlay::None;
                    if unchanged {
                        Action::None
                    } else {
                        Action::McpSetApps { id, apps: next }
                    }
                }
                _ => Action::None,
            },
            Overlay::SpeedtestRunning { .. } => match key.code {
                KeyCode::Esc => {
                    self.overlay = Overlay::None;
                    Action::None
                }
                _ => Action::None,
            },
            Overlay::SpeedtestResult { scroll, lines, .. } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.overlay = Overlay::None;
                    Action::None
                }
                KeyCode::Up => {
                    *scroll = scroll.saturating_sub(1);
                    Action::None
                }
                KeyCode::Down => {
                    if !lines.is_empty() {
                        *scroll = (*scroll + 1).min(lines.len() - 1);
                    }
                    Action::None
                }
                _ => Action::None,
            },
            Overlay::UpdateAvailable { selected, .. } => match key.code {
                KeyCode::Left => {
                    *selected = 0;
                    Action::None
                }
                KeyCode::Right => {
                    *selected = 1;
                    Action::None
                }
                KeyCode::Enter => {
                    if *selected == 0 {
                        Action::ConfirmUpdate
                    } else {
                        Action::CancelUpdate
                    }
                }
                KeyCode::Esc => Action::CancelUpdate,
                _ => Action::None,
            },
            Overlay::UpdateDownloading { .. } => match key.code {
                KeyCode::Esc => {
                    self.overlay = Overlay::None;
                    Action::None
                }
                _ => Action::None,
            },
            Overlay::UpdateResult { success, .. } => match key.code {
                KeyCode::Enter => {
                    let should_exit = *success;
                    self.overlay = Overlay::None;
                    if should_exit {
                        self.should_quit = true;
                    }
                    Action::None
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.overlay = Overlay::None;
                    Action::None
                }
                _ => Action::None,
            },
        }
    }

    pub fn open_editor(
        &mut self,
        title: impl Into<String>,
        kind: EditorKind,
        initial: impl Into<String>,
        submit: EditorSubmit,
    ) {
        self.filter.active = false;
        self.overlay = Overlay::None;
        self.focus = Focus::Content;
        self.editor = Some(EditorState::new(title, kind, submit, initial));
    }

    fn common_snippet_text_for(&self, app_type: &AppType, data: &UiData) -> String {
        if app_type == &self.app_type {
            data.config.common_snippet.clone()
        } else {
            data.config
                .common_snippets
                .get(app_type)
                .cloned()
                .unwrap_or_default()
        }
    }

    fn open_common_snippet_view(&mut self, app_type: AppType, data: &UiData) {
        let snippet = self.common_snippet_text_for(&app_type, data);
        let snippet = if snippet.trim().is_empty() {
            texts::tui_default_common_snippet_for_app(app_type.as_str()).to_string()
        } else {
            snippet
        };

        self.overlay = Overlay::CommonSnippetView {
            app_type: app_type.clone(),
            view: TextViewState {
                title: texts::tui_common_snippet_title(app_type.as_str()),
                lines: snippet.lines().map(|s| s.to_string()).collect(),
                scroll: 0,
            },
        };
    }

    fn open_common_snippet_editor(
        &mut self,
        app_type: AppType,
        data: &UiData,
        initial_override: Option<String>,
    ) {
        let snippet = initial_override.unwrap_or_else(|| {
            let snippet = self.common_snippet_text_for(&app_type, data);
            if snippet.trim().is_empty() {
                texts::tui_default_common_snippet_for_app(app_type.as_str()).to_string()
            } else {
                snippet
            }
        });

        let kind = if matches!(app_type, AppType::Codex) {
            EditorKind::Plain
        } else {
            EditorKind::Json
        };

        self.open_editor(
            texts::tui_common_snippet_title(app_type.as_str()),
            kind,
            snippet,
            EditorSubmit::ConfigCommonSnippet { app_type },
        );
    }

    fn open_provider_add_form(&mut self) {
        self.filter.active = false;
        self.overlay = Overlay::None;
        self.focus = Focus::Content;
        self.editor = None;
        self.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            self.app_type.clone(),
        )));
    }

    fn open_provider_edit_form(&mut self, row: &super::data::ProviderRow) {
        self.filter.active = false;
        self.overlay = Overlay::None;
        self.focus = Focus::Content;
        self.editor = None;
        self.form = Some(FormState::ProviderAdd(ProviderAddFormState::from_provider(
            self.app_type.clone(),
            &row.provider,
        )));
    }

    fn open_mcp_add_form(&mut self) {
        self.filter.active = false;
        self.overlay = Overlay::None;
        self.focus = Focus::Content;
        self.editor = None;
        let mut state = McpAddFormState::new();
        state.apps.set_enabled_for(&self.app_type, true);
        self.form = Some(FormState::McpAdd(state));
    }

    fn open_mcp_edit_form(&mut self, row: &super::data::McpRow) {
        self.filter.active = false;
        self.overlay = Overlay::None;
        self.focus = Focus::Content;
        self.editor = None;
        self.form = Some(FormState::McpAdd(McpAddFormState::from_server(&row.server)));
    }

    fn on_form_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        let Some(form) = &mut self.form else {
            return Action::None;
        };

        if matches!(key.code, KeyCode::Tab) {
            match form {
                FormState::ProviderAdd(provider) => {
                    if matches!(provider.app_type, AppType::Codex) {
                        match (
                            &provider.mode,
                            provider.focus,
                            provider.codex_preview_section,
                        ) {
                            (FormMode::Add, FormFocus::Templates, _) => {
                                provider.focus = FormFocus::Fields;
                            }
                            (FormMode::Add, FormFocus::Fields, _) => {
                                provider.focus = FormFocus::JsonPreview;
                                provider.codex_preview_section =
                                    super::form::CodexPreviewSection::Auth;
                            }
                            (
                                FormMode::Add,
                                FormFocus::JsonPreview,
                                super::form::CodexPreviewSection::Auth,
                            ) => {
                                provider.focus = FormFocus::JsonPreview;
                                provider.codex_preview_section =
                                    super::form::CodexPreviewSection::Config;
                            }
                            (
                                FormMode::Add,
                                FormFocus::JsonPreview,
                                super::form::CodexPreviewSection::Config,
                            ) => {
                                provider.focus = FormFocus::Templates;
                            }
                            (FormMode::Edit { .. }, FormFocus::Fields, _) => {
                                provider.focus = FormFocus::JsonPreview;
                                provider.codex_preview_section =
                                    super::form::CodexPreviewSection::Auth;
                            }
                            (
                                FormMode::Edit { .. },
                                FormFocus::JsonPreview,
                                super::form::CodexPreviewSection::Auth,
                            ) => {
                                provider.focus = FormFocus::JsonPreview;
                                provider.codex_preview_section =
                                    super::form::CodexPreviewSection::Config;
                            }
                            (
                                FormMode::Edit { .. },
                                FormFocus::JsonPreview,
                                super::form::CodexPreviewSection::Config,
                            ) => {
                                provider.focus = FormFocus::Fields;
                            }
                            (FormMode::Edit { .. }, FormFocus::Templates, _) => {
                                provider.focus = FormFocus::Fields;
                            }
                        }
                    } else {
                        provider.focus = match (&provider.mode, provider.focus) {
                            (FormMode::Add, FormFocus::Templates) => FormFocus::Fields,
                            (FormMode::Add, FormFocus::Fields) => FormFocus::JsonPreview,
                            (FormMode::Add, FormFocus::JsonPreview) => FormFocus::Templates,
                            (FormMode::Edit { .. }, FormFocus::Fields) => FormFocus::JsonPreview,
                            (FormMode::Edit { .. }, FormFocus::JsonPreview) => FormFocus::Fields,
                            (FormMode::Edit { .. }, FormFocus::Templates) => FormFocus::Fields,
                        };
                    }
                }
                FormState::McpAdd(mcp) => {
                    mcp.focus = match (&mcp.mode, mcp.focus) {
                        (FormMode::Add, FormFocus::Templates) => FormFocus::Fields,
                        (FormMode::Add, FormFocus::Fields) => FormFocus::JsonPreview,
                        (FormMode::Add, FormFocus::JsonPreview) => FormFocus::Templates,
                        (FormMode::Edit { .. }, FormFocus::Fields) => FormFocus::JsonPreview,
                        (FormMode::Edit { .. }, FormFocus::JsonPreview) => FormFocus::Fields,
                        (FormMode::Edit { .. }, FormFocus::Templates) => FormFocus::Fields,
                    };
                }
            }
            return Action::None;
        }

        if let FormState::ProviderAdd(provider) = form {
            if provider.focus == FormFocus::Templates && matches!(provider.mode, FormMode::Add) {
                match key.code {
                    KeyCode::Left => {
                        provider.template_idx = provider.template_idx.saturating_sub(1);
                        return Action::None;
                    }
                    KeyCode::Right => {
                        let max = provider.template_count().saturating_sub(1);
                        provider.template_idx = (provider.template_idx + 1).min(max);
                        return Action::None;
                    }
                    KeyCode::Enter => {
                        let existing_ids = data
                            .providers
                            .rows
                            .iter()
                            .map(|row| row.id.clone())
                            .collect::<Vec<_>>();
                        provider.apply_template(provider.template_idx, &existing_ids);
                        provider.focus = FormFocus::Fields;
                        return Action::None;
                    }
                    _ => {}
                }
            }
        }

        if let FormState::McpAdd(mcp) = form {
            if mcp.focus == FormFocus::Templates && matches!(mcp.mode, FormMode::Add) {
                match key.code {
                    KeyCode::Left => {
                        mcp.template_idx = mcp.template_idx.saturating_sub(1);
                        return Action::None;
                    }
                    KeyCode::Right => {
                        let max = mcp.template_count().saturating_sub(1);
                        mcp.template_idx = (mcp.template_idx + 1).min(max);
                        return Action::None;
                    }
                    KeyCode::Enter => {
                        mcp.apply_template(mcp.template_idx);
                        mcp.focus = FormFocus::Fields;
                        return Action::None;
                    }
                    _ => {}
                }
            }
        }

        if let FormState::ProviderAdd(provider) = form {
            if provider.focus == FormFocus::Fields {
                let fields = provider.fields();
                if !fields.is_empty() {
                    provider.field_idx = provider.field_idx.min(fields.len() - 1);
                } else {
                    provider.field_idx = 0;
                }

                if matches!(
                    fields.get(provider.field_idx),
                    Some(ProviderAddField::CommonConfigDivider)
                ) {
                    if provider.field_idx < fields.len().saturating_sub(1) {
                        provider.field_idx = (provider.field_idx + 1).min(fields.len() - 1);
                    } else {
                        provider.field_idx = provider.field_idx.saturating_sub(1);
                    }
                }

                let Some(selected) = fields.get(provider.field_idx).copied() else {
                    return Action::None;
                };

                if provider.editing {
                    match key.code {
                        KeyCode::Esc | KeyCode::Enter => {
                            provider.editing = false;
                            return Action::None;
                        }
                        KeyCode::Left => {
                            if let Some(input) = provider.input_mut(selected) {
                                input.move_left();
                            }
                            return Action::None;
                        }
                        KeyCode::Right => {
                            if let Some(input) = provider.input_mut(selected) {
                                input.move_right();
                            }
                            return Action::None;
                        }
                        KeyCode::Home => {
                            if let Some(input) = provider.input_mut(selected) {
                                input.move_home();
                            }
                            return Action::None;
                        }
                        KeyCode::End => {
                            if let Some(input) = provider.input_mut(selected) {
                                input.move_end();
                            }
                            return Action::None;
                        }
                        KeyCode::Backspace => {
                            let changed = provider
                                .input_mut(selected)
                                .map(|input| input.backspace())
                                .unwrap_or(false);
                            if changed && selected == ProviderAddField::Id {
                                provider.id_is_manual = true;
                            }
                            if changed
                                && selected == ProviderAddField::Name
                                && !provider.id_is_manual
                            {
                                let existing_ids = data
                                    .providers
                                    .rows
                                    .iter()
                                    .map(|row| row.id.clone())
                                    .collect::<Vec<_>>();
                                provider.id.set(
                                    crate::cli::commands::provider_input::generate_provider_id(
                                        provider.name.value.trim(),
                                        &existing_ids,
                                    ),
                                );
                            }
                            return Action::None;
                        }
                        KeyCode::Delete => {
                            let changed = provider
                                .input_mut(selected)
                                .map(|input| input.delete())
                                .unwrap_or(false);
                            if changed && selected == ProviderAddField::Id {
                                provider.id_is_manual = true;
                            }
                            if changed
                                && selected == ProviderAddField::Name
                                && !provider.id_is_manual
                            {
                                let existing_ids = data
                                    .providers
                                    .rows
                                    .iter()
                                    .map(|row| row.id.clone())
                                    .collect::<Vec<_>>();
                                provider.id.set(
                                    crate::cli::commands::provider_input::generate_provider_id(
                                        provider.name.value.trim(),
                                        &existing_ids,
                                    ),
                                );
                            }
                            return Action::None;
                        }
                        KeyCode::Char(c) => {
                            if c.is_control() {
                                return Action::None;
                            }
                            if selected == ProviderAddField::Notes {
                                let can_insert = provider
                                    .input(selected)
                                    .map(|input| {
                                        input.value.chars().count() < PROVIDER_NOTES_MAX_CHARS
                                    })
                                    .unwrap_or(true);
                                if !can_insert {
                                    return Action::None;
                                }
                            }
                            let changed = provider
                                .input_mut(selected)
                                .map(|input| input.insert_char(c))
                                .unwrap_or(false);
                            if changed && selected == ProviderAddField::Id {
                                provider.id_is_manual = true;
                            }
                            if changed
                                && selected == ProviderAddField::Name
                                && !provider.id_is_manual
                            {
                                let existing_ids = data
                                    .providers
                                    .rows
                                    .iter()
                                    .map(|row| row.id.clone())
                                    .collect::<Vec<_>>();
                                provider.id.set(
                                    crate::cli::commands::provider_input::generate_provider_id(
                                        provider.name.value.trim(),
                                        &existing_ids,
                                    ),
                                );
                            }
                            return Action::None;
                        }
                        _ => {}
                    }
                } else {
                    match key.code {
                        KeyCode::Up => {
                            provider.field_idx = provider.field_idx.saturating_sub(1);
                            while provider.field_idx > 0
                                && matches!(
                                    fields.get(provider.field_idx),
                                    Some(ProviderAddField::CommonConfigDivider)
                                )
                            {
                                provider.field_idx = provider.field_idx.saturating_sub(1);
                            }
                            return Action::None;
                        }
                        KeyCode::Down => {
                            provider.field_idx = (provider.field_idx + 1).min(fields.len() - 1);
                            while provider.field_idx < fields.len().saturating_sub(1)
                                && matches!(
                                    fields.get(provider.field_idx),
                                    Some(ProviderAddField::CommonConfigDivider)
                                )
                            {
                                provider.field_idx = (provider.field_idx + 1).min(fields.len() - 1);
                            }
                            return Action::None;
                        }
                        KeyCode::Char(' ') | KeyCode::Enter => match selected {
                            ProviderAddField::CodexWireApi => {
                                provider.codex_wire_api = match provider.codex_wire_api {
                                    CodexWireApi::Chat => CodexWireApi::Responses,
                                    CodexWireApi::Responses => CodexWireApi::Chat,
                                };
                                return Action::None;
                            }
                            ProviderAddField::CodexRequiresOpenaiAuth => {
                                provider.codex_requires_openai_auth =
                                    !provider.codex_requires_openai_auth;
                                return Action::None;
                            }
                            ProviderAddField::IncludeCommonConfig => {
                                if let Err(err) = provider
                                    .toggle_include_common_config(&data.config.common_snippet)
                                {
                                    self.push_toast(err, ToastKind::Warning);
                                }
                                return Action::None;
                            }
                            ProviderAddField::GeminiAuthType => {
                                provider.gemini_auth_type = match provider.gemini_auth_type {
                                    GeminiAuthType::OAuth => GeminiAuthType::ApiKey,
                                    GeminiAuthType::ApiKey => GeminiAuthType::OAuth,
                                };
                                return Action::None;
                            }
                            ProviderAddField::ClaudeModelConfig => {
                                self.overlay = Overlay::ClaudeModelPicker {
                                    selected: 0,
                                    editing: false,
                                };
                                return Action::None;
                            }
                            ProviderAddField::CommonSnippet => {
                                if matches!(key.code, KeyCode::Enter) {
                                    let app_type = provider.app_type.clone();
                                    self.open_common_snippet_editor(app_type, data, None);
                                }
                                return Action::None;
                            }
                            _ => {
                                if selected == ProviderAddField::Id && !provider.is_id_editable() {
                                    return Action::None;
                                }
                                if provider.input(selected).is_some() {
                                    provider.editing = true;
                                }
                                return Action::None;
                            }
                        },
                        _ => {}
                    }
                }
            } else if provider.focus == FormFocus::JsonPreview {
                if matches!(provider.app_type, AppType::Codex) {
                    match key.code {
                        KeyCode::Enter => match provider.codex_preview_section {
                            super::form::CodexPreviewSection::Auth => {
                                if provider.is_codex_official_provider() {
                                    self.push_toast(
                                        texts::tui_toast_codex_official_auth_json_disabled(),
                                        ToastKind::Warning,
                                    );
                                    return Action::None;
                                }

                                let provider_json = provider.to_provider_json_value();
                                let auth_value = provider_json
                                    .get("settingsConfig")
                                    .and_then(|v| v.get("auth"))
                                    .cloned()
                                    .unwrap_or_else(|| {
                                        serde_json::Value::Object(serde_json::Map::new())
                                    });
                                let auth_value = if auth_value.is_object() {
                                    auth_value
                                } else {
                                    serde_json::Value::Object(serde_json::Map::new())
                                };
                                let content = serde_json::to_string_pretty(&auth_value)
                                    .unwrap_or_else(|_| "{}".to_string());
                                self.open_editor(
                                    texts::tui_codex_auth_json_title(),
                                    EditorKind::Json,
                                    content,
                                    EditorSubmit::ProviderFormApplyCodexAuth,
                                );
                                if let Some(editor) = self.editor.as_mut() {
                                    editor.mode = EditorMode::Edit;
                                }
                                return Action::None;
                            }
                            super::form::CodexPreviewSection::Config => {
                                let provider_json = provider.to_provider_json_value();
                                let config_text = provider_json
                                    .get("settingsConfig")
                                    .and_then(|v| v.get("config"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                self.open_editor(
                                    texts::tui_codex_config_toml_title(),
                                    EditorKind::Plain,
                                    config_text,
                                    EditorSubmit::ProviderFormApplyCodexConfigToml,
                                );
                                if let Some(editor) = self.editor.as_mut() {
                                    editor.mode = EditorMode::Edit;
                                }
                                return Action::None;
                            }
                        },
                        KeyCode::Up => {
                            let scroll = match provider.codex_preview_section {
                                super::form::CodexPreviewSection::Auth => {
                                    &mut provider.codex_auth_scroll
                                }
                                super::form::CodexPreviewSection::Config => {
                                    &mut provider.codex_config_scroll
                                }
                            };
                            *scroll = (*scroll).saturating_sub(1);
                            return Action::None;
                        }
                        KeyCode::Down => {
                            let scroll = match provider.codex_preview_section {
                                super::form::CodexPreviewSection::Auth => {
                                    &mut provider.codex_auth_scroll
                                }
                                super::form::CodexPreviewSection::Config => {
                                    &mut provider.codex_config_scroll
                                }
                            };
                            *scroll = (*scroll).saturating_add(1);
                            return Action::None;
                        }
                        KeyCode::PageUp => {
                            let scroll = match provider.codex_preview_section {
                                super::form::CodexPreviewSection::Auth => {
                                    &mut provider.codex_auth_scroll
                                }
                                super::form::CodexPreviewSection::Config => {
                                    &mut provider.codex_config_scroll
                                }
                            };
                            *scroll = (*scroll).saturating_sub(10);
                            return Action::None;
                        }
                        KeyCode::PageDown => {
                            let scroll = match provider.codex_preview_section {
                                super::form::CodexPreviewSection::Auth => {
                                    &mut provider.codex_auth_scroll
                                }
                                super::form::CodexPreviewSection::Config => {
                                    &mut provider.codex_config_scroll
                                }
                            };
                            *scroll = (*scroll).saturating_add(10);
                            return Action::None;
                        }
                        _ => {}
                    }
                } else {
                    match key.code {
                        KeyCode::Enter => {
                            let provider_json = match provider
                                .to_provider_json_value_with_common_config(
                                    &data.config.common_snippet,
                                ) {
                                Ok(value) => value,
                                Err(err) => {
                                    self.push_toast(err, ToastKind::Error);
                                    return Action::None;
                                }
                            };

                            let settings_value = provider_json
                                .get("settingsConfig")
                                .cloned()
                                .unwrap_or_else(|| {
                                    serde_json::Value::Object(serde_json::Map::new())
                                });
                            let content = serde_json::to_string_pretty(&settings_value)
                                .unwrap_or_else(|_| "{}".to_string());
                            self.open_editor(
                                texts::tui_form_json_title(),
                                EditorKind::Json,
                                content,
                                EditorSubmit::ProviderFormApplyJson,
                            );
                            if let Some(editor) = self.editor.as_mut() {
                                editor.mode = EditorMode::Edit;
                            }
                            return Action::None;
                        }
                        KeyCode::Up => {
                            provider.json_scroll = provider.json_scroll.saturating_sub(1);
                            return Action::None;
                        }
                        KeyCode::Down => {
                            provider.json_scroll = provider.json_scroll.saturating_add(1);
                            return Action::None;
                        }
                        KeyCode::PageUp => {
                            provider.json_scroll = provider.json_scroll.saturating_sub(10);
                            return Action::None;
                        }
                        KeyCode::PageDown => {
                            provider.json_scroll = provider.json_scroll.saturating_add(10);
                            return Action::None;
                        }
                        _ => {}
                    }
                }
            }
        }

        if let FormState::McpAdd(mcp) = form {
            if mcp.focus == FormFocus::Fields {
                let fields = mcp.fields();
                if !fields.is_empty() {
                    mcp.field_idx = mcp.field_idx.min(fields.len() - 1);
                } else {
                    mcp.field_idx = 0;
                }

                let Some(selected) = fields.get(mcp.field_idx).copied() else {
                    return Action::None;
                };

                if mcp.editing {
                    match key.code {
                        KeyCode::Esc | KeyCode::Enter => {
                            mcp.editing = false;
                            return Action::None;
                        }
                        KeyCode::Left => {
                            if let Some(input) = mcp.input_mut(selected) {
                                input.move_left();
                            }
                            return Action::None;
                        }
                        KeyCode::Right => {
                            if let Some(input) = mcp.input_mut(selected) {
                                input.move_right();
                            }
                            return Action::None;
                        }
                        KeyCode::Home => {
                            if let Some(input) = mcp.input_mut(selected) {
                                input.move_home();
                            }
                            return Action::None;
                        }
                        KeyCode::End => {
                            if let Some(input) = mcp.input_mut(selected) {
                                input.move_end();
                            }
                            return Action::None;
                        }
                        KeyCode::Backspace => {
                            let _ = mcp.input_mut(selected).map(|input| input.backspace());
                            return Action::None;
                        }
                        KeyCode::Delete => {
                            let _ = mcp.input_mut(selected).map(|input| input.delete());
                            return Action::None;
                        }
                        KeyCode::Char(c) => {
                            if c.is_control() {
                                return Action::None;
                            }
                            let _ = mcp.input_mut(selected).map(|input| input.insert_char(c));
                            return Action::None;
                        }
                        _ => {}
                    }
                } else {
                    match key.code {
                        KeyCode::Up => {
                            mcp.field_idx = mcp.field_idx.saturating_sub(1);
                            return Action::None;
                        }
                        KeyCode::Down => {
                            mcp.field_idx = (mcp.field_idx + 1).min(fields.len() - 1);
                            return Action::None;
                        }
                        KeyCode::Char(' ') | KeyCode::Enter => match selected {
                            McpAddField::AppClaude => {
                                mcp.apps.claude = !mcp.apps.claude;
                                return Action::None;
                            }
                            McpAddField::AppCodex => {
                                mcp.apps.codex = !mcp.apps.codex;
                                return Action::None;
                            }
                            McpAddField::AppGemini => {
                                mcp.apps.gemini = !mcp.apps.gemini;
                                return Action::None;
                            }
                            _ => {
                                if selected == McpAddField::Id && mcp.locked_id().is_some() {
                                    return Action::None;
                                }
                                if mcp.input(selected).is_some() {
                                    mcp.editing = true;
                                }
                                return Action::None;
                            }
                        },
                        _ => {}
                    }
                }
            } else if mcp.focus == FormFocus::JsonPreview {
                match key.code {
                    KeyCode::Up => {
                        mcp.json_scroll = mcp.json_scroll.saturating_sub(1);
                        return Action::None;
                    }
                    KeyCode::Down => {
                        mcp.json_scroll = mcp.json_scroll.saturating_add(1);
                        return Action::None;
                    }
                    KeyCode::PageUp => {
                        mcp.json_scroll = mcp.json_scroll.saturating_sub(10);
                        return Action::None;
                    }
                    KeyCode::PageDown => {
                        mcp.json_scroll = mcp.json_scroll.saturating_add(10);
                        return Action::None;
                    }
                    _ => {}
                }
            }
        }

        if is_save_shortcut(key) {
            match form {
                FormState::ProviderAdd(provider) => {
                    if !provider.has_required_fields() {
                        if provider.mode.is_edit() {
                            self.push_toast(
                                texts::tui_toast_provider_missing_name(),
                                ToastKind::Warning,
                            );
                            return Action::None;
                        }
                        self.push_toast(
                            texts::tui_toast_provider_add_missing_fields(),
                            ToastKind::Warning,
                        );
                        return Action::None;
                    }

                    let provider_json = if matches!(provider.app_type, AppType::Codex) {
                        provider.to_provider_json_value()
                    } else {
                        match provider
                            .to_provider_json_value_with_common_config(&data.config.common_snippet)
                        {
                            Ok(value) => value,
                            Err(err) => {
                                self.push_toast(err, ToastKind::Error);
                                return Action::None;
                            }
                        }
                    };
                    let content = serde_json::to_string_pretty(&provider_json)
                        .unwrap_or_else(|_| "{}".to_string());

                    return Action::EditorSubmit {
                        submit: match &provider.mode {
                            FormMode::Add => EditorSubmit::ProviderAdd,
                            FormMode::Edit { id } => EditorSubmit::ProviderEdit { id: id.clone() },
                        },
                        content,
                    };
                }
                FormState::McpAdd(mcp) => {
                    if !mcp.has_required_fields() {
                        self.push_toast(texts::tui_toast_mcp_missing_fields(), ToastKind::Warning);
                        return Action::None;
                    }
                    if mcp.command.is_blank() {
                        self.push_toast(texts::tui_toast_command_empty(), ToastKind::Warning);
                        return Action::None;
                    }

                    let content = serde_json::to_string_pretty(&mcp.to_mcp_server_json_value())
                        .unwrap_or_else(|_| "{}".to_string());

                    return Action::EditorSubmit {
                        submit: match &mcp.mode {
                            FormMode::Add => EditorSubmit::McpAdd,
                            FormMode::Edit { id } => EditorSubmit::McpEdit { id: id.clone() },
                        },
                        content,
                    };
                }
            }
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.form = None;
                Action::None
            }
            _ => Action::None,
        }
    }

    fn on_editor_key(&mut self, key: KeyEvent) -> Action {
        let viewport = self.editor_viewport_size();
        let jump_rows = viewport.height as usize;

        let Some(editor) = &mut self.editor else {
            return Action::None;
        };

        if is_save_shortcut(key) {
            return Action::EditorSubmit {
                submit: editor.submit.clone(),
                content: editor.text(),
            };
        }

        match key.code {
            KeyCode::Esc => {
                if editor.is_dirty() {
                    self.overlay = Overlay::Confirm(ConfirmOverlay {
                        title: texts::tui_editor_save_before_close_title().to_string(),
                        message: texts::tui_editor_save_before_close_message().to_string(),
                        action: ConfirmAction::EditorSaveBeforeClose,
                    });
                    Action::None
                } else {
                    self.editor = None;
                    Action::None
                }
            }
            KeyCode::Up => {
                editor.cursor_row = editor.cursor_row.saturating_sub(1);
                editor.cursor_col = editor
                    .cursor_col
                    .min(editor.line_len_chars(editor.cursor_row));
                editor.ensure_cursor_visible(viewport);
                Action::None
            }
            KeyCode::Down => {
                if !editor.lines.is_empty() {
                    editor.cursor_row = (editor.cursor_row + 1).min(editor.lines.len() - 1);
                }
                editor.cursor_col = editor
                    .cursor_col
                    .min(editor.line_len_chars(editor.cursor_row));
                editor.ensure_cursor_visible(viewport);
                Action::None
            }
            KeyCode::Left => {
                if editor.cursor_col > 0 {
                    editor.cursor_col -= 1;
                } else if editor.cursor_row > 0 {
                    editor.cursor_row -= 1;
                    editor.cursor_col = editor.line_len_chars(editor.cursor_row);
                }
                editor.ensure_cursor_visible(viewport);
                Action::None
            }
            KeyCode::Right => {
                let line_len = editor.line_len_chars(editor.cursor_row);
                if editor.cursor_col < line_len {
                    editor.cursor_col += 1;
                } else if editor.cursor_row + 1 < editor.lines.len() {
                    editor.cursor_row += 1;
                    editor.cursor_col = 0;
                }
                editor.ensure_cursor_visible(viewport);
                Action::None
            }
            KeyCode::Home => {
                editor.cursor_col = 0;
                editor.ensure_cursor_visible(viewport);
                Action::None
            }
            KeyCode::End => {
                editor.cursor_col = editor.line_len_chars(editor.cursor_row);
                editor.ensure_cursor_visible(viewport);
                Action::None
            }
            KeyCode::PageUp => {
                editor.scroll = editor.scroll.saturating_sub(jump_rows);
                editor.cursor_row = editor.cursor_row.saturating_sub(jump_rows);
                editor.cursor_col = editor
                    .cursor_col
                    .min(editor.line_len_chars(editor.cursor_row));
                editor.ensure_cursor_visible(viewport);
                Action::None
            }
            KeyCode::PageDown => {
                if !editor.lines.is_empty() {
                    editor.scroll = (editor.scroll + jump_rows).min(editor.lines.len() - 1);
                    editor.cursor_row = (editor.cursor_row + jump_rows).min(editor.lines.len() - 1);
                    editor.cursor_col = editor
                        .cursor_col
                        .min(editor.line_len_chars(editor.cursor_row));
                }
                editor.ensure_cursor_visible(viewport);
                Action::None
            }
            KeyCode::Backspace => {
                editor.backspace();
                editor.ensure_cursor_visible(viewport);
                Action::None
            }
            KeyCode::Delete => {
                editor.delete();
                editor.ensure_cursor_visible(viewport);
                Action::None
            }
            KeyCode::Enter => {
                editor.newline();
                editor.ensure_cursor_visible(viewport);
                Action::None
            }
            KeyCode::Tab => {
                editor.insert_str("  ");
                editor.ensure_cursor_visible(viewport);
                Action::None
            }
            KeyCode::Char(c) => {
                if !c.is_control() {
                    editor.insert_char(c);
                    editor.ensure_cursor_visible(viewport);
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    fn editor_viewport_size(&self) -> Size {
        // Matches `render()` + `render_content()` + `render_editor()` layout math in `ui.rs`.
        let mut width = self.last_size.width.saturating_sub(30);
        let mut height = self.last_size.height.saturating_sub(3).saturating_sub(1);

        if self.filter.active || !self.filter.buffer.trim().is_empty() {
            height = height.saturating_sub(5);
        }

        // render_editor:
        // - outer borders (2)
        // - key bar row (1)
        // - field borders (2)
        width = width.saturating_sub(2).saturating_sub(2);
        height = height.saturating_sub(2).saturating_sub(1).saturating_sub(2);

        Size {
            width: width.max(1),
            height: height.max(1),
        }
    }

    fn clamp_selections(&mut self, data: &UiData) {
        let providers_len = visible_providers(&self.filter, data).len();
        if providers_len == 0 {
            self.provider_idx = 0;
        } else {
            self.provider_idx = self.provider_idx.min(providers_len - 1);
        }

        let mcp_len = visible_mcp(&self.filter, data).len();
        if mcp_len == 0 {
            self.mcp_idx = 0;
        } else {
            self.mcp_idx = self.mcp_idx.min(mcp_len - 1);
        }

        let prompt_len = visible_prompts(&self.filter, data).len();
        if prompt_len == 0 {
            self.prompt_idx = 0;
        } else {
            self.prompt_idx = self.prompt_idx.min(prompt_len - 1);
        }

        let skills_len = visible_skills_installed(&self.filter, data).len();
        if skills_len == 0 {
            self.skills_idx = 0;
        } else {
            self.skills_idx = self.skills_idx.min(skills_len - 1);
        }

        let discover_len =
            visible_skills_discover(&self.filter, &self.skills_discover_results).len();
        if discover_len == 0 {
            self.skills_discover_idx = 0;
        } else {
            self.skills_discover_idx = self.skills_discover_idx.min(discover_len - 1);
        }

        let repos_len = visible_skills_repos(&self.filter, data).len();
        if repos_len == 0 {
            self.skills_repo_idx = 0;
        } else {
            self.skills_repo_idx = self.skills_repo_idx.min(repos_len - 1);
        }

        let unmanaged_len =
            visible_skills_unmanaged(&self.filter, &self.skills_unmanaged_results).len();
        if unmanaged_len == 0 {
            self.skills_unmanaged_idx = 0;
        } else {
            self.skills_unmanaged_idx = self.skills_unmanaged_idx.min(unmanaged_len - 1);
        }

        let config_len = visible_config_items(&self.filter).len();
        if config_len == 0 {
            self.config_idx = 0;
        } else {
            self.config_idx = self.config_idx.min(config_len - 1);
        }

        let config_webdav_len = visible_webdav_config_items(&self.filter).len();
        if config_webdav_len == 0 {
            self.config_webdav_idx = 0;
        } else {
            self.config_webdav_idx = self.config_webdav_idx.min(config_webdav_len - 1);
        }
    }
}

fn route_has_content_list(route: &Route) -> bool {
    matches!(
        route,
        Route::Providers
            | Route::ProviderDetail { .. }
            | Route::Mcp
            | Route::Prompts
            | Route::Config
            | Route::ConfigWebDav
            | Route::Skills
            | Route::SkillsDiscover
            | Route::SkillsRepos
            | Route::SkillsUnmanaged
            | Route::SkillDetail { .. }
            | Route::Settings
    )
}

fn route_default_focus(route: &Route) -> Focus {
    match route {
        Route::Main => Focus::Nav,
        _ => Focus::Content,
    }
}

fn visible_providers<'a>(
    filter: &FilterState,
    data: &'a UiData,
) -> Vec<&'a super::data::ProviderRow> {
    let query = filter.query_lower();
    data.providers
        .rows
        .iter()
        .filter(|row| match &query {
            None => true,
            Some(q) => {
                row.provider.name.to_lowercase().contains(q) || row.id.to_lowercase().contains(q)
            }
        })
        .collect()
}

fn visible_mcp<'a>(filter: &FilterState, data: &'a UiData) -> Vec<&'a super::data::McpRow> {
    let query = filter.query_lower();
    data.mcp
        .rows
        .iter()
        .filter(|row| match &query {
            None => true,
            Some(q) => {
                row.server.name.to_lowercase().contains(q) || row.id.to_lowercase().contains(q)
            }
        })
        .collect()
}

fn visible_prompts<'a>(filter: &FilterState, data: &'a UiData) -> Vec<&'a super::data::PromptRow> {
    let query = filter.query_lower();
    data.prompts
        .rows
        .iter()
        .filter(|row| match &query {
            None => true,
            Some(q) => {
                row.prompt.name.to_lowercase().contains(q) || row.id.to_lowercase().contains(q)
            }
        })
        .collect()
}

fn visible_skills_installed<'a>(
    filter: &FilterState,
    data: &'a UiData,
) -> Vec<&'a crate::services::skill::InstalledSkill> {
    let query = filter.query_lower();
    data.skills
        .installed
        .iter()
        .filter(|skill| match &query {
            None => true,
            Some(q) => {
                skill.name.to_lowercase().contains(q)
                    || skill.directory.to_lowercase().contains(q)
                    || skill.id.to_lowercase().contains(q)
            }
        })
        .collect()
}

fn visible_skills_discover<'a>(
    filter: &FilterState,
    skills: &'a [crate::services::skill::Skill],
) -> Vec<&'a crate::services::skill::Skill> {
    let query = filter.query_lower();
    skills
        .iter()
        .filter(|skill| match &query {
            None => true,
            Some(q) => {
                skill.name.to_lowercase().contains(q)
                    || skill.directory.to_lowercase().contains(q)
                    || skill.key.to_lowercase().contains(q)
            }
        })
        .collect()
}

fn visible_skills_repos<'a>(
    filter: &FilterState,
    data: &'a UiData,
) -> Vec<&'a crate::services::skill::SkillRepo> {
    let query = filter.query_lower();
    data.skills
        .repos
        .iter()
        .filter(|repo| match &query {
            None => true,
            Some(q) => {
                repo.owner.to_lowercase().contains(q)
                    || repo.name.to_lowercase().contains(q)
                    || repo.branch.to_lowercase().contains(q)
            }
        })
        .collect()
}

fn visible_skills_unmanaged<'a>(
    filter: &FilterState,
    skills: &'a [crate::services::skill::UnmanagedSkill],
) -> Vec<&'a crate::services::skill::UnmanagedSkill> {
    let query = filter.query_lower();
    skills
        .iter()
        .filter(|skill| match &query {
            None => true,
            Some(q) => {
                skill.name.to_lowercase().contains(q)
                    || skill.directory.to_lowercase().contains(q)
                    || skill
                        .description
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(q)
                    || skill.found_in.iter().any(|s| s.to_lowercase().contains(q))
            }
        })
        .collect()
}

fn visible_config_items(filter: &FilterState) -> Vec<ConfigItem> {
    let all = ConfigItem::ALL.to_vec();
    let Some(q) = filter.query_lower() else {
        return all;
    };

    all.into_iter()
        .filter(|item| config_item_label(item).to_lowercase().contains(&q))
        .collect()
}

fn config_item_label(item: &ConfigItem) -> &'static str {
    match item {
        ConfigItem::Path => crate::cli::i18n::texts::tui_config_item_show_path(),
        ConfigItem::ShowFull => crate::cli::i18n::texts::tui_config_item_show_full(),
        ConfigItem::Export => crate::cli::i18n::texts::tui_config_item_export(),
        ConfigItem::Import => crate::cli::i18n::texts::tui_config_item_import(),
        ConfigItem::Backup => crate::cli::i18n::texts::tui_config_item_backup(),
        ConfigItem::Restore => crate::cli::i18n::texts::tui_config_item_restore(),
        ConfigItem::Validate => crate::cli::i18n::texts::tui_config_item_validate(),
        ConfigItem::CommonSnippet => crate::cli::i18n::texts::tui_config_item_common_snippet(),
        ConfigItem::WebDavSync => crate::cli::i18n::texts::tui_config_item_webdav_sync(),
        ConfigItem::Reset => crate::cli::i18n::texts::tui_config_item_reset(),
    }
}

fn visible_webdav_config_items(filter: &FilterState) -> Vec<WebDavConfigItem> {
    let all = WebDavConfigItem::ALL.to_vec();
    let Some(q) = filter.query_lower() else {
        return all;
    };

    all.into_iter()
        .filter(|item| webdav_config_item_label(item).to_lowercase().contains(&q))
        .collect()
}

fn webdav_config_item_label(item: &WebDavConfigItem) -> &'static str {
    match item {
        WebDavConfigItem::Settings => crate::cli::i18n::texts::tui_config_item_webdav_settings(),
        WebDavConfigItem::CheckConnection => {
            crate::cli::i18n::texts::tui_config_item_webdav_check_connection()
        }
        WebDavConfigItem::Upload => crate::cli::i18n::texts::tui_config_item_webdav_upload(),
        WebDavConfigItem::Download => crate::cli::i18n::texts::tui_config_item_webdav_download(),
        WebDavConfigItem::Reset => crate::cli::i18n::texts::tui_config_item_webdav_reset(),
        WebDavConfigItem::JianguoyunQuickSetup => {
            crate::cli::i18n::texts::tui_config_item_webdav_jianguoyun_quick_setup()
        }
    }
}

fn cycle_app_type(current: &AppType, dir: i8) -> AppType {
    match (current, dir) {
        (AppType::Claude, 1) => AppType::Codex,
        (AppType::Codex, 1) => AppType::Gemini,
        (AppType::Gemini, 1) => AppType::Claude,
        (AppType::Claude, -1) => AppType::Gemini,
        (AppType::Codex, -1) => AppType::Claude,
        (AppType::Gemini, -1) => AppType::Codex,
        (other, _) => other.clone(),
    }
}

fn app_type_picker_index(app_type: &AppType) -> usize {
    match app_type {
        AppType::Claude => 0,
        AppType::Codex => 1,
        AppType::Gemini => 2,
    }
}

fn app_type_for_picker_index(index: usize) -> AppType {
    match index {
        1 => AppType::Codex,
        2 => AppType::Gemini,
        _ => AppType::Claude,
    }
}

fn snippet_picker_index_for_app_type(app_type: &AppType) -> usize {
    app_type_picker_index(app_type)
}

fn snippet_picker_app_type(index: usize) -> AppType {
    app_type_for_picker_index(index)
}

fn sync_method_picker_index(method: SyncMethod) -> usize {
    match method {
        SyncMethod::Auto => 0,
        SyncMethod::Symlink => 1,
        SyncMethod::Copy => 2,
    }
}

fn sync_method_for_picker_index(index: usize) -> SyncMethod {
    match index {
        1 => SyncMethod::Symlink,
        2 => SyncMethod::Copy,
        _ => SyncMethod::Auto,
    }
}

fn is_save_shortcut(key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('s' | 'S') => key.modifiers.contains(KeyModifiers::CONTROL),
        KeyCode::Char('\u{13}') => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use serde_json::json;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn data() -> UiData {
        UiData::default()
    }

    #[test]
    fn nav_menu_includes_skills_entry() {
        assert!(
            NavItem::ALL
                .iter()
                .any(|item| matches!(item, NavItem::Skills)),
            "Ratatui TUI nav should include a Skills entry"
        );
        assert!(matches!(
            NavItem::ALL[NavItem::ALL.len() - 1],
            NavItem::Exit
        ));
    }

    #[test]
    fn skills_nav_item_routes_to_skills_page() {
        assert_eq!(
            NavItem::Skills.to_route(),
            Some(Route::Skills),
            "Skills nav item should route to the Skills page"
        );
    }

    #[test]
    fn skills_f_does_nothing_when_discover_is_disabled() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Skills;
        app.focus = Focus::Content;

        let action = app.on_key(key(KeyCode::Char('f')), &data());
        assert!(
            matches!(action, Action::None),
            "Discover is disabled; f should do nothing on Skills page"
        );
        assert!(
            matches!(&app.overlay, Overlay::None),
            "Discover is disabled; overlay should stay closed"
        );
    }

    #[test]
    fn skills_i_opens_import_existing_page() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Skills;
        app.focus = Focus::Content;

        let action = app.on_key(key(KeyCode::Char('i')), &data());
        assert!(
            matches!(action, Action::SwitchRoute(Route::SkillsUnmanaged)),
            "i in Skills page should navigate to Import Existing"
        );
    }

    #[test]
    fn config_e_key_opens_common_snippet_picker_when_selected() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Config;
        app.focus = Focus::Content;
        app.config_idx = ConfigItem::ALL
            .iter()
            .position(|item| matches!(item, ConfigItem::CommonSnippet))
            .expect("CommonSnippet missing from ConfigItem::ALL");

        let action = app.on_key(key(KeyCode::Char('e')), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::CommonSnippetPicker { .. }));
    }

    #[test]
    fn app_cycles_left_right() {
        let mut app = App::new(Some(AppType::Claude));
        assert!(matches!(
            app.on_key(key(KeyCode::Char(']')), &data()),
            Action::SetAppType(AppType::Codex)
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('[')), &data()),
            Action::SetAppType(AppType::Gemini)
        ));
    }

    #[test]
    fn q_from_main_opens_exit_confirm_overlay() {
        let mut app = App::new(Some(AppType::Claude));
        assert_eq!(app.route, Route::Main);
        app.on_key(key(KeyCode::Char('q')), &data());
        assert!(matches!(app.overlay, Overlay::Confirm(_)));
    }

    #[test]
    fn provider_add_form_notes_is_length_limited() {
        let mut app = App::new(Some(AppType::Claude));
        app.open_provider_add_form();

        let notes_idx = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form
                .fields()
                .iter()
                .position(|f| *f == ProviderAddField::Notes)
                .expect("Notes field should exist"),
            _ => panic!("provider form should be open"),
        };

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = FormFocus::Fields;
            form.field_idx = notes_idx;
            form.editing = false;
        }

        // Enter edit mode for Notes.
        app.on_key(key(KeyCode::Enter), &data());
        for _ in 0..(PROVIDER_NOTES_MAX_CHARS + 10) {
            app.on_key(key(KeyCode::Char('a')), &data());
        }

        let notes_len = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form.notes.value.chars().count(),
            _ => 0,
        };
        assert_eq!(notes_len, PROVIDER_NOTES_MAX_CHARS);
    }

    #[test]
    fn filter_mode_updates_buffer_and_exits() {
        let mut app = App::new(Some(AppType::Claude));
        assert_eq!(app.filter.active, false);
        app.on_key(key(KeyCode::Char('/')), &data());
        assert_eq!(app.filter.active, true);
        app.on_key(key(KeyCode::Char('a')), &data());
        app.on_key(key(KeyCode::Char('b')), &data());
        assert_eq!(app.filter.buffer, "ab");
        app.on_key(key(KeyCode::Backspace), &data());
        assert_eq!(app.filter.buffer, "a");
        app.on_key(key(KeyCode::Enter), &data());
        assert_eq!(app.filter.active, false);
    }

    #[test]
    fn tab_key_is_noop() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Nav;

        let data = UiData::default();
        let action = app.on_key(key(KeyCode::Tab), &data);
        assert!(matches!(action, Action::None));
        assert_eq!(app.focus, Focus::Nav);
    }

    #[test]
    fn provider_json_editor_hides_internal_fields() {
        let original = json!({
            "id": "p1",
            "name": "demo",
            "meta": {
                "applyCommonConfig": true,
                "custom_endpoints": {
                    "https://example.com": {
                        "url": "https://example.com"
                    }
                }
            },
            "icon": "openai",
            "iconColor": "#00A67E",
            "settingsConfig": {
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "secret-token",
                    "FOO": "bar"
                }
            },
            "createdAt": 123,
            "sortIndex": 9,
            "category": "demo",
            "inFailoverQueue": true
        });

        let display = super::super::form::strip_provider_internal_fields(&original);
        assert!(display.get("createdAt").is_none());
        assert!(display.get("meta").is_none());
        assert!(display.get("icon").is_none());
        assert!(display.get("iconColor").is_none());
        assert!(display.get("sortIndex").is_none());
        assert!(display.get("category").is_none());
        assert!(display.get("inFailoverQueue").is_none());
        assert_eq!(
            display["settingsConfig"]["env"]["ANTHROPIC_AUTH_TOKEN"],
            "secret-token"
        );
    }

    #[test]
    fn providers_enter_key_opens_detail() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
        });

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::SwitchRoute(Route::ProviderDetail { id }) if id == "p1"
        ));
    }

    #[test]
    fn providers_s_key_triggers_switch_action() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
        });

        let action = app.on_key(key(KeyCode::Char('s')), &data);
        assert!(matches!(action, Action::ProviderSwitch { id } if id == "p1"));
    }

    #[test]
    fn provider_detail_s_key_triggers_switch_action_and_enter_is_noop() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ProviderDetail {
            id: "p1".to_string(),
        };
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
        });

        let enter_action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(enter_action, Action::None));

        let action = app.on_key(key(KeyCode::Char('s')), &data);
        assert!(matches!(action, Action::ProviderSwitch { id } if id == "p1"));
    }

    #[test]
    fn mcp_x_key_toggles_current_app() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.mcp.rows.push(super::super::data::McpRow {
            id: "m1".to_string(),
            server: crate::app_config::McpServer {
                id: "m1".to_string(),
                name: "Server".to_string(),
                server: json!({}),
                apps: crate::app_config::McpApps::default(),
                description: None,
                homepage: None,
                docs: None,
                tags: vec![],
            },
        });

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(
            action,
            Action::McpToggle {
                id,
                enabled: true
            } if id == "m1"
        ));
    }

    #[test]
    fn mcp_a_opens_add_form() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let data = UiData::default();
        let action = app.on_key(key(KeyCode::Char('a')), &data);
        assert!(matches!(action, Action::None));
        assert!(
            app.editor.is_none(),
            "MCP 'a' should open the new add form (not the JSON editor)"
        );
    }

    #[test]
    fn mcp_m_opens_apps_picker_overlay() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.mcp.rows.push(super::super::data::McpRow {
            id: "m1".to_string(),
            server: crate::app_config::McpServer {
                id: "m1".to_string(),
                name: "Server".to_string(),
                server: json!({}),
                apps: crate::app_config::McpApps::default(),
                description: None,
                homepage: None,
                docs: None,
                tags: vec![],
            },
        });

        let action = app.on_key(key(KeyCode::Char('m')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::McpAppsPicker {
                id,
                name,
                selected: 1,
                ..
            } if id == "m1" && name == "Server"
        ));
    }

    #[test]
    fn mcp_apps_picker_x_toggles_selected_app_and_enter_emits_action() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.mcp.rows.push(super::super::data::McpRow {
            id: "m1".to_string(),
            server: crate::app_config::McpServer {
                id: "m1".to_string(),
                name: "Server".to_string(),
                server: json!({}),
                apps: crate::app_config::McpApps::default(),
                description: None,
                homepage: None,
                docs: None,
                tags: vec![],
            },
        });

        app.on_key(key(KeyCode::Char('m')), &data);

        let action = app.on_key(key(KeyCode::Char('x')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::McpAppsPicker { apps, .. } if apps.codex
        ));

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::McpSetApps { id, apps } if id == "m1" && apps.codex && !apps.claude && !apps.gemini
        ));
    }

    #[test]
    fn mcp_e_opens_edit_form() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Mcp;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.mcp.rows.push(super::super::data::McpRow {
            id: "m1".to_string(),
            server: crate::app_config::McpServer {
                id: "m1".to_string(),
                name: "Server".to_string(),
                server: json!({"command":"foo","args":[]}),
                apps: crate::app_config::McpApps::default(),
                description: None,
                homepage: None,
                docs: None,
                tags: vec![],
            },
        });

        let action = app.on_key(key(KeyCode::Char('e')), &data);
        assert!(matches!(action, Action::None));
        assert!(app.editor.is_none());
        assert!(app.form.is_some());
    }

    #[test]
    fn prompts_a_key_triggers_activate_action() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.prompts.rows.push(super::super::data::PromptRow {
            id: "pr1".to_string(),
            prompt: crate::prompt::Prompt {
                id: "pr1".to_string(),
                name: "My Prompt".to_string(),
                content: "Hello".to_string(),
                description: None,
                enabled: false,
                created_at: None,
                updated_at: None,
            },
        });

        let action = app.on_key(key(KeyCode::Char('a')), &data);
        assert!(matches!(action, Action::PromptActivate { id } if id == "pr1"));
    }

    #[test]
    fn back_from_provider_detail_returns_to_providers() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
        });

        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::SwitchRoute(Route::ProviderDetail { .. })
        ));
        assert!(matches!(app.route, Route::ProviderDetail { .. }));

        assert!(matches!(
            app.on_key(key(KeyCode::Esc), &data),
            Action::SwitchRoute(Route::Providers)
        ));
        assert_eq!(app.route, Route::Providers);
    }

    #[test]
    fn config_common_snippet_picker_and_view_support_edit_clear_apply_actions() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Config;
        app.focus = Focus::Content;
        app.config_idx = ConfigItem::ALL
            .iter()
            .position(|item| matches!(item, ConfigItem::CommonSnippet))
            .expect("CommonSnippet missing from ConfigItem::ALL");

        let data = UiData::default();
        app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(app.overlay, Overlay::CommonSnippetPicker { .. }));

        // Picker default should be the current app type (Claude). Enter opens the preview overlay.
        app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            app.overlay,
            Overlay::CommonSnippetView {
                app_type: AppType::Claude,
                ..
            }
        ));

        assert!(matches!(
            app.on_key(key(KeyCode::Char('a')), &data),
            Action::ConfigCommonSnippetApply {
                app_type: AppType::Claude
            }
        ));
        assert!(matches!(
            app.on_key(key(KeyCode::Char('c')), &data),
            Action::ConfigCommonSnippetClear {
                app_type: AppType::Claude
            }
        ));

        let action = app.on_key(key(KeyCode::Char('e')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.editor.as_ref().map(|e| e.kind),
            Some(EditorKind::Json)
        ));
    }

    #[test]
    fn config_common_snippet_picker_shows_snippet_for_non_current_app() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Config;
        app.focus = Focus::Content;
        app.config_idx = ConfigItem::ALL
            .iter()
            .position(|item| matches!(item, ConfigItem::CommonSnippet))
            .expect("CommonSnippet missing from ConfigItem::ALL");

        let mut data = UiData::default();
        data.config.common_snippets.codex = Some("disable_response_storage = true".to_string());

        app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(app.overlay, Overlay::CommonSnippetPicker { .. }));

        app.on_key(key(KeyCode::Down), &data); // Claude -> Codex
        app.on_key(key(KeyCode::Enter), &data);

        let snippet = match &app.overlay {
            Overlay::CommonSnippetView {
                app_type: AppType::Codex,
                view,
            } => view.lines.join("\n"),
            other => panic!("expected Codex snippet view, got {other:?}"),
        };
        assert!(
            snippet.contains("disable_response_storage"),
            "expected Codex snippet content to be loaded from snapshot"
        );
    }

    #[test]
    fn provider_add_form_codex_tab_cycles_fields_auth_config_templates() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields

        app.on_key(key(KeyCode::Tab), &data); // fields -> auth preview
        let (focus, section) = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => (form.focus, form.codex_preview_section),
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(focus, super::super::form::FormFocus::JsonPreview);
        assert_eq!(section, super::super::form::CodexPreviewSection::Auth);

        app.on_key(key(KeyCode::Tab), &data); // auth preview -> config preview
        let (focus, section) = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => (form.focus, form.codex_preview_section),
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(focus, super::super::form::FormFocus::JsonPreview);
        assert_eq!(section, super::super::form::CodexPreviewSection::Config);

        app.on_key(key(KeyCode::Tab), &data); // config preview -> templates
        let focus = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form.focus,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(focus, super::super::form::FormFocus::Templates);
    }

    #[test]
    fn provider_add_form_codex_preview_left_right_do_not_switch_panes() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        app.on_key(key(KeyCode::Tab), &data); // fields -> auth preview

        app.on_key(key(KeyCode::Right), &data);
        let section = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form.codex_preview_section,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(section, super::super::form::CodexPreviewSection::Auth);

        app.on_key(key(KeyCode::Left), &data);
        let section = match app.form.as_ref() {
            Some(FormState::ProviderAdd(form)) => form.codex_preview_section,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(section, super::super::form::CodexPreviewSection::Auth);
    }

    #[test]
    fn provider_add_form_common_snippet_row_opens_editor_claude() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.editing = false;
            let fields = form.fields();
            form.field_idx = fields
                .iter()
                .position(|f| *f == ProviderAddField::CommonSnippet)
                .expect("CommonSnippet field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            app.editor.as_ref().map(|e| (&e.kind, &e.submit)),
            Some((
                EditorKind::Json,
                EditorSubmit::ConfigCommonSnippet {
                    app_type: AppType::Claude
                }
            ))
        ));
    }

    #[test]
    fn provider_add_form_common_snippet_row_opens_editor_codex() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.editing = false;
            let fields = form.fields();
            form.field_idx = fields
                .iter()
                .position(|f| *f == ProviderAddField::CommonSnippet)
                .expect("CommonSnippet field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            app.editor.as_ref().map(|e| (&e.kind, &e.submit)),
            Some((
                EditorKind::Plain,
                EditorSubmit::ConfigCommonSnippet {
                    app_type: AppType::Codex
                }
            ))
        ));
    }

    #[test]
    fn provider_add_form_common_snippet_row_opens_editor_gemini() {
        let mut app = App::new(Some(AppType::Gemini));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields

        if let Some(FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.editing = false;
            let fields = form.fields();
            form.field_idx = fields
                .iter()
                .position(|f| *f == ProviderAddField::CommonSnippet)
                .expect("CommonSnippet field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            app.editor.as_ref().map(|e| (&e.kind, &e.submit)),
            Some((
                EditorKind::Json,
                EditorSubmit::ConfigCommonSnippet {
                    app_type: AppType::Gemini
                }
            ))
        ));
    }

    #[test]
    fn provider_add_form_codex_preview_enter_opens_auth_editor() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        app.on_key(key(KeyCode::Tab), &data); // fields -> preview

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.editor.as_ref().map(|e| (&e.kind, &e.submit)),
            Some((EditorKind::Json, EditorSubmit::ProviderFormApplyCodexAuth))
        ));
    }

    #[test]
    fn provider_add_form_codex_preview_tab_then_enter_opens_config_editor() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        app.on_key(key(KeyCode::Tab), &data); // fields -> preview
        app.on_key(key(KeyCode::Tab), &data); // auth -> config

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.editor.as_ref().map(|e| (&e.kind, &e.submit)),
            Some((
                EditorKind::Plain,
                EditorSubmit::ProviderFormApplyCodexConfigToml
            ))
        ));
    }

    #[test]
    fn provider_add_form_codex_preview_c_does_not_open_common_snippet_view() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.common_snippet = "disable_response_storage = true".to_string();

        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        app.on_key(key(KeyCode::Tab), &data); // fields -> preview

        let action = app.on_key(key(KeyCode::Char('c')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.editor.is_none());
    }

    #[test]
    fn provider_add_form_codex_official_auth_enter_does_not_open_editor() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Right), &data); // select OpenAI Official
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        app.on_key(key(KeyCode::Tab), &data); // fields -> preview

        app.on_key(key(KeyCode::Enter), &data); // try to edit auth
        assert!(app.editor.is_none());
        assert!(
            app.toast.is_some(),
            "should show a toast explaining auth is disabled"
        );
    }

    #[test]
    fn config_webdav_item_opens_second_level_menu() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Config;
        app.focus = Focus::Content;
        app.config_idx = ConfigItem::ALL
            .iter()
            .position(|item| matches!(item, ConfigItem::WebDavSync))
            .expect("WebDavSync missing from ConfigItem::ALL");

        let data = UiData::default();
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::SwitchRoute(Route::ConfigWebDav)));
        assert!(matches!(app.route, Route::ConfigWebDav));
    }

    #[test]
    fn config_webdav_settings_opens_json_editor_in_second_level_menu() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ConfigWebDav;
        app.focus = Focus::Content;
        app.config_webdav_idx = WebDavConfigItem::ALL
            .iter()
            .position(|item| matches!(item, WebDavConfigItem::Settings))
            .expect("Settings missing from WebDavConfigItem::ALL");

        let mut data = UiData::default();
        data.config.webdav_sync = Some(crate::settings::WebDavSyncSettings {
            enabled: true,
            base_url: "https://dav.example.com".to_string(),
            ..crate::settings::WebDavSyncSettings::default()
        });

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.editor.as_ref().map(|e| &e.submit),
            Some(EditorSubmit::ConfigWebDavSettings)
        ));
    }

    #[test]
    fn config_webdav_submenu_items_emit_expected_actions() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ConfigWebDav;
        app.focus = Focus::Content;
        let data = UiData::default();

        let check_idx = WebDavConfigItem::ALL
            .iter()
            .position(|item| matches!(item, WebDavConfigItem::CheckConnection))
            .expect("WebDavCheckConnection missing");
        app.config_webdav_idx = check_idx;
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::ConfigWebDavCheckConnection
        ));

        let upload_idx = WebDavConfigItem::ALL
            .iter()
            .position(|item| matches!(item, WebDavConfigItem::Upload))
            .expect("WebDavUpload missing");
        app.config_webdav_idx = upload_idx;
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::ConfigWebDavUpload
        ));

        let download_idx = WebDavConfigItem::ALL
            .iter()
            .position(|item| matches!(item, WebDavConfigItem::Download))
            .expect("WebDavDownload missing");
        app.config_webdav_idx = download_idx;
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::ConfigWebDavDownload
        ));

        let reset_idx = WebDavConfigItem::ALL
            .iter()
            .position(|item| matches!(item, WebDavConfigItem::Reset))
            .expect("WebDavReset missing");
        app.config_webdav_idx = reset_idx;
        assert!(matches!(
            app.on_key(key(KeyCode::Enter), &data),
            Action::ConfigWebDavReset
        ));

        assert_eq!(
            WebDavConfigItem::ALL.len(),
            6,
            "WebDav submenu should include Jianguoyun quick setup"
        );
    }

    #[test]
    fn config_webdav_quick_setup_requires_username_then_password() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ConfigWebDav;
        app.focus = Focus::Content;

        let quick_setup_idx = WebDavConfigItem::ALL
            .iter()
            .position(|item| matches!(item, WebDavConfigItem::JianguoyunQuickSetup))
            .expect("JianguoyunQuickSetup missing");
        app.config_webdav_idx = quick_setup_idx;

        let data = UiData::default();
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::WebDavJianguoyunUsername,
                ..
            })
        ));

        if let Overlay::TextInput(ref mut input) = app.overlay {
            input.buffer = "demo@nutstore.com".to_string();
        }
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::WebDavJianguoyunPassword,
                secret: true,
                ..
            })
        ));

        if let Overlay::TextInput(ref mut input) = app.overlay {
            input.buffer = "app-password".to_string();
        }
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            action,
            Action::ConfigWebDavJianguoyunQuickSetup {
                username,
                password
            } if username == "demo@nutstore.com" && password == "app-password"
        ));
    }

    #[test]
    fn config_webdav_quick_setup_empty_inputs_keep_prompt_open() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::ConfigWebDav;
        app.focus = Focus::Content;

        let quick_setup_idx = WebDavConfigItem::ALL
            .iter()
            .position(|item| matches!(item, WebDavConfigItem::JianguoyunQuickSetup))
            .expect("JianguoyunQuickSetup missing");
        app.config_webdav_idx = quick_setup_idx;

        let data = UiData::default();
        let _ = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::WebDavJianguoyunUsername,
                ..
            })
        ));

        if let Overlay::TextInput(ref mut input) = app.overlay {
            input.buffer = "   ".to_string();
        }
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::WebDavJianguoyunUsername,
                ..
            })
        ));

        if let Overlay::TextInput(ref mut input) = app.overlay {
            input.buffer = "demo@nutstore.com".to_string();
        }
        let _ = app.on_key(key(KeyCode::Enter), &data);
        if let Overlay::TextInput(ref mut input) = app.overlay {
            input.buffer = "   ".to_string();
        }
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::TextInput(TextInputState {
                submit: TextSubmit::WebDavJianguoyunPassword,
                secret: true,
                ..
            })
        ));
    }

    #[test]
    fn prompts_e_opens_editor_and_ctrl_s_submits() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.prompts.rows.push(super::super::data::PromptRow {
            id: "pr1".to_string(),
            prompt: crate::prompt::Prompt {
                id: "pr1".to_string(),
                name: "Demo".to_string(),
                content: "hello".to_string(),
                description: None,
                enabled: false,
                created_at: None,
                updated_at: None,
            },
        });

        let action = app.on_key(key(KeyCode::Char('e')), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.editor.as_ref().map(|e| &e.submit),
            Some(EditorSubmit::PromptEdit { id }) if id == "pr1"
        ));

        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);
        assert!(matches!(
            submit,
            Action::EditorSubmit {
                submit: EditorSubmit::PromptEdit { .. },
                content
            } if content.contains("hello")
        ));
    }

    #[test]
    fn prompts_editor_ctrl_shift_s_submits() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.prompts.rows.push(super::super::data::PromptRow {
            id: "pr1".to_string(),
            prompt: crate::prompt::Prompt {
                id: "pr1".to_string(),
                name: "Demo".to_string(),
                content: "hello".to_string(),
                description: None,
                enabled: false,
                created_at: None,
                updated_at: None,
            },
        });

        let action = app.on_key(key(KeyCode::Char('e')), &data);
        assert!(matches!(action, Action::None));
        let submit = app.on_key(
            KeyEvent::new(KeyCode::Char('S'), KeyModifiers::CONTROL),
            &data,
        );
        assert!(
            matches!(
                submit,
                Action::EditorSubmit {
                    submit: EditorSubmit::PromptEdit { .. },
                    ..
                }
            ),
            "Ctrl+Shift+S should be accepted as save shortcut in editor"
        );
    }

    #[test]
    fn prompts_editor_ctrl_s_control_char_submits() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.prompts.rows.push(super::super::data::PromptRow {
            id: "pr1".to_string(),
            prompt: crate::prompt::Prompt {
                id: "pr1".to_string(),
                name: "Demo".to_string(),
                content: "hello".to_string(),
                description: None,
                enabled: false,
                created_at: None,
                updated_at: None,
            },
        });

        let action = app.on_key(key(KeyCode::Char('e')), &data);
        assert!(matches!(action, Action::None));
        let submit = app.on_key(key(KeyCode::Char('\u{13}')), &data);
        assert!(
            matches!(
                submit,
                Action::EditorSubmit {
                    submit: EditorSubmit::PromptEdit { .. },
                    ..
                }
            ),
            "ASCII XOFF control char should be accepted as save shortcut in editor"
        );
    }

    #[test]
    fn prompts_editor_esc_dirty_opens_save_before_close_confirm() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.prompts.rows.push(super::super::data::PromptRow {
            id: "pr1".to_string(),
            prompt: crate::prompt::Prompt {
                id: "pr1".to_string(),
                name: "Demo".to_string(),
                content: "hello".to_string(),
                description: None,
                enabled: false,
                created_at: None,
                updated_at: None,
            },
        });

        app.on_key(key(KeyCode::Char('e')), &data);
        app.on_key(key(KeyCode::Char('x')), &data);
        let action = app.on_key(key(KeyCode::Esc), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::EditorSaveBeforeClose,
                ..
            })
        ));
    }

    #[test]
    fn prompts_editor_save_confirm_yes_submits_changes() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.prompts.rows.push(super::super::data::PromptRow {
            id: "pr1".to_string(),
            prompt: crate::prompt::Prompt {
                id: "pr1".to_string(),
                name: "Demo".to_string(),
                content: "hello".to_string(),
                description: None,
                enabled: false,
                created_at: None,
                updated_at: None,
            },
        });

        app.on_key(key(KeyCode::Char('e')), &data);
        app.on_key(key(KeyCode::Char('x')), &data);
        app.on_key(key(KeyCode::Esc), &data);

        let action = app.on_key(key(KeyCode::Char('y')), &data);
        assert!(
            matches!(
                action,
                Action::EditorSubmit {
                    submit: EditorSubmit::PromptEdit { .. },
                    content
                } if content.starts_with("xhello")
            ),
            "confirm yes should save current editor content"
        );
    }

    #[test]
    fn prompts_editor_save_confirm_no_discards_and_closes() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Prompts;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.prompts.rows.push(super::super::data::PromptRow {
            id: "pr1".to_string(),
            prompt: crate::prompt::Prompt {
                id: "pr1".to_string(),
                name: "Demo".to_string(),
                content: "hello".to_string(),
                description: None,
                enabled: false,
                created_at: None,
                updated_at: None,
            },
        });

        app.on_key(key(KeyCode::Char('e')), &data);
        app.on_key(key(KeyCode::Char('x')), &data);
        app.on_key(key(KeyCode::Esc), &data);

        let action = app.on_key(key(KeyCode::Char('n')), &data);
        assert!(matches!(action, Action::None));
        assert!(
            app.editor.is_none(),
            "confirm no should discard and close editor"
        );
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn providers_e_opens_edit_form_and_ctrl_s_submits() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
        });

        let action = app.on_key(key(KeyCode::Char('e')), &data);
        assert!(matches!(action, Action::None));
        assert!(app.editor.is_none());
        assert!(app.form.is_some());

        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);
        assert!(matches!(
            submit,
            Action::EditorSubmit {
                submit: EditorSubmit::ProviderEdit { .. },
                content
            } if content.contains("\"id\"") && content.contains("Provider One")
        ));
    }

    #[test]
    fn provider_edit_form_tab_cycles_between_fields_and_json() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.providers.rows.push(super::super::data::ProviderRow {
            id: "p1".to_string(),
            provider: crate::provider::Provider::with_id(
                "p1".to_string(),
                "Provider One".to_string(),
                json!({"env":{"ANTHROPIC_BASE_URL":"https://example.com"}}),
                None,
            ),
            api_url: Some("https://example.com".to_string()),
            is_current: false,
        });

        app.on_key(key(KeyCode::Char('e')), &data);

        let focus = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.focus,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(focus, super::super::form::FormFocus::Fields);

        app.on_key(key(KeyCode::Tab), &data);
        let focus = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.focus,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(focus, super::super::form::FormFocus::JsonPreview);

        app.on_key(key(KeyCode::Tab), &data);
        let focus = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.focus,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(focus, super::super::form::FormFocus::Fields);
    }

    #[test]
    fn providers_a_opens_add_form() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        let action = app.on_key(key(KeyCode::Char('a')), &data);
        assert!(matches!(action, Action::None));
        assert!(
            app.editor.is_none(),
            "Providers 'a' should open the new add form (not the JSON editor)"
        );

        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);
        assert!(
            !matches!(submit, Action::EditorSubmit { .. }),
            "Provider add form should validate fields before submitting"
        );
    }

    #[test]
    fn provider_add_form_tab_cycles_focus() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);

        let focus = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.focus,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(focus, super::super::form::FormFocus::Templates);

        app.on_key(key(KeyCode::Tab), &data);
        let focus = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.focus,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(focus, super::super::form::FormFocus::Fields);
    }

    #[test]
    fn provider_add_form_right_moves_template_selection() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);

        let idx = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.template_idx,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(idx, 0);

        app.on_key(key(KeyCode::Right), &data);
        let idx = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.template_idx,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(idx, 1);
    }

    #[test]
    fn provider_add_form_enter_applies_template_and_focuses_fields() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(app.editor.is_none());
        let focus = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => form.focus,
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(focus, super::super::form::FormFocus::Fields);
    }

    #[test]
    fn provider_add_form_json_focus_enter_opens_json_editor() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        app.on_key(key(KeyCode::Tab), &data); // fields -> json

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(
            app.editor.is_some(),
            "Enter on provider JSON preview should open in-app JSON editor"
        );
        assert!(matches!(
            app.editor.as_ref().map(|editor| &editor.submit),
            Some(EditorSubmit::ProviderFormApplyJson)
        ));
        assert!(
            matches!(
                app.editor.as_ref().map(|editor| editor.mode),
                Some(EditorMode::Edit)
            ),
            "Enter on provider JSON preview should directly enter edit mode"
        );
        let content = app
            .editor
            .as_ref()
            .map(|editor| editor.text())
            .unwrap_or_default();
        assert!(
            !content.contains("\"id\""),
            "provider id should not be exposed in settingsConfig JSON editor"
        );
        assert!(
            !content.contains("\"name\""),
            "provider name should not be exposed in settingsConfig JSON editor"
        );
    }

    #[test]
    fn provider_json_editor_single_enter_then_ctrl_s_submits_edited_content() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        app.on_key(key(KeyCode::Tab), &data); // fields -> json
        app.on_key(key(KeyCode::Enter), &data); // json -> editor(edit mode)

        let original = app
            .editor
            .as_ref()
            .map(|editor| editor.text())
            .expect("editor should be opened");
        assert!(!original.starts_with(' '));

        // Edit immediately (without pressing Enter again) then submit.
        app.on_key(key(KeyCode::Char(' ')), &data);
        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);

        let Action::EditorSubmit { submit, content } = submit else {
            panic!("Ctrl+S in JSON editor should submit edited content");
        };
        assert!(
            matches!(submit, EditorSubmit::ProviderFormApplyJson),
            "JSON editor submit should apply back to provider form"
        );
        assert!(
            content.starts_with(' '),
            "submitted content should include the in-editor change made right after opening"
        );
    }

    #[test]
    fn provider_json_editor_ctrl_s_applies_unknown_fields_back_to_form() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields
        app.on_key(key(KeyCode::Tab), &data); // fields -> json
        app.on_key(key(KeyCode::Enter), &data); // json -> editor

        // Replace the whole JSON with a value that contains an unknown key inside settingsConfig.
        let injected = r#"{
  "env": {
    "ANTHROPIC_BASE_URL": "https://after.example"
  },
  "unknownField": "kept"
}"#;
        if let Some(editor) = app.editor.as_mut() {
            editor.lines = injected.lines().map(|s| s.to_string()).collect();
            editor.cursor_row = 0;
            editor.cursor_col = 0;
            editor.scroll = 0;
        } else {
            panic!("expected editor to be open");
        }

        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);
        let Action::EditorSubmit { submit, content } = submit else {
            panic!("expected EditorSubmit action");
        };
        assert!(matches!(submit, EditorSubmit::ProviderFormApplyJson));

        // Simulate main-loop handling of the submit to apply it back to the form.
        let settings_value: serde_json::Value = serde_json::from_str(&content).expect("valid json");
        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            let mut provider_value = form.to_provider_json_value();
            if let Some(obj) = provider_value.as_object_mut() {
                obj.insert("settingsConfig".to_string(), settings_value);
            }
            form.apply_provider_json_value_to_fields(provider_value)
                .expect("apply should succeed");
        } else {
            panic!("expected ProviderAdd form");
        }
        app.editor = None;

        // Re-open the JSON editor and ensure the unknown field is still present.
        app.on_key(key(KeyCode::Enter), &data);
        let reopened = app
            .editor
            .as_ref()
            .map(|editor| editor.text())
            .unwrap_or_default();
        assert!(
            reopened.contains("\"unknownField\""),
            "unknownField should be preserved after applying JSON back to form"
        );
        assert!(
            reopened.contains("\"kept\""),
            "unknownField value should be preserved after applying JSON back to form"
        );
    }

    #[test]
    fn provider_form_ctrl_s_merges_common_snippet_into_submitted_json() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.common_snippet = r#"{"alwaysThinkingEnabled":false,"statusLine":{"type":"command","command":"~/.claude/statusline.sh","padding":0}}"#.to_string();

        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.id.set("p1");
            form.name.set("Provider One");
            form.include_common_config = true;
            form.claude_base_url.set("https://api.example.com");
        } else {
            panic!("expected ProviderAdd form");
        }

        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);
        assert!(matches!(submit, Action::EditorSubmit { .. }));
        let Action::EditorSubmit { content, .. } = submit else {
            unreachable!("expected submit action");
        };
        assert!(
            content.contains("\"alwaysThinkingEnabled\""),
            "submitted provider JSON should include merged common snippet keys when enabled"
        );
        assert!(
            content.contains("\"statusLine\""),
            "submitted provider JSON should include nested common snippet keys when enabled"
        );
    }

    #[test]
    fn provider_form_ctrl_s_does_not_merge_common_snippet_for_codex() {
        let mut app = App::new(Some(AppType::Codex));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let mut data = UiData::default();
        data.config.common_snippet = "network_access = true".to_string();

        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data); // apply template -> fields

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.id.set("p1");
            form.name.set("Provider One");
            form.include_common_config = true;
            form.codex_base_url.set("https://api.example.com/v1");
        } else {
            panic!("expected ProviderAdd form");
        }

        let submit = app.on_key(ctrl(KeyCode::Char('s')), &data);
        assert!(matches!(submit, Action::EditorSubmit { .. }));
        let Action::EditorSubmit { content, .. } = submit else {
            unreachable!("expected submit action");
        };
        assert!(
            !content.contains("network_access"),
            "submitted Codex provider JSON should not include merged common snippet TOML"
        );
    }

    #[test]
    fn provider_claude_model_config_field_enter_opens_overlay() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.editing = false;
            form.field_idx = form
                .fields()
                .iter()
                .position(|field| *field == ProviderAddField::ClaudeModelConfig)
                .expect("ClaudeModelConfig field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        let action = app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(
            app.overlay,
            Overlay::ClaudeModelPicker {
                selected: 0,
                editing: false
            }
        ));
    }

    #[test]
    fn claude_model_overlay_editing_updates_form_value() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.editing = false;
            form.field_idx = form
                .fields()
                .iter()
                .position(|field| *field == ProviderAddField::ClaudeModelConfig)
                .expect("ClaudeModelConfig field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Enter), &data);
        app.on_key(key(KeyCode::Enter), &data); // enter editing mode in overlay
        app.on_key(key(KeyCode::Char('m')), &data);
        app.on_key(key(KeyCode::Char('1')), &data);

        let model = match app.form.as_ref() {
            Some(super::super::form::FormState::ProviderAdd(form)) => {
                form.claude_model.value.clone()
            }
            other => panic!("expected ProviderAdd form, got: {other:?}"),
        };
        assert_eq!(model, "m1");
    }

    #[test]
    fn claude_model_overlay_esc_closes_without_exiting_parent_form() {
        let mut app = App::new(Some(AppType::Claude));
        app.route = Route::Providers;
        app.focus = Focus::Content;

        let data = UiData::default();
        app.on_key(key(KeyCode::Char('a')), &data);
        app.on_key(key(KeyCode::Enter), &data);

        if let Some(super::super::form::FormState::ProviderAdd(form)) = app.form.as_mut() {
            form.focus = super::super::form::FormFocus::Fields;
            form.editing = false;
            form.field_idx = form
                .fields()
                .iter()
                .position(|field| *field == ProviderAddField::ClaudeModelConfig)
                .expect("ClaudeModelConfig field should exist");
        } else {
            panic!("expected ProviderAdd form");
        }

        app.on_key(key(KeyCode::Enter), &data);
        assert!(matches!(app.overlay, Overlay::ClaudeModelPicker { .. }));

        let action = app.on_key(key(KeyCode::Esc), &data);
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(matches!(app.form, Some(FormState::ProviderAdd(_))));
    }

    #[test]
    fn update_available_overlay_left_right_switches_selection() {
        let mut app = App::new(None);
        app.overlay = Overlay::UpdateAvailable {
            current: "4.7.0".to_string(),
            latest: "v9.9.9".to_string(),
            selected: 0,
        };

        let action = app.on_key(key(KeyCode::Right), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::UpdateAvailable { selected: 1, .. }
        ));

        let action = app.on_key(key(KeyCode::Left), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::UpdateAvailable { selected: 0, .. }
        ));
    }

    #[test]
    fn update_available_overlay_up_down_does_not_switch_selection() {
        let mut app = App::new(None);
        app.overlay = Overlay::UpdateAvailable {
            current: "4.7.0".to_string(),
            latest: "v9.9.9".to_string(),
            selected: 0,
        };

        let action = app.on_key(key(KeyCode::Down), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::UpdateAvailable { selected: 0, .. }
        ));

        let action = app.on_key(key(KeyCode::Up), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(
            &app.overlay,
            Overlay::UpdateAvailable { selected: 0, .. }
        ));
    }

    #[test]
    fn update_check_loading_overlay_esc_emits_cancel_action() {
        let mut app = App::new(None);
        app.overlay = Overlay::Loading {
            kind: LoadingKind::UpdateCheck,
            title: texts::tui_update_checking_title().to_string(),
            message: "Working...".to_string(),
        };

        let action = app.on_key(key(KeyCode::Esc), &data());
        assert!(matches!(action, Action::CancelUpdateCheck));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn update_result_overlay_success_esc_hides_without_exiting() {
        let mut app = App::new(None);
        app.overlay = Overlay::UpdateResult {
            success: true,
            message: "ok".to_string(),
        };

        let action = app.on_key(key(KeyCode::Esc), &data());
        assert!(matches!(action, Action::None));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(
            !app.should_quit,
            "Esc should hide the success result overlay without exiting"
        );
    }

    #[test]
    fn update_result_overlay_success_enter_exits() {
        let mut app = App::new(None);
        app.overlay = Overlay::UpdateResult {
            success: true,
            message: "ok".to_string(),
        };

        let action = app.on_key(key(KeyCode::Enter), &data());
        assert!(matches!(action, Action::None));
        assert!(
            app.should_quit,
            "Enter should exit after a successful update"
        );
    }
}
