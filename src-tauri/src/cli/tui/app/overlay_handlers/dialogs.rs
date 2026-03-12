use super::*;

impl App {
    pub(super) fn handle_dialog_overlay_key(
        &mut self,
        key: KeyEvent,
        _data: &UiData,
    ) -> Option<Action> {
        if let Some(action) = self.handle_confirm_overlay_key(key) {
            return Some(action);
        }

        if let Some(action) = self.handle_text_input_overlay_key(key) {
            return Some(action);
        }

        None
    }

    fn handle_confirm_overlay_key(&mut self, key: KeyEvent) -> Option<Action> {
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
                    ConfirmAction::ProviderApiFormatProxyNotice => Action::None,
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
                self.overlay = Overlay::None;
                action
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                if matches!(confirm.action, ConfirmAction::EditorSaveBeforeClose) {
                    self.editor = None;
                }
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Action::None
            }
            _ => Action::None,
        };

        Some(action)
    }

    fn handle_text_input_overlay_key(&mut self, key: KeyEvent) -> Option<Action> {
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
                self.handle_text_input_submit(submit, raw)
            }
            KeyCode::Backspace => {
                if let Overlay::TextInput(input) = &mut self.overlay {
                    input.buffer.pop();
                }
                Action::None
            }
            KeyCode::Char(c) => {
                if !c.is_control() {
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

    fn handle_text_input_submit(&mut self, submit: TextSubmit, raw: String) -> Action {
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
            TextSubmit::WebDavJianguoyunUsername => self.handle_webdav_username_submit(raw),
            TextSubmit::WebDavJianguoyunPassword => self.handle_webdav_password_submit(raw),
        }
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
}
