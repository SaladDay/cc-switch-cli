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
    IncludeCommonConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderTemplateId {
    Custom,
    ClaudeOfficial,
    OpenAiOfficial,
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
    chip_label: "★ PackyCode",
    website_url: "https://www.packyapi.com",
    register_url: "https://www.packyapi.com/register?aff=cc-switch-cli",
    promo_code: "cc-switch-cli",
    partner_promotion_key: "packycode",
    claude_base_url: "https://www.packyapi.com",
    codex_base_url: "https://www.packyapi.com/v1",
    gemini_base_url: "https://www.packyapi.com",
}];

const PROVIDER_TEMPLATE_DEFS_CLAUDE: [ProviderTemplateDef; 2] = [
    ProviderTemplateDef {
        id: ProviderTemplateId::Custom,
        label: "Custom",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::ClaudeOfficial,
        label: "Claude Official",
    },
];

const PROVIDER_TEMPLATE_DEFS_CODEX: [ProviderTemplateDef; 2] = [
    ProviderTemplateDef {
        id: ProviderTemplateId::Custom,
        label: "Custom",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::OpenAiOfficial,
        label: "OpenAI Official",
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

    // Claude
    pub claude_api_key: TextInput,
    pub claude_base_url: TextInput,

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

            claude_api_key: TextInput::new(""),
            claude_base_url: TextInput::new(""),

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
                    if let Some(token) = env.get("ANTHROPIC_AUTH_TOKEN").and_then(|v| v.as_str()) {
                        form.claude_api_key.set(token);
                    }
                    if let Some(url) = env.get("ANTHROPIC_BASE_URL").and_then(|v| v.as_str()) {
                        form.claude_base_url.set(url);
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
            ProviderAddField::Id,
            ProviderAddField::Name,
            ProviderAddField::WebsiteUrl,
            ProviderAddField::Notes,
        ];

        match self.app_type {
            AppType::Claude => {
                fields.push(ProviderAddField::ClaudeBaseUrl);
                fields.push(ProviderAddField::ClaudeApiKey);
            }
            AppType::Codex => {
                fields.push(ProviderAddField::CodexBaseUrl);
                fields.push(ProviderAddField::CodexModel);
                fields.push(ProviderAddField::CodexWireApi);
                fields.push(ProviderAddField::CodexRequiresOpenaiAuth);
                if !self.codex_requires_openai_auth {
                    fields.push(ProviderAddField::CodexEnvKey);
                }
                fields.push(ProviderAddField::CodexApiKey);
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
            | ProviderAddField::GeminiAuthType
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
            | ProviderAddField::GeminiAuthType
            | ProviderAddField::IncludeCommonConfig => None,
        }
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
                    self.claude_api_key = defaults.claude_api_key;
                    self.claude_base_url = defaults.claude_base_url;
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
                    self.name.set("OpenAI Official");
                    self.website_url.set("https://openai.com");
                    self.codex_base_url.set("https://api.openai.com/v1");
                    self.codex_model.set("gpt-5.2-codex");
                    self.codex_wire_api = CodexWireApi::Responses;
                    self.codex_requires_openai_auth = true;
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
                self.codex_requires_openai_auth = false;
                self.codex_env_key.set("OPENAI_API_KEY");
            }
            AppType::Gemini => {
                self.gemini_auth_type = GeminiAuthType::ApiKey;
                self.gemini_base_url.set(preset.gemini_base_url);
            }
        }
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
                set_or_remove_trimmed(env_obj, "ANTHROPIC_AUTH_TOKEN", &self.claude_api_key.value);
                set_or_remove_trimmed(env_obj, "ANTHROPIC_BASE_URL", &self.claude_base_url.value);
            }
            AppType::Codex => {
                let original = settings_obj
                    .get("config")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let updated = update_codex_config_snippet(
                    original,
                    self.codex_base_url.value.trim(),
                    self.codex_model.value.trim(),
                    self.codex_wire_api,
                    self.codex_requires_openai_auth,
                    self.codex_env_key.value.trim(),
                );
                settings_obj.insert("config".to_string(), Value::String(updated));

                if self.codex_api_key.is_blank() {
                    if let Some(auth) = settings_obj.get_mut("auth") {
                        if let Some(obj) = auth.as_object_mut() {
                            obj.remove("OPENAI_API_KEY");
                            if obj.is_empty() {
                                settings_obj.remove("auth");
                            }
                        } else {
                            settings_obj.remove("auth");
                        }
                    }
                } else {
                    let auth = settings_obj
                        .entry("auth".to_string())
                        .or_insert_with(|| json!({}));
                    if !auth.is_object() {
                        *auth = json!({});
                    }
                    let obj = auth.as_object_mut().expect("auth must be a JSON object");
                    obj.insert(
                        "OPENAI_API_KEY".to_string(),
                        json!(self.codex_api_key.value.trim()),
                    );
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

    lines
        .into_iter()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
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
        assert!(cfg.contains("base_url = \"https://www.packyapi.com/v1\""));
        assert!(cfg.contains("requires_openai_auth = false"));
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
            provider["settingsConfig"]["env"]["ANTHROPIC_AUTH_TOKEN"],
            "token"
        );
        assert_eq!(
            provider["settingsConfig"]["env"]["ANTHROPIC_BASE_URL"],
            "https://claude.example"
        );
    }

    #[test]
    fn provider_add_form_codex_builds_toml_snippet() {
        let mut form = ProviderAddFormState::new(AppType::Codex);
        form.id.set("c1");
        form.name.set("Codex Provider");
        form.codex_base_url.set("https://api.openai.com/v1");
        form.codex_model.set("gpt-5.2-codex");
        form.codex_wire_api = CodexWireApi::Responses;
        form.codex_requires_openai_auth = true;

        let provider = form.to_provider_json_value();
        let cfg = provider["settingsConfig"]["config"]
            .as_str()
            .expect("settingsConfig.config should be string");
        assert!(cfg.contains("base_url = \"https://api.openai.com/v1\""));
        assert!(cfg.contains("model = \"gpt-5.2-codex\""));
        assert!(cfg.contains("wire_api = \"responses\""));
        assert!(cfg.contains("requires_openai_auth = true"));
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
}
