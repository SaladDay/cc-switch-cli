use crate::settings::WebDavSyncSettings;

use super::{InlineFieldError, TextEditSession, TextInput};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebDavSyncField {
    BaseUrl,
    Username,
    Password,
    RemoteRoot,
    Profile,
}

impl WebDavSyncField {
    pub const ALL: [Self; 5] = [
        Self::BaseUrl,
        Self::Username,
        Self::Password,
        Self::RemoteRoot,
        Self::Profile,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WebDavFormSnapshot {
    base_url: String,
    username: String,
    password: String,
    remote_root: String,
    profile: String,
}

#[derive(Debug, Clone)]
pub struct WebDavSyncFormState {
    pub field_idx: usize,
    pub text_edit: Option<TextEditSession<WebDavSyncField>>,
    pub field_errors: Vec<InlineFieldError<WebDavSyncField>>,
    pub base_url: TextInput,
    pub username: TextInput,
    pub password: TextInput,
    pub remote_root: TextInput,
    pub profile: TextInput,
    pub password_touched: bool,
    original_enabled: bool,
    original_password: String,
    original_status: crate::settings::WebDavSyncStatus,
    initial: WebDavFormSnapshot,
}

impl WebDavSyncFormState {
    pub fn from_settings(settings: Option<&WebDavSyncSettings>) -> Self {
        let settings = settings.cloned().unwrap_or_default();
        let initial = WebDavFormSnapshot {
            base_url: settings.base_url.clone(),
            username: settings.username.clone(),
            password: settings.password.clone(),
            remote_root: settings.remote_root.clone(),
            profile: settings.profile.clone(),
        };
        Self {
            field_idx: 0,
            text_edit: None,
            field_errors: Vec::new(),
            base_url: TextInput::new(settings.base_url),
            username: TextInput::new(settings.username),
            password: TextInput::new(settings.password.clone()),
            remote_root: TextInput::new(settings.remote_root),
            profile: TextInput::new(settings.profile),
            password_touched: false,
            original_enabled: settings.enabled,
            original_password: settings.password,
            original_status: settings.status,
            initial,
        }
    }

    pub fn fields(&self) -> &'static [WebDavSyncField] {
        &WebDavSyncField::ALL
    }

    pub fn selected_field(&self) -> WebDavSyncField {
        Self::field_at(self.field_idx)
    }

    fn field_at(index: usize) -> WebDavSyncField {
        WebDavSyncField::ALL
            .get(index.min(WebDavSyncField::ALL.len() - 1))
            .copied()
            .unwrap_or(WebDavSyncField::BaseUrl)
    }

    pub fn input(&self, field: WebDavSyncField) -> &TextInput {
        match field {
            WebDavSyncField::BaseUrl => &self.base_url,
            WebDavSyncField::Username => &self.username,
            WebDavSyncField::Password => &self.password,
            WebDavSyncField::RemoteRoot => &self.remote_root,
            WebDavSyncField::Profile => &self.profile,
        }
    }

    pub fn input_mut(&mut self, field: WebDavSyncField) -> &mut TextInput {
        match field {
            WebDavSyncField::BaseUrl => &mut self.base_url,
            WebDavSyncField::Username => &mut self.username,
            WebDavSyncField::Password => &mut self.password,
            WebDavSyncField::RemoteRoot => &mut self.remote_root,
            WebDavSyncField::Profile => &mut self.profile,
        }
    }

    pub fn begin_text_edit(&mut self, field: WebDavSyncField) {
        let original = self.input(field).clone();
        let original_error = self.field_error(field).map(str::to_string);
        self.clear_field_error(field);
        self.text_edit = Some(TextEditSession::new(field, original, original_error));
    }

    pub fn text_edit_target(&self) -> Option<WebDavSyncField> {
        self.text_edit.as_ref().map(TextEditSession::target)
    }

    pub fn commit_text_edit(&mut self) -> Option<WebDavSyncField> {
        let (field, original, _) = self.text_edit.take()?.into_parts();
        if field == WebDavSyncField::Password && self.input(field).value != original.value {
            self.password_touched = true;
        }
        Some(field)
    }

    pub fn cancel_text_edit(&mut self) -> Option<WebDavSyncField> {
        let (field, original, original_error) = self.text_edit.take()?.into_parts();
        *self.input_mut(field) = original;
        if let Some(message) = original_error {
            self.set_field_error(field, message);
        } else {
            self.clear_field_error(field);
        }
        Some(field)
    }

    pub fn field_error(&self, field: WebDavSyncField) -> Option<&str> {
        self.field_errors
            .iter()
            .find(|error| error.field == field)
            .map(|error| error.message.as_str())
    }

    pub fn set_field_error(&mut self, field: WebDavSyncField, message: impl Into<String>) {
        self.clear_field_error(field);
        self.field_errors.push(InlineFieldError {
            field,
            message: message.into(),
        });
    }

    pub fn clear_field_error(&mut self, field: WebDavSyncField) {
        self.field_errors.retain(|error| error.field != field);
    }

    pub fn clear_errors(&mut self) {
        self.field_errors.clear();
    }

    pub fn has_unsaved_changes(&self) -> bool {
        self.snapshot() != self.initial
    }

    pub fn is_editing(&self) -> bool {
        self.text_edit.is_some()
    }

    pub fn to_settings(&self) -> WebDavSyncSettings {
        let password = if !self.password_touched && self.password.value.is_empty() {
            self.original_password.clone()
        } else {
            self.password.value.clone()
        };
        WebDavSyncSettings {
            enabled: self.original_enabled,
            auto_sync: false,
            base_url: self.base_url.value.clone(),
            username: self.username.value.clone(),
            password,
            remote_root: self.remote_root.value.clone(),
            profile: self.profile.value.clone(),
            status: self.original_status.clone(),
        }
    }

    fn snapshot(&self) -> WebDavFormSnapshot {
        WebDavFormSnapshot {
            base_url: self.base_url.value.clone(),
            username: self.username.value.clone(),
            password: self.password.value.clone(),
            remote_root: self.remote_root.value.clone(),
            profile: self.profile.value.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_hides_nonfunctional_auto_sync_and_preserves_status() {
        let settings = WebDavSyncSettings {
            auto_sync: true,
            status: crate::settings::WebDavSyncStatus {
                last_error: Some("old".to_string()),
                ..crate::settings::WebDavSyncStatus::default()
            },
            ..WebDavSyncSettings::default()
        };
        let saved = WebDavSyncFormState::from_settings(Some(&settings)).to_settings();
        assert!(!saved.auto_sync);
        assert_eq!(saved.status.last_error.as_deref(), Some("old"));
    }
}
