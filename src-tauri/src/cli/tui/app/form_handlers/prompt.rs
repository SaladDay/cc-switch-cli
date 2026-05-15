use super::*;

impl App {
    pub(super) fn handle_prompt_meta_focus_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Some(FormState::PromptMeta(prompt)) = self.form.as_ref() else {
            return None;
        };

        match prompt.focus {
            FormFocus::Fields => self.handle_prompt_meta_fields_key(key),
            FormFocus::Content => self.handle_prompt_content_key(key),
            FormFocus::Templates | FormFocus::JsonPreview => None,
        }
    }

    pub(super) fn build_prompt_meta_form_save_action(&mut self) -> Action {
        let Some(FormState::PromptMeta(prompt)) = self.form.as_ref() else {
            return Action::None;
        };

        let id = prompt.id_value();
        let name = prompt.name_value();
        let description = prompt.description_value();

        if let Err(err) = crate::services::PromptService::validate_prompt_id(&id) {
            self.push_toast(err.to_string(), ToastKind::Warning);
            return Action::None;
        }
        if name.is_empty() {
            self.push_toast(texts::tui_toast_prompt_name_empty(), ToastKind::Warning);
            return Action::None;
        }

        Action::PromptSave {
            old_id: match &prompt.mode {
                FormMode::Add => None,
                FormMode::Edit { id } => Some(id.clone()),
            },
            new_id: id,
            name,
            description,
            content: prompt.content_value(),
        }
    }

    fn handle_prompt_content_key(&mut self, key: KeyEvent) -> Option<Action> {
        if matches!(key.code, KeyCode::Esc) {
            return None;
        }
        if is_open_external_editor_shortcut(key) {
            return Some(Action::PromptFormOpenExternal);
        }

        let viewport = self.prompt_content_viewport_size();
        let Some(FormState::PromptMeta(prompt)) = self.form.as_mut() else {
            return None;
        };

        if prompt.content.apply_editor_key(key, viewport) {
            return Some(Action::None);
        }
        None
    }

    fn prompt_content_viewport_size(&self) -> Size {
        // Approximate the one-page prompt form layout: app content minus form chrome, key bar,
        // metadata pane, and content pane borders.
        let width = self
            .last_size
            .width
            .saturating_sub(30)
            .saturating_sub(36)
            .saturating_sub(6)
            .max(1);
        let height = self
            .last_size
            .height
            .saturating_sub(3)
            .saturating_sub(1)
            .saturating_sub(2)
            .saturating_sub(1)
            .saturating_sub(2)
            .max(1);

        Size { width, height }
    }

    fn handle_prompt_meta_fields_key(&mut self, key: KeyEvent) -> Option<Action> {
        let (fields, selected, editing) = match self.prepare_prompt_meta_field_selection() {
            Some(state) => state,
            None => return None,
        };

        if editing {
            self.handle_prompt_meta_field_editing(selected, key)
        } else {
            self.handle_prompt_meta_field_navigation(fields, key)
        }
    }

    fn handle_prompt_meta_field_editing(
        &mut self,
        selected: PromptMetaField,
        key: KeyEvent,
    ) -> Option<Action> {
        let Some(FormState::PromptMeta(prompt)) = self.form.as_mut() else {
            return None;
        };

        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                prompt.editing = false;
                Some(Action::None)
            }
            _ => {
                if TextEditCommand::from_key(key).is_none() {
                    return None;
                }
                prompt.input_mut(selected).apply_key(key);
                Some(Action::None)
            }
        }
    }

    fn handle_prompt_meta_field_navigation(
        &mut self,
        fields: Vec<PromptMetaField>,
        key: KeyEvent,
    ) -> Option<Action> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                let Some(FormState::PromptMeta(prompt)) = self.form.as_mut() else {
                    return None;
                };
                prompt.field_idx = prompt.field_idx.saturating_sub(1);
                Some(Action::None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let Some(FormState::PromptMeta(prompt)) = self.form.as_mut() else {
                    return None;
                };
                prompt.field_idx = (prompt.field_idx + 1).min(fields.len() - 1);
                Some(Action::None)
            }
            KeyCode::Enter => {
                let Some(FormState::PromptMeta(prompt)) = self.form.as_mut() else {
                    return None;
                };
                prompt.editing = true;
                Some(Action::None)
            }
            _ => None,
        }
    }

    fn prepare_prompt_meta_field_selection(
        &mut self,
    ) -> Option<(Vec<PromptMetaField>, PromptMetaField, bool)> {
        let Some(FormState::PromptMeta(prompt)) = self.form.as_mut() else {
            return None;
        };
        if prompt.focus != FormFocus::Fields {
            return None;
        }

        let fields = prompt.fields();
        if !fields.is_empty() {
            prompt.field_idx = prompt.field_idx.min(fields.len() - 1);
        } else {
            prompt.field_idx = 0;
        }

        let selected = fields.get(prompt.field_idx).copied()?;
        Some((fields, selected, prompt.editing))
    }
}
