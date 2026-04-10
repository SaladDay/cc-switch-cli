use super::*;

impl App {
    pub(super) fn handle_dialog_overlay_key(
        &mut self,
        key: KeyEvent,
        data: &UiData,
    ) -> Option<Action> {
        if let Some(action) = self.handle_provider_switch_first_use_overlay_key(key) {
            return Some(action);
        }

        if let Some(action) = self.handle_confirm_overlay_key(key, data) {
            return Some(action);
        }

        if let Some(action) = self.handle_text_input_overlay_key(key, data) {
            return Some(action);
        }

        None
    }

    fn handle_provider_switch_first_use_overlay_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Overlay::ProviderSwitchFirstUseConfirm {
            provider_id,
            selected,
            ..
        } = &mut self.overlay
        else {
            return None;
        };

        Some(match key.code {
            KeyCode::Left | KeyCode::BackTab => {
                *selected = selected.saturating_sub(1);
                Action::None
            }
            KeyCode::Right | KeyCode::Tab => {
                *selected = (*selected + 1).min(2);
                Action::None
            }
            KeyCode::Enter => {
                let action = match *selected {
                    0 => Action::ProviderImportLiveConfig,
                    1 => Action::ProviderSwitchForce {
                        id: provider_id.clone(),
                    },
                    _ => Action::None,
                };
                self.close_overlay();
                action
            }
            KeyCode::Esc => {
                self.close_overlay();
                Action::None
            }
            _ => Action::None,
        })
    }

    fn handle_confirm_overlay_key(&mut self, key: KeyEvent, data: &UiData) -> Option<Action> {
        let Overlay::Confirm(confirm) = &self.overlay else {
            return None;
        };
        let confirm = confirm.clone();

        let action = match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                let action = match &confirm.action {
                    ConfirmAction::Quit => Action::Quit,
                    ConfirmAction::ProviderDelete { id } => {
                        Action::ProviderDelete { id: id.clone() }
                    }
                    ConfirmAction::McpDelete { id } => Action::McpDelete { id: id.clone() },
                    ConfirmAction::PromptDelete { id } => Action::PromptDelete { id: id.clone() },
                    ConfirmAction::SkillsUninstall { directory } => Action::SkillsUninstall {
                        directory: directory.clone(),
                    },
                    ConfirmAction::SkillsRepoRemove { owner, name } => Action::SkillsRepoRemove {
                        owner: owner.clone(),
                        name: name.clone(),
                    },
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
                    ConfirmAction::SettingsSetClaudePluginIntegration { enabled } => {
                        Action::SetClaudePluginIntegration { enabled: *enabled }
                    }
                    ConfirmAction::SettingsClearUpstreamProxy => {
                        Action::SetUpstreamProxyUrl { url: None }
                    }
                    ConfirmAction::ProviderApiFormatProxyNotice => Action::None,
                    ConfirmAction::ProviderSwitchSharedConfigNotice => Action::None,
                    ConfirmAction::OpenClawDailyMemoryDelete { filename } => {
                        Action::OpenClawDailyMemoryDelete {
                            filename: filename.clone(),
                        }
                    }
                    ConfirmAction::FormSaveBeforeClose => self.handle_form_save_shortcut(data),
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
                    ConfirmAction::WebDavMigrateV1ToV2 => Action::ConfigWebDavMigrateV1ToV2,
                };
                self.close_overlay();
                action
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                if matches!(confirm.action, ConfirmAction::EditorSaveBeforeClose) {
                    self.editor = None;
                }
                if matches!(confirm.action, ConfirmAction::FormSaveBeforeClose) {
                    self.form = None;
                }
                self.close_overlay();
                Action::None
            }
            KeyCode::Esc => {
                self.close_overlay();
                Action::None
            }
            _ => Action::None,
        };

        Some(action)
    }

    fn handle_text_input_overlay_key(&mut self, key: KeyEvent, data: &UiData) -> Option<Action> {
        let Overlay::TextInput(input) = &self.overlay else {
            return None;
        };
        let submit = input.submit;

        let action = match key.code {
            KeyCode::Esc => {
                if matches!(
                    submit,
                    TextSubmit::WebDavJianguoyunUsername | TextSubmit::WebDavJianguoyunPassword
                ) {
                    self.webdav_quick_setup_username = None;
                }
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Enter => {
                let raw = match &self.overlay {
                    Overlay::TextInput(input) => input.buffer.trim().to_string(),
                    _ => String::new(),
                };
                self.overlay = Overlay::None;
                self.handle_text_input_submit(submit, raw, data)
            }
            KeyCode::Backspace => {
                if let Overlay::TextInput(input) = &mut self.overlay {
                    input.buffer.pop();
                }
                Action::None
            }
            KeyCode::Char(c) => {
                if !c.is_control() && !key.modifiers.contains(KeyModifiers::CONTROL) {
                    if let Overlay::TextInput(input) = &mut self.overlay {
                        input.buffer.push(c);
                    }
                }
                Action::None
            }
            _ => Action::None,
        };

        Some(action)
    }

    fn handle_text_input_submit(
        &mut self,
        submit: TextSubmit,
        raw: String,
        data: &UiData,
    ) -> Action {
        match submit {
            TextSubmit::ConfigExport => {
                if raw.is_empty() {
                    self.push_toast(texts::tui_toast_export_path_empty(), ToastKind::Warning);
                    return Action::None;
                }
                Action::ConfigExport { path: raw }
            }
            TextSubmit::ConfigImport => {
                if raw.is_empty() {
                    self.push_toast(texts::tui_toast_import_path_empty(), ToastKind::Warning);
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
            TextSubmit::SettingsProxyListenAddress => {
                self.handle_settings_proxy_listen_address_submit(data, raw)
            }
            TextSubmit::SettingsProxyListenPort => {
                self.handle_settings_proxy_listen_port_submit(data, raw)
            }
            TextSubmit::SettingsUpstreamProxyUrl => {
                self.handle_settings_upstream_proxy_url_submit(data, raw)
            }
            TextSubmit::SettingsOpenClawConfigDir => {
                let trimmed = raw.trim().to_string();
                let path = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                };
                Action::SetOpenClawConfigDir { path }
            }
            TextSubmit::SkillsInstallSpec => {
                if raw.is_empty() {
                    self.push_toast(texts::tui_toast_skill_spec_empty(), ToastKind::Warning);
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
                    self.push_toast(texts::tui_toast_repo_spec_empty(), ToastKind::Warning);
                    return Action::None;
                }
                Action::SkillsRepoAdd { spec: raw }
            }
            TextSubmit::OpenClawDailyMemoryFilename => {
                let filename = raw.trim().to_string();
                let regex = regex::Regex::new(r"^\d{4}-\d{2}-\d{2}\.md$")
                    .expect("valid daily memory regex");
                if !regex.is_match(&filename) {
                    self.push_toast(
                        texts::tui_openclaw_daily_memory_invalid_filename(),
                        ToastKind::Warning,
                    );
                    self.open_daily_memory_filename_prompt(filename);
                    return Action::None;
                }
                Action::OpenClawDailyMemoryOpenFile { filename }
            }
            TextSubmit::OpenClawAgentsRuntimeField { field } => {
                self.handle_openclaw_agents_runtime_submit(field, raw, data)
            }
            TextSubmit::OpenClawToolsRule { section, row } => {
                self.handle_openclaw_tools_rule_submit(section, row, raw, data)
            }
            TextSubmit::WebDavJianguoyunUsername => self.handle_webdav_username_submit(raw),
            TextSubmit::WebDavJianguoyunPassword => self.handle_webdav_password_submit(raw),
        }
    }

    fn handle_openclaw_agents_runtime_submit(
        &mut self,
        field: OpenClawAgentsRuntimeField,
        raw: String,
        data: &UiData,
    ) -> Action {
        self.submit_openclaw_agents_runtime_popup_field(data, field, raw)
    }

    fn handle_openclaw_tools_rule_submit(
        &mut self,
        section: OpenClawToolsSection,
        row: Option<usize>,
        raw: String,
        data: &UiData,
    ) -> Action {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            self.push_toast(
                texts::tui_toast_openclaw_tools_rule_empty(),
                ToastKind::Warning,
            );
            return self.open_openclaw_tools_rule_editor(section, row, raw);
        }

        let value = trimmed.to_string();
        self.mutate_openclaw_tools_form(data, move |form| {
            form.upsert_rule(section, row, value);
        })
    }

    fn handle_webdav_username_submit(&mut self, raw: String) -> Action {
        if raw.is_empty() {
            self.push_toast(texts::tui_toast_webdav_username_empty(), ToastKind::Warning);
            self.overlay = Overlay::TextInput(TextInputState {
                title: texts::tui_webdav_jianguoyun_setup_title().to_string(),
                prompt: texts::tui_webdav_jianguoyun_username_prompt().to_string(),
                buffer: String::new(),
                submit: TextSubmit::WebDavJianguoyunUsername,
                secret: false,
            });
            return Action::None;
        }

        self.webdav_quick_setup_username = Some(raw);
        self.overlay = Overlay::TextInput(TextInputState {
            title: texts::tui_webdav_jianguoyun_setup_title().to_string(),
            prompt: texts::tui_webdav_jianguoyun_app_password_prompt().to_string(),
            buffer: String::new(),
            submit: TextSubmit::WebDavJianguoyunPassword,
            secret: true,
        });
        Action::None
    }

    fn handle_webdav_password_submit(&mut self, raw: String) -> Action {
        if raw.is_empty() {
            self.push_toast(texts::tui_toast_webdav_password_empty(), ToastKind::Warning);
            self.overlay = Overlay::TextInput(TextInputState {
                title: texts::tui_webdav_jianguoyun_setup_title().to_string(),
                prompt: texts::tui_webdav_jianguoyun_app_password_prompt().to_string(),
                buffer: String::new(),
                submit: TextSubmit::WebDavJianguoyunPassword,
                secret: true,
            });
            return Action::None;
        }

        let username = self.webdav_quick_setup_username.take().unwrap_or_default();
        if username.trim().is_empty() {
            self.push_toast(texts::tui_toast_webdav_username_empty(), ToastKind::Warning);
            return Action::None;
        }

        Action::ConfigWebDavJianguoyunQuickSetup {
            username,
            password: raw,
        }
    }

    fn handle_settings_proxy_listen_address_submit(
        &mut self,
        data: &UiData,
        raw: String,
    ) -> Action {
        if data.proxy.running {
            self.push_toast(
                texts::tui_toast_proxy_settings_stop_before_edit(),
                ToastKind::Info,
            );
            return Action::None;
        }

        let trimmed = raw.trim().to_string();
        if !is_valid_proxy_listen_address(&trimmed) {
            self.push_toast(
                texts::tui_toast_proxy_listen_address_invalid(),
                ToastKind::Warning,
            );
            self.overlay = Overlay::TextInput(TextInputState {
                title: texts::tui_settings_proxy_title().to_string(),
                prompt: texts::tui_settings_proxy_listen_address_prompt().to_string(),
                buffer: trimmed,
                submit: TextSubmit::SettingsProxyListenAddress,
                secret: false,
            });
            return Action::None;
        }

        Action::SetProxyListenAddress { address: trimmed }
    }

    fn handle_settings_proxy_listen_port_submit(&mut self, data: &UiData, raw: String) -> Action {
        if data.proxy.running {
            self.push_toast(
                texts::tui_toast_proxy_settings_stop_before_edit(),
                ToastKind::Info,
            );
            return Action::None;
        }

        let trimmed = raw.trim().to_string();
        let Ok(port) = trimmed.parse::<u16>() else {
            self.push_toast(
                texts::tui_toast_proxy_listen_port_invalid(),
                ToastKind::Warning,
            );
            self.overlay = Overlay::TextInput(TextInputState {
                title: texts::tui_settings_proxy_title().to_string(),
                prompt: texts::tui_settings_proxy_listen_port_prompt().to_string(),
                buffer: trimmed,
                submit: TextSubmit::SettingsProxyListenPort,
                secret: false,
            });
            return Action::None;
        };

        if !(1024..=65535).contains(&port) {
            self.push_toast(
                texts::tui_toast_proxy_listen_port_invalid(),
                ToastKind::Warning,
            );
            self.overlay = Overlay::TextInput(TextInputState {
                title: texts::tui_settings_proxy_title().to_string(),
                prompt: texts::tui_settings_proxy_listen_port_prompt().to_string(),
                buffer: trimmed,
                submit: TextSubmit::SettingsProxyListenPort,
                secret: false,
            });
            return Action::None;
        }

        Action::SetProxyListenPort { port }
    }

    fn handle_settings_upstream_proxy_url_submit(&mut self, _data: &UiData, raw: String) -> Action {
        let trimmed = raw.trim().to_string();

        if trimmed.is_empty() {
            // Clear proxy setting - show confirmation dialog
            self.overlay = Overlay::Confirm(ConfirmOverlay {
                title: texts::tui_settings_upstream_proxy_clear_title().to_string(),
                message: texts::tui_confirm_upstream_proxy_clear().to_string(),
                action: ConfirmAction::SettingsClearUpstreamProxy,
            });
            return Action::None;
        }

        // Validate URL format (simple validation)
        if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") && !trimmed.starts_with("socks5://") {
            self.push_toast(
                texts::tui_toast_upstream_proxy_url_invalid(),
                ToastKind::Warning,
            );
            self.overlay = Overlay::TextInput(TextInputState {
                title: texts::tui_settings_upstream_proxy_title().to_string(),
                prompt: texts::tui_settings_upstream_proxy_url_prompt().to_string(),
                buffer: trimmed,
                submit: TextSubmit::SettingsUpstreamProxyUrl,
                secret: false,
            });
            return Action::None;
        }

        Action::SetUpstreamProxyUrl { url: Some(trimmed) }
    }
}

fn is_valid_proxy_listen_address(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    if matches!(value, "localhost" | "0.0.0.0") {
        return true;
    }

    let parts = value.split('.').collect::<Vec<_>>();
    if parts.len() != 4 {
        return false;
    }

    parts
        .iter()
        .all(|part| !part.is_empty() && part.parse::<u8>().is_ok())
}
