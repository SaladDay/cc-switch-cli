use crate::settings::S3SyncSettings;

use super::{InlineFieldError, TextEditSession, TextInput};
use crate::cli::tui::text_edit::passive_text_contains;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum S3Preset {
    Aws,
    Minio,
    CloudflareR2,
    Custom,
}

impl S3Preset {
    // OSS/COS/OBS stay out of the first TUI release until upstream settles
    // virtual-hosted/path-style endpoint handling for those providers.
    pub const ALL: [Self; 4] = [Self::Aws, Self::Minio, Self::CloudflareR2, Self::Custom];

    pub fn picker_index(self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(Self::ALL.len() - 1)
    }

    pub fn from_picker_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or(Self::Custom)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Aws => crate::cli::i18n::texts::tui_s3_preset_aws(),
            Self::Minio => crate::cli::i18n::texts::tui_s3_preset_minio(),
            Self::CloudflareR2 => crate::cli::i18n::texts::tui_s3_preset_r2(),
            Self::Custom => crate::cli::i18n::texts::tui_s3_preset_custom(),
        }
    }

    pub fn detect(endpoint: &str) -> Self {
        if endpoint.trim().is_empty() {
            return Self::Aws;
        }
        if passive_text_contains(endpoint, "r2.cloudflarestorage.com") {
            Self::CloudflareR2
        } else if passive_text_contains(endpoint, "amazonaws.com") {
            Self::Aws
        } else {
            Self::Custom
        }
    }

    pub fn default_region(self) -> &'static str {
        match self {
            Self::CloudflareR2 => "auto",
            Self::Aws | Self::Minio | Self::Custom => "us-east-1",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum S3SyncField {
    Preset,
    Region,
    Bucket,
    AccessKeyId,
    SecretAccessKey,
    Endpoint,
    RemoteRoot,
    Profile,
}

impl S3SyncField {
    pub const ALL: [Self; 8] = [
        Self::Preset,
        Self::Region,
        Self::Bucket,
        Self::AccessKeyId,
        Self::SecretAccessKey,
        Self::Endpoint,
        Self::RemoteRoot,
        Self::Profile,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct S3SyncFormSnapshot {
    region: String,
    bucket: String,
    access_key_id: String,
    secret_access_key: String,
    endpoint: String,
    remote_root: String,
    profile: String,
}

#[derive(Debug, Clone)]
pub struct S3SyncFormState {
    pub field_idx: usize,
    pub preset: S3Preset,
    pub text_edit: Option<TextEditSession<S3SyncField>>,
    pub field_errors: Vec<InlineFieldError<S3SyncField>>,
    pub region: TextInput,
    pub bucket: TextInput,
    pub access_key_id: TextInput,
    pub secret_access_key: TextInput,
    pub endpoint: TextInput,
    pub remote_root: TextInput,
    pub profile: TextInput,
    pub secret_touched: bool,
    original_enabled: bool,
    original_secret: String,
    original_status: crate::settings::WebDavSyncStatus,
    initial: S3SyncFormSnapshot,
}

impl S3SyncFormState {
    pub fn from_settings(settings: Option<&S3SyncSettings>) -> Self {
        let settings = settings.cloned().unwrap_or_default();
        let initial = S3SyncFormSnapshot {
            region: settings.region.clone(),
            bucket: settings.bucket.clone(),
            access_key_id: settings.access_key_id.clone(),
            secret_access_key: settings.secret_access_key.clone(),
            endpoint: settings.endpoint.clone(),
            remote_root: settings.remote_root.clone(),
            profile: settings.profile.clone(),
        };
        Self {
            field_idx: 0,
            preset: S3Preset::detect(&settings.endpoint),
            text_edit: None,
            field_errors: Vec::new(),
            region: TextInput::new(settings.region),
            bucket: TextInput::new(settings.bucket),
            access_key_id: TextInput::new(settings.access_key_id),
            secret_access_key: TextInput::new(settings.secret_access_key.clone()),
            endpoint: TextInput::new(settings.endpoint),
            remote_root: TextInput::new(settings.remote_root),
            profile: TextInput::new(settings.profile),
            secret_touched: false,
            original_enabled: settings.enabled,
            original_secret: settings.secret_access_key,
            original_status: settings.status,
            initial,
        }
    }

    pub fn fields(&self) -> &'static [S3SyncField] {
        &S3SyncField::ALL
    }

    pub fn selected_field(&self) -> S3SyncField {
        S3SyncField::ALL
            .get(self.field_idx.min(S3SyncField::ALL.len() - 1))
            .copied()
            .unwrap_or(S3SyncField::Preset)
    }

    pub fn input(&self, field: S3SyncField) -> Option<&TextInput> {
        match field {
            S3SyncField::Preset => None,
            S3SyncField::Region => Some(&self.region),
            S3SyncField::Bucket => Some(&self.bucket),
            S3SyncField::AccessKeyId => Some(&self.access_key_id),
            S3SyncField::SecretAccessKey => Some(&self.secret_access_key),
            S3SyncField::Endpoint => Some(&self.endpoint),
            S3SyncField::RemoteRoot => Some(&self.remote_root),
            S3SyncField::Profile => Some(&self.profile),
        }
    }

    pub fn input_mut(&mut self, field: S3SyncField) -> Option<&mut TextInput> {
        match field {
            S3SyncField::Preset => None,
            S3SyncField::Region => Some(&mut self.region),
            S3SyncField::Bucket => Some(&mut self.bucket),
            S3SyncField::AccessKeyId => Some(&mut self.access_key_id),
            S3SyncField::SecretAccessKey => Some(&mut self.secret_access_key),
            S3SyncField::Endpoint => Some(&mut self.endpoint),
            S3SyncField::RemoteRoot => Some(&mut self.remote_root),
            S3SyncField::Profile => Some(&mut self.profile),
        }
    }

    pub fn begin_text_edit(&mut self, field: S3SyncField) -> bool {
        let Some(original) = self.input(field).cloned() else {
            return false;
        };
        let original_error = self.field_error(field).map(str::to_string);
        self.clear_field_error(field);
        self.text_edit = Some(TextEditSession::new(field, original, original_error));
        true
    }

    pub fn text_edit_target(&self) -> Option<S3SyncField> {
        self.text_edit.as_ref().map(TextEditSession::target)
    }

    pub fn commit_text_edit(&mut self) -> Option<S3SyncField> {
        let edit = self.text_edit.take()?;
        let (field, original, _) = edit.into_parts();
        if field == S3SyncField::SecretAccessKey
            && self
                .input(field)
                .is_some_and(|input| input.value != original.value)
        {
            self.secret_touched = true;
        }
        Some(field)
    }

    pub fn cancel_text_edit(&mut self) -> Option<S3SyncField> {
        let (field, original, original_error) = self.text_edit.take()?.into_parts();
        if let Some(input) = self.input_mut(field) {
            *input = original;
        }
        if let Some(message) = original_error {
            self.set_field_error(field, message);
        } else {
            self.clear_field_error(field);
        }
        Some(field)
    }

    pub fn apply_preset(&mut self, preset: S3Preset) {
        self.preset = preset;
        if self.region.is_blank() {
            self.region.set(preset.default_region());
        }
    }

    pub fn field_error(&self, field: S3SyncField) -> Option<&str> {
        self.field_errors
            .iter()
            .find(|error| error.field == field)
            .map(|error| error.message.as_str())
    }

    pub fn set_field_error(&mut self, field: S3SyncField, message: impl Into<String>) {
        self.clear_field_error(field);
        self.field_errors.push(InlineFieldError {
            field,
            message: message.into(),
        });
    }

    pub fn clear_field_error(&mut self, field: S3SyncField) {
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

    pub fn to_settings(&self) -> S3SyncSettings {
        let secret_access_key = if !self.secret_touched && self.secret_access_key.value.is_empty() {
            self.original_secret.clone()
        } else {
            self.secret_access_key.value.clone()
        };
        S3SyncSettings {
            enabled: self.original_enabled,
            auto_sync: false,
            region: self.region.value.clone(),
            bucket: self.bucket.value.clone(),
            access_key_id: self.access_key_id.value.clone(),
            secret_access_key,
            endpoint: self.endpoint.value.clone(),
            remote_root: self.remote_root.value.clone(),
            profile: self.profile.value.clone(),
            status: self.original_status.clone(),
        }
    }

    fn snapshot(&self) -> S3SyncFormSnapshot {
        S3SyncFormSnapshot {
            region: self.region.value.clone(),
            bucket: self.bucket.value.clone(),
            access_key_id: self.access_key_id.value.clone(),
            secret_access_key: self.secret_access_key.value.clone(),
            endpoint: self.endpoint.value.clone(),
            remote_root: self.remote_root.value.clone(),
            profile: self.profile.value.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_well_known_endpoints_without_persisting_presentation_state() {
        assert_eq!(
            S3Preset::detect("https://abc.r2.cloudflarestorage.com"),
            S3Preset::CloudflareR2
        );
        assert_eq!(
            S3Preset::detect("https://oss-cn-hangzhou.aliyuncs.com"),
            S3Preset::Custom
        );
        assert_eq!(S3Preset::detect(""), S3Preset::Aws);
    }

    #[test]
    fn applying_preset_only_fills_an_empty_region() {
        let mut form = S3SyncFormState::from_settings(None);
        form.apply_preset(S3Preset::CloudflareR2);
        assert_eq!(form.region.value, "auto");
        form.region.set("eu-west-1");
        form.apply_preset(S3Preset::Aws);
        assert_eq!(form.region.value, "eu-west-1");
    }

    #[test]
    fn untouched_blank_secret_preserves_existing_value() {
        let settings = S3SyncSettings {
            secret_access_key: "existing-secret".to_string(),
            ..S3SyncSettings::default()
        };
        let mut form = S3SyncFormState::from_settings(Some(&settings));
        form.secret_access_key.set("");

        assert_eq!(form.to_settings().secret_access_key, "existing-secret");
    }

    #[test]
    fn auto_sync_is_never_exposed_or_reenabled_by_the_tui_form() {
        let settings = S3SyncSettings {
            auto_sync: true,
            ..S3SyncSettings::default()
        };
        let form = S3SyncFormState::from_settings(Some(&settings));
        assert!(!form.to_settings().auto_sync);
    }
}
