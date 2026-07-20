use super::*;

fn model_fetch_model_index(
    models_len: usize,
    filtered_indices: Option<&[usize]>,
    filtered_index: usize,
) -> Option<usize> {
    let model_index = match filtered_indices {
        Some(indices) => indices.get(filtered_index).copied()?,
        None => filtered_index,
    };
    (model_index < models_len).then_some(model_index)
}

const SESSION_PROJECT_PICKER_PAGE_STEP: usize = 8;
const SESSION_PROJECT_FILTER_MAX_CHARS: usize = 256;
const SESSION_PROJECT_PATH_SCROLL_STEP: usize = 8;

fn project_path_scroll_right(path: &str, current: usize) -> usize {
    if current == usize::MAX || path.is_empty() {
        return current;
    }
    let mut start = current.min(path.len());
    while start > 0 && !path.is_char_boundary(start) {
        start -= 1;
    }
    let mut used = 0usize;
    let mut next = start;
    for (offset, ch) in path[start..].char_indices() {
        next = start + offset + ch.len_utf8();
        used = used.saturating_add(UnicodeWidthChar::width(ch).unwrap_or(1).max(1));
        if used >= SESSION_PROJECT_PATH_SCROLL_STEP {
            break;
        }
    }
    if next >= path.len() {
        usize::MAX
    } else {
        next
    }
}

fn project_path_scroll_left(path: &str, current: usize) -> usize {
    if current == 0 || path.is_empty() {
        return 0;
    }
    let mut end = if current == usize::MAX {
        path.len()
    } else {
        current.min(path.len())
    };
    while end > 0 && !path.is_char_boundary(end) {
        end -= 1;
    }
    let mut used = 0usize;
    let mut start = end;
    for (offset, ch) in path[..end].char_indices().rev() {
        start = offset;
        used = used.saturating_add(UnicodeWidthChar::width(ch).unwrap_or(1).max(1));
        if used >= SESSION_PROJECT_PATH_SCROLL_STEP {
            break;
        }
    }
    start
}

fn session_project_option_index(
    option_count: usize,
    filtered_indices: Option<&[usize]>,
    selected_idx: usize,
) -> Option<usize> {
    let option_index = match filtered_indices {
        Some(indices) => indices.get(selected_idx).copied()?,
        None => selected_idx,
    };
    (option_index < option_count).then_some(option_index)
}

impl App {
    pub(super) fn handle_picker_overlay_key(
        &mut self,
        key: KeyEvent,
        data: &UiData,
    ) -> Option<Action> {
        if let Some(action) = self.handle_sync_method_picker_key(key, data) {
            return Some(action);
        }
        if let Some(action) = self.handle_claude_api_format_picker_key(key, data) {
            return Some(action);
        }
        if let Some(action) = self.handle_user_agent_picker_key(key) {
            return Some(action);
        }
        if let Some(action) = self.handle_external_editor_picker_key(key) {
            return Some(action);
        }
        if let Some(action) = self.handle_usage_query_template_picker_key(key) {
            return Some(action);
        }
        if let Some(action) = self.handle_s3_preset_picker_key(key) {
            return Some(action);
        }
        if let Some(action) = self.handle_managed_account_picker_key(key) {
            return Some(action);
        }
        if let Some(action) = self.handle_managed_account_action_picker_key(key) {
            return Some(action);
        }
        if let Some(action) = self.handle_hermes_models_picker_key(key) {
            return Some(action);
        }
        if let Some(action) = self.handle_provider_test_menu_key(key, data) {
            return Some(action);
        }
        if let Some(action) = self.handle_claude_model_picker_key(key) {
            return Some(action);
        }
        if let Some(action) = self.handle_model_fetch_picker_key(key) {
            return Some(action);
        }
        if let Some(action) = self.handle_session_project_picker_key(key) {
            return Some(action);
        }
        if let Some(action) = self.handle_openclaw_tools_profile_picker_key(key, data) {
            return Some(action);
        }
        if let Some(action) = self.handle_openclaw_agents_fallback_picker_key(key, data) {
            return Some(action);
        }
        if let Some(action) = self.handle_mcp_type_picker_key(key) {
            return Some(action);
        }
        if let Some(action) = self.handle_mcp_apps_picker_key(key, data) {
            return Some(action);
        }
        if let Some(action) = self.handle_visible_apps_picker_key(key) {
            return Some(action);
        }
        if let Some(action) = self.handle_skills_apps_picker_key(key, data) {
            return Some(action);
        }
        if let Some(action) = self.handle_skills_import_picker_key(key) {
            return Some(action);
        }
        if let Some(action) = self.handle_failover_queue_manager_key(key, data) {
            return Some(action);
        }
        None
    }

    fn handle_hermes_models_picker_key(&mut self, key: KeyEvent) -> Option<Action> {
        let editing = match self.overlay {
            Overlay::HermesModelsPicker { editing } => editing,
            _ => return None,
        };

        if editing {
            return Some(self.handle_hermes_models_picker_editing_key(key));
        }

        Some(self.handle_hermes_models_picker_navigation_key(key))
    }

