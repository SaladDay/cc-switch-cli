use crate::app_config::{AppType, McpApps, McpServer};
use crate::provider::Provider;
use serde_json::{json, Value};

#[derive(Debug, Clone, Default)]
pub struct TextInput {
    pub value: String,
    pub cursor: usize,
}

impl TextInput {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.chars().count();
        Self { value, cursor }
    }

    pub fn set(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.value.chars().count();
    }

    pub fn is_blank(&self) -> bool {
        self.value.trim().is_empty()
    }

    fn byte_index(line: &str, col: usize) -> usize {
        line.char_indices()
            .nth(col)
            .map(|(i, _)| i)
            .unwrap_or(line.len())
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_right(&mut self) {
        let len = self.value.chars().count();
        self.cursor = (self.cursor + 1).min(len);
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.value.chars().count();
    }

    pub fn insert_char(&mut self, c: char) -> bool {
        let idx = Self::byte_index(&self.value, self.cursor);
        self.value.insert(idx, c);
        self.cursor += 1;
        true
    }

    pub fn backspace(&mut self) -> bool {
        if self.cursor == 0 || self.value.is_empty() {
            return false;
        }
        let start = Self::byte_index(&self.value, self.cursor.saturating_sub(1));
        let end = Self::byte_index(&self.value, self.cursor);
        self.value.replace_range(start..end, "");
        self.cursor = self.cursor.saturating_sub(1);
        true
    }

    pub fn delete(&mut self) -> bool {
        let len = self.value.chars().count();
        if self.value.is_empty() || self.cursor >= len {
            return false;
        }
        let start = Self::byte_index(&self.value, self.cursor);
        let end = Self::byte_index(&self.value, self.cursor + 1);
        self.value.replace_range(start..end, "");
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeminiAuthType {
    OAuth,
    ApiKey,
}

impl GeminiAuthType {
    pub fn as_str(self) -> &'static str {
        match self {
            GeminiAuthType::OAuth => "oauth",
            GeminiAuthType::ApiKey => "api_key",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexWireApi {
    Chat,
    Responses,
}

impl CodexWireApi {
    pub fn as_str(self) -> &'static str {
        match self {
            CodexWireApi::Chat => "chat",
            CodexWireApi::Responses => "responses",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormFocus {
    Templates,
    Fields,
    JsonPreview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexPreviewSection {
    Auth,
    Config,
}

impl CodexPreviewSection {
    pub fn toggle(self) -> Self {
        match self {
            Self::Auth => Self::Config,
            Self::Config => Self::Auth,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormMode {
    Add,
    Edit { id: String },
}

impl FormMode {
    pub fn is_edit(&self) -> bool {
        matches!(self, FormMode::Edit { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderAddField {
    Id,
    Name,
    WebsiteUrl,
    Notes,
    ClaudeBaseUrl,
    ClaudeApiKey,
    ClaudeModelConfig,
    CodexBaseUrl,
    CodexModel,
    CodexWireApi,
    CodexRequiresOpenaiAuth,
    CodexEnvKey,
    CodexApiKey,
    GeminiAuthType,
    GeminiApiKey,
    GeminiBaseUrl,
    GeminiModel,
    CommonConfigDivider,
    CommonSnippet,
    IncludeCommonConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderTemplateId {
    Custom,
    ClaudeOfficial,
    OpenAiOfficial,
    RightCode,
    GoogleOAuth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProviderTemplateDef {
    id: ProviderTemplateId,
    label: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SponsorProviderPreset {
    id: &'static str,
    provider_name: &'static str,
    chip_label: &'static str,
    website_url: &'static str,
    register_url: &'static str,
    promo_code: &'static str,
    partner_promotion_key: &'static str,
    claude_base_url: &'static str,
    codex_base_url: &'static str,
    gemini_base_url: &'static str,
}

// Add new sponsor presets here.
// They will automatically show up as extra templates in the TUI "Add Provider" form
// for Claude/Codex/Gemini (appended after the built-in templates).
const SPONSOR_PROVIDER_PRESETS: [SponsorProviderPreset; 1] = [SponsorProviderPreset {
    id: "packycode",
    provider_name: "PackyCode",
    chip_label: "* PackyCode",
    website_url: "https://www.packyapi.com",
    register_url: "https://www.packyapi.com/register?aff=cc-switch-cli",
    promo_code: "cc-switch-cli",
    partner_promotion_key: "packycode",
    claude_base_url: "https://www.packyapi.com",
    codex_base_url: "https://www.packyapi.com/v1",
    gemini_base_url: "https://www.packyapi.com",
}];

const PROVIDER_TEMPLATE_DEFS_CLAUDE: [ProviderTemplateDef; 3] = [
    ProviderTemplateDef {
        id: ProviderTemplateId::Custom,
        label: "Custom",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::ClaudeOfficial,
        label: "Claude Official",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::RightCode,
        label: "RightCode",
    },
];

const PROVIDER_TEMPLATE_DEFS_CODEX: [ProviderTemplateDef; 3] = [
    ProviderTemplateDef {
        id: ProviderTemplateId::Custom,
        label: "Custom",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::OpenAiOfficial,
        label: "OpenAI Official",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::RightCode,
        label: "RightCode",
    },
];

const PROVIDER_TEMPLATE_DEFS_GEMINI: [ProviderTemplateDef; 2] = [
    ProviderTemplateDef {
        id: ProviderTemplateId::Custom,
        label: "Custom",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::GoogleOAuth,
        label: "Google OAuth",
    },
];

fn provider_builtin_template_defs(app_type: &AppType) -> &'static [ProviderTemplateDef] {
    match app_type {
        AppType::Claude => &PROVIDER_TEMPLATE_DEFS_CLAUDE,
        AppType::Codex => &PROVIDER_TEMPLATE_DEFS_CODEX,
        AppType::Gemini => &PROVIDER_TEMPLATE_DEFS_GEMINI,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpAddField {
    Id,
    Name,
    Command,
    Args,
    AppClaude,
    AppCodex,
    AppGemini,
}

#[derive(Debug, Clone)]
pub struct ProviderAddFormState {
    pub app_type: AppType,
    pub mode: FormMode,
    pub focus: FormFocus,
    pub template_idx: usize,
    pub field_idx: usize,
    pub editing: bool,
    pub extra: Value,
    pub id: TextInput,
    pub id_is_manual: bool,
    pub name: TextInput,
    pub website_url: TextInput,
    pub notes: TextInput,
    pub include_common_config: bool,
    pub json_scroll: usize,
    pub codex_preview_section: CodexPreviewSection,
    pub codex_auth_scroll: usize,
    pub codex_config_scroll: usize,
    claude_model_config_touched: bool,

    // Claude
    pub claude_api_key: TextInput,
    pub claude_base_url: TextInput,
    pub claude_model: TextInput,
    pub claude_reasoning_model: TextInput,
    pub claude_haiku_model: TextInput,
    pub claude_sonnet_model: TextInput,
    pub claude_opus_model: TextInput,

    // Codex
    pub codex_base_url: TextInput,
    pub codex_model: TextInput,
    pub codex_wire_api: CodexWireApi,
    pub codex_requires_openai_auth: bool,
    pub codex_env_key: TextInput,
    pub codex_api_key: TextInput,

    // Gemini
    pub gemini_auth_type: GeminiAuthType,
    pub gemini_api_key: TextInput,
    pub gemini_base_url: TextInput,
    pub gemini_model: TextInput,
}

impl ProviderAddFormState {
    pub fn new(app_type: AppType) -> Self {
        let codex_defaults = match app_type {
            AppType::Codex => (
                "https://api.openai.com/v1",
                "gpt-5.2-codex",
                CodexWireApi::Responses,
                true,
            ),
            _ => ("", "", CodexWireApi::Responses, true),
        };

        let gemini_base_url_default = "https://generativelanguage.googleapis.com";

        Self {
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
            include_common_config: true,
            json_scroll: 0,
            codex_preview_section: CodexPreviewSection::Auth,
            codex_auth_scroll: 0,
            codex_config_scroll: 0,
            claude_model_config_touched: false,

            claude_api_key: TextInput::new(""),
            claude_base_url: TextInput::new(""),
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
            gemini_base_url: TextInput::new(gemini_base_url_default),
            gemini_model: TextInput::new(""),
        }
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
            .unwrap_or(true);

        match app_type {
            AppType::Claude => {
                if let Some(env) = provider
                    .settings_config
                    .get("env")
                    .and_then(|v| v.as_object())
                {
                    if let Some(token) = crate::services::provider::get_claude_token_from_env(env) {
                        form.claude_api_key.set(token);
                    }
                    if let Some(url) = env.get("ANTHROPIC_BASE_URL").and_then(|v| v.as_str()) {
                        form.claude_base_url.set(url);
                    }
                    if let Some(model) = env.get("ANTHROPIC_MODEL").and_then(|v| v.as_str()) {
                        form.claude_model.set(model);
                    }
                    if let Some(reasoning) = env
                        .get("ANTHROPIC_REASONING_MODEL")
                        .and_then(|v| v.as_str())
                    {
                        form.claude_reasoning_model.set(reasoning);
                    }

                    let model = env.get("ANTHROPIC_MODEL").and_then(|v| v.as_str());
                    let small_fast = env
                        .get("ANTHROPIC_SMALL_FAST_MODEL")
                        .and_then(|v| v.as_str());

                    if let Some(haiku) = env
                        .get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
                        .and_then(|v| v.as_str())
                        .or(small_fast)
                        .or(model)
                    {
                        form.claude_haiku_model.set(haiku);
                    }
                    if let Some(sonnet) = env
                        .get("ANTHROPIC_DEFAULT_SONNET_MODEL")
                        .and_then(|v| v.as_str())
                        .or(model)
                        .or(small_fast)
                    {
                        form.claude_sonnet_model.set(sonnet);
                    }
                    if let Some(opus) = env
                        .get("ANTHROPIC_DEFAULT_OPUS_MODEL")
                        .and_then(|v| v.as_str())
                        .or(model)
                        .or(small_fast)
                    {
                        form.claude_opus_model.set(opus);
                    }
                }
            }
            AppType::Codex => {
                if let Some(cfg) = provider
                    .settings_config
                    .get("config")
                    .and_then(|v| v.as_str())
                {
                    let parsed = parse_codex_config_snippet(cfg);
                    if let Some(base_url) = parsed.base_url {
                        form.codex_base_url.set(base_url);
                    }
                    if let Some(model) = parsed.model {
                        form.codex_model.set(model);
                    }
                    if let Some(wire_api) = parsed.wire_api {
                        form.codex_wire_api = wire_api;
                    }
                    if let Some(requires_openai_auth) = parsed.requires_openai_auth {
                        form.codex_requires_openai_auth = requires_openai_auth;
                    }
                    if let Some(env_key) = parsed.env_key {
                        form.codex_env_key.set(env_key);
                    }
                }
                if let Some(auth) = provider
                    .settings_config
                    .get("auth")
                    .and_then(|v| v.as_object())
                {
                    if let Some(key) = auth.get("OPENAI_API_KEY").and_then(|v| v.as_str()) {
                        form.codex_api_key.set(key);
                    }
                }
            }
            AppType::Gemini => {
                if let Some(env) = provider
                    .settings_config
                    .get("env")
                    .and_then(|v| v.as_object())
                {
                    if let Some(key) = env.get("GEMINI_API_KEY").and_then(|v| v.as_str()) {
                        form.gemini_auth_type = GeminiAuthType::ApiKey;
                        form.gemini_api_key.set(key);
                    } else {
                        form.gemini_auth_type = GeminiAuthType::OAuth;
                    }

                    if let Some(url) = env
                        .get("GOOGLE_GEMINI_BASE_URL")
                        .or_else(|| env.get("GEMINI_BASE_URL"))
                        .and_then(|v| v.as_str())
                    {
                        form.gemini_base_url.set(url);
                    }

                    if let Some(model) = env.get("GEMINI_MODEL").and_then(|v| v.as_str()) {
                        form.gemini_model.set(model);
                    }
                } else {
                    form.gemini_auth_type = GeminiAuthType::OAuth;
                }
            }
        }

        form
    }

    pub fn is_id_editable(&self) -> bool {
        !self.mode.is_edit()
    }

    pub fn has_required_fields(&self) -> bool {
        !self.id.is_blank() && !self.name.is_blank()
    }

    pub fn template_count(&self) -> usize {
        provider_builtin_template_defs(&self.app_type).len() + SPONSOR_PROVIDER_PRESETS.len()
    }

    pub fn template_labels(&self) -> Vec<&'static str> {
        let mut labels = provider_builtin_template_defs(&self.app_type)
            .iter()
            .map(|def| def.label)
            .collect::<Vec<_>>();
        labels.extend(
            SPONSOR_PROVIDER_PRESETS
                .iter()
                .map(|preset| preset.chip_label),
        );
        labels
    }

    pub fn fields(&self) -> Vec<ProviderAddField> {
        let mut fields = vec![
            ProviderAddField::Name,
            ProviderAddField::WebsiteUrl,
            ProviderAddField::Notes,
        ];

        match self.app_type {
            AppType::Claude => {
                fields.push(ProviderAddField::ClaudeBaseUrl);
                fields.push(ProviderAddField::ClaudeApiKey);
                fields.push(ProviderAddField::ClaudeModelConfig);
            }
            AppType::Codex => {
                fields.push(ProviderAddField::CodexBaseUrl);
                fields.push(ProviderAddField::CodexModel);
                if !self.is_codex_official_provider() {
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
        }

        fields.push(ProviderAddField::CommonConfigDivider);
        fields.push(ProviderAddField::CommonSnippet);
        fields.push(ProviderAddField::IncludeCommonConfig);
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
            ProviderAddField::CodexWireApi
            | ProviderAddField::CodexRequiresOpenaiAuth
            | ProviderAddField::ClaudeModelConfig
            | ProviderAddField::GeminiAuthType
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
            ProviderAddField::CodexWireApi
            | ProviderAddField::CodexRequiresOpenaiAuth
            | ProviderAddField::ClaudeModelConfig
            | ProviderAddField::GeminiAuthType
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

    pub fn apply_template(&mut self, idx: usize, existing_ids: &[String]) {
        let builtin_defs = provider_builtin_template_defs(&self.app_type);
        let total_templates = builtin_defs.len() + SPONSOR_PROVIDER_PRESETS.len();
        let idx = idx.min(total_templates.saturating_sub(1));
        self.template_idx = idx;
        self.id_is_manual = false;

        if idx >= builtin_defs.len() {
            let sponsor_idx = idx.saturating_sub(builtin_defs.len());
            if let Some(preset) = SPONSOR_PROVIDER_PRESETS.get(sponsor_idx) {
                self.apply_sponsor_preset(preset);
            }
        } else {
            let template_id = builtin_defs
                .get(idx)
                .map(|def| def.id)
                .unwrap_or(ProviderTemplateId::Custom);

            if template_id == ProviderTemplateId::Custom {
                if matches!(self.mode, FormMode::Add) {
                    let defaults = Self::new(self.app_type.clone());
                    self.extra = defaults.extra;
                    self.id = defaults.id;
                    self.id_is_manual = defaults.id_is_manual;
                    self.name = defaults.name;
                    self.website_url = defaults.website_url;
                    self.notes = defaults.notes;
                    self.json_scroll = defaults.json_scroll;
                    self.codex_preview_section = defaults.codex_preview_section;
                    self.codex_auth_scroll = defaults.codex_auth_scroll;
                    self.codex_config_scroll = defaults.codex_config_scroll;
                    self.claude_model_config_touched = defaults.claude_model_config_touched;
                    self.claude_api_key = defaults.claude_api_key;
                    self.claude_base_url = defaults.claude_base_url;
                    self.claude_model = defaults.claude_model;
                    self.claude_reasoning_model = defaults.claude_reasoning_model;
                    self.claude_haiku_model = defaults.claude_haiku_model;
                    self.claude_sonnet_model = defaults.claude_sonnet_model;
                    self.claude_opus_model = defaults.claude_opus_model;
                    self.codex_base_url = defaults.codex_base_url;
                    self.codex_model = defaults.codex_model;
                    self.codex_wire_api = defaults.codex_wire_api;
                    self.codex_requires_openai_auth = defaults.codex_requires_openai_auth;
                    self.codex_env_key = defaults.codex_env_key;
                    self.codex_api_key = defaults.codex_api_key;
                    self.gemini_auth_type = defaults.gemini_auth_type;
                    self.gemini_api_key = defaults.gemini_api_key;
                    self.gemini_base_url = defaults.gemini_base_url;
                    self.gemini_model = defaults.gemini_model;
                }
                return;
            }

            self.extra = json!({});
            self.notes.set("");
            match template_id {
                ProviderTemplateId::Custom => {}
                ProviderTemplateId::ClaudeOfficial => {
                    self.name.set("Claude Official");
                    self.website_url.set("https://anthropic.com");
                    self.claude_base_url.set("https://api.anthropic.com");
                }
                ProviderTemplateId::OpenAiOfficial => {
                    self.extra = json!({
                        "category": "official",
                        "meta": {
                            "codexOfficial": true,
                        }
                    });
                    self.name.set("OpenAI Official");
                    self.website_url.set("https://chatgpt.com/codex");
                    self.codex_base_url.set("https://api.openai.com/v1");
                    self.codex_model.set("gpt-5.2-codex");
                    self.codex_wire_api = CodexWireApi::Responses;
                    self.codex_requires_openai_auth = true;
                }
                ProviderTemplateId::RightCode => {
                    self.name.set("RightCode");
                    self.website_url.set("https://right.codes");
                    match self.app_type {
                        AppType::Claude => {
                            self.claude_base_url.set("https://www.right.codes/claude");
                        }
                        AppType::Codex => {
                            self.codex_base_url.set("https://right.codes/codex/v1");
                            self.codex_model.set("gpt-5.2-codex");
                            self.codex_wire_api = CodexWireApi::Responses;
                        }
                        AppType::Gemini => {}
                    }
                }
                ProviderTemplateId::GoogleOAuth => {
                    self.name.set("Google OAuth");
                    self.website_url.set("https://ai.google.dev");
                    self.gemini_auth_type = GeminiAuthType::OAuth;
                }
            };
        }

        if !self.id_is_manual && !self.name.is_blank() {
            let id = crate::cli::commands::provider_input::generate_provider_id(
                self.name.value.trim(),
                existing_ids,
            );
            self.id.set(id);
        }
    }

    fn apply_sponsor_preset(&mut self, preset: &SponsorProviderPreset) {
        self.extra = json!({
            "meta": {
                "isPartner": true,
                "partnerPromotionKey": preset.partner_promotion_key,
            }
        });
        self.name.set(preset.provider_name);
        self.website_url.set(preset.website_url);
        self.notes.set("");

        match self.app_type {
            AppType::Claude => {
                self.claude_base_url.set(preset.claude_base_url);
            }
            AppType::Codex => {
                self.codex_base_url.set(preset.codex_base_url);
                self.codex_model.set("gpt-5.2-codex");
                self.codex_wire_api = CodexWireApi::Responses;
            }
            AppType::Gemini => {
                self.gemini_auth_type = GeminiAuthType::ApiKey;
                self.gemini_base_url.set(preset.gemini_base_url);
            }
        }
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
            .is_some_and(|category| category.eq_ignore_ascii_case("official"));

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

    pub fn to_provider_json_value(&self) -> Value {
        let mut provider_obj = match self.extra.clone() {
            Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };

        provider_obj.insert("id".to_string(), json!(self.id.value.trim()));
        provider_obj.insert("name".to_string(), json!(self.name.value.trim()));

        upsert_optional_trimmed(
            &mut provider_obj,
            "websiteUrl",
            self.website_url.value.as_str(),
        );
        upsert_optional_trimmed(&mut provider_obj, "notes", self.notes.value.as_str());

        let meta_value = provider_obj
            .entry("meta".to_string())
            .or_insert_with(|| json!({}));
        if !meta_value.is_object() {
            *meta_value = json!({});
        }
        if let Some(meta_obj) = meta_value.as_object_mut() {
            meta_obj.insert(
                "applyCommonConfig".to_string(),
                json!(self.include_common_config),
            );
        }

        let settings_value = provider_obj
            .entry("settingsConfig".to_string())
            .or_insert_with(|| json!({}));
        if !settings_value.is_object() {
            *settings_value = json!({});
        }
        let settings_obj = settings_value
            .as_object_mut()
            .expect("settingsConfig must be a JSON object");

        match self.app_type {
            AppType::Claude => {
                let env_value = settings_obj
                    .entry("env".to_string())
                    .or_insert_with(|| json!({}));
                if !env_value.is_object() {
                    *env_value = json!({});
                }
                let env_obj = env_value
                    .as_object_mut()
                    .expect("env must be a JSON object");
                let (key, old_key) = crate::services::provider::claude_auth_env_keys(&self.claude_api_key.value);
                set_or_remove_trimmed(env_obj, key, &self.claude_api_key.value);
                env_obj.remove(old_key);
                set_or_remove_trimmed(env_obj, "ANTHROPIC_BASE_URL", &self.claude_base_url.value);
                if self.claude_model_config_touched {
                    set_or_remove_trimmed(env_obj, "ANTHROPIC_MODEL", &self.claude_model.value);
                    set_or_remove_trimmed(
                        env_obj,
                        "ANTHROPIC_REASONING_MODEL",
                        &self.claude_reasoning_model.value,
                    );
                    set_or_remove_trimmed(
                        env_obj,
                        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
                        &self.claude_haiku_model.value,
                    );
                    set_or_remove_trimmed(
                        env_obj,
                        "ANTHROPIC_DEFAULT_SONNET_MODEL",
                        &self.claude_sonnet_model.value,
                    );
                    set_or_remove_trimmed(
                        env_obj,
                        "ANTHROPIC_DEFAULT_OPUS_MODEL",
                        &self.claude_opus_model.value,
                    );
                    env_obj.remove("ANTHROPIC_SMALL_FAST_MODEL");
                }
            }
            AppType::Codex => {
                let provider_key =
                    clean_codex_provider_key(self.id.value.trim(), self.name.value.trim());
                let base_url = self.codex_base_url.value.trim().trim_end_matches('/');
                let model = if self.codex_model.is_blank() {
                    "gpt-5.2-codex"
                } else {
                    self.codex_model.value.trim()
                };

                let wire_api = self.codex_wire_api;
                let requires_openai_auth = self.codex_requires_openai_auth;
                let env_key = self.codex_env_key.value.trim();

                let existing_config = settings_obj
                    .get("config")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let base_config = if existing_config.trim().is_empty() {
                    build_codex_provider_config_toml(&provider_key, base_url, model, wire_api)
                } else {
                    existing_config.to_string()
                };
                let config_toml = update_codex_config_snippet(
                    &base_config,
                    base_url,
                    model,
                    wire_api,
                    requires_openai_auth,
                    env_key,
                );
                settings_obj.insert("config".to_string(), Value::String(config_toml));

                if self.is_codex_official_provider() {
                    settings_obj.remove("auth");
                } else {
                    let api_key = self.codex_api_key.value.trim();
                    if api_key.is_empty() {
                        if let Some(auth_obj) = settings_obj
                            .get_mut("auth")
                            .and_then(|value| value.as_object_mut())
                        {
                            auth_obj.remove("OPENAI_API_KEY");
                            if auth_obj.is_empty() {
                                settings_obj.remove("auth");
                            }
                        } else {
                            settings_obj.remove("auth");
                        }
                    } else {
                        let auth = settings_obj
                            .entry("auth".to_string())
                            .or_insert_with(|| json!({}));
                        if !auth.is_object() {
                            *auth = json!({});
                        }
                        let obj = auth.as_object_mut().expect("auth must be a JSON object");
                        obj.insert("OPENAI_API_KEY".to_string(), json!(api_key));
                    }
                }
            }
            AppType::Gemini => {
                let env_value = settings_obj
                    .entry("env".to_string())
                    .or_insert_with(|| json!({}));
                if !env_value.is_object() {
                    *env_value = json!({});
                }
                let env_obj = env_value
                    .as_object_mut()
                    .expect("env must be a JSON object");

                match self.gemini_auth_type {
                    GeminiAuthType::OAuth => {
                        env_obj.remove("GEMINI_API_KEY");
                        env_obj.remove("GOOGLE_GEMINI_BASE_URL");
                        env_obj.remove("GEMINI_BASE_URL");
                        env_obj.remove("GEMINI_MODEL");
                    }
                    GeminiAuthType::ApiKey => {
                        set_or_remove_trimmed(
                            env_obj,
                            "GEMINI_API_KEY",
                            &self.gemini_api_key.value,
                        );
                        set_or_remove_trimmed(
                            env_obj,
                            "GOOGLE_GEMINI_BASE_URL",
                            &self.gemini_base_url.value,
                        );
                        set_or_remove_trimmed(env_obj, "GEMINI_MODEL", &self.gemini_model.value);
                    }
                }
            }
        }

        Value::Object(provider_obj)
    }

    pub fn to_provider_json_value_with_common_config(
        &self,
        common_snippet: &str,
    ) -> Result<Value, String> {
        let mut provider_value = self.to_provider_json_value();
        if !self.include_common_config {
            return Ok(provider_value);
        }

        let snippet = common_snippet.trim();
        if snippet.is_empty() {
            return Ok(provider_value);
        }

        let Some(settings_value) = provider_value
            .as_object_mut()
            .and_then(|obj| obj.get_mut("settingsConfig"))
        else {
            return Ok(provider_value);
        };

        match self.app_type {
            AppType::Claude | AppType::Gemini => {
                let mut common: Value = serde_json::from_str(snippet).map_err(|e| {
                    crate::cli::i18n::texts::common_config_snippet_invalid_json(&e.to_string())
                })?;
                if !common.is_object() {
                    return Err(
                        crate::cli::i18n::texts::common_config_snippet_not_object().to_string()
                    );
                }

                merge_json_values(&mut common, settings_value);
                *settings_value = common;
            }
            AppType::Codex => {
                if !settings_value.is_object() {
                    *settings_value = json!({});
                }
                let settings_obj = settings_value
                    .as_object_mut()
                    .expect("settingsConfig must be a JSON object");
                let base_config = settings_obj
                    .get("config")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();
                let merged_config = merge_codex_common_config_snippet(base_config, snippet)?;
                settings_obj.insert("config".to_string(), Value::String(merged_config));
            }
        }

        Ok(provider_value)
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
        let previous_extra = self.extra.clone();

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

        // Preserve internal provider fields (e.g. meta/applyCommonConfig, partner flags) that are
        // intentionally hidden from the JSON editor preview, so they don't get dropped on save.
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
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct McpAddFormState {
    pub mode: FormMode,
    pub focus: FormFocus,
    pub template_idx: usize,
    pub field_idx: usize,
    pub editing: bool,
    pub extra: Value,
    pub id: TextInput,
    pub name: TextInput,
    pub command: TextInput,
    pub args: TextInput,
    pub apps: McpApps,
    pub json_scroll: usize,
}

impl McpAddFormState {
    pub fn new() -> Self {
        Self {
            mode: FormMode::Add,
            focus: FormFocus::Templates,
            template_idx: 0,
            field_idx: 0,
            editing: false,
            extra: json!({}),
            id: TextInput::new(""),
            name: TextInput::new(""),
            command: TextInput::new(""),
            args: TextInput::new(""),
            apps: McpApps::default(),
            json_scroll: 0,
        }
    }

    pub fn from_server(server: &McpServer) -> Self {
        let mut form = Self::new();
        form.mode = FormMode::Edit {
            id: server.id.clone(),
        };
        form.focus = FormFocus::Fields;
        form.extra = serde_json::to_value(server).unwrap_or_else(|_| json!({}));
        form.id.set(server.id.clone());
        form.name.set(server.name.clone());
        form.apps = server.apps.clone();

        if let Some(command) = server.server.get("command").and_then(|v| v.as_str()) {
            form.command.set(command);
        }
        if let Some(args) = server.server.get("args").and_then(|v| v.as_array()) {
            let joined = args
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            form.args.set(joined);
        }

        form
    }

    pub fn locked_id(&self) -> Option<&str> {
        match &self.mode {
            FormMode::Edit { id } => Some(id.as_str()),
            FormMode::Add => None,
        }
    }

    pub fn has_required_fields(&self) -> bool {
        !self.id.is_blank() && !self.name.is_blank()
    }

    pub fn template_count(&self) -> usize {
        MCP_TEMPLATES.len()
    }

    pub fn template_labels(&self) -> Vec<&'static str> {
        MCP_TEMPLATES.to_vec()
    }

    pub fn fields(&self) -> Vec<McpAddField> {
        vec![
            McpAddField::Id,
            McpAddField::Name,
            McpAddField::Command,
            McpAddField::Args,
            McpAddField::AppClaude,
            McpAddField::AppCodex,
            McpAddField::AppGemini,
        ]
    }

    pub fn input(&self, field: McpAddField) -> Option<&TextInput> {
        match field {
            McpAddField::Id => Some(&self.id),
            McpAddField::Name => Some(&self.name),
            McpAddField::Command => Some(&self.command),
            McpAddField::Args => Some(&self.args),
            McpAddField::AppClaude | McpAddField::AppCodex | McpAddField::AppGemini => None,
        }
    }

    pub fn input_mut(&mut self, field: McpAddField) -> Option<&mut TextInput> {
        match field {
            McpAddField::Id => Some(&mut self.id),
            McpAddField::Name => Some(&mut self.name),
            McpAddField::Command => Some(&mut self.command),
            McpAddField::Args => Some(&mut self.args),
            McpAddField::AppClaude | McpAddField::AppCodex | McpAddField::AppGemini => None,
        }
    }

    pub fn apply_template(&mut self, idx: usize) {
        let idx = idx.min(self.template_count().saturating_sub(1));
        self.template_idx = idx;

        let template = idx;
        if template == 0 {
            if matches!(self.mode, FormMode::Add) {
                let defaults = Self::new();
                self.extra = defaults.extra;
                self.name = defaults.name;
                self.command = defaults.command;
                self.args = defaults.args;
                self.json_scroll = defaults.json_scroll;
            }
            return;
        }

        match template {
            1 => {
                self.name.set("Filesystem");
                self.command.set("npx");
                self.args
                    .set("-y @modelcontextprotocol/server-filesystem /");
            }
            _ => {}
        }
    }

    pub fn to_mcp_server_json_value(&self) -> Value {
        let args = self
            .args
            .value
            .split_whitespace()
            .map(|s| Value::String(s.to_string()))
            .collect::<Vec<_>>();

        let mut obj = match self.extra.clone() {
            Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };

        obj.insert("id".to_string(), json!(self.id.value.trim()));
        obj.insert("name".to_string(), json!(self.name.value.trim()));

        let server_value = obj.entry("server".to_string()).or_insert_with(|| json!({}));
        if !server_value.is_object() {
            *server_value = json!({});
        }
        let server_obj = server_value
            .as_object_mut()
            .expect("server must be a JSON object");
        server_obj.insert("command".to_string(), json!(self.command.value.trim()));
        server_obj.insert("args".to_string(), Value::Array(args));

        obj.insert(
            "apps".to_string(),
            json!({
                "claude": self.apps.claude,
                "codex": self.apps.codex,
                "gemini": self.apps.gemini,
            }),
        );

        Value::Object(obj)
    }
}

fn upsert_optional_trimmed(obj: &mut serde_json::Map<String, Value>, key: &str, raw: &str) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        obj.remove(key);
    } else {
        obj.insert(key.to_string(), json!(trimmed));
    }
}

fn set_or_remove_trimmed(obj: &mut serde_json::Map<String, Value>, key: &str, raw: &str) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        obj.remove(key);
    } else {
        obj.insert(key.to_string(), json!(trimmed));
    }
}

#[derive(Debug, Default)]
struct ParsedCodexConfigSnippet {
    base_url: Option<String>,
    model: Option<String>,
    wire_api: Option<CodexWireApi>,
    requires_openai_auth: Option<bool>,
    env_key: Option<String>,
}

fn parse_codex_config_snippet(cfg: &str) -> ParsedCodexConfigSnippet {
    let mut out = ParsedCodexConfigSnippet::default();
    for line in cfg.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let raw = value.trim().trim_matches('"').trim_matches('\'');
        match key {
            "base_url" => out.base_url = Some(raw.to_string()),
            "model" => out.model = Some(raw.to_string()),
            "wire_api" => {
                out.wire_api = match raw {
                    "chat" => Some(CodexWireApi::Chat),
                    "responses" => Some(CodexWireApi::Responses),
                    _ => None,
                }
            }
            "requires_openai_auth" => {
                out.requires_openai_auth = match raw {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                }
            }
            "env_key" => out.env_key = Some(raw.to_string()),
            _ => {}
        }
    }
    out
}

fn update_codex_config_snippet(
    original: &str,
    base_url: &str,
    model: &str,
    wire_api: CodexWireApi,
    requires_openai_auth: bool,
    env_key: &str,
) -> String {
    let mut lines = original.lines().map(|s| s.to_string()).collect::<Vec<_>>();

    toml_set_string(&mut lines, "base_url", non_empty(base_url));
    toml_set_string(&mut lines, "model", non_empty(model));
    toml_set_string(&mut lines, "wire_api", Some(wire_api.as_str()));
    toml_set_bool(
        &mut lines,
        "requires_openai_auth",
        Some(requires_openai_auth),
    );

    if requires_openai_auth {
        toml_set_string(&mut lines, "env_key", None);
    } else {
        let env_key = non_empty(env_key).unwrap_or("OPENAI_API_KEY");
        toml_set_string(&mut lines, "env_key", Some(env_key));
    }

    // Keep user formatting/comments; only trim leading/trailing empty lines after updates.
    let mut start = 0;
    while start < lines.len() && lines[start].trim().is_empty() {
        start += 1;
    }
    let mut end = lines.len();
    while end > start && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    lines[start..end].join("\n")
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn is_toml_key_line(line: &str, key: &str) -> bool {
    if !line.starts_with(key) {
        return false;
    }
    let rest = &line[key.len()..];
    rest.starts_with(|c: char| c.is_whitespace() || c == '=')
}

fn toml_set_string(lines: &mut Vec<String>, key: &str, value: Option<&str>) {
    if let Some(idx) = lines
        .iter()
        .position(|line| is_toml_key_line(line.trim_start(), key))
    {
        if let Some(value) = value {
            lines[idx] = format!("{key} = \"{}\"", escape_toml_string(value));
        } else {
            lines.remove(idx);
        }
        return;
    }

    if let Some(value) = value {
        lines.push(format!("{key} = \"{}\"", escape_toml_string(value)));
    }
}

fn toml_set_bool(lines: &mut Vec<String>, key: &str, value: Option<bool>) {
    if let Some(idx) = lines
        .iter()
        .position(|line| is_toml_key_line(line.trim_start(), key))
    {
        if let Some(value) = value {
            lines[idx] = format!("{key} = {value}");
        } else {
            lines.remove(idx);
        }
        return;
    }

    if let Some(value) = value {
        lines.push(format!("{key} = {value}"));
    }
}

fn escape_toml_string(value: &str) -> String {
    value.replace('"', "\\\"")
}

fn clean_codex_provider_key(provider_id: &str, provider_name: &str) -> String {
    // Follow upstream's style: lowercase + underscores, trimmed.
    // This key is used for:
    // - model_provider = "<key>"
    // - [model_providers.<key>]
    let raw = if provider_id.trim().is_empty() {
        provider_name.trim()
    } else {
        provider_id.trim()
    };

    let mut key = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();

    while key.starts_with('_') {
        key.remove(0);
    }
    while key.ends_with('_') {
        key.pop();
    }

    if key.is_empty() {
        "custom".to_string()
    } else {
        key
    }
}

fn build_codex_provider_config_toml(
    provider_key: &str,
    base_url: &str,
    model: &str,
    wire_api: CodexWireApi,
) -> String {
    let provider_key = escape_toml_string(provider_key);
    let model = escape_toml_string(model);
    let base_url = escape_toml_string(base_url);

    // Keep in sync with `.upstream/cc-switch`:
    // - uses model_provider + [model_providers.<key>]
    // - defaults to responses wire API
    // - requires_openai_auth = true for OpenAI-compatible providers
    [
        format!("model_provider = \"{}\"", provider_key),
        format!("model = \"{}\"", model),
        "model_reasoning_effort = \"high\"".to_string(),
        "disable_response_storage = true".to_string(),
        String::new(),
        format!("[model_providers.{}]", provider_key),
        format!("name = \"{}\"", provider_key),
        format!("base_url = \"{}\"", base_url),
        format!("wire_api = \"{}\"", wire_api.as_str()),
        "requires_openai_auth = true".to_string(),
        String::new(),
    ]
    .join("\n")
}

fn merge_json_values(base: &mut Value, overlay: &Value) {
    match (base, overlay) {
        (Value::Object(base_obj), Value::Object(overlay_obj)) => {
            for (overlay_key, overlay_value) in overlay_obj {
                match base_obj.get_mut(overlay_key) {
                    Some(base_value) => merge_json_values(base_value, overlay_value),
                    None => {
                        base_obj.insert(overlay_key.clone(), overlay_value.clone());
                    }
                }
            }
        }
        (base_value, overlay_value) => {
            *base_value = overlay_value.clone();
        }
    }
}

fn merge_codex_common_config_snippet(
    config_toml: &str,
    common_snippet: &str,
) -> Result<String, String> {
    use toml_edit::DocumentMut;

    let common_trimmed = common_snippet.trim();
    if common_trimmed.is_empty() {
        return Ok(config_toml.to_string());
    }

    let mut common_doc: DocumentMut = common_trimmed
        .parse()
        .map_err(|e| format!("Invalid common Codex TOML: {e}"))?;

    let config_trimmed = config_toml.trim();
    let config_doc: DocumentMut = if config_trimmed.is_empty() {
        DocumentMut::default()
    } else {
        config_trimmed
            .parse()
            .map_err(|e| format!("Invalid provider Codex TOML: {e}"))?
    };

    merge_toml_tables(common_doc.as_table_mut(), config_doc.as_table());
    Ok(common_doc.to_string())
}

fn merge_toml_tables(dst: &mut toml_edit::Table, src: &toml_edit::Table) {
    for (key, src_item) in src.iter() {
        match (dst.get_mut(key), src_item) {
            (Some(dst_item), toml_edit::Item::Table(src_table)) if dst_item.is_table() => {
                if let Some(dst_table) = dst_item.as_table_mut() {
                    merge_toml_tables(dst_table, src_table);
                }
            }
            _ => {
                dst.insert(key, src_item.clone());
            }
        }
    }
}

fn strip_common_config_from_settings(
    app_type: &AppType,
    settings_value: &mut Value,
    common_snippet: &str,
) -> Result<(), String> {
    let snippet = common_snippet.trim();
    if snippet.is_empty() {
        return Ok(());
    }

    match app_type {
        AppType::Claude | AppType::Gemini => {
            let common: Value = serde_json::from_str(snippet).map_err(|e| {
                crate::cli::i18n::texts::common_config_snippet_invalid_json(&e.to_string())
            })?;
            if !common.is_object() {
                return Err(crate::cli::i18n::texts::common_config_snippet_not_object().to_string());
            }

            strip_common_json_values(settings_value, &common);
        }
        AppType::Codex => {
            if !settings_value.is_object() {
                return Ok(());
            }
            let settings_obj = settings_value
                .as_object_mut()
                .expect("settingsConfig must be a JSON object");
            let current_config = settings_obj
                .get("config")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let stripped = strip_codex_common_config_snippet(current_config, snippet)?;
            settings_obj.insert("config".to_string(), Value::String(stripped));
        }
    }

    Ok(())
}

fn strip_common_json_values(target: &mut Value, common: &Value) {
    if let (Value::Object(target_obj), Value::Object(common_obj)) = (target, common) {
        let keys_to_remove = common_obj
            .iter()
            .filter_map(|(key, common_value)| {
                let Some(target_value) = target_obj.get_mut(key) else {
                    return None;
                };

                if value_matches_common(target_value, common_value) {
                    return Some(key.clone());
                }

                if target_value.is_object() && common_value.is_object() {
                    strip_common_json_values(target_value, common_value);
                    if target_value
                        .as_object()
                        .map(|obj| obj.is_empty())
                        .unwrap_or(false)
                    {
                        return Some(key.clone());
                    }
                }
                None
            })
            .collect::<Vec<_>>();

        for key in keys_to_remove {
            target_obj.remove(&key);
        }
    }
}

fn value_matches_common(value: &Value, common: &Value) -> bool {
    match (value, common) {
        (Value::Object(value_obj), Value::Object(common_obj)) => {
            value_obj.len() == common_obj.len()
                && common_obj.iter().all(|(key, common_value)| {
                    value_obj
                        .get(key)
                        .map(|value_item| value_matches_common(value_item, common_value))
                        .unwrap_or(false)
                })
        }
        (Value::Array(value_arr), Value::Array(common_arr)) => {
            value_arr.len() == common_arr.len()
                && value_arr
                    .iter()
                    .zip(common_arr.iter())
                    .all(|(value_item, common_item)| value_matches_common(value_item, common_item))
        }
        _ => value == common,
    }
}

fn strip_codex_common_config_snippet(
    config_toml: &str,
    common_snippet: &str,
) -> Result<String, String> {
    use toml_edit::DocumentMut;

    let common_trimmed = common_snippet.trim();
    if common_trimmed.is_empty() {
        return Ok(config_toml.to_string());
    }

    let common_doc: DocumentMut = common_trimmed
        .parse()
        .map_err(|e| format!("Invalid common Codex TOML: {e}"))?;

    let config_trimmed = config_toml.trim();
    if config_trimmed.is_empty() {
        return Ok(String::new());
    }

    let mut config_doc: DocumentMut = config_trimmed
        .parse()
        .map_err(|e| format!("Invalid provider Codex TOML: {e}"))?;
    strip_toml_tables(config_doc.as_table_mut(), common_doc.as_table());
    Ok(config_doc.to_string())
}

fn strip_toml_tables(dst: &mut toml_edit::Table, common: &toml_edit::Table) {
    let mut keys_to_remove = Vec::new();

    for (key, common_item) in common.iter() {
        let Some(dst_item) = dst.get_mut(key) else {
            continue;
        };

        match (dst_item, common_item) {
            (toml_edit::Item::Table(dst_table), toml_edit::Item::Table(common_table)) => {
                strip_toml_tables(dst_table, common_table);
                if dst_table.is_empty() {
                    keys_to_remove.push(key.to_string());
                }
            }
            (dst_item, common_item) => {
                if toml_items_equal(dst_item, common_item) {
                    keys_to_remove.push(key.to_string());
                }
            }
        }
    }

    for key in keys_to_remove {
        dst.remove(&key);
    }
}

fn toml_items_equal(left: &toml_edit::Item, right: &toml_edit::Item) -> bool {
    match (left.as_value(), right.as_value()) {
        (Some(left_value), Some(right_value)) => {
            left_value.to_string().trim() == right_value.to_string().trim()
        }
        _ => left.to_string().trim() == right.to_string().trim(),
    }
}

fn should_hide_provider_field(key: &str) -> bool {
    matches!(
        key,
        "category"
            | "createdAt"
            | "icon"
            | "iconColor"
            | "inFailoverQueue"
            | "meta"
            | "sortIndex"
            | "updatedAt"
    )
}

pub fn strip_provider_internal_fields(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                if should_hide_provider_field(k) {
                    continue;
                }
                out.insert(k.clone(), strip_provider_internal_fields(v));
            }
            Value::Object(out)
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(strip_provider_internal_fields).collect())
        }
        other => other.clone(),
    }
}

#[derive(Debug, Clone)]
pub enum FormState {
    ProviderAdd(ProviderAddFormState),
    McpAdd(McpAddFormState),
}

const MCP_TEMPLATES: [&str; 2] = ["Custom", "Filesystem (npx)"];

#[cfg(test)]
mod tests {
    use super::*;

    fn packycode_template_index(app_type: AppType) -> usize {
        let builtin_len = provider_builtin_template_defs(&app_type).len();
        let sponsor_idx = SPONSOR_PROVIDER_PRESETS
            .iter()
            .position(|preset| preset.id == "packycode")
            .expect("PackyCode sponsor preset should exist");
        builtin_len + sponsor_idx
    }

    #[test]
    fn provider_add_form_template_labels_use_ascii_prefix_for_packycode() {
        let form = ProviderAddFormState::new(AppType::Claude);
        let labels = form.template_labels();

        assert!(
            labels.contains(&"* PackyCode"),
            "expected PackyCode chip label to use ASCII prefix for alignment stability"
        );
    }

    #[test]
    fn provider_add_form_template_labels_include_rightcode_for_claude_and_codex() {
        let claude_form = ProviderAddFormState::new(AppType::Claude);
        let claude_labels = claude_form.template_labels();
        assert!(
            claude_labels.contains(&"RightCode"),
            "expected RightCode template label to exist for Claude"
        );
        assert!(
            !claude_labels.contains(&"* RightCode"),
            "expected RightCode template label to not include '*' prefix"
        );

        let codex_form = ProviderAddFormState::new(AppType::Codex);
        let codex_labels = codex_form.template_labels();
        assert!(
            codex_labels.contains(&"RightCode"),
            "expected RightCode template label to exist for Codex"
        );
        assert!(
            !codex_labels.contains(&"* RightCode"),
            "expected RightCode template label to not include '*' prefix"
        );
    }

    #[test]
    fn provider_add_form_template_labels_exclude_rightcode_for_gemini() {
        let form = ProviderAddFormState::new(AppType::Gemini);
        let labels = form.template_labels();

        assert!(
            !labels.contains(&"RightCode"),
            "expected RightCode template label to not exist for Gemini"
        );
        assert!(
            !labels.contains(&"* RightCode"),
            "expected RightCode template label to not exist for Gemini"
        );
    }

    fn rightcode_template_index(app_type: AppType) -> usize {
        ProviderAddFormState::new(app_type)
            .template_labels()
            .iter()
            .position(|label| *label == "RightCode")
            .expect("RightCode template should exist")
    }

    #[test]
    fn provider_add_form_rightcode_template_claude_sets_base_url_and_no_partner_meta() {
        let mut form = ProviderAddFormState::new(AppType::Claude);
        let existing_ids = Vec::<String>::new();

        let idx = rightcode_template_index(AppType::Claude);
        form.apply_template(idx, &existing_ids);

        let provider = form.to_provider_json_value();
        assert_eq!(provider["name"], "RightCode");
        assert_eq!(provider["websiteUrl"], "https://right.codes");
        assert_eq!(
            provider["settingsConfig"]["env"]["ANTHROPIC_BASE_URL"],
            "https://www.right.codes/claude"
        );
        let meta = provider["meta"]
            .as_object()
            .expect("meta should be an object");
        assert!(
            meta.get("isPartner").is_none(),
            "expected RightCode template to not set meta.isPartner"
        );
        assert!(
            meta.get("partnerPromotionKey").is_none(),
            "expected RightCode template to not set meta.partnerPromotionKey"
        );
    }

    #[test]
    fn provider_add_form_rightcode_template_codex_sets_base_url_and_no_partner_meta() {
        let mut form = ProviderAddFormState::new(AppType::Codex);
        let existing_ids = Vec::<String>::new();

        let idx = rightcode_template_index(AppType::Codex);
        form.apply_template(idx, &existing_ids);

        let provider = form.to_provider_json_value();
        assert_eq!(provider["name"], "RightCode");
        assert_eq!(provider["websiteUrl"], "https://right.codes");
        let cfg = provider["settingsConfig"]["config"]
            .as_str()
            .expect("settingsConfig.config should be string");
        assert!(cfg.contains("base_url = \"https://right.codes/codex/v1\""));
        let meta = provider["meta"]
            .as_object()
            .expect("meta should be an object");
        assert!(
            meta.get("isPartner").is_none(),
            "expected RightCode template to not set meta.isPartner"
        );
        assert!(
            meta.get("partnerPromotionKey").is_none(),
            "expected RightCode template to not set meta.partnerPromotionKey"
        );
    }

    #[test]
    fn provider_add_form_fields_include_notes() {
        for app_type in [AppType::Claude, AppType::Codex, AppType::Gemini] {
            let form = ProviderAddFormState::new(app_type.clone());
            let fields = form.fields();

            let website_idx = fields
                .iter()
                .position(|f| *f == ProviderAddField::WebsiteUrl)
                .expect("WebsiteUrl field should exist");
            let notes_idx = fields
                .iter()
                .position(|f| *f == ProviderAddField::Notes)
                .expect("Notes field should exist");
            assert!(
                notes_idx > website_idx,
                "Notes field should appear after WebsiteUrl for {:?}",
                app_type
            );
        }
    }

    #[test]
    fn provider_add_form_claude_fields_include_model_config_entry() {
        let form = ProviderAddFormState::new(AppType::Claude);
        let fields = form.fields();
        let api_key_idx = fields
            .iter()
            .position(|f| *f == ProviderAddField::ClaudeApiKey)
            .expect("ClaudeApiKey field should exist");
        let model_cfg_idx = fields
            .iter()
            .position(|f| *f == ProviderAddField::ClaudeModelConfig)
            .expect("ClaudeModelConfig field should exist");
        assert!(
            model_cfg_idx > api_key_idx,
            "ClaudeModelConfig should appear after ClaudeApiKey"
        );
    }

    #[test]
    fn provider_add_form_packycode_template_claude_sets_partner_meta_and_base_url() {
        let mut form = ProviderAddFormState::new(AppType::Claude);
        let existing_ids = Vec::<String>::new();

        let idx = packycode_template_index(AppType::Claude);
        form.apply_template(idx, &existing_ids);

        let provider = form.to_provider_json_value();
        assert_eq!(provider["name"], "PackyCode");
        assert_eq!(provider["websiteUrl"], "https://www.packyapi.com");
        assert_eq!(
            provider["settingsConfig"]["env"]["ANTHROPIC_BASE_URL"],
            "https://www.packyapi.com"
        );
        assert_eq!(provider["meta"]["isPartner"], true);
        assert_eq!(provider["meta"]["partnerPromotionKey"], "packycode");
    }

    #[test]
    fn provider_add_form_packycode_template_codex_sets_partner_meta_and_base_url() {
        let mut form = ProviderAddFormState::new(AppType::Codex);
        let existing_ids = Vec::<String>::new();

        let idx = packycode_template_index(AppType::Codex);
        form.apply_template(idx, &existing_ids);

        let provider = form.to_provider_json_value();
        assert_eq!(provider["name"], "PackyCode");
        assert_eq!(provider["websiteUrl"], "https://www.packyapi.com");
        let cfg = provider["settingsConfig"]["config"]
            .as_str()
            .expect("settingsConfig.config should be string");
        assert!(cfg.contains("model_provider ="));
        assert!(cfg.contains("[model_providers."));
        assert!(cfg.contains("base_url = \"https://www.packyapi.com/v1\""));
        assert!(cfg.contains("wire_api = \"responses\""));
        assert!(cfg.contains("requires_openai_auth = true"));
        assert_eq!(provider["meta"]["isPartner"], true);
        assert_eq!(provider["meta"]["partnerPromotionKey"], "packycode");
    }

    #[test]
    fn provider_add_form_packycode_template_gemini_sets_partner_meta_and_base_url() {
        let mut form = ProviderAddFormState::new(AppType::Gemini);
        let existing_ids = Vec::<String>::new();

        let idx = packycode_template_index(AppType::Gemini);
        form.apply_template(idx, &existing_ids);

        let provider = form.to_provider_json_value();
        assert_eq!(provider["name"], "PackyCode");
        assert_eq!(provider["websiteUrl"], "https://www.packyapi.com");
        assert_eq!(
            provider["settingsConfig"]["env"]["GOOGLE_GEMINI_BASE_URL"],
            "https://www.packyapi.com"
        );
        assert_eq!(provider["meta"]["isPartner"], true);
        assert_eq!(provider["meta"]["partnerPromotionKey"], "packycode");
    }

    #[test]
    fn provider_add_form_claude_builds_env_settings() {
        let mut form = ProviderAddFormState::new(AppType::Claude);
        form.id.set("p1");
        form.name.set("Provider One");
        form.claude_api_key.set("token");
        form.claude_base_url.set("https://claude.example");

        let provider = form.to_provider_json_value();
        assert_eq!(provider["id"], "p1");
        assert_eq!(provider["name"], "Provider One");
        assert_eq!(
            provider["settingsConfig"]["env"]["ANTHROPIC_API_KEY"],
            "token"
        );
        assert_eq!(
            provider["settingsConfig"]["env"]["ANTHROPIC_BASE_URL"],
            "https://claude.example"
        );
    }

    #[test]
    fn provider_add_form_claude_from_provider_backfills_models_with_legacy_fallback() {
        let provider = Provider::with_id(
            "p1".to_string(),
            "Provider One".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_MODEL": "model-main",
                    "ANTHROPIC_REASONING_MODEL": "model-reasoning",
                    "ANTHROPIC_SMALL_FAST_MODEL": "model-small-fast",
                    "ANTHROPIC_DEFAULT_SONNET_MODEL": "model-sonnet-explicit",
                }
            }),
            None,
        );

        let form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
        assert_eq!(form.claude_model.value, "model-main");
        assert_eq!(form.claude_reasoning_model.value, "model-reasoning");
        assert_eq!(form.claude_haiku_model.value, "model-small-fast");
        assert_eq!(form.claude_sonnet_model.value, "model-sonnet-explicit");
        assert_eq!(form.claude_opus_model.value, "model-main");
    }

    #[test]
    fn provider_add_form_claude_writes_new_model_keys_and_removes_small_fast() {
        let mut form = ProviderAddFormState::new(AppType::Claude);
        form.id.set("p1");
        form.name.set("Provider One");
        form.extra = json!({
            "settingsConfig": {
                "env": {
                    "ANTHROPIC_SMALL_FAST_MODEL": "legacy-small",
                    "FOO": "bar"
                }
            }
        });
        form.claude_model.set("model-main");
        form.claude_reasoning_model.set("model-reasoning");
        form.claude_haiku_model.set("model-haiku");
        form.claude_sonnet_model.set("model-sonnet");
        form.claude_opus_model.set("model-opus");
        form.mark_claude_model_config_touched();

        let provider = form.to_provider_json_value();
        let env = provider["settingsConfig"]["env"]
            .as_object()
            .expect("settingsConfig.env should be object");
        assert_eq!(
            env.get("ANTHROPIC_MODEL").and_then(|v| v.as_str()),
            Some("model-main")
        );
        assert_eq!(
            env.get("ANTHROPIC_REASONING_MODEL")
                .and_then(|v| v.as_str()),
            Some("model-reasoning")
        );
        assert_eq!(
            env.get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
                .and_then(|v| v.as_str()),
            Some("model-haiku")
        );
        assert_eq!(
            env.get("ANTHROPIC_DEFAULT_SONNET_MODEL")
                .and_then(|v| v.as_str()),
            Some("model-sonnet")
        );
        assert_eq!(
            env.get("ANTHROPIC_DEFAULT_OPUS_MODEL")
                .and_then(|v| v.as_str()),
            Some("model-opus")
        );
        assert!(env.get("ANTHROPIC_SMALL_FAST_MODEL").is_none());
        assert_eq!(env.get("FOO").and_then(|v| v.as_str()), Some("bar"));
    }

    #[test]
    fn provider_add_form_claude_empty_model_fields_remove_env_keys() {
        let mut form = ProviderAddFormState::new(AppType::Claude);
        form.id.set("p1");
        form.name.set("Provider One");
        form.extra = json!({
            "settingsConfig": {
                "env": {
                    "ANTHROPIC_MODEL": "old-main",
                    "ANTHROPIC_REASONING_MODEL": "old-reasoning",
                    "ANTHROPIC_DEFAULT_HAIKU_MODEL": "old-haiku",
                    "ANTHROPIC_DEFAULT_SONNET_MODEL": "old-sonnet",
                    "ANTHROPIC_DEFAULT_OPUS_MODEL": "old-opus",
                    "ANTHROPIC_SMALL_FAST_MODEL": "old-small-fast",
                }
            }
        });
        form.mark_claude_model_config_touched();

        let provider = form.to_provider_json_value();
        let env = provider["settingsConfig"]["env"]
            .as_object()
            .expect("settingsConfig.env should be object");
        assert!(env.get("ANTHROPIC_MODEL").is_none());
        assert!(env.get("ANTHROPIC_REASONING_MODEL").is_none());
        assert!(env.get("ANTHROPIC_DEFAULT_HAIKU_MODEL").is_none());
        assert!(env.get("ANTHROPIC_DEFAULT_SONNET_MODEL").is_none());
        assert!(env.get("ANTHROPIC_DEFAULT_OPUS_MODEL").is_none());
        assert!(env.get("ANTHROPIC_SMALL_FAST_MODEL").is_none());
    }

    #[test]
    fn provider_add_form_claude_untouched_model_popup_keeps_model_keys() {
        let provider = Provider::with_id(
            "p1".to_string(),
            "Provider One".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "token-old",
                    "ANTHROPIC_BASE_URL": "https://claude.example",
                    "ANTHROPIC_MODEL": "model-main",
                    "ANTHROPIC_SMALL_FAST_MODEL": "model-small-fast",
                }
            }),
            None,
        );

        let mut form = ProviderAddFormState::from_provider(AppType::Claude, &provider);
        form.name.set("Provider One Updated");

        let out = form.to_provider_json_value();
        let env = out["settingsConfig"]["env"]
            .as_object()
            .expect("settingsConfig.env should be object");
        assert_eq!(
            env.get("ANTHROPIC_MODEL").and_then(|v| v.as_str()),
            Some("model-main")
        );
        assert_eq!(
            env.get("ANTHROPIC_SMALL_FAST_MODEL")
                .and_then(|v| v.as_str()),
            Some("model-small-fast")
        );
        assert_eq!(
            env.get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
                .and_then(|v| v.as_str()),
            None
        );
        assert_eq!(
            env.get("ANTHROPIC_DEFAULT_SONNET_MODEL")
                .and_then(|v| v.as_str()),
            None
        );
        assert_eq!(
            env.get("ANTHROPIC_DEFAULT_OPUS_MODEL")
                .and_then(|v| v.as_str()),
            None
        );
    }

    #[test]
    fn provider_add_form_codex_builds_full_toml_config() {
        let mut form = ProviderAddFormState::new(AppType::Codex);
        form.id.set("c1");
        form.name.set("Codex Provider");
        form.codex_base_url.set("https://api.openai.com/v1");
        form.codex_model.set("gpt-5.2-codex");
        form.codex_api_key.set("sk-test");

        let provider = form.to_provider_json_value();
        assert_eq!(
            provider["settingsConfig"]["auth"]["OPENAI_API_KEY"],
            "sk-test"
        );
        let cfg = provider["settingsConfig"]["config"]
            .as_str()
            .expect("settingsConfig.config should be string");
        assert!(cfg.contains("model_provider ="));
        assert!(cfg.contains("[model_providers."));
        assert!(cfg.contains("base_url = \"https://api.openai.com/v1\""));
        assert!(cfg.contains("model = \"gpt-5.2-codex\""));
        assert!(cfg.contains("wire_api = \"responses\""));
        assert!(cfg.contains("requires_openai_auth = true"));
        assert!(cfg.contains("disable_response_storage = true"));
    }

    #[test]
    fn provider_add_form_codex_preserves_existing_config_toml_custom_keys() {
        let provider = crate::provider::Provider::with_id(
            "c1".to_string(),
            "Codex Provider".to_string(),
            json!({
                "auth": {
                    "OPENAI_API_KEY": "sk-test"
                },
                "config": r#"
model_provider = "custom"
model = "gpt-5.2-codex"
network_access = true

[model_providers.custom]
name = "custom"
base_url = "https://api.example.com/v1"
wire_api = "responses"
requires_openai_auth = true
"#,
            }),
            None,
        );

        let mut form = ProviderAddFormState::from_provider(AppType::Codex, &provider);
        form.codex_base_url.set("https://changed.example/v1");

        let out = form.to_provider_json_value();
        let cfg = out["settingsConfig"]["config"]
            .as_str()
            .expect("settingsConfig.config should be string");
        assert!(
            cfg.contains("network_access = true"),
            "existing Codex config.toml keys should be preserved"
        );
        assert!(
            cfg.contains("base_url = \"https://changed.example/v1\""),
            "Codex base_url form field should still update config.toml"
        );
    }

    #[test]
    fn provider_add_form_codex_custom_includes_api_key_and_hides_advanced_fields() {
        let form = ProviderAddFormState::new(AppType::Codex);
        let fields = form.fields();

        assert!(
            fields.contains(&ProviderAddField::CodexApiKey),
            "custom Codex provider should include API Key field"
        );
        assert!(
            !fields.contains(&ProviderAddField::CodexWireApi),
            "Codex wire_api should not be configurable in the UI"
        );
        assert!(
            !fields.contains(&ProviderAddField::CodexRequiresOpenaiAuth),
            "Codex auth mode should not be configurable in the UI"
        );
        assert!(
            !fields.contains(&ProviderAddField::CodexEnvKey),
            "Codex env key should not be configurable in the UI"
        );
    }

    #[test]
    fn provider_add_form_codex_openai_official_sets_website_and_hides_api_key_field() {
        let mut form = ProviderAddFormState::new(AppType::Codex);
        let existing_ids = Vec::<String>::new();

        form.apply_template(1, &existing_ids);

        assert_eq!(form.website_url.value, "https://chatgpt.com/codex");
        let fields = form.fields();
        assert!(
            !fields.contains(&ProviderAddField::CodexApiKey),
            "official Codex provider should not require API Key input"
        );
    }

    #[test]
    fn provider_add_form_codex_packycode_hides_env_key_field() {
        let mut form = ProviderAddFormState::new(AppType::Codex);
        let existing_ids = Vec::<String>::new();

        let idx = packycode_template_index(AppType::Codex);
        form.apply_template(idx, &existing_ids);

        let fields = form.fields();
        assert!(
            fields.contains(&ProviderAddField::CodexApiKey),
            "PackyCode Codex provider should include API Key field"
        );
        assert!(
            !fields.contains(&ProviderAddField::CodexEnvKey),
            "Codex env key should not be configurable for PackyCode"
        );
    }

    #[test]
    fn provider_add_form_gemini_builds_env_settings() {
        let mut form = ProviderAddFormState::new(AppType::Gemini);
        form.id.set("g1");
        form.name.set("Gemini Provider");
        form.gemini_auth_type = GeminiAuthType::ApiKey;
        form.gemini_api_key.set("AIza...");
        form.gemini_base_url
            .set("https://generativelanguage.googleapis.com");

        let provider = form.to_provider_json_value();
        assert_eq!(
            provider["settingsConfig"]["env"]["GEMINI_API_KEY"],
            "AIza..."
        );
        assert_eq!(
            provider["settingsConfig"]["env"]["GOOGLE_GEMINI_BASE_URL"],
            "https://generativelanguage.googleapis.com"
        );
    }

    #[test]
    fn provider_add_form_gemini_includes_model_in_env_when_set() {
        let mut form = ProviderAddFormState::new(AppType::Gemini);
        form.id.set("g1");
        form.name.set("Gemini Provider");
        form.gemini_auth_type = GeminiAuthType::ApiKey;
        form.gemini_api_key.set("AIza...");
        form.gemini_base_url
            .set("https://generativelanguage.googleapis.com");
        form.gemini_model.set("gemini-3-pro-preview");

        let provider = form.to_provider_json_value();
        assert_eq!(
            provider["settingsConfig"]["env"]["GEMINI_MODEL"],
            "gemini-3-pro-preview"
        );
    }

    #[test]
    fn provider_add_form_gemini_oauth_does_not_include_model_or_api_key_env() {
        let mut form = ProviderAddFormState::new(AppType::Gemini);
        form.id.set("g1");
        form.name.set("Gemini Provider");
        form.gemini_auth_type = GeminiAuthType::OAuth;
        form.gemini_model.set("gemini-3-pro-preview");

        let provider = form.to_provider_json_value();
        let env = provider["settingsConfig"]["env"]
            .as_object()
            .expect("settingsConfig.env should be an object");
        assert!(env.get("GEMINI_API_KEY").is_none());
        assert!(env.get("GOOGLE_GEMINI_BASE_URL").is_none());
        assert!(env.get("GEMINI_BASE_URL").is_none());
        assert!(env.get("GEMINI_MODEL").is_none());
    }

    #[test]
    fn mcp_add_form_builds_server_and_apps() {
        let mut form = McpAddFormState::new();
        form.id.set("m1");
        form.name.set("Server One");
        form.command.set("npx");
        form.args
            .set("-y @modelcontextprotocol/server-filesystem /tmp");
        form.apps.claude = true;
        form.apps.codex = false;
        form.apps.gemini = true;

        let server = form.to_mcp_server_json_value();
        assert_eq!(server["id"], "m1");
        assert_eq!(server["name"], "Server One");
        assert_eq!(server["server"]["command"], "npx");
        assert_eq!(server["server"]["args"][0], "-y");
        assert_eq!(server["apps"]["claude"], true);
        assert_eq!(server["apps"]["codex"], false);
        assert_eq!(server["apps"]["gemini"], true);
    }

    #[test]
    fn provider_add_form_switching_back_to_custom_clears_template_values() {
        let mut form = ProviderAddFormState::new(AppType::Claude);
        let existing_ids = Vec::<String>::new();

        form.apply_template(1, &existing_ids);
        assert_eq!(form.name.value, "Claude Official");
        assert_eq!(form.website_url.value, "https://anthropic.com");
        assert_eq!(form.claude_base_url.value, "https://api.anthropic.com");
        assert_eq!(form.id.value, "claude-official");

        form.apply_template(0, &existing_ids);
        assert_eq!(form.name.value, "");
        assert_eq!(form.website_url.value, "");
        assert_eq!(form.claude_base_url.value, "");
        assert_eq!(form.id.value, "");
    }

    #[test]
    fn mcp_add_form_switching_back_to_custom_clears_template_values() {
        let mut form = McpAddFormState::new();
        form.id.set("m1");

        form.apply_template(1);
        assert_eq!(form.name.value, "Filesystem");
        assert_eq!(form.command.value, "npx");
        assert!(form
            .args
            .value
            .contains("@modelcontextprotocol/server-filesystem"));

        form.apply_template(0);
        assert_eq!(form.id.value, "m1");
        assert_eq!(form.name.value, "");
        assert_eq!(form.command.value, "");
        assert_eq!(form.args.value, "");
    }

    #[test]
    fn provider_add_form_common_config_json_merges_into_settings_for_preview_and_submit() {
        let mut form = ProviderAddFormState::new(AppType::Claude);
        form.id.set("p1");
        form.name.set("Provider One");
        form.include_common_config = true;
        form.claude_base_url.set("https://provider.example");
        form.claude_api_key.set("sk-provider");

        let merged = form
            .to_provider_json_value_with_common_config(
                r#"{
                    "alwaysThinkingEnabled": false,
                    "env": {
                        "ANTHROPIC_BASE_URL": "https://common.example",
                        "COMMON_FLAG": "1"
                    }
                }"#,
            )
            .expect("common config should merge");
        let settings = merged
            .get("settingsConfig")
            .expect("settingsConfig should exist");

        assert_eq!(settings["alwaysThinkingEnabled"], false);
        assert_eq!(settings["env"]["COMMON_FLAG"], "1");
        assert_eq!(
            settings["env"]["ANTHROPIC_BASE_URL"], "https://provider.example",
            "provider field should override common snippet value"
        );
        assert_eq!(settings["env"]["ANTHROPIC_API_KEY"], "sk-provider");
    }

    #[test]
    fn provider_add_form_apply_provider_json_updates_fields_and_preserves_include_toggle() {
        let mut form = ProviderAddFormState::new(AppType::Claude);
        form.include_common_config = false;
        form.extra = json!({
            "category": "custom"
        });

        let parsed = Provider::with_id(
            "json-id".to_string(),
            "JSON Provider".to_string(),
            json!({
                "alwaysThinkingEnabled": false,
                "env": {
                    "ANTHROPIC_BASE_URL": "https://json.example"
                }
            }),
            Some("https://site.example".to_string()),
        );

        form.apply_provider_json_to_fields(&parsed);

        assert_eq!(form.id.value, "json-id");
        assert_eq!(form.name.value, "JSON Provider");
        assert_eq!(form.website_url.value, "https://site.example");
        assert_eq!(form.claude_base_url.value, "https://json.example");
        assert!(
            !form.include_common_config,
            "include_common_config should be preserved when editor JSON omits meta.applyCommonConfig"
        );
        assert_eq!(form.extra["category"], "custom");
        assert_eq!(form.extra["settingsConfig"]["alwaysThinkingEnabled"], false);
    }

    #[test]
    fn provider_edit_form_apply_provider_json_keeps_locked_id() {
        let original = Provider::with_id(
            "locked-id".to_string(),
            "Original".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://before.example"
                }
            }),
            None,
        );
        let mut form = ProviderAddFormState::from_provider(AppType::Claude, &original);

        let edited = Provider::with_id(
            "changed-id".to_string(),
            "Edited Name".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://after.example"
                }
            }),
            None,
        );

        form.apply_provider_json_to_fields(&edited);

        assert_eq!(form.id.value, "locked-id");
        assert_eq!(form.name.value, "Edited Name");
        assert_eq!(form.claude_base_url.value, "https://after.example");
    }

    #[test]
    fn provider_add_form_disabling_common_config_strips_common_fields_from_json() {
        let mut form = ProviderAddFormState::new(AppType::Claude);
        form.id.set("p1");
        form.name.set("Provider One");
        form.include_common_config = true;

        let parsed = Provider::with_id(
            "p1".to_string(),
            "Provider One".to_string(),
            json!({
                "alwaysThinkingEnabled": false,
                "statusLine": {
                    "type": "command",
                    "command": "~/.claude/statusline.sh",
                    "padding": 0
                },
                "env": {
                    "ANTHROPIC_BASE_URL": "https://provider.example"
                }
            }),
            None,
        );
        form.apply_provider_json_to_fields(&parsed);

        let common = r#"{
            "alwaysThinkingEnabled": false,
            "statusLine": {
                "type": "command",
                "command": "~/.claude/statusline.sh",
                "padding": 0
            }
        }"#;
        form.toggle_include_common_config(common)
            .expect("toggle should succeed");

        assert!(
            !form.include_common_config,
            "toggle should disable include_common_config"
        );
        let provider = form.to_provider_json_value();
        let settings = provider
            .get("settingsConfig")
            .expect("settingsConfig should exist");
        assert!(
            settings.get("alwaysThinkingEnabled").is_none(),
            "common scalar field should be removed after disabling common config"
        );
        assert!(
            settings.get("statusLine").is_none(),
            "common nested field should be removed after disabling common config"
        );
    }

    #[test]
    fn provider_add_form_disabling_common_config_preserves_provider_specific_env_keys() {
        let mut form = ProviderAddFormState::new(AppType::Claude);
        form.id.set("p1");
        form.name.set("Provider One");
        form.include_common_config = true;

        let parsed = Provider::with_id(
            "p1".to_string(),
            "Provider One".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://common.example",
                    "ANTHROPIC_AUTH_TOKEN": "sk-provider"
                }
            }),
            None,
        );
        form.apply_provider_json_to_fields(&parsed);

        form.toggle_include_common_config(
            r#"{"env":{"ANTHROPIC_BASE_URL":"https://common.example"}}"#,
        )
        .expect("toggle should succeed");

        let provider = form.to_provider_json_value();
        let env = provider
            .get("settingsConfig")
            .and_then(|settings| settings.get("env"))
            .and_then(|value| value.as_object())
            .expect("env should exist");

        assert!(
            env.get("ANTHROPIC_BASE_URL").is_none(),
            "common env keys should be removed"
        );
        assert_eq!(
            env.get("ANTHROPIC_API_KEY")
                .and_then(|value| value.as_str()),
            Some("sk-provider"),
            "provider-specific env keys should be preserved"
        );
    }
}
