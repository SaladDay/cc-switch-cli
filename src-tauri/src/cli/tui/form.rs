use crate::app_config::{AppType, McpApps};
use crate::provider::{ClaudeApiKeyField, CodexChatReasoningConfig};
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

pub(crate) use crate::hermes_config::{HERMES_API_MODES, HERMES_DEFAULT_API_MODE};
pub(crate) use crate::openclaw_config::{
    OPENCLAW_API_PROTOCOLS, OPENCLAW_DEFAULT_API_PROTOCOL, OPENCLAW_DEFAULT_USER_AGENT,
};

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
    GeminiNative,
}

impl ClaudeApiFormat {
    pub const ALL: [Self; 4] = [
        ClaudeApiFormat::Anthropic,
        ClaudeApiFormat::OpenAiChat,
        ClaudeApiFormat::OpenAiResponses,
        ClaudeApiFormat::GeminiNative,
    ];
    pub const CODEX: [Self; 2] = [
        ClaudeApiFormat::OpenAiResponses,
        ClaudeApiFormat::OpenAiChat,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ClaudeApiFormat::Anthropic => "anthropic",
            ClaudeApiFormat::OpenAiChat => "openai_chat",
            ClaudeApiFormat::OpenAiResponses => "openai_responses",
            ClaudeApiFormat::GeminiNative => "gemini_native",
        }
    }

    pub fn from_raw(value: &str) -> Self {
        match value {
            "openai_chat" => ClaudeApiFormat::OpenAiChat,
            "openai_responses" => ClaudeApiFormat::OpenAiResponses,
            "gemini_native" => ClaudeApiFormat::GeminiNative,
            _ => ClaudeApiFormat::Anthropic,
        }
    }

    pub fn choices_for_app(app_type: &AppType) -> &'static [Self] {
        match app_type {
            AppType::Codex => &Self::CODEX,
            _ => &Self::ALL,
        }
    }

    pub fn picker_index_for_app(self, app_type: &AppType) -> usize {
        Self::choices_for_app(app_type)
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(0)
    }

    pub fn from_picker_index_for_app(index: usize, app_type: &AppType) -> Self {
        Self::choices_for_app(app_type)
            .get(index)
            .copied()
            .unwrap_or_else(|| {
                if matches!(app_type, AppType::Codex) {
                    ClaudeApiFormat::OpenAiResponses
                } else {
                    ClaudeApiFormat::Anthropic
                }
            })
    }

    pub fn requires_proxy_for_app(self, app_type: &AppType) -> bool {
        match app_type {
            AppType::Codex => matches!(self, ClaudeApiFormat::OpenAiChat),
            _ => self.requires_proxy(),
        }
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
    #[allow(dead_code)]
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
    CodexOAuthAccount,
    CodexFastMode,
    CodexBaseUrl,
    CodexModel,
    CodexLocalRouting,
    #[allow(dead_code)]
    CodexWireApi,
    #[allow(dead_code)]
    CodexRequiresOpenaiAuth,
    #[allow(dead_code)]
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
    HermesApiMode,
    HermesApiKey,
    HermesBaseUrl,
    HermesModels,
    HermesAdvancedDivider,
    HermesRateLimitDelay,
    CommonConfigDivider,
    CommonSnippet,
    IncludeCommonConfig,
    UsageQueryDivider,
    UsageQuery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderFormPage {
    Main,
    CodexLocalRouting,
    CodexModelCatalog,
    UsageQuery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HermesModelField {
    Id(usize),
    Name(usize),
    ContextLength(usize),
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
pub enum CodexLocalRoutingField {
    Enabled,
    SupportsThinking,
    SupportsEffort,
    ModelCatalog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexModelCatalogField {
    Model,
    DisplayName,
    ContextWindow,
}

impl CodexModelCatalogField {
    pub const ALL: [Self; 3] = [Self::Model, Self::DisplayName, Self::ContextWindow];

    pub fn index(self) -> usize {
        match self {
            Self::Model => 0,
            Self::DisplayName => 1,
            Self::ContextWindow => 2,
        }
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or(Self::ContextWindow)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexModelCatalogRow {
    pub model: String,
    pub display_name: String,
    pub context_window: String,
}

impl CodexModelCatalogRow {
    pub(crate) fn summary_label(&self) -> String {
        let model = self.model.trim();
        let display_name = self.display_name.trim();
        let mut label = if display_name.is_empty() || display_name == model {
            model.to_string()
        } else {
            format!("{display_name} ({model})")
        };

        let context_window = codex_model_catalog_context_window_label(&self.context_window);
        if !context_window.is_empty() {
            label.push_str(&format!(" [{context_window}]"));
        }
        label
    }
}

pub(crate) fn parse_codex_model_catalog_context_window(raw: &str) -> Option<u64> {
    let compact = raw
        .trim()
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != ',' && *ch != '_')
        .collect::<String>()
        .to_ascii_lowercase();
    if compact.is_empty() {
        return None;
    }

    let number_len = compact
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .map(char::len_utf8)
        .sum::<usize>();
    if number_len == 0 {
        return None;
    }

    let (number, suffix) = compact.split_at(number_len);
    let value = number.parse::<f64>().ok()?;
    if !value.is_finite() || value <= 0.0 {
        return None;
    }

    let multiplier = if suffix.starts_with('k') {
        1_000.0
    } else if suffix.starts_with('m') {
        1_000_000.0
    } else {
        1.0
    };
    let normalized = (value * multiplier).round();
    (normalized > 0.0 && normalized <= u64::MAX as f64).then_some(normalized as u64)
}

pub(crate) fn codex_model_catalog_context_window_label(raw: &str) -> String {
    if let Some(value) = parse_codex_model_catalog_context_window(raw) {
        if value >= 1_000_000 && value % 1_000_000 == 0 {
            format!("{}m", value / 1_000_000)
        } else if value >= 1_000 && value % 1_000 == 0 {
            format!("{}k", value / 1_000)
        } else {
            value.to_string()
        }
    } else {
        raw.trim().to_string()
    }
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
    AppHermes,
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
    pub copy_source_id: Option<String>,
    pub focus: FormFocus,
    pub page: ProviderFormPage,
    pub template_idx: usize,
    pub field_idx: usize,
    pub editing: bool,
    pub usage_query_touched: bool,
    pub usage_query_field_idx: usize,
    pub usage_query_editing: bool,
    pub codex_local_routing_field_idx: usize,
    pub codex_model_catalog_idx: usize,
    pub codex_model_catalog_field: CodexModelCatalogField,
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
    pub claude_api_key_field: ClaudeApiKeyField,
    pub claude_base_url: TextInput,
    pub claude_api_format: ClaudeApiFormat,
    pub claude_model: TextInput,
    pub claude_reasoning_model: TextInput,
    pub claude_haiku_model: TextInput,
    pub claude_sonnet_model: TextInput,
    pub claude_opus_model: TextInput,
    pub claude_hide_attribution: bool,
    claude_hide_attribution_touched: bool,
    pub codex_oauth_account_id: Option<String>,
    pub codex_fast_mode: bool,

    pub codex_base_url: TextInput,
    pub codex_model: TextInput,
    pub codex_wire_api: CodexWireApi,
    pub codex_requires_openai_auth: bool,
    pub codex_env_key: TextInput,
    pub codex_api_key: TextInput,
    pub codex_chat_reasoning: CodexChatReasoningConfig,
    pub codex_model_catalog: Vec<CodexModelCatalogRow>,

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
    pub hermes_api_mode: String,
    pub hermes_api_key: TextInput,
    pub hermes_base_url: TextInput,
    pub hermes_models: Vec<Value>,
    pub hermes_models_field_idx: usize,
    pub hermes_models_editing: bool,
    pub hermes_model_input: TextInput,
    pub hermes_rate_limit_delay: TextInput,
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
