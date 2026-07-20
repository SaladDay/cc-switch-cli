use super::*;

impl App {
    pub(super) fn on_webdav_sync_form_key(&mut self, key: KeyEvent) -> Action {
        if is_save_shortcut(key) {
            if let Some(FormState::WebDavSync(form)) = self.form.as_mut() {
                form.commit_text_edit();
            }
            return self.build_webdav_sync_form_save_action();
        }

        let editing = self.form.as_ref().and_then(|state| match state {
            FormState::WebDavSync(form) => form.text_edit_target(),
            _ => None,
        });
        if let Some(field) = editing {
            return match key.code {
                KeyCode::Esc => {
                    if let Some(FormState::WebDavSync(form)) = self.form.as_mut() {
                        form.cancel_text_edit();
                    }
                    Action::None
                }
                KeyCode::Enter => {
                    if let Some(FormState::WebDavSync(form)) = self.form.as_mut() {
                        form.commit_text_edit();
                    }
                    Action::None
                }
                _ => {
                    if TextEditCommand::from_key(key).is_some() {
                        if let Some(FormState::WebDavSync(form)) = self.form.as_mut() {
                            if form
                                .input_mut(field)
                                .apply_key(key)
                                .is_some_and(|edit| edit.changed)
                            {
                                form.clear_field_error(field);
                            }
                        }
                    }
                    Action::None
                }
            };
        }

        match key.code {
            KeyCode::Up => {
                if let Some(FormState::WebDavSync(form)) = self.form.as_mut() {
                    form.field_idx = form.field_idx.saturating_sub(1);
                }
                Action::None
            }
            KeyCode::Down => {
                if let Some(FormState::WebDavSync(form)) = self.form.as_mut() {
                    form.field_idx = (form.field_idx + 1).min(form.fields().len() - 1);
                }
                Action::None
            }
            KeyCode::Enter => {
                if let Some(FormState::WebDavSync(form)) = self.form.as_mut() {
                    let field = form.selected_field();
                    form.begin_text_edit(field);
                }
                Action::None
            }
            KeyCode::Esc | KeyCode::Char('q') => self.handle_form_exit_key(),
            _ => Action::None,
        }
    }

    pub(super) fn build_webdav_sync_form_save_action(&mut self) -> Action {
        let validation = self.form.as_ref().and_then(|state| {
            let FormState::WebDavSync(form) = state else {
                return None;
            };
            if form.base_url.is_blank() {
                Some((
                    form::WebDavSyncField::BaseUrl,
                    texts::tui_webdav_base_url_required().to_string(),
                ))
            } else {
                let settings = form.to_settings();
                settings.validate().err().map(|error| {
                    let field = if settings.remote_root.trim().is_empty()
                        || settings.remote_root.contains("..")
                    {
                        form::WebDavSyncField::RemoteRoot
                    } else if settings.profile.trim().is_empty() || settings.profile.contains("..")
                    {
                        form::WebDavSyncField::Profile
                    } else {
                        form::WebDavSyncField::BaseUrl
                    };
                    (field, error.to_string())
                })
            }
        });
        if let Some((field, message)) = validation {
            if let Some(FormState::WebDavSync(form)) = self.form.as_mut() {
                form.set_field_error(field, message.clone());
                if let Some(index) = form
                    .fields()
                    .iter()
                    .position(|candidate| *candidate == field)
                {
                    form.field_idx = index;
                }
            }
            self.push_toast(&message, ToastKind::Warning);
            return Action::None;
        }

        let Some(FormState::WebDavSync(form)) = self.form.as_mut() else {
            return Action::None;
        };
        form.clear_errors();
        Action::ConfigWebDavSave {
            settings: form.to_settings(),
        }
    }
}
