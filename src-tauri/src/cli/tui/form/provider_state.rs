use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::provider::Provider;
use serde_json::{json, Value};

use super::provider_json::{
    merge_json_values, should_hide_provider_field, strip_common_config_from_settings,
};
use super::provider_state_loading::populate_form_from_provider;
use super::{
    ClaudeApiFormat, CodexPreviewSection, CodexWireApi, FormFocus, FormMode, GeminiAuthType,
    ProviderAddField, ProviderAddFormState, TextInput, OPENCLAW_DEFAULT_API_PROTOCOL,
};

impl ProviderAddFormState {
    pub fn new(app_type: AppType) -> Self {
        let include_common_config = !matches!(app_type, AppType::OpenClaw);
        let openclaw_api_default = match app_type {
            AppType::OpenClaw => OPENCLAW_DEFAULT_API_PROTOCOL,
            _ => "@ai-sdk/openai-compatible",
        };

        let codex_defaults = match app_type {
            AppType::Codex => ("", "gpt-5.4", CodexWireApi::Responses, true),
            _ => ("", "", CodexWireApi::Responses, true),
        };

        let mut form = Self {
            app_type,
            mode: FormMode::Add,
            focus: FormFocus::Templates,
            template_idx: 0,
            field_idx: 0,
            editing: false,
            extra: json!({}),
            id: TextInput::new(""),
            id_is_manual: false,
            name: TextInput::new(""),
            website_url: TextInput::new(""),
            notes: TextInput::new(""),
            include_common_config,
            include_common_config_touched: false,
            json_scroll: 0,
            codex_preview_section: CodexPreviewSection::Auth,
            codex_auth_scroll: 0,
            codex_config_scroll: 0,
            claude_model_config_touched: false,
            claude_api_key: TextInput::new(""),
            claude_base_url: TextInput::new(""),
            claude_api_format: ClaudeApiFormat::Anthropic,
            claude_model: TextInput::new(""),
            claude_reasoning_model: TextInput::new(""),
            claude_haiku_model: TextInput::new(""),
            claude_sonnet_model: TextInput::new(""),
            claude_opus_model: TextInput::new(""),
            codex_base_url: TextInput::new(codex_defaults.0),
            codex_model: TextInput::new(codex_defaults.1),
            codex_wire_api: codex_defaults.2,
            codex_requires_openai_auth: codex_defaults.3,
            codex_env_key: TextInput::new("OPENAI_API_KEY"),
            codex_api_key: TextInput::new(""),
            gemini_auth_type: GeminiAuthType::ApiKey,
            gemini_api_key: TextInput::new(""),
            gemini_base_url: TextInput::new("https://generativelanguage.googleapis.com"),
            gemini_model: TextInput::new(""),
            openclaw_user_agent: false,
            openclaw_models: Vec::new(),
            opencode_npm_package: TextInput::new(openclaw_api_default),
            opencode_api_key: TextInput::new(""),
            opencode_base_url: TextInput::new(""),
            opencode_model_id: TextInput::new(""),
            opencode_model_name: TextInput::new(""),
            opencode_model_context_limit: TextInput::new(""),
            opencode_model_output_limit: TextInput::new(""),
            opencode_model_original_id: None,
            initial_snapshot: Value::Null,
        };
        form.capture_initial_snapshot();
        form
    }

    pub fn from_provider(app_type: AppType, provider: &Provider) -> Self {
        let mut form = Self::new(app_type.clone());
        form.mode = FormMode::Edit {
            id: provider.id.clone(),
        };
        form.focus = FormFocus::Fields;
        form.extra = serde_json::to_value(provider).unwrap_or_else(|_| json!({}));

        form.id.set(provider.id.clone());
        form.id_is_manual = true;
        form.name.set(provider.name.clone());
        if let Some(url) = provider.website_url.as_deref() {
            form.website_url.set(url);
        }
        if let Some(notes) = provider.notes.as_deref() {
            form.notes.set(notes);
        }
        form.include_common_config = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.apply_common_config)
            .unwrap_or(!matches!(app_type, AppType::OpenClaw));

        if matches!(app_type, AppType::OpenClaw) {
            form.include_common_config = false;
        }

        populate_form_from_provider(&mut form, &app_type, provider);
        form.capture_initial_snapshot();

