use crate::app_config::{AppType, McpApps};
use serde_json::Value;

use super::app::EditorState;

mod codex_config;
mod mcp;
mod prompt;
mod provider_json;
mod provider_state;
mod provider_state_loading;
mod provider_templates;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use provider_json::strip_provider_internal_fields;

pub(crate) use super::text_edit::TextInput;
pub(crate) use codex_config::parse_codex_config_snippet;
pub(crate) use provider_json::claude_hide_attribution_enabled;
pub(crate) use provider_json::strip_common_config_from_settings;
pub(crate) use provider_json::{normalize_usage_interval, normalize_usage_timeout};
pub(crate) use provider_state::resolve_provider_id_for_submit;
pub(crate) use provider_state::{
    detect_balance_provider_for_usage_query, detect_coding_plan_provider_for_usage_query,
};

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
    Content,
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
    ClaudeHideAttribution,
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
    UsageQueryDivider,
    UsageQuery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderFormPage {
    Main,
    UsageQuery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageQueryTemplate {
    Custom,
    General,
    NewApi,
    GitHubCopilot,
    TokenPlan,
    Balance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageQueryField {
    Enabled,
    Template,
    ApiKey,
    BaseUrl,
    AccessToken,
    UserId,
    Timeout,
    AutoInterval,
    CodingPlanProvider,
    Script,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpAddField {
    Id,
    Name,
    Type,
    Command,
    Args,
    Url,
    Env,
    AppClaude,
    AppCodex,
    AppGemini,
    AppOpenCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptMetaField {
    Id,
    Name,
    Description,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTransport {
    Stdio,
    Http,
    Sse,
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
    pub page: ProviderFormPage,
    pub template_idx: usize,
    pub field_idx: usize,
    pub editing: bool,
    pub usage_query_touched: bool,
    pub usage_query_field_idx: usize,
    pub usage_query_editing: bool,
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
    pub claude_hide_attribution: bool,
    claude_hide_attribution_touched: bool,

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
    pub usage_query_enabled: bool,
    pub usage_query_template: UsageQueryTemplate,
    pub usage_query_api_key: TextInput,
    pub usage_query_base_url: TextInput,
    pub usage_query_access_token: TextInput,
    pub usage_query_user_id: TextInput,
    pub usage_query_timeout: TextInput,
    pub usage_query_auto_interval: TextInput,
    pub usage_query_code: String,
    pub usage_query_coding_plan_provider: TextInput,
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
    pub server_type: McpTransport,
    pub command: TextInput,
    pub args: TextInput,
    pub url: TextInput,
    pub env_rows: Vec<McpEnvVarRow>,
    pub apps: McpApps,
    pub json_scroll: usize,
    initial_snapshot: Value,
}

#[derive(Debug, Clone)]
pub struct PromptMetaFormState {
    pub mode: FormMode,
    pub focus: FormFocus,
    pub field_idx: usize,
    pub editing: bool,
    pub id: TextInput,
    pub name: TextInput,
    pub description: TextInput,
    pub content: EditorState,
    initial_snapshot: (String, String, String, String),
}

// This controls whether the main UI should consider itself in "editing mode" and e.g. respond to vim-style navigation.
impl ProviderAddFormState {
    pub fn is_editing(&self) -> bool {
        self.editing || self.usage_query_editing
    }
}

impl McpAddFormState {
    pub fn is_editing(&self) -> bool {
        self.editing
    }
}

impl PromptMetaFormState {
    pub fn is_editing(&self) -> bool {
        self.editing || matches!(self.focus, FormFocus::Content)
    }
}

#[derive(Debug, Clone)]
pub enum FormState {
    ProviderAdd(ProviderAddFormState),
    McpAdd(McpAddFormState),
    PromptMeta(PromptMetaFormState),
}

impl FormState {
    pub fn has_unsaved_changes(&self) -> bool {
        match self {
            FormState::ProviderAdd(form) => form.has_unsaved_changes(),
            FormState::McpAdd(form) => form.has_unsaved_changes(),
            FormState::PromptMeta(form) => form.has_unsaved_changes(),
        }
    }

    pub fn is_editing(&self) -> bool {
        match self {
            FormState::ProviderAdd(form) => form.is_editing(),
            FormState::McpAdd(form) => form.is_editing(),
            FormState::PromptMeta(form) => form.is_editing(),
        }
    }
}
