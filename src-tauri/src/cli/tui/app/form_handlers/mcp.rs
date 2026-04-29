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

    pub(super) fn handle_mcp_focus_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Some(FormState::McpAdd(mcp)) = self.form.as_ref() else {
            return None;
        };

        match mcp.focus {
            FormFocus::Fields => self.handle_mcp_fields_key(key),
            FormFocus::JsonPreview => self.handle_mcp_json_preview_key(key),
            FormFocus::Templates => None,
        }
    }

    pub(super) fn build_mcp_form_save_action(&mut self) -> Action {
        let Some(FormState::McpAdd(mcp)) = self.form.as_ref() else {
            return Action::None;
        };

        if !mcp.has_required_fields() {
            self.push_toast(texts::tui_toast_mcp_missing_fields(), ToastKind::Warning);
            return Action::None;
        }
        if mcp.server_type.is_remote() {
            if mcp.url.is_blank() {
                self.push_toast(texts::tui_toast_url_empty(), ToastKind::Warning);
                return Action::None;
            }
        } else if mcp.command.is_blank() {
            self.push_toast(texts::tui_toast_command_empty(), ToastKind::Warning);
            return Action::None;
        }

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

    fn handle_mcp_fields_key(&mut self, key: KeyEvent) -> Option<Action> {
        let (fields, selected, editing) = match self.prepare_mcp_field_selection() {
            Some(state) => state,
            None => return None,
        };

        if editing {
            self.handle_mcp_field_editing(selected, key)
        } else {
            self.handle_mcp_field_navigation(fields, selected, key)
        }
    }

    fn handle_mcp_field_editing(&mut self, selected: McpAddField, key: KeyEvent) -> Option<Action> {
        let Some(FormState::McpAdd(mcp)) = self.form.as_mut() else {
            return None;
        };

        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                mcp.editing = false;
                Some(Action::None)
            }
            KeyCode::Left => {
                if let Some(input) = mcp.input_mut(selected) {
                    input.move_left();
                }
                Some(Action::None)
            }
            KeyCode::Right => {
                if let Some(input) = mcp.input_mut(selected) {
                    input.move_right();
                }
                Some(Action::None)
            }
            KeyCode::Home => {
                if let Some(input) = mcp.input_mut(selected) {
                    input.move_home();
                }
                Some(Action::None)
            }
            KeyCode::End => {
                if let Some(input) = mcp.input_mut(selected) {
                    input.move_end();
                }
                Some(Action::None)
            }
            KeyCode::Backspace => {
                let _ = mcp.input_mut(selected).map(|input| input.backspace());
                Some(Action::None)
            }
            KeyCode::Delete => {
                let _ = mcp.input_mut(selected).map(|input| input.delete());
                Some(Action::None)
            }
            KeyCode::Char(c) => {
                if c.is_control() {
                    return Some(Action::None);
                }
                let _ = mcp.input_mut(selected).map(|input| input.insert_char(c));
                Some(Action::None)
            }
            _ => None,
        }
    }

    fn handle_mcp_field_navigation(
        &mut self,
        fields: Vec<McpAddField>,
        selected: McpAddField,
        key: KeyEvent,
    ) -> Option<Action> {
        match key.code {
            KeyCode::Up => {
                let Some(FormState::McpAdd(mcp)) = self.form.as_mut() else {
                    return None;
                };
                mcp.field_idx = mcp.field_idx.saturating_sub(1);
                Some(Action::None)
            }
            KeyCode::Down => {
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
                        self.overlay = Overlay::McpTypePicker {
                            selected: mcp.server_type.picker_index(),
                        };
                    }
                    McpAddField::Env => {
                        let selected = 0;
                        self.overlay = Overlay::McpEnvPicker { selected };
                    }
                    McpAddField::AppClaude => mcp.apps.claude = !mcp.apps.claude,
                    McpAddField::AppCodex => mcp.apps.codex = !mcp.apps.codex,
                    McpAddField::AppGemini => mcp.apps.gemini = !mcp.apps.gemini,
                    McpAddField::AppOpenCode => mcp.apps.opencode = !mcp.apps.opencode,
                    _ => {
                        if selected == McpAddField::Id && mcp.locked_id().is_some() {
                            return Some(Action::None);
                        }
                        if mcp.input(selected).is_some() {
                            mcp.editing = true;
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
            KeyCode::Up => {
                mcp.json_scroll = mcp.json_scroll.saturating_sub(1);
                Some(Action::None)
            }
            KeyCode::Down => {
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

    fn prepare_mcp_field_selection(&mut self) -> Option<(Vec<McpAddField>, McpAddField, bool)> {
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
        Some((fields, selected, mcp.editing))
    }
}
