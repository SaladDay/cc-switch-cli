use super::*;

impl App {
    fn open_openclaw_editor<T: serde::Serialize>(
        &mut self,
        title: &'static str,
        section: Option<&T>,
        submit: EditorSubmit,
    ) {
        let initial = section
            .map(|section| {
                serde_json::to_string_pretty(section).unwrap_or_else(|_| "{}".to_string())
            })
            .unwrap_or_else(|| "{}".to_string());
        self.open_editor(title, EditorKind::Json, initial, submit);
    }

    pub(crate) fn open_daily_memory_filename_prompt(&mut self, initial: String) {
        self.overlay = Overlay::TextInput(TextInputState {
            title: texts::tui_openclaw_daily_memory_create_title().to_string(),
            prompt: texts::tui_openclaw_daily_memory_create_prompt().to_string(),
            buffer: initial,
            submit: TextSubmit::OpenClawDailyMemoryFilename,
            secret: false,
        });
    }

    fn today_daily_memory_filename() -> String {
        chrono::Local::now().format("%Y-%m-%d.md").to_string()
    }

    pub(crate) fn on_config_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        let items = visible_config_items(&self.filter, &self.app_type);
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
                            action: None,
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
                    ConfigItem::Proxy => Action::ConfigOpenProxyHelp,
                    ConfigItem::OpenClawWorkspace
                    | ConfigItem::OpenClawEnv
                    | ConfigItem::OpenClawTools
                    | ConfigItem::OpenClawAgents => self.push_route_and_switch(
                        item.detail_route()
                            .expect("OpenClaw config item should define a detail route"),
                    ),
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

    pub(crate) fn on_config_openclaw_workspace_key(
        &mut self,
        key: KeyEvent,
        _data: &UiData,
    ) -> Action {
        match key.code {
            KeyCode::Up => {
                self.workspace_idx = self.workspace_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                self.workspace_idx =
                    (self.workspace_idx + 1).min(openclaw_workspace_entry_count() - 1);
                Action::None
            }
            KeyCode::Enter | KeyCode::Char('e') => {
                match openclaw_workspace_row(self.workspace_idx) {
                    Some(OpenClawWorkspaceRow::DailyMemory) => {
                        self.push_route_and_switch(Route::ConfigOpenClawDailyMemory)
                    }
                    Some(OpenClawWorkspaceRow::File(filename)) => {
                        Action::OpenClawWorkspaceOpenFile {
                            filename: filename.to_string(),
                        }
                    }
                    None => Action::None,
                }
            }
            KeyCode::Char('o') => Action::OpenClawOpenDirectory {
                subdir: String::new(),
            },
            _ => Action::None,
        }
    }

    pub(crate) fn on_config_openclaw_daily_memory_key(
        &mut self,
        key: KeyEvent,
        data: &UiData,
    ) -> Action {
        let visible = visible_openclaw_daily_memory(self, data);
        match key.code {
            KeyCode::Up => {
                self.daily_memory_idx = self.daily_memory_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                if !visible.is_empty() {
                    self.daily_memory_idx = (self.daily_memory_idx + 1).min(visible.len() - 1);
                }
                Action::None
            }
            KeyCode::Enter | KeyCode::Char('e') => {
                let Some(row) = visible.get(self.daily_memory_idx) else {
                    return Action::None;
                };
                Action::OpenClawDailyMemoryOpenFile {
                    filename: row.filename().to_string(),
                }
            }
            KeyCode::Char('a') => {
                self.open_daily_memory_filename_prompt(Self::today_daily_memory_filename());
                Action::None
            }
            KeyCode::Char('d') => {
                let Some(row) = visible.get(self.daily_memory_idx) else {
                    return Action::None;
                };
                let filename = row.filename().to_string();
                self.overlay = Overlay::Confirm(ConfirmOverlay {
                    title: texts::tui_openclaw_daily_memory_delete_title().to_string(),
                    message: texts::tui_openclaw_daily_memory_delete_message(&filename),
                    action: ConfirmAction::OpenClawDailyMemoryDelete { filename },
                });
                Action::None
            }
            KeyCode::Char('o') => Action::OpenClawOpenDirectory {
                subdir: "memory".to_string(),
            },
            _ => Action::None,
        }
    }

