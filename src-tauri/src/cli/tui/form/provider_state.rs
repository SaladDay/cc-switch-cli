use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::provider::{ClaudeApiKeyField, Provider};
use crate::services::ProviderService;
use serde_json::{json, Value};

use super::provider_json::{
    merge_json_values, should_hide_provider_field, strip_common_config_from_settings,
};
use super::provider_state_loading::populate_form_from_provider;
use super::{
    ClaudeApiFormat, CodexPreviewSection, CodexWireApi, FormFocus, FormMode, GeminiAuthType,
    HermesModelField, ProviderAddField, ProviderAddFormState, ProviderFormPage, TextInput,
    UsageQueryTemplate, HERMES_API_MODES, HERMES_DEFAULT_API_MODE, OPENCLAW_DEFAULT_API_PROTOCOL,
};

fn provider_copy_id(original_id: &str, existing_ids: &[String]) -> String {
    let base_id = format!("{}-copy", original_id.trim());
    if !existing_ids.iter().any(|id| id == &base_id) {
        return base_id;
    }

    let mut counter = 2;
    loop {
        let candidate = format!("{base_id}-{counter}");
        if !existing_ids.iter().any(|id| id == &candidate) {
            return candidate;
        }
        counter += 1;
    }
}

impl ProviderAddFormState {
    pub fn new(app_type: AppType) -> Self {
        Self::new_with_common_snippet(app_type, "")
    }