        form
    }

    fn capture_initial_snapshot(&mut self) {
        self.initial_snapshot = self.to_provider_json_value();
    }

    pub fn has_unsaved_changes(&self) -> bool {
        self.to_provider_json_value() != self.initial_snapshot
    }

    pub fn is_id_editable(&self) -> bool {
        !self.mode.is_edit()
    }

    pub fn ensure_generated_id(&mut self, existing_ids: &[String]) -> bool {
        let Some(generated_id) = resolve_provider_id_for_submit(
            self.name.value.as_str(),
            self.id.value.as_str(),
            existing_ids,
        ) else {
            return false;
        };

        if self.id.is_blank() {
            self.id.set(generated_id);
        }

        true
    }

    pub fn fields(&self) -> Vec<ProviderAddField> {
        let mut fields = vec![
            ProviderAddField::Name,
            ProviderAddField::WebsiteUrl,
            ProviderAddField::Notes,
        ];

        if matches!(self.app_type, AppType::OpenClaw) {
            fields.insert(0, ProviderAddField::Id);
        }

        match self.app_type {
            AppType::Claude => {
                if !self.is_claude_official_provider() {
                    fields.push(ProviderAddField::ClaudeBaseUrl);
                    fields.push(ProviderAddField::ClaudeApiFormat);
                    fields.push(ProviderAddField::ClaudeApiKey);
                    fields.push(ProviderAddField::ClaudeModelConfig);
                }
            }
            AppType::Codex => {
                if !self.is_codex_official_provider() {
                    fields.push(ProviderAddField::CodexBaseUrl);
                    fields.push(ProviderAddField::CodexModel);
                    fields.push(ProviderAddField::CodexApiKey);
                }
            }
            AppType::Gemini => {
                fields.push(ProviderAddField::GeminiAuthType);
                if self.gemini_auth_type == GeminiAuthType::ApiKey {
                    fields.push(ProviderAddField::GeminiApiKey);
                    fields.push(ProviderAddField::GeminiBaseUrl);
                    fields.push(ProviderAddField::GeminiModel);
                }
            }
            AppType::OpenCode => {
                fields.push(ProviderAddField::OpenCodeNpmPackage);
                fields.push(ProviderAddField::OpenCodeApiKey);
                fields.push(ProviderAddField::OpenCodeBaseUrl);
                fields.push(ProviderAddField::OpenCodeModelId);
                fields.push(ProviderAddField::OpenCodeModelName);
                fields.push(ProviderAddField::OpenCodeModelContextLimit);
                fields.push(ProviderAddField::OpenCodeModelOutputLimit);
            }
            AppType::OpenClaw => {
                fields.push(ProviderAddField::OpenClawApiProtocol);
                fields.push(ProviderAddField::OpenCodeApiKey);
                fields.push(ProviderAddField::OpenCodeBaseUrl);
                fields.push(ProviderAddField::OpenClawUserAgent);
                fields.push(ProviderAddField::OpenClawModels);
            }
        }

        if !matches!(self.app_type, AppType::OpenClaw) {
            fields.push(ProviderAddField::CommonConfigDivider);
            fields.push(ProviderAddField::CommonSnippet);
            fields.push(ProviderAddField::IncludeCommonConfig);
        }
        fields
    }

    pub fn input(&self, field: ProviderAddField) -> Option<&TextInput> {
        match field {
            ProviderAddField::Id => Some(&self.id),
            ProviderAddField::Name => Some(&self.name),
            ProviderAddField::WebsiteUrl => Some(&self.website_url),
            ProviderAddField::Notes => Some(&self.notes),
            ProviderAddField::ClaudeBaseUrl => Some(&self.claude_base_url),
            ProviderAddField::ClaudeApiKey => Some(&self.claude_api_key),
            ProviderAddField::CodexBaseUrl => Some(&self.codex_base_url),
            ProviderAddField::CodexModel => Some(&self.codex_model),
            ProviderAddField::CodexEnvKey => Some(&self.codex_env_key),
            ProviderAddField::CodexApiKey => Some(&self.codex_api_key),
            ProviderAddField::GeminiApiKey => Some(&self.gemini_api_key),
            ProviderAddField::GeminiBaseUrl => Some(&self.gemini_base_url),
            ProviderAddField::GeminiModel => Some(&self.gemini_model),
            ProviderAddField::OpenCodeNpmPackage => Some(&self.opencode_npm_package),
            ProviderAddField::OpenCodeApiKey => Some(&self.opencode_api_key),
            ProviderAddField::OpenCodeBaseUrl => Some(&self.opencode_base_url),
            ProviderAddField::OpenCodeModelId => Some(&self.opencode_model_id),
            ProviderAddField::OpenCodeModelName => Some(&self.opencode_model_name),
            ProviderAddField::OpenCodeModelContextLimit => Some(&self.opencode_model_context_limit),
            ProviderAddField::OpenCodeModelOutputLimit => Some(&self.opencode_model_output_limit),
            ProviderAddField::CodexWireApi
            | ProviderAddField::CodexRequiresOpenaiAuth
            | ProviderAddField::ClaudeApiFormat
            | ProviderAddField::ClaudeModelConfig
            | ProviderAddField::GeminiAuthType
            | ProviderAddField::OpenClawApiProtocol
            | ProviderAddField::OpenClawUserAgent
            | ProviderAddField::OpenClawModels
            | ProviderAddField::CommonConfigDivider
            | ProviderAddField::CommonSnippet
            | ProviderAddField::IncludeCommonConfig => None,
        }
    }

    pub fn input_mut(&mut self, field: ProviderAddField) -> Option<&mut TextInput> {
        match field {
            ProviderAddField::Id => Some(&mut self.id),
            ProviderAddField::Name => Some(&mut self.name),
            ProviderAddField::WebsiteUrl => Some(&mut self.website_url),
            ProviderAddField::Notes => Some(&mut self.notes),
            ProviderAddField::ClaudeBaseUrl => Some(&mut self.claude_base_url),
            ProviderAddField::ClaudeApiKey => Some(&mut self.claude_api_key),
            ProviderAddField::CodexBaseUrl => Some(&mut self.codex_base_url),
            ProviderAddField::CodexModel => Some(&mut self.codex_model),
            ProviderAddField::CodexEnvKey => Some(&mut self.codex_env_key),
            ProviderAddField::CodexApiKey => Some(&mut self.codex_api_key),
            ProviderAddField::GeminiApiKey => Some(&mut self.gemini_api_key),
            ProviderAddField::GeminiBaseUrl => Some(&mut self.gemini_base_url),
            ProviderAddField::GeminiModel => Some(&mut self.gemini_model),
            ProviderAddField::OpenCodeNpmPackage => Some(&mut self.opencode_npm_package),
            ProviderAddField::OpenCodeApiKey => Some(&mut self.opencode_api_key),
            ProviderAddField::OpenCodeBaseUrl => Some(&mut self.opencode_base_url),
            ProviderAddField::OpenCodeModelId => Some(&mut self.opencode_model_id),
            ProviderAddField::OpenCodeModelName => Some(&mut self.opencode_model_name),
            ProviderAddField::OpenCodeModelContextLimit => {
                Some(&mut self.opencode_model_context_limit)
            }
            ProviderAddField::OpenCodeModelOutputLimit => {
                Some(&mut self.opencode_model_output_limit)
            }
            ProviderAddField::CodexWireApi
            | ProviderAddField::CodexRequiresOpenaiAuth
            | ProviderAddField::ClaudeApiFormat
            | ProviderAddField::ClaudeModelConfig
            | ProviderAddField::GeminiAuthType
            | ProviderAddField::OpenClawApiProtocol
            | ProviderAddField::OpenClawUserAgent
            | ProviderAddField::OpenClawModels
            | ProviderAddField::CommonConfigDivider
            | ProviderAddField::CommonSnippet
            | ProviderAddField::IncludeCommonConfig => None,
        }
    }

    pub fn claude_model_input(&self, index: usize) -> Option<&TextInput> {
        match index {
            0 => Some(&self.claude_model),
            1 => Some(&self.claude_reasoning_model),
            2 => Some(&self.claude_haiku_model),
            3 => Some(&self.claude_sonnet_model),
            4 => Some(&self.claude_opus_model),
            _ => None,
        }
    }

    pub fn claude_model_input_mut(&mut self, index: usize) -> Option<&mut TextInput> {
        match index {
            0 => Some(&mut self.claude_model),
            1 => Some(&mut self.claude_reasoning_model),
            2 => Some(&mut self.claude_haiku_model),
            3 => Some(&mut self.claude_sonnet_model),
            4 => Some(&mut self.claude_opus_model),
            _ => None,
        }
    }

    pub fn claude_model_configured_count(&self) -> usize {
        [
            &self.claude_model,
            &self.claude_reasoning_model,
            &self.claude_haiku_model,
            &self.claude_sonnet_model,
            &self.claude_opus_model,
        ]
        .into_iter()
        .filter(|input| !input.is_blank())
        .count()
    }

    pub fn mark_claude_model_config_touched(&mut self) {
        self.claude_model_config_touched = true;
    }

    pub fn is_claude_official_provider(&self) -> bool {
        if !matches!(self.app_type, AppType::Claude) {
            return false;
        }

        self.extra
            .get("category")
            .and_then(|value| value.as_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("official"))
    }

    pub fn is_codex_official_provider(&self) -> bool {
        if !matches!(self.app_type, AppType::Codex) {
            return false;
        }

        let meta_flag = self
            .extra
            .get("meta")
            .and_then(|meta| meta.get("codexOfficial"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false);

        let category_flag = self
            .extra
            .get("category")
            .and_then(|value| value.as_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("official"));

        let website_flag = self
            .website_url
            .value
            .trim()
            .eq_ignore_ascii_case("https://chatgpt.com/codex");

        let name_flag = self
            .name
            .value
            .trim()
            .eq_ignore_ascii_case("OpenAI Official");

        meta_flag || category_flag || website_flag || name_flag
    }

    pub fn apply_provider_json_to_fields(&mut self, provider: &Provider) {
        let previous_mode = self.mode.clone();
        let previous_focus = self.focus;
        let previous_template_idx = self.template_idx;
        let previous_field_idx = self.field_idx;
        let previous_json_scroll = self.json_scroll;
        let previous_codex_preview_section = self.codex_preview_section;
        let previous_codex_auth_scroll = self.codex_auth_scroll;
        let previous_codex_config_scroll = self.codex_config_scroll;
        let previous_include_common_config = self.include_common_config;
        let previous_include_common_config_touched = self.include_common_config_touched;
        let previous_extra = self.extra.clone();
        let previous_initial_snapshot = self.initial_snapshot.clone();

        let mut next = Self::from_provider(self.app_type.clone(), provider);
        let overlay = serde_json::to_value(provider).unwrap_or_else(|_| json!({}));
        let mut merged_extra = previous_extra;
        merge_json_values(&mut merged_extra, &overlay);
        next.extra = merged_extra;

        if provider
            .meta
            .as_ref()
            .and_then(|meta| meta.apply_common_config)
            .is_none()
        {
            next.include_common_config = previous_include_common_config;
            next.include_common_config_touched = previous_include_common_config_touched;
        } else {
            next.include_common_config_touched = true;
        }

        next.mode = previous_mode.clone();
        next.focus = previous_focus;
        next.template_idx = previous_template_idx;
        next.json_scroll = previous_json_scroll;
        next.codex_preview_section = previous_codex_preview_section;
        next.codex_auth_scroll = previous_codex_auth_scroll;
        next.codex_config_scroll = previous_codex_config_scroll;
        next.editing = false;
        let fields_len = next.fields().len();
        next.field_idx = if fields_len == 0 {
            0
        } else {
            previous_field_idx.min(fields_len - 1)
        };

        if let FormMode::Edit { id } = previous_mode {
            next.id.set(id);
            next.id_is_manual = true;
        }
        next.initial_snapshot = previous_initial_snapshot;

        *self = next;
    }

    pub fn apply_provider_json_value_to_fields(
        &mut self,
        mut provider_value: Value,
    ) -> Result<(), String> {
        let previous_mode = self.mode.clone();
        let previous_focus = self.focus;
        let previous_template_idx = self.template_idx;
        let previous_field_idx = self.field_idx;
        let previous_json_scroll = self.json_scroll;
        let previous_codex_preview_section = self.codex_preview_section;
        let previous_codex_auth_scroll = self.codex_auth_scroll;
        let previous_codex_config_scroll = self.codex_config_scroll;
        let previous_include_common_config = self.include_common_config;
        let previous_include_common_config_touched = self.include_common_config_touched;
        let previous_initial_snapshot = self.initial_snapshot.clone();

        let current_value = self.to_provider_json_value();
        if let (Some(current_obj), Some(edited_obj)) =
            (current_value.as_object(), provider_value.as_object_mut())
        {
            for (key, value) in current_obj {
                if should_hide_provider_field(key) && !edited_obj.contains_key(key) {
                    edited_obj.insert(key.clone(), value.clone());
                }
            }
        }

        let provider: Provider = serde_json::from_value(provider_value.clone())
            .map_err(|e| crate::cli::i18n::texts::tui_toast_invalid_json(&e.to_string()))?;

        let mut next = Self::from_provider(self.app_type.clone(), &provider);
        next.extra = provider_value;

        if provider
            .meta
            .as_ref()
            .and_then(|meta| meta.apply_common_config)
            .is_none()
        {
            next.include_common_config = previous_include_common_config;
            next.include_common_config_touched = previous_include_common_config_touched;
        } else {
            next.include_common_config_touched = true;
        }

        next.mode = previous_mode.clone();
        next.focus = previous_focus;
        next.template_idx = previous_template_idx;
        next.json_scroll = previous_json_scroll;
        next.codex_preview_section = previous_codex_preview_section;
        next.codex_auth_scroll = previous_codex_auth_scroll;
        next.codex_config_scroll = previous_codex_config_scroll;
        next.editing = false;

        let fields_len = next.fields().len();
        next.field_idx = if fields_len == 0 {
            0
        } else {
            previous_field_idx.min(fields_len - 1)
        };

        if let FormMode::Edit { id } = previous_mode {
            next.id.set(id);
            next.id_is_manual = true;
        }
        next.initial_snapshot = previous_initial_snapshot;

        *self = next;
        Ok(())
    }

    pub fn toggle_include_common_config(&mut self, common_snippet: &str) -> Result<(), String> {
        let next_enabled = !self.include_common_config;
        if self.include_common_config && !next_enabled {
            let mut provider_value = self.to_provider_json_value();
            if let Some(settings_value) = provider_value
                .as_object_mut()
                .and_then(|obj| obj.get_mut("settingsConfig"))
            {
                strip_common_config_from_settings(&self.app_type, settings_value, common_snippet)?;
            }

            if let Ok(provider) = serde_json::from_value::<Provider>(provider_value) {
                let stripped_settings = provider.settings_config.clone();
                self.apply_provider_json_to_fields(&provider);
                if let Some(extra_obj) = self.extra.as_object_mut() {
                    extra_obj.insert("settingsConfig".to_string(), stripped_settings);
                }
            }
        }
        self.include_common_config = next_enabled;
        self.include_common_config_touched = true;
        Ok(())
    }

    pub(super) fn opencode_primary_model_id(&self) -> Option<String> {
        let model_id = self.opencode_model_id.value.trim();
        if !model_id.is_empty() {
            return Some(model_id.to_string());
        }

        let model_name = self.opencode_model_name.value.trim();
        if !model_name.is_empty() {
            return Some(model_name.to_string());
        }

        None
    }

    pub(super) fn openclaw_primary_model_id(&self) -> Option<String> {
        let model_id = self.opencode_model_id.value.trim();
        if model_id.is_empty() {
            None
        } else {
            Some(model_id.to_string())
        }
    }

    pub(crate) fn openclaw_models_summary(&self) -> String {
        let total = self.openclaw_models.len();
        texts::tui_openclaw_models_summary(total)
    }

    pub(crate) fn openclaw_models_editor_text(&self) -> String {
        serde_json::to_string_pretty(&Value::Array(self.openclaw_models.clone()))
            .unwrap_or_else(|_| "[]".to_string())
    }

    pub fn apply_openclaw_models_value(&mut self, models_value: Value) -> Result<(), String> {
        if !matches!(self.app_type, AppType::OpenClaw) {
            return Ok(());
        }
        if !models_value.is_array() {
            return Err(texts::tui_toast_json_must_be_array().to_string());
        }

        let mut provider_value = self.to_provider_json_value();
        let settings_value = provider_value
            .as_object_mut()
            .and_then(|obj| obj.get_mut("settingsConfig"))
            .ok_or_else(|| texts::tui_toast_json_must_be_object().to_string())?;
        let settings_obj = settings_value
            .as_object_mut()
            .ok_or_else(|| texts::tui_toast_json_must_be_object().to_string())?;
        settings_obj.insert("models".to_string(), models_value);
        self.apply_provider_json_value_to_fields(provider_value)
    }
}

pub(crate) fn resolve_provider_id_for_submit(
    name: &str,
    id: &str,
    existing_ids: &[String],
) -> Option<String> {
    if name.trim().is_empty() {
        return None;
    }

    if !id.trim().is_empty() {
        return Some(id.to_string());
    }

    let generated_id =
        crate::cli::commands::provider_input::generate_provider_id(name.trim(), existing_ids);
    (!generated_id.trim().is_empty()).then_some(generated_id)
}