    fn handle_hermes_models_picker_editing_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                if let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() {
                    provider.hermes_models_editing = false;
                }
                self.overlay = Overlay::HermesModelsPicker { editing: false };
                Action::None
            }
            _ => {
                if TextEditCommand::from_key(key).is_none() {
                    return Action::None;
                }
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    return Action::None;
                };
                let Some(selected) = provider.selected_hermes_model_field() else {
                    return Action::None;
                };
                if provider
                    .hermes_model_input
                    .apply_key(key)
                    .is_some_and(|edit| edit.changed)
                {
                    let value = provider.hermes_model_input.value.clone();
                    provider.set_hermes_model_field_text(selected, &value);
                }
                Action::None
            }
        }
    }

    fn handle_hermes_models_picker_navigation_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                if let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() {
                    provider.close_hermes_models_picker();
                }
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() {
                    provider.hermes_models_field_idx =
                        provider.hermes_models_field_idx.saturating_sub(1);
                    provider.sync_hermes_model_input_from_selection();
                }
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() {
                    let fields_len = provider.hermes_model_field_count();
                    if fields_len > 0 {
                        provider.hermes_models_field_idx =
                            (provider.hermes_models_field_idx + 1).min(fields_len - 1);
                    } else {
                        provider.hermes_models_field_idx = 0;
                    }
                    provider.sync_hermes_model_input_from_selection();
                }
                Action::None
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                if let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() {
                    provider.add_empty_hermes_model();
                }
                Action::None
            }
            KeyCode::Char('d') | KeyCode::Char('D') | KeyCode::Delete | KeyCode::Backspace => {
                if let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() {
                    provider.remove_selected_hermes_model();
                }
                Action::None
            }
            KeyCode::Char('f') | KeyCode::Char('F') => self.build_hermes_models_fetch_action(),
            KeyCode::Enter => {
                if let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() {
                    if provider.selected_hermes_model_field().is_some() {
                        provider.sync_hermes_model_input_from_selection();
                        provider.hermes_models_editing = true;
                        self.overlay = Overlay::HermesModelsPicker { editing: true };
                    }
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    fn handle_sync_method_picker_key(&mut self, key: KeyEvent, data: &UiData) -> Option<Action> {
        let Overlay::SkillsSyncMethodPicker { selected } = &mut self.overlay else {
            return None;
        };

        Some(match key.code {
            KeyCode::Esc => {
                self.close_overlay();
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
        })
    }

    fn handle_claude_api_format_picker_key(
        &mut self,
        key: KeyEvent,
        data: &UiData,
    ) -> Option<Action> {
        let app_type = self
            .form
            .as_ref()
            .and_then(|form| match form {
                FormState::ProviderAdd(provider) => Some(provider.app_type.clone()),
                _ => None,
            })
            .unwrap_or_else(|| self.app_type.clone());
        let Overlay::ClaudeApiFormatPicker { selected } = &mut self.overlay else {
            return None;
        };

        Some(match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Up => {
                *selected = selected.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                *selected = (*selected + 1).min(
                    crate::cli::tui::form::ClaudeApiFormat::choices_for_app(&app_type)
                        .len()
                        .saturating_sub(1),
                );
                Action::None
            }
            KeyCode::Enter => {
                let next_format = crate::cli::tui::form::ClaudeApiFormat::from_picker_index_for_app(
                    *selected, &app_type,
                );
                let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
                    self.overlay = Overlay::None;
                    return Some(Action::None);
                };

                let changed = provider.claude_api_format != next_format;
                provider.claude_api_format = next_format;
                self.overlay = Overlay::None;

                let proxy_ready = data
                    .proxy
                    .routes_current_app_through_proxy(&provider.app_type)
                    .unwrap_or(false);
                if changed && next_format.requires_proxy_for_app(&provider.app_type) && !proxy_ready
                {
                    let message = if matches!(provider.app_type, crate::app_config::AppType::Codex)
                    {
                        texts::tui_codex_api_format_requires_proxy_message(next_format.as_str())
                    } else {
                        texts::tui_claude_api_format_requires_proxy_message(next_format.as_str())
                    };
                    self.overlay = Overlay::Confirm(ConfirmOverlay {
                        title: texts::tui_claude_api_format_requires_proxy_title().to_string(),
                        message,
                        action: ConfirmAction::ProviderApiFormatProxyNotice,
                    });
                }

                Action::None
            }
            _ => Action::None,
        })
    }

    fn handle_usage_query_template_picker_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Overlay::UsageQueryTemplatePicker { selected } = &mut self.overlay else {
            return None;
        };

        let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
            self.overlay = Overlay::None;
            return Some(Action::None);
        };

        let options = provider.available_usage_query_templates();
        if options.is_empty() {
            self.overlay = Overlay::None;
            return Some(Action::None);
        }

        *selected = (*selected).min(options.len() - 1);

        Some(match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Up => {
                *selected = selected.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                *selected = (*selected + 1).min(options.len() - 1);
                Action::None
            }
            KeyCode::Enter => {
                let template = options[*selected];
                provider.set_usage_query_template(template);
                provider.touch_usage_query();
                self.overlay = Overlay::None;
                Action::None
            }
            _ => Action::None,
        })
    }

    fn handle_s3_preset_picker_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Overlay::S3PresetPicker { selected } = &mut self.overlay else {
            return None;
        };
        let Some(FormState::S3Sync(form)) = self.form.as_mut() else {
            self.overlay = Overlay::None;
            return Some(Action::None);
        };
        *selected = (*selected).min(form::S3Preset::ALL.len().saturating_sub(1));

        Some(match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Up => {
                *selected = selected.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                *selected = (*selected + 1).min(form::S3Preset::ALL.len().saturating_sub(1));
                Action::None
            }
            KeyCode::Enter => {
                form.apply_preset(form::S3Preset::from_picker_index(*selected));
                self.overlay = Overlay::None;
                Action::None
            }
            _ => Action::None,
        })
    }

    fn handle_user_agent_picker_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Overlay::UserAgentPicker { selected } = &mut self.overlay else {
            return None;
        };
        let max = form::user_agent_picker_option_count().saturating_sub(1);
        *selected = (*selected).min(max);

        Some(match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                *selected = selected.saturating_sub(1);
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                *selected = (*selected + 1).min(max);
                Action::None
            }
            KeyCode::Enter => {
                let selected = *selected;
                if selected == form::USER_AGENT_PICKER_CUSTOM_INDEX {
                    let current = self
                        .form
                        .as_ref()
                        .and_then(|form| match form {
                            FormState::ProviderAdd(provider) => {
                                Some(provider.custom_user_agent.value.clone())
                            }
                            _ => None,
                        })
                        .unwrap_or_default();
                    self.overlay = Overlay::TextInput(TextInputState {
                        title: texts::tui_label_custom_user_agent().to_string(),
                        prompt: texts::tui_label_custom_user_agent().to_string(),
                        input: TextInput::new(current),
                        submit: TextSubmit::ProviderCustomUserAgent,
                    });
                } else if selected == form::USER_AGENT_PICKER_NO_OVERRIDE_INDEX {
                    if let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() {
                        provider.custom_user_agent.set("");
                    }
                    self.overlay = Overlay::None;
                } else {
                    let preset_index =
                        selected.saturating_sub(form::USER_AGENT_PICKER_PRESET_OFFSET);
                    if let (Some(FormState::ProviderAdd(provider)), Some(preset)) = (
                        self.form.as_mut(),
                        form::USER_AGENT_PRESETS.get(preset_index),
                    ) {
                        provider.set_custom_user_agent_preset(preset);
                    }
                    self.overlay = Overlay::None;
                }
                Action::None
            }
            _ => Action::None,
        })
    }

    fn handle_external_editor_picker_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Overlay::ExternalEditorPicker { selected, editors } = &mut self.overlay else {
            return None;
        };
        let max = editors.len();
        *selected = (*selected).min(max);

        Some(match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                *selected = selected.saturating_sub(1);
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                *selected = selected.saturating_add(1).min(max);
                Action::None
            }
            KeyCode::Enter => {
                let configured = crate::settings::get_preferred_editor();
                let selected = *selected;
                if let Some(editor) = editors.get(selected) {
                    let command = editor.command.clone();
                    self.overlay = Overlay::None;
                    if configured.as_deref() == Some(command.as_str()) {
                        Action::None
                    } else {
                        Action::SetPreferredEditor {
                            command: Some(command),
                        }
                    }
                } else {
                    self.overlay = Overlay::TextInput(TextInputState {
                        title: texts::tui_settings_preferred_editor_custom().to_string(),
                        prompt: texts::tui_settings_preferred_editor_prompt().to_string(),
                        input: TextInput::new(configured.unwrap_or_default()),
                        submit: TextSubmit::SettingsPreferredEditor,
                    });
                    Action::None
                }
            }
            _ => Action::None,
        })
    }

    fn handle_managed_account_picker_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Overlay::ManagedAccountPicker {
            auth_provider,
            selected,
            binding,
            selected_account_id: _,
        } = &mut self.overlay
        else {
            return None;
        };

        let auth_provider = auth_provider.clone();
        let binding = *binding;
        let accounts = self
            .managed_auth_status
            .as_ref()
            .filter(|status| status.provider == auth_provider)
            .map(|status| status.accounts.as_slice())
            .unwrap_or_default();
        let row_count = if binding {
            accounts.len() + 1
        } else {
            accounts.len()
        };

        // The status is fetched on demand when this picker opens, so it may not
        // be loaded yet. Until it is, keep the picker open but inert except for
        // Esc — otherwise a premature Enter (binding row 0) would silently bind
        // the empty/None selection before the accounts have arrived.
        let status_loaded = self
            .managed_auth_status
            .as_ref()
            .is_some_and(|status| status.provider == auth_provider)
            && !self.managed_auth_loading;
        if !status_loaded {
            if matches!(key.code, KeyCode::Esc) {
                self.overlay = Overlay::None;
            }
            return Some(Action::None);
        }

        if row_count == 0 {
            self.overlay = Overlay::None;
            return Some(Action::None);
        }

        *selected = (*selected).min(row_count.saturating_sub(1));

        Some(match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Up => {
                *selected = selected.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                *selected = (*selected + 1).min(row_count.saturating_sub(1));
                Action::None
            }
            KeyCode::Enter => {
                let selected_account_id = if binding && *selected == 0 {
                    None
                } else {
                    let account_idx = if binding { *selected - 1 } else { *selected };
                    accounts.get(account_idx).map(|account| account.id.clone())
                };

                if binding {
                    if let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() {
                        provider.set_codex_oauth_account_id(selected_account_id);
                    }
                }
                self.overlay = Overlay::None;
                Action::None
            }
            _ => Action::None,
        })
    }

    fn handle_managed_account_action_picker_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Overlay::ManagedAccountActionPicker {
            auth_provider,
            account_id,
            selected,
        } = &mut self.overlay
        else {
            return None;
        };

        *selected = (*selected).min(1);

        Some(match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Up => {
                *selected = selected.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                *selected = (*selected + 1).min(1);
                Action::None
            }
            KeyCode::Enter => {
                let auth_provider = auth_provider.clone();
                let account_id = account_id.clone();
                let action = match *selected {
                    0 => Action::ManagedAuthSetDefault {
                        auth_provider,
                        account_id,
                    },
                    _ => Action::ManagedAuthRemove {
                        auth_provider,
                        account_id,
                    },
                };
                self.overlay = Overlay::None;
                action
            }
            _ => Action::None,
        })
    }

    fn handle_provider_test_menu_key(&mut self, key: KeyEvent, data: &UiData) -> Option<Action> {
        let Overlay::ProviderTestMenu {
            provider_id,
            selected,
        } = &mut self.overlay
        else {
            return None;
        };

        let items = provider_test_menu_items(&self.app_type);
        if items.is_empty() {
            self.overlay = Overlay::None;
            return Some(Action::None);
        }

        *selected = (*selected).min(items.len() - 1);

        Some(match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Up => {
                *selected = selected.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                *selected = (*selected + 1).min(items.len() - 1);
                Action::None
            }
            KeyCode::Enter => {
                let provider_id = provider_id.clone();
                let item = items[*selected];
                let row = data
                    .providers
                    .rows
                    .iter()
                    .find(|provider_row| provider_row.id == provider_id)
                    .cloned();

                self.overlay = Overlay::None;

                let Some(row) = row else {
                    return Some(Action::None);
                };

                match item {
                    ProviderTestMenuItem::Speedtest => self.provider_speedtest_action(&row),
                    ProviderTestMenuItem::StreamCheck => self.provider_stream_check_action(&row),
                }
            }
            _ => Action::None,
        })
    }

    fn handle_claude_model_picker_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Overlay::ClaudeModelPicker { .. } = &self.overlay else {
            return None;
        };

        let Some(FormState::ProviderAdd(provider)) = self.form.as_ref() else {
            self.overlay = Overlay::None;
            return Some(Action::None);
        };
        if !matches!(provider.app_type, AppType::Claude) {
            self.overlay = Overlay::None;
            return Some(Action::None);
        }

        let editing = matches!(
            self.overlay,
            Overlay::ClaudeModelPicker { editing: true, .. }
        );

        Some(if editing {
            self.handle_claude_model_picker_edit_key(key)
        } else {
            self.handle_claude_model_picker_select_key(key)
        })
    }

    fn handle_claude_model_picker_edit_key(&mut self, key: KeyEvent) -> Action {
        let selected = match &mut self.overlay {
            Overlay::ClaudeModelPicker {
                selected,
                column,
                editing,
            } => {
                *selected = (*selected).min(2);
                if !*editing {
                    return Action::None;
                }
                *column = ClaudeModelPickerColumn::Model;
                *selected
            }
            _ => return Action::None,
        };

        let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
            self.overlay = Overlay::None;
            return Action::None;
        };

        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                if provider.normalize_claude_model_input(selected) {
                    provider.mark_claude_model_config_touched();
                }
                if let Overlay::ClaudeModelPicker { editing, .. } = &mut self.overlay {
                    *editing = false;
                }
                Action::None
            }
            _ => {
                if let Some(input) = provider.claude_model_input_mut(selected) {
                    if input.apply_key(key).is_some_and(|edit| edit.changed) {
                        provider.mark_claude_model_config_touched();
                    }
                }
                Action::None
            }
        }
    }

    fn handle_claude_model_picker_select_key(&mut self, key: KeyEvent) -> Action {
        let (selected, column) = match &mut self.overlay {
            Overlay::ClaudeModelPicker {
                selected,
                column,
                editing,
            } => {
                *selected = (*selected).min(2);
                if *editing {
                    return Action::None;
                }
                if *column == ClaudeModelPickerColumn::OneM
                    && !ProviderAddFormState::claude_model_supports_one_m(*selected)
                {
                    *column = ClaudeModelPickerColumn::Model;
                }
                (*selected, *column)
            }
            _ => return Action::None,
        };

        match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Up => {
                let next = selected.saturating_sub(1);
                if let Overlay::ClaudeModelPicker {
                    selected, column, ..
                } = &mut self.overlay
                {
                    *selected = next;
                    if *column == ClaudeModelPickerColumn::OneM
                        && !ProviderAddFormState::claude_model_supports_one_m(next)
                    {
                        *column = ClaudeModelPickerColumn::Model;
                    }
                }
                Action::None
            }
            KeyCode::Down => {
                let next = (selected + 1).min(2);
                if let Overlay::ClaudeModelPicker {
                    selected, column, ..
                } = &mut self.overlay
                {
                    *selected = next;
                    if *column == ClaudeModelPickerColumn::OneM
                        && !ProviderAddFormState::claude_model_supports_one_m(next)
                    {
                        *column = ClaudeModelPickerColumn::Model;
                    }
                }
                Action::None
            }
            KeyCode::Left => {
                if let Overlay::ClaudeModelPicker { column, .. } = &mut self.overlay {
                    *column = ClaudeModelPickerColumn::Model;
                }
                Action::None
            }
            KeyCode::Right => {
                if ProviderAddFormState::claude_model_supports_one_m(selected) {
                    if let Overlay::ClaudeModelPicker { column, .. } = &mut self.overlay {
                        *column = ClaudeModelPickerColumn::OneM;
                    }
                }
                Action::None
            }
            KeyCode::Enter => {
                if column == ClaudeModelPickerColumn::Model {
                    if let Overlay::ClaudeModelPicker { editing, .. } = &mut self.overlay {
                        *editing = true;
                    }
                } else if let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() {
                    provider.toggle_claude_model_one_m(selected);
                }
                Action::None
            }
            KeyCode::Char(' ') if column == ClaudeModelPickerColumn::Model => {
                if let Some(FormState::ProviderAdd(provider)) = self.form.as_ref() {
                    let codex_oauth = provider.is_claude_codex_oauth_provider();
                    let codex_oauth_account_id = provider
                        .is_claude_codex_oauth_provider()
                        .then(|| provider.codex_oauth_account_id.clone())
                        .flatten();
                    Action::ProviderModelFetch {
                        base_url: provider.claude_base_url.value.clone(),
                        api_key: (!provider.claude_api_key.value.trim().is_empty())
                            .then(|| provider.claude_api_key.value.clone()),
                        custom_user_agent: (!provider.custom_user_agent.value.trim().is_empty())
                            .then(|| provider.custom_user_agent.value.clone()),
                        codex_oauth,
                        codex_oauth_account_id,
                        field: ProviderAddField::ClaudeModelConfig,
                        claude_idx: Some(selected),
                    }
                } else {
                    Action::None
                }
            }
            KeyCode::Char('a') => {
                let source_idx = selected;
                let source_empty = self
                    .form
                    .as_ref()
                    .and_then(|f| match f {
                        FormState::ProviderAdd(p) => p.claude_model_input(source_idx),
                        _ => None,
                    })
                    .map(|input| input.value.trim().is_empty())
                    .unwrap_or(true);

                if source_empty {
                    self.push_toast(
                        texts::tui_claude_model_fill_all_empty_source().to_string(),
                        ToastKind::Warning,
                    );
                } else {
                    let source_label =
                        texts::tui_claude_model_label_for_index(source_idx).to_string();
                    self.overlay = Overlay::Confirm(ConfirmOverlay {
                        title: texts::tui_claude_model_fill_all_title().to_string(),
                        message: texts::tui_claude_model_fill_all_message(&source_label),
                        action: ConfirmAction::ClaudeModelFillAll { source_idx },
                    });
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    fn handle_model_fetch_picker_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Overlay::ModelFetchPicker {
            field,
            claude_idx,
            input,
            query,
            models,
            filtered_indices,
            filter_incomplete,
            selected_idx,
            selection_active,
            ..
        } = &mut self.overlay
        else {
            return None;
        };

        let filtered_len = filtered_indices.as_ref().map_or(models.len(), Vec::len);

        let is_claude_model = *field == ProviderAddField::ClaudeModelConfig;
        let restore_idx = claude_idx.unwrap_or(0);

        Some(match key.code {
            KeyCode::Esc => {
                if is_claude_model {
                    self.overlay = Overlay::ClaudeModelPicker {
                        selected: restore_idx,
                        column: ClaudeModelPickerColumn::Model,
                        editing: false,
                    };
                } else {
                    self.close_overlay();
                }
                Action::None
            }
            KeyCode::Up => {
                *selected_idx = selected_idx.saturating_sub(1);
                *selection_active = filtered_len > 0;
                Action::None
            }
            KeyCode::Down => {
                if filtered_len > 0 {
                    *selected_idx = (*selected_idx + 1).min(filtered_len - 1);
                }
                *selection_active = filtered_len > 0;
                Action::None
            }
            KeyCode::Tab => {
                *selection_active = filtered_len > 0;
                Action::None
            }
            KeyCode::Enter => {
                let input_is_blank = input.value.len() <= MODEL_FETCH_QUERY_MAX_BYTES
                    && input.value.trim().is_empty();
                let use_fetched_model = *selection_active || input_is_blank;
                let selected_model = if use_fetched_model {
                    let selected = if *selection_active { *selected_idx } else { 0 };
                    let Some(model_index) = model_fetch_model_index(
                        models.len(),
                        filtered_indices.as_deref(),
                        selected,
                    ) else {
                        self.close_overlay();
                        return Some(Action::None);
                    };
                    std::mem::take(&mut models[model_index])
                } else if input.value.len() <= MODEL_FETCH_QUERY_MAX_BYTES {
                    let value = std::mem::take(&mut input.value);
                    let selected = value.trim().to_string();
                    if selected.is_empty() {
                        self.close_overlay();
                        return Some(Action::None);
                    }
                    selected
                } else {
                    *filter_incomplete = true;
                    return Some(Action::None);
                };

                let field = *field;
                let claude_idx = *claude_idx;

                if field == ProviderAddField::ClaudeModelConfig {
                    self.overlay = Overlay::ClaudeModelPicker {
                        selected: claude_idx.unwrap_or(0),
                        column: ClaudeModelPickerColumn::Model,
                        editing: false,
                    };
                } else {
                    self.close_overlay();
                }

                if let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() {
                    if field == ProviderAddField::ClaudeModelConfig {
                        if let Some(idx) = claude_idx {
                            provider.set_claude_model_from_picker(idx, &selected_model);
                        }
                    } else if field == ProviderAddField::HermesModels {
                        provider.set_selected_hermes_model_id_from_picker(&selected_model);
                    } else if field == ProviderAddField::CodexLocalRouting {
                        provider.upsert_codex_model_catalog_model(&selected_model);
                    } else if let Some(input_field) = provider.input_mut(field) {
                        input_field.set(selected_model);
                    }
                }
                Action::None
            }
            _ => {
                if let Some(edit) = input.apply_key_with_policy(
                    key,
                    TextInputPolicy {
                        max_chars: Some(MODEL_FETCH_QUERY_MAX_CHARS),
                        sanitize: None,
                    },
                ) {
                    *selection_active = false;
                    if edit.changed {
                        *query = input.value.clone();
                        let filter = model_fetch_filter(models, query);
                        *filtered_indices = filter.indices;
                        *filter_incomplete = filter.incomplete;
                        *selected_idx = 0;
                    }
                }
                Action::None
            }
        })
    }

    fn handle_session_project_picker_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Overlay::SessionProjectPicker(picker) = &mut self.overlay else {
            return None;
        };

        let option_count = session_project_option_count(&self.sessions, picker);
        let filtered_len = picker
            .filtered_indices
            .as_ref()
            .map_or(option_count, Vec::len);

        Some(match key.code {
            KeyCode::Home if key.modifiers.contains(KeyModifiers::SHIFT) => {
                picker.path_scroll = 0;
                Action::None
            }
            KeyCode::End if key.modifiers.contains(KeyModifiers::SHIFT) => {
                picker.path_scroll = usize::MAX;
                Action::None
            }
            KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                picker.path_scroll = session_project_option_at(
                    &self.sessions,
                    picker,
                    session_project_option_index(
                        option_count,
                        picker.filtered_indices.as_deref(),
                        picker.selected_idx,
                    )
                    .unwrap_or(0),
                )
                .and_then(|option| match option {
                    SessionProjectOption::Exact { display_path, .. } => {
                        Some(project_path_scroll_left(display_path, picker.path_scroll))
                    }
                    _ => None,
                })
                .unwrap_or(0);
                Action::None
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
                picker.path_scroll = session_project_option_at(
                    &self.sessions,
                    picker,
                    session_project_option_index(
                        option_count,
                        picker.filtered_indices.as_deref(),
                        picker.selected_idx,
                    )
                    .unwrap_or(0),
                )
                .and_then(|option| match option {
                    SessionProjectOption::Exact { display_path, .. } => {
                        Some(project_path_scroll_right(display_path, picker.path_scroll))
                    }
                    _ => None,
                })
                .unwrap_or(0);
                Action::None
            }
            KeyCode::Esc => {
                self.close_overlay();
                Action::SessionsProjectFilterCancel
            }
            KeyCode::Up => {
                picker.selected_idx = picker.selected_idx.saturating_sub(1);
                picker.path_scroll = 0;
                Action::None
            }
            KeyCode::Down => {
                if filtered_len > 0 {
                    picker.selected_idx =
                        picker.selected_idx.saturating_add(1).min(filtered_len - 1);
                }
                picker.path_scroll = 0;
                Action::None
            }
            KeyCode::PageUp => {
                picker.selected_idx = picker
                    .selected_idx
                    .saturating_sub(SESSION_PROJECT_PICKER_PAGE_STEP);
                picker.path_scroll = 0;
                Action::None
            }
            KeyCode::PageDown => {
                if filtered_len > 0 {
                    picker.selected_idx = picker
                        .selected_idx
                        .saturating_add(SESSION_PROJECT_PICKER_PAGE_STEP)
                        .min(filtered_len - 1);
                }
                picker.path_scroll = 0;
                Action::None
            }
            KeyCode::Home => {
                picker.selected_idx = 0;
                picker.path_scroll = 0;
                Action::None
            }
            KeyCode::End => {
                picker.selected_idx = filtered_len.saturating_sub(1);
                picker.path_scroll = 0;
                Action::None
            }
            KeyCode::Enter => {
                let Some(option_index) = session_project_option_index(
                    option_count,
                    picker.filtered_indices.as_deref(),
                    picker.selected_idx,
                ) else {
                    return Some(Action::None);
                };
                let Some(scope) = session_project_option_at(&self.sessions, picker, option_index)
                    .map(session_project_option_scope)
                else {
                    return Some(Action::None);
                };
                self.close_overlay();
                Action::SessionsProjectApply { scope }
            }
            _ => {
                if picker
                    .input
                    .apply_key_with_policy(
                        key,
                        TextInputPolicy {
                            max_chars: Some(SESSION_PROJECT_FILTER_MAX_CHARS),
                            sanitize: None,
                        },
                    )
                    .is_some_and(|edit| edit.changed)
                {
                    let query = picker.input.value.trim().to_lowercase();
                    picker.filter_error = None;
                    if query.is_empty() {
                        picker.filtered_indices = None;
                    } else {
                        picker.filtered_indices = Some(Vec::new());
                    }
                    picker.selected_idx = if query.is_empty() {
                        session_project_active_option_index(&self.sessions, picker)
                    } else {
                        0
                    };
                    picker.path_scroll = 0;
                    if query.is_empty() {
                        Action::SessionsProjectFilterCancel
                    } else {
                        Action::SessionsProjectFilter { query }
                    }
                } else {
                    Action::None
                }
            }
        })
    }

    fn handle_openclaw_tools_profile_picker_key(
        &mut self,
        key: KeyEvent,
        data: &UiData,
    ) -> Option<Action> {
        let Overlay::OpenClawToolsProfilePicker { selected } = &mut self.overlay else {
            return None;
        };

        Some(match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Up => {
                *selected = Some(match *selected {
                    Some(selected) => selected.saturating_sub(1),
                    None => super::OPENCLAW_TOOLS_PROFILE_PICKER_LEN.saturating_sub(1),
                });
                Action::None
            }
            KeyCode::Down => {
                *selected = Some(match *selected {
                    Some(selected) => (selected + 1)
                        .min(super::OPENCLAW_TOOLS_PROFILE_PICKER_LEN.saturating_sub(1)),
                    None => 0,
                });
                Action::None
            }
            KeyCode::Enter => {
                let Some(selected) = *selected else {
                    return Some(Action::None);
                };
                let next_profile =
                    super::openclaw_tools_profile_for_picker_index(selected).map(str::to_string);
                self.overlay = Overlay::None;
                self.mutate_openclaw_tools_form(data, move |form| {
                    form.profile = next_profile;
                })
            }
            _ => Action::None,
        })
    }

    fn handle_openclaw_agents_fallback_picker_key(
        &mut self,
        key: KeyEvent,
        data: &UiData,
    ) -> Option<Action> {
        let Overlay::OpenClawAgentsFallbackPicker {
            insert_at,
            selected,
            options,
            ..
        } = &mut self.overlay
        else {
            return None;
        };

        Some(match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Up => {
                if !options.is_empty() {
                    *selected = if *selected == OPENCLAW_AGENTS_MODEL_PICKER_NONE {
                        options.len().saturating_sub(1)
                    } else {
                        selected.saturating_sub(1)
                    };
                }
                Action::None
            }
            KeyCode::Down => {
                if !options.is_empty() {
                    *selected = if *selected == OPENCLAW_AGENTS_MODEL_PICKER_NONE {
                        0
                    } else {
                        (*selected + 1).min(options.len() - 1)
                    };
                }
                Action::None
            }
            KeyCode::Enter => {
                let Some(option) = options.get(*selected).cloned() else {
                    return Some(Action::None);
                };
                let insert_at = *insert_at;
                let Some(form) = self.openclaw_agents_form.as_ref() else {
                    self.overlay = Overlay::None;
                    return Some(Action::None);
                };
                let section = form.section;
                let row = form.row;
                let fallback_len = form.fallbacks.len();
                self.overlay = Overlay::None;
                match section {
                    OpenClawAgentsSection::PrimaryModel => {
                        self.mutate_openclaw_agents_form(data, |form| {
                            form.primary_model = option.value;
                        })
                    }
                    OpenClawAgentsSection::FallbackModels if row < fallback_len => self
                        .mutate_openclaw_agents_form(data, |form| {
                            let target_row = insert_at.min(form.fallbacks.len().saturating_sub(1));
                            form.row = target_row;
                            form.set_current_fallback(target_row, option.value);
                        }),
                    OpenClawAgentsSection::FallbackModels => {
                        self.mutate_openclaw_agents_form(data, |form| {
                            form.row = insert_at.min(form.fallbacks.len());
                            form.insert_fallback(option.value);
                        })
                    }
                    OpenClawAgentsSection::Runtime => Action::None,
                }
            }
            _ => Action::None,
        })
    }

    fn handle_mcp_apps_picker_key(&mut self, key: KeyEvent, data: &UiData) -> Option<Action> {
        let Overlay::McpAppsPicker {
            id, selected, apps, ..
        } = &mut self.overlay
        else {
            return None;
        };

        Some(match key.code {
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
            KeyCode::Char(' ') => {
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
        })
    }

    fn handle_mcp_type_picker_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Overlay::McpTypePicker { selected } = &mut self.overlay else {
            return None;
        };

        Some(match key.code {
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
                let next = McpTransport::from_picker_index(*selected);
                self.overlay = Overlay::None;
                if let Some(FormState::McpAdd(mcp)) = self.form.as_mut() {
                    mcp.server_type = next;
                    let fields = mcp.fields();
                    if !fields.is_empty() {
                        mcp.field_idx = mcp.field_idx.min(fields.len() - 1);
                    }
                    mcp.clear_text_edit();
                }
                Action::None
            }
            _ => Action::None,
        })
    }

    fn handle_visible_apps_picker_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Overlay::VisibleAppsPicker { selected, apps } = &mut self.overlay else {
            return None;
        };

        Some(match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Up => {
                *selected = selected.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                *selected = (*selected + 1).min(5);
                Action::None
            }
            KeyCode::Char(' ') => {
                let app_type = app_type_for_picker_index(*selected);
                let mut next = apps.clone();
                let enabled = next.is_enabled_for(&app_type);
                next.set_enabled_for(&app_type, !enabled);

                if crate::settings::get_visible_apps_settings().mode
                    == crate::settings::VisibleAppsMode::Auto
                {
                    self.overlay = Overlay::Confirm(ConfirmOverlay {
                        title: texts::tui_visible_apps_manual_switch_prompt_title().to_string(),
                        message: texts::tui_visible_apps_manual_switch_prompt_message().to_string(),
                        action: ConfirmAction::VisibleAppsSwitchToManual {
                            apps: next,
                            selected: *selected,
                        },
                    });
                    return Some(Action::None);
                }

                apps.set_enabled_for(&app_type, !enabled);
                Action::None
            }
            KeyCode::Enter => {
                let next = apps.clone();
                if next.ordered_enabled().is_empty() {
                    self.push_toast(
                        texts::tui_toast_visible_apps_zero_selection_warning(),
                        ToastKind::Warning,
                    );
                    return Some(Action::None);
                }

                let unchanged = crate::settings::get_visible_apps() == next;
                self.overlay = Overlay::None;
                if unchanged {
                    Action::None
                } else {
                    Action::SetVisibleApps { apps: next }
                }
            }
            _ => Action::None,
        })
    }

    fn handle_skills_apps_picker_key(&mut self, key: KeyEvent, data: &UiData) -> Option<Action> {
        let Overlay::SkillsAppsPicker {
            directory,
            selected,
            apps,
            ..
        } = &mut self.overlay
        else {
            return None;
        };

        Some(match key.code {
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
            KeyCode::Char(' ') => {
                let app_type = app_type_for_picker_index(*selected);
                let enabled = apps.is_enabled_for(&app_type);
                apps.set_enabled_for(&app_type, !enabled);
                Action::None
            }
            KeyCode::Enter => {
                let directory = directory.clone();
                let next = apps.clone();
                let unchanged = data
                    .skills
                    .installed
                    .iter()
                    .find(|skill| skill.directory == directory)
                    .map(|skill| skill.apps == next)
                    .unwrap_or(false);

                self.overlay = Overlay::None;
                if unchanged {
                    Action::None
                } else {
                    Action::SkillsSetApps {
                        directory,
                        apps: next,
                    }
                }
            }
            _ => Action::None,
        })
    }

    fn handle_skills_import_picker_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Overlay::SkillsImportPicker {
            skills,
            selected_idx,
            selected,
        } = &mut self.overlay
        else {
            return None;
        };

        Some(match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Up => {
                *selected_idx = selected_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                if !skills.is_empty() {
                    *selected_idx = (*selected_idx + 1).min(skills.len() - 1);
                }
                Action::None
            }
            KeyCode::Char(' ') => {
                let Some(skill) = skills.get(*selected_idx) else {
                    return Some(Action::None);
                };
                if selected.contains(&skill.directory) {
                    selected.remove(&skill.directory);
                } else {
                    selected.insert(skill.directory.clone());
                }
                Action::None
            }
            KeyCode::Char('r') => Action::SkillsOpenImport,
            KeyCode::Char('i') | KeyCode::Enter => {
                if selected.is_empty() {
                    self.push_toast(texts::tui_toast_no_unmanaged_selected(), ToastKind::Info);
                    return Some(Action::None);
                }

                let imports = skills
                    .iter()
                    .filter(|skill| selected.contains(&skill.directory))
                    .map(|skill| crate::services::skill::ImportSkillSelection {
                        directory: skill.directory.clone(),
                        apps: crate::app_config::SkillApps::from_labels(&skill.found_in),
                    })
                    .collect();
                self.overlay = Overlay::None;
                Action::SkillsImportFromApps { imports }
            }
            _ => Action::None,
        })
    }

    fn handle_failover_queue_manager_key(
        &mut self,
        key: KeyEvent,
        data: &UiData,
    ) -> Option<Action> {
        let Overlay::FailoverQueueManager {
            selected_provider_id,
        } = &mut self.overlay
        else {
            return None;
        };

        let rows = failover_queue_rows(data);
        if rows.is_empty() {
            return Some(match key.code {
                KeyCode::Esc => {
                    self.overlay = Overlay::None;
                    Action::None
                }
                KeyCode::Char('f') => self.request_auto_failover_toggle(data),
                _ => Action::None,
            });
        }

        let selected = failover_queue_selected_index(data, selected_provider_id.as_deref())
            .unwrap_or_default();
        let selected_row = rows[selected];
        let selected_id = selected_row.id.clone();
        let selected_is_queued = selected_row.provider.in_failover_queue;
        if selected_provider_id.as_deref() != Some(selected_id.as_str()) {
            *selected_provider_id = Some(selected_id.clone());
        }

        Some(match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if selected_is_queued {
                    Action::ProviderMoveFailoverQueue {
                        id: selected_id,
                        direction: MoveDirection::Up,
                    }
                } else {
                    Action::None
                }
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if selected_is_queued {
                    Action::ProviderMoveFailoverQueue {
                        id: selected_id,
                        direction: MoveDirection::Down,
                    }
                } else {
                    Action::None
                }
            }
            KeyCode::Up => {
                *selected_provider_id = Some(rows[selected.saturating_sub(1)].id.clone());
                Action::None
            }
            KeyCode::Down => {
                *selected_provider_id = Some(rows[(selected + 1).min(rows.len() - 1)].id.clone());
                Action::None
            }
            KeyCode::Char('f') => self.request_auto_failover_toggle(data),
            KeyCode::Enter => Action::ProviderSetFailoverQueue {
                id: selected_id,
                enabled: !selected_is_queued,
            },
            // Uppercase J/K are secondary aliases documented only in help.
            // Lowercase j/k remain normal list navigation after vim-key
            // normalization, and d retains its usual delete meaning elsewhere.
            KeyCode::Char('K') => {
                if selected_is_queued {
                    Action::ProviderMoveFailoverQueue {
                        id: selected_id,
                        direction: MoveDirection::Up,
                    }
                } else {
                    Action::None
                }
            }
            KeyCode::Char('J') => {
                if selected_is_queued {
                    Action::ProviderMoveFailoverQueue {
                        id: selected_id,
                        direction: MoveDirection::Down,
                    }
                } else {
                    Action::None
                }
            }
            _ => Action::None,
        })
    }
}
