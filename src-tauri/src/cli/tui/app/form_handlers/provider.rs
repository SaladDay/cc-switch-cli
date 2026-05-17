use super::*;

impl App {
    pub(super) fn handle_provider_template_key(
        &mut self,
        key: KeyEvent,
        data: &UiData,
    ) -> Option<Action> {
        let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
            return None;
        };

        if !matches!(provider.page, form::ProviderFormPage::Main) {
            return None;
        }

        if provider.focus != FormFocus::Templates || !matches!(provider.mode, FormMode::Add) {
            return None;
        }

        match key.code {
            KeyCode::Left => {
                provider.template_idx = provider.template_idx.saturating_sub(1);
                Some(Action::None)
            }
            KeyCode::Right => {
                let max = provider.template_count().saturating_sub(1);
                provider.template_idx = (provider.template_idx + 1).min(max);
                Some(Action::None)
            }
            KeyCode::Enter => {
                let existing_ids = collect_existing_provider_ids(data);
                provider.apply_template(provider.template_idx, &existing_ids);
                provider.focus = FormFocus::Fields;
                Some(Action::None)
            }
            _ => None,
        }
    }

    pub(super) fn handle_provider_focus_key(
        &mut self,
        key: KeyEvent,
        data: &UiData,
    ) -> Option<Action> {
        let Some(FormState::ProviderAdd(provider)) = self.form.as_ref() else {
            return None;
        };

        if matches!(provider.page, form::ProviderFormPage::UsageQuery) {
            return self.handle_usage_query_page_key(key);
        }

        match provider.focus {
            FormFocus::Fields => self.handle_provider_fields_key(key, data),
            FormFocus::JsonPreview => self.handle_provider_json_preview_key(key, data),
            FormFocus::Templates | FormFocus::Content => None,
        }
    }

    pub(super) fn build_provider_form_save_action(&mut self, data: &UiData) -> Action {
        let validation_message = {
            let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                return Action::None;
            };

            if provider.name.is_blank() {
                Some(if provider.mode.is_edit() {
                    texts::tui_toast_provider_missing_name()
                } else {
                    texts::tui_toast_provider_add_missing_fields()
                })
            } else if matches!(provider.app_type, crate::app_config::AppType::Codex)
                && !provider.is_codex_official_provider()
                && provider.codex_base_url.is_blank()
            {
                Some(texts::base_url_empty_error())
            } else if let Some(message) = validate_usage_query_form(provider) {
                Some(message)
            } else if !provider.ensure_generated_id(&collect_existing_provider_ids(data)) {
                Some(if provider.mode.is_edit() {
                    texts::tui_toast_provider_missing_name()
                } else {
                    texts::tui_toast_provider_add_missing_fields()
                })
            } else {
                None
            }
        };

        if let Some(message) = validation_message {
            self.push_toast(message, ToastKind::Warning);
            return Action::None;
        }

        let Some(FormState::ProviderAdd(provider)) = self.form.as_ref() else {
            return Action::None;
        };

        let provider_json = provider.to_provider_json_value();
        let content =
            serde_json::to_string_pretty(&provider_json).unwrap_or_else(|_| "{}".to_string());

        Action::EditorSubmit {
            submit: match &provider.mode {
                FormMode::Add => EditorSubmit::ProviderAdd,
                FormMode::Edit { id } => EditorSubmit::ProviderEdit { id: id.clone() },
            },
            content,
        }
    }

    fn handle_provider_fields_key(&mut self, key: KeyEvent, data: &UiData) -> Option<Action> {
        let (fields, selected, editing) = match self.prepare_provider_field_selection() {
            Some(state) => state,
            None => return None,
        };

        if editing {
            self.handle_provider_field_editing(selected, key, data)
        } else {
            self.handle_provider_field_navigation(fields, selected, key, data)
        }
    }

    fn handle_provider_field_editing(
        &mut self,
        selected: ProviderAddField,
        key: KeyEvent,
        data: &UiData,
    ) -> Option<Action> {
        let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
            return None;
        };

        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                provider.editing = false;
                Some(Action::None)
            }
            _ => {
                if TextEditCommand::from_key(key).is_none() {
                    return None;
                }
                let policy = TextInputPolicy {
                    max_chars: (selected == ProviderAddField::Notes)
                        .then_some(PROVIDER_NOTES_MAX_CHARS),
                };
                let changed = provider
                    .input_mut(selected)
                    .and_then(|input| input.apply_key_with_policy(key, policy))
                    .map(|edit| edit.changed)
                    .unwrap_or(false);
                self.finish_provider_input_change(selected, changed, data);
                Some(Action::None)
            }
        }
    }

    fn handle_provider_field_navigation(
        &mut self,
        fields: Vec<ProviderAddField>,
        selected: ProviderAddField,
        key: KeyEvent,
        data: &UiData,
    ) -> Option<Action> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    return None;
                };
                provider.field_idx = provider.field_idx.saturating_sub(1);
                while provider.field_idx > 0
                    && is_provider_divider_field(fields.get(provider.field_idx))
                {
                    provider.field_idx = provider.field_idx.saturating_sub(1);
                }
                Some(Action::None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    return None;
                };
                provider.field_idx = (provider.field_idx + 1).min(fields.len() - 1);
                while provider.field_idx < fields.len().saturating_sub(1)
                    && is_provider_divider_field(fields.get(provider.field_idx))
                {
                    provider.field_idx = (provider.field_idx + 1).min(fields.len() - 1);
                }
                Some(Action::None)
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                Some(self.handle_provider_field_activate(selected, key, data))
            }
            _ => None,
        }
    }

    fn handle_provider_field_activate(
        &mut self,
        selected: ProviderAddField,
        key: KeyEvent,
        data: &UiData,
    ) -> Action {
        match selected {
            ProviderAddField::ClaudeApiFormat => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_ref() else {
                    return Action::None;
                };
                self.overlay = Overlay::ClaudeApiFormatPicker {
                    selected: provider.claude_api_format.picker_index(),
                };
                Action::None
            }
            ProviderAddField::CodexWireApi => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    return Action::None;
                };
                provider.codex_wire_api = match provider.codex_wire_api {
                    CodexWireApi::Chat => CodexWireApi::Responses,
                    CodexWireApi::Responses => CodexWireApi::Chat,
                };
                Action::None
            }
            ProviderAddField::CodexRequiresOpenaiAuth => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    return Action::None;
                };
                provider.codex_requires_openai_auth = !provider.codex_requires_openai_auth;
                Action::None
            }
            ProviderAddField::IncludeCommonConfig => {
                let toggle_result = {
                    let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                        return Action::None;
                    };
                    provider.toggle_include_common_config(&data.config.common_snippet)
                };
                if let Err(err) = toggle_result {
                    self.push_toast(err, ToastKind::Warning);
                }
                Action::None
            }
            ProviderAddField::GeminiAuthType => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    return Action::None;
                };
                provider.gemini_auth_type = match provider.gemini_auth_type {
                    GeminiAuthType::OAuth => GeminiAuthType::ApiKey,
                    GeminiAuthType::ApiKey => GeminiAuthType::OAuth,
                };
                Action::None
            }
            ProviderAddField::OpenClawApiProtocol => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    return Action::None;
                };
                provider
                    .opencode_npm_package
                    .set(next_openclaw_api_protocol(
                        &provider.opencode_npm_package.value,
                    ));
                Action::None
            }
            ProviderAddField::OpenClawUserAgent => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    return Action::None;
                };
                provider.openclaw_user_agent = !provider.openclaw_user_agent;
                Action::None
            }
            ProviderAddField::ClaudeModelConfig => {
                self.overlay = Overlay::ClaudeModelPicker {
                    selected: 0,
                    editing: false,
                };
                Action::None
            }
            ProviderAddField::ClaudeHideAttribution => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    return Action::None;
                };
                provider.toggle_claude_hide_attribution();
                Action::None
            }
            ProviderAddField::OpenClawModels => {
                if matches!(key.code, KeyCode::Enter) {
                    let Some(FormState::ProviderAdd(provider)) = self.form.as_ref() else {
                        return Action::None;
                    };
                    self.open_editor(
                        texts::tui_openclaw_models_editor_title(),
                        EditorKind::Json,
                        provider.openclaw_models_editor_text(),
                        EditorSubmit::ProviderFormApplyOpenClawModels,
                    );
                    if let Some(editor) = self.editor.as_mut() {
                        editor.mode = EditorMode::Edit;
                    }
                }
                Action::None
            }
            ProviderAddField::CommonSnippet => {
                if matches!(key.code, KeyCode::Enter) {
                    let Some(FormState::ProviderAdd(provider)) = self.form.as_ref() else {
                        return Action::None;
                    };
                    self.open_common_snippet_editor(
                        provider.app_type.clone(),
                        data,
                        None,
                        CommonSnippetViewSource::ProviderForm,
                    );
                }
                Action::None
            }
            ProviderAddField::UsageQuery => {
                if matches!(key.code, KeyCode::Enter) {
                    self.open_usage_query_page_with_notice();
                }
                Action::None
            }
            ProviderAddField::CodexModel
            | ProviderAddField::GeminiModel
            | ProviderAddField::OpenCodeModelId => {
                self.handle_provider_model_field_activate(selected, key)
            }
            _ => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    return Action::None;
                };
                if selected == ProviderAddField::Id && !provider.is_id_editable() {
                    return Action::None;
                }
                if provider.input(selected).is_some() {
                    provider.editing = true;
                }
                Action::None
            }
        }
    }

    fn handle_usage_query_page_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Some(FormState::ProviderAdd(provider)) = self.form.as_ref() else {
            return None;
        };
        if matches!(provider.focus, FormFocus::JsonPreview) {
            if !provider.usage_query_extractor_available() {
                if let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() {
                    provider.focus = FormFocus::Fields;
                }
                return Some(Action::None);
            }
            return match key.code {
                KeyCode::Enter => Some(self.open_usage_query_script_editor()),
                _ => None,
            };
        }
        if matches!(provider.focus, FormFocus::Content) {
            if !provider.usage_query_extractor_available() {
                if let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() {
                    provider.focus = FormFocus::Fields;
                }
                return Some(Action::None);
            }
            return match key.code {
                KeyCode::Enter => Some(self.open_usage_query_script_help_view()),
                _ => None,
            };
        }

        let (fields, selected, editing) = match self.prepare_usage_query_field_selection() {
            Some(state) => state,
            None => return None,
        };

        if editing {
            self.handle_usage_query_field_editing(selected, key)
        } else {
            self.handle_usage_query_field_navigation(fields, selected, key)
        }
    }

    fn open_usage_query_page_with_notice(&mut self) {
        let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
            return;
        };
        provider.open_usage_query_page();

        if self.usage_query_notice_confirmed {
            return;
        }

        self.overlay = Overlay::Confirm(ConfirmOverlay {
            title: texts::tui_usage_query_notice_title().to_string(),
            message: texts::tui_usage_query_notice_message().to_string(),
            action: ConfirmAction::UsageQueryNotice,
        });
    }

    fn handle_usage_query_field_editing(
        &mut self,
        selected: form::UsageQueryField,
        key: KeyEvent,
    ) -> Option<Action> {
        let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
            return None;
        };

        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                provider.usage_query_editing = false;
                if matches!(
                    selected,
                    form::UsageQueryField::Timeout | form::UsageQueryField::AutoInterval
                ) {
                    normalize_usage_query_numeric_fields(provider);
                }
                Some(Action::None)
            }
            _ => {
                if TextEditCommand::from_key(key).is_none() {
                    return None;
                }
                let changed = provider
                    .usage_query_input_mut(selected)
                    .and_then(|input| input.apply_key(key))
                    .map(|edit| edit.changed)
                    .unwrap_or(false);
                if changed {
                    provider.touch_usage_query();
                }
                Some(Action::None)
            }
        }
    }

    fn handle_usage_query_field_navigation(
        &mut self,
        fields: Vec<form::UsageQueryField>,
        selected: form::UsageQueryField,
        key: KeyEvent,
    ) -> Option<Action> {
        match key.code {
            KeyCode::Esc => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    return None;
                };
                provider.close_usage_query_page();
                Some(Action::None)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    return None;
                };
                provider.usage_query_field_idx = provider.usage_query_field_idx.saturating_sub(1);
                Some(Action::None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    return None;
                };
                provider.usage_query_field_idx =
                    (provider.usage_query_field_idx + 1).min(fields.len() - 1);
                Some(Action::None)
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                Some(self.handle_usage_query_field_activate(selected))
            }
            _ => None,
        }
    }

    fn handle_usage_query_field_activate(&mut self, selected: form::UsageQueryField) -> Action {
        match selected {
            form::UsageQueryField::Enabled => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    return Action::None;
                };
                provider.toggle_usage_query_enabled();
                Action::None
            }
            form::UsageQueryField::Template => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_ref() else {
                    return Action::None;
                };
                let options = provider.available_usage_query_templates();
                let selected = options
                    .iter()
                    .position(|template| *template == provider.usage_query_template)
                    .unwrap_or(0);
                self.overlay = Overlay::UsageQueryTemplatePicker { selected };
                Action::None
            }
            form::UsageQueryField::CodingPlanProvider => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    return Action::None;
                };
                provider.cycle_usage_query_coding_plan_provider();
                Action::None
            }
            form::UsageQueryField::Script => self.open_usage_query_script_editor(),
            _ => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    return Action::None;
                };
                if provider.usage_query_input(selected).is_some() {
                    provider.usage_query_editing = true;
                }
                Action::None
            }
        }
    }

    fn open_usage_query_script_editor(&mut self) -> Action {
        let Some(FormState::ProviderAdd(provider)) = self.form.as_ref() else {
            return Action::None;
        };
        if !provider.usage_query_extractor_available() {
            return Action::None;
        }
        self.open_editor(
            texts::tui_usage_query_script(),
            EditorKind::Plain,
            provider.usage_query_code.clone(),
            EditorSubmit::ProviderFormApplyUsageScriptCode,
        );
        Action::None
    }

    fn open_usage_query_script_help_view(&mut self) -> Action {
        self.overlay = Overlay::TextView(TextViewState {
            title: texts::tui_usage_query_script_help_title().to_string(),
            lines: form::ProviderAddFormState::usage_query_script_help_lines(),
            scroll: 0,
            action: None,
        });
        Action::None
    }

    fn handle_provider_model_field_activate(
        &mut self,
        selected: ProviderAddField,
        key: KeyEvent,
    ) -> Action {
        let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
            return Action::None;
        };

        if matches!(key.code, KeyCode::Enter) {
            let api_key = match selected {
                ProviderAddField::CodexModel => (!provider.codex_api_key.value.trim().is_empty())
                    .then(|| provider.codex_api_key.value.clone()),
                ProviderAddField::GeminiModel => (!provider.gemini_api_key.value.trim().is_empty())
                    .then(|| provider.gemini_api_key.value.clone()),
                ProviderAddField::OpenCodeModelId => {
                    (!provider.opencode_api_key.value.trim().is_empty())
                        .then(|| provider.opencode_api_key.value.clone())
                }
                _ => None,
            };
            let base_url = match selected {
                ProviderAddField::CodexModel => provider.codex_base_url.value.clone(),
                ProviderAddField::GeminiModel => provider.gemini_base_url.value.clone(),
                ProviderAddField::OpenCodeModelId => provider.opencode_base_url.value.clone(),
                _ => String::new(),
            };
            Action::ProviderModelFetch {
                base_url,
                api_key,
                field: selected,
                claude_idx: None,
            }
        } else {
            provider.editing = true;
            Action::None
        }
    }

    fn handle_provider_json_preview_key(&mut self, key: KeyEvent, data: &UiData) -> Option<Action> {
        let Some(FormState::ProviderAdd(provider)) = self.form.as_ref() else {
            return None;
        };

        if matches!(provider.app_type, AppType::Codex) {
            self.handle_codex_provider_preview_key(key)
        } else {
            self.handle_regular_provider_preview_key(key, data)
        }
    }

    fn handle_codex_provider_preview_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Enter => Some(self.open_codex_provider_preview_editor()),
            KeyCode::Up | KeyCode::Char('k') => {
                self.adjust_codex_preview_scroll(|scroll| scroll.saturating_sub(1));
                Some(Action::None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.adjust_codex_preview_scroll(|scroll| scroll.saturating_add(1));
                Some(Action::None)
            }
            KeyCode::PageUp => {
                self.adjust_codex_preview_scroll(|scroll| scroll.saturating_sub(10));
                Some(Action::None)
            }
            KeyCode::PageDown => {
                self.adjust_codex_preview_scroll(|scroll| scroll.saturating_add(10));
                Some(Action::None)
            }
            _ => None,
        }
    }

    fn open_codex_provider_preview_editor(&mut self) -> Action {
        let Some(FormState::ProviderAdd(provider)) = self.form.as_ref() else {
            return Action::None;
        };

        match provider.codex_preview_section {
            form::CodexPreviewSection::Auth => {
                let provider_json = provider.to_provider_json_value();
                let auth_value = provider_json
                    .get("settingsConfig")
                    .and_then(|value| value.get("auth"))
                    .cloned()
                    .filter(|value| value.is_object())
                    .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
                let content =
                    serde_json::to_string_pretty(&auth_value).unwrap_or_else(|_| "{}".to_string());
                self.open_editor(
                    texts::tui_codex_auth_json_title(),
                    EditorKind::Json,
                    content,
                    EditorSubmit::ProviderFormApplyCodexAuth,
                );
            }
            form::CodexPreviewSection::Config => {
                let provider_json = provider.to_provider_json_value();
                let config_text = provider_json
                    .get("settingsConfig")
                    .and_then(|value| value.get("config"))
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string();
                self.open_editor(
                    texts::tui_codex_config_toml_title(),
                    EditorKind::Plain,
                    config_text,
                    EditorSubmit::ProviderFormApplyCodexConfigToml,
                );
            }
        }

        if let Some(editor) = self.editor.as_mut() {
            editor.mode = EditorMode::Edit;
        }
        Action::None
    }

    fn adjust_codex_preview_scroll(&mut self, update: impl FnOnce(usize) -> usize) {
        let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
            return;
        };

        let scroll = match provider.codex_preview_section {
            form::CodexPreviewSection::Auth => &mut provider.codex_auth_scroll,
            form::CodexPreviewSection::Config => &mut provider.codex_config_scroll,
        };
        *scroll = update(*scroll);
    }

    fn handle_regular_provider_preview_key(
        &mut self,
        key: KeyEvent,
        data: &UiData,
    ) -> Option<Action> {
        match key.code {
            KeyCode::Enter => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_ref() else {
                    return None;
                };
                let provider_json = match provider
                    .to_provider_json_value_with_common_config(&data.config.common_snippet)
                {
                    Ok(value) => value,
                    Err(err) => {
                        self.push_toast(err, ToastKind::Error);
                        return Some(Action::None);
                    }
                };

                let settings_value = provider_json
                    .get("settingsConfig")
                    .cloned()
                    .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
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
                Some(Action::None)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    return None;
                };
                provider.json_scroll = provider.json_scroll.saturating_sub(1);
                Some(Action::None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    return None;
                };
                provider.json_scroll = provider.json_scroll.saturating_add(1);
                Some(Action::None)
            }
            KeyCode::PageUp => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    return None;
                };
                provider.json_scroll = provider.json_scroll.saturating_sub(10);
                Some(Action::None)
            }
            KeyCode::PageDown => {
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    return None;
                };
                provider.json_scroll = provider.json_scroll.saturating_add(10);
                Some(Action::None)
            }
            _ => None,
        }
    }

    fn prepare_provider_field_selection(
        &mut self,
    ) -> Option<(Vec<ProviderAddField>, ProviderAddField, bool)> {
        let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
            return None;
        };
        if provider.focus != FormFocus::Fields {
            return None;
        }

        let fields = provider.fields();
        if !fields.is_empty() {
            provider.field_idx = provider.field_idx.min(fields.len() - 1);
        } else {
            provider.field_idx = 0;
        }

        if is_provider_divider_field(fields.get(provider.field_idx)) {
            if provider.field_idx < fields.len().saturating_sub(1) {
                provider.field_idx = (provider.field_idx + 1).min(fields.len() - 1);
            } else {
                provider.field_idx = provider.field_idx.saturating_sub(1);
            }
        }

        let selected = fields.get(provider.field_idx).copied()?;
        Some((fields, selected, provider.editing))
    }

    fn prepare_usage_query_field_selection(
        &mut self,
    ) -> Option<(Vec<form::UsageQueryField>, form::UsageQueryField, bool)> {
        let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
            return None;
        };
        if !matches!(provider.page, form::ProviderFormPage::UsageQuery) {
            return None;
        }
        if !matches!(provider.focus, FormFocus::Fields) {
            return None;
        }

        let fields = provider.usage_query_table_fields();
        if !fields.is_empty() {
            provider.usage_query_field_idx = provider.usage_query_field_idx.min(fields.len() - 1);
        } else {
            provider.usage_query_field_idx = 0;
        }

        let selected = fields.get(provider.usage_query_field_idx).copied()?;
        Some((fields, selected, provider.usage_query_editing))
    }

    fn finish_provider_input_change(
        &mut self,
        selected: ProviderAddField,
        changed: bool,
        data: &UiData,
    ) {
        let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
            return;
        };

        if changed && selected == ProviderAddField::Id {
            provider.id_is_manual = true;
        }
        if changed && selected == ProviderAddField::Name && !provider.id_is_manual {
            let existing_ids = collect_existing_provider_ids(data);
            provider
                .id
                .set(crate::cli::commands::provider_input::generate_provider_id(
                    provider.name.value.trim(),
                    &existing_ids,
                ));
        }
        if changed && usage_query_provider_credential_field(selected) {
            provider.refresh_usage_query_custom_variable_comment();
        }
    }
}

