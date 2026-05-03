use super::*;

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
        if let Some(action) = self.handle_claude_model_picker_key(key) {
            return Some(action);
        }
        if let Some(action) = self.handle_model_fetch_picker_key(key) {
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

    fn handle_sync_method_picker_key(&mut self, key: KeyEvent, data: &UiData) -> Option<Action> {
        let Overlay::SkillsSyncMethodPicker { selected } = &mut self.overlay else {
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
                *selected = (*selected + 1).min(3);
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
                    crate::cli::tui::form::ClaudeApiFormat::ALL
                        .len()
                        .saturating_sub(1),
                );
                Action::None
            }
            KeyCode::Enter => {
                let next_format =
                    crate::cli::tui::form::ClaudeApiFormat::from_picker_index(*selected);
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
                if changed && next_format.requires_proxy() && !proxy_ready {
                    self.overlay = Overlay::Confirm(ConfirmOverlay {
                        title: texts::tui_claude_api_format_requires_proxy_title().to_string(),
                        message: texts::tui_claude_api_format_requires_proxy_message(
                            next_format.as_str(),
                        ),
                        action: ConfirmAction::ProviderApiFormatProxyNotice,
                    });
                }

                Action::None
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
            Overlay::ClaudeModelPicker { selected, editing } => {
                *selected = (*selected).min(4);
                if !*editing {
                    return Action::None;
                }
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
                if let Overlay::ClaudeModelPicker { editing, .. } = &mut self.overlay {
                    *editing = false;
                }
                Action::None
            }
            KeyCode::Left => {
                if let Some(input) = provider.claude_model_input_mut(selected) {
                    input.move_left();
                }
                Action::None
            }
            KeyCode::Right => {
                if let Some(input) = provider.claude_model_input_mut(selected) {
                    input.move_right();
                }
                Action::None
            }
            KeyCode::Home => {
                if let Some(input) = provider.claude_model_input_mut(selected) {
                    input.move_home();
                }
                Action::None
            }
            KeyCode::End => {
                if let Some(input) = provider.claude_model_input_mut(selected) {
                    input.move_end();
                }
                Action::None
            }
            KeyCode::Backspace => {
                if let Some(input) = provider.claude_model_input_mut(selected) {
                    if input.backspace() {
                        provider.mark_claude_model_config_touched();
                    }
                }
                Action::None
            }
            KeyCode::Delete => {
                if let Some(input) = provider.claude_model_input_mut(selected) {
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
                if let Some(input) = provider.claude_model_input_mut(selected) {
                    if input.insert_char(c) {
                        provider.mark_claude_model_config_touched();
                    }
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    fn handle_claude_model_picker_select_key(&mut self, key: KeyEvent) -> Action {
        let selected = match &mut self.overlay {
            Overlay::ClaudeModelPicker { selected, editing } => {
                *selected = (*selected).min(4);
                if *editing {
                    return Action::None;
                }
                selected
            }
            _ => return Action::None,
        };

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
            KeyCode::Enter => {
                if let Some(FormState::ProviderAdd(provider)) = self.form.as_ref() {
                    Action::ProviderModelFetch {
                        base_url: provider.claude_base_url.value.clone(),
                        api_key: (!provider.claude_api_key.value.trim().is_empty())
                            .then(|| provider.claude_api_key.value.clone()),
                        field: ProviderAddField::ClaudeModelConfig,
                        claude_idx: Some(*selected),
                    }
                } else {
                    Action::None
                }
            }
            KeyCode::Char(' ') => {
                if let Overlay::ClaudeModelPicker { editing, .. } = &mut self.overlay {
                    *editing = true;
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
            selected_idx,
            ..
        } = &mut self.overlay
        else {
            return None;
        };

        let filtered: Vec<&String> = if query.trim().is_empty() {
            models.iter().collect()
        } else {
            let q = query.trim().to_lowercase();
            models
                .iter()
                .filter(|model| model.to_lowercase().contains(&q))
                .collect()
        };

        Some(match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Action::None
            }
            KeyCode::Up => {
                *selected_idx = selected_idx.saturating_sub(1);
                if let Some(model) = filtered.get(*selected_idx) {
                    *input = (*model).to_string();
                }
                Action::None
            }
            KeyCode::Down => {
                if !filtered.is_empty() {
                    *selected_idx = (*selected_idx + 1).min(filtered.len() - 1);
                    if let Some(model) = filtered.get(*selected_idx) {
                        *input = (*model).to_string();
                    }
                }
                Action::None
            }
            KeyCode::Tab => {
                if let Some(model) = filtered.get(*selected_idx) {
                    *input = (*model).to_string();
                    *query = (*model).to_string();
                    *selected_idx = 0;
                }
                Action::None
            }
            KeyCode::Backspace => {
                if !input.is_empty() {
                    input.pop();
                    *query = input.clone();
                    *selected_idx = 0;
                }
                Action::None
            }
            KeyCode::Char(c) if !c.is_control() => {
                input.push(c);
                *query = input.clone();
                *selected_idx = 0;
                Action::None
            }
            KeyCode::Enter => {
                let selected_model = input.trim().to_string();
                if selected_model.is_empty() {
                    self.overlay = Overlay::None;
                    return Some(Action::None);
                }

                let field = *field;
                let claude_idx = *claude_idx;
                self.overlay = Overlay::None;

                if let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() {
                    if field == ProviderAddField::ClaudeModelConfig {
                        if let Some(idx) = claude_idx {
                            if let Some(input_field) = provider.claude_model_input_mut(idx) {
                                input_field.set(selected_model);
                                provider.mark_claude_model_config_touched();
                            }
                        }
                    } else if let Some(input_field) = provider.input_mut(field) {
                        input_field.set(selected_model);
                    }
                }
                Action::None
            }
            _ => Action::None,
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
                *selected = (*selected + 1).min(3);
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
                    mcp.editing = false;
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
                *selected = (*selected + 1).min(4);
                Action::None
            }
            KeyCode::Char('x') | KeyCode::Char(' ') => {
                let app_type = app_type_for_picker_index(*selected);
                let enabled = apps.is_enabled_for(&app_type);
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
                *selected = (*selected + 1).min(3);
                Action::None
            }
            KeyCode::Char('x') | KeyCode::Char(' ') => {
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
            KeyCode::Char('x') | KeyCode::Char(' ') => {
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

                let directories = skills
                    .iter()
                    .filter(|skill| selected.contains(&skill.directory))
                    .map(|skill| skill.directory.clone())
                    .collect();
                self.overlay = Overlay::None;
                Action::SkillsImportFromApps { directories }
            }
            _ => Action::None,
        })
    }

    fn handle_failover_queue_manager_key(
        &mut self,
        key: KeyEvent,
        data: &UiData,
    ) -> Option<Action> {
        let Overlay::FailoverQueueManager { selected } = &mut self.overlay else {
            return None;
        };

        let rows = failover_queue_rows(data);
        if rows.is_empty() {
            return Some(match key.code {
                KeyCode::Esc => {
                    self.overlay = Overlay::None;
                    Action::None
                }
                KeyCode::Char('f') => Action::SetProxyAutoFailover {
                    app_type: self.app_type.clone(),
                    enabled: !data.proxy.auto_failover_enabled,
                },
                _ => Action::None,
            });
        }

        *selected = (*selected).min(rows.len() - 1);
        let selected_row = rows[*selected];

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
                *selected = (*selected + 1).min(rows.len() - 1);
                Action::None
            }
            KeyCode::Char('f') => Action::SetProxyAutoFailover {
                app_type: self.app_type.clone(),
                enabled: !data.proxy.auto_failover_enabled,
            },
            KeyCode::Char(' ') | KeyCode::Enter => Action::ProviderSetFailoverQueue {
                id: selected_row.id.clone(),
                enabled: !selected_row.provider.in_failover_queue,
            },
            KeyCode::Char('<') | KeyCode::Char('u') => {
                if selected_row.provider.in_failover_queue {
                    Action::ProviderMoveFailoverQueue {
                        id: selected_row.id.clone(),
                        direction: MoveDirection::Up,
                    }
                } else {
                    Action::None
                }
            }
            KeyCode::Char('>') | KeyCode::Char('d') => {
                if selected_row.provider.in_failover_queue {
                    Action::ProviderMoveFailoverQueue {
                        id: selected_row.id.clone(),
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