    pub fn new_with_common_snippet(app_type: AppType, common_snippet: &str) -> Self {
        let include_common_config =
            Self::snippet_has_effective_common_config(&app_type, common_snippet);
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
            copy_source_id: None,
            focus: FormFocus::Templates,
            page: ProviderFormPage::Main,
            template_idx: 0,
            field_idx: 0,
            editing: false,
            usage_query_touched: false,
            usage_query_field_idx: 0,
            usage_query_editing: false,
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
            claude_api_key_field: ClaudeApiKeyField::AuthToken,
            claude_base_url: TextInput::new(""),
            claude_api_format: ClaudeApiFormat::Anthropic,
            claude_model: TextInput::new(""),
            claude_reasoning_model: TextInput::new(""),
            claude_haiku_model: TextInput::new(""),
            claude_sonnet_model: TextInput::new(""),
            claude_opus_model: TextInput::new(""),
            claude_hide_attribution: false,
            claude_hide_attribution_touched: false,
            codex_oauth_account_id: None,
            codex_fast_mode: false,
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
            usage_query_enabled: false,
            usage_query_template: UsageQueryTemplate::General,
            usage_query_api_key: TextInput::new(""),
            usage_query_base_url: TextInput::new(""),
            usage_query_access_token: TextInput::new(""),
            usage_query_user_id: TextInput::new(""),
            usage_query_timeout: TextInput::new("10"),
            usage_query_auto_interval: TextInput::new("5"),
            usage_query_code: Self::USAGE_QUERY_GENERAL_PRESET.to_string(),
            opencode_npm_package: TextInput::new(openclaw_api_default),
            opencode_api_key: TextInput::new(""),
            opencode_base_url: TextInput::new(""),
            opencode_model_id: TextInput::new(""),
            opencode_model_name: TextInput::new(""),
            opencode_model_context_limit: TextInput::new(""),
            opencode_model_output_limit: TextInput::new(""),
            opencode_model_original_id: None,
            hermes_api_mode: HERMES_DEFAULT_API_MODE.to_string(),
            hermes_api_key: TextInput::new(""),
            hermes_base_url: TextInput::new(""),
            hermes_models: Vec::new(),
            hermes_models_field_idx: 0,
            hermes_models_editing: false,
            hermes_model_input: TextInput::new(""),
            hermes_rate_limit_delay: TextInput::new(""),
            initial_snapshot: Value::Null,
        };
        form.capture_initial_snapshot();
        form
    }

    pub fn from_provider(app_type: AppType, provider: &Provider) -> Self {
        Self::from_provider_with_common_snippet(app_type, provider, "")
    }

    pub fn from_provider_with_common_snippet(
        app_type: AppType,
        provider: &Provider,
        common_snippet: &str,
    ) -> Self {
        let mut form = Self::new_with_common_snippet(app_type.clone(), common_snippet);
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
        let explicit_common_config = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.apply_common_config);
        form.include_common_config = explicit_common_config.unwrap_or_else(|| {
            Self::provider_settings_contain_common_config(&app_type, provider, common_snippet)
        });
        form.include_common_config_touched = explicit_common_config.is_some();

        if !Self::supports_common_config(&app_type) {
            form.include_common_config = false;
            form.include_common_config_touched = false;
        }

        populate_form_from_provider(&mut form, &app_type, provider);
        form.capture_initial_snapshot();

        form
    }

    pub fn copy_from_provider_with_common_snippet(
        app_type: AppType,
        provider: &Provider,
        common_snippet: &str,
        existing_ids: &[String],
    ) -> Self {
        let mut form = Self::from_provider_with_common_snippet(app_type, provider, common_snippet);
        form.mode = FormMode::Add;
        form.copy_source_id = Some(provider.id.clone());
        form.id_is_manual = false;
        form.name.set(format!("{} copy", provider.name.trim()));
        // Remove fields that should be unique or not copied over
        if let Some(extra) = form.extra.as_object_mut() {
            for key in ["id", "createdAt", "inFailoverQueue"] {
                extra.remove(key);
            }
        }
        form.id.set(provider_copy_id(&provider.id, existing_ids));
        form
    }

    pub fn supports_common_config(app_type: &AppType) -> bool {
        matches!(app_type, AppType::Claude | AppType::Codex | AppType::Gemini)
    }

    pub fn snippet_has_effective_common_config(app_type: &AppType, common_snippet: &str) -> bool {
        if !Self::supports_common_config(app_type) {
            return false;
        }

        let snippet = common_snippet.trim();
        if snippet.is_empty() {
            return false;
        }

        match app_type {
            AppType::Codex => snippet
                .parse::<toml_edit::DocumentMut>()
                .ok()
                .is_some_and(|doc| doc.as_table().iter().next().is_some()),
            AppType::Claude | AppType::Gemini => serde_json::from_str::<Value>(snippet)
                .ok()
                .and_then(|value| value.as_object().cloned())
                .is_some_and(|obj| !obj.is_empty()),
            AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => false,
        }
    }

    pub fn provider_settings_contain_common_config(
        app_type: &AppType,
        provider: &Provider,
        common_snippet: &str,
    ) -> bool {
        if !Self::supports_common_config(app_type) {
            return false;
        }

        ProviderService::settings_contain_common_config_for_preview(
            app_type,
            &provider.settings_config,
            common_snippet,
        )
    }

    fn capture_initial_snapshot(&mut self) {
        self.initial_snapshot = self.to_provider_json_value();
    }

    pub fn has_unsaved_changes(&self) -> bool {
        self.to_provider_json_value() != self.initial_snapshot
    }

    pub fn is_id_editable(&self) -> bool {
        !self.mode.is_edit() && self.copy_source_id.is_none()
    }

    pub fn ensure_generated_id(&mut self, existing_ids: &[String]) -> bool {
        let Some(generated_id) = resolve_provider_id_for_submit(
            &self.app_type,
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

        if matches!(self.app_type, AppType::Hermes | AppType::OpenClaw)
            && self.copy_source_id.is_none()
        {
            fields.insert(0, ProviderAddField::Id);
        }

        match self.app_type {
            AppType::Claude => {
                if self.is_claude_codex_oauth_provider() {
                    fields.push(ProviderAddField::CodexOAuthAccount);
                    fields.push(ProviderAddField::CodexFastMode);
                    fields.push(ProviderAddField::ClaudeModelConfig);
                } else if !self.is_claude_official_provider() {
                    fields.push(ProviderAddField::ClaudeBaseUrl);
                    fields.push(ProviderAddField::ClaudeApiFormat);
                    fields.push(ProviderAddField::ClaudeApiKey);
                    fields.push(ProviderAddField::ClaudeModelConfig);
                }
                fields.push(ProviderAddField::ClaudeHideAttribution);
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
            AppType::Hermes => {
                fields.push(ProviderAddField::HermesApiMode);
                fields.push(ProviderAddField::HermesBaseUrl);
                fields.push(ProviderAddField::HermesApiKey);
                fields.push(ProviderAddField::HermesModels);
                fields.push(ProviderAddField::HermesAdvancedDivider);
                fields.push(ProviderAddField::HermesRateLimitDelay);
            }
            AppType::OpenClaw => {
                fields.push(ProviderAddField::OpenClawApiProtocol);
                fields.push(ProviderAddField::OpenCodeApiKey);
                fields.push(ProviderAddField::OpenCodeBaseUrl);
                fields.push(ProviderAddField::OpenClawUserAgent);
                fields.push(ProviderAddField::OpenClawModels);
            }
        }

        if Self::supports_common_config(&self.app_type) {
            fields.push(ProviderAddField::CommonConfigDivider);
            fields.push(ProviderAddField::CommonSnippet);
            fields.push(ProviderAddField::IncludeCommonConfig);
        }
        fields.push(ProviderAddField::UsageQueryDivider);
        fields.push(ProviderAddField::UsageQuery);
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
            ProviderAddField::HermesApiKey => Some(&self.hermes_api_key),
            ProviderAddField::HermesBaseUrl => Some(&self.hermes_base_url),
            ProviderAddField::HermesRateLimitDelay => Some(&self.hermes_rate_limit_delay),
            ProviderAddField::CodexOAuthAccount
            | ProviderAddField::CodexFastMode
            | ProviderAddField::CodexWireApi
            | ProviderAddField::CodexRequiresOpenaiAuth
            | ProviderAddField::ClaudeApiFormat
            | ProviderAddField::ClaudeModelConfig
            | ProviderAddField::ClaudeHideAttribution
            | ProviderAddField::GeminiAuthType
            | ProviderAddField::OpenClawApiProtocol
            | ProviderAddField::OpenClawUserAgent
            | ProviderAddField::OpenClawModels
            | ProviderAddField::HermesApiMode
            | ProviderAddField::HermesModels
            | ProviderAddField::HermesAdvancedDivider
            | ProviderAddField::CommonConfigDivider
            | ProviderAddField::CommonSnippet
            | ProviderAddField::IncludeCommonConfig
            | ProviderAddField::UsageQueryDivider
            | ProviderAddField::UsageQuery => None,
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
            ProviderAddField::HermesApiKey => Some(&mut self.hermes_api_key),
            ProviderAddField::HermesBaseUrl => Some(&mut self.hermes_base_url),
            ProviderAddField::HermesRateLimitDelay => Some(&mut self.hermes_rate_limit_delay),
            ProviderAddField::CodexOAuthAccount
            | ProviderAddField::CodexFastMode
            | ProviderAddField::CodexWireApi
            | ProviderAddField::CodexRequiresOpenaiAuth
            | ProviderAddField::ClaudeApiFormat
            | ProviderAddField::ClaudeModelConfig
            | ProviderAddField::ClaudeHideAttribution
            | ProviderAddField::GeminiAuthType
            | ProviderAddField::OpenClawApiProtocol
            | ProviderAddField::OpenClawUserAgent
            | ProviderAddField::OpenClawModels
            | ProviderAddField::HermesApiMode
            | ProviderAddField::HermesModels
            | ProviderAddField::HermesAdvancedDivider
            | ProviderAddField::CommonConfigDivider
            | ProviderAddField::CommonSnippet
            | ProviderAddField::IncludeCommonConfig
            | ProviderAddField::UsageQueryDivider
            | ProviderAddField::UsageQuery => None,
        }
    }

    pub fn open_hermes_models_picker(&mut self) {
        if !matches!(self.app_type, AppType::Hermes) {
            return;
        }
        self.focus = FormFocus::Fields;
        self.editing = false;
        self.hermes_models_editing = false;
        let len = self.hermes_model_fields().len();
        self.hermes_models_field_idx = self.hermes_models_field_idx.min(len.saturating_sub(1));
        self.sync_hermes_model_input_from_selection();
    }

    pub fn close_hermes_models_picker(&mut self) {
        self.hermes_models_editing = false;
        self.hermes_model_input.set("");
    }

    pub fn hermes_model_fields(&self) -> Vec<HermesModelField> {
        let mut fields = Vec::with_capacity(self.hermes_models.len().saturating_mul(3));
        for index in 0..self.hermes_models.len() {
            fields.push(HermesModelField::Id(index));
            fields.push(HermesModelField::Name(index));
            fields.push(HermesModelField::ContextLength(index));
        }
        fields
    }

    pub fn selected_hermes_model_field(&self) -> Option<HermesModelField> {
        let fields = self.hermes_model_fields();
        fields
            .get(
                self.hermes_models_field_idx
                    .min(fields.len().saturating_sub(1)),
            )
            .copied()
    }

    pub fn add_empty_hermes_model(&mut self) {
        if !matches!(self.app_type, AppType::Hermes) {
            return;
        }
        self.hermes_models.push(json!({ "id": "", "name": "" }));
        self.hermes_models_field_idx = self
            .hermes_model_fields()
            .iter()
            .position(|field| matches!(field, HermesModelField::Id(index) if *index == self.hermes_models.len().saturating_sub(1)))
            .unwrap_or(self.hermes_models_field_idx);
        self.sync_hermes_model_input_from_selection();
    }

    pub fn remove_hermes_model(&mut self, index: usize) {
        if index >= self.hermes_models.len() {
            return;
        }
        self.hermes_models.remove(index);
        let fields_len = self.hermes_model_fields().len();
        self.hermes_models_field_idx = self
            .hermes_models_field_idx
            .min(fields_len.saturating_sub(1));
        self.hermes_models_editing = false;
        self.sync_hermes_model_input_from_selection();
    }

    pub fn remove_selected_hermes_model(&mut self) -> bool {
        let Some(field) = self.selected_hermes_model_field() else {
            return false;
        };
        let index = match field {
            HermesModelField::Id(index)
            | HermesModelField::Name(index)
            | HermesModelField::ContextLength(index) => index,
        };
        if index >= self.hermes_models.len() {
            return false;
        }
        self.remove_hermes_model(index);
        true
    }

    pub fn hermes_model_field_input(&self, field: HermesModelField) -> Option<TextInput> {
        let (index, key) = match field {
            HermesModelField::Id(index) => (index, "id"),
            HermesModelField::Name(index) => (index, "name"),
            HermesModelField::ContextLength(index) => (index, "context_length"),
        };
        let model = self.hermes_models.get(index)?;
        let value = model
            .get(key)
            .and_then(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| value.as_i64().map(|number| number.to_string()))
                    .or_else(|| value.as_u64().map(|number| number.to_string()))
            })
            .unwrap_or_default();
        Some(TextInput::new(value))
    }

    pub fn sync_hermes_model_input_from_selection(&mut self) {
        let input = self
            .selected_hermes_model_field()
            .and_then(|field| self.hermes_model_field_input(field))
            .unwrap_or_else(|| TextInput::new(""));
        self.hermes_model_input = input;
    }

    pub fn set_hermes_model_field_text(&mut self, field: HermesModelField, value: &str) {
        let (index, key) = match field {
            HermesModelField::Id(index) => (index, "id"),
            HermesModelField::Name(index) => (index, "name"),
            HermesModelField::ContextLength(index) => (index, "context_length"),
        };
        let Some(model) = self.hermes_models.get_mut(index) else {
            return;
        };
        if !model.is_object() {
            *model = json!({});
        }
        let Some(obj) = model.as_object_mut() else {
            return;
        };
        let trimmed = value.trim();
        if key == "context_length" {
            if trimmed.is_empty() {
                obj.remove(key);
            } else if let Ok(number) = trimmed.parse::<u64>() {
                obj.insert(key.to_string(), json!(number));
            } else {
                obj.insert(key.to_string(), json!(trimmed));
            }
        } else {
            obj.insert(key.to_string(), json!(value));
        }
    }

    pub(crate) fn set_selected_hermes_model_id_from_picker(&mut self, model_id: &str) -> bool {
        if !matches!(self.app_type, AppType::Hermes) {
            return false;
        }
        let model_id = model_id.trim();
        if model_id.is_empty() {
            return false;
        }

        let selected = self.selected_hermes_model_field();
        let target_index = match selected {
            Some(HermesModelField::Id(index)) if index < self.hermes_models.len() => index,
            Some(HermesModelField::Name(index) | HermesModelField::ContextLength(index))
                if index < self.hermes_models.len() =>
            {
                index
            }
            _ => {
                self.add_empty_hermes_model();
                self.hermes_models.len().saturating_sub(1)
            }
        };

        self.set_hermes_model_field_text(HermesModelField::Id(target_index), model_id);
        self.hermes_models_field_idx = self
            .hermes_model_fields()
            .iter()
            .position(|field| *field == HermesModelField::Id(target_index))
            .unwrap_or(self.hermes_models_field_idx);
        self.sync_hermes_model_input_from_selection();
        true
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

    pub fn toggle_claude_hide_attribution(&mut self) {
        self.claude_hide_attribution = !self.claude_hide_attribution;
        self.claude_hide_attribution_touched = true;
    }

    pub fn toggle_codex_fast_mode(&mut self) {
        if self.is_claude_codex_oauth_provider() {
            self.codex_fast_mode = !self.codex_fast_mode;
        }
    }

    pub fn set_codex_oauth_account_id(&mut self, account_id: Option<String>) {
        self.codex_oauth_account_id = account_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
    }

    pub fn codex_oauth_account_display(&self) -> String {
        self.codex_oauth_account_id
            .clone()
            .unwrap_or_else(|| texts::tui_managed_accounts_follow_default().to_string())
    }

    pub fn is_claude_codex_oauth_provider(&self) -> bool {
        if !matches!(self.app_type, AppType::Claude) {
            return false;
        }

        self.extra
            .get("meta")
            .and_then(|meta| meta.get("providerType"))
            .and_then(|value| value.as_str())
            .is_some_and(|value| value == "codex_oauth")
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
        let previous_page = self.page;
        let previous_copy_source_id = self.copy_source_id.clone();
        let previous_template_idx = self.template_idx;
        let previous_field_idx = self.field_idx;
        let previous_usage_query_field_idx = self.usage_query_field_idx;
        let previous_hermes_models_field_idx = self.hermes_models_field_idx;
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
        next.copy_source_id = previous_copy_source_id;
        next.focus = previous_focus;
        next.page = previous_page;
        next.template_idx = previous_template_idx;
        next.json_scroll = previous_json_scroll;
        next.codex_preview_section = previous_codex_preview_section;
        next.codex_auth_scroll = previous_codex_auth_scroll;
        next.codex_config_scroll = previous_codex_config_scroll;
        next.editing = false;
        next.usage_query_editing = false;
        next.hermes_models_editing = false;
        let fields_len = next.fields().len();
        next.field_idx = if fields_len == 0 {
            0
        } else {
            previous_field_idx.min(fields_len - 1)
        };
        let usage_fields_len = next.usage_query_table_fields().len();
        next.usage_query_field_idx = if usage_fields_len == 0 {
            0
        } else {
            previous_usage_query_field_idx.min(usage_fields_len - 1)
        };
        let hermes_model_fields_len = next.hermes_model_fields().len();
        next.hermes_models_field_idx = if hermes_model_fields_len == 0 {
            0
        } else {
            previous_hermes_models_field_idx.min(hermes_model_fields_len - 1)
        };
        next.sync_hermes_model_input_from_selection();

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
        let previous_page = self.page;
        let previous_copy_source_id = self.copy_source_id.clone();
        let previous_template_idx = self.template_idx;
        let previous_field_idx = self.field_idx;
        let previous_usage_query_field_idx = self.usage_query_field_idx;
        let previous_hermes_models_field_idx = self.hermes_models_field_idx;
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
        next.copy_source_id = previous_copy_source_id;
        next.focus = previous_focus;
        next.page = previous_page;
        next.template_idx = previous_template_idx;
        next.json_scroll = previous_json_scroll;
        next.codex_preview_section = previous_codex_preview_section;
        next.codex_auth_scroll = previous_codex_auth_scroll;
        next.codex_config_scroll = previous_codex_config_scroll;
        next.editing = false;
        next.usage_query_editing = false;
        next.hermes_models_editing = false;

        let fields_len = next.fields().len();
        next.field_idx = if fields_len == 0 {
            0
        } else {
            previous_field_idx.min(fields_len - 1)
        };
        let usage_fields_len = next.usage_query_table_fields().len();
        next.usage_query_field_idx = if usage_fields_len == 0 {
            0
        } else {
            previous_usage_query_field_idx.min(usage_fields_len - 1)
        };
        let hermes_model_fields_len = next.hermes_model_fields().len();
        next.hermes_models_field_idx = if hermes_model_fields_len == 0 {
            0
        } else {
            previous_hermes_models_field_idx.min(hermes_model_fields_len - 1)
        };
        next.sync_hermes_model_input_from_selection();

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

    pub(crate) fn cycle_hermes_api_mode(&mut self) {
        let current = HERMES_API_MODES
            .iter()
            .position(|mode| *mode == self.hermes_api_mode.trim())
            .unwrap_or(0);
        self.hermes_api_mode = HERMES_API_MODES[(current + 1) % HERMES_API_MODES.len()].to_string();
    }

    pub(crate) fn hermes_api_mode_value(&self) -> &str {
        if HERMES_API_MODES
            .iter()
            .any(|mode| *mode == self.hermes_api_mode.trim())
        {
            self.hermes_api_mode.trim()
        } else {
            HERMES_DEFAULT_API_MODE
        }
    }

    pub(crate) fn hermes_models_summary(&self) -> String {
        texts::tui_hermes_models_summary(self.hermes_models.len())
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
    app_type: &AppType,
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

    let generated_id = crate::cli::commands::provider_input::generate_provider_id_for_app(
        app_type,
        name.trim(),
        existing_ids,
    );
    (!generated_id.trim().is_empty()).then_some(generated_id)
}