fn usage_query_provider_credential_field(field: ProviderAddField) -> bool {
    matches!(
        field,
        ProviderAddField::ClaudeApiKey
            | ProviderAddField::ClaudeBaseUrl
            | ProviderAddField::CodexApiKey
            | ProviderAddField::CodexBaseUrl
            | ProviderAddField::GeminiApiKey
            | ProviderAddField::GeminiBaseUrl
            | ProviderAddField::OpenCodeApiKey
            | ProviderAddField::OpenCodeBaseUrl
    )
}

fn is_provider_divider_field(field: Option<&ProviderAddField>) -> bool {
    matches!(
        field,
        Some(ProviderAddField::CommonConfigDivider | ProviderAddField::UsageQueryDivider)
    )
}

fn collect_existing_provider_ids(data: &UiData) -> Vec<String> {
    data.providers
        .rows
        .iter()
        .map(|row| row.id.clone())
        .collect()
}

fn next_openclaw_api_protocol(current: &str) -> &'static str {
    let current = current.trim();
    let protocols = &form::OPENCLAW_API_PROTOCOLS;
    let next_idx = protocols
        .iter()
        .position(|candidate| *candidate == current)
        .map(|idx| (idx + 1) % protocols.len())
        .unwrap_or(0);
    protocols[next_idx]
}

fn normalize_usage_query_numeric_fields(provider: &mut form::ProviderAddFormState) {
    let timeout = form::normalize_usage_timeout(&provider.usage_query_timeout.value);
    provider.usage_query_timeout.set(timeout.to_string());

    let interval = form::normalize_usage_interval(&provider.usage_query_auto_interval.value);
    provider.usage_query_auto_interval.set(interval.to_string());
}

fn validate_usage_query_form(provider: &form::ProviderAddFormState) -> Option<&'static str> {
    if !provider.usage_query_enabled {
        return None;
    }

    if matches!(
        provider.usage_query_template,
        form::UsageQueryTemplate::GitHubCopilot
            | form::UsageQueryTemplate::TokenPlan
            | form::UsageQueryTemplate::Balance
    ) {
        return None;
    }

    let code = provider.usage_query_code.trim();
    if code.is_empty() {
        return Some(texts::tui_usage_query_script_empty());
    }
    if !code.contains("return") {
        return Some(texts::tui_usage_query_must_have_return());
    }

    None
}
