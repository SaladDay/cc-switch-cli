use crate::app_config::AppType;
use crate::claude_model_config::split_claude_one_m_marker;
use crate::cli::i18n::texts;
use crate::provider::{ClaudeApiKeyField, CodexChatReasoningConfig, Provider};
use crate::provider_preset_models::{CODEX_DEFAULT_MODEL, GEMINI_DEFAULT_MODEL};
use crate::services::ProviderService;
use serde_json::{json, Value};

use super::super::text_edit::{
    passive_text_contains, passive_trimmed_eq_ignore_ascii_case, PASSIVE_TEXT_SCAN_MAX_BYTES,
};

use super::provider_json::{
    claude_settings_contain_common_config_for_form, codex_settings_contain_common_config_for_form,
    merge_json_values, normalize_gemini_common_config_for_form,
    sanitize_claude_common_config_for_form, should_hide_provider_field,
    strip_common_config_from_settings,
};
use super::provider_state_loading::populate_form_from_provider;
use super::{
    ClaudeApiFormat, ClaudeModelRole, CodexLocalRoutingField, CodexModelCatalogField,
    CodexModelCatalogRow, CodexPreviewSection, CodexWireApi, FormFocus, FormMode, GeminiAuthType,
    HermesModelField, InlineFieldError, LocalProxySettingsField, PromptCacheRoutingMode,
    ProviderAddField, ProviderAddFormState, ProviderFormPage, ProviderTextField, TextEditSession,
    TextInput, UsageQueryField, UsageQueryTemplate, HERMES_API_MODES, HERMES_DEFAULT_API_MODE,
    OPENCLAW_DEFAULT_API_PROTOCOL,
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
    pub const USAGE_QUERY_GENERAL_PRESET: &'static str = r#"({
  request: {
    url: "{{baseUrl}}/user/balance",
    method: "GET",
    headers: {
      "Authorization": "Bearer {{apiKey}}",
      "User-Agent": "cc-switch/1.0"
    }
  },
  extractor: function(response) {
    return {
      isValid: response.is_active || true,
      remaining: response.balance,
      unit: "USD"
    };
  }
})"#;

    pub const USAGE_QUERY_CUSTOM_PRESET: &'static str = r#"({
  request: {
    url: "",
    method: "GET",
    headers: {}
  },
  extractor: function(response) {
    return {
      remaining: 0,
      unit: "USD"
    };
  }
})"#;

    pub const USAGE_QUERY_NEWAPI_PRESET: &'static str = r#"({
  request: {
    url: "{{baseUrl}}/api/user/self",
    method: "GET",
    headers: {
      "Content-Type": "application/json",
      "Authorization": "Bearer {{accessToken}}",
      "User-Agent": "cc-switch/1.0",
      "New-Api-User": "{{userId}}"
    },
  },
  extractor: function (response) {
    if (response.success && response.data) {
      return {
        planName: response.data.group || "Default Plan",
        remaining: response.data.quota / 500000,
        used: response.data.used_quota / 500000,
        total: (response.data.quota + response.data.used_quota) / 500000,
        unit: "USD",
      };
    }
    return {
      isValid: false,
      invalidMessage: response.message || "Query failed"
    };
  },
})"#;

    pub fn new(app_type: AppType) -> Self {
        Self::new_with_common_snippet(app_type, "")
    }

    pub fn new_with_common_snippet(app_type: AppType, common_snippet: &str) -> Self {
        let include_common_config = if matches!(app_type, AppType::Claude) {
            // The upstream new-provider effect checks the raw object's key
            // count before its merge helper skips prototype-pollution keys.
            serde_json::from_str::<Value>(common_snippet.trim())
                .ok()
                .and_then(|value| value.as_object().cloned())
                .is_some_and(|object| !object.is_empty())
        } else {
            Self::snippet_has_effective_common_config(&app_type, common_snippet)
        };
        let is_codex = matches!(app_type, AppType::Codex);
        let is_gemini = matches!(app_type, AppType::Gemini);
        let openclaw_api_default = match app_type {
            AppType::OpenClaw => OPENCLAW_DEFAULT_API_PROTOCOL,
            _ => "@ai-sdk/openai-compatible",
        };

        let codex_defaults = if is_codex {
            ("", CODEX_DEFAULT_MODEL, CodexWireApi::Responses, true)
        } else {
            ("", "", CodexWireApi::Responses, true)
        };

        let mut form = Self {
            app_type,
            mode: FormMode::Add,
            copy_source_id: None,
            focus: FormFocus::Templates,
            page: ProviderFormPage::Main,
            template_idx: 0,
            field_idx: 0,
            text_edit: None,
            field_errors: Vec::new(),
            usage_query_touched: false,
            usage_query_field_idx: 0,
            usage_query_field_errors: Vec::new(),
            codex_local_routing_field_idx: 0,
            local_proxy_settings_field_idx: 0,
            codex_model_catalog_idx: 0,
            codex_model_catalog_field: CodexModelCatalogField::Model,
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
            claude_fallback_model_touched: false,
            claude_model_role_touched: [false; ClaudeModelRole::COUNT],
            claude_api_key: TextInput::new(""),
            claude_api_key_field: ClaudeApiKeyField::AuthToken,
            claude_base_url: TextInput::new(""),
            claude_api_format: if is_codex {
                ClaudeApiFormat::OpenAiResponses
            } else {
                ClaudeApiFormat::Anthropic
            },
            claude_model: TextInput::new(""),
            claude_haiku_model: TextInput::new(""),
            claude_sonnet_model: TextInput::new(""),
            claude_opus_model: TextInput::new(""),
            claude_fable_model: TextInput::new(""),
            claude_subagent_model: TextInput::new(""),
            claude_sonnet_one_m: false,
            claude_opus_one_m: false,
            claude_fable_one_m: false,
            claude_subagent_one_m: false,
            claude_hide_attribution: false,
            claude_hide_attribution_touched: false,
            claude_teammates: false,
            claude_teammates_touched: false,
            claude_tool_search: false,
            claude_tool_search_touched: false,
            claude_effort_max: false,
            claude_effort_max_touched: false,
            claude_disable_auto_upgrade: false,
            claude_disable_auto_upgrade_touched: false,
            is_full_url: false,
            claude_quick_config_idx: 0,
            codex_goal_mode: false,
            codex_goal_mode_touched: false,
            codex_remote_compaction: false,
            codex_remote_compaction_touched: false,
            codex_quick_config_idx: 0,
            codex_oauth_account_id: None,
            codex_fast_mode: false,
            codex_impersonate_claude_code: false,
            codex_max_output_tokens: TextInput::new(""),
            codex_base_url: TextInput::new(codex_defaults.0),
            codex_model: TextInput::new(codex_defaults.1),
            codex_wire_api: codex_defaults.2,
            codex_requires_openai_auth: codex_defaults.3,
            codex_env_key: TextInput::new("OPENAI_API_KEY"),
            codex_api_key: TextInput::new(""),
            codex_chat_reasoning: CodexChatReasoningConfig::default(),
            codex_prompt_cache_routing: PromptCacheRoutingMode::Auto,
            codex_model_catalog: Vec::new(),
            codex_local_routing_enabled: false,
            custom_user_agent: TextInput::new(""),
            local_proxy_header_overrides: Default::default(),
            local_proxy_body_override: None,
            gemini_auth_type: GeminiAuthType::ApiKey,
            gemini_api_key: TextInput::new(""),
            gemini_base_url: TextInput::new("https://generativelanguage.googleapis.com"),
            gemini_model: TextInput::new(if is_gemini { GEMINI_DEFAULT_MODEL } else { "" }),
            openclaw_user_agent: false,
            openclaw_models: Vec::new(),
            usage_query_enabled: false,
            usage_query_official_subscription: false,
            usage_query_template: UsageQueryTemplate::General,
            usage_query_api_key: TextInput::new(""),
            usage_query_base_url: TextInput::new(""),
            usage_query_access_token: TextInput::new(""),
            usage_query_user_id: TextInput::new(""),
            usage_query_timeout: TextInput::new("10"),
            usage_query_auto_interval: TextInput::new("5"),
            usage_query_code: Self::USAGE_QUERY_GENERAL_PRESET.to_string(),
            usage_query_coding_plan_provider: TextInput::new("kimi"),
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
        let _ = form.refresh_quick_config_from_common_snippet(common_snippet);
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
        let inferred_common_config =
            Self::provider_settings_contain_common_config(&app_type, provider, common_snippet);
        form.include_common_config = explicit_common_config.unwrap_or(inferred_common_config);
        form.include_common_config_touched = explicit_common_config.is_some();

        if !Self::supports_common_config(&app_type) {
            form.include_common_config = false;
            form.include_common_config_touched = false;
        }

        populate_form_from_provider(&mut form, &app_type, provider);
        let refresh_failed = form.include_common_config
            && !inferred_common_config
            && form
                .refresh_quick_config_from_common_snippet(common_snippet)
                .is_err();
        if refresh_failed
            || (form.include_common_config
                && matches!(app_type, AppType::Codex | AppType::Gemini)
                && !Self::snippet_has_effective_common_config(&app_type, common_snippet))
        {
            // All upstream hooks disable sharing when initialization cannot
            // apply/parse the stored snippet. Codex and Gemini additionally
            // disable it when a valid snippet has no effective entries;
            // Claude's apply path marks even an empty object as an internal
            // update and therefore preserves an explicit enabled flag.
            form.include_common_config = false;
        }
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
            AppType::Claude => sanitize_claude_common_config_for_form(snippet)
                .ok()
                .and_then(|value| value.as_object().cloned())
                .is_some_and(|obj| !obj.is_empty()),
            AppType::Gemini => normalize_gemini_common_config_for_form(snippet)
                .ok()
                .and_then(|value| value.as_object().cloned())
                .is_some_and(|env| !env.is_empty()),
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

        Self::settings_contain_common_config_for_form(
            app_type,
            &provider.settings_config,
            common_snippet,
        )
    }

    pub(crate) fn settings_contain_common_config_for_form(
        app_type: &AppType,
        settings: &Value,
        common_snippet: &str,
    ) -> bool {
        if !Self::supports_common_config(app_type) {
            return false;
        }
        if !Self::snippet_has_effective_common_config(app_type, common_snippet) {
            // Upstream UI membership checks treat `{}` and comment-only TOML
            // as not applied even though the backend's empty subset check is
            // vacuously true. Keep that distinction at the TUI boundary.
            return false;
        }

        match app_type {
            AppType::Claude => {
                claude_settings_contain_common_config_for_form(settings, common_snippet)
            }
            AppType::Codex => {
                codex_settings_contain_common_config_for_form(settings, common_snippet)
            }
            AppType::Gemini => {
                let Ok(normalized) = normalize_gemini_common_config_for_form(common_snippet) else {
                    return false;
                };
                if normalized.as_object().is_none_or(serde_json::Map::is_empty) {
                    return false;
                }
                let Ok(snippet) = serde_json::to_string(&normalized) else {
                    return false;
                };
                ProviderService::settings_contain_common_config_for_preview(
                    app_type, settings, &snippet,
                )
            }
            AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => false,
        }
    }

    fn validate_common_config_snippet_for_form(
        app_type: &AppType,
        common_snippet: &str,
    ) -> Result<(), String> {
        if matches!(app_type, AppType::Gemini) {
            normalize_gemini_common_config_for_form(common_snippet).map(|_| ())
        } else {
            ProviderService::validate_common_config_snippet_for_preview(app_type, common_snippet)
                .map_err(|error| error.to_string())
        }
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
                    fields.push(ProviderAddField::ClaudeAdvancedDivider);
                    fields.push(ProviderAddField::ClaudeModelConfig);
                    fields.push(ProviderAddField::ClaudeFallbackModel);
                    fields.push(ProviderAddField::LocalProxySettings);
                } else if !self.is_claude_official_provider() {
                    fields.push(ProviderAddField::ClaudeBaseUrl);
                    fields.push(ProviderAddField::ClaudeApiKey);
                    fields.push(ProviderAddField::ClaudeAdvancedDivider);
                    fields.push(ProviderAddField::ClaudeApiFormat);
                    fields.push(ProviderAddField::ClaudeModelConfig);
                    fields.push(ProviderAddField::ClaudeFallbackModel);
                    if self.supports_local_proxy_settings() {
                        fields.push(ProviderAddField::LocalProxySettings);
                    }
                }
            }
            AppType::Codex => {
                if !self.is_codex_official_provider() {
                    fields.push(ProviderAddField::CodexBaseUrl);
                    fields.push(ProviderAddField::CodexApiKey);
                    // No standalone model field (matches upstream): the model is
                    // configured via 模型映射 / the catalog, falling back to the
                    // default when empty.
                    fields.push(ProviderAddField::CodexAdvancedDivider);
                    // Upstream format is an independent picker; local routing /
                    // model mapping is decoupled from it.
                    fields.push(ProviderAddField::ClaudeApiFormat);
                    if matches!(self.claude_api_format, ClaudeApiFormat::Anthropic) {
                        fields.push(ProviderAddField::CodexAnthropicApiKeyField);
                        fields.push(ProviderAddField::CodexImpersonateClaudeCode);
                        fields.push(ProviderAddField::CodexMaxOutputTokens);
                    }
                    if self.codex_is_chat_format() {
                        fields.push(ProviderAddField::CodexPromptCacheRouting);
                    }
                    fields.push(ProviderAddField::CodexLocalRouting);
                    fields.push(ProviderAddField::LocalProxySettings);
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
        // The Claude/Codex quick toggles are collapsed into a single sub-page
        // entry ("快捷配置菜单") that sits directly below the common-config
        // controls. Goal mode applies to every Codex provider, so the Codex
        // entry is present for official providers too.
        match self.app_type {
            AppType::Claude => fields.push(ProviderAddField::ClaudeQuickConfig),
            AppType::Codex => fields.push(ProviderAddField::CodexQuickConfig),
            _ => {}
        }
        fields.push(ProviderAddField::UsageQueryDivider);
        fields.push(ProviderAddField::UsageQuery);
        fields
    }

    pub fn usage_query_fields(&self) -> Vec<UsageQueryField> {
        let mut fields = vec![UsageQueryField::Enabled];

        if !self.usage_query_enabled {
            return fields;
        }

        fields.push(UsageQueryField::Template);

        match self.usage_query_template {
            UsageQueryTemplate::General => {
                fields.extend([
                    UsageQueryField::ApiKey,
                    UsageQueryField::BaseUrl,
                    UsageQueryField::Timeout,
                    UsageQueryField::AutoInterval,
                    UsageQueryField::Script,
                ]);
            }
            UsageQueryTemplate::NewApi => {
                fields.extend([
                    UsageQueryField::BaseUrl,
                    UsageQueryField::AccessToken,
                    UsageQueryField::UserId,
                    UsageQueryField::Timeout,
                    UsageQueryField::AutoInterval,
                    UsageQueryField::Script,
                ]);
            }
            UsageQueryTemplate::Custom => {
                fields.extend([
                    UsageQueryField::Timeout,
                    UsageQueryField::AutoInterval,
                    UsageQueryField::Script,
                ]);
            }
            UsageQueryTemplate::GitHubCopilot => {
                fields.extend([UsageQueryField::Timeout, UsageQueryField::AutoInterval]);
            }
            UsageQueryTemplate::Balance => {
                fields.extend([
                    UsageQueryField::Timeout,
                    UsageQueryField::AutoInterval,
                    UsageQueryField::Script,
                ]);
            }
            UsageQueryTemplate::TokenPlan => {
                fields.extend([
                    UsageQueryField::CodingPlanProvider,
                    UsageQueryField::Timeout,
                    UsageQueryField::AutoInterval,
                ]);
            }
            UsageQueryTemplate::OfficialSubscription => {
                fields.extend([UsageQueryField::Timeout, UsageQueryField::AutoInterval]);
            }
        }

        fields
    }

    pub fn usage_query_table_fields(&self) -> Vec<UsageQueryField> {
        self.usage_query_fields()
            .into_iter()
            .filter(|field| *field != UsageQueryField::Script)
            .collect()
    }

    pub fn usage_query_script_validation_error(&self) -> Option<&'static str> {
        if !self.usage_query_enabled
            || matches!(
                self.usage_query_template,
                UsageQueryTemplate::GitHubCopilot
                    | UsageQueryTemplate::TokenPlan
                    | UsageQueryTemplate::Balance
                    | UsageQueryTemplate::OfficialSubscription
            )
        {
            return None;
        }

        let code = self.usage_query_code.trim();
        if code.is_empty() {
            return Some(texts::tui_usage_query_script_empty());
        }
        if !code.contains("return") {
            return Some(texts::tui_usage_query_must_have_return());
        }

        None
    }

    pub fn input(&self, field: ProviderAddField) -> Option<&TextInput> {
        match field {
            ProviderAddField::Id => Some(&self.id),
            ProviderAddField::Name => Some(&self.name),
            ProviderAddField::WebsiteUrl => Some(&self.website_url),
            ProviderAddField::Notes => Some(&self.notes),
            ProviderAddField::ClaudeBaseUrl => Some(&self.claude_base_url),
            ProviderAddField::ClaudeApiKey => Some(&self.claude_api_key),
            ProviderAddField::ClaudeFallbackModel => Some(&self.claude_model),
            ProviderAddField::CodexBaseUrl => Some(&self.codex_base_url),
            ProviderAddField::CodexMaxOutputTokens => Some(&self.codex_max_output_tokens),
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
            | ProviderAddField::CodexAnthropicApiKeyField
            | ProviderAddField::CodexImpersonateClaudeCode
            | ProviderAddField::CodexPromptCacheRouting
            | ProviderAddField::CodexLocalRouting
            | ProviderAddField::LocalProxySettings
            | ProviderAddField::CodexQuickConfig
            | ProviderAddField::CodexGoalMode
            | ProviderAddField::CodexRemoteCompaction
            | ProviderAddField::CodexWireApi
            | ProviderAddField::CodexRequiresOpenaiAuth
            | ProviderAddField::ClaudeApiFormat
            | ProviderAddField::ClaudeModelConfig
            | ProviderAddField::ClaudeAdvancedDivider
            | ProviderAddField::CodexAdvancedDivider
            | ProviderAddField::ClaudeQuickConfig
            | ProviderAddField::ClaudeHideAttribution
            | ProviderAddField::ClaudeTeammates
            | ProviderAddField::ClaudeToolSearch
            | ProviderAddField::ClaudeEffortMax
            | ProviderAddField::ClaudeDisableAutoUpgrade
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
            ProviderAddField::ClaudeFallbackModel => Some(&mut self.claude_model),
            ProviderAddField::CodexBaseUrl => Some(&mut self.codex_base_url),
            ProviderAddField::CodexMaxOutputTokens => Some(&mut self.codex_max_output_tokens),
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
            | ProviderAddField::CodexAnthropicApiKeyField
            | ProviderAddField::CodexImpersonateClaudeCode
            | ProviderAddField::CodexPromptCacheRouting
            | ProviderAddField::CodexLocalRouting
            | ProviderAddField::LocalProxySettings
            | ProviderAddField::CodexQuickConfig
            | ProviderAddField::CodexGoalMode
            | ProviderAddField::CodexRemoteCompaction
            | ProviderAddField::CodexWireApi
            | ProviderAddField::CodexRequiresOpenaiAuth
            | ProviderAddField::ClaudeApiFormat
            | ProviderAddField::ClaudeModelConfig
            | ProviderAddField::ClaudeAdvancedDivider
            | ProviderAddField::CodexAdvancedDivider
            | ProviderAddField::ClaudeQuickConfig
            | ProviderAddField::ClaudeHideAttribution
            | ProviderAddField::ClaudeTeammates
            | ProviderAddField::ClaudeToolSearch
            | ProviderAddField::ClaudeEffortMax
            | ProviderAddField::ClaudeDisableAutoUpgrade
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

    pub fn usage_query_input(&self, field: UsageQueryField) -> Option<&TextInput> {
        match field {
            UsageQueryField::ApiKey => Some(&self.usage_query_api_key),
            UsageQueryField::BaseUrl => Some(&self.usage_query_base_url),
            UsageQueryField::AccessToken => Some(&self.usage_query_access_token),
            UsageQueryField::UserId => Some(&self.usage_query_user_id),
            UsageQueryField::Timeout => Some(&self.usage_query_timeout),
            UsageQueryField::AutoInterval => Some(&self.usage_query_auto_interval),
            UsageQueryField::CodingPlanProvider => Some(&self.usage_query_coding_plan_provider),
            UsageQueryField::Enabled | UsageQueryField::Template | UsageQueryField::Script => None,
        }
    }

    pub fn usage_query_input_mut(&mut self, field: UsageQueryField) -> Option<&mut TextInput> {
        match field {
            UsageQueryField::ApiKey => Some(&mut self.usage_query_api_key),
            UsageQueryField::BaseUrl => Some(&mut self.usage_query_base_url),
            UsageQueryField::AccessToken => Some(&mut self.usage_query_access_token),
            UsageQueryField::UserId => Some(&mut self.usage_query_user_id),
            UsageQueryField::Timeout => Some(&mut self.usage_query_timeout),
            UsageQueryField::AutoInterval => Some(&mut self.usage_query_auto_interval),
            UsageQueryField::CodingPlanProvider => Some(&mut self.usage_query_coding_plan_provider),
            UsageQueryField::Enabled | UsageQueryField::Template | UsageQueryField::Script => None,
        }
    }

    pub fn text_edit_target(&self) -> Option<ProviderTextField> {
        self.text_edit.as_ref().map(TextEditSession::target)
    }

    pub fn is_editing_main_field(&self, field: ProviderAddField) -> bool {
        self.text_edit_target() == Some(ProviderTextField::Main(field))
    }

    pub fn is_editing_usage_query_field(&self, field: UsageQueryField) -> bool {
        self.text_edit_target() == Some(ProviderTextField::UsageQuery(field))
    }

    pub fn is_editing_main_text(&self) -> bool {
        matches!(self.text_edit_target(), Some(ProviderTextField::Main(_)))
    }

    pub fn is_editing_usage_query_text(&self) -> bool {
        matches!(
            self.text_edit_target(),
            Some(ProviderTextField::UsageQuery(_))
        )
    }

    pub fn can_edit_main_field(&self, field: ProviderAddField) -> bool {
        self.input(field).is_some() && (field != ProviderAddField::Id || self.is_id_editable())
    }

    pub fn can_edit_usage_query_field(&self, field: UsageQueryField) -> bool {
        field != UsageQueryField::CodingPlanProvider && self.usage_query_input(field).is_some()
    }

    pub fn begin_main_text_edit(&mut self, field: ProviderAddField) -> bool {
        if !self.can_edit_main_field(field) {
            return false;
        }
        let Some(original) = self.input(field).cloned() else {
            return false;
        };
        let original_error = self.main_field_error(field).map(str::to_string);
        self.clear_main_field_error(field);
        self.text_edit = Some(TextEditSession::new(
            ProviderTextField::Main(field),
            original,
            original_error,
        ));
        self.sync_main_field_index(field);
        true
    }

    pub fn begin_usage_query_text_edit(&mut self, field: UsageQueryField) -> bool {
        if !self.can_edit_usage_query_field(field) {
            return false;
        }
        let Some(original) = self.usage_query_input(field).cloned() else {
            return false;
        };
        let original_error = self.usage_query_field_error(field).map(str::to_string);
        self.clear_usage_query_field_error(field);
        self.text_edit = Some(TextEditSession::new(
            ProviderTextField::UsageQuery(field),
            original,
            original_error,
        ));
        self.sync_usage_query_field_index(field);
        true
    }

    pub fn take_text_edit(&mut self) -> Option<TextEditSession<ProviderTextField>> {
        self.text_edit.take()
    }

    pub fn cancel_text_edit(&mut self) -> Option<ProviderTextField> {
        let (target, original, original_error) = self.text_edit.take()?.into_parts();
        match target {
            ProviderTextField::Main(field) => {
                if let Some(input) = self.input_mut(field) {
                    *input = original;
                }
                self.sync_main_field_index(field);
                if let Some(message) = original_error {
                    self.set_main_field_error(field, message);
                } else {
                    self.clear_main_field_error(field);
                }
            }
            ProviderTextField::UsageQuery(field) => {
                if let Some(input) = self.usage_query_input_mut(field) {
                    *input = original;
                }
                self.sync_usage_query_field_index(field);
                if let Some(message) = original_error {
                    self.set_usage_query_field_error(field, message);
                } else {
                    self.clear_usage_query_field_error(field);
                }
            }
        }
        Some(target)
    }

    pub fn clear_text_edit(&mut self) {
        self.text_edit = None;
    }

    pub fn sync_main_field_index(&mut self, field: ProviderAddField) {
        if let Some(index) = self
            .fields()
            .iter()
            .position(|candidate| *candidate == field)
        {
            self.field_idx = index;
        } else {
            self.field_idx = self.field_idx.min(self.fields().len().saturating_sub(1));
        }
    }

    pub fn sync_usage_query_field_index(&mut self, field: UsageQueryField) {
        let fields = self.usage_query_table_fields();
        if let Some(index) = fields.iter().position(|candidate| *candidate == field) {
            self.usage_query_field_idx = index;
        } else {
            self.usage_query_field_idx = self
                .usage_query_field_idx
                .min(fields.len().saturating_sub(1));
        }
    }

    pub fn main_field_error(&self, field: ProviderAddField) -> Option<&str> {
        self.field_errors
            .iter()
            .find(|error| error.field == field)
            .map(|error| error.message.as_str())
    }

    pub fn set_main_field_error(&mut self, field: ProviderAddField, message: impl Into<String>) {
        self.clear_main_field_error(field);
        self.field_errors.push(InlineFieldError {
            field,
            message: message.into(),
        });
    }

    pub fn clear_main_field_error(&mut self, field: ProviderAddField) {
        self.field_errors.retain(|error| error.field != field);
    }

    pub fn usage_query_field_error(&self, field: UsageQueryField) -> Option<&str> {
        self.usage_query_field_errors
            .iter()
            .find(|error| error.field == field)
            .map(|error| error.message.as_str())
    }

    pub fn set_usage_query_field_error(
        &mut self,
        field: UsageQueryField,
        message: impl Into<String>,
    ) {
        self.clear_usage_query_field_error(field);
        self.usage_query_field_errors.push(InlineFieldError {
            field,
            message: message.into(),
        });
    }

    pub fn clear_usage_query_field_error(&mut self, field: UsageQueryField) {
        self.usage_query_field_errors
            .retain(|error| error.field != field);
    }

    /// The five Claude quick toggles, in upstream order. They live on the
    /// "快捷配置菜单" sub-page rather than the main field list.
    pub fn claude_quick_config_fields(&self) -> Vec<ProviderAddField> {
        vec![
            ProviderAddField::ClaudeHideAttribution,
            ProviderAddField::ClaudeTeammates,
            ProviderAddField::ClaudeToolSearch,
            ProviderAddField::ClaudeEffortMax,
            ProviderAddField::ClaudeDisableAutoUpgrade,
        ]
    }

    pub fn claude_quick_config_enabled_count(&self) -> usize {
        [
            self.claude_hide_attribution,
            self.claude_teammates,
            self.claude_tool_search,
            self.claude_effort_max,
            self.claude_disable_auto_upgrade,
        ]
        .into_iter()
        .filter(|enabled| *enabled)
        .count()
    }

    pub fn selected_claude_quick_config_field(&self) -> Option<ProviderAddField> {
        let fields = self.claude_quick_config_fields();
        fields
            .get(
                self.claude_quick_config_idx
                    .min(fields.len().saturating_sub(1)),
            )
            .copied()
    }

    pub fn open_claude_quick_config_page(&mut self) {
        if !matches!(self.app_type, AppType::Claude) {
            return;
        }
        self.page = ProviderFormPage::ClaudeQuickConfig;
        self.focus = FormFocus::Fields;
        self.clear_text_edit();
        let len = self.claude_quick_config_fields().len();
        self.claude_quick_config_idx = self.claude_quick_config_idx.min(len.saturating_sub(1));
    }

    pub fn close_claude_quick_config_page(&mut self) {
        self.page = ProviderFormPage::Main;
        self.focus = FormFocus::Fields;
        self.clear_text_edit();
    }

    pub fn toggle_codex_goal_mode(&mut self) {
        self.codex_goal_mode = !self.codex_goal_mode;
        self.codex_goal_mode_touched = true;
    }

    pub fn toggle_codex_remote_compaction(&mut self) {
        self.codex_remote_compaction = !self.codex_remote_compaction;
        self.codex_remote_compaction_touched = true;
    }

    /// The Codex quick toggles, in upstream order. They live on the Codex
    /// "快捷配置菜单" sub-page rather than the main field list. Goal mode is
    /// available for every Codex provider; remote compaction is only meaningful
    /// for custom providers (upstream `showRemoteCompaction = category != "official"`).
    pub fn codex_quick_config_fields(&self) -> Vec<ProviderAddField> {
        let mut fields = vec![ProviderAddField::CodexGoalMode];
        if !self.is_codex_official_provider() {
            fields.push(ProviderAddField::CodexRemoteCompaction);
        }
        fields
    }

    pub fn codex_quick_config_enabled_count(&self) -> usize {
        self.codex_quick_config_fields()
            .iter()
            .filter(|field| match field {
                ProviderAddField::CodexGoalMode => self.codex_goal_mode,
                ProviderAddField::CodexRemoteCompaction => self.codex_remote_compaction,
                _ => false,
            })
            .count()
    }

    pub fn selected_codex_quick_config_field(&self) -> Option<ProviderAddField> {
        let fields = self.codex_quick_config_fields();
        fields
            .get(
                self.codex_quick_config_idx
                    .min(fields.len().saturating_sub(1)),
            )
            .copied()
    }

    pub fn open_codex_quick_config_page(&mut self) {
        if !matches!(self.app_type, AppType::Codex) {
            return;
        }
        self.page = ProviderFormPage::CodexQuickConfig;
        self.focus = FormFocus::Fields;
        self.clear_text_edit();
        let len = self.codex_quick_config_fields().len();
        self.codex_quick_config_idx = self.codex_quick_config_idx.min(len.saturating_sub(1));
    }

    pub fn close_codex_quick_config_page(&mut self) {
        self.page = ProviderFormPage::Main;
        self.focus = FormFocus::Fields;
        self.clear_text_edit();
    }

    pub(crate) fn refresh_quick_config_from_common_snippet(
        &mut self,
        common_snippet: &str,
    ) -> Result<(), String> {
        if !self.include_common_config {
            return Ok(());
        }

        let effective_provider = self.to_provider_json_value_with_common_config(common_snippet)?;
        let effective = effective_provider
            .get("settingsConfig")
            .cloned()
            .unwrap_or_else(|| json!({}));

        match self.app_type {
            AppType::Claude => {
                self.claude_hide_attribution = super::claude_hide_attribution_enabled(&effective);
                self.claude_teammates = super::claude_teammates_enabled(&effective);
                self.claude_tool_search = super::claude_tool_search_enabled(&effective);
                self.claude_effort_max = super::claude_effort_max_enabled(&effective);
                self.claude_disable_auto_upgrade =
                    super::claude_disable_auto_upgrade_enabled(&effective);
            }
            AppType::Codex => {
                if let Some(config) = effective.get("config").and_then(Value::as_str) {
                    self.codex_goal_mode = crate::codex_config::is_codex_goal_mode_enabled(config);
                    self.codex_remote_compaction =
                        crate::codex_config::is_codex_remote_compaction_enabled(config);
                }
            }
            AppType::Gemini | AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => {}
        }
        Ok(())
    }

    fn materialize_common_config_for_quick_edit(
        &mut self,
        common_snippet: &str,
    ) -> Result<(), String> {
        let field_errors = self.field_errors.clone();
        let usage_query_field_errors = self.usage_query_field_errors.clone();
        let raw_provider_value = self.to_provider_json_value();
        let mut provider_value = self.to_provider_json_value_with_common_config(common_snippet)?;
        if matches!(self.app_type, AppType::Codex) {
            // The backend's Codex live snapshot intentionally contains only
            // auth.json and config.toml. Rehydrating the form from that live
            // shape must not discard provider-owned storage fields such as
            // modelCatalog or future settingsConfig siblings.
            let raw_settings = raw_provider_value
                .get("settingsConfig")
                .and_then(Value::as_object);
            let effective_settings = provider_value
                .get_mut("settingsConfig")
                .and_then(Value::as_object_mut);
            if let (Some(raw_settings), Some(effective_settings)) =
                (raw_settings, effective_settings)
            {
                for (key, value) in raw_settings {
                    effective_settings
                        .entry(key.clone())
                        .or_insert_with(|| value.clone());
                }
            }
        }
        self.apply_provider_json_value_to_fields(provider_value)?;
        self.field_errors = field_errors;
        self.usage_query_field_errors = usage_query_field_errors;
        self.include_common_config = false;
        self.include_common_config_touched = true;
        Ok(())
    }

    fn common_config_overrides_quick_config_value(
        &self,
        field: ProviderAddField,
        desired: bool,
        common_snippet: &str,
    ) -> Result<bool, String> {
        if !self.include_common_config || common_snippet.trim().is_empty() {
            return Ok(false);
        }

        let mut candidate = self.clone();
        match field {
            ProviderAddField::ClaudeHideAttribution => {
                candidate.claude_hide_attribution = desired;
                candidate.claude_hide_attribution_touched = true;
            }
            ProviderAddField::ClaudeTeammates => {
                candidate.claude_teammates = desired;
                candidate.claude_teammates_touched = true;
            }
            ProviderAddField::ClaudeToolSearch => {
                candidate.claude_tool_search = desired;
                candidate.claude_tool_search_touched = true;
            }
            ProviderAddField::ClaudeEffortMax => {
                candidate.claude_effort_max = desired;
                candidate.claude_effort_max_touched = true;
            }
            ProviderAddField::ClaudeDisableAutoUpgrade => {
                candidate.claude_disable_auto_upgrade = desired;
                candidate.claude_disable_auto_upgrade_touched = true;
            }
            ProviderAddField::CodexGoalMode => {
                candidate.codex_goal_mode = desired;
                candidate.codex_goal_mode_touched = true;
            }
            ProviderAddField::CodexRemoteCompaction => {
                candidate.codex_remote_compaction = desired;
                candidate.codex_remote_compaction_touched = true;
            }
            _ => return Ok(false),
        }

        let effective = candidate.to_provider_json_value_with_common_config(common_snippet)?;
        let settings = effective
            .get("settingsConfig")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let effective_value = match field {
            ProviderAddField::ClaudeHideAttribution => {
                super::claude_hide_attribution_enabled(&settings)
            }
            ProviderAddField::ClaudeTeammates => super::claude_teammates_enabled(&settings),
            ProviderAddField::ClaudeToolSearch => super::claude_tool_search_enabled(&settings),
            ProviderAddField::ClaudeEffortMax => super::claude_effort_max_enabled(&settings),
            ProviderAddField::ClaudeDisableAutoUpgrade => {
                super::claude_disable_auto_upgrade_enabled(&settings)
            }
            ProviderAddField::CodexGoalMode => settings
                .get("config")
                .and_then(Value::as_str)
                .is_some_and(crate::codex_config::is_codex_goal_mode_enabled),
            ProviderAddField::CodexRemoteCompaction => settings
                .get("config")
                .and_then(Value::as_str)
                .is_some_and(crate::codex_config::is_codex_remote_compaction_enabled),
            _ => return Ok(false),
        };

        Ok(effective_value != desired)
    }

    fn current_quick_config_value(&self, field: ProviderAddField) -> Option<bool> {
        match field {
            ProviderAddField::ClaudeHideAttribution => Some(self.claude_hide_attribution),
            ProviderAddField::ClaudeTeammates => Some(self.claude_teammates),
            ProviderAddField::ClaudeToolSearch => Some(self.claude_tool_search),
            ProviderAddField::ClaudeEffortMax => Some(self.claude_effort_max),
            ProviderAddField::ClaudeDisableAutoUpgrade => Some(self.claude_disable_auto_upgrade),
            ProviderAddField::CodexGoalMode => Some(self.codex_goal_mode),
            ProviderAddField::CodexRemoteCompaction => Some(self.codex_remote_compaction),
            _ => None,
        }
    }

    pub fn toggle_claude_quick_config_field(
        &mut self,
        field: ProviderAddField,
        common_snippet: &str,
    ) -> Result<(), String> {
        if !matches!(
            field,
            ProviderAddField::ClaudeHideAttribution
                | ProviderAddField::ClaudeTeammates
                | ProviderAddField::ClaudeToolSearch
                | ProviderAddField::ClaudeEffortMax
                | ProviderAddField::ClaudeDisableAutoUpgrade
        ) {
            return Ok(());
        }
        let Some(currently_enabled) = self.current_quick_config_value(field) else {
            return Ok(());
        };
        let desired = !currently_enabled;
        if self.include_common_config
            && !Self::snippet_has_effective_common_config(&self.app_type, common_snippet)
        {
            // Claude's upstream quick editor still mutates provider settings
            // when its shared snippet is malformed or has no content. The
            // membership effect then detaches that provider from sharing.
            self.include_common_config = false;
            self.include_common_config_touched = true;
        } else if self.common_config_overrides_quick_config_value(field, desired, common_snippet)? {
            // Upstream edits the effective settings document. Removing one of
            // its shared values makes that provider independent of the
            // shared snippet, while retaining the other currently effective
            // values as provider-owned settings.
            self.materialize_common_config_for_quick_edit(common_snippet)?;
        }
        if self.include_common_config
            && Self::snippet_has_effective_common_config(&self.app_type, common_snippet)
        {
            // A legacy provider may infer sharing from its stored subset while
            // lacking explicit metadata. A quick edit is an intentional form
            // mutation, so persist the inferred enabled state on save.
            self.include_common_config_touched = true;
        }

        match field {
            ProviderAddField::ClaudeHideAttribution => self.toggle_claude_hide_attribution(),
            ProviderAddField::ClaudeTeammates => self.toggle_claude_teammates(),
            ProviderAddField::ClaudeToolSearch => self.toggle_claude_tool_search(),
            ProviderAddField::ClaudeEffortMax => self.toggle_claude_effort_max(),
            ProviderAddField::ClaudeDisableAutoUpgrade => self.toggle_claude_disable_auto_upgrade(),
            _ => {}
        }
        Ok(())
    }

    pub fn toggle_codex_quick_config_field(
        &mut self,
        field: ProviderAddField,
        common_snippet: &str,
    ) -> Result<(), String> {
        if !matches!(
            field,
            ProviderAddField::CodexGoalMode | ProviderAddField::CodexRemoteCompaction
        ) {
            return Ok(());
        }
        let Some(currently_enabled) = self.current_quick_config_value(field) else {
            return Ok(());
        };
        let desired = !currently_enabled;
        if self.include_common_config
            && !Self::snippet_has_effective_common_config(&self.app_type, common_snippet)
        {
            // Codex's membership effect disables sharing after the next
            // config edit when a valid snippet has no effective TOML keys.
            // Invalid TOML still blocks the edit and surfaces its parse error.
            Self::validate_common_config_snippet_for_form(&self.app_type, common_snippet)?;
            self.include_common_config = false;
            self.include_common_config_touched = true;
        } else if self.common_config_overrides_quick_config_value(field, desired, common_snippet)? {
            self.materialize_common_config_for_quick_edit(common_snippet)?;
        }
        if self.include_common_config
            && Self::snippet_has_effective_common_config(&self.app_type, common_snippet)
        {
            self.include_common_config_touched = true;
        }

        match field {
            ProviderAddField::CodexGoalMode => self.toggle_codex_goal_mode(),
            ProviderAddField::CodexRemoteCompaction => self.toggle_codex_remote_compaction(),
            _ => {}
        }
        Ok(())
    }

    pub fn supports_local_proxy_settings(&self) -> bool {
        match self.app_type {
            AppType::Claude => {
                !self.is_claude_official_provider() && !self.is_claude_github_copilot_provider()
            }
            AppType::Codex => !self.is_codex_official_provider(),
            AppType::Gemini | AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => false,
        }
    }

    pub fn local_proxy_settings_fields(&self) -> &'static [LocalProxySettingsField] {
        &LocalProxySettingsField::ALL
    }

    pub fn selected_local_proxy_settings_field(&self) -> Option<LocalProxySettingsField> {
        self.local_proxy_settings_fields()
            .get(
                self.local_proxy_settings_field_idx
                    .min(self.local_proxy_settings_fields().len().saturating_sub(1)),
            )
            .copied()
    }

    pub fn open_local_proxy_settings_page(&mut self) {
        if !self.supports_local_proxy_settings() {
            return;
        }
        self.page = ProviderFormPage::LocalProxySettings;
        self.focus = FormFocus::Fields;
        self.clear_text_edit();
        self.local_proxy_settings_field_idx = self
            .local_proxy_settings_field_idx
            .min(self.local_proxy_settings_fields().len().saturating_sub(1));
    }

    pub fn close_local_proxy_settings_page(&mut self) {
        self.page = ProviderFormPage::Main;
        self.focus = FormFocus::Fields;
        self.clear_text_edit();
    }

    pub fn local_proxy_body_field_count(&self) -> usize {
        self.local_proxy_body_override
            .as_ref()
            .and_then(Value::as_object)
            .map_or(0, serde_json::Map::len)
    }

    pub fn local_proxy_settings_summary(&self) -> String {
        texts::tui_local_proxy_settings_summary(
            !self.custom_user_agent.is_blank_for_passive_display(),
            self.local_proxy_header_overrides.len(),
            self.local_proxy_body_field_count(),
        )
    }

    pub fn local_proxy_header_overrides_summary(&self) -> String {
        texts::tui_local_proxy_headers_summary(self.local_proxy_header_overrides.len())
    }

    pub fn local_proxy_body_override_summary(&self) -> String {
        texts::tui_local_proxy_body_summary(self.local_proxy_body_field_count())
    }

    pub fn custom_user_agent_is_valid(&self) -> bool {
        if self.custom_user_agent.value.len() > PASSIVE_TEXT_SCAN_MAX_BYTES {
            return false;
        }
        super::provider_request_overrides::is_valid_custom_user_agent(&self.custom_user_agent.value)
    }

    pub fn set_custom_user_agent_preset(&mut self, preset: &str) {
        self.custom_user_agent.set(preset);
    }

    pub fn apply_local_proxy_header_overrides(
        &mut self,
        overrides: std::collections::BTreeMap<String, String>,
    ) {
        self.local_proxy_header_overrides = overrides;
    }

    pub fn apply_local_proxy_body_override(&mut self, override_value: Option<Value>) {
        self.local_proxy_body_override = override_value;
    }

    pub(super) fn reset_local_proxy_settings_state(&mut self) {
        self.custom_user_agent.set("");
        self.local_proxy_header_overrides.clear();
        self.local_proxy_body_override = None;
        self.local_proxy_settings_field_idx = 0;
        if matches!(self.page, ProviderFormPage::LocalProxySettings) {
            self.page = ProviderFormPage::Main;
        }
    }

    pub fn open_usage_query_page(&mut self) {
        self.refresh_usage_query_provider_kind();
        self.refresh_default_usage_query_template();
        self.page = ProviderFormPage::UsageQuery;
        self.focus = FormFocus::Fields;
        self.clear_text_edit();
        let len = self.usage_query_table_fields().len();
        self.usage_query_field_idx = self.usage_query_field_idx.min(len.saturating_sub(1));
    }

    pub fn open_codex_local_routing_page(&mut self) {
        if !matches!(self.app_type, AppType::Codex) || self.is_codex_official_provider() {
            return;
        }
        self.page = ProviderFormPage::CodexLocalRouting;
        self.focus = FormFocus::Fields;
        self.clear_text_edit();
        let len = self.codex_local_routing_fields().len();
        self.codex_local_routing_field_idx = self
            .codex_local_routing_field_idx
            .min(len.saturating_sub(1));
    }

    pub fn close_codex_local_routing_page(&mut self) {
        self.page = ProviderFormPage::Main;
        self.focus = FormFocus::Fields;
        self.clear_text_edit();
    }

    pub fn open_codex_model_catalog_page(&mut self) {
        // Model mapping is available for both Chat and native Responses formats.
        self.page = ProviderFormPage::CodexModelCatalog;
        self.focus = FormFocus::Fields;
        self.clear_text_edit();
        self.codex_model_catalog_idx = self
            .codex_model_catalog_idx
            .min(self.codex_model_catalog.len().saturating_sub(1));
    }

    pub fn close_codex_model_catalog_page(&mut self) {
        self.page = ProviderFormPage::CodexLocalRouting;
        self.focus = FormFocus::Fields;
        self.clear_text_edit();
        self.codex_model_catalog_idx = self
            .codex_model_catalog_idx
            .min(self.codex_model_catalog.len().saturating_sub(1));
    }

    pub fn selected_codex_model_catalog_row(&self) -> Option<&CodexModelCatalogRow> {
        self.codex_model_catalog.get(
            self.codex_model_catalog_idx
                .min(self.codex_model_catalog.len().saturating_sub(1)),
        )
    }

    pub fn selected_codex_model_catalog_field_value(
        &self,
        field: CodexModelCatalogField,
    ) -> String {
        let Some(row) = self.selected_codex_model_catalog_row() else {
            return String::new();
        };
        match field {
            CodexModelCatalogField::Model => row.model.clone(),
            CodexModelCatalogField::DisplayName => row.display_name.clone(),
            CodexModelCatalogField::ContextWindow => row.context_window.clone(),
        }
    }

    pub fn upsert_codex_model_catalog_model(&mut self, model: &str) -> bool {
        let model = model.trim();
        if model.is_empty() {
            return false;
        }

        if let Some(index) = self
            .codex_model_catalog
            .iter()
            .position(|row| row.model.eq_ignore_ascii_case(model))
        {
            self.codex_model_catalog_idx = index;
            return false;
        }

        self.codex_model_catalog.push(CodexModelCatalogRow {
            model: model.to_string(),
            display_name: String::new(),
            context_window: String::new(),
            supports_parallel_tool_calls: None,
            input_modalities: Vec::new(),
            base_instructions: String::new(),
        });
        self.codex_model_catalog_idx = self.codex_model_catalog.len().saturating_sub(1);
        true
    }

    pub fn set_selected_codex_model_catalog_model(&mut self, model: &str) -> bool {
        let model = model.trim();
        if model.is_empty() {
            return false;
        }

        let idx = self
            .codex_model_catalog_idx
            .min(self.codex_model_catalog.len().saturating_sub(1));
        if self.codex_model_catalog.get(idx).is_none() {
            return self.upsert_codex_model_catalog_model(model);
        }

        if let Some(existing_idx) = self
            .codex_model_catalog
            .iter()
            .position(|row| row.model.eq_ignore_ascii_case(model))
        {
            if existing_idx != idx {
                self.codex_model_catalog.remove(idx);
                self.codex_model_catalog_idx =
                    existing_idx.min(self.codex_model_catalog.len().saturating_sub(1));
                return true;
            }
        }

        if let Some(row) = self.codex_model_catalog.get_mut(idx) {
            row.model = model.to_string();
            self.codex_model_catalog_idx = idx;
            return true;
        }
        false
    }

    pub fn set_codex_model_catalog_field(
        &mut self,
        row: Option<usize>,
        field: CodexModelCatalogField,
        value: &str,
    ) -> bool {
        match (row, field) {
            (None, CodexModelCatalogField::Model) => self.upsert_codex_model_catalog_model(value),
            (None, _) => false,
            (Some(index), CodexModelCatalogField::Model) => {
                self.codex_model_catalog_idx =
                    index.min(self.codex_model_catalog.len().saturating_sub(1));
                self.set_selected_codex_model_catalog_model(value)
            }
            (Some(index), CodexModelCatalogField::DisplayName) => {
                let Some(row) = self.codex_model_catalog.get_mut(index) else {
                    return false;
                };
                row.display_name = value.trim().to_string();
                self.codex_model_catalog_idx = index;
                true
            }
            (Some(index), CodexModelCatalogField::ContextWindow) => {
                let Some(row) = self.codex_model_catalog.get_mut(index) else {
                    return false;
                };
                row.context_window = value.trim().to_string();
                self.codex_model_catalog_idx = index;
                true
            }
        }
    }

    pub fn remove_selected_codex_model_catalog_model(&mut self) -> bool {
        if self.codex_model_catalog.is_empty() {
            self.codex_model_catalog_idx = 0;
            return false;
        }
        let idx = self
            .codex_model_catalog_idx
            .min(self.codex_model_catalog.len() - 1);
        self.codex_model_catalog.remove(idx);
        self.codex_model_catalog_idx = self
            .codex_model_catalog_idx
            .min(self.codex_model_catalog.len().saturating_sub(1));
        true
    }

    pub fn codex_local_routing_fields(&self) -> Vec<CodexLocalRoutingField> {
        // The "需要本地路由映射" toggle gates everything (decoupled from the
        // upstream format). When on, reasoning shows only for Chat, and the
        // model-mapping table is available for both formats.
        let mut fields = vec![CodexLocalRoutingField::Enabled];
        if self.codex_local_routing_enabled() {
            if self.codex_is_chat_format() {
                fields.push(CodexLocalRoutingField::SupportsThinking);
                fields.push(CodexLocalRoutingField::SupportsEffort);
            }
            fields.push(CodexLocalRoutingField::ModelCatalog);
        }
        fields
    }

    pub fn selected_codex_local_routing_field(&self) -> Option<CodexLocalRoutingField> {
        let fields = self.codex_local_routing_fields();
        fields
            .get(
                self.codex_local_routing_field_idx
                    .min(fields.len().saturating_sub(1)),
            )
            .copied()
    }

    pub fn toggle_codex_local_routing_enabled(&mut self) {
        // Independent of the upstream format: just flip the routing/mapping gate.
        self.codex_local_routing_enabled = !self.codex_local_routing_enabled;
        let len = self.codex_local_routing_fields().len();
        self.codex_local_routing_field_idx = self
            .codex_local_routing_field_idx
            .min(len.saturating_sub(1));
    }

    pub fn cycle_codex_prompt_cache_routing(&mut self) {
        self.codex_prompt_cache_routing = self.codex_prompt_cache_routing.next();
    }

    pub fn toggle_codex_reasoning_thinking(&mut self) {
        let next = !self.codex_reasoning_supports_thinking();
        self.codex_chat_reasoning.supports_thinking = Some(next);
        if !next {
            self.codex_chat_reasoning.supports_effort = Some(false);
        }
    }

    pub fn toggle_codex_reasoning_effort(&mut self) {
        let next = !self.codex_reasoning_supports_effort();
        self.codex_chat_reasoning.supports_effort = Some(next);
        if next {
            self.codex_chat_reasoning.supports_thinking = Some(true);
            if self.codex_chat_reasoning.effort_param.is_none() {
                self.codex_chat_reasoning.effort_param = Some("reasoning_effort".to_string());
            }
        } else {
            self.codex_chat_reasoning.effort_param = Some("none".to_string());
        }
    }

    pub fn codex_reasoning_supports_thinking(&self) -> bool {
        self.codex_chat_reasoning.supports_thinking == Some(true)
            || self.codex_chat_reasoning.supports_effort == Some(true)
    }

    pub fn codex_reasoning_supports_effort(&self) -> bool {
        self.codex_chat_reasoning.supports_effort == Some(true)
    }

    pub fn refresh_default_usage_query_template(&mut self) {
        if self.usage_query_official_subscription {
            if self.usage_query_template != UsageQueryTemplate::OfficialSubscription {
                self.set_usage_query_template(UsageQueryTemplate::OfficialSubscription);
            }
            return;
        }

        if self.usage_query_touched || self.has_usage_script_meta() {
            return;
        }

        let template = self.default_non_official_usage_query_template();

        self.set_usage_query_template(template);
        if let Some(provider) =
            detect_coding_plan_provider_for_usage_query(&self.current_provider_base_url())
        {
            self.usage_query_coding_plan_provider.set(provider);
        }
    }

    fn default_non_official_usage_query_template(&self) -> UsageQueryTemplate {
        match self
            .extra
            .get("meta")
            .and_then(|meta| meta.get("providerType"))
            .and_then(|value| value.as_str())
        {
            Some("github_copilot") => UsageQueryTemplate::GitHubCopilot,
            _ if detect_balance_provider_for_usage_query(&self.current_provider_base_url()) => {
                UsageQueryTemplate::Balance
            }
            _ => UsageQueryTemplate::General,
        }
    }

    pub fn close_usage_query_page(&mut self) {
        self.page = ProviderFormPage::Main;
        self.focus = FormFocus::Fields;
        self.clear_text_edit();
    }

    pub fn open_hermes_models_picker(&mut self) {
        if !matches!(self.app_type, AppType::Hermes) {
            return;
        }
        self.focus = FormFocus::Fields;
        self.clear_text_edit();
        self.hermes_models_editing = false;
        let len = self.hermes_model_field_count();
        self.hermes_models_field_idx = self.hermes_models_field_idx.min(len.saturating_sub(1));
        self.sync_hermes_model_input_from_selection();
    }

    pub fn close_hermes_models_picker(&mut self) {
        self.hermes_models_editing = false;
        self.hermes_model_input.set("");
    }

    pub fn hermes_model_field_count(&self) -> usize {
        self.hermes_models.len().saturating_mul(3)
    }

    fn hermes_model_field_at_index(index: usize) -> HermesModelField {
        let model = index / 3;
        match index % 3 {
            0 => HermesModelField::Id(model),
            1 => HermesModelField::Name(model),
            _ => HermesModelField::ContextLength(model),
        }
    }

    pub fn selected_hermes_model_field(&self) -> Option<HermesModelField> {
        let count = self.hermes_model_field_count();
        (count > 0)
            .then(|| self.hermes_models_field_idx.min(count - 1))
            .map(Self::hermes_model_field_at_index)
    }

    pub fn add_empty_hermes_model(&mut self) {
        if !matches!(self.app_type, AppType::Hermes) {
            return;
        }
        self.hermes_models.push(json!({ "id": "", "name": "" }));
        self.hermes_models_field_idx = self.hermes_models.len().saturating_sub(1).saturating_mul(3);
        self.sync_hermes_model_input_from_selection();
    }

    pub fn remove_hermes_model(&mut self, index: usize) {
        if index >= self.hermes_models.len() {
            return;
        }
        self.hermes_models.remove(index);
        let fields_len = self.hermes_model_field_count();
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
        self.hermes_models_field_idx = target_index.saturating_mul(3);
        self.sync_hermes_model_input_from_selection();
        true
    }

    pub fn touch_usage_query(&mut self) {
        self.usage_query_touched = true;
    }

    pub fn toggle_usage_query_enabled(&mut self) {
        self.usage_query_enabled = !self.usage_query_enabled;
        self.touch_usage_query();
    }

    pub fn selected_usage_query_field(&self) -> Option<UsageQueryField> {
        let fields = self.usage_query_table_fields();
        fields
            .get(
                self.usage_query_field_idx
                    .min(fields.len().saturating_sub(1)),
            )
            .copied()
    }

    pub fn cycle_usage_query_coding_plan_provider(&mut self) {
        let options = ["kimi", "zhipu", "minimax"];
        let current = options
            .iter()
            .position(|value| *value == self.usage_query_coding_plan_provider.value.trim())
            .unwrap_or(0);
        self.usage_query_coding_plan_provider
            .set(options[(current + 1) % options.len()]);
        self.touch_usage_query();
    }

    pub fn available_usage_query_templates(&self) -> Vec<UsageQueryTemplate> {
        if self.usage_query_official_subscription {
            return vec![UsageQueryTemplate::OfficialSubscription];
        }

        vec![
            UsageQueryTemplate::Custom,
            UsageQueryTemplate::General,
            UsageQueryTemplate::NewApi,
            UsageQueryTemplate::Balance,
        ]
    }

    pub fn set_usage_query_template(&mut self, template: UsageQueryTemplate) {
        self.clear_usage_query_field_error(UsageQueryField::Script);
        self.usage_query_template = template;
        match template {
            UsageQueryTemplate::Custom => {
                self.usage_query_code = self.usage_query_custom_preset_with_variables();
                self.usage_query_api_key.set("");
                self.usage_query_base_url.set("");
                self.usage_query_access_token.set("");
                self.usage_query_user_id.set("");
            }
            UsageQueryTemplate::General => {
                self.usage_query_code = Self::USAGE_QUERY_GENERAL_PRESET.to_string();
                self.usage_query_access_token.set("");
                self.usage_query_user_id.set("");
            }
            UsageQueryTemplate::NewApi => {
                self.usage_query_code = Self::USAGE_QUERY_NEWAPI_PRESET.to_string();
                self.usage_query_api_key.set("");
            }
            UsageQueryTemplate::GitHubCopilot | UsageQueryTemplate::Balance => {
                self.usage_query_code.clear();
                self.usage_query_api_key.set("");
                self.usage_query_base_url.set("");
                self.usage_query_access_token.set("");
                self.usage_query_user_id.set("");
            }
            UsageQueryTemplate::OfficialSubscription => {
                self.usage_query_code.clear();
                self.usage_query_api_key.set("");
                self.usage_query_base_url.set("");
                self.usage_query_access_token.set("");
                self.usage_query_user_id.set("");
            }
            UsageQueryTemplate::TokenPlan => {
                self.usage_query_code.clear();
                self.usage_query_api_key.set("");
                self.usage_query_base_url.set("");
                self.usage_query_access_token.set("");
                self.usage_query_user_id.set("");
                if self
                    .usage_query_coding_plan_provider
                    .value
                    .trim()
                    .is_empty()
                {
                    self.usage_query_coding_plan_provider.set("kimi");
                }
            }
        }
        let len = self.usage_query_table_fields().len();
        self.usage_query_field_idx = self.usage_query_field_idx.min(len.saturating_sub(1));
    }

    pub fn refresh_usage_query_custom_variable_comment(&mut self) {
        if self.usage_query_template != UsageQueryTemplate::Custom {
            return;
        }

        let Some(body) = Self::strip_usage_query_custom_variable_comment(&self.usage_query_code)
            .map(str::to_string)
        else {
            return;
        };
        let next = format!("{}{}", self.usage_query_custom_variable_comment(), body);
        if self.usage_query_code != next {
            self.usage_query_code = next;
            self.touch_usage_query();
        }
    }

    pub fn usage_query_script_help_lines() -> Vec<String> {
        vec![
            texts::tui_usage_query_config_format().to_string(),
            "({".to_string(),
            "  request: {".to_string(),
            "    url: \"{{baseUrl}}/api/usage\",".to_string(),
            "    method: \"POST\",".to_string(),
            "    headers: {".to_string(),
            "      \"Authorization\": \"Bearer {{apiKey}}\",".to_string(),
            "      \"User-Agent\": \"cc-switch/1.0\"".to_string(),
            "    }".to_string(),
            "  },".to_string(),
            "  extractor: function(response) {".to_string(),
            "    return {".to_string(),
            "      isValid: !response.error,".to_string(),
            "      remaining: response.balance,".to_string(),
            "      unit: \"USD\"".to_string(),
            "    };".to_string(),
            "  }".to_string(),
            "})".to_string(),
            String::new(),
            texts::tui_usage_query_extractor_format().to_string(),
            texts::tui_usage_query_field_is_valid().to_string(),
            texts::tui_usage_query_field_invalid_message().to_string(),
            texts::tui_usage_query_field_remaining().to_string(),
            texts::tui_usage_query_field_unit().to_string(),
            texts::tui_usage_query_field_plan_name().to_string(),
            texts::tui_usage_query_field_total().to_string(),
            texts::tui_usage_query_field_used().to_string(),
            texts::tui_usage_query_field_extra().to_string(),
            String::new(),
            texts::tui_usage_query_tips().to_string(),
            texts::tui_usage_query_tip1().to_string(),
            texts::tui_usage_query_tip2().to_string(),
            texts::tui_usage_query_tip3().to_string(),
        ]
    }

    pub fn usage_query_template_value(&self) -> &'static str {
        if self.usage_query_template == UsageQueryTemplate::OfficialSubscription {
            self.usage_query_template.label()
        } else {
            self.usage_query_template.as_str()
        }
    }

    pub fn usage_query_template_label(&self) -> &'static str {
        self.usage_query_template.label()
    }

    pub fn usage_query_extractor_available(&self) -> bool {
        self.usage_query_enabled
            && !matches!(
                self.usage_query_template,
                UsageQueryTemplate::GitHubCopilot
                    | UsageQueryTemplate::TokenPlan
                    | UsageQueryTemplate::OfficialSubscription
            )
    }

    pub fn refresh_usage_query_provider_kind(&mut self) {
        let was_official = self.usage_query_official_subscription;
        let is_official = serde_json::from_value::<Provider>(self.to_provider_json_value())
            .ok()
            .and_then(|provider| provider.official_subscription_tool(&self.app_type))
            .is_some();
        self.usage_query_official_subscription = is_official;

        if was_official == is_official && !self.usage_query_touched {
            return;
        }

        let next_template = if is_official {
            Some(UsageQueryTemplate::OfficialSubscription)
        } else if self.usage_query_template == UsageQueryTemplate::OfficialSubscription {
            Some(self.default_non_official_usage_query_template())
        } else {
            None
        };
        let entering_official = !was_official && is_official;
        let should_persist = self.usage_query_touched || self.has_usage_script_meta();
        let template_changed = if let Some(template) =
            next_template.filter(|template| *template != self.usage_query_template)
        {
            self.set_usage_query_template(template);
            true
        } else {
            false
        };
        if entering_official {
            // Matching the desktop modal, crossing into the native OAuth
            // template is a fresh opt-in rather than inheriting an enabled
            // custom query.
            self.usage_query_enabled = false;
            self.usage_query_timeout.set("10");
            self.usage_query_auto_interval.set("5");
            self.usage_query_coding_plan_provider.set("");
        }
        if should_persist && (template_changed || entering_official) {
            self.touch_usage_query();
        }
    }

    fn usage_query_custom_preset_with_variables(&self) -> String {
        format!(
            "{}{}",
            self.usage_query_custom_variable_comment(),
            Self::USAGE_QUERY_CUSTOM_PRESET
        )
    }

    fn usage_query_custom_variable_comment(&self) -> String {
        let (api_key, base_url) = self.usage_query_provider_credentials();
        format!(
            "// 支持的变量\n// {{{{baseUrl}}}}\n// =\n// {base_url}\n// {{{{apiKey}}}}\n// =\n// {api_key}\n\n"
        )
    }

    pub fn current_provider_base_url(&self) -> String {
        if self.is_claude_codex_oauth_provider() {
            return "https://chatgpt.com/backend-api/codex".to_string();
        }

        match self.app_type {
            AppType::Claude => self.claude_base_url.value.clone(),
            AppType::Codex => self.codex_base_url.value.clone(),
            AppType::Gemini => self.gemini_base_url.value.clone(),
            AppType::Hermes => self.hermes_base_url.value.clone(),
            AppType::OpenCode | AppType::OpenClaw => self.opencode_base_url.value.clone(),
        }
    }

    fn usage_query_provider_credentials(&self) -> (String, String) {
        if self.is_claude_codex_oauth_provider() {
            return (
                String::new(),
                "https://chatgpt.com/backend-api/codex".to_string(),
            );
        }

        let (api_key, base_url) = match self.app_type {
            AppType::Claude => (&self.claude_api_key.value, &self.claude_base_url.value),
            AppType::Codex => (&self.codex_api_key.value, &self.codex_base_url.value),
            AppType::Gemini => (&self.gemini_api_key.value, &self.gemini_base_url.value),
            AppType::Hermes => (&self.hermes_api_key.value, &self.hermes_base_url.value),
            AppType::OpenCode | AppType::OpenClaw => {
                (&self.opencode_api_key.value, &self.opencode_base_url.value)
            }
        };
        (
            Self::usage_query_comment_value(api_key),
            Self::usage_query_comment_value(base_url),
        )
    }

    fn usage_query_comment_value(value: &str) -> String {
        value.trim().replace(['\r', '\n'], " ").trim().to_string()
    }

    fn strip_usage_query_custom_variable_comment(code: &str) -> Option<&str> {
        if !code.starts_with("// 支持的变量\n") {
            return None;
        }

        let mut newline_count = 0;
        for (idx, ch) in code.char_indices() {
            if ch == '\n' {
                newline_count += 1;
                if newline_count == 8 {
                    return code.get(idx + ch.len_utf8()..);
                }
            }
        }
        None
    }

    // The model-mapping sub-page exposes role models; the main
    // model (ANTHROPIC_MODEL) is edited via the top-level ClaudeFallbackModel row.
    pub fn claude_model_input(&self, index: usize) -> Option<&TextInput> {
        match ClaudeModelRole::from_index(index)? {
            ClaudeModelRole::Haiku => Some(&self.claude_haiku_model),
            ClaudeModelRole::Sonnet => Some(&self.claude_sonnet_model),
            ClaudeModelRole::Opus => Some(&self.claude_opus_model),
            ClaudeModelRole::Fable => Some(&self.claude_fable_model),
            ClaudeModelRole::Subagent => Some(&self.claude_subagent_model),
        }
    }

    pub fn claude_model_input_mut(&mut self, index: usize) -> Option<&mut TextInput> {
        match ClaudeModelRole::from_index(index)? {
            ClaudeModelRole::Haiku => Some(&mut self.claude_haiku_model),
            ClaudeModelRole::Sonnet => Some(&mut self.claude_sonnet_model),
            ClaudeModelRole::Opus => Some(&mut self.claude_opus_model),
            ClaudeModelRole::Fable => Some(&mut self.claude_fable_model),
            ClaudeModelRole::Subagent => Some(&mut self.claude_subagent_model),
        }
    }

    pub fn claude_model_supports_one_m(index: usize) -> bool {
        ClaudeModelRole::from_index(index).is_some_and(ClaudeModelRole::supports_one_m)
    }

    pub fn claude_model_one_m_enabled(&self, index: usize) -> bool {
        match ClaudeModelRole::from_index(index) {
            Some(ClaudeModelRole::Sonnet) => self.claude_sonnet_one_m,
            Some(ClaudeModelRole::Opus) => self.claude_opus_one_m,
            Some(ClaudeModelRole::Fable) => self.claude_fable_one_m,
            Some(ClaudeModelRole::Subagent) => self.claude_subagent_one_m,
            _ => false,
        }
    }

    fn set_claude_model_one_m_enabled(&mut self, index: usize, enabled: bool) {
        match ClaudeModelRole::from_index(index) {
            Some(ClaudeModelRole::Sonnet) => self.claude_sonnet_one_m = enabled,
            Some(ClaudeModelRole::Opus) => self.claude_opus_one_m = enabled,
            Some(ClaudeModelRole::Fable) => self.claude_fable_one_m = enabled,
            Some(ClaudeModelRole::Subagent) => self.claude_subagent_one_m = enabled,
            _ => {}
        }
    }

    pub fn set_claude_model_from_config(&mut self, index: usize, value: &str) {
        let (model, one_m) = split_claude_one_m_marker(value);
        if let Some(input) = self.claude_model_input_mut(index) {
            input.set(model);
        }
        if Self::claude_model_supports_one_m(index) {
            self.set_claude_model_one_m_enabled(index, one_m);
        }
    }

    pub fn set_claude_model_from_picker(&mut self, index: usize, value: &str) {
        let preserve_one_m = self.claude_model_one_m_enabled(index);
        let (model, explicit_one_m) = split_claude_one_m_marker(value);
        if let Some(input) = self.claude_model_input_mut(index) {
            input.set(model);
        }
        if Self::claude_model_supports_one_m(index) {
            self.set_claude_model_one_m_enabled(index, explicit_one_m || preserve_one_m);
        }
        self.mark_claude_model_role_touched(index);
    }

    pub fn normalize_claude_model_input(&mut self, index: usize) -> bool {
        let Some(raw) = self
            .claude_model_input(index)
            .map(|input| input.value.clone())
        else {
            return false;
        };
        let (model, explicit_one_m) = split_claude_one_m_marker(&raw);
        let supports_one_m = Self::claude_model_supports_one_m(index);
        let enabled = supports_one_m
            && !model.trim().is_empty()
            && (explicit_one_m || self.claude_model_one_m_enabled(index));
        let changed =
            model != raw || (supports_one_m && enabled != self.claude_model_one_m_enabled(index));
        if changed {
            if let Some(input) = self.claude_model_input_mut(index) {
                input.set(model);
            }
            if supports_one_m {
                self.set_claude_model_one_m_enabled(index, enabled);
            }
        }
        changed
    }

    pub fn toggle_claude_model_one_m(&mut self, index: usize) -> bool {
        if !Self::claude_model_supports_one_m(index) {
            return false;
        }

        let currently_enabled = self.claude_model_one_m_enabled(index);
        let model_is_blank = self
            .claude_model_input(index)
            .is_none_or(|input| input.value.trim().is_empty());
        if model_is_blank && !currently_enabled {
            return false;
        }

        let enabled = !currently_enabled;
        self.set_claude_model_one_m_enabled(index, enabled);
        self.mark_claude_model_role_touched(index);
        true
    }

    pub fn claude_model_value_for_config(&self, index: usize) -> String {
        let Some(input) = self.claude_model_input(index) else {
            return String::new();
        };
        let model = input.value.trim();
        if model.is_empty() {
            return String::new();
        }
        if !Self::claude_model_supports_one_m(index) {
            return split_claude_one_m_marker(model).0;
        }
        if self.claude_model_one_m_enabled(index) {
            format!("{model}[1M]")
        } else {
            model.to_string()
        }
    }

    pub fn fill_claude_models_from(&mut self, source_index: usize) -> bool {
        let Some(source) = self.claude_model_input(source_index) else {
            return false;
        };
        if source.value.trim().is_empty() {
            return false;
        }

        let (model, _) = split_claude_one_m_marker(&source.value);
        let one_m = Self::claude_model_supports_one_m(source_index)
            && self.claude_model_one_m_enabled(source_index);
        for role in ClaudeModelRole::ALL {
            let index = role.index();
            if let Some(input) = self.claude_model_input_mut(index) {
                input.set(model.clone());
            }
            self.set_claude_model_one_m_enabled(
                index,
                Self::claude_model_supports_one_m(index) && one_m,
            );
        }
        self.mark_all_claude_model_roles_touched();
        true
    }

    pub fn claude_model_configured_count(&self) -> usize {
        ClaudeModelRole::ALL
            .into_iter()
            .filter_map(|role| self.claude_model_input(role.index()))
            .filter(|input| !input.is_blank_for_passive_display())
            .count()
    }

    pub fn mark_claude_fallback_model_touched(&mut self) {
        self.claude_fallback_model_touched = true;
    }

    pub fn mark_claude_model_role_touched(&mut self, index: usize) {
        if let Some(touched) = self.claude_model_role_touched.get_mut(index) {
            *touched = true;
        }
    }

    pub fn mark_all_claude_model_roles_touched(&mut self) {
        self.claude_model_role_touched.fill(true);
    }

    pub fn claude_model_role_was_touched(&self, index: usize) -> bool {
        self.claude_model_role_touched
            .get(index)
            .copied()
            .unwrap_or(false)
    }

    pub fn any_claude_model_role_was_touched(&self) -> bool {
        self.claude_model_role_touched
            .iter()
            .any(|touched| *touched)
    }

    pub fn toggle_claude_hide_attribution(&mut self) {
        self.claude_hide_attribution = !self.claude_hide_attribution;
        self.claude_hide_attribution_touched = true;
    }

    pub fn toggle_claude_teammates(&mut self) {
        self.claude_teammates = !self.claude_teammates;
        self.claude_teammates_touched = true;
    }

    pub fn toggle_claude_tool_search(&mut self) {
        self.claude_tool_search = !self.claude_tool_search;
        self.claude_tool_search_touched = true;
    }

    pub fn toggle_claude_effort_max(&mut self) {
        self.claude_effort_max = !self.claude_effort_max;
        self.claude_effort_max_touched = true;
    }

    pub fn toggle_claude_disable_auto_upgrade(&mut self) {
        self.claude_disable_auto_upgrade = !self.claude_disable_auto_upgrade;
        self.claude_disable_auto_upgrade_touched = true;
    }

    pub fn supports_full_url_mode(&self) -> bool {
        match self.app_type {
            AppType::Claude => {
                !self.is_claude_official_provider() && !self.is_claude_codex_oauth_provider()
            }
            AppType::Codex => !self.is_codex_official_provider(),
            AppType::Gemini | AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => false,
        }
    }

    pub fn full_url_mode_enabled_for_field(&self, field: ProviderAddField) -> bool {
        self.is_full_url
            && self.supports_full_url_mode()
            && matches!(
                (&self.app_type, field),
                (AppType::Claude, ProviderAddField::ClaudeBaseUrl)
                    | (AppType::Codex, ProviderAddField::CodexBaseUrl)
            )
    }

    pub fn toggle_full_url_mode(&mut self) -> bool {
        if self.supports_full_url_mode() {
            self.is_full_url = !self.is_full_url;
        }
        self.is_full_url
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

    pub fn is_claude_github_copilot_provider(&self) -> bool {
        if !matches!(self.app_type, AppType::Claude) {
            return false;
        }

        let provider_type_matches = self
            .extra
            .get("meta")
            .and_then(|meta| meta.get("providerType"))
            .and_then(Value::as_str)
            .is_some_and(|value| value == "github_copilot");
        let endpoint_matches =
            passive_text_contains(&self.claude_base_url.value, "githubcopilot.com");

        provider_type_matches || endpoint_matches
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

        let website_flag = passive_trimmed_eq_ignore_ascii_case(
            &self.website_url.value,
            "https://chatgpt.com/codex",
        );

        let name_flag = passive_trimmed_eq_ignore_ascii_case(&self.name.value, "OpenAI Official");

        meta_flag || category_flag || website_flag || name_flag
    }

    pub fn codex_local_routing_enabled(&self) -> bool {
        matches!(self.app_type, AppType::Codex)
            && !self.is_codex_official_provider()
            && self.codex_local_routing_enabled
    }

    /// Whether the Codex upstream format is Chat Completions (which needs proxy
    /// conversion). Reasoning capability is Chat-only.
    pub fn codex_is_chat_format(&self) -> bool {
        matches!(self.app_type, AppType::Codex)
            && !self.is_codex_official_provider()
            && matches!(self.claude_api_format, ClaudeApiFormat::OpenAiChat)
    }

    pub fn apply_provider_json_to_fields(&mut self, provider: &Provider) {
        let previous_mode = self.mode.clone();
        let previous_focus = self.focus;
        let previous_page = self.page;
        let previous_copy_source_id = self.copy_source_id.clone();
        let previous_template_idx = self.template_idx;
        let previous_field_idx = self.field_idx;
        let previous_usage_query_field_idx = self.usage_query_field_idx;
        let previous_codex_local_routing_field_idx = self.codex_local_routing_field_idx;
        let previous_local_proxy_settings_field_idx = self.local_proxy_settings_field_idx;
        let previous_claude_quick_config_idx = self.claude_quick_config_idx;
        let previous_codex_quick_config_idx = self.codex_quick_config_idx;
        let previous_codex_model_catalog_field = self.codex_model_catalog_field;
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

        if let Some(enabled) = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.apply_common_config)
        {
            // This is an in-place form rehydration, not a fresh upstream-hook
            // initialization. Honor the editor payload's explicit flag; the
            // caller refreshes it against the actual common snippet next.
            next.include_common_config = enabled;
            next.include_common_config_touched = true;
        } else {
            next.include_common_config = previous_include_common_config;
            next.include_common_config_touched = previous_include_common_config_touched;
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
        next.codex_model_catalog_field = previous_codex_model_catalog_field;
        next.clear_text_edit();
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
        let codex_local_routing_fields_len = next.codex_local_routing_fields().len();
        next.codex_local_routing_field_idx = if codex_local_routing_fields_len == 0 {
            0
        } else {
            previous_codex_local_routing_field_idx.min(codex_local_routing_fields_len - 1)
        };
        next.local_proxy_settings_field_idx = previous_local_proxy_settings_field_idx
            .min(next.local_proxy_settings_fields().len().saturating_sub(1));
        next.claude_quick_config_idx = previous_claude_quick_config_idx
            .min(next.claude_quick_config_fields().len().saturating_sub(1));
        next.codex_quick_config_idx = previous_codex_quick_config_idx
            .min(next.codex_quick_config_fields().len().saturating_sub(1));
        let hermes_model_fields_len = next.hermes_model_field_count();
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
        let previous_codex_local_routing_field_idx = self.codex_local_routing_field_idx;
        let previous_local_proxy_settings_field_idx = self.local_proxy_settings_field_idx;
        let previous_claude_quick_config_idx = self.claude_quick_config_idx;
        let previous_codex_quick_config_idx = self.codex_quick_config_idx;
        let previous_codex_model_catalog_field = self.codex_model_catalog_field;
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

        if let Some(enabled) = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.apply_common_config)
        {
            next.include_common_config = enabled;
            next.include_common_config_touched = true;
        } else {
            next.include_common_config = previous_include_common_config;
            next.include_common_config_touched = previous_include_common_config_touched;
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
        next.codex_model_catalog_field = previous_codex_model_catalog_field;
        next.clear_text_edit();
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
        let codex_local_routing_fields_len = next.codex_local_routing_fields().len();
        next.codex_local_routing_field_idx = if codex_local_routing_fields_len == 0 {
            0
        } else {
            previous_codex_local_routing_field_idx.min(codex_local_routing_fields_len - 1)
        };
        next.local_proxy_settings_field_idx = previous_local_proxy_settings_field_idx
            .min(next.local_proxy_settings_fields().len().saturating_sub(1));
        next.claude_quick_config_idx = previous_claude_quick_config_idx
            .min(next.claude_quick_config_fields().len().saturating_sub(1));
        next.codex_quick_config_idx = previous_codex_quick_config_idx
            .min(next.codex_quick_config_fields().len().saturating_sub(1));
        let hermes_model_fields_len = next.hermes_model_field_count();
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
        if next_enabled {
            Self::validate_common_config_snippet_for_form(&self.app_type, common_snippet)?;
            if !matches!(self.app_type, AppType::Claude)
                && !Self::snippet_has_effective_common_config(&self.app_type, common_snippet)
            {
                return Err(texts::common_config_snippet_required().to_string());
            }
            let mut next = self.clone();
            next.include_common_config = true;
            next.include_common_config_touched = true;
            next.refresh_quick_config_from_common_snippet(common_snippet)?;
            *self = next;
            return Ok(());
        }

        if self.include_common_config {
            let mut next = self.clone();
            let field_errors = next.field_errors.clone();
            let usage_query_field_errors = next.usage_query_field_errors.clone();
            let mut provider_value = next.to_provider_json_value();
            if let Some(settings_value) = provider_value
                .as_object_mut()
                .and_then(|obj| obj.get_mut("settingsConfig"))
            {
                if let Err(err) = strip_common_config_from_settings(
                    &next.app_type,
                    settings_value,
                    common_snippet,
                ) {
                    // Upstream turns the switch off even when removing a
                    // malformed snippet fails, while keeping the settings
                    // document unchanged and surfacing the parse error.
                    next.include_common_config = false;
                    next.include_common_config_touched = true;
                    *self = next;
                    return Err(err);
                }
            }

            if let Ok(provider) = serde_json::from_value::<Provider>(provider_value) {
                let stripped_settings = provider.settings_config.clone();
                next.apply_provider_json_to_fields(&provider);
                if let Some(extra_obj) = next.extra.as_object_mut() {
                    extra_obj.insert("settingsConfig".to_string(), stripped_settings);
                }
            }
            next.field_errors = field_errors;
            next.usage_query_field_errors = usage_query_field_errors;
            next.include_common_config = false;
            next.include_common_config_touched = true;
            *self = next;
        }
        Ok(())
    }

    pub(crate) fn replace_common_config_snippet(
        &mut self,
        previous_snippet: &str,
        next_snippet: &str,
    ) -> Result<(), String> {
        if !self.include_common_config {
            if !next_snippet.trim().is_empty() {
                Self::validate_common_config_snippet_for_form(&self.app_type, next_snippet)?;
            }
            let settings = self
                .to_provider_json_value()
                .get("settingsConfig")
                .cloned()
                .unwrap_or_else(|| json!({}));
            if Self::settings_contain_common_config_for_form(
                &self.app_type,
                &settings,
                next_snippet,
            ) {
                // Upstream membership effects run after every snippet change,
                // even while the switch is off. The values already present in
                // the draft are the effective view, so no apply step is needed.
                self.include_common_config = true;
                self.include_common_config_touched = true;
            }
            return Ok(());
        }

        if !next_snippet.trim().is_empty() {
            Self::validate_common_config_snippet_for_form(&self.app_type, next_snippet)?;
        }

        if let Err(previous_error) =
            Self::validate_common_config_snippet_for_form(&self.app_type, previous_snippet)
        {
            if matches!(self.app_type, AppType::Codex | AppType::Gemini) {
                // Upstream skips removal when the previous Codex/Gemini
                // snippet cannot be parsed, then applies the valid replacement
                // (or simply disables sharing when the snippet was cleared).
                let mut next = self.clone();
                next.include_common_config = false;
                next.include_common_config_touched = true;
                if !next_snippet.trim().is_empty() {
                    next.include_common_config = true;
                    next.refresh_quick_config_from_common_snippet(next_snippet)?;
                }
                *self = next;
                return Ok(());
            }

            // Claude ignores the malformed-old removal error when clearing,
            // but a non-empty replacement reports it and its membership effect
            // turns sharing off against the newly saved snippet.
            self.include_common_config = false;
            self.include_common_config_touched = true;
            return if next_snippet.trim().is_empty() {
                Ok(())
            } else {
                Err(previous_error)
            };
        }

        let previous = self.clone();
        let result = (|| {
            self.toggle_include_common_config(previous_snippet)?;
            if !next_snippet.trim().is_empty() {
                self.include_common_config_touched = true;
                // Upstream keeps the switch enabled after replacing an
                // enabled snippet with a valid non-empty document, even when
                // Gemini normalization yields no effective env entries. Its
                // membership effect detaches only after a later env edit.
                self.include_common_config = true;
                self.refresh_quick_config_from_common_snippet(next_snippet)?;
            }
            Ok(())
        })();
        if result.is_err() {
            *self = previous;
        }
        result
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
        if self.hermes_api_mode.len() > PASSIVE_TEXT_SCAN_MAX_BYTES {
            return HERMES_DEFAULT_API_MODE;
        }
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

    pub(crate) fn codex_model_catalog_summary(&self) -> String {
        let count = self.codex_model_catalog.len();
        if crate::cli::i18n::is_chinese() {
            format!("{count} 个模型")
        } else if count == 1 {
            "1 model".to_string()
        } else {
            format!("{count} models")
        }
    }

    #[cfg(test)]
    pub fn apply_codex_model_catalog_value(&mut self, models_value: Value) -> Result<(), String> {
        if !matches!(self.app_type, AppType::Codex) {
            return Ok(());
        }
        let Some(models) = models_value.as_array() else {
            return Err(texts::tui_toast_json_must_be_array().to_string());
        };
        self.codex_model_catalog = models
            .iter()
            .filter_map(codex_model_catalog_row_from_value)
            .collect();
        self.codex_model_catalog_idx = self
            .codex_model_catalog_idx
            .min(self.codex_model_catalog.len().saturating_sub(1));
        if self.codex_model_catalog.is_empty() {
            self.codex_model_catalog_field = CodexModelCatalogField::Model;
        }
        // Applying a catalog implies routing/mapping is on (mirrors load init).
        self.codex_local_routing_enabled = !self.codex_model_catalog.is_empty();
        Ok(())
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

pub(crate) fn codex_model_catalog_row_from_value(value: &Value) -> Option<CodexModelCatalogRow> {
    let model = value
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if model.is_empty() {
        return None;
    }

    let display_name = value
        .get("displayName")
        .or_else(|| value.get("display_name"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let context_window = value
        .get("contextWindow")
        .or_else(|| value.get("context_window"))
        .and_then(|value| {
            value
                .as_str()
                .map(|value| value.trim().to_string())
                .or_else(|| value.as_u64().map(|value| value.to_string()))
                .or_else(|| value.as_i64().map(|value| value.to_string()))
        })
        .unwrap_or_default();
    let supports_parallel_tool_calls = value
        .get("supportsParallelToolCalls")
        .and_then(Value::as_bool)
        .or_else(|| {
            value
                .get("supports_parallel_tool_calls")
                .and_then(Value::as_bool)
        });
    let input_modalities = value
        .get("inputModalities")
        .and_then(Value::as_array)
        .or_else(|| value.get("input_modalities").and_then(Value::as_array))
        .map(|modalities| {
            modalities
                .iter()
                .filter_map(Value::as_str)
                .filter(|modality| !modality.trim().is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    let base_instructions = value
        .get("baseInstructions")
        .and_then(Value::as_str)
        .or_else(|| value.get("base_instructions").and_then(Value::as_str))
        .unwrap_or("")
        .trim()
        .to_string();

    Some(CodexModelCatalogRow {
        model,
        display_name,
        context_window,
        supports_parallel_tool_calls,
        input_modalities,
        base_instructions,
    })
}

pub(crate) fn detect_coding_plan_provider_for_usage_query(base_url: &str) -> Option<&'static str> {
    let url = base_url.to_lowercase();
    if url.contains("api.kimi.com/coding") {
        Some("kimi")
    } else if url.contains("open.bigmodel.cn")
        || url.contains("bigmodel.cn")
        || url.contains("api.z.ai")
    {
        Some("zhipu")
    } else if url.contains("api.minimaxi.com")
        || url.contains("api.minimax.io")
        || url.contains("api.minimax.com")
    {
        Some("minimax")
    } else {
        None
    }
}

pub(crate) fn detect_balance_provider_for_usage_query(base_url: &str) -> bool {
    let url = base_url.to_lowercase();
    url.contains("api.deepseek.com")
        || url.contains("api.stepfun.ai")
        || url.contains("api.stepfun.com")
        || url.contains("api.siliconflow.cn")
        || url.contains("api.siliconflow.com")
        || url.contains("openrouter.ai")
        || url.contains("api.novita.ai")
}

impl UsageQueryTemplate {
    pub fn label(self) -> &'static str {
        match self {
            Self::Custom => {
                if crate::cli::i18n::is_chinese() {
                    "自定义"
                } else {
                    "Custom"
                }
            }
            Self::General => {
                if crate::cli::i18n::is_chinese() {
                    "通用模板"
                } else {
                    "General"
                }
            }
            Self::NewApi => "NewAPI",
            Self::GitHubCopilot => "GitHub Copilot",
            Self::TokenPlan => "Token Plan",
            Self::Balance => {
                if crate::cli::i18n::is_chinese() {
                    "官方"
                } else {
                    "Official"
                }
            }
            Self::OfficialSubscription => {
                if crate::cli::i18n::is_chinese() {
                    "官方订阅"
                } else {
                    "Official Subscription"
                }
            }
        }
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