    pub(crate) fn on_config_openclaw_env_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        match key.code {
            KeyCode::Enter | KeyCode::Char('e') => {
                self.open_openclaw_editor(
                    texts::tui_openclaw_config_env_editor_title(),
                    data.config.openclaw_env.as_ref(),
                    EditorSubmit::ConfigOpenClawEnv,
                );
                Action::None
            }
            _ => Action::None,
        }
    }

    fn openclaw_tools_form(&mut self, data: &UiData) -> &mut OpenClawToolsFormState {
        self.openclaw_tools_form.get_or_insert_with(|| {
            OpenClawToolsFormState::from_snapshot(data.config.openclaw_tools.as_ref())
        })
    }

    fn submit_openclaw_tools_form(&self) -> Action {
        let Some(form) = self.openclaw_tools_form.as_ref() else {
            return Action::None;
        };

        let content =
            serde_json::to_string_pretty(&form.to_config()).unwrap_or_else(|_| "{}".to_string());
        Action::EditorSubmit {
            submit: EditorSubmit::ConfigOpenClawTools,
            content,
        }
    }

    fn try_submit_openclaw_tools_form(&mut self, data: &UiData) -> Action {
        if super::openclaw_tools_has_blocking_warning(data) {
            self.push_toast(
                texts::tui_toast_openclaw_tools_save_blocked_parse_error(),
                ToastKind::Error,
            );
            Action::None
        } else {
            self.submit_openclaw_tools_form()
        }
    }

    pub(super) fn mutate_openclaw_tools_form<F>(&mut self, data: &UiData, mutate: F) -> Action
    where
        F: FnOnce(&mut OpenClawToolsFormState),
    {
        let changed = {
            let form = self.openclaw_tools_form(data);
            let before = form.clone();
            mutate(form);
            *form != before
        };

        if changed {
            self.try_submit_openclaw_tools_form(data)
        } else {
            Action::None
        }
    }

    fn open_openclaw_tools_profile_picker(&mut self, data: &UiData) -> Action {
        let selected = super::openclaw_tools_profile_picker_index(
            self.openclaw_tools_form(data).profile.as_deref(),
        );
        self.overlay = Overlay::OpenClawToolsProfilePicker { selected };
        Action::None
    }

    pub(super) fn open_openclaw_tools_rule_editor(
        &mut self,
        section: OpenClawToolsSection,
        row: Option<usize>,
        initial: String,
    ) -> Action {
        let title = match section {
            OpenClawToolsSection::Allow => texts::tui_openclaw_tools_allow_list_label(),
            OpenClawToolsSection::Deny => texts::tui_openclaw_tools_deny_list_label(),
            OpenClawToolsSection::Profile => return Action::None,
        };

        self.overlay = Overlay::TextInput(TextInputState {
            title: title.to_string(),
            prompt: texts::tui_openclaw_tools_pattern_placeholder().to_string(),
            buffer: initial,
            submit: TextSubmit::OpenClawToolsRule { section, row },
            secret: false,
        });
        Action::None
    }

    fn open_current_openclaw_tools_rule_editor(&mut self, data: &UiData) -> Action {
        let (section, row, initial) = {
            let form = self.openclaw_tools_form(data);
            (
                form.section,
                form.selected_rule_row(),
                form.selected_rule_value().unwrap_or_default().to_string(),
            )
        };
        self.open_openclaw_tools_rule_editor(section, row, initial)
    }

    pub(crate) fn on_config_openclaw_tools_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        if super::openclaw_tools_load_failed(data) {
            self.openclaw_tools_form = None;
            if matches!(key.code, KeyCode::Enter) {
                self.push_toast(
                    texts::tui_toast_openclaw_tools_save_blocked_parse_error(),
                    ToastKind::Error,
                );
            }
            return Action::None;
        }

        let section = self.openclaw_tools_form(data).section;
        match key.code {
            KeyCode::Up => {
                self.openclaw_tools_form(data).move_up();
                Action::None
            }
            KeyCode::Down => {
                self.openclaw_tools_form(data).move_down();
                Action::None
            }
            KeyCode::Enter | KeyCode::Char('e') => match section {
                OpenClawToolsSection::Profile => self.open_openclaw_tools_profile_picker(data),
                OpenClawToolsSection::Allow | OpenClawToolsSection::Deny => {
                    self.open_current_openclaw_tools_rule_editor(data)
                }
            },
            KeyCode::Delete | KeyCode::Backspace => {
                self.mutate_openclaw_tools_form(data, |form| form.remove_current_list_item())
            }
            _ => Action::None,
        }
    }

    fn openclaw_agents_form(&mut self, data: &UiData) -> &mut OpenClawAgentsFormState {
        self.openclaw_agents_form.get_or_insert_with(|| {
            OpenClawAgentsFormState::from_snapshot(data.config.openclaw_agents_defaults.as_ref())
        })
    }

    fn submit_openclaw_agents_form(&self) -> Action {
        let Some(form) = self.openclaw_agents_form.as_ref() else {
            return Action::None;
        };

        let content =
            serde_json::to_string_pretty(&form.to_config()).unwrap_or_else(|_| "{}".to_string());
        Action::EditorSubmit {
            submit: EditorSubmit::ConfigOpenClawAgents,
            content,
        }
    }

    fn try_submit_openclaw_agents_form(&mut self, data: &UiData) -> Action {
        let message = if super::openclaw_agents_has_blocking_warning(data) {
            Some(texts::tui_toast_openclaw_agents_save_blocked_parse_error())
        } else if self
            .openclaw_agents_form(data)
            .has_unmigratable_legacy_timeout()
        {
            Some(texts::tui_toast_openclaw_agents_save_blocked_legacy_timeout())
        } else {
            None
        };

        if let Some(message) = message {
            self.push_toast(message, ToastKind::Error);
            Action::None
        } else {
            self.submit_openclaw_agents_form()
        }
    }

    pub(super) fn mutate_openclaw_agents_form<F>(&mut self, data: &UiData, mutate: F) -> Action
    where
        F: FnOnce(&mut OpenClawAgentsFormState),
    {
        let changed = {
            let form = self.openclaw_agents_form(data);
            let before = form.clone();
            mutate(form);
            *form != before
        };

        if changed {
            self.try_submit_openclaw_agents_form(data)
        } else {
            Action::None
        }
    }

    pub(super) fn submit_openclaw_agents_runtime_popup_field(
        &mut self,
        data: &UiData,
        field: OpenClawAgentsRuntimeField,
        raw: String,
    ) -> Action {
        if raw.trim().is_empty() {
            return Action::None;
        }

        let changed = {
            let form = self.openclaw_agents_form(data);
            let before = form.runtime_field_value(field).to_string();
            if before == raw {
                false
            } else {
                form.set_runtime_field(field, raw);
                true
            }
        };

        if !changed {
            return Action::None;
        }

        if super::openclaw_agents_has_blocking_warning(data) {
            self.push_toast(
                texts::tui_toast_openclaw_agents_save_blocked_parse_error(),
                ToastKind::Error,
            );
            Action::None
        } else {
            self.submit_openclaw_agents_form()
        }
    }

    fn open_openclaw_agents_runtime_editor(&mut self, data: &UiData) -> Action {
        let (field, title, buffer) = {
            let form = self.openclaw_agents_form(data);
            let Some(field) = form.selected_runtime_field() else {
                return Action::None;
            };
            let title = match field {
                OpenClawAgentsRuntimeField::Workspace => texts::tui_openclaw_agents_workspace(),
                OpenClawAgentsRuntimeField::Timeout => texts::tui_openclaw_agents_timeout(),
                OpenClawAgentsRuntimeField::ContextTokens => {
                    texts::tui_openclaw_agents_context_tokens()
                }
                OpenClawAgentsRuntimeField::MaxConcurrent => {
                    texts::tui_openclaw_agents_max_concurrent()
                }
            };

            (field, title, form.runtime_field_value(field).to_string())
        };

        self.overlay = Overlay::TextInput(TextInputState {
            title: title.to_string(),
            prompt: title.to_string(),
            buffer,
            submit: TextSubmit::OpenClawAgentsRuntimeField { field },
            secret: false,
        });
        Action::None
    }

    fn open_openclaw_agents_model_picker(&mut self, data: &UiData) -> Action {
        let model_options = super::openclaw_agents_model_options(data);
        let Some((insert_at, selected, options)) = ({
            let form = self.openclaw_agents_form(data);
            match form.section {
                OpenClawAgentsSection::PrimaryModel => (!model_options.is_empty()).then(|| {
                    (
                        0,
                        form.primary_model_picker_selection(&model_options),
                        model_options.clone(),
                    )
                }),
                OpenClawAgentsSection::FallbackModels => {
                    let row = form.row.min(form.fallbacks.len());
                    if row < form.fallbacks.len() {
                        let options = form.available_fallback_options_for_row(row, &model_options);
                        (!options.is_empty()).then(|| {
                            (
                                row,
                                form.current_fallback_picker_selection(row, &options),
                                options,
                            )
                        })
                    } else {
                        let options = form.available_fallback_options(&model_options);
                        (!options.is_empty()).then(|| (row, 0, options))
                    }
                }
                OpenClawAgentsSection::Runtime => None,
            }
        }) else {
            return Action::None;
        };

        self.overlay = Overlay::OpenClawAgentsFallbackPicker {
            insert_at,
            selected,
            options,
        };
        Action::None
    }

    fn skip_disabled_openclaw_agents_add_row(&mut self, data: &UiData, moving_down: bool) {
        let disabled_add_row_selected = {
            let model_options = super::openclaw_agents_model_options(data);
            let form = self.openclaw_agents_form(data);
            form.section == OpenClawAgentsSection::FallbackModels
                && form.row == form.fallbacks.len()
                && form.available_fallback_options(&model_options).is_empty()
        };

        if disabled_add_row_selected {
            let form = self.openclaw_agents_form(data);
            if moving_down {
                form.move_down();
            } else {
                form.move_up();
            }
        }
    }

    pub(crate) fn on_config_openclaw_agents_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        if super::openclaw_agents_load_failed(data) {
            self.openclaw_agents_form = None;
            if matches!(key.code, KeyCode::Enter) {
                self.push_toast(
                    texts::tui_toast_openclaw_agents_save_blocked_parse_error(),
                    ToastKind::Error,
                );
            }
            return Action::None;
        }
        let section = self.openclaw_agents_form(data).section;

        match key.code {
            KeyCode::Up => {
                self.openclaw_agents_form(data).move_up();
                self.skip_disabled_openclaw_agents_add_row(data, false);
                Action::None
            }
            KeyCode::Down => {
                self.openclaw_agents_form(data).move_down();
                self.skip_disabled_openclaw_agents_add_row(data, true);
                Action::None
            }
            KeyCode::Enter => match section {
                OpenClawAgentsSection::PrimaryModel | OpenClawAgentsSection::FallbackModels => {
                    self.open_openclaw_agents_model_picker(data)
                }
                OpenClawAgentsSection::Runtime => self.open_openclaw_agents_runtime_editor(data),
            },
            KeyCode::Delete | KeyCode::Backspace => match section {
                OpenClawAgentsSection::PrimaryModel => {
                    self.mutate_openclaw_agents_form(data, |form| form.clear_primary_model())
                }
                OpenClawAgentsSection::FallbackModels => {
                    self.mutate_openclaw_agents_form(data, |form| form.remove_current_fallback())
                }
                OpenClawAgentsSection::Runtime => self.mutate_openclaw_agents_form(data, |form| {
                    if let Some(field) = form.selected_runtime_field() {
                        form.clear_runtime_field(field);
                    }
                }),
            },
            _ => Action::None,
        }
    }

    pub(crate) fn on_config_webdav_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
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

    pub(crate) fn on_settings_key(&mut self, key: KeyEvent, _data: &UiData) -> Action {
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
                Some(SettingsItem::VisibleApps) => {
                    self.overlay = Overlay::VisibleAppsPicker {
                        selected: app_type_picker_index(&self.app_type),
                        apps: crate::settings::get_visible_apps(),
                    };
                    Action::None
                }
                Some(SettingsItem::OpenClawConfigDir) => {
                    let buffer = crate::settings::get_settings()
                        .openclaw_config_dir
                        .unwrap_or_default();
                    self.overlay = Overlay::TextInput(TextInputState {
                        title: texts::tui_settings_openclaw_config_dir_label().to_string(),
                        prompt: texts::tui_settings_openclaw_config_dir_prompt().to_string(),
                        buffer,
                        submit: TextSubmit::SettingsOpenClawConfigDir,
                        secret: false,
                    });
                    Action::None
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
                Some(SettingsItem::ClaudePluginIntegration) => {
                    let current = crate::settings::get_enable_claude_plugin_integration();
                    let next = !current;
                    let path = match crate::claude_plugin::claude_config_path() {
                        Ok(path) => path,
                        Err(_) => std::path::PathBuf::from("~/.claude/config.json"),
                    };

                    self.overlay = Overlay::Confirm(ConfirmOverlay {
                        title: texts::tui_confirm_title().to_string(),
                        message: texts::enable_claude_plugin_integration_confirm(
                            next,
                            path.to_string_lossy().as_ref(),
                        ),
                        action: ConfirmAction::SettingsSetClaudePluginIntegration { enabled: next },
                    });
                    Action::None
                }
                Some(SettingsItem::Proxy) => self.push_route_and_switch(Route::SettingsProxy),
                Some(SettingsItem::UpstreamProxy) => self.push_route_and_switch(Route::SettingsUpstreamProxy),
                Some(SettingsItem::CheckForUpdates) => Action::CheckUpdate,
                None => Action::None,
            },
            _ => Action::None,
        }
    }

    pub(crate) fn on_settings_proxy_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        let items_len = LocalProxySettingsItem::ALL.len();
        match key.code {
            KeyCode::Up => {
                self.settings_proxy_idx = self.settings_proxy_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                self.settings_proxy_idx = (self.settings_proxy_idx + 1).min(items_len - 1);
                Action::None
            }
            KeyCode::Enter => {
                if data.proxy.running {
                    self.push_toast(
                        texts::tui_toast_proxy_settings_stop_before_edit(),
                        ToastKind::Info,
                    );
                    return Action::None;
                }

                match LocalProxySettingsItem::ALL.get(self.settings_proxy_idx) {
                    Some(LocalProxySettingsItem::ListenAddress) => {
                        self.overlay = Overlay::TextInput(TextInputState {
                            title: texts::tui_settings_proxy_title().to_string(),
                            prompt: texts::tui_settings_proxy_listen_address_prompt().to_string(),
                            buffer: data.proxy.configured_listen_address.clone(),
                            submit: TextSubmit::SettingsProxyListenAddress,
                            secret: false,
                        });
                        Action::None
                    }
                    Some(LocalProxySettingsItem::ListenPort) => {
                        self.overlay = Overlay::TextInput(TextInputState {
                            title: texts::tui_settings_proxy_title().to_string(),
                            prompt: texts::tui_settings_proxy_listen_port_prompt().to_string(),
                            buffer: data.proxy.configured_listen_port.to_string(),
                            submit: TextSubmit::SettingsProxyListenPort,
                            secret: false,
                        });
                        Action::None
                    }
                    None => Action::None,
                }
            }
            _ => Action::None,
        }
    }

    pub(crate) fn on_settings_upstream_proxy_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        // We have two rows: URL (0) and Status (1)
        match key.code {
            KeyCode::Up => {
                self.settings_proxy_idx = self.settings_proxy_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                self.settings_proxy_idx = (self.settings_proxy_idx + 1).min(1); // Only 2 rows
                Action::None
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                match self.settings_proxy_idx {
                    0 => { // URL row - open text input
                        let buffer = data.proxy.upstream_proxy_url.clone().unwrap_or_default();
                        self.overlay = Overlay::TextInput(TextInputState {
                            title: texts::tui_settings_upstream_proxy_title().to_string(),
                            prompt: texts::tui_settings_upstream_proxy_url_prompt().to_string(),
                            buffer,
                            submit: TextSubmit::SettingsUpstreamProxyUrl,
                            secret: false,
                        });
                        Action::None
                    }
                    1 => { // Status row - toggle enabled state
                        Action::SetUpstreamProxyEnabled {
                            enabled: !data.proxy.upstream_proxy_enabled
                        }
                    }
                    _ => Action::None,
                }
            }
            KeyCode::Char('e') => {
                // Edit key - only works for URL row
                if self.settings_proxy_idx == 0 {
                    let buffer = data.proxy.upstream_proxy_url.clone().unwrap_or_default();
                    self.overlay = Overlay::TextInput(TextInputState {
                        title: texts::tui_settings_upstream_proxy_title().to_string(),
                        prompt: texts::tui_settings_upstream_proxy_url_prompt().to_string(),
                        buffer,
                        submit: TextSubmit::SettingsUpstreamProxyUrl,
                        secret: false,
                    });
                }
                Action::None
            }
            _ => Action::None,
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

    pub(crate) fn common_snippet_text_for(&self, app_type: &AppType, data: &UiData) -> String {
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

    pub(crate) fn open_common_snippet_view(&mut self, app_type: AppType, data: &UiData) {
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
                action: None,
            },
        };
    }

    pub(crate) fn open_proxy_help_view(
        &mut self,
        data: &UiData,
        config: Option<&crate::proxy::ProxyConfig>,
    ) {
        let current_provider = if data.providers.current_id.trim().is_empty() {
            crate::t!("(not set)", "（未设置）").to_string()
        } else {
            data.providers.current_id.clone()
        };

        let runtime_state = if data.proxy.running {
            crate::t!("running", "运行中")
        } else {
            crate::t!("stopped", "未运行")
        };
        let current_takeover = data.proxy.takeover_enabled_for(&self.app_type);
        let current_app_routed = data.proxy.routes_current_app_through_proxy(&self.app_type);
        let proxy_action_available = current_app_routed.is_some_and(|current_app_routed| {
            !data.proxy.running || data.proxy.managed_runtime || current_app_routed
        });
        let takeover_state = match current_takeover {
            Some(true) => crate::t!("active", "已接管"),
            Some(false) => crate::t!("inactive", "未接管"),
            None => crate::t!("not supported", "不支持"),
        };
        let toggle_action = match current_app_routed {
            Some(true) if proxy_action_available => Some(TextViewAction::ProxyToggleTakeover {
                app_type: self.app_type.clone(),
                enabled: false,
            }),
            Some(false) if proxy_action_available => Some(TextViewAction::ProxyToggleTakeover {
                app_type: self.app_type.clone(),
                enabled: true,
            }),
            _ => None,
        };

        let mut lines = vec![
            crate::t!(
                "Manual takeover status for the foreground proxy.",
                "前台代理的手动接管状态。"
            )
            .to_string(),
            String::new(),
            format!(
                "{}: {}",
                crate::t!("Current app", "当前应用"),
                self.app_type.as_str()
            ),
            format!(
                "{}: {}",
                crate::t!("Current provider", "当前供应商"),
                current_provider
            ),
            format!(
                "{}: {}",
                crate::t!("Foreground runtime", "前台运行态"),
                runtime_state
            ),
            format!(
                "{}: {}",
                crate::t!("Current app takeover", "当前应用接管"),
                takeover_state
            ),
            crate::t!(
                "Manual takeover only. Automatic failover is disabled.",
                "仅支持手动接管，不提供自动故障转移。"
            )
            .to_string(),
        ];

        if let Some(config) = config {
            lines.extend([
                format!(
                    "{}: {}:{}",
                    crate::t!("Listen", "监听"),
                    config.listen_address,
                    config.listen_port
                ),
                format!(
                    "{}: {}",
                    crate::t!("Global proxy switch", "全局代理开关"),
                    if data.proxy.enabled {
                        crate::t!("enabled", "开启")
                    } else {
                        crate::t!("disabled", "关闭")
                    }
                ),
            ]);
        } else {
            lines.push(
                crate::t!(
                    "Proxy configuration is unavailable.",
                    "代理配置暂时不可用。"
                )
                .to_string(),
            );
        }

        lines.push(String::new());
        lines.push(match current_app_routed {
            Some(true) => crate::t!(
                "Press T to restore the current app to its live config.",
                "按 T 恢复当前应用的 live 配置。"
            )
            .to_string(),
            Some(false) if !proxy_action_available => {
                texts::tui_proxy_dashboard_running_elsewhere().to_string()
            }
            Some(false) if data.proxy.running => crate::t!(
                "Press T to route the current app through the running managed proxy.",
                "按 T 让当前应用接入正在运行的托管代理。"
            )
            .to_string(),
            Some(false) => crate::t!(
                "Press T to start the managed proxy and route the current app through cc-switch.",
                "按 T 启动托管代理，并让当前应用走 cc-switch。"
            )
            .to_string(),
            None => crate::t!(
                "This app does not support proxy takeover in the TUI.",
                "这个应用暂不支持在 TUI 中进行代理接管。"
            )
            .to_string(),
        });

        if matches!(self.app_type, AppType::Claude) {
            lines.push(String::new());
            lines.push(crate::t!("Manual Claude setup:", "Claude 手动接线：").to_string());
            if let Some(config) = config {
                lines.push(format!(
                    "{}: cc-switch proxy serve --listen-address {} --listen-port {}",
                    crate::t!("Foreground command", "前台命令"),
                    config.listen_address,
                    config.listen_port
                ));
                lines.push(format!(
                    "ANTHROPIC_BASE_URL=http://{}:{}",
                    config.listen_address, config.listen_port
                ));
            } else {
                lines.push(format!(
                    "{}: cc-switch proxy serve",
                    crate::t!("Foreground command", "前台命令")
                ));
                lines.push("ANTHROPIC_BASE_URL=http://127.0.0.1:3456".to_string());
            }
            lines.extend([
                "ANTHROPIC_AUTH_TOKEN=proxy-placeholder".to_string(),
                crate::t!(
                    "Keep the real upstream base URL and key in the selected Claude provider inside cc-switch.",
                    "真实上游地址和密钥仍保存在 cc-switch 里当前选中的 Claude provider。"
                )
                .to_string(),
            ]);
        }

        self.overlay = Overlay::TextView(TextViewState {
            title: texts::tui_config_item_proxy().to_string(),
            lines,
            scroll: 0,
            action: toggle_action,
        });
    }

    pub(crate) fn open_common_snippet_editor(
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

    pub(crate) fn open_provider_add_form(&mut self) {
        self.filter.active = false;
        self.overlay = Overlay::None;
        self.focus = Focus::Content;
        self.editor = None;
        self.form = Some(FormState::ProviderAdd(ProviderAddFormState::new(
            self.app_type.clone(),
        )));
    }

    pub(crate) fn open_provider_edit_form(&mut self, row: &super::data::ProviderRow) {
        self.filter.active = false;
        self.overlay = Overlay::None;
        self.focus = Focus::Content;
        self.editor = None;
        self.form = Some(FormState::ProviderAdd(ProviderAddFormState::from_provider(
            self.app_type.clone(),
            &row.provider,
        )));
    }

    pub(crate) fn open_mcp_add_form(&mut self) {
        self.filter.active = false;
        self.overlay = Overlay::None;
        self.focus = Focus::Content;
        self.editor = None;
        let mut state = McpAddFormState::new();
        state.apps.set_enabled_for(&self.app_type, true);
        state.rebase_initial_snapshot();
        self.form = Some(FormState::McpAdd(state));
    }

    pub(crate) fn open_mcp_edit_form(&mut self, row: &super::data::McpRow) {
        self.filter.active = false;
        self.overlay = Overlay::None;
        self.focus = Focus::Content;
        self.editor = None;
        self.form = Some(FormState::McpAdd(McpAddFormState::from_server(&row.server)));
    }
}
