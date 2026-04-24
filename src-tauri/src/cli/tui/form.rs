use crate::app_config::{AppType, McpApps};
use serde_json::Value;

mod codex_config;
mod mcp;
mod provider_json;
mod provider_state;
mod provider_state_loading;
mod provider_templates;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use provider_json::strip_provider_internal_fields;

pub(crate) use codex_config::parse_codex_config_snippet;
pub(crate) use provider_json::strip_common_config_from_settings;
pub(crate) use provider_state::resolve_provider_id_for_submit;

pub const OPENCLAW_DEFAULT_API_PROTOCOL: &str = "openai-completions";
pub const OPENCLAW_DEFAULT_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:148.0) Gecko/20100101 Firefox/148.0";
pub const OPENCLAW_API_PROTOCOLS: [&str; 5] = [
    "openai-completions",
    "openai-responses",
    "anthropic-messages",
    "google-generative-ai",
    "bedrock-converse-stream",
];

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
pub enum ClaudeApiFormat {
    Anthropic,
    OpenAiChat,
    OpenAiResponses,
}

impl ClaudeApiFormat {
    pub const ALL: [Self; 3] = [
        ClaudeApiFormat::Anthropic,
        ClaudeApiFormat::OpenAiChat,
        ClaudeApiFormat::OpenAiResponses,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ClaudeApiFormat::Anthropic => "anthropic",
            ClaudeApiFormat::OpenAiChat => "openai_chat",
            ClaudeApiFormat::OpenAiResponses => "openai_responses",
        }
    }

    pub fn from_raw(value: &str) -> Self {
        match value {
            "openai_chat" => ClaudeApiFormat::OpenAiChat,
            "openai_responses" => ClaudeApiFormat::OpenAiResponses,
            _ => ClaudeApiFormat::Anthropic,
        }
    }

    pub fn picker_index(self) -> usize {
        match self {
            ClaudeApiFormat::Anthropic => 0,
            ClaudeApiFormat::OpenAiChat => 1,
            ClaudeApiFormat::OpenAiResponses => 2,
        }
    }

    pub fn from_picker_index(index: usize) -> Self {
        Self::ALL
            .get(index)
            .copied()
            .unwrap_or(ClaudeApiFormat::Anthropic)
    }

    pub fn requires_proxy(self) -> bool {
        !matches!(self, ClaudeApiFormat::Anthropic)
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
    ClaudeApiFormat,
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
    OpenClawApiProtocol,
    OpenClawUserAgent,
    OpenClawModels,
    OpenCodeNpmPackage,
    OpenCodeApiKey,
    OpenCodeBaseUrl,
    OpenCodeModelId,
    OpenCodeModelName,
    OpenCodeModelContextLimit,
    OpenCodeModelOutputLimit,
    CommonConfigDivider,
    CommonSnippet,
    IncludeCommonConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpAddField {
    Id,
    Name,
    Command,
    Args,
    Env,
    AppClaude,
    AppCodex,
    AppGemini,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpEnvVarRow {
    pub key: String,
    pub value: String,
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
    include_common_config_touched: bool,
    pub json_scroll: usize,
    pub codex_preview_section: CodexPreviewSection,
    pub codex_auth_scroll: usize,
    pub codex_config_scroll: usize,
    claude_model_config_touched: bool,

    pub claude_api_key: TextInput,
    pub claude_base_url: TextInput,
    pub claude_api_format: ClaudeApiFormat,
    pub claude_model: TextInput,
    pub claude_reasoning_model: TextInput,
    pub claude_haiku_model: TextInput,
    pub claude_sonnet_model: TextInput,
    pub claude_opus_model: TextInput,

    pub codex_base_url: TextInput,
    pub codex_model: TextInput,
    pub codex_wire_api: CodexWireApi,
    pub codex_requires_openai_auth: bool,
    pub codex_env_key: TextInput,
    pub codex_api_key: TextInput,

    pub gemini_auth_type: GeminiAuthType,
    pub gemini_api_key: TextInput,
    pub gemini_base_url: TextInput,
    pub gemini_model: TextInput,

    pub openclaw_user_agent: bool,
    pub openclaw_models: Vec<Value>,
    pub opencode_npm_package: TextInput,
    pub opencode_api_key: TextInput,
    pub opencode_base_url: TextInput,
    pub opencode_model_id: TextInput,
    pub opencode_model_name: TextInput,
    pub opencode_model_context_limit: TextInput,
    pub opencode_model_output_limit: TextInput,
    opencode_model_original_id: Option<String>,
    initial_snapshot: Value,
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
    pub env_rows: Vec<McpEnvVarRow>,
    pub apps: McpApps,
    pub json_scroll: usize,
    initial_snapshot: Value,
}

#[derive(Debug, Clone)]
pub enum FormState {
    ProviderAdd(ProviderAddFormState),
    McpAdd(McpAddFormState),
}

impl FormState {
    pub fn has_unsaved_changes(&self) -> bool {
        match self {
            FormState::ProviderAdd(form) => form.has_unsaved_changes(),
            FormState::McpAdd(form) => form.has_unsaved_changes(),
        }
    }
}
