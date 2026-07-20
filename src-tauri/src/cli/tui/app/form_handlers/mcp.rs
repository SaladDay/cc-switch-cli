use super::*;

impl App {
    pub(super) fn handle_mcp_template_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Some(FormState::McpAdd(mcp)) = self.form.as_mut() else {
            return None;
        };

        if mcp.focus != FormFocus::Templates || !matches!(mcp.mode, FormMode::Add) {
            return None;
        }

        match key.code {
            KeyCode::Left => {
                mcp.template_idx = mcp.template_idx.saturating_sub(1);
                Some(Action::None)
            }
            KeyCode::Right => {
                let max = mcp.template_count().saturating_sub(1);
                mcp.template_idx = (mcp.template_idx + 1).min(max);
                Some(Action::None)
            }
            KeyCode::Enter => {
                mcp.apply_template(mcp.template_idx);
                mcp.focus = FormFocus::Fields;
                Some(Action::None)
            }
            _ => None,
        }
    }

    pub(super) fn handle_mcp_focus_key(&mut self, key: KeyEvent, data: &UiData) -> Option<Action> {
        let Some(FormState::McpAdd(mcp)) = self.form.as_ref() else {
            return None;
        };

        match mcp.focus {
            FormFocus::Fields => self.handle_mcp_fields_key(key, data),
            FormFocus::JsonPreview => self.handle_mcp_json_preview_key(key),
            FormFocus::Templates | FormFocus::Content => None,
        }
    }

    pub(super) fn build_mcp_form_save_action(&mut self, data: &UiData) -> Action {
        let args_valid = match self.form.as_mut() {
            Some(FormState::McpAdd(mcp)) if !mcp.server_type.is_remote() => mcp.commit_args_input(),
            _ => true,
        };
        let validation = self.form.as_ref().and_then(|form| match form {
            FormState::McpAdd(mcp) if mcp.id.is_blank() => {
                Some((McpAddField::Id, texts::tui_toast_mcp_missing_fields()))
            }
            FormState::McpAdd(mcp)
                if matches!(mcp.mode, FormMode::Add)
                    && data
                        .mcp
                        .rows
                        .iter()
                        .any(|row| row.id == mcp.id.value.trim()) =>
            {
                Some((McpAddField::Id, texts::tui_toast_mcp_id_exists()))
            }
            FormState::McpAdd(mcp) if mcp.name.is_blank() => {
                Some((McpAddField::Name, texts::tui_toast_mcp_missing_fields()))
            }
            FormState::McpAdd(mcp) if mcp.server_type.is_remote() && mcp.url.is_blank() => {
                Some((McpAddField::Url, texts::tui_toast_url_empty()))
            }
            FormState::McpAdd(mcp) if !mcp.server_type.is_remote() && mcp.command.is_blank() => {
                Some((McpAddField::Command, texts::tui_toast_command_empty()))
            }
            FormState::McpAdd(_) if !args_valid => {
                Some((McpAddField::Args, texts::tui_mcp_args_invalid()))
            }
            _ => None,
        });
        if let Some((field, message)) = validation {
            if let Some(FormState::McpAdd(mcp)) = self.form.as_mut() {
                mcp.focus = FormFocus::Fields;
                if let Some(index) = mcp
                    .fields()
                    .iter()
                    .position(|candidate| *candidate == field)
                {
                    mcp.field_idx = index;
                }
                mcp.set_field_error(field, message);
            }
            self.push_toast(message, ToastKind::Warning);
            return Action::None;
        }

        let Some(FormState::McpAdd(mcp)) = self.form.as_mut() else {
            return Action::None;
        };
        mcp.field_errors.clear();

        let content = serde_json::to_string_pretty(&mcp.to_mcp_server_json_value())
            .unwrap_or_else(|_| "{}".to_string());

        Action::EditorSubmit {
            submit: match &mcp.mode {
                FormMode::Add => EditorSubmit::McpAdd,
                FormMode::Edit { id } => EditorSubmit::McpEdit { id: id.clone() },
            },
            content,
        }
    }

    fn handle_mcp_fields_key(&mut self, key: KeyEvent, data: &UiData) -> Option<Action> {
        let editing_field = self.form.as_ref().and_then(|form| match form {
            FormState::McpAdd(mcp) => mcp.text_edit_target(),
            _ => None,
        });
        if let Some(field) = editing_field {
            return self.handle_mcp_field_editing(field, key, data);
        }

        let (fields, selected) = self.prepare_mcp_field_selection()?;
        self.handle_mcp_field_navigation(fields, selected, key)
    }

    fn handle_mcp_field_editing(
        &mut self,
        selected: McpAddField,
        key: KeyEvent,
        data: &UiData,
    ) -> Option<Action> {
        match key.code {
            KeyCode::Esc => {
                let Some(FormState::McpAdd(mcp)) = self.form.as_mut() else {
                    return None;
                };
                mcp.cancel_text_edit();
                Some(Action::None)
            }
            KeyCode::Enter => {
                self.commit_active_text_edit(data);
                Some(Action::None)
            }
            _ => {
                TextEditCommand::from_key(key)?;
                let Some(FormState::McpAdd(mcp)) = self.form.as_mut() else {
                    return None;
                };
                if let Some(input) = mcp.input_mut(selected) {
                    if input.apply_key(key).is_some_and(|edit| edit.changed) {
                        mcp.clear_field_error(selected);
                    }
                }
                Some(Action::None)
            }
        }
    }

    fn handle_mcp_field_navigation(
        &mut self,
        fields: Vec<McpAddField>,
        selected: McpAddField,
        key: KeyEvent,
    ) -> Option<Action> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                let Some(FormState::McpAdd(mcp)) = self.form.as_mut() else {
                    return None;
                };
                mcp.field_idx = mcp.field_idx.saturating_sub(1);
                Some(Action::None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let Some(FormState::McpAdd(mcp)) = self.form.as_mut() else {
                    return None;
                };
                mcp.field_idx = (mcp.field_idx + 1).min(fields.len() - 1);
                Some(Action::None)
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                let Some(FormState::McpAdd(mcp)) = self.form.as_mut() else {
                    return None;
                };
                match selected {
                    McpAddField::Type => {
                        if matches!(key.code, KeyCode::Enter) {
                            self.overlay = Overlay::McpTypePicker {
                                selected: mcp.server_type.picker_index(),
                            };
                        }
                    }
                    McpAddField::Env => {
                        if matches!(key.code, KeyCode::Enter) {
                            let selected = 0;
                            self.overlay = Overlay::McpEnvPicker { selected };
                        }
                    }
                    McpAddField::AppClaude => mcp.apps.claude = !mcp.apps.claude,
                    McpAddField::AppCodex => mcp.apps.codex = !mcp.apps.codex,
                    McpAddField::AppGemini => mcp.apps.gemini = !mcp.apps.gemini,
                    McpAddField::AppOpenCode => mcp.apps.opencode = !mcp.apps.opencode,
                    McpAddField::AppHermes => mcp.apps.hermes = !mcp.apps.hermes,
                    _ => {
                        if selected == McpAddField::Id && mcp.locked_id().is_some() {
                            return Some(Action::None);
                        }
                        if matches!(key.code, KeyCode::Enter) {
                            mcp.begin_text_edit(selected);
                        }
                    }
                }
                Some(Action::None)
            }
            _ => None,
        }
    }

    fn handle_mcp_json_preview_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Some(FormState::McpAdd(mcp)) = self.form.as_mut() else {
            return None;
        };

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                mcp.json_scroll = mcp.json_scroll.saturating_sub(1);
                Some(Action::None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                mcp.json_scroll = mcp.json_scroll.saturating_add(1);
                Some(Action::None)
            }
            KeyCode::PageUp => {
                mcp.json_scroll = mcp.json_scroll.saturating_sub(10);
                Some(Action::None)
            }
            KeyCode::PageDown => {
                mcp.json_scroll = mcp.json_scroll.saturating_add(10);
                Some(Action::None)
            }
            _ => None,
        }
    }

    fn prepare_mcp_field_selection(&mut self) -> Option<(Vec<McpAddField>, McpAddField)> {
        let Some(FormState::McpAdd(mcp)) = self.form.as_mut() else {
            return None;
        };
        if mcp.focus != FormFocus::Fields {
            return None;
        }

        let fields = mcp.fields();
        if !fields.is_empty() {
            mcp.field_idx = mcp.field_idx.min(fields.len() - 1);
        } else {
            mcp.field_idx = 0;
        }

        let selected = fields.get(mcp.field_idx).copied()?;
        Some((fields, selected))
    }
}
