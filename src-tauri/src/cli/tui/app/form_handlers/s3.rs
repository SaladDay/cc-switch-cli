use super::*;

impl App {
    pub(super) fn on_s3_sync_form_key(&mut self, key: KeyEvent) -> Action {
        if is_save_shortcut(key) {
            if let Some(FormState::S3Sync(form)) = self.form.as_mut() {
                form.commit_text_edit();
            }
            return self.build_s3_sync_form_save_action();
        }

        let editing = self.form.as_ref().and_then(|form| match form {
            FormState::S3Sync(form) => form.text_edit_target(),
            _ => None,
        });
        if let Some(field) = editing {
            return match key.code {
                KeyCode::Esc => {
                    if let Some(FormState::S3Sync(form)) = self.form.as_mut() {
                        form.cancel_text_edit();
                    }
                    Action::None
                }
                KeyCode::Enter => {
                    if let Some(FormState::S3Sync(form)) = self.form.as_mut() {
                        form.commit_text_edit();
                    }
                    Action::None
                }
                _ => {
                    if TextEditCommand::from_key(key).is_some() {
                        if let Some(FormState::S3Sync(form)) = self.form.as_mut() {
                            if form
                                .input_mut(field)
                                .and_then(|input| input.apply_key(key))
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
                if let Some(FormState::S3Sync(form)) = self.form.as_mut() {
                    form.field_idx = form.field_idx.saturating_sub(1);
                }
                Action::None
            }
            KeyCode::Down => {
                if let Some(FormState::S3Sync(form)) = self.form.as_mut() {
                    form.field_idx = (form.field_idx + 1).min(form.fields().len() - 1);
                }
                Action::None
            }
            KeyCode::Enter => {
                let Some(FormState::S3Sync(form)) = self.form.as_mut() else {
                    return Action::None;
                };
                let field = form.selected_field();
                if field == form::S3SyncField::Preset {
                    self.overlay = Overlay::S3PresetPicker {
                        selected: form.preset.picker_index(),
                    };
                } else {
                    form.begin_text_edit(field);
                }
                Action::None
            }
            KeyCode::Esc | KeyCode::Char('q') => self.handle_form_exit_key(),
            _ => Action::None,
        }
    }

    pub(super) fn build_s3_sync_form_save_action(&mut self) -> Action {
        let validation = self.form.as_ref().and_then(|state| {
            let FormState::S3Sync(form) = state else {
                return None;
            };
            if form.bucket.is_blank() {
                Some((form::S3SyncField::Bucket, texts::tui_s3_bucket_required()))
            } else if form.region.is_blank() {
                Some((form::S3SyncField::Region, texts::tui_s3_region_required()))
            } else if form.access_key_id.is_blank() {
                Some((
                    form::S3SyncField::AccessKeyId,
                    texts::tui_s3_access_key_required(),
                ))
            } else if form.secret_access_key.is_blank() {
                Some((
                    form::S3SyncField::SecretAccessKey,
                    texts::tui_s3_secret_key_required(),
                ))
            } else {
                None
            }
        });

        if let Some((field, message)) = validation {
            if let Some(FormState::S3Sync(form)) = self.form.as_mut() {
                form.set_field_error(field, message);
                if let Some(index) = form
                    .fields()
                    .iter()
                    .position(|candidate| *candidate == field)
                {
                    form.field_idx = index;
                }
            }
            self.push_toast(message, ToastKind::Warning);
            return Action::None;
        }

        let Some(FormState::S3Sync(form)) = self.form.as_mut() else {
            return Action::None;
        };
        form.clear_errors();
        Action::ConfigS3Save {
            settings: form.to_settings(),
        }
    }
}
