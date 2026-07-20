use super::*;

impl App {
    pub(super) fn handle_prompt_meta_focus_key(
        &mut self,
        key: KeyEvent,
        data: &UiData,
    ) -> Option<Action> {
        let Some(FormState::PromptMeta(prompt)) = self.form.as_ref() else {
            return None;
        };

        match prompt.focus {
            FormFocus::Fields => self.handle_prompt_meta_fields_key(key, data),
            FormFocus::Content => self.handle_prompt_content_key(key),
            FormFocus::Templates | FormFocus::JsonPreview => None,
        }
    }

    pub(super) fn build_prompt_meta_form_save_action(&mut self) -> Action {
        let validation = self.form.as_ref().and_then(|form| match form {
            FormState::PromptMeta(prompt) => {
                let id = prompt.id_value();
                if let Err(error) = crate::services::PromptService::validate_prompt_id(&id) {
                    Some((PromptMetaField::Id, error.to_string()))
                } else if prompt.name_value().is_empty() {
                    Some((
                        PromptMetaField::Name,
                        texts::tui_toast_prompt_name_empty().to_string(),
                    ))
                } else {
                    None
                }
            }
            _ => None,
        });
        if let Some((field, message)) = validation {
            if let Some(FormState::PromptMeta(prompt)) = self.form.as_mut() {
                prompt.focus = FormFocus::Fields;
                if let Some(index) = prompt
                    .fields()
                    .iter()
                    .position(|candidate| *candidate == field)
                {
                    prompt.field_idx = index;
                }
                prompt.set_field_error(field, message.clone());
            }
            self.push_toast(message, ToastKind::Warning);
            return Action::None;
        }

        let Some(FormState::PromptMeta(prompt)) = self.form.as_mut() else {
            return Action::None;
        };
        prompt.field_errors.clear();

        let id = prompt.id_value();
        let name = prompt.name_value();
        let description = prompt.description_value();

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
        // Approximate the one-page prompt form layout using the same responsive
        // breakpoint as the renderer. In compact mode the content pane owns the
        // full form body instead of losing the fixed metadata width.
        let form_body_width = self.last_size.width.saturating_sub(30).saturating_sub(4);
        let content_pane_width = if form_body_width >= form::PROMPT_FORM_SPLIT_MIN_BODY_WIDTH {
            form_body_width.saturating_mul(58) / 100
        } else {
            form_body_width
        };
        let width = content_pane_width.saturating_sub(2).max(1);
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

    fn handle_prompt_meta_fields_key(&mut self, key: KeyEvent, data: &UiData) -> Option<Action> {
        let editing_field = self.form.as_ref().and_then(|form| match form {
            FormState::PromptMeta(prompt) => prompt.text_edit_target(),
            _ => None,
        });
        if let Some(field) = editing_field {
            return self.handle_prompt_meta_field_editing(field, key, data);
        }

        let (fields, _selected) = self.prepare_prompt_meta_field_selection()?;
        self.handle_prompt_meta_field_navigation(fields, key)
    }

    fn handle_prompt_meta_field_editing(
        &mut self,
        selected: PromptMetaField,
        key: KeyEvent,
        data: &UiData,
    ) -> Option<Action> {
        match key.code {
            KeyCode::Esc => {
                let Some(FormState::PromptMeta(prompt)) = self.form.as_mut() else {
                    return None;
                };
                prompt.cancel_text_edit();
                Some(Action::None)
            }
            KeyCode::Enter => {
                self.commit_active_text_edit(data);
                Some(Action::None)
            }
            _ => {
                TextEditCommand::from_key(key)?;
                let Some(FormState::PromptMeta(prompt)) = self.form.as_mut() else {
                    return None;
                };
                if prompt
                    .input_mut(selected)
                    .apply_key(key)
                    .is_some_and(|edit| edit.changed)
                {
                    prompt.clear_field_error(selected);
                }
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
                let selected = fields
                    .get(prompt.field_idx.min(fields.len().saturating_sub(1)))
                    .copied()?;
                prompt.begin_text_edit(selected);
                Some(Action::None)
            }
            _ => None,
        }
    }

    fn prepare_prompt_meta_field_selection(
        &mut self,
    ) -> Option<(Vec<PromptMetaField>, PromptMetaField)> {
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
        Some((fields, selected))
    }
}
